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

use opentelemetry::{
    Context, KeyValue, global,
    metrics::{Counter, Histogram, MeterProvider},
    trace::{TraceContextExt, Tracer},
};
use opentelemetry_otlp::{ExporterBuildError, MetricExporter, SpanExporter};
use opentelemetry_sdk::{
    Resource,
    metrics::SdkMeterProvider,
    resource::{EnvResourceDetector, TelemetryResourceDetector},
    trace::SdkTracerProvider,
};

static ENABLED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static OPERATION_METRICS: OnceLock<OperationMetrics> = OnceLock::new();

struct OperationMetrics {
    count: Counter<u64>,
    duration: Histogram<f64>,
    retry_count: Counter<u64>,
}

struct ErrorTypeState(AtomicBool);

/// Process-level telemetry handle. Dropping it flushes and shuts down its providers.
#[must_use = "keep the telemetry handle alive until application work is complete"]
pub struct Telemetry {
    providers: Option<(SdkTracerProvider, SdkMeterProvider)>,
}

impl Telemetry {
    /// Flushes and shuts down the configured providers. Export errors are ignored.
    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        let Some((tracer_provider, meter_provider)) = self.providers.take() else {
            return;
        };

        ENABLED.store(false, Ordering::Release);
        let _ = tracer_provider.shutdown();
        let _ = meter_provider.shutdown();
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

/// Initializes process-global trace and metric providers when explicitly enabled.
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
        INITIALIZED.store(false, Ordering::Release);
        return Err(InitError(InitErrorKind::AlreadyInitialized));
    }

    global::set_tracer_provider(tracer_provider.clone());
    global::set_meter_provider(meter_provider.clone());
    ENABLED.store(true, Ordering::Release);

    Ok(Telemetry {
        providers: Some((tracer_provider, meter_provider)),
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
                    .with_value(ErrorTypeState(AtomicBool::new(false))),
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
        if claim_error_type(context) {
            context
                .span()
                .set_attribute(KeyValue::new("error.type", error_type));
        }
    }

    /// Ends the span and records its outcome and duration. Unknown outcomes are treated as errors.
    pub fn finish(mut self, outcome: &'static str) {
        self.finish_inner(outcome);
    }

    /// Finishes the operation using a safe outcome and error category from a result.
    pub fn finish_result<T>(mut self, result: &crate::error::Result<T>) {
        let outcome = match result {
            Ok(_) => "success",
            Err(crate::error::CruiseError::Interrupted) => "cancelled",
            Err(crate::error::CruiseError::StepTimeout { .. }) => "timeout",
            Err(crate::error::CruiseError::StepPaused) => "paused",
            Err(error) => {
                self.set_error_type(error.telemetry_type());
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
        if outcome != "success" && claim_error_type(context) {
            let error_type = match outcome {
                "cancelled" => "cancelled",
                "timeout" => "timeout",
                "paused" => "paused",
                _ => "step_failed",
            };
            span.set_attribute(KeyValue::new("error.type", error_type));
        }
        span.set_attribute(KeyValue::new("cruise.outcome", outcome));
        if outcome == "success" {
            span.set_status(opentelemetry::trace::Status::Ok);
        } else {
            span.set_status(opentelemetry::trace::Status::error(""));
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

fn claim_error_type(context: &Context) -> bool {
    context.get::<ErrorTypeState>().is_none_or(|state| {
        state
            .0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    })
}

/// Sets `error.type` on the current span without exposing error text.
pub fn record_error_type(error_type: &'static str) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let context = Context::current();
    if claim_error_type(&context) {
        context
            .span()
            .set_attribute(KeyValue::new("error.type", error_type));
    }
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
    use super::{Context, ErrorTypeState, claim_error_type, is_enabled, workflow_identity};
    use crate::session_config::SessionConfigRef;
    use std::{borrow::Cow, path::PathBuf, sync::atomic::AtomicBool};

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
        let context = Context::new().with_value(ErrorTypeState(AtomicBool::new(false)));
        assert!(claim_error_type(&context));
        assert!(!claim_error_type(&context));

        let child = context.with_value(ErrorTypeState(AtomicBool::new(false)));
        assert!(claim_error_type(&child));
        assert!(!claim_error_type(&context));
    }
}
