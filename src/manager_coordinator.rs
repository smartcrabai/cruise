//! Goal-scoped control over already-reserved Manager children.
//!
//! These operations never infer authorization from a session ID supplied by a
//! client. The durable attempt identity and the live runtime claim must agree.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use crate::application::{
    CruiseApplication, OperationClaim, OperationKind, PendingPrompt, PendingPromptKind,
};
use crate::error::{CruiseError, Result};
use crate::manager::{self, Goal, TaskAttempt};
use crate::session::ManagedChildOwner;

// The durable pause and the corresponding runtime signal form one control
// operation. Do not let a concurrent resume overtake that signal.
pub(crate) static CONTROL: Mutex<()> = Mutex::new(());
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
type ResumeRequest = (std::path::PathBuf, String, String, u32, OperationClaim);
static RESUMES: Mutex<(bool, Vec<ResumeRequest>)> = Mutex::new((false, Vec::new()));

#[cfg(test)]
pub(crate) fn accept_resume_requests_without_dispatch() -> impl Drop {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            *RESUMES.lock().unwrap() = (false, Vec::new());
        }
    }
    let mut resumes = RESUMES.lock().unwrap();
    assert!(!resumes.0 && resumes.1.is_empty());
    resumes.0 = true;
    Restore
}
static ACTIVE_COORDINATORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PUBLICATIONS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<String, (uuid::Uuid, crate::cancellation::CancellationToken)>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

fn same_canonical_path(left: &Path, right: &Path) -> Result<bool> {
    Ok(left.canonicalize()? == right.canonicalize()?)
}

fn clear_failed_child_reason(goal: &mut Goal, task_id: &str) {
    let prefix = format!("Managed child failure for task {task_id} ");
    if goal
        .reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with(&prefix))
    {
        goal.reason = None;
    }
}

pub(crate) struct PublicationLease {
    goal_id: String,
    repository: std::path::PathBuf,
    id: uuid::Uuid,
    pub(crate) token: crate::cancellation::CancellationToken,
}

impl PublicationLease {
    pub(crate) fn owns(&self, cwd: &Path, goal_id: &str) -> Result<bool> {
        Ok(self.goal_id == goal_id
            && same_canonical_path(&self.repository, &manager::snapshot(cwd)?.repository)?)
    }
}

struct CoordinatorWork;

impl CoordinatorWork {
    fn begin() -> Self {
        ACTIVE_COORDINATORS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self
    }
}

impl Drop for CoordinatorWork {
    fn drop(&mut self) {
        ACTIVE_COORDINATORS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub(crate) fn active_operations() -> usize {
    ACTIVE_COORDINATORS.load(std::sync::atomic::Ordering::Acquire)
        + PUBLICATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
        + usize::from(!crate::manager_provider::is_idle())
}

impl Drop for PublicationLease {
    fn drop(&mut self) {
        let mut operations = PUBLICATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if operations
            .get(&self.goal_id)
            .is_some_and(|(id, _)| *id == self.id)
        {
            operations.remove(&self.goal_id);
        }
    }
}

/// Atomically admit at most one publication operation for a non-paused Goal.
pub(crate) fn begin_publication(
    cwd: &Path,
    goal_id: &str,
    shutdown: &crate::cancellation::CancellationToken,
) -> Result<PublicationLease> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal".into()))?;
    if !matches!(
        goal.status,
        manager::GoalStatus::Active
            | manager::GoalStatus::Verifying
            | manager::GoalStatus::AwaitingMerge
    ) || shutdown.is_cancelled()
    {
        return Err(CruiseError::Other(
            "Manager publication is paused or shutting down".into(),
        ));
    }
    let mut operations = PUBLICATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if operations.contains_key(goal_id) {
        return Err(CruiseError::Busy(
            "Manager publication is already running".into(),
        ));
    }
    let id = uuid::Uuid::new_v4();
    let token = shutdown.child_token();
    operations.insert(goal_id.to_owned(), (id, token.clone()));
    Ok(PublicationLease {
        goal_id: goal_id.to_owned(),
        repository: snapshot.repository,
        id,
        token,
    })
}

/// Wake the daemon after an explicit Manager action or child completion.
pub(crate) fn wake() {
    WAKE.notify_one();
}

/// Close the effect-start gate before cancelling daemon-owned operations.
pub(crate) fn stop(token: &crate::cancellation::CancellationToken) {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    token.cancel();
}

fn block_active(cwd: &Path, goal_id: &str, reason: String) -> Result<()> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    manager::update_goal(cwd, goal_id, |goal| {
        if goal.status == manager::GoalStatus::Active {
            goal.status = manager::GoalStatus::Blocked;
            goal.reason = Some(reason);
        }
        Ok(())
    })
}

#[expect(
    clippy::large_futures,
    reason = "Tokio's JoinSet stores the managed operation in a heap-allocated task"
)]
fn dispatch_pending_resumes(
    application: &CruiseApplication,
    sink: &std::sync::Arc<dyn crate::application::ApplicationEventSink>,
    log_sink: &std::sync::Arc<dyn crate::application::LogSink>,
    stop: &crate::cancellation::CancellationToken,
    children: &mut tokio::task::JoinSet<()>,
) {
    let resumes = std::mem::take(
        &mut RESUMES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1,
    );
    for (cwd, goal_id, task_id, attempt_number, claim) in resumes {
        // Pause and shutdown use the same gate, so neither can overtake
        // dispatch after the durable intent is consumed.
        let _guard = CONTROL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stop.is_cancelled() || claim.token().is_cancelled() {
            continue;
        }
        let admitted =
            manager::update_goal(&cwd, &goal_id, |goal| {
                if goal.status != manager::GoalStatus::Active {
                    return Ok(false);
                }
                let Some(attempt) = goal.execution.attempts.iter_mut().find(|attempt| {
                    attempt.task_id == task_id && attempt.attempt == attempt_number
                }) else {
                    return Ok(false);
                };
                if !attempt.resume_pending {
                    return Ok(false);
                }
                attempt.resume_pending = false;
                attempt.resume_starting = true;
                Ok(true)
            });
        match admitted {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                eprintln!("warning: Manager resume dispatch failed: {error}");
                continue;
            }
        }
        let application = application.clone();
        let sink = std::sync::Arc::clone(sink);
        let log_sink = std::sync::Arc::clone(log_sink);
        children.spawn(async move {
            let result = application
                .run_with_claim_and_log_sink(
                    claim,
                    crate::application::RunRequest::default(),
                    sink,
                    Some(log_sink),
                )
                .await;
            let failure = result.err().map(|error| error.to_string());
            if let Some(error) = &failure {
                eprintln!("warning: Manager child resume failed: {error}");
            }
            let _guard = CONTROL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Err(error) = manager::update_goal(&cwd, &goal_id, |goal| {
                if let Some(attempt) = goal.execution.attempts.iter_mut().find(|attempt| {
                    attempt.task_id == task_id && attempt.attempt == attempt_number
                }) {
                    attempt.resume_starting = false;
                }
                if let Some(error) = &failure
                    && goal.status == manager::GoalStatus::Active
                {
                    goal.reason = Some(format!(
                        "Child resume failed for {task_id}; inspect the session before retrying: {error}"
                    ));
                }
                Ok(())
            }) {
                eprintln!("warning: Manager could not record resume outcome: {error}");
            }
            wake();
        });
    }
}

fn record_failed_child(
    cwd: &Path,
    goal: &Goal,
    attempt: &TaskAttempt,
    failure: &str,
) -> Result<()> {
    let reason = format!(
        "Managed child failure for task {} (session {}): {failure}. Preserve its worktree and logs. Next action: `/resume {} {}` resumes this child; `/retry {} {}` starts a fresh attempt.",
        attempt.task_id, attempt.session_id, goal.id, attempt.task_id, goal.id, attempt.task_id,
    );
    if goal.reason.as_deref() == Some(reason.as_str()) {
        return Ok(());
    }
    manager::update_goal(cwd, &goal.id, |current| {
        let attempt_is_current = current.execution.attempts.iter().any(|saved| {
            saved.task_id == attempt.task_id
                && saved.attempt == attempt.attempt
                && saved.session_id == attempt.session_id
        }) && !current
            .execution
            .attempts
            .iter()
            .any(|newer| newer.task_id == attempt.task_id && newer.attempt > attempt.attempt);
        if current.status == manager::GoalStatus::Active && attempt_is_current {
            current.reason = Some(reason.clone());
        }
        Ok(())
    })
}

async fn finalize_and_integrate_child(
    application: &CruiseApplication,
    goal: &Goal,
    anchor: &Path,
    attempt: &TaskAttempt,
    stop: &crate::cancellation::CancellationToken,
) -> std::result::Result<bool, String> {
    if let Err(error) = finalize_child(
        application,
        anchor,
        &goal.id,
        &attempt.task_id,
        attempt.attempt,
    )
    .await
    {
        return Err(format!(
            "Child {} finalization requires attention: {error}",
            attempt.session_id
        ));
    }
    if stop.is_cancelled() {
        return Ok(false);
    }
    if let Err(error) = integrate(
        application,
        anchor,
        &goal.id,
        &attempt.task_id,
        attempt.attempt,
    ) {
        return Err(format!(
            "Child {} integration requires attention: {error}",
            attempt.session_id
        ));
    }
    Ok(true)
}

async fn inspect_goal_children(
    application: &CruiseApplication,
    goal: &Goal,
    anchor: &Path,
    stop: &crate::cancellation::CancellationToken,
    interrupted_resumes: Option<&HashSet<String>>,
) -> Option<String> {
    let mut unsafe_state = None;
    for attempt in &goal.execution.attempts {
        // A newer explicitly reserved attempt supersedes this one,
        // but its session and result remain durable evidence.
        if goal
            .execution
            .attempts
            .iter()
            .any(|newer| newer.task_id == attempt.task_id && newer.attempt > attempt.attempt)
        {
            continue;
        }
        if attempt.integrated {
            continue;
        }
        if attempt.launch_started == Some(false) {
            continue;
        }
        let child = match application.read_session(&attempt.session_id) {
            Ok(child) => child,
            Err(error) => {
                unsafe_state = Some(format!(
                    "Cannot recover child {}: {error}",
                    attempt.session_id
                ));
                break;
            }
        };
        if child.managed_child.as_ref() != Some(&owner(goal, attempt)) {
            unsafe_state = Some(format!(
                "Child {} has a different owner",
                attempt.session_id
            ));
            break;
        }
        if application
            .runtime()
            .active_operation(&attempt.session_id)
            .is_some()
            || child.live_foreign_runner_pid().is_some()
        {
            continue;
        }
        if attempt.resume_starting
            && interrupted_resumes.is_some_and(|ids| ids.contains(&attempt.session_id))
            && !matches!(child.phase, crate::session::SessionPhase::Completed)
        {
            unsafe_state = Some(format!(
                "Child {} has an interrupted resume dispatch; explicit review is required",
                attempt.session_id
            ));
            break;
        }
        match child.phase {
            crate::session::SessionPhase::Completed => {
                match finalize_and_integrate_child(application, goal, anchor, attempt, stop).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(reason) => {
                        unsafe_state = Some(reason);
                        break;
                    }
                }
            }
            crate::session::SessionPhase::Failed(failure) => {
                if let Err(error) = record_failed_child(anchor, goal, attempt, &failure) {
                    unsafe_state = Some(format!(
                        "Could not record failure for child {}: {error}",
                        attempt.session_id
                    ));
                    break;
                }
            }

            crate::session::SessionPhase::Planned | crate::session::SessionPhase::Running => {
                unsafe_state = Some(format!(
                    "Child {} has an interrupted or uncertain launch; explicit review is required",
                    attempt.session_id
                ));
                break;
            }
            _ => {}
        }
    }
    unsafe_state
}

#[expect(
    clippy::large_futures,
    reason = "Tokio's JoinSet stores the managed operation in a heap-allocated task"
)]
fn dispatch_ready_goal(
    application: &CruiseApplication,
    goal: &Goal,
    anchor: &Path,
    sinks: (
        &std::sync::Arc<dyn crate::application::ApplicationEventSink>,
        &std::sync::Arc<dyn crate::application::LogSink>,
    ),
    stop: &crate::cancellation::CancellationToken,
    children: &mut tokio::task::JoinSet<()>,
    publications: &mut tokio::task::JoinSet<()>,
) {
    // Re-read after integration: the snapshot above predates any
    // child finalization performed during this iteration.
    if let Ok(current) = manager::snapshot(anchor)
        && let Some(current_goal) = current.goals.iter().find(|g| g.id == goal.id)
        && current_goal.status == manager::GoalStatus::Active
        && current_goal.proposal.tasks.iter().all(|task| {
            current_goal
                .execution
                .attempts
                .iter()
                .any(|attempt| attempt.task_id == task.id && attempt.integrated)
        })
    {
        schedule_publication(publications, anchor, &goal.id, stop);
        return;
    }
    match reserve_and_claim(application, anchor, &goal.id) {
        Ok(Some(claim)) => {
            if stop.is_cancelled() || claim.token().is_cancelled() {
                drop(claim);
                return;
            }
            let application = application.clone();
            let sink = std::sync::Arc::clone(sinks.0);
            let log_sink = std::sync::Arc::clone(sinks.1);
            children.spawn(async move {
                if let Err(error) = application
                    .run_with_claim_and_log_sink(
                        claim,
                        crate::application::RunRequest::default(),
                        sink,
                        Some(log_sink),
                    )
                    .await
                {
                    eprintln!("warning: Manager child run failed: {error}");
                }
            });
        }
        Ok(None) => {}
        Err(error) => {
            if let Err(block_error) = block_active(
                anchor,
                &goal.id,
                format!("Manager dispatch requires attention: {error}"),
            ) {
                eprintln!(
                    "warning: Manager could not block goal {}: {block_error}",
                    goal.id
                );
            }
        }
    }
}

async fn schedule_inventory(
    application: &CruiseApplication,
    sinks: (
        &std::sync::Arc<dyn crate::application::ApplicationEventSink>,
        &std::sync::Arc<dyn crate::application::LogSink>,
    ),
    stop: &crate::cancellation::CancellationToken,
    interrupted_resumes: Option<&HashSet<String>>,
    children: &mut tokio::task::JoinSet<()>,
    publications: &mut tokio::task::JoinSet<()>,
) -> bool {
    let mut observing = false;
    match manager::snapshots() {
        Ok(snapshots) => {
            for snapshot in snapshots {
                for goal in snapshot.goals {
                    if stop.is_cancelled() {
                        break;
                    }
                    if matches!(
                        goal.status,
                        manager::GoalStatus::Verifying | manager::GoalStatus::AwaitingMerge
                    ) {
                        observing = true;
                        if let Some(anchor) = goal.execution.anchor.clone() {
                            schedule_publication(publications, &anchor, &goal.id, stop);
                        }
                        continue;
                    }
                    if goal.status != manager::GoalStatus::Active {
                        continue;
                    }
                    observing = true;
                    let Some(anchor) = goal.execution.anchor.clone() else {
                        continue;
                    };
                    // Durable pending requests are re-admitted after restart. A
                    // paused goal is never considered by this active-only loop.
                    for attempt in &goal.execution.attempts {
                        if attempt.resume_pending
                            && application
                                .runtime()
                                .active_operation(&attempt.session_id)
                                .is_none()
                            && let Err(error) = resume_task_inner(
                                application,
                                &anchor,
                                &goal.id,
                                &attempt.task_id,
                                true,
                            )
                        {
                            eprintln!(
                                "warning: Manager pending resume for {} not admitted: {error}",
                                attempt.session_id
                            );
                        }
                    }
                    let unsafe_state = inspect_goal_children(
                        application,
                        &goal,
                        &anchor,
                        stop,
                        interrupted_resumes,
                    )
                    .await;
                    if let Some(reason) = unsafe_state {
                        if let Err(error) = block_active(&anchor, &goal.id, reason) {
                            eprintln!("warning: Manager could not block goal {}: {error}", goal.id);
                        }
                        continue;
                    }
                    if stop.is_cancelled() {
                        break;
                    }
                    dispatch_ready_goal(
                        application,
                        &goal,
                        &anchor,
                        sinks,
                        stop,
                        children,
                        publications,
                    );
                }
            }
        }
        Err(error) => {
            eprintln!("warning: Manager inventory unavailable, dispatch stopped: {error}");
            observing = true;
        }
    }
    observing
}

/// The daemon owns this loop and joins it during shutdown. It never retries a
/// failed or uncertain child and never starts a Goal that is not active.
pub(crate) async fn run_daemon(
    application: CruiseApplication,
    sink: std::sync::Arc<dyn crate::application::ApplicationEventSink>,
    log_sink: std::sync::Arc<dyn crate::application::LogSink>,
    stop: crate::cancellation::CancellationToken,
) {
    RESUMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .0 = true;
    // Do not discard interruption evidence if inventory is temporarily unreadable.
    let mut interrupted_resumes: Option<HashSet<String>> = None;
    let mut children = tokio::task::JoinSet::new();
    let mut publications = tokio::task::JoinSet::new();
    loop {
        while let Some(result) = publications.try_join_next() {
            if let Err(error) = result {
                eprintln!("warning: Manager publication worker failed: {error}");
            }
        }
        if stop.is_cancelled() {
            break;
        }
        if interrupted_resumes.is_none() {
            match manager::snapshots() {
                Ok(snapshots) => {
                    interrupted_resumes = Some(
                        snapshots
                            .into_iter()
                            .flat_map(|snapshot| snapshot.goals)
                            .flat_map(|goal| goal.execution.attempts)
                            .filter(|attempt| attempt.resume_starting)
                            .map(|attempt| attempt.session_id)
                            .collect(),
                    );
                }
                Err(error) => {
                    eprintln!("warning: Manager resume recovery inventory unavailable: {error}");
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {},
                    }
                    continue;
                }
            }
        }
        dispatch_pending_resumes(&application, &sink, &log_sink, &stop, &mut children);
        let work = CoordinatorWork::begin();
        let observing = schedule_inventory(
            &application,
            (&sink, &log_sink),
            &stop,
            interrupted_resumes.as_ref(),
            &mut children,
            &mut publications,
        )
        .await;
        drop(work);
        tokio::select! {
            () = stop.cancelled() => break,
            () = WAKE.notified() => {},
            _ = children.join_next(), if !children.is_empty() => {},
            () = tokio::time::sleep(std::time::Duration::from_secs(2)), if observing => {},
        }
    }
    {
        let mut resumes = RESUMES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resumes.0 = false;
        resumes.1.clear();
    }
    while children.join_next().await.is_some() {}
    while publications.join_next().await.is_some() {}
}

fn schedule_publication(
    workers: &mut tokio::task::JoinSet<()>,
    anchor: &Path,
    goal_id: &str,
    stop: &crate::cancellation::CancellationToken,
) {
    let lease = match begin_publication(anchor, goal_id, stop) {
        Ok(lease) => lease,
        Err(CruiseError::Busy(_)) => return,
        Err(error) => {
            eprintln!("warning: Manager publication admission failed for {goal_id}: {error}");
            return;
        }
    };
    let anchor = anchor.to_owned();
    let goal_id = goal_id.to_owned();
    workers.spawn_blocking(move || {
        if let Err(error) = crate::manager_publication::reconcile(&anchor, &goal_id, &lease) {
            eprintln!("warning: Manager publication reconciliation failed for {goal_id}: {error}");
        }
    });
}

pub(crate) fn validate_execution_contract(goal: &Goal) -> Result<()> {
    goal.proposal.validate()?;
    let contract = goal.proposal.contract.as_ref().ok_or_else(|| {
        CruiseError::Other("Goal is missing its reviewed execution contract".into())
    })?;
    if goal.execution.anchor.as_ref() != Some(&contract.anchor)
        || goal.execution.origin.as_deref() != Some(contract.origin.as_str())
        || goal
            .execution
            .workflow
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            != Some(serde_json::to_value(&contract.workflow)?)
    {
        return Err(CruiseError::Other(
            "Goal execution settings differ from the reviewed contract".into(),
        ));
    }
    Ok(())
}

/// Pick undispatched tasks in proposal order. A dependency is ready only after
/// integration was persisted, not merely when its child session completed.
/// This function does not reserve or dispatch anything: callers must do that
/// atomically across all Goals while enforcing the daemon-wide capacity.
pub(crate) fn ready_tasks(goal: &Goal, capacity: usize) -> Result<Vec<String>> {
    validate_execution_contract(goal)?;
    if goal.status != manager::GoalStatus::Active || capacity == 0 {
        return Ok(Vec::new());
    }
    if goal.execution.anchor.is_none()
        || goal.execution.origin.as_deref().is_none_or(str::is_empty)
        || goal.execution.workflow.is_none()
        || goal
            .execution
            .integrated_tip
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return Err(CruiseError::Other(
            "Goal is missing its frozen repository or workflow execution contract".into(),
        ));
    }
    let tasks = &goal.proposal.tasks;
    let mut sessions = HashSet::new();
    let mut attempts = HashSet::new();
    if goal.execution.attempts.iter().any(|attempt| {
        !tasks.iter().any(|task| task.id == attempt.task_id)
            || attempt.attempt == 0
            || attempt.session_id.is_empty()
            || attempt.base_sha.is_empty()
            || (attempt.integrated && attempt.result_sha.as_deref().is_none_or(str::is_empty))
            || !sessions.insert(&attempt.session_id)
            || !attempts.insert((&attempt.task_id, attempt.attempt))
    }) {
        return Err(CruiseError::Other(
            "invalid Manager task attempt record".into(),
        ));
    }
    let mut ready = Vec::new();
    for task in tasks {
        // Any previous attempt requires an explicit retry decision, even if it failed.
        if goal.execution.attempts.iter().any(|a| a.task_id == task.id) {
            continue;
        }
        let dependencies_integrated = task.dependencies.iter().all(|dependency| {
            goal.execution
                .attempts
                .iter()
                .any(|a| a.task_id == *dependency && a.integrated)
        });
        if dependencies_integrated {
            ready.push(task.id.clone());
            if ready.len() == capacity {
                break;
            }
        }
    }
    Ok(ready)
}

fn approved_child_plan(goal: &Goal, task: &manager::Task) -> String {
    let mut plan = format!(
        "# Goal\n\n{}\n\n# Task\n\n{}\n\n# Acceptance criteria\n",
        goal.proposal.objective, task.instructions
    );
    for criterion in &goal.proposal.criteria {
        let _ = write!(plan, "\n- {}: {}", criterion.id, criterion.description);
    }
    plan.push_str("\n\n# Exclusions\n");
    for exclusion in &goal.proposal.exclusions {
        let _ = write!(plan, "\n- {exclusion}");
    }
    plan.push('\n');
    plan
}

/// Validate the durable owner and approved inputs again after acquiring the
/// runtime claim, immediately before entering the shared execution path.
pub(crate) fn validate_claimed_child(
    application: &CruiseApplication,
    claim: &OperationClaim,
    child: &crate::session::SessionState,
) -> Result<()> {
    let Some(child_owner) = child.managed_child.as_ref() else {
        return Ok(());
    };
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if claim.token().is_cancelled()
        || application.runtime().active_identity(&child.id) != Some(claim.identity())
    {
        return Err(CruiseError::Other(
            "Manager child claim was cancelled or replaced".into(),
        ));
    }
    let snapshot = manager::snapshot(&child.base_dir)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == child_owner.goal_id)
        .ok_or_else(|| CruiseError::Other("Manager child Goal is missing".into()))?;
    validate_execution_contract(goal)?;
    if goal.status != manager::GoalStatus::Active {
        return Err(CruiseError::Other("Manager Goal is not active".into()));
    }
    let attempt = goal
        .execution
        .attempts
        .iter()
        .find(|attempt| {
            attempt.task_id == child_owner.task_id
                && attempt.attempt == child_owner.attempt
                && attempt.session_id == child.id
        })
        .ok_or_else(|| CruiseError::Other("Manager attempt is missing".into()))?;
    let task = goal
        .proposal
        .tasks
        .iter()
        .find(|task| task.id == attempt.task_id)
        .ok_or_else(|| CruiseError::Other("Manager task is missing".into()))?;
    let contract = goal
        .proposal
        .contract
        .as_ref()
        .ok_or_else(|| CruiseError::Other("Manager contract is missing".into()))?;
    let actual_config = application.session_config(child)?;
    if child.managed_child.as_ref() != Some(&owner(goal, attempt))
        || !same_canonical_path(&child.base_dir, &contract.anchor)?
        || git_output(&child.base_dir, &["remote", "get-url", "origin"])? != contract.origin
        || git_output(
            &child.base_dir,
            &["remote", "get-url", "--push", "--all", "origin"],
        )? != contract.origin
        || !child.config.is_snapshot()
        || child.workspace_mode != crate::session::WorkspaceMode::Worktree
        || child.repo.is_some()
        || child.pr_target.is_some()
        || child.target_branch.is_some()
        || child.exec
        || child.allow_dirty_working_tree
        || !child.input_as_plan
        || child.cleanup_after_pr_override.is_some()
        || !child.attachments.is_empty()
        || !child.flags.is_empty()
        || !child.skipped_steps.is_empty()
        || actual_config.create_pr
        || actual_config.cleanup_after_pr
        || actual_config.force_exec
        || !actual_config.after_pr.is_empty()
        || serde_json::to_value(&actual_config)? != serde_json::to_value(&contract.workflow)?
        || application.session_plan(&child.id)? != approved_child_plan(goal, task)
    {
        return Err(CruiseError::Other(
            "Manager child settings differ from the approved contract".into(),
        ));
    }
    Ok(())
}

fn check_orphan_children(application: &CruiseApplication, ids: &HashSet<&String>) -> Result<()> {
    let store = application.managed_session_store();
    match std::fs::read_dir(store.sessions_dir()) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_dir() && entry.path().join("manager-owner.json").exists() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !ids.contains(&name) {
                        return Err(CruiseError::Other(format!(
                            "orphan Manager reservation {name} prevents safe capacity accounting"
                        )));
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for child in application.list_sessions()? {
        if child.managed_child.is_some() && !ids.contains(&child.id) {
            return Err(CruiseError::Other(format!(
                "orphan Manager child {} prevents safe capacity accounting",
                child.id
            )));
        }
    }
    Ok(())
}

fn count_active_children(
    application: &CruiseApplication,
    inventory: &[manager::Snapshot],
) -> Result<usize> {
    let runtime = application.runtime();
    let mut active = 0usize;
    let mut ids = HashSet::new();
    for snapshot in inventory {
        for existing_goal in &snapshot.goals {
            for attempt in &existing_goal.execution.attempts {
                if !ids.insert(&attempt.session_id) {
                    return Err(CruiseError::Other(
                        "duplicate Manager child reservation".into(),
                    ));
                }
                let child = application
                    .read_session(&attempt.session_id)
                    .map_err(|error| {
                        CruiseError::Other(format!(
                            "cannot account for Manager child {}: {error}",
                            attempt.session_id
                        ))
                    });
                let child = match child {
                    Ok(child) => child,
                    Err(error) if attempt.launch_started == Some(false) => {
                        let dir = application
                            .managed_session_store()
                            .sessions_dir()
                            .join(&attempt.session_id);
                        if dir.join("state.json").exists()
                            || runtime.active_operation(&attempt.session_id).is_some()
                        {
                            return Err(error);
                        }
                        if dir.exists() {
                            let marker = std::fs::read(dir.join("manager-owner.json")).map_err(|error| {
                                CruiseError::Other(format!(
                                    "Manager reservation {} has an unreadable or missing owner marker ({error}); ownership cannot be established, manual investigation is required",
                                    attempt.session_id
                                ))
                            })?;
                            let saved: ManagedChildOwner = serde_json::from_slice(&marker).map_err(|error| {
                                CruiseError::Other(format!(
                                    "Manager reservation {} has an invalid owner marker ({error}); ownership cannot be established, manual investigation is required",
                                    attempt.session_id
                                ))
                            })?;
                            if saved != owner(existing_goal, attempt) {
                                return Err(CruiseError::Other(format!(
                                    "Manager reservation {} owner changed",
                                    attempt.session_id
                                )));
                            }
                        }
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if child.managed_child.as_ref() != Some(&owner(existing_goal, attempt)) {
                    return Err(CruiseError::Other(format!(
                        "Manager child {} owner changed",
                        attempt.session_id
                    )));
                }
                let claimed = runtime.active_operation(&attempt.session_id).is_some();
                let foreign = child.live_foreign_runner_pid().is_some();
                if claimed || foreign {
                    active += 1;
                } else if attempt.launch_started != Some(false)
                    && matches!(
                        child.phase,
                        crate::session::SessionPhase::Running
                            | crate::session::SessionPhase::Planned
                    )
                    && existing_goal.status != manager::GoalStatus::Blocked
                {
                    return Err(CruiseError::Other(format!(
                        "Manager child {} has an uncertain interrupted launch; review required",
                        attempt.session_id
                    )));
                }
            }
        }
    }
    check_orphan_children(application, &ids)?;
    Ok(active)
}

fn validate_checkout(
    anchor: &Path,
    contract: &manager::FrozenContract,
    repository: &Path,
) -> Result<()> {
    if !same_canonical_path(
        anchor,
        &std::path::PathBuf::from(git_output(anchor, &["rev-parse", "--show-toplevel"])?),
    )? || !same_canonical_path(&manager::snapshot(anchor)?.repository, repository)?
        || git_output(anchor, &["remote", "get-url", "origin"])? != contract.origin
        || git_output(anchor, &["remote", "get-url", "--push", "--all", "origin"])?
            != contract.origin
    {
        return Err(CruiseError::Other(
            "Manager checkout or origin changed".into(),
        ));
    }
    let forge =
        crate::forge::resolve_remote(&contract.origin, crate::forge::forge_override_from_env()?)?;
    if forge.kind != contract.forge_kind
        || forge.host != contract.forge_host
        || forge.repository != contract.forge_repository
    {
        return Err(CruiseError::Other("Manager forge identity changed".into()));
    }
    if contract.workflow.create_pr
        || contract.workflow.cleanup_after_pr
        || contract.workflow.force_exec
        || !contract.workflow.after_pr.is_empty()
    {
        return Err(CruiseError::Other(
            "unsafe Manager workflow settings".into(),
        ));
    }
    Ok(())
}

fn claim_reserved_child(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    goal: &Goal,
    attempt: &TaskAttempt,
    child: &crate::session::SessionState,
    plan: &str,
) -> Result<Option<OperationClaim>> {
    let contract = goal
        .proposal
        .contract
        .as_ref()
        .ok_or_else(|| CruiseError::Other("missing Manager contract".into()))?;
    let child_owner = owner(goal, attempt);
    let runtime = application.runtime();
    let store = application.managed_session_store();
    let result = (|| {
        let created = store.create_managed_child(child, &contract.workflow, plan)?;
        if created.managed_child.as_ref() != Some(&child_owner)
            || created.phase != crate::session::SessionPhase::Planned
            || application.session_plan(&created.id)? != plan
            || serde_json::to_value(application.session_config(&created)?)?
                != serde_json::to_value(&contract.workflow)?
        {
            return Err(CruiseError::Other(
                "reserved Manager child differs from approved contract".into(),
            ));
        }
        if created.live_foreign_runner_pid().is_some()
            || runtime.active_operation(&created.id).is_some()
        {
            return Err(CruiseError::Other(
                "reserved Manager child already has a runner".into(),
            ));
        }
        let claim = runtime.try_begin_managed(&created.id, &child_owner, OperationKind::Run)?;
        manager::update_goal(cwd, goal_id, |current| {
            if current.status != manager::GoalStatus::Active {
                return Err(CruiseError::Other("Manager Goal is not active".into()));
            }
            let saved = current
                .execution
                .attempts
                .iter_mut()
                .find(|saved| {
                    saved.session_id == attempt.session_id
                        && saved.task_id == attempt.task_id
                        && saved.attempt == attempt.attempt
                })
                .ok_or_else(|| CruiseError::Other("Manager reservation disappeared".into()))?;
            if saved.launch_started != Some(false) || saved.base_sha != attempt.base_sha {
                return Err(CruiseError::Other("Manager reservation changed".into()));
            }
            saved.launch_started = Some(true);
            Ok(())
        })?;
        Ok(claim)
    })();
    if let Err(error) = &result {
        let reason = format!(
            "Manager dispatch interrupted for {}: {error}",
            attempt.session_id
        );
        let _ = manager::update_goal(cwd, goal_id, |current| {
            current.status = manager::GoalStatus::Blocked;
            current.reason = Some(reason);
            Ok(())
        });
    }
    result.map(Some)
}

fn validate_goal_tip<'a>(goal: &'a Goal, anchor: &Path) -> Result<&'a String> {
    let tip = goal
        .execution
        .integrated_tip
        .as_ref()
        .ok_or_else(|| CruiseError::Other("missing Goal tip".into()))?;
    let reference = format!("refs/heads/cruise/goal/{}", goal.id);
    // Initializing this ref is only allowed from the approved target, before
    // any task was reserved. Existing refs must match the durable exact tip.
    if goal.execution.attempts.is_empty() && tip == &goal.proposal.target_sha {
        crate::manager_git::initialize(anchor, &reference, tip).map_err(CruiseError::Other)?;
    }
    if git_output(anchor, &["rev-parse", "--verify", &reference])? != *tip {
        return Err(CruiseError::Other(
            "Goal ref does not match integrated tip".into(),
        ));
    }
    Ok(tip)
}

/// Reserve a previously undispatched task and claim its new child while holding
/// the same gate as pause. The returned claim must be run or dropped by the
/// daemon. An interrupted reservation is never silently launched again.
pub(crate) fn reserve_and_claim(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
) -> Result<Option<OperationClaim>> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let inventory = manager::snapshots()?;
    let active = count_active_children(application, &inventory)?;
    if active >= 2 {
        return Ok(None);
    }
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    if goal.status != manager::GoalStatus::Active {
        return Ok(None);
    }
    let recoverable: Vec<_> = goal
        .execution
        .attempts
        .iter()
        .filter(|attempt| attempt.launch_started == Some(false))
        .collect();
    if recoverable.len() > 1 {
        return Err(CruiseError::Other(
            "multiple interrupted Manager reservations require review".into(),
        ));
    }
    let task_id = if let Some(attempt) = recoverable.first() {
        attempt.task_id.clone()
    } else if let Some(task_id) = ready_tasks(goal, 1)?.into_iter().next() {
        task_id
    } else {
        return Ok(None);
    };
    validate_execution_contract(goal)?;
    let contract = goal
        .proposal
        .contract
        .as_ref()
        .ok_or_else(|| CruiseError::Other("missing Manager contract".into()))?;
    let anchor = &contract.anchor;
    validate_checkout(anchor, contract, &snapshot.repository)?;
    let tip = validate_goal_tip(goal, anchor)?;
    let task = goal
        .proposal
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| CruiseError::Other("Manager task disappeared".into()))?;
    let plan = approved_child_plan(goal, task);
    let attempt = recoverable.first().map_or_else(
        || TaskAttempt {
            task_id: task_id.clone(),
            attempt: 1,
            session_id: crate::session::SessionManager::new_session_id(),
            base_sha: tip.clone(),
            launch_started: Some(false),
            resume_pending: false,
            resume_starting: false,
            result_sha: None,
            finalization_intent: None,
            integrated: false,
            integration_intent: None,
        },
        |attempt| (*attempt).clone(),
    );
    if attempt.base_sha != *tip || attempt.integrated || attempt.result_sha.is_some() {
        return Err(CruiseError::Other(
            "interrupted Manager reservation no longer matches Goal tip".into(),
        ));
    }
    let child_owner = owner(goal, &attempt);
    let mut child = crate::session::SessionState::new(
        attempt.session_id.clone(),
        anchor.clone(),
        crate::session_config::SessionConfigRef::inline_snapshot(),
        task.instructions.clone(),
    );
    child.managed_child = Some(child_owner.clone());
    child.mark_input_as_plan();
    child.phase = crate::session::SessionPhase::Planned;
    if recoverable.is_empty() {
        manager::update_goal(cwd, goal_id, |current| {
            if current.status != manager::GoalStatus::Active
                || current.execution.integrated_tip.as_ref() != Some(tip)
                || !ready_tasks(current, 1)?.contains(&task_id)
            {
                return Err(CruiseError::Other("Manager dispatch state changed".into()));
            }
            current.execution.attempts.push(attempt.clone());
            Ok(())
        })?;
    }
    claim_reserved_child(application, cwd, goal_id, goal, &attempt, &child, &plan)
}

/// Finalize a completed owned child without publishing or touching the user's checkout.
/// A persisted parent/tree pair identifies the only commit this operation may recover.
pub(crate) async fn finalize_child(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
    attempt_number: u32,
) -> Result<String> {
    finalize_child_inner(application, cwd, goal_id, task_id, attempt_number, false).await
}

fn validate_finalization_worktree(
    child: &crate::session::SessionState,
    attempt: &TaskAttempt,
    worktree: &Path,
    anchor: &Path,
    repository: &Path,
) -> Result<String> {
    if !same_canonical_path(
        worktree,
        &std::path::PathBuf::from(git_output(worktree, &["rev-parse", "--show-toplevel"])?),
    )? || !same_canonical_path(&manager::snapshot(worktree)?.repository, repository)?
        || same_canonical_path(worktree, anchor)?
        || worktree.file_name().and_then(|name| name.to_str()) != Some(attempt.session_id.as_str())
    {
        return Err(CruiseError::Other("child worktree identity changed".into()));
    }
    let branch = child
        .worktree_branch
        .as_deref()
        .ok_or_else(|| CruiseError::Other("completed child has no worktree branch".into()))?;
    let prefix = format!("cruise/{}", attempt.session_id);
    if !(branch == prefix
        || branch
            .strip_prefix(&prefix)
            .is_some_and(|suffix| suffix.starts_with('-')))
        || git_output(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"])? != branch
    {
        return Err(CruiseError::Other("child worktree branch changed".into()));
    }
    let head = git_output(worktree, &["rev-parse", "HEAD"])?;
    if !Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["merge-base", "--is-ancestor", &attempt.base_sha, &head])
        .status()?
        .success()
    {
        return Err(CruiseError::Other(
            "child HEAD does not descend from its pinned base".into(),
        ));
    }
    Ok(head)
}

fn finalization_intent(
    cwd: &Path,
    goal: &Goal,
    attempt: &TaskAttempt,
    worktree: &Path,
    head: &str,
    observed_only: bool,
) -> Result<manager::ChildFinalizationIntent> {
    let goal_id = goal.id.as_str();
    let task_id = attempt.task_id.as_str();
    let attempt_number = attempt.attempt;
    let intent = if let Some(intent) = &attempt.finalization_intent {
        intent.clone()
    } else {
        if goal.status == manager::GoalStatus::Paused || observed_only {
            return Err(CruiseError::Other(
                "Goal is paused; finalization has not begun".into(),
            ));
        }
        // Staging is confined to the owned worktree. Persist the resulting
        // tree before the commit so a lost receipt is distinguishable.
        git_output(worktree, &["add", "-A"])?;
        let intent = manager::ChildFinalizationIntent {
            expected_head: head.to_owned(),
            expected_tree: git_output(worktree, &["write-tree"])?,
        };
        manager::update_goal(cwd, goal_id, |goal| {
            let attempt = goal
                .execution
                .attempts
                .iter_mut()
                .find(|attempt| attempt.task_id == task_id && attempt.attempt == attempt_number)
                .ok_or_else(|| CruiseError::Other("Manager attempt disappeared".into()))?;
            if attempt.finalization_intent.is_some() || attempt.result_sha.is_some() {
                return Err(CruiseError::Other(
                    "Manager finalization state changed".into(),
                ));
            }
            attempt.finalization_intent = Some(intent.clone());
            Ok(())
        })?;
        intent
    };
    Ok(intent)
}

fn validate_finalization_anchor(goal: &Goal, anchor: &Path, repository: &Path) -> Result<()> {
    if !same_canonical_path(
        anchor,
        &std::path::PathBuf::from(git_output(anchor, &["rev-parse", "--show-toplevel"])?),
    )? || !same_canonical_path(&manager::snapshot(anchor)?.repository, repository)?
    {
        return Err(CruiseError::Other("Goal repository anchor changed".into()));
    }
    let origin = goal
        .execution
        .origin
        .as_deref()
        .ok_or_else(|| CruiseError::Other("Goal has no approved origin".into()))?;
    if git_output(anchor, &["remote", "get-url", "origin"])? != origin {
        return Err(CruiseError::Other(
            "Goal origin changed since approval".into(),
        ));
    }
    Ok(())
}

fn validate_finalized_commit(
    worktree: &Path,
    intent: &manager::ChildFinalizationIntent,
) -> Result<String> {
    let result = git_output(worktree, &["rev-parse", "HEAD"])?;
    let tree = git_output(worktree, &["rev-parse", "HEAD^{tree}"])?;
    if tree != intent.expected_tree
        || !git_output(
            worktree,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty()
    {
        return Err(CruiseError::Other(
            "finalized child tree does not match its saved intent".into(),
        ));
    }
    if result != intent.expected_head {
        let parents = git_output(worktree, &["rev-list", "--parents", "-n", "1", &result])?;
        if parents != format!("{result} {}", intent.expected_head) {
            return Err(CruiseError::Other(
                "finalized child commit does not match its saved parent".into(),
            ));
        }
    }
    Ok(result)
}

fn existing_finalized_result(
    attempt: &TaskAttempt,
    worktree: &Path,
    head: &str,
) -> Result<Option<String>> {
    if let Some(result) = &attempt.result_sha {
        if head != *result
            || !git_output(
                worktree,
                &["status", "--porcelain", "--untracked-files=all"],
            )?
            .is_empty()
        {
            return Err(CruiseError::Other(
                "child worktree changed after result was pinned".into(),
            ));
        }
        return Ok(Some(result.clone()));
    }
    Ok(None)
}

async fn finalize_child_inner(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
    attempt_number: u32,
    observed_only: bool,
) -> Result<String> {
    let application = application.clone();
    let cwd = cwd.to_path_buf();
    let goal_id = goal_id.to_owned();
    let task_id = task_id.to_owned();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _guard = CONTROL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = manager::snapshot(&cwd)?;
        let goal = snapshot
            .goals
            .iter()
            .find(|goal| goal.id == goal_id)
            .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
        validate_execution_contract(goal)?;
        if !matches!(
            goal.status,
            manager::GoalStatus::Active | manager::GoalStatus::Paused
        ) {
            return Err(CruiseError::Other("Goal is not active or paused".into()));
        }
        let anchor =
            goal.execution.anchor.as_deref().ok_or_else(|| {
                CruiseError::Other("Goal has no approved repository anchor".into())
            })?;
        validate_finalization_anchor(goal, anchor, &snapshot.repository)?;
        let attempt = goal
            .execution
            .attempts
            .iter()
            .find(|attempt| attempt.task_id == task_id && attempt.attempt == attempt_number)
            .ok_or_else(|| CruiseError::Other("unknown Manager task attempt".into()))?;
        let child = application.read_session(&attempt.session_id)?;
        if child.managed_child.as_ref() != Some(&owner(goal, attempt))
            || child.phase != crate::session::SessionPhase::Completed
        {
            return Err(CruiseError::Other(
                "child owner or completion state does not match Goal attempt".into(),
            ));
        }
        let worktree = child
            .worktree_path
            .as_deref()
            .ok_or_else(|| CruiseError::Other("completed child has no worktree".into()))?;
        let head = validate_finalization_worktree(
            &child,
            attempt,
            worktree,
            anchor,
            &snapshot.repository,
        )?;
        if let Some(result) = existing_finalized_result(attempt, worktree, &head)? {
            return Ok(result);
        }
        let intent = finalization_intent(&cwd, goal, attempt, worktree, &head, observed_only)?;
        if head == intent.expected_head {
            if goal.status == manager::GoalStatus::Paused || observed_only {
                return Err(CruiseError::Other(
                    "Goal is paused; finalization has not completed".into(),
                ));
            }
            git_output(worktree, &["add", "-A"])?;
            if git_output(worktree, &["write-tree"])? != intent.expected_tree {
                return Err(CruiseError::Other(
                    "child tree changed during finalization".into(),
                ));
            }
            runtime.block_on(crate::worktree_pr::commit_changes(
                worktree,
                || Ok("Finalize Manager task".to_string()),
                None,
            ))?;
        }
        let result = validate_finalized_commit(worktree, &intent)?;
        manager::update_goal(&cwd, &goal_id, |goal| {
            let attempt = goal
                .execution
                .attempts
                .iter_mut()
                .find(|attempt| attempt.task_id == task_id && attempt.attempt == attempt_number)
                .ok_or_else(|| CruiseError::Other("Manager attempt disappeared".into()))?;
            if attempt.finalization_intent.as_ref() != Some(&intent) || attempt.result_sha.is_some()
            {
                return Err(CruiseError::Other(
                    "Manager finalization intent changed".into(),
                ));
            }
            attempt.result_sha = Some(result.clone());
            Ok(())
        })?;
        Ok(result)
    })
    .await
    .map_err(|error| CruiseError::Other(format!("child finalization task failed: {error}")))?
}

/// Integrate an already finalized child result using a persisted compare-and-swap
/// intent. This is deliberately separate from child finalization: callers must
/// have pinned the exact result SHA before calling it.
pub(crate) fn integrate(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
    attempt_number: u32,
) -> Result<()> {
    integrate_inner(application, cwd, goal_id, task_id, attempt_number, false)
}

fn validate_integrated_child(
    application: &CruiseApplication,
    goal: &Goal,
    attempt: &TaskAttempt,
    repository: &Path,
    result: &str,
) -> Result<()> {
    let child = application.read_session(&attempt.session_id)?;
    if child.managed_child.as_ref() != Some(&owner(goal, attempt))
        || child.phase != crate::session::SessionPhase::Completed
    {
        return Err(CruiseError::Other(
            "child owner or completion state does not match Goal attempt".into(),
        ));
    }
    let worktree = child
        .worktree_path
        .as_deref()
        .ok_or_else(|| CruiseError::Other("completed child has no worktree".into()))?;
    if !same_canonical_path(&manager::snapshot(worktree)?.repository, repository)? {
        return Err(CruiseError::Other(
            "child worktree belongs to a different repository".into(),
        ));
    }
    if git_output(worktree, &["rev-parse", "HEAD"])? != result
        || !git_output(
            worktree,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty()
    {
        return Err(CruiseError::Other(
            "child worktree changed after result was pinned".into(),
        ));
    }
    Ok(())
}

fn validate_integration_anchor(goal: &Goal, anchor: &Path, repository: &Path) -> Result<()> {
    if !same_canonical_path(
        anchor,
        &std::path::PathBuf::from(git_output(anchor, &["rev-parse", "--show-toplevel"])?),
    )? {
        return Err(CruiseError::Other("Goal checkout anchor changed".into()));
    }
    if !same_canonical_path(&manager::snapshot(anchor)?.repository, repository)? {
        return Err(CruiseError::Other("Goal repository anchor changed".into()));
    }
    let origin = goal
        .execution
        .origin
        .as_deref()
        .ok_or_else(|| CruiseError::Other("Goal has no approved origin".into()))?;
    if git_output(anchor, &["remote", "get-url", "origin"])? != origin {
        return Err(CruiseError::Other(
            "Goal origin changed since approval".into(),
        ));
    }
    Ok(())
}

fn integration_intent(
    cwd: &Path,
    anchor: &Path,
    goal: &Goal,
    attempt: &TaskAttempt,
    observed_only: bool,
) -> Result<crate::manager_git::IntegrationIntent> {
    let goal_id = &goal.id;
    let task_id = attempt.task_id.as_str();
    let attempt_number = attempt.attempt;
    let result = attempt
        .result_sha
        .as_deref()
        .ok_or_else(|| CruiseError::Other("child result is not pinned".into()))?;
    let tip = goal
        .execution
        .integrated_tip
        .as_deref()
        .ok_or_else(|| CruiseError::Other("Goal has no integrated tip".into()))?;
    let intent = if let Some(intent) = &attempt.integration_intent {
        if intent.goal_ref != format!("refs/heads/cruise/goal/{goal_id}")
            || intent.expected_tip != tip
            || intent.child_base != attempt.base_sha
            || intent.child_result != result
        {
            return Err(CruiseError::Other(
                "stored integration intent disagrees with Goal attempt".into(),
            ));
        }
        intent.clone()
    } else {
        if goal.status == manager::GoalStatus::Paused || observed_only {
            return Err(CruiseError::Other(
                "Goal is paused; no integration intent has been applied".into(),
            ));
        }
        let reference = format!("refs/heads/cruise/goal/{goal_id}");
        let intent = crate::manager_git::prepare_integration(
            anchor,
            &reference,
            tip,
            &attempt.base_sha,
            result,
        )
        .map_err(CruiseError::Other)?;
        manager::update_goal(cwd, goal_id, |goal| {
            let attempt = goal
                .execution
                .attempts
                .iter_mut()
                .find(|attempt| attempt.task_id == task_id && attempt.attempt == attempt_number)
                .ok_or_else(|| CruiseError::Other("Manager attempt disappeared".into()))?;
            attempt.integration_intent = Some(intent.clone());
            Ok(())
        })?;
        intent
    };
    Ok(intent)
}

fn integrate_inner(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
    attempt_number: u32,
    observed_only: bool,
) -> Result<()> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    validate_execution_contract(goal)?;
    if observed_only {
        let mut validation = goal.clone();
        validation.status = manager::GoalStatus::Active;
        ready_tasks(&validation, 1)?;
    }
    if !matches!(
        goal.status,
        manager::GoalStatus::Active | manager::GoalStatus::Paused
    ) {
        return Err(CruiseError::Other(
            "Goal is not active; integration is paused".into(),
        ));
    }
    let anchor = goal
        .execution
        .anchor
        .as_deref()
        .ok_or_else(|| CruiseError::Other("Goal has no approved repository anchor".into()))?;
    validate_integration_anchor(goal, anchor, &snapshot.repository)?;
    let attempt = goal
        .execution
        .attempts
        .iter()
        .find(|attempt| attempt.task_id == task_id && attempt.attempt == attempt_number)
        .ok_or_else(|| CruiseError::Other("unknown Manager task attempt".into()))?;
    if attempt.integrated {
        return Ok(());
    }
    let result = attempt
        .result_sha
        .as_deref()
        .ok_or_else(|| CruiseError::Other("child result is not pinned".into()))?;
    if !observed_only {
        validate_integrated_child(application, goal, attempt, &snapshot.repository, result)?;
    }
    let intent = integration_intent(cwd, anchor, goal, attempt, observed_only)?;
    if observed_only {
        if crate::manager_git::observe_integration(anchor, &intent).map_err(CruiseError::Other)?
            == crate::manager_git::IntegrationObservation::Pending
        {
            return Ok(());
        }
    } else if goal.status == manager::GoalStatus::Paused {
        if !crate::manager_git::integration_already_applied(anchor, &intent)
            .map_err(CruiseError::Other)?
        {
            return Err(CruiseError::Other(
                "Goal is paused; integration has not already completed".into(),
            ));
        }
    } else {
        crate::manager_git::apply_integration(anchor, &intent).map_err(CruiseError::Other)?;
    }
    manager::update_goal(cwd, goal_id, |goal| {
        if !matches!(
            goal.status,
            manager::GoalStatus::Active | manager::GoalStatus::Paused
        ) || goal.execution.integrated_tip.as_deref() != Some(intent.expected_tip.as_str())
        {
            return Err(CruiseError::Other(
                "Goal state changed during integration reconciliation".into(),
            ));
        }
        let attempt = goal
            .execution
            .attempts
            .iter_mut()
            .find(|attempt| attempt.task_id == task_id && attempt.attempt == attempt_number)
            .ok_or_else(|| CruiseError::Other("Manager attempt disappeared".into()))?;
        if attempt.integration_intent.as_ref() != Some(&intent)
            || attempt.base_sha != intent.child_base
            || attempt.result_sha.as_deref() != Some(intent.child_result.as_str())
        {
            return Err(CruiseError::Other(
                "Manager integration intent changed".into(),
            ));
        }
        attempt.integrated = true;
        goal.execution.integrated_tip = Some(intent.prepared_tip.clone());
        Ok(())
    })
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map(|output| output.trim().to_owned())
        .map_err(|error| CruiseError::Other(error.to_string()))
}

fn owner(goal: &Goal, attempt: &TaskAttempt) -> ManagedChildOwner {
    ManagedChildOwner {
        goal_id: goal.id.clone(),
        task_id: attempt.task_id.clone(),
        attempt: attempt.attempt,
        revision: goal.proposal.revision,
        base_sha: attempt.base_sha.clone(),
    }
}

/// Explicitly continue the latest failed or suspended attempt in its existing worktree.
pub(crate) fn resume_task(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
) -> Result<Goal> {
    resume_task_inner(application, cwd, goal_id, task_id, false)
}

fn validate_resume_capacity(
    application: &CruiseApplication,
    inventory: &[manager::Snapshot],
) -> Result<()> {
    let mut ids = HashSet::new();
    let mut active = 0;
    for repository in inventory {
        for existing_goal in &repository.goals {
            for existing in &existing_goal.execution.attempts {
                if !ids.insert(existing.session_id.clone()) {
                    return Err(CruiseError::Other(
                        "duplicate Manager child reservation".into(),
                    ));
                }
                let state = application.read_session(&existing.session_id)?;
                if state.managed_child.as_ref() != Some(&owner(existing_goal, existing)) {
                    return Err(CruiseError::Other("Manager child owner changed".into()));
                }
                if application.runtime().active_operation(&state.id).is_some()
                    || state.live_foreign_runner_pid().is_some()
                {
                    active += 1;
                }
            }
        }
    }
    match std::fs::read_dir(application.managed_session_store().sessions_dir()) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_dir()
                    && entry.path().join("manager-owner.json").exists()
                    && !ids.contains(&entry.file_name().to_string_lossy().into_owned())
                {
                    return Err(CruiseError::Other(
                        "orphan Manager reservation prevents safe capacity accounting".into(),
                    ));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for state in application.list_sessions()? {
        if state.managed_child.is_some() && !ids.contains(&state.id) {
            return Err(CruiseError::Other(
                "orphan Manager child prevents safe capacity accounting".into(),
            ));
        }
    }
    if active >= 2 {
        return Err(CruiseError::Other("Manager child capacity is full".into()));
    }
    Ok(())
}

fn validate_resume_worktree(
    application: &CruiseApplication,
    goal: &Goal,
    attempt: &TaskAttempt,
    child: &crate::session::SessionState,
    repository: &Path,
) -> Result<()> {
    let goal_id = &goal.id;
    let anchor = goal
        .execution
        .anchor
        .as_deref()
        .ok_or_else(|| CruiseError::Other("Goal anchor missing".into()))?;
    let tip = goal
        .execution
        .integrated_tip
        .as_deref()
        .ok_or_else(|| CruiseError::Other("Goal tip missing".into()))?;
    let worktree = child
        .worktree_path
        .as_deref()
        .ok_or_else(|| CruiseError::Other("child worktree missing".into()))?;
    let expected = application
        .managed_session_store()
        .worktrees_dir()
        .join(&child.id);
    if !same_canonical_path(
        anchor,
        &std::path::PathBuf::from(git_output(anchor, &["rev-parse", "--show-toplevel"])?),
    )? || !same_canonical_path(&manager::snapshot(anchor)?.repository, repository)?
        || git_output(
            anchor,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/cruise/goal/{goal_id}"),
            ],
        )? != tip
        || !expected.is_dir()
        || !same_canonical_path(worktree, &expected)?
        || !same_canonical_path(&manager::snapshot(worktree)?.repository, repository)?
        || !Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["merge-base", "--is-ancestor", &attempt.base_sha, "HEAD"])
            .status()?
            .success()
    {
        return Err(CruiseError::Other(
            "Goal ref or child worktree identity changed".into(),
        ));
    }
    Ok(())
}

fn validate_resume_child(
    application: &CruiseApplication,
    goal: &Goal,
    attempt: &TaskAttempt,
) -> Result<crate::session::SessionState> {
    let child = application.read_session(&attempt.session_id)?;
    if child.managed_child.as_ref() != Some(&owner(goal, attempt))
        || !matches!(
            child.phase,
            crate::session::SessionPhase::Failed(_) | crate::session::SessionPhase::Suspended
        )
        || child.live_foreign_runner_pid().is_some()
        || application.runtime().active_operation(&child.id).is_some()
    {
        return Err(CruiseError::Other(
            "child owner, phase or runtime state is unsafe for resume".into(),
        ));
    }
    Ok(child)
}

fn validate_resume_attempt(attempt: &TaskAttempt, recovery: bool) -> Result<()> {
    if attempt.resume_starting {
        return Err(CruiseError::Other(
            "child resume dispatch requires explicit review".into(),
        ));
    }
    if attempt.resume_pending != recovery {
        return Err(CruiseError::Other(if recovery {
            "resume is no longer pending".into()
        } else {
            "child resume is already pending".into()
        }));
    }
    if attempt.launch_started != Some(true)
        || attempt.integrated
        || attempt.result_sha.is_some()
        || attempt.finalization_intent.is_some()
        || attempt.integration_intent.is_some()
    {
        return Err(CruiseError::Other(
            "child attempt has an uncertain launch or finalization".into(),
        ));
    }
    Ok(())
}

fn resume_task_inner(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
    recovery: bool,
) -> Result<Goal> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let inventory = manager::snapshots()?;
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    validate_execution_contract(goal)?;
    if goal.status != manager::GoalStatus::Active {
        return Err(CruiseError::Other(
            "Goal must be active to resume a child".into(),
        ));
    }
    ready_tasks(goal, 1)?;
    let task = goal
        .proposal
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager task".into()))?;
    if !task.dependencies.iter().all(|dependency| {
        goal.execution
            .attempts
            .iter()
            .any(|attempt| attempt.task_id == *dependency && attempt.integrated)
    }) || goal.execution.attempts.iter().any(|attempt| {
        attempt.task_id == task_id && (attempt.integrated || attempt.integration_intent.is_some())
    }) {
        return Err(CruiseError::Other(
            "task dependencies or prior integration are unsafe".into(),
        ));
    }
    let attempt = goal
        .execution
        .attempts
        .iter()
        .filter(|attempt| attempt.task_id == task_id)
        .max_by_key(|attempt| attempt.attempt)
        .ok_or_else(|| CruiseError::Other("task has no previous attempt".into()))?;
    validate_resume_attempt(attempt, recovery)?;
    let child = validate_resume_child(application, goal, attempt)?;
    validate_resume_worktree(application, goal, attempt, &child, &snapshot.repository)?;
    validate_resume_capacity(application, &inventory)?;
    let mut resumes = RESUMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !resumes.0 {
        return Err(CruiseError::Other(
            "Manager coordinator is not running".into(),
        ));
    }
    let claim = application.runtime().try_begin_managed(
        &child.id,
        &owner(goal, attempt),
        OperationKind::Run,
    )?;
    if !recovery {
        manager::update_goal(cwd, goal_id, |saved_goal| {
            let saved = saved_goal
                .execution
                .attempts
                .iter_mut()
                .find(|saved| {
                    saved.task_id == task_id
                        && saved.attempt == attempt.attempt
                        && saved.session_id == child.id
                })
                .ok_or_else(|| CruiseError::Other("child attempt changed".into()))?;
            saved.resume_pending = true;
            clear_failed_child_reason(saved_goal, task_id);
            Ok(())
        })?;
    }
    resumes.1.push((
        cwd.to_path_buf(),
        goal_id.to_owned(),
        task_id.to_owned(),
        attempt.attempt,
        claim,
    ));
    wake();
    Ok(goal.clone())
}

fn validate_retry_phase(
    goal: &Goal,
    previous: &TaskAttempt,
    child: &crate::session::SessionState,
    repository: &Path,
    anchor: &Path,
    tip: &str,
) -> Result<()> {
    match &child.phase {
        crate::session::SessionPhase::Failed(_) | crate::session::SessionPhase::Suspended
            if goal.status == manager::GoalStatus::Active
                && previous.result_sha.is_none()
                && previous.finalization_intent.is_none() => {}
        crate::session::SessionPhase::Completed if goal.status == manager::GoalStatus::Blocked => {
            let expected_reason = format!(
                "Child {} integration requires attention:",
                previous.session_id
            );
            if !goal
                .reason
                .as_deref()
                .is_some_and(|r| r.starts_with(&expected_reason))
            {
                return Err(CruiseError::Other(
                    "Goal is blocked for a different reason".into(),
                ));
            }
            let result = previous
                .result_sha
                .as_deref()
                .ok_or_else(|| CruiseError::Other("child result is not pinned".into()))?;
            let worktree = child
                .worktree_path
                .as_deref()
                .ok_or_else(|| CruiseError::Other("child worktree missing".into()))?;
            if !same_canonical_path(&manager::snapshot(worktree)?.repository, repository)?
                || git_output(worktree, &["rev-parse", "HEAD"])? != result
                || !git_output(
                    worktree,
                    &["status", "--porcelain", "--untracked-files=all"],
                )?
                .is_empty()
            {
                return Err(CruiseError::Other(
                    "child result or worktree changed".into(),
                ));
            }
            for commit in [tip, result] {
                if !Command::new("git")
                    .arg("-C")
                    .arg(anchor)
                    .args(["merge-base", "--is-ancestor", &previous.base_sha, commit])
                    .status()?
                    .success()
                {
                    return Err(CruiseError::Other(
                        "child base ancestry cannot be established".into(),
                    ));
                }
            }
            let merge = Command::new("git")
                .arg("-C")
                .arg(anchor)
                .args(["merge-tree", "--write-tree", tip, result])
                .output()?;
            if merge.status.code() != Some(1) {
                return Err(CruiseError::Other(
                    "integration conflict cannot be established safely".into(),
                ));
            }
        }
        _ => {
            return Err(CruiseError::Other(
                "previous task attempt is not safely retryable".into(),
            ));
        }
    }
    Ok(())
}

fn select_retry_attempt<'a>(goal: &'a Goal, task_id: &str) -> Result<&'a TaskAttempt> {
    let task = goal
        .proposal
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager task".into()))?;
    if !task.dependencies.iter().all(|dep| {
        goal.execution
            .attempts
            .iter()
            .any(|a| a.task_id == *dep && a.integrated)
    }) {
        return Err(CruiseError::Other(
            "task dependencies are not integrated".into(),
        ));
    }
    if goal
        .execution
        .attempts
        .iter()
        .any(|a| a.launch_started == Some(false))
    {
        return Err(CruiseError::Other(
            "a Manager reservation is already pending".into(),
        ));
    }
    let attempts: Vec<_> = goal
        .execution
        .attempts
        .iter()
        .filter(|a| a.task_id == task_id)
        .collect();
    if attempts.iter().any(|a| a.integrated) {
        return Err(CruiseError::Other("task is already integrated".into()));
    }
    if attempts.iter().any(|a| a.integration_intent.is_some()) {
        return Err(CruiseError::Other(
            "an earlier attempt has an integration intent requiring reconciliation".into(),
        ));
    }
    let previous = attempts
        .iter()
        .max_by_key(|a| a.attempt)
        .ok_or_else(|| CruiseError::Other("task has no previous attempt".into()))?;
    if previous.resume_pending
        || (previous.resume_starting && goal.status != manager::GoalStatus::Blocked)
    {
        return Err(CruiseError::Other(
            "a child resume is pending or requires review".into(),
        ));
    }
    if previous.attempt == 0
        || previous.integration_intent.is_some()
        || previous.launch_started != Some(true)
    {
        return Err(CruiseError::Other(
            "previous attempt has an uncertain launch or integration intent; review required"
                .into(),
        ));
    }
    Ok(*previous)
}

/// Explicitly reserve a fresh attempt without changing or deleting prior evidence.
/// The coordinator gate serializes this decision with dispatch and integration.
pub(crate) fn retry_task(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    task_id: &str,
) -> Result<Goal> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|g| g.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    validate_execution_contract(goal)?;
    if !matches!(
        goal.status,
        manager::GoalStatus::Active | manager::GoalStatus::Blocked
    ) {
        return Err(CruiseError::Other(
            "Goal must be active or blocked to retry a task".into(),
        ));
    }
    // Reuse the scheduler's complete attempt identity validation, even for a
    // blocked Goal, before treating an older attempt as superseded.
    let mut validation = goal.clone();
    validation.status = manager::GoalStatus::Active;
    ready_tasks(&validation, 1)?;
    let previous = select_retry_attempt(goal, task_id)?;
    let child = application.read_session(&previous.session_id)?;
    if child.managed_child.as_ref() != Some(&owner(goal, previous))
        || application
            .runtime()
            .active_operation(&previous.session_id)
            .is_some()
        || child.live_foreign_runner_pid().is_some()
    {
        return Err(CruiseError::Other(
            "previous child owner or runtime state is unsafe for retry".into(),
        ));
    }
    let anchor = goal
        .execution
        .anchor
        .as_deref()
        .ok_or_else(|| CruiseError::Other("Goal anchor missing".into()))?;
    let tip = goal
        .execution
        .integrated_tip
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CruiseError::Other("Goal tip missing".into()))?;
    let reference = format!("refs/heads/cruise/goal/{goal_id}");
    if !same_canonical_path(
        anchor,
        &std::path::PathBuf::from(git_output(anchor, &["rev-parse", "--show-toplevel"])?),
    )? || !same_canonical_path(&manager::snapshot(anchor)?.repository, &snapshot.repository)?
        || git_output(anchor, &["rev-parse", "--verify", &reference])? != *tip
    {
        return Err(CruiseError::Other("Goal anchor or ref changed".into()));
    }
    validate_retry_phase(goal, previous, &child, &snapshot.repository, anchor, tip)?;
    let number = previous
        .attempt
        .checked_add(1)
        .ok_or_else(|| CruiseError::Other("attempt number exhausted".into()))?;
    let attempt = TaskAttempt {
        task_id: task_id.to_owned(),
        attempt: number,
        session_id: crate::session::SessionManager::new_session_id(),
        base_sha: tip.clone(),
        launch_started: Some(false),
        resume_pending: false,
        resume_starting: false,
        result_sha: None,
        finalization_intent: None,
        integrated: false,
        integration_intent: None,
    };
    manager::update_goal(cwd, goal_id, |current| {
        if current.status != goal.status
            || current.execution.integrated_tip.as_ref() != Some(tip)
            || current.execution.attempts.len() != goal.execution.attempts.len()
        {
            return Err(CruiseError::Other("Goal changed during retry".into()));
        }
        current.execution.attempts.push(attempt);
        if current.status == manager::GoalStatus::Blocked {
            current.status = manager::GoalStatus::Active;
            current.reason = None;
        }
        clear_failed_child_reason(current, task_id);
        Ok(current.clone())
    })
}

/// Recover only effects which Git already shows as applied. This never starts a
/// child, makes a commit, or moves a ref. A lost or corrupt inventory prevents
/// recovery rather than allowing records to be silently skipped.
pub(crate) async fn reconcile_applied(application: &CruiseApplication) -> Result<Vec<String>> {
    let snapshots = manager::snapshots()?;
    let mut errors = Vec::new();
    for snapshot in snapshots {
        for goal in &snapshot.goals {
            if !matches!(
                goal.status,
                manager::GoalStatus::Active | manager::GoalStatus::Paused
            ) {
                continue;
            }
            let Some(anchor) = goal.execution.anchor.as_deref() else {
                if goal
                    .execution
                    .attempts
                    .iter()
                    .any(|attempt| !attempt.integrated && attempt.integration_intent.is_some())
                    && let Err(error) = manager::block_startup_goal(
                        &snapshot.repository,
                        &goal.id,
                        "Goal has no approved repository anchor",
                    )
                {
                    errors.push(format!("{}: {error}", goal.id));
                }
                continue;
            };
            for attempt in &goal.execution.attempts {
                if attempt.integrated {
                    continue;
                }
                // A completed child commit with a matching persisted parent/tree
                // can be recorded without creating another commit. finalize_child
                // performs the full owner, worktree and ancestry checks.
                if attempt.result_sha.is_none()
                    && let Some(intent) = &attempt.finalization_intent
                {
                    let head_changed = application
                        .read_session(&attempt.session_id)
                        .ok()
                        .and_then(|child| child.worktree_path)
                        .and_then(|path| git_output(&path, &["rev-parse", "HEAD"]).ok())
                        .is_some_and(|head| head != intent.expected_head);
                    if head_changed
                        && let Err(error) = finalize_child_inner(
                            application,
                            anchor,
                            &goal.id,
                            &attempt.task_id,
                            attempt.attempt,
                            true,
                        )
                        .await
                    {
                        errors.push(format!("{} / {}: {error}", goal.id, attempt.task_id));
                        continue;
                    }
                }
                if attempt.integration_intent.is_some() {
                    // Do not inspect the stored ref before binding the intent to
                    // the current Goal and attempt inside integrate_inner.
                    if let Err(error) = integrate_inner(
                        application,
                        anchor,
                        &goal.id,
                        &attempt.task_id,
                        attempt.attempt,
                        true,
                    ) {
                        if let Err(block_error) = manager::block_startup_goal(
                            &snapshot.repository,
                            &goal.id,
                            &format!("task {} integration recovery: {error}", attempt.task_id),
                        ) {
                            errors
                                .push(format!("{} / {}: {block_error}", goal.id, attempt.task_id));
                        }
                        break;
                    }
                }
            }
        }
    }
    Ok(errors)
}

/// Save the pause intent before signalling every owned child. Continue signalling
/// siblings even if one has a mismatched or missing owner, then report failures.
pub(crate) fn pause(application: &CruiseApplication, cwd: &Path, goal_id: &str) -> Result<Goal> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let goal = manager::pause_goal(cwd, goal_id)?;
    if let Some((_, token)) = PUBLICATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(goal_id)
    {
        token.cancel();
    }
    let runtime = application.runtime();
    let mut failures = Vec::new();
    for attempt in &goal.execution.attempts {
        if let Err(error) = runtime.pause_managed(&attempt.session_id, &owner(&goal, attempt)) {
            failures.push(format!("{}: {error}", attempt.session_id));
        }
    }
    if !failures.is_empty() {
        return Err(CruiseError::Other(format!(
            "Goal pause was recorded, but some children could not be signalled: {}",
            failures.join(", ")
        )));
    }
    Ok(goal)
}

/// Explicitly requeue verification only after the previous worker has drained.
pub(crate) fn retry_verification(cwd: &Path, goal_id: &str) -> Result<Goal> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if PUBLICATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(goal_id)
    {
        return Err(CruiseError::Busy(
            "Manager publication is still stopping; retry shortly".into(),
        ));
    }
    crate::manager_publication::retry_verification(cwd, goal_id)?;
    manager::snapshot(cwd)?
        .goals
        .into_iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal".into()))
}

/// Restore the pre-pause state without retrying failed or interrupted attempts.
pub(crate) fn resume(cwd: &Path, goal_id: &str) -> Result<Goal> {
    let _guard = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if PUBLICATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(goal_id)
    {
        return Err(CruiseError::Busy(
            "Manager publication is still stopping; retry resume shortly".into(),
        ));
    }
    manager::resume_goal(cwd, goal_id)
}

/// Return live prompt identities only from exact owned children of this Goal.
pub(crate) fn prompts(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
) -> Result<Vec<PendingPrompt>> {
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    let runtime = application.runtime();
    let mut prompts = Vec::new();
    for attempt in &goal.execution.attempts {
        let pending = runtime.pending_prompts(&attempt.session_id);
        if pending.is_empty() {
            continue;
        }
        let state = application.read_session(&attempt.session_id)?;
        if state.managed_child.as_ref() != Some(&owner(goal, attempt)) {
            return Err(CruiseError::Other(format!(
                "session {} has a different Manager owner",
                attempt.session_id
            )));
        }
        prompts.extend(pending);
    }
    Ok(prompts)
}

/// Answer an active ask by exact prompt ID. Never substitute a replacement prompt.
pub(crate) fn answer(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    prompt_id: &str,
    answer: String,
) -> Result<()> {
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    let runtime = application.runtime();
    for attempt in &goal.execution.attempts {
        if runtime
            .pending_prompts(&attempt.session_id)
            .iter()
            .any(|prompt| {
                prompt.request_id == prompt_id && matches!(prompt.kind, PendingPromptKind::Ask)
            })
        {
            return runtime.respond_to_managed_ask(
                &attempt.session_id,
                &owner(goal, attempt),
                prompt_id,
                answer,
            );
        }
    }
    Err(CruiseError::Other(
        "stale or unknown Manager ask prompt".into(),
    ))
}

/// Answer an active option without ever substituting a replacement prompt.
pub(crate) fn answer_option(
    application: &CruiseApplication,
    cwd: &Path,
    goal_id: &str,
    prompt_id: &str,
    result: crate::step::option::OptionResult,
) -> Result<()> {
    let snapshot = manager::snapshot(cwd)?;
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    let runtime = application.runtime();
    for attempt in &goal.execution.attempts {
        if runtime
            .pending_prompts(&attempt.session_id)
            .iter()
            .any(|prompt| {
                prompt.request_id == prompt_id && matches!(prompt.kind, PendingPromptKind::Option)
            })
        {
            return runtime.respond_to_managed_option(
                &attempt.session_id,
                &owner(goal, attempt),
                prompt_id,
                result,
            );
        }
    }
    Err(CruiseError::Other(
        "stale or unknown Manager option prompt".into(),
    ))
}
