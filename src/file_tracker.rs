use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::error::Result;

/// SHA-256 digest of a single file.
type FileHash = [u8; 32];

/// A snapshot of a directory tree: file path → hash.
type Snapshot = HashMap<PathBuf, FileHash>;

/// Tracks file changes between workflow steps using SHA-256 snapshots.
#[derive(Debug)]
pub struct FileTracker {
    /// Root directory to scan.
    root: PathBuf,

    /// Per-step snapshots keyed by step name.
    snapshots: HashMap<String, Snapshot>,
}

/// Directories excluded from scanning.
const EXCLUDED_DIRS: &[&str] = &[".git", "target", "node_modules"];

impl FileTracker {
    pub fn new() -> Self {
        Self {
            root: PathBuf::from("."),
            snapshots: HashMap::new(),
        }
    }

    pub(crate) fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            snapshots: HashMap::new(),
        }
    }

    /// Capture a snapshot of the root directory and associate it with `step_name`.
    pub fn take_snapshot(&mut self, step_name: &str) -> Result<()> {
        let snapshot = take_current_snapshot(&self.root)?;
        self.snapshots.insert(step_name.to_string(), snapshot);
        Ok(())
    }

    /// Compare the snapshot for `step_name` against the current state.
    /// Returns `false` if no snapshot exists for that step.
    pub fn has_files_changed(&self, step_name: &str) -> Result<bool> {
        let Some(old_snapshot) = self.snapshots.get(step_name) else {
            return Ok(false);
        };

        let current_snapshot = take_current_snapshot(&self.root)?;

        if old_snapshot.len() != current_snapshot.len() {
            return Ok(true);
        }

        for (path, old_hash) in old_snapshot {
            match current_snapshot.get(path) {
                Some(current_hash) => {
                    if old_hash != current_hash {
                        return Ok(true);
                    }
                }
                None => return Ok(true), // file deleted
            }
        }

        Ok(false)
    }
}

impl Default for FileTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Scan `root` recursively and return a path→hash map, skipping excluded dirs.
fn take_current_snapshot(root: impl AsRef<Path>) -> Result<Snapshot> {
    let mut snapshot = HashMap::new();

    for entry in WalkDir::new(root.as_ref())
        .into_iter()
        .filter_entry(|e| !is_excluded(e.path()))
    {
        let entry = entry.map_err(|e| {
            crate::error::CruiseError::IoError(std::io::Error::other(e.to_string()))
        })?;

        if entry.file_type().is_file() {
            let path = entry.path().to_path_buf();
            let hash = hash_file(&path)?;
            snapshot.insert(path, hash);
        }
    }

    Ok(snapshot)
}

/// Compute the SHA-256 hash of a file.
fn hash_file(path: &Path) -> Result<FileHash> {
    let content = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    Ok(hash)
}

/// Return true if `path` contains an excluded directory component.
fn is_excluded(path: &Path) -> bool {
    path.components().any(|component| {
        if let std::path::Component::Normal(name) = component {
            EXCLUDED_DIRS.contains(&name.to_str().unwrap_or(""))
        } else {
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();
        dir
    }

    #[test]
    fn test_snapshot_captures_files() {
        let dir = setup_test_dir();
        let snapshot = take_current_snapshot(dir.path()).unwrap();
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn test_excluded_dirs() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "git config").unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let snapshot = take_current_snapshot(dir.path()).unwrap();
        assert_eq!(snapshot.len(), 1);
    }

    #[test]
    fn test_no_changes_detected() {
        let dir = setup_test_dir();
        let mut tracker = FileTracker::with_root(dir.path().to_path_buf());
        tracker.take_snapshot("step1").unwrap();
        assert!(!tracker.has_files_changed("step1").unwrap());
    }

    #[test]
    fn test_file_modification_detected() {
        let dir = setup_test_dir();
        let mut tracker = FileTracker::with_root(dir.path().to_path_buf());
        tracker.take_snapshot("step1").unwrap();

        std::fs::write(dir.path().join("file1.txt"), "modified content").unwrap();

        assert!(tracker.has_files_changed("step1").unwrap());
    }

    #[test]
    fn test_file_addition_detected() {
        let dir = setup_test_dir();
        let mut tracker = FileTracker::with_root(dir.path().to_path_buf());
        tracker.take_snapshot("step1").unwrap();

        std::fs::write(dir.path().join("new_file.txt"), "new content").unwrap();

        assert!(tracker.has_files_changed("step1").unwrap());
    }

    #[test]
    fn test_file_deletion_detected() {
        let dir = setup_test_dir();
        let mut tracker = FileTracker::with_root(dir.path().to_path_buf());
        tracker.take_snapshot("step1").unwrap();

        std::fs::remove_file(dir.path().join("file1.txt")).unwrap();

        assert!(tracker.has_files_changed("step1").unwrap());
    }

    #[test]
    fn test_no_snapshot_returns_false() {
        let tracker = FileTracker::new();
        assert!(!tracker.has_files_changed("nonexistent").unwrap());
    }
}
