use crate::error::Result;

/// Result of a multiline input prompt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InputResult {
    /// User confirmed the input with Enter.
    Submitted(String),
    /// User cancelled with Escape or Ctrl+C.
    Cancelled,
}

/// Display a multiline-capable prompt and return the user's input.
///
/// Keys:
/// - Enter         → submit
/// - Shift+Enter   → newline (kitty protocol terminals)
/// - Alt+Enter     → newline (all terminals)
/// - Ctrl+C/Escape → cancel
pub(crate) fn prompt_multiline(message: &str) -> Result<InputResult> {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
        execute,
        terminal::{self, Clear, ClearType},
    };
    use std::io::{Write, stdout};

    // RAII guard: ensures raw mode is disabled even if we return early.
    // Defined before any statements so clippy does not complain about items
    // appearing after statements.
    struct RawModeGuard;
    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
        }
    }

    let mut out = stdout();

    // Print the prompt label on its own line, then save the position where
    // the buffer will be rendered so we can redraw on each keystroke.
    writeln!(out, "{message}")?;
    execute!(out, cursor::SavePosition)?;
    out.flush()?;

    terminal::enable_raw_mode()?;
    let guard = RawModeGuard;

    let mut buf = InputBuffer::new();

    let result = loop {
        // Redraw the entire buffer from the saved position.
        execute!(
            out,
            cursor::RestorePosition,
            Clear(ClearType::FromCursorDown)
        )?;
        for (i, line) in buf.lines.iter().enumerate() {
            if i > 0 {
                write!(out, "\r\n")?;
            }
            write!(out, "{line}")?;
        }

        // Reposition the terminal cursor at (cursor_row, cursor_col).
        execute!(out, cursor::RestorePosition)?;
        if buf.cursor_row > 0 {
            let rows = u16::try_from(buf.cursor_row).unwrap_or(u16::MAX);
            execute!(out, cursor::MoveDown(rows))?;
        }
        execute!(out, cursor::MoveToColumn(buf.display_col()))?;
        out.flush()?;

        let event = event::read()?;
        if let Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) = event
        {
            match (code, modifiers) {
                (KeyCode::Enter, KeyModifiers::NONE) => {
                    if !buf.is_empty_content() {
                        break InputResult::Submitted(buf.text());
                    }
                }
                (KeyCode::Enter, m) if m == KeyModifiers::SHIFT || m == KeyModifiers::ALT => {
                    buf.insert_newline();
                }
                (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                    break InputResult::Cancelled;
                }
                (KeyCode::Left, _) => buf.move_left(),
                (KeyCode::Right, _) => buf.move_right(),
                (KeyCode::Up, _) => buf.move_up(),
                (KeyCode::Down, _) => buf.move_down(),
                (KeyCode::Backspace, _) => buf.delete_char(),
                (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
                    buf.insert_char(c);
                }
                _ => {}
            }
        }
    };

    // `guard` drops here, disabling raw mode.
    drop(guard);

    // Clear the draft and show the final submitted text (or nothing for Cancelled).
    execute!(
        out,
        cursor::RestorePosition,
        Clear(ClearType::FromCursorDown)
    )?;
    if let InputResult::Submitted(ref text) = result {
        writeln!(out, "{text}")?;
    }
    out.flush()?;

    Ok(result)
}

// ─── Internal pure logic (testable without a terminal) ────────────────────────

/// Text buffer with a cursor position for the multiline input widget.
///
/// Lines are stored as a `Vec<String>` so each line can be manipulated
/// independently. The cursor is tracked as `(row, col)` indices into that
/// vector, where `col` is a **char** (Unicode scalar) index, not a byte index.
#[derive(Debug, Clone)]
struct InputBuffer {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

impl InputBuffer {
    /// Create an empty buffer with the cursor at the start.
    fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    /// Create a buffer pre-loaded with `text`, cursor positioned at the end.
    #[cfg(test)]
    fn from_text(text: &str) -> Self {
        if text.is_empty() {
            return Self::new();
        }
        let lines: Vec<String> = text.split('\n').map(ToString::to_string).collect();
        let cursor_row = lines.len() - 1;
        let cursor_col = lines[cursor_row].chars().count();
        Self {
            lines,
            cursor_row,
            cursor_col,
        }
    }

    /// Insert a regular character at the current cursor position.
    fn insert_char(&mut self, ch: char) {
        let byte_pos = self.char_to_byte(self.cursor_row, self.cursor_col);
        self.lines[self.cursor_row].insert(byte_pos, ch);
        self.cursor_col += 1;
    }

    /// Insert a newline at the current cursor position, splitting the current
    /// line into two.
    fn insert_newline(&mut self) {
        let byte_pos = self.char_to_byte(self.cursor_row, self.cursor_col);
        let second_part = self.lines[self.cursor_row].split_off(byte_pos);
        self.lines.insert(self.cursor_row + 1, second_part);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    /// Delete the character immediately before the cursor (backspace).
    /// If the cursor is at the start of a non-first line, the newline is
    /// removed and the line is merged with the previous one.
    fn delete_char(&mut self) {
        if self.cursor_col > 0 {
            let byte_pos = self.char_to_byte(self.cursor_row, self.cursor_col - 1);
            self.lines[self.cursor_row].remove(byte_pos);
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            let current_line = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].chars().count();
            self.lines[self.cursor_row].push_str(&current_line);
        }
    }

    /// Move the cursor one position to the left.
    /// At the start of a non-first line, the cursor wraps to the end of the
    /// previous line.
    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].chars().count();
        }
    }

    /// Move the cursor one position to the right.
    /// At the end of a non-last line, the cursor wraps to the start of the
    /// next line.
    fn move_right(&mut self) {
        let line_len = self.lines[self.cursor_row].chars().count();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// Move the cursor up one line, clamping the column to the line length.
    /// If the cursor is at the end of the current line, it moves to the end of
    /// the previous line. Has no effect when already on the first line.
    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            let current_line_len = self.lines[self.cursor_row].chars().count();
            self.cursor_row -= 1;
            let target_line_len = self.lines[self.cursor_row].chars().count();
            self.cursor_col = self.clamp_col(current_line_len, target_line_len);
        }
    }

    /// Move the cursor down one line, clamping the column to the line length.
    /// If the cursor is at the end of the current line, it moves to the end of
    /// the next line. Has no effect when already on the last line.
    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            let current_line_len = self.lines[self.cursor_row].chars().count();
            self.cursor_row += 1;
            let target_line_len = self.lines[self.cursor_row].chars().count();
            self.cursor_col = self.clamp_col(current_line_len, target_line_len);
        }
    }

    /// Return the full text content, joining lines with `'\n'`.
    fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Return `true` when the entire content is empty or only whitespace.
    /// Leading/trailing blank lines are also considered empty.
    fn is_empty_content(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Return the terminal display column for the current cursor position.
    ///
    /// This differs from `cursor_col` (a char index) for wide characters such
    /// as CJK ideographs that occupy two terminal columns each.
    fn display_col(&self) -> u16 {
        use unicode_width::UnicodeWidthChar;
        let width: usize = self.lines[self.cursor_row]
            .chars()
            .take(self.cursor_col)
            .map(|c| c.width().unwrap_or(0))
            .sum();
        u16::try_from(width).unwrap_or(u16::MAX)
    }

    /// Convert a char-index `col` within line `row` to a byte offset.
    fn char_to_byte(&self, row: usize, col: usize) -> usize {
        self.lines[row]
            .char_indices()
            .nth(col)
            .map_or(self.lines[row].len(), |(i, _)| i)
    }

    /// Compute the new column when moving vertically between lines.
    ///
    /// If the cursor was at the end of `current_line_len`, it snaps to the end
    /// of the target line; otherwise it is clamped to `target_line_len`.
    fn clamp_col(&self, current_line_len: usize, target_line_len: usize) -> usize {
        if self.cursor_col == current_line_len {
            target_line_len
        } else {
            self.cursor_col.min(target_line_len)
        }
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test helpers ───────────────────────────────────────────────────────────

    fn buf_with(text: &str) -> InputBuffer {
        InputBuffer::from_text(text)
    }

    // ── InputResult ────────────────────────────────────────────────────────────

    #[test]
    fn test_input_result_submitted_holds_text() {
        // Given a submitted result with some text
        let result = InputResult::Submitted("hello world".to_string());
        // Then the text is preserved
        assert_eq!(result, InputResult::Submitted("hello world".to_string()));
    }

    #[test]
    fn test_input_result_cancelled_is_distinct() {
        // Given a cancelled result
        let result = InputResult::Cancelled;
        // Then it is not equal to any submitted value
        assert_ne!(result, InputResult::Submitted(String::new()));
        assert_eq!(result, InputResult::Cancelled);
    }

    #[test]
    fn test_input_result_submitted_empty_string() {
        // Given a submitted result with an empty string
        let result = InputResult::Submitted(String::new());
        // Then it is distinct from Cancelled
        assert_ne!(result, InputResult::Cancelled);
    }

    // ── InputBuffer – creation ─────────────────────────────────────────────────

    #[test]
    fn test_new_buffer_starts_empty() {
        // Given a new buffer
        let buf = InputBuffer::new();
        // Then the text is empty and the cursor is at the origin
        assert_eq!(buf.text(), "");
        assert_eq!(buf.cursor_row, 0);
        assert_eq!(buf.cursor_col, 0);
    }

    #[test]
    fn test_new_buffer_has_one_line() {
        // Given a new buffer
        let buf = InputBuffer::new();
        // Then there is exactly one (empty) line
        assert_eq!(buf.lines.len(), 1);
        assert_eq!(buf.lines[0], "");
    }

    // ── InputBuffer – insert_char ──────────────────────────────────────────────

    #[test]
    fn test_insert_single_char_produces_text() {
        let mut buf = InputBuffer::new();
        buf.insert_char('a');
        assert_eq!(buf.text(), "a");
    }

    #[test]
    fn test_insert_single_char_advances_cursor() {
        let mut buf = InputBuffer::new();
        buf.insert_char('x');
        assert_eq!(buf.cursor_col, 1);
        assert_eq!(buf.cursor_row, 0);
    }

    #[test]
    fn test_insert_multiple_chars_builds_word() {
        let buf = buf_with("hello");
        assert_eq!(buf.text(), "hello");
        assert_eq!(buf.cursor_col, 5);
    }

    #[test]
    fn test_insert_char_in_middle_shifts_rest() {
        let mut buf = buf_with("helo");
        buf.move_left();
        buf.insert_char('l');
        assert_eq!(buf.text(), "hello");
        assert_eq!(buf.cursor_col, 4);
    }

    #[test]
    fn test_insert_unicode_char() {
        let mut buf = InputBuffer::new();
        buf.insert_char('あ');
        assert_eq!(buf.text(), "あ");
        assert_eq!(buf.cursor_col, 1);
    }

    // ── InputBuffer – insert_newline ───────────────────────────────────────────

    #[test]
    fn test_insert_newline_at_end_creates_empty_second_line() {
        let mut buf = buf_with("hello");
        buf.insert_newline();
        assert_eq!(buf.lines, vec!["hello", ""]);
        assert_eq!(buf.cursor_row, 1);
        assert_eq!(buf.cursor_col, 0);
    }

    #[test]
    fn test_insert_newline_in_middle_splits_line() {
        let mut buf = buf_with("hello");
        buf.move_left();
        buf.move_left();
        buf.insert_newline();
        assert_eq!(buf.lines, vec!["hel", "lo"]);
        assert_eq!(buf.cursor_row, 1);
        assert_eq!(buf.cursor_col, 0);
    }

    #[test]
    fn test_text_joins_multiple_lines_with_newline() {
        assert_eq!(buf_with("hello\nworld").text(), "hello\nworld");
    }

    // ── InputBuffer – delete_char ──────────────────────────────────────────────

    #[test]
    fn test_delete_char_at_start_of_empty_buffer_does_nothing() {
        let mut buf = InputBuffer::new();
        buf.delete_char();
        assert_eq!(buf.text(), "");
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 0));
    }

    #[test]
    fn test_delete_char_removes_char_before_cursor() {
        let mut buf = buf_with("ab");
        buf.delete_char();
        assert_eq!(buf.text(), "a");
        assert_eq!(buf.cursor_col, 1);
    }

    #[test]
    fn test_delete_char_at_line_start_merges_with_previous_line() {
        let mut buf = buf_with("hello\nworld");
        // cursor is at (1, 5); move to (1, 0)
        for _ in 0..5 {
            buf.move_left();
        }
        buf.delete_char();
        assert_eq!(buf.lines.len(), 1);
        assert_eq!(buf.text(), "helloworld");
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 5));
    }

    #[test]
    fn test_delete_char_in_middle_of_line() {
        let mut buf = buf_with("hello");
        buf.move_left();
        buf.move_left();
        buf.delete_char();
        assert_eq!(buf.text(), "helo");
        assert_eq!(buf.cursor_col, 2);
    }

    // ── InputBuffer – move_left ────────────────────────────────────────────────

    #[test]
    fn test_move_left_at_start_of_buffer_does_nothing() {
        let mut buf = InputBuffer::new();
        buf.move_left();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 0));
    }

    #[test]
    fn test_move_left_moves_cursor_within_line() {
        let mut buf = buf_with("abc");
        buf.move_left();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 2));
    }

    #[test]
    fn test_move_left_at_line_start_wraps_to_end_of_previous_line() {
        let mut buf = buf_with("hello\n");
        // cursor is at (1, 0) after the trailing newline
        assert_eq!((buf.cursor_row, buf.cursor_col), (1, 0));
        buf.move_left();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 5));
    }

    // ── InputBuffer – move_right ───────────────────────────────────────────────

    #[test]
    fn test_move_right_at_end_of_buffer_does_nothing() {
        let mut buf = InputBuffer::new();
        buf.move_right();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 0));
    }

    #[test]
    fn test_move_right_moves_cursor_within_line() {
        let mut buf = buf_with("abc");
        for _ in 0..3 {
            buf.move_left();
        }
        buf.move_right();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 1));
    }

    #[test]
    fn test_move_right_at_line_end_wraps_to_next_line() {
        let mut buf = buf_with("hello\nworld");
        buf.move_up(); // to (0, 5)
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 5));
        buf.move_right();
        assert_eq!((buf.cursor_row, buf.cursor_col), (1, 0));
    }

    // ── InputBuffer – move_up ──────────────────────────────────────────────────

    #[test]
    fn test_move_up_at_first_line_does_nothing() {
        let mut buf = buf_with("hello");
        buf.move_up();
        assert_eq!(buf.cursor_row, 0);
    }

    #[test]
    fn test_move_up_moves_to_previous_line_same_col() {
        let mut buf = buf_with("hello\nworld");
        // cursor at (1, 5); move left twice → (1, 3)
        buf.move_left();
        buf.move_left();
        buf.move_up();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 3));
    }

    #[test]
    fn test_move_up_clamps_col_to_shorter_line() {
        // cursor ends at (1, 5) — end of "world"
        let mut buf = buf_with("hi\nworld");
        buf.move_up();
        // "hi" is len 2, so col clamps to 2
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 2));
    }

    #[test]
    fn test_move_up_snaps_to_end_of_previous_line() {
        // cursor ends at (1, 5) — end of "world" (also end of line)
        // "hello" is len 5, so "snap to end" → (0, 5)
        let mut buf = buf_with("hello\nworld");
        buf.move_up();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 5));
    }

    // ── InputBuffer – move_down ────────────────────────────────────────────────

    #[test]
    fn test_move_down_at_last_line_does_nothing() {
        let mut buf = buf_with("hello");
        buf.move_down();
        assert_eq!(buf.cursor_row, 0);
    }

    #[test]
    fn test_move_down_moves_to_next_line_same_col() {
        let mut buf = buf_with("hello\nworld");
        // move to (0, 3)
        buf.move_up();
        buf.move_left();
        buf.move_left();
        buf.move_down();
        assert_eq!((buf.cursor_row, buf.cursor_col), (1, 3));
    }

    #[test]
    fn test_move_down_clamps_col_to_shorter_line() {
        // cursor at (0, 5) — end of "world"
        let mut buf = buf_with("world\nhi");
        buf.move_up();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 5));
        buf.move_down();
        // "hi" is len 2, so col clamps to 2
        assert_eq!((buf.cursor_row, buf.cursor_col), (1, 2));
    }

    #[test]
    fn test_move_down_snaps_to_end_of_next_line() {
        // cursor at (0, 5) — end of "hello" (also end of line)
        // "world" is len 5, so "snap to end" → (1, 5)
        let mut buf = buf_with("hello\nworld");
        buf.move_up();
        assert_eq!((buf.cursor_row, buf.cursor_col), (0, 5));
        buf.move_down();
        assert_eq!((buf.cursor_row, buf.cursor_col), (1, 5));
    }

    // ── InputBuffer – is_empty_content ────────────────────────────────────────

    #[test]
    fn test_is_empty_content_for_new_buffer() {
        assert!(InputBuffer::new().is_empty_content());
    }

    #[test]
    fn test_is_empty_content_whitespace_only() {
        assert!(buf_with("   ").is_empty_content());
    }

    #[test]
    fn test_is_empty_content_blank_lines_only() {
        assert!(buf_with("\n\n").is_empty_content());
    }

    #[test]
    fn test_is_empty_content_with_actual_text() {
        assert!(!buf_with("hello").is_empty_content());
    }

    #[test]
    fn test_is_empty_content_text_surrounded_by_blank_lines() {
        assert!(!buf_with("\nhi\n").is_empty_content());
    }

    // ── InputBuffer – text() multiline ────────────────────────────────────────

    #[test]
    fn test_text_preserves_internal_newlines() {
        assert_eq!(
            buf_with("line1\nline2\nline3").text(),
            "line1\nline2\nline3"
        );
    }

    #[test]
    fn test_text_single_empty_line_is_empty_string() {
        assert_eq!(InputBuffer::new().text(), "");
    }

    // ── InputBuffer::from_text ─────────────────────────────────────────────────

    #[test]
    fn test_from_text_single_line_cursor_at_end() {
        // Given: initial text "hello"
        let buf = InputBuffer::from_text("hello");
        // Then: text matches
        assert_eq!(buf.text(), "hello");
        // And: cursor is positioned at the end of the line
        assert_eq!(buf.cursor_row, 0);
        assert_eq!(buf.cursor_col, 5);
    }

    #[test]
    fn test_from_text_multiline_cursor_at_end_of_last_line() {
        // Given: multiline initial text
        let buf = InputBuffer::from_text("hello\nworld");
        // Then: text is preserved
        assert_eq!(buf.text(), "hello\nworld");
        // And: cursor is at end of last line
        assert_eq!(buf.cursor_row, 1);
        assert_eq!(buf.cursor_col, 5);
    }

    #[test]
    fn test_from_text_empty_string_same_as_new() {
        // Given: empty initial text
        let buf = InputBuffer::from_text("");
        // Then: text is empty
        assert_eq!(buf.text(), "");
        // And: cursor is at origin
        assert_eq!(buf.cursor_row, 0);
        assert_eq!(buf.cursor_col, 0);
    }

    #[test]
    fn test_from_text_can_append_after_initialization() {
        // Given: buffer initialized with "hello"
        let mut buf = InputBuffer::from_text("hello");
        // When: a character is inserted at the current cursor (end)
        buf.insert_char('!');
        // Then: character is appended
        assert_eq!(buf.text(), "hello!");
    }

    #[test]
    fn test_from_text_three_lines_cursor_at_end() {
        // Given: three-line initial text
        let buf = InputBuffer::from_text("a\nbb\nccc");
        // Then: text preserved and cursor at end of last line
        assert_eq!(buf.text(), "a\nbb\nccc");
        assert_eq!(buf.cursor_row, 2);
        assert_eq!(buf.cursor_col, 3);
    }

    #[test]
    fn test_from_text_unicode_initial_text() {
        // Given: initial text with CJK characters
        let buf = InputBuffer::from_text("あいう");
        // Then: text preserved and cursor at char position 3 (not byte position)
        assert_eq!(buf.text(), "あいう");
        assert_eq!(buf.cursor_col, 3);
    }
}
