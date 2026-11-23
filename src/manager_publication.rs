//! Durable, fail-closed verification and publication of a single Goal.
//! The coordinator must serialize calls for a Goal and drain this blocking work
//! during shutdown. An interrupted check or create is never replayed implicitly.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{CruiseError, Result};
use crate::manager::{self, Goal, GoalStatus};
use crate::manager_forge::{self, PullRequest, PullRequestState};
use crate::manager_git::CheckEvidence;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Publication {
    tip: String,
    worktree: PathBuf,
    branch: String,
    phase: Phase,
    checks: Vec<CheckEvidence>,
    pending_check: Option<PendingCheck>,
    request: Option<PullRequest>,
    forge_kind: crate::forge::ForgeKind,
    forge_host: String,
    forge_repository: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingCheck {
    check_id: String,
    argv: Vec<String>,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    WorktreeIntent,
    Ready,
    CheckIntent,
    Verified,
    PushIntent,
    Pushed,
    CreateIntent,
    Published,
}

fn error(message: impl Into<String>) -> CruiseError {
    CruiseError::Other(message.into())
}
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(error(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    String::from_utf8(out.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|e| error(e.to_string()))
}
fn load(cwd: &Path, id: &str) -> Result<Goal> {
    manager::snapshot(cwd)?
        .goals
        .into_iter()
        .find(|g| g.id == id)
        .ok_or_else(|| error("unknown Manager Goal"))
}

/// Linearize the start of a new external effect with durable pause. Callers
/// hold this only through process spawn, never while waiting for a long check.
pub(crate) fn effect_guard(
    cwd: &Path,
    id: &str,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<std::sync::MutexGuard<'static, ()>> {
    let guard = crate::manager_coordinator::CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cancel.is_cancelled() || load(cwd, id)?.status != GoalStatus::Verifying {
        return Err(error("Goal is paused, stopped, or no longer verifying"));
    }
    Ok(guard)
}
fn anchor(cwd: &Path, goal: &Goal) -> Result<PathBuf> {
    let anchor = goal
        .execution
        .anchor
        .as_ref()
        .ok_or_else(|| error("missing approved checkout anchor"))?;
    if anchor.canonicalize()? != *anchor
        || PathBuf::from(git(anchor, &["rev-parse", "--show-toplevel"])?).canonicalize()? != *anchor
        || manager::snapshot(anchor)?.repository != manager::snapshot(cwd)?.repository
    {
        return Err(error("Goal repository anchor changed"));
    }
    if git(anchor, &["remote", "get-url", "origin"])?
        != goal
            .execution
            .origin
            .as_deref()
            .ok_or_else(|| error("missing approved origin"))?
    {
        return Err(error("Goal origin changed"));
    }
    let push_urls = git(anchor, &["remote", "get-url", "--push", "--all", "origin"])?;
    if push_urls != goal.execution.origin.as_deref().unwrap_or_default() {
        return Err(error(
            "Goal origin push destination differs from approved origin",
        ));
    }
    Ok(anchor.clone())
}
fn save(
    cwd: &Path,
    id: &str,
    publication: Publication,
    status: GoalStatus,
    reason: Option<String>,
) -> Result<()> {
    manager::update_goal(cwd, id, |goal| {
        if !matches!(
            goal.status,
            GoalStatus::Active | GoalStatus::Verifying | GoalStatus::AwaitingMerge
        ) {
            return Err(error("Goal state changed while publication was running"));
        }
        if goal.execution.integrated_tip.as_deref() != Some(publication.tip.as_str()) {
            return Err(error("Goal tip changed during publication"));
        }
        goal.execution.publication = Some(publication);
        goal.status = status;
        goal.reason = reason;
        Ok(())
    })
}
fn block(cwd: &Path, id: &str, p: Publication, message: String) -> Result<()> {
    save(cwd, id, p, GoalStatus::Blocked, Some(message))
}
fn validate(repo: &Path, goal: &Goal, p: &Publication) -> Result<()> {
    let parent = crate::paths::data_dir()?
        .join("manager")
        .join("verification");
    let expected_parent = parent.canonicalize()?;
    let worktree = p.worktree.canonicalize()?;
    if worktree.parent() != Some(expected_parent.as_path())
        || worktree == repo.canonicalize()?
        || PathBuf::from(git(&worktree, &["rev-parse", "--show-toplevel"])?).canonicalize()?
            != worktree
    {
        return Err(error("verification worktree identity changed"));
    }
    crate::manager_git::validate_verification_state(
        repo,
        &p.worktree,
        &format!("refs/heads/cruise/goal/{}", goal.id),
        &goal.proposal.target_sha,
        &p.tip,
    )
    .map_err(error)
}
fn matches_request(pr: &PullRequest, repo: &str, goal: &Goal, p: &Publication) -> bool {
    pr.repository == repo
        && pr.source == p.branch
        && pr.target == goal.proposal.target_branch
        && pr.head_sha == p.tip
}

/// A corrective preview is meaningful only after a definite failed approved
/// check, before any push or PR creation. Interrupted or dirty checks require
/// recovery instead of treating their outcome as a task specification.
pub(crate) fn corrective_tip(goal: &Goal) -> Result<&str> {
    if goal.status != GoalStatus::Blocked {
        return Err(error("Goal is not blocked by a failed verification"));
    }
    crate::manager_coordinator::validate_execution_contract(goal)?;
    if goal.proposal.tasks.iter().any(|task| {
        !goal.execution.attempts.iter().any(|attempt| {
            attempt.task_id == task.id && attempt.integrated && attempt.result_sha.is_some()
        })
    }) {
        return Err(error("Goal has tasks that are not integrated"));
    }
    let p = goal
        .execution
        .publication
        .as_ref()
        .ok_or_else(|| error("Goal has no verification attempt"))?;
    if p.phase != Phase::Ready
        || p.pending_check.is_some()
        || goal.execution.integrated_tip.as_deref() != Some(p.tip.as_str())
    {
        return Err(error(
            "verification state is not eligible for corrective review",
        ));
    }
    if p.checks.len() > goal.proposal.checks.len()
        || p.checks
            .iter()
            .take(p.checks.len().saturating_sub(1))
            .zip(&goal.proposal.checks)
            .any(|(e, c)| {
                !e.passed() || e.check_id != c.id || e.argv != c.argv || e.verified_tip != p.tip
            })
    {
        return Err(error(
            "verification evidence does not match approved checks",
        ));
    }
    let Some((evidence, check)) = p
        .checks
        .last()
        .zip(goal.proposal.checks.get(p.checks.len().saturating_sub(1)))
    else {
        return Err(error("no failed approved check evidence"));
    };
    if evidence.check_id != check.id
        || evidence.argv != check.argv
        || evidence.verified_tip != p.tip
        || evidence.cancelled
        || evidence.state_error.is_some()
        || !matches!(evidence.exit_code, Some(code) if code != 0)
    {
        return Err(error("no definite failed approved check evidence"));
    }
    Ok(p.tip.as_str())
}

pub(crate) fn validate_verification_retry(goal: &Goal) -> Result<()> {
    if goal.status != GoalStatus::Blocked {
        return Err(error("Goal is not blocked"));
    }
    crate::manager_coordinator::validate_execution_contract(goal)?;
    if goal.proposal.tasks.iter().any(|task| {
        !goal.execution.attempts.iter().any(|attempt| {
            attempt.task_id == task.id && attempt.integrated && attempt.result_sha.is_some()
        })
    }) {
        return Err(error("Goal has tasks that are not integrated"));
    }
    let publication = goal
        .execution
        .publication
        .as_ref()
        .ok_or_else(|| error("Goal has no verification attempt"))?;
    if goal.execution.integrated_tip.as_deref() != Some(publication.tip.as_str()) {
        return Err(error(
            "verification tip differs from the integrated Goal tip",
        ));
    }
    if !matches!(
        publication.phase,
        Phase::WorktreeIntent | Phase::Ready | Phase::CheckIntent
    ) {
        return Err(error(
            "publication may have occurred; resolve its state instead of retrying verification",
        ));
    }
    Ok(())
}

/// Start a fresh verification only on an explicit user request. Never use this
/// to retry an uncertain push or PR create. Prior evidence and logs remain
/// attached to the Goal and are not overwritten.
pub(crate) fn retry_verification(cwd: &Path, id: &str) -> Result<()> {
    manager::update_goal(cwd, id, |goal| {
        validate_verification_retry(goal)?;
        let publication = goal
            .execution
            .publication
            .take()
            .ok_or_else(|| error("verification attempt disappeared"))?;
        goal.execution.publication_history.push(publication);
        if let Some(preview) = goal.execution.corrective_preview.take() {
            goal.execution.corrective_discarded.push(preview);
        }
        goal.status = GoalStatus::Active;
        goal.reason = None;
        Ok(())
    })
}

/// Reconcile one Goal on a blocking worker. Explicit user action is required to
/// retry blocked verification or ambiguous publication; neither is auto-replayed.
pub(crate) fn reconcile(
    cwd: &Path,
    id: &str,
    lease: &crate::manager_coordinator::PublicationLease,
) -> Result<()> {
    if !lease.owns(cwd, id)? {
        return Err(error("publication lease belongs to a different Goal"));
    }
    let cancel = &lease.token;
    match reconcile_inner(cwd, id, cancel) {
        Ok(()) => Ok(()),
        Err(cause) => {
            let message = cause.to_string();
            manager::update_goal(cwd, id, |goal| {
                if matches!(
                    goal.status,
                    GoalStatus::Active | GoalStatus::Verifying | GoalStatus::AwaitingMerge
                ) {
                    goal.status = GoalStatus::Blocked;
                    goal.reason = Some(format!(
                        "Verification or publication requires attention: {message}"
                    ));
                }
                Ok(())
            })?;
            Err(cause)
        }
    }
}

struct Reconciliation<'a> {
    cwd: &'a Path,
    id: &'a str,
    cancel: &'a crate::cancellation::CancellationToken,
    goal: Goal,
    repo: PathBuf,
    tip: String,
    reference: String,
    ctx: crate::forge::ForgeContext,
}

fn prepare_reconciliation<'a>(
    cwd: &'a Path,
    id: &'a str,
    cancel: &'a crate::cancellation::CancellationToken,
) -> Result<Option<(Reconciliation<'a>, Publication)>> {
    let goal = load(cwd, id)?;
    if !matches!(
        goal.status,
        GoalStatus::Active | GoalStatus::Verifying | GoalStatus::AwaitingMerge
    ) {
        return Ok(None);
    }
    if cancel.is_cancelled() {
        return Ok(None);
    }
    crate::manager_coordinator::validate_execution_contract(&goal)?;
    if !goal.proposal.tasks.iter().all(|t| {
        goal.execution
            .attempts
            .iter()
            .any(|a| a.task_id == t.id && a.integrated)
    }) {
        return Ok(None);
    }
    let repo = anchor(cwd, &goal)?;
    let tip = goal
        .execution
        .integrated_tip
        .clone()
        .ok_or_else(|| error("missing integrated Goal tip"))?;
    let reference = format!("refs/heads/cruise/goal/{id}");
    let ctx = crate::forge::resolve_remote(
        goal.execution
            .origin
            .as_deref()
            .ok_or_else(|| error("missing origin"))?,
        crate::forge::forge_override_from_env()?,
    )?;
    let contract = goal
        .proposal
        .contract
        .as_ref()
        .ok_or_else(|| error("missing approved forge identity"))?;
    if ctx.kind != contract.forge_kind
        || ctx.host != contract.forge_host
        || ctx.repository != contract.forge_repository
        || goal.execution.anchor.as_ref() != Some(&contract.anchor)
        || goal.execution.origin.as_deref() != Some(contract.origin.as_str())
    {
        return Err(error(
            "forge or checkout identity differs from approved proposal",
        ));
    }
    let context = Reconciliation {
        cwd,
        id,
        cancel,
        goal,
        repo,
        tip,
        reference,
        ctx,
    };
    let Some(p) = prepare_worktree(&context)? else {
        return Ok(None);
    };
    Ok(Some((context, p)))
}

fn prepare_worktree(context: &Reconciliation<'_>) -> Result<Option<Publication>> {
    let Reconciliation {
        cwd,
        id,
        cancel,
        goal,
        repo,
        tip,
        ctx,
        ..
    } = context;
    let (cwd, id, cancel) = (*cwd, *id, *cancel);
    let p = if let Some(p) = goal.execution.publication.clone() {
        p
    } else {
        if goal.status != GoalStatus::Active {
            return Err(error("missing publication intent"));
        }
        let root = crate::paths::data_dir()?
            .join("manager")
            .join("verification");
        std::fs::create_dir_all(&root)?;
        let p = Publication {
            tip: tip.clone(),
            worktree: root.join(uuid::Uuid::new_v4().to_string()),
            branch: format!("cruise/goal/{id}"),
            phase: Phase::WorktreeIntent,
            checks: Vec::new(),
            pending_check: None,
            request: None,
            forge_kind: ctx.kind,
            forge_host: ctx.host.clone(),
            forge_repository: ctx.repository.clone(),
        };
        save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
        // Worktree creation is recoverable only by inspecting this exact path.
        if let Err(e) = manager_forge::run_effect(
            repo,
            "git",
            &[
                "worktree".into(),
                "add".into(),
                "--detach".into(),
                p.worktree
                    .to_str()
                    .ok_or_else(|| error("invalid worktree path"))?
                    .into(),
                tip.clone(),
            ],
            cancel,
            id,
        ) {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            block(
                cwd,
                id,
                p,
                format!("Cannot create verification worktree: {e}"),
            )?;
            return Ok(None);
        }
        let mut p = p;
        if let Err(e) = validate(repo, goal, &p) {
            block(cwd, id, p, e.to_string())?;
            return Ok(None);
        }
        p.phase = Phase::Ready;
        save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
        p
    };
    Ok(Some(p))
}

fn validate_publication_intent(
    context: &Reconciliation<'_>,
    p: Publication,
) -> Result<Option<Publication>> {
    let Reconciliation {
        cwd,
        id,
        goal,
        tip,
        ctx,
        ..
    } = context;
    let (cwd, id) = (*cwd, *id);
    if p.tip != *tip
        || p.branch != format!("cruise/goal/{id}")
        || p.forge_kind != ctx.kind
        || p.forge_host != ctx.host
        || p.forge_repository != ctx.repository
    {
        return Err(error(
            "publication intent or forge identity does not match Goal",
        ));
    }
    if p.checks.len() > goal.proposal.checks.len()
        || p.checks.iter().zip(&goal.proposal.checks).any(|(e, c)| {
            !e.passed() || e.check_id != c.id || e.argv != c.argv || e.verified_tip != *tip
        })
        || (p.phase != Phase::CheckIntent && p.pending_check.is_some())
        || (p.phase == Phase::Published) != p.request.is_some()
    {
        return Err(error(
            "saved verification or publication evidence is inconsistent",
        ));
    }
    if matches!(p.phase, Phase::WorktreeIntent | Phase::CheckIntent) {
        block(
            cwd,
            id,
            p,
            "Verification was interrupted. Inspect its worktree and explicitly retry verification"
                .into(),
        )?;
        return Ok(None);
    }
    Ok(Some(p))
}

fn run_approved_checks(
    context: &Reconciliation<'_>,
    mut p: Publication,
) -> Result<Option<Publication>> {
    let Reconciliation {
        cwd,
        id,
        cancel,
        goal,
        repo,
        tip,
        reference,
        ..
    } = context;
    let (cwd, id, cancel) = (*cwd, *id, *cancel);
    if p.phase == Phase::Ready {
        for (index, check) in goal.proposal.checks.iter().enumerate().skip(p.checks.len()) {
            if cancel.is_cancelled() {
                block(
                    cwd,
                    id,
                    p,
                    "Verification cancelled. Explicitly retry verification".into(),
                )?;
                return Ok(None);
            }
            let latest = load(cwd, id)?;
            if latest.status != GoalStatus::Verifying {
                return Ok(None);
            }
            let log_dir = p
                .worktree
                .parent()
                .ok_or_else(|| error("invalid verification directory"))?;
            let log_id = uuid::Uuid::new_v4();
            let stdout = log_dir.join(format!("{log_id}-{index}.stdout"));
            let stderr = log_dir.join(format!("{log_id}-{index}.stderr"));
            p.phase = Phase::CheckIntent;
            p.pending_check = Some(PendingCheck {
                check_id: check.id.clone(),
                argv: check.argv.clone(),
                stdout_log: stdout.clone(),
                stderr_log: stderr.clone(),
            });
            save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
            let result = crate::manager_git::run_verification_check_guarded(
                crate::manager_git::VerificationCheck {
                    repo,
                    worktree: &p.worktree,
                    goal_ref: reference,
                    approved_base: &goal.proposal.target_sha,
                    verified_tip: tip,
                    check_id: &check.id,
                    argv: &check.argv,
                    stdout_log: &stdout,
                    stderr_log: &stderr,
                    cancel,
                },
                id,
            );
            match result {
                Ok(evidence) => {
                    let passed = evidence.passed();
                    p.checks.push(evidence);
                    p.pending_check = None;
                    p.phase = Phase::Ready;
                    if !passed {
                        block(
                            cwd,
                            id,
                            p,
                            format!("Verification check {} failed or was interrupted", check.id),
                        )?;
                        return Ok(None);
                    }
                    save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
                }
                Err(e) => {
                    block(
                        cwd,
                        id,
                        p,
                        format!("Verification check {} could not complete: {e}", check.id),
                    )?;
                    return Ok(None);
                }
            }
        }
        validate(repo, goal, &p)?;
        p.phase = Phase::Verified;
        save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
    }
    if p.checks.len() != goal.proposal.checks.len()
        || p.checks.iter().zip(&goal.proposal.checks).any(|(e, c)| {
            !e.passed() || e.check_id != c.id || e.argv != c.argv || e.verified_tip != *tip
        })
    {
        return Err(error(
            "verification evidence does not match approved checks",
        ));
    }
    Ok(Some(p))
}

fn reconcile_push(context: &Reconciliation<'_>, mut p: Publication) -> Result<Option<Publication>> {
    let Reconciliation {
        cwd,
        id,
        cancel,
        goal,
        repo,
        ctx,
        ..
    } = context;
    let (cwd, id, cancel) = (*cwd, *id, *cancel);
    if p.phase == Phase::Published {
        let pr = p
            .request
            .as_ref()
            .ok_or_else(|| error("missing saved PR identity"))?;
        if !matches_request(pr, &ctx.repository, goal, &p) {
            return Err(error("saved PR identity does not match the verified Goal"));
        }
        match manager_forge::observe(repo, ctx, pr, cancel) {
            Ok(PullRequestState::Merged) => save(cwd, id, p, GoalStatus::Completed, None)?,
            Ok(PullRequestState::Open) => {}
            Ok(PullRequestState::Closed) => block(
                cwd,
                id,
                p,
                "PR closed without merge. Human resolution required".into(),
            )?,
            Err(_) if cancel.is_cancelled() => return Ok(None),
            Err(e) => block(
                cwd,
                id,
                p,
                format!("Cannot confirm PR identity and state: {e}"),
            )?,
        }
        return Ok(None);
    }
    if cancel.is_cancelled() || load(cwd, id)?.status != GoalStatus::Verifying {
        return Ok(None);
    }
    validate(repo, goal, &p)?;
    if p.phase == Phase::PushIntent {
        // Do not repeat an uncertain push. Confirm the remote ref instead.
        let remote = match manager_forge::remote_head(repo, &p.branch, cancel) {
            Ok(remote) => remote,
            Err(_) if cancel.is_cancelled() => return Ok(None),
            Err(e) => return Err(error(format!("cannot reconcile remote branch: {e}"))),
        };
        if remote != format!("{}\trefs/heads/{}", p.tip, p.branch) {
            block(
                cwd,
                id,
                p,
                "Push outcome uncertain or remote head differs. Human resolution required".into(),
            )?;
            return Ok(None);
        }
        p.phase = Phase::Pushed;
        save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
    }
    if p.phase == Phase::Verified {
        match manager_forge::lookup(repo, ctx, &p.branch, &goal.proposal.target_branch, cancel) {
            Ok(None) => {}
            Ok(Some(_)) => {
                block(cwd, id, p, "A PR already exists for the Goal branch before push. Human resolution required".into())?;
                return Ok(None);
            }
            Err(_) if cancel.is_cancelled() => return Ok(None),
            Err(e) => {
                block(
                    cwd,
                    id,
                    p,
                    format!("Cannot establish PR absence before push: {e}"),
                )?;
                return Ok(None);
            }
        }
        p.phase = Phase::PushIntent;
        save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
        if let Err(e) = manager_forge::push(repo, &p.tip, &p.branch, cancel, id) {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            block(cwd, id, p, format!("Push outcome uncertain: {e}"))?;
            return Ok(None);
        }
        p.phase = Phase::Pushed;
        save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
    }
    if cancel.is_cancelled() || load(cwd, id)?.status != GoalStatus::Verifying {
        return Ok(None);
    }
    Ok(Some(p))
}

fn finish_publication(context: &Reconciliation<'_>, mut p: Publication) -> Result<()> {
    let Reconciliation {
        cwd,
        id,
        cancel,
        goal,
        repo,
        ctx,
        ..
    } = context;
    let (cwd, id, cancel) = (*cwd, *id, *cancel);
    validate(repo, goal, &p)?;
    let existing =
        match manager_forge::lookup(repo, ctx, &p.branch, &goal.proposal.target_branch, cancel) {
            Ok(pr) => pr,
            Err(_) if cancel.is_cancelled() => return Ok(()),
            Err(e) => {
                block(
                    cwd,
                    id,
                    p,
                    format!("Cannot establish PR identity or absence: {e}"),
                )?;
                return Ok(());
            }
        };
    let pr = if let Some(pr) = existing {
        pr
    } else {
        if p.phase == Phase::CreateIntent {
            block(
                cwd,
                id,
                p,
                "PR create outcome uncertain, and no PR is observable. Human resolution required"
                    .into(),
            )?;
            return Ok(());
        }
        if p.phase != Phase::Pushed {
            return Err(error("invalid publication phase"));
        }
        p.phase = Phase::CreateIntent;
        save(cwd, id, p.clone(), GoalStatus::Verifying, None)?;
        let create_result = manager_forge::create(
            repo,
            ctx,
            manager_forge::CreateRequest {
                source: &p.branch,
                target: &goal.proposal.target_branch,
                title: &goal.proposal.objective,
                body: "Created by Cruise Manager. Merge manually after review.",
            },
            cancel,
            id,
        );
        if cancel.is_cancelled() {
            return Ok(());
        }
        match manager_forge::lookup(repo, ctx, &p.branch, &goal.proposal.target_branch, cancel) {
            Ok(Some(pr)) => pr,
            _ if cancel.is_cancelled() => return Ok(()),
            other => {
                block(
                    cwd,
                    id,
                    p,
                    format!(
                        "PR create outcome uncertain ({create_result:?}, lookup {other:?}). Human resolution required"
                    ),
                )?;
                return Ok(());
            }
        }
    };
    if !matches_request(&pr, &ctx.repository, goal, &p) {
        block(
            cwd,
            id,
            p,
            "Existing PR identity or head does not match verified Goal".into(),
        )?;
        return Ok(());
    }
    let state = pr.state;
    p.request = Some(pr);
    p.phase = Phase::Published;
    match state {
        PullRequestState::Open => save(cwd, id, p, GoalStatus::AwaitingMerge, None),
        PullRequestState::Merged => save(cwd, id, p, GoalStatus::Completed, None),
        PullRequestState::Closed => block(
            cwd,
            id,
            p,
            "PR closed without merge. Human resolution required".into(),
        ),
    }
}

fn reconcile_inner(
    cwd: &Path,
    id: &str,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<()> {
    let Some((context, p)) = prepare_reconciliation(cwd, id, cancel)? else {
        return Ok(());
    };
    let Some(p) = validate_publication_intent(&context, p)? else {
        return Ok(());
    };
    let Some(p) = run_approved_checks(&context, p)? else {
        return Ok(());
    };
    let Some(p) = reconcile_push(&context, p)? else {
        return Ok(());
    };
    finish_publication(&context, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_intent_and_check_evidence_must_match_goal()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let goal: Goal = serde_json::from_value(serde_json::json!({
            "id": "g", "status": "verifying", "reason": null,
            "proposal": {
                "id": "p", "revision": 1, "objective": "goal",
                "target_branch": "main", "target_sha": "base", "criteria": [],
                "checks": [], "tasks": [], "workflow": "builtin:tdd"
            }
        }))?;
        let cancel = crate::cancellation::CancellationToken::new();
        let context = Reconciliation {
            cwd: Path::new("."),
            id: "g",
            cancel: &cancel,
            goal,
            repo: PathBuf::from("."),
            tip: "tip".into(),
            reference: "refs/heads/cruise/goal/g".into(),
            ctx: crate::forge::ForgeContext {
                kind: crate::forge::ForgeKind::GitHub,
                host: "github.com".into(),
                repository: "example/project".into(),
            },
        };
        let publication = Publication {
            tip: "tip".into(),
            worktree: PathBuf::from("verification"),
            branch: "cruise/goal/g".into(),
            phase: Phase::Verified,
            checks: vec![],
            pending_check: None,
            request: None,
            forge_kind: crate::forge::ForgeKind::GitHub,
            forge_host: "github.com".into(),
            forge_repository: "example/project".into(),
        };
        let validated = validate_publication_intent(&context, publication.clone())?
            .ok_or("valid intent unexpectedly stopped")?;
        assert!(run_approved_checks(&context, validated)?.is_some());
        let mut wrong_tip = publication.clone();
        wrong_tip.tip = "other".into();
        assert!(validate_publication_intent(&context, wrong_tip).is_err());
        let mut unexpected_pending = publication;
        unexpected_pending.pending_check = Some(PendingCheck {
            check_id: "check".into(),
            argv: vec!["true".into()],
            stdout_log: PathBuf::from("stdout"),
            stderr_log: PathBuf::from("stderr"),
        });
        assert!(validate_publication_intent(&context, unexpected_pending).is_err());
        Ok(())
    }
}
