//! Tailwind-compatible CSS generated from the TSX templates with `encre-css`.
//!
//! The React app used Tailwind v4 with `@custom-variant dark` mapped to the
//! media query, which is `encre_css`'s default dark mode, so the ported class
//! strings keep their meaning.

use std::path::Path;
use std::sync::RwLock;

use crate::error::{CruiseError, Result};

pub(crate) const EXTRA_CSS_FILE: &str = "app.css";

/// Generated stylesheet, cached unless the `WebUI` runs from a developer
/// directory.
#[derive(Debug)]
pub(crate) struct GeneratedCss {
    text: RwLock<Option<String>>,
    dev_mode: bool,
}

impl GeneratedCss {
    pub(crate) fn new(initial: Option<String>, dev_mode: bool) -> Self {
        Self {
            text: RwLock::new(initial),
            dev_mode,
        }
    }

    /// Return the stylesheet, regenerating it in development mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the template or extra CSS sources cannot be read.
    pub(crate) fn text(&self, templates_dir: &Path, static_dir: &Path) -> Result<String> {
        if !self.dev_mode
            && let Ok(guard) = self.text.read()
            && let Some(text) = guard.as_ref()
        {
            return Ok(text.clone());
        }
        let generated = generate(templates_dir, static_dir)?;
        if !self.dev_mode
            && let Ok(mut guard) = self.text.write()
        {
            *guard = Some(generated.clone());
        }
        Ok(generated)
    }
}

/// Scan every template plus the handwritten extras and generate the stylesheet.
///
/// # Errors
///
/// Returns an error when a source file cannot be read.
pub(crate) fn generate(templates_dir: &Path, static_dir: &Path) -> Result<String> {
    let mut sources = Vec::new();
    for entry in walkdir::WalkDir::new(templates_dir)
        .sort_by_file_name()
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "tsx") {
            sources.push(read(entry.path())?);
        }
    }
    let extra_path = static_dir.join(EXTRA_CSS_FILE);
    let extra = if extra_path.is_file() {
        read(&extra_path)?
    } else {
        String::new()
    };
    sources.push(extra.clone());

    let mut config = encre_css::Config::default();
    config.preflight = encre_css::preflight::Preflight::new_full();
    let mut css = encre_css::generate(sources.iter().map(String::as_str), &config);
    css.push('\n');
    css.push_str(&extra);
    Ok(css)
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|error| CruiseError::Other(format!("failed to read {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::generate;

    fn webui_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"))
    }

    #[test]
    fn generated_css_covers_layout_classes_preflight_and_extras() {
        let root = webui_dir();
        let Ok(css) = generate(&root.join("templates"), &root.join("static")) else {
            panic!("generate");
        };
        assert!(css.contains("box-sizing"), "preflight missing");
        assert!(css.contains(".h-screen"), "layout utility missing");
        assert!(
            css.contains("prefers-color-scheme: dark"),
            "dark mode missing"
        );
        assert!(css.contains(".prose"), "handwritten extras missing");
    }
}
