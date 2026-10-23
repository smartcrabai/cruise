//! Native Cucumber contracts reconstructed from `graph_state.rs` evidence.
//! IDs are paired property references, not recovered historical audit labels.
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]

#[cfg(unix)]
mod contracts {
    use std::path::Path;

    use cruise::cancellation::CancellationToken;
    use cruise::config::WorkflowConfig;
    use cruise::engine::{ExecutionContext, ExecutionResult, execute_steps_with_graph};
    use cruise::error::{CruiseError, Result};
    use cruise::file_tracker::FileTracker;
    use cruise::graph::persistence::{load_graph, prepare_resume, save_graph};
    use cruise::graph::{EdgeCounter, ExecutionGraph, Transition, TransitionReason, build_graph};
    use cruise::option_handler::OptionHandler;
    use cruise::variable::VariableStore;
    use cruise::workflow::{CompiledWorkflow, compile};
    use cucumber::{World as _, given, then, when};

    struct NoOpOptionHandler;
    impl OptionHandler for NoOpOptionHandler {
        fn select_option(
            &self,
            _: &[cruise::step::OptionChoice],
            _: Option<&str>,
        ) -> Result<cruise::step::option::OptionResult> {
            panic!("unexpected option interaction");
        }
    }

    #[derive(Default, cucumber::World)]
    #[world(init = Self::new)]
    pub struct CheckpointWorld {
        dir: Option<tempfile::TempDir>,
        compiled: Option<CompiledWorkflow>,
    }

    impl std::fmt::Debug for CheckpointWorld {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("CheckpointWorld")
                .field("dir", &self.dir)
                .finish_non_exhaustive()
        }
    }

    impl CheckpointWorld {
        fn new() -> Self {
            Self::default()
        }
        fn dir(&self) -> &Path {
            self.dir.as_ref().unwrap().path()
        }
        fn compiled(&self) -> &CompiledWorkflow {
            self.compiled.as_ref().unwrap()
        }
        fn graph(&self, max: usize) -> ExecutionGraph {
            build_graph(self.compiled(), max).unwrap()
        }
        fn contents(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir().join(name)).unwrap()
        }
    }

    fn context<'a>(
        compiled: &'a CompiledWorkflow,
        dir: &'a Path,
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
            flags: &cruise::flags::NO_FLAGS,
            on_step_log: None,
        }
    }

    // Every invocation uses fresh runtime objects, as a new process would.
    async fn run(
        w: &CheckpointWorld,
        graph: &mut ExecutionGraph,
        max: usize,
    ) -> Result<ExecutionResult> {
        let path = w.dir().join("dag.json");
        execute_steps_with_graph(
            &context(w.compiled(), w.dir(), max),
            &mut VariableStore::new(String::new()),
            &mut FileTracker::with_root(w.dir().into()),
            graph,
            &|_, graph| save_graph(graph, &path),
        )
        .await
    }

    #[given("an isolated checkpoint workspace")]
    fn isolated(w: &mut CheckpointWorld) {
        w.dir = Some(tempfile::tempdir().unwrap());
    }

    #[when(regex = r"^I compile the (retrying|two-effect|skipped|grouped) shell workflow$")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber regex captures are parsed as owned strings"
    )]
    fn workflow(w: &mut CheckpointWorld, kind: String) {
        let yaml = match kind.as_str() {
            "retrying" => {
                "steps:\n  retry:\n    command: 'printf x >> visits; echo attempt >&2; exit 1'\n    if:\n      fail:\n        retry: true\n  finish:\n    command: 'true'\n"
            }
            "two-effect" => {
                "steps:\n  first:\n    command: 'printf x >> first; printf authoritative >&2'\n  second:\n    command: 'printf x >> second; printf \"%s\" \"{prev.stderr}\" > observed'\n"
            }
            "skipped" => {
                "steps:\n  first:\n    command: 'touch forbidden'\n    skip: true\n    next: first\n  second:\n    command: 'touch second'\n"
            }
            "grouped" => {
                "groups:\n  review:\n    max_retries: 2\n    if:\n      file-changed: pass\n    steps:\n      work:\n        command: printf x >> changes\nsteps:\n  pass:\n    group: review\n  finish:\n    command: touch finished\n"
            }
            _ => unreachable!(),
        };
        w.compiled = Some(
            compile(WorkflowConfig::from_yaml(&format!("command: [echo]\n{yaml}")).unwrap())
                .unwrap(),
        );
    }

    #[then("accepted counts survive repeated resumes and lowered ceilings")]
    async fn accepted_counts(w: &mut CheckpointWorld) {
        let path = w.dir().join("dag.json");
        let key = ("retry".into(), "retry".into());
        let mut graph = w.graph(2);
        assert!(matches!(
            run(w, &mut graph, 2).await,
            Err(CruiseError::LoopProtection { .. })
        ));
        assert_eq!(w.contents("visits"), "xxx");
        assert_eq!(
            graph.state.edge_counts[&key],
            EdgeCounter {
                traversals: 2,
                budgeted_traversals: 2
            }
        );
        for _ in 0..3 {
            let saved = load_graph(&path).unwrap();
            let mut resumed = w.graph(1);
            resumed.restore(&saved).unwrap();
            assert!(matches!(
                run(w, &mut resumed, 1).await,
                Err(CruiseError::LoopProtection { .. })
            ));
            assert_eq!(resumed.state.edge_counts, graph.state.edge_counts);
            assert_eq!(resumed.state.pending, graph.state.pending);
            assert_eq!(w.contents("visits"), "xxx");
        }
    }

    #[then("a pending retry resumes without rerunning its source effect")]
    async fn pending(w: &mut CheckpointWorld) {
        let path = w.dir().join("dag.json");
        let mut graph = w.graph(0);
        assert!(matches!(
            run(w, &mut graph, 0).await,
            Err(CruiseError::LoopProtection { .. })
        ));
        let saved = load_graph(&path).unwrap();
        assert_eq!(saved.state.current.as_deref(), Some("retry"));
        assert_eq!(
            saved.state.pending.as_ref().unwrap().reason,
            TransitionReason::IfFailRetry
        );
        assert_eq!(
            saved.state.runtime.prev_stderr.as_deref(),
            Some("attempt\n")
        );
        assert_eq!(w.contents("visits"), "x");
        let mut resumed = w.graph(1);
        resumed.restore(&saved).unwrap();
        let token = CancellationToken::new();
        let mut ctx = context(w.compiled(), w.dir(), 1);
        ctx.cancel_token = Some(&token);
        let result = execute_steps_with_graph(
            &ctx,
            &mut VariableStore::new(String::new()),
            &mut FileTracker::with_root(w.dir().into()),
            &mut resumed,
            &|_, graph| {
                save_graph(graph, &path)?;
                if graph
                    .state
                    .edge_counts
                    .get(&("retry".into(), "retry".into()))
                    .is_some_and(|c| c.traversals == 1)
                {
                    token.cancel();
                }
                Ok(())
            },
        )
        .await;
        assert!(matches!(result, Err(CruiseError::Interrupted)));
        let accepted = load_graph(&path).unwrap();
        assert!(accepted.state.pending.is_none());
        assert_eq!(
            accepted.state.edge_counts[&("retry".into(), "retry".into())].traversals,
            1
        );
        assert_eq!(accepted.state.runtime, saved.state.runtime);
        assert_eq!(
            w.contents("visits"),
            "x",
            "accepting pending must not execute its source"
        );
    }

    #[then("a failed checkpoint prevents the second effect")]
    async fn checkpoint_gate(w: &mut CheckpointWorld) {
        let mut graph = w.graph(3);
        let path = w.dir().join("dag.json");
        // Real persistence failure: a file cannot be the checkpoint's parent directory.
        let blocked = w.dir().join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        let result = execute_steps_with_graph(
            &context(w.compiled(), w.dir(), 3),
            &mut VariableStore::new(String::new()),
            &mut FileTracker::with_root(w.dir().into()),
            &mut graph,
            &|_, graph| {
                if graph.state.current.as_deref() == Some("second") {
                    save_graph(graph, &blocked.join("dag.json"))
                } else {
                    save_graph(graph, &path)
                }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(w.contents("first"), "x");
        assert!(!w.dir().join("second").exists());
        assert_eq!(
            load_graph(&path).unwrap().state.current.as_deref(),
            Some("first")
        );
        // Deliberately no claim that replay after an effect-before-save crash is exactly once.
    }

    #[then("the skipped traversal is counted without budget or overflow")]
    async fn skipped(w: &mut CheckpointWorld) {
        let mut graph = w.graph(0);
        let result = run(w, &mut graph, 0).await.unwrap();
        assert_eq!((result.run, result.skipped), (1, 1));
        assert!(!w.dir().join("forbidden").exists());
        assert!(w.dir().join("second").exists());
        let key = ("first".into(), "second".into());
        assert_eq!(
            load_graph(&w.dir().join("dag.json"))
                .unwrap()
                .state
                .edge_counts[&key],
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
        assert!(run(w, &mut graph, 0).await.is_err());
        assert_eq!(graph.state.edge_counts[&key].traversals, usize::MAX);
    }

    #[then("checkpoint authority and explicit reposition preserve history")]
    async fn authority(w: &mut CheckpointWorld) {
        let path = w.dir().join("dag.json");
        let mut graph = w.graph(3);
        graph.state.execution_id = Some("execution".into());
        let token = CancellationToken::new();
        let mut ctx = context(w.compiled(), w.dir(), 3);
        ctx.cancel_token = Some(&token);
        let result = execute_steps_with_graph(
            &ctx,
            &mut VariableStore::new(String::new()),
            &mut FileTracker::with_root(w.dir().into()),
            &mut graph,
            &|_, graph| {
                save_graph(graph, &path)?;
                if graph.state.current.as_deref() == Some("second") {
                    token.cancel();
                }
                Ok(())
            },
        )
        .await;
        assert!(matches!(result, Err(CruiseError::Interrupted)));
        let saved = load_graph(&path).unwrap();
        let mut resumed = w.graph(1);
        assert_eq!(
            prepare_resume(
                &mut resumed,
                &path,
                false,
                Some("first"),
                true,
                Some("execution")
            )
            .unwrap(),
            "second"
        );
        assert_eq!(resumed.state.runtime, saved.state.runtime);
        assert_eq!(
            resumed.state.runtime.prev_stderr.as_deref(),
            Some("authoritative")
        );
        assert_eq!(resumed.state.edge_counts, saved.state.edge_counts);
        let mut repositioned = w.graph(1);
        assert_eq!(
            prepare_resume(
                &mut repositioned,
                &path,
                true,
                Some("first"),
                false,
                Some("execution")
            )
            .unwrap(),
            "first"
        );
        assert_eq!(repositioned.state.edge_counts, saved.state.edge_counts);
        assert_eq!(repositioned.state.runtime, saved.state.runtime);
        // The display session can still say Planned while the checkpoint is active.
        let config_path = w.dir().join("workflow.yaml");
        std::fs::write(&config_path, "command: [echo]\nsteps:\n  first:\n    command: 'true'\n  second:\n    command: 'true'\n").unwrap();
        let manager = cruise::session::SessionManager::new(w.dir().join("sessions"));
        let session = cruise::session::SessionState::new(
            "20260916000001".into(),
            w.dir().into(),
            cruise::session_config::SessionConfigRef::File { path: config_path },
            String::new(),
        );
        manager.create(&session).unwrap();
        save_graph(&saved, &manager.dag_path(&session.id)).unwrap();
        let application = cruise::application::CruiseApplication::new(manager);
        let displayed = application.session_dag(&session.id).unwrap().unwrap();
        assert_eq!(displayed.state, saved.state);
        assert!(repositioned.state.pending.is_none());
        assert!(!repositioned.state.completed);
        let mut stale_vars = VariableStore::new(String::new());
        stale_vars.set_prev_stderr(Some("stale caller runtime".into()));
        stale_vars.set_prev_success(Some(false));
        execute_steps_with_graph(
            &context(w.compiled(), w.dir(), 1),
            &mut stale_vars,
            &mut FileTracker::with_root(w.dir().into()),
            &mut resumed,
            &|_, graph| save_graph(graph, &path),
        )
        .await
        .unwrap();
        assert_eq!(w.contents("observed"), "authoritative");
        assert_eq!(w.contents("first"), "x");
        assert_eq!(w.contents("second"), "x");
    }

    #[then("invalid legacy mismatched and incomplete checkpoints preserve their bytes")]
    fn rejection(w: &mut CheckpointWorld) {
        let path = w.dir().join("dag.json");
        let mut saved = w.graph(3);
        saved.state.current = Some("first".into());
        saved.state.execution_id = Some("execution".into());
        let mut legacy = serde_json::to_value(&saved).unwrap();
        legacy.as_object_mut().unwrap().remove("version");
        legacy.as_object_mut().unwrap().remove("state");
        let mut incomplete = saved.clone();
        incomplete.state.current = None;
        let mut invalid_runtime = incomplete.clone();
        invalid_runtime.state.runtime.prev_output = Some("stale".into());
        let mut invalid_pending = saved.clone();
        invalid_pending.state.pending = Some(Transition {
            target: Some("first".into()),
            reason: TransitionReason::IfFailRetry,
            budgeted: true,
            group_retry: None,
        });
        let cases = [
            ("malformed", "{broken".to_owned(), Some("execution")),
            (
                "unknown version",
                "{\"version\":999}".to_owned(),
                Some("execution"),
            ),
            (
                "missing state",
                "{\"version\":1}".to_owned(),
                Some("execution"),
            ),
            ("legacy", legacy.to_string(), Some("execution")),
            (
                "incomplete",
                serde_json::to_string(&incomplete).unwrap(),
                Some("execution"),
            ),
            (
                "invalid runtime",
                serde_json::to_string(&invalid_runtime).unwrap(),
                Some("execution"),
            ),
            (
                "invalid pending",
                serde_json::to_string(&invalid_pending).unwrap(),
                Some("execution"),
            ),
            (
                "different execution",
                serde_json::to_string(&saved).unwrap(),
                Some("other"),
            ),
        ];
        for (label, bytes, execution) in cases {
            std::fs::write(&path, &bytes).unwrap();
            let mut graph = w.graph(3);
            assert!(
                prepare_resume(&mut graph, &path, true, None, false, execution).is_err(),
                "{label}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes(), "{label}");
        }
        save_graph(&saved, &path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let changed = compile(
            WorkflowConfig::from_yaml(
                "command: [echo]\nsteps:\n  replacement:\n    command: 'true'\n",
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            prepare_resume(
                &mut build_graph(&changed, 3).unwrap(),
                &path,
                true,
                None,
                false,
                Some("execution")
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!w.dir().join("first").exists());
        assert!(!w.dir().join("second").exists());
    }

    #[then("group counts and runtime snapshots survive resume and exhaustion")]
    async fn groups(w: &mut CheckpointWorld) {
        // Keep checkpoint storage outside the tracked shell workspace.
        let repo = w.dir().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let path = w.dir().join("dag.json");
        let mut graph = w.graph(3);
        let token = CancellationToken::new();
        let mut ctx = context(w.compiled(), &repo, 3);
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
        let mut resumed = w.graph(3);
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.state.runtime, saved.state.runtime);
        assert_eq!(resumed.state.group_counts, saved.state.group_counts);
        let result = execute_steps_with_graph(
            &context(w.compiled(), &repo, 3),
            &mut VariableStore::new(String::new()),
            &mut FileTracker::with_root(repo.clone()),
            &mut resumed,
            &|_, graph| save_graph(graph, &path),
        )
        .await
        .unwrap();
        assert_eq!(result.skipped, 1);
        assert_eq!(resumed.state.group_counts["pass"], 2);
        assert_eq!(std::fs::read_to_string(repo.join("changes")).unwrap(), "xx");
        assert!(repo.join("finished").exists());
        assert_eq!(
            resumed.state.edge_counts[&("pass/work".into(), "finish".into())],
            EdgeCounter {
                traversals: 1,
                budgeted_traversals: 0
            }
        );
        let completed = load_graph(&path).unwrap();
        assert!(completed.state.completed);
        assert_eq!(completed.state.group_counts, resumed.state.group_counts);
        assert_eq!(
            completed.state.runtime.file_snapshots,
            resumed.state.runtime.file_snapshots
        );
    }

    #[then("topology stays stable and completed resume runs no effects")]
    async fn completed(w: &mut CheckpointWorld) {
        let topology = w.graph(1);
        let cyclic = compile(WorkflowConfig::from_yaml("command: [echo]\nsteps:\n  loop:\n    command: 'true'\n    next: loop\n  finish:\n    command: 'true'\n").unwrap()).unwrap();
        let cyclic_topology = build_graph(&cyclic, 1).unwrap();
        for max in [0, 3, 1_000_000] {
            let cyclic_graph = build_graph(&cyclic, max).unwrap();
            assert_eq!(cyclic_graph.nodes.len(), 2);
            assert_eq!(cyclic_graph.nodes, cyclic_topology.nodes);
            assert_eq!(cyclic_graph.predecessors, cyclic_topology.predecessors);
            let graph = w.graph(max);
            assert_eq!(graph.nodes, topology.nodes);
            assert_eq!(graph.predecessors, topology.predecessors);
            assert_eq!(graph.start, topology.start);
        }
        let mut graph = w.graph(3);
        run(w, &mut graph, 3).await.unwrap();
        assert!(graph.state.completed);
        let path = w.dir().join("dag.json");
        let bytes = std::fs::read(&path).unwrap();
        let saved = load_graph(&path).unwrap();
        let mut resumed = w.graph(0);
        prepare_resume(&mut resumed, &path, true, Some("first"), true, None).unwrap();
        let result = run(w, &mut resumed, 0).await.unwrap();
        assert_eq!((result.run, result.skipped, result.failed), (0, 0, 0));
        assert_eq!(resumed.state, saved.state);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(w.contents("first"), "x");
        assert_eq!(w.contents("second"), "x");
        for (id, node) in &resumed.nodes {
            assert_eq!(node.successors, topology.nodes[id].successors);
        }
    }

    pub async fn main() {
        CheckpointWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run_and_exit(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/checkpoint.feature"
            ))
            .await;
    }
}

#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::main().await;
}

// Production shell fixtures are POSIX-only, like tests/graph_state.rs.
// Keep harness=false usable by cargo test --all-features on Windows and other hosts.
#[cfg(not(unix))]
fn main() {
    println!("checkpoint_bdd: POSIX shell contracts are only run on Unix");
}
