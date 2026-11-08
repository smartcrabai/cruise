//! TSX template rendering through `jsxrs`.
//!
//! Sources are read on each render so development changes are reflected immediately.
use crate::error::{CruiseError, Result};
use jsxrs::RenderConfig;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Pre-rendered HTML passed to jsxrs as `{"__html": "..."}`.
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
/// Escape text composed into HTML without a template.
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
    locale: crate::i18n::Locale,
}
impl Templates {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            locale: crate::i18n::Locale::En,
        }
    }
    pub(crate) fn with_locale(&self, locale: crate::i18n::Locale) -> Self {
        Self {
            dir: self.dir.clone(),
            locale,
        }
    }
    pub(crate) fn locale(&self) -> crate::i18n::Locale {
        self.locale
    }
    fn props(&self, props: &impl Serialize) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(props)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("t".into(), crate::i18n::webui_props(self.locale).clone());
            object.insert("locale".into(), serde_json::json!(self.locale.tag()));
            object.insert("languages".into(), serde_json::Value::Array(crate::i18n::Locale::ALL.into_iter().map(|locale| serde_json::json!({"tag": locale.tag(), "name": locale.native_name(), "selected": locale == self.locale})).collect()));
        }
        Ok(value)
    }
    fn config(&self, path: &Path, fragment: bool) -> RenderConfig {
        RenderConfig {
            base_dir: Some(path.parent().unwrap_or(&self.dir).to_path_buf()),
            fragment,
            ..RenderConfig::default()
        }
    }
    pub(crate) fn render(&self, name: &str, props: &impl Serialize) -> Result<String> {
        let value = self.props(props)?;
        let path = self.dir.join(format!("{name}.tsx"));
        jsxrs::render_file(&path, &value, &self.config(&path, true))
            .map_err(|error| CruiseError::Other(format!("template {name}: {error}")))
    }
    pub(crate) fn render_document(&self, props: &impl Serialize) -> Result<String> {
        let value = self.props(props)?;
        let path = self.dir.join("layout.tsx");
        jsxrs::render_file(&path, &value, &self.config(&path, false))
            .map(|html| {
                html.replacen(
                    "<html>",
                    &format!("<html lang=\"{}\">", self.locale.tag()),
                    1,
                )
            })
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
