use std::{
    borrow::Cow,
    env,
    error::Error,
    fmt,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};

use opentelemetry::{
    Context, KeyValue, global,
    logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity},
    metrics::{Counter, Histogram, MeterProvider},
    trace::{TraceContextExt, Tracer},
};
use opentelemetry_otlp::{ExporterBuildError, LogExporter, MetricExporter, SpanExporter};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::SdkMeterProvider,
    resource::{EnvResourceDetector, TelemetryResourceDetector},
    trace::SdkTracerProvider,
};

mod stderr;

static ENABLED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static OPERATION_METRICS: OnceLock<OperationMetrics> = OnceLock::new();
static LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();

struct OperationMetrics {
    count: Counter<u64>,
    duration: Histogram<f64>,
    retry_count: Counter<u64>,
}
struct ErrorTypeState(OnceLock<&'static str>);

/// Process-level telemetry handle. Dropping it flushes and shuts down its providers.
#[must_use = "keep the telemetry handle alive until application work is complete"]
pub struct Telemetry {
    providers: Option<(SdkTracerProvider, SdkMeterProvider, SdkLoggerProvider)>,
}

impl Telemetry {
    /// Flushes and shuts down the configured providers. Export errors are ignored.
    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        let Some((tracer_provider, meter_provider, logger_provider)) = self.providers.take() else {
            return;
        };

        ENABLED.store(false, Ordering::Release);
        let _ = tracer_provider.shutdown();
        let _ = meter_provider.shutdown();
        let _ = logger_provider.shutdown();
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

/// Initializes process-global trace, metric, and log providers when explicitly enabled.
///
/// Call once before creating [`Operation`]s, and keep the returned handle alive until
/// all instrumented work is complete. With observability disabled this returns a no-op
/// handle without reading exporter configuration or creating network clients.
///
/// # Errors
///
/// Returns an error if telemetry is already initialized or an enabled exporter fails to initialize.
pub fn init() -> Result<Telemetry, InitError> {
    let cruise_enabled = env::var("CRUISE_OBSERVABILITY_ENABLED").ok();
    let sdk_disabled = env::var("OTEL_SDK_DISABLED").ok();
    if !is_enabled(cruise_enabled.as_deref(), sdk_disabled.as_deref()) {
        return Ok(Telemetry { providers: None });
    }

    if INITIALIZED.load(Ordering::Acquire) {
        return Err(InitError(InitErrorKind::AlreadyInitialized));
    }

    let span_exporter = SpanExporter::builder()
        .with_http()
        .build()
        .map_err(|error| InitError::exporter("traces", &error))?;
    let metric_exporter = MetricExporter::builder()
        .with_http()
        .build()
        .map_err(|error| InitError::exporter("metrics", &error))?;
    let log_exporter = LogExporter::builder()
        .with_http()
        .build()
        .map_err(|error| InitError::exporter("logs", &error))?;

    if INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(InitError(InitErrorKind::AlreadyInitialized));
    }

    let resource = resource();
    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(metric_exporter)
        .with_resource(resource.clone())
        .build();
    let logger_provider = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource)
        .build();
    let meter = meter_provider.meter("cruise");
    let metrics = OperationMetrics {
        count: meter.u64_counter("cruise.operation.count").build(),
        duration: meter
            .f64_histogram("cruise.operation.duration")
            .with_unit("s")
            .build(),
        retry_count: meter.u64_counter("cruise.retry.count").build(),
    };

    if OPERATION_METRICS.set(metrics).is_err() {
        let _ = tracer_provider.shutdown();
        let _ = meter_provider.shutdown();
        let _ = logger_provider.shutdown();
        INITIALIZED.store(false, Ordering::Release);
        return Err(InitError(InitErrorKind::AlreadyInitialized));
    }
    if LOGGER_PROVIDER.set(logger_provider.clone()).is_err() {
        let _ = tracer_provider.shutdown();
        let _ = meter_provider.shutdown();
        let _ = logger_provider.shutdown();
        return Err(InitError(InitErrorKind::AlreadyInitialized));
    }

    global::set_tracer_provider(tracer_provider.clone());
    global::set_meter_provider(meter_provider.clone());
    ENABLED.store(true, Ordering::Release);

    Ok(Telemetry {
        providers: Some((tracer_provider, meter_provider, logger_provider)),
    })
}

/// An instrumented operation represented by a span and duration metric.
#[must_use = "operations must be finished"]
pub struct Operation {
    kind: &'static str,
    context: Option<Context>,
    started_at: Option<Instant>,
    finished: bool,
}

impl Operation {
    /// Starts a root execution span with its safe session and workflow identity.
    pub fn execution(
        kind: &'static str,
        session_id: &str,
        config: Option<&crate::session_config::SessionConfigRef>,
    ) -> Self {
        let operation = Self::start(
            "execution",
            &[
                ("cruise.execution.kind", kind),
                ("cruise.session.id", session_id),
            ],
        );
        if let Some(config) = config {
            operation.set_workflow(config);
        }
        operation
    }

    /// Updates this execution span with the prepared workflow's safe identity.
    pub fn set_workflow(&self, config: &crate::session_config::SessionConfigRef) {
        if !ENABLED.load(Ordering::Acquire) {
            return;
        }
        let Some(context) = self.context.as_ref() else {
            return;
        };
        let (kind, name) = workflow_identity(config);
        let span = context.span();
        span.set_attribute(KeyValue::new("cruise.workflow.kind", kind));
        span.set_attribute(KeyValue::new("cruise.workflow.name", name.into_owned()));
    }

    /// Starts a phase span with the supplied name.
    pub fn phase(name: &'static str) -> Self {
        Self::start("phase", &[("cruise.phase.name", name)])
    }

    /// Starts an agent attempt span with structural backend/model metadata.
    pub fn attempt(attempt: usize, backend: &'static str, model: Option<&str>) -> Self {
        let operation = Self::start("attempt", &[("cruise.agent.backend", backend)]);
        if let Some(context) = operation.context.as_ref() {
            let span = context.span();
            if let Some(model) = model {
                span.set_attribute(KeyValue::new("cruise.agent.model", model.to_owned()));
            }
            span.set_attribute(KeyValue::new(
                "cruise.retry.attempt",
                i64::try_from(attempt).unwrap_or(i64::MAX).max(1),
            ));
        }
        operation
    }

    /// Starts a span named `cruise.{kind}` with the supplied structural attributes.
    pub fn start(kind: &'static str, attributes: &[(&'static str, &str)]) -> Self {
        if !ENABLED.load(Ordering::Acquire) {
            return Self {
                kind,
                context: None,
                started_at: None,
                finished: false,
            };
        }

        let started_at = Instant::now();
        let parent = Context::current();
        let tracer = global::tracer("cruise");
        let mut span_attributes = Vec::with_capacity(attributes.len() + 1);
        span_attributes.push(KeyValue::new("cruise.operation", kind));
        span_attributes.extend(
            attributes
                .iter()
                .map(|(key, value)| KeyValue::new(*key, (*value).to_owned())),
        );
        let span = tracer
            .span_builder(span_name(kind))
            .with_attributes(span_attributes)
            .start_with_context(&tracer, &parent);

        Self {
            kind,
            context: Some(
                parent
                    .with_span(span)
                    .with_value(ErrorTypeState(OnceLock::new())),
            ),
            started_at: Some(started_at),
            finished: false,
        }
    }

    /// Returns this operation's context for async parent-child propagation.
    pub fn context(&self) -> Context {
        self.context.clone().unwrap_or_else(Context::current)
    }

    /// Sets `error.type` on this operation's span without exposing error text.
    pub fn set_error_type(&self, error_type: &'static str) {
        if !ENABLED.load(Ordering::Acquire) {
            return;
        }
        let Some(context) = self.context.as_ref() else {
            return;
        };
        if claim_error_type(context, error_type) {
            context
                .span()
                .set_attribute(KeyValue::new("error.type", error_type));
        }
    }

    /// Ends the span and records its outcome and duration. Unknown outcomes are treated as errors.
    pub fn finish(mut self, outcome: &'static str) {
        self.finish_inner(outcome);
    }

    pub fn finish_result<T>(mut self, result: &crate::error::Result<T>) {
        let outcome = match result {
            Ok(_) => "success",
            Err(crate::error::CruiseError::Interrupted) => "cancelled",
            Err(crate::error::CruiseError::StepTimeout { .. }) => "timeout",
            Err(crate::error::CruiseError::StepPaused) => "paused",
            Err(error) => {
                self.set_error_type(error.telemetry_type());
                if let crate::error::CruiseError::LoopProtection {
                    from,
                    to,
                    max_retries,
                    edge_counts,
                    skipped_steps,
                } = error
                    && let Some(context) = self.context.as_ref()
                {
                    record_loop_protection(
                        context,
                        from,
                        to,
                        *max_retries,
                        edge_counts,
                        skipped_steps,
                    );
                }
                "error"
            }
        };
        self.finish_inner(outcome);
    }

    fn finish_inner(&mut self, outcome: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let Some(context) = self.context.as_ref() else {
            return;
        };

        let span = context.span();
        let error_type = if outcome == "success" {
            None
        } else {
            let fallback = match outcome {
                "cancelled" => "cancelled",
                "timeout" => "timeout",
                "paused" => "paused",
                _ => "step_failed",
            };
            if claim_error_type(context, fallback) {
                span.set_attribute(KeyValue::new("error.type", fallback));
            }
            Some(
                context
                    .get::<ErrorTypeState>()
                    .and_then(|state| state.0.get().copied())
                    .unwrap_or(fallback),
            )
        };
        span.set_attribute(KeyValue::new("cruise.outcome", outcome));
        if let Some(error_type) = error_type {
            span.set_status(opentelemetry::trace::Status::error(error_type));
        } else {
            span.set_status(opentelemetry::trace::Status::Ok);
        }
        span.end();

        let Some(metrics) = OPERATION_METRICS.get() else {
            return;
        };
        let attributes = [
            KeyValue::new("cruise.operation", self.kind),
            KeyValue::new("cruise.outcome", outcome),
        ];
        metrics.count.add(1, &attributes);
        metrics.duration.record(
            self.started_at
                .map_or(0.0, |started_at| started_at.elapsed().as_secs_f64()),
            &attributes,
        );
    }
}
fn record_loop_protection(
    context: &Context,
    from: &str,
    to: &str,
    max_retries: usize,
    edge_counts: &[(String, String, usize)],
    skipped_steps: &[String],
) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let span = context.span();
    span.set_attribute(KeyValue::new("cruise.loop.from", from.to_owned()));
    span.set_attribute(KeyValue::new("cruise.loop.to", to.to_owned()));
    span.set_attribute(KeyValue::new(
        "cruise.loop.max_retries",
        i64::try_from(max_retries).unwrap_or(i64::MAX),
    ));
    if let Some((_, _, traversals)) = edge_counts
        .iter()
        .find(|(edge_from, edge_to, _)| edge_from.as_str() == from && edge_to.as_str() == to)
    {
        span.set_attribute(KeyValue::new(
            "cruise.loop.traversals",
            i64::try_from(*traversals).unwrap_or(i64::MAX),
        ));
    }
    span.set_attribute(KeyValue::new(
        "cruise.loop.skipped_step_count",
        i64::try_from(skipped_steps.len()).unwrap_or(i64::MAX),
    ));
}

fn claim_error_type(context: &Context, error_type: &'static str) -> bool {
    context
        .get::<ErrorTypeState>()
        .is_none_or(|state| state.0.set(error_type).is_ok())
}

/// Sets `error.type` on the current span without exposing error text.
pub fn record_error_type(error_type: &'static str) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let context = Context::current();
    if claim_error_type(&context, error_type) {
        context
            .span()
            .set_attribute(KeyValue::new("error.type", error_type));
    }
}

pub(crate) fn capture_jcode_context() -> Option<Context> {
    ENABLED.load(Ordering::Acquire).then(Context::current)
}

pub(crate) fn record_jcode_failure(stage: &'static str, reason: &'static str) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let context = Context::current();
    let span = context.span();
    span.set_attribute(KeyValue::new("cruise.agent.failure_stage", stage));
    span.set_attribute(KeyValue::new("cruise.agent.failure_reason", reason));
}
#[derive(Debug, Eq, PartialEq, Serialize)]
struct CiFailedCheck {
    name: String,
    state: CiCheckState,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

#[derive(Debug)]
struct CiFailedCheckFields {
    name: String,
    state: CiCheckState,
    run_id: Option<serde_json::Value>,
    job_id: Option<serde_json::Value>,
    url: Option<serde_json::Value>,
}

impl<'de> Deserialize<'de> for CiFailedCheckFields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            name: String,
            state: CiCheckState,
            #[serde(default)]
            run_id: Option<serde_json::Value>,
            #[serde(default)]
            job_id: Option<serde_json::Value>,
            #[serde(default)]
            url: Option<serde_json::Value>,
        }

        let fields: Fields = deserialize_object(deserializer)?;
        Ok(Self {
            name: fields.name,
            state: fields.state,
            run_id: fields.run_id,
            job_id: fields.job_id,
            url: fields.url,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CiDiagnosticDetailsFields {
    checks: Vec<CiFailedCheckFields>,
    truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum CiCheckState {
    #[serde(rename = "FAILURE")]
    Failure,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "TIMED_OUT")]
    TimedOut,
    #[serde(rename = "CANCELLED")]
    Cancelled,
    #[serde(rename = "ACTION_REQUIRED")]
    ActionRequired,
    #[serde(rename = "STARTUP_FAILURE")]
    StartupFailure,
    #[serde(rename = "STALE")]
    Stale,
    #[serde(rename = "failed")]
    Failed,
}

impl<'de> Deserialize<'de> for CiCheckState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(CiCheckStateVisitor)
    }
}

struct CiCheckStateVisitor;

impl serde::de::Visitor<'_> for CiCheckStateVisitor {
    type Value = CiCheckState;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string CI check state")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        match value {
            "FAILURE" => Ok(CiCheckState::Failure),
            "ERROR" => Ok(CiCheckState::Error),
            "TIMED_OUT" => Ok(CiCheckState::TimedOut),
            "CANCELLED" => Ok(CiCheckState::Cancelled),
            "ACTION_REQUIRED" => Ok(CiCheckState::ActionRequired),
            "STARTUP_FAILURE" => Ok(CiCheckState::StartupFailure),
            "STALE" => Ok(CiCheckState::Stale),
            "failed" => Ok(CiCheckState::Failed),
            _ => Err(E::custom("unknown CI check state")),
        }
    }
}

fn deserialize_object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct ObjectVisitor<T>(std::marker::PhantomData<T>);

    impl<'de, T> serde::de::Visitor<'de> for ObjectVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_map(ObjectVisitor(std::marker::PhantomData))
}

struct CiDiagnosticDetails {
    checks: Vec<CiFailedCheck>,
    truncated: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct CiDiagnostic {
    provider: &'static str,
    reason: &'static str,
    state: &'static str,
    head_matches: Option<bool>,
    check_count: Option<i64>,
    http_status: Option<i64>,
    failed_checks: Vec<CiFailedCheck>,
    failed_checks_truncated: bool,
}

fn next_ascii_token<'a>(line: &'a str, position: &mut usize) -> Option<&'a str> {
    let bytes = line.as_bytes();
    while *position < bytes.len() && bytes[*position].is_ascii_whitespace() {
        *position += 1;
    }
    let start = *position;
    while *position < bytes.len() && !bytes[*position].is_ascii_whitespace() {
        *position += 1;
    }
    (start != *position).then_some(&line[start..*position])
}

fn ci_check_state_allowed(provider: &str, state: CiCheckState) -> bool {
    match provider {
        "github" => matches!(
            state,
            CiCheckState::Failure
                | CiCheckState::Error
                | CiCheckState::TimedOut
                | CiCheckState::Cancelled
                | CiCheckState::ActionRequired
                | CiCheckState::StartupFailure
                | CiCheckState::Stale
        ),
        "gitlab" => state == CiCheckState::Failed,
        _ => false,
    }
}

const MAX_CI_DIAGNOSTIC_BYTES: usize = 16_384;
const MAX_CI_FAILED_CHECKS: usize = 10;
const MAX_CI_CHECK_NAME_CHARS: usize = 128;
const MAX_CI_CHECK_URL_BYTES: usize = 1024;

fn reference_string(value: Option<serde_json::Value>) -> (Option<String>, bool) {
    match value {
        Some(serde_json::Value::String(value)) => (Some(value), false),
        Some(_) => (None, true),
        None => (None, false),
    }
}
fn validate_ci_reference_id(value: Option<String>) -> (Option<String>, bool) {
    match value {
        Some(value)
            if !value.is_empty()
                && value.len() <= 20
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.bytes().any(|byte| byte != b'0') =>
        {
            (Some(value), false)
        }
        Some(_) => (None, true),
        None => (None, false),
    }
}

fn sanitize_ci_reference_url(value: Option<String>) -> (Option<String>, bool) {
    let Some(value) = value else {
        return (None, false);
    };
    let Ok(mut url) = url::Url::parse(&value) else {
        return (None, true);
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return (None, true);
    }

    let has_userinfo = url
        .as_str()
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'));
    let stripped_sensitive_parts = has_userinfo
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    let sanitized = url.to_string();
    if sanitized.len() > MAX_CI_CHECK_URL_BYTES {
        return (None, true);
    }
    (Some(sanitized), stripped_sensitive_parts)
}

fn validate_ci_failed_check(fields: CiFailedCheckFields) -> (CiFailedCheck, bool) {
    let (raw_run_id, invalid_run_id_type) = reference_string(fields.run_id);
    let (raw_job_id, invalid_job_id_type) = reference_string(fields.job_id);
    let (raw_url, invalid_url_type) = reference_string(fields.url);
    let (run_id, invalid_run_id) = validate_ci_reference_id(raw_run_id);
    let (job_id, invalid_job_id) = validate_ci_reference_id(raw_job_id);
    let (url, url_details_lost) = sanitize_ci_reference_url(raw_url);
    (
        CiFailedCheck {
            name: fields.name,
            state: fields.state,
            run_id,
            job_id,
            url,
        },
        invalid_run_id_type
            || invalid_job_id_type
            || invalid_url_type
            || invalid_run_id
            || invalid_job_id
            || url_details_lost,
    )
}

pub(crate) fn record_command_failure(
    context: &Context,
    exit_code: Option<i32>,
    signal: Option<i32>,
    stderr: &str,
) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }

    let span = context.span();
    let (diagnostic, diagnostic_status) = last_ci_diagnostic(stderr);
    span.set_attribute(KeyValue::new(
        "cruise.ci.diagnostic_status",
        diagnostic_status,
    ));
    span.set_attribute(KeyValue::new(
        "cruise.command.stderr_present",
        !stderr.is_empty(),
    ));
    if let Some(exit_code) = exit_code {
        span.set_attribute(KeyValue::new(
            "cruise.command.exit_code",
            i64::from(exit_code),
        ));
    }
    if let Some(signal) = signal {
        span.set_attribute(KeyValue::new("cruise.command.signal", i64::from(signal)));
    }
    if let Some(diagnostic) = diagnostic {
        span.set_attribute(KeyValue::new("cruise.ci.provider", diagnostic.provider));
        span.set_attribute(KeyValue::new("cruise.ci.failure_reason", diagnostic.reason));
        span.set_attribute(KeyValue::new("cruise.ci.state", diagnostic.state));
        if let Some(head_matches) = diagnostic.head_matches {
            span.set_attribute(KeyValue::new("cruise.ci.head_matches", head_matches));
        }
        if let Some(check_count) = diagnostic.check_count {
            span.set_attribute(KeyValue::new("cruise.ci.check_count", check_count));
        }
        if let Some(http_status) = diagnostic.http_status {
            span.set_attribute(KeyValue::new("cruise.ci.http_status", http_status));
        }
        if diagnostic.reason == "ci_failed" {
            let truncated = match serde_json::to_string(&diagnostic.failed_checks) {
                Ok(failed_checks) => {
                    span.set_attribute(KeyValue::new("cruise.ci.failed_checks", failed_checks));
                    diagnostic.failed_checks_truncated
                }
                Err(_) => true,
            };
            span.set_attribute(KeyValue::new(
                "cruise.ci.failed_checks_truncated",
                truncated,
            ));
        }
    }
    if let Some(reason) = last_sync_base_diagnostic(stderr) {
        span.set_attribute(KeyValue::new("cruise.sync_base.failure_reason", reason));
    }
}
pub(crate) fn record_command_stderr_log(context: &Context, stderr_text: &str) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let Some(logger_provider) = LOGGER_PROVIDER.get() else {
        return;
    };
    let sanitized = stderr::sanitize(stderr_text);
    if sanitized.body.is_empty() {
        return;
    }

    let logger = logger_provider.logger("cruise");
    let mut record = logger.create_log_record();
    record.set_event_name("cruise.command.stderr");
    record.set_severity_number(Severity::Error);
    record.set_severity_text("ERROR");
    record.set_body(AnyValue::String(sanitized.body.into()));
    record.add_attribute("cruise.command.stderr_truncated", sanitized.truncated);
    let span = context.span();
    let span_context = span.span_context();
    if span_context.is_valid() {
        record.set_trace_context(
            span_context.trace_id(),
            span_context.span_id(),
            Some(span_context.trace_flags()),
        );
    }
    logger.emit(record);
}

fn last_ci_diagnostic(stderr: &str) -> (Option<CiDiagnostic>, &'static str) {
    const V2_PREFIX: &str = "CRUISE_CI_DIAGNOSTIC_V2";
    const V4_PREFIX: &str = "CRUISE_CI_DIAGNOSTIC_V4";
    const VERSION_PREFIX: &str = "CRUISE_CI_DIAGNOSTIC_V";

    let mut saw_invalid = false;
    let mut saw_unsupported = false;
    for line in stderr.lines().rev() {
        if let Some(diagnostic) = parse_ci_diagnostic_line(line) {
            return (Some(diagnostic), "valid");
        }

        let Some(token) = line.split_ascii_whitespace().next() else {
            continue;
        };
        if token == V2_PREFIX || token == V4_PREFIX {
            saw_invalid = true;
        } else if let Some(version) = token.strip_prefix(VERSION_PREFIX)
            && !version.is_empty()
            && version != "2"
            && version != "4"
            && version.bytes().all(|byte| byte.is_ascii_digit())
        {
            saw_unsupported = true;
        }
    }

    let status = if saw_invalid {
        "invalid"
    } else if saw_unsupported {
        "unsupported"
    } else {
        "missing"
    };
    (None, status)
}

fn parse_ci_diagnostic_line(line: &str) -> Option<CiDiagnostic> {
    const V2_PREFIX: &str = "CRUISE_CI_DIAGNOSTIC_V2";
    const V4_PREFIX: &str = "CRUISE_CI_DIAGNOSTIC_V4";

    if line.len() > MAX_CI_DIAGNOSTIC_BYTES
        || !line.is_ascii()
        || !(line.starts_with(V2_PREFIX) || line.starts_with(V4_PREFIX))
    {
        return None;
    }
    let mut position = 0;
    let is_v2 = match next_ascii_token(line, &mut position)? {
        V2_PREFIX => true,
        V4_PREFIX => false,
        _ => return None,
    };
    let provider = match next_ascii_token(line, &mut position)? {
        "github" => "github",
        "gitlab" => "gitlab",
        _ => return None,
    };
    let reason = match next_ascii_token(line, &mut position)? {
        "local_head_error" => "local_head_error",
        "repository_query_error" => "repository_query_error",
        "repository_invalid" => "repository_invalid",
        "api_error" => "api_error",
        "graphql_error" => "graphql_error",
        "pr_head_missing" => "pr_head_missing",
        "head_mismatch" => "head_mismatch",
        "rollup_head_missing" => "rollup_head_missing",
        "malformed_rollup" => "malformed_rollup",
        "invalid_ci_state" => "invalid_ci_state",
        "invalid_check_count" => "invalid_check_count",
        "ci_failed" => "ci_failed",
        "unknown_state" => "unknown_state",
        "clock_error" => "clock_error",
        "timeout" => "timeout",
        _ => return None,
    };
    let state = match next_ascii_token(line, &mut position)? {
        "unobserved" => "unobserved",
        "SUCCESS" => "SUCCESS",
        "FAILURE" => "FAILURE",
        "ERROR" => "ERROR",
        "PENDING" => "PENDING",
        "EXPECTED" => "EXPECTED",
        "stale-rollup" => "stale-rollup",
        "not-registered" => "not-registered",
        "no-checks" => "no-checks",
        "success" => "success",
        "failed" => "failed",
        "canceled" => "canceled",
        "cancelled" => "cancelled",
        "skipped" => "skipped",
        "running" => "running",
        "pending" => "pending",
        "created" => "created",
        "waiting_for_resource" => "waiting_for_resource",
        "preparing" => "preparing",
        "scheduled" => "scheduled",
        "manual" => "manual",
        "unknown" => "unknown",
        _ => return None,
    };
    let head_matches = match next_ascii_token(line, &mut position)? {
        "true" => Some(true),
        "false" => Some(false),
        "unknown" => None,
        _ => return None,
    };
    let check_count = match next_ascii_token(line, &mut position)? {
        "unknown" => None,
        value if value.bytes().all(|byte| byte.is_ascii_digit()) && !value.is_empty() => {
            Some(value.parse::<i64>().ok()?)
        }
        _ => return None,
    };
    let http_status = match next_ascii_token(line, &mut position)? {
        "unknown" => None,
        value if value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_digit()) => {
            let status = value.parse::<i64>().ok()?;
            if !(100..=599).contains(&status) {
                return None;
            }
            Some(status)
        }
        _ => return None,
    };
    let details = parse_ci_diagnostic_details(&line[position..], provider, reason, is_v2)?;

    Some(CiDiagnostic {
        provider,
        reason,
        state,
        head_matches,
        check_count,
        http_status,
        failed_checks: details.checks,
        failed_checks_truncated: details.truncated,
    })
}

fn parse_ci_diagnostic_details(
    input: &str,
    provider: &str,
    reason: &str,
    is_v2: bool,
) -> Option<CiDiagnosticDetails> {
    if is_v2 {
        if !input.bytes().all(|byte| byte.is_ascii_whitespace()) {
            return None;
        }
        return Some(CiDiagnosticDetails {
            checks: Vec::new(),
            truncated: reason == "ci_failed",
        });
    }

    let tail_start = input
        .as_bytes()
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())?;
    let input = &input[tail_start..];

    let mut deserializer = serde_json::Deserializer::from_str(input);
    let details: CiDiagnosticDetailsFields = deserialize_object(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    if reason != "ci_failed" {
        if !details.checks.is_empty() || details.truncated {
            return None;
        }
        return Some(CiDiagnosticDetails {
            checks: Vec::new(),
            truncated: false,
        });
    }
    if details.checks.len() > MAX_CI_FAILED_CHECKS
        || details.checks.iter().any(|check| {
            check.name.is_empty()
                || check.name.chars().take(MAX_CI_CHECK_NAME_CHARS + 1).count()
                    > MAX_CI_CHECK_NAME_CHARS
                || check.name.chars().any(char::is_control)
                || !ci_check_state_allowed(provider, check.state)
        })
    {
        return None;
    }

    let mut truncated = details.truncated;
    let checks = details
        .checks
        .into_iter()
        .map(|fields| {
            let (check, lost_reference) = validate_ci_failed_check(fields);
            truncated |= lost_reference;
            check
        })
        .collect();
    Some(CiDiagnosticDetails { checks, truncated })
}

fn last_sync_base_diagnostic(stderr: &str) -> Option<&'static str> {
    stderr
        .lines()
        .rev()
        .find_map(parse_sync_base_diagnostic_line)
}

fn parse_sync_base_diagnostic_line(line: &str) -> Option<&'static str> {
    const PREFIX: &str = "CRUISE_SYNC_BASE_DIAGNOSTIC_V1";

    if line.len() > 256 || !line.is_ascii() || !line.starts_with(PREFIX) {
        return None;
    }
    let mut fields = line.split_ascii_whitespace();
    if fields.next()? != PREFIX {
        return None;
    }
    let field = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    crate::worktree_pr::SyncBaseFailure::ALL
        .into_iter()
        .map(crate::worktree_pr::SyncBaseFailure::marker_value)
        .find(|value| *value == field)
}

/// Captures the current span context only when telemetry is enabled.
pub(crate) fn capture_github_context() -> Option<Context> {
    ENABLED.load(Ordering::Acquire).then(Context::current)
}

/// Records the final GitHub readiness observation on its captured span.
pub(crate) fn record_github_readiness(
    context: &Context,
    mergeability: Option<(&str, &str)>,
    waiting_for: &'static str,
    ci_state: &str,
    ci_head_matches: bool,
) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let span = context.span();
    if let Some((mergeable, merge_state_status)) = mergeability {
        span.set_attribute(KeyValue::new(
            "cruise.github.mergeable",
            mergeable.to_owned(),
        ));
        span.set_attribute(KeyValue::new(
            "cruise.github.merge_state_status",
            merge_state_status.to_owned(),
        ));
    }
    span.set_attribute(KeyValue::new("cruise.github.waiting_for", waiting_for));
    span.set_attribute(KeyValue::new("cruise.github.ci_state", ci_state.to_owned()));
    span.set_attribute(KeyValue::new(
        "cruise.github.ci_head_matches",
        ci_head_matches,
    ));
}

/// Records only the classified GitHub merge refusal and numeric process metadata.
pub(crate) fn record_github_merge_refusal(
    context: &Context,
    reason: &'static str,
    http_status: Option<u16>,
    exit_code: Option<i32>,
) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let span = context.span();
    span.set_attribute(KeyValue::new("cruise.github.merge_refusal_reason", reason));
    if let Some(status) = http_status {
        span.set_attribute(KeyValue::new(
            "cruise.github.merge_http_status",
            i64::from(status),
        ));
    }
    if let Some(code) = exit_code {
        span.set_attribute(KeyValue::new(
            "cruise.github.merge_exit_code",
            i64::from(code),
        ));
    }
}

/// Records a retry/fallback event and a low-cardinality retry counter.
pub fn record_retry(
    action: &'static str,
    reason: &'static str,
    attempt: usize,
    from_model: Option<&str>,
    to_model: Option<&str>,
) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let mut attributes = vec![
        KeyValue::new("cruise.retry.action", action),
        KeyValue::new("cruise.retry.reason", reason),
        KeyValue::new(
            "cruise.retry.attempt",
            i64::try_from(attempt).unwrap_or(i64::MAX).max(1),
        ),
    ];
    if let Some(model) = from_model {
        attributes.push(KeyValue::new("cruise.retry.from_model", model.to_owned()));
    }
    if let Some(model) = to_model {
        attributes.push(KeyValue::new("cruise.retry.to_model", model.to_owned()));
    }
    Context::current()
        .span()
        .add_event("cruise.retry".to_owned(), attributes);
    if let Some(metrics) = OPERATION_METRICS.get() {
        metrics.retry_count.add(
            1,
            &[
                KeyValue::new("cruise.retry.action", action),
                KeyValue::new("cruise.retry.reason", reason),
            ],
        );
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if !self.finished {
            self.finish_inner("cancelled");
        }
    }
}

fn span_name(kind: &'static str) -> Cow<'static, str> {
    match kind {
        "workflow" => Cow::Borrowed("cruise.workflow"),
        "step" => Cow::Borrowed("cruise.step"),
        "agent" => Cow::Borrowed("cruise.agent"),
        "execution" => Cow::Borrowed("cruise.execution"),
        "phase" => Cow::Borrowed("cruise.phase"),
        "attempt" => Cow::Borrowed("cruise.attempt"),
        _ => Cow::Owned(format!("cruise.{kind}")),
    }
}

fn workflow_identity(
    config: &crate::session_config::SessionConfigRef,
) -> (&'static str, Cow<'_, str>) {
    match config {
        crate::session_config::SessionConfigRef::File { path } => (
            "file",
            path.file_name().map_or_else(
                || Cow::Borrowed("unknown"),
                std::ffi::OsStr::to_string_lossy,
            ),
        ),
        crate::session_config::SessionConfigRef::BuiltinSnapshot { name } => {
            let name = match name.as_deref() {
                Some(name) => crate::builtin_workflows::find_builtin(name)
                    .map_or("unknown", |workflow| workflow.name),
                None => crate::builtin_workflows::DEFAULT_BUILTIN_NAME,
            };
            ("builtin", Cow::Borrowed(name))
        }
        crate::session_config::SessionConfigRef::RepoSnapshot { relative_path } => (
            "repo_snapshot",
            relative_path.file_name().map_or_else(
                || Cow::Borrowed("unknown"),
                std::ffi::OsStr::to_string_lossy,
            ),
        ),
        crate::session_config::SessionConfigRef::InlineSnapshot => {
            ("inline", Cow::Borrowed("inline"))
        }
    }
}

fn resource() -> Resource {
    let mut resource = Resource::builder_empty()
        .with_service_name("cruise")
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .with_detector(Box::new(TelemetryResourceDetector))
        .with_detector(Box::new(EnvResourceDetector::new()));

    if let Ok(service_name) = env::var("OTEL_SERVICE_NAME")
        && !service_name.is_empty()
    {
        resource = resource.with_service_name(service_name);
    }

    resource.build()
}

fn is_enabled(cruise_enabled: Option<&str>, sdk_disabled: Option<&str>) -> bool {
    !sdk_disabled.is_some_and(|value| value.eq_ignore_ascii_case("true"))
        && cruise_enabled.is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Safe error returned when explicit OpenTelemetry configuration cannot initialize.
#[derive(Debug)]
pub struct InitError(InitErrorKind);

#[derive(Debug)]
enum InitErrorKind {
    AlreadyInitialized,
    InvalidExporterConfiguration(&'static str),
    ExporterInitializationFailed(&'static str),
}

impl InitError {
    fn exporter(signal: &'static str, error: &ExporterBuildError) -> Self {
        match error {
            ExporterBuildError::InvalidConfiguration(_) => {
                Self(InitErrorKind::InvalidExporterConfiguration(signal))
            }
            ExporterBuildError::InternalFailure(_) => {
                Self(InitErrorKind::ExporterInitializationFailed(signal))
            }
        }
    }
}

impl fmt::Display for InitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            InitErrorKind::AlreadyInitialized => {
                formatter.write_str("OpenTelemetry is already initialized in this process")
            }
            InitErrorKind::InvalidExporterConfiguration(signal) => write!(
                formatter,
                "invalid OpenTelemetry {signal} exporter configuration; check OTEL_EXPORTER_OTLP_* settings"
            ),
            InitErrorKind::ExporterInitializationFailed(signal) => write!(
                formatter,
                "failed to initialize OpenTelemetry {signal} exporter; check OTLP endpoint and TLS settings"
            ),
        }
    }
}

impl Error for InitError {}

#[cfg(test)]
mod tests {
    use super::{
        CiCheckState, CiDiagnostic, CiFailedCheck, Context, ErrorTypeState, claim_error_type,
        is_enabled, last_ci_diagnostic, last_sync_base_diagnostic, parse_ci_diagnostic_line,
        parse_sync_base_diagnostic_line, workflow_identity,
    };
    use crate::session_config::SessionConfigRef;
    use std::{borrow::Cow, path::PathBuf, sync::OnceLock};

    #[test]
    fn ci_diagnostic_parser_accepts_bounded_whitelisted_fields_and_v4_details() {
        let valid = concat!(
            "CRUISE_CI_DIAGNOSTIC_V4 github head_mismatch FAILURE false 4 503 ",
            r#"{"checks":[],"truncated":false}"#
        );
        assert_eq!(
            parse_ci_diagnostic_line(valid),
            Some(CiDiagnostic {
                provider: "github",
                reason: "head_mismatch",
                state: "FAILURE",
                head_matches: Some(false),
                check_count: Some(4),
                http_status: Some(503),
                failed_checks: vec![],
                failed_checks_truncated: false,
            })
        );
        assert_eq!(
            parse_ci_diagnostic_line(concat!(
                "CRUISE_CI_DIAGNOSTIC_V4 gitlab api_error unknown unknown unknown unknown ",
                r#"{"checks":[],"truncated":false}"#
            )),
            Some(CiDiagnostic {
                provider: "gitlab",
                reason: "api_error",
                state: "unknown",
                head_matches: None,
                check_count: None,
                http_status: None,
                failed_checks: vec![],
                failed_checks_truncated: false,
            })
        );
        let failed = parse_ci_diagnostic_line(concat!(
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 ",
            r#"{"checks":[{"name":"unit-tests","state":"FAILURE"}],"truncated":true}"#
        ))
        .unwrap_or_else(|| panic!("expected failed CI diagnostic to parse"));
        assert_eq!(
            failed.failed_checks,
            vec![CiFailedCheck {
                name: "unit-tests".into(),
                state: CiCheckState::Failure,
                run_id: None,
                job_id: None,
                url: None,
            }]
        );
        assert!(failed.failed_checks_truncated);
        assert_eq!(
            serde_json::to_string(&failed.failed_checks)
                .unwrap_or_else(|_| panic!("expected failed checks to serialize")),
            r#"[{"name":"unit-tests","state":"FAILURE"}]"#
        );

        let invalid = [
            "CRUISE_CI_DIAGNOSTIC_V1 github ci_failed FAILURE false 1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V3 github ci_failed FAILURE false 1 500 {",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 not-json",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 {\"checks\":[],\"truncated\":false",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 {\"checks\":\"not-an-array\",\"truncated\":false}",
            " CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 secret-provider ci_failed FAILURE false 1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github arbitrary-reason FAILURE false 1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed private-state false 1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE secret 1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false -1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false +1 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 9223372036854775808 500 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 099 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 600 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 abc {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 1000 {\"checks\":[],\"truncated\":false}",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 {\"checks\":[],\"truncated\":false} extra",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500",
        ];
        for line in invalid {
            assert_eq!(parse_ci_diagnostic_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn ci_v2_parser_accepts_scalar_diagnostics_without_check_descriptors() {
        let failed = parse_ci_diagnostic_line(
            "CRUISE_CI_DIAGNOSTIC_V2 github ci_failed FAILURE true 6 unknown",
        )
        .unwrap_or_else(|| panic!("expected scalar V2 diagnostic to parse"));
        assert_eq!(
            failed,
            CiDiagnostic {
                provider: "github",
                reason: "ci_failed",
                state: "FAILURE",
                head_matches: Some(true),
                check_count: Some(6),
                http_status: None,
                failed_checks: vec![],
                failed_checks_truncated: true,
            }
        );

        assert_eq!(
            parse_ci_diagnostic_line(concat!(
                "CRUISE_CI_DIAGNOSTIC_V2 github repository_query_error ",
                "unknown unknown unknown unknown"
            )),
            Some(CiDiagnostic {
                provider: "github",
                reason: "repository_query_error",
                state: "unknown",
                head_matches: None,
                check_count: None,
                http_status: None,
                failed_checks: vec![],
                failed_checks_truncated: false,
            })
        );

        for malformed in [
            "CRUISE_CI_DIAGNOSTIC_V2 github ci_failed FAILURE true 6 unknown extra",
            "CRUISE_CI_DIAGNOSTIC_V2 github ci_failed FAILURE invalid 6 unknown",
            "CRUISE_CI_DIAGNOSTIC_V2 github ci_failed FAILURE true 6 unknown {\"checks\":[],\"truncated\":false}",
        ] {
            assert_eq!(parse_ci_diagnostic_line(malformed), None, "{malformed:?}");
        }
    }

    #[test]
    fn ci_diagnostic_parser_rejects_unsafe_or_inconsistent_details() {
        let marker = |reason: &str, details: &str| {
            format!("CRUISE_CI_DIAGNOSTIC_V4 github {reason} FAILURE false 1 500 {details}")
        };
        let invalid_details = [
            r#"{"checks":[]}"#,
            r#"{"checks":[],"truncated":"false"}"#,
            r#"{"checks":[],"truncated":false,"extra":"private"}"#,
            r#"{"checks":[{"name":"build","state":{"FAILURE":null}}],"truncated":false}"#,
            "[[],false]",
            r#"{"checks":[["wrong-check","FAILURE"]],"truncated":false}"#,
            r#"{"checks":[{"name":"build","state":"FAILURE","extra":"private"}],"truncated":false}"#,
            r#"{"checks":[{"name":"build","state":"SUCCESS"}],"truncated":false}"#,
            r#"{"checks":[{"name":"build","state":"failed"}],"truncated":false}"#,
            r#"{"checks":[{"name":"","state":"FAILURE"}],"truncated":false}"#,
            r#"{"checks":[{"name":"build","state":"failed"}],"truncated":false}"#,
            r#"{"checks":[{"name":"build\u0001job","state":"FAILURE"}],"truncated":false}"#,
        ];
        for details in invalid_details {
            assert_eq!(
                parse_ci_diagnostic_line(&marker("ci_failed", details)),
                None,
                "{details:?}"
            );
        }
        assert_eq!(
            parse_ci_diagnostic_line(&marker(
                "head_mismatch",
                r#"{"checks":[{"name":"build","state":"FAILURE"}],"truncated":false}"#
            )),
            None
        );
        assert_eq!(
            parse_ci_diagnostic_line(&marker(
                "head_mismatch",
                r#"{"checks":[],"truncated":true}"#
            )),
            None
        );

        let ten_checks = format!(
            r#"{{"checks":[{}],"truncated":false}}"#,
            [r#"{"name":"build","state":"FAILURE"}"#; 10].join(",")
        );
        let ten = parse_ci_diagnostic_line(&marker("ci_failed", &ten_checks))
            .unwrap_or_else(|| panic!("expected ten-check diagnostic to parse"));
        assert_eq!(ten.failed_checks.len(), super::MAX_CI_FAILED_CHECKS);

        let too_many = format!(
            r#"{{"checks":[{}],"truncated":false}}"#,
            [r#"{"name":"build","state":"FAILURE"}"#; 11].join(",")
        );
        assert_eq!(
            parse_ci_diagnostic_line(&marker("ci_failed", &too_many)),
            None
        );

        let too_long_name = format!(
            r#"{{"checks":[{{"name":"{}","state":"FAILURE"}}],"truncated":false}}"#,
            "x".repeat(129)
        );
        assert_eq!(
            parse_ci_diagnostic_line(&marker("ci_failed", &too_long_name)),
            None
        );
        let unicode_name = format!(
            r#"{{"checks":[{{"name":"{}","state":"FAILURE"}}],"truncated":false}}"#,
            r"\u00e9".repeat(128)
        );
        let unicode = parse_ci_diagnostic_line(&marker("ci_failed", &unicode_name))
            .unwrap_or_else(|| panic!("expected Unicode diagnostic to parse"));
        assert_eq!(unicode.failed_checks[0].name.chars().count(), 128);
        let non_ascii_wire = marker(
            "ci_failed",
            r#"{"checks":[{"name":"café","state":"FAILURE"}],"truncated":false}"#,
        );
        assert_eq!(parse_ci_diagnostic_line(&non_ascii_wire), None);

        let valid = marker("ci_failed", r#"{"checks":[],"truncated":false}"#);
        let at_limit = format!(
            "{valid}{}",
            " ".repeat(super::MAX_CI_DIAGNOSTIC_BYTES - valid.len())
        );
        assert!(parse_ci_diagnostic_line(&at_limit).is_some());
        let oversized = format!("{at_limit} ");
        assert_eq!(parse_ci_diagnostic_line(&oversized), None);
    }

    #[test]
    fn ci_v4_failed_check_references_are_validated_and_sanitized() {
        let marker = |details: &str| {
            format!("CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 {details}")
        };
        let references = parse_ci_diagnostic_line(&marker(
            r#"{"checks":[{"name":"build","state":"FAILURE","run_id":"00000000000000000001","job_id":"12345678901234567890","url":"https://user:password@example.test/path?token=query-secret#fragment-secret"}],"truncated":false}"#,
        ))
        .unwrap_or_else(|| panic!("expected V4 check references to parse"));
        let check = &references.failed_checks[0];
        assert_eq!(check.name, "build");
        assert_eq!(check.state, CiCheckState::Failure);
        assert_eq!(check.run_id.as_deref(), Some("00000000000000000001"));
        assert_eq!(check.job_id.as_deref(), Some("12345678901234567890"));
        assert_eq!(check.url.as_deref(), Some("https://example.test/path"));
        assert!(references.failed_checks_truncated);
        let exported = serde_json::to_string(&references.failed_checks)
            .unwrap_or_else(|_| panic!("expected safe references to serialize"));
        for secret in ["user", "password", "query-secret", "fragment-secret"] {
            assert!(!exported.contains(secret));
        }

        let invalid_refs = parse_ci_diagnostic_line(&marker(
            r#"{"checks":[{"name":"lint","state":"ERROR","run_id":0,"job_id":"123456789012345678901","url":"ftp://example.test/job"}],"truncated":false}"#,
        ))
        .unwrap_or_else(|| panic!("invalid optional refs must not erase failed check"));
        let check = &invalid_refs.failed_checks[0];
        assert_eq!(check.name, "lint");
        assert_eq!(check.state, CiCheckState::Error);
        assert_eq!(check.run_id, None);
        assert_eq!(check.job_id, None);
        assert_eq!(check.url, None);
        assert!(invalid_refs.failed_checks_truncated);

        let long_url = format!("https://example.test/{}", "x".repeat(1024));
        let oversized_url = format!(
            r#"{{"checks":[{{"name":"build","state":"FAILURE","url":"{long_url}"}}],"truncated":false}}"#
        );
        let oversized = parse_ci_diagnostic_line(&marker(&oversized_url))
            .unwrap_or_else(|| panic!("oversized optional URL must retain failed check"));
        assert_eq!(oversized.failed_checks[0].name, "build");
        assert_eq!(oversized.failed_checks[0].url, None);
        assert!(oversized.failed_checks_truncated);
    }

    #[test]
    fn ci_diagnostic_parser_uses_last_valid_full_line_without_neighbor_leaks() {
        let stderr = concat!(
            "raw command error private-details-token\n",
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 1 500 {\"checks\":[{\"name\":\"old-check\",\"state\":\"FAILURE\"}],\"truncated\":false}\n",
            "CRUISE_CI_DIAGNOSTIC_V4 github private-reason FAILURE false 99 500 {\"checks\":[{\"name\":\"neighbor-private-token\",\"state\":\"FAILURE\"}],\"truncated\":false}\n",
            "CRUISE_CI_DIAGNOSTIC_V4 gitlab ci_failed failed unknown 2 unknown {\"checks\":[{\"name\":\"deploy\",\"state\":\"failed\"}],\"truncated\":true}\n",
            "raw trailing stderr private-trailing-token\n",
        );
        let (diagnostic, status) = last_ci_diagnostic(stderr);
        assert_eq!(status, "valid");
        let diagnostic = diagnostic.unwrap_or_else(|| panic!("expected valid GitLab diagnostic"));
        assert_eq!(diagnostic.provider, "gitlab");
        assert_eq!(diagnostic.reason, "ci_failed");
        assert_eq!(diagnostic.state, "failed");
        assert_eq!(diagnostic.check_count, Some(2));
        assert_eq!(
            diagnostic.failed_checks,
            vec![CiFailedCheck {
                name: "deploy".into(),
                state: CiCheckState::Failed,
                run_id: None,
                job_id: None,
                url: None,
            }]
        );
        assert!(diagnostic.failed_checks_truncated);
        let exported = serde_json::to_string(&diagnostic.failed_checks)
            .unwrap_or_else(|_| panic!("expected failed checks to serialize"));
        assert!(!exported.contains("private-details-token"));
        assert!(!exported.contains("neighbor-private-token"));
        assert!(!exported.contains("private-trailing-token"));
    }

    #[test]
    fn ci_diagnostic_status_is_bounded_and_uses_global_precedence() {
        let valid = concat!(
            "CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 2 500 ",
            r#"{"checks":[],"truncated":false}"#
        );
        let valid_v2 = "CRUISE_CI_DIAGNOSTIC_V2 github ci_failed FAILURE true 6 unknown";
        for (stderr, expected_status, has_valid_diagnostic) in [
            ("", "missing", false),
            ("private human-readable error", "missing", false),
            (
                "error mentions CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 2 500",
                "missing",
                false,
            ),
            (
                "CRUISE_CI_DIAGNOSTIC_V4suffix github ci_failed FAILURE false 2 500",
                "missing",
                false,
            ),
            ("CRUISE_CI_DIAGNOSTIC_Vx private-value", "missing", false),
            (
                "CRUISE_CI_DIAGNOSTIC_V4 github private-reason FAILURE false 2 500 {\"checks\":[],\"truncated\":false}",
                "invalid",
                false,
            ),
            (
                " CRUISE_CI_DIAGNOSTIC_V4 github ci_failed FAILURE false 2 500 {\"checks\":[],\"truncated\":false}",
                "invalid",
                false,
            ),
            (
                "CRUISE_CI_DIAGNOSTIC_V1 github ci_failed FAILURE false 2 500 {\"checks\":[],\"truncated\":false}",
                "unsupported",
                false,
            ),
            (
                "CRUISE_CI_DIAGNOSTIC_V2 github ci_failed FAILURE false 2 500 {\"checks\":[],\"truncated\":false}",
                "invalid",
                false,
            ),
            (
                "CRUISE_CI_DIAGNOSTIC_V3 github ci_failed FAILURE false 2 500 {",
                "unsupported",
                false,
            ),
            ("CRUISE_CI_DIAGNOSTIC_V7 unknown", "unsupported", false),
            (valid_v2, "valid", true),
            (
                "CRUISE_CI_DIAGNOSTIC_V4 private-value\nCRUISE_CI_DIAGNOSTIC_V2 malformed",
                "invalid",
                false,
            ),
            (
                "CRUISE_CI_DIAGNOSTIC_V2 malformed\nCRUISE_CI_DIAGNOSTIC_V4 private-value",
                "invalid",
                false,
            ),
            (
                "CRUISE_CI_DIAGNOSTIC_V4 private-value\n{valid}\nCRUISE_CI_DIAGNOSTIC_V4 malformed",
                "valid",
                true,
            ),
        ] {
            let stderr = stderr.replace("{valid}", valid);
            let (diagnostic, status) = last_ci_diagnostic(&stderr);
            assert_eq!(status, expected_status, "{stderr:?}");
            assert_eq!(diagnostic.is_some(), has_valid_diagnostic, "{stderr:?}");
        }
        let latest_v2 = concat!(
            "CRUISE_CI_DIAGNOSTIC_V2 github repository_query_error ",
            "unknown unknown unknown unknown"
        );
        let stderr = format!("{valid}\n{latest_v2}");
        let (diagnostic, status) = last_ci_diagnostic(&stderr);
        assert_eq!(status, "valid");
        assert_eq!(
            diagnostic
                .unwrap_or_else(|| panic!("expected latest V2 diagnostic"))
                .reason,
            "repository_query_error"
        );
    }

    #[test]
    fn sdk_disabled_overrides_explicit_opt_in() {
        assert!(!is_enabled(Some("true"), Some("true")));
    }

    #[test]
    fn telemetry_requires_explicit_opt_in() {
        assert!(!is_enabled(None, None));
        assert!(!is_enabled(Some("false"), None));
        assert!(is_enabled(Some("true"), None));
    }

    #[test]
    fn sync_base_diagnostic_parser_accepts_only_allowlisted_reason() {
        assert_eq!(
            parse_sync_base_diagnostic_line("CRUISE_SYNC_BASE_DIAGNOSTIC_V1 merge_conflict"),
            Some("merge_conflict")
        );
        let stderr = concat!(
            "private human-readable sync error\n",
            "CRUISE_SYNC_BASE_DIAGNOSTIC_V1 fetch_failed\n",
            "CRUISE_SYNC_BASE_DIAGNOSTIC_V1 private-reason\n",
            "CRUISE_SYNC_BASE_DIAGNOSTIC_V1 merge_failed\n",
        );
        assert_eq!(last_sync_base_diagnostic(stderr), Some("merge_failed"));
        for invalid in [
            " CRUISE_SYNC_BASE_DIAGNOSTIC_V1 fetch_failed",
            "CRUISE_SYNC_BASE_DIAGNOSTIC_V1 unknown",
            "CRUISE_SYNC_BASE_DIAGNOSTIC_V1 fetch_failed leaked",
            "CRUISE_SYNC_BASE_DIAGNOSTIC_V1",
            "CRUISE_SYNC_BASE_DIAGNOSTIC_V1 fetch_failed\u{e9}",
        ] {
            assert_eq!(
                parse_sync_base_diagnostic_line(invalid),
                None,
                "{invalid:?}"
            );
        }
        assert_eq!(
            parse_sync_base_diagnostic_line(&format!(
                "CRUISE_SYNC_BASE_DIAGNOSTIC_V1 fetch_failed{}",
                "x".repeat(230)
            )),
            None
        );
    }

    #[test]
    fn workflow_identity_exports_only_safe_names() {
        let file = SessionConfigRef::File {
            path: PathBuf::from("/private/user/workflows/build.yaml"),
        };
        let repo = SessionConfigRef::RepoSnapshot {
            relative_path: PathBuf::from("workflows/review.yaml"),
        };
        let builtin = SessionConfigRef::BuiltinSnapshot {
            name: Some("tdd".into()),
        };
        let invalid_builtin = SessionConfigRef::BuiltinSnapshot {
            name: Some("/private/user/prompt".into()),
        };

        assert_eq!(
            workflow_identity(&file),
            ("file", Cow::Borrowed("build.yaml"))
        );
        assert_eq!(
            workflow_identity(&repo),
            ("repo_snapshot", Cow::Borrowed("review.yaml"))
        );
        assert_eq!(
            workflow_identity(&builtin),
            ("builtin", Cow::Borrowed("tdd"))
        );
        assert_eq!(
            workflow_identity(&invalid_builtin),
            ("builtin", Cow::Borrowed("unknown"))
        );
        assert_eq!(
            workflow_identity(&SessionConfigRef::InlineSnapshot),
            ("inline", Cow::Borrowed("inline"))
        );
    }

    #[test]
    fn error_type_is_first_write_wins_per_context() {
        let context = Context::new().with_value(ErrorTypeState(OnceLock::new()));
        assert!(claim_error_type(&context, "command_exit"));
        assert!(!claim_error_type(&context, "step_failed"));
        assert_eq!(
            context
                .get::<ErrorTypeState>()
                .and_then(|state| state.0.get().copied()),
            Some("command_exit")
        );

        let child = context.with_value(ErrorTypeState(OnceLock::new()));
        assert!(claim_error_type(&child, "cancelled"));
        assert!(!claim_error_type(&context, "timeout"));
    }
}
