//! `sdk: claude` backend: drive the `claude` CLI through `claude-agent-sdk`.
//!
//! [`stream_agent`] runs one prompt on a dedicated thread with its own
//! current-thread tokio runtime and reports progress as
//! [`StreamChunk`]s, so `executor.rs` folds this backend's output exactly like
//! any other. Cruise's [`CruiseTool`]s are registered on a single in-process
//! MCP toolbox named [`CRUISE_TOOLBOX_NAME`], which the CLI exposes to the
//! model as `mcp__cruise__<tool>`.

use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use claude_agent_sdk::internal::client::user_message_frame;
use claude_agent_sdk::tool::{AgentTool, AgentToolbox};
use claude_agent_sdk::transport::{SubprocessCliTransport, Transport as _};
use claude_agent_sdk::{ClaudeAgentOptions, ContentBlock, Message, PermissionMode};
use futures::StreamExt as _;

use crate::backend::effort::EffortLevel;
use crate::backend::stream::{LimitError, StreamChunk};
use crate::backend::tool::CruiseTool;
use crate::cancellation::CancellationToken;

/// MCP server name for cruise's in-process toolbox. The CLI exposes its tools
/// to the model as `mcp__cruise__<tool>`.
const CRUISE_TOOLBOX_NAME: &str = "cruise";

/// Provider label attached to a [`LimitError`]. The `sdk: claude` backend
/// always runs against the `claude` CLI, so there is nothing finer to report.
const PROVIDER_LABEL: &str = "claude";

/// Number of trailing stderr lines retained for rate-limit classification and
/// error reporting. 64 lines is more than enough for a CLI error tail; capping
/// prevents unbounded growth if the CLI starts logging copiously to stderr.
const STDERR_TAIL_LINES: usize = 64;

/// Reported when the CLI's stdout ends without a `result` frame, i.e. the
/// process died before finishing the turn (bad flag, failed auth, killed).
/// The CLI's own diagnosis is on stderr, which is appended by
/// [`send_error_with_stderr`].
const NO_RESULT_MESSAGE: &str = "the claude CLI exited without reporting a result";

/// Cap on how long [`await_stderr_drain`] waits for the stderr drain task to
/// consume residual lines after the transport closes. The child has already
/// exited at that point so 2s is plenty; the timeout exists only to prevent a
/// runaway task from blocking the runtime.
const STDERR_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const PRIVATE_MCP_CONFIG_FILE_PREFIX: &str = "cruise-claude-mcp-config-";
static PRIVATE_MCP_CONFIG_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One `sdk: claude` prompt run.
///
/// `model` is a plain `claude --model` name (the `:effort` suffix has already
/// been split off into `effort`), and `resume_session_id` continues a prior
/// session so planning's plan/fix/ask turns share context.
///
/// Deliberately not `Debug`: `env` can carry provider credentials, and no
/// caller needs to format the config.
#[derive(Default)]
pub(crate) struct ClaudeRunnerConfig {
    pub(crate) permission: crate::config::PermissionMode,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<EffortLevel>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) resume_session_id: Option<String>,
    /// Custom in-process tools the model can call, registered as one SDK MCP
    /// toolbox (server name [`CRUISE_TOOLBOX_NAME`]).
    pub(crate) tools: Vec<CruiseTool>,
    /// Workflow-configured MCP servers.
    pub(crate) mcp_servers: crate::config::McpServers,
    /// Environment variables for the spawned `claude` process.
    pub(crate) env: HashMap<String, String>,
    /// Cancellation signal for the run. Firing it stops reading the CLI's
    /// output and closes the transport, terminating the child; without a token
    /// the run can only end on its own.
    pub(crate) cancel: Option<CancellationToken>,
    /// Binary to invoke instead of resolving `claude` on `$PATH`. Left `None`
    /// in production; tests point it at a stub CLI.
    pub(crate) cli_path: Option<PathBuf>,
}

/// Run `prompt` through the `claude` CLI and surface output as
/// [`StreamChunk`]s on a dedicated thread.
///
/// Each `text` content block of an assistant message becomes a
/// [`StreamChunk::Delta`]; the session id becomes [`StreamChunk::Session`] as
/// soon as the CLI reports one (normally its opening `system: init` frame).
/// Rate/usage limits become [`StreamChunk::Limit`] and everything else terminal
/// becomes [`StreamChunk::Error`]. Other block kinds (`tool_use`,
/// `tool_result`, `thinking`) are consumed but not forwarded -- cruise only
/// renders assistant text.
///
/// The run ends early, with no terminal chunk, when
/// [`ClaudeRunnerConfig::cancel`] fires or the returned receiver is dropped;
/// either way the transport is closed, which terminates the `claude` child.
pub(crate) fn stream_agent(config: ClaudeRunnerConfig, prompt: String) -> Receiver<StreamChunk> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || run_in_runtime(config, prompt, tx));
    rx
}

fn run_in_runtime(config: ClaudeRunnerConfig, prompt: String, tx: Sender<StreamChunk>) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        let _ = tx.send(StreamChunk::Error("failed to build tokio runtime".into()));
        return;
    };
    rt.block_on(async move { run_async(config, prompt, &tx).await });
}

/// Terminal outcome of one CLI run. Delta/Session chunks are streamed out
/// inline while the message stream is being read; the terminal verdict is
/// delayed until after stderr has been fully drained so it can consult the
/// trailing lines.
enum Terminal {
    /// A `Result` frame arrived with `is_error: true`. Carries the
    /// CLI-reported error text, falling back to the frame's subtype.
    ResultError(String),
    /// The message stream surfaced a transport/parse error.
    StreamError(String),
    /// A `Result` frame arrived with `is_error: false`: the turn completed.
    Completed,
    /// Stdout ended without any `Result` frame. The CLI never finished the
    /// turn, so this is a failure however clean the stream looked -- the
    /// process exits this way on an unknown flag, a rejected
    /// `--permission-mode`, failed auth, or being killed.
    NoResult,
}

/// Resolves when `token` is cancelled, or waits forever if there is no token.
async fn until_cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(t) => t.cancelled().await,
        None => std::future::pending().await,
    }
}

/// Owns a private MCP config file until the CLI transport has shut down.
struct PrivateMcpConfigFile {
    path: PathBuf,
}

impl PrivateMcpConfigFile {
    fn create(servers: &crate::config::McpServers) -> std::io::Result<Option<Self>> {
        if servers.is_empty() {
            return Ok(None);
        }
        let mcp_servers: serde_json::Map<String, serde_json::Value> = servers
            .iter()
            .map(|(name, server)| (name.clone(), server.to_backend_json()))
            .collect();
        let contents = serde_json::to_vec(&serde_json::json!({
            "mcpServers": mcp_servers
        }))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let directory = std::env::temp_dir();
        for _ in 0..32 {
            let sequence = PRIVATE_MCP_CONFIG_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                "{PRIVATE_MCP_CONFIG_FILE_PREFIX}{}-{sequence}.json",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);

            match options.open(&path) {
                Ok(mut file) => {
                    let guard = Self { path };
                    let result = file.write_all(&contents);
                    drop(file);
                    if let Err(error) = result {
                        drop(guard);
                        return Err(error);
                    }
                    return Ok(Some(guard));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary MCP configuration file",
        ))
    }
}

impl Drop for PrivateMcpConfigFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Create workflow MCP options and connect while retaining the private file
/// guard for the caller's transport lifetime.
async fn connect_transport(
    config: &ClaudeRunnerConfig,
) -> Result<(Option<PrivateMcpConfigFile>, SubprocessCliTransport), String> {
    let mcp_config_file = PrivateMcpConfigFile::create(&config.mcp_servers)
        .map_err(|error| format!("failed to create private MCP config file: {error}"))?;
    let mut options = build_options(config);
    if let Some(file) = &mcp_config_file {
        options.extra_args.insert(
            "--mcp-config".into(),
            Some(file.path.to_string_lossy().into_owned()),
        );
    }
    let mut transport = SubprocessCliTransport::streaming(options);
    if let Err(error) = transport.connect().await {
        let _ = transport.close().await;
        return Err(error.to_string());
    }
    Ok((mcp_config_file, transport))
}

/// Drain stderr into a bounded tail before any writes can fail or the child
/// can close its output pipes.
fn start_stderr_drain(
    transport: &mut SubprocessCliTransport,
    stderr_tail: &Arc<Mutex<VecDeque<String>>>,
) -> Option<tokio::task::JoinHandle<()>> {
    transport.take_stderr_rx().map(|mut rx_stderr| {
        let buf = Arc::clone(stderr_tail);
        tokio::spawn(async move {
            while let Some(line) = rx_stderr.recv().await {
                push_stderr_line(&buf, line);
            }
        })
    })
}

async fn run_async(config: ClaudeRunnerConfig, prompt: String, tx: &Sender<StreamChunk>) {
    let cancel = config.cancel.clone();
    // Streaming mode rather than `one_shot` / `--print`: with an SDK MCP
    // toolbox registered the CLI opens an MCP `initialize` handshake over
    // stdout/stdin, and `--print` has no stdin open to answer it on, so the
    // server is marked `failed` and cruise's tools never reach the model. One
    // path for tools and no-tools runs keeps the frame format and `end_input`
    // timing under the same tests either way.
    let (_mcp_config_file, mut transport) = match connect_transport(&config).await {
        Ok(connected) => connected,
        Err(error) => {
            send_error_with_stderr(tx, &error, &[]);
            return;
        }
    };

    let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let drain_handle = start_stderr_drain(&mut transport, &stderr_tail);

    if let Err(e) = transport
        .write(&user_message_frame(&prompt, "default"))
        .await
    {
        let _ = transport.close().await;
        await_stderr_drain(drain_handle).await;
        let tail = snapshot_stderr_tail(&stderr_tail);
        send_error_with_stderr(tx, &e.to_string(), &tail);
        return;
    }
    if let Err(e) = transport.end_input().await {
        let _ = transport.close().await;
        await_stderr_drain(drain_handle).await;
        let tail = snapshot_stderr_tail(&stderr_tail);
        send_error_with_stderr(tx, &e.to_string(), &tail);
        return;
    }

    let mut stream = transport
        .take_message_stream()
        .map(|item| item.and_then(Message::from_frame));

    let mut session_reported = false;
    // `None` means "abandon quietly": either the run was cancelled or the
    // receiver is gone, so there is nobody left to report a verdict to.
    let mut terminal = Some(Terminal::NoResult);
    loop {
        let next = tokio::select! {
            biased;
            () = until_cancelled(cancel.as_ref()) => {
                terminal = None;
                break;
            }
            item = stream.next() => item,
        };
        let Some(item) = next else { break };
        match item {
            Ok(Message::System(s)) => {
                // The CLI's opening frames (`init` and the session hooks)
                // carry the session id, so it is normally known before any
                // assistant text -- and still known if the run then fails.
                let id = session_id_of(&s.data);
                if !report_session(tx, &mut session_reported, id) {
                    terminal = None;
                    break;
                }
            }
            Ok(Message::Assistant(a)) => {
                if !report_session(tx, &mut session_reported, a.session_id)
                    || !forward_text(tx, a.content)
                {
                    terminal = None;
                    break;
                }
            }
            Ok(Message::Result(r)) => {
                if report_session(tx, &mut session_reported, r.session_id) {
                    terminal = Some(if r.is_error {
                        Terminal::ResultError(r.result.unwrap_or_else(|| r.subtype.clone()))
                    } else {
                        Terminal::Completed
                    });
                } else {
                    terminal = None;
                }
                break;
            }
            Ok(_) => {}
            Err(e) => {
                terminal = Some(Terminal::StreamError(e.to_string()));
                break;
            }
        }
    }
    // Drop the message stream to release its receiver, then close the transport:
    // it shuts stdin down and waits for the child, escalating to a kill if the
    // child outlives that wait (the cancelled case). Only once the child is
    // gone is stderr guaranteed to be at EOF, so the drain task is awaited
    // after that; snapshotting earlier would miss a rate-limit line that
    // arrived in the same scheduling tick as the stdout error.
    drop(stream);
    let _ = transport.close().await;
    let Some(terminal) = terminal else { return };
    await_stderr_drain(drain_handle).await;
    let tail = snapshot_stderr_tail(&stderr_tail);

    match terminal {
        Terminal::ResultError(msg) | Terminal::StreamError(msg) => {
            send_error_with_stderr(tx, &msg, &tail);
        }
        Terminal::NoResult => send_error_with_stderr(tx, NO_RESULT_MESSAGE, &tail),
        // A completed turn is never re-classified from stderr: the CLI logs
        // there while retrying internally, and demoting a finished turn to a
        // limit would discard its output and re-run the whole step.
        // Empty text: the reducer uses the accumulated deltas.
        Terminal::Completed => {
            let _ = tx.send(StreamChunk::Done(String::new()));
        }
    }
}

/// The `session_id` field of a raw CLI frame, if it names one.
fn session_id_of(frame: &serde_json::Value) -> Option<String> {
    frame
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Report the run's session id the first time the CLI names one, tracking that
/// in `reported` so later frames don't repeat it. Returns `false` when the
/// receiver is gone and the run should be abandoned.
fn report_session(tx: &Sender<StreamChunk>, reported: &mut bool, id: Option<String>) -> bool {
    match id {
        Some(id) if !*reported => {
            *reported = true;
            tx.send(StreamChunk::Session(id)).is_ok()
        }
        _ => true,
    }
}

/// Forward an assistant message's text blocks as deltas, skipping the block
/// kinds cruise doesn't render. Returns `false` when the receiver is gone and
/// the run should be abandoned.
fn forward_text(tx: &Sender<StreamChunk>, content: Vec<ContentBlock>) -> bool {
    content.into_iter().all(|block| match block {
        ContentBlock::Text(t) => tx.send(StreamChunk::Delta(t.text)).is_ok(),
        _ => true,
    })
}

/// Wait for the stderr drain task to finish processing residual lines.
///
/// The task ends when its upstream sender (held by the SDK's stderr reader) is
/// dropped, which happens once the child's stderr pipe EOFs. The wait is capped
/// at [`STDERR_DRAIN_TIMEOUT`] so a runaway task cannot block the runtime.
async fn await_stderr_drain(handle: Option<tokio::task::JoinHandle<()>>) {
    let Some(h) = handle else { return };
    let _ = tokio::time::timeout(STDERR_DRAIN_TIMEOUT, h).await;
}

fn push_stderr_line(buf: &Arc<Mutex<VecDeque<String>>>, line: String) {
    // `PoisonError` carries the inner guard; recover from it instead of losing
    // the whole stderr tail because some other task panicked.
    let mut guard = match buf.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.len() >= STDERR_TAIL_LINES {
        guard.pop_front();
    }
    guard.push_back(line);
}

fn snapshot_stderr_tail(buf: &Arc<Mutex<VecDeque<String>>>) -> Vec<String> {
    match buf.lock() {
        Ok(g) => g.iter().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().iter().cloned().collect(),
    }
}

/// Report `msg` together with the collected stderr tail.
///
/// The SDK error message alone is often insufficient: the CLI's real diagnosis
/// (rate limit, unknown flag, MCP server failure) goes to stderr, while stdout
/// carries at most a truncated frame that surfaces as a short JSON decode
/// error. So the tail is both fed to [`crate::retry::is_limit_message`] to
/// decide Limit vs. Error, and reported with the text -- otherwise the failure
/// reaches the user as an undiagnosable one-liner, unlike the command backend
/// which returns the CLI's stderr. A limit carries it as
/// [`LimitError::detail`], which is where the retry policy reads a server
/// `Retry-After` hint from ([`crate::retry::parse_retry_after`]).
fn send_error_with_stderr(tx: &Sender<StreamChunk>, msg: &str, stderr_tail: &[String]) {
    let text = if stderr_tail.is_empty() {
        std::borrow::Cow::Borrowed(msg)
    } else {
        std::borrow::Cow::Owned(format!("{msg}\nclaude stderr:\n{}", stderr_tail.join("\n")))
    };
    if crate::retry::is_limit_message(&text) {
        let _ = tx.send(StreamChunk::Limit(LimitError {
            provider: PROVIDER_LABEL.to_string(),
            detail: text.into_owned(),
        }));
    } else {
        let _ = tx.send(StreamChunk::Error(text.into_owned()));
    }
}

fn build_options(config: &ClaudeRunnerConfig) -> ClaudeAgentOptions {
    let mut opts = ClaudeAgentOptions::new();
    opts.model.clone_from(&config.model);
    opts.effort = config.effort.map(|e| e.as_str().to_string());
    opts.cwd.clone_from(&config.cwd);
    opts.cli_path.clone_from(&config.cli_path);
    opts.resume.clone_from(&config.resume_session_id);
    // Cruise runs prompts unattended: the workflow decides what a step may
    // touch, and there is no console to answer a permission prompt on, so a
    // prompt would deadlock the step until its `timeout:` fires.
    match config.permission {
        crate::config::PermissionMode::Full => {
            opts.permission_mode = Some(PermissionMode::BypassPermissions);
        }
        restricted => {
            // `DontAsk` denies anything not explicitly allowed instead of
            // prompting, so restricted steps never wait on a console.
            opts.permission_mode = Some(PermissionMode::DontAsk);
            opts.disallowed_tools.push("Bash".to_string());
            if restricted == crate::config::PermissionMode::ReadOnly {
                opts.disallowed_tools
                    .extend(["Edit", "Write", "NotebookEdit"].map(str::to_string));
            } else {
                opts.allowed_tools
                    .extend(["Edit", "Write", "NotebookEdit"].map(str::to_string));
            }
            opts.allowed_tools.extend(
                config
                    .tools
                    .iter()
                    .map(|tool| format!("mcp__{CRUISE_TOOLBOX_NAME}__{}", tool.name)),
            );
        }
    }
    // Workflow MCP servers are passed to the CLI by `run_async` through a
    // private config file, never through the SDK's inline `--mcp-config` JSON.
    if !config.tools.is_empty() {
        let tools: Vec<AgentTool> = config.tools.iter().map(cruise_tool_to_agent_tool).collect();
        opts.sdk_mcp_server = Some(AgentToolbox::new(CRUISE_TOOLBOX_NAME).with_tools(tools));
    }
    opts.env.clone_from(&config.env);
    opts
}

/// Adapt a [`CruiseTool`] to an [`AgentTool`].
///
/// Both handlers are the same type alias -- `Arc<dyn Fn(Value) ->
/// Result<String, String> + Send + Sync>` -- so the handler is cloned straight
/// through with no wrapping closure, and the interior state cruise's tools
/// capture (the plan-persist flag, the title / PR-metadata stores) stays shared.
fn cruise_tool_to_agent_tool(tool: &CruiseTool) -> AgentTool {
    AgentTool::new(
        tool.name.clone(),
        tool.description.clone(),
        tool.parameters.clone(),
        Arc::clone(&tool.handler),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn tool(name: &str) -> CruiseTool {
        CruiseTool::new(
            name,
            "desc",
            serde_json::json!({"type": "object", "properties": {}}),
            Arc::new(|_| Ok("ok".to_string())),
        )
    }

    // -- build_options --------------------------------------------------------

    #[test]
    fn build_options_defaults_to_bypass_permissions() {
        let opts = build_options(&ClaudeRunnerConfig::default());
        assert!(matches!(
            opts.permission_mode,
            Some(PermissionMode::BypassPermissions)
        ));
    }

    #[test]
    fn build_options_carries_model_effort_cwd_and_resume() {
        let opts = build_options(&ClaudeRunnerConfig {
            model: Some("claude-sonnet-4-6".into()),
            effort: Some(EffortLevel::XHigh),
            cwd: Some(PathBuf::from("/tmp")),
            resume_session_id: Some("sess-1".into()),
            ..Default::default()
        });
        assert_eq!(opts.model.as_deref(), Some("claude-sonnet-4-6"));
        // `claude --effort` accepts exactly low/medium/high/xhigh/max, which is
        // what `EffortLevel::as_str` spells.
        assert_eq!(opts.effort.as_deref(), Some("xhigh"));
        assert_eq!(opts.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        assert_eq!(opts.resume.as_deref(), Some("sess-1"));
    }

    #[test]
    fn build_options_leaves_effort_unset_without_a_tier() {
        let opts = build_options(&ClaudeRunnerConfig::default());
        assert_eq!(opts.effort, None);
    }

    #[test]
    fn build_options_registers_tools_on_the_cruise_toolbox() {
        let opts = build_options(&ClaudeRunnerConfig {
            tools: vec![tool("ask_user"), tool("submit_plan")],
            ..Default::default()
        });
        let Some(toolbox) = opts.sdk_mcp_server else {
            panic!("toolbox registered");
        };
        // The CLI derives the model-visible names (`mcp__cruise__ask_user`)
        // from this server name, which prompt templates depend on.
        assert_eq!(toolbox.name, "cruise");
        let names: Vec<&str> = toolbox.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["ask_user", "submit_plan"]);
    }

    #[test]
    fn build_options_keeps_workflow_servers_in_a_private_config_file() {
        const SECRET: &str = "credential-sentinel-never-in-argv";
        let mut mcp_servers = crate::config::McpServers::new();
        mcp_servers.insert(
            "remote_tool".to_string(),
            crate::config::McpServerConfig {
                transport: Some(crate::config::McpTransport::Http),
                url: Some("https://example.test/mcp".to_string()),
                headers: indexmap::IndexMap::from([(
                    "Authorization".to_string(),
                    SECRET.to_string(),
                )]),
                ..Default::default()
            },
        );
        let config = ClaudeRunnerConfig {
            tools: vec![tool("submit_plan")],
            mcp_servers,
            ..Default::default()
        };
        let private_dir = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("create private MCP config directory: {error}"));
        let private_path = private_dir.path().join("workflow-mcp-config.json");
        let mut opts = build_options(&config);
        opts.extra_args.insert(
            "--mcp-config".into(),
            Some(private_path.to_string_lossy().into_owned()),
        );

        let args = SubprocessCliTransport::streaming(opts).build_args();
        let mcp_config_args: Vec<_> = args
            .windows(2)
            .filter(|pair| pair[0] == "--mcp-config")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(mcp_config_args.len(), 2);
        assert!(
            !args.join("\n").contains(SECRET),
            "credential must not appear in CLI arguments: {args:?}"
        );
        let private_path_arg = private_path.to_string_lossy();
        assert!(mcp_config_args.contains(&private_path_arg.as_ref()));
        let inline_config_arg = mcp_config_args
            .iter()
            .copied()
            .find(|value| *value != private_path_arg.as_ref())
            .unwrap_or_else(|| panic!("SDK inline MCP config was not passed"));
        let inline_config: serde_json::Value = serde_json::from_str(inline_config_arg)
            .unwrap_or_else(|error| panic!("invalid inline --mcp-config JSON: {error}"));
        assert_eq!(inline_config["mcpServers"]["cruise"]["type"], "sdk");
        assert_eq!(
            inline_config["mcpServers"]["remote_tool"],
            serde_json::Value::Null
        );
        assert!(!args.iter().any(|arg| arg == "--strict-mcp-config"));
    }

    #[test]
    fn build_options_omits_toolbox_without_tools() {
        let opts = build_options(&ClaudeRunnerConfig::default());
        assert!(opts.sdk_mcp_server.is_none());
    }

    #[test]
    fn agent_tool_shares_the_cruise_handler() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let cruise_tool = CruiseTool::new(
            "count",
            "desc",
            serde_json::json!({"type": "object", "properties": {}}),
            Arc::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok("counted".to_string())
            }),
        );
        let agent_tool = cruise_tool_to_agent_tool(&cruise_tool);
        assert_eq!(
            (agent_tool.handler)(serde_json::Value::Null),
            Ok("counted".to_string())
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(agent_tool.input_schema, cruise_tool.parameters);
    }

    // -- rate-limit classification -------------------------------------------

    #[test]
    fn rate_limit_phrases_are_detected_case_insensitively() {
        // The predicate lives in `crate::retry`, shared with `sdk: jcode`, so
        // the same provider text is classified identically on both backends.
        for msg in [
            "API Error: 429 Rate Limit exceeded",
            "usage limit reached",
            "Too Many Requests",
            "5-hour session limit reached",
            // Retried by the command backend's `is_rate_limited`, so
            // `sdk: claude` must retry it too.
            r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error"}}"#,
            // Transient capacity refusal: retryable on both SDK backends.
            "Provider returned: overloaded_error",
        ] {
            assert!(crate::retry::is_limit_message(msg), "expected limit: {msg}");
        }
    }

    #[test]
    fn permanent_failures_are_not_rate_limits() {
        for msg in [
            "invalid_request_error: model not found",
            "authentication_error: invalid API key",
            "prompt is too long: 250000 tokens > 200000 maximum",
            "error: unknown option '--effort'",
        ] {
            assert!(
                !crate::retry::is_limit_message(msg),
                "expected error: {msg}"
            );
        }
    }

    fn drain(rx: &Receiver<StreamChunk>) -> StreamChunk {
        rx.recv()
            .unwrap_or_else(|_| StreamChunk::Error("no chunk sent".into()))
    }

    #[test]
    fn stderr_phrase_promotes_a_decode_error_to_a_limit() {
        // The classic rate-limit-mid-frame case: the SDK surfaces a truncated
        // JSON decode error whose snippet has no limit phrase, while stderr
        // carries the real reason.
        let (tx, rx) = std::sync::mpsc::channel();
        let msg = r#"failed to decode JSON: {"type":"assistant","message":{"model":"claude-sonnet-4-6","id":"msg_x"#;
        send_error_with_stderr(
            &tx,
            msg,
            &["API Error: 429 rate limit exceeded".to_string()],
        );
        match drain(&rx) {
            StreamChunk::Limit(e) => assert_eq!(e.provider, "claude"),
            other => panic!("expected Limit, got {other:?}"),
        }
    }

    #[test]
    fn unrelated_stderr_is_reported_with_the_error_rather_than_dropped() {
        let (tx, rx) = std::sync::mpsc::channel();
        send_error_with_stderr(&tx, "boom", &["MCP server failed".into()]);
        match drain(&rx) {
            StreamChunk::Error(m) => {
                assert!(m.starts_with("boom"), "message kept first: {m}");
                assert!(m.contains("MCP server failed"), "stderr reported: {m}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn message_alone_can_classify_a_limit() {
        let (tx, rx) = std::sync::mpsc::channel();
        send_error_with_stderr(&tx, "Too Many Requests", &[]);
        assert!(matches!(drain(&rx), StreamChunk::Limit(_)));
    }

    #[test]
    fn a_limit_phrase_on_any_stderr_line_is_detected() {
        let (tx, rx) = std::sync::mpsc::channel();
        send_error_with_stderr(
            &tx,
            "boom",
            &[
                "INFO: starting request".to_string(),
                "Error: usage limit reached".to_string(),
            ],
        );
        assert!(matches!(drain(&rx), StreamChunk::Limit(_)));
    }

    // -- stderr ring buffer ---------------------------------------------------

    #[test]
    fn stderr_ring_buffer_keeps_the_newest_lines() {
        let buf: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        for i in 0..(STDERR_TAIL_LINES + 5) {
            push_stderr_line(&buf, format!("line {i}"));
        }
        let tail = snapshot_stderr_tail(&buf);
        assert_eq!(tail.len(), STDERR_TAIL_LINES);
        assert_eq!(tail[0], "line 5");
        assert_eq!(
            tail[STDERR_TAIL_LINES - 1],
            format!("line {}", STDERR_TAIL_LINES + 4)
        );
    }

    // -- stream_agent against a stub CLI --------------------------------------
    //
    // `cli_path` points the SDK at a shell script that speaks the same
    // stream-json / control-request protocol as `claude`, so the whole
    // transport path -- streaming mode, the user frame, `end_input`, the MCP
    // handshake, stderr collection, `Terminal` classification -- runs without
    // needing the real CLI installed.

    #[cfg(unix)]
    mod stub_cli {
        use super::*;
        use std::time::{Duration, Instant};

        const STEP_TIMEOUT: Duration = Duration::from_secs(20);

        struct Stub {
            _dir: tempfile::TempDir,
            path: PathBuf,
            dir: PathBuf,
        }

        /// Write an executable `/bin/sh` stub CLI. Every stub starts by reading
        /// the user frame off stdin, which is what the real CLI does in
        /// streaming mode and what makes these tests deterministic: the write
        /// cannot lose a race against the child exiting.
        fn stub(body: &str) -> Stub {
            let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
            let path = dir.path().join("claude-stub");
            crate::test_support::write_executable_script(
                &path,
                &format!("#!/bin/sh\nread -r _user_frame\n{body}"),
            );
            Stub {
                path,
                dir: dir.path().to_path_buf(),
                _dir: dir,
            }
        }

        fn run(
            stub: &Stub,
            env: HashMap<String, String>,
            tools: Vec<CruiseTool>,
        ) -> Vec<StreamChunk> {
            let rx = stream_agent(
                ClaudeRunnerConfig {
                    cli_path: Some(stub.path.clone()),
                    env,
                    tools,
                    ..Default::default()
                },
                "hi".to_string(),
            );
            let mut chunks = Vec::new();
            while let Ok(chunk) = rx.recv_timeout(STEP_TIMEOUT) {
                chunks.push(chunk);
            }
            chunks
        }

        fn expect_error(chunks: &[StreamChunk]) -> &str {
            match chunks.last() {
                Some(StreamChunk::Error(m)) => m,
                other => panic!("expected a terminal Error, got {other:?} in {chunks:?}"),
            }
        }

        const MCP_SENTINEL: &str = "credential-sentinel-never-in-argv";

        fn mcp_servers_with_sentinel() -> crate::config::McpServers {
            let mut servers = crate::config::McpServers::new();
            servers.insert(
                "remote_tool".to_string(),
                crate::config::McpServerConfig {
                    transport: Some(crate::config::McpTransport::Http),
                    url: Some("https://example.test/mcp".to_string()),
                    headers: indexmap::IndexMap::from([(
                        "Authorization".to_string(),
                        MCP_SENTINEL.to_string(),
                    )]),
                    ..Default::default()
                },
            );
            servers
        }

        fn mcp_stub(body: &str) -> Stub {
            stub(&format!(
                concat!(
                    "mcp_config=''\n",
                    "expect_path=''\n",
                    "for arg in \"$@\"; do\n",
                    "  if [ \"$expect_path\" = yes ]; then mcp_config=\"$arg\"; expect_path=''; continue; fi\n",
                    "  if [ \"$arg\" = '--mcp-config' ]; then expect_path=yes; fi\n",
                    "done\n",
                    "printf '%s\\n' \"$@\" > \"$CRUISE_STUB_ARGS_FILE\"\n",
                    "printf '%s\\n' \"$mcp_config\" > \"$CRUISE_STUB_MCP_CONFIG_PATH_FILE\"\n",
                    "attempt=0\n",
                    "while [ ! -f \"$CRUISE_STUB_RELEASE_FILE\" ] && [ $attempt -lt 300 ]; do sleep 0.01; attempt=$((attempt + 1)); done\n",
                    "{body}"
                ),
                body = body
            ))
        }

        fn mcp_stub_env(stub: &Stub) -> HashMap<String, String> {
            HashMap::from([
                (
                    "CRUISE_STUB_ARGS_FILE".to_string(),
                    stub.dir.join("args.txt").display().to_string(),
                ),
                (
                    "CRUISE_STUB_MCP_CONFIG_PATH_FILE".to_string(),
                    stub.dir.join("mcp-config-path.txt").display().to_string(),
                ),
                (
                    "CRUISE_STUB_RELEASE_FILE".to_string(),
                    stub.dir.join("release").display().to_string(),
                ),
            ])
        }

        fn wait_for_nonempty_file(path: &std::path::Path) -> String {
            let deadline = Instant::now() + STEP_TIMEOUT;
            loop {
                if let Ok(text) = std::fs::read_to_string(path)
                    && !text.trim().is_empty()
                {
                    return text;
                }
                assert!(
                    Instant::now() < deadline,
                    "stub did not write {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn assert_private_mcp_config(stub: &Stub) -> PathBuf {
            let path_file = stub.dir.join("mcp-config-path.txt");
            let config_path = PathBuf::from(wait_for_nonempty_file(&path_file).trim());
            let args = std::fs::read_to_string(stub.dir.join("args.txt"))
                .unwrap_or_else(|e| panic!("read captured CLI arguments: {e}"));
            let args: Vec<_> = args.lines().collect();
            let mcp_config_args: Vec<_> = args
                .windows(2)
                .filter(|pair| pair[0] == "--mcp-config")
                .map(|pair| pair[1])
                .collect();
            assert_eq!(mcp_config_args.len(), 2);
            let config_path_arg = config_path.to_string_lossy();
            assert!(mcp_config_args.contains(&config_path_arg.as_ref()));
            let inline_config_arg = mcp_config_args
                .iter()
                .copied()
                .find(|value| *value != config_path_arg.as_ref())
                .unwrap_or_else(|| panic!("SDK inline MCP config was not passed: {args:?}"));
            assert!(
                !args.join("\n").contains(MCP_SENTINEL),
                "credential must not appear in CLI arguments: {args:?}"
            );

            let inline_config: serde_json::Value = serde_json::from_str(inline_config_arg)
                .unwrap_or_else(|e| panic!("parse inline MCP config: {e}"));
            assert_eq!(inline_config["mcpServers"]["cruise"]["type"], "sdk");
            assert_eq!(
                inline_config["mcpServers"]["remote_tool"],
                serde_json::Value::Null
            );

            let metadata = std::fs::metadata(&config_path)
                .unwrap_or_else(|e| panic!("read MCP config file metadata: {e}"));
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

            let contents = std::fs::read_to_string(&config_path)
                .unwrap_or_else(|e| panic!("read MCP config file: {e}"));
            let config: serde_json::Value = serde_json::from_str(&contents)
                .unwrap_or_else(|e| panic!("parse MCP config file: {e}"));
            assert_eq!(
                config["mcpServers"]["remote_tool"]["headers"]["Authorization"],
                MCP_SENTINEL
            );
            assert_eq!(config["mcpServers"]["remote_tool"]["type"], "http");
            assert_eq!(config["mcpServers"]["cruise"], serde_json::Value::Null);
            config_path
        }

        fn start_mcp_run(stub: &Stub, cancel: Option<CancellationToken>) -> Receiver<StreamChunk> {
            stream_agent(
                ClaudeRunnerConfig {
                    cli_path: Some(stub.path.clone()),
                    env: mcp_stub_env(stub),
                    tools: vec![tool("probe")],
                    mcp_servers: mcp_servers_with_sentinel(),
                    cancel,
                    ..Default::default()
                },
                "hi".to_string(),
            )
        }

        fn receive_all(rx: &Receiver<StreamChunk>) -> Vec<StreamChunk> {
            let mut chunks = Vec::new();
            while let Ok(chunk) = rx.recv_timeout(STEP_TIMEOUT) {
                chunks.push(chunk);
            }
            chunks
        }

        fn mcp_config_temp_files_for_current_process() -> Vec<PathBuf> {
            let prefix = format!("cruise-claude-mcp-config-{}-", std::process::id());
            std::fs::read_dir(std::env::temp_dir())
                .unwrap_or_else(|e| panic!("read temp directory: {e}"))
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix))
                })
                .collect()
        }

        #[test]
        fn private_mcp_config_hides_credentials_and_is_removed_for_all_run_outcomes() {
            for (body, expected_error) in [
                (
                    r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"sess-ok","result":"ok"}'"#,
                    false,
                ),
                (
                    r#"printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"sess-error","result":"expected failure"}'"#,
                    true,
                ),
            ] {
                let stub = mcp_stub(body);
                let rx = start_mcp_run(&stub, None);
                let config_path = assert_private_mcp_config(&stub);
                std::fs::write(stub.dir.join("release"), "continue")
                    .unwrap_or_else(|e| panic!("release stub CLI: {e}"));
                let chunks = receive_all(&rx);
                if expected_error {
                    assert!(matches!(chunks.last(), Some(StreamChunk::Error(_))));
                } else {
                    assert!(matches!(chunks.last(), Some(StreamChunk::Done(_))));
                }
                assert!(
                    !config_path.exists(),
                    "MCP config file must be removed after run"
                );
            }

            let stub = mcp_stub("sleep 60\n");
            let cancel = CancellationToken::new();
            let rx = start_mcp_run(&stub, Some(cancel.clone()));
            let config_path = assert_private_mcp_config(&stub);
            cancel.cancel();
            assert!(matches!(
                rx.recv_timeout(STEP_TIMEOUT),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            assert!(
                !config_path.exists(),
                "MCP config file must be removed on cancellation"
            );

            let before = mcp_config_temp_files_for_current_process();
            let failed_to_start = stream_agent(
                ClaudeRunnerConfig {
                    cli_path: Some(stub.dir.join("missing-claude")),
                    mcp_servers: mcp_servers_with_sentinel(),
                    ..Default::default()
                },
                "hi".to_string(),
            );
            let chunks = receive_all(&failed_to_start);
            assert!(matches!(chunks.last(), Some(StreamChunk::Error(_))));
            assert_eq!(
                mcp_config_temp_files_for_current_process(),
                before,
                "MCP config file must be removed when the process cannot start"
            );
        }

        #[test]
        fn stdout_without_a_result_frame_fails_instead_of_reporting_empty_success() {
            // How the CLI exits on a flag it doesn't accept, an unusable
            // `--permission-mode`, or missing credentials: no frames at all,
            // non-zero status, the reason on stderr. Reporting `Done("")` here
            // would let a step "succeed" having run nothing.
            let stub = stub("printf 'error: unknown option\\n' >&2\nexit 1\n");
            let chunks = run(&stub, HashMap::new(), Vec::new());
            let msg = expect_error(&chunks);
            assert!(msg.contains(NO_RESULT_MESSAGE), "diagnosis: {msg}");
            assert!(msg.contains("error: unknown option"), "stderr shown: {msg}");
        }

        #[test]
        fn a_completed_turn_is_not_demoted_by_a_limit_phrase_on_stderr() {
            // The CLI logs its internal 429 retries to stderr and still
            // finishes. Turning that into a Limit would discard the finished
            // output and re-run the whole step against a dirty worktree.
            let stub = stub(concat!(
                "printf 'API Error (429 rate limit) - Retrying in 5s\\n' >&2\n",
                r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-ok"}'"#,
                "\n",
                r#"printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]},"session_id":"sess-ok"}'"#,
                "\n",
                r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"sess-ok","result":"hello"}'"#,
                "\n",
            ));
            let chunks = run(&stub, HashMap::new(), Vec::new());
            let kinds: Vec<String> = chunks.iter().map(|c| format!("{c:?}")).collect();
            match chunks.last() {
                Some(StreamChunk::Done(text)) => assert_eq!(text.as_str(), ""),
                other => panic!("expected Done with empty text, got {other:?}"),
            }
            assert!(
                chunks
                    .iter()
                    .any(|c| matches!(c, StreamChunk::Delta(d) if d == "hello")),
                "text streamed: {kinds:?}"
            );
            assert!(
                !chunks.iter().any(|c| matches!(c, StreamChunk::Limit(_))),
                "no limit: {kinds:?}"
            );
        }

        #[test]
        fn a_limit_on_stderr_without_a_result_frame_is_retryable() {
            let stub = stub("printf 'Claude usage limit reached\\n' >&2\nexit 1\n");
            let chunks = run(&stub, HashMap::new(), Vec::new());
            assert!(
                matches!(chunks.last(), Some(StreamChunk::Limit(e)) if e.provider == "claude"),
                "expected Limit, got {chunks:?}"
            );
        }

        #[test]
        fn a_result_frame_error_is_reported_with_the_cli_text() {
            // How the installed CLI reports an unusable `--model`: a result
            // frame with `is_error: true` and the reason in `result`.
            let stub = stub(concat!(
                r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-bad"}'"#,
                "\n",
                r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"session_id":"sess-bad","result":"There is an issue with the selected model"}'"#,
                "\n",
                "exit 1\n",
            ));
            let chunks = run(&stub, HashMap::new(), Vec::new());
            let msg = expect_error(&chunks);
            assert!(msg.contains("issue with the selected model"), "{msg}");
        }

        #[test]
        fn the_session_id_is_reported_from_the_init_frame_even_when_the_run_fails() {
            let stub = stub(concat!(
                r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-early"}'"#,
                "\n",
                "printf 'boom\\n' >&2\nexit 1\n",
            ));
            let chunks = run(&stub, HashMap::new(), Vec::new());
            assert!(
                matches!(chunks.first(), Some(StreamChunk::Session(id)) if id == "sess-early"),
                "session reported first: {chunks:?}"
            );
            assert!(matches!(chunks.last(), Some(StreamChunk::Error(_))));
        }

        #[test]
        fn tools_are_served_over_streaming_mode_after_end_input() {
            // Pins the workaround `run_async` depends on: the prompt goes out
            // as a user frame and stdin is closed immediately, yet the CLI's
            // MCP `control_request` -- which arrives *after* that -- still gets
            // answered (the SDK's demux keeps a strong stdin sender while a
            // control handler is registered). If that ever regresses, cruise's
            // tools stop reaching the model.
            let stub = stub(concat!(
                r#"printf '%s\n' '{"type":"control_request","request_id":"req-1","request":{"subtype":"mcp_message","server_name":"cruise","message":{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"probe","arguments":{"k":"v"}}}}}'"#,
                "\n",
                "read -r control_response\n",
                "printf '%s\\n' \"$control_response\" > \"$CRUISE_STUB_RESPONSE_FILE\"\n",
                r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"sess-tools"}'"#,
                "\n",
            ));
            let response_file = stub.dir.join("control-response.json");
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = Arc::clone(&calls);
            let probe = CruiseTool::new(
                "probe",
                "desc",
                serde_json::json!({"type": "object", "properties": {}}),
                Arc::new(move |input| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(format!("probe ran with {input}"))
                }),
            );
            let env = HashMap::from([(
                "CRUISE_STUB_RESPONSE_FILE".to_string(),
                response_file.display().to_string(),
            )]);
            let chunks = run(&stub, env, vec![probe]);

            assert!(
                matches!(chunks.last(), Some(StreamChunk::Done(_))),
                "run completed: {chunks:?}"
            );
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            let written = std::fs::read_to_string(&response_file)
                .unwrap_or_else(|e| panic!("stub wrote no control_response: {e}"));
            let frame: serde_json::Value = serde_json::from_str(written.trim())
                .unwrap_or_else(|e| panic!("control_response not JSON: {e}: {written}"));
            assert_eq!(frame["type"], "control_response");
            assert_eq!(frame["response"]["request_id"], "req-1");
            assert!(
                written.contains(r#"probe ran with {\"k\":\"v\"}"#),
                "handler result returned to the CLI: {written}"
            );
        }

        #[test]
        fn cancelling_stops_the_run_and_terminates_the_child() {
            // The case a `timeout:` or Ctrl-C hits in practice: the model is
            // mid tool-call, so no chunk is being sent and a dropped receiver
            // would go unnoticed. Without the cancel signal the worker thread
            // and the `claude` child stay alive, still writing to a worktree
            // cruise has already given up on.
            let stub = stub(concat!(
                r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-hang"}'"#,
                "\n",
                "printf '%s\\n' \"$$\" > \"$CRUISE_STUB_PID_FILE\"\n",
                "sleep 60\n",
            ));
            let pid_file = stub.dir.join("child.pid");
            let cancel = CancellationToken::new();
            let rx = stream_agent(
                ClaudeRunnerConfig {
                    cli_path: Some(stub.path.clone()),
                    cancel: Some(cancel.clone()),
                    env: HashMap::from([(
                        "CRUISE_STUB_PID_FILE".to_string(),
                        pid_file.display().to_string(),
                    )]),
                    ..Default::default()
                },
                "hi".to_string(),
            );
            assert!(
                matches!(rx.recv_timeout(STEP_TIMEOUT), Ok(StreamChunk::Session(id)) if id == "sess-hang"),
                "stub started"
            );
            let pid = read_pid(&pid_file);

            cancel.cancel();
            // No terminal chunk is sent for a cancelled run; the channel closes
            // once the worker thread has torn the transport down. A *timeout*
            // here is the pre-fix behaviour: the thread parked on the CLI's
            // silent stdout until the stub finished on its own.
            assert!(
                matches!(
                    rx.recv_timeout(STEP_TIMEOUT),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
                ),
                "worker thread exited without reporting a verdict"
            );
            assert!(
                wait_until_gone(pid),
                "claude child {pid} outlived the cancelled run"
            );
        }

        fn read_pid(path: &std::path::Path) -> u32 {
            let deadline = Instant::now() + STEP_TIMEOUT;
            loop {
                if let Ok(text) = std::fs::read_to_string(path)
                    && let Ok(pid) = text.trim().parse()
                {
                    return pid;
                }
                assert!(Instant::now() < deadline, "stub never wrote its pid");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// Poll `kill -0 <pid>` until the process is gone. Used instead of
        /// `wait` because the child belongs to the SDK's transport, not to the
        /// test.
        fn wait_until_gone(pid: u32) -> bool {
            let deadline = Instant::now() + STEP_TIMEOUT;
            while Instant::now() < deadline {
                let alive = std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
                if !alive {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        }
    }

    // -- permission mapping ---------------------------------------------------

    fn claude_args(config: &ClaudeRunnerConfig) -> Vec<String> {
        SubprocessCliTransport::one_shot(build_options(config), "hi".to_string()).build_args()
    }

    fn arg_after(args: &[String], flag: &str) -> Option<String> {
        args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
    }

    fn csv(args: &[String], flag: &str) -> Vec<String> {
        arg_after(args, flag)
            .map(|v| v.split(',').map(str::to_string).collect())
            .unwrap_or_default()
    }

    #[test]
    fn build_options_full_keeps_bypass_permissions() {
        let opts = build_options(&ClaudeRunnerConfig {
            permission: crate::config::PermissionMode::Full,
            ..Default::default()
        });
        assert!(matches!(
            opts.permission_mode,
            Some(PermissionMode::BypassPermissions)
        ));
        assert_eq!(opts.disallowed_tools, Vec::<String>::new());
    }

    #[test]
    fn build_options_restricted_modes_use_dont_ask() {
        for mode in [
            crate::config::PermissionMode::ReadOnly,
            crate::config::PermissionMode::Edit,
        ] {
            let opts = build_options(&ClaudeRunnerConfig {
                permission: mode,
                ..Default::default()
            });
            assert!(
                matches!(opts.permission_mode, Some(PermissionMode::DontAsk)),
                "{mode:?} must not prompt"
            );
        }
    }

    #[test]
    fn build_options_read_only_disallows_shell_and_file_mutation_tools() {
        let opts = build_options(&ClaudeRunnerConfig {
            permission: crate::config::PermissionMode::ReadOnly,
            ..Default::default()
        });
        for tool in ["Bash", "Edit", "Write", "NotebookEdit"] {
            assert!(
                opts.disallowed_tools.iter().any(|t| t == tool),
                "{tool} should be disallowed: {:?}",
                opts.disallowed_tools
            );
        }
    }

    #[test]
    fn build_options_edit_disallows_bash_but_allows_direct_edit_tools() {
        let opts = build_options(&ClaudeRunnerConfig {
            permission: crate::config::PermissionMode::Edit,
            ..Default::default()
        });
        assert!(opts.disallowed_tools.iter().any(|t| t == "Bash"));
        for tool in ["Edit", "Write"] {
            assert!(
                opts.allowed_tools.iter().any(|t| t == tool),
                "{tool} should be allowed: {:?}",
                opts.allowed_tools
            );
            assert!(!opts.disallowed_tools.iter().any(|t| t == tool));
        }
    }

    #[test]
    fn build_options_restricted_modes_allow_cruise_tools_only() {
        for mode in [
            crate::config::PermissionMode::ReadOnly,
            crate::config::PermissionMode::Edit,
        ] {
            let opts = build_options(&ClaudeRunnerConfig {
                permission: mode,
                tools: vec![tool("ask_user"), tool("submit_plan")],
                ..Default::default()
            });
            assert!(
                opts.allowed_tools
                    .iter()
                    .any(|t| t == "mcp__cruise__ask_user")
            );
            assert!(
                opts.allowed_tools
                    .iter()
                    .any(|t| t == "mcp__cruise__submit_plan")
            );
            assert!(
                !opts
                    .allowed_tools
                    .iter()
                    .any(|t| t.starts_with("mcp__") && !t.starts_with("mcp__cruise__")),
                "external MCP must not be allowed: {:?}",
                opts.allowed_tools
            );
        }
    }

    #[test]
    fn restricted_modes_pass_dont_ask_and_tool_lists_to_the_cli() {
        let args = claude_args(&ClaudeRunnerConfig {
            permission: crate::config::PermissionMode::ReadOnly,
            ..Default::default()
        });
        assert_eq!(
            arg_after(&args, "--permission-mode").as_deref(),
            Some("dontAsk")
        );
        let disallowed = csv(&args, "--disallowedTools");
        for tool in ["Bash", "Edit", "Write", "NotebookEdit"] {
            assert!(disallowed.iter().any(|t| t == tool), "{disallowed:?}");
        }

        let args = claude_args(&ClaudeRunnerConfig {
            permission: crate::config::PermissionMode::Edit,
            ..Default::default()
        });
        assert_eq!(
            arg_after(&args, "--permission-mode").as_deref(),
            Some("dontAsk")
        );
        assert!(csv(&args, "--disallowedTools").iter().any(|t| t == "Bash"));
        assert!(!args.iter().any(|a| a == "default" || a == "acceptEdits"));
    }

    #[test]
    fn full_mode_cli_args_keep_bypass_permissions_without_tool_lists() {
        let args = claude_args(&ClaudeRunnerConfig::default());
        assert_eq!(
            arg_after(&args, "--permission-mode").as_deref(),
            Some("bypassPermissions")
        );
        assert!(!args.iter().any(|a| a == "--disallowedTools"));
    }
}
