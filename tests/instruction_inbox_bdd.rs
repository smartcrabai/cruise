//! Executable instruction contracts against the real cruise CLI and engine.
#![expect(clippy::unwrap_used, reason = "isolated BDD fixtures")]
#[cfg(unix)]
mod contracts {
    use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
    use cruise::session_config::SessionConfigRef;
    use cucumber::{World as _, given, then, when};
    use serde_json::Value;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::{Child, Command, Output, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const ID: &str = "20261024000009";
    const TEXT: &str = "check the migration";

    #[derive(Debug, Default, cucumber::World)]
    pub struct World {
        root: Option<tempfile::TempDir>,
        child: Option<Child>,
        result: Option<Output>,
        before: Option<Vec<u8>>,
        success: bool,
    }
    impl Drop for World {
        fn drop(&mut self) {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    impl World {
        fn path(&self, p: &str) -> PathBuf {
            self.root.as_ref().unwrap().path().join(p)
        }
        fn ledger(&self) -> PathBuf {
            self.path(&format!("data/cruise/sessions/{ID}/instructions.jsonl"))
        }
        fn command(&self) -> Command {
            let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
            for (k, _) in std::env::vars_os() {
                if k.to_string_lossy().starts_with("CRUISE_") {
                    c.env_remove(k);
                }
            }
            c.current_dir(self.path("repo"))
                .env("HOME", self.path("home"))
                .env("XDG_CONFIG_HOME", self.path("config"))
                .env("XDG_DATA_HOME", self.path("data"))
                .env("XDG_STATE_HOME", self.path("state"))
                .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
                .env("CRUISE_DISABLE_HERDR", "1")
                .env("NO_COLOR", "1")
                .env_remove("HERDR_ENV")
                .stdin(Stdio::null());
            c
        }
        fn setup(&mut self, running: bool, two: bool) {
            self.root = Some(tempfile::tempdir().unwrap());
            fs::create_dir(self.path("repo")).unwrap();
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
                        .current_dir(self.path("repo"))
                        .args(args)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            fs::write(self.path("agent.sh"), "#!/bin/sh\nif [ ! -f prompt-1 ]; then cat > prompt-1; else cat > prompt-2; fi\nprintf 'ok\\n'\n").unwrap();
            fs::set_permissions(self.path("agent.sh"), fs::Permissions::from_mode(0o755)).unwrap();
            let manager = SessionManager::new(self.path("data/cruise"));
            let mut state = SessionState::new(
                ID.into(),
                self.path("repo"),
                SessionConfigRef::BuiltinSnapshot { name: None },
                "bdd fixture".into(),
            );
            state.phase = SessionPhase::Planned;
            state.workspace_mode = WorkspaceMode::CurrentBranch;
            manager.create(&state).unwrap();
            let session = manager.sessions_dir().join(ID);
            let agent = serde_json::to_string(&self.path("agent.sh").to_string_lossy()).unwrap();
            let second = if two {
                "  second:\n    prompt: SECOND\n"
            } else {
                ""
            };
            fs::write(session.join("config.yaml"), format!("command: [{agent}]\nsteps:\n  gate:\n    command: 'touch gate-ready; n=0; while [ ! -f gate-release ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done; test -f gate-release'\n  first:\n    prompt: FIRST\n{second}")).unwrap();
            fs::write(session.join("plan.md"), "# BDD plan\n").unwrap();
            if running {
                self.child = Some(
                    self.command()
                        .args(["run", ID])
                        .stdout(Stdio::null())
                        .stderr(Stdio::from(fs::File::create(self.path("run.err")).unwrap()))
                        .spawn()
                        .unwrap(),
                );
                let deadline = Instant::now() + Duration::from_secs(20);
                while !self.path("repo/gate-ready").exists() {
                    assert!(
                        self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                        "runner exited: {}",
                        fs::read_to_string(self.path("run.err")).unwrap_or_default()
                    );
                    assert!(Instant::now() < deadline, "runner did not reach gate");
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
        fn events(&self) -> Vec<Value> {
            fs::read_to_string(self.ledger())
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
        fn count(&self, kind: &str) -> usize {
            self.events().iter().filter(|e| e["kind"] == kind).count()
        }
    }
    #[given("a run paused in a command before two agent steps")]
    fn sequential(w: &mut World) {
        w.setup(true, true);
    }
    #[given("a run paused in a command before a failing agent step")]
    fn failing(w: &mut World) {
        w.setup(true, false);
    }
    #[given("a planned session")]
    fn planned(w: &mut World) {
        w.setup(false, false);
        fs::write(
            w.ledger(),
            "{\"kind\":\"run_opened\",\"at\":\"2026-10-24T00:00:00Z\"}\n",
        )
        .unwrap();
    }
    #[when(expr = "the operator tells the run {string}")]
    fn tell(w: &mut World, text: String) {
        w.before = fs::read(w.ledger()).ok();
        let result = w.command().args(["tell", ID, &text]).output().unwrap();
        if w.child.is_some() {
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        w.result = Some(result);
    }
    #[then("the command has not leased the instruction")]
    fn not_leased(w: &mut World) {
        assert_eq!(w.count("accepted"), 1);
        assert_eq!(w.count("leased"), 0);
    }
    #[when("the agent executable becomes unavailable")]
    fn remove_agent(w: &mut World) {
        fs::remove_file(w.path("agent.sh")).unwrap();
    }
    #[when("the command completes")]
    fn complete(w: &mut World) {
        fs::write(w.path("repo/gate-release"), "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = w.child.as_mut().unwrap().try_wait().unwrap() {
                w.success = status.success();
                break;
            }
            assert!(Instant::now() < deadline, "runner did not finish");
            thread::sleep(Duration::from_millis(20));
        }
    }
    #[then("the first agent receives the instruction before the commit guard")]
    fn first(w: &mut World) {
        assert!(
            w.success,
            "{}",
            fs::read_to_string(w.path("run.err")).unwrap_or_default()
        );
        let prompt = fs::read_to_string(w.path("repo/prompt-1")).unwrap();
        assert!(prompt.contains("FIRST"));
        assert!(prompt.find(TEXT).unwrap() < prompt.find("do not create commits").unwrap());
    }
    #[then("the second agent does not receive the instruction")]
    fn second(w: &mut World) {
        let p = fs::read_to_string(w.path("repo/prompt-2")).unwrap();
        assert!(p.contains("SECOND"));
        assert!(!p.contains(TEXT));
    }
    #[then("the instruction is leased and delivered exactly once")]
    fn delivered(w: &mut World) {
        assert_eq!(w.count("leased"), 1);
        assert_eq!(w.count("delivered"), 1);
        assert_eq!(w.count("released"), 0);
    }
    #[then("the instruction is released without being delivered")]
    fn released(w: &mut World) {
        assert!(!w.success);
        assert_eq!(w.count("leased"), 1);
        assert_eq!(w.count("released"), 1);
        assert_eq!(w.count("delivered"), 0);
    }
    #[then("the command rejects the instruction without changing the inbox")]
    fn rejected(w: &mut World) {
        let result = w.result.as_ref().unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains(&format!("not running: session {ID} is in phase Planned"))
        );
        assert_eq!(w.before, fs::read(w.ledger()).ok());
    }
    pub async fn run() {
        use cucumber::writer::Stats as _;
        let writer = World::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/instruction_inbox.feature"
            ))
            .await;
        assert!(
            !writer.execution_has_failed(),
            "{} failed steps, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
        assert!(
            writer.passed_steps() + writer.failed_steps() > 0,
            "no scenarios executed"
        );
    }
}
#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::run().await;
}
#[cfg(not(unix))]
fn main() {
    panic!("instruction_inbox_bdd requires Unix subprocess fixtures");
}
