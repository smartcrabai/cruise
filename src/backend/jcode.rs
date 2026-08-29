//! `sdk: jcode` backend: drive the `jcode` CLI as an NDJSON subprocess.
//!
//! One prompt is one `jcode run --ndjson` child. [`stream_agent`] runs it on a
//! dedicated thread and reports progress as [`StreamChunk`]s, so `executor.rs`
//! folds this backend's output exactly like any other.
//!
//! Custom tools cannot be registered in-process -- jcode's harness API has no
//! such request -- so cruise's tools reach the model through a stdio MCP server:
//! [`ensure_mcp_registration`] writes a fixed `mcp.json` entry pointing at
//! `cruise mcp-bridge`, and the per-run socket path travels to that child in the
//! `jcode` process environment (see [`crate::tool_bridge`]).
//!
//! ## Isolation from the user's jcode
//!
//! jcode keeps credentials, `config.toml`, sessions, logs and its global
//! `mcp.json` under `$JCODE_HOME`, and it writes into that home on *any*
//! subcommand -- a probe as small as `jcode version` creates `logs/` and
//! migration stamps there. So every cruise invocation is built by
//! [`jcode_command`], which points `$JCODE_HOME` at cruise's own directory
//! ([`jcode_home`]), disables telemetry and suppresses the auto-update check;
//! the user's `~/.jcode` is never read or written.
//!
//! The one MCP source outside that home is the run directory: jcode also reads
//! `.jcode/mcp.json` / `.mcp.json` / `.claude/mcp.json` from it, last-wins over
//! the home, so a repository can shadow cruise's registration.
//! [`check_project_mcp_config`] is the gate for that.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use tokio::io::AsyncBufReadExt as _;

use crate::backend::effort::EffortLevel;
use crate::backend::stream::{LimitError, StreamChunk};
use crate::cancellation::CancellationToken;
use crate::error::{CruiseError, Result};
use crate::tool_bridge::{MCP_SERVER_NAME, TOOL_SOCKET_ENV};

/// Executable name looked up on `PATH` when no explicit binary is configured.
const JCODE_BINARY: &str = "jcode";

/// Directory under cruise's data dir used as `$JCODE_HOME`.
const JCODE_HOME_DIR: &str = "jcode-home";

/// Lowest `jcode` version whose `run --ndjson` event shape this backend is
/// verified against (the version the P3 spike in `JCODE.md` was run on).
///
/// Older versions are rejected outright rather than warned about: the event
/// names are the entire contract between cruise and jcode, and a silently
/// mismatching stream would surface as an empty step output rather than an
/// error. There is no upper bound -- unknown events are ignored, so a newer
/// jcode that only adds events keeps working.
const MIN_JCODE_VERSION: (u64, u64, u64) = (0, 81, 1);

/// Argument that makes this binary act as the stdio MCP server.
const MCP_BRIDGE_SUBCOMMAND: &str = "mcp-bridge";

/// Suppresses jcode's auto-update check. A global flag -- `jcode --help`,
/// `jcode run --help` and `jcode version --help` all list it -- so it is
/// accepted on either side of the subcommand; cruise must never trigger a
/// self-update from inside a workflow run.
const NO_UPDATE_FLAG: &str = "--no-update";

/// Provider label used for [`LimitError`] when an event names none.
const PROVIDER_LABEL: &str = "jcode";

/// Number of trailing stderr lines retained for rate-limit classification and
/// error reporting. Matches the `sdk: claude` backend: enough for a CLI error
/// tail, bounded so a chatty child cannot grow it without limit.
const STDERR_TAIL_LINES: usize = 64;

/// Reported when the child exits without emitting a `done` or `error` event,
/// i.e. it died before finishing the turn (bad flag, unknown session, killed).
/// jcode's own diagnosis is on stderr, which [`send_failure`] appends.
const NO_RESULT_MESSAGE: &str = "the jcode CLI exited without reporting a result";

/// Files jcode merges *after* `$JCODE_HOME/mcp.json`, i.e. whose entries win.
///
/// Discovery is limited to the run directory itself (jcode does not walk up to
/// parents), so scanning these three paths under the working directory covers
/// every project-local override that can affect a cruise run.
const PROJECT_MCP_FILES: &[&str] = &[".jcode/mcp.json", ".mcp.json", ".claude/mcp.json"];

/// One `sdk: jcode` prompt run.
///
/// `model` is a bare jcode model id and `provider` a jcode provider id, already
/// split out of the cruise `provider/model[:effort]` reference by
/// [`parse_model_ref`]. `resume_session_id` continues a prior session so
/// planning's plan/fix/ask turns share context.
///
/// Deliberately not `Debug`: `env` can carry provider credentials, and no
/// caller needs to format the config.
#[derive(Default)]
pub(crate) struct JcodeRunnerConfig {
    pub(crate) model: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) effort: Option<EffortLevel>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) resume_session_id: Option<String>,
    /// `$JCODE_HOME` for the child: cruise's private jcode home, as returned by
    /// [`jcode_home`].
    pub(crate) home: PathBuf,
    /// Unix socket the spawned `cruise mcp-bridge` should dial, published to the
    /// child as [`TOOL_SOCKET_ENV`].
    pub(crate) tool_socket: PathBuf,
    /// Environment variables for the spawned `jcode` process.
    pub(crate) env: HashMap<String, String>,
    /// Cancellation signal for the run. Firing it stops reading the child's
    /// output and drops it, which kills the process.
    pub(crate) cancel: Option<CancellationToken>,
    /// Binary to invoke instead of resolving [`JCODE_BINARY`] on `$PATH`. Left
    /// `None` in production; tests point it at a stub CLI.
    pub(crate) binary: Option<PathBuf>,
}

/// Cruise's private `$JCODE_HOME`: `<data dir>/jcode-home`.
///
/// Separate from the user's `~/.jcode` so cruise's credentials, sessions and
/// MCP registration never mix with (or invalidate) the ones the user's own
/// jcode TUI relies on.
///
/// # Errors
///
/// Returns an error if the cruise data directory cannot be determined.
pub fn jcode_home() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join(JCODE_HOME_DIR))
}

/// The `jcode` executable to invoke: `binary` when a caller names one (tests
/// point it at a stub CLI), else [`JCODE_BINARY`] resolved on `$PATH`.
pub(crate) fn resolve_binary(binary: Option<&Path>) -> PathBuf {
    binary.map_or_else(|| PathBuf::from(JCODE_BINARY), Path::to_path_buf)
}

/// Environment that binds a cruise-launched `jcode` to cruise's private home.
///
/// `JCODE_HOME` is what keeps the user's `~/.jcode` untouched: jcode writes
/// `logs/`, `config.toml` and migration stamps into its home on *any*
/// subcommand, so even a `jcode version` probe would otherwise touch it.
/// `JCODE_NO_TELEMETRY` keeps an embedded run from reporting. Both are applied
/// *after* any workflow `env:`, so a workflow cannot undo either.
fn isolation_env(home: &Path) -> [(&'static str, std::ffi::OsString); 2] {
    [
        ("JCODE_HOME", home.as_os_str().to_os_string()),
        ("JCODE_NO_TELEMETRY", std::ffi::OsString::from("1")),
    ]
}

/// A `jcode` invocation bound to `home`, with telemetry and the auto-update
/// check off and no stdin.
///
/// Every short cruise probe (`version`, `auth status`, `model list`) and
/// `cruise login` build on this; the prompt run adds its own stdio wiring in
/// [`build_command`].
pub(crate) fn jcode_command(binary: &Path, home: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command
        .arg(NO_UPDATE_FLAG)
        .envs(isolation_env(home))
        .stdin(Stdio::null());
    command
}

/// What a failed `jcode` probe actually reported: its exit status plus the tail
/// of its stderr.
///
/// Without this, jcode's own diagnosis (an unreadable `config.toml`, a rejected
/// argument, a provider error) is dropped and the user sees only cruise's
/// "could not read ..." wrapper, which names no cause.
fn probe_detail(output: &std::process::Output) -> String {
    let status = match output.status.code() {
        Some(code) => format!("exit status {code}"),
        None => "terminated by a signal".to_string(),
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    if lines.is_empty() {
        status
    } else {
        format!("{status}; stderr:\n{}", lines[start..].join("\n"))
    }
}

/// Everything that must hold before a `sdk: jcode` prompt can run: a
/// new-enough binary, a prepared private home with cruise registered as an MCP
/// server, no project-local MCP config in the run directory that would shadow
/// it, and at least one authenticated provider.
///
/// `env` is the workflow's `env:`, which [`build_command`] passes to the child:
/// the authentication gate must see the same environment the run will, since
/// jcode also accepts credentials from variables such as `ANTHROPIC_API_KEY`.
///
/// Returns the `$JCODE_HOME` to run under. Runs once per prompt (not per
/// rate-limit attempt), so the two short `jcode` invocations it makes are
/// negligible next to a model turn.
///
/// # Errors
///
/// Returns an error if `jcode` is missing or older than [`MIN_JCODE_VERSION`],
/// if the home or its `mcp.json` cannot be prepared, if the run directory
/// carries an MCP server named [`MCP_SERVER_NAME`], or if no provider is
/// authenticated for cruise's home.
pub(crate) fn preflight(
    binary: Option<&Path>,
    working_dir: Option<&Path>,
    env: &HashMap<String, String>,
    on_notice: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<PathBuf> {
    let bin = resolve_binary(binary);
    let home = jcode_home()?;
    std::fs::create_dir_all(&home)?;
    check_version(&bin, &home)?;
    ensure_mcp_registration(&home)?;
    // jcode discovers project-local MCP config in the directory it runs in:
    // `-C <working_dir>` when the caller gave one, cruise's own cwd otherwise
    // (see [`build_command`]). Check whichever it will actually be.
    let run_dir = match working_dir {
        Some(dir) => Some(dir.to_path_buf()),
        None => std::env::current_dir().ok(),
    };
    if let Some(dir) = run_dir {
        check_project_mcp_config(&dir, on_notice)?;
    }
    ensure_authenticated(&bin, &home, env)?;
    Ok(home)
}

/// Reject a `jcode` older than [`MIN_JCODE_VERSION`], and a missing one with a
/// message that names the install requirement instead of a bare ENOENT.
fn check_version(binary: &Path, home: &Path) -> Result<()> {
    let output = jcode_command(binary, home)
        .args(["version", "--json"])
        .output()
        .map_err(|e| {
            CruiseError::Other(format!(
                "`sdk: jcode` needs the `jcode` CLI on PATH, but running \
                 `{} version` failed: {e}",
                binary.display()
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let semver = serde_json::from_str::<serde_json::Value>(&stdout)
        .ok()
        .and_then(|v| {
            v.get("semver")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| {
            CruiseError::Other(format!(
                "could not read a version from `{} version --json` ({}); \
                 `sdk: jcode` requires jcode {} or newer",
                binary.display(),
                probe_detail(&output),
                format_version(MIN_JCODE_VERSION)
            ))
        })?;
    let parsed = parse_version(&semver).ok_or_else(|| {
        CruiseError::Other(format!(
            "`{} version --json` reported an unparseable version '{semver}'; \
             `sdk: jcode` requires jcode {} or newer",
            binary.display(),
            format_version(MIN_JCODE_VERSION)
        ))
    })?;
    if parsed < MIN_JCODE_VERSION {
        return Err(CruiseError::Other(format!(
            "jcode {semver} is too old for `sdk: jcode`, which requires {} or newer \
             (its `run --ndjson` event stream is the compatibility boundary); \
             upgrade jcode and retry",
            format_version(MIN_JCODE_VERSION)
        )));
    }
    Ok(())
}

/// Parse the leading `major.minor.patch` of a version string, ignoring any
/// pre-release / build suffix.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text
        .trim()
        .trim_start_matches('v')
        .split(['-', '+', ' '])
        .next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn format_version((major, minor, patch): (u64, u64, u64)) -> String {
    format!("{major}.{minor}.{patch}")
}

/// The `mcpServers.cruise` entry cruise registers: this binary, run as the
/// stdio MCP bridge.
///
/// Deliberately free of the per-run socket path. `$JCODE_HOME` is shared by
/// every cruise process on the machine, so a per-run rewrite would have
/// concurrent runs (CLI next to GUI, several repositories) overwrite each
/// other's registration. The socket travels in the `jcode` child's environment
/// instead, which jcode passes on to the MCP servers it spawns.
fn registration_entry(exe: &Path) -> serde_json::Value {
    serde_json::json!({
        "command": exe.to_string_lossy(),
        "args": [MCP_BRIDGE_SUBCOMMAND],
    })
}

/// Register `cruise mcp-bridge` in `<home>/mcp.json`, rewriting only when the
/// entry does not already name the running executable.
///
/// The entry is `current_exe`-derived, so it is rewritten when cruise moves (a
/// reinstall, a different build) and when the CLI and the GUI take turns -- the
/// GUI runs prompts in-process, so `current_exe` is then `cruise-gui`, which
/// serves `mcp-bridge` too. Every rewrite takes an advisory lock and lands
/// through tmp+rename, so concurrent cruise processes sharing the home can
/// neither interleave writes nor expose a partial file, and a `jcode` child
/// reading across a rewrite sees one whole valid entry either way.
///
/// # Errors
///
/// Returns an error if the executable path cannot be determined, if the lock
/// cannot be taken, or if the file cannot be read or replaced.
fn ensure_mcp_registration(home: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let path = home.join("mcp.json");
    let entry = registration_entry(&exe);
    if registration_matches(&path, &entry) {
        return Ok(());
    }
    with_registration_lock(&home.join("mcp.json.cruise-lock"), || {
        // Re-check under the lock: a concurrent cruise may have written the
        // same entry while this process waited.
        if registration_matches(&path, &entry) {
            return Ok(());
        }
        write_registration(&path, &entry)
    })
}

/// Whether `<home>/mcp.json` already carries exactly `entry` under
/// `mcpServers.cruise`.
fn registration_matches(path: &Path, entry: &serde_json::Value) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("mcpServers")
                .and_then(|s| s.get(MCP_SERVER_NAME))
                .cloned()
        })
        .is_some_and(|found| &found == entry)
}

/// Write `entry` into `mcpServers.cruise`, atomically replacing the file.
///
/// Any other server in the file is preserved: the home is cruise's, but a user
/// may legitimately add an MCP server for cruise's runs to use, and silently
/// dropping it on an executable-path change would be a surprise. Anything that
/// is not a JSON object is replaced outright -- there is nothing to merge into.
fn write_registration(path: &Path, entry: &serde_json::Value) -> Result<()> {
    let mut document = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let servers = document
        .as_object_mut()
        .and_then(|o| {
            o.entry("mcpServers")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
        })
        .ok_or_else(|| {
            CruiseError::Other(format!(
                "{} has a non-object `mcpServers`; remove or fix the file so cruise can \
                 register its tool bridge",
                path.display()
            ))
        })?;
    servers.insert(MCP_SERVER_NAME.to_string(), entry.clone());

    // Unique tmp name so two racing processes cannot clobber one another's
    // staging file even if the lock is somehow bypassed.
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut body = serde_json::to_string_pretty(&document)?;
    body.push('\n');
    std::fs::write(&tmp, body)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// Run `f` while holding an exclusive advisory lock on `lock_path`.
#[cfg(unix)]
fn with_registration_lock<T>(lock_path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    use std::os::unix::io::AsRawFd as _;

    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    // SAFETY: `file` owns the descriptor for the whole call, so the fd is valid
    // for both the lock and the unlock below.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let outcome = f();
    // SAFETY: same descriptor, still owned by `file`.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    outcome
}

#[cfg(not(unix))]
fn with_registration_lock<T>(_lock_path: &Path, _f: impl FnOnce() -> Result<T>) -> Result<T> {
    Err(CruiseError::Other(
        "`sdk: jcode` needs advisory file locking to share its jcode home safely between \
         concurrent cruise runs, which this platform does not provide"
            .to_string(),
    ))
}

/// A project-local MCP configuration found in the run directory.
struct ProjectMcpConfig {
    path: PathBuf,
    servers: Vec<String>,
}

/// Reject or warn about project-local MCP configuration in `dir`.
///
/// jcode merges MCP sources last-wins with project-local files *after*
/// `$JCODE_HOME/mcp.json`, so a repository-provided server named
/// [`MCP_SERVER_NAME`] replaces cruise's bridge outright and the model silently
/// loses `ask_user` / `submit_plan` / the rest. That is a hard error. Servers
/// under other names are additive but still load third-party processes into a
/// cruise run, so they are reported through `on_notice`.
///
/// # Errors
///
/// Returns an error if a project-local file defines a server named
/// [`MCP_SERVER_NAME`].
fn check_project_mcp_config(
    dir: &Path,
    on_notice: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<()> {
    for found in project_mcp_configs(dir) {
        if found.servers.iter().any(|s| s == MCP_SERVER_NAME) {
            return Err(CruiseError::Other(format!(
                "{} defines an MCP server named '{MCP_SERVER_NAME}', which jcode would load \
                 instead of cruise's tool bridge (project-local MCP config wins over \
                 $JCODE_HOME/mcp.json), leaving the model without cruise's planning tools. \
                 Rename that server or remove the file to run with `sdk: jcode`.",
                found.path.display()
            )));
        }
        if let Some(notice) = on_notice {
            notice(&format!(
                "jcode will also load project-local MCP server(s) {} from {}",
                found.servers.join(", "),
                found.path.display()
            ));
        }
    }
    Ok(())
}

/// The project-local MCP files present in `dir`, with the server names each
/// declares. Unreadable or malformed files are skipped: jcode will not honor
/// them either, so they cannot shadow cruise's registration.
fn project_mcp_configs(dir: &Path) -> Vec<ProjectMcpConfig> {
    let mut found = Vec::new();
    for relative in PROJECT_MCP_FILES {
        let path = dir.join(relative);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let servers: Vec<String> = value
            .get("mcpServers")
            .and_then(serde_json::Value::as_object)
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        if !servers.is_empty() {
            found.push(ProjectMcpConfig { path, servers });
        }
    }
    found
}

/// Authentication state of cruise's private jcode home, as reported by
/// `jcode auth status --json`.
pub struct AuthStatus {
    /// jcode's own summary flag. Narrower than [`AuthStatus::is_usable`]: it
    /// stays `false` for an `openai-compatible` / custom `[providers.<name>]`
    /// profile even when that profile runs.
    pub any_available: bool,
    /// `(provider id, status)` for every provider jcode knows about.
    pub providers: Vec<(String, String)>,
}

impl AuthStatus {
    /// The providers whose credentials jcode considers usable.
    ///
    /// `"available"` is jcode's own literal for a provider it can authenticate
    /// as; every other value (`"not_configured"`, ...) means it cannot.
    #[must_use]
    pub fn available(&self) -> Vec<&str> {
        self.providers
            .iter()
            .filter(|(_, status)| status == "available")
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// Whether a `sdk: jcode` run can reach a provider at all.
    ///
    /// `any_available` alone is not enough: jcode reports it `false` for an
    /// `openai-compatible` endpoint or a custom `[providers.<name>]` profile
    /// (the arrangement JCODE.md §3.5 documents for custom providers) while
    /// still listing that provider as `available` and running turns through it.
    /// Gating on the flag alone would block those runs outright.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.any_available || !self.available().is_empty()
    }
}

/// Read `jcode auth status --json` for cruise's private home.
///
/// `env` is added to the probe's environment so credentials a workflow supplies
/// that way (jcode reads e.g. `ANTHROPIC_API_KEY`) count as authenticated here
/// exactly as they will for the run itself.
///
/// # Errors
///
/// Returns an error if `jcode` cannot be run or its JSON cannot be read.
pub fn auth_status<S: std::hash::BuildHasher>(
    binary: Option<&Path>,
    home: &Path,
    env: &HashMap<String, String, S>,
) -> Result<AuthStatus> {
    let bin = resolve_binary(binary);
    let mut command = std::process::Command::new(&bin);
    command
        .arg(NO_UPDATE_FLAG)
        .args(["auth", "status", "--json"]);
    // Workflow `env:` first, so cruise's isolation settings win over it.
    command
        .envs(env)
        .envs(isolation_env(home))
        .stdin(Stdio::null());
    let output = command.output().map_err(|e| {
        CruiseError::Other(format!(
            "`sdk: jcode` needs the `jcode` CLI on PATH, but running \
             `{} auth status` failed: {e}",
            bin.display()
        ))
    })?;
    parse_auth_status(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        CruiseError::Other(format!(
            "could not read authentication status from `{} auth status --json` ({})",
            bin.display(),
            probe_detail(&output)
        ))
    })
}

fn parse_auth_status(stdout: &str) -> Option<AuthStatus> {
    let value: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let any_available = value.get("any_available")?.as_bool()?;
    let providers = value
        .get("providers")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    let id = e.get("id").and_then(serde_json::Value::as_str)?;
                    let status = e.get("status").and_then(serde_json::Value::as_str)?;
                    Some((id.to_string(), status.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(AuthStatus {
        any_available,
        providers,
    })
}

/// Fail with `cruise login` guidance when cruise's jcode home has no usable
/// credentials, instead of letting the run reach the model and come back with
/// jcode's raw provider error.
fn ensure_authenticated(binary: &Path, home: &Path, env: &HashMap<String, String>) -> Result<()> {
    if auth_status(Some(binary), home, env)?.is_usable() {
        return Ok(());
    }
    Err(CruiseError::Other(format!(
        "no provider is authenticated for `sdk: jcode`. cruise keeps its jcode credentials \
         separate from your own `~/.jcode`, in {}; run `cruise login` to sign in there \
         (`cruise login --status` lists what is configured).",
        home.display()
    )))
}

/// A cruise `provider/model[:effort]` reference split into the parts
/// `jcode run` takes: `(provider, model, effort)`, each unset when the
/// reference leaves it to jcode.
pub(crate) type ModelRef = (Option<String>, Option<String>, Option<EffortLevel>);

/// Split a cruise `provider/model[:effort]` reference into the parts
/// `jcode run` takes.
///
/// Accepted forms mirror the other SDK backends:
///
/// - `None` / empty -> everything unset; jcode picks its configured default
///   provider and model.
/// - `"model"` (no `/`) -> `--model model`, provider left to jcode.
/// - `"provider/model"` -> `--provider provider --model model`.
/// - `":effort"` alone -> effort only, model and provider left to jcode.
///
/// A `/` with an empty side (`"/model"`, `"provider/"`) is a configuration
/// error: passing it through would surface as an opaque provider or model
/// lookup failure inside jcode.
///
/// The `:effort` suffix is always split off. `jcode run` has no effort flag, so
/// it must never reach `--model`; [`build_command`] forwards it through jcode's
/// reasoning-effort environment overrides instead.
///
/// # Errors
///
/// Returns an error if the reference has a `/` with an empty provider or model.
pub(crate) fn parse_model_ref(model_ref: Option<&str>) -> Result<ModelRef> {
    let Some(raw) = model_ref.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((None, None, None));
    };
    let (base, suffix) = crate::backend::effort::split_thinking_suffix(raw);
    let effort = suffix.and_then(crate::backend::effort::effort_from_suffix);
    let base = base.trim();
    if base.is_empty() {
        return Ok((None, None, effort));
    }
    match base.split_once('/') {
        None => Ok((None, Some(base.to_string()), effort)),
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
            Ok((Some(provider.to_string()), Some(model.to_string()), effort))
        }
        Some(_) => Err(CruiseError::InvalidStepConfig(format!(
            "invalid model reference '{raw}' for `sdk: jcode`: expected \
             'provider/model[:effort]', 'model[:effort]', or no value"
        ))),
    }
}

/// Run `prompt` through the `jcode` CLI and surface output as [`StreamChunk`]s
/// on a dedicated thread.
///
/// `text_delta` events become [`StreamChunk::Delta`], the opening `start` event's
/// session id becomes [`StreamChunk::Session`], and the turn's `done` event
/// becomes [`StreamChunk::Done`] with jcode's own final text. A limit-shaped
/// `error` event becomes [`StreamChunk::Limit`], everything else terminal
/// becomes [`StreamChunk::Error`]. Every other event kind (`connection_phase`,
/// `reasoning_delta`, `tool_start` / `tool_done`, `message_end`, and anything a
/// newer jcode adds) is ignored -- cruise only renders assistant text.
///
/// The run ends early, with no terminal chunk, when
/// [`JcodeRunnerConfig::cancel`] fires or the returned receiver is dropped;
/// either way the child is dropped, which kills it.
pub(crate) fn stream_agent(config: JcodeRunnerConfig, prompt: String) -> Receiver<StreamChunk> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || run_in_runtime(config, prompt, tx));
    rx
}

fn run_in_runtime(config: JcodeRunnerConfig, prompt: String, tx: Sender<StreamChunk>) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        let _ = tx.send(StreamChunk::Error("failed to build tokio runtime".into()));
        return;
    };
    rt.block_on(async move { run_async(config, prompt, &tx).await });
}

/// Terminal outcome of one child run. Delta/Session chunks are streamed out
/// while stdout is being read; the terminal verdict waits until stderr has been
/// drained so it can consult the trailing lines.
enum Terminal {
    /// A `done` event arrived, carrying jcode's final text for the turn.
    Completed(String),
    /// An `error` event arrived.
    Failed(String),
    /// Stdout ended without a `done` or `error` event: the child died before
    /// finishing the turn (unknown flag, unknown `--resume` id, killed).
    NoResult,
}

/// Resolves when `token` is cancelled, or waits forever if there is no token.
async fn until_cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(t) => t.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn run_async(config: JcodeRunnerConfig, prompt: String, tx: &Sender<StreamChunk>) {
    let cancel = config.cancel.clone();
    let mut command = build_command(&config, &prompt);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            send_failure(
                tx,
                &format!("failed to start the jcode CLI: {e}"),
                false,
                &[],
                None,
            );
            return;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        send_failure(tx, "the jcode CLI provided no stdout", false, &[], None);
        return;
    };

    // Drain stderr into a small ring buffer: an invocation that fails before it
    // can emit NDJSON (bad flag, unknown session, provider auth) reports only
    // there, and a rate-limit line can arrive there too.
    let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let drain_handle = child.stderr.take().map(|stderr| {
        let buf = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                push_stderr_line(&buf, line);
            }
        })
    });

    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut provider: Option<String> = None;
    let terminal = fold_events(&mut lines, cancel.as_ref(), tx, &mut provider).await;
    let Some(terminal) = terminal else {
        // Abandoned: cancelled, or the receiver is gone. There is nobody to
        // report to, but the child still has to be collected here -- see
        // [`reap`].
        reap(&mut child).await;
        return;
    };
    // A finished turn waits for jcode to exit by itself: it persists the session
    // under $JCODE_HOME on shutdown, and `--resume` for the next planning turn
    // depends on that file. Only once the child is gone is stderr at EOF, so the
    // drain task is awaited after that -- snapshotting earlier would miss a
    // rate-limit line that arrived in the same scheduling tick.
    wait_for_exit(&mut child, &mut lines, cancel.as_ref()).await;
    await_stderr_drain(drain_handle).await;
    let tail = snapshot_stderr_tail(&stderr_tail);

    match terminal {
        Terminal::Completed(text) => {
            let _ = tx.send(StreamChunk::Done(text));
        }
        // jcode's own `error.message` is the diagnosis of the turn, so it alone
        // decides retryable vs. permanent. Classifying the stderr tail alongside
        // it would let an unrelated `429` in a log line turn a permanent failure
        // (authentication, invalid request) into a retry, which PROHIBITED §7
        // forbids; the tail is still appended, for context only.
        Terminal::Failed(message) => {
            let limited = is_jcode_rate_limit_message(&message);
            send_failure(tx, &message, limited, &tail, provider.as_deref());
        }
        // No event message exists here -- the child died before emitting NDJSON
        // -- so stderr carries the only diagnosis there is, and is what gets
        // classified.
        Terminal::NoResult => {
            let limited = tail.iter().any(|line| is_jcode_rate_limit_message(line));
            send_failure(tx, NO_RESULT_MESSAGE, limited, &tail, provider.as_deref());
        }
    }
}

/// Kill the child and collect it.
///
/// `kill_on_drop` only signals: tokio reaps a killed child when its process
/// driver next polls, and this backend drops its current-thread runtime as soon
/// as the run returns, so a child left to `kill_on_drop` alone would linger as a
/// zombie for the whole life of the cruise process.
async fn reap(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Wait for a child that has already reported its turn to exit, then collect it.
///
/// `lines` stays alive and drained for the whole wait: dropping the stdout
/// reader closes the pipe, and a jcode that still has shutdown output to write
/// would then die of EPIPE before flushing the session file `--resume` needs.
///
/// Cancellation and [`CHILD_EXIT_TIMEOUT`] both cut the wait short, and the
/// child is killed and reaped instead of being left behind.
async fn wait_for_exit(
    child: &mut tokio::process::Child,
    lines: &mut ChildLines,
    cancel: Option<&CancellationToken>,
) {
    let deadline = tokio::time::sleep(CHILD_EXIT_TIMEOUT);
    tokio::pin!(deadline);
    let mut stdout_open = true;
    loop {
        tokio::select! {
            biased;
            () = until_cancelled(cancel) => break,
            () = &mut deadline => break,
            _ = child.wait() => return,
            line = lines.next_line(), if stdout_open => {
                stdout_open = matches!(line, Ok(Some(_)));
            }
        }
    }
    reap(child).await;
}

/// Line reader over the child's stdout, from which the NDJSON events are read.
type ChildLines = tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>;

/// Read NDJSON events from `lines`, streaming [`StreamChunk::Session`] and
/// [`StreamChunk::Delta`] out as they arrive, and return the turn's terminal
/// verdict.
///
/// `None` means "abandon quietly": the run was cancelled or the receiver is
/// gone, so there is nobody left to report a verdict to. `provider` collects the
/// last provider an event named, which labels a [`LimitError`].
async fn fold_events(
    lines: &mut ChildLines,
    cancel: Option<&CancellationToken>,
    tx: &Sender<StreamChunk>,
    provider: &mut Option<String>,
) -> Option<Terminal> {
    let mut session_reported = false;
    loop {
        let next = tokio::select! {
            biased;
            () = until_cancelled(cancel) => return None,
            line = lines.next_line() => line,
        };
        let line = match next {
            Ok(Some(line)) => line,
            Ok(None) => return Some(Terminal::NoResult),
            Err(e) => {
                return Some(Terminal::Failed(format!(
                    "failed to read the jcode CLI output: {e}"
                )));
            }
        };
        let Some(event) = parse_event(&line) else {
            continue;
        };
        if let Some(named) = event.provider {
            *provider = Some(named);
        }
        if let Some(id) = event.session_id
            && !report_session(tx, &mut session_reported, &id)
        {
            return None;
        }
        match event.kind {
            EventKind::Delta(text) => {
                if tx.send(StreamChunk::Delta(text)).is_err() {
                    return None;
                }
            }
            EventKind::Done(text) => return Some(Terminal::Completed(text)),
            EventKind::Failed(message) => return Some(Terminal::Failed(message)),
            EventKind::Other => {}
        }
    }
}

/// What one NDJSON event contributes to the stream.
enum EventKind {
    /// `text_delta`: assistant text to forward.
    Delta(String),
    /// `done`: the turn finished with this final text.
    Done(String),
    /// `error`: the turn failed with this message.
    Failed(String),
    /// Anything else, including events a newer jcode adds.
    Other,
}

/// One parsed NDJSON event: its contribution plus the session id and provider
/// it may carry (both appear on several event kinds, not just `start`).
struct Event {
    kind: EventKind,
    session_id: Option<String>,
    provider: Option<String>,
}

/// Parse one NDJSON line into the event shape cruise cares about.
///
/// Returns `None` for a line that is not a JSON object: jcode's non-NDJSON
/// diagnostics can share stdout with the event stream, and those lines carry
/// nothing for the fold.
///
/// Unrecognized `type` values map to [`EventKind::Other`] rather than failing,
/// which is what lets a newer jcode add events without breaking this backend.
fn parse_event(line: &str) -> Option<Event> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if !value.is_object() {
        return None;
    }
    let string = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let kind = match value.get("type").and_then(serde_json::Value::as_str)? {
        "text_delta" => EventKind::Delta(string("text").unwrap_or_default()),
        // `done.text` is jcode's authoritative text for the whole turn, so it
        // replaces the accumulated deltas in the reducer rather than being
        // appended to them.
        "done" => EventKind::Done(string("text").unwrap_or_default()),
        "error" => EventKind::Failed(
            string("message").unwrap_or_else(|| "the jcode CLI reported an error".to_string()),
        ),
        _ => EventKind::Other,
    };
    Some(Event {
        kind,
        session_id: string("session_id"),
        provider: string("provider"),
    })
}

/// Retain `line` as the newest entry of the bounded stderr tail.
fn push_stderr_line(buf: &Arc<Mutex<VecDeque<String>>>, line: String) {
    if let Ok(mut guard) = buf.lock() {
        if guard.len() == STDERR_TAIL_LINES {
            guard.pop_front();
        }
        guard.push_back(line);
    }
}

fn snapshot_stderr_tail(buf: &Arc<Mutex<VecDeque<String>>>) -> Vec<String> {
    buf.lock()
        .map(|guard| guard.iter().cloned().collect())
        .unwrap_or_default()
}

/// Cap on how long a finished turn waits for the `jcode` child to exit by
/// itself before it is killed. jcode exits as soon as the turn is reported, so
/// this only bounds a wedged child; the wait exists so jcode can flush the
/// session it just reported (the one a follow-up `--resume` needs).
const CHILD_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Cap on how long [`await_stderr_drain`] waits for the drain task after the
/// child has exited. The pipe is already at EOF by then, so this only guards
/// against a task that cannot make progress.
const STDERR_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

async fn await_stderr_drain(handle: Option<tokio::task::JoinHandle<()>>) {
    if let Some(handle) = handle {
        let _ = tokio::time::timeout(STDERR_DRAIN_TIMEOUT, handle).await;
    }
}

/// Report the run's session id the first time an event names one, tracking that
/// in `reported` so later events don't repeat it. Returns `false` when the
/// receiver is gone and the run should be abandoned.
fn report_session(tx: &Sender<StreamChunk>, reported: &mut bool, id: &str) -> bool {
    if *reported || id.is_empty() {
        return true;
    }
    *reported = true;
    tx.send(StreamChunk::Session(id.to_string())).is_ok()
}

/// Rate/usage-limit detector for the free-form error text jcode reports.
///
/// Extends the command backend's [`crate::step::command::is_rate_limited`] --
/// which covers `rate limit` / `429` / `too many requests` -- with the limit
/// wording jcode's provider runtimes add. Deliberately narrow: only limit
/// conditions are retryable, so permanent failures (authentication, invalid
/// request, context overflow, exhausted balance) must not match.
fn is_jcode_rate_limit_message(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    crate::step::command::is_rate_limited(&lower)
        || lower.contains("usage limit")
        || lower.contains("session limit")
        || lower.contains("overloaded")
}

/// Report a failed turn: `msg` plus the collected stderr tail for context, as a
/// retryable [`StreamChunk::Limit`] when `limited`, else a [`StreamChunk::Error`].
///
/// The tail is appended because jcode's diagnosis for an invocation that never
/// produced NDJSON (unknown flag, unknown `--resume` id, provider auth failure)
/// lives on stderr only -- without it the failure reaches the user as an
/// undiagnosable one-liner. Whether the tail also *classifies* the failure is
/// the caller's call: see the match in [`run_async`].
fn send_failure(
    tx: &Sender<StreamChunk>,
    msg: &str,
    limited: bool,
    stderr_tail: &[String],
    provider: Option<&str>,
) {
    if limited {
        let _ = tx.send(StreamChunk::Limit(LimitError {
            provider: provider.unwrap_or(PROVIDER_LABEL).to_string(),
        }));
        return;
    }
    let text = if stderr_tail.is_empty() {
        msg.to_string()
    } else {
        format!("{msg}\njcode stderr:\n{}", stderr_tail.join("\n"))
    };
    let _ = tx.send(StreamChunk::Error(text));
}

/// Build the `jcode run` invocation for `config`.
fn build_command(config: &JcodeRunnerConfig, prompt: &str) -> tokio::process::Command {
    let binary = resolve_binary(config.binary.as_deref());
    let mut command = tokio::process::Command::new(binary);
    command.arg(NO_UPDATE_FLAG).arg("run").arg("--ndjson");
    // `--quiet` drops jcode's own status chatter, leaving stdout as pure NDJSON.
    command.arg("--quiet");
    if let Some(model) = &config.model {
        command.arg("--model").arg(model);
    }
    if let Some(provider) = &config.provider {
        command.arg("--provider").arg(provider);
    }
    if let Some(session) = &config.resume_session_id {
        command.arg("--resume").arg(session);
    }
    if let Some(cwd) = &config.cwd {
        command.arg("-C").arg(cwd);
        command.current_dir(cwd);
    }
    command.arg(prompt);

    // Workflow `env:` first, so cruise's own isolation settings below cannot be
    // overridden by a workflow into reading the user's jcode home or enabling
    // telemetry.
    command.envs(&config.env);
    command.envs(isolation_env(&config.home));
    command.env(TOOL_SOCKET_ENV, &config.tool_socket);
    if let Some(effort) = config.effort {
        // `jcode run` has no effort flag; these are jcode's environment
        // overrides for its `[provider] *_reasoning_effort` config keys, and are
        // ignored by providers and models that do not support reasoning effort.
        command.env("JCODE_ANTHROPIC_REASONING_EFFORT", effort.as_str());
        command.env("JCODE_OPENAI_REASONING_EFFORT", effort.as_str());
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_ref_accepts_the_documented_forms() {
        type Expected = (
            Option<&'static str>,
            Option<&'static str>,
            Option<EffortLevel>,
        );
        let cases: &[(Option<&str>, Expected)] = &[
            (None, (None, None, None)),
            (Some(""), (None, None, None)),
            (Some("  "), (None, None, None)),
            (Some("claude-opus-5"), (None, Some("claude-opus-5"), None)),
            (
                Some("claude/claude-opus-5"),
                (Some("claude"), Some("claude-opus-5"), None),
            ),
            (
                Some("openai/gpt-5.6:high"),
                (Some("openai"), Some("gpt-5.6"), Some(EffortLevel::High)),
            ),
            (
                Some("gpt-5.6:xhigh"),
                (None, Some("gpt-5.6"), Some(EffortLevel::XHigh)),
            ),
            (Some(":max"), (None, None, Some(EffortLevel::Max))),
        ];
        for (input, expected) in cases {
            let got = parse_model_ref(*input).unwrap_or_else(|e| panic!("{input:?}: {e:?}"));
            assert_eq!(
                (got.0.as_deref(), got.1.as_deref(), got.2),
                *expected,
                "for {input:?}"
            );
        }
    }

    /// A model id carrying a legitimate `:` (an `OpenRouter` variant) must not
    /// lose its suffix to effort parsing.
    #[test]
    fn parse_model_ref_keeps_non_effort_colon_suffixes_in_the_model_id() {
        let (provider, model, effort) =
            parse_model_ref(Some("openrouter/meta-llama/llama-3.1-8b-instruct:free"))
                .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(provider.as_deref(), Some("openrouter"));
        assert_eq!(
            model.as_deref(),
            Some("meta-llama/llama-3.1-8b-instruct:free")
        );
        assert_eq!(effort, None);
    }

    #[test]
    fn parse_model_ref_rejects_an_empty_provider_or_model_side() {
        for input in ["/model", "provider/"] {
            let err = parse_model_ref(Some(input))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains("provider/model"), "for {input}: got {err}");
        }
    }

    #[test]
    fn parse_event_maps_the_stream_shaping_events() {
        let delta = parse_event(r#"{"type":"text_delta","text":"hi"}"#)
            .unwrap_or_else(|| panic!("expected an event"));
        assert!(matches!(&delta.kind, EventKind::Delta(t) if t == "hi"));

        let start = parse_event(
            r#"{"type":"start","session_id":"session_herb_1","model":"m","provider":"Claude"}"#,
        )
        .unwrap_or_else(|| panic!("expected an event"));
        assert!(matches!(start.kind, EventKind::Other));
        assert_eq!(start.session_id.as_deref(), Some("session_herb_1"));
        assert_eq!(start.provider.as_deref(), Some("Claude"));

        let done = parse_event(r#"{"type":"done","text":"full turn","session_id":"s1"}"#)
            .unwrap_or_else(|| panic!("expected an event"));
        assert!(matches!(&done.kind, EventKind::Done(t) if t == "full turn"));
        assert_eq!(done.session_id.as_deref(), Some("s1"));

        let failed = parse_event(r#"{"type":"error","message":"boom","provider":"Claude"}"#)
            .unwrap_or_else(|| panic!("expected an event"));
        assert!(matches!(&failed.kind, EventKind::Failed(m) if m == "boom"));
    }

    /// A newer jcode adding events must not break the fold: unknown `type`s are
    /// ignored rather than treated as a failure.
    #[test]
    fn parse_event_ignores_unknown_and_non_object_lines() {
        for line in [
            r#"{"type":"connection_phase","phase":"sending request"}"#,
            r#"{"type":"tool_start","id":"call_1","name":"mcp__cruise__ask_user"}"#,
            r#"{"type":"message_end","stop_reason":"stop"}"#,
            r#"{"type":"a_future_event","payload":{}}"#,
        ] {
            let event = parse_event(line).unwrap_or_else(|| panic!("expected an event: {line}"));
            assert!(matches!(event.kind, EventKind::Other), "for {line}");
        }
        for line in ["", "Error: something went wrong", "[1,2,3]", "null"] {
            assert!(parse_event(line).is_none(), "for {line:?}");
        }
    }

    #[test]
    fn rate_limit_classification_is_limited_to_limit_conditions() {
        for message in [
            "HTTP 429 Too Many Requests",
            "Anthropic API error: rate limit exceeded",
            "You have hit your usage limit for this window",
            "session limit reached",
            "Provider returned: overloaded_error",
        ] {
            assert!(is_jcode_rate_limit_message(message), "for {message}");
        }
        for message in [
            "Anthropic API error (401 Unauthorized): API key is invalid.",
            "invalid_request_error: model not found",
            "prompt is too long: 250000 tokens > 200000 maximum",
            "payment required: balance exhausted",
        ] {
            assert!(!is_jcode_rate_limit_message(message), "for {message}");
        }
    }

    #[test]
    fn parse_version_reads_the_leading_semver_triple() {
        assert_eq!(parse_version("0.81.1"), Some((0, 81, 1)));
        assert_eq!(parse_version("v0.81.1"), Some((0, 81, 1)));
        assert_eq!(parse_version("0.82.0-rc.1"), Some((0, 82, 0)));
        assert_eq!(parse_version(" 1.0.0 "), Some((1, 0, 0)));
        assert_eq!(parse_version("0.81"), Some((0, 81, 0)));
        assert_eq!(parse_version("not-a-version"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn parse_auth_status_reads_availability_and_providers() {
        let status = parse_auth_status(
            r#"{"any_available":true,"providers":[
                 {"id":"claude","status":"not_configured"},
                 {"id":"anthropic-api","status":"available"}]}"#,
        )
        .unwrap_or_else(|| panic!("expected a status"));
        assert!(status.any_available);
        assert_eq!(status.available(), vec!["anthropic-api"]);
        assert!(parse_auth_status("not json").is_none());
    }

    /// jcode reports `any_available: false` for an `openai-compatible` endpoint
    /// or a custom `[providers.<name>]` profile while still listing it as
    /// `available` and running turns through it (measured on jcode 0.81.1), so
    /// the run gate must not key on the flag alone.
    #[test]
    fn a_custom_provider_profile_counts_as_usable_despite_any_available_false() {
        let status = parse_auth_status(
            r#"{"any_available":false,"providers":[
                 {"id":"claude","status":"not_configured"},
                 {"id":"openai-compatible","status":"available"}]}"#,
        )
        .unwrap_or_else(|| panic!("expected a status"));
        assert!(!status.any_available);
        assert_eq!(status.available(), vec!["openai-compatible"]);
        assert!(status.is_usable());
    }

    #[test]
    fn a_home_with_no_configured_provider_is_not_usable() {
        let status = parse_auth_status(
            r#"{"any_available":false,"providers":[{"id":"claude","status":"not_configured"}]}"#,
        )
        .unwrap_or_else(|| panic!("expected a status"));
        assert!(!status.is_usable());
    }

    /// Cruise's jcode home must be its own directory under the cruise data dir,
    /// never the user's `~/.jcode` (PROHIBITED §6).
    #[test]
    fn jcode_home_is_cruise_owned_and_not_the_user_home() {
        let _guard = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let home = jcode_home().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            home,
            tmp.path()
                .join(".local")
                .join("share")
                .join("cruise")
                .join(JCODE_HOME_DIR)
        );
    }

    mod mcp_registration {
        use super::*;

        fn read_json(path: &Path) -> serde_json::Value {
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{e:?}"));
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e:?}: {text}"))
        }

        #[test]
        fn writes_a_fixed_cruise_entry_pointing_at_this_executable() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            ensure_mcp_registration(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
            let document = read_json(&tmp.path().join("mcp.json"));
            let entry = &document["mcpServers"][MCP_SERVER_NAME];
            let exe = std::env::current_exe().unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(entry["command"], exe.to_string_lossy().as_ref());
            assert_eq!(entry["args"], serde_json::json!([MCP_BRIDGE_SUBCOMMAND]));
            // No socket path in the file: it is per-run and travels in the
            // child's environment, so concurrent runs never rewrite each other.
            assert!(
                !document.to_string().contains(TOOL_SOCKET_ENV),
                "mcp.json must not carry the per-run socket: {document}"
            );
        }

        /// The steady state is a no-op, which is what makes a shared
        /// `$JCODE_HOME` safe for concurrent runs: the file is only rewritten
        /// when the executable path changes.
        #[test]
        fn is_idempotent_and_leaves_the_file_untouched_on_a_second_call() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let path = tmp.path().join("mcp.json");
            ensure_mcp_registration(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
            let first = std::fs::metadata(&path).unwrap_or_else(|e| panic!("{e:?}"));
            let before = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e:?}"));
            ensure_mcp_registration(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
            let after = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(before, after);
            // Same inode: not replaced via rename.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                let second = std::fs::metadata(&path).unwrap_or_else(|e| panic!("{e:?}"));
                assert_eq!(first.ino(), second.ino());
            }
            let _ = first;
        }

        #[test]
        fn replaces_a_stale_cruise_entry_and_keeps_other_servers() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let path = tmp.path().join("mcp.json");
            std::fs::write(
                &path,
                r#"{"mcpServers":{"cruise":{"command":"/old/cruise","args":["mcp-bridge"]},
                                   "other":{"command":"other-server","args":[]}}}"#,
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
            ensure_mcp_registration(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
            let document = read_json(&path);
            let exe = std::env::current_exe().unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(
                document["mcpServers"][MCP_SERVER_NAME]["command"],
                exe.to_string_lossy().as_ref()
            );
            assert_eq!(document["mcpServers"]["other"]["command"], "other-server");
        }

        #[test]
        fn replaces_a_file_that_is_not_a_json_object() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let path = tmp.path().join("mcp.json");
            std::fs::write(&path, "this is not json").unwrap_or_else(|e| panic!("{e:?}"));
            ensure_mcp_registration(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
            let document = read_json(&path);
            assert!(document["mcpServers"][MCP_SERVER_NAME].is_object());
        }

        #[test]
        fn leaves_no_temporary_file_behind() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            ensure_mcp_registration(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
            let strays: Vec<String> = std::fs::read_dir(tmp.path())
                .unwrap_or_else(|e| panic!("{e:?}"))
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
                .map(|path| path.to_string_lossy().to_string())
                .collect();
            assert!(strays.is_empty(), "left behind {strays:?}");
        }
    }

    mod project_mcp {
        use super::*;

        fn write(dir: &Path, relative: &str, body: &str) {
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{e:?}"));
            }
            std::fs::write(&path, body).unwrap_or_else(|e| panic!("{e:?}"));
        }

        /// jcode merges project-local MCP config *after* `$JCODE_HOME/mcp.json`,
        /// so a repository server named `cruise` replaces cruise's bridge and
        /// the model silently loses every planning tool. That must be an error,
        /// not a warning.
        #[test]
        fn a_project_server_named_cruise_is_rejected_for_every_discovered_path() {
            for relative in PROJECT_MCP_FILES {
                let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
                write(
                    tmp.path(),
                    relative,
                    r#"{"mcpServers":{"cruise":{"command":"./evil","args":[]}}}"#,
                );
                let err = check_project_mcp_config(tmp.path(), None)
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                assert!(err.contains(relative), "for {relative}: got {err}");
                assert!(err.contains(MCP_SERVER_NAME), "for {relative}: got {err}");
            }
        }

        #[test]
        fn other_project_servers_are_reported_but_allowed() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            write(
                tmp.path(),
                ".mcp.json",
                r#"{"mcpServers":{"projlocal":{"command":"./server","args":[]}}}"#,
            );
            let notices = std::sync::Mutex::new(Vec::<String>::new());
            let sink = |text: &str| {
                if let Ok(mut guard) = notices.lock() {
                    guard.push(text.to_string());
                }
            };
            check_project_mcp_config(tmp.path(), Some(&sink)).unwrap_or_else(|e| panic!("{e:?}"));
            let notices = notices.lock().unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(notices.len(), 1, "got {notices:?}");
            assert!(notices[0].contains("projlocal"), "got {notices:?}");
            assert!(notices[0].contains(".mcp.json"), "got {notices:?}");
        }

        #[test]
        fn a_clean_directory_produces_no_findings() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            assert!(project_mcp_configs(tmp.path()).is_empty());
            check_project_mcp_config(tmp.path(), None).unwrap_or_else(|e| panic!("{e:?}"));
        }

        /// jcode ignores files it cannot parse, so they cannot shadow cruise's
        /// registration and must not block the run either.
        #[test]
        fn malformed_or_serverless_files_are_skipped() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            write(tmp.path(), ".mcp.json", "{ not json");
            write(tmp.path(), ".jcode/mcp.json", r#"{"mcpServers":{}}"#);
            assert!(project_mcp_configs(tmp.path()).is_empty());
            check_project_mcp_config(tmp.path(), None).unwrap_or_else(|e| panic!("{e:?}"));
        }
    }

    mod invocation {
        use super::*;

        fn config(home: &Path) -> JcodeRunnerConfig {
            JcodeRunnerConfig {
                home: home.to_path_buf(),
                tool_socket: PathBuf::from("/tmp/cruise-test.sock"),
                ..JcodeRunnerConfig::default()
            }
        }

        fn args_of(command: &tokio::process::Command) -> Vec<String> {
            command
                .as_std()
                .get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect()
        }

        fn env_of(command: &tokio::process::Command, key: &str) -> Option<String> {
            command.as_std().get_envs().find_map(|(k, v)| {
                (k == key).then(|| v.unwrap_or_default().to_string_lossy().to_string())
            })
        }

        #[test]
        fn always_streams_ndjson_and_suppresses_updates() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let command = build_command(&config(tmp.path()), "do the thing");
            let args = args_of(&command);
            assert!(args.contains(&"run".to_string()), "got {args:?}");
            assert!(args.contains(&"--ndjson".to_string()), "got {args:?}");
            assert!(args.contains(&"--no-update".to_string()), "got {args:?}");
            assert_eq!(args.last().map(String::as_str), Some("do the thing"));
        }

        #[test]
        fn isolates_the_home_and_disables_telemetry() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let command = build_command(&config(tmp.path()), "p");
            assert_eq!(
                env_of(&command, "JCODE_HOME").as_deref(),
                Some(tmp.path().to_string_lossy().as_ref())
            );
            assert_eq!(env_of(&command, "JCODE_NO_TELEMETRY").as_deref(), Some("1"));
            assert_eq!(
                env_of(&command, TOOL_SOCKET_ENV).as_deref(),
                Some("/tmp/cruise-test.sock")
            );
        }

        /// A workflow `env:` block must not be able to redirect jcode at the
        /// user's home or re-enable telemetry (PROHIBITED §6).
        #[test]
        fn workflow_env_cannot_override_the_isolation_settings() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let mut cfg = config(tmp.path());
            cfg.env
                .insert("JCODE_HOME".to_string(), "/home/someone/.jcode".to_string());
            cfg.env
                .insert("JCODE_NO_TELEMETRY".to_string(), "0".to_string());
            cfg.env.insert("MY_VAR".to_string(), "kept".to_string());
            let command = build_command(&cfg, "p");
            assert_eq!(
                env_of(&command, "JCODE_HOME").as_deref(),
                Some(tmp.path().to_string_lossy().as_ref())
            );
            assert_eq!(env_of(&command, "JCODE_NO_TELEMETRY").as_deref(), Some("1"));
            assert_eq!(env_of(&command, "MY_VAR").as_deref(), Some("kept"));
        }

        #[test]
        fn model_provider_resume_and_cwd_reach_the_command_line() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let mut cfg = config(tmp.path());
            cfg.model = Some("claude-opus-5".to_string());
            cfg.provider = Some("claude".to_string());
            cfg.resume_session_id = Some("session_herb_1".to_string());
            cfg.cwd = Some(tmp.path().to_path_buf());
            let args = args_of(&build_command(&cfg, "p"));
            let pair = |flag: &str| {
                args.iter()
                    .position(|a| a == flag)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            assert_eq!(pair("--model").as_deref(), Some("claude-opus-5"));
            assert_eq!(pair("--provider").as_deref(), Some("claude"));
            assert_eq!(pair("--resume").as_deref(), Some("session_herb_1"));
            assert_eq!(
                pair("-C").as_deref(),
                Some(tmp.path().to_string_lossy().as_ref())
            );
        }

        /// `jcode run` has no effort flag, so the tier must travel as jcode's own
        /// reasoning-effort environment overrides and must never be appended to
        /// `--model` (which would make the model id unresolvable).
        #[test]
        fn effort_travels_as_environment_overrides_not_as_a_model_suffix() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let mut cfg = config(tmp.path());
            cfg.model = Some("gpt-5.6".to_string());
            cfg.effort = Some(EffortLevel::XHigh);
            let command = build_command(&cfg, "p");
            assert_eq!(
                env_of(&command, "JCODE_OPENAI_REASONING_EFFORT").as_deref(),
                Some("xhigh")
            );
            assert_eq!(
                env_of(&command, "JCODE_ANTHROPIC_REASONING_EFFORT").as_deref(),
                Some("xhigh")
            );
            assert!(
                args_of(&command).contains(&"gpt-5.6".to_string()),
                "the model id must stay clean"
            );
        }

        #[test]
        fn no_effort_leaves_the_reasoning_overrides_unset() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let command = build_command(&config(tmp.path()), "p");
            assert_eq!(env_of(&command, "JCODE_OPENAI_REASONING_EFFORT"), None);
        }
    }

    /// End-to-end fold against a stub `jcode` that replays a recorded NDJSON
    /// stream, so the mapping is exercised without the real binary installed.
    #[cfg(unix)]
    mod stub_cli {
        use super::*;
        use std::time::Duration;

        /// Install an executable stub at `<dir>/jcode` that prints `stdout_body`
        /// on stdout and `stderr_body` on stderr, then exits with `code`.
        ///
        /// It also records what cruise handed it: `<binary>.env` gets
        /// `$JCODE_HOME|$CRUISE_TOOL_SOCKET`, `<binary>.args` the argv, and
        /// `<binary>.marker` the `CRUISE_PROBE_MARKER` variable a test uses to
        /// prove workflow `env:` reached the child.
        fn install_stub(dir: &Path, stdout_body: &str, stderr_body: &str, code: i32) -> PathBuf {
            use std::os::unix::fs::PermissionsExt as _;
            let path = dir.join("jcode");
            let script = format!(
                "#!/bin/sh\nprintf '%s' \"$JCODE_HOME|$CRUISE_TOOL_SOCKET\" > \"$0.env\"\n\
                 printf '%s' \"$*\" > \"$0.args\"\n\
                 printf '%s' \"$CRUISE_PROBE_MARKER\" > \"$0.marker\"\n\
                 cat <<'CRUISE_EOF'\n{stdout_body}\nCRUISE_EOF\n\
                 cat >&2 <<'CRUISE_ERR'\n{stderr_body}\nCRUISE_ERR\nexit {code}\n"
            );
            std::fs::write(&path, script).unwrap_or_else(|e| panic!("{e:?}"));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|e| panic!("{e:?}"));
            path
        }

        fn drain(rx: &Receiver<StreamChunk>) -> Vec<StreamChunk> {
            let mut chunks = Vec::new();
            while let Ok(chunk) = rx.recv_timeout(Duration::from_secs(20)) {
                chunks.push(chunk);
            }
            chunks
        }

        fn run_stub(stdout_body: &str, stderr_body: &str, code: i32) -> Vec<StreamChunk> {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(tmp.path(), stdout_body, stderr_body, code);
            let rx = stream_agent(
                JcodeRunnerConfig {
                    home: tmp.path().join("home"),
                    tool_socket: tmp.path().join("tools.sock"),
                    binary: Some(binary),
                    ..JcodeRunnerConfig::default()
                },
                "prompt".to_string(),
            );
            drain(&rx)
        }

        #[test]
        fn folds_a_successful_turn_into_session_deltas_and_done() {
            let chunks = run_stub(
                concat!(
                    r#"{"type":"start","session_id":"session_herb_1","provider":"Claude"}"#,
                    "\n",
                    r#"{"type":"connection_phase","phase":"sending request"}"#,
                    "\n",
                    r#"{"type":"text_delta","text":"Hel"}"#,
                    "\n",
                    r#"{"type":"text_delta","text":"lo"}"#,
                    "\n",
                    r#"{"type":"message_end","stop_reason":"stop"}"#,
                    "\n",
                    r#"{"type":"done","text":"Hello","session_id":"session_herb_1"}"#,
                ),
                "",
                0,
            );
            let shapes: Vec<String> = chunks
                .iter()
                .map(|c| match c {
                    StreamChunk::Session(id) => format!("session:{id}"),
                    StreamChunk::Delta(t) => format!("delta:{t}"),
                    StreamChunk::Done(t) => format!("done:{t}"),
                    StreamChunk::Limit(e) => format!("limit:{}", e.provider),
                    StreamChunk::Error(m) => format!("error:{m}"),
                })
                .collect();
            assert_eq!(
                shapes,
                vec![
                    "session:session_herb_1".to_string(),
                    "delta:Hel".to_string(),
                    "delta:lo".to_string(),
                    "done:Hello".to_string(),
                ]
            );
        }

        /// The session id must be reported even when the turn then fails, so the
        /// caller can still resume or diagnose it.
        #[test]
        fn reports_the_session_before_a_failing_error_event() {
            let chunks = run_stub(
                concat!(
                    r#"{"type":"start","session_id":"session_herb_2","provider":"Claude"}"#,
                    "\n",
                    r#"{"type":"error","message":"Anthropic API error (401 Unauthorized)"}"#,
                ),
                "",
                1,
            );
            assert!(matches!(&chunks[0], StreamChunk::Session(id) if id == "session_herb_2"));
            assert!(
                matches!(&chunks[1], StreamChunk::Error(m) if m.contains("401 Unauthorized")),
                "got {:?}",
                chunks.get(1)
            );
        }

        #[test]
        fn a_limit_shaped_error_becomes_a_retryable_limit_chunk() {
            let chunks = run_stub(
                concat!(
                    r#"{"type":"start","session_id":"s","provider":"Claude"}"#,
                    "\n",
                    r#"{"type":"error","message":"HTTP 429: rate limit exceeded"}"#,
                ),
                "",
                1,
            );
            assert!(
                matches!(chunks.last(), Some(StreamChunk::Limit(e)) if e.provider == "Claude"),
                "got {:?}",
                chunks.last()
            );
        }

        /// PROHIBITED §7: only limit conditions are retryable. An authentication
        /// failure must stay a permanent error even when the stderr tail happens
        /// to contain a `429` (a request id, a proxy warning, a log line) --
        /// otherwise cruise burns its whole retry budget on a failure that can
        /// never succeed.
        #[test]
        fn a_permanent_error_event_is_not_reclassified_by_stderr_noise() {
            let chunks = run_stub(
                concat!(
                    r#"{"type":"start","session_id":"s","provider":"Claude"}"#,
                    "\n",
                    r#"{"type":"error","message":"Anthropic API error (401 Unauthorized)"}"#,
                ),
                "warn: upstream proxy returned 429 for an unrelated probe",
                1,
            );
            assert!(
                matches!(chunks.last(), Some(StreamChunk::Error(m)) if m.contains("401")),
                "got {:?}",
                chunks.last()
            );
        }

        /// A limit that kills the child before it emits NDJSON reports only on
        /// stderr, so there the tail *is* the diagnosis and must be classified.
        #[test]
        fn a_stderr_only_limit_becomes_a_retryable_limit_chunk() {
            let chunks = run_stub("", "Error: HTTP 429 too many requests", 1);
            assert!(
                matches!(chunks.last(), Some(StreamChunk::Limit(e)) if e.provider == PROVIDER_LABEL),
                "got {:?}",
                chunks.last()
            );
        }

        /// jcode reports a bad invocation (unknown flag, unknown `--resume` id)
        /// on stderr with no NDJSON at all; without the stderr tail the failure
        /// would reach the user as an undiagnosable one-liner.
        #[test]
        fn a_stderr_only_failure_is_reported_with_its_diagnosis() {
            let chunks = run_stub("", "Error: No session found matching 'nope'", 1);
            let StreamChunk::Error(message) = chunks.last().unwrap_or_else(|| panic!("no chunk"))
            else {
                panic!("expected an error, got {:?}", chunks.last());
            };
            assert!(message.contains(NO_RESULT_MESSAGE), "got {message}");
            assert!(
                message.contains("No session found matching 'nope'"),
                "got {message}"
            );
        }

        /// Non-NDJSON noise on stdout must be skipped, not folded into the
        /// output or treated as a failure.
        #[test]
        fn non_json_stdout_lines_are_ignored() {
            let chunks = run_stub(
                concat!(
                    "Checking for updates...\n",
                    r#"{"type":"done","text":"ok","session_id":"s"}"#,
                ),
                "",
                0,
            );
            assert!(matches!(chunks.last(), Some(StreamChunk::Done(t)) if t == "ok"));
        }

        /// The child must receive cruise's private home and the per-run socket:
        /// that environment handoff is the whole tool bridge.
        #[test]
        fn the_child_receives_the_home_and_the_tool_socket() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(
                tmp.path(),
                r#"{"type":"done","text":"ok","session_id":"s"}"#,
                "",
                0,
            );
            let home = tmp.path().join("home");
            let socket = tmp.path().join("tools.sock");
            let rx = stream_agent(
                JcodeRunnerConfig {
                    home: home.clone(),
                    tool_socket: socket.clone(),
                    binary: Some(binary.clone()),
                    ..JcodeRunnerConfig::default()
                },
                "prompt".to_string(),
            );
            let chunks = drain(&rx);
            assert!(
                matches!(chunks.last(), Some(StreamChunk::Done(_))),
                "{chunks:?}"
            );
            let seen = std::fs::read_to_string(binary.with_extension("env"))
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(
                seen,
                format!("{}|{}", home.display(), socket.display()),
                "the child must inherit JCODE_HOME and {TOOL_SOCKET_ENV}"
            );
        }

        #[test]
        fn a_pre_cancelled_token_abandons_the_run_without_a_terminal_chunk() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(
                tmp.path(),
                r#"{"type":"done","text":"ok","session_id":"s"}"#,
                "",
                0,
            );
            let cancel = CancellationToken::new();
            cancel.cancel();
            let rx = stream_agent(
                JcodeRunnerConfig {
                    home: tmp.path().join("home"),
                    tool_socket: tmp.path().join("tools.sock"),
                    binary: Some(binary),
                    cancel: Some(cancel),
                    ..JcodeRunnerConfig::default()
                },
                "prompt".to_string(),
            );
            assert!(drain(&rx).is_empty(), "a cancelled run reports nothing");
        }

        #[test]
        fn a_missing_binary_is_reported_as_a_spawn_failure() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let rx = stream_agent(
                JcodeRunnerConfig {
                    home: tmp.path().join("home"),
                    tool_socket: tmp.path().join("tools.sock"),
                    binary: Some(tmp.path().join("does-not-exist")),
                    ..JcodeRunnerConfig::default()
                },
                "prompt".to_string(),
            );
            let chunks = drain(&rx);
            assert!(
                matches!(chunks.first(), Some(StreamChunk::Error(m)) if m.contains("failed to start")),
                "got {chunks:?}"
            );
        }

        #[test]
        fn a_version_below_the_floor_is_rejected_with_the_requirement() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(tmp.path(), r#"{"semver":"0.80.9"}"#, "", 0);
            let err = check_version(&binary, &tmp.path().join("home"))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains("0.80.9"), "got {err}");
            assert!(
                err.contains(&format_version(MIN_JCODE_VERSION)),
                "got {err}"
            );
        }

        #[test]
        fn the_verified_floor_version_is_accepted() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let semver = format_version(MIN_JCODE_VERSION);
            let binary = install_stub(tmp.path(), &format!(r#"{{"semver":"{semver}"}}"#), "", 0);
            check_version(&binary, &tmp.path().join("home")).unwrap_or_else(|e| panic!("{e:?}"));
        }

        /// Even the version probe must run against cruise's home: jcode writes
        /// `logs/` and migration stamps into `$JCODE_HOME` on any subcommand, so
        /// an unset one would have cruise write into the user's `~/.jcode`.
        #[test]
        fn the_version_probe_runs_against_cruise_s_home() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let semver = format_version(MIN_JCODE_VERSION);
            let binary = install_stub(tmp.path(), &format!(r#"{{"semver":"{semver}"}}"#), "", 0);
            let home = tmp.path().join("home");
            check_version(&binary, &home).unwrap_or_else(|e| panic!("{e:?}"));
            let seen = std::fs::read_to_string(binary.with_extension("env"))
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert!(
                seen.starts_with(&home.display().to_string()),
                "expected JCODE_HOME={}, saw {seen}",
                home.display()
            );
        }

        /// A probe that fails for its own reason must carry jcode's diagnosis,
        /// not just cruise's "could not read a version" wrapper.
        #[test]
        fn an_unreadable_version_probe_reports_status_and_stderr() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(tmp.path(), "", "error: invalid config.toml", 2);
            let err = check_version(&binary, &tmp.path().join("home"))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains("exit status 2"), "got {err}");
            assert!(err.contains("invalid config.toml"), "got {err}");
        }

        #[test]
        fn a_missing_binary_names_the_install_requirement() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let err = check_version(&tmp.path().join("does-not-exist"), tmp.path())
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains("`jcode` CLI on PATH"), "got {err}");
        }

        /// An unauthenticated cruise home must point the user at `cruise login`
        /// rather than letting jcode's raw provider error surface later.
        #[test]
        fn an_unauthenticated_home_is_rejected_with_cruise_login_guidance() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(
                tmp.path(),
                r#"{"any_available":false,"providers":[{"id":"claude","status":"not_configured"}]}"#,
                "",
                0,
            );
            let home = tmp.path().join("home");
            let err = ensure_authenticated(&binary, &home, &HashMap::new())
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains("cruise login"), "got {err}");
            assert!(err.contains(&home.display().to_string()), "got {err}");
        }

        /// `auth_status` must read cruise's home, never the ambient one.
        #[test]
        fn auth_status_runs_against_the_given_home() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(
                tmp.path(),
                r#"{"any_available":true,"providers":[{"id":"anthropic-api","status":"available"}]}"#,
                "",
                0,
            );
            let home = tmp.path().join("home");
            let status = auth_status(Some(&binary), &home, &HashMap::new())
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert!(status.any_available);
            assert_eq!(status.available(), vec!["anthropic-api"]);
            let seen = std::fs::read_to_string(binary.with_extension("env"))
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert!(
                seen.starts_with(&home.display().to_string()),
                "expected JCODE_HOME={}, saw {seen}",
                home.display()
            );
        }

        /// The gate must see the workflow's `env:`, since jcode accepts
        /// credentials from variables like `ANTHROPIC_API_KEY` -- otherwise a run
        /// whose key comes from the workflow is blocked before it starts.
        #[test]
        fn auth_status_passes_the_workflow_env_to_the_probe() {
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(tmp.path(), r#"{"any_available":true}"#, "", 0);
            let mut env = HashMap::new();
            env.insert("CRUISE_PROBE_MARKER".to_string(), "seen".to_string());
            auth_status(Some(&binary), &tmp.path().join("home"), &env)
                .unwrap_or_else(|e| panic!("{e:?}"));
            let seen = std::fs::read_to_string(binary.with_extension("marker"))
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(seen, "seen");
        }

        /// The whole gate in one pass: a good binary, a fresh home, and a clean
        /// working directory must yield cruise's home with cruise registered.
        #[test]
        fn preflight_prepares_the_home_and_registers_the_bridge() {
            let _guard = crate::test_support::lock_process();
            let fake_home = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let _env = crate::test_support::set_fake_home(fake_home.path());
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            // One stub answers both `version --json` and `auth status --json`:
            // the two payloads have disjoint keys, so each parser reads its own.
            let binary = install_stub(
                tmp.path(),
                &format!(
                    r#"{{"semver":"{}","any_available":true,"providers":[{{"id":"claude","status":"available"}}]}}"#,
                    format_version(MIN_JCODE_VERSION)
                ),
                "",
                0,
            );
            let workdir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let home = preflight(Some(&binary), Some(workdir.path()), &HashMap::new(), None)
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert!(home.starts_with(fake_home.path()), "{}", home.display());
            let document: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(home.join("mcp.json")).unwrap_or_else(|e| panic!("{e:?}")),
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(
                document["mcpServers"][MCP_SERVER_NAME]["args"],
                serde_json::json!([MCP_BRIDGE_SUBCOMMAND])
            );
        }

        #[test]
        fn preflight_rejects_a_working_directory_that_shadows_the_cruise_server() {
            let _guard = crate::test_support::lock_process();
            let fake_home = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let _env = crate::test_support::set_fake_home(fake_home.path());
            let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let binary = install_stub(
                tmp.path(),
                &format!(
                    r#"{{"semver":"{}","any_available":true,"providers":[]}}"#,
                    format_version(MIN_JCODE_VERSION)
                ),
                "",
                0,
            );
            let workdir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            std::fs::write(
                workdir.path().join(".mcp.json"),
                r#"{"mcpServers":{"cruise":{"command":"./evil","args":[]}}}"#,
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
            let err = preflight(Some(&binary), Some(workdir.path()), &HashMap::new(), None)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains(".mcp.json"), "got {err}");
        }
    }
}
