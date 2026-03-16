use console::{Term, measure_text_width, style};

/// Print `text` inside a Unicode box-drawing border.
///
/// The border adapts to the current terminal width (capped at 100 columns).
/// An optional `title` is embedded in the top border line.
pub fn print_bordered(text: &str, title: Option<&str>) {
    let term_width = Term::stdout().size().1 as usize;
    let box_width = term_width.clamp(20, 100);
    let content_width = box_width - 4; // "│ " + content + " │"

    let bar = "─".repeat(box_width - 2);

    // Top border
    let top = match title {
        Some(t) => {
            let label = format!(" {t} ");
            let label_width = measure_text_width(&label);
            let remaining = box_width.saturating_sub(3 + label_width);
            format!(
                "{}{}{}{}",
                style("┌─").cyan(),
                style(&label).cyan().bold(),
                style("─".repeat(remaining)).cyan(),
                style("┐").cyan(),
            )
        }
        None => format!(
            "{}{}{}",
            style("┌").cyan(),
            style(&bar).cyan(),
            style("┐").cyan()
        ),
    };

    // Bottom border
    let bottom = format!(
        "{}{}{}",
        style("└").cyan(),
        style(&bar).cyan(),
        style("┘").cyan()
    );

    println!();
    println!("{top}");

    for line in text.lines() {
        for chunk in wrap_line(line, content_width) {
            let visible_width = measure_text_width(&chunk);
            let pad = content_width.saturating_sub(visible_width);
            println!(
                "{} {}{} {}",
                style("│").cyan(),
                chunk,
                " ".repeat(pad),
                style("│").cyan(),
            );
        }
    }

    println!("{bottom}");
}

/// Truncate `s` to `max` characters (first line only), appending `…` if truncated.
#[must_use]
pub fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.chars().count() <= max {
        first_line.to_string()
    } else {
        format!("{}…", first_line.chars().take(max).collect::<String>())
    }
}

/// Wrap a single line into chunks of at most `width` visible characters.
fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for word in line.split_inclusive(' ') {
        let word_width = measure_text_width(word);

        // If a single word is wider than the width, split by characters.
        if word_width > width && current.is_empty() {
            for ch in word.chars() {
                let mut buf = [0u8; 4];
                let ch_str = ch.encode_utf8(&mut buf);
                let ch_width = measure_text_width(ch_str);
                if current_width + ch_width > width && !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                current.push(ch);
                current_width += ch_width;
            }
            continue;
        }

        if current_width + word_width > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push_str(word);
        current_width += word_width;
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_line_short() {
        let result = wrap_line("hello world", 80);
        assert_eq!(result, vec!["hello world"]);
    }

    #[test]
    fn wrap_line_empty() {
        let result = wrap_line("", 80);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn wrap_line_exact_width() {
        let result = wrap_line("abcd", 4);
        assert_eq!(result, vec!["abcd"]);
    }

    #[test]
    fn wrap_line_overflow() {
        let result = wrap_line("hello world foo", 10);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "hello ");
        assert_eq!(result[1], "world foo");
    }

    #[test]
    fn wrap_line_long_word() {
        let result = wrap_line("abcdefghij", 4);
        assert_eq!(result, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn test_truncate_short_string() {
        // max より短い文字列はそのまま返る
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact_length() {
        // ちょうど max の文字列はそのまま返る
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        // max を超えると末尾に `…` が付与される
        let result = truncate("hello world", 5);
        assert_eq!(result, "hello…");
    }

    #[test]
    fn test_truncate_multiline() {
        // 複数行の場合は最初の行のみを使用
        let result = truncate("first line\nsecond line", 20);
        assert_eq!(result, "first line");
    }

    // ── truncate ──────────────────────────────────────────────────────────────

    #[test]
    fn test_truncate_short_single_line_returns_as_is() {
        // Given: max より短い単一行
        let result = truncate("hello", 80);
        // Then: そのまま返る
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_long_single_line_appends_ellipsis() {
        // Given: max を超える単一行
        let result = truncate("abcde", 3);
        // Then: 3 文字 + "…" で切り詰められる
        assert_eq!(result, "abc…");
    }

    #[test]
    fn test_truncate_multiline_returns_first_line_only() {
        // Given: 複数行入力（セッション input が multiline になった場合を想定）
        let result = truncate("line1\nline2\nline3", 80);
        // Then: 最初の行のみ返る
        assert_eq!(result, "line1");
    }

    #[test]
    fn test_truncate_multiline_long_first_line_truncated() {
        // Given: 長い第 1 行 + 短い第 2 行
        let first = "a".repeat(100);
        let input = format!("{first}\nshort");
        let result = truncate(&input, 10);
        // Then: 10 文字 + "…" で切り詰められ、"short" は含まれない
        assert_eq!(result, format!("{}…", "a".repeat(10)));
        assert!(!result.contains("short"));
    }

    #[test]
    fn test_truncate_trims_leading_trailing_whitespace() {
        // Given: 先頭・末尾に空白のある入力
        let result = truncate("  hello  ", 80);
        // Then: trim された文字列が返る
        assert_eq!(result, "hello");
    }
}
