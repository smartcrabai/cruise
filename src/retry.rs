//! Retryable-failure handling for the SDK backends (`sdk: jcode`, `sdk: claude`).
//!
//! `FallbackEngine` decides, after a failed turn, whether to retry and how.
//! It runs in one of two modes, selected by whether the workflow declares a
//! `retry:` block ([`crate::config::RetryConfig`]):
//!
//! - **No policy** — cruise's historical behavior: only a backend-reported
//!   rate limit (`Failure::Limited`) is retryable, always on the same model,
//!   on the command backend's 2s-doubling backoff
//!   ([`crate::step::command::calculate_backoff`]).
//! - **Policy** — 5xx and network failures become retryable too
//!   (`classify_retryable`), the backoff becomes
//!   `min(base_delay_ms * 2^(attempt-1), 8s)` with jitter (a server
//!   `Retry-After` hint wins), and a model that has spent its retry budget is
//!   swapped for the next entry of its fallback chain — with no delay, a fresh
//!   budget, and never after the turn already streamed visible text.
//!
//! The retry *budget* is always the caller's existing `PromptRun::max_retries`,
//! spent per model: this module adds no second retry count, and
//! `max_retries == 0` (`--rate-limit-retries 0`) disables retrying, and
//! therefore switching, entirely.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::backend::effort::split_thinking_suffix;
use crate::config::RetryConfig;

/// Fallback-chain key used when no model-specific chain matches.
pub(crate) const DEFAULT_CHAIN_KEY: &str = "default";

/// Suffix marking a chain key or entry as a provider wildcard (`anthropic/*`).
pub(crate) const WILDCARD_SUFFIX: &str = "/*";

/// Ceiling of the computed exponential backoff, before jitter.
const BACKOFF_CEILING_MS: u64 = 8_000;

/// Upper bound applied to a server-provided `Retry-After` hint.
const RETRY_AFTER_CLAMP: Duration = Duration::from_secs(60);

/// How long a model that failed retryably is skipped when resolving the model
/// for a later turn. In-memory only: nothing is persisted across processes.
const MODEL_COOLDOWN: Duration = Duration::from_secs(300);

/// Characters scanned after a `Retry-After` marker while looking for its value.
const HINT_SCAN_CHARS: usize = 24;

/// Bytes before a status code that may carry a marker making it an HTTP status.
const STATUS_MARKER_WINDOW: usize = 24;

/// Markers that turn a bare number into an HTTP status code.
const STATUS_MARKERS: &[&str] = &["http", "status", "code", "error", "returned", "upstream"];

/// Status codes reported in provider error text.
const STATUS_CODES: &[&str] = &["429", "500", "502", "503", "504"];

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
    "context length",
    "context window",
    "too long",
    "max_tokens",
    "insufficient",
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

// Policy context for one asynchronous run. Tokio task-local storage follows
// the run future without changing the public `crate::executor::PromptRun`
// shape or sharing policies between concurrent sessions.
tokio::task_local! {
    static TASK_POLICY: Option<Arc<RetryPolicy>>;
}

/// A published `retry:` policy together with the model cooldowns it produced.
///
/// The cooldowns belong to the policy rather than to the process so they are
/// scoped to the workflow that created them: every prompt of a run snapshots
/// the same `Arc`, so a model that failed in step 3 is still skipped in step 4,
/// while another config's runs (and each unit test) keep their own state.
pub struct RetryPolicy {
    config: RetryConfig,
    cooldowns: Mutex<HashMap<String, Instant>>,
}

impl RetryPolicy {
    /// Wrap `config` with empty cooldown state.
    #[must_use]
    pub(crate) fn new(config: RetryConfig) -> Self {
        Self {
            config,
            cooldowns: Mutex::new(HashMap::new()),
        }
    }

    /// Mark `model` as recently failed, so later turns prefer its fallbacks.
    ///
    /// Keyed by the effort-stripped reference: `provider/m:high` and
    /// `provider/m:low` are the same model behind the same quota.
    fn start_cooldown(&self, model: &str) {
        let mut map = self
            .cooldowns
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        map.insert(
            split_thinking_suffix(model).0.to_string(),
            Instant::now() + MODEL_COOLDOWN,
        );
    }

    /// Whether `model` is still cooling down, dropping expired entries as it
    /// goes.
    fn is_cooling(&self, model: &str) -> bool {
        let mut map = self
            .cooldowns
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        map.retain(|_, until| *until > now);
        map.contains_key(split_thinking_suffix(model).0)
    }
}

/// Build a policy with fresh cooldown state for one resolved workflow config.
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
    if let Ok(policy) = TASK_POLICY.try_with(|policy| policy.clone()) {
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
    /// Transport failure (connection reset, timeout, terminated stream).
    Network,
    /// The backend could not dispatch the model reference at all, so only
    /// another model can help.
    ModelUnusable,
}

impl RetryClass {
    /// Sentence-leading label for progress notices.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            RetryClass::RateLimit => "Rate limit",
            RetryClass::ServerError => "Server error",
            RetryClass::Network => "Network error",
            RetryClass::ModelUnusable => "Unusable model",
        }
    }
}

/// How a turn failed, as reported by the backend.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Failure<'a> {
    /// The backend classified the failure as a rate/usage limit.
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

/// Whether `lower` (already lowercased) carries a permanent-failure wording.
fn is_permanent(lower: &str) -> bool {
    PERMANENT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
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
/// (authentication, invalid request, context overflow) which must never be
/// retried.
///
/// Unifies the command backend's [`crate::step::command::is_rate_limited`] and
/// the SDK backends' limit wordings ([`is_limit_message`]), and extends them
/// with the 5xx and network conditions the fallback engine also retries.
#[must_use]
pub(crate) fn classify_retryable(text: &str) -> Option<RetryClass> {
    let lower = text.to_lowercase();
    if is_permanent(&lower) {
        return None;
    }
    if is_limit_wording(&lower) {
        return Some(RetryClass::RateLimit);
    }
    if SERVER_MARKERS.iter().any(|marker| lower.contains(marker))
        || ["500", "502", "503", "504"]
            .iter()
            .any(|code| has_status_code(&lower, code))
    {
        return Some(RetryClass::ServerError);
    }
    if NETWORK_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Some(RetryClass::Network);
    }
    None
}

/// Whether `code` occurs as a standalone number preceded, within
/// [`STATUS_MARKER_WINDOW`] bytes, by a marker that makes it an HTTP status.
/// Keeps `max_tokens: 5000` and `context length 500000` out of the retryable
/// classes.
fn has_status_code(lower: &str, code: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(code) {
        let at = from + rel;
        let end = at + code.len();
        let standalone = (at == 0 || !bytes[at - 1].is_ascii_alphanumeric())
            && (end == bytes.len() || !bytes[end].is_ascii_alphanumeric());
        if standalone {
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

/// Notice detail for a model switch: the HTTP status code the failure text
/// names (the `429` of JCODE.md §3.7's `fallback: a -> b (429, attempt 2/5)`),
/// or the class label when it names none.
fn failure_detail(class: RetryClass, text: &str) -> String {
    let lower = text.to_lowercase();
    STATUS_CODES
        .iter()
        .find(|code| has_status_code(&lower, code))
        .map_or_else(|| class.label().to_lowercase(), |code| (*code).to_string())
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
/// provider, carrying the reference's `:effort` suffix along; every other entry
/// is used verbatim. Entries equal to the failing model, and duplicates, are
/// dropped.
fn chain_for(config: &RetryConfig, model: Option<&str>) -> VecDeque<String> {
    let base = model.map(|m| split_thinking_suffix(m).0);
    let effort = model
        .and_then(|m| split_thinking_suffix(m).1)
        .map(|suffix| format!(":{suffix}"));
    let entries = base
        .and_then(|b| {
            config.fallback_chains.get(b).or_else(|| {
                b.split_once('/').and_then(|(provider, _)| {
                    config
                        .fallback_chains
                        .get(&format!("{provider}{WILDCARD_SUFFIX}"))
                })
            })
        })
        .or_else(|| config.fallback_chains.get(DEFAULT_CHAIN_KEY));

    let mut chain = VecDeque::new();
    for entry in entries.into_iter().flatten() {
        let Some(candidate) = expand_entry(entry, base, effort.as_deref()) else {
            continue;
        };
        if Some(candidate.as_str()) == model || chain.contains(&candidate) {
            continue;
        }
        chain.push_back(candidate);
    }
    chain
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

/// Per-run retry state: which model the next attempt uses, what is left of the
/// fallback chain, and how much of the retry budget the current model spent.
pub(crate) struct FallbackEngine {
    policy: Option<Arc<RetryPolicy>>,
    max_retries: usize,
    model: Option<String>,
    chain: VecDeque<String>,
    /// Retries spent on the current model.
    attempts: usize,
    /// Retries spent across every model, for notices.
    total: usize,
    /// Retries the run may spend in total, for notices.
    budget: usize,
    /// A cooldown-driven model replacement made before the first attempt,
    /// waiting to be reported.
    startup_switch: Option<(String, String)>,
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
    pub(crate) fn new(
        policy: Option<Arc<RetryPolicy>>,
        model_ref: Option<&str>,
        max_retries: usize,
    ) -> Self {
        let model = model_ref.filter(|m| !m.is_empty()).map(str::to_string);
        let chain = match policy.as_deref() {
            Some(policy) if policy.config.model_fallback => {
                chain_for(&policy.config, model.as_deref())
            }
            _ => VecDeque::new(),
        };
        let mut engine = Self {
            policy,
            max_retries,
            model,
            chain,
            attempts: 0,
            total: 0,
            budget: 0,
            startup_switch: None,
        };
        if let Some(policy) = engine.policy.clone()
            && let Some(current) = engine.model.clone()
            && policy.is_cooling(&current)
            && let Some(next) = engine.take_candidate()
        {
            engine.model = Some(next.clone());
            engine.startup_switch = Some((current, next));
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

    /// The cooldown-driven `(from, to)` replacement made at construction, if
    /// any. Yielded once, for the caller to report.
    pub(crate) fn take_startup_switch(&mut self) -> Option<(String, String)> {
        self.startup_switch.take()
    }

    /// Decide what to do after a failed turn. `streamed` reports whether the
    /// turn already pushed assistant text to the user's output sink.
    pub(crate) fn next(&mut self, failure: Failure<'_>, streamed: bool) -> RetryAction {
        let Some(class) = self.classify(failure) else {
            return RetryAction::GiveUp;
        };
        let message = failure.message();

        // Nothing was sent, so there is no delay to serve and no budget to
        // spend: the only useful move is the next chain entry.
        if matches!(failure, Failure::Unusable(_)) {
            return self.switch(class, message).unwrap_or(RetryAction::GiveUp);
        }

        // The same model first, so `PromptRun::max_retries` alone decides how
        // often a model runs: `--rate-limit-retries 0` still means no retry,
        // and a momentary limit does not cost the primary model.
        if self.attempts < self.max_retries
            && let Some(delay) = self.delay_for(message, self.attempts + 1)
        {
            self.attempts += 1;
            self.total += 1;
            return RetryAction::Backoff {
                delay,
                class,
                attempt: self.total,
                of: self.of(),
            };
        }

        // Budget spent (or the next delay would exceed `max_delay_ms`): move to
        // the next chain model, with no delay and a fresh budget. Replay
        // safety: a turn whose text the user already saw is never re-run on
        // another model.
        if self.max_retries > 0
            && !streamed
            && let Some(action) = self.switch(class, message)
        {
            return action;
        }
        if let Some((policy, model)) = self.policy.as_deref().zip(self.model.as_deref()) {
            policy.start_cooldown(model);
        }
        RetryAction::GiveUp
    }

    /// Retryable class of `failure`: without a policy only a backend-reported
    /// limit qualifies, which is what cruise retried before the fallback engine
    /// existed.
    fn classify(&self, failure: Failure<'_>) -> Option<RetryClass> {
        match failure {
            // The backend already decided this is retryable; its text only
            // picks the label, so `overloaded` reads as a server error on both
            // SDK backends.
            Failure::Limited(message) => {
                Some(classify_retryable(message).unwrap_or(RetryClass::RateLimit))
            }
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

    /// Move to the next usable chain entry, cooling down the model left behind.
    fn switch(&mut self, class: RetryClass, message: &str) -> Option<RetryAction> {
        let to = self.take_candidate()?;
        let from = self.model.replace(to.clone());
        if let Some((policy, previous)) = self.policy.as_deref().zip(from.as_deref()) {
            policy.start_cooldown(previous);
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

    /// Next chain entry that is not cooling down.
    fn take_candidate(&mut self) -> Option<String> {
        let policy = self.policy.clone()?;
        while let Some(candidate) = self.chain.pop_front() {
            if !policy.is_cooling(&candidate) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn retry_classification_rejects_permanent_failures() {
        for message in [
            "authentication_error: invalid API key",
            "invalid_request_error: model not found",
            "context length exceeded: 500000 tokens > limit",
            "max_tokens must be <= 5000",
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
            engine.next(Failure::Limited("429; retry-after: 30"), false),
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
            engine.next(Failure::Limited("429; retry-after: 30"), false),
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
        let chain = chain_for(&config, Some("anthropic/opus"));
        assert_eq!(chain, ["fallback/by-model"]);
        let chain = chain_for(&config, Some("anthropic/haiku"));
        assert_eq!(chain, ["fallback/by-provider"]);
        let chain = chain_for(&config, Some("openai/gpt-5.5"));
        assert_eq!(chain, ["fallback/default-model"]);
        let chain = chain_for(&config, None);
        assert_eq!(chain, ["fallback/default-model"]);
    }

    #[test]
    fn retry_chain_wildcard_entry_keeps_the_model_and_effort() {
        let config = policy(&[("default", &["openrouter/*"])]);
        let chain = chain_for(&config, Some("anthropic/claude-opus-4-6:high"));
        assert_eq!(chain, ["openrouter/claude-opus-4-6:high"]);
    }

    // --- engine ---------------------------------------------------------------

    #[test]
    fn retry_spends_the_model_budget_before_switching_chains() {
        let config = policy(&[(
            "test-a/model-429",
            &["test-b/model-429", "test-c/model-429"],
        )]);
        let mut engine = engine(config, "test-a/model-429", 1);
        assert_eq!(engine.model(), Some("test-a/model-429"));

        // The primary model is retried on its own budget first.
        assert!(matches!(
            engine.next(Failure::Limited("HTTP status 429"), false),
            RetryAction::Backoff { attempt: 1, .. }
        ));
        assert_eq!(engine.model(), Some("test-a/model-429"));

        // Budget spent: switch, immediately, with a fresh budget.
        assert_eq!(
            engine.next(Failure::Limited("HTTP status 429"), false),
            RetryAction::Switch {
                from: Some("test-a/model-429".to_string()),
                to: "test-b/model-429".to_string(),
                detail: "429".to_string(),
                attempt: 2,
                of: 5,
            }
        );
        assert_eq!(engine.model(), Some("test-b/model-429"));
        assert_eq!(engine.attempts, 0, "the new model gets a fresh budget");
    }

    #[test]
    fn retry_exhausting_the_chain_backs_off_then_fails() {
        let config = policy(&[("test-d/primary", &["test-d/spare"])]);
        let mut engine = engine(config, "test-d/primary", 1);

        assert!(matches!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::Backoff { .. }
        ));
        assert!(matches!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::Switch { .. }
        ));
        // Chain exhausted: the spare backs off within its own budget, then the
        // failure surfaces.
        assert!(matches!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::Backoff { .. }
        ));
        assert_eq!(
            engine.next(Failure::Limited("429"), false),
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
            engine.next(Failure::Limited("429"), false),
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-z/primary"));
    }

    #[test]
    fn retry_without_a_policy_keeps_the_legacy_same_model_backoff() {
        let mut engine = FallbackEngine::new(None, Some("test-e/only"), 2);

        // Non-limit failures stayed permanent before the fallback engine.
        assert_eq!(
            engine.next(Failure::Failed("HTTP 503 service unavailable"), false),
            RetryAction::GiveUp
        );
        // Limits retry on the command backend's 2s-doubling backoff, on the
        // same model, even after visible output.
        assert_eq!(
            engine.next(Failure::Limited("429"), true),
            RetryAction::Backoff {
                delay: crate::step::command::calculate_backoff(1),
                class: RetryClass::RateLimit,
                attempt: 1,
                of: 2,
            }
        );
        assert_eq!(
            engine.next(Failure::Limited("429"), true),
            RetryAction::Backoff {
                delay: crate::step::command::calculate_backoff(2),
                class: RetryClass::RateLimit,
                attempt: 2,
                of: 2,
            }
        );
        assert_eq!(
            engine.next(Failure::Limited("429"), true),
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
            engine.next(Failure::Limited("429"), true),
            RetryAction::Backoff { .. }
        ));
        // Replaying streamed text on another model is not.
        assert_eq!(
            engine.next(Failure::Limited("429"), true),
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-f/primary"));
    }

    #[test]
    fn retry_switches_when_the_computed_delay_exceeds_max_delay_ms() {
        let mut config = policy(&[("test-g/only", &["test-g/spare"])]);
        config.base_delay_ms = 5_000;
        config.max_delay_ms = 1_000;
        let mut engine = engine(config, "test-g/only", 3);
        assert!(matches!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::Switch { .. }
        ));
        // The spare cannot serve a usable delay either, and the chain is empty.
        assert_eq!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::GiveUp
        );
    }

    #[test]
    fn retry_skips_a_model_that_is_still_cooling_down_and_reports_it() {
        let shared = Arc::new(RetryPolicy::new(policy(&[(
            "test-h/primary",
            &["test-h/spare"],
        )])));
        let mut first = FallbackEngine::new(Some(Arc::clone(&shared)), Some("test-h/primary"), 1);
        assert!(matches!(
            first.next(Failure::Limited("429"), false),
            RetryAction::Backoff { .. }
        ));
        assert!(matches!(
            first.next(Failure::Limited("429"), false),
            RetryAction::Switch { .. }
        ));

        // The next turn of the same run starts on the spare, and says so.
        let mut second = FallbackEngine::new(Some(shared), Some("test-h/primary"), 1);
        assert_eq!(second.model(), Some("test-h/spare"));
        assert_eq!(
            second.take_startup_switch(),
            Some(("test-h/primary".to_string(), "test-h/spare".to_string()))
        );
        assert_eq!(second.take_startup_switch(), None, "reported once");
    }

    #[test]
    fn retry_cooldowns_are_scoped_to_their_own_policy() {
        let config = policy(&[("test-j/primary", &["test-j/spare"])]);
        let mine = Arc::new(RetryPolicy::new(config.clone()));
        let mut engine = FallbackEngine::new(Some(mine), Some("test-j/primary"), 1);
        assert!(matches!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::Backoff { .. }
        ));
        assert!(matches!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::Switch { .. }
        ));

        // An unrelated workflow's policy has its own cooldowns.
        let theirs = Arc::new(RetryPolicy::new(config));
        let other = FallbackEngine::new(Some(theirs), Some("test-j/primary"), 1);
        assert_eq!(other.model(), Some("test-j/primary"));
    }

    #[test]
    fn retry_cooldown_ignores_the_effort_suffix() {
        // `:high` and `:low` are one model behind one quota.
        let shared = Arc::new(RetryPolicy::new(policy(&[(
            "test-k/primary",
            &["test-k/spare"],
        )])));
        shared.start_cooldown("test-k/primary:high");
        assert!(shared.is_cooling("test-k/primary:low"));
    }

    #[test]
    fn retry_model_fallback_disabled_never_switches() {
        let mut config = policy(&[("test-i/primary", &["test-i/spare"])]);
        config.model_fallback = false;
        let mut engine = engine(config, "test-i/primary", 1);
        assert!(matches!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::Backoff { .. }
        ));
        assert_eq!(
            engine.next(Failure::Limited("429"), false),
            RetryAction::GiveUp
        );
        assert_eq!(engine.model(), Some("test-i/primary"));
    }

    #[test]
    fn retry_undispatchable_model_moves_to_the_next_candidate_without_budget() {
        let config = policy(&[("test-l/primary", &["test-l/spare"])]);
        let mut engine = engine(config, "test-l/primary", 0);
        assert_eq!(
            engine.next(
                Failure::Unusable("invalid model reference 'test-l/primary'"),
                false
            ),
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
            engine.next(Failure::Unusable("invalid model reference"), false),
            RetryAction::GiveUp
        );
    }

    #[test]
    fn retry_undispatchable_model_without_a_policy_fails_the_run() {
        let mut engine = FallbackEngine::new(None, Some("test-m/primary"), 3);
        assert_eq!(
            engine.next(Failure::Unusable("invalid model reference"), false),
            RetryAction::GiveUp
        );
    }
}
