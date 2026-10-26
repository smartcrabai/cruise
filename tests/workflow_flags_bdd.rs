//! Real launcher/runtime contracts for the planning-to-execution flag boundary.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod contracts {
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
    use cruise::session_config::SessionConfigRef;
    use cucumber::writer::Stats as _;
    use cucumber::{World as _, given, then, when};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    const PROMPT: &str = "Which optional workflow flags should be enabled?";
    const ID: &str = "20261024000000";
    const WORKFLOW: &str = "command: [echo]\nflags:\n  e2e:\n    description: Run E2E\n  docs: {}\nsteps:\n  build:\n    command: touch build.txt\n  e2e-test:\n    flag: e2e\n    command: touch e2e.txt\n  docs-test:\n    flag: docs\n    command: touch docs.txt\n";

    #[derive(Debug, Default, cucumber::World)]
    pub struct FlagWorld {
        root: Option<tempfile::TempDir>,
        repo: PathBuf,
        config: PathBuf,
        before: Vec<u8>,
        success: bool,
        transcript: String,
        responses: Vec<(String, Vec<u8>)>,
    }

    struct Reap(Box<dyn portable_pty::Child + Send + Sync>);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    impl FlagWorld {
        fn root(&self) -> &std::path::Path {
            self.root
                .as_ref()
                .unwrap_or_else(|| panic!("missing fixture"))
                .path()
        }
        fn manager(&self) -> SessionManager {
            SessionManager::new(self.root().join("data/cruise"))
        }
        fn state_bytes(&self) -> Vec<u8> {
            std::fs::read(self.manager().sessions_dir().join(ID).join("state.json"))
                .unwrap_or_else(|e| panic!("read state: {e}"))
        }
        fn env(&self) -> Vec<(String, String)> {
            let mut values: Vec<_> = ["CONFIG", "DATA", "STATE"]
                .into_iter()
                .map(|kind| {
                    (
                        format!("XDG_{kind}_HOME"),
                        self.root()
                            .join(kind.to_lowercase())
                            .to_string_lossy()
                            .into_owned(),
                    )
                })
                .collect();
            values.extend([
                ("CRUISE_DISABLE_NOTIFICATIONS".into(), "1".into()),
                ("TERM".into(), "xterm-256color".into()),
                ("NO_COLOR".into(), "1".into()),
            ]);
            values
        }
        #[expect(
            clippy::too_many_lines,
            reason = "keep bounded PTY lifecycle and transcript handling together"
        )]
        fn run(&mut self, args: &[&str], terminal: bool, response: Option<&[u8]>) {
            if !terminal {
                let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
                for (key, _) in std::env::vars_os() {
                    if (key.to_string_lossy().starts_with("CRUISE_")
                        && key != "CRUISE_GUARD_COMMON_DIR")
                        || key == "HERDR_ENV"
                    {
                        command.env_remove(key);
                    }
                }
                let output = command
                    .args(args)
                    .current_dir(&self.repo)
                    .envs(self.env())
                    .stdin(Stdio::null())
                    .output()
                    .unwrap_or_else(|e| panic!("run CLI: {e}"));
                self.success = output.status.success();
                self.transcript = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                return;
            }
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 40,
                    cols: 120,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap_or_else(|e| panic!("open PTY: {e}"));
            let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
            command.args(args);
            command.cwd(&self.repo);
            for (key, _) in std::env::vars_os() {
                if (key.to_string_lossy().starts_with("CRUISE_")
                    && key != "CRUISE_GUARD_COMMON_DIR")
                    || key == "HERDR_ENV"
                {
                    command.env_remove(key);
                }
            }
            for (key, value) in self.env() {
                command.env(key, value);
            }
            let child = pair
                .slave
                .spawn_command(command)
                .unwrap_or_else(|e| panic!("spawn CLI: {e}"));
            let mut child = Reap(child);
            drop(pair.slave);
            let mut reader = pair
                .master
                .try_clone_reader()
                .unwrap_or_else(|e| panic!("PTY reader: {e}"));
            let mut input = pair
                .master
                .take_writer()
                .unwrap_or_else(|e| panic!("PTY writer: {e}"));
            let (tx, rx) = mpsc::sync_channel(32);
            std::thread::spawn(move || {
                let mut bytes = [0; 4096];
                while let Ok(n) = reader.read(&mut bytes) {
                    if n == 0 || tx.send(bytes[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut bytes = Vec::new();
            let mut sent = false;
            let mut response_index = 0;
            let mut response_start = 0;
            let mut interrupted = false;
            let mut queries = 0;
            let mut status = None;
            loop {
                assert!(
                    Instant::now() < deadline,
                    "PTY timed out: {}",
                    String::from_utf8_lossy(&bytes)
                );
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(chunk) => bytes.extend(chunk),
                    Err(mpsc::RecvTimeoutError::Disconnected) if status.is_some() => break,
                    Err(_) => {}
                }
                assert!(bytes.len() < 2 * 1024 * 1024, "excessive PTY output");
                let text = String::from_utf8_lossy(&bytes);
                let count = text.matches("\x1b[6n").count();
                for _ in queries..count {
                    input
                        .write_all(b"\x1b[1;1R")
                        .unwrap_or_else(|e| panic!("cursor reply: {e}"));
                }
                queries = count;
                if let Some((marker, keys)) = self.responses.get(response_index)
                    && text[response_start..].contains(marker)
                {
                    input
                        .write_all(keys)
                        .unwrap_or_else(|e| panic!("script response: {e}"));
                    input.flush().unwrap_or_else(|e| panic!("flush: {e}"));
                    response_index += 1;
                    response_start = text.len();
                }
                if !sent
                    && text.contains("Run E2E")
                    && let Some(keys) = response
                {
                    input
                        .write_all(keys)
                        .unwrap_or_else(|e| panic!("selection: {e}"));
                    input.flush().unwrap_or_else(|e| panic!("flush: {e}"));
                    sent = true;
                }
                if !interrupted && text.contains("BDD_INTERRUPT") {
                    input
                        .write_all(b"\x03")
                        .unwrap_or_else(|e| panic!("interrupt: {e}"));
                    interrupted = true;
                }
                if status.is_none() {
                    status = child
                        .0
                        .try_wait()
                        .unwrap_or_else(|e| panic!("wait CLI: {e}"));
                }
            }
            self.success = status.is_some_and(|s| s.success());
            self.transcript = String::from_utf8_lossy(&bytes).into_owned();
            assert_eq!(
                response_index,
                self.responses.len(),
                "unreached PTY responses: {}",
                self.transcript
            );
            self.responses.clear();
            if response.is_some() {
                assert!(sent, "selection never reached: {}", self.transcript);
            }
        }
    }

    #[given("an isolated planned workflow with docs enabled")]
    fn fixture(w: &mut FlagWorld) {
        w.root = Some(tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}")));
        w.repo = w.root().join("repo");
        w.config = w.root().join("workflow.yaml");
        std::fs::create_dir(&w.repo).unwrap_or_else(|e| panic!("mkdir: {e}"));
        // Fixture-only repository, never operate on the source worktree's refs.
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=BDD",
                "-c",
                "user.email=bdd@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ][..],
        ] {
            let output = Command::new("git")
                .args(args)
                .current_dir(&w.repo)
                .output()
                .unwrap_or_else(|e| panic!("git: {e}"));
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::write(&w.config, WORKFLOW).unwrap_or_else(|e| panic!("config: {e}"));
        let mut state = SessionState::new(
            ID.into(),
            w.repo.clone(),
            SessionConfigRef::File {
                path: w.config.clone(),
            },
            "BDD saved plan".into(),
        );
        state.phase = SessionPhase::Planned;
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.flags.insert("docs".into(), true);
        w.manager()
            .create(&state)
            .unwrap_or_else(|e| panic!("create: {e}"));
        std::fs::write(
            state.plan_path(&w.manager().sessions_dir()),
            "# Approved BDD plan\n",
        )
        .unwrap_or_else(|e| panic!("plan: {e}"));
        w.before = w.state_bytes();
    }

    #[when("I run the workflow in a terminal and select e2e instead")]
    fn select(w: &mut FlagWorld) {
        w.run(&["run", ID], true, Some(b" \x1b[B \r"));
    }
    #[when("I run the workflow in a terminal and clear all flags")]
    fn clear(w: &mut FlagWorld) {
        w.run(&["run", ID], true, Some(b"\x1b[B \r"));
    }
    #[when("I run the workflow in a terminal and accept saved flags")]
    fn accept(w: &mut FlagWorld) {
        w.run(&["run", ID], true, Some(b"\r"));
    }
    #[when("I run the workflow in a terminal with explicit e2e and docs off")]
    fn explicit(w: &mut FlagWorld) {
        w.run(&["run", ID, "-f", "e2e,docs=off"], true, None);
    }
    #[when("I run the workflow without a terminal")]
    fn nonterminal(w: &mut FlagWorld) {
        w.run(&["run", ID], false, None);
    }
    #[when("I dry run the workflow in a terminal")]
    fn dry(w: &mut FlagWorld) {
        w.run(&["run", ID, "--dry-run"], true, None);
        assert!(w.success, "{}", w.transcript);
    }
    #[when("I cancel workflow flag selection")]
    fn cancel(w: &mut FlagWorld) {
        w.run(&["run", ID], true, Some(b"\x1b"));
    }
    #[when("I cancel flag selection for an ephemeral exec")]
    fn cancel_exec(w: &mut FlagWorld) {
        let config = w.config.to_string_lossy().into_owned();
        w.run(&["exec", "--config", &config], true, Some(b"\x03"));
    }
    #[when("I resume after an interrupted workflow")]
    fn resume(w: &mut FlagWorld) {
        // Interrupt a real shell step to create a resumable checkpoint.
        std::fs::write(
            &w.config,
            WORKFLOW.replace(
                "touch build.txt",
                "if test ! -f allow-resume; then echo BDD_INTERRUPT; sleep 20; fi; touch build.txt",
            ),
        )
        .unwrap_or_else(|e| panic!("config: {e}"));
        w.run(&["run", ID], true, Some(b"\r"));
        assert!(!w.success, "first execution should fail: {}", w.transcript);
        let state = w.manager().load(ID).unwrap_or_else(|e| panic!("load: {e}"));
        assert!(
            state.current_step.is_some(),
            "failed execution must be resumable"
        );
        std::fs::write(w.repo.join("allow-resume"), "ok")
            .unwrap_or_else(|e| panic!("resume gate: {e}"));
        w.run(&["run", ID], true, None);
    }

    #[then(regex = r"^only the (e2e|docs) optional effect runs$")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber capture arguments are owned"
    )]
    fn effects(w: &mut FlagWorld, enabled: String) {
        assert!(w.success, "{}", w.transcript);
        assert!(w.repo.join("build.txt").exists(), "{}", w.transcript);
        for name in ["e2e", "docs"] {
            assert_eq!(
                w.repo.join(format!("{name}.txt")).exists(),
                name == enabled,
                "{}",
                w.transcript
            );
        }
    }
    #[then("no optional effects run")]
    fn none(w: &mut FlagWorld) {
        assert!(w.success, "{}", w.transcript);
        assert!(w.repo.join("build.txt").exists());
        for name in ["e2e.txt", "docs.txt"] {
            assert!(!w.repo.join(name).exists());
        }
    }
    #[then("no workflow effects run")]
    fn no_effects(w: &mut FlagWorld) {
        for name in ["build.txt", "e2e.txt", "docs.txt"] {
            assert!(!w.repo.join(name).exists(), "{name}: {}", w.transcript);
        }
    }
    #[then("no flag selection prompt is displayed")]
    fn no_prompt(w: &mut FlagWorld) {
        assert!(!w.transcript.contains(PROMPT), "{}", w.transcript);
    }
    #[then(regex = r"^the saved flags enable (e2e|docs) and disable (docs|e2e)$")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber capture arguments are owned"
    )]
    fn saved(w: &mut FlagWorld, enabled: String, disabled: String) {
        assert!(w.transcript.contains(PROMPT), "{}", w.transcript);
        let state = w.manager().load(ID).unwrap_or_else(|e| panic!("load: {e}"));
        assert_eq!(state.flags.get(&enabled), Some(&true));
        assert_eq!(state.flags.get(&disabled), Some(&false));
    }
    #[then("both saved declared flags are explicitly off")]
    fn off(w: &mut FlagWorld) {
        let state = w.manager().load(ID).unwrap_or_else(|e| panic!("load: {e}"));
        for name in ["e2e", "docs"] {
            assert_eq!(state.flags.get(name), Some(&false));
        }
    }
    #[then("the planned session is unchanged")]
    fn unchanged(w: &mut FlagWorld) {
        assert_eq!(w.state_bytes(), w.before);
    }
    #[then("selection fails with a cancellation diagnostic")]
    fn cancelled(w: &mut FlagWorld) {
        assert!(!w.success, "{}", w.transcript);
        assert!(
            w.transcript.contains("workflow flag selection cancelled"),
            "{}",
            w.transcript
        );
    }
    #[then("no ephemeral session remains")]
    fn cleaned(w: &mut FlagWorld) {
        let sessions = w.manager().list().unwrap_or_else(|e| panic!("list: {e}"));
        assert_eq!(
            sessions.len(),
            1,
            "only original planned session may remain"
        );
        assert_eq!(sessions[0].id, ID);
        assert_eq!(
            std::fs::read_dir(w.manager().sessions_dir())
                .unwrap_or_else(|e| panic!("sessions: {e}"))
                .count(),
            1,
            "no orphan exec directory"
        );
        assert_eq!(w.state_bytes(), w.before);
    }

    #[given("an isolated approval workflow with gated skip candidates")]
    fn approval_fixture(w: &mut FlagWorld) {
        w.root = Some(tempfile::tempdir().unwrap_or_else(|e| panic!("approval tempdir: {e}")));
        w.repo = w.root().join("plain");
        std::fs::create_dir(&w.repo).unwrap_or_else(|e| panic!("plain directory: {e}"));
        w.config = w.root().join("workflow.yaml");
        std::fs::write(
            &w.config,
            include_str!("fixtures/workflow_flags_specs/skip-selection.yaml"),
        )
        .unwrap_or_else(|e| panic!("approval workflow: {e}"));
    }

    #[given("approval history previously skipped enabled and disabled gated steps")]
    fn history_fixture(w: &mut FlagWorld) {
        // Production deliberately ignores OS temporary directories in history.
        // A disposable home child exercises real scoped persistence instead.
        w.root = Some(
            tempfile::Builder::new()
                .prefix(".cruise-bdd-history-")
                .tempdir_in(
                    std::env::var_os("HOME")
                        .unwrap_or_else(|| panic!("home for persistent-scope fixture")),
                )
                .unwrap_or_else(|e| panic!("history fixture: {e}")),
        );
        w.repo = w.root().join("plain");
        std::fs::create_dir(&w.repo).unwrap_or_else(|e| panic!("history working directory: {e}"));
        w.config = w.root().join("workflow.yaml");
        std::fs::write(
            &w.config,
            include_str!("fixtures/workflow_flags_specs/skip-selection.yaml"),
        )
        .unwrap_or_else(|e| panic!("history workflow: {e}"));
        let history = serde_json::json!({"entries": [{
            "working_dir": w.repo,
            "resolved_config_key": w.config,
            "skipped_steps": ["e2e-test", "docs-test", "docs-publish"]
        }]});
        let directory = w.root().join("state/cruise");
        std::fs::create_dir_all(&directory).unwrap_or_else(|e| panic!("history directory: {e}"));
        std::fs::write(
            directory.join("history.json"),
            serde_json::to_vec(&history).unwrap_or_else(|e| panic!("history JSON: {e}")),
        )
        .unwrap_or_else(|e| panic!("seed real history: {e}"));
    }

    #[when("I approve e2e while accepting its remembered skip")]
    fn approve_remembered(w: &mut FlagWorld) {
        approve(
            w,
            &[
                ("Publish as Issue", b"\r"),
                ("Run E2E", b" \r"),
                ("Steps to skip (", b"\r"),
            ],
            false,
        );
    }

    #[then("only the enabled remembered skip is saved in the session and real history")]
    fn remembered_saved(w: &mut FlagWorld) {
        let state = approved_state(w);
        assert_eq!(state.skipped_steps, vec!["e2e-test"], "{}", w.transcript);
        assert_eq!(state.flags.get("e2e"), Some(&true));
        assert_eq!(state.flags.get("docs"), Some(&false));
        let history: cruise::new_session_history::NewSessionHistory = serde_json::from_slice(
            &std::fs::read(w.root().join("state/cruise/history.json"))
                .unwrap_or_else(|e| panic!("read production history: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse production history: {e}"));
        let key = w.config.to_string_lossy();
        let directory = w.repo.to_string_lossy();
        let entry = history
            .entries
            .iter()
            .find(|entry| entry.resolved_config_key == key && entry.working_dir == directory)
            .unwrap_or_else(|| panic!("real scoped history entry"));
        assert_eq!(
            entry.skipped_steps,
            vec!["e2e-test"],
            "hidden historical skips must be removed"
        );
        no_effects(w);
    }

    fn approve(w: &mut FlagWorld, responses: &[(&str, &[u8])], explicit: bool) {
        w.responses = responses
            .iter()
            .map(|(s, b)| ((*s).into(), b.to_vec()))
            .collect();
        let config = w.config.to_string_lossy().into_owned();
        let mut args = vec!["plan", "--skip-planning", "--config", &config];
        if explicit {
            args.extend(["-f", "docs,e2e=off"]);
        }
        args.push("BDD approval");
        w.run(&args, true, None);
    }

    #[when("I approve with every flag off")]
    fn approve_off(w: &mut FlagWorld) {
        approve(
            w,
            &[
                ("Publish as Issue", b"\r"),
                ("Run E2E", b"\r"),
                ("Steps to skip (", b"\r"),
            ],
            false,
        );
    }

    #[when("I execute now after selecting e2e")]
    fn execute_selected(w: &mut FlagWorld) {
        approve(
            w,
            &[
                ("Publish as Issue", b"\x1b[B\x1b[B\x1b[B\r"),
                ("Run E2E", b" \r"),
                ("Steps to skip (", b"\r"),
                ("Use current branch", b"\x1b"),
            ],
            false,
        );
    }

    #[when("I execute now with explicit docs and e2e off")]
    fn execute_explicit(w: &mut FlagWorld) {
        approve(
            w,
            &[
                ("Publish as Issue", b"\x1b[B\x1b[B\x1b[B\r"),
                ("Steps to skip (", b"\r"),
                ("Use current branch", b"\x1b"),
            ],
            true,
        );
    }

    #[when(regex = r"^I cancel the (flag|skip) approval selection then accept untouched defaults$")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber capture arguments are owned"
    )]
    fn cancel_approval(w: &mut FlagWorld, selection: String) {
        let mut responses: Vec<(&str, &[u8])> = vec![("Publish as Issue", b"\r")];
        if selection == "skip" {
            responses.extend([
                ("Run E2E", &b" \r"[..]),
                ("Steps to skip (", &b"\x1b"[..]),
            ]);
        } else {
            responses.push(("Run E2E", b" \x1b"));
        }
        responses.extend([
            ("Publish as Issue", &b"\r"[..]),
            ("Run E2E", &b"\r"[..]),
            ("Steps to skip (", &b"\r"[..]),
        ]);
        approve(w, &responses, false);
    }

    fn approved_state(w: &FlagWorld) -> SessionState {
        let sessions = w
            .manager()
            .list()
            .unwrap_or_else(|e| panic!("approval sessions: {e}"));
        assert_eq!(sessions.len(), 1, "{}", w.transcript);
        sessions
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("approved session"))
    }

    #[then(regex = r"^the approval skip candidates reflect (off|e2e|docs) flags$")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber capture arguments are owned"
    )]
    fn candidates(w: &mut FlagWorld, enabled: String) {
        let start = w
            .transcript
            .find("Steps to skip (Space")
            .unwrap_or_else(|| panic!("skip prompt: {}", w.transcript));
        let rest = &w.transcript[start..];
        let end = rest.find("Use current branch").unwrap_or(rest.len());
        let skips = &rest[..end];
        assert!(skips.contains("build"), "{}", w.transcript);
        for (name, visible) in [
            ("e2e-test", enabled == "e2e"),
            ("docs-test", enabled == "docs"),
            ("either-test", enabled != "off"),
            ("gated-review", enabled != "off"),
            ("group-child", enabled != "off"),
            ("e2e-publish", enabled == "e2e"),
            ("docs-publish", enabled == "docs"),
        ] {
            assert_eq!(
                skips.contains(name),
                visible,
                "candidate {name}: {}",
                w.transcript
            );
        }
    }

    #[then("approval asks flags before skips and execution does not ask again")]
    fn approval_order(w: &mut FlagWorld) {
        let flags = w
            .transcript
            .find(PROMPT)
            .unwrap_or_else(|| panic!("flags prompt: {}", w.transcript));
        let skips = w
            .transcript
            .find("Steps to skip (Space")
            .unwrap_or_else(|| panic!("skip prompt: {}", w.transcript));
        assert!(flags < skips, "{}", w.transcript);
        assert!(!w.transcript[skips..].contains(PROMPT), "{}", w.transcript);
    }

    #[then(regex = r"^the approved session records (off|e2e|docs) flags without hidden skips$")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber capture arguments are owned"
    )]
    fn approval_saved(w: &mut FlagWorld, enabled: String) {
        let state = approved_state(w);
        assert_eq!(state.phase, SessionPhase::Planned);
        for name in ["e2e", "docs"] {
            assert_eq!(
                state.flags.get(name),
                Some(&(name == enabled)),
                "{}",
                w.transcript
            );
        }
        assert!(state.skipped_steps.is_empty(), "{}", w.transcript);
        no_effects(w);
    }

    #[then("execution reached workspace selection and was cancelled")]
    fn workspace_cancel(w: &mut FlagWorld) {
        assert!(!w.success, "{}", w.transcript);
        assert!(
            w.transcript.contains("workspace") && w.transcript.contains("cancel"),
            "{}",
            w.transcript
        );
    }

    #[then("approval completed successfully")]
    fn approval_success(w: &mut FlagWorld) {
        assert!(w.success, "{}", w.transcript);
    }

    pub async fn main() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/workflow_flags.feature"
        );
        let feature =
            cucumber::gherkin::Feature::parse_path(path, cucumber::gherkin::GherkinEnv::default())
                .unwrap_or_else(|e| panic!("parse feature: {e}"));
        assert_eq!(
            feature.scenarios.len(),
            15,
            "scenario inventory must not silently become empty"
        );
        let writer = FlagWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(path)
            .await;
        assert!(
            writer.passed_steps() + writer.failed_steps() > 0,
            "no workflow flag scenarios executed"
        );
        assert!(
            !writer.execution_has_failed(),
            "{} failed steps, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::main().await;
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    panic!("workflow_flags_bdd requires a macOS or Linux PTY, refusing a zero-scenario pass");
}
