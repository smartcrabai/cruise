//! Session-scoped prompt output artifacts.
//!
//! Artifact names are always relative to a caller-provided session directory.
//! This module deliberately owns both path validation and I/O so prompt reads
//! and writes cannot drift into different path-safety rules.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write as _};
use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::error::{CruiseError, Result};

/// Directory name below a session directory where prompt artifacts live.
pub const ARTIFACTS_DIR: &str = "artifacts";

/// Prefix used by prompt templates to read an artifact's contents.
pub const FILE_REFERENCE_PREFIX: &str = "file:";

/// Maximum size of an artifact, in bytes.
pub const MAX_ARTIFACT_SIZE: usize = 1024 * 1024;

/// Validate an artifact name and return its normalized relative path.
///
/// Artifact names may contain nested ordinary path components, but may not be
/// empty, absolute, or contain `.`/`..` traversal components. The normalized
/// path is suitable for comparisons such as parallel-output collision checks.
///
/// # Errors
///
/// Returns [`CruiseError::InvalidStepConfig`] when the name is empty, absolute,
/// or contains a traversal component.
pub fn validate_name(name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        return Err(CruiseError::InvalidStepConfig(
            "artifact name must not be empty".to_string(),
        ));
    }

    let path = Path::new(name);
    if path.is_absolute() {
        return Err(CruiseError::InvalidStepConfig(format!(
            "artifact name '{name}' must be relative, not absolute"
        )));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "artifact name '{name}' must not contain '.' path components"
                )));
            }
            Component::ParentDir => {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "artifact name '{name}' must not contain parent '..' path components"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "artifact name '{name}' must be relative"
                )));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(CruiseError::InvalidStepConfig(format!(
            "artifact name '{name}' is empty"
        )));
    }

    Ok(normalized)
}

/// Read an artifact as an unmodified UTF-8 string.
///
/// # Errors
///
/// Returns an error when the root or artifact is missing, unsafe, not a regular
/// file, larger than [`MAX_ARTIFACT_SIZE`], unreadable, or not valid UTF-8.
pub fn read(root: &Path, name: &str) -> Result<String> {
    let relative = validate_name_for_io(name)?;
    let path = resolve_existing_path(root, &relative, name)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| read_error(name, &error))?;
    if !metadata.file_type().is_file() {
        return Err(read_error(
            name,
            &std::io::Error::other("artifact is not a regular file"),
        ));
    }
    if metadata.len() > MAX_ARTIFACT_SIZE as u64 {
        return Err(size_error(name));
    }

    let mut file = File::open(&path).map_err(|error| read_error(name, &error))?;
    let capacity = usize::try_from(metadata.len())
        .unwrap_or(MAX_ARTIFACT_SIZE)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(MAX_ARTIFACT_SIZE.min(capacity));
    std::io::Read::by_ref(&mut file)
        .take(MAX_ARTIFACT_SIZE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| read_error(name, &error))?;
    if bytes.len() > MAX_ARTIFACT_SIZE {
        return Err(size_error(name));
    }

    String::from_utf8(bytes).map_err(|error| {
        read_error(
            name,
            &std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid UTF-8: {error}"),
            ),
        )
    })
}

/// Atomically save an artifact, replacing an existing regular file only after
/// the complete new contents have been written.
///
/// # Errors
///
/// Returns an error when the name is unsafe, the root or parent cannot be
/// prepared, the content exceeds [`MAX_ARTIFACT_SIZE`], or the atomic write
/// cannot complete. A failed replacement leaves an existing artifact intact.
pub fn write(root: &Path, name: &str, content: &str) -> Result<()> {
    if content.len() > MAX_ARTIFACT_SIZE {
        return Err(size_error(name));
    }

    let relative = validate_name_for_io(name)?;
    let path = prepare_write_path(root, &relative, name)?;
    let parent = path.parent().ok_or_else(|| {
        CruiseError::Other(format!(
            "failed to write artifact '{name}': missing parent directory"
        ))
    })?;
    let temporary = parent.join(format!(".artifact-{}.tmp", Uuid::new_v4().simple()));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();

    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(CruiseError::Other(format!(
            "failed to write artifact '{name}': {error}"
        )));
    }

    Ok(())
}

fn validate_name_for_io(name: &str) -> Result<PathBuf> {
    validate_name(name).map_err(|error| match error {
        CruiseError::InvalidStepConfig(message) => {
            CruiseError::Other(format!("failed to access artifact '{name}': {message}"))
        }
        other => other,
    })
}

fn resolve_existing_path(root: &Path, relative: &Path, name: &str) -> Result<PathBuf> {
    ensure_existing_directory(root, name)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|error| read_error(name, &error))?;
        if metadata.file_type().is_symlink() {
            return Err(read_error_message(
                name,
                "artifact path contains a symbolic link",
            ));
        }
        if !metadata.file_type().is_file() && !metadata.file_type().is_dir() {
            return Err(read_error_message(
                name,
                "artifact path contains a non-directory entry",
            ));
        }
    }
    Ok(current)
}

fn prepare_write_path(root: &Path, relative: &Path, name: &str) -> Result<PathBuf> {
    ensure_directory_tree(root, name)?;
    let mut current = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        current.push(component.as_os_str());
        let is_final = components.peek().is_none();
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(write_error_message(
                        name,
                        "artifact path contains a symbolic link",
                    ));
                }
                if is_final {
                    if metadata.file_type().is_dir() {
                        return Err(write_error_message(name, "artifact path is a directory"));
                    }
                    if !metadata.file_type().is_file() {
                        return Err(write_error_message(
                            name,
                            "artifact path is not a regular file",
                        ));
                    }
                } else if !metadata.file_type().is_dir() {
                    return Err(write_error_message(
                        name,
                        "artifact parent is not a directory",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !is_final => {
                create_write_directory(&current, name)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(write_error(name, &error)),
        }
    }
    Ok(current)
}

fn ensure_existing_directory(path: &Path, name: &str) -> Result<()> {
    ensure_directory(path, name, read_error)
}

fn ensure_write_directory(path: &Path, name: &str) -> Result<()> {
    ensure_directory(path, name, write_error)
}

/// Reject symlinked ancestors that form the session artifact root.
///
/// The root itself is checked separately. The artifact root is normally
/// `<data>/sessions/<session>/artifacts`, so check those three managed
/// ancestors while avoiding assumptions about unrelated system paths above the
/// data root.
fn ensure_directory_ancestors(
    path: &Path,
    name: &str,
    error: fn(&str, &std::io::Error) -> CruiseError,
) -> Result<()> {
    for current_path in path.ancestors().skip(1).take(3) {
        match fs::symlink_metadata(current_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(error(
                        name,
                        &std::io::Error::other(
                            "artifact root or path component is a symbolic link",
                        ),
                    ));
                }
                if !metadata.file_type().is_dir() {
                    return Err(error(
                        name,
                        &std::io::Error::other(
                            "artifact root or path component is not a directory",
                        ),
                    ));
                }
            }
            Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {}
            Err(io_error) => return Err(error(name, &io_error)),
        }
    }
    Ok(())
}

fn create_write_directory(path: &Path, name: &str) -> Result<()> {
    if let Err(error) = fs::create_dir(path)
        && error.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(write_error(name, &error));
    }
    ensure_write_directory(path, name)
}

fn ensure_directory(
    path: &Path,
    name: &str,
    error: fn(&str, &std::io::Error) -> CruiseError,
) -> Result<()> {
    ensure_directory_ancestors(path, name, error)?;
    let metadata = fs::symlink_metadata(path).map_err(|io_error| error(name, &io_error))?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            name,
            &std::io::Error::other("artifact root or path component is a symbolic link"),
        ));
    }
    if !metadata.file_type().is_dir() {
        return Err(error(
            name,
            &std::io::Error::other("artifact root is not a directory"),
        ));
    }
    Ok(())
}

fn ensure_directory_tree(root: &Path, name: &str) -> Result<()> {
    let mut missing = Vec::new();
    let mut existing = root.to_path_buf();
    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                ensure_write_directory(&existing, name)?;
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = existing
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                missing.push(existing);
                existing = parent;
            }
            Err(error) => return Err(write_error(name, &error)),
        }
    }

    for path in missing.into_iter().rev() {
        create_write_directory(&path, name)?;
    }
    Ok(())
}

fn size_error(name: &str) -> CruiseError {
    CruiseError::Other(format!(
        "artifact '{name}' exceeds maximum size of {MAX_ARTIFACT_SIZE} bytes (1 MiB)"
    ))
}

fn read_error(name: &str, error: &std::io::Error) -> CruiseError {
    CruiseError::Other(format!("failed to read artifact '{name}': {error}"))
}

fn read_error_message(name: &str, message: &'static str) -> CruiseError {
    read_error(name, &std::io::Error::other(message))
}

fn write_error(name: &str, error: &std::io::Error) -> CruiseError {
    CruiseError::Other(format!("failed to write artifact '{name}': {error}"))
}

fn write_error_message(name: &str, message: &'static str) -> CruiseError {
    write_error(name, &std::io::Error::other(message))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validates_relative_names() {
        assert!(validate_name("nested/file.md").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("/tmp/file.md").is_err());
        assert!(validate_name("../file.md").is_err());
    }

    #[test]
    fn writes_and_reads_exact_content_atomically() {
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        write(root.path(), "nested/file.md", "日本語\n{input}")
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        assert_eq!(
            read(root.path(), "nested/file.md")
                .unwrap_or_else(|error| panic!("read failed: {error}")),
            "日本語\n{input}"
        );
    }

    #[test]
    fn concurrent_writes_can_create_shared_nested_directories() {
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let root_path = root.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(32));
        let handles = (0..32)
            .map(|index| {
                let root_path = root_path.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    write(
                        &root_path,
                        &format!("nested/file-{index}.md"),
                        &index.to_string(),
                    )
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle
                .join()
                .unwrap_or_else(|_| panic!("concurrent artifact writer panicked"))
                .unwrap_or_else(|error| panic!("concurrent artifact write failed: {error}"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_artifact_roots_under_symlinked_session_ancestors() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let real_session = root.path().join("real-session");
        let real_artifacts = real_session.join(ARTIFACTS_DIR);
        fs::create_dir_all(&real_artifacts)
            .unwrap_or_else(|error| panic!("artifact directory setup failed: {error}"));
        fs::write(real_artifacts.join("existing.md"), "existing")
            .unwrap_or_else(|error| panic!("artifact setup failed: {error}"));
        let linked_session = root.path().join("linked-session");
        symlink(&real_session, &linked_session)
            .unwrap_or_else(|error| panic!("session symlink setup failed: {error}"));
        let linked_artifacts = linked_session.join(ARTIFACTS_DIR);

        assert!(
            write(&linked_artifacts, "new.md", "new").is_err(),
            "writes must not follow a symlinked session ancestor"
        );
        assert!(
            read(&linked_artifacts, "existing.md").is_err(),
            "reads must not follow a symlinked session ancestor"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_artifact_roots_under_symlinked_data_ancestors() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let real_data = root.path().join("real-data");
        let real_artifacts = real_data.join("sessions/session/artifacts");
        fs::create_dir_all(&real_artifacts)
            .unwrap_or_else(|error| panic!("artifact directory setup failed: {error}"));
        fs::write(real_artifacts.join("existing.md"), "existing")
            .unwrap_or_else(|error| panic!("artifact setup failed: {error}"));
        let linked_data = root.path().join("linked-data");
        symlink(&real_data, &linked_data)
            .unwrap_or_else(|error| panic!("data-root symlink setup failed: {error}"));
        let linked_artifacts = linked_data.join("sessions/session/artifacts");

        assert!(
            write(&linked_artifacts, "new.md", "new").is_err(),
            "writes must not follow a symlinked data ancestor"
        );
        assert!(
            read(&linked_artifacts, "existing.md").is_err(),
            "reads must not follow a symlinked data ancestor"
        );
    }

    #[cfg(unix)]
    #[test]
    fn allows_symlinked_ancestors_above_the_data_root() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let real_parent = root.path().join("real-parent");
        let real_artifacts = real_parent.join("data/sessions/session/artifacts");
        fs::create_dir_all(&real_artifacts)
            .unwrap_or_else(|error| panic!("artifact directory setup failed: {error}"));
        let linked_parent = root.path().join("linked-parent");
        symlink(&real_parent, &linked_parent)
            .unwrap_or_else(|error| panic!("parent symlink setup failed: {error}"));
        let linked_artifacts = linked_parent.join("data/sessions/session/artifacts");

        write(&linked_artifacts, "nested/output.md", "output")
            .unwrap_or_else(|error| panic!("write through parent symlink failed: {error}"));
        assert_eq!(
            read(&linked_artifacts, "nested/output.md")
                .unwrap_or_else(|error| panic!("read through parent symlink failed: {error}")),
            "output"
        );
    }
}
