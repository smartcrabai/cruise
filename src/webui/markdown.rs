use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub(crate) fn render_markdown(md: &str) -> String {
    let parser = Parser::new_ext(
        md,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    );
    let mut strip_link_end = false;
    let mut strip_image_end = false;
    let events = parser.filter_map(|event| match event {
        Event::Start(Tag::Link { dest_url, .. }) if !is_safe_target(&dest_url) => {
            strip_link_end = true;
            None
        }
        Event::End(TagEnd::Link) if strip_link_end => {
            strip_link_end = false;
            None
        }
        Event::Start(Tag::Image { dest_url, .. }) if !is_safe_target(&dest_url) => {
            strip_image_end = true;
            None
        }
        Event::End(TagEnd::Image) if strip_image_end => {
            strip_image_end = false;
            None
        }
        event => Some(match event {
            Event::Html(html) | Event::InlineHtml(html) => Event::Text(html),
            event => event,
        }),
    });
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, events);
    output
}

fn is_safe_target(target: &str) -> bool {
    let path_start = target.find(['/', '?', '#']).unwrap_or(target.len());
    let Some((scheme, _)) = target[..path_start].split_once(':') else {
        return true;
    };
    scheme.eq_ignore_ascii_case("http")
        || scheme.eq_ignore_ascii_case("https")
        || scheme.eq_ignore_ascii_case("mailto")
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
    #[test]
    fn unsafe_link_and_image_targets_render_as_text() {
        let rendered = render_markdown(
            "[javascript link](javascript:alert(1)) [data link](data:text/html,evil) ![image text](data:image/png;base64,AAAA)",
        );
        assert!(rendered.contains("javascript link"), "{rendered}");
        assert!(rendered.contains("data link"), "{rendered}");
        assert!(rendered.contains("image text"), "{rendered}");
        assert!(!rendered.contains("<a "), "{rendered}");
        assert!(!rendered.contains("<img "), "{rendered}");
        assert!(!rendered.contains("javascript:"), "{rendered}");
        assert!(!rendered.contains("data:text/html"), "{rendered}");
        assert!(!rendered.contains("data:image"), "{rendered}");
    }

    #[test]
    fn safe_link_schemes_and_relative_targets_render_as_links() {
        for target in [
            "http://example.test",
            "https://example.test",
            "mailto:hello@example.test",
            "/relative/path",
            "../relative/path",
            "#anchor",
        ] {
            let rendered = render_markdown(&format!("[link]({target})"));
            assert!(rendered.contains("<a href="), "{target}: {rendered}");
        }
    }
}
