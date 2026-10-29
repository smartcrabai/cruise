//! Retryable-failure handling for the SDK backends (`sdk: jcode`, `sdk: claude`, `sdk: opencode`).
//!
//! `FallbackEngine` decides, after a failed turn, whether to retry and how.
//! It runs in one of two modes, selected by whether the workflow declares an
//! explicit `retry:` block or a model array with fallback entries
//! ([`crate::config::RetryConfig`]):
//!
//! - **No policy** — cruise's historical behavior: only a backend-reported
//!   rate limit (`Failure::Limited`) is retryable, always on the same model,
//!   on the command backend's 2s-doubling backoff
//!   ([`crate::step::command::calculate_backoff`]).
//! - **Policy** — 4xx (except 429, 401, 403, 407 and 408), 5xx and network
//!   failures become retryable too ([`classify_retryable`]), the backoff
//!   becomes `min(base_delay_ms * 2^(attempt-1), 8s)` with jitter (a server
//!   `Retry-After` hint wins), and a failing model is swapped at once for the
//!   next usable entry of its fallback chain (a rate-limited model is cooled
//!   down first) — with no delay, a fresh budget, and never after the turn already streamed visible text. A
//!   [`RetryClass::ClientError`] only switches and leaves no cooldown behind,
//!   while a [`RetryClass::ModelMissing`] switches *and* cools the model down
//!   so later turns stop re-selecting a model the provider does not have;
//!   see [`classify_retryable`] for the per-status rules.
//!
//! The retry *budget* is always the caller's existing `PromptRun::max_retries`,
//! spent per model: this module adds no second retry count, and
//! `max_retries == 0` (`--rate-limit-retries 0`) disables retrying, and
//! therefore switching for retryable responses; an unusable model reference
//! can still move to the next entry because no request was sent.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::backend::effort::split_thinking_suffix;
use crate::config::RetryConfig;
use crate::model_selection::ModelAvailability;

/// Fallback-chain key used when no model-specific chain matches.
pub(crate) const DEFAULT_CHAIN_KEY: &str = "default";

/// Suffix marking a chain key or entry as a provider wildcard (`anthropic/*`).
pub(crate) const WILDCARD_SUFFIX: &str = "/*";

/// Backend-selected model syntax, never inferred from a display label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ModelReferenceFormat {
    /// Existing jcode and Claude `model:effort` references.
    LegacyEffort,
    /// `OpenCode` `provider/model:effort` or `provider/model#variant` references.
    OpenCode,
}

/// Keep common effort distinct from native variants until backend resolution.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ModelSuffix<'a> {
    Effort(&'a str),
    Variant(&'a str),
}

impl ModelReferenceFormat {
    pub(crate) fn for_sdk(sdk: crate::model_selection::Sdk) -> Self {
        match sdk {
            crate::model_selection::Sdk::OpenCode => Self::OpenCode,
            _ => Self::LegacyEffort,
        }
    }

    pub(crate) fn split(self, model: &str) -> (&str, Option<ModelSuffix<'_>>) {
        match self {
            Self::LegacyEffort => {
                let (base, effort) = split_thinking_suffix(model);
                (base, effort.map(ModelSuffix::Effort))
            }
            Self::OpenCode => {
                if let Some((base, variant)) = model.split_once('#') {
                    (
                        split_thinking_suffix(base).0,
                        Some(ModelSuffix::Variant(variant)),
                    )
                } else {
                    let (base, effort) = split_thinking_suffix(model);
                    (base, effort.map(ModelSuffix::Effort))
                }
            }
        }
    }

    fn suffix(self, suffix: ModelSuffix<'_>) -> String {
        match (self, suffix) {
            (_, ModelSuffix::Effort(value)) | (Self::LegacyEffort, ModelSuffix::Variant(value)) => {
                format!(":{value}")
            }
            (Self::OpenCode, ModelSuffix::Variant(value)) => format!("#{value}"),
        }
    }
}

/// Ceiling of the computed exponential backoff, before jitter.
const BACKOFF_CEILING_MS: u64 = 8_000;

/// Upper bound applied to a server-provided `Retry-After` hint.
const RETRY_AFTER_CLAMP: Duration = Duration::from_secs(60);

/// How long a model that failed retryably is skipped when resolving the model
/// for a later turn. In-memory only: nothing is persisted across processes.
const MODEL_COOLDOWN: Duration = Duration::from_mins(30);

/// Characters scanned after a `Retry-After` marker while looking for its value.
const HINT_SCAN_CHARS: usize = 24;

/// Bytes before a status code that may carry a marker making it an HTTP status.
const STATUS_MARKER_WINDOW: usize = 24;

/// Markers that turn a bare number into an HTTP status code.
const STATUS_MARKERS: &[&str] = &["http", "status", "code", "error", "returned", "upstream"];

/// Status codes another model cannot fix, so they never become a
/// [`RetryClass::ClientError`]: `429` is a wait, `401` / `403` / `407` are
/// credential and permission failures, and `408` is a path timeout classified
/// as a network error.
const NON_CLIENT_4XX: &[&str] = &["401", "403", "407", "408", "429"];

/// Wordings that make a failure permanent: neither waiting nor another model
/// changes the outcome, so text containing one is never classified as
/// retryable.
const PERMANENT_MARKERS: &[&str] = &[
    "invalid_request",
    "invalid request",
    "invalid_api_key",
    "invalid api key",
    "authentication",
    "unauthorized",
    "forbidden",
    "permission denied",
    "not found",
    "insufficient",
];

/// Provider stop messages for exhausted context/output budgets are not
/// transient rate limits, even when the backend reports `LimitReached`.
const NON_RETRYABLE_LIMIT_MARKERS: &[&str] = &[
    "context limit",
    "context length",
    "context window",
    "too long",
    "max_tokens",
    "max_output_tokens",
    "max_completion_tokens",
    "max output tokens",
    "maximum output tokens",
    "output token limit",
    "output tokens exhausted",
    "provider stopped with length",
    "output may be incomplete",
];

/// Wordings that name the *model* as the defect rather than the request: the
/// provider has no such model, so every later turn must avoid it too.
///
/// Matched against [`provider_head`] only, never the appended child stderr,
/// Generic `unsupported model` is excluded because it can also spell a
/// refused parameter (`unsupported model parameter: 'temperature'`). The
/// `OpenAI` provider's more specific `unsupported OpenAI model` wording is
/// unambiguous and may be classified as a missing model.
const MODEL_MISSING_MARKERS: &[&str] = &[
    "model_not_supported",
    "model not supported",
    "model is not supported",
    "unsupported openai model",
    "model_not_found",
    "model not found",
    "not_found_error",
    "requested entity was not found",
    "unknown model",
    "model does not exist",
    "no such model",
    "not a valid model",
];

/// Provider-side failure wordings (HTTP 5xx and its prose equivalents).
const SERVER_MARKERS: &[&str] = &[
    "overloaded",
    "internal server error",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
];

/// Transport failure wordings. Deliberately phrase-level: a bare `timeout` or
/// `terminated` also occurs in permanent diagnostics.
const NETWORK_MARKERS: &[&str] = &[
    "connection refused",
    "connection reset",
    "connection closed",
    "connection aborted",
    "connection terminated",
    "econnrefused",
    "econnreset",
    "etimedout",
    "socket hang up",
    "broken pipe",
    "network error",
    "fetch failed",
    "name resolution",
    "dns error",
    "stream terminated",
    "timed out",
];

/// The process-wide fallback policy for calls that are not inside a run task
/// (for example, config inspection and focused unit tests).
///
/// Run tasks must use [`with_active_policy`] instead: `run --all` resolves
/// several configs concurrently, so one process-wide slot cannot identify
/// which policy belongs to a prompt.
static ACTIVE_POLICY: LazyLock<RwLock<Option<Arc<RetryPolicy>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Process-wide cooldowns for models that failed retryably.
///
/// A workflow may recreate its retry policy when a later turn or config reload
/// starts, but the plan requires a failed model to remain skipped for the full
/// cooldown window within this process. The map is deliberately in memory only
/// and is keyed by the reference format and suffix-stripped model reference.
type ModelCooldownKey = (ModelReferenceFormat, crate::model_selection::Sdk, String);
static MODEL_COOLDOWNS: LazyLock<Mutex<HashMap<ModelCooldownKey, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// Policy context for one asynchronous run. Tokio task-local storage follows
// the run future without changing the public `crate::executor::PromptRun`
// shape or sharing policies between concurrent sessions.
tokio::task_local! {
    static TASK_POLICY: Option<Arc<RetryPolicy>>;
}

/// A published retry policy, whether explicit or derived from a model array.
///
/// Model cooldowns are process-wide and independent of this policy's config so
/// recreating a policy for a later turn cannot immediately reuse a failed model.
pub struct RetryPolicy {
    config: RetryConfig,
}

impl RetryPolicy {
    /// Wrap `config` as a retry policy.
    #[must_use]
    pub(crate) fn new(config: RetryConfig) -> Self {
        Self { config }
    }

    pub(crate) fn config(&self) -> &RetryConfig {
        &self.config
    }

    /// Mark `model` as recently failed, so later turns prefer its fallbacks.
    ///
    /// Keyed by the effort-stripped reference: `provider/m:high` and
    /// `provider/m:low` are the same model behind the same quota.
    #[cfg(test)]
    fn start_cooldown(format: ModelReferenceFormat, model: &str) {
        let sdk = if format == ModelReferenceFormat::OpenCode {
            crate::model_selection::Sdk::OpenCode
        } else {
            crate::model_selection::Sdk::Jcode
        };
        Self::start_sdk_cooldown(format, sdk, model);
    }

    fn start_sdk_cooldown(
        _format: ModelReferenceFormat,
        sdk: crate::model_selection::Sdk,
        model: &str,
    ) {
        let format = ModelReferenceFormat::for_sdk(sdk);
        let mut map = MODEL_COOLDOWNS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        map.insert(
            (format, sdk, format.split(model).0.to_string()),
            Instant::now() + MODEL_COOLDOWN,
        );
    }

    /// Whether `model` is still cooling down, dropping expired entries as it
    /// goes.
    #[cfg(test)]
    fn is_cooling(format: ModelReferenceFormat, model: &str) -> bool {
        let sdk = if format == ModelReferenceFormat::OpenCode {
            crate::model_selection::Sdk::OpenCode
        } else {
            crate::model_selection::Sdk::Jcode
        };
        Self::is_sdk_cooling(format, sdk, model)
    }

    fn is_sdk_cooling(
        _format: ModelReferenceFormat,
        sdk: crate::model_selection::Sdk,
        model: &str,
    ) -> bool {
        let format = ModelReferenceFormat::for_sdk(sdk);
        let mut map = MODEL_COOLDOWNS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        map.retain(|_, until| *until > now);
        map.contains_key(&(format, sdk, format.split(model).0.to_string()))
    }
}

/// Build a retry policy for one resolved workflow config.
#[must_use]
pub fn policy_for_config(policy: Option<RetryConfig>) -> Option<Arc<RetryPolicy>> {
    policy.map(|config| Arc::new(RetryPolicy::new(config)))
}

/// Publish `policy` for calls that are not inside a run-task scope.
pub(crate) fn set_active_policy(policy: Option<RetryConfig>) {
    let mut slot = ACTIVE_POLICY
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    *slot = policy_for_config(policy);
}

/// Run `future` with the policy belonging to one workflow session.
pub async fn with_active_policy<F, T>(policy: Option<Arc<RetryPolicy>>, future: F) -> T
where
    F: Future<Output = T>,
{
    TASK_POLICY.scope(policy, future).await
}

/// The retry policy in force, or `None` when no workflow declared one.
///
/// A run-task scope wins over the process-wide fallback, including an explicit
/// `None`, so concurrent sessions cannot observe each other's policy.
#[must_use]
pub fn active_policy() -> Option<Arc<RetryPolicy>> {
    if let Ok(policy) = TASK_POLICY.try_with(Clone::clone) {
        return policy;
    }
    ACTIVE_POLICY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Why a failed turn is worth retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryClass {
    /// Provider rate / usage limit (HTTP 429 and the limit wordings).
    RateLimit,
    /// Provider-side failure (HTTP 5xx, "overloaded", "service unavailable").
    ServerError,
    /// Request-side refusal (HTTP 4xx other than 429, 401, 403, 407 and 408;
    /// see [`classify_retryable`]): only another model can help, and it is the
    /// one class that never cools down the model it leaves behind, because a
    /// refused request says nothing about the model's health.
    ClientError,
    /// The provider answered, but does not have this model at all
    /// (`model_not_supported`, `404 model_not_found`). Switch-only like
    /// [`RetryClass::ClientError`] — yet here the model *is* the defect, so it
    /// cools down and later turns stop re-selecting it.
    ModelMissing,
    /// Transport failure (connection reset, timeout, terminated stream), and
    /// HTTP 408: the path failed rather than the request being refused. Like
    /// every class but [`RetryClass::ClientError`], an abandoned model cools
    /// down.
    Network,
    /// The backend could not dispatch the model reference at all, so only
    /// another model can help.
    ModelUnusable,
}

impl RetryClass {
    /// Stable low-cardinality category for retry telemetry and error types.
    #[must_use]
    pub(crate) fn telemetry_type(self) -> &'static str {
        match self {
            RetryClass::RateLimit => "rate_limit",
            RetryClass::ServerError => "server_error",
            RetryClass::ClientError => "client_error",
            RetryClass::ModelMissing => "model_missing",
            RetryClass::Network => "network",
            RetryClass::ModelUnusable => "model_unusable",
        }
    }

    /// Retry metrics reuse the same fixed categories as retry decisions.
    #[must_use]
    pub(crate) fn telemetry_reason(self) -> &'static str {
        self.telemetry_type()
    }
    /// Sentence-leading label for progress notices.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            RetryClass::RateLimit => "Rate limit",
            RetryClass::ServerError => "Server error",
            RetryClass::ClientError => "Client error",
            RetryClass::ModelMissing => "Missing model",
            RetryClass::Network => "Network error",
            RetryClass::ModelUnusable => "Unusable model",
        }
    }

    /// Whether leaving a model behind for this class makes the model itself
    /// suspect, so it must be skipped for [`MODEL_COOLDOWN`].
    /// [`RetryClass::ClientError`] is the one class that says nothing about
    /// the model.
    #[must_use]
    fn cools_down(self) -> bool {
        !matches!(self, RetryClass::ClientError)
    }
}

/// How a turn failed, as reported by the backend.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Failure<'a> {
    /// The backend tagged this as a limit; context/output exhaustion must not
    /// be mistaken for a transient provider limit.
    Limited(&'a str),
    /// Any other failure; retryable only when [`classify_retryable`] says so.
    Failed(&'a str),
    /// The backend refused the model reference before sending anything (an
    /// unparseable `provider/model[:effort]`, an unknown effort suffix).
    Unusable(&'a str),
}
impl<'a> Failure<'a> {
    /// The reported failure text.
    fn message(self) -> &'a str {
        match self {
            Failure::Limited(message) | Failure::Failed(message) | Failure::Unusable(message) => {
                message
            }
        }
    }
}

/// What the caller should do after a failed turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetryAction {
    /// Re-run immediately on `to`, with a fresh retry budget.
    Switch {
        from: Option<String>,
        to: String,
        /// Failure detail for the notice: the HTTP status code the provider
        /// named, else the [`RetryClass`] label.
        detail: String,
        /// Retries spent so far, across every model.
        attempt: usize,
        /// Retries this run may spend in total.
        of: usize,
    },
    /// Re-run on the same model after `delay`.
    Backoff {
        delay: Duration,
        class: RetryClass,
        /// Retries spent so far, across every model.
        attempt: usize,
        /// Retries this run may spend in total.
        of: usize,
    },
    /// Surface the failure.
    GiveUp,
}

/// Whether `lower` carries a provider context/output exhaustion stop.
///
/// Checked against [`provider_head`] for backend-reported limits so an
/// unrelated child stderr line cannot suppress a genuine rate-limit retry.
fn is_non_retryable_limit(lower: &str) -> bool {
    NON_RETRYABLE_LIMIT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Whether `lower` (already lowercased) carries a permanent-failure wording.
fn is_permanent(lower: &str) -> bool {
    NON_RETRYABLE_LIMIT_MARKERS
        .iter()
        .chain(PERMANENT_MARKERS)
        .any(|marker| lower.contains(marker))
}

/// Whether `lower` (already lowercased) reports that the provider does not
/// have this model, rather than refusing this request. The reference itself is
/// the defect: neither waiting nor re-sending helps, and no later turn should
/// pick the model again.
///
/// Takes the [`provider_head`] of the failure text, not the whole of it.
fn is_model_missing(head: &str) -> bool {
    MODEL_MISSING_MARKERS
        .iter()
        .any(|marker| head.contains(marker))
}

/// Only the structured `OpenCode` execution diagnostic identifies a missing route.
/// Authentication and invalid requests must not inherit missing-model overrides.
fn is_opencode_no_route(head: &str) -> bool {
    head.starts_with("opencode model execution failed [provider.no-route]:")
}

fn is_no_route_permanent(head: &str) -> bool {
    ["401", "403", "407"]
        .iter()
        .any(|code| has_status_code(head, code))
        || NON_RETRYABLE_LIMIT_MARKERS
            .iter()
            .chain(
                PERMANENT_MARKERS
                    .iter()
                    .filter(|marker| **marker != "not found"),
            )
            .any(|marker| head.contains(marker))
}

/// The provider's own error text: everything before the child-process stderr
/// tail the SDK backends append (`\njcode stderr:\n…`,
/// `\nclaude stderr:\n…`; see [`crate::backend::jcode`]).
///
/// Up to 64 lines of arbitrary child logging travel in that tail, including
/// lines about other models and other turns, so it must never decide *which
/// model* a failure is about. The status-code rules guard the same hazard by
/// checking 4xx last; a wording cannot be ordered out of reach that way, so it
/// is scoped instead.
fn provider_head(lower: &str) -> &str {
    lower
        .find(" stderr:\n")
        .map_or(lower, |at| &lower[..lower[..at].rfind('\n').unwrap_or(0)])
}

/// The provider limit wordings: the command backend's
/// [`crate::step::command::is_rate_limited`] — so the SDK backends retry
/// exactly what `command:` retries — plus the subscription-window phrasings the
/// SDK providers add.
fn is_limit_wording(lower: &str) -> bool {
    crate::step::command::is_rate_limited(lower)
        || lower.contains("usage limit")
        || lower.contains("session limit")
}

/// Whether a backend must report `text` as a retryable
/// [`crate::backend::stream::StreamChunk::Limit`] rather than a plain error.
///
/// The single definition behind both SDK backends' limit classification: the
/// limit wordings, plus the `overloaded` capacity refusal provider runtimes
/// report in place of a 5xx. A permanent failure never qualifies, however
/// limit-like the rest of the text reads.
#[must_use]
pub(crate) fn is_limit_message(text: &str) -> bool {
    let lower = text.to_lowercase();
    !is_permanent(&lower) && (is_limit_wording(&lower) || lower.contains("overloaded"))
}

/// Classify an error message, returning `None` for permanent failures
/// (authentication, invalid request, context/output exhaustion) which name no
/// HTTP status code and must never be retried.
///
/// Unifies the command backend's [`crate::step::command::is_rate_limited`] and
/// the SDK backends' limit wordings ([`is_limit_message`]), and extends them
/// with the 5xx, network and 4xx conditions the fallback engine also retries.
/// The per-status rules, authoritative for this module:
///
/// - `429` is [`RetryClass::RateLimit`]: a wait, not a refusal.
/// - `401`, `403` and `407` never become retryable — no other model can supply
///   credentials the run does not have, so they fall through to the permanent
///   wordings, which is where credential and permission failures belong.
/// - `408` is [`RetryClass::Network`]: a request timeout is the path failing
///   rather than the request being refused.
/// - every other 4xx is [`RetryClass::ClientError`]; see
///   [`FallbackEngine::next`] for what that class does.
/// - a failure whose provider text names the model itself as absent
///   (`400 model_not_supported`, `404 model_not_found`) is
///   [`RetryClass::ModelMissing`] rather than a client error. It is checked
///   after the limit, 408, 5xx and network rules — which keep winning — and
///   before the bare 4xx fallthrough, so it never turns a throttled or
///   unreachable provider into a dead model. It does override the permanent
///   wordings, which is how `invalid_request_error: model not found` reaches
///   the next chain entry instead of failing the step.
///
/// The order matters, and the bare 4xx check is deliberately **last**. The
/// text classified here is not a clean provider status line: the jcode backend
/// appends a tail of the process's stderr to it, so a Node stack frame
/// (`at fetch (node:internal/errors:496:11)`) or a `retry-after-ms: 400` hint
/// travels with the real error. Checking 4xx early let such a number hijack
/// `429 Too Many Requests` into a client error, which never waits and never
/// re-sends. The unambiguous wordings therefore win first, and only text that
/// names nothing else falls through to the client-error class.
///
/// A named 4xx status still wins over permanent request wordings: `400
/// invalid_request: context length exceeded` is the provider refusing *this*
/// model's request, which the next entry of the fallback chain may well
/// accept. A permanent wording without a status code (`unknown option
/// '--effort'`) stays `None`, unless it also names the model as absent.
#[must_use]
pub(crate) fn classify_retryable(text: &str) -> Option<RetryClass> {
    classify_retryable_lower(&text.to_lowercase())
}

/// Classify `lower`, which must already be lowercased.
fn classify_retryable_lower(lower: &str) -> Option<RetryClass> {
    let client_status = first_client_status_code(lower);
    let head = provider_head(lower);
    let no_route = is_opencode_no_route(head);
    if no_route && is_no_route_permanent(head) {
        return None;
    }
    let model_missing = is_model_missing(head) || no_route;
    if is_permanent(lower) && client_status.is_none() && !model_missing {
        return None;
    }
    if is_limit_wording(lower) {
        return Some(RetryClass::RateLimit);
    }
    if has_status_code(lower, "408") {
        return Some(RetryClass::Network);
    }
    if SERVER_MARKERS.iter().any(|marker| lower.contains(marker))
        || first_server_status_code(lower).is_some()
    {
        return Some(RetryClass::ServerError);
    }
    if NETWORK_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Some(RetryClass::Network);
    }
    if model_missing {
        return Some(RetryClass::ModelMissing);
    }
    client_status.map(|_| RetryClass::ClientError)
}

/// Classify a backend-reported limit without retrying exhausted context/output
/// stops as rate limits.
fn classify_limited_message(text: &str) -> Option<RetryClass> {
    let lower = text.to_lowercase();
    if is_non_retryable_limit(provider_head(&lower)) {
        return None;
    }
    Some(classify_retryable_lower(&lower).unwrap_or(RetryClass::RateLimit))
}

/// Return the first standalone HTTP status code named in `lower` that starts
/// with `leading` and is not in `excluded`.
fn first_status_code<'t>(lower: &'t str, leading: char, excluded: &[&str]) -> Option<&'t str> {
    lower.char_indices().find_map(|(at, ch)| {
        if ch != leading {
            return None;
        }
        let code = lower.get(at..at + 3)?;
        (!excluded.contains(&code)
            && code.as_bytes()[1..].iter().all(u8::is_ascii_digit)
            && has_status_code(lower, code))
        .then_some(code)
    })
}

/// Return the first standalone HTTP 5xx status code named in `lower`.
fn first_server_status_code(lower: &str) -> Option<&str> {
    first_status_code(lower, '5', &[])
}

/// Return the first standalone 4xx status code in `lower` outside
/// [`NON_CLIENT_4XX`].
fn first_client_status_code(lower: &str) -> Option<&str> {
    first_status_code(lower, '4', NON_CLIENT_4XX)
}

/// Whether `code` occurs as a standalone number preceded, within
/// [`STATUS_MARKER_WINDOW`] bytes, by a marker that makes it an HTTP status.
/// Keeps `max_tokens: 5000` and `context length 500000` out of the retryable
/// classes.
///
/// Two shapes are rejected even when a marker precedes them, because the
/// jcode backend appends process stderr to the failure text and both occur
/// there next to the word `error`:
///
/// - a source position, `code` followed by `:` and another digit —
///   `at fetch (node:internal/errors:496:11)` names line 496, column 11;
/// - a URL port, `code` preceded by `:` and followed by `/` —
///   `https://api.example.com:443/v1/messages`.
///
/// `status 404: not found` and `HTTP/1.1 400 bad request` keep matching: a
/// colon followed by a space is punctuation, not a column number.
fn has_status_code(lower: &str, code: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(code) {
        let at = from + rel;
        let end = at + code.len();
        let before = (at > 0).then(|| bytes[at - 1]);
        let after = bytes.get(end).copied();
        let source_position =
            after == Some(b':') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit);
        let url_port = before == Some(b':') && after == Some(b'/');
        let standalone = before.is_none_or(|b| !b.is_ascii_alphanumeric())
            && after.is_none_or(|b| !b.is_ascii_alphanumeric());
        if standalone && !source_position && !url_port {
            let mut start = at.saturating_sub(STATUS_MARKER_WINDOW);
            while !lower.is_char_boundary(start) {
                start += 1;
            }
            if STATUS_MARKERS
                .iter()
                .any(|marker| lower[start..at].contains(marker))
            {
                return true;
            }
        }
        from = end;
    }
    false
}

/// Notice detail for a model switch: the HTTP status code that produced
/// `class` (the `429` of `Warning: Fallback: a -> b (429, attempt 2/5)`), or
/// the class label when the text names none. The code must agree with the
/// class, because the text may name several: a [`RetryClass::ClientError`]
/// reports its own 4xx even when an `(upstream 503)` aside follows.
fn failure_detail(class: RetryClass, text: &str) -> String {
    let lower = text.to_lowercase();
    let code = match class {
        RetryClass::RateLimit => has_status_code(&lower, "429").then_some("429"),
        RetryClass::ServerError => first_server_status_code(&lower),
        RetryClass::ClientError | RetryClass::ModelMissing => first_client_status_code(&lower),
        RetryClass::Network => has_status_code(&lower, "408").then_some("408"),
        RetryClass::ModelUnusable => None,
    };
    code.map_or_else(|| class.label().to_lowercase(), str::to_string)
}

/// The server's own retry hint, clamped to [`RETRY_AFTER_CLAMP`].
///
/// Recognizes the `retry-after-ms` (milliseconds) and `Retry-After` (seconds)
/// header wordings providers echo into their error text. A zero hint is
/// ignored: re-sending immediately would burn the whole budget in milliseconds.
#[must_use]
pub(crate) fn parse_retry_after(text: &str) -> Option<Duration> {
    let lower = text.to_lowercase();
    for marker in ["retry-after-ms", "retry_after_ms"] {
        if let Some(ms) = number_after(&lower, marker).filter(|ms| *ms > 0) {
            return Some(Duration::from_millis(ms).min(RETRY_AFTER_CLAMP));
        }
    }
    for marker in ["retry-after", "retry_after", "retry after"] {
        if let Some(secs) = number_after(&lower, marker).filter(|secs| *secs > 0) {
            return Some(Duration::from_secs(secs).min(RETRY_AFTER_CLAMP));
        }
    }
    None
}

/// The first integer following `marker`, if it starts within
/// [`HINT_SCAN_CHARS`] and is separated from the marker only by punctuation.
fn number_after(lower: &str, marker: &str) -> Option<u64> {
    let at = lower.find(marker)?;
    let mut digits = String::new();
    for ch in lower[at + marker.len()..].chars().take(HINT_SCAN_CHARS) {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if digits.is_empty() {
            if !matches!(ch, ' ' | ':' | '=' | '"' | '\'' | ',' | '\t') {
                return None;
            }
        } else {
            break;
        }
    }
    digits.parse().ok()
}

/// Exponential backoff with jitter: `min(base * 2^(attempt-1), 8s)` scaled by
/// `jitter_permille` (750..=1000, i.e. 0.75..1.00).
#[must_use]
pub(crate) fn backoff_delay(base_delay_ms: u64, attempt: usize, jitter_permille: u32) -> Duration {
    let exp = u32::try_from(attempt).unwrap_or(u32::MAX).saturating_sub(1);
    let factor = 1u64.checked_shl(exp).unwrap_or(u64::MAX);
    let capped = base_delay_ms.saturating_mul(factor).min(BACKOFF_CEILING_MS);
    Duration::from_millis(capped.saturating_mul(u64::from(jitter_permille)) / 1_000)
}

/// Jitter in permille (750..=1000), taken from the clock's sub-second noise so
/// concurrent runs do not retry in lockstep.
fn jitter_permille() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    750 + nanos % 251
}

/// The fallback chain for `model`, most specific key first: the exact
/// `provider/model`, then `provider/*`, then [`DEFAULT_CHAIN_KEY`].
///
/// A `provider/*` **entry** keeps the failing model id and swaps only the
/// provider, carrying the reference's effort or variant suffix along; every other entry
/// is used verbatim. Entries equal to the failing model, and duplicates, are
/// dropped.
fn explicit_chain<'a>(
    format: ModelReferenceFormat,
    config: &'a RetryConfig,
    model: Option<&str>,
) -> Option<&'a Vec<String>> {
    model
        .and_then(|raw| {
            config.fallback_chains.get(raw).or_else(|| {
                let base = format.split(raw).0;
                config.fallback_chains.get(base).or_else(|| {
                    base.split_once('/').and_then(|(provider, _)| {
                        config
                            .fallback_chains
                            .get(&format!("{provider}{WILDCARD_SUFFIX}"))
                    })
                })
            })
        })
        .or_else(|| config.fallback_chains.get(DEFAULT_CHAIN_KEY))
}

#[cfg(test)]
fn chain_for(
    format: ModelReferenceFormat,
    config: &RetryConfig,
    model: Option<&str>,
) -> VecDeque<String> {
    chain_for_sdk(format, config, model, true)
}

#[cfg(test)]
fn chain_for_sdk(
    format: ModelReferenceFormat,
    config: &RetryConfig,
    model: Option<&str>,
    same_sdk: bool,
) -> VecDeque<String> {
    chain_for_formats(format, format, config, model, same_sdk)
}

pub(crate) fn chain_for_formats(
    format: ModelReferenceFormat,
    entry_format: ModelReferenceFormat,
    config: &RetryConfig,
    model: Option<&str>,
    same_sdk: bool,
) -> VecDeque<String> {
    let base = model.map(|m| format.split(m).0);
    let effort = model
        .and_then(|m| format.split(m).1)
        .map(|suffix| entry_format.suffix(suffix));
    let entries = explicit_chain(format, config, model);

    let mut chain: VecDeque<String> = VecDeque::new();
    for entry in entries.into_iter().flatten() {
        let Some(candidate) = expand_entry(entry, base, effort.as_deref()) else {
            continue;
        };
        let candidate_base = entry_format.split(&candidate).0;
        if (same_sdk && Some(candidate_base) == base)
            || chain
                .iter()
                .any(|existing| entry_format.split(existing).0 == candidate_base)
        {
            continue;
        }
        chain.push_back(candidate);
    }
    chain
}

/// Resolve the authoritative sequence before capability validation or cooldowns.
pub(crate) fn effective_candidates(
    config: Option<&RetryConfig>,
    candidates: &[crate::model_selection::ModelCandidate],
    base_sdk: crate::model_selection::Sdk,
) -> crate::model_selection::ModelCandidates {
    use crate::model_selection::ModelCandidate;
    let Some(primary) = candidates.first() else {
        return Vec::new();
    };
    let primary = ModelCandidate {
        sdk: primary.sdk,
        model: primary.model.clone().filter(|model| !model.is_empty()),
    };
    let mut result = vec![primary.clone()];
    if config.is_some_and(|config| !config.model_fallback) {
        return result;
    }
    let format = ModelReferenceFormat::for_sdk(primary.sdk);
    if let Some(config) =
        config.filter(|config| explicit_chain(format, config, primary.model.as_deref()).is_some())
    {
        result.extend(
            chain_for_formats(
                format,
                ModelReferenceFormat::for_sdk(base_sdk),
                config,
                primary.model.as_deref(),
                primary.sdk == base_sdk,
            )
            .into_iter()
            .map(|model| ModelCandidate {
                sdk: base_sdk,
                model: Some(model),
            }),
        );
    } else {
        let mut seen = std::collections::HashSet::new();
        if let Some(model) = primary.model.as_deref() {
            seen.insert((primary.sdk, format.split(model).0.to_owned()));
        }
        result.extend(
            candidates
                .iter()
                .skip(1)
                .filter(|candidate| {
                    candidate.model.as_deref().is_some_and(|model| {
                        seen.insert((
                            candidate.sdk,
                            ModelReferenceFormat::for_sdk(candidate.sdk)
                                .split(model)
                                .0
                                .to_owned(),
                        ))
                    })
                })
                .cloned(),
        );
    }
    result
}

/// Resolve one chain entry against the failing reference.
fn expand_entry(entry: &str, base: Option<&str>, effort: Option<&str>) -> Option<String> {
    let Some(provider) = entry.strip_suffix(WILDCARD_SUFFIX) else {
        return Some(entry.to_string());
    };
    // A provider wildcard has nothing to keep when the failing reference did
    // not name a model at all.
    let failing = base?;
    let model_id = failing.split_once('/').map_or(failing, |(_, model)| model);
    Some(format!(
        "{provider}/{model_id}{}",
        effort.unwrap_or_default()
    ))
}

#[derive(Debug, Clone)]
pub(crate) enum Unavailable {
    CoolingDown,
    TimeWindow(String),
}

/// Per-run retry state: which model the next attempt uses, what is left of the
/// fallback chain, and how much of the retry budget the current model spent.
pub(crate) struct FallbackEngine {
    sdk: crate::model_selection::Sdk,
    format: ModelReferenceFormat,
    policy: Option<Arc<RetryPolicy>>,
    max_retries: usize,
    model: Option<String>,
    chain: VecDeque<(crate::model_selection::Sdk, String)>,
    /// Retries spent on the current model.
    attempts: usize,
    /// Retries spent across every model, for notices.
    total: usize,
    /// Retries the run may spend in total, for notices.
    budget: usize,
    /// An availability-driven model replacement made before the first attempt,
    /// waiting to be reported.
    startup_switch: Option<(String, String)>,
    availability: ModelAvailability,
    /// Why the primary was unavailable before the first attempt.
    startup_reason: Option<Unavailable>,
    /// No configured model is usable because all are disabled or cooling down.
    startup_blocked: bool,
}

impl FallbackEngine {
    /// Start a run of `model_ref` under `policy`.
    ///
    /// `model_ref` reaches the backend exactly as the caller wrote it (an empty
    /// reference means "backend default"); only a fallback switch replaces it.
    /// When the configured model is still cooling down from an earlier turn,
    /// the run starts on the first usable chain entry instead — reported
    /// through [`FallbackEngine::take_startup_switch`], so a run never silently
    /// uses a model the workflow did not name.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn new(
        format: ModelReferenceFormat,
        policy: Option<Arc<RetryPolicy>>,
        model_ref: Option<&str>,
        max_retries: usize,
    ) -> Self {
        let sdk = if format == ModelReferenceFormat::OpenCode {
            crate::model_selection::Sdk::OpenCode
        } else {
            crate::model_selection::Sdk::Jcode
        };
        Self::new_candidates(
            ModelAvailability::default(),
            policy,
            sdk,
            sdk,
            model_ref,
            &[],
            max_retries,
        )
    }

    pub(crate) fn new_candidates(
        availability: ModelAvailability,
        policy: Option<Arc<RetryPolicy>>,
        sdk: crate::model_selection::Sdk,
        base_sdk: crate::model_selection::Sdk,
        model_ref: Option<&str>,
        candidates: &[crate::model_selection::ModelCandidate],
        max_retries: usize,
    ) -> Self {
        let format = ModelReferenceFormat::for_sdk(sdk);
        let model = model_ref.filter(|m| !m.is_empty()).map(str::to_string);
        let mut all = vec![crate::model_selection::ModelCandidate {
            sdk,
            model: model.clone(),
        }];
        all.extend_from_slice(candidates);
        let chain = policy.as_deref().map_or_else(VecDeque::new, |policy| {
            effective_candidates(Some(&policy.config), &all, base_sdk)
                .into_iter()
                .skip(1)
                .filter_map(|candidate| candidate.model.map(|model| (candidate.sdk, model)))
                .collect()
        });
        let mut engine = Self {
            sdk,
            format,
            policy,
            max_retries,
            model,
            chain,
            attempts: 0,
            total: 0,
            budget: 0,
            startup_switch: None,
            availability,
            startup_reason: None,
            startup_blocked: false,
        };
        if let Some(current) = engine.model.clone() {
            let reason = engine
                .availability
                .disabled_by(sdk, &current, unix_seconds())
                .map(|window| Unavailable::TimeWindow(window.to_owned()))
                .or_else(|| {
                    (engine.policy.is_some() && RetryPolicy::is_sdk_cooling(format, sdk, &current))
                        .then_some(Unavailable::CoolingDown)
                });
            if let Some(reason) = reason {
                engine.startup_reason = Some(reason);
                if let Some(next) = engine.take_candidate() {
                    engine.model = Some(next.clone());
                    engine.startup_switch = Some((current, next));
                } else {
                    engine.startup_blocked = true;
                }
            }
        }
        // One budget per model, so `max_retries` retries of the primary plus,
        // for every chain entry, its switch and its own `max_retries` retries.
        engine.budget = max_retries
            .saturating_add(1)
            .saturating_mul(1 + engine.chain.len())
            .saturating_sub(1);
        engine
    }

    /// The model reference the next attempt must run with.
    #[must_use]
    pub(crate) fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub(crate) fn sdk(&self) -> crate::model_selection::Sdk {
        self.sdk
    }

    pub(crate) fn candidate_sdks(&self) -> impl Iterator<Item = crate::model_selection::Sdk> + '_ {
        std::iter::once(self.sdk).chain(self.chain.iter().map(|(sdk, _)| *sdk))
    }

    /// The availability-driven `(from, to)` replacement made at construction, if
    /// any. Yielded once, for the caller to report.
    pub(crate) fn take_startup_switch(&mut self) -> Option<(String, String)> {
        self.startup_switch.take()
    }

    pub(crate) fn startup_reason(&self) -> Option<&Unavailable> {
        self.startup_reason.as_ref()
    }

    /// Whether the run must stop rather than reuse an unavailable model.
    #[must_use]
    pub(crate) fn startup_blocked(&self) -> bool {
        self.startup_blocked
    }

    /// Decide what to do after a failed turn. `streamed` reports whether the
    /// turn already pushed assistant text to the user's output sink. Returns
    /// both the action and its existing retry class for telemetry.
    ///
    /// A [`RetryClass::ClientError`] only ever switches models: the provider
    /// refused this request, so re-sending it unchanged would fail the same
    /// way, and with no usable candidate the original error surfaces. It is
    /// also the one class that leaves no cooldown on the model it abandons.
    /// A [`RetryClass::ModelMissing`] switches the same way but does cool the
    /// model down: the provider has no such model, so every later turn — and
    /// every later step retrying through `if.fail.retry` — must stop choosing
    /// it instead of burning a request on the same 4xx.
    /// Every other class — including the [`RetryClass::Network`] an HTTP 408
    /// arrives as — switches first too, but falls back to a same-model backoff
    /// when no candidate is usable, the turn already streamed visible text, or
    /// `--rate-limit-retries 0` disabled retrying, and cools the model it
    /// switched away from down.
    pub(crate) fn next_with_class(
        &mut self,
        failure: Failure<'_>,
        streamed: bool,
    ) -> (RetryAction, Option<RetryClass>) {
        let Some(class) = self.classify(failure) else {
            return (RetryAction::GiveUp, None);
        };
        let message = failure.message();

        // Nothing was sent, so there is no delay to serve and no budget to
        // spend: the only useful move is the next chain entry.
        if matches!(failure, Failure::Unusable(_)) {
            return (
                self.switch(class, message).unwrap_or(RetryAction::GiveUp),
                Some(class),
            );
        }

        // Every class uses a fallback immediately when one is available:
        // another model is the only thing that can help, and a rate-limited
        // model is already cooling down. If switching is disabled or no
        // candidate is usable, retain the policy's same-model retry behavior
        // instead of making `model_fallback: false` silently disable all
        // retries.
        if self.max_retries > 0
            && !streamed
            && let Some(action) = self.switch(class, message)
        {
            return (action, Some(class));
        }

        // Failures reach this path only when no usable fallback can be
        // selected, or when the turn already streamed visible text. A refused
        // request and a missing model never do: the same request to the same
        // model returns the same status.
        if !matches!(class, RetryClass::ClientError | RetryClass::ModelMissing)
            && self.attempts < self.max_retries
            && let Some(delay) = self.delay_for(message, self.attempts + 1)
        {
            self.attempts += 1;
            self.total += 1;
            return (
                RetryAction::Backoff {
                    delay,
                    class,
                    attempt: self.total,
                    of: self.of(),
                },
                Some(class),
            );
        }

        if self.policy.is_some()
            && class.cools_down()
            && let Some(model) = self.model.as_deref()
        {
            RetryPolicy::start_sdk_cooldown(self.format, self.sdk, model);
        }
        (RetryAction::GiveUp, Some(class))
    }

    /// Classify a failure without applying the retry decision, for the
    /// streamed-output barrier which must never re-run a turn.
    pub(crate) fn classify_failure(&self, failure: Failure<'_>) -> Option<RetryClass> {
        self.classify(failure)
    }

    /// Retryable class of `failure`: without a policy only a backend-reported
    /// limit qualifies, which is what cruise retried before the fallback engine
    /// existed.
    fn classify(&self, failure: Failure<'_>) -> Option<RetryClass> {
        match failure {
            // The backend tagged this as a limit; context/output exhaustion
            // must not inherit the retryable default. Otherwise classify the
            // provider wording so overloaded errors retain their server label.
            Failure::Limited(message) => classify_limited_message(message),
            Failure::Failed(message) => self
                .policy
                .as_deref()
                .and_then(|_| classify_retryable(message)),
            Failure::Unusable(_) => self.policy.as_deref().map(|_| RetryClass::ModelUnusable),
        }
    }

    /// Delay before the next same-model attempt, or `None` when it would exceed
    /// the policy's `max_delay_ms` — which sends the run to its next fallback
    /// model instead.
    fn delay_for(&self, message: &str, attempt: usize) -> Option<Duration> {
        let Some(policy) = self.policy.as_deref() else {
            return Some(crate::step::command::calculate_backoff(attempt));
        };
        let config = &policy.config;
        let delay = parse_retry_after(message)
            .unwrap_or_else(|| backoff_delay(config.base_delay_ms, attempt, jitter_permille()));
        (delay <= Duration::from_millis(config.max_delay_ms)).then_some(delay)
    }

    /// Move to the next usable chain entry, cooling down the model left behind
    /// — except after a [`RetryClass::ClientError`] (see
    /// [`FallbackEngine::next_with_class`]).
    fn switch(&mut self, class: RetryClass, message: &str) -> Option<RetryAction> {
        let previous_sdk = self.sdk;
        let to = self.take_candidate()?;
        let from = self.model.replace(to.clone());
        if self.policy.is_some()
            && class.cools_down()
            && let Some(previous) = from.as_deref()
        {
            RetryPolicy::start_sdk_cooldown(self.format, previous_sdk, previous);
        }
        self.attempts = 0;
        self.total += 1;
        Some(RetryAction::Switch {
            from,
            to,
            detail: failure_detail(class, message),
            attempt: self.total,
            of: self.of(),
        })
    }

    /// Next chain entry that is neither disabled nor cooling down.
    fn take_candidate(&mut self) -> Option<String> {
        self.policy.as_ref()?;
        while let Some((sdk, candidate)) = self.chain.pop_front() {
            if !RetryPolicy::is_sdk_cooling(self.format, sdk, &candidate)
                && self
                    .availability
                    .disabled_by(sdk, &candidate, unix_seconds())
                    .is_none()
            {
                self.sdk = sdk;
                return Some(candidate);
            }
        }
        None
    }

    /// Notice denominator: the run's retry budget, never below the retries
    /// already spent (an unusable model reference switches without spending
    /// any).
    fn of(&self) -> usize {
        self.budget.max(self.total)
    }
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_selection::{ModelCandidate, Sdk};

    #[test]
    fn disabled_window_startup_reason_and_policy_independence() {
        let spec: crate::config::ModelSpec = serde_json::from_value(serde_json::json!([
            {"model": "test-retry-window/primary:high", "disabled_during": ["00:00-24:00"]},
            "test-retry-window/spare"
        ]))
        .unwrap_or_else(|error| panic!("schedule fixture: {error}"));
        let availability = crate::model_selection::availability(Some(&spec), Sdk::Jcode)
            .unwrap_or_else(|error| panic!("schedule availability: {error}"));
        let candidates = crate::model_selection::resolve_candidates(Some(&spec), Sdk::Jcode);
        let mut engine = FallbackEngine::new_candidates(
            availability.clone(),
            Some(Arc::new(RetryPolicy::new(policy(&[])))),
            Sdk::Jcode,
            Sdk::Jcode,
            candidates[0].model.as_deref(),
            &candidates[1..],
            0,
        );
        assert!(
            matches!(engine.startup_reason(), Some(Unavailable::TimeWindow(raw)) if raw == "00:00-24:00")
        );
        assert_eq!(
            engine.take_startup_switch(),
            Some((
                "test-retry-window/primary:high".to_owned(),
                "test-retry-window/spare".to_owned()
            ))
        );
        assert!(!engine.startup_blocked());
        assert!(!RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-retry-window/primary"
        ));
        let engine = FallbackEngine::new_candidates(
            availability,
            None,
            Sdk::Jcode,
            Sdk::Jcode,
            candidates[0].model.as_deref(),
            &[],
            0,
        );
        assert!(engine.startup_blocked());
        assert!(matches!(
            engine.startup_reason(),
            Some(Unavailable::TimeWindow(_))
        ));
    }

    #[test]
    fn opencode_common_effort_chain_precedence_and_wildcards() {
        let format = ModelReferenceFormat::OpenCode;
        let mut config = policy(&[
            ("source/team/model:free:max", &["exact/*"]),
            ("source/team/model:free", &["base/*"]),
            ("source/*", &["provider/*"]),
            ("default", &["default/*"]),
        ]);
        for (key, expected) in [
            ("source/team/model:free:max", "exact/team/model:free:max"),
            ("source/team/model:free", "base/team/model:free:max"),
            ("source/*", "provider/team/model:free:max"),
            ("default", "default/team/model:free:max"),
        ] {
            assert_eq!(
                chain_for(format, &config, Some("source/team/model:free:max")),
                [expected],
                "chain selected before removing {key}"
            );
            config.fallback_chains.remove(key);
        }
        for (source, target) in [
            (format, format),
            (format, ModelReferenceFormat::LegacyEffort),
            (ModelReferenceFormat::LegacyEffort, format),
        ] {
            assert_eq!(
                chain_for_formats(
                    source,
                    target,
                    &policy(&[("default", &["other/*"])]),
                    Some("source/team/model:2026:MED"),
                    false,
                ),
                ["other/team/model:2026:MED"],
                "common effort must not become a native variant"
            );
        }
    }

    #[test]
    fn opencode_common_effort_and_native_variants_share_only_same_sdk_identity() {
        let candidates = [
            (Sdk::OpenCode, "effort-identity/main:high"),
            (Sdk::OpenCode, "effort-identity/main#max"),
            (Sdk::Jcode, "effort-identity/main:low"),
            (Sdk::OpenCode, "effort-identity/spare:high"),
            (Sdk::OpenCode, "effort-identity/spare:max"),
            (Sdk::OpenCode, "effort-identity/spare#max"),
        ]
        .map(|(sdk, model)| ModelCandidate {
            sdk,
            model: Some(model.into()),
        });
        let mut engine = FallbackEngine::new_candidates(
            ModelAvailability::default(),
            Some(Arc::new(RetryPolicy::new(policy(&[])))),
            Sdk::OpenCode,
            Sdk::OpenCode,
            Some("effort-identity/main:max"),
            &candidates,
            1,
        );
        let failure = Failure::Failed("HTTP 422 request refused");
        for (sdk, model) in [
            (Sdk::Jcode, "effort-identity/main:low"),
            (Sdk::OpenCode, "effort-identity/spare:high"),
        ] {
            let (action, _) = engine.next_with_class(failure, false);
            assert!(
                matches!(action, RetryAction::Switch { ref to, .. } if to == model),
                "{action:?}"
            );
            assert_eq!(engine.sdk(), sdk);
            assert_eq!(engine.model(), Some(model));
        }
        assert_eq!(
            engine.next_with_class(failure, false).0,
            RetryAction::GiveUp
        );
    }

    #[test]
    fn opencode_no_route_classification_is_header_scoped_and_narrow() {
        for body in [
            "Model unavailable: provider/model",
            "not found",
            "no matching route",
        ] {
            let message = format!("OpenCode model execution failed [provider.no-route]: {body}");
            assert_eq!(classify_retryable(&message), Some(RetryClass::ModelMissing));
        }
        for message in [
            "Model unavailable: provider/model",
            "OpenCode model execution failed [provider.auth]: Model unavailable: provider/model",
            "OpenCode model execution failed [provider.api]: Model unavailable: provider/model",
            "Other model execution failed [provider.no-route]: Model unavailable: provider/model",
            "prefix OpenCode model execution failed [provider.no-route]: Model unavailable",
            "OpenCode model execution failed [provider.no-route-extra]: Model unavailable",
            "execution failed\nopencode stderr:\nOpenCode model execution failed [provider.no-route]: Model unavailable",
        ] {
            assert_eq!(classify_retryable(message), None, "for {message}");
        }
        // Even generic missing-model markers must not bypass this header's auth guard.
        for body in [
            "HTTP 401 model not found",
            "HTTP 403 unknown model",
            "HTTP 407 model_not_supported",
            "authentication failed: model not found",
            "permission denied: unknown model",
            "invalid_request_error: model not found",
        ] {
            let message = format!("OpenCode model execution failed [provider.no-route]: {body}");
            assert_eq!(classify_retryable(&message), None, "for {message}");
        }
        for (status, expected) in [
            ("429 Too Many Requests", RetryClass::RateLimit),
            ("408 Request Timeout", RetryClass::Network),
            ("500 Internal Server Error", RetryClass::ServerError),
            ("503 Service Unavailable", RetryClass::ServerError),
        ] {
            let message = format!(
                "OpenCode model execution failed [provider.no-route]: HTTP {status}: Model unavailable"
            );
            assert_eq!(
                classify_retryable(&message),
                Some(expected),
                "for {message}"
            );
        }
    }

    #[test]
    fn opencode_no_route_switches_immediately_and_cools_all_effort_variants() {
        let format = ModelReferenceFormat::OpenCode;
        let config = policy(&[("no-route-cool/main", &["no-route-cool/spare:max"])]);
        let mut first = FallbackEngine::new(
            format,
            Some(Arc::new(RetryPolicy::new(config.clone()))),
            Some("no-route-cool/main:max"),
            1,
        );
        let message = "OpenCode model execution failed [provider.no-route]: Model unavailable: no-route-cool/main";
        let (action, class) = first.next_with_class(Failure::Failed(message), false);
        assert_eq!(class, Some(RetryClass::ModelMissing));
        let RetryAction::Switch {
            from, to, attempt, ..
        } = action
        else {
            panic!("expected immediate switch, got {action:?}");
        };
        assert_eq!(from.as_deref(), Some("no-route-cool/main:max"));
        assert_eq!(to, "no-route-cool/spare:max");
        assert_eq!(attempt, 1);
        for suffix in [":high", ":max", "#max"] {
            assert!(RetryPolicy::is_cooling(
                format,
                &format!("no-route-cool/main{suffix}")
            ));
        }
        assert!(!RetryPolicy::is_sdk_cooling(
            format,
            Sdk::Jcode,
            "no-route-cool/main:max"
        ));
        let later = FallbackEngine::new(
            format,
            Some(Arc::new(RetryPolicy::new(config))),
            Some("no-route-cool/main#max"),
            1,
        );
        assert_eq!(later.model(), Some("no-route-cool/spare:max"));
    }

    #[test]
    fn opencode_no_route_does_not_switch_without_policy_budget_or_replay_safety() {
        let message =
            "OpenCode model execution failed [provider.no-route]: Model unavailable: primary";
        for case in [
            "no-policy",
            "disabled",
            "no-candidate",
            "zero-budget",
            "visible-output",
        ] {
            let primary = format!("no-route-exclusion-{case}/main:max");
            let mut config = policy(&[("default", &["no-route-exclusion/spare"])]);
            if case == "disabled" {
                config.model_fallback = false;
            }
            if case == "no-candidate" {
                config.fallback_chains.clear();
            }
            let mut engine = FallbackEngine::new(
                ModelReferenceFormat::OpenCode,
                (case != "no-policy").then(|| Arc::new(RetryPolicy::new(config))),
                Some(&primary),
                if case == "zero-budget" { 0 } else { 2 },
            );
            let failure = Failure::Failed(message);
            assert_eq!(failure.message(), message);
            let (action, class) = engine.next_with_class(failure, case == "visible-output");
            assert_eq!(action, RetryAction::GiveUp, "for {case}");
            assert_eq!(engine.model(), Some(primary.as_str()), "for {case}");
            assert_eq!(
                class,
                (case != "no-policy").then_some(RetryClass::ModelMissing)
            );
        }
    }

    #[test]
    fn explicit_chain_lookup_precedence_includes_present_empty_entries() {
        let mut config = policy(&[
            ("provider/main:high", &[]),
            ("provider/main", &["base"]),
            ("provider/*", &["wildcard"]),
            ("default", &["default"]),
        ]);
        let format = ModelReferenceFormat::LegacyEffort;
        for (key, expected) in [
            ("provider/main:high", Vec::<String>::new()),
            ("provider/main", vec!["base".into()]),
            ("provider/*", vec!["wildcard".into()]),
            ("default", vec!["default".into()]),
        ] {
            assert_eq!(
                explicit_chain(format, &config, Some("provider/main:high")),
                Some(&expected)
            );
            config.fallback_chains.remove(key);
        }
        assert_eq!(
            explicit_chain(format, &config, Some("provider/main:high")),
            None
        );
        config
            .fallback_chains
            .insert("default".into(), vec!["next/*".into()]);
        assert!(explicit_chain(format, &config, None).is_some());
        assert!(chain_for(format, &config, None).is_empty());
    }

    #[test]
    fn mixed_sdk_normalization_uses_candidate_owner_not_workflow() {
        // Recognized effort suffixes are reserved, so literal colon IDs paired
        // with native variants use unknown suffixes instead of :low/:high.
        let candidates = [
            (Sdk::Jcode, "normalize/jcode:low"),
            (Sdk::Jcode, "normalize/jcode:high"),
            (Sdk::Claude, "normalize/claude:low"),
            (Sdk::Claude, "normalize/claude:high"),
            (Sdk::OpenCode, "normalize/open:free#fast"),
            (Sdk::OpenCode, "normalize/open:free#slow"),
            (Sdk::OpenCode, "normalize/open:2026#fast"),
        ]
        .map(|(sdk, model)| ModelCandidate {
            sdk,
            model: Some(model.into()),
        });
        for workflow in [Sdk::OpenCode, Sdk::Jcode] {
            let engine = FallbackEngine::new_candidates(
                ModelAvailability::default(),
                Some(Arc::new(RetryPolicy::new(policy(&[])))),
                Sdk::Claude,
                workflow,
                Some("normalize/primary"),
                &candidates,
                1,
            );
            assert_eq!(
                engine.chain.iter().cloned().collect::<Vec<_>>(),
                vec![
                    (Sdk::Jcode, "normalize/jcode:low".into()),
                    (Sdk::Claude, "normalize/claude:low".into()),
                    (Sdk::OpenCode, "normalize/open:free#fast".into()),
                    (Sdk::OpenCode, "normalize/open:2026#fast".into()),
                ]
            );
        }
    }

    #[test]
    fn cooldown_identity_survives_workflow_sdk_changes() {
        let policy = Some(Arc::new(RetryPolicy::new(policy(&[]))));
        let mut first = FallbackEngine::new_candidates(
            ModelAvailability::default(),
            policy.clone(),
            Sdk::Jcode,
            Sdk::OpenCode,
            Some("cross-workflow/cooldown:high"),
            &[],
            1,
        );
        assert_eq!(
            first
                .next_with_class(Failure::Failed("404 model not found"), false)
                .0,
            RetryAction::GiveUp
        );
        let later = FallbackEngine::new_candidates(
            ModelAvailability::default(),
            policy,
            Sdk::Jcode,
            Sdk::Claude,
            Some("cross-workflow/cooldown:low"),
            &[],
            1,
        );
        assert!(later.startup_blocked());
    }

    #[test]
    fn qualified_primary_keys_and_string_entries_use_their_own_syntax() {
        for key in [
            "qualified/main:free#fast",
            "qualified/main:free",
            "qualified/*",
            "default",
        ] {
            let config = policy(&[(key, &["entry/other:high", "entry/other:low"])]);
            let engine = FallbackEngine::new_candidates(
                ModelAvailability::default(),
                Some(Arc::new(RetryPolicy::new(config))),
                Sdk::OpenCode,
                Sdk::Claude,
                Some("qualified/main:free#fast"),
                &[],
                1,
            );
            assert_eq!(
                engine.chain.iter().cloned().collect::<Vec<_>>(),
                vec![(Sdk::Claude, "entry/other:high".into())]
            );
        }
    }

    #[test]
    fn mixed_sdk_identity_keeps_same_name_and_isolates_cooldowns() {
        let model = "test-sdk-identity/shared";
        let candidates = [
            ModelCandidate {
                sdk: Sdk::Claude,
                model: Some(format!("{model}:low")),
            },
            ModelCandidate {
                sdk: Sdk::Jcode,
                model: Some(format!("{model}:high")),
            },
        ];
        let policy = Some(Arc::new(RetryPolicy::new(policy(&[]))));
        let mut engine = FallbackEngine::new_candidates(
            ModelAvailability::default(),
            policy.clone(),
            Sdk::Jcode,
            Sdk::Jcode,
            Some(model),
            &candidates,
            1,
        );
        assert!(matches!(
            engine
                .next_with_class(Failure::Failed("404 model not found"), false)
                .0,
            RetryAction::Switch { .. }
        ));
        assert_eq!(engine.sdk(), Sdk::Claude);
        assert_eq!(engine.model(), Some("test-sdk-identity/shared:low"));
        assert!(RetryPolicy::is_sdk_cooling(
            ModelReferenceFormat::LegacyEffort,
            Sdk::Jcode,
            model
        ));
        assert!(!RetryPolicy::is_sdk_cooling(
            ModelReferenceFormat::LegacyEffort,
            Sdk::Claude,
            model
        ));
        let later = FallbackEngine::new_candidates(
            ModelAvailability::default(),
            policy,
            Sdk::Claude,
            Sdk::Claude,
            Some(model),
            &[],
            1,
        );
        assert!(!later.startup_blocked());
    }

    #[test]
    fn implicit_chains_are_call_local_and_explicit_matches_override_every_tail() {
        let primary = "test-sdk-scopes/primary:high";
        let exec = [ModelCandidate {
            sdk: Sdk::Claude,
            model: Some("exec-tail".to_owned()),
        }];
        let plan = [ModelCandidate {
            sdk: Sdk::Jcode,
            model: Some("plan-tail".to_owned()),
        }];
        let default = Some(Arc::new(RetryPolicy::new(policy(&[]))));
        for (tail, expected_sdk, expected_model) in [
            (&exec, Sdk::Claude, "exec-tail"),
            (&plan, Sdk::Jcode, "plan-tail"),
        ] {
            let mut engine = FallbackEngine::new_candidates(
                ModelAvailability::default(),
                default.clone(),
                Sdk::Jcode,
                Sdk::Jcode,
                Some(primary),
                tail,
                1,
            );
            assert!(matches!(
                engine
                    .next_with_class(Failure::Failed("HTTP 400 bad request"), false)
                    .0,
                RetryAction::Switch { .. }
            ));
            assert_eq!(
                (engine.sdk(), engine.model()),
                (expected_sdk, Some(expected_model))
            );
        }
        for key in [
            primary,
            "test-sdk-scopes/primary",
            "test-sdk-scopes/*",
            DEFAULT_CHAIN_KEY,
        ] {
            let explicit = Some(Arc::new(RetryPolicy::new(policy(&[(
                key,
                &["explicit-tail"],
            )]))));
            let mut engine = FallbackEngine::new_candidates(
                ModelAvailability::default(),
                explicit,
                Sdk::Claude,
                Sdk::Jcode,
                Some(primary),
                &exec,
                1,
            );
            assert!(matches!(
                engine
                    .next_with_class(Failure::Failed("HTTP 400 bad request"), false)
                    .0,
                RetryAction::Switch { .. }
            ));
            assert_eq!(
                (engine.sdk(), engine.model()),
                (Sdk::Jcode, Some("explicit-tail"))
            );
        }
    }

    fn policy(chains: &[(&str, &[&str])]) -> RetryConfig {
        RetryConfig {
            base_delay_ms: 500,
            max_delay_ms: 300_000,
            model_fallback: true,
            fallback_chains: chains
                .iter()
                .map(|(key, entries)| {
                    (
                        (*key).to_string(),
                        entries.iter().map(|e| (*e).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn active_policy_isolated_between_concurrent_run_scopes() {
        let first = policy_for_config(Some(policy(&[(
            "test-scope/first",
            &["test-scope/first-fallback"],
        )])))
        .unwrap_or_else(|| panic!("policy should exist"));
        let second = policy_for_config(Some(policy(&[(
            "test-scope/second",
            &["test-scope/second-fallback"],
        )])))
        .unwrap_or_else(|| panic!("policy should exist"));
        let gate = Arc::new(tokio::sync::Barrier::new(2));

        let first_seen = {
            let gate = Arc::clone(&gate);
            let first = Arc::clone(&first);
            with_active_policy(Some(first), async move {
                gate.wait().await;
                tokio::task::yield_now().await;
                active_policy()
            })
        };
        let second_seen = {
            let gate = Arc::clone(&gate);
            let second = Arc::clone(&second);
            with_active_policy(Some(second), async move {
                gate.wait().await;
                tokio::task::yield_now().await;
                active_policy()
            })
        };
        let (first_seen, second_seen) = tokio::join!(first_seen, second_seen);

        assert!(Arc::ptr_eq(
            &first,
            &first_seen.unwrap_or_else(|| panic!("first policy missing"))
        ));
        assert!(Arc::ptr_eq(
            &second,
            &second_seen.unwrap_or_else(|| panic!("second policy missing"))
        ));
    }

    fn engine(config: RetryConfig, model: &str, max_retries: usize) -> FallbackEngine {
        FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            Some(Arc::new(RetryPolicy::new(config))),
            Some(model),
            max_retries,
        )
    }

    // --- classify_retryable ---------------------------------------------------

    #[test]
    fn retryable_classes_cover_limits_server_errors_and_network_failures() {
        for (message, expected) in [
            ("API Error: 429 rate limit exceeded", RetryClass::RateLimit),
            ("Too Many Requests", RetryClass::RateLimit),
            ("5-hour session limit reached", RetryClass::RateLimit),
            ("you hit your usage limit", RetryClass::RateLimit),
            ("HTTP 500 while calling the API", RetryClass::ServerError),
            ("upstream returned 502", RetryClass::ServerError),
            ("status 503", RetryClass::ServerError),
            ("Error: 504 gateway timeout", RetryClass::ServerError),
            (
                "Provider returned: overloaded_error",
                RetryClass::ServerError,
            ),
            ("service unavailable", RetryClass::ServerError),
            ("connection refused", RetryClass::Network),
            ("socket hang up", RetryClass::Network),
            ("fetch failed", RetryClass::Network),
            ("stream terminated", RetryClass::Network),
            ("request timed out", RetryClass::Network),
        ] {
            assert_eq!(classify_retryable(message), Some(expected), "for {message}");
        }
    }

    #[test]
    fn server_status_detail_includes_any_retryable_5xx_code() {
        assert_eq!(
            failure_detail(RetryClass::ServerError, "HTTP status 501"),
            "501"
        );
    }

    #[test]
    fn retryable_classes_cover_client_errors_except_rate_limits() {
        for (message, expected) in [
            ("API Error: HTTP 400 bad request", RetryClass::ClientError),
            ("status 404 model_not_found", RetryClass::ModelMissing),
            ("upstream returned 413", RetryClass::ClientError),
            // A named 4xx beats the permanent wordings: another model may
            // accept the request the provider just refused.
            (
                "API Error: 400 invalid_request: context length exceeded",
                RetryClass::ClientError,
            ),
            // 429 stays a rate limit, whatever else the text says.
            ("HTTP status 429", RetryClass::RateLimit),
            ("API Error: 429 rate limit exceeded", RetryClass::RateLimit),
        ] {
            assert_eq!(classify_retryable(message), Some(expected), "for {message}");
        }
    }

    #[test]
    fn credential_and_permission_statuses_are_never_retryable() {
        for message in [
            // Given a credential or permission failure, no other model can
            // supply what the run is missing. None of these carries a
            // permanent wording, so only the excluded-code list keeps them
            // out of the client-error class.
            "HTTP status 401",
            "upstream returned 403",
            "status 407",
        ] {
            assert_eq!(classify_retryable(message), None, "for {message}");
        }
    }

    #[test]
    fn request_timeout_status_is_a_network_failure() {
        // Given a 408, the path timed out rather than the request being
        // refused, so it joins the transport failures rather than the 4xx.
        assert_eq!(
            classify_retryable("API Error: HTTP 408 request timeout"),
            Some(RetryClass::Network)
        );
    }

    #[test]
    fn appended_stderr_numbers_do_not_hijack_the_named_status() {
        // The jcode backend appends a stderr tail to the failure text, so a
        // Node stack frame and a `retry-after-ms` hint travel with the real
        // status. Neither may demote the failure to a client error, which
        // would stop the run waiting out a 429 or a 5xx.
        for (message, expected) in [
            (
                "API Error: 429 Too Many Requests\njcode stderr:\nat fetch (node:internal/errors:496:11)",
                RetryClass::RateLimit,
            ),
            (
                "Error: 500 internal server error, retry-after-ms: 400",
                RetryClass::ServerError,
            ),
        ] {
            assert_eq!(classify_retryable(message), Some(expected), "for {message}");
        }
    }

    #[test]
    fn source_positions_and_url_ports_are_not_status_codes() {
        // Both shapes sit within a marker window of the word `error`, so only
        // their punctuation tells them apart from a real status.
        assert!(!has_status_code(
            "fetch failed: https://api.example.com:443/v1/messages",
            "443"
        ));
        assert!(!has_status_code(
            "uncaught error at /app/errors.js:404:17",
            "404"
        ));
        assert_eq!(
            classify_retryable("fetch failed: https://api.example.com:443/v1/messages"),
            Some(RetryClass::Network)
        );

        // The wordings a provider really uses keep classifying as before.
        for message in ["status 404: not found", "HTTP/1.1 400 bad request"] {
            assert_eq!(
                classify_retryable(message),
                Some(RetryClass::ClientError),
                "for {message}"
            );
        }
    }

    #[test]
    fn client_status_detail_names_the_refused_code() {
        assert_eq!(
            failure_detail(RetryClass::ClientError, "API Error: HTTP 400 bad request"),
            "400"
        );
        assert_eq!(
            failure_detail(RetryClass::ModelMissing, "status 404 model_not_found"),
            "404"
        );
        // The notice must name the code that produced the class, not a 5xx
        // the provider mentioned in passing.
        assert_eq!(
            failure_detail(
                RetryClass::ClientError,
                "API Error: 400 bad request (upstream 503)"
            ),
            "400"
        );
    }

    #[test]
    fn request_timeout_detail_names_the_status_not_the_class() {
        assert_eq!(
            failure_detail(RetryClass::Network, "API Error: HTTP 408 request timeout"),
            "408"
        );
        // A transport failure that names no status still falls back to the
        // class label.
        assert_eq!(
            failure_detail(RetryClass::Network, "socket hang up"),
            "network error"
        );
    }

    #[test]
    fn client_error_classification_needs_a_real_status_code() {
        for message in [
            // Permanent wordings without a status code stay permanent.
            "invalid request: the model does not support tools",
            "error: unknown option '--effort'",
            // Numbers that only look like status codes are not statuses.
            "max_tokens: 5000 is above the model maximum",
            "context length 500000 exceeded",
            "model claude-opus-4-6 refused the 400 token continuation",
        ] {
            assert_eq!(classify_retryable(message), None, "for {message}");
        }
    }

    #[test]
    fn context_and_output_exhaustion_limits_are_not_retried_as_rate_limits() {
        for message in [
            "Context limit exceeded after 3 compaction retries",
            "The provider stopped with max_tokens after automatic continuation attempts were exhausted. Output may be incomplete.",
            "The provider stopped with length after automatic continuation attempts were exhausted. Output may be incomplete.",
            "max_output_tokens exhausted",
        ] {
            assert!(!is_limit_message(message), "expected non-limit: {message}");
            assert_eq!(classify_retryable(message), None, "for {message}");
            let mut engine = engine(policy(&[]), "test-limit-exhaustion/model", 1);
            assert_eq!(
                engine.next_with_class(Failure::Limited(message), false).0,
                RetryAction::GiveUp,
                "for {message}"
            );
        }

        let mut engine = engine(policy(&[]), "test-genuine-rate-limit/model", 1);
        assert!(matches!(
            engine
                .next_with_class(Failure::Limited("HTTP 429 Too Many Requests"), false)
                .0,
            RetryAction::Backoff {
                class: RetryClass::RateLimit,
                ..
            }
        ));
    }

    #[test]
    fn provider_missing_model_errors_use_v088_formats() {
        for message in [
            "anthropic api error (404 not found): {\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"claude fable 5 is not available. please use opus 4.8.\"}}",
            "OpenAI-compatible chat request failed\n  model: openai/gpt-missing\n  status: 404 Not Found\n  response: {\"error\":{\"code\":\"model_not_found\",\"message\":\"The requested model was not found\"}}",
            "Gemini request generateContent failed (HTTP 404 Not Found): {\"error\":{\"status\":\"NOT_FOUND\",\"message\":\"Requested entity was not found.\"}}",
        ] {
            assert_eq!(
                classify_retryable(message),
                Some(RetryClass::ModelMissing),
                "for {message}"
            );
        }
    }

    #[test]
    fn retry_classification_rejects_permanent_failures() {
        for message in [
            "authentication_error: invalid API key",
            "invalid_request_error: unsupported parameter 'reasoning'",
            "context length exceeded: 500000 tokens > limit",
            "max_tokens must be <= 5000",
            "Context limit exceeded after 3 compaction retries",
            "The provider stopped with length after automatic continuation attempts were exhausted. Output may be incomplete.",
            "insufficient credit balance",
            // A permanent failure whose text also carries a status code and a
            // transport-looking word must stay permanent.
            "invalid_request_error: max_tokens 500 exceeds the limit",
            "authentication timed out while refreshing the OAuth token",
            "tool call terminated: invalid arguments",
        ] {
            assert_eq!(classify_retryable(message), None, "for {message}");
        }
    }

    #[test]
    fn limit_messages_are_one_shared_predicate_for_both_sdk_backends() {
        for message in [
            "API Error: 429 Rate Limit exceeded",
            "usage limit reached",
            "Too Many Requests",
            "5-hour session limit reached",
            // The transient capacity refusal, retryable on both backends.
            "Provider returned: overloaded_error",
        ] {
            assert!(is_limit_message(message), "expected limit: {message}");
        }
        for message in [
            "invalid_request_error: model not found",
            "authentication_error: invalid API key",
            "prompt is too long: 250000 tokens > 200000 maximum",
            "error: unknown option '--effort'",
            "HTTP 503 service unavailable",
        ] {
            assert!(!is_limit_message(message), "expected non-limit: {message}");
        }
    }

    // --- backoff / hints ------------------------------------------------------

    #[test]
    fn retry_backoff_doubles_up_to_the_eight_second_ceiling() {
        assert_eq!(backoff_delay(500, 1, 1_000), Duration::from_millis(500));
        assert_eq!(backoff_delay(500, 2, 1_000), Duration::from_millis(1_000));
        assert_eq!(backoff_delay(500, 5, 1_000), Duration::from_millis(8_000));
        assert_eq!(backoff_delay(500, 40, 1_000), Duration::from_millis(8_000));
        // Jitter only ever shortens the delay.
        assert_eq!(backoff_delay(500, 2, 750), Duration::from_millis(750));
    }

    #[test]
    fn retry_after_hint_is_parsed_and_clamped_to_a_minute() {
        assert_eq!(
            parse_retry_after("429; retry-after: 12"),
            Some(Duration::from_secs(12))
        );
        assert_eq!(
            parse_retry_after(r#"{"retry-after-ms":"1500"}"#),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(
            parse_retry_after("Retry-After: 3600"),
            Some(Duration::from_secs(60))
        );
        assert_eq!(parse_retry_after("rate limit exceeded"), None);
        // A zero hint is no hint: an immediate re-send would burn the budget.
        assert_eq!(parse_retry_after("retry-after: 0"), None);
    }

    #[test]
    fn retry_after_hint_beats_the_computed_backoff() {
        let mut config = policy(&[]);
        config.base_delay_ms = 500;
        let mut engine = engine(config, "test-hint/model", 1);
        assert_eq!(
            engine
                .next_with_class(Failure::Limited("429; retry-after: 30"), false)
                .0,
            RetryAction::Backoff {
                delay: Duration::from_secs(30),
                class: RetryClass::RateLimit,
                attempt: 1,
                of: 1,
            }
        );
    }

    #[test]
    fn retry_after_hint_above_max_delay_ms_is_refused() {
        let mut config = policy(&[]);
        config.max_delay_ms = 5_000;
        let mut engine = engine(config, "test-hint2/model", 3);
        assert_eq!(
            engine
                .next_with_class(Failure::Limited("429; retry-after: 30"), false)
                .0,
            RetryAction::GiveUp
        );
    }

    // --- chain selection ------------------------------------------------------

    #[test]
    fn retry_chain_selection_prefers_the_most_specific_key() {
        let config = policy(&[
            ("default", &["fallback/default-model"]),
            ("anthropic/*", &["fallback/by-provider"]),
            ("anthropic/opus", &["fallback/by-model"]),
        ]);
        let chain = chain_for(
            ModelReferenceFormat::LegacyEffort,
            &config,
            Some("anthropic/opus"),
        );
        assert_eq!(chain, ["fallback/by-model"]);
        let chain = chain_for(
            ModelReferenceFormat::LegacyEffort,
            &config,
            Some("anthropic/haiku"),
        );
        assert_eq!(chain, ["fallback/by-provider"]);
        let chain = chain_for(
            ModelReferenceFormat::LegacyEffort,
            &config,
            Some("openai/gpt-5.5"),
        );
        assert_eq!(chain, ["fallback/default-model"]);
        let chain = chain_for(ModelReferenceFormat::LegacyEffort, &config, None);
        assert_eq!(chain, ["fallback/default-model"]);
    }

    #[test]
    fn retry_chain_wildcard_entry_keeps_the_model_and_effort() {
        let config = policy(&[("default", &["openrouter/*"])]);
        let chain = chain_for(
            ModelReferenceFormat::LegacyEffort,
            &config,
            Some("anthropic/claude-opus-4-6:high"),
        );
        assert_eq!(chain, ["openrouter/claude-opus-4-6:high"]);
    }

    #[test]
    fn retry_opencode_chain_preserves_nested_colon_ids_and_variants() {
        let config = policy(&[
            ("default", &["fallback/default"]),
            ("source/*", &["wildcard/*"]),
            ("source/team/model:free", &["base/*"]),
            (
                "source/team/model:free#fast",
                &[
                    "source/team/model:free#slow",
                    "exact/*",
                    "exact/team/model:free#slow",
                    "exact/team/model:2026#fast",
                ],
            ),
        ]);
        let format = ModelReferenceFormat::OpenCode;
        assert_eq!(
            chain_for(format, &config, Some("source/team/model:free#fast")),
            ["exact/team/model:free#fast", "exact/team/model:2026#fast"]
        );
        assert_eq!(
            chain_for(format, &config, Some("source/team/model:free#slow")),
            ["base/team/model:free#slow"]
        );
        assert_eq!(
            chain_for(format, &config, Some("source/other/id:free#fast")),
            ["wildcard/other/id:free#fast"]
        );
        assert_eq!(chain_for(format, &config, None), ["fallback/default"]);
    }

    #[test]
    fn retry_opencode_cooldowns_separate_formats_and_preserve_colons() {
        let format = ModelReferenceFormat::OpenCode;
        RetryPolicy::start_cooldown(format, "test-opencode-cool/team/model:free#fast");
        assert!(RetryPolicy::is_cooling(
            format,
            "test-opencode-cool/team/model:free#slow"
        ));
        assert!(!RetryPolicy::is_cooling(
            format,
            "test-opencode-cool/team/model:2026#fast"
        ));
        assert!(!RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-opencode-cool/team/model:free#fast"
        ));
        let config = policy(&[("default", &["replacement/*"])]);
        let engine = FallbackEngine::new(
            format,
            Some(Arc::new(RetryPolicy::new(config))),
            Some("test-opencode-cool/team/model:free#slow"),
            1,
        );
        assert_eq!(engine.model(), Some("replacement/team/model:free#slow"));
    }

    #[test]
    fn retry_chain_drops_same_model_effort_variants() {
        let config = policy(&[(
            "test-effort/primary",
            &["test-effort/primary:low", "test-effort/spare"],
        )]);
        let chain = chain_for(
            ModelReferenceFormat::LegacyEffort,
            &config,
            Some("test-effort/primary:high"),
        );
        assert_eq!(chain, ["test-effort/spare"]);
    }

    // --- engine ---------------------------------------------------------------

    #[test]
    fn retry_rate_limit_switches_immediately_and_cools_the_model_down() {
        let config = policy(&[(
            "test-a/model-429",
            &["test-b/model-429", "test-c/model-429"],
        )]);
        let mut engine = engine(config, "test-a/model-429", 1);
        assert_eq!(engine.model(), Some("test-a/model-429"));

        // No same-model Backoff first: the first 429 moves to the spare.
        let (action, class) = engine.next_with_class(Failure::Limited("HTTP status 429"), false);
        assert_eq!(class, Some(RetryClass::RateLimit));
        assert!(
            matches!(
                &action,
                RetryAction::Switch { from, to, attempt: 1, .. }
                    if from.as_deref() == Some("test-a/model-429") && to == "test-b/model-429"
            ),
            "unexpected action: {action:?}"
        );
        assert_eq!(engine.model(), Some("test-b/model-429"));
        assert_eq!(engine.attempts, 0, "the new model gets a fresh budget");
        assert!(RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-a/model-429"
        ));
    }

    #[test]
    fn retry_rate_limit_with_long_retry_after_switches_without_a_delay() {
        let config = policy(&[("test-ra/primary", &["test-ra/spare"])]);
        let mut engine = engine(config, "test-ra/primary", 3);
        let (action, _) = engine.next_with_class(Failure::Limited("429; retry-after: 60"), false);
        assert!(
            matches!(&action, RetryAction::Switch { to, .. } if to == "test-ra/spare"),
            "unexpected action: {action:?}"
        );
    }

    #[test]
    fn retry_rate_limit_chain_advances_past_each_limited_model() {
        let config = policy(&[("test-chain/a", &["test-chain/b", "test-chain/c"])]);
        let mut engine = engine(config, "test-chain/a", 1);
        for expected in ["test-chain/b", "test-chain/c"] {
            let (action, _) = engine.next_with_class(Failure::Limited("429"), false);
            assert!(
                matches!(&action, RetryAction::Switch { to, .. } if to == expected),
                "unexpected action: {action:?}"
            );
        }
        // Terminal model: the old same-model budget, then the failure.
        assert!(matches!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::Backoff { .. }
        ));
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-chain/c"));
    }

    #[test]
    fn retry_rate_limit_skips_cooling_candidates() {
        RetryPolicy::start_cooldown(ModelReferenceFormat::LegacyEffort, "test-skip/cooling");
        let config = policy(&[(
            "test-skip/primary",
            &["test-skip/cooling", "test-skip/open"],
        )]);
        let mut engine = engine(config, "test-skip/primary", 2);
        let (action, _) = engine.next_with_class(Failure::Limited("429"), false);
        assert!(
            matches!(&action, RetryAction::Switch { to, .. } if to == "test-skip/open"),
            "unexpected action: {action:?}"
        );
    }

    #[test]
    fn retry_rate_limit_keeps_retry_after_backoff_when_no_candidate_is_usable() {
        RetryPolicy::start_cooldown(ModelReferenceFormat::LegacyEffort, "test-skip2/cooling");
        let config = policy(&[("test-skip2/primary", &["test-skip2/cooling"])]);
        let mut engine = engine(config, "test-skip2/primary", 2);
        let (action, _) = engine.next_with_class(Failure::Limited("429; retry-after: 60"), false);
        assert!(
            matches!(
                action,
                RetryAction::Backoff { delay, class: RetryClass::RateLimit, attempt: 1, .. }
                    if delay == Duration::from_secs(60)
            ),
            "unexpected action: {action:?}"
        );
        assert_eq!(engine.model(), Some("test-skip2/primary"));
    }

    #[test]
    fn retry_exhausting_the_chain_backs_off_then_fails() {
        let config = policy(&[("test-d/primary", &["test-d/spare"])]);
        let mut engine = engine(config, "test-d/primary", 1);

        assert!(matches!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::Switch { .. }
        ));
        // Chain exhausted: the spare backs off within its own budget, then the
        // failure surfaces.
        assert!(matches!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::Backoff { .. }
        ));
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::GiveUp
        );
    }

    #[test]
    fn retry_budget_of_zero_never_switches_models() {
        // `--rate-limit-retries 0` disables retrying: a fallback chain must not
        // become a second retry count (PROHIBITED §7).
        let config = policy(&[("test-z/primary", &["test-z/spare"])]);
        let mut engine = engine(config, "test-z/primary", 0);
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-z/primary"));
    }

    #[test]
    fn retry_without_a_policy_keeps_the_legacy_same_model_backoff() {
        let mut engine = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            None,
            Some("test-e/only"),
            2,
        );

        // Non-limit failures stayed permanent before the fallback engine.
        assert_eq!(
            engine
                .next_with_class(Failure::Failed("HTTP 503 service unavailable"), false)
                .0,
            RetryAction::GiveUp
        );
        // Limits retry on the command backend's 2s-doubling backoff, on the
        // same model, even after visible output.
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), true).0,
            RetryAction::Backoff {
                delay: crate::step::command::calculate_backoff(1),
                class: RetryClass::RateLimit,
                attempt: 1,
                of: 2,
            }
        );
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), true).0,
            RetryAction::Backoff {
                delay: crate::step::command::calculate_backoff(2),
                class: RetryClass::RateLimit,
                attempt: 2,
                of: 2,
            }
        );
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), true).0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-e/only"), "no model switching");
    }

    #[test]
    fn retry_after_visible_text_backs_off_but_never_switches_model() {
        let config = policy(&[("test-f/primary", &["test-f/spare"])]);
        let mut engine = engine(config, "test-f/primary", 1);

        // Same-model backoff is what cruise always did, policy or not.
        assert!(matches!(
            engine.next_with_class(Failure::Limited("429"), true).0,
            RetryAction::Backoff { .. }
        ));
        // Replaying streamed text on another model is not.
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), true).0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-f/primary"));
    }

    #[test]
    fn retry_server_error_switches_immediately_without_retrying_the_primary() {
        let config = policy(&[("test-server/primary", &["test-server/fallback"])]);
        let mut engine = engine(config, "test-server/primary", 3);

        assert_eq!(
            engine
                .next_with_class(Failure::Failed("HTTP status 503"), false)
                .0,
            RetryAction::Switch {
                from: Some("test-server/primary".to_string()),
                to: "test-server/fallback".to_string(),
                detail: "503".to_string(),
                attempt: 1,
                of: 7,
            }
        );
        assert_eq!(engine.model(), Some("test-server/fallback"));
    }

    #[test]
    fn retry_client_error_switches_immediately_and_never_resends_the_same_model() {
        let config = policy(&[("test-client/primary", &["test-client/fallback"])]);
        let mut engine = engine(config, "test-client/primary", 3);

        // Given a 4xx, the next chain entry takes over at once.
        assert_eq!(
            engine
                .next_with_class(
                    Failure::Failed("API Error: 400 invalid_request: context length exceeded"),
                    false
                )
                .0,
            RetryAction::Switch {
                from: Some("test-client/primary".to_string()),
                to: "test-client/fallback".to_string(),
                detail: "400".to_string(),
                attempt: 1,
                of: 7,
            }
        );
        assert_eq!(engine.model(), Some("test-client/fallback"));

        // Then, with the chain exhausted, the error surfaces instead of the
        // fallback re-sending a request the provider already refused.
        assert_eq!(
            engine
                .next_with_class(Failure::Failed("API Error: 400 invalid_request"), false)
                .0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-client/fallback"));
    }

    #[test]
    fn retry_client_error_after_visible_text_fails_without_switching() {
        let config = policy(&[(
            "test-client-streamed/primary",
            &["test-client-streamed/spare"],
        )]);
        let mut streamed = engine(config.clone(), "test-client-streamed/primary", 3);

        assert_eq!(
            streamed
                .next_with_class(Failure::Failed("HTTP 413 payload too large"), true)
                .0,
            RetryAction::GiveUp
        );
        assert_eq!(streamed.model(), Some("test-client-streamed/primary"));

        // And the model the run gave up on stays usable: the request was
        // refused, so there is nothing to cool down. Observed directly,
        // because "a later engine still starts on the primary" also holds
        // when the primary *and* every candidate are cooling.
        assert!(!RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-client-streamed/primary"
        ));
        let mut later = engine(config, "test-client-streamed/primary", 3);
        assert_eq!(later.model(), Some("test-client-streamed/primary"));
        assert!(!later.startup_blocked());
        assert_eq!(later.take_startup_switch(), None);
    }

    #[test]
    fn model_missing_wordings_classify_where_no_other_rule_names_the_failure() {
        // The real Copilot refusal that motivated the class: a 400 whose body
        // names the model, not the request.
        assert_eq!(
            classify_retryable(
                "Copilot API error (HTTP 400 Bad Request): {\"error\":{\"message\":\"The requested \
                 model is not supported.\",\"code\":\"model_not_supported\",\"param\":\"model\",\
                 \"type\":\"invalid_request_error\"}}"
            ),
            Some(RetryClass::ModelMissing)
        );
        // A spelling naming no status code still classifies, where the
        // permanent wordings would otherwise swallow it.
        assert_eq!(
            classify_retryable("unknown model: gpt-5.6-luna"),
            Some(RetryClass::ModelMissing)
        );
        assert_eq!(
            classify_retryable(
                "invalid_request: Unsupported OpenAI model 'does-not-exist-9'. Use /model to choose from the models available to your account."
            ),
            Some(RetryClass::ModelMissing)
        );
        // A missing model also beats the permanent wordings it travels with:
        // the request is fine, the reference is not.
        assert_eq!(
            classify_retryable("invalid_request_error: model not found"),
            Some(RetryClass::ModelMissing)
        );
        // A refused request stays a client error: the model itself is fine.
        assert_eq!(
            classify_retryable("API Error: HTTP 400 bad request"),
            Some(RetryClass::ClientError)
        );
        // And a bare `not found` is not a statement about the model.
        assert_eq!(
            classify_retryable("status 404: not found"),
            Some(RetryClass::ClientError)
        );
    }

    #[test]
    fn model_missing_never_outranks_a_limit_a_5xx_or_the_stderr_tail() {
        // A marker riding in the appended stderr tail is a log line about some
        // other model or turn: it must not turn a wait, an outage or a
        // transport blip into a dead model. Same hazard the status-code rules
        // guard by checking 4xx last.
        for (message, expected) in [
            (
                "HTTP 429 Too Many Requests\njcode stderr:\n[warn] copilot: model_not_supported \
                 for gpt-5-mini, falling back",
                RetryClass::RateLimit,
            ),
            (
                "Error: 503 service unavailable\nclaude stderr:\nwarn: unknown model alias ignored",
                RetryClass::ServerError,
            ),
            (
                "fetch failed\njcode stderr:\n[jcode] unknown model alias in config, ignoring",
                RetryClass::Network,
            ),
            (
                "HTTP 400 bad request\njcode stderr:\nunknown model in $JCODE_HOME/config.toml",
                RetryClass::ClientError,
            ),
            // A refused *parameter* names a model without the model being
            // absent, so it stays the class that leaves no cooldown.
            (
                "API Error: 400 invalid_request_error: unsupported model parameter: 'temperature'",
                RetryClass::ClientError,
            ),
        ] {
            assert_eq!(classify_retryable(message), Some(expected), "for {message}");
        }
    }

    #[test]
    fn retry_model_missing_switches_and_cools_the_model_down() {
        let config = policy(&[("test-missing/primary", &["test-missing/spare"])]);
        let mut first = engine(config.clone(), "test-missing/primary", 1);
        assert_eq!(
            first
                .next_with_class(Failure::Failed("HTTP 400 model_not_supported"), false)
                .0,
            RetryAction::Switch {
                from: Some("test-missing/primary".to_string()),
                to: "test-missing/spare".to_string(),
                detail: "400".to_string(),
                attempt: 1,
                of: 3,
            }
        );

        // Unlike a refused request, a model the provider does not have must
        // never be selected again: a later turn — a step retried through
        // `if.fail.retry`, say — starts on the spare instead of spending
        // another request on the identical 400.
        assert!(RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-missing/primary"
        ));
        let mut later = engine(config, "test-missing/primary", 1);
        assert_eq!(later.model(), Some("test-missing/spare"));
        assert_eq!(
            later.take_startup_switch(),
            Some((
                "test-missing/primary".to_string(),
                "test-missing/spare".to_string()
            ))
        );
    }

    #[test]
    fn retry_model_missing_cools_the_model_down_even_when_it_cannot_switch() {
        // No candidate to switch to, and re-sending is pointless, so the error
        // surfaces at once — without spending a single same-model backoff.
        let solo = policy(&[("test-missing-solo/primary", &[])]);
        let mut solo = engine(solo, "test-missing-solo/primary", 3);
        assert_eq!(
            solo.next_with_class(Failure::Failed("404 model_not_found"), false)
                .0,
            RetryAction::GiveUp
        );
        assert_eq!(solo.model(), Some("test-missing-solo/primary"));
        assert!(RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-missing-solo/primary"
        ));

        // Replaying text the user already saw on another model is never safe,
        // so a streamed turn gives up as well — yet the replay hazard belongs
        // to this turn and the missing model does not, so it cools down too.
        let config = policy(&[(
            "test-missing-streamed/primary",
            &["test-missing-streamed/spare"],
        )]);
        let mut streamed = engine(config, "test-missing-streamed/primary", 3);
        assert_eq!(
            streamed
                .next_with_class(Failure::Failed("HTTP 404 model_not_found"), true)
                .0,
            RetryAction::GiveUp
        );
        assert_eq!(streamed.model(), Some("test-missing-streamed/primary"));
        assert!(RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-missing-streamed/primary"
        ));
    }

    #[test]
    fn retry_client_error_switch_leaves_the_abandoned_model_usable() {
        // Given a 4xx switch away from the primary,
        let config = policy(&[("test-cool4xx/primary", &["test-cool4xx/spare"])]);
        let mut first = engine(config.clone(), "test-cool4xx/primary", 1);
        assert!(matches!(
            first
                .next_with_class(Failure::Failed("HTTP 400 bad request"), false)
                .0,
            RetryAction::Switch { .. }
        ));

        // When a later turn resolves the same primary, it is not cooling: a
        // refused request says nothing about the model's health.
        assert!(!RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-cool4xx/primary"
        ));
        let mut later = engine(config, "test-cool4xx/primary", 1);
        assert_eq!(later.model(), Some("test-cool4xx/primary"));
        assert!(!later.startup_blocked());
        assert_eq!(later.take_startup_switch(), None);
    }

    #[test]
    fn retry_client_error_with_a_zero_budget_gives_up_without_switching() {
        // `--rate-limit-retries 0` disables retrying, and a fallback chain
        // must not become a second retry count for 4xx either.
        let config = policy(&[("test-client-zero/primary", &["test-client-zero/spare"])]);
        let mut engine = engine(config, "test-client-zero/primary", 0);
        assert_eq!(
            engine
                .next_with_class(Failure::Failed("HTTP 400 bad request"), false)
                .0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-client-zero/primary"));
    }

    #[test]
    fn retry_client_error_without_model_fallback_gives_up_instead_of_backing_off() {
        // With switching disabled a server error degrades to a same-model
        // backoff, but a 4xx must not: the same request returns the same
        // status however long the run waits.
        let mut config = policy(&[("test-client-nofb/primary", &["test-client-nofb/spare"])]);
        config.model_fallback = false;
        let mut engine = engine(config, "test-client-nofb/primary", 3);
        assert_eq!(
            engine
                .next_with_class(Failure::Failed("HTTP 400 bad request"), false)
                .0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-client-nofb/primary"));
    }

    #[test]
    fn retry_request_timeout_takes_the_network_path() {
        // A 408 switches first, like any transport failure, and the notice
        // names the status rather than the class.
        let config = policy(&[("test-408/primary", &["test-408/spare"])]);
        let mut switching = engine(config, "test-408/primary", 3);
        assert_eq!(
            switching
                .next_with_class(
                    Failure::Failed("API Error: HTTP 408 request timeout"),
                    false
                )
                .0,
            RetryAction::Switch {
                from: Some("test-408/primary".to_string()),
                to: "test-408/spare".to_string(),
                detail: "408".to_string(),
                attempt: 1,
                of: 7,
            }
        );

        // With no candidate it re-sends to the same model, unlike a 4xx.
        let mut config = policy(&[]);
        config.model_fallback = false;
        let mut alone = engine(config, "test-408-alone/primary", 1);
        assert!(matches!(
            alone
                .next_with_class(
                    Failure::Failed("API Error: HTTP 408 request timeout"),
                    false
                )
                .0,
            RetryAction::Backoff {
                class: RetryClass::Network,
                attempt: 1,
                ..
            }
        ));
        assert_eq!(alone.model(), Some("test-408-alone/primary"));
    }

    #[test]
    fn retry_server_error_switch_cools_the_abandoned_model() {
        // Given a 5xx switch away from the primary,
        let config = policy(&[("test-cool5xx/primary", &["test-cool5xx/spare"])]);
        let mut first = engine(config.clone(), "test-cool5xx/primary", 1);
        assert!(matches!(
            first
                .next_with_class(Failure::Failed("HTTP status 503"), false)
                .0,
            RetryAction::Switch { .. }
        ));

        // When a later turn resolves the same primary, the unhealthy model is
        // still skipped for its cooldown.
        let mut later = engine(config, "test-cool5xx/primary", 1);
        assert_eq!(later.model(), Some("test-cool5xx/spare"));
        assert_eq!(
            later.take_startup_switch(),
            Some((
                "test-cool5xx/primary".to_string(),
                "test-cool5xx/spare".to_string()
            ))
        );
    }

    #[test]
    fn retry_client_error_without_a_policy_fails_the_run() {
        let mut engine = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            None,
            Some("test-client-none/only"),
            3,
        );

        assert_eq!(
            engine
                .next_with_class(Failure::Failed("HTTP 400 bad request"), false)
                .0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-client-none/only"));
    }

    #[test]
    fn retry_server_error_backs_off_when_model_switching_is_unavailable() {
        let mut config = policy(&[]);
        config.model_fallback = false;
        let mut engine = engine(config, "test-server-no-switch/primary", 1);

        assert!(matches!(
            engine
                .next_with_class(Failure::Failed("HTTP status 503"), false)
                .0,
            RetryAction::Backoff {
                class: RetryClass::ServerError,
                attempt: 1,
                of: 1,
                ..
            }
        ));
        assert_eq!(
            engine
                .next_with_class(Failure::Failed("HTTP status 503"), false)
                .0,
            RetryAction::GiveUp
        );
    }

    #[test]
    fn retry_network_failure_switches_immediately_without_retrying_the_primary() {
        let config = policy(&[("test-network/primary", &["test-network/fallback"])]);
        let mut engine = engine(config, "test-network/primary", 3);

        assert_eq!(
            engine
                .next_with_class(Failure::Failed("connection reset by peer"), false)
                .0,
            RetryAction::Switch {
                from: Some("test-network/primary".to_string()),
                to: "test-network/fallback".to_string(),
                detail: "network error".to_string(),
                attempt: 1,
                of: 7,
            }
        );
        assert_eq!(engine.model(), Some("test-network/fallback"));
    }

    #[test]
    fn retry_switches_when_the_computed_delay_exceeds_max_delay_ms() {
        let mut config = policy(&[("test-g/only", &["test-g/spare"])]);
        config.base_delay_ms = 5_000;
        config.max_delay_ms = 1_000;
        let mut engine = engine(config, "test-g/only", 3);
        assert!(matches!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::Switch { .. }
        ));
        // The spare cannot serve a usable delay either, and the chain is empty.
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::GiveUp
        );
    }

    #[test]
    fn retry_skips_a_model_that_is_still_cooling_down_and_reports_it() {
        let shared = Arc::new(RetryPolicy::new(policy(&[(
            "test-h/primary",
            &["test-h/spare"],
        )])));
        let mut first = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            Some(Arc::clone(&shared)),
            Some("test-h/primary"),
            1,
        );
        assert!(matches!(
            first.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::Switch { .. }
        ));

        // The next turn of the same run starts on the spare, and says so.
        let mut second = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            Some(shared),
            Some("test-h/primary"),
            1,
        );
        assert_eq!(second.model(), Some("test-h/spare"));
        assert_eq!(
            second.take_startup_switch(),
            Some(("test-h/primary".to_string(), "test-h/spare".to_string()))
        );
        assert_eq!(second.take_startup_switch(), None, "reported once");
    }

    #[test]
    fn retry_model_cooldown_last_for_thirty_minutes() {
        RetryPolicy::start_cooldown(ModelReferenceFormat::LegacyEffort, "test-cooldown/model");

        let until = MODEL_COOLDOWNS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(
                ModelReferenceFormat::LegacyEffort,
                crate::model_selection::Sdk::Jcode,
                "test-cooldown/model".to_string(),
            ))
            .copied()
            .unwrap_or_else(|| panic!("cooldown entry missing"));
        let remaining = until.saturating_duration_since(Instant::now());

        assert!(
            remaining >= Duration::from_secs(1_790),
            "cooldown should last about 30 minutes, only {remaining:?} remain"
        );
        assert!(
            remaining <= Duration::from_mins(30),
            "cooldown should not exceed 30 minutes, {remaining:?} remain"
        );
    }

    #[test]
    fn retry_cooldowns_are_shared_across_policies_in_one_process() {
        let config = policy(&[("test-j/primary", &["test-j/spare"])]);
        let mine = Arc::new(RetryPolicy::new(config.clone()));
        let mut engine = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            Some(mine),
            Some("test-j/primary"),
            1,
        );
        assert!(matches!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::Switch { .. }
        ));

        // A later policy in the same process still skips the failed model.
        let theirs = Arc::new(RetryPolicy::new(config));
        let mut other = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            Some(theirs),
            Some("test-j/primary"),
            1,
        );
        assert_eq!(other.model(), Some("test-j/spare"));
        assert_eq!(
            other.take_startup_switch(),
            Some(("test-j/primary".to_string(), "test-j/spare".to_string()))
        );
    }

    #[test]
    fn retry_does_not_start_when_all_configured_models_are_cooling_down() {
        RetryPolicy::start_cooldown(ModelReferenceFormat::LegacyEffort, "test-blocked/primary");
        RetryPolicy::start_cooldown(ModelReferenceFormat::LegacyEffort, "test-blocked/spare");

        let config = policy(&[("test-blocked/primary", &["test-blocked/spare"])]);
        let engine = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            Some(Arc::new(RetryPolicy::new(config))),
            Some("test-blocked/primary"),
            1,
        );

        assert!(engine.startup_blocked());
    }

    #[test]
    fn retry_cooldown_ignores_the_effort_suffix() {
        // `:high` and `:low` are one model behind one quota.
        RetryPolicy::start_cooldown(ModelReferenceFormat::LegacyEffort, "test-k/primary:high");
        assert!(RetryPolicy::is_cooling(
            ModelReferenceFormat::LegacyEffort,
            "test-k/primary:low"
        ));
    }

    #[test]
    fn retry_model_fallback_disabled_never_switches() {
        let mut config = policy(&[("test-i/primary", &["test-i/spare"])]);
        config.model_fallback = false;
        let mut engine = engine(config, "test-i/primary", 1);
        assert!(matches!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::Backoff { .. }
        ));
        assert_eq!(
            engine.next_with_class(Failure::Limited("429"), false).0,
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-i/primary"));
    }

    #[test]
    fn retry_undispatchable_model_moves_to_the_next_candidate_without_budget() {
        let config = policy(&[("test-l/primary", &["test-l/spare"])]);
        let mut engine = engine(config, "test-l/primary", 0);
        assert_eq!(
            engine
                .next_with_class(
                    Failure::Unusable("invalid model reference 'test-l/primary'"),
                    false
                )
                .0,
            RetryAction::Switch {
                from: Some("test-l/primary".to_string()),
                to: "test-l/spare".to_string(),
                detail: "unusable model".to_string(),
                attempt: 1,
                of: 1,
            }
        );
        // No candidate left: the caller surfaces the backend's own error.
        assert_eq!(
            engine
                .next_with_class(Failure::Unusable("invalid model reference"), false)
                .0,
            RetryAction::GiveUp
        );
    }

    #[test]
    fn retry_undispatchable_model_without_a_policy_fails_the_run() {
        let mut engine = FallbackEngine::new(
            ModelReferenceFormat::LegacyEffort,
            None,
            Some("test-m/primary"),
            3,
        );
        assert_eq!(
            engine
                .next_with_class(Failure::Unusable("invalid model reference"), false)
                .0,
            RetryAction::GiveUp
        );
    }
}
