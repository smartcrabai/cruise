//! Prompt-execution backend abstraction.
//!
//! Cruise can drive prompts either through an external `command` (the classic
//! `claude -p` path) or through the in-process **seher SDK**. [`Executor`] hides
//! that choice behind a single [`Executor::run`] call so that `planning.rs`,
//! `engine.rs`, and the GUI command layer don't need to branch on the backend.
//!
//! In SDK mode the cruise `model` / `plan_model` / per-step `model` fields are
//! reinterpreted as seher **mode keys** (see [`mode_key_for_step`] /
//! [`mode_key_for_plan`]).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use seher::sdk::{
    CodexBarProbe, PiRunner, PiRunnerOptions, PollOptions, SeherTool, StreamChunk, poll_for_agent,
    split_thinking_suffix,
};

use crate::cancellation::CancellationToken;
use crate::error::{CruiseError, Result};
use crate::step::prompt::{PromptResult, StreamCallbacks, run_prompt};

/// Default seher mode key for ordinary prompt steps when neither the step nor
/// the workflow specifies one.
pub const DEFAULT_STEP_MODE_KEY: &str = "build";

/// Default seher mode key for the built-in planning step.
pub const DEFAULT_PLAN_MODE_KEY: &str = "plan";

/// Poll interval (ms) used while every seher provider is rate-limited.
const SDK_POLL_INTERVAL_MS: u64 = 60_000;

/// A single prompt execution request, backend-agnostic.
///
/// Built by the caller and handed to [`Executor::run`]. `model_or_mode` carries a
/// model name in command mode and a seher `mode_key` in SDK mode (compute it with
/// [`Executor::step_model_or_mode`] / [`Executor::plan_model_or_mode`]). `tools`
/// and `resume` are honored only by the SDK backend.
pub struct PromptRun<'a> {
    /// The fully-resolved prompt text to send.
    pub prompt: &'a str,
    /// Model name (command mode) or `mode_key` (SDK mode).
    pub model_or_mode: Option<&'a str>,
    /// Maximum rate-limit retries.
    pub max_retries: usize,
    /// Environment variables for the spawned process (command mode).
    pub env: &'a HashMap<String, String>,
    /// Callback invoked with a human-readable message on each rate-limit retry.
    pub on_retry: Option<&'a (dyn Fn(&str) + Send + Sync)>,
    /// Cooperative cancellation token.
    pub cancel_token: Option<&'a CancellationToken>,
    /// Working directory for the command / agent.
    pub working_dir: Option<&'a Path>,
    /// Streaming stdout/stderr callbacks.
    pub stream: Option<&'a StreamCallbacks<'a>>,
    /// Custom tools to inject (SDK mode only).
    pub tools: Vec<SeherTool>,
    /// Prior session id to resume (SDK mode only).
    pub resume: Option<String>,
}

/// Outcome of [`Executor::run`]: the prompt result plus, in SDK mode, the seher
/// session id (for a follow-up `resume`). `session_id` is `None` in command mode.
#[derive(Debug, Clone)]
pub struct PromptOutcome {
    pub result: PromptResult,
    pub session_id: Option<String>,
}

/// Prompt-execution backend.
///
/// The SDK backend's tools (which capture the [`AskHandler`]) are built by the
/// caller and passed via [`PromptRun::tools`], so the executor itself holds no
/// handler.
pub enum Executor {
    /// Spawn an external command (the classic `claude -p` path).
    Command { command: Vec<String> },
    /// Drive prompts through one of the seher SDKs (`pi`, `claude`,
    /// `claude-terminal`, `claude-headless`); the concrete backend is picked
    /// by [`spawn_agent_stream`] from the resolved provider's `sdk` field.
    Sdk,
}

impl Executor {
    /// Build an executor from the workflow's backend selection.
    ///
    /// `sdk` set -> [`Executor::Sdk`]; otherwise [`Executor::Command`] wrapping
    /// `command`. (Mutual exclusivity is enforced earlier by
    /// [`crate::config::validate_sdk`].)
    #[must_use]
    pub fn new(sdk: Option<&str>, command: &[String]) -> Self {
        if sdk.is_some() {
            Executor::Sdk
        } else {
            Executor::Command {
                command: command.to_vec(),
            }
        }
    }

    /// Whether this executor uses the seher SDK backend.
    #[must_use]
    pub fn is_sdk(&self) -> bool {
        matches!(self, Executor::Sdk)
    }

    /// Resolve the model name (command mode) or `mode_key` (SDK mode) for an
    /// ordinary prompt step.
    #[must_use]
    pub fn step_model_or_mode(
        &self,
        step_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. } => step_model.or(global_model).map(str::to_string),
            Executor::Sdk => Some(mode_key_for_step(step_model, global_model)),
        }
    }

    /// Resolve the model name (command mode) or `mode_key` (SDK mode) for the
    /// built-in planning step.
    #[must_use]
    pub fn plan_model_or_mode(
        &self,
        plan_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. } => plan_model.or(global_model).map(str::to_string),
            Executor::Sdk => Some(mode_key_for_plan(plan_model, global_model)),
        }
    }

    /// Execute one prompt on the selected backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to spawn / exits non-zero, or if
    /// seher provider resolution or the SDK run fails.
    pub async fn run(&self, req: PromptRun<'_>) -> Result<PromptOutcome> {
        match self {
            Executor::Command { command } => run_command(command, req).await,
            Executor::Sdk => run_sdk(req).await,
        }
    }
}

/// Resolve a non-rate-limited seher provider for `mode_key`.
///
/// `require_tools` restricts candidates to SDKs that can execute custom tools
/// (`pi` and `claude`); with `false`, the tool-incapable SDKs (`claude-terminal`
/// and `claude-headless`) are also eligible and the caller must dispatch on
/// `ResolvedAgent::sdk`.
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
    let has_placeholder = command.iter().any(|s| s.contains("{model}"));
    let (resolved_command, model_arg) = if has_placeholder {
        (
            crate::engine::resolve_command_with_model(command, req.model_or_mode),
            None,
        )
    } else {
        (command.to_vec(), req.model_or_mode.map(str::to_string))
    };

    let retry = |msg: &str| {
        if let Some(cb) = req.on_retry {
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
/// - `pi` — in-process pi engine. `resolved.model_id` is a full pi model ref
///   (`"<pi-provider>/<model>[:thinking]"`).
/// - `claude` — `claude-agent-sdk` (supports custom tools). `resolved.model_id`
///   is a plain `claude --model` name.
/// - `claude-terminal` — local `claude` CLI via tmux. No tools.
/// - `claude-headless` — `claude -p` subprocess. No tools.
///
/// `seher::sdk::is_supported_sdk` filters the candidate list to exactly these
/// four kinds before resolution, so the match is exhaustive; an unknown kind
/// here indicates the cruise<->seher dispatch mapping has drifted out of sync
/// with the seher version in use and is treated as a bug.
fn spawn_agent_stream(
    resolved: &seher::sdk::ResolvedAgent,
    req: &PromptRun<'_>,
) -> std::sync::mpsc::Receiver<StreamChunk> {
    let cwd_string = req.working_dir.map(|p| p.to_string_lossy().into_owned());
    match resolved.sdk.as_str() {
        "claude" => {
            // claude-agent-sdk supports custom tools natively, so `req.tools`
            // flows straight through. `resolved.model_id` is a plain
            // `claude --model` name, not a pi model ref.
            let config = seher::claude_agent::ClaudeAgentRunnerConfig {
                model: Some(resolved.model_id.clone()),
                cwd: cwd_string,
                resume_session_id: req.resume.clone(),
                tools: req.tools.clone(),
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
            headless_cfg.model = Some(resolved.model_id.clone());
            headless_cfg.cwd = cwd_string;
            headless_cfg.resume_session_id.clone_from(&req.resume);
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
                Some(resolved.model_id.clone()),
                None,
                None,
                cwd_string,
            );
            seher::claude_terminal::stream_via_thread(
                sdk,
                req.prompt.to_string(),
                resolved.provider.clone(),
                req.resume.clone(),
            )
        }
        "pi" => {
            // `resolved.model_id` is a full pi model ref ("<pi-provider>/<model>[:thinking]",
            // e.g. "openai-codex/gpt-5.5:xhigh") while `resolved.provider` is the seher
            // config label (e.g. "codex"), which pi does not know about. Split the ref
            // so pi receives its own provider / model / thinking parts.
            let (provider, model, thinking) =
                split_model_ref(&resolved.provider, &resolved.model_id);
            let opts = PiRunnerOptions {
                provider: Some(provider),
                model: Some(model),
                api_key: resolved.api.as_ref().and_then(|a| a.key.clone()),
                thinking,
                system_prompt: None,
                working_directory: req.working_dir.map(Path::to_path_buf),
                tools: req.tools.clone(),
            };
            PiRunner::new(opts).stream(req.prompt.to_string(), req.resume.clone())
        }
        other => unreachable!(
            "seher resolver returned an unsupported sdk kind: {other:?} \
             (cruise dispatch is out of sync with the seher version in use)"
        ),
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
    // Custom tools only run on tool-capable SDKs (`pi`, `claude`), and `resume`
    // ids belong to whichever SDK started the session — every resumable turn in
    // the planning flow starts with a tool-registering one — so both pin
    // resolution to tool-capable providers. Tool-less fresh runs may also
    // resolve tool-incapable SDKs (`claude-terminal`, `claude-headless`).
    let require_tools = !req.tools.is_empty() || req.resume.is_some();

    let mut attempts = 0;
    loop {
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

        let rx_std = spawn_agent_stream(&resolved, &req);

        // Bridge the blocking std channel to an async one so we can stream
        // deltas through the borrowed `on_delta` callback without moving it
        // onto the seher backend's worker thread.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamChunk>();
        std::thread::spawn(move || {
            while let Ok(chunk) = rx_std.recv() {
                if tx.send(chunk).is_err() {
                    break;
                }
            }
        });

        // pi emits token-level deltas; `StreamCallbacks::on_stdout` is
        // line-oriented (like the command backend), so buffer into whole lines.
        let mut line_buf = LineBuffer::new();
        let mut reducer = ChunkReducer::new();
        let outcome = loop {
            tokio::select! {
                biased;
                () = maybe_cancelled(req.cancel_token) => return Err(CruiseError::Interrupted),
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
                    if let Some(cb) = req.on_retry {
                        cb(&format!(
                            "Provider rate-limited; re-resolving... ({attempts}/{})",
                            req.max_retries
                        ));
                    }
                    continue;
                }
                return Err(CruiseError::CommandError(message));
            }
            ChunkOutcome::Closed { .. } => {
                return Err(CruiseError::Other(
                    "seher stream closed before completion".to_string(),
                ));
            }
        }
    }
}

/// Buffers streamed token fragments and emits them as complete lines, so a
/// line-oriented [`StreamCallbacks::on_stdout`] sees the same shape from the SDK
/// backend as from the command backend.
pub(crate) struct LineBuffer {
    pending: String,
}

impl LineBuffer {
    pub(crate) fn new() -> Self {
        Self {
            pending: String::new(),
        }
    }

    /// Append `frag`, emitting each newly-completed line (without its trailing
    /// `\n`/`\r\n`).
    pub(crate) fn push<F: FnMut(&str)>(&mut self, frag: &str, mut emit: F) {
        self.pending.push_str(frag);
        while let Some(idx) = self.pending.find('\n') {
            let rest = self.pending.split_off(idx + 1);
            let mut line = std::mem::replace(&mut self.pending, rest);
            line.pop(); // drop '\n'
            if line.ends_with('\r') {
                line.pop();
            }
            emit(&line);
        }
    }

    /// Emit any buffered partial line (no trailing newline) and clear.
    pub(crate) fn flush<F: FnMut(&str)>(&mut self, mut emit: F) {
        if !self.pending.is_empty() {
            emit(&self.pending);
            self.pending.clear();
        }
    }
}

/// Terminal (or closed) outcome of folding a stream of [`StreamChunk`]s.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChunkOutcome {
    /// The run completed with the given full text.
    Done {
        output: String,
        session: Option<String>,
    },
    /// The provider reported a rate/usage limit.
    Limited {
        message: String,
        session: Option<String>,
    },
    /// The run failed with a non-limit error.
    Failed {
        message: String,
        session: Option<String>,
    },
    /// The channel closed before any terminal chunk arrived.
    Closed {
        partial: String,
        session: Option<String>,
    },
}

/// Incrementally folds [`StreamChunk`]s into a [`ChunkOutcome`], surfacing text
/// deltas through an `on_delta` sink as they arrive.
pub(crate) struct ChunkReducer {
    buf: String,
    session: Option<String>,
}

impl ChunkReducer {
    pub(crate) fn new() -> Self {
        Self {
            buf: String::new(),
            session: None,
        }
    }

    /// Feed one chunk. Returns `Some(outcome)` when a terminal chunk arrives,
    /// `None` to keep consuming.
    pub(crate) fn step<F: FnMut(&str)>(
        &mut self,
        chunk: StreamChunk,
        on_delta: &mut F,
    ) -> Option<ChunkOutcome> {
        match chunk {
            StreamChunk::Delta(d) => {
                on_delta(&d);
                self.buf.push_str(&d);
                None
            }
            StreamChunk::Session(id) => {
                self.session = Some(id);
                None
            }
            StreamChunk::Done(text) => {
                let output = if text.is_empty() {
                    std::mem::take(&mut self.buf)
                } else {
                    text
                };
                Some(ChunkOutcome::Done {
                    output,
                    session: self.session.take(),
                })
            }
            StreamChunk::Limit(e) => Some(ChunkOutcome::Limited {
                message: e.to_string(),
                session: self.session.take(),
            }),
            StreamChunk::Error(msg) => Some(ChunkOutcome::Failed {
                message: msg,
                session: self.session.take(),
            }),
        }
    }

    /// Produce a [`ChunkOutcome::Closed`] when the stream ends without a terminal
    /// chunk.
    pub(crate) fn finish(&mut self) -> ChunkOutcome {
        ChunkOutcome::Closed {
            partial: std::mem::take(&mut self.buf),
            session: self.session.take(),
        }
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

/// Split a seher model ref into the `(provider, model, thinking)` triple
/// expected by [`PiRunnerOptions`].
///
/// `ResolvedAgent::model_id` carries a full pi model ref
/// (`"<pi-provider>/<model>[:thinking]"`, e.g. `"openai-codex/gpt-5.5:xhigh"`),
/// while `ResolvedAgent::provider` is the seher config label (e.g. `"codex"`),
/// which pi's model registry does not know about. The leading path segment is
/// therefore the pi provider. A ref without `/` carries no provider
/// information, so as a best effort the seher label is passed through as
/// `fallback_provider` — that only resolves when the label happens to equal a
/// pi provider id. The trailing `:level` is split off only when it parses as a
/// pi thinking level (`off`/`low`/`xhigh`/`0`-`4`/…, see
/// [`split_thinking_suffix`]); any other `:` suffix (e.g. `:free`) stays part
/// of the model id.
fn split_model_ref(fallback_provider: &str, model_id: &str) -> (String, String, Option<String>) {
    let (without_thinking, thinking) = split_thinking_suffix(model_id);
    let (provider, model) = without_thinking
        .split_once('/')
        .unwrap_or((fallback_provider, without_thinking));
    (
        provider.to_string(),
        model.to_string(),
        thinking.map(str::to_string),
    )
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

    // -- split_model_ref --------------------------------------------------------

    #[test]
    fn split_model_ref_extracts_provider_and_thinking() {
        assert_eq!(
            split_model_ref("codex", "openai-codex/gpt-5.5:xhigh"),
            (
                "openai-codex".to_string(),
                "gpt-5.5".to_string(),
                Some("xhigh".to_string())
            )
        );
    }

    #[test]
    fn split_model_ref_keeps_slashes_in_model_id() {
        assert_eq!(
            split_model_ref("openrouter", "openrouter/moonshotai/kimi-k2.6"),
            (
                "openrouter".to_string(),
                "moonshotai/kimi-k2.6".to_string(),
                None
            )
        );
    }

    #[test]
    fn split_model_ref_falls_back_to_seher_provider_for_bare_model() {
        assert_eq!(
            split_model_ref("anthropic", "claude-sonnet-4-5"),
            (
                "anthropic".to_string(),
                "claude-sonnet-4-5".to_string(),
                None
            )
        );
    }

    #[test]
    fn split_model_ref_ignores_non_thinking_colon_suffix() {
        assert_eq!(
            split_model_ref("openrouter", "openrouter/meta-llama/llama-3-8b:free"),
            (
                "openrouter".to_string(),
                "meta-llama/llama-3-8b:free".to_string(),
                None
            )
        );
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

    #[test]
    fn new_picks_sdk_when_sdk_set() {
        let e = Executor::new(Some("seher"), &[]);
        assert!(e.is_sdk());
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

    // -- ChunkReducer ---------------------------------------------------------

    fn no_sink() -> impl FnMut(&str) {
        |_: &str| {}
    }

    #[test]
    fn reducer_accumulates_deltas_and_captures_session() {
        let mut r = ChunkReducer::new();
        let mut collected = String::new();
        let mut sink = |d: &str| collected.push_str(d);
        assert_eq!(
            r.step(StreamChunk::Session("sid-1".to_string()), &mut sink),
            None
        );
        assert_eq!(
            r.step(StreamChunk::Delta("Hello ".to_string()), &mut sink),
            None
        );
        assert_eq!(
            r.step(StreamChunk::Delta("world".to_string()), &mut sink),
            None
        );
        let out = r
            .step(StreamChunk::Done(String::new()), &mut sink)
            .unwrap_or_else(|| panic!("expected terminal"));
        assert_eq!(collected, "Hello world");
        assert_eq!(
            out,
            ChunkOutcome::Done {
                output: "Hello world".to_string(),
                session: Some("sid-1".to_string()),
            }
        );
    }

    #[test]
    fn reducer_done_text_overrides_buffered_deltas() {
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        r.step(StreamChunk::Delta("partial".to_string()), &mut sink);
        let out = r
            .step(StreamChunk::Done("FINAL".to_string()), &mut sink)
            .unwrap_or_else(|| panic!("expected terminal"));
        assert_eq!(
            out,
            ChunkOutcome::Done {
                output: "FINAL".to_string(),
                session: None,
            }
        );
    }

    #[test]
    fn reducer_surfaces_error_chunk() {
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        let out = r
            .step(StreamChunk::Error("boom".to_string()), &mut sink)
            .unwrap_or_else(|| panic!("expected terminal"));
        assert_eq!(
            out,
            ChunkOutcome::Failed {
                message: "boom".to_string(),
                session: None,
            }
        );
    }

    #[test]
    fn reducer_surfaces_limit_chunk() {
        use seher::sdk::errors::LimitError;
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        let out = r
            .step(
                StreamChunk::Limit(LimitError {
                    provider: "anthropic".to_string(),
                    reset_at: None,
                }),
                &mut sink,
            )
            .unwrap_or_else(|| panic!("expected terminal"));
        match out {
            ChunkOutcome::Limited { message, .. } => {
                assert!(message.contains("anthropic"), "got: {message}");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // -- LineBuffer -----------------------------------------------------------

    fn collect_lines(frags: &[&str]) -> (Vec<String>, Vec<String>) {
        let mut lb = LineBuffer::new();
        let mut lines = Vec::new();
        for f in frags {
            lb.push(f, |l| lines.push(l.to_string()));
        }
        let mut flushed = Vec::new();
        lb.flush(|l| flushed.push(l.to_string()));
        (lines, flushed)
    }

    #[test]
    fn line_buffer_emits_complete_lines_and_flushes_remainder() {
        let (lines, flushed) = collect_lines(&["Hel", "lo\nwor", "ld"]);
        assert_eq!(lines, vec!["Hello".to_string()]);
        assert_eq!(flushed, vec!["world".to_string()]);
    }

    #[test]
    fn line_buffer_handles_multiple_lines_in_one_fragment() {
        let (lines, flushed) = collect_lines(&["a\nb\nc\n"]);
        assert_eq!(
            lines,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(flushed.is_empty(), "no partial line should remain");
    }

    #[test]
    fn line_buffer_strips_carriage_return() {
        let (lines, _) = collect_lines(&["x\r\n"]);
        assert_eq!(lines, vec!["x".to_string()]);
    }

    #[test]
    fn reducer_finish_reports_closed_with_partial() {
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        r.step(StreamChunk::Delta("half".to_string()), &mut sink);
        assert_eq!(
            r.finish(),
            ChunkOutcome::Closed {
                partial: "half".to_string(),
                session: None,
            }
        );
    }
}
