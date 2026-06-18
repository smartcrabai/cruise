//! Image attachment handling for planning input.
//!
//! When the user attaches images to a planning task (via CLI `--image` flag,
//! drag-and-drop into the CLI prompt, or the GUI form), this module:
//! 1. Copies each source file into the session's `attachments/` directory so
//!    the path stays stable after the user moves the original.
//! 2. Returns the absolute paths of the stored copies. Callers append those
//!    paths to the LLM input via [`format_input_with_attachments`] so the
//!    planning agent can `Read` them as image content (Anthropic models view
//!    image files natively via their Read tool).

use std::path::{Path, PathBuf};

use crate::error::{CruiseError, Result};

/// File extensions recognized as supported image attachments.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

/// Name of the per-session subdirectory that stores copied attachments.
pub const ATTACHMENTS_DIRNAME: &str = "attachments";

/// Build the `<session_dir>/attachments/` path. The directory is not created.
#[must_use]
pub fn attachments_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(ATTACHMENTS_DIRNAME)
}

/// True if `path` has one of [`IMAGE_EXTENSIONS`] (case-insensitive).
#[must_use]
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            let lower = ext.to_ascii_lowercase();
            IMAGE_EXTENSIONS.iter().any(|e| *e == lower)
        })
}

/// Copy each source image into `session_dir/attachments/` and return the
/// absolute stored paths in the same order.
///
/// Each copy keeps the original filename, with a numeric suffix (`-2`, `-3`, …)
/// added on collision so two `image.png` attachments do not overwrite each
/// other.
///
/// # Errors
///
/// Returns an error if a source path does not exist, is not a recognized image
/// extension, or any filesystem operation fails.
pub fn copy_images_into_session(session_dir: &Path, sources: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if sources.is_empty() {
        return Ok(vec![]);
    }
    let dest_dir = attachments_dir(session_dir);
    std::fs::create_dir_all(&dest_dir).map_err(|e| {
        CruiseError::Other(format!(
            "failed to create attachments dir {}: {}",
            dest_dir.display(),
            e
        ))
    })?;

    let mut stored = Vec::with_capacity(sources.len());
    for src in sources {
        if !src.exists() {
            return Err(CruiseError::Other(format!(
                "image attachment not found: {}",
                src.display()
            )));
        }
        if !is_image_path(src) {
            return Err(CruiseError::Other(format!(
                "unsupported image type (expected one of {}): {}",
                IMAGE_EXTENSIONS.join(", "),
                src.display()
            )));
        }
        let dest = unique_dest_path(&dest_dir, src)?;
        std::fs::copy(src, &dest).map_err(|e| {
            CruiseError::Other(format!(
                "failed to copy {} to {}: {}",
                src.display(),
                dest.display(),
                e
            ))
        })?;
        stored.push(dest);
    }
    Ok(stored)
}

/// Choose a non-colliding destination path inside `dest_dir` based on `src`'s
/// filename. Adds `-2`, `-3`, … before the extension when needed. Errors out
/// if the namespace `image[-2..999].ext` is exhausted instead of silently
/// overwriting an existing attachment.
fn unique_dest_path(dest_dir: &Path, src: &Path) -> Result<PathBuf> {
    let filename = src.file_name().map_or_else(
        || std::ffi::OsString::from("image"),
        std::ffi::OsStr::to_os_string,
    );
    let candidate = dest_dir.join(&filename);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let stem = Path::new(&filename)
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("image")
        .to_string();
    let ext = Path::new(&filename)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_string);
    for n in 2..=999 {
        let name = match &ext {
            Some(e) => format!("{stem}-{n}.{e}"),
            None => format!("{stem}-{n}"),
        };
        let candidate = dest_dir.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(CruiseError::Other(format!(
        "too many attachments with filename {} in {} (>=999 collisions)",
        filename.to_string_lossy(),
        dest_dir.display()
    )))
}

/// Append an `Attached images:` reference block to `input` so the planning
/// agent (which receives just the prompt text) knows which image files to read.
///
/// Returns `input` unchanged when `image_paths` is empty.
#[must_use]
pub fn format_input_with_attachments(input: &str, image_paths: &[PathBuf]) -> String {
    if image_paths.is_empty() {
        return input.to_string();
    }
    let trimmed = input.trim_end();
    let mut out = String::with_capacity(trimmed.len() + 64 + image_paths.len() * 80);
    out.push_str(trimmed);
    if !trimmed.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("Attached images (use the Read tool to view):\n");
    for p in image_paths {
        out.push_str("- ");
        out.push_str(&p.display().to_string());
        out.push('\n');
    }
    out
}

/// Extract image file paths from free-form input text (e.g. paths the user
/// dragged onto the terminal). Returns the cleaned text (with detected paths
/// removed) and the list of paths in the order they appeared.
///
/// Recognized forms:
/// - Quoted: `"/path/to/img.png"` or `'/path/to/img.png'`
/// - Backslash-escaped spaces: `/path/to/some\ image.png`
/// - Bare path with no spaces
///
/// Only absolute paths are recognized (must start with `/` or `~`). Only paths
/// whose extension matches [`IMAGE_EXTENSIONS`] are extracted; everything else
/// is left in the text untouched.
#[must_use]
pub fn extract_image_paths(input: &str) -> (String, Vec<PathBuf>) {
    let mut paths = Vec::new();
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            if let Some((path, consumed)) = try_extract_quoted(&chars, i, c)
                && is_absolute_or_home(&path)
            {
                let pb = expand_tilde(&path);
                if is_image_path(&pb) {
                    paths.push(pb);
                    i += consumed;
                    skip_separating_whitespace(&chars, &mut i);
                    continue;
                }
            }
        } else if c == '/' || c == '~' {
            // Only treat as a path candidate when at start or after whitespace
            // so URLs like https://example.com/img.png inside prose are left
            // alone (they start with letters, not `/`).
            let at_boundary = i == 0 || chars[i - 1].is_whitespace();
            if at_boundary && let Some((path, consumed)) = try_extract_unquoted(&chars, i) {
                let pb = expand_tilde(&path);
                if is_image_path(&pb) {
                    paths.push(pb);
                    i += consumed;
                    skip_separating_whitespace(&chars, &mut i);
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    (out.trim().to_string(), paths)
}

/// True for paths that look absolute (`/...`) or rooted in the home
/// directory (`~/...`). Matches the contract of [`extract_image_paths`].
fn is_absolute_or_home(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("~/")
}

/// Consume one space (if present) so removing a path mid-sentence doesn't
/// leave double spaces.
fn skip_separating_whitespace(chars: &[char], i: &mut usize) {
    if *i < chars.len() && chars[*i] == ' ' {
        *i += 1;
    }
}

/// Attempt to read a quoted path starting at `start` (which points at the
/// opening quote `quote`). Returns the unquoted text and the total chars
/// consumed including both quotes.
fn try_extract_quoted(chars: &[char], start: usize, quote: char) -> Option<(String, usize)> {
    let mut s = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        if c == quote {
            return Some((s, i - start + 1));
        }
        if c == '\n' {
            return None;
        }
        s.push(c);
        i += 1;
    }
    None
}

/// Attempt to read a bare/escaped path starting at `start`. Path ends at the
/// next unescaped whitespace.
fn try_extract_unquoted(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut s = String::new();
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            s.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c.is_whitespace() {
            break;
        }
        s.push(c);
        i += 1;
    }
    if s.is_empty() {
        return None;
    }
    Some((s, i - start))
}

/// Expand a leading `~/` to the user's home directory. Other `~` forms are
/// left as-is.
fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = home::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(path: &Path) {
        std::fs::write(path, b"x").unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn is_image_path_recognizes_common_extensions() {
        assert!(is_image_path(Path::new("a.png")));
        assert!(is_image_path(Path::new("a.JPG")));
        assert!(is_image_path(Path::new("a.jpeg")));
        assert!(is_image_path(Path::new("a.webp")));
        assert!(is_image_path(Path::new("a.gif")));
        assert!(!is_image_path(Path::new("a.txt")));
        assert!(!is_image_path(Path::new("a")));
    }

    #[test]
    fn copy_images_stores_files_in_attachments_dir() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_dir = tmp.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let src1 = src_dir.join("a.png");
        let src2 = src_dir.join("b.jpg");
        touch(&src1);
        touch(&src2);

        let stored = copy_images_into_session(&session_dir, &[src1.clone(), src2.clone()])
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(stored.len(), 2);
        assert!(stored[0].ends_with("attachments/a.png"));
        assert!(stored[1].ends_with("attachments/b.jpg"));
        assert!(stored[0].exists());
        assert!(stored[1].exists());
    }

    #[test]
    fn copy_images_handles_name_collisions() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_dir = tmp.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let src1 = tmp.path().join("dir1");
        let src2 = tmp.path().join("dir2");
        std::fs::create_dir_all(&src1).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::create_dir_all(&src2).unwrap_or_else(|e| panic!("{e:?}"));
        let f1 = src1.join("image.png");
        let f2 = src2.join("image.png");
        touch(&f1);
        touch(&f2);

        let stored =
            copy_images_into_session(&session_dir, &[f1, f2]).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(stored[0].ends_with("attachments/image.png"));
        assert!(stored[1].ends_with("attachments/image-2.png"));
    }

    #[test]
    fn copy_images_rejects_non_image_extension() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_dir = tmp.path().join("s");
        std::fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let bad = tmp.path().join("a.txt");
        touch(&bad);
        let err = copy_images_into_session(&session_dir, &[bad]);
        assert!(err.is_err());
    }

    #[test]
    fn copy_images_rejects_missing_source() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_dir = tmp.path().join("s");
        std::fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let missing = tmp.path().join("nope.png");
        let err = copy_images_into_session(&session_dir, &[missing]);
        assert!(err.is_err());
    }

    #[test]
    fn format_input_appends_paths() {
        let out = format_input_with_attachments(
            "describe this UI",
            &[PathBuf::from("/img/a.png"), PathBuf::from("/img/b.jpg")],
        );
        assert!(out.contains("describe this UI"));
        assert!(out.contains("Attached images"));
        assert!(out.contains("/img/a.png"));
        assert!(out.contains("/img/b.jpg"));
    }

    #[test]
    fn format_input_returns_unchanged_when_no_images() {
        assert_eq!(format_input_with_attachments("hi", &[]), "hi");
    }

    #[test]
    fn extract_image_paths_finds_bare_absolute_path() {
        let (text, paths) = extract_image_paths("look at /tmp/a.png and tell me");
        assert_eq!(paths, vec![PathBuf::from("/tmp/a.png")]);
        assert_eq!(text, "look at and tell me");
    }

    #[test]
    fn extract_image_paths_finds_quoted_path() {
        let (text, paths) = extract_image_paths("see \"/tmp/foo bar.png\" please");
        assert_eq!(paths, vec![PathBuf::from("/tmp/foo bar.png")]);
        assert_eq!(text, "see please");
    }

    #[test]
    fn extract_image_paths_finds_escaped_space_path() {
        let (text, paths) = extract_image_paths(r"check /tmp/has\ space.jpg now");
        assert_eq!(paths, vec![PathBuf::from("/tmp/has space.jpg")]);
        assert_eq!(text, "check now");
    }

    #[test]
    fn extract_image_paths_ignores_urls_and_non_image_paths() {
        let (text, paths) = extract_image_paths("see https://example.com/a.png and /tmp/notes.txt");
        assert!(paths.is_empty());
        assert_eq!(text, "see https://example.com/a.png and /tmp/notes.txt");
    }

    #[test]
    fn extract_image_paths_ignores_quoted_relative_path() {
        let (text, paths) = extract_image_paths("see \"image.png\" please");
        assert!(paths.is_empty());
        assert_eq!(text, "see \"image.png\" please");
    }

    #[test]
    fn extract_image_paths_finds_multiple() {
        let (text, paths) = extract_image_paths("/a/x.png and /b/y.jpeg");
        assert_eq!(
            paths,
            vec![PathBuf::from("/a/x.png"), PathBuf::from("/b/y.jpeg")]
        );
        assert_eq!(text, "and");
    }
}
