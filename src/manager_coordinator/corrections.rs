//! Owner-approved corrections to an approved Goal.
//!
//! A preview records exactly what the owner reviews. Approval recomputes those facts and
//! refuses anything that changed, so an approval never authorizes a different state.

use std::path::{Component, Path};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    CONTROL, Resume, admit_resume, git_output, owner, ready_tasks, resumable_attempt,
    validate_execution_contract,
};
use crate::application::CruiseApplication;
use crate::error::{CruiseError, Result};
use crate::manager::{
    self, AttemptCorrection, Correction, CorrectivePreview, EvidenceRequest, EvidenceRun, Goal,
    ReviewedBlob, TaskAttempt,
};
use crate::session::{SessionPhase, SessionState};

/// What a client asks to preview. The daemon derives every fact that is reviewed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CorrectionRequest {
    Task(manager::Task),
    Amendment(Vec<manager::ContractEdit>),
    Resume {
        task_id: String,
        instructions: String,
    },
    Evidence {
        task_id: String,
        criteria: Vec<String>,
        paths: Vec<String>,
    },
}

fn other(message: &str) -> CruiseError {
    CruiseError::Other(message.into())
}

fn find<'a>(snapshot: &'a manager::Snapshot, goal_id: &str) -> Result<&'a Goal> {
    snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| other("unknown Manager goal for this repository"))
}

fn has_duplicates(values: &[String]) -> bool {
    (1..values.len()).any(|index| values[..index].contains(&values[index]))
}

/// Save a correction for review. Nothing is authorized until the owner approves the
/// returned revision.
pub(crate) fn preview(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    expected_tip: &str,
    request: CorrectionRequest,
) -> Result<CorrectivePreview> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let snapshot = manager::snapshot(cwd)?;
    let goal = find(&snapshot, goal_id)?;
    let correction = match request {
        CorrectionRequest::Task(task) => {
            return manager::preview_corrective(cwd, goal_id, expected_tip, task);
        }
        CorrectionRequest::Amendment(edits) => {
            validate_execution_contract(goal)?;
            crate::manager_publication::validate_amendable(goal)?;
            manager::amendment(goal, edits)?
        }
        CorrectionRequest::Resume {
            task_id,
            instructions,
        } => {
            let instructions = instructions.trim();
            let limit = crate::instruction_inbox::MAX_INSTRUCTION_BYTES;
            if instructions.is_empty() || instructions.len() > limit {
                return Err(CruiseError::Other(format!(
                    "correction instructions must be between 1 and {limit} bytes"
                )));
            }
            let (attempt, child) =
                resumable_attempt(application, goal, &snapshot.repository, &task_id, false)?;
            Correction::Resume(reviewed_attempt(attempt, &child, instructions)?)
        }
        CorrectionRequest::Evidence {
            task_id,
            criteria,
            paths,
        } => Correction::Evidence(evidence_request(
            application,
            goal,
            &task_id,
            criteria,
            &paths,
        )?),
    };
    manager::preview_correction(
        cwd,
        goal_id,
        expected_tip,
        goal.contract_revision(),
        correction,
    )
}

/// Approve the reviewed preview and start what it authorizes, as one step for every other
/// Manager control operation. Approving the same preview again returns the Goal unchanged:
/// it never authorizes or launches anything twice.
pub(crate) fn approve(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    preview_id: &str,
    revision: u64,
) -> Result<Goal> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let snapshot = manager::snapshot(cwd)?;
    let goal = find(&snapshot, goal_id)?;
    if goal
        .execution
        .corrective_approvals
        .iter()
        .any(|approved| approved.id == preview_id && approved.revision == revision)
    {
        return Ok(goal.clone());
    }
    let preview = goal.reviewed_preview(preview_id, revision)?;
    match &preview.correction {
        Correction::Task(_) => manager::approve_corrective(cwd, goal_id, preview_id, revision),
        Correction::Resume(correction) => admit_resume(
            application,
            cwd,
            goal_id,
            &correction.task_id,
            &Resume::Corrected(preview),
        ),
        Correction::Amendment(_) => {
            manager::approve_correction(cwd, goal_id, preview_id, revision, |goal, _| {
                crate::manager_publication::validate_amendable(goal)
            })
        }
        Correction::Evidence(reviewed) => {
            let paths: Vec<String> = reviewed.blobs.iter().map(|b| b.path.clone()).collect();
            let current = evidence_request(
                application,
                goal,
                &reviewed.task_id,
                reviewed.criteria.clone(),
                &paths,
            )?;
            if current != *reviewed {
                return Err(CruiseError::Busy(
                    "the attempt, checks or reviewed files changed since the evidence preview"
                        .into(),
                ));
            }
            let worktree = crate::paths::data_dir()?
                .join("manager")
                .join("verification")
                .join(uuid::Uuid::new_v4().to_string());
            manager::approve_correction(cwd, goal_id, preview_id, revision, |goal, _| {
                goal.execution.evidence_runs.push(EvidenceRun {
                    revision,
                    task_id: current.task_id,
                    worktree,
                    started: false,
                    checks: Vec::new(),
                    outcome: None,
                });
                Ok(())
            })
        }
    }
}

pub(super) fn no_evidence_running(goal: &Goal, task_id: &str) -> Result<()> {
    if goal
        .execution
        .evidence_runs
        .iter()
        .any(|run| run.task_id == task_id && run.outcome.is_none())
    {
        return Err(other("an evidence run for this task is in progress"));
    }
    Ok(())
}

/// The facts the owner reviews about a failed attempt, read from its session and worktree.
fn reviewed_attempt(
    attempt: &TaskAttempt,
    child: &SessionState,
    instructions: &str,
) -> Result<AttemptCorrection> {
    let (SessionPhase::Failed(_), Some(step), Some(worktree)) =
        (&child.phase, &child.current_step, &child.worktree_path)
    else {
        return Err(other(
            "only a failed attempt with a saved resume position takes a correction",
        ));
    };
    Ok(AttemptCorrection {
        task_id: attempt.task_id.clone(),
        attempt: attempt.attempt,
        session_id: attempt.session_id.clone(),
        base_sha: attempt.base_sha.clone(),
        step: step.clone(),
        head: git_output(worktree, &["rev-parse", "HEAD"])?,
        state: worktree_state(worktree)?,
        instructions: instructions.to_owned(),
    })
}

/// SHA-256 over everything in a worktree that its HEAD commit does not contain: the
/// porcelain status and the content of every changed or untracked path. Read-only: even
/// the index refresh that `git status` would normally save is skipped.
fn worktree_state(worktree: &Path) -> Result<String> {
    let status = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(worktree)
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .output()?;
    if !status.status.success() {
        return Err(other("cannot inspect the child worktree"));
    }
    let mut hasher = Sha256::new();
    hasher.update(&status.stdout);
    // Each entry is `XY <path>`; a deleted path is already described by its status.
    for entry in status.stdout.split(|byte| *byte == 0) {
        let Some(path) = entry.get(3..).filter(|path| !path.is_empty()) else {
            continue;
        };
        let path = worktree.join(
            std::str::from_utf8(path).map_err(|_| other("child worktree path is not UTF-8"))?,
        );
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => hasher.update(std::fs::read(&path)?),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                hasher.update(std::fs::read_link(&path)?.to_string_lossy().as_bytes());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        hasher.update([0]);
    }
    Ok(manager::hex(&hasher.finalize()))
}

/// The Goal and the attempt are exactly what the owner reviewed for this correction.
pub(super) fn verify_reviewed_attempt(
    goal: &Goal,
    attempt: &TaskAttempt,
    child: &SessionState,
    preview: &CorrectivePreview,
) -> Result<()> {
    let Correction::Resume(reviewed) = &preview.correction else {
        return Err(other("approved correction is not for an attempt"));
    };
    if goal.execution.integrated_tip.as_deref() != Some(preview.integrated_tip.as_str())
        || goal.contract_revision() != preview.contract_revision
        || reviewed_attempt(attempt, child, &reviewed.instructions)? != *reviewed
    {
        return Err(CruiseError::Busy(
            "the Goal tip, contract or attempt worktree changed since the correction was reviewed"
                .into(),
        ));
    }
    Ok(())
}

/// Why the correction armed for this attempt must not be launched, if anything it was
/// approved for changed. An attempt without an armed correction has nothing to compare.
pub(super) fn launch_mismatch(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
    attempt_number: u32,
) -> Option<String> {
    let check = || -> Result<()> {
        let snapshot = manager::snapshot(cwd)?;
        let goal = find(&snapshot, goal_id)?;
        let Some(attempt) = goal
            .execution
            .attempts
            .iter()
            .find(|attempt| attempt.task_id == task_id && attempt.attempt == attempt_number)
        else {
            return Ok(());
        };
        let Some(revision) = attempt.correction_pending else {
            return Ok(());
        };
        let approved = goal
            .approved(revision)
            .ok_or_else(|| other("the armed correction was never approved"))?;
        let child = application.read_session(&attempt.session_id)?;
        verify_reviewed_attempt(goal, attempt, &child, approved)
    };
    check().err().map(|error| error.to_string())
}

/// Everything an evidence approval is bound to. The checks are every approved check of the
/// selected criteria: the requester chooses requirements, never a narrower set of checks.
fn evidence_request(
    application: &CruiseApplication,
    goal: &Goal,
    task_id: &str,
    criteria: Vec<String>,
    paths: &[String],
) -> Result<EvidenceRequest> {
    validate_execution_contract(goal)?;
    if goal.status != manager::GoalStatus::Active {
        return Err(other("Goal must be active to resolve a task by evidence"));
    }
    ready_tasks(goal, 1)?;
    let task = goal
        .proposal
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| other("unknown Manager task"))?;
    if goal.task_done(task_id) || !task.dependencies.iter().all(|id| goal.task_done(id)) {
        return Err(other(
            "task is already done or its dependencies are not done",
        ));
    }
    if goal
        .execution
        .evidence_runs
        .iter()
        .any(|run| run.outcome.is_none())
    {
        return Err(other("an evidence run is already in progress"));
    }
    let attempt = goal
        .execution
        .attempts
        .iter()
        .filter(|attempt| attempt.task_id == task_id)
        .max_by_key(|attempt| attempt.attempt)
        .ok_or_else(|| other("task has no failed attempt"))?;
    let child = application.read_session(&attempt.session_id)?;
    if attempt.launch_started != Some(true)
        || attempt.resume_pending
        || attempt.resume_starting
        || attempt.result_sha.is_some()
        || attempt.finalization_intent.is_some()
        || attempt.integration_intent.is_some()
        || child.managed_child.as_ref() != Some(&owner(goal, attempt))
        || !matches!(child.phase, SessionPhase::Failed(_))
        || child.live_foreign_runner_pid().is_some()
        || application.runtime().active_operation(&child.id).is_some()
    {
        return Err(other(
            "only a failed, stopped attempt can be resolved by evidence",
        ));
    }
    let known = |criterion: &String| goal.proposal.criteria.iter().any(|c| c.id == *criterion);
    let checked = |criterion: &String| {
        goal.proposal
            .checks
            .iter()
            .any(|check| check.criterion_ids.contains(criterion))
    };
    if criteria.is_empty()
        || has_duplicates(&criteria)
        || !criteria.iter().all(|c| known(c) && checked(c))
    {
        return Err(other(
            "evidence needs distinct approved criteria that each have an approved check",
        ));
    }
    let checks = goal
        .proposal
        .checks
        .iter()
        .filter(|check| check.criterion_ids.iter().any(|id| criteria.contains(id)))
        .map(|check| check.id.clone())
        .collect();
    let (Some(anchor), Some(tip)) = (
        goal.execution.anchor.as_deref(),
        goal.execution.integrated_tip.as_deref(),
    ) else {
        return Err(other("Goal anchor or tip missing"));
    };
    let reference = format!("refs/heads/cruise/goal/{}", goal.id);
    if git_output(anchor, &["rev-parse", "--verify", &reference])? != tip {
        return Err(other("Goal ref does not match its integrated tip"));
    }
    if paths.is_empty() || has_duplicates(paths) {
        return Err(other("evidence needs distinct reviewed file paths"));
    }
    let blobs = paths
        .iter()
        .map(|path| reviewed_blob(anchor, tip, path))
        .collect::<Result<_>>()?;
    Ok(EvidenceRequest {
        task_id: task_id.to_owned(),
        attempt: attempt.attempt,
        session_id: attempt.session_id.clone(),
        criteria,
        checks,
        blobs,
    })
}

/// The blob a reviewed path has at the tip. A path that is missing there is not evidence.
fn reviewed_blob(anchor: &Path, tip: &str, path: &str) -> Result<ReviewedBlob> {
    let missing = || other("a reviewed path is not a file at the Goal tip");
    if !Path::new(path)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(missing());
    }
    let blob = git_output(anchor, &["rev-parse", "--verify", &format!("{tip}:{path}")])
        .map_err(|_| missing())?;
    if git_output(anchor, &["cat-file", "-t", &blob])? != "blob" {
        return Err(missing());
    }
    Ok(ReviewedBlob {
        path: path.to_owned(),
        blob,
    })
}
