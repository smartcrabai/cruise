#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);
const LOADER_FRAME_CHARS: [char; 4] = ['-', '/', '|', '\\'];

static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    root: TempDir,
    home: PathBuf,
    fake_bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let home = root.path().join("home");
        let fake_bin = root.path().join("fake-bin");
        for directory in [
            home.clone(),
            home.join("config"),
            home.join("data"),
            home.join("state"),
            fake_bin.clone(),
        ] {
            fs::create_dir_all(directory).unwrap_or_else(|error| panic!("{error}"));
        }
        Self {
            root,
            home,
            fake_bin,
        }
    }

    fn configure<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        let mut paths = vec![self.fake_bin.clone()];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        let path = std::env::join_paths(paths)
            .unwrap_or_else(|error| panic!("failed to construct test PATH: {error}"));
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("PATH", path)
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
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
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
    }

    fn write_sdk_config(&self, interactive_planning: bool) -> PathBuf {
        let path = self.root.path().join("sdk.yaml");
        let yaml = format!(
            "sdk: jcode\ninteractive_planning: {interactive_planning}\nsteps:\n  check:\n    prompt: check\n"
        );
        fs::write(&path, yaml).unwrap_or_else(|error| panic!("{error}"));
        path
    }

    fn write_command_config(&self, control: &Path) -> PathBuf {
        let path = self.root.path().join("command.yaml");
        let script = concat!(
            "touch \"$PLANNING_STARTED\"; ",
            "while [ ! -f \"$PLANNING_RELEASE\" ]; do sleep 0.02; done; ",
            "printf '%s' '# Command Plan\\n\\n- complete\\n' > \"$PLAN_FILE\""
        );
        let yaml = format!(
            "command:\n  - sh\n  - -c\n  - {}\nenv:\n  PLANNING_STARTED: {}\n  PLANNING_RELEASE: {}\n  PLAN_FILE: \"{{plan}}\"\nsteps:\n  check:\n    prompt: check\n",
            yaml_scalar(script),
            yaml_scalar(&control.with_extension("started").to_string_lossy()),
            yaml_scalar(&control.with_extension("release").to_string_lossy()),
        );
        fs::write(&path, yaml).unwrap_or_else(|error| panic!("{error}"));
        path
    }

    fn write_failing_command_config(&self, control: &Path) -> PathBuf {
        let path = self.root.path().join("failing-command.yaml");
        let script = concat!(
            "touch \"$PLANNING_STARTED\"; ",
            "while [ ! -f \"$PLANNING_RELEASE\" ]; do sleep 0.02; done; ",
            "printf '%s\\n' 'backend failed' >&2; exit 1"
        );
        let yaml = format!(
            "command:\n  - sh\n  - -c\n  - {}\nenv:\n  PLANNING_STARTED: {}\n  PLANNING_RELEASE: {}\nsteps:\n  check:\n    prompt: check\n",
            yaml_scalar(script),
            yaml_scalar(&control.with_extension("started").to_string_lossy()),
            yaml_scalar(&control.with_extension("release").to_string_lossy()),
        );
        fs::write(&path, yaml).unwrap_or_else(|error| panic!("{error}"));
        path
    }

    fn install_fake_jcode(&self) -> PathBuf {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt as _;

        let path = self.fake_bin.join("jcode");
        let script = r##"#!/bin/sh
set -eu

case " $* " in
  *" version --json "*)
    printf '%s\n' '{"semver":"0.82.0"}'
    ;;
  *" auth status --json "*)
    printf '%s\n' '{"any_available":true,"providers":[{"id":"fake","status":"available"}]}'
    ;;
  *" run --ndjson "*)
    printf '%s\n' '{"type":"start","session_id":"fake-planning-session","provider":"fake"}'
    : > "$FAKE_CONTROL.started"
    if [ -n "${CRUISE_TOOL_SOCKET:-}" ]; then
      printf '%s' "$CRUISE_TOOL_SOCKET" > "$FAKE_CONTROL.socket"
    fi
    while [ ! -f "$FAKE_CONTROL.release" ]; do
      sleep 0.02
    done
    printf '%s\n' '{"type":"done","text":"# Silent SDK Plan\n\n- complete"}'
    ;;
  *)
    printf '%s\n' 'unexpected fake jcode invocation' >&2
    exit 1
    ;;
esac
"##;
        fs::write(&path, script).unwrap_or_else(|error| panic!("{error}"));
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&path)
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap_or_else(|error| panic!("{error}"));
        }
        path
    }

    fn start_pty(&self, args: &[&str], control: &Path) -> PtySession {
        let stdout_path = self.root.path().join("pty.stdout");
        let stderr_path = self.root.path().join("pty.stderr");
        let stdout = File::create(&stdout_path).unwrap_or_else(|error| panic!("{error}"));
        let stderr = File::create(&stderr_path).unwrap_or_else(|error| panic!("{error}"));
        let binary = Path::new(env!("CARGO_BIN_EXE_cruise"));
        let mut command = Command::new("script");
        let command_line = std::iter::once(binary.to_string_lossy().into_owned())
            .chain(args.iter().map(|arg| (*arg).to_string()))
            .map(|arg| shell_quote(&arg))
            .collect::<Vec<_>>()
            .join(" ");
        let command_line = format!("stty cols 160 rows 40; exec {command_line}");

        #[cfg(target_os = "macos")]
        {
            command.args(["-q", "/dev/null", "/bin/sh", "-c", &command_line]);
        }

        #[cfg(target_os = "linux")]
        {
            command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);
        }

        let mut child = self
            .configure(&mut command)
            .env("FAKE_CONTROL", control)
            .current_dir(self.root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|error| panic!("failed to start script: {error}"));
        // Take stdin after construction so the child remains owned by the PTY
        // session and the writer can be closed explicitly during teardown.
        let input = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("script stdin should be piped"));
        PtySession {
            child,
            input: Some(input),
            stdout_path,
            stderr_path,
            control: control.to_path_buf(),
            keyboard_queries_answered: 0,
            device_queries_answered: 0,
            cursor_queries_answered: 0,
        }
    }
}

struct PtySession {
    child: Child,
    input: Option<std::process::ChildStdin>,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    control: PathBuf,
    keyboard_queries_answered: usize,
    device_queries_answered: usize,
    cursor_queries_answered: usize,
}

impl PtySession {
    fn send(&mut self, bytes: &[u8]) {
        let input = self
            .input
            .as_mut()
            .unwrap_or_else(|| panic!("PTY input is already closed"));
        if input.write_all(bytes).is_ok() {
            input
                .flush()
                .unwrap_or_else(|error| panic!("failed to flush PTY input: {error}"));
        }
    }

    fn raw(&self) -> String {
        let mut bytes = fs::read(&self.stdout_path).unwrap_or_default();
        bytes.extend(fs::read(&self.stderr_path).unwrap_or_default());
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn screen(&self) -> String {
        let bytes = fs::read(&self.stdout_path).unwrap_or_default();
        let mut parser = vt100::Parser::new(40, 160, 0);
        parser.process(&bytes);
        parser.screen().contents()
    }

    fn wait_for_screen<F>(&mut self, predicate: F) -> String
    where
        F: Fn(&str) -> bool,
    {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            self.answer_terminal_queries();
            let screen = self.screen();
            if predicate(&screen) {
                return screen;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for PTY screen predicate; screen:\n{screen}\nraw:\n{}",
                self.raw()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_file(&mut self, path: &Path) {
        let deadline = Instant::now() + START_TIMEOUT;
        while !path.exists() {
            self.answer_terminal_queries();
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}\nscreen:\n{}\nraw:\n{}",
                path.display(),
                self.screen(),
                self.raw()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn answer_terminal_queries(&mut self) {
        let raw = self.raw();
        let keyboard_query_count = raw.matches("\x1b[?u").count();
        let device_query_count = raw.matches("\x1b[c").count();
        let cursor_query_count = raw.matches("\x1b[6n").count();
        while self.keyboard_queries_answered < keyboard_query_count {
            if let Some(input) = self.input.as_mut() {
                let _ = input.write_all(b"\x1b[?1u");
                let _ = input.flush();
            }
            self.keyboard_queries_answered += 1;
        }
        while self.device_queries_answered < device_query_count {
            if let Some(input) = self.input.as_mut() {
                let _ = input.write_all(b"\x1b[?1;2c");
                let _ = input.flush();
            }
            self.device_queries_answered += 1;
        }
        while self.cursor_queries_answered < cursor_query_count {
            if let Some(input) = self.input.as_mut() {
                let _ = input.write_all(b"\x1b[1;1R");
                let _ = input.flush();
            }
            self.cursor_queries_answered += 1;
        }
    }

    fn finish(mut self) -> (ExitStatus, String) {
        self.input.take();
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("failed to poll script: {error}"))
            {
                Some(status) => return (status, self.raw()),
                None if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    let status = self
                        .child
                        .wait()
                        .unwrap_or_else(|error| panic!("failed to reap script: {error}"));
                    return (status, self.raw());
                }
                None => thread::sleep(Duration::from_millis(25)),
            }
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = fs::write(self.control.with_extension("release"), b"cleanup");
        if let Some(input) = self.input.as_mut() {
            let _ = input.write_all(b"\x1b");
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

fn yaml_scalar(value: &str) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|error| panic!("failed to serialize YAML scalar: {error}"))
        .trim_end()
        .to_string()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

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

fn planning_frame_chars(raw: &str) -> Vec<char> {
    let mut frames = Vec::new();
    for (index, _) in raw.match_indices("Planning...") {
        let frame = raw[..index]
            .chars()
            .rev()
            .take(8)
            .find(|character| LOADER_FRAME_CHARS.contains(character));
        if let Some(frame) = frame
            && !frames.contains(&frame)
        {
            frames.push(frame);
        }
    }
    frames
}

fn wait_for_loader_frames(session: &PtySession, minimum: usize) {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let frames = planning_frame_chars(&session.raw());
        if frames.len() >= minimum {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {minimum} distinct Planning... frames; raw transcript:\n{}",
            session.raw()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

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

fn run_cruise(fixture: &Fixture, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    fixture
        .configure(&mut command)
        .args(args)
        .current_dir(fixture.root.path())
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise {args:?}: {error}"))
}

#[test]
#[cfg(unix)]
fn cli_plan_renders_sdk_loader_during_silent_turn_and_clears_it() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Given: a TTY CLI plan using the SDK backend, with no streamed output and
    // no interactive planning tools so the fake turn can finish with text.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("sdk-silent");
    let _fake_jcode = fixture.install_fake_jcode();
    let config = fixture.write_sdk_config(false);
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let mut pty = fixture.start_pty(
        &["plan", "--config", config_arg, "silent SDK planning"],
        &control,
    );

    // When: the backend has started and remains silent.
    pty.wait_for_file(&control.with_extension("started"));
    wait_for_loader_frames(&pty, 2);

    // Then: releasing the backend lets planning complete, and the transient
    // loader is absent from the final terminal screen.
    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release fake jcode: {error}"));
    pty.wait_for_screen(|screen| screen.contains("Action:"));
    pty.send(b"\x1b");
    let (status, raw) = pty.finish();
    assert!(
        status.success(),
        "cruise plan failed; raw transcript:\n{raw}"
    );
    let mut parser = vt100::Parser::new(40, 160, 0);
    parser.process(raw.as_bytes());
    assert!(
        !parser.screen().contents().contains("Planning..."),
        "loader remained on the final screen:\n{}",
        parser.screen().contents()
    );
}

#[test]
#[cfg(unix)]
#[expect(
    clippy::too_many_lines,
    reason = "the PTY lifecycle test covers question display, pause, resume, and teardown"
)]
fn cli_plan_pauses_loader_for_ask_user_and_resumes_after_answer() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Given: an interactive SDK planning turn whose ToolBridge asks a multiline
    // question while the backend remains alive.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("sdk-ask");
    let _fake_jcode = fixture.install_fake_jcode();
    let config = fixture.write_sdk_config(true);
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let mut pty = fixture.start_pty(
        &["plan", "--config", config_arg, "SDK ask planning"],
        &control,
    );
    pty.wait_for_file(&control.with_extension("socket"));
    let socket = fs::read_to_string(control.with_extension("socket"))
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
    let question_screen = pty
        .wait_for_screen(|screen| screen.contains("First line") && screen.contains("Second line"));
    let frames_while_waiting = planning_frame_chars(&pty.raw());
    let loader_rendered_before_question = frames_while_waiting.len() >= 2;

    // The question editor owns the terminal while the handler is blocked. No
    // new loader frame may be drawn over the two-line question.
    pty.answer_terminal_queries();
    let frame_count = pty.raw().matches("Planning...").count();
    let stable_deadline = Instant::now() + Duration::from_millis(300);
    let mut loader_paused_while_waiting = true;
    while Instant::now() < stable_deadline {
        pty.answer_terminal_queries();
        if pty.raw().matches("Planning...").count() != frame_count {
            loader_paused_while_waiting = false;
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    pty.send(b"answer from PTY\r");
    let response = ask_thread
        .join()
        .unwrap_or_else(|_| panic!("ask_user ToolBridge thread panicked"));
    assert_eq!(
        response["result"]["isError"], false,
        "ask_user response: {response}"
    );
    assert_eq!(response["result"]["content"][0]["text"], "answer from PTY");
    assert!(pty.raw().contains("answer from PTY"));

    let frames_before_resume = pty.raw().matches("Planning...").count();
    let resume_deadline = Instant::now() + Duration::from_millis(500);
    let mut loader_resumed_after_answer = false;
    while Instant::now() < resume_deadline {
        if pty.raw().matches("Planning...").count() > frames_before_resume {
            loader_resumed_after_answer = true;
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    let submit = call_tool(
        &socket,
        2,
        "submit_plan",
        &serde_json::json!({
            "content": "# Answered Plan\n\n- preserve the input path\n"
        }),
    );
    assert_eq!(submit["result"]["isError"], false);
    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release fake jcode: {error}"));

    pty.wait_for_screen(|screen| screen.contains("Action:"));
    pty.send(b"\x1b");
    let (status, raw) = pty.finish();
    assert!(
        status.success(),
        "cruise plan failed after ask_user; raw transcript:\n{raw}"
    );
    assert!(
        loader_rendered_before_question,
        "SDK loader did not run before ask_user; screen:\n{question_screen}\nraw:\n{raw}"
    );
    assert!(
        loader_paused_while_waiting,
        "loader advanced while ask_user was waiting; raw:\n{raw}"
    );
    assert!(
        loader_resumed_after_answer,
        "loader did not resume after ask_user returned; raw:\n{raw}"
    );
    let mut parser = vt100::Parser::new(40, 160, 0);
    parser.process(raw.as_bytes());
    let screen = parser.screen().contents();
    assert!(
        !screen.contains("First line"),
        "question remained on screen: {screen}"
    );
    assert!(
        !screen.contains("Planning..."),
        "loader remained on screen: {screen}"
    );
}

#[test]
#[cfg(unix)]
fn cli_list_generate_plan_reuses_planning_loader_and_clears_it() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Given: a Draft session reachable through `cruise list` → Generate Plan.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("list-generate");
    let config = fixture.write_command_config(&control);
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let draft = run_cruise(
        &fixture,
        &["draft", "--config", config_arg, "list generate"],
    );
    assert!(
        draft.status.success(),
        "draft failed: {}{}",
        String::from_utf8_lossy(&draft.stdout),
        String::from_utf8_lossy(&draft.stderr)
    );

    // When: Generate Plan is selected from the interactive list menu.
    let mut pty = fixture.start_pty(&["list"], &control);
    pty.wait_for_screen(|screen| screen.contains("list generate"));
    pty.send(b"\r");
    pty.wait_for_screen(|screen| screen.contains("Action:"));
    pty.send(b"\r");
    pty.wait_for_file(&control.with_extension("started"));

    // Then: list's shared planning path renders and later clears the loader.
    wait_for_loader_frames(&pty, 2);
    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release command backend: {error}"));
    pty.wait_for_screen(|screen| screen.contains("Action:"));
    pty.send(b"\x1b");
    pty.send(b"\x1b");
    let (status, raw) = pty.finish();
    assert!(status.success(), "cruise list failed:\n{raw}");
    let mut parser = vt100::Parser::new(40, 160, 0);
    parser.process(raw.as_bytes());
    assert!(
        !parser.screen().contents().contains("Planning..."),
        "loader remained after Generate Plan returned to the list menu:\n{}",
        parser.screen().contents()
    );
}

#[test]
#[cfg(unix)]
fn cli_list_replan_reuses_planning_loader_and_preserves_planned_state() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Given: a Planned session that exposes the list menu's Replan action.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("list-replan");
    let config = fixture.write_command_config(&control);
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let initial = run_cruise(
        &fixture,
        &[
            "plan",
            "--skip-planning",
            "--config",
            config_arg,
            "list replan",
        ],
    );
    assert!(
        initial.status.success(),
        "initial plan failed: {}{}",
        String::from_utf8_lossy(&initial.stdout),
        String::from_utf8_lossy(&initial.stderr)
    );

    // When: Replan is selected and feedback is submitted.
    let mut pty = fixture.start_pty(&["list"], &control);
    pty.wait_for_screen(|screen| screen.contains("list replan"));
    pty.send(b"\r");
    pty.wait_for_screen(|screen| screen.contains("Action:"));
    for _ in 0..3 {
        pty.send(b"\x1b[B");
    }
    pty.send(b"\r");
    pty.wait_for_screen(|screen| screen.contains("Describe the changes needed:"));
    pty.send(b"keep the plan structure\r");
    pty.wait_for_file(&control.with_extension("started"));

    // Then: Replan uses the same loader boundary and leaves a Planned session
    // after the existing approved-state contract completes.
    wait_for_loader_frames(&pty, 2);
    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release command backend: {error}"));
    pty.wait_for_screen(|screen| screen.contains("Action:"));
    pty.send(b"\x1b");
    // The action editor restores terminal mode before the outer picker can
    // consume another key; wait for that observable boundary instead of
    // relying on two back-to-back Escape bytes.
    pty.wait_for_screen(|screen| screen.contains("Select a session:"));
    pty.send(b"\x1b");
    let (status, raw) = pty.finish();
    assert!(status.success(), "cruise list replan failed:\n{raw}");

    let listed = run_cruise(&fixture, &["list", "--json"]);
    assert!(
        listed.status.success(),
        "list --json failed: {}{}",
        String::from_utf8_lossy(&listed.stdout),
        String::from_utf8_lossy(&listed.stderr)
    );
    let sessions: serde_json::Value = serde_json::from_slice(&listed.stdout)
        .unwrap_or_else(|error| panic!("list --json was not valid JSON: {error}"));
    assert_eq!(sessions[0]["phase"], "Planned");
    assert!(!String::from_utf8_lossy(&listed.stdout).contains("Planning..."));
}

#[test]
#[cfg(unix)]
fn cli_plan_clears_loader_when_backend_fails() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Given: a TTY plan whose backend fails after a silent interval.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("failing-plan");
    let config = fixture.write_failing_command_config(&control);
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let mut pty = fixture.start_pty(
        &["plan", "--config", config_arg, "failing planning"],
        &control,
    );
    pty.wait_for_file(&control.with_extension("started"));
    wait_for_loader_frames(&pty, 2);

    // When: the backend is released to report its failure.
    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release failing backend: {error}"));
    pty.wait_for_screen(|screen| screen.contains("Plan generation failed"));
    let (status, raw) = pty.finish();

    // Then: the CLI reports failure and the dropped planning loader leaves no
    // residual frame on the terminal.
    assert!(
        !status.success(),
        "failing plan unexpectedly succeeded:\n{raw}"
    );
    let mut parser = vt100::Parser::new(40, 160, 0);
    parser.process(raw.as_bytes());
    assert!(
        !parser.screen().contents().contains("Planning..."),
        "loader remained after backend failure:\n{}",
        parser.screen().contents()
    );
}

#[test]
#[cfg(unix)]
fn redirected_stderr_keeps_planning_status_without_animation() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Given: planning with stderr redirected away from a terminal.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("redirected");
    let config = fixture.write_command_config(&control);
    let stderr_path = fixture.root.path().join("stderr.log");
    let stdout_path = fixture.root.path().join("stdout.log");
    let stderr = File::create(&stderr_path).unwrap_or_else(|error| panic!("{error}"));
    let stdout = File::create(&stdout_path).unwrap_or_else(|error| panic!("{error}"));
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    let mut child = fixture
        .configure(&mut command)
        .args(["plan", "--config", config_arg, "redirected planning"])
        .current_dir(fixture.root.path())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap_or_else(|error| panic!("failed to start redirected cruise: {error}"));

    wait_for_file(&control.with_extension("started"), START_TIMEOUT);
    let observation_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < observation_deadline {
        let output = fs::read_to_string(&stderr_path).unwrap_or_default();
        assert!(
            !output.contains("Planning...") && !output.contains("Cruising..."),
            "redirected stderr contains transient loader output:\n{output}"
        );
        assert!(
            !output.contains('\r'),
            "redirected stderr contains carriage-return animation output:\n{output}"
        );
        thread::sleep(Duration::from_millis(25));
    }

    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release command backend: {error}"));
    let deadline = Instant::now() + EXIT_TIMEOUT;
    let status = loop {
        match child
            .try_wait()
            .unwrap_or_else(|error| panic!("failed to poll redirected cruise: {error}"))
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break child
                    .wait()
                    .unwrap_or_else(|error| panic!("failed to reap redirected cruise: {error}"));
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };
    let output = fs::read_to_string(&stderr_path).unwrap_or_default();
    assert!(status.success(), "redirected cruise failed:\n{output}");
    assert!(
        output.contains("[plan] creating plan..."),
        "the stable planning status disappeared:\n{output}"
    );
    assert!(!output.contains("Planning..."));
    assert!(!output.contains("Cruising..."));
    assert!(!output.contains('\r'));
}

#[test]
#[cfg(unix)]
fn redirected_stderr_keeps_sdk_planning_status_without_animation() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Given: a non-TTY SDK planning run with no streamed output.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("redirected-sdk");
    let _fake_jcode = fixture.install_fake_jcode();
    let config = fixture.write_sdk_config(false);
    let stderr_path = fixture.root.path().join("sdk-stderr.log");
    let stdout_path = fixture.root.path().join("sdk-stdout.log");
    let stderr = File::create(&stderr_path).unwrap_or_else(|error| panic!("{error}"));
    let stdout = File::create(&stdout_path).unwrap_or_else(|error| panic!("{error}"));
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    let mut child = fixture
        .configure(&mut command)
        .env("FAKE_CONTROL", &control)
        .args(["plan", "--config", config_arg, "redirected SDK planning"])
        .current_dir(fixture.root.path())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap_or_else(|error| panic!("failed to start redirected SDK cruise: {error}"));

    wait_for_file(&control.with_extension("started"), START_TIMEOUT);
    let observation_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < observation_deadline {
        let output = fs::read_to_string(&stderr_path).unwrap_or_default();
        assert!(!output.contains("Planning...") && !output.contains("Cruising..."));
        assert!(!output.contains('\r'));
        thread::sleep(Duration::from_millis(25));
    }

    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release fake SDK: {error}"));
    let deadline = Instant::now() + EXIT_TIMEOUT;
    let status = loop {
        match child
            .try_wait()
            .unwrap_or_else(|error| panic!("failed to poll redirected SDK cruise: {error}"))
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break child.wait().unwrap_or_else(|error| {
                    panic!("failed to reap redirected SDK cruise: {error}")
                });
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };
    let output = fs::read_to_string(&stderr_path).unwrap_or_default();
    assert!(status.success(), "redirected SDK cruise failed:\n{output}");
    assert!(output.contains("[plan] creating plan..."));
    assert!(!output.contains("Planning..."));
    assert!(!output.contains("Cruising..."));
    assert!(!output.contains('\r'));
}
