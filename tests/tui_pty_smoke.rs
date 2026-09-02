#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
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
        self.start_with_options_in_directory(columns, rows, no_color, None, None)
    }

    fn start_with_options(
        &self,
        columns: u16,
        rows: u16,
        no_color: bool,
        path_prefix: Option<&Path>,
    ) -> PtySession {
        self.start_with_options_in_directory(columns, rows, no_color, path_prefix, None)
    }

    fn start_in_directory(
        &self,
        columns: u16,
        rows: u16,
        no_color: bool,
        working_dir: &Path,
    ) -> PtySession {
        self.start_with_options_in_directory(columns, rows, no_color, None, Some(working_dir))
    }

    fn start_with_options_in_directory(
        &self,
        columns: u16,
        rows: u16,
        no_color: bool,
        path_prefix: Option<&Path>,
        working_dir: Option<&Path>,
    ) -> PtySession {
        PtySession::start(
            Path::new(env!("CARGO_BIN_EXE_cruise")),
            self,
            columns,
            rows,
            no_color,
            path_prefix,
            working_dir,
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
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
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

    fn configure_with_options(
        &self,
        command: &mut Command,
        no_color: bool,
        path_prefix: Option<&Path>,
    ) {
        self.configure(command, no_color);
        if let Some(prefix) = path_prefix {
            let mut paths = vec![prefix.to_path_buf()];
            if let Some(path) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&path));
            }
            let path = std::env::join_paths(paths)
                .unwrap_or_else(|error| panic!("failed to construct PTY PATH: {error}"));
            command.env("PATH", path);
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
    fn start(
        binary: &Path,
        fixture: &Fixture,
        columns: u16,
        rows: u16,
        no_color: bool,
        path_prefix: Option<&Path>,
        working_dir: Option<&Path>,
    ) -> Self {
        let stdout_path = fixture.root.path().join("tui.stdout");
        let stderr_path = fixture.root.path().join("tui.stderr");
        let stdout = File::create(&stdout_path).unwrap_or_else(|error| panic!("{error}"));
        let stderr = File::create(&stderr_path).unwrap_or_else(|error| panic!("{error}"));
        let command_line = match working_dir {
            Some(directory) => format!(
                "cd {}; stty cols {columns} rows {rows}; exec {}",
                shell_quote(&directory.to_string_lossy()),
                shell_quote(&binary.to_string_lossy())
            ),
            None => format!(
                "stty cols {columns} rows {rows}; exec {}",
                shell_quote(&binary.to_string_lossy())
            ),
        };
        let mut command = Command::new("script");
        #[cfg(target_os = "macos")]
        command.args(["-q", "/dev/null", "/bin/sh", "-c", &command_line]);
        #[cfg(target_os = "linux")]
        command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);
        fixture.configure_with_options(&mut command, no_color, path_prefix);
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
        self.wait_for_screen(timeout, |screen| screen.contains(expected));
    }

    fn wait_for_screen<F>(&self, timeout: Duration, predicate: F) -> String
    where
        F: Fn(&str) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let screen = self.screen();
            if predicate(&screen) {
                return screen;
            }
            assert!(
                Instant::now() < deadline,
                "PTY screen did not satisfy predicate:\n{screen}\nraw transcript: {}",
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

#[test]
fn new_session_config_candidates_are_visible_selectable_and_persist_arbitrary_path() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let project = fixture.root.path().join("config-project");
    let custom_dir = project.join("custom");
    std::fs::create_dir_all(&custom_dir).unwrap_or_else(|error| panic!("{error}"));
    write_test_workflow(&project.join("cruise.yaml"), "local_step");
    write_test_workflow(&custom_dir.join("workflow.yaml"), "custom_step");

    let workflows_dir = fixture.home.join("config/cruise/workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|error| panic!("{error}"));
    write_test_workflow(&workflows_dir.join("user.yaml"), "user_step");

    let mut tui = fixture.start_in_directory(120, 30, true, &project);
    tui.send(b"2");
    tui.wait_for_output("What should cruise do?", START_TIMEOUT);
    tui.send(b"config candidate PTY test");
    tui.send(b"\t\t\t\t");
    tui.wait_for_output("Which workflow config?", START_TIMEOUT);
    tui.wait_for_output("Auto-detect", START_TIMEOUT);
    tui.wait_for_output("./cruise.yaml", START_TIMEOUT);

    tui.send(b"\x1b[B");
    tui.wait_for_output("▸ ./cruise.yaml", START_TIMEOUT);
    // Local config -> user workflow -> Built-in -> Auto-detect.
    tui.send(b"\x1b[B");
    tui.send(b"\x1b[B");
    tui.send(b"\x1b[B");
    tui.wait_for_output("▸ Auto-detect", START_TIMEOUT);

    tui.send(b"custom/workflow.yaml");
    tui.wait_for_output("custom/workflow.yaml", START_TIMEOUT);
    tui.send(b"\x13");
    tui.wait_for_output("Saved draft", START_TIMEOUT);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let session = fixture
        .manager
        .list()
        .unwrap_or_else(|error| panic!("{error}"))
        .into_iter()
        .find(|session| session.input == "config candidate PTY test")
        .unwrap_or_else(|| panic!("config PTY session was not persisted"));
    let expected_config_path = std::fs::canonicalize(custom_dir.join("workflow.yaml"))
        .unwrap_or_else(|error| panic!("failed to canonicalize expected config path: {error}"));
    let actual_config_path = session.config_path.map(|path| {
        std::fs::canonicalize(&path)
            .unwrap_or_else(|error| panic!("failed to canonicalize persisted config path: {error}"))
    });
    assert_eq!(actual_config_path, Some(expected_config_path));
}

fn write_test_workflow(path: &Path, step: &str) {
    std::fs::write(
        path,
        format!("command: [echo]\nsteps:\n  {step}:\n    command: echo {step}\n"),
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

fn assert_palette(tui: &mut PtySession, actions: &[&str]) {
    tui.send(b"a");
    for action in actions {
        tui.wait_for_output(action, START_TIMEOUT);
    }
    tui.send(b"\x1b");
}

#[cfg(unix)]
fn install_fake_jcode(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.join("jcode");
    let script = r#"#!/bin/sh
set -eu

case " $* " in
  *" version --json "*)
    printf '%s\n' '{"semver":"0.81.1"}'
    ;;
  *" auth status --json "*)
    printf '%s\n' '{"any_available":true,"providers":[{"id":"fake","status":"available"}]}'
    ;;
  *" run --ndjson "*)
    count_file="$0.count"
    if [ -f "$count_file" ]; then
      count=$(cat "$count_file")
    else
      count=0
    fi
    count=$((count + 1))
    printf '%s\n' "$count" > "$count_file"
    if [ "$count" -eq 1 ]; then
      printf '%s' "$CRUISE_TOOL_SOCKET" > "$0.socket"
      printf '%s\n' '{"type":"start","session_id":"fake-planning-session","provider":"fake"}'
      while [ ! -f "$0.complete" ]; do
        sleep 0.02
      done
    else
      printf '%s\n' '{"type":"start","session_id":"fake-follow-up","provider":"fake"}'
    fi
    printf '%s\n' '{"type":"done","text":"fake planning turn complete"}'
    ;;
  *)
    printf '%s\n' 'unexpected fake jcode invocation' >&2
    exit 1
    ;;
esac
"#;
    std::fs::write(&path, script).unwrap_or_else(|error| panic!("{error}"));
    let mut permissions = std::fs::metadata(&path)
        .unwrap_or_else(|error| panic!("{error}"))
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap_or_else(|error| panic!("{error}"));
    path
}

#[cfg(unix)]
fn call_tool(
    socket: &Path,
    id: u64,
    name: &str,
    arguments: &serde_json::Value,
) -> serde_json::Value {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket)
        .unwrap_or_else(|error| panic!("failed to connect to ToolBridge socket: {error}"));
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    writeln!(stream, "{request}")
        .unwrap_or_else(|error| panic!("failed to send ToolBridge request: {error}"));
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .unwrap_or_else(|error| panic!("failed to read ToolBridge response: {error}"));
    serde_json::from_str(response.trim())
        .unwrap_or_else(|error| panic!("invalid ToolBridge response: {error}: {response}"))
}

#[cfg(unix)]
fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
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
    tui.wait_for_output("What should cruise do?", START_TIMEOUT);
    tui.send(b"\t");
    tui.wait_for_output("Any images to attach?", START_TIMEOUT);
    tui.send(b"\t");
    tui.wait_for_output(
        "Task description or an image attachment is required",
        START_TIMEOUT,
    );
    // Esc closes the error, then backs out of the dialogue one question at a time.
    for _ in 0..3 {
        tui.send(b"\x1b");
        thread::sleep(Duration::from_millis(100));
    }
    tui.wait_for_output("No sessions yet", START_TIMEOUT);
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
    tui.wait_for_output("What should cruise do?", START_TIMEOUT);
    tui.send(b"draft created through terminal e2e");
    tui.send(b"\x13");
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
fn new_session_ctrl_p_validates_before_starting_planning() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(120, 30, true);

    tui.send(b"n");
    tui.send(b"\x10");
    tui.wait_for_output(
        "Task description or an image attachment is required",
        START_TIMEOUT,
    );
    // First Esc closes the error, the second leaves the task question so `q` quits.
    for _ in 0..2 {
        tui.send(b"\x1b");
        thread::sleep(Duration::from_millis(100));
    }
    tui.wait_for_output("No sessions yet", START_TIMEOUT);
    tui.send(b"q");

    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    let sessions = fixture
        .manager
        .list()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        sessions.is_empty(),
        "validation unexpectedly created a session"
    );
}

#[test]
fn new_session_form_applies_workspace_options_with_ctrl_u() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let mut tui = fixture.start(120, 30, true);

    tui.send(b"2");
    tui.wait_for_output("What should cruise do?", START_TIMEOUT);
    tui.send(b"planned through terminal e2e");
    // Task → Images → Source → Working directory → Config → Skipped steps → Workspace
    tui.send(b"\t\t\t\t\t\t");
    tui.wait_for_output("Where should cruise execute?", START_TIMEOUT);
    tui.send(b" ");
    tui.send(b"\t");
    tui.wait_for_output("even with uncommitted changes?", START_TIMEOUT);
    tui.send(b" ");
    tui.send(b"\x15");
    tui.wait_for_output("Phase    Planned", START_TIMEOUT);
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
    tui.send(b"?");
    tui.wait_for_output("Keyboard-only; no mouse or child-owned TTY.", START_TIMEOUT);
    tui.send(b"\x1b");
    thread::sleep(Duration::from_millis(100));
    tui.send(b"2");
    tui.wait_for_output("question 1 of 9", START_TIMEOUT);
    tui.wait_for_output("Ctrl+P/G/U start now", START_TIMEOUT);
    tui.send(b"\x1b");
    thread::sleep(Duration::from_millis(100));
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
    tui.wait_for_output("What should cruise do?", START_TIMEOUT);
    tui.send(b"autosaved through terminal e2e");
    // Esc at the first question leaves the dialogue so `q` quits instead of typing.
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

#[test]
#[cfg(unix)]
fn ask_user_uses_the_real_pty_path_for_multiline_display_and_immediate_dismiss() {
    if !tui_available() {
        return;
    }
    let fixture = Fixture::new();
    let fake_bin = fixture.root.path().join("fake-bin");
    std::fs::create_dir_all(&fake_bin).unwrap_or_else(|error| panic!("{error}"));
    let fake_jcode = install_fake_jcode(&fake_bin);
    let mut tui = fixture.start_with_options(120, 30, true, Some(&fake_bin));

    tui.send(b"2");
    tui.wait_for_output("What should cruise do?", START_TIMEOUT);
    tui.send(b"interactive ask user e2e");
    tui.send(b"\x10");
    let socket_file = PathBuf::from(format!("{}.socket", fake_jcode.display()));
    wait_for_file(&socket_file, START_TIMEOUT);
    let socket = std::fs::read_to_string(&socket_file)
        .unwrap_or_else(|error| panic!("failed to read ToolBridge socket path: {error}"));
    let socket = PathBuf::from(socket);
    wait_for_file(&socket, START_TIMEOUT);

    let ask_thread = thread::spawn({
        let socket = socket.clone();
        move || {
            call_tool(
                &socket,
                1,
                "ask_user",
                &serde_json::json!({ "question": "First line\nSecond line" }),
            )
        }
    });
    let prompt_screen = tui.wait_for_screen(START_TIMEOUT, |screen| {
        let rows = screen.lines().collect::<Vec<_>>();
        rows.windows(2)
            .any(|pair| pair[0].contains("First line") && pair[1].contains("Second line"))
            && screen.contains("Prompt")
            && screen.contains("Enter submit")
    });
    assert!(prompt_screen.contains("First line"));
    assert!(prompt_screen.contains("Second line"));

    tui.send(b"answer from PTY\r");
    let response = ask_thread
        .join()
        .unwrap_or_else(|_| panic!("ask_user ToolBridge thread panicked"));
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(response["result"]["content"][0]["text"], "answer from PTY");

    let cleared_screen = tui.wait_for_screen(START_TIMEOUT, |screen| {
        !screen.contains("First line")
            && !screen.contains("Second line")
            && !screen.contains("Enter submit")
            && !screen.contains("Prompt")
    });
    assert!(!cleared_screen.contains("First line"));
    assert!(!cleared_screen.contains("Enter submit"));

    let submit = call_tool(
        &socket,
        2,
        "submit_plan",
        &serde_json::json!({
            "content": "# Interactive E2E Plan\n\n- preserve the answer path\n"
        }),
    );
    assert_eq!(submit["result"]["isError"], false);
    std::fs::write(format!("{}.complete", fake_jcode.display()), b"done")
        .unwrap_or_else(|error| panic!("failed to release fake jcode: {error}"));

    tui.wait_for_screen(START_TIMEOUT, |screen| screen.contains("Awaiting Approval"));
    tui.send(b"q");
    let (status, transcript) = tui.finish();
    assert!(status.success(), "cruise failed in PTY: {transcript}");
    assert!(transcript.contains("\u{1b}[?1049l"));
    assert!(transcript.contains("\u{1b}[?25h"));

    let session = fixture
        .manager
        .list()
        .unwrap_or_else(|error| panic!("{error}"))
        .into_iter()
        .find(|session| session.input == "interactive ask user e2e")
        .unwrap_or_else(|| panic!("interactive session was not persisted"));
    assert!(matches!(
        session.phase,
        SessionPhase::AwaitingApproval | SessionPhase::Planned
    ));
    let plan = std::fs::read_to_string(session.plan_path(&fixture.manager.sessions_dir()))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(plan.contains("Interactive E2E Plan"));
}
