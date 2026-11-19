#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use tempfile::TempDir;

const WAIT_TIMEOUT: Duration = Duration::from_secs(20);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(45);
const SELECTOR_CONFIG: &str = "command: [cat]\ncreate_pr: false\nsteps:\n  choose:\n    option:\n      - selector: Fast path\n        next: fast\n      - selector: Alternate path\n        next: alternate\n  fast:\n    command: %FAST%\n  alternate:\n    command: %ALTERNATE%\n";

struct Fixture {
    _root: TempDir,
    home: PathBuf,
    project: PathBuf,
    config: PathBuf,
    result: PathBuf,
}

impl Fixture {
    fn new(config: impl FnOnce(&Path) -> String) -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("create fixture: {error}"));
        let home = root.path().join("home");
        let project = root.path().join("project");
        fs::create_dir_all(&home).unwrap_or_else(|error| panic!("create home: {error}"));
        fs::create_dir_all(&project).unwrap_or_else(|error| panic!("create project: {error}"));
        let result = root.path().join("chosen-step.txt");
        let config_path = root.path().join("cruise.yaml");
        fs::write(&config_path, config(&result))
            .unwrap_or_else(|error| panic!("write workflow config: {error}"));
        Self::git_init(&project, &home);
        Self {
            _root: root,
            home,
            project,
            config: config_path,
            result,
        }
    }

    fn selector() -> Self {
        Self::new(|result| {
            let fast = yaml_scalar(&format!("printf fast > {}", shell_quote(result)));
            let alternate = yaml_scalar(&format!("printf alternate > {}", shell_quote(result)));
            SELECTOR_CONFIG
                .replace("%FAST%", &fast)
                .replace("%ALTERNATE%", &alternate)
        })
    }

    fn text_input() -> Self {
        Self::new(|result| {
            let capture = yaml_scalar(&format!(
                "printf '%s' \"{{prev.input}}\" > {}",
                shell_quote(result)
            ));
            format!(
                "command: [cat]\ncreate_pr: false\nsteps:\n  choose:\n    option:\n      - text-input: Provide a value\n        next: capture\n  capture:\n    command: {capture}\n"
            )
        })
    }
    fn instruction() -> Self {
        Self::new(|_| {
            "command: [cat]\ncreate_pr: false\nsteps:\n  ask:\n    instruction: What would you like to do?\n    prompt: 'Use this answer: {input}'\n  capture:\n    command: 'printf \"%s\" \"{input}\" > instruction-answer.md'\n".to_owned()
        })
    }

    fn instruction_answer(&self) -> String {
        fs::read_to_string(self.project.join("instruction-answer.md"))
            .unwrap_or_else(|error| panic!("read instruction answer: {error}"))
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        self.configure_command(&mut command);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(args);
        command_output(command, &format!("cruise {args:?}"))
    }

    fn configure_command<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        for key in cruise::config_env::all_override_names() {
            command.env_remove(key);
        }
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("CRUISE_UI_LANGUAGE", "en")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .env_remove("CRUISE_CONFIG")
            .current_dir(&self.project)
            .stdin(Stdio::null());
        command
    }

    fn start_daemon(&self) {
        let output = self.run(&["daemon", "start", "--port", "0"]);
        assert!(
            output.status.success(),
            "daemon start failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn create_planned_session(&self) -> String {
        self.start_daemon();
        let config = self
            .config
            .to_str()
            .unwrap_or_else(|| panic!("non-UTF-8 workflow path"));
        let output = self.run(&["plan", "--skip-planning", "--config", config, "prompt test"]);
        assert!(
            output.status.success(),
            "plan failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let output = self.run(&["list", "--json"]);
        assert!(
            output.status.success(),
            "list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let sessions: Vec<Value> = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("invalid list JSON: {error}"));
        assert_eq!(sessions.len(), 1, "unexpected sessions: {sessions:?}");
        assert_eq!(sessions[0]["phase"], "Planned", "{sessions:?}");
        sessions[0]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("session id missing: {sessions:?}"))
            .to_owned()
    }

    fn start_run_pty(&self, session_id: &str) -> RunPty {
        self.start_pty(&["run", session_id])
    }

    fn start_exec_pty(&self) -> RunPty {
        let config = self
            .config
            .to_str()
            .unwrap_or_else(|| panic!("non-UTF-8 workflow path"));
        self.start_pty(&["exec", "--config", config])
    }

    fn start_pty(&self, args: &[&str]) -> RunPty {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap_or_else(|error| panic!("open PTY: {error}"));
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
        command.args(args);
        command.cwd(&self.project);
        for key in cruise::config_env::all_override_names() {
            command.env_remove(key);
        }
        command.env("HOME", &self.home);
        command.env("XDG_CONFIG_HOME", self.home.join("config"));
        command.env("XDG_DATA_HOME", self.home.join("data"));
        command.env("XDG_STATE_HOME", self.home.join("state"));
        command.env("GIT_CONFIG_COUNT", "0");
        command.env("CRUISE_DISABLE_NOTIFICATIONS", "1");
        command.env("CRUISE_UI_LANGUAGE", "en");
        command.env("NO_COLOR", "1");
        command.env("TERM", "xterm-256color");
        command.env_remove("HERDR_ENV");
        command.env_remove("CRUISE_CONFIG");
        let child = pair
            .slave
            .spawn_command(command)
            .unwrap_or_else(|error| panic!("spawn cruise {args:?} in PTY: {error}"));
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .unwrap_or_else(|error| panic!("clone PTY reader: {error}"));
        let writer = pair
            .master
            .take_writer()
            .unwrap_or_else(|error| panic!("take PTY writer: {error}"));
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                captured
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(&buffer[..count]);
            }
        });
        RunPty {
            _master: pair.master,
            child,
            writer,
            output,
            answered_queries: [0; 3],
        }
    }

    fn state_path(&self, session_id: &str) -> PathBuf {
        self.home
            .join("data/cruise/sessions")
            .join(session_id)
            .join("state.json")
    }

    fn state(&self, session_id: &str) -> Value {
        let path = self.state_path(session_id);
        serde_json::from_slice(
            &fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        )
        .unwrap_or_else(|error| panic!("invalid state at {}: {error}", path.display()))
    }
    fn result(&self) -> String {
        fs::read_to_string(&self.result).unwrap_or_else(|error| panic!("read step result: {error}"))
    }

    fn api(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let record_path = self.home.join("state/cruise/daemon/instance.json");
        let record: Value = serde_json::from_slice(
            &fs::read(&record_path).unwrap_or_else(|error| panic!("read daemon registry: {error}")),
        )
        .unwrap_or_else(|error| panic!("invalid daemon registry: {error}"));
        let port = record["port"]
            .as_u64()
            .unwrap_or_else(|| panic!("daemon port missing: {record}"));
        let port =
            u16::try_from(port).unwrap_or_else(|error| panic!("invalid daemon port: {error}"));
        let token = record["token"]
            .as_str()
            .unwrap_or_else(|| panic!("daemon token missing: {record}"));
        let payload = body.map_or_else(String::new, Value::to_string);
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .unwrap_or_else(|error| panic!("connect to isolated daemon: {error}"));
        stream
            .set_read_timeout(Some(WAIT_TIMEOUT))
            .unwrap_or_else(|error| panic!("set API read timeout: {error}"));
        write!(
            stream,
            "{method} /api/v1{path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
        .unwrap_or_else(|error| panic!("write API request: {error}"));
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .unwrap_or_else(|error| panic!("read API response: {error}"));
        let (headers, response_body) = response
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("malformed API response: {response}"));
        let status = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("missing HTTP status: {headers}"));
        let value = if response_body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(response_body)
                .unwrap_or_else(|error| panic!("invalid API response JSON: {error}: {response}"))
        };
        (status, value)
    }

    fn pending_option(&self, session_id: &str) -> Value {
        let (status, prompts) = self.api("GET", &format!("/sessions/{session_id}/prompts"), None);
        assert_eq!(status, 200, "pending prompts response: {prompts}");
        let prompts = prompts
            .as_array()
            .unwrap_or_else(|| panic!("prompts response is not an array: {prompts}"));
        assert_eq!(prompts.len(), 1, "unexpected pending prompts: {prompts:?}");
        assert_eq!(prompts[0]["kind"], "option", "{prompts:?}");
        prompts[0].clone()
    }

    fn answer_option(&self, session_id: &str, request_id: &str, next_step: &str) {
        let body = json!({
            "option": { "nextStep": next_step, "textInput": null }
        });
        let (status, response) = self.api(
            "POST",
            &format!("/sessions/{session_id}/prompts/{request_id}"),
            Some(&body),
        );
        assert_eq!(status, 200, "answer prompt response: {response}");
    }
    fn pending_ask(&self, session_id: &str) -> Value {
        let (status, prompts) = self.api("GET", &format!("/sessions/{session_id}/prompts"), None);
        assert_eq!(status, 200, "pending prompts response: {prompts}");
        let prompts = prompts
            .as_array()
            .unwrap_or_else(|| panic!("prompts response is not an array: {prompts}"));
        assert_eq!(prompts.len(), 1, "unexpected pending prompts: {prompts:?}");
        assert_eq!(prompts[0]["kind"], "ask", "{prompts:?}");
        prompts[0].clone()
    }

    fn answer_ask(&self, session_id: &str, request_id: &str, answer: &str) {
        let body = json!({ "answer": answer });
        let (status, response) = self.api(
            "POST",
            &format!("/sessions/{session_id}/prompts/{request_id}"),
            Some(&body),
        );
        assert_eq!(status, 200, "answer prompt response: {response}");
    }

    fn wait_for_exec_completion(&self, session_id: &str, answer: &str) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let (status, state) = self.api("GET", &format!("/sessions/{session_id}"), None);
            if status == 404 {
                let output = self.instruction_answer();
                assert!(
                    output.contains(answer),
                    "answer missing from step output: {output}"
                );
                return;
            }
            assert_eq!(status, 200, "read exec session response: {state}");
            assert!(
                Instant::now() < deadline,
                "exec session did not finish: {state}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_phase(&self, session_id: &str, expected: &str) -> Value {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let (status, state) = self.api("GET", &format!("/sessions/{session_id}"), None);
            assert_eq!(status, 200, "read session response: {state}");
            if state["phase"] == expected {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "session did not reach {expected}: {state}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn stop_daemon(&self) {
        let _ = self.run(&["daemon", "stop", "--force"]);
    }

    fn git_init(project: &Path, home: &Path) {
        let mut init = Command::new("git");
        init.args(["init", "-q"])
            .current_dir(project)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("GIT_CONFIG_COUNT", "0");
        let status = command_output(init, "git init");
        assert!(status.status.success(), "git init failed");

        let mut commit = Command::new("git");
        commit
            .args([
                "-c",
                "user.name=cruise-test",
                "-c",
                "user.email=cruise-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(project)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("GIT_CONFIG_COUNT", "0");
        let status = command_output(commit, "git initial commit");
        assert!(status.status.success(), "git initial commit failed");
    }
}

fn command_output(mut command: Command, description: &str) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("start {description}: {error}"));
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        if child
            .try_wait()
            .unwrap_or_else(|error| panic!("wait for {description}: {error}"))
            .is_some()
        {
            return child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("collect {description} output: {error}"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("collect timed-out {description} output: {error}"));
            panic!(
                "{description} exceeded {WAIT_TIMEOUT:?}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_daemon();
    }
}

struct RunPty {
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    answered_queries: [usize; 3],
}

impl RunPty {
    fn raw(&self) -> String {
        String::from_utf8_lossy(
            &self
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("write to PTY: {error}"));
        self.writer
            .flush()
            .unwrap_or_else(|error| panic!("flush PTY input: {error}"));
    }

    fn select_current_branch(&mut self) {
        let workspace_menu = self.wait_for("Use current branch");
        assert!(
            workspace_menu.contains("Create worktree"),
            "workspace menu missing its first choice: {workspace_menu}"
        );
        self.send(b"\x1b[B");
        thread::sleep(Duration::from_millis(100));
        self.send(b"\r");
    }

    fn answer_terminal_queries(&mut self) {
        let output = self.raw();
        for (index, (query, answer)) in [
            ("\x1b[?u", "\x1b[?1u"),
            ("\x1b[c", "\x1b[?1;2c"),
            ("\x1b[6n", "\x1b[1;1R"),
        ]
        .iter()
        .enumerate()
        {
            while self.answered_queries[index] < output.matches(query).count() {
                self.send(answer.as_bytes());
                self.answered_queries[index] += 1;
            }
        }
    }

    fn wait_for(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            self.answer_terminal_queries();
            let output = self.raw();
            if output.contains(needle) {
                return output;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}:\n{output}"
            );
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("poll PTY process: {error}"))
            {
                panic!("cruise run exited {status:?} waiting for {needle:?}:\n{output}");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_occurrences(&mut self, needle: &str, count: usize) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            self.answer_terminal_queries();
            let output = self.raw();
            if output.matches(needle).count() >= count {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {count} occurrences of {needle:?}:\n{output}"
            );
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("poll PTY process: {error}"))
            {
                panic!("cruise run exited {status:?} waiting for {needle:?}:\n{output}");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn finish(mut self) -> (bool, String) {
        self.wait_for_exit(WAIT_TIMEOUT)
    }

    fn cancel_until_exit(mut self) -> (bool, String) {
        self.send(b"\x1b");
        self.wait_for_exit(CANCEL_TIMEOUT)
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> (bool, String) {
        let deadline = Instant::now() + timeout;
        loop {
            self.answer_terminal_queries();
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("poll PTY process: {error}"))
            {
                return (status.success(), self.raw());
            }
            assert!(
                Instant::now() < deadline,
                "cruise run did not exit:\n{}",
                self.raw()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for RunPty {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn yaml_scalar(value: &str) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|error| panic!("serialize YAML scalar: {error}"))
        .trim_end()
        .to_owned()
}

fn shell_quote(value: &Path) -> String {
    format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"))
}

#[test]
fn tty_selector_choice_runs_its_selected_next_step() {
    let fixture = Fixture::selector();
    let session_id = fixture.create_planned_session();
    let mut terminal = fixture.start_run_pty(&session_id);
    terminal.select_current_branch();
    let menu = terminal.wait_for("Alternate path");
    assert!(
        menu.contains("Fast path"),
        "option menu missing first choice: {menu}"
    );
    terminal.send(b"\x1b[B");
    thread::sleep(Duration::from_millis(100));
    terminal.send(b"\r");
    let (success, transcript) = terminal.finish();
    assert!(success, "cruise run failed:\n{transcript}");
    assert_eq!(fixture.result(), "alternate");
    assert_eq!(fixture.state(&session_id)["phase"], "Completed");
}

#[test]
fn tty_text_input_option_is_available_as_prev_input() {
    let fixture = Fixture::text_input();
    let session_id = fixture.create_planned_session();
    let mut terminal = fixture.start_run_pty(&session_id);
    terminal.select_current_branch();
    terminal.wait_for("Provide a value");
    terminal.send(b"\r");
    terminal.wait_for_occurrences("Provide a value", 2);
    terminal.send(b"text reached the command\r");
    let (success, transcript) = terminal.finish();
    assert!(success, "cruise run failed:\n{transcript}");
    assert_eq!(fixture.result(), "text reached the command");
    assert_eq!(fixture.state(&session_id)["phase"], "Completed");
}

#[test]
fn api_answer_wins_over_a_visible_terminal_option_menu() {
    let fixture = Fixture::selector();
    let session_id = fixture.create_planned_session();
    let mut terminal = fixture.start_run_pty(&session_id);
    terminal.select_current_branch();
    let menu = terminal.wait_for("Alternate path");
    assert!(
        menu.contains("Fast path"),
        "option menu missing first choice: {menu}"
    );
    let prompt = fixture.pending_option(&session_id);
    let request_id = prompt["requestId"]
        .as_str()
        .unwrap_or_else(|| panic!("request id missing: {prompt}"));
    fixture.answer_option(&session_id, request_id, "alternate");
    terminal.send(b"\r");
    let (success, transcript) = terminal.finish();
    assert!(success, "cruise run failed:\n{transcript}");
    assert!(
        transcript.contains("This prompt was already answered elsewhere."),
        "stale CLI answer notice missing:\n{transcript}"
    );
    assert_eq!(fixture.result(), "alternate");
    assert_eq!(fixture.state(&session_id)["phase"], "Completed");
}

#[test]
fn escape_pauses_and_rerunning_renders_the_pending_option() {
    let fixture = Fixture::selector();
    let session_id = fixture.create_planned_session();
    let mut terminal = fixture.start_run_pty(&session_id);
    terminal.select_current_branch();
    terminal.wait_for_occurrences("[↑↓ to move, enter to select, type to filter]", 2);
    let menu = terminal.raw();
    assert!(
        menu.contains("Alternate path"),
        "option menu did not finish rendering: {menu}"
    );
    let (success, transcript) = terminal.cancel_until_exit();
    assert!(success, "dismissing the menu failed:\n{transcript}");
    assert!(
        transcript.contains("Session paused. Resume with `cruise run`."),
        "resume hint missing:\n{transcript}"
    );
    let paused = fixture.state(&session_id);
    assert_eq!(paused["phase"], "Suspended", "{paused}");
    assert_eq!(paused["current_step"], "choose", "{paused}");

    let mut resumed = fixture.start_run_pty(&session_id);
    let menu = resumed.wait_for("Alternate path");
    assert!(
        menu.contains("Fast path"),
        "resumed option menu missing: {menu}"
    );
    resumed.send(b"\r");
    let (success, transcript) = resumed.finish();
    assert!(success, "resumed cruise run failed:\n{transcript}");
    let resumed_result = fixture.result();
    assert!(
        matches!(resumed_result.as_str(), "fast" | "alternate"),
        "resumed prompt produced no selected-step result: {resumed_result}"
    );
    let completed = fixture.state(&session_id);
    assert_eq!(completed["phase"], "Completed", "{completed}");
    assert_eq!(
        completed["awaiting_input"].as_bool(),
        Some(false),
        "{completed}"
    );
}

#[test]
fn non_tty_client_leaves_option_available_for_api_answer() {
    let fixture = Fixture::selector();
    let session_id = fixture.create_planned_session();
    let output = fixture.run(&["run", &session_id]);
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "non-TTY run failed:\n{transcript}");
    assert!(
        transcript.contains("waiting for input")
            && transcript.contains("cruise webui")
            && transcript.contains("cruise run"),
        "non-TTY guidance missing:\n{transcript}"
    );

    let pending = fixture.pending_option(&session_id);
    let waiting = fixture.state(&session_id);
    assert_eq!(waiting["phase"], "Running", "{waiting}");
    assert_eq!(waiting["awaiting_input"].as_bool(), Some(true), "{waiting}");
    let request_id = pending["requestId"]
        .as_str()
        .unwrap_or_else(|| panic!("request id missing: {pending}"));
    fixture.answer_option(&session_id, request_id, "alternate");
    let completed = fixture.wait_for_phase(&session_id, "Completed");
    assert_eq!(
        completed["awaiting_input"].as_bool(),
        Some(false),
        "{completed}"
    );
    assert_eq!(fixture.result(), "alternate");
}
#[test]
fn tty_exec_instruction_prompt_answer_reaches_the_step() {
    let fixture = Fixture::instruction();
    let mut terminal = fixture.start_exec_pty();
    terminal.wait_for("What would you like to do?");
    terminal.send(b"answer from the terminal\r");

    let (success, transcript) = terminal.finish();
    assert!(success, "cruise exec failed:\n{transcript}");
    let output = fixture.instruction_answer();
    assert!(
        output.contains("answer from the terminal"),
        "terminal answer did not reach the step: {output}"
    );
}

#[test]
fn non_tty_exec_instruction_stays_pending_for_api_answer() {
    let fixture = Fixture::instruction();
    let config = fixture
        .config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 workflow path"));
    let output = fixture.run(&["exec", "--config", config]);
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "non-TTY exec failed:\n{transcript}"
    );
    assert!(
        transcript.contains("waiting for input")
            && transcript.contains("cruise webui")
            && transcript.contains("cruise run"),
        "non-TTY guidance missing:\n{transcript}"
    );
    let session_id = transcript
        .split("Session ")
        .find_map(|line| line.split_once(" is waiting for input").map(|(id, _)| id))
        .unwrap_or_else(|| panic!("pending session id missing from guidance:\n{transcript}"));
    let prompt = fixture.pending_ask(session_id);
    assert_eq!(prompt["question"], "What would you like to do?", "{prompt}");
    let request_id = prompt["requestId"]
        .as_str()
        .unwrap_or_else(|| panic!("request id missing: {prompt}"));

    fixture.answer_ask(session_id, request_id, "answer from the API");
    fixture.wait_for_exec_completion(session_id, "answer from the API");
}
#[test]
fn exec_instruction_uses_existing_input_without_asking() {
    let fixture = Fixture::instruction();
    let config = fixture
        .config
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 workflow path"));
    let output = fixture.run(&["exec", "--config", config, "input already supplied"]);
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "cruise exec failed:\n{transcript}");
    assert_eq!(fixture.instruction_answer(), "input already supplied");
    assert!(
        !transcript.contains("waiting for input"),
        "pre-existing input unexpectedly prompted: {transcript}"
    );
}
