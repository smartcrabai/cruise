//! Goal-private Git primitives. Callers must durably save returned intents before
//! invoking `apply_integration`, and save verification results before publication.
//! None of these operations checks out or modifies the user's worktree.

use command_group::CommandGroup;
use std::path::Path;
use std::process::{Command, Output};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IntegrationIntent {
    pub goal_ref: String,
    pub expected_tip: String,
    pub child_base: String,
    pub child_result: String,
    pub prepared_tip: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    AlreadyApplied,
}

fn git(repo: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))
}

fn successful(output: Output, what: &str) -> Result<String, String> {
    if !output.status.success() {
        return Err(format!(
            "{what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|e| format!("{what}: invalid UTF-8: {e}"))
}

fn object(repo: &Path, sha: &str) -> Result<String, String> {
    // Require a complete object ID, not a ref or abbreviation that can move.
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected a complete commit object ID".into());
    }
    let resolved = successful(
        git(
            repo,
            &["rev-parse", "--verify", &format!("{sha}^{{commit}}")],
        )?,
        "resolve commit",
    )?;
    if resolved != sha.to_ascii_lowercase() {
        return Err("object ID does not identify the requested commit".into());
    }
    Ok(resolved)
}

fn validate_ref(repo: &Path, goal_ref: &str) -> Result<(), String> {
    if !goal_ref.starts_with("refs/heads/cruise/goal/")
        || !git(repo, &["check-ref-format", goal_ref])?.status.success()
    {
        return Err("invalid private Goal ref".into());
    }
    Ok(())
}

fn read_ref(repo: &Path, goal_ref: &str) -> Result<Option<String>, String> {
    let out = git(repo, &["rev-parse", "--verify", "--quiet", goal_ref])?;
    if out.status.success() {
        return successful(out, "read Goal ref").map(Some);
    }
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    Err(format!(
        "read Goal ref: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

fn ancestor(repo: &Path, base: &str, tip: &str) -> Result<bool, String> {
    let out = git(repo, &["merge-base", "--is-ancestor", base, tip])?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "check ancestry: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// Initialize once, or recover an already initialized ref at exactly the approved base.
pub fn initialize(repo: &Path, goal_ref: &str, base: &str) -> Result<(), String> {
    validate_ref(repo, goal_ref)?;
    let base = object(repo, base)?;
    if let Some(actual) = read_ref(repo, goal_ref)? {
        return if actual == base {
            Ok(())
        } else {
            Err(format!("Goal ref drift: expected {base}, found {actual}"))
        };
    }
    let zero = "0".repeat(base.len());
    let out = git(repo, &["update-ref", goal_ref, &base, &zero])?;
    if out.status.success() {
        return Ok(());
    }
    if read_ref(repo, goal_ref)?.as_deref() == Some(&base) {
        return Ok(());
    }
    Err(format!(
        "initialize Goal ref: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Build an immutable integration intent without updating any ref. The caller must
/// persist the entire intent before applying it, including for a no-change result.
pub fn prepare_integration(
    repo: &Path,
    goal_ref: &str,
    expected_tip: &str,
    child_base: &str,
    child_result: &str,
) -> Result<IntegrationIntent, String> {
    validate_ref(repo, goal_ref)?;
    let expected_tip = object(repo, expected_tip)?;
    let child_base = object(repo, child_base)?;
    let child_result = object(repo, child_result)?;
    if read_ref(repo, goal_ref)?.as_deref() != Some(&expected_tip) {
        return Err("Goal ref missing or drifted before integration".into());
    }
    if !ancestor(repo, &child_base, &child_result)? || !ancestor(repo, &child_base, &expected_tip)?
    {
        return Err("child base is not an ancestor of both result and Goal tip".into());
    }
    let base_tree = successful(
        git(repo, &["rev-parse", &format!("{child_base}^{{tree}}")])?,
        "resolve base tree",
    )?;
    let result_tree = successful(
        git(repo, &["rev-parse", &format!("{child_result}^{{tree}}")])?,
        "resolve result tree",
    )?;
    let prepared_tip = if base_tree == result_tree {
        expected_tip.clone()
    } else {
        let out = git(
            repo,
            &["merge-tree", "--write-tree", &expected_tip, &child_result],
        )?;
        if !out.status.success() {
            return Err(format!(
                "integration conflict or merge-tree failure: {} {}",
                String::from_utf8_lossy(&out.stdout).trim(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let tree = successful(out, "prepare merge tree")?;
        // If the child is already incorporated, avoid a redundant empty merge.
        let tip_tree = successful(
            git(repo, &["rev-parse", &format!("{expected_tip}^{{tree}}")])?,
            "resolve tip tree",
        )?;
        if tree == tip_tree {
            expected_tip.clone()
        } else {
            successful(
                git(
                    repo,
                    &[
                        "commit-tree",
                        &tree,
                        "-p",
                        &expected_tip,
                        "-p",
                        &child_result,
                        "-m",
                        "Integrate Goal task",
                    ],
                )?,
                "prepare integration commit",
            )?
        }
    };
    Ok(IntegrationIntent {
        goal_ref: goal_ref.into(),
        expected_tip,
        child_base,
        child_result,
        prepared_tip,
    })
}

/// Apply a durably recorded intent. An unexpected ref always blocks, including
/// when the ref changed between the read and the compare-and-swap.
fn validate_intent(repo: &Path, intent: &IntegrationIntent) -> Result<(String, String), String> {
    validate_ref(repo, &intent.goal_ref)?;
    let expected = object(repo, &intent.expected_tip)?;
    let prepared = object(repo, &intent.prepared_tip)?;
    let base = object(repo, &intent.child_base)?;
    let result = object(repo, &intent.child_result)?;
    if !ancestor(repo, &base, &expected)?
        || !ancestor(repo, &base, &result)?
        || !ancestor(repo, &expected, &prepared)?
    {
        return Err("invalid integration intent ancestry".into());
    }
    if prepared != expected && !ancestor(repo, &result, &prepared)? {
        return Err("prepared commit does not contain child result".into());
    }
    if prepared != expected {
        let parents = successful(
            git(repo, &["show", "-s", "--format=%P", &prepared])?,
            "inspect prepared commit parents",
        )?;
        if parents != format!("{expected} {result}") {
            return Err("prepared commit has unexpected parents".into());
        }
        let merge_tree = successful(
            git(repo, &["merge-tree", "--write-tree", &expected, &result])?,
            "validate prepared merge tree",
        )?;
        let prepared_tree = successful(
            git(repo, &["rev-parse", &format!("{prepared}^{{tree}}")])?,
            "resolve prepared tree",
        )?;
        if merge_tree != prepared_tree {
            return Err("prepared commit does not match integration merge tree".into());
        }
    }
    if prepared == expected {
        let base_tree = successful(
            git(repo, &["rev-parse", &format!("{base}^{{tree}}")])?,
            "resolve base tree",
        )?;
        let result_tree = successful(
            git(repo, &["rev-parse", &format!("{result}^{{tree}}")])?,
            "resolve result tree",
        )?;
        if base_tree != result_tree {
            let merge_tree = successful(
                git(repo, &["merge-tree", "--write-tree", &expected, &result])?,
                "validate no-change integration",
            )?;
            let expected_tree = successful(
                git(repo, &["rev-parse", &format!("{expected}^{{tree}}")])?,
                "resolve tip tree",
            )?;
            if merge_tree != expected_tree {
                return Err("no-change intent would discard child changes".into());
            }
        }
    }
    Ok((expected, prepared))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationObservation {
    Pending,
    Applied,
}

/// Validate an intent and distinguish a pending ref from an applied one.
/// Any missing or unrelated ref is drift, not evidence of a pending update.
pub fn observe_integration(repo: &Path, intent: &IntegrationIntent) -> Result<IntegrationObservation, String> {
    let (expected, prepared) = validate_intent(repo, intent)?;
    let actual = read_ref(repo, &intent.goal_ref)?;
    if actual.as_deref() == Some(prepared.as_str()) {
        Ok(IntegrationObservation::Applied)
    } else if actual.as_deref() == Some(expected.as_str()) {
        Ok(IntegrationObservation::Pending)
    } else {
        Err(format!("Goal ref drift: expected {expected} or {prepared}, found {actual:?}"))
    }
}

/// Check whether the exact validated integration is already present, without
/// updating a ref. This is safe to use when a Goal is paused after a crash.
pub fn integration_already_applied(
    repo: &Path,
    intent: &IntegrationIntent,
) -> Result<bool, String> {
    let (_, prepared) = validate_intent(repo, intent)?;
    Ok(read_ref(repo, &intent.goal_ref)?.as_deref() == Some(prepared.as_str()))
}

pub fn apply_integration(repo: &Path, intent: &IntegrationIntent) -> Result<ApplyOutcome, String> {
    let (expected, prepared) = validate_intent(repo, intent)?;
    let actual = read_ref(repo, &intent.goal_ref)?;
    if actual.as_deref() == Some(&prepared) && prepared != expected {
        return Ok(ApplyOutcome::AlreadyApplied);
    }
    if actual.as_deref() != Some(&expected) {
        return Err(format!(
            "Goal ref drift: expected {expected}, found {actual:?}"
        ));
    }
    if prepared == expected {
        return Ok(ApplyOutcome::Applied);
    }
    let out = git(
        repo,
        &["update-ref", &intent.goal_ref, &prepared, &expected],
    )?;
    if out.status.success() {
        return Ok(ApplyOutcome::Applied);
    }
    if read_ref(repo, &intent.goal_ref)?.as_deref() == Some(&prepared) {
        return Ok(ApplyOutcome::AlreadyApplied);
    }
    Err(format!(
        "Goal ref compare-and-swap failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Check Git facts before and after approved checks. The caller must separately
/// persist each check result and reject cancellation or a nonzero exit.
pub fn validate_verification_state(
    repo: &Path,
    worktree: &Path,
    goal_ref: &str,
    approved_base: &str,
    verified_tip: &str,
) -> Result<(), String> {
    validate_ref(repo, goal_ref)?;
    let common_dir = |path: &Path| -> Result<std::path::PathBuf, String> {
        let dir = successful(
            git(path, &["rev-parse", "--git-common-dir"])?,
            "resolve Git common directory",
        )?;
        std::fs::canonicalize(path.join(dir))
            .map_err(|e| format!("resolve Git common directory: {e}"))
    };
    if common_dir(repo)? != common_dir(worktree)? {
        return Err("verification worktree belongs to a different repository".into());
    }
    let base = object(repo, approved_base)?;
    let tip = object(repo, verified_tip)?;
    if read_ref(repo, goal_ref)?.as_deref() != Some(&tip) {
        return Err("Goal ref no longer matches verification SHA".into());
    }
    let head = successful(
        git(worktree, &["rev-parse", "--verify", "HEAD^{commit}"])?,
        "read verification HEAD",
    )?;
    if head != tip {
        return Err("verification worktree HEAD changed".into());
    }
    match git(worktree, &["symbolic-ref", "--quiet", "HEAD"])?
        .status
        .code()
    {
        Some(1) => {}
        Some(0) => return Err("verification worktree is not detached".into()),
        _ => return Err("cannot determine verification worktree HEAD".into()),
    }
    let status = successful(
        git(
            worktree,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--ignore-submodules=none",
            ],
        )?,
        "inspect verification worktree",
    )?;
    if !status.is_empty() {
        return Err("verification worktree has modified tracked files".into());
    }
    let base_tree = successful(
        git(repo, &["rev-parse", &format!("{base}^{{tree}}")])?,
        "resolve approved base tree",
    )?;
    let tip_tree = successful(
        git(repo, &["rev-parse", &format!("{tip}^{{tree}}")])?,
        "resolve verified tree",
    )?;
    if base_tree == tip_tree {
        return Err("Goal has no net change from approved base".into());
    }
    if !ancestor(repo, &base, &tip)? {
        return Err("verified tip does not descend from approved base".into());
    }
    Ok(())
}

/// Evidence for one approved check. The caller must persist an intent before
/// invoking the check and persist this result before starting another check or
/// publishing. An absent exit code is never a successful check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckEvidence {
    pub check_id: String,
    pub verified_tip: String,
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub stdout_log: std::path::PathBuf,
    pub stderr_log: std::path::PathBuf,
    pub state_error: Option<String>,
}

impl CheckEvidence {
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.cancelled && self.state_error.is_none()
    }
}

/// Run exactly one approved argv in an already-created detached worktree.
/// Log paths must be new, so a retry cannot overwrite earlier evidence. This
/// function blocks and should run on a blocking worker, not an async executor.
pub fn run_verification_check(
    repo: &Path,
    worktree: &Path,
    goal_ref: &str,
    approved_base: &str,
    verified_tip: &str,
    check_id: &str,
    argv: &[String],
    stdout_log: &Path,
    stderr_log: &Path,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<CheckEvidence, String> {
    run_verification_check_inner(
        repo,
        worktree,
        goal_ref,
        approved_base,
        verified_tip,
        check_id,
        argv,
        stdout_log,
        stderr_log,
        cancel,
        None,
    )
}

pub(crate) fn run_verification_check_guarded(
    repo: &Path,
    worktree: &Path,
    goal_ref: &str,
    approved_base: &str,
    verified_tip: &str,
    check_id: &str,
    argv: &[String],
    stdout_log: &Path,
    stderr_log: &Path,
    cancel: &crate::cancellation::CancellationToken,
    goal_id: &str,
) -> Result<CheckEvidence, String> {
    run_verification_check_inner(
        repo,
        worktree,
        goal_ref,
        approved_base,
        verified_tip,
        check_id,
        argv,
        stdout_log,
        stderr_log,
        cancel,
        Some(goal_id),
    )
}

fn run_verification_check_inner(
    repo: &Path,
    worktree: &Path,
    goal_ref: &str,
    approved_base: &str,
    verified_tip: &str,
    check_id: &str,
    argv: &[String],
    stdout_log: &Path,
    stderr_log: &Path,
    cancel: &crate::cancellation::CancellationToken,
    goal_id: Option<&str>,
) -> Result<CheckEvidence, String> {
    use std::process::Stdio;
    if check_id.trim().is_empty() || argv.is_empty() || argv[0].is_empty() {
        return Err("invalid approved check".into());
    }
    if cancel.is_cancelled() {
        return Err("verification cancelled before check started".into());
    }
    validate_verification_state(repo, worktree, goal_ref, approved_base, verified_tip)?;
    let open_log = |path: &Path| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("cannot create check log {}: {e}", path.display()))
    };
    let stdout = open_log(stdout_log)?;
    let stderr = open_log(stderr_log)?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let gate = goal_id
        .map(|id| {
            crate::manager_publication::effect_guard(repo, id, cancel).map_err(|e| e.to_string())
        })
        .transpose()?;
    let mut child = command
        .group_spawn()
        .map_err(|e| format!("cannot start check: {e}"))?;
    drop(gate);
    let mut cancelled = false;
    let exit_code = loop {
        if cancel.is_cancelled() {
            cancelled = true;
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for check: {e}"));
            }
        }
    };
    cancelled |= cancel.is_cancelled();
    let state_error =
        validate_verification_state(repo, worktree, goal_ref, approved_base, verified_tip).err();
    Ok(CheckEvidence {
        check_id: check_id.to_owned(),
        verified_tip: verified_tip.to_owned(),
        argv: argv.to_vec(),
        exit_code,
        cancelled,
        stdout_log: stdout_log.to_path_buf(),
        stderr_log: stderr_log.to_path_buf(),
        state_error,
    })
}
