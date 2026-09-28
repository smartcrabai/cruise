#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs::{self, File};
use std::io::Write;
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
            fs::create_dir_all(directory).unwrap_or_else(|error| panic!("{error}"));
        }
        Self { root, home }
    }

    fn configure<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
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

    /// Cancel the CLI's remaining interactive prompts and wait for it to exit.
    ///
    /// A single Escape byte per prompt level is not reliable: `inquire` restores
    /// the terminal mode between nested prompts, and a byte delivered inside
    /// that window is discarded rather than queued. Escape is therefore resent
    /// until the process actually exits, which keeps the teardown deterministic
    /// regardless of how many prompt levels remain or how slow the host is.
    fn cancel_until_exit(mut self) -> (ExitStatus, String) {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("failed to poll script: {error}"))
            {
                return (status, self.raw());
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let status = self
                    .child
                    .wait()
                    .unwrap_or_else(|error| panic!("failed to reap script: {error}"));
                return (status, self.raw());
            }
            self.answer_terminal_queries();
            self.send(b"\x1b");
            thread::sleep(Duration::from_millis(50));
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

/// Wait until `needle` has been written to the PTY more often than `baseline`.
///
/// Screen-content predicates cannot distinguish a freshly rendered prompt from
/// the answered copy `inquire` leaves behind, which matters when the menu before
/// and after planning offer the same options. Counting new writes in the raw
/// transcript gives an unambiguous "the next prompt has rendered" boundary.
fn wait_for_new_output(session: &mut PtySession, needle: &str, baseline: usize) {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        session.answer_terminal_queries();
        if session.raw().matches(needle).count() > baseline {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a new {needle:?} render beyond {baseline}; raw transcript:\n{}",
            session.raw()
        );
        thread::sleep(Duration::from_millis(25));
    }
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
fn cli_plan_renders_loader_during_silent_command_turn_and_clears_it() {
    let _lock = PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The planning loader belongs to the CLI surface, not a particular backend.
    let fixture = Fixture::new();
    let control = fixture.root.path().join("command-silent");
    let config = fixture.write_command_config(&control);
    let config_arg = config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 config path"));
    let mut pty = fixture.start_pty(
        &["plan", "--config", config_arg, "silent command planning"],
        &control,
    );

    pty.wait_for_file(&control.with_extension("started"));
    wait_for_loader_frames(&pty, 2);
    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release command: {error}"));
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
        "loader remained on the final screen:\\n{}",
        parser.screen().contents()
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
    // `inquire` leaves the answered "? Action: Generate Plan" line on screen, so
    // "Action:" alone matches before planning even starts. Wait for an option
    // that only the post-planning AwaitingApproval menu offers.
    pty.wait_for_screen(|screen| screen.contains("Approve"));
    pty.send(b"\x1b");
    // Escape must dismiss the action menu and then the session picker. Resend it
    // until the CLI exits so the teardown does not depend on a byte landing
    // while `inquire` is between prompts.
    let (status, raw) = pty.cancel_until_exit();
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
    // The pre-planning menu has already rendered "Action:", and the menu shown
    // after replanning offers the same options, so record the current count and
    // wait for a strictly newer render below.
    let action_renders_before_planning = pty.raw().matches("Action:").count();
    pty.wait_for_file(&control.with_extension("started"));

    // Then: Replan uses the same loader boundary and leaves a Planned session
    // after the existing approved-state contract completes.
    wait_for_loader_frames(&pty, 2);
    fs::write(control.with_extension("release"), b"release")
        .unwrap_or_else(|error| panic!("failed to release command backend: {error}"));
    wait_for_new_output(&mut pty, "Action:", action_renders_before_planning);
    pty.send(b"\x1b");
    // The action editor restores terminal mode before the outer picker can
    // consume another key, so a single extra Escape can be dropped. Resend it
    // until the CLI exits instead.
    let (status, raw) = pty.cancel_until_exit();
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
