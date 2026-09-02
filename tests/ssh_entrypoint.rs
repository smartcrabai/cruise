#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct Fixture {
    root: TempDir,
    fake_bin: PathBuf,
    remote_bin: PathBuf,
    argv_file: PathBuf,
    remote_command_file: PathBuf,
    remote_args_file: PathBuf,
    stdin_file: PathBuf,
    invocation_file: PathBuf,
    marker_file: PathBuf,
    home: PathBuf,
    xdg_config: PathBuf,
    xdg_data: PathBuf,
    xdg_state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            TempDir::new().unwrap_or_else(|error| panic!("failed to create fixture: {error}"));
        let fake_bin = root.path().join("fake-bin");
        std::fs::create_dir_all(&fake_bin)
            .unwrap_or_else(|error| panic!("failed to create fake bin: {error}"));

        let fake_ssh = fake_bin.join("ssh");
        write_executable(&fake_ssh, FAKE_SSH);

        let remote_bin = root.path().join("remote cruise");
        write_executable(&remote_bin, FAKE_REMOTE_CRUISE);

        let home = root.path().join("home");
        let xdg_config = root.path().join("local-config");
        let xdg_data = root.path().join("local-data");
        let xdg_state = root.path().join("local-state");
        Self {
            argv_file: root.path().join("ssh.argv"),
            remote_command_file: root.path().join("remote-command"),
            remote_args_file: root.path().join("remote-args"),
            stdin_file: root.path().join("ssh.stdin"),
            invocation_file: root.path().join("ssh.invocations"),
            marker_file: root.path().join("injection-marker"),
            root,
            fake_bin,
            remote_bin,
            home,
            xdg_config,
            xdg_data,
            xdg_state,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        self.configure_command(&mut command, &self.fake_bin, "PATH");
        command
    }

    fn configure_command(&self, command: &mut Command, path_prefix: &Path, path_label: &str) {
        let current_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(path_prefix.to_path_buf()).chain(std::env::split_paths(&current_path)),
        )
        .unwrap_or_else(|error| panic!("failed to construct {path_label}: {error}"));
        command
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("XDG_STATE_HOME", &self.xdg_state)
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_SSH_FAKE_ARGV_FILE", &self.argv_file)
            .env(
                "CRUISE_SSH_FAKE_REMOTE_COMMAND_FILE",
                &self.remote_command_file,
            )
            .env("CRUISE_SSH_FAKE_REMOTE_ARGS_FILE", &self.remote_args_file)
            .env("CRUISE_SSH_FAKE_STDIN_FILE", &self.stdin_file)
            .env("CRUISE_SSH_FAKE_INVOCATIONS_FILE", &self.invocation_file)
            .env("CRUISE_SSH_FAKE_MARKER_FILE", &self.marker_file)
            .env("CRUISE_SSH_REMOTE_ARGS_FILE", &self.remote_args_file)
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_MODEL")
            .env_remove("CRUISE_PLAN_MODEL")
            .env_remove("CRUISE_SDK");
    }

    fn read_text(path: &Path) -> String {
        std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
    }

    fn read_delimited(path: &Path) -> Vec<String> {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let mut fields = bytes.split(|byte| *byte == 0x1f).collect::<Vec<_>>();
        if fields.last() == Some(&[].as_slice()) {
            fields.pop();
        }
        fields
            .into_iter()
            .map(|field| {
                String::from_utf8(field.to_vec())
                    .unwrap_or_else(|error| panic!("field was not UTF-8: {error}"))
            })
            .collect()
    }

    fn read_argv(&self) -> Vec<String> {
        Self::read_delimited(&self.argv_file)
    }

    fn remote_command(&self) -> String {
        Self::read_text(&self.remote_command_file)
    }

    fn invocation_count(&self) -> usize {
        Self::read_text(&self.invocation_file).lines().count()
    }
}

fn write_executable(path: &Path, contents: &str) {
    std::fs::write(path, contents)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(path)
            .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)
            .unwrap_or_else(|error| panic!("failed to chmod {}: {error}", path.display()));
    }
}

fn assert_ssh_invocation(fixture: &Fixture, destination: &str) -> String {
    let argv = fixture.read_argv();
    let separator = argv
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or_else(|| panic!("ssh invocation should contain --: {argv:?}"));
    let destination_index = argv
        .iter()
        .position(|argument| argument == destination)
        .unwrap_or_else(|| panic!("ssh destination should be an independent argument: {argv:?}"));
    assert!(
        separator < destination_index,
        "ssh -- must precede destination: {argv:?}"
    );
    let remote_command = fixture.remote_command();
    assert_eq!(
        argv.get(destination_index + 1),
        Some(&remote_command),
        "remote command should be one argument after destination: {argv:?}"
    );
    assert!(
        !argv
            .iter()
            .any(|argument| argument.contains("StrictHostKeyChecking=no")),
        "host-key verification must remain delegated to OpenSSH: {argv:?}"
    );
    remote_command
}

#[test]
fn ssh_destination_only_starts_bare_remote_cruise() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["ssh", "devbox"])
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise ssh: {error}"));

    assert!(
        output.status.success(),
        "destination-only SSH invocation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.invocation_count(),
        1,
        "SSH should connect exactly once"
    );
    let command = assert_ssh_invocation(&fixture, "devbox");
    assert_eq!(command, format!("exec {}", shell_quote("cruise")));
}

#[test]
fn ssh_forwards_remote_options_without_local_reparsing() {
    let fixture = Fixture::new();
    let remote_cwd = "/srv/project";
    let remote_config = "/srv/project/cruise.yaml";
    let remote_image = "/srv/project/diagram.png";
    let task = "task with spaces and ' quote";
    let remote_bin = fixture
        .remote_bin
        .to_str()
        .unwrap_or_else(|| panic!("remote binary path is not UTF-8"));

    let output = fixture
        .command()
        .args([
            "ssh",
            "devbox",
            "--cwd",
            remote_cwd,
            "--cruise-bin",
            remote_bin,
            "--",
        ])
        .args([
            "--plan",
            task,
            "--config",
            remote_config,
            "--image",
            remote_image,
        ])
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise ssh: {error}"));

    assert!(
        output.status.success(),
        "forwarded SSH invocation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let command = assert_ssh_invocation(&fixture, "devbox");
    let expected = format!(
        "cd {} && exec {} {} {} {} {} {} {}",
        shell_quote(remote_cwd),
        shell_quote(remote_bin),
        shell_quote("--plan"),
        shell_quote(task),
        shell_quote("--config"),
        shell_quote(remote_config),
        shell_quote("--image"),
        shell_quote(remote_image),
    );
    assert_eq!(
        command, expected,
        "remote command should preserve forwarded argument order"
    );
    assert_eq!(fixture.invocation_count(), 1);
}

#[test]
fn ssh_quotes_shell_data_and_prevents_command_injection() {
    let fixture = Fixture::new();
    let remote_cwd_path = fixture.root.path().join("remote cwd; $HOME 'quoted'\nnext");
    std::fs::create_dir_all(&remote_cwd_path)
        .unwrap_or_else(|error| panic!("failed to create remote cwd fixture: {error}"));
    let remote_cwd = remote_cwd_path
        .to_str()
        .unwrap_or_else(|| panic!("remote cwd path is not UTF-8"));
    let remote_bin = fixture
        .remote_bin
        .to_str()
        .unwrap_or_else(|| panic!("remote binary path is not UTF-8"));
    let empty = "";
    let whitespace = "  ";
    let single_quote = "single'quote";
    let injection = format!(
        "$(touch {}) ; `touch {}`\n$HOME",
        fixture.marker_file.display(),
        fixture.marker_file.display()
    );

    let output = fixture
        .command()
        .args([
            "ssh",
            "devbox",
            "--cwd",
            remote_cwd,
            "--cruise-bin",
            remote_bin,
        ])
        .args(["--", empty, whitespace, single_quote, &injection])
        .env("CRUISE_SSH_FAKE_EXECUTE", "1")
        .output()
        .unwrap_or_else(|error| panic!("failed to run quoted SSH command: {error}"));

    assert!(
        output.status.success(),
        "quoted SSH command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let command = assert_ssh_invocation(&fixture, "devbox");
    assert_eq!(
        command,
        format!(
            "cd {} && exec {} {} {} {} {}",
            shell_quote(remote_cwd),
            shell_quote(remote_bin),
            shell_quote(empty),
            shell_quote(whitespace),
            shell_quote(single_quote),
            shell_quote(&injection)
        )
    );
    assert_eq!(
        Fixture::read_delimited(&fixture.remote_args_file),
        vec![
            empty.to_string(),
            whitespace.to_string(),
            single_quote.to_string(),
            injection.clone()
        ]
    );
    assert!(
        !fixture.marker_file.exists(),
        "shell metacharacters in forwarded data must not execute locally or remotely"
    );
}

#[test]
fn ssh_transparently_forwards_stdin_stdout_and_stderr() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args(["ssh", "devbox", "--", "plan"])
        .env("CRUISE_SSH_FAKE_READ_STDIN", "1")
        .env("CRUISE_SSH_FAKE_STDOUT", "remote stdout\n")
        .env("CRUISE_SSH_FAKE_STDERR", "remote stderr\n")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("failed to spawn cruise ssh: {error}"));
    let input = b"task from the pipe\nwith a second line\n";
    child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("cruise ssh stdin should be piped"))
        .write_all(input)
        .unwrap_or_else(|error| panic!("failed to write SSH stdin: {error}"));
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("failed to collect cruise ssh output: {error}"));

    assert!(
        output.status.success(),
        "streaming SSH invocation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(Fixture::read_text(&fixture.stdin_file).as_bytes(), input);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "remote stdout\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "remote stderr\n");
}

#[test]
fn ssh_maps_tty_modes_and_auto_pipe_behavior() {
    for (mode, expected_option) in [("always", "-tt"), ("never", "-T")] {
        let fixture = Fixture::new();
        let output = fixture
            .command()
            .args(["ssh", "--tty", mode, "devbox", "--", "list"])
            .output()
            .unwrap_or_else(|error| panic!("failed to run --tty {mode}: {error}"));
        assert!(
            output.status.success(),
            "--tty {mode} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let argv = fixture.read_argv();
        assert_eq!(
            argv.iter()
                .filter(|argument| argument.as_str() == expected_option)
                .count(),
            1,
            "--tty {mode} should request {expected_option}: {argv:?}"
        );
        let opposite = if expected_option == "-t" { "-T" } else { "-t" };
        assert!(!argv.iter().any(|argument| argument == opposite));
    }

    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["ssh", "devbox", "--", "list", "--json"])
        .output()
        .unwrap_or_else(|error| panic!("failed to run auto TTY mode through pipes: {error}"));
    assert!(
        output.status.success(),
        "auto TTY pipe invocation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let argv = fixture.read_argv();
    assert!(
        !argv
            .iter()
            .any(|argument| argument == "-t" || argument == "-T"),
        "auto mode must not allocate a PTY for non-TTY streams: {argv:?}"
    );
    assert_eq!(fixture.remote_command(), "exec 'cruise' 'list' '--json'");
}

#[test]
fn ssh_auto_requests_a_pty_when_invoked_from_a_pty() {
    if !script_available() {
        eprintln!("skipping SSH PTY smoke test: script is unavailable");
        return;
    }

    let fixture = Fixture::new();
    let binary = shell_quote(env!("CARGO_BIN_EXE_cruise"));
    let command_line = format!("exec {binary} ssh devbox -- list");
    let mut command = Command::new("script");
    #[cfg(target_os = "macos")]
    command.args(["-q", "/dev/null", "/bin/sh", "-c", &command_line]);
    #[cfg(target_os = "linux")]
    command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);
    fixture.configure_command(&mut command, &fixture.fake_bin, "PTY PATH");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run SSH PTY smoke test: {error}"));

    assert!(
        output.status.success(),
        "SSH PTY smoke test failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let argv = fixture.read_argv();
    assert!(
        argv.iter().any(|argument| argument == "-t"),
        "auto TTY mode should request a PTY when local streams are terminals: {argv:?}"
    );
    assert!(!argv.iter().any(|argument| argument == "-T"));
}

#[test]
fn ssh_reports_remote_exit_statuses() {
    for status in ["1", "255"] {
        let fixture = Fixture::new();
        let output = fixture
            .command()
            .args(["ssh", "devbox", "--", "list"])
            .env("CRUISE_SSH_FAKE_STATUS", status)
            .output()
            .unwrap_or_else(|error| panic!("failed to run fake SSH status {status}: {error}"));

        assert!(
            !output.status.success(),
            "status {status} must fail locally"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("ssh") && stderr.contains(&format!("status {status}")),
            "SSH status failure should identify the exit status {status}: {stderr}"
        );
    }
}

#[test]
fn ssh_reports_signal_termination() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["ssh", "devbox", "--", "list"])
        .env("CRUISE_SSH_FAKE_SIGNAL", "1")
        .output()
        .unwrap_or_else(|error| panic!("failed to run signal-terminated fake SSH: {error}"));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ssh") && stderr.contains("signal"),
        "signal termination should be reported as an SSH failure: {stderr}"
    );
}

#[test]
fn ssh_spawn_failure_identifies_missing_openssh_client() {
    let fixture = Fixture::new();
    let empty_path = fixture.root.path().join("empty-path");
    std::fs::create_dir_all(&empty_path)
        .unwrap_or_else(|error| panic!("failed to create empty PATH: {error}"));
    let path = std::env::join_paths([empty_path])
        .unwrap_or_else(|error| panic!("failed to construct empty PATH: {error}"));
    let mut command = fixture.command();
    command
        .env("PATH", path)
        .args(["ssh", "devbox", "--", "list"]);
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run missing-ssh test: {error}"));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("ssh") && stderr.contains("path"),
        "missing OpenSSH client should be identified as a PATH problem: {stderr}"
    );
}

#[test]
fn ssh_does_not_create_local_session_state() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["ssh", "devbox", "--", "list"])
        .output()
        .unwrap_or_else(|error| panic!("failed to run local-state isolation test: {error}"));

    assert!(
        output.status.success(),
        "SSH state-isolation invocation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !fixture.xdg_config.exists(),
        "SSH must not create local config state"
    );
    assert!(
        !fixture.xdg_data.exists(),
        "SSH must not create local session state"
    );
    assert!(
        !fixture.xdg_state.exists(),
        "SSH must not create local runtime state"
    );
}

fn script_available() -> bool {
    match Command::new("script").arg("-V").output() {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!("failed to probe script: {error}"),
    }
}

const FAKE_SSH: &str = r#"#!/bin/sh
set -eu

: >> "${CRUISE_SSH_FAKE_INVOCATIONS_FILE:?}"
printf '1\n' >> "${CRUISE_SSH_FAKE_INVOCATIONS_FILE:?}"
: > "${CRUISE_SSH_FAKE_ARGV_FILE:?}"
last=
for argument in "$@"; do
  printf '%s\037' "$argument" >> "${CRUISE_SSH_FAKE_ARGV_FILE:?}"
  last=$argument
done
printf '%s' "$last" > "${CRUISE_SSH_FAKE_REMOTE_COMMAND_FILE:?}"
if [ "${CRUISE_SSH_FAKE_READ_STDIN:-0}" = "1" ]; then
  cat > "${CRUISE_SSH_FAKE_STDIN_FILE:?}"
fi
if [ "${CRUISE_SSH_FAKE_EXECUTE:-0}" = "1" ]; then
  /bin/sh -c "$last"
  exit $?
fi
printf '%s' "${CRUISE_SSH_FAKE_STDOUT:-}"
printf '%s' "${CRUISE_SSH_FAKE_STDERR:-}" >&2
if [ "${CRUISE_SSH_FAKE_SIGNAL:-0}" = "1" ]; then
  kill -TERM $$
fi
exit "${CRUISE_SSH_FAKE_STATUS:-0}"
"#;

const FAKE_REMOTE_CRUISE: &str = r#"#!/bin/sh
set -eu
: > "${CRUISE_SSH_REMOTE_ARGS_FILE:?}"
for argument in "$@"; do
  printf '%s\037' "$argument" >> "${CRUISE_SSH_REMOTE_ARGS_FILE:?}"
done
"#;
