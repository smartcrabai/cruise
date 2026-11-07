#![cfg(unix)]
//! Delivery contracts exercised through the real CLI and an executable agent.
//!
//! The initial command is a deterministic rendezvous: the instruction is sent
//! while it is running, before either prompt is dispatched.
use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use cruise::session_config::SessionConfigRef;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const ID: &str = "20261024000003";
struct Fixture {
    root: TempDir,
    repo: PathBuf,
    session: PathBuf,
}
impl Fixture {
    fn new(steps: &str, agent: &str) -> Self {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let repo = root.path().join("repo");
        fs::create_dir(&repo).unwrap_or_else(|error| panic!("{error}"));
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .current_dir(&repo)
                    .args(args)
                    .status()
                    .unwrap_or_else(|error| panic!("{error}"))
                    .success()
            );
        }
        let script = root.path().join("agent.sh");
        fs::write(&script, agent).unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{error}"));
        let manager = SessionManager::new(root.path().join("data/cruise"));
        let mut state = SessionState::new(
            ID.into(),
            repo.clone(),
            SessionConfigRef::BuiltinSnapshot { name: None },
            "delivery test".into(),
        );
        state.phase = SessionPhase::Planned;
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        manager
            .create(&state)
            .unwrap_or_else(|error| panic!("{error}"));
        let session = manager.sessions_dir().join(ID);
        let command = serde_json::to_string(&script.to_string_lossy())
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(session.join("config.yaml"), format!("command: [{command}]\nsteps:\n  gate:\n    command: 'touch gate-ready; n=0; while [ ! -f gate-release ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done; test -f gate-release'\n{steps}")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(session.join("plan.md"), "# delivery test\n")
            .unwrap_or_else(|error| panic!("{error}"));
        Self {
            root,
            repo,
            session,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(&self.repo)
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("CRUISE_DISABLE_HERDR", "1")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null());
        cmd
    }
    fn start(&self) -> Running {
        let child = self
            .command()
            .args(["run", ID])
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                fs::File::create(self.root.path().join("runner.stderr"))
                    .unwrap_or_else(|error| panic!("{error}")),
            ))
            .spawn()
            .unwrap_or_else(|error| panic!("{error}"));
        let mut run = Running(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.repo.join("gate-ready").exists() {
            assert!(
                run.0
                    .try_wait()
                    .unwrap_or_else(|error| panic!("{error}"))
                    .is_none(),
                "runner exited before command rendezvous: {}",
                fs::read_to_string(self.root.path().join("runner.stderr")).unwrap_or_default()
            );
            assert!(
                Instant::now() < deadline,
                "runner did not reach command rendezvous"
            );
            thread::sleep(Duration::from_millis(20));
        }
        run
    }
    fn tell(&self) {
        let out = self
            .command()
            .args(["tell", ID, "operator-marker"])
            .output()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            out.status.success(),
            "tell failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.session.join("instructions.jsonl"))
            .unwrap_or_else(|error| panic!("{error}"))
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}")))
            .collect()
    }
    fn release(&self) {
        fs::write(self.repo.join("gate-release"), "").unwrap_or_else(|error| panic!("{error}"));
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.repo.join(name)).unwrap_or_else(|error| panic!("{error}"))
    }
}
struct Running(Child);
impl Running {
    fn finish(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().unwrap_or_else(|error| panic!("{error}")) {
                return status;
            }
            assert!(Instant::now() < deadline, "runner failed to finish");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn kinds(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["kind"] == kind).count()
}

#[test]
fn sequential_delivery_is_acknowledged_once_and_command_does_not_lease() {
    let f = Fixture::new(
        "  first:\n    prompt: FIRST-PROMPT\n  second:\n    prompt: SECOND-PROMPT\n",
        "#!/bin/sh\nif [ ! -f prompt-1 ]; then cat > prompt-1; else cat > prompt-2; fi\nprintf 'ok\\n'\n",
    );
    let mut run = f.start();
    f.tell();
    let before = f.events();
    assert_eq!(kinds(&before, "leased"), 0, "command step must not lease");
    f.release();
    assert!(run.finish().success());
    let first = f.read("prompt-1");
    let second = f.read("prompt-2");
    assert!(
        first.contains("FIRST-PROMPT") && first.contains("operator-marker"),
        "{first}"
    );
    let instruction = first
        .find("operator-marker")
        .unwrap_or_else(|| panic!("missing operator marker"));
    assert!(
        instruction
            < first
                .find("do not create commits")
                .unwrap_or_else(|| panic!("missing commit guard"))
    );
    assert!(!second.contains("operator-marker"), "{second}");
    let events = f.events();
    assert_eq!(kinds(&events, "leased"), 1);
    assert_eq!(kinds(&events, "delivered"), 1);
    assert_eq!(kinds(&events, "released"), 0);
    assert_eq!(
        events
            .iter()
            .find(|e| e["kind"] == "leased")
            .unwrap_or_else(|| panic!("missing lease"))["step"],
        "first"
    );
}

#[test]
fn prompt_timeout_releases_instruction() {
    let f = Fixture::new(
        "  slow:\n    prompt: SLOW-PROMPT\n    timeout: '1'\n",
        "#!/bin/sh\ncat > prompt-1\nsleep 5\n",
    );
    let mut run = f.start();
    f.tell();
    f.release();
    let _ = run.finish();
    assert!(f.read("prompt-1").contains("operator-marker"));
    let events = f.events();
    assert_eq!(kinds(&events, "leased"), 1);
    assert_eq!(kinds(&events, "released"), 1);
    assert_eq!(kinds(&events, "delivered"), 0);
}

#[test]
fn sequential_backend_error_releases_without_acknowledging() {
    let f = Fixture::new(
        "  first:\n    prompt: FIRST\n",
        "#!/bin/sh\ncat >/dev/null\nprintf 'ok\\n'\n",
    );
    let mut run = f.start();
    f.tell();
    fs::remove_file(f.root.path().join("agent.sh")).unwrap_or_else(|error| panic!("{error}"));
    f.release();
    assert!(!run.finish().success());
    let events = f.events();
    assert_eq!(kinds(&events, "leased"), 1);
    assert_eq!(kinds(&events, "released"), 1);
    assert_eq!(kinds(&events, "delivered"), 0);
}

#[test]
fn parallel_backend_error_releases_block_without_acknowledging() {
    let f = Fixture::new(
        "  checks:\n    parallel:\n      alpha:\n        prompt: ALPHA\n      beta:\n        prompt: BETA\n",
        "#!/bin/sh\ncat >/dev/null\nprintf 'ok\\n'\n",
    );
    let mut run = f.start();
    f.tell();
    fs::remove_file(f.root.path().join("agent.sh")).unwrap_or_else(|error| panic!("{error}"));
    f.release();
    assert!(!run.finish().success());
    let events = f.events();
    assert_eq!(kinds(&events, "leased"), 1);
    assert_eq!(kinds(&events, "released"), 1);
    assert_eq!(kinds(&events, "delivered"), 0);
}

#[test]
fn parallel_prompt_children_share_one_lease_and_ack() {
    let f = Fixture::new(
        "  checks:\n    parallel:\n      alpha:\n        prompt: ALPHA\n      beta:\n        prompt: BETA\n      shell:\n        command: touch shell-ran\n",
        "#!/bin/sh\ncat > \"capture-$$\"\nprintf 'ok\\n'\n",
    );
    let mut run = f.start();
    f.tell();
    f.release();
    assert!(run.finish().success());
    let captures: Vec<String> = fs::read_dir(&f.repo)
        .unwrap_or_else(|error| panic!("{error}"))
        .map(|e| e.unwrap_or_else(|error| panic!("{error}")).path())
        .filter(|p| {
            p.file_name()
                .unwrap_or_else(|| panic!("missing file name"))
                .to_string_lossy()
                .starts_with("capture-")
        })
        .map(|p| fs::read_to_string(p).unwrap_or_else(|error| panic!("{error}")))
        .collect();
    assert_eq!(captures.len(), 2);
    assert!(
        captures.iter().any(|p| p.contains("ALPHA")) && captures.iter().any(|p| p.contains("BETA"))
    );
    for prompt in &captures {
        assert!(prompt.contains("operator-marker"), "{prompt}");
    }
    let block = |prompt: &str| {
        prompt
            .split("## Operator instructions received during this run")
            .nth(1)
            .unwrap_or_else(|| panic!("missing instruction block"))
            .split("In this working repository")
            .next()
            .unwrap_or_else(|| panic!("missing instruction content"))
            .trim()
            .to_owned()
    };
    assert_eq!(block(&captures[0]), block(&captures[1]));
    assert!(f.repo.join("shell-ran").exists());
    let events = f.events();
    assert_eq!(kinds(&events, "leased"), 1);
    assert_eq!(kinds(&events, "delivered"), 1);
    assert_eq!(kinds(&events, "released"), 0);
    assert_eq!(
        events
            .iter()
            .find(|e| e["kind"] == "leased")
            .unwrap_or_else(|| panic!("missing lease"))["scope"],
        "parallel:checks"
    );
}

#[test]
fn all_skipped_parallel_prompts_do_not_consume_instruction() {
    let f = Fixture::new(
        "  checks:\n    parallel:\n      skipped:\n        prompt: NEVER\n        when: { exists: '*.absent' }\n  after:\n    prompt: AFTER\n",
        "#!/bin/sh\ncat > prompt-1\nprintf 'ok\\n'\n",
    );
    let mut run = f.start();
    f.tell();
    f.release();
    assert!(run.finish().success());
    let prompt = f.read("prompt-1");
    assert!(prompt.contains("AFTER") && prompt.contains("operator-marker"));
    let events = f.events();
    assert_eq!(kinds(&events, "leased"), 1);
    assert_eq!(
        events
            .iter()
            .find(|e| e["kind"] == "leased")
            .unwrap_or_else(|| panic!("missing lease"))["step"],
        "after"
    );
}

#[test]
fn command_only_parallel_does_not_consume_instruction() {
    let f = Fixture::new(
        "  checks:\n    parallel:\n      shell:\n        command: touch shell-ran\n  after:\n    prompt: AFTER\n",
        "#!/bin/sh\ncat > prompt-1\nprintf 'ok\\n'\n",
    );
    let mut run = f.start();
    f.tell();
    f.release();
    assert!(run.finish().success());
    assert!(f.repo.join("shell-ran").exists());
    assert!(f.read("prompt-1").contains("operator-marker"));
    let events = f.events();
    assert_eq!(kinds(&events, "leased"), 1);
    assert_eq!(
        events
            .iter()
            .find(|e| e["kind"] == "leased")
            .unwrap_or_else(|| panic!("missing lease"))["step"],
        "after"
    );
}
