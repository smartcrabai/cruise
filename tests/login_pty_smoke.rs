#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);
const ACTIONS: [&str; 4] = [
    "Sign in or configure a provider",
    "Store an API key directly",
    "View authentication status",
    "Exit",
];
const AUTHENTICATED: &str =
    r#"{"any_available":true,"providers":[{"id":"anthropic-api","status":"available"}]}"#;
const MODELS: &str = r#"{"models":["claude-sonnet","gpt-5"]}"#;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct Fixture {
    root: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    stdin_file: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let home = root.path().join("home");
        let bin = root.path().join("bin");
        let log = root.path().join("jcode.log");
        let stdin_file = root.path().join("jcode.stdin");
        for directory in [
            &home,
            &bin,
            &home.join("config"),
            &home.join("data"),
            &home.join("state"),
        ] {
            std::fs::create_dir_all(directory).unwrap_or_else(|error| panic!("{error}"));
        }
        let fixture = Self {
            root,
            home,
            bin,
            log,
            stdin_file,
        };
        fixture.install_jcode_stub();
        fixture
    }

    fn install_jcode_stub(&self) {
        let path = self.bin.join("jcode");
        let script = r#"#!/bin/sh
printf 'argv=%s\n' "$*" >> "${CRUISE_LOGIN_TEST_LOG:?}"
printf 'JCODE_HOME=%s JCODE_NO_TELEMETRY=%s\n' "${JCODE_HOME:-}" "${JCODE_NO_TELEMETRY:-}" >> "${CRUISE_LOGIN_TEST_LOG:?}"
printf 'CRUISE_LOGIN_API_KEY=%s\n' "${CRUISE_LOGIN_API_KEY:-}" >> "${CRUISE_LOGIN_TEST_LOG:?}"
if [ "${1:-}" = "--no-update" ]; then shift; fi
case "${1:-}" in
  login)
    if [ "${CRUISE_LOGIN_CAPTURE_STDIN:-0}" = "1" ]; then
      cat > "${CRUISE_LOGIN_STDIN_FILE:?}"
    fi
    printf 'stub jcode login complete\n'
    exit 0
    ;;
  auth)
    if [ "${2:-}" = "status" ] && [ "${3:-}" = "--json" ]; then
      printf '%s\n' "${CRUISE_LOGIN_AUTH_JSON:?}"
      exit 0
    fi
    ;;
  model)
    if [ "${2:-}" = "list" ] && [ "${3:-}" = "--json" ]; then
      printf '%s\n' "${CRUISE_LOGIN_MODELS_JSON:?}"
      exit 0
    fi
    ;;
esac
printf 'unexpected jcode invocation: %s\n' "$*" >&2
exit 64
"#;
        std::fs::write(&path, script).unwrap_or_else(|error| panic!("{error}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&path)
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap_or_else(|error| panic!("{error}"));
        }
    }

    fn expected_jcode_home(&self) -> PathBuf {
        self.home.join("data/cruise/jcode-home")
    }

    fn configure(
        &self,
        command: &mut Command,
        no_color: bool,
        auth_json: &str,
        models_json: &str,
        capture_stdin: bool,
        api_key: Option<&str>,
    ) {
        let current_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(&current_path)),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("PATH", path)
            .env("CRUISE_LOGIN_TEST_LOG", &self.log)
            .env("CRUISE_LOGIN_AUTH_JSON", auth_json)
            .env("CRUISE_LOGIN_MODELS_JSON", models_json)
            .env_remove("CRUISE_CONFIG")
            .env_remove("NO_COLOR")
            .env_remove("CRUISE_LOGIN_API_KEY")
            .env_remove("CRUISE_LOGIN_CAPTURE_STDIN")
            .env_remove("CRUISE_LOGIN_STDIN_FILE");
        if no_color {
            command.env("NO_COLOR", "1");
        }
        if capture_stdin {
            command
                .env("CRUISE_LOGIN_CAPTURE_STDIN", "1")
                .env("CRUISE_LOGIN_STDIN_FILE", &self.stdin_file);
        }
        if let Some(api_key) = api_key {
            command.env("CRUISE_LOGIN_API_KEY", api_key);
        }
    }

    fn run(
        &self,
        args: &[&str],
        no_color: bool,
        auth_json: &str,
        models_json: &str,
        capture_stdin: bool,
        api_key: Option<&str>,
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        command.args(args);
        self.configure(
            &mut command,
            no_color,
            auth_json,
            models_json,
            capture_stdin,
            api_key,
        );
        command
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|error| panic!("failed to run cruise: {error}"))
    }

    fn start(
        &self,
        no_color: bool,
        auth_json: &str,
        models_json: &str,
        capture_stdin: bool,
    ) -> LoginPty {
        self.start_internal(no_color, auth_json, models_json, capture_stdin, None)
    }

    fn start_internal(
        &self,
        no_color: bool,
        auth_json: &str,
        models_json: &str,
        capture_stdin: bool,
        redirected_stdout_path: Option<&PathBuf>,
    ) -> LoginPty {
        let stdout_path = self.root.path().join("login.stdout");
        let stderr_path = self.root.path().join("login.stderr");
        let stdout = File::create(&stdout_path).unwrap_or_else(|error| panic!("{error}"));
        let stderr = File::create(&stderr_path).unwrap_or_else(|error| panic!("{error}"));
        let command_line = format!(
            "stty cols 120 rows 40; exec {} login{}",
            shell_quote(env!("CARGO_BIN_EXE_cruise")),
            redirected_stdout_path.map_or_else(String::new, |path| format!(
                " > {}",
                shell_quote(&path.to_string_lossy())
            ),),
        );
        let mut command = Command::new("script");
        #[cfg(target_os = "macos")]
        command.args(["-q", "/dev/null", "/bin/sh", "-c", &command_line]);
        #[cfg(target_os = "linux")]
        command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);
        self.configure(
            &mut command,
            no_color,
            auth_json,
            models_json,
            capture_stdin,
            None,
        );
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|error| panic!("failed to start login PTY: {error}"));
        let input = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("script stdin should be piped"));
        LoginPty {
            child,
            input: Some(input),
            stdout_path,
            stderr_path,
        }
    }
}

struct LoginPty {
    child: Child,
    input: Option<ChildStdin>,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl LoginPty {
    fn send(&mut self, bytes: &[u8]) {
        let input = self
            .input
            .as_mut()
            .unwrap_or_else(|| panic!("login PTY input is closed"));
        input
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("failed to send login input: {error}"));
        input
            .flush()
            .unwrap_or_else(|error| panic!("failed to flush login input: {error}"));
        thread::sleep(Duration::from_millis(150));
    }

    fn transcript(&self) -> String {
        let mut bytes = std::fs::read(&self.stdout_path).unwrap_or_default();
        bytes.extend(std::fs::read(&self.stderr_path).unwrap_or_default());
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn wait_for(&self, expected: &str) {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.transcript().contains(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "PTY transcript missing {expected:?}:\n{}",
                self.transcript()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_lowercase(&self, expected: &str) {
        let normalized = expected.to_ascii_lowercase();
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.transcript().to_ascii_lowercase().contains(&normalized) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "PTY transcript missing {expected:?}:\n{}",
                self.transcript()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_occurrences(&self, expected: &str, count: usize) {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.transcript().matches(expected).count() >= count {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "PTY transcript has fewer than {count} occurrences of {expected:?}:\n{}",
                self.transcript()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_redirected_stdout(path: &PathBuf, expected: &str) {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if std::fs::read_to_string(path)
                .unwrap_or_default()
                .contains(expected)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "redirected stdout missing {expected:?}:\n{}",
                std::fs::read_to_string(path).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn finish(mut self) -> (ExitStatus, String) {
        drop(self.input.take());
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let status = loop {
            match self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("failed to poll login PTY: {error}"))
            {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    panic!("cruise login did not exit: {}", self.transcript());
                }
                None => thread::sleep(Duration::from_millis(25)),
            }
        };
        (status, self.transcript())
    }
}

impl Drop for LoginPty {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        if let Some(input) = self.input.as_mut() {
            let _ = input.write_all(b"\x1b");
            let _ = input.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn select_action(login: &mut LoginPty, down_count: usize) {
    for _ in 0..down_count {
        login.send(b"\x1b[B");
    }
    login.send(b"\r");
}

fn has_sgr(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(2).enumerate().any(|(index, pair)| {
        if pair != [0x1b, b'['] {
            return false;
        }
        let sequence = &bytes[index + 2..];
        let end = sequence
            .iter()
            .position(|byte| *byte == 0x1b)
            .unwrap_or(sequence.len());
        sequence[..end].contains(&b'm')
    })
}

fn plain_transcript(text: &str) -> String {
    console::strip_ansi_codes(text).to_string()
}

fn line_contains_count(text: &str, label: &str, count: usize) -> bool {
    let label = label.to_ascii_lowercase();
    let count = count.to_string();
    text.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.contains(&label) && line.contains(&count)
    })
}

#[test]
fn login_tty_menu_has_four_actions_and_exit_works_with_ansi() {
    let fixture = Fixture::new();
    let mut login = fixture.start(false, AUTHENTICATED, MODELS, false);
    login.wait_for("cruise login");
    login.wait_for("Configure providers for cruise's private jcode backend");
    for action in ACTIONS {
        login.wait_for(action);
    }
    assert!(
        has_sgr(&login.transcript()),
        "TTY login should use ANSI styling"
    );

    select_action(&mut login, 3);
    let (status, transcript) = login.finish();
    assert!(status.success(), "cruise login failed: {transcript}");
}

#[test]
fn login_tty_menu_escape_exits_successfully() {
    let fixture = Fixture::new();
    let mut login = fixture.start(true, AUTHENTICATED, MODELS, false);
    login.wait_for(ACTIONS[0]);
    login.send(b"\x1b");

    let (status, transcript) = login.finish();
    assert!(
        status.success(),
        "Esc should exit cruise login: {transcript}"
    );
}

#[test]
fn login_tty_no_color_keeps_the_menu_words_without_sgr() {
    let fixture = Fixture::new();
    let mut login = fixture.start(true, AUTHENTICATED, MODELS, false);
    for action in ACTIONS {
        login.wait_for(action);
    }
    let transcript = login.transcript();
    let plain = plain_transcript(&transcript);
    assert!(
        !has_sgr(&transcript),
        "NO_COLOR should remove styling: {transcript}"
    );
    for action in ACTIONS {
        assert!(
            plain.contains(action),
            "NO_COLOR removed menu text {action:?}: {plain}"
        );
    }

    select_action(&mut login, 3);
    let (status, transcript) = login.finish();
    assert!(
        status.success(),
        "cruise login failed with NO_COLOR: {transcript}"
    );
}

#[test]
fn login_tty_with_redirected_stdout_skips_the_menu_and_delegates_to_jcode() {
    let fixture = Fixture::new();
    let redirected_stdout_path = fixture.root.path().join("redirected.stdout");
    let login = fixture.start_internal(
        true,
        AUTHENTICATED,
        MODELS,
        false,
        Some(&redirected_stdout_path),
    );
    LoginPty::wait_for_redirected_stdout(&redirected_stdout_path, "stub jcode login complete");

    let (status, transcript) = login.finish();
    assert!(status.success(), "redirected login failed: {transcript}");
    assert!(
        !transcript.contains(ACTIONS[0]),
        "redirected stdout must not trigger the interactive menu: {transcript}"
    );
    let stdout =
        std::fs::read_to_string(redirected_stdout_path).unwrap_or_else(|error| panic!("{error}"));
    assert!(stdout.contains("stub jcode login complete"));
}

#[test]
fn login_tty_sign_in_delegates_to_jcode_and_returns_to_the_menu() {
    let fixture = Fixture::new();
    let mut login = fixture.start(true, AUTHENTICATED, MODELS, false);
    login.wait_for(ACTIONS[0]);
    select_action(&mut login, 0);
    login.wait_for("stub jcode login complete");
    login.wait_for_occurrences(ACTIONS[0], 2);

    login.send(b"\x1b");
    let (status, transcript) = login.finish();
    assert!(
        status.success(),
        "cruise login failed after delegated login: {transcript}"
    );
    let plain = plain_transcript(&transcript);
    assert!(
        plain.contains('✓'),
        "successful provider login should have a textual completion marker: {plain}"
    );
    let log = std::fs::read_to_string(&fixture.log).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        log.contains("argv=--no-update login"),
        "unexpected jcode args: {log}"
    );
    assert!(
        log.contains(&format!(
            "JCODE_HOME={}",
            fixture.expected_jcode_home().display()
        )),
        "jcode did not receive cruise's private home: {log}"
    );
    assert!(
        log.contains("JCODE_NO_TELEMETRY=1"),
        "jcode telemetry isolation was not preserved: {log}"
    );
}

#[test]
fn login_tty_status_renders_provider_and_model_counts_and_items() {
    let fixture = Fixture::new();
    let auth = r#"{"any_available":true,"providers":[{"id":"anthropic-api","status":"available"},{"id":"openai-api","status":"available"},{"id":"claude","status":"not_configured"}]}"#;
    let models = r#"{"models":["claude-sonnet","gpt-5"]}"#;
    let mut login = fixture.start(true, auth, models, false);
    login.wait_for(ACTIONS[0]);
    select_action(&mut login, 2);
    login.wait_for("anthropic-api");
    login.wait_for("openai-api");
    login.wait_for("claude-sonnet");
    login.wait_for("gpt-5");
    login.wait_for_occurrences(ACTIONS[2], 2);

    let plain = plain_transcript(&login.transcript());
    assert!(
        line_contains_count(&plain, "provider", 2),
        "status should show the authenticated-provider count: {plain}"
    );
    assert!(
        line_contains_count(&plain, "model", 2),
        "status should show the available-model count: {plain}"
    );
    assert!(
        plain.contains('✓'),
        "authenticated providers should have a non-color success marker: {plain}"
    );
    let log = std::fs::read_to_string(&fixture.log).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        log.contains("argv=--no-update auth status --json"),
        "status should obtain authentication state from jcode: {log}"
    );
    assert!(
        log.contains("argv=--no-update model list --json"),
        "status should obtain models from jcode: {log}"
    );
    login.send(b"\x1b");
    let (status, transcript) = login.finish();
    assert!(status.success(), "cruise login status failed: {transcript}");
}

#[test]
fn login_tty_status_with_no_authenticated_provider_shows_next_action() {
    let fixture = Fixture::new();
    let auth = r#"{"any_available":false,"providers":[{"id":"claude","status":"not_configured"}]}"#;
    let mut login = fixture.start(true, auth, r#"{"models":[]}"#, false);
    login.wait_for(ACTIONS[0]);
    select_action(&mut login, 2);
    login.wait_for_lowercase("cruise login");
    login.wait_for("No authenticated providers");
    login.wait_for_occurrences(ACTIONS[2], 2);

    let plain = plain_transcript(&login.transcript());
    assert!(
        line_contains_count(&plain, "provider", 0),
        "empty status should show zero authenticated providers: {plain}"
    );
    assert!(
        plain.to_ascii_lowercase().contains("cruise login"),
        "empty status should explain the next login action: {plain}"
    );
    assert!(
        plain.contains('!'),
        "empty status should have a non-color warning marker: {plain}"
    );
    login.send(b"\x1b");
    let (status, transcript) = login.finish();
    assert!(status.success(), "cruise login status failed: {transcript}");
}

#[test]
fn login_tty_status_with_no_models_shows_an_empty_model_section() {
    let fixture = Fixture::new();
    let mut login = fixture.start(true, AUTHENTICATED, r#"{"models":[]}"#, false);
    login.wait_for(ACTIONS[0]);
    select_action(&mut login, 2);
    login.wait_for("anthropic-api");
    login.wait_for("Available models (0)");
    login.wait_for_occurrences(ACTIONS[2], 2);

    let plain = plain_transcript(&login.transcript());
    assert!(
        line_contains_count(&plain, "provider", 1),
        "status should show one authenticated provider: {plain}"
    );
    assert!(
        line_contains_count(&plain, "model", 0),
        "status should show zero available models: {plain}"
    );
    login.send(b"\x1b");
    let (status, transcript) = login.finish();
    assert!(status.success(), "cruise login status failed: {transcript}");
}

#[test]
fn login_tty_api_key_action_rejects_an_empty_provider_and_returns_to_menu() {
    let fixture = Fixture::new();
    let mut login = fixture.start(true, AUTHENTICATED, MODELS, false);
    login.wait_for(ACTIONS[0]);
    select_action(&mut login, 1);
    login.wait_for_lowercase("provider");
    login.send(b"\r");
    login.wait_for_occurrences(ACTIONS[1], 2);
    assert!(
        std::fs::read_to_string(&fixture.log)
            .unwrap_or_default()
            .is_empty(),
        "empty provider must not invoke jcode"
    );

    login.send(b"\x1b");
    let (status, transcript) = login.finish();
    assert!(
        status.success(),
        "empty provider should return to the menu: {transcript}"
    );
}

#[test]
fn login_tty_api_key_provider_cancellation_returns_to_menu_without_invoking_jcode() {
    let fixture = Fixture::new();
    let mut login = fixture.start(true, AUTHENTICATED, MODELS, false);
    login.wait_for(ACTIONS[0]);
    select_action(&mut login, 1);
    login.wait_for_lowercase("provider");
    login.send(b"\x1b");
    login.wait_for_occurrences(ACTIONS[1], 2);
    assert!(
        std::fs::read_to_string(&fixture.log)
            .unwrap_or_default()
            .is_empty(),
        "cancelled provider input must not invoke jcode"
    );

    login.send(b"\x1b");
    let (status, transcript) = login.finish();
    assert!(
        status.success(),
        "provider cancellation should return to the menu: {transcript}"
    );
}

#[test]
fn login_tty_api_key_is_not_echoed_and_reaches_only_jcode_stdin() {
    let fixture = Fixture::new();
    let secret = "pty-secret-api-key";
    let mut login = fixture.start(true, AUTHENTICATED, MODELS, true);
    login.wait_for(ACTIONS[0]);
    select_action(&mut login, 1);
    login.wait_for_lowercase("provider");
    login.send(b"anthropic-api\r");
    login.wait_for_lowercase("api key");
    login.send(format!("{secret}\r").as_bytes());
    login.wait_for("stub jcode login complete");
    login.wait_for_occurrences(ACTIONS[1], 2);

    let transcript = login.transcript();
    assert!(
        !transcript.contains(secret),
        "the API key must not appear in the PTY transcript: {transcript}"
    );
    let log = std::fs::read_to_string(&fixture.log).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        !log.contains(secret),
        "the API key must not appear in jcode args/logs: {log}"
    );
    let captured =
        std::fs::read_to_string(&fixture.stdin_file).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(captured, format!("{secret}\n"));
    assert!(log.contains("argv=--no-update login anthropic-api --no-validate"));

    login.send(b"\x1b");
    let (status, transcript) = login.finish();
    assert!(status.success(), "API-key login failed: {transcript}");
}

#[test]
fn login_non_tty_status_keeps_the_machine_readable_three_line_format() {
    let fixture = Fixture::new();
    let output = fixture.run(
        &["login", "--status"],
        false,
        r#"{"any_available":true,"providers":[{"id":"anthropic-api","status":"available"},{"id":"openai-api","status":"available"}]}"#,
        MODELS,
        false,
        None,
    );
    assert!(
        output.status.success(),
        "non-TTY status failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some(
            format!(
                "cruise jcode home: {}",
                fixture.expected_jcode_home().display()
            )
            .as_str()
        )
    );
    assert_eq!(
        lines.get(1).copied(),
        Some("providers: anthropic-api, openai-api")
    );
    assert_eq!(lines.get(2).copied(), Some("models: claude-sonnet, gpt-5"));
    assert!(output.stderr.is_empty(), "non-TTY status wrote to stderr");
    assert!(
        !has_sgr(&stdout),
        "non-TTY status must not emit ANSI styling: {stdout}"
    );
    assert!(
        !fixture.home.join(".jcode").exists(),
        "non-TTY status must not touch the user's ~/.jcode"
    );
}

#[test]
fn login_non_tty_without_arguments_delegates_without_menu_decoration() {
    let fixture = Fixture::new();
    let output = fixture.run(&["login"], true, AUTHENTICATED, MODELS, false, None);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "non-TTY login failed: {stdout}{stderr}"
    );
    assert!(
        stdout.contains("stub jcode login complete"),
        "jcode was not invoked: {stdout}"
    );
    assert!(
        !stdout.contains(ACTIONS[0]),
        "non-TTY login emitted the interactive menu: {stdout}"
    );
    assert!(!has_sgr(&format!("{stdout}{stderr}")));
    let log = std::fs::read_to_string(&fixture.log).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        log.contains("argv=--no-update login"),
        "unexpected jcode args: {log}"
    );
}

#[test]
fn login_explicit_provider_remains_a_one_shot_jcode_delegation() {
    let fixture = Fixture::new();
    let output = fixture.run(
        &["login", "anthropic-api"],
        true,
        AUTHENTICATED,
        MODELS,
        false,
        None,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "provider login failed: {stdout}");
    assert!(stdout.contains("stub jcode login complete"));
    assert!(
        !stdout.contains(ACTIONS[0]),
        "provider shortcut opened the menu: {stdout}"
    );
    let log = std::fs::read_to_string(&fixture.log).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        log.contains("argv=--no-update login anthropic-api"),
        "provider was not passed to jcode: {log}"
    );
}

#[test]
fn login_api_key_shortcut_keeps_the_secret_out_of_output_and_arguments() {
    let fixture = Fixture::new();
    let secret = "shortcut-secret-api-key";
    let output = fixture.run(
        &["login", "--api-key", "anthropic-api"],
        true,
        AUTHENTICATED,
        MODELS,
        true,
        Some(secret),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "API-key shortcut failed: {stdout}{stderr}"
    );
    assert!(!stdout.contains(secret));
    assert!(!stderr.contains(secret));
    let log = std::fs::read_to_string(&fixture.log).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        !log.contains(secret),
        "secret leaked into jcode invocation log: {log}"
    );
    assert!(log.contains("argv=--no-update login anthropic-api --no-validate"));
    assert!(
        log.contains("CRUISE_LOGIN_API_KEY=\n"),
        "the login-only environment variable must not be forwarded to jcode: {log}"
    );
    let captured =
        std::fs::read_to_string(&fixture.stdin_file).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(captured, format!("{secret}\n"));
}
