//! Preserve workspace test evidence in the session record.
//!
//! UI E2E runners usually write screenshots and reports to ignored output
//! directories (`test-results/`, `target/…`). Those files never reach the PR
//! and disappear with the worktree or `--repo` clone, so on completion Cruise
//! copies a bounded set of untracked evidence files into the session's
//! `artifacts/evidence/` directory and lists them in `artifacts/evidence.md`.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{CruiseError, Result};

/// Artifact directory holding copied evidence, relative to the artifacts root.
pub const EVIDENCE_DIR: &str = "evidence";
/// Artifact manifest listing saved and skipped evidence files.
pub const MANIFEST: &str = "evidence.md";

/// Largest single file that is copied.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Total bytes copied per session.
pub const MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;
/// Files copied per session.
pub const MAX_FILES: usize = 100;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
const TEXT_EXTENSIONS: &[&str] = &["md", "txt", "log"];
/// Dependency, environment, and build-cache directories that hold third-party
/// or generated files rather than test evidence.
const SKIPPED_DIRS: &[&str] = &[
    ".dart_tool",
    ".fingerprint",
    ".gradle",
    ".venv",
    "Pods",
    "__pycache__",
    "incremental",
    "node_modules",
    "venv",
];
const MAX_LISTED_SKIPS: usize = 50;

/// Result of one preservation pass.
#[derive(Debug, PartialEq, Eq)]
pub struct Preserved {
    pub saved: usize,
    pub skipped: usize,
    /// Absolute path to the manifest.
    pub manifest: PathBuf,
}

struct Candidate {
    relative: String,
    size: u64,
    image: bool,
}

/// Copy untracked evidence files from `workspace` into `artifacts_root`.
///
/// Images are copied before text reports, then in path order, until a limit
/// is reached. Any previous evidence for the session is replaced. Returns
/// `None` when the workspace has no candidate files.
///
/// # Errors
///
/// Returns an error when Git cannot list untracked files or the session
/// record cannot be written.
pub fn preserve(workspace: &Path, artifacts_root: &Path) -> Result<Option<Preserved>> {
    let mut candidates = candidates(workspace)?;
    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.sort_by(|a, b| b.image.cmp(&a.image).then(a.relative.cmp(&b.relative)));
    clear_previous(artifacts_root)?;

    let mut saved = Vec::new();
    let mut skipped = Vec::new();
    let mut total = 0_u64;
    for candidate in candidates {
        let reason = if candidate.size > MAX_FILE_BYTES {
            Some("exceeds the per-file limit")
        } else if saved.len() >= MAX_FILES || total + candidate.size > MAX_TOTAL_BYTES {
            Some("exceeds the session limit")
        } else {
            None
        };
        if let Some(reason) = reason {
            skipped.push((candidate, reason));
            continue;
        }
        let bytes = match fs::read(workspace.join(&candidate.relative)) {
            Ok(bytes) if bytes.len() as u64 <= MAX_FILE_BYTES => bytes,
            Ok(_) => {
                skipped.push((candidate, "exceeds the per-file limit"));
                continue;
            }
            Err(_) => {
                skipped.push((candidate, "unreadable"));
                continue;
            }
        };
        let name = format!("{EVIDENCE_DIR}/{}", candidate.relative);
        if crate::artifacts::write_bytes(artifacts_root, &name, &bytes, bytes.len()).is_err() {
            skipped.push((candidate, "cannot be stored"));
            continue;
        }
        total += bytes.len() as u64;
        saved.push(candidate);
    }

    crate::artifacts::write(
        artifacts_root,
        MANIFEST,
        &manifest(workspace, &saved, &skipped),
    )?;
    Ok(Some(Preserved {
        saved: saved.len(),
        skipped: skipped.len(),
        manifest: artifacts_root.join(MANIFEST),
    }))
}

fn candidates(workspace: &Path) -> Result<Vec<Candidate>> {
    let mut command = Command::new("git");
    command.args(["ls-files", "--others", "-z"]);
    for dir in SKIPPED_DIRS {
        // `-x name/` prunes the directory at any depth instead of listing its files.
        command.arg("-x").arg(format!("{dir}/"));
    }
    let output = command
        .current_dir(workspace)
        .output()
        .map_err(|error| CruiseError::Other(format!("failed to list workspace files: {error}")))?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "failed to list workspace files: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut candidates = Vec::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
        let Ok(relative) = std::str::from_utf8(entry) else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        let Some(extension) = Path::new(relative)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        let image = IMAGE_EXTENSIONS.contains(&extension.as_str());
        if !image && !TEXT_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        if crate::artifacts::validate_name(relative).is_err() {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(workspace.join(relative)) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        candidates.push(Candidate {
            relative: relative.to_owned(),
            size: metadata.len(),
            image,
        });
    }
    Ok(candidates)
}

fn clear_previous(artifacts_root: &Path) -> Result<()> {
    let dir = artifacts_root.join(EVIDENCE_DIR);
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(&dir),
        Ok(_) => fs::remove_file(&dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .map_err(|error| CruiseError::Other(format!("failed to replace saved evidence: {error}")))
}

fn manifest(workspace: &Path, saved: &[Candidate], skipped: &[(Candidate, &str)]) -> String {
    let mut text = format!(
        "# Workspace evidence\n\nCopied from untracked files in `{}` when the session completed.\n\
         Saved files are under `{EVIDENCE_DIR}/` next to this manifest.\n\n## Saved ({})\n\n",
        workspace.display(),
        saved.len()
    );
    if saved.is_empty() {
        text.push_str("None.\n");
    }
    for candidate in saved {
        let _ = writeln!(
            text,
            "- `{}` ({} bytes)",
            candidate.relative, candidate.size
        );
    }
    if !skipped.is_empty() {
        let _ = write!(text, "\n## Skipped ({})\n\n", skipped.len());
        for (candidate, reason) in skipped.iter().take(MAX_LISTED_SKIPS) {
            let _ = writeln!(
                text,
                "- `{}` ({} bytes): {reason}",
                candidate.relative, candidate.size
            );
        }
        if skipped.len() > MAX_LISTED_SKIPS {
            let _ = writeln!(text, "- … and {} more", skipped.len() - MAX_LISTED_SKIPS);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(status.success(), "git {args:?}");
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap_or_else(|| panic!("parent")))
            .unwrap_or_else(|e| panic!("{e}"));
        fs::write(path, bytes).unwrap_or_else(|e| panic!("{e}"));
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        git(dir.path(), &["init", "-q"]);
        write(&dir.path().join(".gitignore"), b"test-results/\ntarget/\n");
        write(&dir.path().join("README.md"), b"tracked");
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "init",
            ],
        );
        dir
    }

    #[test]
    fn copies_ignored_and_untracked_evidence_only() {
        let workspace = repo();
        let root = workspace.path();
        write(&root.join("test-results/flow/done.png"), b"png");
        write(&root.join("target/ui-e2e/report.md"), b"report");
        write(&root.join("notes.log"), b"log");
        write(&root.join("test-results/trace.zip"), b"zip");
        write(&root.join("node_modules/pkg/logo.png"), b"dep");
        let artifacts = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));

        let preserved = preserve(root, artifacts.path())
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("evidence expected"));

        assert_eq!(preserved.saved, 3);
        assert_eq!(preserved.skipped, 0);
        let saved = artifacts.path().join(EVIDENCE_DIR);
        assert_eq!(
            fs::read(saved.join("test-results/flow/done.png")).ok(),
            Some(b"png".to_vec())
        );
        assert_eq!(
            fs::read(saved.join("target/ui-e2e/report.md")).ok(),
            Some(b"report".to_vec())
        );
        assert!(saved.join("notes.log").is_file());
        assert!(
            !saved.join("README.md").exists(),
            "tracked files are in the PR"
        );
        assert!(!saved.join("test-results/trace.zip").exists());
        assert!(!saved.join("node_modules").exists());
        let manifest = fs::read_to_string(&preserved.manifest).unwrap_or_else(|e| panic!("{e}"));
        assert!(manifest.contains("`target/ui-e2e/report.md`"), "{manifest}");
    }

    #[test]
    fn enforces_file_and_session_limits_preferring_images() {
        let workspace = repo();
        let root = workspace.path();
        let too_large = usize::try_from(MAX_FILE_BYTES + 1).unwrap_or(usize::MAX);
        write(&root.join("test-results/huge.png"), &vec![0; too_large]);
        for index in 0..=MAX_FILES {
            write(
                &root.join(format!("test-results/shot-{index:03}.png")),
                b"png",
            );
        }
        write(&root.join("test-results/a-report.md"), b"report");
        let artifacts = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));

        let preserved = preserve(root, artifacts.path())
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("evidence expected"));

        assert_eq!(preserved.saved, MAX_FILES);
        assert_eq!(preserved.skipped, 3);
        let saved = artifacts.path().join(EVIDENCE_DIR).join("test-results");
        assert!(!saved.join("huge.png").exists());
        assert!(!saved.join("a-report.md").exists(), "images take priority");
        let manifest = fs::read_to_string(&preserved.manifest).unwrap_or_else(|e| panic!("{e}"));
        assert!(manifest.contains("`test-results/huge.png`"), "{manifest}");
        assert!(
            manifest.contains("exceeds the per-file limit"),
            "{manifest}"
        );
        assert!(manifest.contains("exceeds the session limit"), "{manifest}");
    }

    #[test]
    fn returns_none_without_candidates() {
        let workspace = repo();
        let artifacts = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            preserve(workspace.path(), artifacts.path()).ok(),
            Some(None)
        );
        assert!(!artifacts.path().join(MANIFEST).exists());
    }
}
