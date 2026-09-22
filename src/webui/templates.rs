//! TSX template rendering through `jsxrs`.
//!
//! Templates are layout only: every value they display is precomputed by
//! [`super::view`]. Sources are read from disk on every render, which keeps the
//! `--webui-dir` development loop instant and costs little for a single-user
//! local UI.

use std::path::PathBuf;

use jsxrs::RenderConfig;
use serde::Serialize;

use crate::error::{CruiseError, Result};

/// Pre-rendered HTML passed to a template as `{"__html": "..."}`, the only
/// escape hatch jsxrs offers for composing fragments.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub(crate) struct RawHtml {
    #[serde(rename = "__html")]
    html: String,
}

impl RawHtml {
    pub(crate) fn new(html: impl Into<String>) -> Self {
        Self { html: html.into() }
    }

    pub(crate) fn empty() -> Self {
        Self {
            html: String::new(),
        }
    }
}

/// HTML-escapes text that Rust composes without a template.
pub(crate) fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[derive(Debug, Clone)]
pub(crate) struct Templates {
    dir: PathBuf,
}

impl Templates {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn config(&self, fragment: bool) -> RenderConfig {
        RenderConfig {
            base_dir: Some(self.dir.clone()),
            fragment,
            ..RenderConfig::default()
        }
    }

    /// Render `<name>.tsx` as a body-only fragment.
    ///
    /// # Errors
    ///
    /// Returns an error when the props cannot be serialized or the template
    /// cannot be parsed or rendered.
    pub(crate) fn render(&self, name: &str, props: &impl Serialize) -> Result<String> {
        let value = serde_json::to_value(props)?;
        let path = self.dir.join(format!("{name}.tsx"));
        jsxrs::render_file(&path, &value, &self.config(true))
            .map_err(|error| CruiseError::Other(format!("template {name}: {error}")))
    }

    /// Render `layout.tsx` as a complete HTML document.
    ///
    /// # Errors
    ///
    /// Returns an error when the props cannot be serialized or the template
    /// cannot be parsed or rendered.
    pub(crate) fn render_document(&self, props: &impl Serialize) -> Result<String> {
        let value = serde_json::to_value(props)?;
        let path = self.dir.join("layout.tsx");
        jsxrs::render_file(&path, &value, &self.config(false))
            .map_err(|error| CruiseError::Other(format!("template layout: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::escape_html;

    #[test]
    fn escape_html_escapes_every_markup_character() {
        assert_eq!(
            escape_html("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
    }
}
