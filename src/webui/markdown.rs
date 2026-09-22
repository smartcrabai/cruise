use pulldown_cmark::{Event, Options, Parser};

pub(crate) fn render_markdown(md: &str) -> String {
    let parser = Parser::new_ext(
        md,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    );
    let events = parser.map(|event| match event {
        Event::Html(html) | Event::InlineHtml(html) => Event::Text(html),
        event => event,
    });
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, events);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_raw_html_as_escaped_text() {
        let rendered = render_markdown("<script>alert(1)</script>");
        assert!(rendered.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }

    #[test]
    fn renders_gfm_tables() {
        let rendered = render_markdown("| A | B |\n| --- | --- |\n| 1 | 2 |");
        assert!(rendered.contains("<table>"));
    }

    #[test]
    fn renders_fenced_code_blocks() {
        let rendered = render_markdown("```rust\nlet answer = 42;\n```");
        assert!(rendered.contains("<pre><code"));
    }
}
