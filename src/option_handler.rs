use crate::error::Result;
use crate::step::OptionChoice;
use crate::step::option::OptionResult;

/// Abstraction over the UI mechanism used to present option choices to the user.
///
/// Implementations:
/// - CLI: [`CliOptionHandler`] using `inquire` interactive prompts.
/// - GUI: [`GuiOptionHandler`] using Tauri events + `oneshot::channel`.
pub trait OptionHandler: Send + Sync {
    /// Present `choices` to the user and return their selection.
    ///
    /// `plan` is optional context text shown before the selection menu (e.g. plan.md contents).
    ///
    /// # Errors
    ///
    /// Returns an error if the user interaction fails or is cancelled.
    fn select_option(&self, choices: &[OptionChoice], plan: Option<&str>) -> Result<OptionResult>;
}

/// The CLI implementation of [`OptionHandler`] that uses `inquire` interactive prompts.
pub struct CliOptionHandler;

impl OptionHandler for CliOptionHandler {
    fn select_option(&self, choices: &[OptionChoice], plan: Option<&str>) -> Result<OptionResult> {
        crate::step::option::run_option(choices, plan)
    }
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
