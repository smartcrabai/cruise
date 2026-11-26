//! Approved recovery of a verified GitHub Goal after a definite legacy title rejection.

use super::{
    Phase, Publication, Reconciliation, can_retry_title_rejection, effect_guard, error, git,
    legacy_title_rejection, load, reconciliation_context, validate, validate_checks_complete,
    validate_saved_intent,
};
use crate::cancellation::CancellationToken;
use crate::error::Result;
use crate::manager::{
    self, Correction, CorrectivePreview, Goal, GoalStatus, RetargetIntegration, RetargetPreview,
};
use crate::manager_coordinator::validate_execution_contract;
use crate::manager_forge::{self, PullRequest};
use crate::manager_git::{self, IntegrationObservation};
use std::path::Path;

fn digest(value: &impl serde::Serialize) -> Result<String> {
    Ok(manager::hex_digest(&serde_json::to_vec(
        &serde_json::to_value(value)?,
    )?))
}

fn contract_digest(goal: &Goal) -> Result<String> {
    let amendments: Vec<_> = goal
        .execution
        .corrective_approvals
        .iter()
        .filter(|preview| matches!(preview.correction, Correction::Amendment(_)))
        .collect();
    digest(&(&goal.proposal, amendments))
}

fn require_integrated_tasks(goal: &Goal) -> Result<()> {
    if goal.proposal.tasks.iter().any(|task| {
        !goal.execution.attempts.iter().any(|attempt| {
            attempt.task_id == task.id
                && attempt.integrated
                && attempt
                    .result_sha
                    .as_deref()
                    .is_some_and(|sha| !sha.is_empty())
        })
    }) || goal.execution.attempts.iter().any(|attempt| {
        attempt.resume_pending
            || attempt.resume_starting
            || (!attempt.integrated
                && (attempt.finalization_intent.is_some() || attempt.integration_intent.is_some()))
    }) || goal
        .execution
        .evidence_runs
        .iter()
        .any(|run| run.outcome.is_none())
    {
        return Err(error(
            "Retarget requires all tasks integrated and no unresolved work",
        ));
    }
    Ok(())
}

fn candidate(goal: &Goal) -> Result<&Publication> {
    if !can_retry_title_rejection(goal) || goal.execution.retarget.is_some() {
        return Err(error(
            "Goal has no eligible unpublished GitHub title rejection",
        ));
    }
    validate_execution_contract(goal)?;
    require_integrated_tasks(goal)?;
    let publication = goal
        .execution
        .publication
        .as_ref()
        .ok_or_else(|| error("missing publication"))?;
    validate_checks_complete(goal, publication)?;
    Ok(publication)
}

pub(crate) fn validate_candidate<'a>(
    goal: &'a Goal,
    reviewed: &RetargetPreview,
) -> Result<&'a Publication> {
    let p = candidate(goal)?;
    if goal.reason.as_deref() != Some(&reviewed.publication_rejection)
        || digest(p)? != reviewed.publication_digest
        || contract_digest(goal)? != reviewed.contract_digest
        || goal.proposal.revision != reviewed.proposal_revision
        || goal.effective_base() != reviewed.old_base
        || format!("refs/heads/{}", goal.proposal.target_branch) != reviewed.target_ref
        || forge_repository(p) != reviewed.repository
    {
        return Err(error(
            "Goal contract, base, or rejection changed since retarget preview",
        ));
    }
    Ok(p)
}

fn forge_repository(p: &Publication) -> String {
    format!("{}/{}", p.forge_host, p.forge_repository)
}

fn remote_sha(context: &Reconciliation<'_>, branch: &str) -> Result<String> {
    let line = manager_forge::remote_head(&context.repo, branch, context.cancel).map_err(error)?;
    let (sha, reference) = line
        .split_once('\t')
        .ok_or_else(|| error(format!("Cannot establish remote ref {branch}")))?;
    if reference != format!("refs/heads/{branch}")
        || !matches!(sha.len(), 40 | 64)
        || !sha.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(error(format!("Ambiguous remote ref {branch}")));
    }
    Ok(sha.to_owned())
}

fn require_remote_refs(
    context: &Reconciliation<'_>,
    p: &Publication,
    base: &str,
    tip: &str,
) -> Result<()> {
    for (branch, sha) in [
        (context.goal.proposal.target_branch.as_str(), base),
        (&p.branch, tip),
    ] {
        if remote_sha(context, branch)? != sha {
            return Err(error(format!(
                "Remote ref {branch} differs from the approved retarget"
            )));
        }
    }
    Ok(())
}

fn observe_pr(
    context: &Reconciliation<'_>,
    branch: &str,
    expected_user: Option<u64>,
) -> Result<(u64, Option<PullRequest>)> {
    let user =
        manager_forge::authenticated_github_user(&context.repo, &context.ctx, context.cancel)
            .map_err(error)?;
    if expected_user.is_some_and(|expected| expected != user) {
        return Err(error(
            "Authenticated GitHub identity changed since retarget preview",
        ));
    }
    let pr = manager_forge::lookup(&context.repo, &context.ctx, branch, None, context.cancel)
        .map_err(error)?;
    Ok((user, pr))
}

fn require_absent(pr: Option<&PullRequest>) -> Result<()> {
    if pr.is_some() {
        return Err(error(
            "A PR already exists for the Goal branch; retarget refused",
        ));
    }
    Ok(())
}

pub(crate) fn preview(
    cwd: &Path,
    id: &str,
    goal: Goal,
    expected_tip: &str,
    cancel: &CancellationToken,
) -> Result<CorrectivePreview> {
    let context = reconciliation_context(cwd, id, cancel, goal)?;
    let goal = &context.goal;
    let p = candidate(goal)?;
    if p.tip != expected_tip {
        return Err(error("Goal tip changed; refresh before retarget review"));
    }
    validate_saved_intent(&context, p)?;
    validate(&context.repo, goal, p)?;
    let (github_user_id, pr) = observe_pr(&context, &p.branch, None)?;
    require_absent(pr.as_ref())?;
    let old_base = goal.effective_base();
    let new_base = remote_sha(&context, &goal.proposal.target_branch)?;
    if new_base == old_base {
        return Err(error(
            "Remote target has not advanced; use Manager retry for the title rejection",
        ));
    }
    let target_ref = format!("refs/heads/{}", goal.proposal.target_branch);
    if git(&context.repo, &["check-ref-format", &target_ref]).is_err() {
        return Err(error("Invalid approved target ref"));
    }
    manager_forge::fetch_review_objects(&context.repo, &target_ref, cancel).map_err(error)?;
    require_remote_refs(&context, p, &new_base, expected_tip)?;
    let (merge_tree, conflict) =
        match manager_git::review_base_advance(&context.repo, old_base, &new_base, expected_tip)
            .map_err(error)?
        {
            Ok(tree) => (Some(tree), None),
            Err(conflict) => (None, Some(conflict)),
        };
    let commits = git(
        &context.repo,
        &[
            "log",
            "--no-notes",
            "--no-show-signature",
            "--no-decorate",
            "--format=%H %s",
            &format!("{old_base}..{new_base}"),
            "--",
        ],
    )?;
    let files = git(
        &context.repo,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            "--no-ext-diff",
            old_base,
            &new_base,
            "--",
        ],
    )?;
    let reviewed = RetargetPreview {
        repository: forge_repository(p),
        target_ref,
        old_base: old_base.into(),
        new_base,
        proposal_revision: goal.proposal.revision,
        publication_rejection: goal
            .reason
            .clone()
            .ok_or_else(|| error("missing rejection reason"))?,
        publication_digest: digest(p)?,
        contract_digest: contract_digest(goal)?,
        github_user_id,
        commits,
        files,
        merge_tree,
        conflict,
    };
    let _guard = effect_guard(cwd, &goal.id, cancel, &GoalStatus::Blocked)?;
    manager::preview_correction(
        cwd,
        &goal.id,
        expected_tip,
        goal.contract_revision(),
        Correction::Retarget(reviewed),
    )
}

pub(crate) fn approve(
    cwd: &Path,
    id: &str,
    goal: Goal,
    preview_id: &str,
    revision: u64,
    cancel: &CancellationToken,
) -> Result<Goal> {
    let context = reconciliation_context(cwd, id, cancel, goal)?;
    let goal = &context.goal;
    let preview = goal.reviewed_preview(preview_id, revision)?;
    let Correction::Retarget(reviewed) = &preview.correction else {
        return Err(error("Corrective preview is not a retarget"));
    };
    let tree = reviewed
        .merge_tree
        .as_deref()
        .filter(|_| reviewed.conflict.is_none())
        .ok_or_else(|| error("A conflicting retarget preview cannot be approved"))?;
    let p = validate_candidate(goal, reviewed)?;
    validate_saved_intent(&context, p)?;
    validate(&context.repo, goal, p)?;
    let (_, pr) = observe_pr(&context, &p.branch, Some(reviewed.github_user_id))?;
    require_absent(pr.as_ref())?;
    require_remote_refs(&context, p, &reviewed.new_base, &preview.integrated_tip)?;
    let intent = manager_git::prepare_base_advance(
        &context.repo,
        &context.reference,
        &preview.integrated_tip,
        &reviewed.old_base,
        &reviewed.new_base,
        tree,
    )
    .map_err(error)?;
    let _guard = effect_guard(cwd, &goal.id, cancel, &GoalStatus::Blocked)?;
    manager::approve_correction(
        cwd,
        &goal.id,
        &preview.id,
        preview.revision,
        |saved, latest_preview| {
            if !matches!(&latest_preview.correction, Correction::Retarget(latest) if latest == reviewed)
            {
                return Err(error("Retarget preview changed during approval"));
            }
            validate_candidate(saved, reviewed)?;
            saved.execution.retarget = Some(RetargetIntegration {
                revision: preview.revision,
                intent,
                applied: false,
            });
            saved.status = GoalStatus::Verifying;
            saved.reason = None;
            Ok(())
        },
    )
}

fn binding<'a>(
    context: &'a Reconciliation<'_>,
    run: &RetargetIntegration,
) -> Result<&'a RetargetPreview> {
    let goal = &context.goal;
    validate_execution_contract(goal)?;
    require_integrated_tasks(goal)?;
    let approved = goal
        .approved(run.revision)
        .ok_or_else(|| error("Missing retarget approval"))?;
    let Correction::Retarget(reviewed) = &approved.correction else {
        return Err(error("Retarget intent has no matching approval"));
    };
    let intent = &run.intent;
    let expected_tip = if run.applied {
        &intent.prepared_tip
    } else {
        &intent.expected_tip
    };
    if approved.goal_id != goal.id
        || approved.contract_revision != goal.contract_revision()
        || contract_digest(goal)? != reviewed.contract_digest
        || goal.proposal.revision != reviewed.proposal_revision
        || approved.integrated_tip != intent.expected_tip
        || intent.goal_ref != context.reference
        || intent.child_base != goal.proposal.target_sha
        || reviewed.old_base != intent.child_base
        || reviewed.new_base != intent.child_result
        || reviewed.old_base == reviewed.new_base
        || reviewed.target_ref != format!("refs/heads/{}", goal.proposal.target_branch)
        || reviewed.repository != format!("{}/{}", context.ctx.host, context.ctx.repository)
        || !legacy_title_rejection(&reviewed.publication_rejection)
        || reviewed.conflict.is_some()
        || intent.expected_tip == intent.prepared_tip
        || &context.tip != expected_tip
        || reviewed.merge_tree.as_deref()
            != Some(
                git(
                    &context.repo,
                    &[
                        "show",
                        "-s",
                        "--format=%T",
                        "--end-of-options",
                        &intent.prepared_tip,
                    ],
                )?
                .as_str(),
            )
    {
        return Err(error(
            "Retarget intent does not match the approved repository, base, tip, or contract",
        ));
    }
    Ok(reviewed)
}

pub(super) fn reconcile(
    cwd: &Path,
    id: &str,
    cancel: &CancellationToken,
    goal: Goal,
) -> Result<()> {
    let context = reconciliation_context(cwd, id, cancel, goal)?;
    let goal = &context.goal;
    let run = goal
        .execution
        .retarget
        .as_ref()
        .ok_or_else(|| error("Missing retarget intent"))?;
    let reviewed = binding(&context, run)?;
    let p = goal
        .execution
        .publication
        .as_ref()
        .ok_or_else(|| error("Missing rejected publication"))?;
    validate_saved_intent(&context, p)?;
    validate_checks_complete(goal, p)?;
    if p.phase != Phase::CreateIntent || digest(p)? != reviewed.publication_digest {
        return Err(error("Rejected publication differs from retarget approval"));
    }
    let observed = manager_git::observe_integration(&context.repo, &run.intent).map_err(error)?;
    if observed == IntegrationObservation::Pending {
        validate(&context.repo, goal, p)?;
    }
    let (_, pr) = observe_pr(&context, &p.branch, Some(reviewed.github_user_id))?;
    require_absent(pr.as_ref())?;
    require_remote_refs(&context, p, &reviewed.new_base, &p.tip)?;
    let _guard = effect_guard(cwd, id, cancel, &GoalStatus::Verifying)?;
    if observed == IntegrationObservation::Pending {
        manager_git::require_unchecked_ref(&context.repo, &run.intent.goal_ref).map_err(error)?;
        manager_git::apply_integration(&context.repo, &run.intent).map_err(error)?;
    }
    manager::update_goal(cwd, id, |latest| {
        let saved = latest
            .execution
            .retarget
            .as_mut()
            .filter(|saved| *saved == run)
            .ok_or_else(|| error("Saved retarget intent changed before adoption"))?;
        let old = latest
            .execution
            .publication
            .take()
            .ok_or_else(|| error("Missing rejected publication"))?;
        if digest(&old)? != reviewed.publication_digest {
            return Err(error("Rejected publication changed before adoption"));
        }
        saved.applied = true;
        latest.execution.integrated_tip = Some(run.intent.prepared_tip.clone());
        latest.execution.publication_history.push(old);
        Ok(())
    })
}

pub(super) fn lookup(
    context: &Reconciliation<'_>,
    p: &Publication,
    expected_head: Option<&str>,
) -> Result<Option<PullRequest>> {
    let latest = load(context.cwd, context.id)?;
    let fresh = reconciliation_context(context.cwd, context.id, context.cancel, latest)?;
    let goal = &fresh.goal;
    let run = goal
        .execution
        .retarget
        .as_ref()
        .filter(|run| run.applied)
        .ok_or_else(|| error("Missing applied retarget intent"))?;
    let reviewed = binding(&fresh, run)?;
    if goal.status != GoalStatus::Verifying
        || goal
            .execution
            .publication
            .as_ref()
            .map(digest)
            .transpose()?
            != Some(digest(p)?)
    {
        return Err(error(
            "Current publication changed after retarget verification",
        ));
    }
    validate_saved_intent(&fresh, p)?;
    validate_checks_complete(goal, p)?;
    let mut rejected = false;
    for old in &goal.execution.publication_history {
        if old.phase == Phase::CreateIntent
            && old.tip == run.intent.expected_tip
            && digest(old)? == reviewed.publication_digest
        {
            validate_checks_complete(goal, old)?;
            rejected = true;
            break;
        }
    }
    if !rejected {
        return Err(error(
            "Retarget is missing the reviewed rejected publication and evidence",
        ));
    }
    let (_, pr) = observe_pr(&fresh, &p.branch, Some(reviewed.github_user_id))?;
    if let Some(head) = expected_head {
        if manager_git::observe_integration(&fresh.repo, &run.intent).map_err(error)?
            != IntegrationObservation::Applied
        {
            return Err(error(
                "Approved retarget integration is no longer on the Goal ref",
            ));
        }
        validate(&fresh.repo, goal, p)?;
        require_remote_refs(&fresh, p, &reviewed.new_base, head)?;
    }
    Ok(pr)
}
