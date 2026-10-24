#![cfg(unix)]

//! Execution entrypoint contracts using child-only credentials and a non-repository cwd.
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Output, Stdio};

use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use cruise::session_config::SessionConfigRef;
use tempfile::TempDir;

const CONFIG: &str =
    "sdk: jcode\nmodel: claude/test-model\nsteps:\n  agent:\n    prompt: never invoke an agent\n";
const SESSION_ID: &str = "20261024000002";

fn init_repo(path: &std::path::Path) {
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["commit", "--allow-empty", "-m", "fixture"],
    ] {
        let output = Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(output.status.success(), "{}", text(&output));
    }
}

fn write_script(path: &std::path::Path, script: &str) {
    fs::write(path, script).unwrap_or_else(|e| panic!("{e}"));
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap_or_else(|e| panic!("{e}"));
}

struct Fixture {
    root: TempDir,
    manager: SessionManager,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let project = root.path().join("not-a-repository");
        fs::create_dir_all(&project).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let auth_home = root.path().join("jcode");
        fs::create_dir_all(&auth_home).unwrap_or_else(|e| panic!("mkdir: {e}"));
        fs::write(
            auth_home.join("auth.json"),
            r#"{"anthropic_accounts":[{"access":"expired-cli-test-token","expires":1}]}"#,
        )
        .unwrap_or_else(|e| panic!("credentials: {e}"));
        fs::write(root.path().join("workflow.yaml"), CONFIG)
            .unwrap_or_else(|e| panic!("workflow: {e}"));
        let manager = SessionManager::new(root.path().join("data/cruise"));
        let mut session = SessionState::new(
            SESSION_ID.to_string(),
            project,
            SessionConfigRef::BuiltinSnapshot { name: None },
            "auth CLI wiring".to_string(),
        );
        session.phase = SessionPhase::Planned;
        session.workspace_mode = WorkspaceMode::Worktree;
        manager
            .create(&session)
            .unwrap_or_else(|e| panic!("session: {e}"));
        let session_dir = manager.sessions_dir().join(SESSION_ID);
        fs::write(session_dir.join("config.yaml"), CONFIG)
            .unwrap_or_else(|e| panic!("config: {e}"));
        fs::write(session_dir.join("plan.md"), "# Auth wiring plan\n")
            .unwrap_or_else(|e| panic!("plan: {e}"));
        Self { root, manager }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.starts_with("CRUISE_")
                || name.starts_with("JCODE_")
                || name.starts_with("OTEL_")
                || name.starts_with("CLAUDE_")
                || name.starts_with("ANTHROPIC_")
            {
                command.env_remove(name.as_ref());
            }
        }
        command
            .current_dir(self.root.path().join("not-a-repository"))
            .env("HOME", self.root.path().join("home"))
            .env("USERPROFILE", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("JCODE_HOME", self.root.path().join("jcode"))
            .env("CRUISE_DISABLE_HERDR", "1")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null());
        command
    }

    fn state(&self) -> Vec<u8> {
        fs::read(
            self.manager
                .sessions_dir()
                .join(SESSION_ID)
                .join("state.json"),
        )
        .unwrap_or_else(|e| panic!("state: {e}"))
    }

    fn assert_no_workspace(&self) {
        assert!(!self.manager.worktrees_dir().exists());
        assert!(!self.manager.clones_dir().exists());
        assert_eq!(
            fs::read_dir(self.root.path().join("not-a-repository"))
                .unwrap_or_else(|e| panic!("cwd: {e}"))
                .count(),
            0
        );
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn resumed_run_resolves_relative_auth_home_in_selected_workspace() {
    for mode in [WorkspaceMode::Worktree, WorkspaceMode::CurrentBranch] {
        for selected_expired in [false, true] {
            let fixture = Fixture::new();
            let mut session = fixture
                .manager
                .load(SESSION_ID)
                .unwrap_or_else(|e| panic!("{e}"));
            session.phase = SessionPhase::Suspended;
            session.current_step = Some("agent".into());
            session.has_dag = true;
            session.workspace_mode = mode;
            session.pr_skipped_reason = Some(cruise::session::PrSkippedReason::Disabled);
            let worktree = fixture.manager.worktrees_dir().join(SESSION_ID);
            session.worktree_path = Some(worktree.clone());
            session.worktree_branch = Some("retained-planning-worktree".into());
            fixture
                .manager
                .save(&session)
                .unwrap_or_else(|e| panic!("{e}"));
            let config =
                cruise::config::WorkflowConfig::from_yaml(CONFIG).unwrap_or_else(|e| panic!("{e}"));
            let compiled = cruise::workflow::compile(config).unwrap_or_else(|e| panic!("{e}"));
            let dag = cruise::graph::build_graph(&compiled, cruise::config::DEFAULT_MAX_RETRIES)
                .unwrap_or_else(|e| panic!("{e}"));
            cruise::graph::persistence::save_graph(&dag, &fixture.manager.dag_path(SESSION_ID))
                .unwrap_or_else(|e| panic!("{e}"));
            for (dir, expired) in [
                (
                    &session.base_dir,
                    if mode == WorkspaceMode::CurrentBranch {
                        selected_expired
                    } else {
                        !selected_expired
                    },
                ),
                (
                    &worktree,
                    if mode == WorkspaceMode::Worktree {
                        selected_expired
                    } else {
                        !selected_expired
                    },
                ),
            ] {
                fs::create_dir_all(dir.join("relative-auth")).unwrap_or_else(|e| panic!("{e}"));
                let expires = if expired { 1_i64 } else { 8_000_000_000_000 };
                fs::write(
                    dir.join("relative-auth/auth.json"),
                    format!(r#"{{"anthropic_accounts":[{{"expires":{expires}}}]}}"#),
                )
                .unwrap_or_else(|e| panic!("{e}"));
            }
            let before = fixture.state();
            let credentials = fs::read(worktree.join("relative-auth/auth.json"))
                .unwrap_or_else(|e| panic!("{e}"));
            let output = fixture
                .command()
                .env("JCODE_HOME", "relative-auth")
                .args(["run", SESSION_ID])
                .output()
                .unwrap_or_else(|e| panic!("{e}"));
            let text = text(&output);
            assert!(
                !output.status.success(),
                "non-repository sentinel must reject: {text}"
            );
            assert_eq!(
                text.contains("agent authentication expired"),
                selected_expired,
                "{text}"
            );
            if selected_expired {
                assert_eq!(fixture.state(), before);
                assert_eq!(
                    fs::read(worktree.join("relative-auth/auth.json"))
                        .unwrap_or_else(|e| panic!("{e}")),
                    credentials
                );
                assert!(!fixture.manager.clones_dir().exists());
                assert_eq!(
                    fs::read_dir(&worktree)
                        .unwrap_or_else(|e| panic!("{e}"))
                        .count(),
                    1
                );
            } else {
                assert!(
                    text.contains("git") || text.contains("repository"),
                    "must reach workspace validation: {text}"
                );
            }
        }
    }
}

fn write_title_auth_fixture(repo: &std::path::Path, case: &str) {
    let inherited = repo.join("title-auth");
    let workflow = repo.join("workflow-auth");
    for dir in [&inherited, &workflow] {
        fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
    }
    let expired = matches!(case, "expired" | "command" | "dry-run");
    let access_deadline = if expired || case == "refreshable" {
        1_i64
    } else {
        8_000_000_000_000
    };
    let refresh = if case == "refreshable" {
        serde_json::json!("refresh-secret")
    } else {
        serde_json::Value::Null
    };
    let credentials = serde_json::json!({
        "claudeAiOauth": {"expiresAt": access_deadline, "refreshToken": refresh},
        "anthropic_accounts": [{"expires": access_deadline, "refresh": refresh}],
    });
    if case != "unknown" {
        for file in ["auth.json", ".credentials.json"] {
            fs::write(inherited.join(file), credentials.to_string())
                .unwrap_or_else(|e| panic!("{e}"));
        }
    }
    // Workflow env is not forwarded to title dispatch. Its opposite
    // verdict must not override the inherited title credentials.
    fs::write(
        workflow.join(".credentials.json"),
        serde_json::json!({
            "claudeAiOauth": {"expiresAt": if expired { 8_000_000_000_000_i64 } else { 1 }},
        })
        .to_string(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn skip_planning_checks_inherited_title_auth_before_dispatch_and_approval() {
    for sdk in ["jcode", "claude"] {
        for case in [
            "expired",
            "usable",
            "refreshable",
            "unknown",
            "command",
            "dry-run",
        ] {
            let fixture = Fixture::new();
            let repo = fixture.root.path().join("not-a-repository");
            init_repo(&repo);
            let bin = fixture.root.path().join("bin");
            fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
            let marker = fixture.root.path().join("title-dispatched");
            write_script(
                &bin.join(sdk),
                &format!(
                    "#!/bin/sh\nprintf invoked > '{}'\nexit 73\n",
                    marker.display()
                ),
            );
            write_title_auth_fixture(&repo, case);
            let workflow = repo.join("workflow-auth");
            let backend = if case == "command" {
                "command: [cat]".to_string()
            } else {
                format!("sdk: {sdk}")
            };
            fs::write(fixture.root.path().join("workflow.yaml"), format!(
                "{backend}\nmodel: claude/test-model\nenv:\n  CLAUDE_CONFIG_DIR: '{}'\nsteps:\n  work:\n    command: echo done\n",
                workflow.display(),
            )).unwrap_or_else(|e| panic!("{e}"));
            let mut command = fixture.command();
            command
                .env("JCODE_HOME", "title-auth")
                .env("CLAUDE_CONFIG_DIR", "title-auth")
                .env(
                    "PATH",
                    std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
                        &std::env::var_os("PATH").unwrap_or_default(),
                    )))
                    .unwrap_or_else(|e| panic!("{e}")),
                )
                .args(["plan", "--skip-planning", "--config"])
                .arg(fixture.root.path().join("workflow.yaml"))
                .arg("title authentication contract");
            if case == "dry-run" {
                command.arg("--dry-run");
            }
            let output = command.output().unwrap_or_else(|e| panic!("{e}"));
            let text = text(&output);
            assert_eq!(
                output.status.success(),
                case != "expired",
                "{sdk}/{case}: {text}"
            );
            assert_eq!(
                marker.exists(),
                !matches!(case, "expired" | "command" | "dry-run"),
                "{sdk}/{case}: {text}"
            );
            assert_eq!(
                text.contains("agent authentication expired"),
                case == "expired",
                "{sdk}/{case}: {text}"
            );
            if case == "expired" {
                let login = if sdk == "jcode" {
                    "jcode login claude"
                } else {
                    "claude auth login"
                };
                assert!(text.contains(login), "{text}");
            }
            for state in fixture.manager.list().unwrap_or_else(|e| panic!("{e}")) {
                if state.id != SESSION_ID && case == "expired" {
                    assert_ne!(state.phase, SessionPhase::Planned);
                }
            }
        }
    }
}

#[test]
fn run_entrypoint_rejects_expired_agent_auth_before_workspace() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["run", SESSION_ID])
        .output()
        .unwrap_or_else(|e| panic!("run: {e}"));
    let text = text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("agent authentication expired"), "{text}");
    assert!(text.contains("jcode login claude"), "{text}");
    assert!(!text.contains("expired-cli-test-token"), "{text}");
    fixture.assert_no_workspace();
}

#[test]
fn run_entrypoint_dry_run_bypasses_expired_agent_auth() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let output = fixture
        .command()
        .args(["run", SESSION_ID, "--dry-run"])
        .output()
        .unwrap_or_else(|e| panic!("run dry-run: {e}"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(!text(&output).contains("agent authentication expired"));
    assert_eq!(fixture.state(), before);
    fixture.assert_no_workspace();
}

#[test]
fn exec_entrypoint_dry_run_bypasses_expired_agent_auth() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let output = fixture
        .command()
        .args(["exec", "--dry-run", "--config"])
        .arg(fixture.root.path().join("workflow.yaml"))
        .arg("auth dry-run")
        .output()
        .unwrap_or_else(|e| panic!("exec dry-run: {e}"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(!text(&output).contains("agent authentication expired"));
    assert_eq!(fixture.state(), before);
    assert_eq!(
        fixture
            .manager
            .list()
            .unwrap_or_else(|e| panic!("{e}"))
            .len(),
        1
    );
    fixture.assert_no_workspace();
}
