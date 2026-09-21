//! `WebUI` asset resolution: embedded copy extracted to the data directory, or a
//! developer-supplied directory served directly.

use std::path::{Path, PathBuf};

use crate::error::{CruiseError, Result};

static EMBEDDED: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/webui");

#[derive(Debug, Clone)]
pub(crate) struct Assets {
    pub(crate) templates_dir: PathBuf,
    pub(crate) static_dir: PathBuf,
    /// Templates and CSS are re-read on every request.
    pub(crate) dev_mode: bool,
}

/// Resolve the directories the `WebUI` serves templates and static files from.
///
/// # Errors
///
/// Returns an error when `override_dir` is not a `WebUI` directory, or when the
/// embedded copy cannot be extracted.
pub(crate) fn prepare(override_dir: Option<PathBuf>, data_dir: &Path) -> Result<Assets> {
    if let Some(dir) = override_dir {
        if !dir.join("templates/layout.tsx").is_file() || !dir.join("static/htmax.min.js").is_file()
        {
            return Err(CruiseError::Other(format!(
                "{} is not a webui directory (expected templates/layout.tsx and static/htmax.min.js)",
                dir.display()
            )));
        }
        return Ok(Assets {
            templates_dir: dir.join("templates"),
            static_dir: dir.join("static"),
            dev_mode: true,
        });
    }

    let target = data_dir.join("webui").join(env!("CARGO_PKG_VERSION"));
    // The path is version-keyed, so an already extracted tree has the exact
    // contents we would write. Re-extracting would delete files a concurrently
    // running server is still reading templates and static assets from.
    let marker = target.join(".complete");
    if !marker.is_file() {
        let extract = || -> std::io::Result<()> {
            match std::fs::remove_dir_all(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            std::fs::create_dir_all(&target)?;
            EMBEDDED.extract(&target)?;
            std::fs::write(&marker, env!("CARGO_PKG_VERSION"))
        };
        extract().map_err(|error| {
            CruiseError::Other(format!(
                "failed to extract WebUI assets to {}: {error}",
                target.display()
            ))
        })?;
    }
    Ok(Assets {
        templates_dir: target.join("templates"),
        static_dir: target.join("static"),
        dev_mode: false,
    })
}

#[cfg(test)]
mod tests {
    use super::{EMBEDDED, prepare};

    #[test]
    fn embedded_tree_contains_layout_and_vendored_htmx() {
        assert!(EMBEDDED.get_file("templates/layout.tsx").is_some());
        assert!(EMBEDDED.get_file("static/htmax.min.js").is_some());
    }

    #[test]
    fn prepare_extracts_embedded_assets() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let Ok(assets) = prepare(None, dir.path()) else {
            panic!("prepare");
        };
        assert!(!assets.dev_mode);
        assert!(assets.templates_dir.join("layout.tsx").is_file());
        assert!(assets.static_dir.join("htmax.min.js").is_file());
    }

    #[test]
    fn prepare_rejects_a_directory_without_templates() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let error = prepare(Some(dir.path().to_path_buf()), dir.path());
        assert!(error.is_err());
    }
}
