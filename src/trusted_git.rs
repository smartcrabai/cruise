//! Host-owned Git commands that ignore repository hooks, filters, and user config.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::error::{CruiseError, Result};

#[cfg(windows)]
pub(crate) const DISABLED_HOOKS_CONFIG: &str = "core.hooksPath=NUL";
#[cfg(not(windows))]
pub(crate) const DISABLED_HOOKS_CONFIG: &str = "core.hooksPath=/dev/null";

#[cfg(windows)]
pub(crate) const DISABLED_GLOBAL_CONFIG: &str = "NUL";
#[cfg(not(windows))]
pub(crate) const DISABLED_GLOBAL_CONFIG: &str = "/dev/null";

pub(crate) struct TemporaryIndex {
    directory: PathBuf,
    pub(crate) path: PathBuf,
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn git_error(message: &str) -> CruiseError {
    CruiseError::Other(format!("trusted Git snapshot failed: {message}"))
}

/// Resolve the artifacts directory of the session owning `plan_path`.
pub(crate) fn session_artifacts_root(plan_path: &Path) -> Result<PathBuf> {
    let invalid = || {
        CruiseError::Other(
            "session plan path must identify a regular plan.md under the Cruise sessions directory"
                .to_string(),
        )
    };
    let session_dir = plan_path.parent().ok_or_else(invalid)?;
    let expected_sessions = crate::paths::data_dir()?.join("sessions");
    let valid_plan = plan_path.file_name().is_some_and(|name| name == "plan.md");
    let valid_session = session_dir.parent() == Some(expected_sessions.as_path())
        && session_dir
            .file_name()
            .is_some_and(|name| name != "." && name != "..");
    let plan_is_file =
        std::fs::symlink_metadata(plan_path).is_ok_and(|metadata| metadata.file_type().is_file());
    if !valid_plan || !valid_session || !plan_is_file {
        return Err(invalid());
    }
    Ok(session_dir.join(crate::artifacts::ARTIFACTS_DIR))
}

/// Reject repository filter config, which `git add` and `git stash create` would execute.
pub(crate) fn ensure_no_repository_filters(cwd: &Path) -> Result<()> {
    let args = ["config", "--name-only", "--get-regexp", r"^filter\."];
    let output = git_command(cwd, &args, None)?;
    if !output.stdout.is_empty() {
        return Err(git_error(
            "repository Git filter configuration is not allowed for trusted snapshots",
        ));
    }
    if output.status.success() || output.status.code() == Some(1) {
        return Ok(());
    }
    Err(git_failure(&args, &output))
}

/// Copy the repository index so snapshots never rewrite the real one.
pub(crate) fn temporary_index_copy(cwd: &Path) -> Result<TemporaryIndex> {
    let output = git_output(cwd, &["rev-parse", "--git-path", "index"])?;
    let index_path = parse_git_path(cwd, &output.stdout)?;
    let metadata = fs::symlink_metadata(&index_path)
        .map_err(|error| git_error(&format!("cannot inspect repository index: {error}")))?;
    if !metadata.file_type().is_file() {
        return Err(git_error("repository index is not a regular file"));
    }

    let directory = create_private_tempdir()?;
    let temporary = TemporaryIndex {
        path: directory.join("index"),
        directory,
    };
    fs::copy(&index_path, &temporary.path)
        .map_err(|error| git_error(&format!("cannot copy repository index: {error}")))?;
    Ok(temporary)
}

pub(crate) fn parse_git_path(cwd: &Path, stdout: &[u8]) -> Result<PathBuf> {
    let value = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    let value = value.strip_suffix(b"\r").unwrap_or(value);
    if value.is_empty() || value.contains(&b'\n') {
        return Err(git_error("Git returned a malformed index path"));
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt as _;
        PathBuf::from(std::ffi::OsString::from_vec(value.to_vec()))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(
        std::str::from_utf8(value).map_err(|_| git_error("Git returned a malformed index path"))?,
    );
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn create_private_tempdir() -> Result<PathBuf> {
    for _ in 0..16 {
        let directory = std::env::temp_dir().join(format!(
            "cruise-git-index-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(git_error(&format!(
                    "cannot create private temporary index directory: {error}"
                )));
            }
        }
    }
    Err(git_error(
        "cannot create a unique private temporary index directory",
    ))
}

pub(crate) fn git_output(cwd: &Path, args: &[&str]) -> Result<Output> {
    git_output_with_index(cwd, args, None)
}

pub(crate) fn git_output_with_index(
    cwd: &Path,
    args: &[&str],
    index_file: Option<&Path>,
) -> Result<Output> {
    let output = git_command(cwd, args, index_file)?;
    if !output.status.success() {
        return Err(git_failure(args, &output));
    }
    Ok(output)
}

fn git_command(cwd: &Path, args: &[&str], index_file: Option<&Path>) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-optional-locks",
            "-c",
            DISABLED_HOOKS_CONFIG,
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
        ])
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", DISABLED_GLOBAL_CONFIG)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_CONFIG_PARAMETERS");
    if let Some(index_file) = index_file {
        command
            .env("GIT_INDEX_FILE", index_file)
            .env("GIT_AUTHOR_NAME", "Cruise snapshot")
            .env("GIT_AUTHOR_EMAIL", "snapshot@cruise.invalid")
            .env("GIT_COMMITTER_NAME", "Cruise snapshot")
            .env("GIT_COMMITTER_EMAIL", "snapshot@cruise.invalid");
    }
    command
        .output()
        .map_err(|error| git_error(&format!("failed to run trusted Git command: {error}")))
}

fn git_failure(args: &[&str], output: &Output) -> CruiseError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    git_error(&format!(
        "trusted Git command `{}` failed: {}",
        args.join(" "),
        stderr.trim()
    ))
}

pub(crate) fn parse_oid(command: &str, stdout: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| git_error(&format!("{command} returned a malformed object id")))?;
    let value = text.strip_suffix('\n').unwrap_or(text);
    let value = value.strip_suffix('\r').unwrap_or(value);
    if !valid_oid(value) {
        return Err(git_error(&format!(
            "{command} returned a malformed object id"
        )));
    }
    Ok(value.to_string())
}

pub(crate) fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
