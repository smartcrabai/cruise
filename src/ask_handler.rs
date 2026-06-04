//! Abstraction over the UI mechanism used when an SDK agent asks the user a
//! question mid-run (the `ask_user` custom tool).
//!
//! Mirrors [`crate::option_handler::OptionHandler`]:
//! - CLI: [`CliAskHandler`] using the reedline multiline prompt.
//! - GUI: a Tauri-backed handler (see `src-tauri`) using events + a channel.
//!
//! The handler is invoked from inside a [`crate::sdk_tools`] tool closure, which
//! runs on seher's dedicated pi worker thread. Implementations may therefore
//! block (read stdin, wait on a channel) — they are never called on the async
//! runtime thread.

use crate::error::Result;

/// Presents an agent's free-form question to the user and returns their answer.
///
/// Implementations block until the user responds (or cancels). The returned
/// string is fed back to the agent as the `ask_user` tool result.
pub trait AskHandler: Send + Sync {
    /// Ask `question` and return the user's answer.
    ///
    /// # Errors
    ///
    /// Returns an error if the interaction fails or the user cancels.
    fn ask_user(&self, question: &str) -> Result<String>;
}

/// Handler for non-interactive contexts where the user cannot be reached.
///
/// Used when `ask_user` should never be registered (e.g. the GUI "Ask about the
/// plan" flow, which has no streaming channel to surface a question on). If it is
/// invoked anyway it returns an error rather than blocking.
pub struct NoninteractiveAskHandler;

impl AskHandler for NoninteractiveAskHandler {
    fn ask_user(&self, _question: &str) -> Result<String> {
        Err(crate::error::CruiseError::Other(
            "ask_user is unavailable in this non-interactive context".to_string(),
        ))
    }
}

/// CLI implementation backed by the reedline multiline prompt.
pub struct CliAskHandler;

impl AskHandler for CliAskHandler {
    fn ask_user(&self, question: &str) -> Result<String> {
        crate::multiline_input::prompt_multiline(question)?.into_result()
    }
}

/// A test double that returns scripted answers in order and records the
/// questions it was asked.
///
/// Used by `ask_handler` / `sdk_tools` unit tests to exercise the `ask_user`
/// path without a terminal. Gated on `cfg(test)` (not `test-utils`) because it
/// has no out-of-crate consumers and would otherwise be dead in the binary when
/// `test-utils` is enabled without the test cfg.
#[cfg(test)]
pub struct ScriptedAskHandler {
    answers: std::sync::Mutex<std::collections::VecDeque<String>>,
    pub asked: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl ScriptedAskHandler {
    /// Build a handler that returns `answers` in order, one per `ask_user` call.
    #[must_use]
    pub fn new(answers: impl IntoIterator<Item = String>) -> Self {
        Self {
            answers: std::sync::Mutex::new(answers.into_iter().collect()),
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
impl AskHandler for ScriptedAskHandler {
    fn ask_user(&self, question: &str) -> Result<String> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(question.to_string());
        }
        let next = self.answers.lock().ok().and_then(|mut q| q.pop_front());
        match next {
            Some(answer) => Ok(answer),
            None => Err(crate::error::CruiseError::Other(
                "ScriptedAskHandler: no more scripted answers".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_handler_returns_answers_in_order() {
        let h = ScriptedAskHandler::new(["first".to_string(), "second".to_string()]);
        assert_eq!(
            h.ask_user("q1").unwrap_or_else(|e| panic!("{e:?}")),
            "first"
        );
        assert_eq!(
            h.ask_user("q2").unwrap_or_else(|e| panic!("{e:?}")),
            "second"
        );
    }

    #[test]
    fn scripted_handler_records_questions() {
        let h = ScriptedAskHandler::new(["a".to_string()]);
        let _ = h.ask_user("why?");
        let asked = h.asked.lock().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(asked.as_slice(), &["why?".to_string()]);
    }

    #[test]
    fn scripted_handler_errors_when_exhausted() {
        let h = ScriptedAskHandler::new(std::iter::empty());
        assert!(h.ask_user("q").is_err(), "exhausted handler should error");
    }

    #[test]
    fn noninteractive_handler_errors_instead_of_blocking() {
        let h = NoninteractiveAskHandler;
        let err = match h.ask_user("anything") {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error from non-interactive handler"),
        };
        assert!(
            err.contains("non-interactive"),
            "error should explain why: {err}"
        );
    }
}
