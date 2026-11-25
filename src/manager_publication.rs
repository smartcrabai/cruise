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
    /// Approved contract amendments the verification and its PR evidence refer to.
    #[serde(default, skip_serializing_if = "manager::is_zero")]
    contract_revision: u64,
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
    status: &GoalStatus,
) -> Result<std::sync::MutexGuard<'static, ()>> {
    let guard = crate::manager_coordinator::CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cancel.is_cancelled() || load(cwd, id)?.status != *status {
        return Err(error(
            "Goal is paused, stopped, or no longer in the state this effect was started for",
        ));
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
        if goal.contract_revision() != publication.contract_revision {
            return Err(error("Goal contract changed during publication"));
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

/// PR description: the amendments and evidence that the verified contract consists of.
/// A Goal without either keeps the plain description.
fn publication_body(goal: &Goal) -> String {
    use std::fmt::Write as _;
    let quote = |body: &mut String, label: &str, text: &str| {
        let _ = writeln!(body, "  - {label}:");
        for line in text.lines() {
            let _ = writeln!(body, "    > {line}");
        }
    };
    let mut body = String::from("Created by Cruise Manager. Merge manually after review.");
    let mut amendments = 0;
    for approved in &goal.execution.corrective_approvals {
        let manager::Correction::Amendment(edits) = &approved.correction else {
            continue;
        };
        if amendments == 0 {
            let _ = writeln!(
                body,
                "\n\n## Approved contract amendments\n\nProposal `{}` revision {} is unchanged. Children reserved after each approval and this verification use the amended text.",
                goal.proposal.id, goal.proposal.revision
            );
        }
        amendments += 1;
        for edit in edits {
            let (kind, id) = edit.target.label();
            let _ = writeln!(
                body,
                "\n- Amendment {amendments} (corrective revision {}), {kind} `{id}`, previous text sha256 `{}`",
                approved.revision, edit.old_sha256
            );
            quote(&mut body, "replaced", &edit.replace);
            quote(&mut body, "with", &edit.with);
        }
    }
    let mut resolved = false;
    for run in &goal.execution.evidence_runs {
        let Some((approved, manager::Correction::Evidence(request))) = goal
            .approved(run.revision)
            .filter(|_| run.outcome == Some(manager::EvidenceOutcome::Resolved))
            .map(|approved| (approved, &approved.correction))
        else {
            continue;
        };
        if !resolved {
            body.push_str("\n\n## Tasks resolved by evidence\n\nNo child commit was integrated for these tasks. Every approved check of the listed criteria passed at the recorded tip, and the full verification ran again at this head.\n");
        }
        resolved = true;
        let _ = write!(
            body,
            "\n- `{}` at `{}` (corrective revision {}): criteria {}; checks {}; reviewed",
            run.task_id,
            approved.integrated_tip,
            run.revision,
            request.criteria.join(", "),
            request.checks.join(", ")
        );
        for blob in &request.blobs {
            let _ = write!(body, " `{}` blob `{}`", blob.path, blob.blob);
        }
        body.push('\n');
    }
    body
}

/// Every task has a pinned integrated result or was resolved by evidence.
fn every_task_done(goal: &Goal) -> bool {
    goal.proposal.tasks.iter().all(|task| {
        goal.task_resolved(&task.id)
            || goal.execution.attempts.iter().any(|attempt| {
                attempt.task_id == task.id && attempt.integrated && attempt.result_sha.is_some()
            })
    })
}

/// A contract amendment must not change what a started verification or publication was
/// run against. A Goal blocked before any push starts a fresh verification afterwards.
pub(crate) fn validate_amendable(goal: &Goal) -> Result<()> {
    let phase = goal.execution.publication.as_ref().map(|p| p.phase);
    let amendable = match goal.status {
        GoalStatus::Active => phase.is_none(),
        GoalStatus::Blocked => matches!(
            phase,
            None | Some(Phase::WorktreeIntent | Phase::Ready | Phase::CheckIntent)
        ),
        _ => false,
    };
    if amendable {
        Ok(())
    } else {
        Err(error(
            "Goal verification or publication is in progress; its contract cannot be amended",
        ))
    }
}

/// A corrective preview is meaningful only after a definite failed approved
/// check, before any push or PR creation. Interrupted or dirty checks require
/// recovery instead of treating their outcome as a task specification.
pub(crate) fn corrective_tip(goal: &Goal) -> Result<&str> {
    if goal.status != GoalStatus::Blocked {
        return Err(error("Goal is not blocked by a failed verification"));
    }
    crate::manager_coordinator::validate_execution_contract(goal)?;
    if !every_task_done(goal) {
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
    if !every_task_done(goal) {
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

fn approved_evidence(
    goal: &Goal,
    revision: u64,
) -> Result<(&manager::CorrectivePreview, &manager::EvidenceRequest)> {
    match goal.approved(revision).map(|a| (a, &a.correction)) {
        Some((approved, manager::Correction::Evidence(request))) => Ok((approved, request)),
        _ => Err(error("evidence run has no approved evidence request")),
    }
}

fn update_evidence(
    cwd: &Path,
    id: &str,
    revision: u64,
    change: impl FnOnce(&mut manager::EvidenceRun),
) -> Result<()> {
    manager::update_goal(cwd, id, |goal| {
        let run = goal
            .execution
            .evidence_runs
            .iter_mut()
            .find(|run| run.revision == revision && run.outcome.is_none())
            .ok_or_else(|| error("evidence run changed"))?;
        change(run);
        Ok(())
    })
}

/// Run every approved check of one evidence request in a detached worktree at the
/// approved tip. The durable `started` mark comes first, so a run found started again
/// has an unknown outcome and is reported instead of replayed.
fn gather_evidence(
    cwd: &Path,
    cancel: &crate::cancellation::CancellationToken,
    goal: &Goal,
    run: &manager::EvidenceRun,
) -> Result<manager::EvidenceOutcome> {
    use manager::EvidenceOutcome::{Resolved, Unresolved};
    let id = goal.id.as_str();
    if run.started {
        return Ok(Unresolved(
            "evidence run was interrupted before its outcome was saved".into(),
        ));
    }
    let (approved, request) = approved_evidence(goal, run.revision)?;
    let tip = approved.integrated_tip.as_str();
    update_evidence(cwd, id, run.revision, |run| run.started = true)?;
    let repo = anchor(cwd, goal)?;
    let root = run
        .worktree
        .parent()
        .ok_or_else(|| error("invalid verification directory"))?;
    std::fs::create_dir_all(root)?;
    manager_forge::run_effect(
        &repo,
        "git",
        &[
            "worktree".into(),
            "add".into(),
            "--detach".into(),
            run.worktree
                .to_str()
                .ok_or_else(|| error("invalid worktree path"))?
                .into(),
            tip.into(),
        ],
        cancel,
        id,
        &GoalStatus::Active,
    )
    .map_err(error)?;
    let reference = format!("refs/heads/cruise/goal/{id}");
    for (index, check_id) in request.checks.iter().enumerate() {
        let check = goal
            .proposal
            .checks
            .iter()
            .find(|check| check.id == *check_id)
            .ok_or_else(|| error("approved evidence check is missing"))?;
        let log_id = uuid::Uuid::new_v4();
        let evidence = crate::manager_git::run_evidence_check(
            crate::manager_git::VerificationCheck {
                repo: &repo,
                worktree: &run.worktree,
                goal_ref: &reference,
                approved_base: &goal.proposal.target_sha,
                verified_tip: tip,
                check_id,
                argv: &check.argv,
                stdout_log: &root.join(format!("{log_id}-{index}.stdout")),
                stderr_log: &root.join(format!("{log_id}-{index}.stderr")),
                cancel,
            },
            id,
        )
        .map_err(error)?;
        let passed = evidence.passed();
        update_evidence(cwd, id, run.revision, |run| run.checks.push(evidence))?;
        if !passed {
            return Ok(Unresolved(format!(
                "check {check_id} failed or was interrupted"
            )));
        }
    }
    Ok(Resolved)
}

/// Evidence is adopted only for exactly what the owner approved: the same tip, contract
/// and failed attempt, with a passing result of every approved check at that tip.
fn evidence_holds(goal: &Goal, revision: u64) -> Result<()> {
    let (approved, request) = approved_evidence(goal, revision)?;
    let tip = approved.integrated_tip.as_str();
    let attempt_unchanged = goal
        .execution
        .attempts
        .iter()
        .filter(|attempt| attempt.task_id == request.task_id)
        .max_by_key(|attempt| attempt.attempt)
        .is_some_and(|attempt| {
            attempt.attempt == request.attempt
                && attempt.session_id == request.session_id
                && !attempt.integrated
                && attempt.result_sha.is_none()
                && attempt.finalization_intent.is_none()
                && attempt.integration_intent.is_none()
                && !attempt.resume_pending
                && !attempt.resume_starting
        });
    let checks_passed = goal
        .execution
        .evidence_runs
        .iter()
        .find(|run| run.revision == revision)
        .is_some_and(|run| {
            run.checks.len() == request.checks.len()
                && run
                    .checks
                    .iter()
                    .zip(&request.checks)
                    .all(|(evidence, id)| {
                        evidence.passed()
                            && evidence.check_id == *id
                            && evidence.verified_tip == tip
                            && goal
                                .proposal
                                .checks
                                .iter()
                                .any(|check| check.id == *id && check.argv == evidence.argv)
                    })
        });
    if goal.status != GoalStatus::Active
        || goal.execution.integrated_tip.as_deref() != Some(tip)
        || goal.contract_revision() != approved.contract_revision
        || !attempt_unchanged
        || !checks_passed
    {
        return Err(error(
            "the Goal tip, contract or attempt changed, or a check result is missing",
        ));
    }
    Ok(())
}

/// Run the pending approved evidence request of a Goal on a blocking worker. Its task is
/// resolved only when every approved check passes at the approved tip and nothing the
/// owner reviewed changed meanwhile. Any other end is recorded as unresolved and is never
/// replayed: the task stays failed until the owner approves a new preview.
pub(crate) fn run_evidence(
    cwd: &Path,
    id: &str,
    lease: &crate::manager_coordinator::PublicationLease,
) -> Result<()> {
    use manager::EvidenceOutcome::{Resolved, Unresolved};
    if !lease.owns(cwd, id)? {
        return Err(error("publication lease belongs to a different Goal"));
    }
    let goal = load(cwd, id)?;
    let Some(run) = goal
        .execution
        .evidence_runs
        .iter()
        .find(|run| run.outcome.is_none())
    else {
        return Ok(());
    };
    let revision = run.revision;
    let gathered = gather_evidence(cwd, &lease.token, &goal, run)
        .unwrap_or_else(|cause| Unresolved(cause.to_string()));
    // Integration and resume admission hold the same gate, so for them the comparison
    // and the resolution below are one step.
    let _guard = crate::manager_coordinator::CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    manager::update_goal(cwd, id, |saved| {
        let outcome = match (gathered, evidence_holds(saved, revision)) {
            (Resolved, Err(cause)) => Unresolved(format!("evidence was not adopted: {cause}")),
            (outcome, _) => outcome,
        };
        if outcome == Resolved {
            crate::manager_coordinator::clear_failed_child_reason(saved, &run.task_id);
        }
        saved
            .execution
            .evidence_runs
            .iter_mut()
            .find(|run| run.revision == revision && run.outcome.is_none())
            .ok_or_else(|| error("evidence run changed"))?
            .outcome = Some(outcome);
        Ok(())
    })?;
    crate::manager_coordinator::wake();
    Ok(())
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
    if !goal.proposal.tasks.iter().all(|t| goal.task_done(&t.id)) {
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
            contract_revision: goal.contract_revision(),
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
            &GoalStatus::Verifying,
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
        || p.contract_revision != goal.contract_revision()
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
                body: &publication_body(goal),
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
            contract_revision: 0,
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
