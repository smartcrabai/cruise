//! Read-only diffs for session branches and their worktrees.
use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs,
    io::Read,
    path::Path,
    process::{Command, Output, Stdio},
};

use crate::{
    error::{CruiseError, Result},
    session::SessionState,
};

pub const MAX_PATCH_BYTES: usize = 1024 * 1024;
pub const UNTRACKED_FILE_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSource {
    Worktree,
    CommittedOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchDiff {
    pub branch: String,
    pub base_commit: String,
    pub source: DiffSource,
    pub stat: String,
    pub patch: String,
    pub truncated: bool,
}

fn git(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| CruiseError::Other(format!("git {}: {e}", git_subcommand(args))))
}

fn git_subcommand<'a>(args: &'a [&str]) -> &'a str {
    args.iter()
        .copied()
        .find(|arg| !arg.starts_with('-'))
        .unwrap_or("")
}

fn git_error(args: &[&str], output: &Output) -> CruiseError {
    CruiseError::Other(format!(
        "git {}: {}",
        git_subcommand(args),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn checked_git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = git(dir, args)?;
    if !output.status.success() {
        return Err(git_error(args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn append_patch(
    dir: &Path,
    args: &[&str],
    file: Option<&OsStr>,
    patch: &mut Vec<u8>,
    truncated: &mut bool,
    no_index: bool,
) -> Result<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .args(file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CruiseError::Other(format!("git {}: {e}", git_subcommand(args))))?;
    let (Some(mut stderr), Some(mut stdout)) = (child.stderr.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(CruiseError::Other(
            "git diff: failed to capture output".into(),
        ));
    };
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr.read_to_end(&mut bytes);
        (result, bytes)
    });
    let mut buffer = [0u8; 8192];
    let read_result = (|| -> std::io::Result<()> {
        loop {
            let n = stdout.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            let keep = n.min(
                MAX_PATCH_BYTES
                    .saturating_add(1)
                    .saturating_sub(patch.len()),
            );
            patch.extend_from_slice(&buffer[..keep]);
            if keep < n || patch.len() > MAX_PATCH_BYTES {
                *truncated = true;
            }
        }
        Ok(())
    })();
    drop(stdout);
    if read_result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let (stderr_result, stderr) = stderr_reader
        .join()
        .map_err(|_| CruiseError::Other("git stderr reader failed".into()))?;
    read_result?;
    stderr_result?;
    if !(status.success() || no_index && status.code() == Some(1)) {
        return Err(git_error(
            args,
            &Output {
                status,
                stdout: Vec::new(),
                stderr,
            },
        ));
    }
    Ok(())
}

fn branch(state: &SessionState) -> Result<&str> {
    state
        .worktree_branch
        .as_deref()
        .ok_or_else(|| CruiseError::Other(format!("session {} has no worktree branch", state.id)))
}

/// Resolve the merge-base used for all session diff operations.
///
/// # Errors
/// Returns an error if the session has no branch, Git fails, or the base and branch have no common history.
pub fn session_base_commit(state: &SessionState) -> Result<String> {
    let name = branch(state)?;
    let dir = &state.base_dir;
    // TODO: Prefer local_merge::merge_target_branch once that sibling feature lands.
    let args = ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"];
    let output = git(dir, &args)?;
    if !output.status.success() {
        // rev-parse --quiet suppresses diagnostics even outside a repository.
        let probe_args = ["rev-parse", "--git-dir"];
        let probe = git(dir, &probe_args)?;
        if !probe.status.success() {
            return Err(git_error(&probe_args, &probe));
        }
        return Err(CruiseError::Other(format!(
            "base repository {} has no HEAD commit",
            dir.display()
        )));
    }
    let base = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let reference = format!("refs/heads/{name}");
    let verify_args = ["rev-parse", "--verify", "--quiet", &reference];
    if !git(dir, &verify_args)?.status.success() {
        return Err(CruiseError::Other(format!(
            "branch {name} no longer exists"
        )));
    }
    let args = ["merge-base", &base, &reference];
    let output = git(dir, &args)?;
    if !output.status.success() {
        if output.status.code() == Some(1) {
            return Err(CruiseError::Other(format!(
                "branch {name} shares no history with the base HEAD"
            )));
        }
        return Err(git_error(&args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn includes(dir: &Path) -> Result<Vec<String>> {
    let content = match fs::read_to_string(dir.join(".worktreeinclude")) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_end_matches('/'))
        .filter(|line| {
            !line.is_empty()
                && !Path::new(line).is_absolute()
                && !line.split('/').any(|part| part == "..")
        })
        .map(str::to_owned)
        .collect())
}

fn diff_args(color: bool) -> Vec<&'static str> {
    vec![
        "--no-pager",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        if color {
            "--color=always"
        } else {
            "--no-color"
        },
    ]
}

/// Compare the session worktree, or its surviving branch, to its merge-base.
///
/// # Errors
/// Returns an error if the base cannot be resolved, Git fails, or worktree metadata cannot be read.
pub fn session_branch_diff(state: &SessionState, color: bool) -> Result<BranchDiff> {
    let name = branch(state)?.to_owned();
    let base = session_base_commit(state)?;
    let reference = format!("refs/heads/{name}");
    let context = state.worktree_context();
    let (dir, source) = match &context {
        Some(ctx) => (ctx.path.as_path(), DiffSource::Worktree),
        None => (state.base_dir.as_path(), DiffSource::CommittedOnly),
    };
    let mut args = diff_args(color);
    args.push(&base);
    if source == DiffSource::CommittedOnly {
        args.push(&reference);
    }
    let mut patch = Vec::new();
    let mut truncated = false;
    append_patch(dir, &args, None, &mut patch, &mut truncated, false)?;
    let mut stat_args = diff_args(false);
    stat_args.push("--stat");
    stat_args.push(&base);
    if source == DiffSource::CommittedOnly {
        stat_args.push(&reference);
    }
    let mut stat = checked_git(dir, &stat_args)?;
    if source == DiffSource::Worktree {
        let excluded = includes(&state.base_dir)?;
        let output = git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        if !output.status.success() {
            return Err(git_error(&["ls-files"], &output));
        }
        let paths = output
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty() && !p.ends_with(b"/"));
        let mut count = 0;
        for raw in paths {
            #[cfg(unix)]
            let file = {
                use std::os::unix::ffi::OsStrExt;
                Path::new(OsStr::from_bytes(raw))
            };
            #[cfg(not(unix))]
            let path_string = String::from_utf8_lossy(raw);
            #[cfg(not(unix))]
            let file = Path::new(path_string.as_ref());
            if excluded.iter().any(|e| file.starts_with(Path::new(e))) {
                continue;
            }
            if count == UNTRACKED_FILE_LIMIT {
                truncated = true;
                break;
            }
            count += 1;
            let _ = writeln!(stat, " {} | untracked", file.display());
            let mut args = diff_args(color);
            args.extend(["--no-index", "--", "/dev/null"]);
            append_patch(
                dir,
                &args,
                Some(file.as_os_str()),
                &mut patch,
                &mut truncated,
                true,
            )?;
        }
    }
    let mut patch = String::from_utf8_lossy(&patch).into_owned();
    if patch.len() > MAX_PATCH_BYTES {
        let end = patch.as_bytes()[..MAX_PATCH_BYTES]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        patch.truncate(end);
        truncated = true;
    }
    Ok(BranchDiff {
        branch: name,
        base_commit: base,
        source,
        stat,
        patch,
        truncated,
    })
}
