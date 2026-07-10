//! Prompt-execution backend abstraction.
//!
//! Cruise drives prompts through one of three backends: an external `command`
//! (the classic `claude -p` path), the in-process **seher SDK**
//! (`sdk: seher`), or **pi directly** (`sdk: pi`). [`Executor`] hides that
//! choice behind a single [`Executor::run`] call so that `planning.rs`,
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

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use seher::sdk::{
    CodexBarProbe, EffortLevel, PiRunner, PiRunnerOptions, PollOptions, SeherTool, StreamChunk,
    poll_for_agent, split_thinking_suffix,
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
/// Built by the caller and handed to [`Executor::run`]. `model_or_mode` carries
/// a model name in command mode, a seher `mode_key` in `sdk: seher` mode, and a
/// raw model reference in `sdk: pi` mode (compute it with
/// [`Executor::step_model_or_mode`] / [`Executor::plan_model_or_mode`]). `tools`
/// and `resume` are honored only by the two SDK backends.
pub struct PromptRun<'a> {
    /// The fully-resolved prompt text to send.
    pub prompt: &'a str,
    /// Model name (command mode), `mode_key` (`sdk: seher`), or model
    /// reference (`sdk: pi`).
    pub model_or_mode: Option<&'a str>,
    /// Maximum rate-limit retries.
    pub max_retries: usize,
    /// Environment variables applied to the prompt run.
    ///
    /// Command mode passes these to the spawned process. `sdk: seher` forwards
    /// them to the selected seher backend; its in-process pi path applies them
    /// via process environment mutation inside seher. `sdk: pi` applies them the
    /// same way, directly (see [`PiRunnerOptions::env`]).
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
/// The SDK backends' tools (which capture the [`AskHandler`]) are built by the
/// caller and passed via [`PromptRun::tools`], so the executor itself holds no
/// handler.
pub enum Executor {
    /// Spawn an external command (the classic `claude -p` path).
    Command { command: Vec<String> },
    /// Drive prompts through one of the seher SDKs (`pi`, `claude`,
    /// `claude-terminal`, `claude-headless`); the concrete backend is picked
    /// by [`spawn_agent_stream`] from the resolved provider's `sdk` field.
    /// Selected by `sdk: seher` (or any `sdk:` value other than `"pi"`).
    Sdk,
    /// Drive prompts through `pi_agent_rust` directly (`sdk: pi`), bypassing
    /// seher's provider-resolution layer and `~/.config/seher/config.yaml`
    /// entirely. See [`run_pi_direct`].
    Pi,
}

impl Executor {
    /// Build an executor from the workflow's backend selection.
    ///
    /// `sdk: pi` -> [`Executor::Pi`]; any other `sdk` value -> [`Executor::Sdk`];
    /// no `sdk` -> [`Executor::Command`] wrapping `command`. (Mutual exclusivity
    /// between `sdk` and `command`, and that `sdk` is one of the accepted
    /// values, is enforced earlier by [`crate::config::validate_sdk`].)
    #[must_use]
    pub fn new(sdk: Option<&str>, command: &[String]) -> Self {
        match sdk {
            Some("pi") => Executor::Pi,
            Some(_) => Executor::Sdk,
            None => Executor::Command {
                command: command.to_vec(),
            },
        }
    }

    /// Whether this executor drives prompts through an in-process agent
    /// backend (`Sdk` or `Pi`) rather than spawning an external `command`.
    #[must_use]
    pub fn is_sdk(&self) -> bool {
        matches!(self, Executor::Sdk | Executor::Pi)
    }

    /// Resolve the model name (command mode), `mode_key` (`sdk: seher`), or
    /// model reference (`sdk: pi`) for an ordinary prompt step.
    #[must_use]
    pub fn step_model_or_mode(
        &self,
        step_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. } | Executor::Pi => {
                step_model.or(global_model).map(str::to_string)
            }
            Executor::Sdk => Some(mode_key_for_step(step_model, global_model)),
        }
    }

    /// Resolve the model name (command mode), `mode_key` (`sdk: seher`), or
    /// model reference (`sdk: pi`) for the built-in planning step.
    #[must_use]
    pub fn plan_model_or_mode(
        &self,
        plan_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. } | Executor::Pi => {
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
    /// seher provider resolution, the seher SDK run, or the direct pi run
    /// fails.
    pub async fn run(&self, req: PromptRun<'_>) -> Result<PromptOutcome> {
        match self {
            Executor::Command { command } => run_command(command, req).await,
            Executor::Sdk => run_sdk(req).await,
            Executor::Pi => run_pi_direct(req).await,
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
    let resolved = crate::engine::resolve_command_with_model(command, req.model_or_mode)?;
    let model_arg = if resolved.consumed_model_placeholder {
        None
    } else {
        req.model_or_mode.map(str::to_string)
    };
    let resolved_command = resolved.command;

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
                model: claude_family_model(&resolved.model_id),
                effort: claude_family_effort(&resolved.model_id, resolved.effort),
                cwd: cwd_string,
                resume_session_id: req.resume.clone(),
                tools: req.tools.clone(),
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
            headless_cfg.effort = claude_family_effort(&resolved.model_id, resolved.effort);
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
                claude_family_effort(&resolved.model_id, resolved.effort),
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
        "pi" => {
            // `resolved.model_id` is a full pi model ref ("<pi-provider>/<model>[:thinking]",
            // e.g. "openai-codex/gpt-5.5:xhigh") while `resolved.provider` is the seher
            // config label (e.g. "codex"), which pi does not know about. Split the ref
            // so pi receives its own provider / model / thinking parts.
            let (provider, model, thinking) =
                split_model_ref(&resolved.provider, &resolved.model_id, resolved.effort);
            let opts = PiRunnerOptions {
                provider: Some(provider),
                model: Some(model),
                api_key: resolved.api.as_ref().and_then(|a| a.key.clone()),
                thinking,
                system_prompt: None,
                working_directory: req.working_dir.map(Path::to_path_buf),
                env: req
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
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
        let outcome = stream_to_outcome(rx_std, on_delta, req.cancel_token).await?;

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

/// Bridge a blocking `std::sync::mpsc::Receiver<StreamChunk>` (as returned by
/// seher's various `stream_*` functions and [`PiRunner::stream`]) into a
/// [`ChunkOutcome`], forwarding text deltas to `on_delta` line-buffered (pi and
/// seher's backends emit token-level deltas; `StreamCallbacks::on_stdout` is
/// line-oriented like the command backend) and returning `Err(Interrupted)` as
/// soon as `cancel_token` fires.
///
/// Shared by [`run_sdk`] and [`run_pi_direct`] so both backends fold a chunk
/// stream into an outcome identically.
async fn stream_to_outcome(
    rx_std: std::sync::mpsc::Receiver<StreamChunk>,
    on_delta: Option<&(dyn Fn(&str) + Send + Sync)>,
    cancel_token: Option<&CancellationToken>,
) -> Result<ChunkOutcome> {
    // Bridge the blocking std channel to an async one so we can stream deltas
    // through the borrowed `on_delta` callback without moving it onto the
    // backend's worker thread.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamChunk>();
    std::thread::spawn(move || {
        while let Ok(chunk) = rx_std.recv() {
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
// limitation as the seher-resolved pi engine; documented in
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
        let outcome = stream_to_outcome(rx_std, on_delta, req.cancel_token).await?;

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
                    if let Some(cb) = req.on_retry {
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
        tools: req.tools.clone(),
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

fn effort_to_thinking(effort: EffortLevel) -> &'static str {
    match effort {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
        EffortLevel::XHigh | EffortLevel::Max => "xhigh",
    }
}

fn effort_from_suffix(suffix: &str) -> Option<EffortLevel> {
    match suffix.trim().to_lowercase().as_str() {
        "minimal" | "min" | "low" | "1" => Some(EffortLevel::Low),
        "medium" | "med" | "2" => Some(EffortLevel::Medium),
        "high" | "3" => Some(EffortLevel::High),
        "xhigh" | "4" => Some(EffortLevel::XHigh),
        "max" => Some(EffortLevel::Max),
        _ => None,
    }
}

fn claude_family_model(model_id: &str) -> Option<String> {
    let (model, _) = split_thinking_suffix(model_id);
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

fn claude_family_effort(
    model_id: &str,
    resolved_effort: Option<EffortLevel>,
) -> Option<EffortLevel> {
    let (_, suffix_thinking) = split_thinking_suffix(model_id);
    resolved_effort.or_else(|| suffix_thinking.and_then(effort_from_suffix))
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
fn split_model_ref(
    fallback_provider: &str,
    model_id: &str,
    effort: Option<EffortLevel>,
) -> (String, String, Option<String>) {
    let (without_thinking, suffix_thinking) = split_thinking_suffix(model_id);
    let (provider, model) = without_thinking
        .split_once('/')
        .unwrap_or((fallback_provider, without_thinking));
    (
        provider.to_string(),
        model.to_string(),
        effort
            .map(effort_to_thinking)
            .map(str::to_string)
            .or_else(|| suffix_thinking.map(str::to_string)),
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
            split_model_ref("codex", "openai-codex/gpt-5.5:xhigh", None),
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
            split_model_ref("openrouter", "openrouter/moonshotai/kimi-k2.6", None),
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
            split_model_ref("anthropic", "claude-sonnet-4-5", None),
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
            split_model_ref("openrouter", "openrouter/meta-llama/llama-3-8b:free", None),
            (
                "openrouter".to_string(),
                "meta-llama/llama-3-8b:free".to_string(),
                None
            )
        );
    }

    #[test]
    fn split_model_ref_prefers_resolved_effort_over_suffix() {
        assert_eq!(
            split_model_ref("codex", "openai-codex/gpt-5.5:low", Some(EffortLevel::Max)),
            (
                "openai-codex".to_string(),
                "gpt-5.5".to_string(),
                Some("xhigh".to_string())
            )
        );
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
            claude_family_effort("claude-sonnet-4-5:low", Some(EffortLevel::High)),
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

    #[test]
    fn new_picks_sdk_when_sdk_set() {
        let e = Executor::new(Some("seher"), &[]);
        assert!(e.is_sdk());
        assert!(matches!(e, Executor::Sdk));
    }

    #[test]
    fn new_picks_sdk_for_any_non_pi_sdk_value() {
        // Any sdk value other than "pi" dispatches to Executor::Sdk; rejecting
        // unknown values is validate_sdk's job, not Executor::new's.
        let e = Executor::new(Some("claude-terminal"), &[]);
        assert!(matches!(e, Executor::Sdk));
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

    fn base_req(env: &HashMap<String, String>) -> PromptRun<'_> {
        PromptRun {
            prompt: "hi",
            model_or_mode: None,
            max_retries: 0,
            env,
            on_retry: None,
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
        let tool = SeherTool::new(
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
            on_retry: None,
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
