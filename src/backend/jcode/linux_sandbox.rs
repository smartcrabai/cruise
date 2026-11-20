//! Linux Bubblewrap launcher for the opt-in jcode sandbox.

use super::linux_git::GitMounts;
use crate::cancellation::CancellationToken;
use jcode_sdk::{ConnectOptions, JcodeClient};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const BOOTSTRAP: &str = r#"
set -u
helper=$1
socket=$2
ready=$3
jcode=$4
api_socket=$5
shift 5
"$helper" internal jcode-relay --socket "$socket" --ready "$ready" "$@" >/dev/null 2>&1 &
relay_pid=$!
cleanup() {
    trap - EXIT HUP INT TERM
    kill "$relay_pid" 2>/dev/null || :
    wait "$relay_pid" 2>/dev/null || :
}
trap cleanup EXIT
trap 'exit 143' HUP INT TERM
ticks=200
while [ ! -s "$ready" ]; do
    if ! kill -0 "$relay_pid" 2>/dev/null; then
        printf '%s\n' 'jcode egress relay exited before readiness' >&2
        wait "$relay_pid" 2>/dev/null || :
        exit 70
    fi
    if [ "$ticks" -le 0 ]; then
        printf '%s\n' 'jcode egress relay readiness timed out' >&2
        exit 71
    fi
    ticks=$((ticks - 1))
    /usr/bin/sleep 0.05
done
if [ "$#" -eq 0 ]; then
    port=$(cat "$ready") || exit 72
    case "$port" in
        ''|*[!0-9]*) printf '%s\n' 'jcode egress relay returned an invalid port' >&2; exit 73 ;;
    esac
    if [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
        printf '%s\n' 'jcode egress relay returned an invalid port' >&2
        exit 73
    fi
    export HTTP_PROXY="http://127.0.0.1:$port"
    export HTTPS_PROXY="http://127.0.0.1:$port"
    export http_proxy="$HTTP_PROXY"
    export https_proxy="$HTTPS_PROXY"
    export NO_PROXY='127.0.0.1,localhost,::1'
    export no_proxy="$NO_PROXY"
fi
"$jcode" --no-update --no-selfdev api-bridge --api-socket "$api_socket" >/dev/null 2>&1
exit $?
"#;

const API_SOCKET_NAME: &str = "api.sock";
const BWRAP_HELP_LIMIT: usize = 64 * 1024;
const DIAGNOSTIC_LIMIT: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
const CONNECT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The API paths are under a separately owned scratch root; its parent can
/// connect to `runtime/api.sock` without exposing that root to the sandbox.
pub(super) struct SandboxSpec<'a> {
    pub(super) workspace: &'a Path,
    pub(super) read_only: bool,
    pub(super) binary: &'a Path,
    pub(super) helper: &'a Path,
    pub(super) state: &'a Path,
    pub(super) runtime: &'a Path,
    pub(super) egress_socket: &'a Path,
    pub(super) local_port: Option<u16>,
    pub(super) git: &'a GitMounts,
    pub(super) inputs: &'a [PathBuf],
    pub(super) output_file: Option<&'a Path>,
    pub(super) env: &'a HashMap<OsString, OsString>,
}

pub(super) struct Sandbox {
    child: Child,
    api_socket: PathBuf,
    diagnostics: Arc<Mutex<Vec<u8>>>,
    diagnostic_reader: Option<JoinHandle<()>>,
    stopped: bool,
}

impl Sandbox {
    pub(super) fn launch(
        spec: SandboxSpec<'_>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Self, String> {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err("Bubblewrap launch cancelled".to_owned());
        }
        validate_spec(&spec)?;
        let api_socket = spec.runtime.join(API_SOCKET_NAME);
        ensure_absent(&api_socket, "private jcode API socket")?;

        let (bwrap, disable_nested_userns) = find_bwrap()?;
        let mut command = make_bwrap_command(&bwrap, disable_nested_userns, &spec)?;
        let mut child = command
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not launch Bubblewrap: {error}"))?;
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Bubblewrap stderr pipe was unavailable".to_owned());
        };
        let diagnostics = Arc::new(Mutex::new(Vec::with_capacity(DIAGNOSTIC_LIMIT)));
        let reader_diagnostics = Arc::clone(&diagnostics);
        let diagnostic_reader = thread::Builder::new()
            .name("jcode-bwrap-diagnostics".to_owned())
            .spawn(move || drain_diagnostics(stderr, reader_diagnostics))
            .map_err(|error| {
                let _ = child.kill();
                let _ = child.wait();
                format!("could not capture Bubblewrap diagnostics: {error}")
            })?;

        let mut sandbox = Self {
            child,
            api_socket,
            diagnostics,
            diagnostic_reader: Some(diagnostic_reader),
            stopped: false,
        };
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            let cleanup = sandbox.stop_inner();
            return Err(append_cleanup(
                "Bubblewrap launch cancelled".to_owned(),
                cleanup,
            ));
        }
        if let Some(status) = sandbox.try_wait().map_err(|error| {
            format!("could not inspect Bubblewrap process: {error}")
        })? {
            let message = sandbox.process_exit_message(status);
            let cleanup = sandbox.stop_inner();
            return Err(append_cleanup(message, cleanup));
        }
        Ok(sandbox)
    }

    pub(super) fn connect(
        &mut self,
        cancel: Option<&CancellationToken>,
    ) -> Result<JcodeClient, String> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return self.fail_connect("Bubblewrap connection cancelled".to_owned());
            }
            match self.try_wait() {
                Ok(Some(status)) => {
                    let message = self.process_exit_message(status);
                    return self.fail_connect(message);
                }
                Ok(None) => {}
                Err(error) => {
                    return self.fail_connect(format!(
                        "could not inspect Bubblewrap process: {error}"
                    ));
                }
            }
            match fs::symlink_metadata(&self.api_socket) {
                Ok(metadata) if metadata.file_type().is_socket() => {
                    let client = JcodeClient::connect(ConnectOptions {
                        socket_path: Some(self.api_socket.clone()),
                        client_name: "cruise-jcode-bubblewrap".to_owned(),
                        request_timeout: Some(CONNECT_REQUEST_TIMEOUT),
                        ensure_runtime: false,
                        supports_user_response: false,
                    });
                    match client {
                        Ok(client) => {
                            if cancel.is_some_and(CancellationToken::is_cancelled) {
                                drop(client);
                                return self.fail_connect(
                                    "Bubblewrap connection cancelled".to_owned(),
                                );
                            }
                            return Ok(client);
                        }
                        Err(_) => {}
                    }
                }
                Ok(_) => {
                    return self.fail_connect(
                        "private jcode API path is not a Unix socket".to_owned(),
                    );
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return self.fail_connect(format!(
                        "could not inspect private jcode API socket: {error}"
                    ));
                }
            }
            if Instant::now() >= deadline {
                return self.fail_connect(
                    "timed out waiting for the private jcode API bridge".to_owned(),
                );
            }
            thread::sleep(CONNECT_POLL_INTERVAL);
        }
    }

    pub(super) fn stop(mut self) -> Result<(), String> {
        self.stop_inner()
    }

    fn fail_connect<T>(&mut self, message: String) -> Result<T, String> {
        let cleanup = self.stop_inner();
        let message = self.with_diagnostics(message);
        Err(append_cleanup(message, cleanup))
    }
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, std::io::Error> {
        self.child.try_wait()
    }

    fn process_exit_message(&self, status: ExitStatus) -> String {
        let status_text = status
            .code()
            .map_or_else(|| "terminated by signal".to_owned(), |code| format!("exit status {code}"));
        self.with_diagnostics(format!("Bubblewrap sandbox exited during startup ({status_text})"))
    }

    fn with_diagnostics(&self, message: String) -> String {
        let bytes = self
            .diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let diagnostic = String::from_utf8_lossy(&bytes)
            .chars()
            .filter(|character| !character.is_control())
            .take(500)
            .collect::<String>();
        if diagnostic.is_empty() {
            message
        } else {
            format!("{message}: {diagnostic}")
        }
    }

    fn stop_inner(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let mut failure = None;
        match self.child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                failure = Some(self.with_diagnostics(format!(
                    "Bubblewrap sandbox exited unexpectedly ({})",
                    status
                        .code()
                        .map_or_else(|| "terminated by signal".to_owned(), |code| format!("exit status {code}"))
                )));
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = self.child.kill()
                    && self.child.try_wait().ok().flatten().is_none()
                {
                    failure = Some(format!("could not stop Bubblewrap process: {error}"));
                }
                if let Err(error) = self.child.wait() {
                    failure.get_or_insert_with(|| format!("could not reap Bubblewrap process: {error}"));
                }
            }
            Err(error) => {
                let mut detail = format!("could not inspect Bubblewrap process: {error}");
                if let Err(error) = self.child.kill() {
                    detail.push_str(&format!("; could not stop Bubblewrap process: {error}"));
                }
                if let Err(error) = self.child.wait() {
                    detail.push_str(&format!("; could not reap Bubblewrap process: {error}"));
                }
                failure = Some(detail);
            }
        }
        if let Some(reader) = self.diagnostic_reader.take()
            && reader.join().is_err()
        {
            failure.get_or_insert_with(|| "Bubblewrap diagnostic reader panicked".to_owned());
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

fn validate_spec(spec: &SandboxSpec<'_>) -> Result<(), String> {
    if spec.local_port == Some(0) {
        return Err("the local relay port must be nonzero".to_owned());
    }
    validate_dir(spec.workspace, "workspace")?;
    validate_dir(spec.state, "sandbox state")?;
    validate_dir(spec.runtime, "sandbox runtime")?;
    validate_executable(spec.binary, "jcode binary")?;
    validate_executable(spec.helper, "relay helper")?;
    validate_socket(spec.egress_socket)?;
    if spec.egress_socket.starts_with(spec.workspace) {
        return Err("the host egress socket must be outside the workspace".to_owned());
    }
    for input in spec.inputs {
        if spec.workspace.starts_with(input) {
            return Err("a read-only input must not cover the workspace mount".into());
        }
        validate_input(input)?;
    }
    if let Some(output_file) = spec.output_file {
        validate_regular_file(output_file, "plan output")?;
        for input in spec.inputs {
            if paths_overlap(input, output_file) {
                return Err("plan output overlaps a read-only input".to_owned());
            }
        }
        for path in &spec.git.read_only {
            if paths_overlap(path, output_file) {
                return Err("plan output overlaps a read-only Git mount".to_owned());
            }
        }
        for (source, destination) in &spec.git.hidden_directories {
            if paths_overlap(source, output_file) || paths_overlap(destination, output_file) {
                return Err("plan output overlaps a hidden Git metadata mount".to_owned());
            }
        }
        for path in &spec.git.read_only_after_masks {
            if paths_overlap(path, output_file) {
                return Err("plan output overlaps a read-only Git mount".to_owned());
            }
        }
        for (_, destination) in &spec.git.config_overlays {
            if paths_overlap(destination, output_file) {
                return Err("plan output overlaps a Git configuration overlay".to_owned());
            }
        }
        if paths_overlap(spec.helper, output_file) || paths_overlap(spec.binary, output_file) {
            return Err("plan output overlaps a sandbox executable".to_owned());
        }
    }
    for (name, path) in [
        ("workspace", spec.workspace),
        ("sandbox state", spec.state),
        ("sandbox runtime", spec.runtime),
    ] {
        validate_guest_path(path, name)?;
    }
    if paths_overlap(spec.workspace, spec.state)
        || paths_overlap(spec.workspace, spec.runtime)
        || paths_overlap(spec.state, spec.runtime)
        || paths_overlap(spec.state, spec.egress_socket)
        || paths_overlap(spec.runtime, spec.egress_socket)
    {
        return Err("workspace, state, runtime, and egress socket must be separate".to_owned());
    }
    for path in [spec.helper, spec.binary] {
        validate_guest_path(path, "runtime executable")?;
    }
    for path in &spec.git.read_only {
        validate_existing_mount_source(path, "read-only Git path")?;
    }
    for (source, destination) in &spec.git.hidden_directories {
        validate_dir(source, "hidden Git skeleton")?;
        validate_guest_path(destination, "hidden Git metadata destination")?;
    }
    for path in &spec.git.read_only_after_masks {
        validate_existing_mount_source(path, "read-only Git metadata path")?;
    }
    for (source, destination) in &spec.git.config_overlays {
        validate_regular_file(source, "Git config overlay source")?;
        validate_guest_path(destination, "Git config overlay destination")?;
    }
    Ok(())
}

fn validate_dir(path: &Path, description: &str) -> Result<(), String> {
    validate_guest_path(path, description)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {description}: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err(format!("{description} must be a real directory"));
    }
    Ok(())
}

fn validate_executable(path: &Path, description: &str) -> Result<(), String> {
    validate_regular_file(path, description)?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("could not inspect {description}: {error}"))?;
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!("{description} is not executable"));
    }
    Ok(())
}

fn validate_regular_file(path: &Path, description: &str) -> Result<(), String> {
    validate_guest_path(path, description)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {description}: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{description} must be a regular file"));
    }
    Ok(())
}

fn validate_socket(path: &Path) -> Result<(), String> {
    validate_guest_path(path, "egress socket")?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect egress socket: {error}"))?;
    if !metadata.file_type().is_socket() {
        return Err("egress path must be a Unix socket".to_owned());
    }
    Ok(())
}

fn validate_input(path: &Path) -> Result<(), String> {
    validate_guest_path(path, "read-only input")?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect read-only input: {error}"))?;
    if !metadata.file_type().is_file() && !metadata.file_type().is_dir() {
        return Err("read-only input must be a regular file or directory".to_owned());
    }
    Ok(())
}

fn validate_existing_mount_source(path: &Path, description: &str) -> Result<(), String> {
    validate_guest_path(path, description)?;
    fs::symlink_metadata(path)
        .map(|_| ())
        .map_err(|error| format!("could not inspect {description}: {error}"))
}

fn validate_guest_path(path: &Path, description: &str) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("{description} must be a normalized absolute path"));
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn ensure_absent(path: &Path, description: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!("{description} already exists")),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not inspect {description}: {error}")),
    }
}

fn find_bwrap() -> Result<(PathBuf, bool), String> {
    let bwrap = [Path::new("/usr/bin/bwrap"), Path::new("/bin/bwrap")]
        .into_iter()
        .find(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
        .ok_or_else(|| "Bubblewrap was not found at /usr/bin/bwrap or /bin/bwrap".to_owned())?;
    let output = Command::new(bwrap)
        .arg("--help")
        .env_clear()
        .output()
        .map_err(|error| format!("could not inspect Bubblewrap capabilities: {error}"))?;
    if !output.status.success() {
        return Err("Bubblewrap capability query failed".to_owned());
    }
    let mut help = Vec::with_capacity(output.stdout.len() + output.stderr.len());
    help.extend_from_slice(&output.stdout[..output.stdout.len().min(BWRAP_HELP_LIMIT)]);
    if help.len() < BWRAP_HELP_LIMIT {
        help.extend_from_slice(
            &output.stderr[..output.stderr.len().min(BWRAP_HELP_LIMIT - help.len())],
        );
    }
    let supports_disable = contains_option(&help, b"--disable-userns");
    for option in [
        b"--unshare-user".as_slice(),
        b"--unshare-pid",
        b"--unshare-net",
        b"--unshare-ipc",
        b"--die-with-parent",
        b"--ro-bind",
        b"--bind",
        b"--proc",
        b"--dev",
        b"--tmpfs",
        b"--dir",
        b"--symlink",
        b"--chdir",
    ] {
        if !contains_option(&help, option) {
            return Err("Bubblewrap lacks a required isolation option".to_owned());
        }
    }
    Ok((bwrap.to_path_buf(), supports_disable))
}

fn contains_option(help: &[u8], option: &[u8]) -> bool {
    help.split(|byte| byte.is_ascii_whitespace() || *byte == b',')
        .any(|word| word == option)
}

fn make_bwrap_command(
    bwrap: &Path,
    disable_nested_userns: bool,
    spec: &SandboxSpec<'_>,
) -> Result<Command, String> {
    let binary_guest = Path::new("/opt/jcode")
        .join(spec.binary.file_name().ok_or_else(|| "jcode binary has no file name".to_owned())?);
    let api_socket_guest = Path::new("/run/cruise/runtime").join(API_SOCKET_NAME);
    let mut args = base_bwrap_args(disable_nested_userns)?;
    for path in [
        Path::new("/state"),
        Path::new("/run/cruise"),
        Path::new("/run/cruise/runtime"),
        Path::new("/opt"),
        Path::new("/opt/jcode"),
    ] {
        push_flag_path(&mut args, "--dir", path);
    }
    push_bind(&mut args, spec.state, Path::new("/state"), false);
    for name in ["home", "jcode", "config", "data", "cache", "runtime"] {
        push_flag_path(&mut args, "--dir", &Path::new("/state").join(name));
    }
    push_bind(
        &mut args,
        spec.runtime,
        Path::new("/run/cruise/runtime"),
        false,
    );
    append_mount(
        &mut args,
        spec.egress_socket,
        Path::new("/run/cruise/egress"),
        true,
    )?;
    append_mount(
        &mut args,
        spec.workspace,
        spec.workspace,
        spec.read_only,
    )?;
    for input in spec.inputs {
        if let Ok(relative) = input.strip_prefix(spec.state) {
            append_mount(&mut args, input, &Path::new("/state").join(relative), true)?;
        } else {
            append_mount(&mut args, input, input, true)?;
        }
    }
    append_jcode_release(&mut args, spec.binary, &binary_guest)?;
    append_mount(&mut args, spec.helper, spec.helper, true)?;
    if let Some(output_file) = spec.output_file {
        append_mount(&mut args, output_file, output_file, false)?;
    }
    append_git_mounts(&mut args, spec.git)?;
    push_flag_path(&mut args, "--chdir", spec.workspace);
    args.push("--".into());
    args.push("/bin/sh".into());
    args.push("-c".into());
    args.push(BOOTSTRAP.into());
    args.push("cruise-jcode-sandbox".into());
    args.push(spec.helper.as_os_str().to_os_string());
    args.push("/run/cruise/egress".into());
    args.push("/run/cruise/relay.ready".into());
    args.push(binary_guest.into_os_string());
    args.push(api_socket_guest.into_os_string());
    if let Some(port) = spec.local_port {
        args.push("--port".into());
        args.push(port.to_string().into());
    }

    let mut command = Command::new(bwrap);
    command.args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::null());
    for (key, value) in spec.env {
        if !is_internal_env(key) && !is_git_env(key) {
            command.env(key, value);
        }
    }
    command
        .env("PATH", "/opt/jcode:/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", "/state/home")
        .env("TMPDIR", "/tmp")
        .env("JCODE_HOME", "/state/jcode")
        .env("JCODE_RUNTIME_DIR", "/run/cruise/runtime")
        .env("JCODE_SOCKET", "/run/cruise/runtime/native.sock")
        .env("JCODE_API_SOCKET", "/run/cruise/runtime/api.sock")
        .env("JCODE_NO_UPDATE", "1")
        .env("JCODE_NO_SELFDEV", "1")
        .env("JCODE_CHECK_UPDATES", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("XDG_CONFIG_HOME", "/state/config")
        .env("XDG_DATA_HOME", "/state/data")
        .env("XDG_CACHE_HOME", "/state/cache")
        .env("XDG_RUNTIME_DIR", "/run/cruise/runtime");
    Ok(command)
}

pub(crate) fn verify_ci_snapshot(
    snapshot: &Path,
    worktree: &Path,
    script: &Path,
) -> Result<(), String> {
    validate_dir(snapshot, "CI verification snapshot")?;
    validate_guest_path(script, "trusted CI verifier script")?;
    let metadata = fs::symlink_metadata(script)
        .map_err(|error| format!("could not inspect the trusted CI verifier script: {error}"))?;
    if !metadata.file_type().is_file() || metadata.nlink() != 1 || metadata.mode() & 0o022 != 0 {
        return Err("trusted CI verifier must be a single-link regular file not writable by group or others".into());
    }
    let script = script.canonicalize()
        .map_err(|error| format!("could not resolve the trusted CI verifier script: {error}"))?;
    if script.starts_with(worktree) || script.starts_with(snapshot) {
        return Err("trusted CI verifier must be outside the worktree and candidate snapshot".into());
    }
    let (bwrap, disable_nested_userns) = find_bwrap()?;
    let mut args = base_bwrap_args(disable_nested_userns)?;
    let candidate = Path::new("/candidate");
    append_mount(&mut args, snapshot, candidate, true)?;
    let verifier = Path::new("/run/cruise-ci-verify.sh");
    append_mount(&mut args, &script, verifier, true)?;
    push_flag_path(&mut args, "--chdir", candidate);
    args.push("--".into());
    let status = Command::new(bwrap)
        .args(args)
        .args(["/bin/sh", "-eu"])
        .arg(verifier)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", "/tmp")
        .env("TMPDIR", "/tmp")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("could not run the configured CI verifier: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("configured CI verifier failed inside Bubblewrap ({status})"))
    }
}

fn base_bwrap_args(disable_nested_userns: bool) -> Result<Vec<OsString>, String> {
    let mut args = Vec::new();
    for flag in ["--unshare-user", "--unshare-pid", "--unshare-net", "--unshare-ipc"] {
        args.push(flag.into());
    }
    if disable_nested_userns {
        args.push("--disable-userns".into());
    }
    args.push("--die-with-parent".into());
    args.push("--new-session".into());
    append_system_mounts(&mut args)?;
    push_flag_path(&mut args, "--dev", Path::new("/dev"));
    push_flag_path(&mut args, "--proc", Path::new("/proc"));
    push_flag_path(&mut args, "--tmpfs", Path::new("/tmp"));
    push_flag_path(&mut args, "--tmpfs", Path::new("/run"));
    Ok(args)
}

fn append_git_mounts(args: &mut Vec<OsString>, git: &GitMounts) -> Result<(), String> {
    for path in &git.read_only {
        append_mount(args, path, path, true)?;
    }
    for (source, destination) in &git.hidden_directories {
        append_mount(args, source, destination, true)?;
    }
    for path in &git.read_only_after_masks {
        append_mount(args, path, path, true)?;
    }
    for (source, destination) in &git.config_overlays {
        append_mount(args, source, destination, true)?;
    }
    Ok(())
}

fn append_system_mounts(args: &mut Vec<OsString>) -> Result<(), String> {
    let usr = Path::new("/usr");
    if !fs::metadata(usr).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("trusted system runtime directory /usr is unavailable".to_owned());
    }
    append_mount(args, usr, usr, true)?;
    for alias in ["/bin", "/sbin", "/lib", "/lib32", "/lib64"] {
        append_system_alias(args, Path::new(alias))?;
    }
    for certificates in [
        "/etc/ssl/certs",
        "/etc/ssl/cert.pem",
        "/etc/pki/tls/certs/ca-bundle.crt",
    ] {
        let source = Path::new(certificates);
        if fs::metadata(source).is_ok_and(|metadata| metadata.is_dir() || metadata.is_file()) {
            append_mount(args, source, source, true)?;
        }
    }
    Ok(())
}

fn append_system_alias(args: &mut Vec<OsString>, path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(path)
                .map_err(|error| format!("could not inspect system runtime alias: {error}"))?;
            args.push("--symlink".into());
            args.push(target.into_os_string());
            args.push(path.as_os_str().to_os_string());
        }
        Ok(metadata) if metadata.file_type().is_dir() => append_mount(args, path, path, true)?,
        Ok(_) => return Err("unexpected system runtime path type".to_owned()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not inspect system runtime path: {error}")),
    }
    Ok(())
}

fn append_jcode_release(
    args: &mut Vec<OsString>,
    binary: &Path,
    binary_guest: &Path,
) -> Result<(), String> {
    append_mount(args, binary, binary_guest, true)?;
    let file_name = binary.file_name().ok_or_else(|| "jcode binary has no file name".to_owned())?;
    let mut bin_name = file_name.to_os_string();
    bin_name.push(".bin");
    let bin_source = binary.parent().unwrap_or_else(|| Path::new("/")).join(&bin_name);
    if fs::metadata(&bin_source).is_ok_and(|metadata| metadata.is_file()) {
        append_mount(args, &bin_source, &Path::new("/opt/jcode").join(&bin_name), true)?;
    }
    let parent = binary.parent().unwrap_or_else(|| Path::new("/"));
    for entry in fs::read_dir(parent)
        .map_err(|error| format!("could not inspect trusted jcode release directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("could not inspect jcode release file: {error}"))?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if !(name_text.starts_with("libssl.so.") || name_text.starts_with("libcrypto.so.")) {
            continue;
        }
        if !fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        append_mount(
            args,
            &entry.path(),
            &Path::new("/opt/jcode").join(name),
            true,
        )?;
    }
    Ok(())
}

fn append_mount(
    args: &mut Vec<OsString>,
    source: &Path,
    destination: &Path,
    read_only: bool,
) -> Result<(), String> {
    let metadata = fs::metadata(source)
        .map_err(|error| format!("could not inspect sandbox mount source: {error}"))?;
    if !metadata.is_dir() && !metadata.is_file() && !metadata.file_type().is_socket() {
        return Err("unsupported sandbox mount source type".to_owned());
    }
    // Bubblewrap creates missing destinations for bind mounts itself.
    // Git's private read-only skeleton already supplies its leaf mountpoints.
    push_bind(args, source, destination, read_only);
    Ok(())
}

fn push_bind(args: &mut Vec<OsString>, source: &Path, destination: &Path, read_only: bool) {
    args.push(if read_only { "--ro-bind" } else { "--bind" }.into());
    args.push(source.as_os_str().to_os_string());
    args.push(destination.as_os_str().to_os_string());
}

fn push_flag_path(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    args.push(flag.into());
    args.push(path.as_os_str().to_os_string());
}

fn is_git_env(key: &OsStr) -> bool {
    key.as_encoded_bytes().starts_with(b"GIT_")
}

fn is_internal_env(key: &OsStr) -> bool {
    const KEYS: &[&str] = &[
        "HOME",
        "TMPDIR",
        "PATH",
        "JCODE_HOME",
        "JCODE_API_SOCKET",
        "JCODE_NO_UPDATE",
        "JCODE_NO_SELFDEV",
        "JCODE_CHECK_UPDATES",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "FTP_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "ftp_proxy",
        "NO_PROXY",
        "no_proxy",
    ];
    key.to_str().is_some_and(|key| KEYS.contains(&key))
}

fn drain_diagnostics(mut stderr: impl Read, diagnostics: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0_u8; 512];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(length) => {
                let mut captured = diagnostics
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let remaining = DIAGNOSTIC_LIMIT.saturating_sub(captured.len());
                captured.extend_from_slice(&chunk[..length.min(remaining)]);
            }
        }
    }
}

fn append_cleanup(message: String, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => message,
        Err(error) => format!("{message}; {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_environment_overrides_runtime_and_never_keeps_git_dirs() {
        assert!(is_internal_env(OsStr::new("HOME")));
        assert!(is_internal_env(OsStr::new("HTTP_PROXY")));
        assert!(is_internal_env(OsStr::new("NO_PROXY")));
        assert!(is_git_env(OsStr::new("GIT_DIR")));
        assert!(is_git_env(OsStr::new("GIT_WORK_TREE")));
        assert!(!is_git_env(OsStr::new("JCODE_MODEL")));
    }

    #[test]
    fn nested_userns_detection_requires_the_exact_option_token() {
        assert!(contains_option(b"--disable-userns  Disable nested user namespaces", b"--disable-userns"));
        assert!(!contains_option(b"--disable-userns-try", b"--disable-userns"));
    }

    #[test]
    fn mount_overlap_is_component_aware() {
        assert!(paths_overlap(Path::new("/work/.git/config"), Path::new("/work/.git")));
        assert!(!paths_overlap(Path::new("/work/.git2/config"), Path::new("/work/.git")));
    }

    #[test]
    #[ignore = "requires working Linux Bubblewrap and python3"]
    #[expect(clippy::unwrap_used, reason = "temporary sandbox fixture")]
    fn ci_verifier_reads_only_candidate_and_private_scratch() {
        let temp = tempfile::tempdir().unwrap();
        let candidate = temp.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        fs::write(candidate.join("code.txt"), "candidate content").unwrap();
        let private = temp.path().join("host-only.txt");
        fs::write(&private, "not a candidate file").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worktree = temp.path().join("worktree");
        fs::create_dir(&worktree).unwrap();
        let verifier = temp.path().join("verifier.sh");
        let script = format!(r#"
test "$(pwd)" = /candidate
test "$(cat code.txt)" = "candidate content"
test ! -e .git
test ! -e '{}'
test -z "${{GH_TOKEN-}}${{ANTHROPIC_API_KEY-}}${{SSH_AUTH_SOCK-}}"
if (printf wrong >code.txt) 2>/dev/null; then exit 1; fi
printf allowed >/tmp/proof
python3 -c 'import socket; assert socket.socket().connect_ex(("127.0.0.1",{port})) != 0'
"#, private.display());
        fs::write(&verifier, script).unwrap();
        fs::set_permissions(&verifier, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(verify_ci_snapshot(&candidate, &worktree, &verifier), Ok(()));
        fs::write(&verifier, "exit 23").unwrap();
        assert!(verify_ci_snapshot(&candidate, &worktree, &verifier).unwrap_err().contains("23"));
        let in_worktree = worktree.join("approve.sh");
        fs::write(&in_worktree, "exit 0").unwrap();
        fs::set_permissions(&in_worktree, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(verify_ci_snapshot(&candidate, &worktree, &in_worktree).unwrap_err().contains("outside the worktree"));
        fs::hard_link(&verifier, worktree.join("linked.sh")).unwrap();
        assert!(verify_ci_snapshot(&candidate, &worktree, &verifier).unwrap_err().contains("single-link"));
        assert_eq!(
            fs::read_to_string(candidate.join("code.txt")).unwrap(),
            "candidate content"
        );
    }
}
