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
    execution_id: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let gh = bin.join("gh");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n",
                root.path().join("gh-invoked").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
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
        .unwrap();
        let config = root.path().join("workflow.yaml");
        let yaml = format!(
            "command: [echo]\ncreate_pr: true\ncleanup_after_pr: false\nsteps:\n  main:\n    command: 'touch {}'\nafter-pr:\n  earlier:\n    command: 'touch {}'\n  failing:\n    command: 'touch {}'\n",
            root.path().join("main-ran").display(),
            root.path().join("earlier-ran").display(),
            root.path().join("failing-ran").display()
        );
        std::fs::write(&config, &yaml).unwrap();
        let compiled =
            cruise::workflow::compile(cruise::config::WorkflowConfig::from_yaml(&yaml).unwrap())
                .unwrap();
        let mut state = SessionState::new(
            id.clone(),
            repo.clone(),
            SessionConfigRef::File { path: config },
            "retry".into(),
        );
        state.phase = SessionPhase::Failed("after-PR failure".into());
        state.worktree_path = Some(worktree.path);
        state.worktree_branch = Some(worktree.branch);
        state.current_step = Some("failing".into());
        state.has_dag = true;
        state.pr_url = Some("https://github.com/example/repo/pull/12".into());
        let execution_id = state.execution_id.clone().unwrap();
        manager.create(&state).unwrap();
        std::fs::write(
            state.plan_path(&manager.sessions_dir()),
            "# Plan\n\nImplement change.\n",
        )
        .unwrap();
        let mut graph = cruise::graph::build_graph(&compiled, 3).unwrap();
        graph.state.execution_id = Some(execution_id.clone());
        graph.state.completed = true;
        graph.state.current = None;
        cruise::graph::persistence::save_graph(&graph, &manager.dag_path(&id)).unwrap();
        // Write the stage through its on-disk representation so the regression
        // compiles against both the old and new session schemas.
        let state_path = manager.sessions_dir().join(&id).join("state.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        json["execution_stage"] = serde_json::json!("AfterPr");
        std::fs::write(state_path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
        let after = manager.sessions_dir().join(&id).join("after-pr-dag.json");
        Self {
            root,
            repo,
            manager,
            id,
            after,
            compiled,
            execution_id,
        }
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
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
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
        match kind {
            "missing" => {}
            "corrupt" => std::fs::write(&f.after, b"{not json").unwrap(),
            "wrong-execution" | "wrong-topology" => {
                let compiled = if kind == "wrong-topology" {
                    f.compiled.clone()
                } else {
                    f.compiled.to_after_pr_compiled()
                };
                let mut graph = cruise::graph::build_graph(&compiled, 3).unwrap();
                graph.state.execution_id = Some(if kind == "wrong-execution" {
                    "another-execution".into()
                } else {
                    f.execution_id.clone()
                });
                graph.state.current = Some(graph.start.clone());
                cruise::graph::persistence::save_graph(&graph, &f.after).unwrap();
            }
            _ => unreachable!(),
        }
        let original = std::fs::read(&f.after).ok();
        let main_before = std::fs::read(f.manager.dag_path(&f.id)).unwrap();
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
            std::fs::read(f.manager.dag_path(&f.id)).unwrap(),
            main_before,
            "{kind}: main checkpoint changed"
        );
    }
}

#[test]
fn after_pr_resume_runs_pending_node_without_repeating_earlier_nodes() {
    let f = Fixture::new();
    let mut graph = cruise::graph::build_graph(&f.compiled.to_after_pr_compiled(), 3).unwrap();
    graph.state.execution_id = Some(f.execution_id.clone());
    graph.state.current = Some("failing".into());
    cruise::graph::persistence::save_graph(&graph, &f.after).unwrap();
    let main_before = std::fs::read(f.manager.dag_path(&f.id)).unwrap();
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
    assert!(
        !f.root.path().join("gh-invoked").exists(),
        "resume contacted forge"
    );
    assert_eq!(
        std::fs::read(f.manager.dag_path(&f.id)).unwrap(),
        main_before
    );
    assert!(matches!(
        f.manager.load(&f.id).unwrap().phase,
        SessionPhase::Completed
    ));
}
