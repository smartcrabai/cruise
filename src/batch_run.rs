//! Bounded-concurrency batch scheduler for `run --all`.
//!
//! This module is shared between the CLI (`src/run_cmd.rs`) and the GUI
//! (`src-tauri/src/commands.rs`) so neither duplicates the scheduling loop.
//!
//! ## Scheduling rules
//! 1. Seed from [`SessionManager::run_all_remaining`] to get the initial candidate list.
//! 2. Launch up to `parallelism` sessions concurrently.
//! 3. Mark sessions as **seen** when *scheduled*, not when finished, to prevent double-running.
//! 4. Whenever a worker finishes, re-scan `run_all_remaining(&seen)` to pick up sessions
//!    added while the batch is in progress.
//! 5. Results are returned in stable **scheduling order** (first-scheduled = index 0),
//!    regardless of completion order.

use std::{collections::HashSet, future::Future};

use crate::{
    cancellation::CancellationToken,
    error::Result,
    session::{SessionManager, SessionState},
};

/// The result of executing a single session within a batch.
#[derive(Debug)]
pub struct BatchSessionResult {
    /// Stable position in the scheduling order (first-scheduled = 0).
    ///
    /// Use this to produce deterministic CLI summaries even when fast sessions
    /// complete before slow ones started earlier.
    pub batch_index: usize,
    /// Session ID.
    pub session_id: String,
    /// The outcome of executing the session.
    pub outcome: Result<()>,
}

/// Run all pending sessions with bounded parallelism.
///
/// # Arguments
///
/// * `manager`      - Provides candidate enumeration via [`SessionManager::run_all_remaining`].
/// * `parallelism`  - Maximum number of sessions running concurrently (must be >= 1).
/// * `cancel_token` - When cancelled, no new sessions are scheduled; in-flight sessions
///   receive the *same* token clone so they can observe cancellation.
/// * `run_fn`       - Called once per session; receives the session state and a
///   cancellation token clone. Must return a `Send` future.
///
/// # Returns
///
/// A `Vec<BatchSessionResult>` **sorted by `batch_index`** (scheduling order).
///
/// # Errors
///
/// Returns an error only if the session list cannot be read from disk, or `parallelism` is 0.
/// Individual session failures are captured inside [`BatchSessionResult::outcome`].
pub async fn run_all_with_parallelism<F, Fut>(
    manager: &SessionManager,
    parallelism: usize,
    cancel_token: CancellationToken,
    run_fn: F,
) -> Result<Vec<BatchSessionResult>>
where
    F: Fn(SessionState, CancellationToken) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    if parallelism == 0 {
        return Err(crate::error::CruiseError::Other(
            "run_all_with_parallelism: parallelism must be >= 1 (got 0)".to_string(),
        ));
    }

    if cancel_token.is_cancelled() {
        return Ok(Vec::new());
    }

    let mut seen: HashSet<String> = HashSet::new();
    // Sessions currently sitting in `candidates` but not yet scheduled.
    // Tracked separately so re-scans do not push duplicates into the deque,
    // which would otherwise cause O(N^2) deque growth for large batches.
    let mut queued: HashSet<String> = HashSet::new();
    let mut next_batch_index: usize = 0;
    // Stores (batch_index, session_id, outcome) from completed tasks.
    let mut completed: Vec<BatchSessionResult> = Vec::new();

    // JoinSet for in-flight tasks; each task yields (batch_index, session_id, outcome).
    let mut join_set: tokio::task::JoinSet<(usize, String, Result<()>)> =
        tokio::task::JoinSet::new();

    // Seed: fetch initial candidates and mark them queued.
    let initial = manager.run_all_remaining(&seen)?;
    for s in &initial {
        queued.insert(s.id.clone());
    }
    let mut candidates: std::collections::VecDeque<SessionState> = initial.into_iter().collect();

    loop {
        // Fill up to `parallelism` concurrent workers.
        while join_set.len() < parallelism && !cancel_token.is_cancelled() {
            let Some(session) = candidates.pop_front() else {
                break;
            };
            let session_id = session.id.clone();
            queued.remove(&session_id);
            if seen.contains(&session_id) {
                continue;
            }
            let batch_index = next_batch_index;
            next_batch_index += 1;
            // Mark as seen immediately when scheduled.
            seen.insert(session_id.clone());

            let run_fn_clone = run_fn.clone();
            let token_clone = cancel_token.clone();
            join_set.spawn(async move {
                let outcome = run_fn_clone(session, token_clone).await;
                (batch_index, session_id, outcome)
            });
        }

        // If no workers are running and no candidates remain, we're done.
        if join_set.is_empty() {
            break;
        }

        // Wait for the next worker to finish.
        let Some(task_result) = join_set.join_next().await else {
            break;
        };

        match task_result {
            Ok((batch_index, session_id, outcome)) => {
                completed.push(BatchSessionResult {
                    batch_index,
                    session_id,
                    outcome,
                });
            }
            Err(join_err) => {
                // Task panicked. The session ID is lost (it lives inside the spawned
                // future), so we cannot record a failure outcome for it.
                eprintln!("batch_run: worker task panicked: {join_err}");
            }
        }

        // If cancelled, drain the queue but let in-flight tasks finish.
        if cancel_token.is_cancelled() {
            candidates.clear();
            queued.clear();
            continue;
        }

        // Re-scan for sessions added while we were running, skipping any that are
        // already queued or scheduled to avoid duplicating entries in the deque.
        let fresh = manager.run_all_remaining(&seen)?;
        for s in fresh {
            if !queued.contains(&s.id) {
                queued.insert(s.id.clone());
                candidates.push_back(s);
            }
        }
    }

    // Sort by scheduling order before returning.
    completed.sort_by_key(|r| r.batch_index);
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    use crate::{
        cancellation::CancellationToken,
        error::CruiseError,
        session::{SessionManager, SessionPhase, SessionState, WorkspaceMode},
    };

    // -- Helpers --------------------------------------------------------------

    /// Create a minimal `Planned` session and register it with the manager.
    fn make_planned_session(manager: &SessionManager, id: &str, base_dir: &std::path::Path) {
        let mut state = SessionState::new(
            id.to_string(),
            base_dir.to_path_buf(),
            "test.yaml".to_string(),
            format!("task for {id}"),
        );
        state.phase = SessionPhase::Planned;
        state.workspace_mode = WorkspaceMode::Worktree;
        manager
            .create(&state)
            .unwrap_or_else(|e| panic!("create session {id}: {e}"));
    }

    /// A `run_fn` that immediately marks the session `Completed` in the manager and returns Ok.
    fn instant_completer(
        manager: Arc<SessionManager>,
    ) -> impl Fn(
        SessionState,
        CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>
    + Clone
    + Send
    + 'static {
        move |session, _cancel| {
            let manager = Arc::clone(&manager);
            Box::pin(async move {
                let mut state = manager
                    .load(&session.id)
                    .unwrap_or_else(|e| panic!("load {}: {e}", session.id));
                state.phase = SessionPhase::Completed;
                manager
                    .save(&state)
                    .unwrap_or_else(|e| panic!("save {}: {e}", session.id));
                Ok(())
            })
        }
    }

    /// A `run_fn` that records the session ID in `log` and then immediately completes.
    fn recording_completer(
        manager: Arc<SessionManager>,
        log: Arc<Mutex<Vec<String>>>,
    ) -> impl Fn(
        SessionState,
        CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>
    + Clone
    + Send
    + 'static {
        move |session, cancel| {
            let manager = Arc::clone(&manager);
            let log = Arc::clone(&log);
            Box::pin(async move {
                log.lock()
                    .unwrap_or_else(|e| panic!("{e}"))
                    .push(session.id.clone());

                if cancel.is_cancelled() {
                    return Err(CruiseError::Interrupted);
                }
                let mut state = manager
                    .load(&session.id)
                    .unwrap_or_else(|e| panic!("load {}: {e}", session.id));
                state.phase = SessionPhase::Completed;
                manager
                    .save(&state)
                    .unwrap_or_else(|e| panic!("save {}: {e}", session.id));
                Ok(())
            })
        }
    }

    // -- Basic scheduling ------------------------------------------------------

    #[tokio::test]
    async fn test_empty_candidate_list_returns_empty_results() {
        // Given: no sessions
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        let cancel = CancellationToken::new();

        // When: run with parallelism=1
        let results =
            run_all_with_parallelism(&manager, 1, cancel, instant_completer(Arc::clone(&manager)))
                .await
                .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: empty results, no error
        assert!(
            results.is_empty(),
            "expected no results for empty candidate list"
        );
    }

    #[tokio::test]
    async fn test_single_session_is_executed_with_parallelism_one() {
        // Given: one planned session
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());
        let cancel = CancellationToken::new();

        // When: run
        let results =
            run_all_with_parallelism(&manager, 1, cancel, instant_completer(Arc::clone(&manager)))
                .await
                .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: exactly one result with Ok outcome
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "20260101000001");
        assert!(results[0].outcome.is_ok(), "expected Ok outcome");
    }

    #[tokio::test]
    async fn test_multiple_sessions_all_executed() {
        // Given: three planned sessions
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        for id in ["20260101000001", "20260101000002", "20260101000003"] {
            make_planned_session(&manager, id, tmp.path());
        }
        let cancel = CancellationToken::new();

        // When: run with parallelism=2
        let results =
            run_all_with_parallelism(&manager, 2, cancel, instant_completer(Arc::clone(&manager)))
                .await
                .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: all three sessions are executed
        assert_eq!(results.len(), 3);
        let mut ids: Vec<_> = results.iter().map(|r| r.session_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["20260101000001", "20260101000002", "20260101000003"]);
    }

    // -- Result ordering -------------------------------------------------------

    #[tokio::test]
    async fn test_results_are_sorted_by_batch_index_ascending() {
        // Given: two sessions with IDs in ascending order
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());
        make_planned_session(&manager, "20260101000002", tmp.path());
        let cancel = CancellationToken::new();

        // When: run
        let results =
            run_all_with_parallelism(&manager, 2, cancel, instant_completer(Arc::clone(&manager)))
                .await
                .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: batch_index values are 0-based and ascending in the returned Vec
        assert_eq!(results[0].batch_index, 0);
        assert_eq!(results[1].batch_index, 1);
    }

    #[tokio::test]
    async fn test_results_maintain_scheduling_order_when_completions_are_out_of_order() {
        // Given: two sessions -- session-A is slow, session-B is fast.
        // With parallelism=2 both start simultaneously.
        // Session B completes first; session A completes second.
        // Expected: results[0].session_id == "A" (scheduled first) regardless.
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        // IDs in ascending order (scheduler picks them in this order)
        make_planned_session(&manager, "20260101000001", tmp.path()); // slow (index 0)
        make_planned_session(&manager, "20260101000002", tmp.path()); // fast (index 1)

        // Barrier: session-1 waits until session-2 calls it
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let slow_id = "20260101000001".to_string();
        let manager_clone = Arc::clone(&manager);
        let barrier_clone = Arc::clone(&barrier);

        let run_fn = {
            let manager = Arc::clone(&manager_clone);
            let barrier = Arc::clone(&barrier_clone);
            move |session: SessionState, _cancel: CancellationToken| {
                let manager = Arc::clone(&manager);
                let barrier = Arc::clone(&barrier);
                let id = session.id.clone();
                let slow = id == slow_id;
                Box::pin(async move {
                    if slow {
                        // Wait for the fast session to also reach the barrier
                        barrier.wait().await;
                    } else {
                        // Signal that we're ready; the slow session can proceed after this
                        barrier.wait().await;
                    }
                    let mut state = manager
                        .load(&id)
                        .unwrap_or_else(|e| panic!("load {id}: {e}"));
                    state.phase = SessionPhase::Completed;
                    manager
                        .save(&state)
                        .unwrap_or_else(|e| panic!("save {id}: {e}"));
                    Ok(())
                }) as std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>
            }
        };

        let cancel = CancellationToken::new();
        let results = run_all_with_parallelism(&manager, 2, cancel, run_fn)
            .await
            .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: results are in scheduling order, not completion order
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].session_id, "20260101000001",
            "first-scheduled session must be at index 0"
        );
        assert_eq!(
            results[1].session_id, "20260101000002",
            "second-scheduled session must be at index 1"
        );
        assert_eq!(results[0].batch_index, 0);
        assert_eq!(results[1].batch_index, 1);
    }

    // -- Session added mid-run ------------------------------------------------

    #[tokio::test]
    async fn test_session_added_while_first_is_running_is_picked_up() {
        // Given: one initial planned session; a second session will be added while
        // the first is executing.
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());

        // Gate: session-1 waits at the gate until we add session-2 and release.
        let gate = Arc::new(tokio::sync::Notify::new());
        let gate_clone = Arc::clone(&gate);
        let manager_for_adder = Arc::clone(&manager);
        let tmp_path = tmp.path().to_path_buf();

        let run_fn = {
            let manager = Arc::clone(&manager);
            let gate = Arc::clone(&gate_clone);
            move |session: SessionState, _cancel: CancellationToken| {
                let manager = Arc::clone(&manager);
                let gate = Arc::clone(&gate);
                let id = session.id.clone();
                Box::pin(async move {
                    if id == "20260101000001" {
                        // Notify the adder, then wait for the gate to be released
                        gate.notify_one();
                        gate.notified().await;
                    }
                    let mut state = manager
                        .load(&id)
                        .unwrap_or_else(|e| panic!("load {id}: {e}"));
                    state.phase = SessionPhase::Completed;
                    manager
                        .save(&state)
                        .unwrap_or_else(|e| panic!("save {id}: {e}"));
                    Ok(())
                }) as std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>
            }
        };

        // Adder task: waits for session-1 to start, adds session-2, then releases the gate.
        let adder = tokio::spawn(async move {
            // Wait until session-1 has started
            gate_clone.notified().await;
            // Add session-2 while the batch is running
            make_planned_session(&manager_for_adder, "20260101000002", &tmp_path);
            // Release session-1 to continue
            gate_clone.notify_one();
        });

        let cancel = CancellationToken::new();
        let results = run_all_with_parallelism(&manager, 1, cancel, run_fn)
            .await
            .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        adder.await.unwrap_or_else(|e| panic!("{e}"));

        // Then: both sessions were executed (dynamic pick-up)
        assert_eq!(
            results.len(),
            2,
            "session added mid-run must be picked up; got IDs: {:?}",
            results.iter().map(|r| &r.session_id).collect::<Vec<_>>()
        );
    }

    // -- Seen set: no duplicate execution -------------------------------------

    #[tokio::test]
    async fn test_session_is_not_executed_twice() {
        // Given: one planned session
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());

        let execution_count = Arc::new(Mutex::new(0usize));
        let count_clone = Arc::clone(&execution_count);
        let mgr_clone = Arc::clone(&manager);

        let run_fn = move |session: SessionState, _cancel: CancellationToken| {
            let manager = Arc::clone(&mgr_clone);
            let count = Arc::clone(&count_clone);
            let id = session.id.clone();
            Box::pin(async move {
                *count.lock().unwrap_or_else(|e| panic!("{e}")) += 1;
                let mut state = manager
                    .load(&id)
                    .unwrap_or_else(|e| panic!("load {id}: {e}"));
                state.phase = SessionPhase::Completed;
                manager
                    .save(&state)
                    .unwrap_or_else(|e| panic!("save {id}: {e}"));
                Ok(())
            }) as std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>
        };

        let cancel = CancellationToken::new();
        run_all_with_parallelism(&manager, 2, cancel, run_fn)
            .await
            .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: the session was executed exactly once
        let count = *execution_count.lock().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(count, 1, "session must not be executed twice");
    }

    // -- Cancellation ---------------------------------------------------------

    #[tokio::test]
    async fn test_cancellation_before_start_returns_empty_results() {
        // Given: one planned session but cancel is already triggered
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());

        let cancel = CancellationToken::new();
        cancel.cancel(); // cancelled before run starts

        let execution_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let results = run_all_with_parallelism(
            &manager,
            1,
            cancel,
            recording_completer(Arc::clone(&manager), Arc::clone(&execution_log)),
        )
        .await
        .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: no sessions were started
        assert!(
            results.is_empty(),
            "pre-cancelled run must not execute any sessions"
        );
        assert!(
            execution_log
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_empty(),
            "run_fn must not be called when already cancelled"
        );
    }

    #[tokio::test]
    async fn test_cancellation_stops_scheduling_new_sessions() {
        // Given: two sessions; session-1 cancels the token before completing,
        // so session-2 must not be scheduled.
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());
        make_planned_session(&manager, "20260101000002", tmp.path());

        let execution_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = Arc::clone(&execution_log);
        let mgr_clone = Arc::clone(&manager);

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let run_fn = move |session: SessionState, _cancel_arg: CancellationToken| {
            let manager = Arc::clone(&mgr_clone);
            let log = Arc::clone(&log_clone);
            let cancel = cancel_clone.clone();
            let id = session.id.clone();
            Box::pin(async move {
                log.lock()
                    .unwrap_or_else(|e| panic!("{e}"))
                    .push(id.clone());
                // Session 1 cancels the batch
                if id == "20260101000001" {
                    cancel.cancel();
                }
                let mut state = manager
                    .load(&id)
                    .unwrap_or_else(|e| panic!("load {id}: {e}"));
                state.phase = SessionPhase::Completed;
                manager
                    .save(&state)
                    .unwrap_or_else(|e| panic!("save {id}: {e}"));
                Ok(())
            }) as std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>
        };

        let results = run_all_with_parallelism(&manager, 1, cancel, run_fn)
            .await
            .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        // Then: only session-1 was executed; session-2 was not scheduled
        let log = execution_log
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .clone();
        assert!(
            log.contains(&"20260101000001".to_string()),
            "session-1 must have run"
        );
        assert!(
            !log.contains(&"20260101000002".to_string()),
            "session-2 must NOT be scheduled after cancellation"
        );
        // And the result for session-1 is present
        assert_eq!(results.len(), 1);
    }

    // -- Error cases ----------------------------------------------------------

    #[tokio::test]
    async fn test_zero_parallelism_returns_error() {
        // Given: parallelism = 0 -- explicitly invalid
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());
        let cancel = CancellationToken::new();

        // When: run with parallelism=0
        let result =
            run_all_with_parallelism(&manager, 0, cancel, instant_completer(Arc::clone(&manager)))
                .await;

        // Then: returns an error
        assert!(result.is_err(), "expected error for parallelism=0, got Ok");
    }

    #[tokio::test]
    async fn test_failed_session_outcome_is_captured_not_propagated() {
        // Given: a session whose run_fn returns an error
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = Arc::new(SessionManager::new(tmp.path().to_path_buf()));
        make_planned_session(&manager, "20260101000001", tmp.path());
        let cancel = CancellationToken::new();

        let run_fn = |_session: SessionState, _cancel: CancellationToken| {
            Box::pin(async { Err(CruiseError::Other("step failed".to_string())) })
                as std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>
        };

        // When: run
        let results = run_all_with_parallelism(&manager, 1, cancel, run_fn)
            .await
            .unwrap_or_else(|e| panic!("batch error must not propagate, got: {e}"));

        // Then: the batch itself succeeds; the failure is inside the result
        assert_eq!(results.len(), 1);
        assert!(
            results[0].outcome.is_err(),
            "failed session outcome must be captured inside BatchSessionResult"
        );
    }
}
