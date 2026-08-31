use crate::cancellation::CancellationToken;
use crate::error::Result;
use crate::step::OptionChoice;
use crate::step::option::OptionResult;
use std::future::Future;
use std::pin::Pin;

/// Abstraction over the UI mechanism used to present option choices to users.
/// Clients may implement the basic method; shared runners call the
/// cancellation-aware default when no broker is required.
pub trait OptionHandler: Send + Sync {
    /// Present `choices` and return the selection.
    ///
    /// # Errors
    ///
    /// Returns an error when the handler cannot present the choices or obtain
    /// a selection.
    fn select_option(&self, choices: &[OptionChoice], plan: Option<&str>) -> Result<OptionResult>;

    /// Present choices while observing operation cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CruiseError::Interrupted`] if cancellation has
    /// already been requested, or the underlying [`Self::select_option`] error.
    fn select_option_with_cancellation(
        &self,
        choices: &[OptionChoice],
        plan: Option<&str>,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<OptionResult> {
        if cancel_token.is_some_and(CancellationToken::is_cancelled) {
            return Err(crate::error::CruiseError::Interrupted);
        }
        self.select_option(choices, plan)
    }

    /// Async bridge for handlers whose prompt wait must not block an async
    /// runtime worker. Existing synchronous handlers retain their behavior;
    /// broker-backed handlers override this and offload their wait.
    fn select_option_async<'a>(
        &'a self,
        choices: &'a [OptionChoice],
        plan: Option<&'a str>,
        cancel_token: Option<&'a CancellationToken>,
    ) -> Pin<Box<dyn Future<Output = Result<OptionResult>> + Send + 'a>> {
        Box::pin(async move { self.select_option_with_cancellation(choices, plan, cancel_token) })
    }
}

/// The CLI implementation of [`OptionHandler`] that uses `inquire` interactive prompts.
pub struct CliOptionHandler;

impl OptionHandler for CliOptionHandler {
    fn select_option(&self, choices: &[OptionChoice], plan: Option<&str>) -> Result<OptionResult> {
        // Serialize against the direct `inquire` prompts in `run_cmd` so
        // concurrent batch workers never draw overlapping terminal menus.
        let _guard = prompt_lock_guard();
        crate::step::option::run_option(choices, plan)
    }
}

/// Process-wide lock serializing interactive terminal prompts.
///
/// Shared by [`CliOptionHandler`], the direct `inquire` prompts in `run_cmd`,
/// and the raw-mode editor in `multiline_input` so concurrent batch workers
/// never draw overlapping terminal UI. Only the synchronous prompt operation
/// is serialized; command/agent work between prompts remains fully
/// concurrent.
///
/// The lock is deliberately acquired at leaf prompt sites only:
/// [`crate::multiline_input::prompt_multiline`] takes it around its raw-mode
/// editor, and an option-step menu ([`CliOptionHandler::select_option`]) takes
/// it across the whole menu interaction *including* its nested text-input
/// editor. Because a `std::sync::Mutex` is not reentrant, that nested editor
/// goes through `prompt_multiline_locked`, which must be called while the
/// prompt lock is already held.
static PROMPT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static PROMPT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Acquire the process-wide prompt lock.
pub(crate) fn prompt_lock_guard() -> std::sync::MutexGuard<'static, ()> {
    let guard = PROMPT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    PROMPT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    guard
}

/// Try to acquire the prompt lock without blocking the dashboard renderer.
pub(crate) fn try_prompt_lock_guard() -> Option<std::sync::MutexGuard<'static, ()>> {
    PROMPT_LOCK.try_lock().ok()
}

/// Return the generation of prompt activity observed by the dashboard.
pub(crate) fn prompt_epoch() -> u64 {
    PROMPT_EPOCH.load(std::sync::atomic::Ordering::Relaxed)
}

/// A test [`OptionHandler`] that panics if called.
///
/// Used in tests where no option steps should be reached.  Panicking (rather
/// than silently returning an empty result) ensures that an unexpected option
/// step is caught immediately.
#[cfg(test)]
pub struct NoOpOptionHandler;

#[cfg(test)]
impl OptionHandler for NoOpOptionHandler {
    fn select_option(
        &self,
        _choices: &[OptionChoice],
        _plan: Option<&str>,
    ) -> Result<OptionResult> {
        panic!(
            "NoOpOptionHandler: unexpected option step -- use FirstChoiceOptionHandler if option steps are expected"
        );
    }
}

/// A test [`OptionHandler`] that always selects the first choice and records how many times
/// `select_option` was called.
///
/// Thread-safe via `AtomicUsize`.
#[cfg(test)]
pub struct FirstChoiceOptionHandler {
    call_count: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl Default for FirstChoiceOptionHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl FirstChoiceOptionHandler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Returns the number of times `select_option` was called.
    pub fn call_count(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
impl OptionHandler for FirstChoiceOptionHandler {
    fn select_option(&self, choices: &[OptionChoice], _plan: Option<&str>) -> Result<OptionResult> {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (next, text_input) = match choices.first() {
            Some(OptionChoice::TextInput { next, .. }) => (next.clone(), Some(String::new())),
            Some(OptionChoice::Selector { next, .. }) => (next.clone(), None),
            None => (None, None),
        };
        Ok(OptionResult {
            next_step: next,
            text_input,
        })
    }
}
