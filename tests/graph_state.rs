#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]
use cruise::cancellation::CancellationToken;
use cruise::config::{StepConfig, WorkflowConfig};
use cruise::engine::{ExecutionContext, execute_steps_with_graph};
use cruise::error::CruiseError;
use cruise::file_tracker::FileTracker;
use cruise::graph::persistence::{load_graph, prepare_resume, save_graph};
use cruise::graph::validation::validate_workflow;
use cruise::graph::{EdgeCounter, Transition, TransitionReason, build_graph};
use cruise::option_handler::OptionHandler;
use cruise::session::{SessionManager, SessionPhase, SessionState};
struct NoOpOptionHandler;
impl OptionHandler for NoOpOptionHandler {
    fn select_option(
        &self,
        _: &[cruise::step::OptionChoice],
        _: Option<&str>,
    ) -> cruise::error::Result<cruise::step::option::OptionResult> {
        panic!("unexpected option interaction");
    }
}
use cruise::variable::VariableStore;
use cruise::workflow::{CompiledWorkflow, compile};

fn workflow(steps: &str) -> CompiledWorkflow {
    compile(WorkflowConfig::from_yaml(&format!("command: [echo]\n{steps}")).unwrap()).unwrap()
}

fn context<'a>(
    compiled: &'a CompiledWorkflow,
    dir: &'a std::path::Path,
    max: usize,
) -> ExecutionContext<'a> {
    ExecutionContext {
        compiled,
        max_retries: max,
        rate_limit_retries: 0,
        on_step_start: &|_| Ok(()),
        cancel_token: None,
        option_handler: &NoOpOptionHandler,
        config_reloader: None,
        working_dir: Some(dir),
        skipped_steps: &[],
        on_step_log: None,
    }
}

#[test]
fn preflight_respects_real_exits_skips_and_resume_positions() {
    for (steps, entry, skipped, allowed) in [
        (
            "steps:\n  a:\n    command: 'true'\n    next: a\n",
            "a",
            vec![],
            false,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n    next: a\n",
            "a",
            vec!["a".into()],
            true,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n    next: b\n  b:\n    command: 'true'\n    next: a\n",
            "a",
            vec![],
            false,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n  b:\n    command: 'true'\n    if:\n      file-changed: a\n",
            "a",
            vec![],
            true,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n    next: a\n    skip: true\n",
            "a",
            vec![],
            true,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n    next: a\n    skip: prev.success\n",
            "a",
            vec![],
            true,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n    next: a\n    when:\n      exists: '*.rs'\n",
            "a",
            vec![],
            true,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n    next: a\n  finish:\n    command: 'true'\n",
            "finish",
            vec![],
            true,
        ),
        (
            "steps:\n  a:\n    command: 'true'\n    next: a\n    if:\n      no-file-changes: failed\n      file-changed: finish\n  finish:\n    command: 'true'\n",
            "a",
            vec![],
            false,
        ),
        (
            "steps:\n  choose:\n    option:\n      - selector: finish\n        next: finish\n      - selector: loop\n        next: trap\n  trap:\n    command: 'true'\n    next: trap\n  finish:\n    command: 'true'\n",
            "choose",
            vec![],
            true,
        ),
    ] {
        let compiled = workflow(steps);
        let graph = build_graph(&compiled, 3).unwrap();
        assert_eq!(
            validate_workflow(&compiled, &graph, entry, &skipped).is_ok(),
            allowed,
            "{steps}"
        );
    }
}

#[test]
fn preflight_does_not_treat_an_inert_group_budget_as_a_normal_exit() {
    let compiled = workflow(
        "groups:\n  loop-group:\n    max_retries: 1\n    steps:\n      loop:\n        command: 'true'\n        next: call/loop\nsteps:\n  call:\n    group: loop-group\n",
    );
    let graph = build_graph(&compiled, 3).unwrap();

    assert!(validate_workflow(&compiled, &graph, &graph.start, &[]).is_err());
}

#[tokio::test]
async fn empty_option_step_falls_through_to_the_next_step() {
    let dir = tempfile::tempdir().unwrap();
    let compiled =
        workflow("steps:\n  choose:\n    option: []\n  finish:\n    command: 'touch finished'\n");
    let mut graph = build_graph(&compiled, 3).unwrap();
    let ctx = context(&compiled, dir.path(), 3);

    let result = execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(dir.path().into()),
        &mut graph,
        &|_, _| Ok(()),
    )
    .await
    .unwrap();

    assert_eq!(result.run, 2);
    assert!(dir.path().join("finished").exists());
}

#[test]
fn large_chain_and_scc_use_fixed_topology() {
    let mut config =
        WorkflowConfig::from_yaml("command: [echo]\nsteps:\n  initial:\n    command: 'true'\n")
            .unwrap();
    config.steps = (0..10_000)
        .map(|i| {
            (
                format!("s{i}"),
                StepConfig {
                    command: Some(cruise::config::StringOrVec::Single("true".into())),
                    ..StepConfig::default()
                },
            )
        })
        .collect();
    let compiled = compile(config.clone()).unwrap();
    for max in [1, 3, 1_000_000] {
        let graph = build_graph(&compiled, max).unwrap();
        assert_eq!(graph.nodes.len(), 10_000);
        assert_eq!(
            graph
                .nodes
                .values()
                .map(|node| node.successors.len())
                .sum::<usize>(),
            20_000
        );
        validate_workflow(&compiled, &graph, &graph.start, &[]).unwrap();
    }
    config.steps.last_mut().unwrap().1.next = Some("s0".into());
    let compiled = compile(config).unwrap();
    let graph = build_graph(&compiled, 1_000_000).unwrap();
    assert!(validate_workflow(&compiled, &graph, &graph.start, &[]).is_err());
}

#[tokio::test]
async fn repeated_resume_never_replenishes_accepted_retry_budget() {
    let dir = tempfile::tempdir().unwrap();
    let compiled = workflow(
        "steps:\n  retry:\n    command: 'printf x >> visits; echo attempt >&2; exit 1'\n    if:\n      fail:\n        retry: true\n  finish:\n    command: 'true'\n",
    );
    let mut graph = build_graph(&compiled, 3).unwrap();
    let key = ("retry".into(), "retry".into());
    let path = dir.path().join("dag.json");
    let token = CancellationToken::new();
    let mut ctx = context(&compiled, dir.path(), 3);
    ctx.cancel_token = Some(&token);
    let mut vars = VariableStore::new(String::new());
    let mut tracker = FileTracker::with_root(dir.path().into());
    let result =
        execute_steps_with_graph(&ctx, &mut vars, &mut tracker, &mut graph, &|_, graph| {
            save_graph(graph, &path)?;
            if graph
                .state
                .edge_counts
                .get(&key)
                .is_some_and(|count| count.traversals == 2)
            {
                token.cancel();
            }
            Ok(())
        })
        .await;
    assert!(matches!(result, Err(CruiseError::Interrupted)));
    let saved = load_graph(&path).unwrap();
    assert_eq!(saved.state.edge_counts[&key].traversals, 2);
    assert_eq!(
        saved.state.runtime.prev_stderr.as_deref(),
        Some("attempt\n")
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("visits")).unwrap(),
        "xx"
    );

    // A fresh process with a lowered ceiling refuses the next retry and retains
    // both accepted counts and a pending transition, without guessing history.
    let mut resumed = build_graph(&compiled, 1).unwrap();
    resumed.restore(&saved).unwrap();
    let mut updated = compiled.clone();
    updated.model = Some("reloaded-model".into());
    let reload = || {
        Ok(Some(cruise::engine::ReloadedWorkflow {
            compiled: updated.clone(),
            retry_policy: None,
        }))
    };
    let mut ctx = context(&compiled, dir.path(), 1);
    ctx.config_reloader = Some(&reload);
    let mut vars = VariableStore::new(String::new());
    let mut tracker = FileTracker::with_root(dir.path().into());
    let result =
        execute_steps_with_graph(&ctx, &mut vars, &mut tracker, &mut resumed, &|_, graph| {
            save_graph(graph, &path)
        })
        .await;
    assert!(matches!(result, Err(CruiseError::LoopProtection { .. })));
    assert_eq!(resumed.state.edge_counts[&key].traversals, 2);
    assert!(resumed.state.pending.is_some());
    let visits = std::fs::read_to_string(dir.path().join("visits")).unwrap();
    for _ in 0..2 {
        let saved = load_graph(&path).unwrap();
        resumed.restore(&saved).unwrap();
        let result =
            execute_steps_with_graph(&ctx, &mut vars, &mut tracker, &mut resumed, &|_, graph| {
                save_graph(graph, &path)
            })
            .await;
        assert!(matches!(result, Err(CruiseError::LoopProtection { .. })));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("visits")).unwrap(),
            visits
        );
    }
    // Raising the ceiling accepts the pending transition exactly once.
    let ctx = context(&compiled, dir.path(), 3);
    let result =
        execute_steps_with_graph(&ctx, &mut vars, &mut tracker, &mut resumed, &|_, graph| {
            save_graph(graph, &path)
        })
        .await;
    assert!(matches!(result, Err(CruiseError::LoopProtection { .. })));
    assert_eq!(resumed.state.edge_counts[&key].traversals, 3);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("visits"))
            .unwrap()
            .len(),
        visits.len() + 1
    );
}

#[tokio::test]
async fn failed_checkpoint_prevents_the_next_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let compiled = workflow(
        "steps:\n  first:\n    command: touch first\n  second:\n    command: touch second\n",
    );
    let mut graph = build_graph(&compiled, 3).unwrap();
    let ctx = context(&compiled, dir.path(), 3);
    let result = execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(dir.path().into()),
        &mut graph,
        &|_, graph| {
            if graph.state.current.as_deref() == Some("second") {
                return Err(CruiseError::Other("checkpoint storage unavailable".into()));
            }
            Ok(())
        },
    )
    .await;
    assert!(result.is_err());
    assert!(dir.path().join("first").exists());
    assert!(!dir.path().join("second").exists());
}

#[tokio::test]
async fn skipped_transitions_are_recorded_without_consuming_budget_or_wrapping() {
    let dir = tempfile::tempdir().unwrap();
    let compiled = workflow(
        "steps:\n  first:\n    command: 'false'\n    skip: true\n    next: first\n  second:\n    command: 'true'\n",
    );
    let mut graph = build_graph(&compiled, 0).unwrap();
    let ctx = context(&compiled, dir.path(), 0);
    let result = execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(dir.path().into()),
        &mut graph,
        &|_, _| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!((result.run, result.skipped), (1, 1));
    let key = ("first".into(), "second".into());
    assert_eq!(
        graph.state.edge_counts[&key],
        EdgeCounter {
            traversals: 1,
            budgeted_traversals: 0
        }
    );
    graph.state.completed = false;
    graph.state.current = Some("first".into());
    graph.state.edge_counts.insert(
        key.clone(),
        EdgeCounter {
            traversals: usize::MAX,
            budgeted_traversals: 0,
        },
    );
    let result = execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(dir.path().into()),
        &mut graph,
        &|_, _| Ok(()),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(graph.state.edge_counts[&key].traversals, usize::MAX);
}

#[test]
fn checkpoint_outvotes_stale_session_position_and_preserves_invalid_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dag.json");
    let compiled =
        workflow("steps:\n  first:\n    command: 'true'\n  second:\n    command: 'true'\n");
    let mut saved = build_graph(&compiled, 3).unwrap();
    saved.state.current = Some("second".into());
    saved.state.execution_id = Some("execution".into());
    saved.state.edge_counts.insert(
        ("first".into(), "second".into()),
        EdgeCounter {
            traversals: 1,
            budgeted_traversals: 1,
        },
    );
    save_graph(&saved, &path).unwrap();
    let mut graph = build_graph(&compiled, 1).unwrap();
    assert_eq!(
        prepare_resume(
            &mut graph,
            &path,
            false,
            Some("first"),
            true,
            Some("execution")
        )
        .unwrap(),
        "second"
    );
    assert_eq!(graph.state.edge_counts, saved.state.edge_counts);
    let mut missing_position = saved.clone();
    missing_position.state.current = None;
    for value in [
        "{broken".to_string(),
        "{\"version\":999}".to_string(),
        "{\"version\":1}".to_string(),
        serde_json::to_string(&missing_position).unwrap(),
    ] {
        std::fs::write(&path, &value).unwrap();
        assert!(load_graph(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), value);
    }
    let mut incomplete = build_graph(&compiled, 3).unwrap();
    incomplete.state.current = None;
    incomplete.state.edge_counts.clear();
    incomplete.state.group_counts.clear();
    incomplete.state.execution_id = Some("execution".into());
    let incomplete_json = serde_json::to_string(&incomplete).unwrap();
    std::fs::write(&path, &incomplete_json).unwrap();
    assert!(load_graph(&path).is_ok());
    let mut resume_graph = build_graph(&compiled, 3).unwrap();
    let error = prepare_resume(
        &mut resume_graph,
        &path,
        false,
        None,
        false,
        Some("execution"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("incomplete execution checkpoint"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), incomplete_json);
    let mut legacy = serde_json::to_value(saved).unwrap();
    legacy.as_object_mut().unwrap().remove("version");
    legacy.as_object_mut().unwrap().remove("state");
    std::fs::write(&path, legacy.to_string()).unwrap();
    let legacy = load_graph(&path).unwrap();
    assert_eq!(legacy.version, 0);
    assert!(graph.restore(&legacy).is_err());
    assert!(path.exists());
}

#[test]
fn topology_only_checkpoint_starts_a_fresh_execution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dag.json");
    let compiled = workflow("steps:\n  first:\n    command: 'true'\n");
    let saved = build_graph(&compiled, 3).unwrap();
    assert!(saved.state.execution_id.is_none());
    save_graph(&saved, &path).unwrap();

    let mut graph = build_graph(&compiled, 3).unwrap();
    let entry = prepare_resume(&mut graph, &path, true, None, false, Some("execution")).unwrap();

    assert_eq!(entry, graph.start);
    assert!(graph.state.current.is_none());
    assert!(!graph.state.completed);
    assert_eq!(graph.state.execution_id.as_deref(), Some("execution"));
}

#[test]
fn incomplete_checkpoint_with_runtime_state_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dag.json");
    let compiled = workflow("steps:\n  first:\n    command: 'true'\n");
    let mut saved = build_graph(&compiled, 3).unwrap();
    saved.state.runtime.prev_output = Some("stale".into());
    save_graph(&saved, &path).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();

    assert!(load_graph(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
}

#[test]
fn invalid_pending_transition_is_rejected_without_modifying_the_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dag.json");
    let compiled =
        workflow("steps:\n  first:\n    command: 'true'\n  second:\n    command: 'true'\n");
    let mut saved = build_graph(&compiled, 3).unwrap();
    saved.state.current = Some("first".into());
    saved.state.pending = Some(Transition {
        target: Some("first".into()),
        reason: TransitionReason::IfFailRetry,
        budgeted: true,
        group_retry: None,
    });
    save_graph(&saved, &path).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();

    assert!(load_graph(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
}

#[test]
fn mismatched_checkpoint_is_not_silently_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dag.json");
    let compiled = workflow("steps:\n  first:\n    command: 'true'\n");
    let mut saved = build_graph(&compiled, 3).unwrap();
    saved.state.current = Some("first".into());
    saved.state.execution_id = Some("old-execution".into());
    save_graph(&saved, &path).unwrap();

    let mut graph = build_graph(&compiled, 3).unwrap();
    let error = prepare_resume(&mut graph, &path, false, None, false, Some("new-execution"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("different execution"), "{error}");

    let mut graph = build_graph(&compiled, 3).unwrap();
    let error = prepare_resume(&mut graph, &path, true, None, false, Some("new-execution"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("different execution"), "{error}");
}

#[test]
fn legacy_step_name_without_checkpoint_requires_explicit_restart() {
    let dir = tempfile::tempdir().unwrap();
    let compiled =
        workflow("steps:\n  first:\n    command: 'true'\n  second:\n    command: 'true'");
    let mut graph = build_graph(&compiled, 3).unwrap();

    let error = prepare_resume(
        &mut graph,
        &dir.path().join("dag.json"),
        false,
        Some("second"),
        false,
        None,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("missing execution checkpoint"), "{error}");
    assert!(error.contains("explicitly restart"), "{error}");
}

#[test]
fn active_checkpoint_rejects_control_structure_changes() {
    let old = workflow("steps:\n  first:\n    command: 'true'\n  second:\n    command: 'true'\n");
    let mut saved = build_graph(&old, 3).unwrap();
    saved.state.current = Some("second".into());
    saved.state.execution_id = Some("execution".into());

    let updated =
        workflow("steps:\n  first:\n    command: 'true'\n  replacement:\n    command: 'true'\n");
    let mut graph = build_graph(&updated, 3).unwrap();
    let error = graph.restore(&saved).unwrap_err().to_string();

    assert!(error.contains("control structure"), "{error}");
    assert!(error.contains("explicitly restart"), "{error}");
}

#[test]
fn graph_view_uses_a_checkpoint_when_session_display_state_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("workflow.yaml");
    std::fs::write(
        &config_path,
        "command: [echo]\nsteps:\n  first:\n    command: 'true'\n  second:\n    command: 'true'\n",
    )
    .unwrap();
    let manager = SessionManager::new(dir.path().join("data"));
    let session_id = "20260916000001";
    let mut session = SessionState::new(
        session_id.into(),
        dir.path().into(),
        config_path.to_string_lossy().into_owned(),
        String::new(),
    );
    session.phase = SessionPhase::Planned;
    session.config_path = Some(config_path.clone());
    manager.create(&session).unwrap();

    let compiled =
        workflow("steps:\n  first:\n    command: 'true'\n  second:\n    command: 'true'\n");
    let mut checkpoint = build_graph(&compiled, 3).unwrap();
    checkpoint.state.current = Some("second".into());
    checkpoint.state.edge_counts.insert(
        ("first".into(), "second".into()),
        EdgeCounter {
            traversals: 2,
            budgeted_traversals: 2,
        },
    );
    save_graph(&checkpoint, &manager.dag_path(session_id)).unwrap();

    let application = cruise::application::CruiseApplication::new(manager);
    let displayed = application.session_dag(session_id).unwrap().unwrap();
    assert_eq!(
        displayed.state.edge_counts[&("first".into(), "second".into())].traversals,
        2
    );
}

#[tokio::test]
async fn group_counts_and_snapshots_survive_resume_and_limit_skip() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let path = dir.path().join("dag.json");
    let compiled = workflow(
        "groups:\n  review:\n    max_retries: 2\n    if:\n      file-changed: pass\n    steps:\n      work:\n        command: printf x >> changes\nsteps:\n  pass:\n    group: review\n  finish:\n    command: touch finished\n",
    );
    let mut graph = build_graph(&compiled, 3).unwrap();
    validate_workflow(&compiled, &graph, &graph.start, &[]).unwrap();
    let token = CancellationToken::new();
    let mut ctx = context(&compiled, &repo, 3);
    ctx.cancel_token = Some(&token);
    let result = execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(repo.clone()),
        &mut graph,
        &|_, graph| {
            save_graph(graph, &path)?;
            if graph.state.group_counts.get("pass") == Some(&1) {
                token.cancel();
            }
            Ok(())
        },
    )
    .await;
    assert!(matches!(result, Err(CruiseError::Interrupted)));
    let saved = load_graph(&path).unwrap();
    assert_eq!(saved.state.group_counts["pass"], 1);
    assert!(
        saved
            .state
            .runtime
            .file_snapshots
            .contains_key("__group__pass")
    );
    let mut graph = build_graph(&compiled, 3).unwrap();
    graph.restore(&saved).unwrap();
    assert_eq!(
        graph.state.runtime.file_snapshots,
        saved.state.runtime.file_snapshots
    );
    let ctx = context(&compiled, &repo, 3);
    execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(repo.clone()),
        &mut graph,
        &|_, graph| save_graph(graph, &path),
    )
    .await
    .unwrap();
    assert_eq!(graph.state.group_counts["pass"], 2);
    assert_eq!(std::fs::read_to_string(repo.join("changes")).unwrap(), "xx");
    assert!(repo.join("finished").exists());
    assert_eq!(
        graph.state.edge_counts[&("pass/work".into(), "finish".into())],
        EdgeCounter {
            traversals: 1,
            budgeted_traversals: 0
        }
    );
    assert!(load_graph(&path).unwrap().state.completed);
}

#[test]
fn explicit_restart_archives_even_a_corrupt_checkpoint() {
    use cruise::session::{SessionManager, SessionState};
    let dir = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(dir.path().into());
    let mut session = SessionState::new(
        "restart".into(),
        dir.path().into(),
        "test".into(),
        String::new(),
    );
    manager.create(&session).unwrap();
    let path = manager.dag_path(&session.id);
    std::fs::write(&path, "corrupt checkpoint").unwrap();
    let previous_execution = session.execution_id.clone();
    session.reset_to_planned();
    manager.save(&session).unwrap();
    assert_ne!(previous_execution, session.execution_id);
    assert!(!path.exists());
    let archive = std::fs::read_dir(manager.sessions_dir().join(&session.id))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("dag.previous.")
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(archive).unwrap(),
        "corrupt checkpoint"
    );
}
