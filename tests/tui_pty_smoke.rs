#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use tempfile::TempDir;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct Fixture {
    root: TempDir,
    home: PathBuf,
    manager: SessionManager,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let home = root.path().join("home");
        for directory in [
            home.clone(),
            home.join("config"),
            home.join("data"),
            home.join("state"),
        ] {
            std::fs::create_dir_all(directory).unwrap_or_else(|error| panic!("{error}"));
        }
        let manager = SessionManager::new(home.join("data/cruise"));
        Self {
            root,
            home,
            manager,
        }
    }

    fn start(&self, columns: u16, rows: u16, no_color: bool) -> PtySession {
        PtySession::start(
            Path::new(env!("CARGO_BIN_EXE_cruise")),
            self,
            columns,
            rows,
            no_color,
        )
    }

    fn configure(&self, command: &mut Command, no_color: bool) {
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            // Ignore GIT_CONFIG_* pairs inherited from an outer cruise commit guard.
            .env("GIT_CONFIG_COUNT", "0")
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_MODEL")
            .env_remove("CRUISE_PLAN_MODEL")
            .env_remove("CRUISE_SDK")
            .env_remove("CRUISE_LANGUAGE_PR")
            .env_remove("CRUISE_LANGUAGE_PLAN")
            .env_remove("CRUISE_CLEANUP_AFTER_PR")
            .env_remove("CRUISE_INTERACTIVE_PLANNING")
            .env_remove("CRUISE_FORCE_EXEC")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        if no_color {
            command.env("NO_COLOR", "1");
        } else {
            command.env_remove("NO_COLOR");
        }
    }

    fn seed_current_branch_session(&self, id: &str, input: &str, command: &str) -> String {
        let repo = self.root.path().join(format!("repo-{id}"));
        std::fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("{error}"));
        run_git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "terminal e2e\n")
            .unwrap_or_else(|error| panic!("{error}"));
        run_git(&repo, &["add", "README.md"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=Cruise E2E",
                "-c",
                "user.email=cruise-e2e@example.com",
                "commit",
                "-qm",
                "initial",
            ],
        );

        let config = self.root.path().join(format!("{id}.yaml"));
        let yaml = format!("command: [cat]\nsteps:\n  verify:\n    command: {command}\n");
        std::fs::write(&config, &yaml).unwrap_or_else(|error| panic!("{error}"));

        let mut state = SessionState::new(
            id.to_string(),
            repo,
            "e2e.yaml".to_string(),
            input.to_string(),
        );
        state.phase = SessionPhase::Planned;
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.target_branch = Some("main".to_string());
        state.config_path = Some(config);
        state.has_dag = true;
        self.manager
            .create(&state)
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            state.plan_path(&self.manager.sessions_dir()),
            format!("# E2E Plan\n\n{input}\n"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let workflow = cruise::config::WorkflowConfig::from_yaml(&yaml)
            .unwrap_or_else(|error| panic!("{error}"));
        let compiled =
            cruise::workflow::compile(workflow).unwrap_or_else(|error| panic!("{error}"));
        let dag = cruise::dag::build_dag(&compiled, 0).unwrap_or_else(|error| panic!("{error}"));
        cruise::dag::save_dag(&dag, &self.manager.dag_path(id))
            .unwrap_or_else(|error| panic!("{error}"));
        id.to_string()
    }

    fn seed_planned_session(&self) -> String {
        let id = "20260831000000000_00000000000000000000000000000000";
        let id =
            self.seed_current_branch_session(id, "E2E terminal session", "echo e2e-run-complete");
        std::fs::write(self.manager.run_log_path(&id), "seeded-log-line\n")
            .unwrap_or_else(|error| panic!("{error}"));
        id
    }

    fn seed_cancellable_session(&self) -> String {
        self.seed_current_branch_session(
            "20260831000000001_00000000000000000000000000000001",
            "Cancellable terminal session",
            "sleep 30",
        )
    }

    fn seed_display_session(
        &self,
        index: u8,
        input: &str,
        phase: SessionPhase,
        with_plan: bool,
        pr_url: Option<&str>,
    ) -> String {
        let id = format!("2026083100000000{index}_{index:032x}");
        let mut state = SessionState::new(
            id.clone(),
            self.root.path().to_path_buf(),
            "__builtin__".to_string(),
            input.to_string(),
        );
        state.phase = phase;
        state.pr_url = pr_url.map(ToString::to_string);
        self.manager
            .create(&state)
            .unwrap_or_else(|error| panic!("{error}"));
        if with_plan {
            std::fs::write(
                state.plan_path(&self.manager.sessions_dir()),
                format!("# {input} plan\n"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        }
        id
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        // Ignore GIT_CONFIG_* pairs inherited from an outer cruise commit guard.
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct PtySession {
    child: Child,
    input: Option<ChildStdin>,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    columns: u16,
    rows: u16,
}
impl PtySession {
    fn start(binary: &Path, fixture: &Fixture, columns: u16, rows: u16, no_color: bool) -> Self {
        let stdout_path = fixture.root.path().join("tui.stdout");
        let stderr_path = fixture.root.path().join("tui.stderr");
        let stdout = File::create(&stdout_path).unwrap_or_else(|error| panic!("{error}"));
        let stderr = File::create(&stderr_path).unwrap_or_else(|error| panic!("{error}"));
        let command_line = format!(
            "stty cols {columns} rows {rows}; exec {}",
            shell_quote(&binary.to_string_lossy())
        );
        let mut command = Command::new("script");
        #[cfg(target_os = "macos")]
        command.args(["-q", "/dev/null", "/bin/sh", "-c", &command_line]);
        #[cfg(target_os = "linux")]
        command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);
        fixture.configure(&mut command, no_color);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|error| panic!("failed to start TUI in PTY: {error}"));
        let input = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("script stdin should be piped"));
        let session = Self {
            child,
            input: Some(input),
            stdout_path,
            stderr_path,
            columns,
            rows,
        };
        session.wait_for_output(
            if columns < 80 || rows < 24 {
                "Terminal too small"
            } else {
                "CRUISE"
            },
            START_TIMEOUT,
        );
        session
    }

    fn send(&mut self, bytes: &[u8]) {
        let input = self
            .input
            .as_mut()
            .unwrap_or_else(|| panic!("PTY input is closed"));
        input
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("failed to send TUI input: {error}"));
        input
            .flush()
            .unwrap_or_else(|error| panic!("failed to flush TUI input: {error}"));
        thread::sleep(Duration::from_millis(150));
    }

    fn wait_for_output(&self, expected: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let screen = self.screen();
            if screen.contains(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "PTY screen missing {expected:?}:\n{screen}\nraw transcript: {}",
                self.transcript()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn screen(&self) -> String {
        let bytes = std::fs::read(&self.stdout_path).unwrap_or_default();
        let mut parser = vt100::Parser::new(self.rows, self.columns, 0);
        parser.process(&bytes);
        parser.screen().contents()
    }

    fn transcript(&self) -> String {
        let mut bytes = std::fs::read(&self.stdout_path).unwrap_or_default();
        bytes.extend(std::fs::read(&self.stderr_path).unwrap_or_default());
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn finish(mut self) -> (ExitStatus, String) {
        drop(self.input.take());
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let status = loop {
            match self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("failed to poll script: {error}"))
            {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    panic!("cruise did not exit; transcript: {}", self.transcript());
                }
                None => thread::sleep(Duration::from_millis(25)),
            }
        };
        (status, self.transcript())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Some(input) = self.input.as_mut() {
            let _ = input.write_all(b"q\r");
            let _ = input.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tui_available() -> bool {
    match Command::new("script").arg("-V").output() {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping TUI PTY E2E: script is unavailable");
            false
        }
        Err(error) => panic!("failed to probe script: {error}"),
    }
}

fn assert_palette(tui: &mut PtySession, actions: &[&str]) {
    tui.send(b"a");
    for action in actions {
        tui.wait_for_output(action, START_TIMEOUT);
    }
    tui.send(b"\x1b");
}

#[test]
fn navigation_modals_validation_and_terminal_lifecycle_work() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(120, 30, true);
    tui.wait_for_output("No sessions yet", START_TIMEOUT);

    tui.send(b"2");
    tui.wait_for_output("Task description", START_TIMEOUT);
    tui.send(b"\x1b[F");
    tui.send(b"\r");
    tui.wait_for_output(
        "Task description or an image attachment is required",
        START_TIMEOUT,
    );
    tui.send(b"\x1b");
    tui.send(b"3");
    tui.wait_for_output("PARALLELISM", START_TIMEOUT);
    tui.send(b"a");
    tui.wait_for_output("Run all Planned and Suspended sessions?", START_TIMEOUT);
    tui.send(b"\x1b");
    tui.send(b"1");
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    for expected in ["\u{1b}[?1049l", "\u{1b}[?25h"] {
        assert!(
            transcript.contains(expected),
            "PTY transcript missing {expected:?}: {transcript}"
        );
    }
    assert!(
        !transcript.contains("\u{1b}[38;2;"),
        "NO_COLOR emitted truecolor styling: {transcript}"
    );
}

#[test]
fn new_session_form_saves_a_draft_without_planning() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(120, 30, true);

    tui.send(b"2");
    tui.send(b"\r");
    tui.send(b"draft created through terminal e2e");
    tui.send(b"\x1b");
    tui.send(b"\x1b[F");
    tui.send(b"\x1b[Z");
    tui.send(b"\r");
    tui.wait_for_output("Saved draft", START_TIMEOUT);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let sessions = fixture
        .manager
        .list()
        .unwrap_or_else(|error| panic!("{error}"));
    let draft = sessions
        .iter()
        .find(|session| session.input == "draft created through terminal e2e")
        .unwrap_or_else(|| panic!("saved draft missing: {sessions:?}"));
    assert_eq!(draft.phase, SessionPhase::Draft);
}

#[test]
fn new_session_form_creates_a_planned_session_with_selected_options() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(120, 30, true);

    tui.send(b"2");
    tui.send(b"\r");
    tui.send(b"planned through terminal e2e");
    tui.send(b"\x1b");
    tui.send(b"\t\t\t\t\t\t");
    tui.send(b" ");
    tui.send(b"\t ");
    tui.send(b"\t\t\t ");
    tui.send(b"\x1b[F");
    tui.send(b"\r");
    tui.wait_for_output("Planning finished: Planned", START_TIMEOUT);
    thread::sleep(Duration::from_millis(250));
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let sessions = fixture
        .manager
        .list()
        .unwrap_or_else(|error| panic!("{error}"));
    let planned = sessions
        .iter()
        .find(|session| session.input == "planned through terminal e2e")
        .unwrap_or_else(|| panic!("planned session missing: {sessions:?}"));
    assert_eq!(planned.phase, SessionPhase::Planned);
    assert_eq!(planned.workspace_mode, WorkspaceMode::CurrentBranch);
    assert!(planned.allow_dirty_working_tree);
    let plan = std::fs::read_to_string(planned.plan_path(&fixture.manager.sessions_dir()))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(plan.contains("planned through terminal e2e"));
}

#[test]
fn run_all_executes_a_planned_session_and_details_remain_browsable() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let id = fixture.seed_planned_session();
    let mut tui = fixture.start(120, 30, false);

    tui.wait_for_output("E2E terminal session", START_TIMEOUT);
    tui.send(b"]");
    tui.wait_for_output("Selected node", START_TIMEOUT);
    tui.send(b"]");
    tui.wait_for_output("E2E Plan", START_TIMEOUT);
    tui.send(b"]");
    tui.wait_for_output("seeded-log-line", START_TIMEOUT);
    tui.send(b"f");
    tui.wait_for_output("Log follow paused", START_TIMEOUT);
    tui.send(b"a");
    tui.wait_for_output("Run on Current Branch", START_TIMEOUT);
    tui.send(b"\x1b");
    tui.send(b"3");
    tui.send(b"a");
    tui.send(b"\r");
    tui.wait_for_output("Run All finished", Duration::from_secs(15));
    thread::sleep(Duration::from_millis(250));
    tui.send(b"1");
    tui.wait_for_output("e2e-run-complete", START_TIMEOUT);
    tui.send(b"a");
    tui.wait_for_output("Reset to Planned", START_TIMEOUT);
    tui.send(b"\x1b");
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let state = fixture
        .manager
        .load(&id)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(state.phase, SessionPhase::Completed);
    let log = std::fs::read_to_string(fixture.manager.run_log_path(&id))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(log.contains("e2e-run-complete"), "run log: {log}");
}

#[test]
fn undersized_terminal_shows_resize_notice_and_restores_terminal() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(79, 23, true);
    tui.wait_for_output("Terminal too small", START_TIMEOUT);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    assert!(transcript.contains("\u{1b}[?1049l"));
    assert!(transcript.contains("\u{1b}[?25h"));
}

#[test]
fn minimum_supported_terminal_keeps_navigation_and_help_usable() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(80, 24, true);

    tui.wait_for_output("No sessions yet", START_TIMEOUT);
    tui.send(b"2");
    tui.wait_for_output("Create session and start", START_TIMEOUT);
    tui.send(b"?");
    tui.wait_for_output("Keyboard-only; no mouse or child-owned TTY.", START_TIMEOUT);
    tui.send(b"\x1b");
    tui.send(b"3");
    tui.wait_for_output("Batch log", START_TIMEOUT);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
}

#[test]
fn new_session_form_autosaves_edited_text() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(120, 30, true);

    tui.send(b"2");
    tui.send(b"\r");
    tui.send(b"autosaved through terminal e2e");
    tui.send(b"\x1b");
    tui.wait_for_output("Draft autosaved", START_TIMEOUT);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let path = fixture.home.join("state/cruise/new_session_draft.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{error}"));
    let draft: cruise::new_session_draft::NewSessionDraft =
        serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(draft.input, "autosaved through terminal e2e");
    assert!(!draft.updated_at.is_empty());
}

#[test]
fn phase_specific_action_palettes_expose_the_supported_operations() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    fixture.seed_display_session(1, "phase draft", SessionPhase::Draft, false, None);
    fixture.seed_display_session(
        2,
        "phase approval",
        SessionPhase::AwaitingApproval,
        true,
        None,
    );
    fixture.seed_display_session(3, "phase planned", SessionPhase::Planned, true, None);
    fixture.seed_display_session(
        4,
        "phase failed",
        SessionPhase::Failed("expected failure".to_string()),
        false,
        None,
    );
    fixture.seed_display_session(5, "phase suspended", SessionPhase::Suspended, false, None);
    fixture.seed_display_session(
        6,
        "phase completed",
        SessionPhase::Completed,
        false,
        Some("https://example.com/pull/1"),
    );
    let mut tui = fixture.start(120, 30, true);

    tui.wait_for_output("phase draft", START_TIMEOUT);
    assert_palette(&mut tui, &["Generate Plan", "Delete"]);
    tui.send(b"j");
    assert_palette(&mut tui, &["Approve", "Ask About Plan"]);
    tui.send(b"j");
    assert_palette(&mut tui, &["Run in Worktree", "Replan"]);
    tui.send(b"j");
    assert_palette(&mut tui, &["Retry", "Edit Current Step"]);
    tui.send(b"j");
    assert_palette(&mut tui, &["Resume"]);
    tui.send(b"j");
    assert_palette(&mut tui, &["Open Pull Request"]);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
}

#[test]
fn destructive_actions_cancel_cleanly_then_apply_after_confirmation() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let draft_id =
        fixture.seed_display_session(1, "destructive draft", SessionPhase::Draft, false, None);
    let completed_id = fixture.seed_display_session(
        2,
        "destructive completed",
        SessionPhase::Completed,
        false,
        None,
    );
    let mut tui = fixture.start(120, 30, true);

    tui.send(b"a");
    tui.send(b"j");
    tui.send(b"\r");
    tui.wait_for_output(&format!("Delete {draft_id}?"), START_TIMEOUT);
    tui.send(b"\x1b");
    assert!(fixture.manager.load(&draft_id).is_ok());
    tui.send(b"a");
    tui.send(b"j");
    tui.send(b"\r");
    tui.send(b"\r");
    tui.wait_for_output("destructive completed", START_TIMEOUT);
    assert!(fixture.manager.load(&draft_id).is_err());

    tui.send(b"a");
    tui.send(b"\r");
    tui.wait_for_output(&format!("Reset to Planned {completed_id}?"), START_TIMEOUT);
    tui.send(b"\r");
    tui.wait_for_output("Planned", START_TIMEOUT);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let completed = fixture
        .manager
        .load(&completed_id)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(completed.phase, SessionPhase::Planned);
}

#[test]
fn active_run_all_can_be_cancelled_and_suspends_the_session() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let id = fixture.seed_cancellable_session();
    let mut tui = fixture.start(120, 30, true);

    tui.send(b"3");
    tui.send(b"a");
    tui.send(b"\r");
    tui.wait_for_output(": verify", START_TIMEOUT);
    tui.send(b"a");
    tui.wait_for_output("Cancel Run All and suspend active sessions?", START_TIMEOUT);
    tui.send(b"\r");
    tui.wait_for_output("Run All cancelled", Duration::from_secs(15));
    thread::sleep(Duration::from_millis(250));
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let state = fixture
        .manager
        .load(&id)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(state.phase, SessionPhase::Suspended);
}
