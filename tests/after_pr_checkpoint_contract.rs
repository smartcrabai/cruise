//! Persistence contracts for after-PR graphs and pre-stage session records.
use cruise::graph::persistence::{prepare_resume, save_graph};
use cruise::graph::{EdgeCounter, Transition, TransitionReason, build_graph};
use cruise::session::{SessionExecutionStage, SessionState};
use cruise::session_config::SessionConfigRef;

#[test]
fn after_pr_checkpoint_restores_counters_pending_and_runtime_with_new_limit() {
    let yaml = "command: [echo]\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  first:\n    command: 'true'\n    if:\n      fail: { retry: true }\n  last:\n    command: 'true'\n";
    let compiled =
        cruise::workflow::compile(cruise::config::WorkflowConfig::from_yaml(yaml).unwrap())
            .unwrap()
            .to_after_pr_compiled();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("after-pr-dag.json");
    let mut saved = build_graph(&compiled, 5).unwrap();
    saved.state.execution_id = Some("run-1".into());
    saved.state.current = Some("first".into());
    saved.state.edge_counts.insert(
        ("first".into(), "first".into()),
        EdgeCounter {
            traversals: 4,
            budgeted_traversals: 3,
        },
    );
    saved.state.pending = Some(Transition {
        target: Some("first".into()),
        reason: TransitionReason::IfFailRetry,
        budgeted: true,
        native_resync: false,
        group_retry: None,
    });
    saved.state.runtime.prev_output = Some("prior output".into());
    saved.state.runtime.prev_stderr = Some("prior error".into());
    saved.state.runtime.prev_success = Some(false);
    saved.state.runtime.file_snapshots.insert(
        "first".into(),
        [("src/lib.rs".into(), [42; 32])].into_iter().collect(),
    );
    save_graph(&saved, &path).unwrap();
    let mut resumed = build_graph(&compiled, 2).unwrap();
    assert_eq!(
        prepare_resume(
            &mut resumed,
            &path,
            true,
            Some("first"),
            true,
            Some("run-1")
        )
        .unwrap(),
        "first"
    );
    assert_eq!(resumed.state, saved.state);
    assert_eq!(
        resumed.max_retries, 2,
        "retry limit must come from current settings"
    );
}

#[test]
fn after_pr_group_retry_count_and_snapshot_survive_checkpoint_restore() {
    let yaml = "command: [echo]\ngroups:\n  review:\n    max_retries: 2\n    if:\n      file-changed: pass\n    steps:\n      work:\n        command: 'true'\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  pass:\n    group: review\n  finish:\n    command: 'true'\n";
    let compiled =
        cruise::workflow::compile(cruise::config::WorkflowConfig::from_yaml(yaml).unwrap())
            .unwrap()
            .to_after_pr_compiled();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("after-pr-dag.json");
    let mut saved = build_graph(&compiled, 3).unwrap();
    saved.state.execution_id = Some("run-1".into());
    saved.state.current = Some("pass/work".into());
    saved.state.group_counts.insert("pass".into(), 1);
    saved.state.runtime.file_snapshots.insert(
        "__group__pass".into(),
        [("src/lib.rs".into(), [7; 32])].into_iter().collect(),
    );
    save_graph(&saved, &path).unwrap();

    let mut resumed = build_graph(&compiled, 3).unwrap();
    prepare_resume(&mut resumed, &path, true, None, true, Some("run-1")).unwrap();
    assert_eq!(resumed.state.group_counts.get("pass"), Some(&1));
    assert_eq!(
        resumed.state.runtime.file_snapshots,
        saved.state.runtime.file_snapshots
    );
}

#[test]
fn old_failed_session_without_execution_stage_defaults_to_main() {
    let mut session = SessionState::new(
        "legacy".into(),
        "/tmp/repo".into(),
        SessionConfigRef::File {
            path: "/tmp/workflow.yaml".into(),
        },
        "legacy".into(),
    );
    session.phase = cruise::session::SessionPhase::Failed("old after-pr failure".into());
    session.current_step = Some("main-last".into());
    session.pr_url = Some("https://github.com/example/repo/pull/42".into());
    let mut value = serde_json::to_value(&session).unwrap();
    value.as_object_mut().unwrap().remove("execution_stage");
    let loaded: SessionState = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.execution_stage, SessionExecutionStage::Main);
    assert_eq!(loaded.current_step.as_deref(), Some("main-last"));
    assert_eq!(
        loaded.pr_url.as_deref(),
        Some("https://github.com/example/repo/pull/42")
    );
}
