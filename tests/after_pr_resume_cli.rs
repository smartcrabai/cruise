#![cfg(unix)]

//! CLI regression contracts for an interrupted after-PR phase.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;

struct Fixture {
    root: tempfile::TempDir,
    repo: PathBuf,
    manager: SessionManager,
    id: String,
    after: PathBuf,
    compiled: cruise::workflow::CompiledWorkflow,
    execution_id: Option<String>,
}

impl Fixture {
    fn new() -> Self {
        Self::build(false)
    }

    fn legacy_main() -> Self {
        Self::build(true)
    }

    fn build(legacy_main: bool) -> Self {
        let root =
            tempfile::tempdir().unwrap_or_else(|error| panic!("create fixture root: {error}"));
        let repo = Self::setup_repo(&root, legacy_main);
        let manager = SessionManager::new(root.path().join("data/cruise"));
        let id = SessionManager::new_session_id();
        let (worktree, _) = cruise::worktree::setup_session_worktree(
            &repo,
            &id,
            "retry",
            &manager.worktrees_dir(),
            None,
            &cruise::worktree::WorktreeBase::default(),
        )
        .unwrap_or_else(|error| panic!("create fixture worktree: {error}"));
        let config = root.path().join("workflow.yaml");
        let yaml = r#"command: [echo]
create_pr: true
cleanup_after_pr: false
steps:
  main:
    command: 'printf x >> "$MAIN_EVENTS"; touch "$MAIN_RAN"; printf main-effect > main-effect.txt; printf main-output'
after-pr:
  earlier:
    command: 'printf x >> "$EARLIER_EVENTS"; touch "$EARLIER_RAN"; printf earlier-output'
  failing:
    github-review:
      reviewers: all
    prompt: unused review prompt
    timeout: "30"
  finish:
    command: 'printf x >> "$LATER_EVENTS"; test "{prev.success}" = true; printf later-output'
"#;
        std::fs::write(&config, yaml)
            .unwrap_or_else(|error| panic!("write fixture workflow: {error}"));
        let compiled = cruise::workflow::compile(
            cruise::config::WorkflowConfig::from_yaml(yaml)
                .unwrap_or_else(|error| panic!("parse fixture workflow: {error}")),
        )
        .unwrap_or_else(|error| panic!("compile fixture workflow: {error}"));
        let mut state = SessionState::new(
            id.clone(),
            repo.clone(),
            SessionConfigRef::File { path: config },
            "retry".into(),
        );
        state.phase = if legacy_main {
            SessionPhase::Planned
        } else {
            SessionPhase::Failed("after-PR failure".into())
        };
        state.worktree_path = Some(worktree.path);
        state.worktree_branch = Some(worktree.branch);
        state.current_step = (!legacy_main).then(|| "failing".into());
        state.has_dag = !legacy_main;
        if legacy_main {
            state.execution_id = None;
        } else {
            state.pr_url = Some("https://github.com/example/repo/pull/12".into());
        }
        manager
            .create(&state)
            .unwrap_or_else(|error| panic!("create fixture session: {error}"));
        std::fs::write(
            state.plan_path(&manager.sessions_dir()),
            "# Plan\n\nImplement change.\n",
        )
        .unwrap_or_else(|error| panic!("write fixture plan: {error}"));
        Self::save_fixture_state(&manager, &id, &state, &compiled, legacy_main);
        let after = manager.after_pr_dag_path(&id);
        Self {
            root,
            repo,
            manager,
            id,
            after,
            compiled,
            execution_id: state.execution_id,
        }
    }

    fn setup_repo(root: &tempfile::TempDir, legacy_main: bool) -> PathBuf {
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap_or_else(|error| panic!("create fixture bin: {error}"));
        let gh = bin.join("gh");
        std::fs::write(
            &gh,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nprintf '%s\\n' \"$*\" >> \"$CRUISE_TEST_GH_LOG\"\nif [ \"$1\" = pr ] && [ \"$2\" = create ]; then echo https://github.com/example/repo/pull/12; exit 0; fi\nif [ \"$1\" = pr ] && [ \"$2\" = view ]; then printf x >> \"$FAILURE_EVENTS\"; touch \"$FAILING_RAN\"; if [ ! -e \"$CRUISE_TEST_REVIEW_FAILURE\" ]; then touch \"$CRUISE_TEST_REVIEW_FAILURE\"; echo simulated review lookup failure >&2; exit 1; fi; git rev-parse HEAD; exit $?; fi\nif [ \"$1\" = api ] && [ \"$2\" = graphql ]; then printf '%s\\n' '{\"data\":{\"repository\":{\"pullRequest\":{\"reviewThreads\":{\"pageInfo\":{\"hasNextPage\":false,\"endCursor\":null},\"nodes\":[]}}}}}'; exit 0; fi\nexit 1\n",
        )
        .unwrap_or_else(|error| panic!("write isolated gh stub: {error}"));
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("make gh stub executable: {error}"));
        let review_failure = root.path().join("review-failed");
        if !legacy_main {
            std::fs::write(&review_failure, b"already failed")
                .unwrap_or_else(|error| panic!("seed successful review retry: {error}"));
        }
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|error| panic!("create fixture repo: {error}"));
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.test"]);
        let remote = root.path().join("remote.git");
        let remote_path = remote.to_string_lossy().into_owned();
        git(root.path(), &["init", "--bare", "-q", &remote_path]);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/repo.git",
            ],
        );
        git(&repo, &["config", "remote.origin.pushurl", &remote_path]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        repo
    }

    fn save_fixture_state(
        manager: &SessionManager,
        id: &str,
        state: &SessionState,
        compiled: &cruise::workflow::CompiledWorkflow,
        legacy_main: bool,
    ) {
        if !legacy_main {
            let mut graph = cruise::graph::build_graph(compiled, 3)
                .unwrap_or_else(|error| panic!("build fixture main graph: {error}"));
            graph.state.execution_id.clone_from(&state.execution_id);
            graph.state.completed = true;
            graph.state.current = None;
            cruise::graph::persistence::save_graph(&graph, &manager.dag_path(id))
                .unwrap_or_else(|error| panic!("save fixture main graph: {error}"));
        }
        // Exercise old serialized state, including a session with no execution ID.
        let state_path = manager.sessions_dir().join(id).join("state.json");
        let mut json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&state_path)
                .unwrap_or_else(|error| panic!("read fixture state: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse fixture state: {error}"));
        if legacy_main {
            json.as_object_mut()
                .unwrap_or_else(|| panic!("serialized session is not an object"))
                .remove("execution_id");
        } else {
            json["execution_stage"] = serde_json::json!("AfterPr");
        }
        std::fs::write(
            state_path,
            serde_json::to_vec_pretty(&json)
                .unwrap_or_else(|error| panic!("serialize fixture state: {error}")),
        )
        .unwrap_or_else(|error| panic!("save fixture state: {error}"));
    }

    fn run(&self) -> std::process::Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(name);
            }
        }
        cmd.args(["run", &self.id])
            .current_dir(&self.repo)
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.path().join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("CRUISE_SKIP_AGENT_AUTH_CHECK", "1")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("CRUISE_FORGE", "github")
            .env("CRUISE_TEST_GH_LOG", self.root.path().join("gh-invoked"))
            .env(
                "CRUISE_TEST_REVIEW_FAILURE",
                self.root.path().join("review-failed"),
            )
            .env("MAIN_EVENTS", self.root.path().join("main-events"))
            .env("MAIN_RAN", self.root.path().join("main-ran"))
            .env("EARLIER_EVENTS", self.root.path().join("earlier-events"))
            .env("EARLIER_RAN", self.root.path().join("earlier-ran"))
            .env("FAILURE_EVENTS", self.root.path().join("failure-events"))
            .env("FAILING_RAN", self.root.path().join("failing-ran"))
            .env("LATER_EVENTS", self.root.path().join("later-events"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|error| panic!("run cruise fixture: {error}"))
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run git {args:?}: {error}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn after_pr_resume_rejects_missing_corrupt_and_incompatible_checkpoints_without_effects() {
    for kind in ["missing", "corrupt", "wrong-execution", "wrong-topology"] {
        let f = Fixture::new();
        let execution_id = f
            .execution_id
            .as_ref()
            .unwrap_or_else(|| panic!("staged fixture has no execution ID"));
        match kind {
            "missing" => {}
            "corrupt" => std::fs::write(&f.after, b"{not json")
                .unwrap_or_else(|error| panic!("write corrupt after-PR checkpoint: {error}")),
            "wrong-execution" | "wrong-topology" => {
                let compiled = if kind == "wrong-topology" {
                    f.compiled.clone()
                } else {
                    f.compiled.to_after_pr_compiled()
                };
                let mut graph = cruise::graph::build_graph(&compiled, 3)
                    .unwrap_or_else(|error| panic!("build incompatible graph: {error}"));
                graph.state.execution_id = Some(if kind == "wrong-execution" {
                    "another-execution".into()
                } else {
                    execution_id.clone()
                });
                graph.state.current = Some(graph.start.clone());
                cruise::graph::persistence::save_graph(&graph, &f.after)
                    .unwrap_or_else(|error| panic!("save incompatible graph: {error}"));
            }
            _ => panic!("unexpected checkpoint fixture: {kind}"),
        }
        let original = std::fs::read(&f.after).ok();
        let main_path = f.manager.dag_path(&f.id);
        let main_before = std::fs::read(&main_path)
            .unwrap_or_else(|error| panic!("read main checkpoint: {error}"));
        let output = f.run();
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.status.success(),
            "{kind} checkpoint accepted: {message}"
        );
        assert!(
            message.to_lowercase().contains("checkpoint")
                || message.to_lowercase().contains("graph"),
            "{kind}: unrelated error: {message}"
        );
        for marker in ["main-ran", "earlier-ran", "failing-ran"] {
            assert!(!f.root.path().join(marker).exists(), "{kind}: ran {marker}");
        }
        assert!(
            !f.root.path().join("gh-invoked").exists(),
            "{kind}: contacted forge"
        );
        assert_eq!(
            std::fs::read(&f.after).ok(),
            original,
            "{kind}: checkpoint changed"
        );
        assert_eq!(
            std::fs::read(main_path)
                .unwrap_or_else(|error| panic!("reread main checkpoint: {error}")),
            main_before,
            "{kind}: main checkpoint changed"
        );
    }
}

#[test]
fn after_pr_resume_runs_pending_node_without_repeating_earlier_nodes() {
    let f = Fixture::new();
    let mut graph = cruise::graph::build_graph(&f.compiled.to_after_pr_compiled(), 3)
        .unwrap_or_else(|error| panic!("build after-PR graph: {error}"));
    graph.state.execution_id = f.execution_id.clone();
    graph.state.current = Some("failing".into());
    graph.state.edge_counts.insert(
        ("earlier".into(), "failing".into()),
        cruise::graph::EdgeCounter {
            traversals: 1,
            budgeted_traversals: 1,
        },
    );
    graph.state.runtime.prev_success = Some(true);
    cruise::graph::persistence::save_graph(&graph, &f.after)
        .unwrap_or_else(|error| panic!("save after-PR graph: {error}"));
    let main_path = f.manager.dag_path(&f.id);
    let main_before =
        std::fs::read(&main_path).unwrap_or_else(|error| panic!("read main checkpoint: {error}"));
    let output = f.run();
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "resume failed: {message}");
    assert!(
        !f.root.path().join("main-ran").exists(),
        "main workflow repeated"
    );
    assert!(
        !f.root.path().join("earlier-ran").exists(),
        "earlier after-PR step repeated"
    );
    assert!(
        f.root.path().join("failing-ran").exists(),
        "pending step did not run"
    );
    let calls = std::fs::read_to_string(f.root.path().join("gh-invoked"))
        .unwrap_or_else(|error| panic!("read fake forge calls: {error}"));
    assert!(
        !calls.lines().any(|line| line.starts_with("pr create ")),
        "resume republished the PR: {calls}"
    );
    assert!(
        calls.lines().any(|line| line.starts_with("pr view ")),
        "pending GitHub review did not query the existing PR: {calls}"
    );
    assert_eq!(
        std::fs::read(main_path).unwrap_or_else(|error| panic!("reread main checkpoint: {error}")),
        main_before
    );
    assert!(matches!(
        f.manager
            .load(&f.id)
            .unwrap_or_else(|error| panic!("reload session: {error}"))
            .phase,
        SessionPhase::Completed
    ));
}

type BudgetedEdge = ((String, String), cruise::graph::EdgeCounter);

fn assert_failed_after_pr_checkpoint(f: &Fixture) -> (String, BudgetedEdge) {
    let failed = f
        .manager
        .load(&f.id)
        .unwrap_or_else(|error| panic!("reload failed session: {error}"));
    assert!(matches!(&failed.phase, SessionPhase::Failed(_)));
    assert_eq!(
        failed.execution_stage,
        cruise::session::SessionExecutionStage::AfterPr
    );
    assert_eq!(failed.current_step.as_deref(), Some("failing"));
    let execution_id = failed
        .execution_id
        .clone()
        .unwrap_or_else(|| panic!("after-PR entry did not persist an execution ID"));
    let failed_graph = cruise::graph::persistence::load_graph(&f.after)
        .unwrap_or_else(|error| panic!("load failed after-PR graph: {error}"));
    assert_eq!(
        failed_graph.state.execution_id.as_deref(),
        Some(execution_id.as_str())
    );
    assert_eq!(failed_graph.state.runtime.prev_output, None);
    assert_eq!(failed_graph.state.runtime.prev_success, Some(true));
    let budgeted_edge = failed_graph
        .state
        .edge_counts
        .iter()
        .find(|((from, to), count)| {
            from == "earlier" && to == "failing" && count.budgeted_traversals > 0
        })
        .map_or_else(
            || panic!("earlier-to-failing retry budget was not checkpointed"),
            |(edge, count)| (edge.clone(), count.clone()),
        );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("main-events"))
            .unwrap_or_else(|error| panic!("read main effects: {error}")),
        "x"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("earlier-events"))
            .unwrap_or_else(|error| panic!("read earlier after-PR effects: {error}")),
        "x"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("failure-events"))
            .unwrap_or_else(|error| panic!("read failed-step effects: {error}")),
        "x"
    );
    let calls = std::fs::read_to_string(f.root.path().join("gh-invoked"))
        .unwrap_or_else(|error| panic!("read fake forge calls: {error}"));
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("pr create "))
            .count(),
        1,
        "initial run did not publish exactly once: {calls}"
    );
    (execution_id, budgeted_edge)
}

fn assert_resumed_after_pr(f: &Fixture, execution_id: &str, budgeted_edge: &BudgetedEdge) {
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("main-events"))
            .unwrap_or_else(|error| panic!("reread main effects: {error}")),
        "x",
        "main phase repeated"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("earlier-events"))
            .unwrap_or_else(|error| panic!("reread earlier after-PR effects: {error}")),
        "x",
        "completed after-PR step repeated"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("failure-events"))
            .unwrap_or_else(|error| panic!("reread failed-step effects: {error}")),
        "xx",
        "failed after-PR step did not resume from its runtime checkpoint"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("later-events"))
            .unwrap_or_else(|error| panic!("read later after-PR effects: {error}")),
        "x",
        "later after-PR step did not run"
    );
    let calls = std::fs::read_to_string(f.root.path().join("gh-invoked"))
        .unwrap_or_else(|error| panic!("reread fake forge calls: {error}"));
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("pr create "))
            .count(),
        1,
        "resume republished the PR: {calls}"
    );
    let completed = f
        .manager
        .load(&f.id)
        .unwrap_or_else(|error| panic!("reload completed session: {error}"));
    assert!(matches!(&completed.phase, SessionPhase::Completed));
    assert_eq!(completed.execution_id.as_deref(), Some(execution_id));
    let completed_graph = cruise::graph::persistence::load_graph(&f.after)
        .unwrap_or_else(|error| panic!("reload completed after-PR graph: {error}"));
    assert_eq!(
        completed_graph.state.edge_counts.get(&budgeted_edge.0),
        Some(&budgeted_edge.1),
        "resume discarded the earlier-to-failing retry budget"
    );
}

#[test]
fn legacy_missing_identity_failure_resumes_after_pr_without_replaying_effects() {
    let f = Fixture::legacy_main();
    let initial = f
        .manager
        .load(&f.id)
        .unwrap_or_else(|error| panic!("load legacy session: {error}"));
    assert!(
        initial.execution_id.is_none(),
        "fixture retained its legacy ID"
    );

    let first = f.run();
    let first_message = format!(
        "{}{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        !first.status.success(),
        "first after-PR failure unexpectedly succeeded: {first_message}"
    );
    let (execution_id, budgeted_edge) = assert_failed_after_pr_checkpoint(&f);

    let resumed = f.run();
    let resumed_message = format!(
        "{}{}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert!(
        resumed.status.success(),
        "after-PR resume failed: {resumed_message}"
    );
    assert_resumed_after_pr(&f, &execution_id, &budgeted_edge);
}
