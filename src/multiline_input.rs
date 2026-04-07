use std::borrow::Cow;

use crate::error::Result;
use reedline::{
    default_emacs_keybindings, Emacs, KeyCode, KeyModifiers, Keybindings, Prompt, PromptEditMode,
    PromptHistorySearch, Reedline, ReedlineEvent, Signal,
};

/// Result of a multiline input prompt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InputResult {
    /// User confirmed the input with Enter.
    Submitted(String),
    /// User cancelled with Escape or Ctrl+C.
    Cancelled,
}

impl InputResult {
    /// Convert into a plain `Result<String>`.
    ///
    /// `Submitted(text)` -> `Ok(text)` preserving internal newlines.
    /// `Cancelled`       -> `Err(CruiseError::StepPaused)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the user cancelled the input.
    pub(crate) fn into_result(self) -> crate::error::Result<String> {
        match self {
            InputResult::Submitted(text) => Ok(text),
            InputResult::Cancelled => Err(crate::error::CruiseError::StepPaused),
        }
    }
}

/// Minimal Cruise prompt: no left/right content, just a `> ` indicator.
struct CruisePrompt;

impl Prompt for CruisePrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _prompt_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("> ")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("... ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        _history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        Cow::Borrowed("")
    }
}

/// Display a multiline-capable prompt and return the user's input.
///
/// Keys:
/// - Enter           -> submit (rejected if blank)
/// - Alt+Enter       -> newline
/// - Shift+Enter     -> newline (kitty protocol terminals)
/// - Ctrl+C / Esc    -> cancel
///
/// # Errors
///
/// Returns `Err(CruiseError::IoError)` on terminal I/O failure.
pub(crate) fn prompt_multiline(message: &str) -> Result<InputResult> {
    println!("{message}");
    let kb = build_keybindings();
    let mut editor = build_reedline(kb);
    let prompt = CruisePrompt;

    loop {
        let signal = editor.read_line(&prompt)?;
        if let Some(result) = map_signal(signal) {
            return Ok(result);
        }
    }
}

/// Create the [`Reedline`] editor used by [`prompt_multiline`].
///
/// Enables the kitty keyboard enhancement protocol so that terminals that
/// support it (e.g. kitty, `WezTerm`, foot) can distinguish `Shift+Enter` from
/// plain `Enter`.  On terminals without support the enhancement is silently
/// ignored by reedline.
fn build_reedline(kb: Keybindings) -> Reedline {
    let edit_mode = Box::new(Emacs::new(kb));
    Reedline::create()
        .use_kitty_keyboard_enhancement(true)
        .with_edit_mode(edit_mode)
}

/// Build the Cruise-specific keybindings on top of the default Emacs set.
///
/// Changes from the Emacs defaults:
/// - `Esc` is remapped from [`ReedlineEvent::Esc`] to [`ReedlineEvent::CtrlC`]
///   so that it cancels input consistently, matching Ctrl+C behaviour.
/// - `Alt+Enter` and `Shift+Enter` already emit `InsertNewline` in the Emacs
///   defaults and are left unchanged.  For `Shift+Enter` to fire in practice
///   the caller must enable kitty keyboard enhancement; see [`build_reedline`].
fn build_keybindings() -> Keybindings {
    let mut kb = default_emacs_keybindings();
    kb.add_binding(KeyModifiers::NONE, KeyCode::Esc, ReedlineEvent::CtrlC);
    kb
}

/// Map a reedline [`Signal`] to an [`InputResult`].
///
/// Returns `None` when the submitted text is blank (empty or whitespace-only),
/// signalling the caller to discard the submission and prompt again.
fn map_signal(signal: Signal) -> Option<InputResult> {
    match signal {
        Signal::Success(text) if text.trim().is_empty() => None,
        Signal::Success(text) => Some(InputResult::Submitted(text)),
        Signal::CtrlC | Signal::CtrlD => Some(InputResult::Cancelled),
    }
}

// --- Tests ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::{EditCommand, KeyCode, KeyModifiers, ReedlineEvent, Signal};

    // -- InputResult::into_result ----------------------------------------------

    #[test]
    fn test_into_result_submitted_returns_text() {
        // Given: a Submitted InputResult
        let result = InputResult::Submitted("add feature X".to_string()).into_result();
        // Then: returns Ok with the same string
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "add feature X");
    }

    #[test]
    fn test_into_result_submitted_multiline_preserved() {
        // Given: a Submitted result with multiline text
        let multiline = "line1\nline2\nline3".to_string();
        let result = InputResult::Submitted(multiline.clone()).into_result();
        // Then: returns Ok with internal newlines preserved
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), multiline);
    }

    #[test]
    fn test_into_result_submitted_empty_string_returns_ok() {
        // Given: a Submitted result with an empty string (edge case)
        let result = InputResult::Submitted(String::new()).into_result();
        // Then: returns Ok("") -- into_result does not validate content
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "");
    }

    #[test]
    fn test_into_result_cancelled_returns_step_paused_err() {
        // Given: the user cancelled with Esc / Ctrl+C
        let result = InputResult::Cancelled.into_result();
        // Then: returns Err(StepPaused), allowing callers to stop processing
        assert!(
            matches!(result, Err(crate::error::CruiseError::StepPaused)),
            "expected Err(StepPaused), got {result:?}"
        );
    }

    // -- map_signal ------------------------------------------------------------

    #[test]
    fn test_map_signal_success_nonempty_returns_submitted() {
        // Given: reedline emits Success with non-empty text
        let sig = Signal::Success("hello world".to_string());
        // When: mapped to InputResult
        let result = map_signal(sig);
        // Then: returns Some(Submitted) with text preserved
        assert_eq!(
            result,
            Some(InputResult::Submitted("hello world".to_string()))
        );
    }

    #[test]
    fn test_map_signal_success_multiline_returns_submitted() {
        // Given: Signal::Success with multiline text (user pressed Alt+Enter)
        let sig = Signal::Success("line1\nline2".to_string());
        // When: mapped
        let result = map_signal(sig);
        // Then: internal newlines are preserved in Submitted
        assert_eq!(
            result,
            Some(InputResult::Submitted("line1\nline2".to_string()))
        );
    }

    #[test]
    fn test_map_signal_success_empty_returns_none() {
        // Given: Signal::Success with an empty string (user pressed Enter immediately)
        let sig = Signal::Success(String::new());
        // When: mapped
        let result = map_signal(sig);
        // Then: returns None so the caller retries
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_signal_success_whitespace_only_returns_none() {
        // Given: Signal::Success with only spaces and tabs
        let sig = Signal::Success("   \t  ".to_string());
        // When: mapped
        let result = map_signal(sig);
        // Then: returns None -- blank submission is rejected
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_signal_success_blank_multiline_returns_none() {
        // Given: Signal::Success with only whitespace across multiple lines
        let sig = Signal::Success("  \n  \n  ".to_string());
        // When: mapped
        let result = map_signal(sig);
        // Then: returns None -- all-blank multiline is rejected
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_signal_ctrl_c_returns_cancelled() {
        // Given: the user pressed Ctrl+C
        // When: Signal::CtrlC is mapped
        let result = map_signal(Signal::CtrlC);
        // Then: returns Some(Cancelled)
        assert_eq!(result, Some(InputResult::Cancelled));
    }

    #[test]
    fn test_map_signal_ctrl_d_returns_cancelled() {
        // Given: the user pressed Ctrl+D (EOF)
        // When: Signal::CtrlD is mapped
        let result = map_signal(Signal::CtrlD);
        // Then: returns Some(Cancelled)
        assert_eq!(result, Some(InputResult::Cancelled));
    }

    // -- build_keybindings -----------------------------------------------------

    #[test]
    fn test_keybindings_alt_enter_is_insert_newline() {
        // Given: the Cruise keybindings
        let kb = build_keybindings();
        // When: looking up Alt+Enter
        let binding = kb.find_binding(KeyModifiers::ALT, KeyCode::Enter);
        // Then: it inserts a newline (not submit)
        assert_eq!(
            binding,
            Some(ReedlineEvent::Edit(vec![EditCommand::InsertNewline]))
        );
    }

    #[test]
    fn test_keybindings_shift_enter_is_insert_newline() {
        // Given: the Cruise keybindings
        let kb = build_keybindings();
        // When: looking up Shift+Enter
        let binding = kb.find_binding(KeyModifiers::SHIFT, KeyCode::Enter);
        // Then: it inserts a newline (not submit)
        assert_eq!(
            binding,
            Some(ReedlineEvent::Edit(vec![EditCommand::InsertNewline]))
        );
    }

    #[test]
    fn test_keybindings_esc_maps_to_ctrl_c() {
        // Given: the Cruise keybindings
        let kb = build_keybindings();
        // When: looking up Esc
        let binding = kb.find_binding(KeyModifiers::NONE, KeyCode::Esc);
        // Then: Esc triggers cancel, remapped to CtrlC for consistent semantics
        assert_eq!(binding, Some(ReedlineEvent::CtrlC));
    }
}
