use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::error::{CruiseError, Result};

const GUARD_DIR: &str = "commit-guard";
const HOOK_DIR: &str = "hooks";
const HOOK_NAME: &str = "reference-transaction";
const HOOK_CONTENT: &str = r#"#!/bin/sh
# Cruise commit guard: reject transactions that move HEAD or a local branch.
set -f

reject() {
    echo "cruise commit guard: $1" >&2
    exit 1
}

is_oid() {
    case "$1" in
        ''|*[!0123456789abcdefABCDEF]*) return 1 ;;
    esac
    [ "${#1}" -eq 40 ] || [ "${#1}" -eq 64 ]
}

is_ref_value() {
    is_oid "$1" && return 0
    case "$1" in
        ref:refs/?*) return 0 ;;
        *) return 1 ;;
    esac
}

state="${1-}"
case "$state" in
    committed|aborted)
        while IFS= read -r _line; do
            :
        done
        exit 0
        ;;
    preparing|prepared)
        seen=0
        while IFS= read -r line; do
            [ -n "$line" ] || reject "malformed reference-transaction input"
            case "$line" in
                " "*|*" "|*"  "*|*"	"*)
                    reject "malformed reference-transaction input"
                    ;;
            esac
            set -- $line
            [ "$#" -eq 3 ] || reject "malformed reference-transaction input"
            old=$1
            new=$2
            ref=$3
            is_ref_value "$old" || reject "malformed old value in reference-transaction input"
            is_ref_value "$new" || reject "malformed new value in reference-transaction input"
            case "$ref" in
                HEAD)
                    reject "updates to HEAD are not allowed"
                    ;;
                refs/heads/*)
                    [ "$ref" != "refs/heads/" ] || reject "malformed branch reference"
                    reject "updates to $ref are not allowed"
                    ;;
                refs/*|ORIG_HEAD|FETCH_HEAD|MERGE_HEAD|CHERRY_PICK_HEAD|REVERT_HEAD|BISECT_HEAD|AUTO_MERGE)
                    ;;
                *)
                    reject "malformed reference-transaction ref"
                    ;;
            esac
            seen=1
        done
        [ "$seen" -eq 1 ] || reject "malformed reference-transaction input"
        exit 0
        ;;
    *)
        reject "unknown reference-transaction phase"
        ;;
esac
"#;

/// Environment and repository state captured for one guarded prompt run.
pub(crate) struct CommitGuard {
    cwd: PathBuf,
    env: HashMap<String, String>,
    before: HeadState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeadState {
    target: Option<String>,
    oid: String,
}

impl CommitGuard {
    /// Prepare a guard for a prompt run. A missing directory or a directory
    /// outside a Git worktree is deliberately a no-op.
    pub(crate) fn prepare(
        working_dir: Option<&Path>,
        env: &HashMap<String, String>,
    ) -> Result<Option<Self>> {
        let Some(cwd) = working_dir else {
            return Ok(None);
        };
        if !cwd.is_dir() {
            return Ok(None);
        }

        if !is_git_worktree(cwd, env)? {
            return Ok(None);
        }

        let hook_dir = install_hook()?;
        let guarded_env = build_guard_env(env, &hook_dir)?;
        let before = head_state(cwd, &guarded_env)?;

        Ok(Some(Self {
            cwd: cwd.to_path_buf(),
            env: guarded_env,
            before,
        }))
    }

    pub(crate) fn env(&self) -> &HashMap<String, String> {
        &self.env
    }

    /// Compare HEAD after the child is fully awaited. If a branch moved, use a
    /// compare-and-swap update-ref rollback, which changes neither index nor
    /// worktree, then still report the movement as a failure.
    pub(crate) fn finish<T>(self, result: Result<T>) -> Result<T> {
        let after = head_state(&self.cwd, &self.env)?;
        if after == self.before {
            return result;
        }

        let movement = describe_movement(&self.before, &after);
        if let Some(target) = self.before.target.as_deref()
            && self.before.target == after.target
        {
            match rollback(&self.cwd, &self.env, target, &self.before.oid, &after.oid) {
                Ok(()) => {
                    let restored = head_state(&self.cwd, &self.env)?;
                    if restored != self.before {
                        return Err(guard_error(format!(
                            "{movement}; compare-and-swap rollback did not restore HEAD"
                        )));
                    }
                    return Err(guard_error(format!(
                        "{movement}; restored the original branch ref"
                    )));
                }
                Err(error) => {
                    return Err(guard_error(format!(
                        "{movement}; compare-and-swap rollback failed: {error}"
                    )));
                }
            }
        }

        Err(guard_error(format!(
            "{movement}; refusing to roll back a changed or detached HEAD target"
        )))
    }
}

fn describe_movement(before: &HeadState, after: &HeadState) -> String {
    format!(
        "HEAD moved from {}@{} to {}@{}",
        before.target.as_deref().unwrap_or("(detached)"),
        before.oid,
        after.target.as_deref().unwrap_or("(detached)"),
        after.oid
    )
}

fn rollback(
    cwd: &Path,
    env: &HashMap<String, String>,
    target: &str,
    before: &str,
    after: &str,
) -> Result<()> {
    ensure_direct_rollback_target(cwd, env, target)?;
    // The guard hook intentionally rejects all branch updates. The explicit
    // command-line override is limited to this parent-owned CAS rollback.
    let output = run_git(
        cwd,
        env,
        [
            OsStr::new("-c"),
            OsStr::new("core.hooksPath=/dev/null"),
            OsStr::new("update-ref"),
            OsStr::new("--no-deref"),
            OsStr::new(target),
            OsStr::new(before),
            OsStr::new(after),
        ],
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(guard_error(format_git_failure(
            "update-ref rollback",
            &output,
        )))
    }
}

fn ensure_direct_rollback_target(
    cwd: &Path,
    env: &HashMap<String, String>,
    target: &str,
) -> Result<()> {
    let head = run_git(
        cwd,
        env,
        [
            OsStr::new("symbolic-ref"),
            OsStr::new("--quiet"),
            OsStr::new("--no-recurse"),
            OsStr::new("HEAD"),
        ],
    )?;
    if !head.status.success() || utf8_stdout("rollback HEAD target probe", &head)?.trim() != target
    {
        return Err(guard_error(
            "HEAD target changed before compare-and-swap rollback",
        ));
    }

    let target_kind = run_git(
        cwd,
        env,
        [
            OsStr::new("symbolic-ref"),
            OsStr::new("--quiet"),
            OsStr::new("--no-recurse"),
            OsStr::new(target),
        ],
    )?;
    match target_kind.status.code() {
        Some(1) => Ok(()),
        Some(0) => Err(guard_error(
            "branch target became symbolic before compare-and-swap rollback",
        )),
        _ => Err(guard_error(format_git_failure(
            "rollback branch target probe",
            &target_kind,
        ))),
    }
}

fn is_git_worktree(cwd: &Path, env: &HashMap<String, String>) -> Result<bool> {
    let output = run_git(
        cwd,
        env,
        [OsStr::new("rev-parse"), OsStr::new("--is-inside-work-tree")],
    )?;
    if output.status.success() {
        return match String::from_utf8_lossy(&output.stdout).trim() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(guard_error("worktree probe returned an unexpected result")),
        };
    }
    if is_not_a_repository(&output) {
        return Ok(false);
    }
    Err(guard_error(format_git_failure("worktree probe", &output)))
}

fn head_state(cwd: &Path, env: &HashMap<String, String>) -> Result<HeadState> {
    let target_output = run_git(
        cwd,
        env,
        [
            OsStr::new("symbolic-ref"),
            OsStr::new("--quiet"),
            OsStr::new("--no-recurse"),
            OsStr::new("HEAD"),
        ],
    )?;
    let target = if target_output.status.success() {
        let target = utf8_stdout("symbolic HEAD probe", &target_output)?;
        let target = target.trim();
        if target.is_empty() || target.chars().any(char::is_whitespace) {
            return Err(guard_error(
                "symbolic HEAD probe returned a malformed target",
            ));
        }
        Some(target.to_string())
    } else if target_output.status.code() == Some(1) {
        None
    } else {
        return Err(guard_error(format_git_failure(
            "symbolic HEAD probe",
            &target_output,
        )));
    };

    let oid_output = run_git(
        cwd,
        env,
        [
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("HEAD"),
        ],
    )?;
    if !oid_output.status.success() {
        return Err(guard_error(format_git_failure(
            "HEAD OID probe",
            &oid_output,
        )));
    }
    let oid = utf8_stdout("HEAD OID probe", &oid_output)?;
    let oid = oid.trim();
    if !is_oid(oid) {
        return Err(guard_error("HEAD OID probe returned a malformed object id"));
    }

    Ok(HeadState {
        target,
        oid: oid.to_string(),
    })
}

fn build_guard_env(
    env: &HashMap<String, String>,
    hooks_path: &Path,
) -> Result<HashMap<String, String>> {
    let count = match effective_env_value(env, "GIT_CONFIG_COUNT")? {
        None => 0,
        Some(value) if value.is_empty() => 0,
        Some(value) if !value.bytes().all(|byte| byte.is_ascii_digit()) => {
            return Err(guard_error(format!(
                "malformed GIT_CONFIG_COUNT value '{value}'"
            )));
        }
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| guard_error(format!("malformed GIT_CONFIG_COUNT value '{value}'")))?,
    };

    let mut guarded = env.clone();
    for index in 0..count {
        let key_name = format!("GIT_CONFIG_KEY_{index}");
        let value_name = format!("GIT_CONFIG_VALUE_{index}");
        let key = effective_env_value(env, &key_name)?.ok_or_else(|| {
            guard_error(format!(
                "missing active {key_name} for GIT_CONFIG_COUNT={count}"
            ))
        })?;
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err(guard_error(format!("malformed active {key_name}")));
        }
        let value = effective_env_value(env, &value_name)?.ok_or_else(|| {
            guard_error(format!(
                "missing active {value_name} for GIT_CONFIG_COUNT={count}"
            ))
        })?;
        // Make inherited active pairs explicit in the child environment. This
        // preserves the effective config while allowing the extra pair below.
        guarded.insert(key_name, key);
        guarded.insert(value_name, value);
    }

    let next = count
        .checked_add(1)
        .ok_or_else(|| guard_error("GIT_CONFIG_COUNT is too large"))?;
    guarded.insert("GIT_CONFIG_COUNT".to_string(), next.to_string());
    guarded.insert(
        format!("GIT_CONFIG_KEY_{count}"),
        "core.hooksPath".to_string(),
    );
    let hooks_path = hooks_path
        .to_str()
        .ok_or_else(|| guard_error("persistent hook path is not valid UTF-8"))?;
    guarded.insert(format!("GIT_CONFIG_VALUE_{count}"), hooks_path.to_string());
    Ok(guarded)
}

fn effective_env_value(env: &HashMap<String, String>, key: &str) -> Result<Option<String>> {
    if let Some(value) = env.get(key) {
        return Ok(Some(value.clone()));
    }
    match std::env::var_os(key) {
        None => Ok(None),
        Some(value) => value
            .into_string()
            .map(Some)
            .map_err(|_| guard_error(format!("active environment value {key} is not valid UTF-8"))),
    }
}

fn install_hook() -> Result<PathBuf> {
    let data_dir = crate::paths::data_dir().map_err(|error| {
        guard_error(format!(
            "cannot determine persistent hook directory: {error}"
        ))
    })?;
    let data_dir = if data_dir.is_absolute() {
        data_dir
    } else {
        std::env::current_dir()
            .map_err(|error| guard_error(format!("cannot make hook directory absolute: {error}")))?
            .join(data_dir)
    };
    let root = data_dir.join(GUARD_DIR);
    let hooks = root.join(HOOK_DIR);
    fs::create_dir_all(&hooks).map_err(|error| {
        guard_error(format!(
            "cannot create persistent hook directory {}: {error}",
            hooks.display()
        ))
    })?;
    set_mode(&root, 0o700)?;
    set_mode(&hooks, 0o700)?;
    let hooks = fs::canonicalize(&hooks).map_err(|error| {
        guard_error(format!(
            "cannot resolve persistent hook directory {}: {error}",
            hooks.display()
        ))
    })?;
    let hook_path = hooks.join(HOOK_NAME);
    if fs::read(&hook_path).is_ok_and(|content| content == HOOK_CONTENT.as_bytes()) {
        set_mode(&hook_path, 0o755)?;
    } else {
        atomic_write_hook(&hook_path)?;
    }
    Ok(hooks)
}

fn atomic_write_hook(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| guard_error("persistent hook path has no parent"))?;
    let temp = parent.join(format!(
        ".{HOOK_NAME}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| guard_error(format!("cannot create temporary hook file: {error}")))?;
        file.write_all(HOOK_CONTENT.as_bytes())
            .map_err(|error| guard_error(format!("cannot write temporary hook file: {error}")))?;
        file.sync_all()
            .map_err(|error| guard_error(format!("cannot sync temporary hook file: {error}")))?;
        set_mode(&temp, 0o755)?;
        fs::rename(&temp, path)
            .map_err(|error| guard_error(format!("cannot install persistent hook: {error}")))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path)
            .map_err(|error| guard_error(format!("cannot stat {}: {error}", path.display())))?
            .permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions).map_err(|error| {
            guard_error(format!(
                "cannot set permissions on {}: {error}",
                path.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

fn run_git<I, S>(cwd: &Path, env: &HashMap<String, String>, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .envs(env)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| guard_error(format!("failed to spawn git for commit guard: {error}")))
}

fn utf8_stdout(context: &str, output: &Output) -> Result<String> {
    String::from_utf8(output.stdout.clone())
        .map_err(|error| guard_error(format!("{context} returned invalid UTF-8: {error}")))
}

fn format_git_failure(context: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{context} failed with status {}", output.status)
    } else {
        format!("{context} failed: {stderr}")
    }
}

fn is_not_a_repository(output: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    stderr.contains("not a git repository")
        || stderr.contains("must be run in a work tree")
        || stderr.contains("not a work tree")
}

fn is_oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn guard_error(message: impl Into<String>) -> CruiseError {
    CruiseError::CommitGuardViolation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_env_appends_hook_pair_and_preserves_existing_pairs() {
        let mut env = HashMap::new();
        env.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
        env.insert("GIT_CONFIG_KEY_0".to_string(), "user.name".to_string());
        env.insert("GIT_CONFIG_VALUE_0".to_string(), "Cruise".to_string());
        let hooks = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));

        let guarded = build_guard_env(&env, hooks.path())
            .unwrap_or_else(|error| panic!("unexpected guard env error: {error}"));
        let expected_path = hooks.path().to_string_lossy().into_owned();
        assert_eq!(
            guarded.get("GIT_CONFIG_COUNT").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            guarded.get("GIT_CONFIG_KEY_0").map(String::as_str),
            Some("user.name")
        );
        assert_eq!(
            guarded.get("GIT_CONFIG_VALUE_0").map(String::as_str),
            Some("Cruise")
        );
        assert_eq!(
            guarded.get("GIT_CONFIG_KEY_1").map(String::as_str),
            Some("core.hooksPath")
        );
        assert_eq!(
            guarded.get("GIT_CONFIG_VALUE_1").map(String::as_str),
            Some(expected_path.as_str())
        );
    }

    #[test]
    fn empty_git_config_count_is_treated_as_zero() {
        let env = HashMap::from([("GIT_CONFIG_COUNT".to_string(), String::new())]);
        let guarded = build_guard_env(&env, Path::new("/tmp/hooks"))
            .unwrap_or_else(|error| panic!("unexpected guard env error: {error}"));
        assert_eq!(
            guarded.get("GIT_CONFIG_COUNT").map(String::as_str),
            Some("1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_hook_path_fails_closed() {
        use std::os::unix::ffi::OsStringExt as _;

        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'/', b't', b'm', b'p', b'/', 0xff,
        ]));
        let result = build_guard_env(&HashMap::new(), &path);
        assert!(matches!(result, Err(CruiseError::CommitGuardViolation(_))));
    }

    #[test]
    fn non_git_directory_is_a_no_op_regardless_of_requested_locale() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let env = HashMap::from([("LC_ALL".to_string(), "ja_JP.UTF-8".to_string())]);
        let guard = CommitGuard::prepare(Some(dir.path()), &env)
            .unwrap_or_else(|error| panic!("unexpected guard error: {error}"));
        assert!(guard.is_none());
    }

    #[test]
    fn malformed_active_config_pair_fails_closed() {
        let mut env = HashMap::new();
        env.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
        env.insert("GIT_CONFIG_KEY_0".to_string(), "user.name".to_string());
        let result = build_guard_env(&env, Path::new("/tmp/hooks"));
        assert!(matches!(result, Err(CruiseError::CommitGuardViolation(_))));
    }

    #[cfg(unix)]
    #[test]
    fn hook_rejects_branch_updates_and_accepts_terminal_phases() {
        use std::io::Write as _;

        let run = |phase: &str, input: &str| {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(HOOK_CONTENT)
                .arg("cruise-reference-transaction")
                .arg(phase)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| panic!("failed to spawn hook: {error}"));
            child
                .stdin
                .take()
                .unwrap_or_else(|| panic!("hook stdin unavailable"))
                .write_all(input.as_bytes())
                .unwrap_or_else(|error| panic!("failed to write hook input: {error}"));
            child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("failed to wait for hook: {error}"))
        };
        let input = format!("{} {} refs/heads/main\n", "0".repeat(40), "1".repeat(40));
        assert!(!run("preparing", &input).status.success());
        assert!(!run("prepared", &input).status.success());
        let symref =
            "ref:refs/remotes/origin/main ref:refs/remotes/origin/next refs/remotes/origin/HEAD\n";
        assert!(run("preparing", symref).status.success());
        let pseudo = format!("{} {} ORIG_HEAD\n", "0".repeat(40), "1".repeat(40));
        assert!(run("prepared", &pseudo).status.success());
        let head_symref = "ref:refs/heads/main ref:refs/heads/next HEAD\n";
        assert!(!run("prepared", head_symref).status.success());
        assert!(run("committed", "malformed\n").status.success());
        assert!(run("aborted", "malformed\n").status.success());
    }

    #[cfg(unix)]
    #[test]
    fn real_git_commit_is_blocked_but_unrelated_refs_are_allowed() {
        let _lock = crate::test_support::lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let _home_guards = crate::test_support::set_fake_home(home.path());
        let repo = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        crate::test_support::init_git_repo(repo.path());

        let guard = CommitGuard::prepare(Some(repo.path()), &HashMap::new())
            .unwrap_or_else(|error| panic!("guard setup failed: {error}"))
            .unwrap_or_else(|| panic!("expected a Git worktree guard"));
        let before = guard.before.oid.clone();
        fs::write(repo.path().join("README.md"), "changed")
            .unwrap_or_else(|error| panic!("write failed: {error}"));

        let commit = Command::new("git")
            .args(["commit", "--no-verify", "-am", "blocked"])
            .current_dir(repo.path())
            .envs(guard.env())
            .output()
            .unwrap_or_else(|error| panic!("commit failed to start: {error}"));
        assert!(!commit.status.success());
        assert!(
            String::from_utf8_lossy(&commit.stderr).contains("cruise commit guard"),
            "unexpected commit error: {}",
            String::from_utf8_lossy(&commit.stderr)
        );
        let current = run_git(repo.path(), guard.env(), ["rev-parse", "HEAD"])
            .unwrap_or_else(|error| panic!("HEAD probe failed: {error}"));
        assert_eq!(String::from_utf8_lossy(&current.stdout).trim(), before);

        let tag = run_git(repo.path(), guard.env(), ["tag", "guard-allows-tags"])
            .unwrap_or_else(|error| panic!("tag failed to start: {error}"));
        assert!(
            tag.status.success(),
            "tag should be allowed: {}",
            String::from_utf8_lossy(&tag.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn bypassed_commit_is_rolled_back_with_changes_still_staged() {
        let _lock = crate::test_support::lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let _home_guards = crate::test_support::set_fake_home(home.path());
        let repo = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        crate::test_support::init_git_repo(repo.path());

        let guard = CommitGuard::prepare(Some(repo.path()), &HashMap::new())
            .unwrap_or_else(|error| panic!("guard setup failed: {error}"))
            .unwrap_or_else(|| panic!("expected a Git worktree guard"));
        let before = guard.before.oid.clone();
        fs::write(repo.path().join("README.md"), "changed")
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let commit = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-am",
                "bypassed",
            ])
            .current_dir(repo.path())
            .envs(guard.env())
            .output()
            .unwrap_or_else(|error| panic!("commit failed to start: {error}"));
        assert!(
            commit.status.success(),
            "bypassed commit failed: {}",
            String::from_utf8_lossy(&commit.stderr)
        );

        let result = guard.finish(Ok::<(), CruiseError>(()));
        assert!(matches!(result, Err(CruiseError::CommitGuardViolation(_))));
        let restored = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap_or_else(|error| panic!("restored HEAD probe failed: {error}"));
        assert_eq!(
            String::from_utf8_lossy(&restored.stdout).trim(),
            before.as_str()
        );
        let staged = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(repo.path())
            .output()
            .unwrap_or_else(|error| panic!("staged diff failed: {error}"));
        assert_eq!(String::from_utf8_lossy(&staged.stdout).trim(), "README.md");
        assert_eq!(
            fs::read_to_string(repo.path().join("README.md"))
                .unwrap_or_else(|error| panic!("read failed: {error}")),
            "changed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rollback_refuses_a_symbolic_branch_target() {
        let _lock = crate::test_support::lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let _home_guards = crate::test_support::set_fake_home(home.path());
        let repo = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        crate::test_support::init_git_repo(repo.path());
        let guard = CommitGuard::prepare(Some(repo.path()), &HashMap::new())
            .unwrap_or_else(|error| panic!("guard setup failed: {error}"))
            .unwrap_or_else(|| panic!("expected a Git worktree guard"));
        let before = guard.before.oid.clone();

        let tree = Command::new("git")
            .args(["rev-parse", "HEAD^{tree}"])
            .current_dir(repo.path())
            .output()
            .unwrap_or_else(|error| panic!("tree probe failed: {error}"));
        let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
        let advanced = Command::new("git")
            .args([
                "commit-tree",
                tree.as_str(),
                "-p",
                before.as_str(),
                "-m",
                "other",
            ])
            .current_dir(repo.path())
            .output()
            .unwrap_or_else(|error| panic!("commit-tree failed: {error}"));
        assert!(advanced.status.success());
        let advanced = String::from_utf8_lossy(&advanced.stdout).trim().to_string();
        crate::test_support::run_git_ok(
            repo.path(),
            &["update-ref", "refs/heads/other", advanced.as_str()],
        );
        crate::test_support::run_git_ok(
            repo.path(),
            &["symbolic-ref", "refs/heads/main", "refs/heads/other"],
        );

        let result = guard.finish(Ok::<(), CruiseError>(()));
        assert!(matches!(result, Err(CruiseError::CommitGuardViolation(_))));
        let other = Command::new("git")
            .args(["rev-parse", "refs/heads/other"])
            .current_dir(repo.path())
            .output()
            .unwrap_or_else(|error| panic!("other branch probe failed: {error}"));
        assert_eq!(
            String::from_utf8_lossy(&other.stdout).trim(),
            advanced.as_str()
        );
    }
}
