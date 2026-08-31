use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// High-level keyboard actions understood by the TUI reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    ViewSessions,
    ViewNewSession,
    ViewRunAll,
    Refresh,
    Help,
    TabNext,
    TabPrevious,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    DetailPrevious,
    DetailNext,
    Palette,
    Open,
    Follow,
    Enter,
    Escape,
    Backspace,
    Character(char),
    None,
}

/// Translate a crossterm event into a stable action.  Text editing is left to
/// the focused textarea; this function deliberately contains no application
/// state or side effects.
#[must_use]
pub fn action_for(event: KeyEvent) -> Action {
    if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    match event.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('1') => Action::ViewSessions,
        KeyCode::Char('2') => Action::ViewNewSession,
        KeyCode::Char('3') => Action::ViewRunAll,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('?') => Action::Help,
        KeyCode::Tab => Action::TabNext,
        KeyCode::BackTab => Action::TabPrevious,
        KeyCode::Up | KeyCode::Char('k') => Action::Up,
        KeyCode::Down | KeyCode::Char('j') => Action::Down,
        KeyCode::Left => Action::Left,
        KeyCode::Right => Action::Right,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Home => Action::Home,
        KeyCode::End => Action::End,
        KeyCode::Char('[') => Action::DetailPrevious,
        KeyCode::Char(']') => Action::DetailNext,
        KeyCode::Char('a') => Action::Palette,
        KeyCode::Char('o') => Action::Open,
        KeyCode::Char('f') => Action::Follow,
        KeyCode::Enter => Action::Enter,
        KeyCode::Esc => Action::Escape,
        KeyCode::Backspace => Action::Backspace,
        KeyCode::Char(c) => Action::Character(c),
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn navigation_keys_are_stable() {
        assert_eq!(action_for(key(KeyCode::Char('j'))), Action::Down);
        assert_eq!(action_for(key(KeyCode::Char('k'))), Action::Up);
        assert_eq!(action_for(key(KeyCode::Char(']'))), Action::DetailNext);
    }

    #[test]
    fn ctrl_c_is_quit_even_when_text_editing() {
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }
}
