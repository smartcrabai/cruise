//! Prompt-execution backend abstraction.
//!
//! Cruise drives prompts through one of five backends: an external `command`
//! (the classic `claude -p` path), the in-process **seher SDK**
//! (`sdk: seher`), **pi directly** (`sdk: pi`), the **`jcode` CLI**
//! (`sdk: jcode`), or the **`claude` CLI** (`sdk: claude`). [`Executor`] hides
//! that choice behind a single [`Executor::run`] call so that `planning.rs`,
//! `engine.rs`, and the GUI command layer don't need to branch on the backend.
//!
//! In `sdk: seher` mode the cruise `model` / `plan_model` / per-step `model`
//! fields are reinterpreted as seher **mode keys** (see [`mode_key_for_step`] /
//! [`mode_key_for_plan`]); seher resolves the actual provider/model from its
//! own `~/.config/seher/config.yaml`.
//!
//! In `sdk: pi` mode those same fields are instead a raw model reference
//! (`"provider/model[:thinking]"` or a bare `"model"`) passed straight to
//! `pi_agent_rust`, bypassing seher's provider-resolution layer entirely --
//! see [`run_pi_direct`] / [`parse_pi_model_ref`].
//!
//! In `sdk: claude` mode they are a plain `claude --model` name with an
//! optional `:effort` suffix, driven in-process through `claude-agent-sdk` --
//! see [`run_claude`] and [`crate::backend::claude`].
//!
//! In `sdk: jcode` mode they are a `provider/model[:effort]` reference in
//! jcode's own provider/model namespace, driven as a `jcode run --ndjson`
//! subprocess under cruise's private `JCODE_HOME` -- see [`run_jcode`] and
//! [`crate::backend::jcode`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use seher::sdk::{
    CodexBarProbe, EffortLevel as SeherEffortLevel, PiRunner, PiRunnerOptions, PollOptions,
    SeherTool, StreamChunk as SeherStreamChunk, poll_for_agent,
};

use crate::backend::claude::{ClaudeRunnerConfig, stream_agent as stream_claude_agent};
use crate::backend::effort::{EffortLevel, effort_from_suffix, split_thinking_suffix};
use crate::backend::jcode::{self, JcodeRunnerConfig, stream_agent as stream_jcode_agent};
use crate::backend::stream::{ChunkOutcome, ChunkReducer, LimitError, LineBuffer, StreamChunk};
use crate::backend::tool::CruiseTool;
use crate::cancellation::CancellationToken;
use crate::error::{CruiseError, Result};
use crate::step::prompt::{PromptResult, StreamCallbacks, run_prompt};
use crate::tool_bridge::ToolBridge;

/// Adapt a seher chunk to cruise's own [`StreamChunk`], which the fold in
/// [`stream_to_outcome`] is written against.
impl From<SeherStreamChunk> for StreamChunk {
    fn from(chunk: SeherStreamChunk) -> Self {
        match chunk {
            SeherStreamChunk::Delta(text) => StreamChunk::Delta(text),
            SeherStreamChunk::Done(text) => StreamChunk::Done(text),
            SeherStreamChunk::Session(id) => StreamChunk::Session(id),
            SeherStreamChunk::Limit(e) => StreamChunk::Limit(LimitError {
                provider: e.provider,
            }),
            SeherStreamChunk::Error(message) => StreamChunk::Error(message),
        }
    }
}

/// Adapt a cruise tool to the seher tool type the seher SDKs register. Both
/// carry the same `Arc`'d handler, so the shared interior state (plan-persist
/// flag, title store) survives the conversion.
impl From<CruiseTool> for SeherTool {
    fn from(tool: CruiseTool) -> Self {
        SeherTool::new(tool.name, tool.description, tool.parameters, tool.handler)
    }
}

/// Adapt a cruise effort tier to the seher one the seher SDK configs take. The
/// tiers are identical, so the mapping is total in both directions.
impl From<EffortLevel> for SeherEffortLevel {
    fn from(effort: EffortLevel) -> Self {
        match effort {
            EffortLevel::Low => SeherEffortLevel::Low,
            EffortLevel::Medium => SeherEffortLevel::Medium,
            EffortLevel::High => SeherEffortLevel::High,
            EffortLevel::XHigh => SeherEffortLevel::XHigh,
            EffortLevel::Max => SeherEffortLevel::Max,
        }
    }
}

impl From<SeherEffortLevel> for EffortLevel {
    fn from(effort: SeherEffortLevel) -> Self {
        match effort {
            SeherEffortLevel::Low => EffortLevel::Low,
            SeherEffortLevel::Medium => EffortLevel::Medium,
            SeherEffortLevel::High => EffortLevel::High,
            SeherEffortLevel::XHigh => EffortLevel::XHigh,
            SeherEffortLevel::Max => EffortLevel::Max,
        }
    }
}

fn seher_tools(tools: &[CruiseTool]) -> Vec<SeherTool> {
    tools.iter().cloned().map(Into::into).collect()
}

/// Default seher mode key for ordinary prompt steps when neither the step nor
/// the workflow specifies one.
pub const DEFAULT_STEP_MODE_KEY: &str = "build";

/// Default seher mode key for the built-in planning step.
pub const DEFAULT_PLAN_MODE_KEY: &str = "plan";

/// Poll interval (ms) used while every seher provider is rate-limited.
const SDK_POLL_INTERVAL_MS: u64 = 60_000;

/// A single prompt execution request, backend-agnostic.
///
/// Built by the caller and handed to [`Executor::run`]. `model_or_mode` carries
/// a model name in command mode, a seher `mode_key` in `sdk: seher` mode, and a
/// raw model reference in `sdk: pi` / `sdk: claude` mode (compute it with
/// [`Executor::step_model_or_mode`] / [`Executor::plan_model_or_mode`]). `tools`
/// and `resume` are honored by SDK backends except the RPC backends `omp` and
/// `pi`, whose sessions are intentionally closed after each prompt because
/// Cruise may rebuild its tools.
pub struct PromptRun<'a> {
    /// The fully-resolved prompt text to send.
    pub prompt: &'a str,
    /// Model name (command mode), `mode_key` (`sdk: seher`), or model
    /// reference (`sdk: pi`, `sdk: claude`).
    pub model_or_mode: Option<&'a str>,
    /// Maximum rate-limit retries.
    pub max_retries: usize,
    /// Environment variables applied to the prompt run.
    ///
    /// Command mode passes these to the spawned process. `sdk: seher` forwards
    /// them to the selected seher backend; external RPC/Claude subprocesses
    /// pass them to child processes, while `pi-rust` applies them through
    /// process environment mutation inside seher. RPC backends inherit ambient
    /// variables, but ignore workflow `PATH`/`PATHEXT` so helper resolution
    /// cannot be redirected. `sdk: pi` applies them directly (see
    /// [`PiRunnerOptions::env`]); `sdk: claude` passes them to the `claude`
    /// child process.
    pub env: &'a HashMap<String, String>,
    /// Callback invoked with human-readable progress notices: seher provider
    /// resolution (`sdk: seher`) and rate-limit retries (every backend).
    pub on_notice: Option<&'a (dyn Fn(&str) + Send + Sync)>,
    /// Cooperative cancellation token.
    pub cancel_token: Option<&'a CancellationToken>,
    /// Working directory for the command / agent.
    pub working_dir: Option<&'a Path>,
    /// Streaming stdout/stderr callbacks.
    pub stream: Option<&'a StreamCallbacks<'a>>,
    /// Custom tools to inject (SDK mode only).
    pub tools: Vec<CruiseTool>,
    /// Prior session id to resume (SDK mode only; the RPC backends `omp` and
    /// `pi` always start a fresh session).
    pub resume: Option<String>,
}

/// Outcome of [`Executor::run`]: the prompt result plus, in SDK mode, the seher
/// session id (for a follow-up `resume`). `session_id` is `None` in command mode
/// and for the RPC backends `omp` / `pi`, whose sessions are closed after each
/// Cruise prompt.
#[derive(Debug, Clone)]
pub struct PromptOutcome {
    pub result: PromptResult,
    pub session_id: Option<String>,
}

/// Prompt-execution backend.
///
/// The SDK backends' tools (which capture the [`AskHandler`]) are built by the
/// caller and passed via [`PromptRun::tools`], so the executor itself holds no
/// handler.
pub enum Executor {
    /// Spawn an external command (the classic `claude -p` path).
    Command { command: Vec<String> },
    /// Drive prompts through one of the seher SDKs (`pi`, `omp`, `pi-rust`,
    /// `claude`, `claude-terminal`, `claude-headless`); the concrete backend is
    /// picked by [`spawn_agent_stream`] from the resolved provider's `sdk` field.
    /// Selected by `sdk: seher` (or any `sdk:` value other than `"pi"` /
    /// `"claude"`).
    Sdk,
    /// Drive prompts through `pi_agent_rust` directly (`sdk: pi`), bypassing
    /// seher's provider-resolution layer and `~/.config/seher/config.yaml`
    /// entirely. See [`run_pi_direct`].
    Pi,
    /// Drive the `jcode` CLI as an NDJSON subprocess (`sdk: jcode`) under
    /// cruise's private `JCODE_HOME`, exposing cruise's tools over the
    /// [`ToolBridge`]. See [`run_jcode`].
    Jcode,
    /// Drive the `claude` CLI in-process through `claude-agent-sdk`
    /// (`sdk: claude`), with no seher provider resolution involved. See
    /// [`run_claude`].
    Claude,
}

impl Executor {
    /// Build an executor from the workflow's backend selection.
    ///
    /// `sdk: pi` -> [`Executor::Pi`]; `sdk: jcode` -> [`Executor::Jcode`];
    /// `sdk: claude` -> [`Executor::Claude`]; any other `sdk` value ->
    /// [`Executor::Sdk`]; no `sdk` -> [`Executor::Command`] wrapping `command`.
    /// (Mutual exclusivity between `sdk` and `command`, and that `sdk` is one of
    /// the accepted values, is enforced earlier by
    /// [`crate::config::validate_sdk`].)
    #[must_use]
    pub fn new(sdk: Option<&str>, command: &[String]) -> Self {
        match sdk {
            Some("pi") => Executor::Pi,
            Some("jcode") => Executor::Jcode,
            Some("claude") => Executor::Claude,
            Some(_) => Executor::Sdk,
            None => Executor::Command {
                command: command.to_vec(),
            },
        }
    }

    /// Whether this executor drives prompts through an agent backend (`Sdk`,
    /// `Pi`, `Jcode`, or `Claude`) rather than spawning an external `command`.
    #[must_use]
    pub fn is_sdk(&self) -> bool {
        matches!(
            self,
            Executor::Sdk | Executor::Pi | Executor::Jcode | Executor::Claude
        )
    }

    /// Resolve the model name (command mode), `mode_key` (`sdk: seher`), or
    /// model reference (`sdk: pi` / `sdk: jcode` / `sdk: claude`) for an
    /// ordinary prompt step.
    #[must_use]
    pub fn step_model_or_mode(
        &self,
        step_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. } | Executor::Pi | Executor::Jcode | Executor::Claude => {
                step_model.or(global_model).map(str::to_string)
            }
            Executor::Sdk => Some(mode_key_for_step(step_model, global_model)),
        }
    }

    /// Resolve the model name (command mode), `mode_key` (`sdk: seher`), or
    /// model reference (`sdk: pi` / `sdk: jcode` / `sdk: claude`) for the
    /// built-in planning step.
    #[must_use]
    pub fn plan_model_or_mode(
        &self,
        plan_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. } | Executor::Pi | Executor::Jcode | Executor::Claude => {
                plan_model.or(global_model).map(str::to_string)
            }
            Executor::Sdk => Some(mode_key_for_plan(plan_model, global_model)),
        }
    }

    /// Execute one prompt on the selected backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to spawn / exits non-zero, or if
    /// seher provider resolution, the seher SDK run, the direct pi run, the
    /// `jcode` run, or the `claude` CLI run fails.
    pub async fn run(&self, req: PromptRun<'_>) -> Result<PromptOutcome> {
        match self {
            Executor::Command { command } => run_command(command, req).await,
            Executor::Sdk => run_sdk(req).await,
            Executor::Pi => run_pi_direct(req).await,
            Executor::Jcode => run_jcode(req).await,
            Executor::Claude => run_claude(req).await,
        }
    }
}

/// Resolve a non-rate-limited seher provider for `mode_key`.
///
/// `require_tools` restricts candidates to SDKs that can execute custom tools
/// (`pi`, `omp`, `pi-rust`, and `claude`); with `false`, the tool-incapable SDKs
/// (`claude-terminal` and `claude-headless`) are also eligible and the caller
/// must dispatch on `ResolvedAgent::sdk`.
///
/// `poll_for_agent` borrows a `&mut dyn LimitProbe` whose probe future is not
/// `Send`, which would make the whole `run_sdk` future `!Send` and break the
/// multi-threaded Tauri runtime. Confine that `!Send` work to a dedicated thread
/// with its own current-thread runtime and return the `Send` `ResolvedAgent`.
async fn resolve_provider(
    mode_key: String,
    require_tools: bool,
    cancel: Arc<AtomicBool>,
) -> Result<seher::sdk::ResolvedAgent> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| CruiseError::Other(format!("failed to build seher resolver runtime: {e}")))
            .and_then(|rt| {
                rt.block_on(async {
                    let mut probe = CodexBarProbe;
                    poll_for_agent(
                        PollOptions {
                            mode_key,
                            require_tools,
                            interval_ms: SDK_POLL_INTERVAL_MS,
                            // Lets the caller abort the (otherwise unbounded)
                            // all-providers-rate-limited wait.
                            cancel: Some(cancel),
                            ..Default::default()
                        },
                        &mut probe,
                    )
                    .await
                    .map_err(|e| {
                        CruiseError::CommandError(format!("seher provider resolution failed: {e}"))
                    })
                })
            });
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| CruiseError::Other("seher resolver thread terminated".to_string()))?
}

/// Sets an abort flag when dropped, so a detached resolver thread stops polling
/// if the awaiting future is cancelled or dropped.
struct AbortOnDrop(Arc<AtomicBool>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Cancels an OMP runner when the SDK execution future is dropped.
struct CancelOnDrop(seher::sdk::CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Resolves when the token is cancelled, or waits forever if no token is given.
async fn maybe_cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(t) => t.cancelled().await,
        None => std::future::pending().await,
    }
}

/// Command-backend execution: resolve the `{model}` placeholder then delegate to
/// the existing [`run_prompt`].
async fn run_command(command: &[String], req: PromptRun<'_>) -> Result<PromptOutcome> {
    let resolved = crate::engine::resolve_command_with_model(command, req.model_or_mode)?;
    let model_arg = if resolved.consumed_model_placeholder {
        None
    } else {
        req.model_or_mode.map(str::to_string)
    };
    let resolved_command = resolved.command;

    let retry = |msg: &str| {
        if let Some(cb) = req.on_notice {
            cb(msg);
        }
    };
    let result = run_prompt(
        &resolved_command,
        model_arg.as_deref(),
        req.prompt,
        req.max_retries,
        req.env,
        Some(&retry),
        req.cancel_token,
        req.working_dir,
        req.stream,
    )
    .await?;
    Ok(PromptOutcome {
        result,
        session_id: None,
    })
}

/// Start `resolved` on the engine its `sdk` kind requires and return the chunk
/// stream. Routes by `ResolvedAgent::sdk`:
///
/// - `pi` — the external pi CLI driven over RPC by seher (`pi`, falling back to
///   `bunx`/`npx`). `resolved.model_id` is a full pi model ref
///   (`"<pi-provider>/<model>[:thinking]"`).
/// - `omp` — oh-my-pi RPC subprocess. `resolved.model_id` is a full pi model ref.
/// - `pi-rust` — seher's in-process Rust pi engine (`pi_agent_rust`), whose
///   model catalog is baked into the crate.
/// - `claude` — `claude-agent-sdk` (supports custom tools). `resolved.model_id`
///   is a plain `claude --model` name.
/// - `claude-terminal` — local `claude` CLI via tmux. No tools.
/// - `claude-headless` — `claude -p` subprocess. No tools.
///
/// `seher::sdk::is_supported_sdk` filters the candidate list to these kinds
/// before resolution; an unknown kind here indicates the cruise<->seher
/// dispatch mapping has drifted out of sync with the seher version in use.
fn spawn_agent_stream(
    resolved: &seher::sdk::ResolvedAgent,
    req: &PromptRun<'_>,
    omp_cancel: seher::sdk::CancelToken,
) -> std::sync::mpsc::Receiver<SeherStreamChunk> {
    let cwd_string = req.working_dir.map(|p| p.to_string_lossy().into_owned());
    match resolved.sdk.as_str() {
        "claude" => {
            // claude-agent-sdk supports custom tools natively, so `req.tools`
            // flows through as seher tools. `resolved.model_id` is a plain
            // `claude --model` name, not a pi model ref.
            let config = seher::claude_agent::ClaudeAgentRunnerConfig {
                model: claude_family_model(&resolved.model_id),
                effort: claude_family_effort(&resolved.model_id, resolved.effort).map(Into::into),
                cwd: cwd_string,
                resume_session_id: req.resume.clone(),
                tools: seher_tools(&req.tools),
                env: req.env.clone(),
                ..Default::default()
            };
            seher::claude_agent::stream_agent(
                config,
                req.prompt.to_string(),
                resolved.provider.clone(),
            )
        }
        "claude-headless" => {
            // `claude -p` subprocess. Cannot run custom tools; `require_tools`
            // in [`run_sdk`] guarantees `req.tools` is empty here.
            // ClaudeHeadlessRunnerConfig is #[non_exhaustive] in seher-sdk
            // 0.0.45+, so we can't use struct-literal syntax across crates.
            let mut headless_cfg = seher::claude_headless::ClaudeHeadlessRunnerConfig::default();
            headless_cfg.model = claude_family_model(&resolved.model_id);
            headless_cfg.effort =
                claude_family_effort(&resolved.model_id, resolved.effort).map(Into::into);
            headless_cfg.cwd = cwd_string;
            headless_cfg.resume_session_id.clone_from(&req.resume);
            headless_cfg.env = req
                .env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let runner = seher::claude_headless::ClaudeHeadlessRunner::new(headless_cfg);
            seher::claude_headless::stream_headless(
                runner,
                req.prompt.to_string(),
                resolved.provider.clone(),
            )
        }
        "claude-terminal" => {
            // claude-terminal cannot run custom tools; `require_tools` in
            // [`run_sdk`] guarantees `req.tools` is empty here. `resolved.model_id`
            // is a plain `claude --model` name, not a pi model ref.
            let sdk = seher::claude_terminal::new_sdk_with_defaults(
                None,
                None,
                claude_family_model(&resolved.model_id),
                None,
                claude_family_effort(&resolved.model_id, resolved.effort).map(Into::into),
                None,
                cwd_string,
                req.env.clone(),
            );
            seher::claude_terminal::stream_via_thread(
                sdk,
                req.prompt.to_string(),
                resolved.provider.clone(),
                req.resume.clone(),
            )
        }
        "pi" | "omp" | "pi-rust" => {
            let resume = match resolved.sdk.as_str() {
                // The RPC backends keep one live child process per session and
                // reject a resume whose tool set / options fingerprint changed,
                // and [`finish_sdk_session`] closes their sessions after every
                // prompt, so there is never a session left to resume.
                "omp" | "pi" => None,
                // PiRust can only open its own on-disk sessions. A provider
                // fallback may hand us a Claude/OMP id; starting fresh is safer
                // than passing a foreign id that PiRust cannot open.
                "pi-rust" => req.resume.as_deref().and_then(|id| {
                    seher::sdk::pi_session_path(req.working_dir, id)
                        .is_file()
                        .then(|| id.to_string())
                }),
                _ => unreachable!("shared RPC/PiRust dispatch branch"),
            };
            let mut resolved = resolved.clone();
            if matches!(resolved.sdk.as_str(), "omp" | "pi") {
                merge_helper_env(&mut resolved, req.env);
            } else {
                resolved
                    .env
                    .extend(req.env.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            seher::sdk::stream_for_resolved(
                &resolved,
                req.prompt.to_string(),
                seher::sdk::RunAgentOptions {
                    working_dir: req.working_dir.map(Path::to_path_buf),
                    resume,
                    tools: seher_tools(&req.tools),
                    cancel: omp_cancel,
                    ..Default::default()
                },
            )
        }
        other => unreachable!(
            "seher resolver returned an unsupported sdk kind: {other:?} \
             (cruise dispatch is out of sync with the seher version in use)"
        ),
    }
}

/// Merge ambient and request variables into an RPC backend's launch
/// environment without allowing workflow `PATH` values to select the helper
/// executable. Configured values in `ResolvedAgent::env` take precedence over
/// ambient defaults; request values take precedence except for `PATH`/`PATHEXT`.
/// `ResolvedAgent::env` is also the environment seher's OMP / pi candidate
/// resolvers search for `omp`, `pi`, `bunx` and `npx`.
fn merge_helper_env(
    resolved: &mut seher::sdk::ResolvedAgent,
    request_env: &HashMap<String, String>,
) {
    for (key, value) in std::env::vars() {
        resolved.env.entry(key).or_insert(value);
    }
    for (key, value) in request_env {
        if matches!(key.as_str(), "PATH" | "PATHEXT") {
            continue;
        }
        resolved.env.insert(key.clone(), value.clone());
    }
}

/// RPC-backend sessions are closed after each Cruise prompt because planning
/// rebuilds its tool handlers between turns; seher fingerprints those handler
/// identities and cannot resume a session with a different tool set. Closing
/// also reaps the backend's child process, which seher keeps alive per session.
fn finish_sdk_session(
    sdk: &str,
    working_dir: Option<&Path>,
    session: Option<String>,
) -> Option<String> {
    match sdk {
        "omp" => {
            if let Some(session_id) = session.as_deref() {
                let _ = seher::sdk::close_omp_session(session_id, working_dir);
            }
            None
        }
        "pi" => {
            if let Some(session_id) = session.as_deref() {
                let _ = seher::sdk::close_pi_session(session_id, working_dir);
            }
            None
        }
        _ => session,
    }
}

/// SDK-backend execution: resolve a non-limited provider, run it via
/// [`spawn_agent_stream`], and fold the streamed chunks into a [`PromptOutcome`].
async fn run_sdk(req: PromptRun<'_>) -> Result<PromptOutcome> {
    let mode_key = req
        .model_or_mode
        .unwrap_or(DEFAULT_STEP_MODE_KEY)
        .to_string();
    let on_delta = req.stream.and_then(|s| s.on_stdout);
    // Custom tools only run on tool-capable SDKs (`pi`, `omp`, `pi-rust`,
    // `claude`), and `resume` ids belong to whichever SDK started the session —
    // every resumable turn in the planning flow starts with a tool-registering
    // one — so both pin resolution to tool-capable providers. Tool-less fresh
    // runs may also resolve tool-incapable SDKs (`claude-terminal`,
    // `claude-headless`).
    let require_tools = !req.tools.is_empty() || req.resume.is_some();

    let mut attempts = 0;
    loop {
        if let Some(cb) = req.on_notice {
            cb(&resolving_notice(&mode_key, require_tools));
        }
        // Signal the detached resolver thread to stop polling if this future is
        // cancelled or dropped (e.g. timeout / Ctrl-C) before resolution finishes.
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let abort_guard = AbortOnDrop(Arc::clone(&cancel_flag));
        let resolved = tokio::select! {
            biased;
            () = maybe_cancelled(req.cancel_token) => return Err(CruiseError::Interrupted),
            out = resolve_provider(mode_key.clone(), require_tools, cancel_flag) => out,
        }?;
        // Resolution finished; the resolver thread has already exited.
        drop(abort_guard);
        if let Some(cb) = req.on_notice {
            cb(&resolution_notice(&resolved));
        }
        let omp_cancel = seher::sdk::CancelToken::new();
        let _omp_cancel_guard = CancelOnDrop(omp_cancel.clone());
        let session_slot =
            matches!(resolved.sdk.as_str(), "omp" | "pi").then(|| Arc::new(Mutex::new(None)));
        let rx_std = spawn_agent_stream(&resolved, &req, omp_cancel);
        let outcome =
            match stream_to_outcome(rx_std, on_delta, req.cancel_token, session_slot.clone()).await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    if matches!(&error, CruiseError::Interrupted) {
                        let session = session_slot
                            .as_ref()
                            .and_then(|slot| slot.lock().ok().and_then(|session| session.clone()));
                        let _ = finish_sdk_session(&resolved.sdk, req.working_dir, session);
                    }
                    return Err(error);
                }
            };

        match outcome {
            ChunkOutcome::Done { output, session } => {
                let session = finish_sdk_session(&resolved.sdk, req.working_dir, session);
                return Ok(PromptOutcome {
                    result: PromptResult {
                        output,
                        stderr: String::new(),
                    },
                    session_id: session,
                });
            }
            ChunkOutcome::Failed { message, session } => {
                let _ = finish_sdk_session(&resolved.sdk, req.working_dir, session);
                return Err(CruiseError::CommandError(message));
            }
            ChunkOutcome::Limited { message, session } => {
                let _ = finish_sdk_session(&resolved.sdk, req.working_dir, session);
                if attempts < req.max_retries {
                    attempts += 1;
                    if let Some(cb) = req.on_notice {
                        cb(&rate_limited_notice(&resolved, attempts, req.max_retries));
                    }
                    continue;
                }
                return Err(CruiseError::CommandError(message));
            }
            ChunkOutcome::Closed { session, .. } => {
                let _ = finish_sdk_session(&resolved.sdk, req.working_dir, session);
                return Err(CruiseError::Other(
                    "seher stream closed before completion".to_string(),
                ));
            }
        }
    }
}

/// Bridge a blocking `std::sync::mpsc::Receiver` of backend chunks (as returned
/// by seher's various `stream_*` functions, [`PiRunner::stream`], and
/// [`stream_claude_agent`]) into a [`ChunkOutcome`], forwarding text deltas to
/// `on_delta` line-buffered (the SDK backends emit token-level deltas;
/// `StreamCallbacks::on_stdout` is line-oriented like the command backend) and
/// returning `Err(Interrupted)` as soon as `cancel_token` fires.
///
/// Generic over the chunk type so a backend that already speaks cruise's
/// [`StreamChunk`] costs nothing to fold, while the seher backends convert on
/// the bridge thread. Shared by [`run_sdk`], [`run_pi_direct`], and
/// [`run_claude`] so every backend folds a chunk stream into an outcome
/// identically.
async fn stream_to_outcome<C: Into<StreamChunk> + Send + 'static>(
    rx_std: std::sync::mpsc::Receiver<C>,
    on_delta: Option<&(dyn Fn(&str) + Send + Sync)>,
    cancel_token: Option<&CancellationToken>,
    session_slot: Option<Arc<Mutex<Option<String>>>>,
) -> Result<ChunkOutcome> {
    // Bridge the blocking std channel to an async one so we can stream deltas
    // through the borrowed `on_delta` callback without moving it onto the
    // backend's worker thread.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamChunk>();
    let bridge_session_slot = session_slot;
    std::thread::spawn(move || {
        while let Ok(chunk) = rx_std.recv() {
            let chunk: StreamChunk = chunk.into();
            if let StreamChunk::Session(id) = &chunk
                && let Some(slot) = &bridge_session_slot
                && let Ok(mut session) = slot.lock()
            {
                *session = Some(id.clone());
            }
            if tx.send(chunk).is_err() {
                break;
            }
        }
    });

    let mut line_buf = LineBuffer::new();
    let mut reducer = ChunkReducer::new();
    let outcome = loop {
        tokio::select! {
            biased;
            () = maybe_cancelled(cancel_token) => return Err(CruiseError::Interrupted),
            maybe = rx.recv() => match maybe {
                Some(chunk) => {
                    let mut sink = |d: &str| {
                        if let Some(cb) = on_delta {
                            line_buf.push(d, cb);
                        }
                    };
                    if let Some(out) = reducer.step(chunk, &mut sink) {
                        break out;
                    }
                }
                None => break reducer.finish(),
            }
        }
    };
    if let Some(cb) = on_delta {
        line_buf.flush(cb);
    }
    Ok(outcome)
}

/// `Pi`-backend execution: build [`PiRunnerOptions`] straight from `req` (via
/// [`build_pi_options`]) and drive [`PiRunner::stream`] in-process, bypassing
/// seher's provider resolution ([`resolve_provider`]) entirely — there is no
/// seher `~/.config/seher/config.yaml` involved and, unlike [`run_sdk`], no
/// fallback provider to hop to on a rate limit.
///
/// A [`ChunkOutcome::Limited`] therefore retries the *same* `PiRunnerOptions`
/// and prompt with exponential backoff
/// ([`crate::step::command::calculate_backoff`]: 2s doubling to a 60s cap),
/// mirroring the command backend's rate-limit handling rather than `run_sdk`'s
/// re-resolve-and-retry loop, up to `req.max_retries` attempts.
// Cancellation caveat: dropping this future (step `timeout:` firing, Ctrl-C)
// stops cruise from waiting, but the in-flight pi call keeps running on its
// detached worker thread until it finishes on its own — `PiRunner::stream`
// offers no cancellation hook. When `env:` overrides are set, that orphaned
// run also keeps holding seher's process-wide env mutex, so a subsequent
// `sdk: pi` step can block until the abandoned call completes. Same
// limitation as the seher-resolved `pi-rust` engine; documented in
// skills/cruise-config/references/sdk.md.
//
// Rate-limit retries deliberately start a *fresh* pi session (the original
// `req.resume`, not the aborted attempt's session id): re-sending the same
// prompt into a partially-answered session would duplicate context, and a
// clean re-run of an idempotent step prompt is strictly safer.
async fn run_pi_direct(req: PromptRun<'_>) -> Result<PromptOutcome> {
    let opts = build_pi_options(&req, req.model_or_mode)?;
    let runner = PiRunner::new(opts);
    let on_delta = req.stream.and_then(|s| s.on_stdout);

    let mut attempts = 0;
    loop {
        let rx_std = runner.stream(req.prompt.to_string(), req.resume.clone());
        let outcome = stream_to_outcome(rx_std, on_delta, req.cancel_token, None).await?;

        match outcome {
            ChunkOutcome::Done { output, session } => {
                return Ok(PromptOutcome {
                    result: PromptResult {
                        output,
                        stderr: String::new(),
                    },
                    session_id: session,
                });
            }
            ChunkOutcome::Failed { message, .. } => return Err(CruiseError::CommandError(message)),
            ChunkOutcome::Limited { message, .. } => {
                if attempts < req.max_retries {
                    attempts += 1;
                    let delay = crate::step::command::calculate_backoff(attempts);
                    if let Some(cb) = req.on_notice {
                        cb(&format!(
                            "Rate limit detected. Retrying in {:.1}s... ({attempts}/{})",
                            delay.as_secs_f64(),
                            req.max_retries
                        ));
                    }
                    tokio::select! {
                        biased;
                        () = maybe_cancelled(req.cancel_token) => return Err(CruiseError::Interrupted),
                        () = tokio::time::sleep(delay) => {}
                    }
                    continue;
                }
                return Err(CruiseError::CommandError(message));
            }
            ChunkOutcome::Closed { .. } => {
                return Err(CruiseError::Other(
                    "pi stream closed before completion".to_string(),
                ));
            }
        }
    }
}

/// `Claude`-backend execution: drive the `claude` CLI in-process through
/// `claude-agent-sdk` ([`crate::backend::claude`]), with no seher provider
/// resolution ([`resolve_provider`]) involved — there is no
/// `~/.config/seher/config.yaml` and, unlike [`run_sdk`], no fallback provider
/// to hop to on a rate limit.
///
/// A [`ChunkOutcome::Limited`] therefore retries the *same* model with
/// exponential backoff ([`crate::step::command::calculate_backoff`]: 2s
/// doubling to a 60s cap), mirroring the command backend's rate-limit handling
/// rather than [`run_sdk`]'s re-resolve-and-retry loop, up to `req.max_retries`
/// attempts.
///
/// Retries deliberately start from `req.resume` (the caller's session), not the
/// aborted attempt's session id: re-sending the same prompt into a
/// partially-answered session would duplicate context.
///
/// `req.cancel_token` is forwarded to the backend thread, which stops reading
/// the CLI's output and closes the transport when it fires — that is what
/// terminates the `claude` child, since the SDK offers no interrupt.
async fn run_claude(req: PromptRun<'_>) -> Result<PromptOutcome> {
    let on_delta = req.stream.and_then(|s| s.on_stdout);

    let mut attempts = 0;
    loop {
        let rx_std = stream_claude_agent(build_claude_config(&req), req.prompt.to_string());
        let outcome = stream_to_outcome(rx_std, on_delta, req.cancel_token, None).await?;

        match outcome {
            ChunkOutcome::Done { output, session } => {
                return Ok(PromptOutcome {
                    result: PromptResult {
                        output,
                        stderr: String::new(),
                    },
                    session_id: session,
                });
            }
            ChunkOutcome::Failed { message, .. } => return Err(CruiseError::CommandError(message)),
            ChunkOutcome::Limited { message, .. } => {
                if attempts < req.max_retries {
                    attempts += 1;
                    let delay = crate::step::command::calculate_backoff(attempts);
                    if let Some(cb) = req.on_notice {
                        cb(&format!(
                            "Rate limit detected. Retrying in {:.1}s... ({attempts}/{})",
                            delay.as_secs_f64(),
                            req.max_retries
                        ));
                    }
                    tokio::select! {
                        biased;
                        () = maybe_cancelled(req.cancel_token) => return Err(CruiseError::Interrupted),
                        () = tokio::time::sleep(delay) => {}
                    }
                    continue;
                }
                return Err(CruiseError::CommandError(message));
            }
            ChunkOutcome::Closed { .. } => {
                return Err(CruiseError::Other(
                    "claude stream closed before completion".to_string(),
                ));
            }
        }
    }
}

/// Build a [`ClaudeRunnerConfig`] straight from `req`, with no seher provider
/// resolution involved. `req.model_or_mode` is a plain `claude --model` name
/// with an optional `:effort` suffix (see [`Executor::step_model_or_mode`] /
/// [`Executor::plan_model_or_mode`]); unset leaves `--model` off so the CLI
/// picks its own default.
///
/// Built fresh per attempt because [`crate::backend::claude::stream_agent`]
/// consumes the config.
fn build_claude_config(req: &PromptRun<'_>) -> ClaudeRunnerConfig {
    let (model, suffix) = split_thinking_suffix(req.model_or_mode.unwrap_or_default());
    ClaudeRunnerConfig {
        model: (!model.is_empty()).then(|| model.to_string()),
        effort: suffix.and_then(effort_from_suffix),
        cwd: req.working_dir.map(Path::to_path_buf),
        resume_session_id: req.resume.clone(),
        tools: req.tools.clone(),
        env: req.env.clone(),
        cancel: req.cancel_token.cloned(),
        cli_path: None,
    }
}

/// `Jcode`-backend execution: run the prompt as a `jcode run --ndjson`
/// subprocess ([`crate::backend::jcode`]) under cruise's private `JCODE_HOME`,
/// with no seher provider resolution involved and, unlike [`run_sdk`], no
/// fallback provider to hop to on a rate limit.
///
/// jcode has no in-process tool registration, so cruise's tools are served to
/// it over a per-run Unix socket by a [`ToolBridge`]: the `cruise mcp-bridge`
/// server jcode spawns relays every call back here, which is what keeps the
/// handlers' in-process state (the terminal `ask_user` prompt, the plan-persist
/// flag, the title / PR-metadata stores) authoritative. The bridge is started
/// once for the whole run, including its retries, and torn down on return.
///
/// A [`ChunkOutcome::Limited`] retries the *same* model with exponential
/// backoff ([`crate::step::command::calculate_backoff`]: 2s doubling to a 60s
/// cap), mirroring the command backend's rate-limit handling, up to
/// `req.max_retries` attempts.
///
/// Retries deliberately start from `req.resume` (the caller's session), not the
/// aborted attempt's session id: re-sending the same prompt into a
/// partially-answered session would duplicate context.
async fn run_jcode(req: PromptRun<'_>) -> Result<PromptOutcome> {
    let on_delta = req.stream.and_then(|s| s.on_stdout);
    let home = jcode::preflight(None, req.working_dir, req.env, req.on_notice)?;
    let bridge = ToolBridge::start(req.tools.clone())?;
    let (provider, model, effort) = jcode::parse_model_ref(req.model_or_mode)?;

    let mut attempts = 0;
    loop {
        let config = JcodeRunnerConfig {
            model: model.clone(),
            provider: provider.clone(),
            effort,
            cwd: req.working_dir.map(Path::to_path_buf),
            resume_session_id: req.resume.clone(),
            home: home.clone(),
            tool_socket: bridge.socket_path().to_path_buf(),
            env: req.env.clone(),
            cancel: req.cancel_token.cloned(),
            binary: None,
        };
        let rx_std = stream_jcode_agent(config, req.prompt.to_string());
        let outcome = stream_to_outcome(rx_std, on_delta, req.cancel_token, None).await?;

        match outcome {
            ChunkOutcome::Done { output, session } => {
                return Ok(PromptOutcome {
                    result: PromptResult {
                        output,
                        stderr: String::new(),
                    },
                    session_id: session,
                });
            }
            ChunkOutcome::Failed { message, .. } => return Err(CruiseError::CommandError(message)),
            ChunkOutcome::Limited { message, .. } => {
                if attempts < req.max_retries {
                    attempts += 1;
                    let delay = crate::step::command::calculate_backoff(attempts);
                    if let Some(cb) = req.on_notice {
                        cb(&format!(
                            "Rate limit detected. Retrying in {:.1}s... ({attempts}/{})",
                            delay.as_secs_f64(),
                            req.max_retries
                        ));
                    }
                    tokio::select! {
                        biased;
                        () = maybe_cancelled(req.cancel_token) => return Err(CruiseError::Interrupted),
                        () = tokio::time::sleep(delay) => {}
                    }
                    continue;
                }
                return Err(CruiseError::CommandError(message));
            }
            ChunkOutcome::Closed { .. } => {
                return Err(CruiseError::Other(
                    "jcode stream closed before completion".to_string(),
                ));
            }
        }
    }
}

/// Build [`PiRunnerOptions`] straight from `req`, with no seher provider
/// resolution involved. `model_ref` is `req.model_or_mode` — a raw model
/// reference in `Pi` mode (see [`Executor::step_model_or_mode`] /
/// [`Executor::plan_model_or_mode`]), parsed by [`parse_pi_model_ref`].
///
/// `api_key` is always left `None`, deferring key resolution to pi's own
/// precedence chain: an explicit key argument (not offered here) wins, then
/// pi's `~/.pi/agent/auth.json` OAuth/Bearer credentials, *then* ambient
/// environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.) — pi
/// prefers stored credentials over env vars so a stale shell key never
/// silently overrides a successful `pi login`. `req.env` is still forwarded
/// via [`PiRunnerOptions::env`] and is visible to that env-var fallback.
fn build_pi_options(req: &PromptRun<'_>, model_ref: Option<&str>) -> Result<PiRunnerOptions> {
    let (provider, model, thinking) = parse_pi_model_ref(model_ref)?;
    Ok(PiRunnerOptions {
        provider,
        model,
        api_key: None,
        thinking,
        system_prompt: None,
        working_directory: req.working_dir.map(Path::to_path_buf),
        env: req
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        tools: seher_tools(&req.tools),
    })
}

/// Split a `sdk: pi` model reference into the `(provider, model, thinking)`
/// triple expected by [`PiRunnerOptions`].
///
/// Cruise's `model` / `plan_model` / per-step `model` are passed straight
/// through in `Pi` mode instead of being reinterpreted as a seher mode key.
/// Accepted forms:
///
/// - `None` / empty -> `(None, None, None)`. Both provider and model are left
///   unset so pi's own auto-selection picks one: it tries
///   `PROVIDER_DEFAULT_MODELS` in order (Codex, then `OpenAI`, ... down to
///   Anthropic) against whichever credentials/env vars are actually
///   configured, exactly like running the `pi` CLI with neither `--provider`
///   nor `--model`.
/// - `"model"` (no `/`) -> `(None, Some("model"), thinking)`. Provider is left
///   unset; pi resolves it by searching its model registry for a model with
///   this id, mirroring `pi --model X` with no `--provider`.
/// - `"provider/model"` -> `(Some("provider"), Some("model"), thinking)`.
/// - `":thinking"` alone -> `(None, None, Some(thinking))` — auto-selected
///   model with an explicit thinking level.
/// - A `/` with an empty provider or model (`"/model"`, `"provider/"`) is a
///   configuration error: passing it through would surface as an opaque model
///   registry miss inside pi instead of a clear cruise-side message.
///
/// A trailing `:thinking` suffix is recognized only when it parses as a pi
/// thinking level (see [`split_thinking_suffix`]); any other `:` suffix (e.g.
/// an `OpenRouter` `:free` variant) stays part of the model id.
fn parse_pi_model_ref(
    model_ref: Option<&str>,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let Some(raw) = model_ref.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((None, None, None));
    };
    let (without_thinking, thinking) = split_thinking_suffix(raw);
    if without_thinking.is_empty() {
        return Ok((None, None, thinking.map(str::to_string)));
    }
    match without_thinking.split_once('/') {
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => Ok((
            Some(provider.to_string()),
            Some(model.to_string()),
            thinking.map(str::to_string),
        )),
        Some(_) => Err(CruiseError::Other(format!(
            "invalid pi model reference '{raw}': provider and model must both be non-empty \
             around '/' (expected \"provider/model[:thinking]\", \"model[:thinking]\", or \
             empty for auto-selection)"
        ))),
        None => Ok((
            None,
            Some(without_thinking.to_string()),
            thinking.map(str::to_string),
        )),
    }
}

/// Resolve the seher `mode_key` for an ordinary prompt step.
///
/// Precedence mirrors the command-mode model resolution
/// (`step.model.or(global.model)`): a per-step value wins over the workflow
/// default. When neither is set the step runs under [`DEFAULT_STEP_MODE_KEY`].
#[must_use]
pub fn mode_key_for_step(step_model: Option<&str>, global_model: Option<&str>) -> String {
    step_model
        .or(global_model)
        .unwrap_or(DEFAULT_STEP_MODE_KEY)
        .to_string()
}

/// Resolve the seher `mode_key` for the built-in planning step.
///
/// Precedence mirrors command-mode plan-model resolution
/// (`plan_model.or(model)`): the dedicated `plan_model` wins, falling back to the
/// workflow `model`, then to [`DEFAULT_PLAN_MODE_KEY`].
#[must_use]
pub fn mode_key_for_plan(plan_model: Option<&str>, global_model: Option<&str>) -> String {
    plan_model
        .or(global_model)
        .unwrap_or(DEFAULT_PLAN_MODE_KEY)
        .to_string()
}

/// The cruise-side reasoning effort for a claude-family model id: the effort
/// seher resolved from its own config if it set one, else the tier named by the
/// model id's `:suffix`.
fn claude_family_effort(
    model_id: &str,
    resolved_effort: Option<SeherEffortLevel>,
) -> Option<EffortLevel> {
    let (_, suffix_thinking) = split_thinking_suffix(model_id);
    resolved_effort
        .map(Into::into)
        .or_else(|| suffix_thinking.and_then(effort_from_suffix))
}

fn claude_family_model(model_id: &str) -> Option<String> {
    let (model, _) = split_thinking_suffix(model_id);
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

/// The model id and reasoning effort the resolved provider will actually run
/// with. Claude-family SDKs take a plain `--model` name plus a separate effort.
/// Pi-family SDKs split the model reference and derive effort from its suffix.
fn effective_model(resolved: &seher::sdk::ResolvedAgent) -> (Cow<'_, str>, Option<EffortLevel>) {
    if matches!(
        resolved.sdk.as_str(),
        "claude" | "claude-headless" | "claude-terminal"
    ) {
        let model =
            claude_family_model(&resolved.model_id).map_or(Cow::Borrowed("default"), Cow::Owned);
        (
            model,
            claude_family_effort(&resolved.model_id, resolved.effort),
        )
    } else {
        let (_, model, suffix) =
            seher::sdk::split_model_ref(&resolved.provider, &resolved.model_id);
        (
            Cow::Owned(model),
            resolved
                .effort
                .map(Into::into)
                .or_else(|| suffix.as_deref().and_then(effort_from_suffix)),
        )
    }
}

fn resolving_notice(mode_key: &str, require_tools: bool) -> String {
    let tools_note = if require_tools {
        " (tool-capable providers only)"
    } else {
        ""
    };
    format!("seher: resolving provider for mode \"{mode_key}\"{tools_note}")
}

fn rate_limited_notice(
    resolved: &seher::sdk::ResolvedAgent,
    attempts: usize,
    max_retries: usize,
) -> String {
    let (model, _) = effective_model(resolved);
    format!(
        "seher: provider={} model={model} rate-limited; re-resolving... ({attempts}/{max_retries})",
        resolved.provider
    )
}

/// Notice describing which seher provider/model a prompt is about to run on.
/// seher logs nothing itself, so [`PromptRun::on_notice`] is the only place this
/// can surface.
fn resolution_notice(resolved: &seher::sdk::ResolvedAgent) -> String {
    let (model, effort) = effective_model(resolved);
    let mut msg = format!(
        "seher: selected provider={} model={model} sdk={} mode={}",
        resolved.provider, resolved.sdk, resolved.mode_key
    );
    if let Some(effort) = effort {
        msg.push_str(" effort=");
        msg.push_str(effort.as_str());
    }
    msg
}

#[test]
fn resolution_notice_reports_provider_model_sdk_mode_and_effort() {
    let resolved = seher::sdk::ResolvedAgent {
        provider: "codex".to_string(),
        model_id: "openai-codex/gpt-5.6-luna:high".to_string(),
        mode_key: "build".to_string(),
        sdk: "pi".to_string(),
        api: None,
        skills: seher::sdk::ResolvedSkillsConfig::default(),
        retry: seher::sdk::RetryConfig::default(),
        env: indexmap::IndexMap::default(),
        effort: Some(SeherEffortLevel::High),
    };
    assert_eq!(
        resolution_notice(&resolved),
        "seher: selected provider=codex model=gpt-5.6-luna sdk=pi mode=build effort=high"
    );
}

/// The claude-family SDKs get a plain `--model` plus a separate effort, so
/// the notice must report what is really dispatched, not the raw config id.
#[test]
fn resolution_notice_splits_thinking_suffix_for_claude_family() {
    let resolved = seher::sdk::ResolvedAgent {
        provider: "claude".to_string(),
        model_id: "claude-sonnet-4-6:xhigh".to_string(),
        mode_key: "plan".to_string(),
        sdk: "claude-terminal".to_string(),
        api: None,
        skills: seher::sdk::ResolvedSkillsConfig::default(),
        retry: seher::sdk::RetryConfig::default(),
        env: indexmap::IndexMap::default(),
        effort: None,
    };
    assert_eq!(
        resolution_notice(&resolved),
        "seher: selected provider=claude model=claude-sonnet-4-6 sdk=claude-terminal mode=plan effort=xhigh"
    );
}

#[test]
fn resolution_notice_reports_claude_default_model() {
    let resolved = seher::sdk::ResolvedAgent {
        provider: "claude".to_string(),
        model_id: ":high".to_string(),
        mode_key: "build".to_string(),
        sdk: "claude-terminal".to_string(),
        api: None,
        skills: seher::sdk::ResolvedSkillsConfig::default(),
        retry: seher::sdk::RetryConfig::default(),
        env: indexmap::IndexMap::default(),
        effort: None,
    };
    assert_eq!(
        resolution_notice(&resolved),
        "seher: selected provider=claude model=default sdk=claude-terminal mode=build effort=high"
    );
}

#[test]
fn resolving_notice_reports_tool_requirement() {
    assert_eq!(
        resolving_notice("build", false),
        "seher: resolving provider for mode \"build\""
    );
    assert_eq!(
        resolving_notice("build", true),
        "seher: resolving provider for mode \"build\" (tool-capable providers only)"
    );
}

#[test]
fn rate_limited_notice_reports_effective_model() {
    let resolved = seher::sdk::ResolvedAgent {
        provider: "codex".to_string(),
        model_id: "openai-codex/gpt-5.6-luna:high".to_string(),
        mode_key: "build".to_string(),
        sdk: "pi".to_string(),
        api: None,
        skills: seher::sdk::ResolvedSkillsConfig::default(),
        retry: seher::sdk::RetryConfig::default(),
        env: indexmap::IndexMap::default(),
        effort: None,
    };
    assert_eq!(
        rate_limited_notice(&resolved, 1, 2),
        "seher: provider=codex model=gpt-5.6-luna rate-limited; re-resolving... (1/2)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_mode_key_prefers_step_over_global() {
        assert_eq!(mode_key_for_step(Some("fast"), Some("build")), "fast");
    }

    #[test]
    fn step_mode_key_falls_back_to_global() {
        assert_eq!(mode_key_for_step(None, Some("build")), "build");
    }

    #[test]
    fn step_mode_key_defaults_to_build() {
        assert_eq!(mode_key_for_step(None, None), DEFAULT_STEP_MODE_KEY);
        assert_eq!(mode_key_for_step(None, None), "build");
    }

    #[test]
    fn plan_mode_key_prefers_plan_model() {
        assert_eq!(mode_key_for_plan(Some("plan"), Some("build")), "plan");
    }

    #[test]
    fn plan_mode_key_falls_back_to_global_model() {
        assert_eq!(mode_key_for_plan(None, Some("build")), "build");
    }

    #[test]
    fn plan_mode_key_defaults_to_plan() {
        assert_eq!(mode_key_for_plan(None, None), DEFAULT_PLAN_MODE_KEY);
        assert_eq!(mode_key_for_plan(None, None), "plan");
    }

    #[test]
    fn claude_family_model_strips_thinking_suffix() {
        assert_eq!(
            claude_family_model("claude-sonnet-4-5:high").as_deref(),
            Some("claude-sonnet-4-5")
        );
    }

    #[test]
    fn claude_family_effort_uses_suffix_when_unresolved() {
        assert_eq!(
            claude_family_effort("claude-sonnet-4-5:med", None),
            Some(EffortLevel::Medium)
        );
    }

    #[test]
    fn claude_family_effort_prefers_resolved_effort_over_suffix() {
        assert_eq!(
            claude_family_effort("claude-sonnet-4-5:low", Some(SeherEffortLevel::High)),
            Some(EffortLevel::High)
        );
    }

    #[test]
    fn claude_family_effort_omits_off_suffix() {
        assert_eq!(
            claude_family_model("claude-sonnet-4-5:off").as_deref(),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(claude_family_effort("claude-sonnet-4-5:off", None), None);
    }

    // -- Executor dispatch ----------------------------------------------------

    fn sdk_executor() -> Executor {
        Executor::Sdk
    }

    fn command_executor() -> Executor {
        Executor::Command {
            command: vec!["claude".to_string(), "-p".to_string()],
        }
    }

    fn pi_executor() -> Executor {
        Executor::Pi
    }

    fn claude_executor() -> Executor {
        Executor::Claude
    }

    fn jcode_executor() -> Executor {
        Executor::Jcode
    }

    #[test]
    fn new_picks_sdk_when_sdk_set() {
        let e = Executor::new(Some("seher"), &[]);
        assert!(e.is_sdk());
        assert!(matches!(e, Executor::Sdk));
    }

    #[test]
    fn new_picks_sdk_for_any_other_sdk_value() {
        // Any sdk value other than "pi" / "jcode" / "claude" dispatches to
        // Executor::Sdk; rejecting unknown values is validate_sdk's job, not
        // Executor::new's.
        let e = Executor::new(Some("claude-terminal"), &[]);
        assert!(matches!(e, Executor::Sdk));
    }

    #[test]
    fn new_picks_claude_when_sdk_is_claude() {
        let e = Executor::new(Some("claude"), &[]);
        assert!(e.is_sdk());
        assert!(matches!(e, Executor::Claude));
    }

    #[test]
    fn new_picks_pi_when_sdk_is_pi() {
        let e = Executor::new(Some("pi"), &[]);
        assert!(e.is_sdk());
        assert!(matches!(e, Executor::Pi));
    }

    #[test]
    fn new_picks_command_when_sdk_unset() {
        let e = Executor::new(None, &["claude".to_string()]);
        assert!(!e.is_sdk());
    }

    #[test]
    fn command_step_model_passes_through_model_name() {
        let e = command_executor();
        assert_eq!(
            e.step_model_or_mode(Some("sonnet"), Some("opus")),
            Some("sonnet".to_string())
        );
        assert_eq!(e.step_model_or_mode(None, None), None);
    }

    #[test]
    fn sdk_step_model_maps_to_mode_key_with_default() {
        let e = sdk_executor();
        assert_eq!(
            e.step_model_or_mode(Some("fast"), None),
            Some("fast".to_string())
        );
        assert_eq!(e.step_model_or_mode(None, None), Some("build".to_string()));
    }

    #[test]
    fn sdk_plan_model_maps_to_plan_mode_key_with_default() {
        let e = sdk_executor();
        assert_eq!(e.plan_model_or_mode(None, None), Some("plan".to_string()));
        assert_eq!(
            e.plan_model_or_mode(None, Some("build")),
            Some("build".to_string())
        );
    }

    #[test]
    fn pi_step_model_passes_through_model_reference() {
        // Pi mode passes model/mode_or_mode straight through as a raw model
        // reference (never reinterpreted as a mode key).
        let e = pi_executor();
        assert_eq!(
            e.step_model_or_mode(Some("anthropic/claude-sonnet-4-6"), Some("opus")),
            Some("anthropic/claude-sonnet-4-6".to_string())
        );
        assert_eq!(
            e.step_model_or_mode(None, Some("opus")),
            Some("opus".to_string())
        );
        assert_eq!(e.step_model_or_mode(None, None), None);
    }

    #[test]
    fn pi_plan_model_passes_through_model_reference() {
        let e = pi_executor();
        assert_eq!(
            e.plan_model_or_mode(Some("openai/gpt-5.5"), None),
            Some("openai/gpt-5.5".to_string())
        );
        assert_eq!(e.plan_model_or_mode(None, None), None);
    }

    #[test]
    fn claude_model_passes_through_model_reference() {
        // Claude mode passes model_or_mode straight through as a `claude
        // --model` name (never reinterpreted as a mode key).
        let e = claude_executor();
        assert_eq!(
            e.step_model_or_mode(Some("claude-sonnet-4-6:high"), Some("opus")),
            Some("claude-sonnet-4-6:high".to_string())
        );
        assert_eq!(e.step_model_or_mode(None, None), None);
        assert_eq!(
            e.plan_model_or_mode(Some("claude-opus-4-6"), None),
            Some("claude-opus-4-6".to_string())
        );
        assert_eq!(e.plan_model_or_mode(None, None), None);
    }

    #[test]
    fn new_picks_jcode_when_sdk_is_jcode() {
        let e = Executor::new(Some("jcode"), &[]);
        assert!(e.is_sdk());
        assert!(matches!(e, Executor::Jcode));
    }

    #[test]
    fn jcode_model_passes_through_model_reference() {
        // Jcode mode passes model_or_mode straight through as a
        // `provider/model[:effort]` reference in jcode's own namespace (never
        // reinterpreted as a mode key).
        let e = jcode_executor();
        assert_eq!(
            e.step_model_or_mode(Some("claude/claude-opus-5:high"), Some("openai/gpt-5.6")),
            Some("claude/claude-opus-5:high".to_string())
        );
        assert_eq!(e.step_model_or_mode(None, None), None);
        assert_eq!(
            e.plan_model_or_mode(Some("openai/gpt-5.6"), Some("claude/claude-opus-5")),
            Some("openai/gpt-5.6".to_string())
        );
        assert_eq!(e.plan_model_or_mode(None, None), None);
    }

    // -- parse_pi_model_ref / build_pi_options ---------------------------------

    #[test]
    fn parse_pi_model_ref_none_when_unset() {
        assert_eq!(
            parse_pi_model_ref(None).unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (None, None, None)
        );
        assert_eq!(
            parse_pi_model_ref(Some("")).unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (None, None, None)
        );
        assert_eq!(
            parse_pi_model_ref(Some("   ")).unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (None, None, None)
        );
    }

    #[test]
    fn parse_pi_model_ref_bare_model_leaves_provider_unset() {
        assert_eq!(
            parse_pi_model_ref(Some("claude-sonnet-4-6"))
                .unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (None, Some("claude-sonnet-4-6".to_string()), None)
        );
    }

    #[test]
    fn parse_pi_model_ref_splits_provider_and_model() {
        assert_eq!(
            parse_pi_model_ref(Some("anthropic/claude-sonnet-4-6"))
                .unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (
                Some("anthropic".to_string()),
                Some("claude-sonnet-4-6".to_string()),
                None
            )
        );
    }

    #[test]
    fn parse_pi_model_ref_extracts_thinking_suffix() {
        assert_eq!(
            parse_pi_model_ref(Some("openai-codex/gpt-5.5:xhigh"))
                .unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (
                Some("openai-codex".to_string()),
                Some("gpt-5.5".to_string()),
                Some("xhigh".to_string())
            )
        );
    }

    #[test]
    fn parse_pi_model_ref_keeps_non_thinking_colon_suffix_in_model() {
        assert_eq!(
            parse_pi_model_ref(Some("openrouter/meta-llama/llama-3-8b:free"))
                .unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (
                Some("openrouter".to_string()),
                Some("meta-llama/llama-3-8b:free".to_string()),
                None
            )
        );
    }

    #[test]
    fn parse_pi_model_ref_bare_model_with_thinking_suffix() {
        assert_eq!(
            parse_pi_model_ref(Some("claude-sonnet-4-6:high"))
                .unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (
                None,
                Some("claude-sonnet-4-6".to_string()),
                Some("high".to_string())
            )
        );
    }

    #[test]
    fn parse_pi_model_ref_thinking_only_means_auto_model() {
        assert_eq!(
            parse_pi_model_ref(Some(":high")).unwrap_or_else(|e| panic!("unexpected error: {e}")),
            (None, None, Some("high".to_string()))
        );
    }

    #[test]
    fn parse_pi_model_ref_rejects_empty_provider_or_model_around_slash() {
        for bad in ["/claude-sonnet-4-6", "anthropic/", "/", "anthropic/:high"] {
            match parse_pi_model_ref(Some(bad)) {
                Err(err) => assert!(
                    err.to_string().contains("invalid pi model reference"),
                    "unexpected error message for {bad:?}: {err}"
                ),
                Ok(parsed) => panic!("expected parse error for {bad:?}, got {parsed:?}"),
            }
        }
    }
    /// `sdk: seher` must tell the CLI which provider/model seher picked; seher
    /// itself logs nothing, so `on_notice` is the only carrier.
    #[cfg(unix)]
    #[tokio::test]
    async fn sdk_run_reports_resolved_provider_through_on_notice() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = crate::test_support::lock_process();
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let _home_guards = crate::test_support::set_fake_home(dir.path());
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap_or_else(|e| panic!("create bin dir: {e}"));
        let script = bin_dir.join("omp");
        let tool_marker = dir.path().join("tool-registered");
        std::fs::write(
            &script,
            r#"#!/bin/sh
printf '%s\n' '{"type":"ready","protocolVersion":1}'
while IFS= read -r line; do
  case "$line" in
    *get_state*) printf '%s\n' '{"id":"seher-handshake","type":"response","command":"get_state","success":true,"data":{"sessionId":"omp-test-session"}}' ;;
    *set_host_tools*) printf '%s\n' '{"id":"seher-host-tools","type":"response","command":"set_host_tools","success":true,"data":{"toolNames":["echo"]}}'; : > "$OMP_TOOL_MARKER" ;;
    *prompt*) if [ ! -f "$OMP_TOOL_MARKER" ]; then exit 42; fi; printf '%s\n' '{"id":"seher-prompt","type":"response","command":"prompt","success":true}' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"ok"}}' '{"type":"agent_end","isTerminal":true,"messages":[]}' ;;
    *abort*) exit 0 ;;
  esac
done
"#,
        )
        .unwrap_or_else(|e| panic!("write fake OMP: {e}"));
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("chmod fake OMP: {e}"));
        for name in ["bunx", "npx"] {
            let blocker = bin_dir.join(name);
            std::fs::write(&blocker, "#!/bin/sh\nexit 97\n")
                .unwrap_or_else(|e| panic!("write {name} blocker: {e}"));
            std::fs::set_permissions(&blocker, std::fs::Permissions::from_mode(0o700))
                .unwrap_or_else(|e| panic!("chmod {name} blocker: {e}"));
        }
        let codexbar = dir.path().join("codexbar");
        std::fs::write(&codexbar, "#!/bin/sh\nexit 1\n")
            .unwrap_or_else(|e| panic!("write codexbar stub: {e}"));
        std::fs::set_permissions(&codexbar, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("chmod codexbar stub: {e}"));
        let config_path = dir.path().join("seher.yaml");
        std::fs::write(
            &config_path,
            format!(
                "providers:\n  test-provider:\n    sdk: omp\n    env:\n      PATH: {}\n      OMP_TOOL_MARKER: {}\n    models:\n      build: test-provider/test-model\n",
                bin_dir.display(),
                tool_marker.display()
            ),
        )
        .unwrap_or_else(|e| panic!("write seher config: {e}"));
        let _codexbar = crate::test_support::EnvGuard::set("SEHER_CODEXBAR_BIN", &codexbar);
        let _seher_config = crate::test_support::EnvGuard::set("SEHER_CONFIG", &config_path);

        let notices = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let notices_sink = std::sync::Arc::clone(&notices);
        let on_notice = move |msg: &str| {
            notices_sink
                .lock()
                .unwrap_or_else(|e| panic!("notices lock: {e}"))
                .push(msg.to_string());
        };
        let env = HashMap::new();
        let echo = CruiseTool::new(
            "echo",
            "Echo",
            serde_json::json!({"type": "object"}),
            std::sync::Arc::new(|input| Ok(input.to_string())),
        );
        let outcome = Executor::Sdk
            .run(PromptRun {
                prompt: "hello",
                model_or_mode: Some("build"),
                max_retries: 0,
                env: &env,
                on_notice: Some(&on_notice),
                cancel_token: None,
                working_dir: Some(dir.path()),
                stream: None,
                tools: vec![echo],
                resume: None,
            })
            .await
            .unwrap_or_else(|e| panic!("SDK run: {e}"));

        assert_eq!(outcome.result.output, "ok");
        let notices = notices
            .lock()
            .unwrap_or_else(|e| panic!("notices lock: {e}"));
        assert_eq!(
            notices[1],
            "seher: selected provider=test-provider model=test-model sdk=omp mode=build"
        );
    }

    fn base_req(env: &HashMap<String, String>) -> PromptRun<'_> {
        PromptRun {
            prompt: "hi",
            model_or_mode: None,
            max_retries: 0,
            env,
            on_notice: None,
            cancel_token: None,
            working_dir: None,
            stream: None,
            tools: Vec::new(),
            resume: None,
        }
    }

    #[test]
    fn build_pi_options_leaves_api_key_none_for_pi_to_resolve() {
        let env = HashMap::new();
        let req = base_req(&env);
        let opts = build_pi_options(&req, Some("anthropic/claude-sonnet-4-6"))
            .unwrap_or_else(|e| panic!("unexpected error: {e}"));
        assert_eq!(opts.provider.as_deref(), Some("anthropic"));
        assert_eq!(opts.model.as_deref(), Some("claude-sonnet-4-6"));
        assert!(opts.api_key.is_none());
        assert!(opts.thinking.is_none());
    }

    #[test]
    fn build_pi_options_forwards_tools_env_and_working_dir() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let tool = CruiseTool::new(
            "echo",
            "Echo",
            serde_json::json!({"type": "object"}),
            std::sync::Arc::new(|_| Ok(String::new())),
        );
        let dir = std::path::PathBuf::from("/tmp/cruise-pi-test");
        let req = PromptRun {
            prompt: "hi",
            model_or_mode: Some("gpt-5.5"),
            max_retries: 0,
            env: &env,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(&dir),
            stream: None,
            tools: vec![tool],
            resume: Some("sess-1".to_string()),
        };
        let opts = build_pi_options(&req, req.model_or_mode)
            .unwrap_or_else(|e| panic!("unexpected error: {e}"));
        assert_eq!(opts.working_directory, Some(dir));
        assert_eq!(opts.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(opts.tools.len(), 1);
        assert_eq!(opts.tools[0].name, "echo");
    }

    // -- build_claude_config ---------------------------------------------------

    #[test]
    fn build_claude_config_splits_effort_suffix_off_the_model() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model_or_mode = Some("claude-sonnet-4-6:xhigh");
        let config = build_claude_config(&req);
        assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(config.effort, Some(EffortLevel::XHigh));
    }

    #[test]
    fn build_claude_config_leaves_model_unset_so_the_cli_picks_its_default() {
        let env = HashMap::new();
        let config = build_claude_config(&base_req(&env));
        assert_eq!(config.model, None);
        assert_eq!(config.effort, None);
    }

    #[test]
    fn build_claude_config_forwards_tools_env_working_dir_and_resume() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let tool = CruiseTool::new(
            "echo",
            "Echo",
            serde_json::json!({"type": "object"}),
            std::sync::Arc::new(|_| Ok(String::new())),
        );
        let dir = std::path::PathBuf::from("/tmp/cruise-claude-test");
        let req = PromptRun {
            prompt: "hi",
            model_or_mode: Some("claude-opus-4-6"),
            max_retries: 0,
            env: &env,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(&dir),
            stream: None,
            tools: vec![tool],
            resume: Some("sess-1".to_string()),
        };
        let config = build_claude_config(&req);
        assert_eq!(config.cwd, Some(dir));
        assert_eq!(config.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(config.resume_session_id.as_deref(), Some("sess-1"));
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.tools[0].name, "echo");
    }

    #[cfg(unix)]
    #[test]
    fn omp_dispatch_streams_through_cruise_dispatch() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = crate::test_support::lock_process();
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let _home_guards = crate::test_support::set_fake_home(dir.path());
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap_or_else(|e| panic!("create bin dir: {e}"));
        let script = bin_dir.join("omp");
        let tool_marker = dir.path().join("tool-registered");
        std::fs::write(
            &script,
            r#"#!/bin/sh
printf '%s\n' '{"type":"ready","protocolVersion":1}'
while IFS= read -r line; do
  case "$line" in
    *get_state*) printf '{"id":"seher-handshake","type":"response","command":"get_state","success":true,"data":{"sessionId":"omp-test-session"}}\n' ;;
    *set_host_tools*) printf '%s\n' '{"id":"seher-host-tools","type":"response","command":"set_host_tools","success":true,"data":{"toolNames":["echo"]}}'; : > "$OMP_TOOL_MARKER" ;;
    *prompt*) if [ ! -f "$OMP_TOOL_MARKER" ]; then exit 42; fi; printf '%s\n' '{"id":"seher-prompt","type":"response","command":"prompt","success":true}' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"ok"}}' '{"type":"agent_end","isTerminal":true,"messages":[]}' ;;
    *abort*) exit 0 ;;
  esac
done
"#,
        )
        .unwrap_or_else(|e| panic!("write fake OMP: {e}"));
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("chmod fake OMP: {e}"));
        for name in ["bunx", "npx"] {
            let blocker = bin_dir.join(name);
            std::fs::write(&blocker, "#!/bin/sh\nexit 97\n")
                .unwrap_or_else(|e| panic!("write {name} blocker: {e}"));
            std::fs::set_permissions(&blocker, std::fs::Permissions::from_mode(0o700))
                .unwrap_or_else(|e| panic!("chmod {name} blocker: {e}"));
        }

        let env = HashMap::new();
        let echo = CruiseTool::new(
            "echo",
            "Echo",
            serde_json::json!({"type": "object"}),
            std::sync::Arc::new(|input| Ok(input.to_string())),
        );
        let req = PromptRun {
            prompt: "hello",
            model_or_mode: None,
            max_retries: 0,
            env: &env,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(dir.path()),
            stream: None,
            tools: vec![echo],
            resume: None,
        };
        let resolved = seher::sdk::ResolvedAgent {
            provider: "test-provider".to_string(),
            model_id: "test-provider/test-model:high".to_string(),
            mode_key: "build".to_string(),
            sdk: "omp".to_string(),
            api: None,
            skills: seher::sdk::ResolvedSkillsConfig::default(),
            retry: seher::sdk::RetryConfig::default(),
            env: [
                (String::from("PATH"), bin_dir.display().to_string()),
                (
                    String::from("OMP_TOOL_MARKER"),
                    tool_marker.display().to_string(),
                ),
            ]
            .into(),
            effort: None,
        };

        let rx = spawn_agent_stream(&resolved, &req, seher::sdk::CancelToken::new());
        let mut output = String::new();
        let mut session = None;
        loop {
            let chunk = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap_or_else(|e| panic!("OMP stream did not finish: {e}"));
            match chunk {
                SeherStreamChunk::Session(id) => session = Some(id),
                SeherStreamChunk::Delta(delta) => output.push_str(&delta),
                SeherStreamChunk::Done(done) => {
                    if !done.is_empty() {
                        output = done;
                    }
                    break;
                }
                SeherStreamChunk::Error(message) => panic!("unexpected OMP error: {message}"),
                SeherStreamChunk::Limit(_) => panic!("unexpected OMP rate limit"),
            }
        }

        assert_eq!(output, "ok");
        assert_eq!(session.as_deref(), Some("omp-test-session"));
        let session_id = session.unwrap_or_else(|| panic!("OMP session id missing"));
        assert!(finish_sdk_session("omp", Some(dir.path()), Some(session_id.clone())).is_none());
        assert!(!seher::sdk::close_omp_session(
            &session_id,
            Some(dir.path())
        ));
    }

    #[test]
    fn rpc_backend_env_does_not_override_helper_search_path() {
        let _lock = crate::test_support::lock_process();
        let _ambient = crate::test_support::EnvGuard::set("CRUISE_RPC_AMBIENT", "ambient");
        let _pathext = crate::test_support::EnvGuard::remove("PATHEXT");
        let mut resolved = seher::sdk::ResolvedAgent {
            provider: "test-provider".to_string(),
            model_id: "test-provider/test-model".to_string(),
            mode_key: "build".to_string(),
            sdk: "omp".to_string(),
            api: None,
            skills: seher::sdk::ResolvedSkillsConfig::default(),
            retry: seher::sdk::RetryConfig::default(),
            env: [("PATH".to_string(), "/trusted/bin".to_string())].into(),
            effort: None,
        };
        let request_env = [
            ("PATH".to_string(), "/repo/bin".to_string()),
            ("PATHEXT".to_string(), ".COM".to_string()),
            ("PROJECT".to_string(), "cruise".to_string()),
        ]
        .into_iter()
        .collect();

        merge_helper_env(&mut resolved, &request_env);

        assert_eq!(
            resolved.env.get("PATH").map(String::as_str),
            Some("/trusted/bin")
        );
        assert!(!resolved.env.contains_key("PATHEXT"));
        assert_eq!(
            resolved.env.get("PROJECT").map(String::as_str),
            Some("cruise")
        );
        assert_eq!(
            resolved.env.get("CRUISE_RPC_AMBIENT").map(String::as_str),
            Some("ambient")
        );
    }

    /// Provider `sdk: pi` must reach seher's *external* pi CLI over RPC, not the
    /// in-process `pi_agent_rust` engine (which is `sdk: pi-rust` and whose baked
    /// model catalog rejects model ids newer than the crate). The fake `pi` on
    /// `PATH` only answers the RPC protocol, so an in-process run cannot pass.
    #[cfg(unix)]
    #[test]
    fn pi_dispatch_streams_through_external_pi_cli() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = crate::test_support::lock_process();
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let _home_guards = crate::test_support::set_fake_home(dir.path());
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap_or_else(|e| panic!("create bin dir: {e}"));
        let script = bin_dir.join("pi");
        std::fs::write(
            &script,
            r#"#!/bin/sh
sid=
extension=
previous=
for arg in "$@"; do
  if [ "$previous" = "--session-id" ]; then sid="$arg"; fi
  if [ "$previous" = "--extension" ]; then extension="$arg"; fi
  previous="$arg"
done
while IFS= read -r line; do
  case "$line" in
    *get_state*) printf '{"id":"seher-handshake","type":"response","command":"get_state","success":true,"data":{"sessionId":"%s"}}\n' "$sid" ;;
    *prompt*) if [ ! -f "$extension" ] || [ ! -s "$SEHER_PI_TOOL_SPEC" ] || [ -z "$SEHER_PI_BRIDGE_HOST" ] || [ -z "$SEHER_PI_BRIDGE_PORT" ] || [ -z "$SEHER_PI_BRIDGE_TOKEN" ]; then exit 42; fi; printf '%s\n' '{"id":"seher-prompt","type":"response","command":"prompt","success":true}' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"ok"}}' '{"type":"agent_settled"}' ;;
    *abort*) exit 0 ;;
  esac
done
"#,
        )
        .unwrap_or_else(|e| panic!("write fake pi: {e}"));
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("chmod fake pi: {e}"));
        for name in ["bunx", "npx"] {
            let blocker = bin_dir.join(name);
            std::fs::write(&blocker, "#!/bin/sh\nexit 97\n")
                .unwrap_or_else(|e| panic!("write {name} blocker: {e}"));
            std::fs::set_permissions(&blocker, std::fs::Permissions::from_mode(0o700))
                .unwrap_or_else(|e| panic!("chmod {name} blocker: {e}"));
        }

        // A workflow `PATH` must not steer which `pi` / `bunx` / `npx` seher
        // launches: dispatch has to keep `resolved.env`'s PATH (`merge_helper_env`),
        // or the fake pi below becomes unreachable. The non-empty tool list also
        // requires the external Pi extension bridge to be configured.
        let env: HashMap<String, String> =
            [(String::from("PATH"), String::from("/nonexistent/bin"))].into();
        let echo = CruiseTool::new(
            "echo",
            "Echo",
            serde_json::json!({"type": "object"}),
            std::sync::Arc::new(|input| Ok(input.to_string())),
        );
        let req = PromptRun {
            prompt: "hello",
            model_or_mode: None,
            max_retries: 0,
            env: &env,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(dir.path()),
            stream: None,
            tools: vec![echo],
            // A foreign session id must not be forwarded to the RPC backend: its
            // sessions are closed after every prompt, so there is none to resume.
            resume: Some("foreign-session".to_string()),
        };
        let resolved = seher::sdk::ResolvedAgent {
            provider: "codex".to_string(),
            // Deliberately newer than `pi_agent_rust`'s baked catalog: the
            // in-process engine rejects it with "Model ... not found".
            model_id: "openai-codex/gpt-5.6-luna:high".to_string(),
            mode_key: "build".to_string(),
            sdk: "pi".to_string(),
            api: None,
            skills: seher::sdk::ResolvedSkillsConfig::default(),
            retry: seher::sdk::RetryConfig::default(),
            env: [(String::from("PATH"), bin_dir.display().to_string())].into(),
            effort: None,
        };

        let rx = spawn_agent_stream(&resolved, &req, seher::sdk::CancelToken::new());
        let mut output = String::new();
        let mut session = None;
        loop {
            let chunk = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("pi stream did not finish: {e}"));
            match chunk {
                SeherStreamChunk::Session(id) => session = Some(id),
                SeherStreamChunk::Delta(delta) => output.push_str(&delta),
                SeherStreamChunk::Done(done) => {
                    if !done.is_empty() {
                        output = done;
                    }
                    break;
                }
                SeherStreamChunk::Error(message) => panic!("unexpected pi error: {message}"),
                SeherStreamChunk::Limit(_) => panic!("unexpected pi rate limit"),
            }
        }

        assert_eq!(output, "ok");
        let session_id = session.unwrap_or_else(|| panic!("pi session id missing"));
        assert_ne!(session_id, "foreign-session");
        // finish_sdk_session must reap the RPC child and drop the id.
        assert!(finish_sdk_session("pi", Some(dir.path()), Some(session_id.clone())).is_none());
        assert!(!seher::sdk::close_pi_session(&session_id, Some(dir.path())));
    }

    #[test]
    fn non_rpc_session_ids_remain_resumable() {
        let session = Some("resumable-session".to_string());
        assert_eq!(finish_sdk_session("claude", None, session.clone()), session);
    }

    #[test]
    fn pi_backoff_matches_command_backoff_schedule() {
        // run_pi_direct reuses step::command::calculate_backoff verbatim for its
        // rate-limit retry delay (no seher provider to fall back to, unlike
        // run_sdk); assert the schedule it inherits.
        use crate::step::command::calculate_backoff;
        assert_eq!(calculate_backoff(1), std::time::Duration::from_secs(2));
        assert_eq!(calculate_backoff(2), std::time::Duration::from_secs(4));
        assert_eq!(calculate_backoff(3), std::time::Duration::from_secs(8));
        assert_eq!(calculate_backoff(10), std::time::Duration::from_mins(1));
    }

    // -- seher adapters -------------------------------------------------------

    #[test]
    fn seher_chunks_map_to_the_matching_cruise_variant() {
        let cases = [
            (
                SeherStreamChunk::Delta("d".to_string()),
                StreamChunk::Delta("d".to_string()),
            ),
            (
                SeherStreamChunk::Done("done".to_string()),
                StreamChunk::Done("done".to_string()),
            ),
            (
                SeherStreamChunk::Session("sid".to_string()),
                StreamChunk::Session("sid".to_string()),
            ),
            (
                SeherStreamChunk::Error("boom".to_string()),
                StreamChunk::Error("boom".to_string()),
            ),
        ];
        for (seher_chunk, expected) in cases {
            assert_eq!(
                format!("{:?}", StreamChunk::from(seher_chunk)),
                format!("{expected:?}")
            );
        }

        // The limit variant carries the provider, and must keep the message the
        // reducer reports verbatim.
        let seher_limit = SeherStreamChunk::Limit(seher::sdk::errors::LimitError {
            provider: "anthropic".to_string(),
            reset_at: None,
        });
        match StreamChunk::from(seher_limit) {
            StreamChunk::Limit(e) => {
                assert_eq!(e.provider, "anthropic");
                assert_eq!(
                    e.to_string(),
                    "Provider 'anthropic' hit API rate/usage limit"
                );
            }
            other => panic!("expected Limit, got {other:?}"),
        }
    }

    #[test]
    fn tool_conversion_preserves_name_description_schema_and_handler() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"question": {"type": "string"}},
            "required": ["question"],
        });
        let calls = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&calls);
        let tool = CruiseTool::new(
            "ask_user",
            "Ask the user a clarifying question.",
            schema.clone(),
            Arc::new(move |input: serde_json::Value| {
                flag.store(true, Ordering::SeqCst);
                Ok(input["question"].as_str().unwrap_or_default().to_string())
            }),
        );

        let converted = SeherTool::from(tool);
        assert_eq!(converted.name, "ask_user");
        assert_eq!(converted.description, "Ask the user a clarifying question.");
        assert_eq!(converted.parameters, schema);
        assert_eq!(
            (converted.handler)(serde_json::json!({"question": "why?"})),
            Ok("why?".to_string())
        );
        assert!(
            calls.load(Ordering::SeqCst),
            "handler was not the shared Arc"
        );
    }
}
