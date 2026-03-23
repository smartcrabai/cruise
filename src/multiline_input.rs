use crate::error::Result;
use unicode_width::UnicodeWidthChar;

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
    /// `Submitted(text)` → `Ok(text)` preserving internal newlines.
    /// `Cancelled`       → `Err(CruiseError::Other("input cancelled"))`.
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

    let mut term_width = terminal::size().map(|(w, _)| w).unwrap_or(80);

    let result = loop {
        // Redraw the entire buffer from the saved position.
        execute!(
            out,
            cursor::RestorePosition,
            Clear(ClearType::FromCursorDown)
        )?;
        for (i, row) in buf.physical_rows(term_width).iter().enumerate() {
            if i > 0 {
                write!(out, "\r\n")?;
            }
            write!(out, "{row}")?;
        }

        // Reposition the terminal cursor, accounting for line wrapping.
        execute!(out, cursor::RestorePosition)?;
        let (phys_row, phys_col) = buf.wrapped_cursor_pos(term_width);
        if phys_row > 0 {
            execute!(out, cursor::MoveDown(phys_row))?;
        }
        execute!(out, cursor::MoveToColumn(phys_col))?;
        out.flush()?;

        let event = event::read()?;
        if let Event::Resize(w, _) = event {
            term_width = w;
            continue;
        }
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

    /// Return the display width of the first `char_limit` characters in line `row`.
    fn display_width_up_to(&self, row: usize, char_limit: usize) -> usize {
        self.lines[row]
            .chars()
            .take(char_limit)
            .map(|c| c.width().unwrap_or(0))
            .sum()
    }

    /// Return the terminal display column for the current cursor position.
    ///
    /// This differs from `cursor_col` (a char index) for wide characters such
    /// as CJK ideographs that occupy two terminal columns each.
    fn display_col(&self) -> usize {
        self.display_width_up_to(self.cursor_row, self.cursor_col)
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

    /// Return the terminal display width of line `row` (full line, not up to cursor).
    fn line_display_width(&self, row: usize) -> usize {
        self.display_width_up_to(row, usize::MAX)
    }

    /// Split all logical lines into physical rows based on terminal width.
    ///
    /// Each logical line is wrapped at `term_width` display columns, producing
    /// one or more physical rows.  An empty logical line contributes exactly one
    /// empty physical row.  A line whose display width equals `term_width`
    /// exactly also contributes exactly one row — consistent with the terminal's
    /// delayed auto-wrap behaviour.
    fn physical_rows(&self, term_width: u16) -> Vec<String> {
        let tw = usize::from(term_width);
        let mut result = Vec::with_capacity(self.lines.len());

        for line in &self.lines {
            let mut current_row = String::with_capacity(line.len());
            let mut current_width: usize = 0;

            for ch in line.chars() {
                let ch_width = ch.width().unwrap_or(0);
                if current_width > 0 && current_width + ch_width > tw {
                    result.push(current_row);
                    current_row = String::new();
                    current_width = 0;
                }
                current_row.push(ch);
                current_width += ch_width;
            }
            result.push(current_row);
        }

        result
    }

    /// Return the physical (row, col) offset from the saved cursor position,
    /// accounting for terminal line wrapping.
    ///
    /// When `term_width` is zero, returns `(0, 0)`.
    fn wrapped_cursor_pos(&self, term_width: u16) -> (u16, u16) {
        if term_width == 0 {
            return (0, 0);
        }
        let tw = usize::from(term_width);
        let mut physical_row: usize = 0;

        // Each logical line before the cursor contributes at least 1 row
        // (the newline itself), plus extra rows from terminal wrap
        // (using the delayed auto-wrap formula).
        for i in 0..self.cursor_row {
            let w = self.line_display_width(i);
            physical_row += (if w > 0 { (w - 1) / tw } else { 0 }) + 1;
        }

        let display = self.display_col();
        physical_row += display / tw;
        let physical_col = display % tw;

        (
            u16::try_from(physical_row).unwrap_or(u16::MAX),
            u16::try_from(physical_col).unwrap_or(u16::MAX),
        )
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

    // ── InputResult::into_result ─────────────────────────────────────────────

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
        // Given: a Submitted result with an empty string (case where empty input was submitted)
        let result = InputResult::Submitted(String::new()).into_result();
        // Then: returns Ok("") (empty string is preserved)
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "");
    }

    #[test]
    fn test_into_result_cancelled_returns_err() {
        // Given: the user cancelled with Esc/Ctrl+C
        let result = InputResult::Cancelled.into_result();
        // Then: returns Err, allowing processing to stop before session creation
        assert!(result.is_err(), "Cancelled should produce Err, got Ok");
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
        buf.insert_char('\u{3042}');
        assert_eq!(buf.text(), "\u{3042}");
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
        let buf = InputBuffer::from_text("\u{3042}\u{3044}\u{3046}");
        // Then: text preserved and cursor at char position 3 (not byte position)
        assert_eq!(buf.text(), "\u{3042}\u{3044}\u{3046}");
        assert_eq!(buf.cursor_col, 3);
    }

    // ── InputBuffer – line_display_width ──────────────────────────────────────

    #[test]
    fn test_line_display_width_empty_line_returns_zero() {
        assert_eq!(InputBuffer::new().line_display_width(0), 0);
    }

    #[test]
    fn test_line_display_width_ascii_line_equals_char_count() {
        assert_eq!(buf_with("hello").line_display_width(0), 5);
    }

    #[test]
    fn test_line_display_width_cjk_is_double_char_count() {
        assert_eq!(
            buf_with("\u{3042}\u{3044}\u{3046}").line_display_width(0),
            6
        );
    }

    #[test]
    fn test_line_display_width_mixed_ascii_and_cjk() {
        assert_eq!(buf_with("a\u{3042}").line_display_width(0), 3);
    }

    #[test]
    fn test_line_display_width_selects_correct_row_in_multiline_buffer() {
        let buf = buf_with("hi\nhello");
        assert_eq!(buf.line_display_width(0), 2);
        assert_eq!(buf.line_display_width(1), 5);
    }

    // ── InputBuffer – wrapped_cursor_pos ──────────────────────────────────────

    #[test]
    fn test_wrapped_cursor_pos_within_term_width() {
        let buf = buf_with("hello");
        assert_eq!(buf.wrapped_cursor_pos(10), (0, 5));
    }

    #[test]
    fn test_wrapped_cursor_pos_exactly_at_term_width() {
        let buf = buf_with("hellohello");
        // 10 % 10 = 0, 10 / 10 = 1
        assert_eq!(buf.wrapped_cursor_pos(10), (1, 0));
    }

    #[test]
    fn test_wrapped_cursor_pos_past_term_width() {
        let buf = buf_with("hellohelloabc");
        // 13 / 10 = 1, 13 % 10 = 3
        assert_eq!(buf.wrapped_cursor_pos(10), (1, 3));
    }

    #[test]
    fn test_wrapped_cursor_pos_cjk_wraps() {
        let buf = buf_with("\u{3042}\u{3042}\u{3042}\u{3042}\u{3042}"); // display width 10
        assert_eq!(buf.wrapped_cursor_pos(10), (1, 0));
    }

    #[test]
    fn test_wrapped_cursor_pos_cursor_in_middle_of_long_line() {
        let mut buf = buf_with("hellohelloabc");
        for _ in 0..8 {
            buf.move_left();
        }
        // display_col = 5, so (5/10, 5%10) = (0, 5)
        assert_eq!(buf.wrapped_cursor_pos(10), (0, 5));
    }

    #[test]
    fn test_wrapped_cursor_pos_term_width_zero_does_not_panic() {
        assert_eq!(buf_with("hello").wrapped_cursor_pos(0), (0, 0));
    }

    #[test]
    fn test_wrapped_cursor_pos_single_line_two_wraps() {
        let buf = buf_with("hellohellohellohelloX"); // 21 chars
        // 21 / 10 = 2, 21 % 10 = 1
        assert_eq!(buf.wrapped_cursor_pos(10), (2, 1));
    }

    #[test]
    fn test_wrapped_cursor_pos_two_lines_no_wrap() {
        let buf = buf_with("hello\nworld");
        // line 0 (w=5): (5-1)/80+1 = 1 row; cursor display 5: 5/80 = 0
        assert_eq!(buf.wrapped_cursor_pos(80), (1, 5));
    }

    #[test]
    fn test_wrapped_cursor_pos_previous_line_wraps_once() {
        let buf = buf_with("hellohellox\nhi");
        // line 0 (w=11): (11-1)/10+1 = 2 rows; cursor display 2: 2/10 = 0
        assert_eq!(buf.wrapped_cursor_pos(10), (2, 2));
    }

    #[test]
    fn test_wrapped_cursor_pos_previous_line_exactly_at_term_width() {
        // Delayed auto-wrap: line exactly term_width wide doesn't add an extra row
        let buf = buf_with("hellohello\nhi");
        // line 0 (w=10): (10-1)/10+1 = 1 row; cursor display 2: 2/10 = 0
        assert_eq!(buf.wrapped_cursor_pos(10), (1, 2));
    }

    #[test]
    fn test_wrapped_cursor_pos_cursor_line_itself_wraps() {
        let buf = buf_with("hi\nhellohelloabc");
        // line 0 (w=2): 1 row; cursor display 13: row 13/10=1, col 13%10=3
        assert_eq!(buf.wrapped_cursor_pos(10), (2, 3));
    }

    #[test]
    fn test_wrapped_cursor_pos_three_lines_all_wrapping() {
        let buf = buf_with("hellohellox\nhellohellox\nhellohellox");
        // lines 0,1 (w=11 each): (11-1)/10+1 = 2 rows each = 4
        // cursor display 11: row 11/10=1, col 11%10=1
        assert_eq!(buf.wrapped_cursor_pos(10), (5, 1));
    }

    // ── InputBuffer – physical_rows ────────────────────────────────────────────

    #[test]
    fn test_physical_rows_empty_buffer_single_empty_row() {
        // one row so the cursor has somewhere to be
        assert_eq!(InputBuffer::new().physical_rows(80), vec![String::new()]);
    }

    #[test]
    fn test_physical_rows_single_short_line_no_wrap() {
        assert_eq!(
            buf_with("hello").physical_rows(10),
            vec!["hello".to_string()]
        );
    }

    #[test]
    fn test_physical_rows_single_line_exactly_at_term_width_no_extra_row() {
        // delayed auto-wrap: filling the last column doesn't start a new row yet
        assert_eq!(
            buf_with("hellohello").physical_rows(10),
            vec!["hellohello".to_string()]
        );
    }

    #[test]
    fn test_physical_rows_single_line_one_char_past_term_width() {
        assert_eq!(
            buf_with("hellohelloX").physical_rows(10),
            vec!["hellohello".to_string(), "X".to_string()]
        );
    }

    #[test]
    fn test_physical_rows_cjk_exactly_at_term_width_no_extra_row() {
        // 5 CJK chars = 10 display columns (each occupies 2 terminal columns)
        // sakoku-ignore-next-line
        let buf = buf_with("あああああ");
        let rows = buf.physical_rows(10);
        // sakoku-ignore-next-line
        assert_eq!(rows, vec!["あああああ".to_string()]);
    }

    #[test]
    fn test_physical_rows_cjk_one_past_term_width() {
        // 5 CJK (10 cols) + "X" wraps to row 1
        // sakoku-ignore-next-line
        let buf = buf_with("あああああX");
        let rows = buf.physical_rows(10);
        // sakoku-ignore-next-line
        assert_eq!(rows, vec!["あああああ".to_string(), "X".to_string()]);
    }

    #[test]
    fn test_physical_rows_multiline_no_wrapping() {
        assert_eq!(
            buf_with("hello\nworld").physical_rows(80),
            vec!["hello".to_string(), "world".to_string()]
        );
    }

    #[test]
    fn test_physical_rows_first_line_exactly_at_term_width_second_line_below() {
        // regression: line at exactly term_width must NOT emit a blank row
        assert_eq!(
            buf_with("hellohello\nhi").physical_rows(10),
            vec!["hellohello".to_string(), "hi".to_string()]
        );
    }

    #[test]
    fn test_physical_rows_long_line_wraps_twice() {
        // 21 chars → 10 + 10 + 1
        assert_eq!(
            buf_with("hellohellohellohelloX").physical_rows(10),
            vec![
                "hellohello".to_string(),
                "hellohello".to_string(),
                "X".to_string(),
            ]
        );
    }
}
