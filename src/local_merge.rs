//! Safe, explicit squash integration of a session branch into its base checkout.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::error::{CruiseError, Result};
use crate::session::{LocalMergeIntent, SessionPhase, SessionState, WorkspaceMode};
use crate::worktree::WorktreeContext;
use crate::worktree_pr::commit_message_with_coauthor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalMergeMode {
    Stage,
    MergeAndCleanup,
}

#[cfg(test)]
mod local_merge_regression_tests {
    use super::*;
    use crate::session_config::SessionConfigRef;
    use crate::test_support::{init_git_repo, run_git_ok};
    use std::cell::RefCell;
    use std::fs;
    use tempfile::TempDir;

    type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

    struct Fixture {
        _tmp: TempDir,
        state: SessionState,
    }

    impl Fixture {
        fn new() -> std::result::Result<Self, Box<dyn std::error::Error>> {
            let tmp = TempDir::new()?;
            let base = tmp.path().join("base");
            fs::create_dir(&base)?;
            init_git_repo(&base);
            // Keep fixture bytes stable even when the user's Git config enables CRLF checkout.
            run_git_ok(&base, &["config", "core.autocrlf", "false"]);
            run_git_ok(&base, &["checkout", "-B", "main"]);
            let (ctx, _) = crate::worktree::setup_session_worktree(
                &base,
                "local-merge-test",
                "task",
                &tmp.path().join("worktrees"),
                None,
                &crate::worktree::WorktreeBase::default(),
            )?;
            let mut state = SessionState::new(
                "local-merge-test".into(),
                base,
                SessionConfigRef::BuiltinSnapshot { name: None },
                "task".into(),
            );
            state.phase = SessionPhase::Completed;
            state.forked_from_branch = Some("main".into());
            state.worktree_path = Some(ctx.path);
            state.worktree_branch = Some(ctx.branch);
            Ok(Self { _tmp: tmp, state })
        }

        fn branch_change(&self) -> std::result::Result<(), Box<dyn std::error::Error>> {
            let wt = self
                .state
                .worktree_path
                .as_ref()
                .ok_or("fixture worktree path missing")?;
            fs::write(wt.join("change.txt"), "session\n")?;
            run_git_ok(wt, &["add", "change.txt"]);
            run_git_ok(wt, &["commit", "-m", "session change"]);
            Ok(())
        }

        fn head(&self) -> std::result::Result<String, Box<dyn std::error::Error>> {
            Ok(checked(&self.state.base_dir, &["rev-parse", "HEAD"])?)
        }

        fn clean(&self) -> TestResult<bool> {
            Ok(checked(
                &self.state.base_dir,
                &["status", "--porcelain", "--untracked-files=no"],
            )?
            .is_empty())
        }
    }

    #[test]
    fn eligibility_requires_completed_local_worktree_without_pr_and_target() -> TestResult {
        let f = Fixture::new()?;
        let state = f.state;
        assert!(can_merge_locally(&state));
        assert!(can_stage_locally(&state));
        let mut failed = state.clone();
        failed.phase = SessionPhase::PrFailed("push failed".into());
        assert!(can_merge_locally(&failed));
        assert!(can_stage_locally(&failed));
        let mut configured = state.clone();
        configured.worktree_base_branch = Some("release".into());
        assert_eq!(merge_target_branch(&configured), Some("release"));
        configured.worktree_base_branch = None;
        assert_eq!(merge_target_branch(&configured), Some("main"));
        for phase in [
            SessionPhase::Planned,
            SessionPhase::Running,
            SessionPhase::Failed("failed".into()),
            SessionPhase::Suspended,
            SessionPhase::AwaitingApproval,
        ] {
            let mut s = state.clone();
            s.phase = phase;
            assert!(!can_merge_locally(&s));
        }
        let mut s = state.clone();
        s.pr_url = Some("https://example.test/pr/1".into());
        assert!(!can_merge_locally(&s));
        let mut s = state.clone();
        s.repo = Some("remote/repo".into());
        assert!(!can_merge_locally(&s));
        let mut s = state.clone();
        s.workspace_mode = WorkspaceMode::CurrentBranch;
        assert!(!can_merge_locally(&s));
        let mut s = state.clone();
        s.worktree_branch = None;
        assert!(!can_merge_locally(&s));
        let mut s = state.clone();
        s.forked_from_branch = None;
        assert!(!can_merge_locally(&s));
        let mut s = state;
        s.local_merge_commit = Some("marker".into());
        assert!(can_merge_locally(&s));
        assert!(!can_stage_locally(&s));
        assert!(merge_session(&mut s, LocalMergeMode::Stage, |_| Ok(()), |_| Ok(())).is_err());
        Ok(())
    }

    #[test]
    fn detached_and_wrong_branch_are_rejected_without_changes() -> TestResult {
        let mut f = Fixture::new()?;
        f.branch_change()?;
        let head = f.head()?;
        run_git_ok(&f.state.base_dir, &["checkout", "--detach"]);
        let err = merge_session(&mut f.state, LocalMergeMode::Stage, |_| Ok(()), |_| Ok(()))
            .err()
            .ok_or("expected operation to fail")?
            .to_string();
        assert!(err.contains("detached"), "{err}");
        assert_eq!(f.head()?, head);
        run_git_ok(&f.state.base_dir, &["checkout", "-b", "develop"]);
        let err = merge_session(&mut f.state, LocalMergeMode::Stage, |_| Ok(()), |_| Ok(()))
            .err()
            .ok_or("expected operation to fail")?
            .to_string();
        assert!(err.contains("develop") && err.contains("main"), "{err}");
        assert_eq!(f.head()?, head);
        assert!(f.clean()?);
        Ok(())
    }

    #[test]
    fn no_branch_changes_are_a_noop() -> TestResult {
        let mut f = Fixture::new()?;
        let head = f.head()?;
        let err = merge_session(&mut f.state, LocalMergeMode::Stage, |_| Ok(()), |_| Ok(()))
            .err()
            .ok_or("expected operation to fail")?
            .to_string();
        assert!(err.contains("nothing to merge"), "{err}");
        assert_eq!(f.head()?, head);
        assert!(f.clean()?);
        assert!(
            f.state
                .worktree_path
                .as_ref()
                .ok_or("fixture worktree path missing")?
                .is_dir()
        );
        Ok(())
    }

    #[test]
    fn conflict_rolls_back_base_and_reports_path() -> TestResult {
        let mut f = Fixture::new()?;
        let base = f.state.base_dir.clone();
        let wt = f
            .state
            .worktree_path
            .as_ref()
            .ok_or("fixture worktree path missing")?;
        fs::write(wt.join("conflict.txt"), "session\n")?;
        run_git_ok(wt, &["add", "conflict.txt"]);
        run_git_ok(wt, &["commit", "-m", "session"]);
        fs::write(base.join("conflict.txt"), "base\n")?;
        run_git_ok(&base, &["add", "conflict.txt"]);
        run_git_ok(&base, &["commit", "-m", "base"]);
        let head = f.head()?;
        let err = merge_session(&mut f.state, LocalMergeMode::Stage, |_| Ok(()), |_| Ok(()))
            .err()
            .ok_or("expected operation to fail")?
            .to_string();
        assert!(err.contains("conflict.txt"), "{err}");
        assert_eq!(f.head()?, head);
        assert!(f.clean()?);
        assert_eq!(fs::read_to_string(base.join("conflict.txt"))?, "base\n");
        assert!(
            f.state
                .worktree_path
                .as_ref()
                .ok_or("fixture worktree path missing")?
                .is_dir()
        );
        Ok(())
    }

    #[test]
    fn cleanup_failure_persists_marker_and_retry_does_not_commit_again() -> TestResult {
        let mut f = Fixture::new()?;
        f.branch_change()?;
        let saved = RefCell::new(None);
        let outcome = merge_session(
            &mut f.state,
            LocalMergeMode::MergeAndCleanup,
            |s| {
                *saved.borrow_mut() = s.local_merge_commit.clone();
                Ok(())
            },
            |_| Err(CruiseError::Other("injected cleanup failure".into())),
        )?;
        assert!(matches!(
            outcome,
            LocalMergeOutcome::MergedCleanupFailed { .. }
        ));
        let head = f.head()?;
        assert_eq!(saved.borrow().as_deref(), Some(head.as_str()));
        assert_eq!(f.state.local_merge_commit.as_deref(), Some(head.as_str()));
        assert!(
            f.state
                .worktree_path
                .as_ref()
                .ok_or("fixture worktree path missing")?
                .is_dir()
        );
        let outcome = merge_session(
            &mut f.state,
            LocalMergeMode::MergeAndCleanup,
            |_| panic!("retry must not persist another commit"),
            crate::worktree::cleanup_worktree,
        )?;
        assert_eq!(outcome, LocalMergeOutcome::Merged);
        assert_eq!(f.head()?, head);
        assert!(
            !f.state
                .worktree_path
                .as_ref()
                .ok_or("fixture worktree path missing")?
                .exists()
        );
        assert_eq!(
            checked(
                &f.state.base_dir,
                &[
                    "branch",
                    "--list",
                    f.state
                        .worktree_branch
                        .as_deref()
                        .ok_or("fixture branch missing")?
                ]
            )?,
            ""
        );
        Ok(())
    }

    #[test]
    fn intent_persistence_failure_prevents_commit_and_cleanup() -> TestResult {
        let mut f = Fixture::new()?;
        f.branch_change()?;
        let head = f.head()?;
        let error = merge_session(
            &mut f.state,
            LocalMergeMode::MergeAndCleanup,
            |_| Err(CruiseError::Other("injected persistence failure".into())),
            |_| panic!("cleanup must not run"),
        )
        .err()
        .ok_or("expected operation to fail")?
        .to_string();
        assert!(
            error.contains("intent") && error.contains("not attempted"),
            "{error}"
        );
        assert_eq!(f.head()?, head);
        assert!(f.clean()?);
        assert!(f.state.local_merge_intent.is_none());
        Ok(())
    }

    #[test]
    fn interrupted_marker_persistence_recovers_without_second_commit() -> TestResult {
        let mut f = Fixture::new()?;
        f.branch_change()?;
        let durable = RefCell::new(None);
        let error = merge_session(
            &mut f.state,
            LocalMergeMode::MergeAndCleanup,
            |s| {
                if s.local_merge_commit.is_some() {
                    Err(CruiseError::Other("injected marker failure".into()))
                } else {
                    *durable.borrow_mut() = Some(s.clone());
                    Ok(())
                }
            },
            |_| panic!("cleanup must not run"),
        )
        .err()
        .ok_or("expected operation to fail")?
        .to_string();
        assert!(
            error.contains("marker") && error.contains("not attempted"),
            "{error}"
        );
        let head = f.head()?;
        let mut restarted = durable.into_inner().ok_or("persisted state missing")?;
        assert!(restarted.local_merge_commit.is_none());
        let saved = RefCell::new(None);
        let result = merge_session(
            &mut restarted,
            LocalMergeMode::MergeAndCleanup,
            |s| {
                *saved.borrow_mut() = s.local_merge_commit.clone();
                Ok(())
            },
            |_| Err(CruiseError::Other("retain fixture".into())),
        )?;
        assert!(matches!(
            result,
            LocalMergeOutcome::MergedCleanupFailed { .. }
        ));
        assert_eq!(saved.borrow().as_deref(), Some(head.as_str()));
        assert_eq!(f.head()?, head);
        Ok(())
    }

    #[test]
    fn interrupted_intent_rejects_mismatched_parent_or_tree() -> TestResult {
        let mut f = Fixture::new()?;
        f.branch_change()?;
        let durable = RefCell::new(None);
        let _ = merge_session(
            &mut f.state,
            LocalMergeMode::MergeAndCleanup,
            |s| {
                if s.local_merge_commit.is_some() {
                    return Err(CruiseError::Other("interrupt".into()));
                }
                *durable.borrow_mut() = Some(s.clone());
                Ok(())
            },
            |_| panic!("cleanup must not run"),
        );
        let original = durable.into_inner().ok_or("persisted state missing")?;
        for parent_mismatch in [true, false] {
            let mut state = original.clone();
            let intent = state.local_merge_intent.as_mut().ok_or("intent missing")?;
            if parent_mismatch {
                intent.base_parent = "wrong".into();
            } else {
                intent.squash_tree = "wrong".into();
            }
            let error = merge_session(
                &mut state,
                LocalMergeMode::MergeAndCleanup,
                |_| panic!("must not persist"),
                |_| panic!("must not clean up"),
            )
            .err()
            .ok_or("expected operation to fail")?
            .to_string();
            assert!(error.contains("does not match"), "{error}");
            assert!(state.local_merge_commit.is_none());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejecting_hook_clears_intent_and_retry_commits_once() -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        let mut f = Fixture::new()?;
        f.branch_change()?;
        let parent = f.head()?;
        let hook = f.state.base_dir.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n")?;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
        let durable = RefCell::new(None);
        let error = merge_session(
            &mut f.state,
            LocalMergeMode::MergeAndCleanup,
            |s| {
                *durable.borrow_mut() = Some(s.clone());
                Ok(())
            },
            |_| panic!("cleanup must not run"),
        )
        .err()
        .ok_or("expected operation to fail")?
        .to_string();
        assert!(error.contains("squash commit failed"), "{error}");
        assert_eq!(f.head()?, parent);
        assert!(f.clean()?);
        assert!(
            durable
                .borrow()
                .as_ref()
                .ok_or("persisted state missing")?
                .local_merge_intent
                .is_none()
        );
        fs::remove_file(hook)?;
        let mut restarted = durable.into_inner().ok_or("persisted state missing")?;
        assert_eq!(
            merge_session(
                &mut restarted,
                LocalMergeMode::MergeAndCleanup,
                |_| Ok(()),
                crate::worktree::cleanup_worktree
            )?,
            LocalMergeOutcome::Merged
        );
        assert_eq!(
            checked(
                &f.state.base_dir,
                &["rev-list", "--count", &format!("{parent}..HEAD")]
            )?,
            "1"
        );
        assert!(
            !restarted
                .worktree_path
                .as_ref()
                .ok_or("fixture worktree path missing")?
                .exists()
        );
        assert_eq!(
            checked(
                &f.state.base_dir,
                &[
                    "branch",
                    "--list",
                    restarted
                        .worktree_branch
                        .as_deref()
                        .ok_or("fixture branch missing")?
                ]
            )?,
            ""
        );
        Ok(())
    }

    #[test]
    fn precommit_interruption_recovers_only_verified_squash() -> TestResult {
        let mut f = Fixture::new()?;
        f.branch_change()?;
        let base = f.state.base_dir.clone();
        let parent = f.head()?;
        let source = checked(
            &base,
            &[
                "rev-parse",
                f.state
                    .worktree_branch
                    .as_deref()
                    .ok_or("fixture branch missing")?,
            ],
        )?;
        checked(&base, &["merge", "--squash", &source])?;
        f.state.local_merge_intent = Some(LocalMergeIntent {
            base_parent: parent.clone(),
            source_commit: source,
            squash_tree: checked(&base, &["write-tree"])?,
            token: "interrupted".into(),
        });
        fs::write(base.join("change.txt"), "unverified\n")?;
        let error = merge_session(
            &mut f.state,
            LocalMergeMode::MergeAndCleanup,
            |_| panic!("must not persist"),
            |_| panic!("must not clean up"),
        )
        .err()
        .ok_or("expected operation to fail")?
        .to_string();
        assert!(error.contains("preserve"), "{error}");
        assert_eq!(fs::read_to_string(base.join("change.txt"))?, "unverified\n");
        assert!(f.state.local_merge_intent.is_some());
        fs::write(base.join("change.txt"), "session\n")?;
        assert_eq!(
            merge_session(
                &mut f.state,
                LocalMergeMode::MergeAndCleanup,
                |_| Ok(()),
                crate::worktree::cleanup_worktree
            )?,
            LocalMergeOutcome::Merged
        );
        assert_eq!(
            checked(&base, &["rev-list", "--count", &format!("{parent}..HEAD")])?,
            "1"
        );
        assert!(
            !f.state
                .worktree_path
                .as_ref()
                .ok_or("fixture worktree path missing")?
                .exists()
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalMergeOutcome {
    Staged,
    Merged,
    MergedCleanupFailed { error: String },
}

#[must_use]
pub fn merge_target_branch(state: &SessionState) -> Option<&str> {
    state
        .worktree_base_branch
        .as_deref()
        .or(state.forked_from_branch.as_deref())
}

#[must_use]
pub fn can_merge_locally(state: &SessionState) -> bool {
    matches!(
        state.phase,
        SessionPhase::Completed | SessionPhase::PrFailed(_)
    ) && state.repo.is_none()
        && state.pr_url.is_none()
        && state.pr_target.is_none()
        && state.workspace_mode == WorkspaceMode::Worktree
        && state.worktree_branch.is_some()
        && merge_target_branch(state).is_some()
}

#[must_use]
pub fn can_stage_locally(state: &SessionState) -> bool {
    can_merge_locally(state)
        && state.local_merge_commit.is_none()
        && state.local_merge_intent.is_none()
}

fn git(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git: {e}")))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

fn checked(dir: &Path, args: &[&str]) -> Result<String> {
    let output = git(dir, args)?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "git {} failed: {}",
            args.join(" "),
            text(&output.stderr)
        )));
    }
    Ok(text(&output.stdout))
}

fn restore_base(base: &Path) -> bool {
    git(base, &["reset", "--merge"]).is_ok_and(|o| o.status.success())
        && git(base, &["status", "--porcelain", "--untracked-files=no"])
            .is_ok_and(|o| o.status.success() && o.stdout.is_empty())
}

fn rollback(base: &Path, reason: String) -> CruiseError {
    if restore_base(base) {
        CruiseError::Other(reason)
    } else {
        CruiseError::Other(format!(
            "{reason}; base checkout not restored; inspect git status"
        ))
    }
}

fn clear_intent(
    state: &mut SessionState,
    persist: &impl Fn(&SessionState) -> Result<()>,
) -> Result<()> {
    let intent = state.local_merge_intent.take();
    if let Err(error) = persist(state) {
        state.local_merge_intent = intent;
        return Err(CruiseError::Other(format!(
            "could not persist cleared local merge intent: {error}"
        )));
    }
    Ok(())
}

fn clean_base(base: &Path, target: &str) -> Result<()> {
    let branch = git(base, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !branch.status.success() {
        return Err(CruiseError::Other("base checkout is detached".into()));
    }
    let branch = text(&branch.stdout);
    if branch != target {
        return Err(CruiseError::Other(format!(
            "base branch {branch} does not match target {target}"
        )));
    }
    checked(
        base,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{target}"),
        ],
    )?;
    if !checked(base, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err(CruiseError::Other(
            "base checkout has uncommitted changes".into(),
        ));
    }
    let upstream = git(
        base,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )?;
    if upstream.status.success() {
        let count = checked(base, &["rev-list", "--count", "HEAD..@{u}"])?;
        if count != "0" {
            return Err(CruiseError::Other(
                "base branch is behind upstream; run git pull --ff-only first".into(),
            ));
        }
    }
    Ok(())
}

fn verify_worktree(ctx: &WorktreeContext) -> Result<()> {
    let expected = checked(&ctx.original_dir, &["rev-parse", "--git-common-dir"])?;
    let actual = checked(&ctx.path, &["rev-parse", "--git-common-dir"])?;
    let expected = std::fs::canonicalize(ctx.original_dir.join(expected))?;
    let actual = std::fs::canonicalize(ctx.path.join(actual))?;
    let branch = checked(&ctx.path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if expected != actual || branch != ctx.branch {
        return Err(CruiseError::Other(format!(
            "session worktree repository or branch mismatch (expected {})",
            ctx.branch
        )));
    }
    Ok(())
}

/// Squash the session branch, optionally commit it and clean up its workspace.
/// The persistence callback is invoked before any destructive cleanup.
///
/// # Errors
/// Returns an error if the session is ineligible, git operations fail, or recovery is unsafe.
pub fn merge_session(
    state: &mut SessionState,
    mode: LocalMergeMode,
    persist: impl Fn(&SessionState) -> Result<()>,
    cleanup: impl Fn(&WorktreeContext) -> Result<()>,
) -> Result<LocalMergeOutcome> {
    if !can_merge_locally(state) || (mode == LocalMergeMode::Stage && !can_stage_locally(state)) {
        return Err(CruiseError::Other(
            "session is not eligible for local merge".into(),
        ));
    }
    let branch = state
        .worktree_branch
        .clone()
        .ok_or_else(|| CruiseError::Other("session branch unavailable".into()))?;
    let base = state.base_dir.clone();
    let ctx = WorktreeContext {
        path: state.worktree_path.clone().unwrap_or_default(),
        branch: branch.clone(),
        original_dir: base.clone(),
        owns_branch: state.pr_target.is_none(),
    };
    recover_intent(state, &base, &persist)?;
    if state.local_merge_commit.is_none() && squash_session(state, mode, &persist, &ctx, &branch)? {
        return Ok(LocalMergeOutcome::Staged);
    }
    finish_cleanup(state, &base, &ctx, &branch, &cleanup)
}

fn recover_intent(
    state: &mut SessionState,
    base: &Path,
    persist: &impl Fn(&SessionState) -> Result<()>,
) -> Result<()> {
    if let (None, Some(intent)) = (&state.local_merge_commit, &state.local_merge_intent) {
        let head = checked(base, &["rev-parse", "HEAD"])?;
        if head == intent.base_parent {
            let target = merge_target_branch(state)
                .ok_or_else(|| CruiseError::Other("merge target unavailable".into()))?;
            if checked(base, &["symbolic-ref", "--quiet", "--short", "HEAD"])? != target {
                return Err(CruiseError::Other(
                    "base branch does not match target; preserve the session and recover manually"
                        .into(),
                ));
            }
            let status = checked(base, &["status", "--porcelain", "--untracked-files=no"])?;
            if status.is_empty() {
                return Err(CruiseError::Other(
                    "local merge intent has no staged changes; preserve the session and recover manually".into(),
                ));
            }
            let tree = checked(base, &["write-tree"])?;
            let unstaged = git(base, &["diff", "--quiet"])?;
            if tree != intent.squash_tree
                || !unstaged.status.success()
                || !restore_base(base)
                || checked(base, &["rev-parse", "HEAD"])? != head
            {
                return Err(CruiseError::Other("local merge intent checkout does not match; preserve changes and recover manually".into()));
            }
            clean_base(base, target)?;
            clear_intent(state, persist)?;
        }
    }
    if let (None, Some(intent)) = (&state.local_merge_commit, &state.local_merge_intent) {
        clean_base(
            base,
            merge_target_branch(state)
                .ok_or_else(|| CruiseError::Other("merge target unavailable".into()))?,
        )?;
        let head = checked(base, &["rev-parse", "HEAD"])?;
        let parents = checked(base, &["show", "-s", "--format=%P", &head])?;
        let tree = checked(base, &["rev-parse", &format!("{head}^{{tree}}")])?;
        let message = checked(base, &["show", "-s", "--format=%B", &head])?;
        if parents != intent.base_parent
            || tree != intent.squash_tree
            || intent.token.is_empty()
            || !message
                .lines()
                .any(|line| line == format!("Cruise-Local-Merge: {}", intent.token))
        {
            return Err(CruiseError::Other("local merge intent does not match base HEAD; preserve the session and recover manually".into()));
        }
        state.local_merge_source_commit = Some(intent.source_commit.clone());
        state.local_merge_commit = Some(head);
        if let Err(error) = persist(state) {
            state.local_merge_commit = None;
            return Err(CruiseError::Other(format!(
                "could not persist recovered local merge marker; cleanup was not attempted: {error}"
            )));
        }
    }
    Ok(())
}

fn squash_session(
    state: &mut SessionState,
    mode: LocalMergeMode,
    persist: &impl Fn(&SessionState) -> Result<()>,
    ctx: &WorktreeContext,
    branch: &str,
) -> Result<bool> {
    let base = &state.base_dir.clone();
    if ctx.path.is_dir() {
        verify_worktree(ctx)?;
    }
    clean_base(
        base,
        merge_target_branch(state)
            .ok_or_else(|| CruiseError::Other("merge target unavailable".into()))?,
    )?;
    let subject = commit_message_with_coauthor(state.title_or_input());
    if ctx.path.is_dir()
        && !checked(
            &ctx.path,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty()
    {
        checked(&ctx.path, &["add", "-A"])?;
        let diff = git(&ctx.path, &["diff", "--cached", "--quiet"])?;
        if diff.status.code() == Some(1) {
            checked(&ctx.path, &["commit", "-m", &subject])?;
        } else if !diff.status.success() {
            return Err(CruiseError::Other(format!(
                "git diff failed: {}",
                text(&diff.stderr)
            )));
        }
    }
    let sha = checked(
        base,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )?;
    let merge_base = checked(base, &["merge-base", "HEAD", &sha])?;
    if checked(
        base,
        &["rev-list", "--count", &format!("{merge_base}..{sha}")],
    )? == "0"
    {
        return Err(CruiseError::Other("nothing to merge".into()));
    }
    let merge = git(base, &["merge", "--squash", &sha])?;
    if !merge.status.success() {
        let paths = checked(base, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
        return Err(rollback(
            base,
            format!(
                "squash merge failed: {} {} {}",
                text(&merge.stderr),
                text(&merge.stdout),
                paths
            ),
        ));
    }
    let diff = git(base, &["diff", "--cached", "--quiet"])
        .map_err(|error| rollback(base, format!("cannot inspect squash result: {error}")))?;
    if diff.status.success() {
        return Err(CruiseError::Other(
            "nothing to merge: changes already in base".into(),
        ));
    }
    if diff.status.code() != Some(1) {
        return Err(rollback(
            base,
            format!("git diff failed: {}", text(&diff.stderr)),
        ));
    }
    if mode == LocalMergeMode::Stage {
        return Ok(true);
    }
    commit_squash(state, persist, base, sha, &subject)?;
    Ok(false)
}

fn commit_squash(
    state: &mut SessionState,
    persist: &impl Fn(&SessionState) -> Result<()>,
    base: &Path,
    sha: String,
    subject: &str,
) -> Result<()> {
    let intent = LocalMergeIntent {
        base_parent: checked(base, &["rev-parse", "HEAD"])
            .map_err(|error| rollback(base, format!("cannot record base parent: {error}")))?,
        source_commit: sha.clone(),
        squash_tree: checked(base, &["write-tree"])
            .map_err(|error| rollback(base, format!("cannot record squash tree: {error}")))?,
        token: uuid::Uuid::new_v4().to_string(),
    };
    state.local_merge_intent = Some(intent.clone());
    if let Err(error) = persist(state) {
        state.local_merge_intent = None;
        return Err(rollback(
            base,
            format!("could not persist local merge intent; commit was not attempted: {error}"),
        ));
    }
    let trailer = format!("Cruise-Local-Merge: {}", intent.token);
    let commit = git(base, &["commit", "-m", subject, "-m", &trailer])
        .map_err(|error| rollback(base, format!("cannot run squash commit: {error}")))?;
    if !commit.status.success() {
        let reason = format!("squash commit failed: {}", text(&commit.stderr));
        let parent = &intent.base_parent;
        if checked(base, &["rev-parse", "HEAD"]).is_ok_and(|head| head == *parent)
            && restore_base(base)
            && checked(base, &["rev-parse", "HEAD"]).is_ok_and(|head| head == *parent)
        {
            if let Err(error) = clear_intent(state, persist) {
                return Err(CruiseError::Other(format!("{reason}; {error}")));
            }
            return Err(CruiseError::Other(reason));
        }
        return Err(CruiseError::Other(format!(
            "{reason}; base checkout not safely restored; inspect git status"
        )));
    }
    let head = checked(base, &["rev-parse", "HEAD"])?;
    if checked(base, &["show", "-s", "--format=%P", &head])? != intent.base_parent
        || checked(base, &["rev-parse", &format!("{head}^{{tree}}")])? != intent.squash_tree
        || !checked(base, &["show", "-s", "--format=%B", &head])?
            .lines()
            .any(|line| line == trailer)
    {
        return Err(CruiseError::Other("created commit does not match local merge intent; preserve the session and recover manually".into()));
    }
    state.local_merge_commit = Some(head);
    state.local_merge_source_commit = Some(sha);
    if let Err(error) = persist(state) {
        state.local_merge_commit = None;
        return Err(CruiseError::Other(format!(
            "base squash commit succeeded but could not persist local merge marker; cleanup was not attempted: {error}"
        )));
    }
    Ok(())
}

fn finish_cleanup(
    state: &SessionState,
    base: &Path,
    ctx: &WorktreeContext,
    branch: &str,
    cleanup: &impl Fn(&WorktreeContext) -> Result<()>,
) -> Result<LocalMergeOutcome> {
    let source = state.local_merge_source_commit.as_deref().ok_or_else(|| CruiseError::Other(
        "merged source commit is unavailable; inspect the session branch and recover manually before cleanup".into()
    ))?;
    let reference = format!("refs/heads/{branch}");
    let exists = git(base, &["show-ref", "--verify", "--quiet", &reference])?;
    if exists.status.success() {
        let current = checked(base, &["rev-parse", "--verify", &reference])?;
        if current != source {
            return Err(CruiseError::Other(
                "session branch changed since the squash; preserve and recover the work manually"
                    .into(),
            ));
        }
    } else if exists.status.code() != Some(1) || ctx.path.is_dir() {
        return Err(CruiseError::Other(
            "session branch unavailable; preserve and recover the work manually".into(),
        ));
    }
    if ctx.path.is_dir() {
        verify_worktree(ctx)?;
        if !checked(
            &ctx.path,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty()
        {
            return Err(CruiseError::Other(
                "session worktree has new changes; preserve and recover them before cleanup".into(),
            ));
        }
    }
    let result = if ctx.path.is_dir() {
        cleanup(ctx)
    } else {
        checked(base, &["worktree", "prune"]).and_then(|_| {
            let exists = git(
                base,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
            )?;
            if exists.status.success() {
                checked(base, &["branch", "-D", branch])?;
            } else if exists.status.code() != Some(1) {
                return Err(CruiseError::Other(format!(
                    "git show-ref failed: {}",
                    text(&exists.stderr)
                )));
            }
            Ok(())
        })
    };
    match result {
        Ok(()) => Ok(LocalMergeOutcome::Merged),
        Err(error) => Ok(LocalMergeOutcome::MergedCleanupFailed {
            error: error.to_string(),
        }),
    }
}
