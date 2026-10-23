//! Prompt-execution backend abstraction.
//!
//! Cruise drives prompts through one of four backends: an external `command`
//! (the classic `claude -p` path), the official **jcode Rust SDK** (`sdk: jcode`,
//! also the default when a workflow names neither `sdk` nor `command`), the
//! **Claude SDK** (`sdk: claude`), or the built-in **`OpenCode` V2 client**
//! (`sdk: opencode`). [`Executor`] hides that choice behind a
//! single [`Executor::run`] call so that `planning.rs`, `engine.rs`, and the
//! `WebUI` command layer don't need to branch on the backend.
//!
//! Every backend reads the cruise `model` / `plan_model` / per-step `model`
//! fields as a plain model reference:
//!
//! - `command` — the model name substituted into the command line.
//! - `sdk: jcode` — a `provider/model[:effort]` reference driven through
//!   `jcode-sdk`; see [`run_sdk`] and [`crate::backend::jcode`].
//! - `sdk: claude` — a plain `claude --model` name with an optional `:effort`
//!   suffix, driven in-process through `claude-agent-sdk` -- see
//!   [`run_sdk`] and [`crate::backend::claude`].
//! - `sdk: opencode` uses `provider/model#variant` references through the
//!   built-in `OpenCode` V2 client, preserving colons in model IDs.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::backend::claude::{ClaudeRunnerConfig, stream_agent as stream_claude_agent};
use crate::backend::effort::{effort_from_suffix, split_thinking_suffix};
use crate::backend::jcode::{self, JcodeRunnerConfig, stream_agent as stream_jcode_agent};
use crate::backend::stream::{ChunkOutcome, ChunkReducer, Folded, LineBuffer, StreamChunk};
use crate::backend::tool::CruiseTool;
use crate::cancellation::CancellationToken;
use crate::config::ModelSpec;
use crate::error::{CruiseError, Result};
use crate::model_selection::{Sdk, SdkSessionId, resolve_candidates};
use crate::retry::{self, Failure, FallbackEngine, ModelReferenceFormat, RetryAction, RetryPolicy};
use crate::step::prompt::{PromptResult, StreamCallbacks, run_prompt};

/// Callback invoked when an SDK backend reports its session identifier.
pub type SessionIdCallback<'a> = dyn Fn(&SdkSessionId) -> Result<()> + Send + Sync + 'a;
type StreamSessionIdCallback<'a> = dyn Fn(&str) -> Result<()> + Send + Sync + 'a;

/// A single prompt execution request, backend-agnostic.
///
/// Built by the caller and handed to [`Executor::run`]. `model` retains the
/// complete candidate list (select it with [`Executor::step_model`] or
/// [`Executor::plan_model`]).
/// `tools` and `resume` are honored by the SDK backends and ignored in command
/// mode.
pub struct PromptRun<'a> {
    /// The fully-resolved prompt text to send.
    pub prompt: &'a str,
    /// Model name (command mode) or model reference (`sdk: jcode`,
    /// `sdk: claude`).
    pub model: Option<&'a ModelSpec>,
    /// Maximum retries per prompt. SDK fallback policies also use this budget
    /// for 4xx (except 429), 5xx and network failures and fallback switching.
    pub max_retries: usize,
    /// Environment variables applied to the prompt run.
    ///
    /// Command mode passes these to the spawned process; `sdk: jcode` and
    /// `sdk: claude` pass them to the `jcode` / `claude` child process.
    pub env: &'a HashMap<String, String>,
    /// MCP servers for the SDK backends; ignored in command mode, which
    /// `validate_mcp_servers` rejects.
    pub mcp_servers: &'a crate::config::McpServers,
    /// Callback invoked with human-readable progress notices: selected models,
    /// rate-limit retries, and model fallbacks.
    pub on_notice: Option<&'a (dyn Fn(&str) + Send + Sync)>,
    /// Cooperative cancellation token.
    pub cancel_token: Option<&'a CancellationToken>,
    /// Working directory for the command / agent.
    pub working_dir: Option<&'a Path>,
    /// Streaming stdout/stderr callbacks.
    pub stream: Option<&'a StreamCallbacks<'a>>,
    /// Custom tools to inject (SDK mode only).
    pub tools: Vec<CruiseTool>,
    /// Called as soon as an SDK backend reports its session ID, before tools
    /// can block waiting for user input.
    pub on_session_id: Option<&'a SessionIdCallback<'a>>,
    /// Prior session id to resume (SDK mode only).
    pub resume: Option<SdkSessionId>,
    /// Allow jcode's macOS desktop-control tool for this turn. This is jcode
    /// backend only and is validated by [`crate::config::validate_computer_use`].
    pub computer_use: bool,
    /// Agent permission mode for this run.
    pub permission: crate::config::PermissionMode,
}

/// Outcome of [`Executor::run`]: the prompt result plus, in SDK mode, the
/// backend session id (for a follow-up `resume`). `session_id` is `None` in
/// command mode.
#[derive(Debug, Clone)]
pub struct PromptOutcome {
    pub result: PromptResult,
    pub session_id: Option<SdkSessionId>,
}

fn resume_for(req: &PromptRun<'_>, sdk: Sdk) -> Option<String> {
    req.resume
        .as_ref()
        .filter(|session| session.sdk == sdk)
        .map(|session| session.id.clone())
}

fn transition_notice(from_sdk: Sdk, from: Option<&str>, to_sdk: Sdk, to: &str) -> String {
    let from = from.unwrap_or("default model");
    if from_sdk == to_sdk {
        format!("{from} -> {to}")
    } else {
        format!("{from_sdk}:{from} -> {to_sdk}:{to}")
    }
}

fn validate_candidate(req: &PromptRun<'_>, sdk: Sdk) -> Result<()> {
    if req.computer_use && sdk == Sdk::Claude {
        return Err(CruiseError::InvalidStepConfig("Claude Code's computer use is only available in interactive sessions and cannot be enabled by cruise".to_owned()));
    }
    if req.computer_use && sdk != Sdk::Jcode {
        return Err(CruiseError::InvalidStepConfig(format!(
            "computer_use requires the jcode backend and is not supported by {sdk}"
        )));
    }
    if sdk == Sdk::Jcode
        && req
            .mcp_servers
            .values()
            .any(|server| server.transport() != crate::config::McpTransport::Stdio)
    {
        return Err(CruiseError::InvalidStepConfig(
            "jcode MCP servers support only stdio transport, not HTTP or SSE".to_owned(),
        ));
    }
    Ok(())
}

/// Prompt-execution backend.
///
/// The SDK backends' tools (which capture the [`AskHandler`]) are built by the
/// caller and passed via [`PromptRun::tools`], so the executor itself holds no
/// handler.
pub enum Executor {
    /// Spawn an external command (the classic `claude -p` path).
    Command { command: Vec<String> },
    /// Drive `jcode` through the official Rust SDK (`sdk: jcode`) in an
    /// isolated runtime, exposing cruise's tools as session tools. See
    /// [`run_sdk`].
    Jcode,
    /// Drive the `claude` CLI in-process through `claude-agent-sdk`
    /// (`sdk: claude`). See [`run_sdk`].
    Claude,
    /// Drive a locally owned `OpenCode` V2 server through the embedded client.
    OpenCode,
    /// An `sdk:` value cruise does not implement. [`Executor::run`] reports it;
    /// no prompt is executed.
    ///
    /// Unreachable through cruise itself — every config is validated by
    /// [`crate::config::validate_sdk`] before it reaches here — but
    /// [`Executor::new`] is public and infallible, and a removed value like
    /// `sdk: seher` must never be quietly read as some other backend.
    Unsupported { sdk: String },
}

impl Executor {
    fn sdk(&self) -> Sdk {
        match self {
            Self::Claude => Sdk::Claude,
            Self::OpenCode => Sdk::OpenCode,
            _ => Sdk::Jcode,
        }
    }

    /// Select an ordinary step model without losing SDK-qualified fallbacks.
    #[must_use]
    pub fn step_model(&self, step: Option<&str>, global: Option<&ModelSpec>) -> Option<ModelSpec> {
        crate::model_selection::step_model_spec(global, step, self.sdk())
    }

    /// Prefer planning candidates, falling back to the ordinary model list.
    #[must_use]
    pub fn plan_model(plan: Option<&ModelSpec>, global: Option<&ModelSpec>) -> Option<ModelSpec> {
        crate::model_selection::planning_spec(plan, global).cloned()
    }
    /// Build an executor from the workflow's backend selection.
    ///
    /// `sdk: jcode` -> [`Executor::Jcode`]; `sdk: claude` -> [`Executor::Claude`];
    /// `sdk: opencode` -> [`Executor::OpenCode`];
    /// a `command` with no `sdk` -> [`Executor::Command`]; neither named ->
    /// [`Executor::Jcode`], the default backend. Any other `sdk` value ->
    /// [`Executor::Unsupported`], which fails at run time.
    ///
    /// Mutual exclusivity between `sdk` and `command` is enforced earlier by
    /// [`crate::config::validate_sdk`], which also rejects every `sdk` value
    /// that would land in [`Executor::Unsupported`].
    #[must_use]
    pub fn new(sdk: Option<&str>, command: &[String]) -> Self {
        match sdk {
            Some("jcode") => Executor::Jcode,
            Some("claude") => Executor::Claude,
            Some("opencode") => Executor::OpenCode,
            Some(other) => Executor::Unsupported {
                sdk: other.to_string(),
            },
            None if command.is_empty() => Executor::Jcode,
            None => Executor::Command {
                command: command.to_vec(),
            },
        }
    }

    /// Whether this executor drives prompts through an agent backend (`Jcode`,
    /// `Claude`, or `OpenCode`) rather than spawning an external `command`.
    #[must_use]
    pub fn is_sdk(&self) -> bool {
        matches!(
            self,
            Executor::Jcode | Executor::Claude | Executor::OpenCode
        )
    }

    /// Execute one prompt on the selected backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to spawn / exits non-zero, if the
    /// `jcode` run or the `claude` CLI run fails, or if `sdk` named a backend
    /// cruise does not implement. A jcode request with no effective Cruise model
    /// returns [`CruiseError::MissingJcodeModel`] before starting the backend.
    pub async fn run(&self, req: PromptRun<'_>) -> Result<PromptOutcome> {
        const GUARD: &str = "read-only";

        if let Some(model) = req.model {
            model.validate("model")?;
            if matches!(self, Self::Command { .. }) && model.has_sdk_overrides() {
                return Err(CruiseError::InvalidStepConfig(
                    "`command` cannot be combined with SDK-qualified model candidates".to_owned(),
                ));
            }
        }

        if self.is_sdk() {
            if req
                .cancel_token
                .is_some_and(CancellationToken::is_cancelled)
            {
                return Err(CruiseError::Interrupted);
            }
            let candidates = resolve_candidates(req.model, self.sdk());
            let primary = &candidates[0];
            if primary.sdk == Sdk::Jcode
                && let Ok((_, model, _)) = jcode::parse_model_ref(primary.model.as_deref())
                && model.is_none()
            {
                return Err(crate::workflow_defaults::missing_jcode_model_error());
            }
            validate_candidate(&req, primary.sdk)?;
        }

        if req.permission != crate::config::PermissionMode::ReadOnly {
            return self.dispatch(req).await;
        }
        // Detection only (not a sandbox or rollback): any change inside the
        // FileTracker scope fails the step, whatever the backend returned.
        let root = req
            .working_dir
            .map_or_else(|| std::path::PathBuf::from("."), Path::to_path_buf);
        let mut tracker = crate::file_tracker::FileTracker::with_root(root);
        tracker.take_snapshot(GUARD)?;
        let result = self.dispatch(req).await;
        if tracker.has_files_changed(GUARD)? {
            return Err(CruiseError::ReadOnlyWorkspaceChanged);
        }
        result
    }

    async fn dispatch(&self, req: PromptRun<'_>) -> Result<PromptOutcome> {
        match self {
            Executor::Command { .. } if req.computer_use => {
                Err(CruiseError::InvalidStepConfig(
                    "computer use requires the jcode backend because a command CLI manages its own tools"
                        .to_string(),
                ))
            }
            Executor::Command { command } => run_command(command, req).await,
            Executor::Jcode | Executor::Claude | Executor::OpenCode => run_sdk(req, self.sdk()).await,
            Executor::Unsupported { sdk } => Err(crate::error::CruiseError::InvalidStepConfig(
                format!("unsupported `sdk` value '{sdk}'"),
            )),
        }
    }
}

/// Resolves when the token is cancelled, or waits forever if no token is given.
async fn maybe_cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(t) => t.cancelled().await,
        None => std::future::pending().await,
    }
}

fn notify_model_notice(
    on_notice: Option<&(dyn Fn(&str) + Send + Sync)>,
    model: Option<&str>,
    backend: &str,
) {
    let Some(cb) = on_notice else {
        return;
    };
    match model.filter(|model| !model.is_empty()) {
        Some(selected) => cb(&format!("Model: {selected}")),
        None => cb(&format!("Model: default model ({backend})")),
    }
}

/// Command-backend execution: resolve the `{model}` placeholder then delegate to
/// the existing [`run_prompt`].
async fn run_command(command: &[String], req: PromptRun<'_>) -> Result<PromptOutcome> {
    let model = req.model.and_then(ModelSpec::primary);
    let resolved = crate::engine::resolve_command_with_model(command, model)?;
    let model_arg = if resolved.consumed_model_placeholder {
        None
    } else {
        model.map(str::to_string)
    };
    let resolved_command = resolved.command;

    notify_model_notice(req.on_notice, model, "command");

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

/// Bridge a blocking `std::sync::mpsc::Receiver` of backend chunks (as returned
/// by [`stream_claude_agent`] and [`stream_jcode_agent`]) into a
/// [`ChunkOutcome`], forwarding text deltas to `on_delta` line-buffered (the
/// SDK backends emit token-level deltas; `StreamCallbacks::on_stdout` is
/// line-oriented like the command backend). Cancellation returns
/// `Err(Interrupted)` only after the backend worker closes its stream following
/// runtime shutdown, so a successor cannot overlap an interrupted attempt.
async fn stream_to_outcome(
    rx_std: std::sync::mpsc::Receiver<StreamChunk>,
    on_delta: Option<&(dyn Fn(&str) + Send + Sync)>,
    on_session_id: Option<&StreamSessionIdCallback<'_>>,
    cancel_token: Option<&CancellationToken>,
) -> Result<Folded> {
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
    // "Streamed" means text actually reached the caller's sink: a turn with no
    // sink shows the user nothing, and a line buffer holds a fragment back
    // until its newline arrives, so neither may block a retry.
    let mut streamed = false;
    let outcome = loop {
        tokio::select! {
            biased;
            () = maybe_cancelled(cancel_token) => {
                // The backend receives the same token. Keep the receiver alive
                // until its worker finishes cleanup; dropping it or returning
                // here would let detached CLI processes outlive this request.
                while rx.recv().await.is_some() {}
                return Err(CruiseError::Interrupted);
            },
            maybe = rx.recv() => match maybe {
                Some(chunk) => {
                    if let StreamChunk::SessionReady { id, ready } = &chunk
                        && let Some(callback) = on_session_id
                        && let Err(error) = callback(id)
                    {
                        let _ = ready.send(false);
                        while rx.recv().await.is_some() {}
                        return Err(error);
                    }
                    if let StreamChunk::Session(id) = &chunk
                        && let Some(callback) = on_session_id
                    {
                        callback(id)?;
                    }
                    let mut sink = |d: &str| {
                        if let Some(cb) = on_delta {
                            line_buf.push(d, |line| {
                                streamed = true;
                                cb(line);
                            });
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
        line_buf.flush(|line| {
            streamed = true;
            cb(line);
        });
    }
    Ok(Folded {
        outcome,
        streamed,
        retry_barrier: reducer.retry_barrier(),
    })
}

/// Build a [`ClaudeRunnerConfig`] straight from `req`. `model_ref` is the model
/// this attempt runs on — the caller's plain `claude --model` name with an
/// optional `:effort` suffix, or a fallback the retry policy switched
/// to. Unset leaves `--model` off so the CLI picks its own default.
///
/// Built fresh per attempt because [`crate::backend::claude::stream_agent`]
/// consumes the config.
fn build_claude_config(req: &PromptRun<'_>, model_ref: Option<&str>) -> ClaudeRunnerConfig {
    let (model, suffix) = split_thinking_suffix(model_ref.unwrap_or_default());
    ClaudeRunnerConfig {
        model: (!model.is_empty()).then(|| model.to_string()),
        effort: suffix.and_then(effort_from_suffix),
        cwd: req.working_dir.map(Path::to_path_buf),
        resume_session_id: resume_for(req, Sdk::Claude),
        tools: req.tools.clone(),
        env: req.env.clone(),
        mcp_servers: req.mcp_servers.clone(),
        cancel: req.cancel_token.cloned(),
        cli_path: None,
        permission: req.permission,
    }
}

/// How one attempt of [`run_sdk_with_fallback`] failed. Keeps the failure text
/// owned, so the borrowed [`Failure`] handed to the engine can be taken after
/// the attempt's outcome is destructured, and keeps the original error of a
/// model reference the backend refused, to surface unchanged if no fallback
/// remains.
enum AttemptFailure {
    Limited { message: String, streamed: bool },
    Failed { message: String, streamed: bool },
    Unusable { error: CruiseError, message: String },
}

impl AttemptFailure {
    /// The failure as the engine sees it, plus whether the turn's text reached
    /// the user.
    fn failure(&self) -> (Failure<'_>, bool) {
        match self {
            AttemptFailure::Limited { message, streamed } => (Failure::Limited(message), *streamed),
            AttemptFailure::Failed { message, streamed } => (Failure::Failed(message), *streamed),
            // Nothing was sent, so nothing was streamed.
            AttemptFailure::Unusable { message, .. } => (Failure::Unusable(message), false),
        }
    }

    /// The original failure detail, shared with the final error.
    fn message(&self) -> &str {
        match self {
            AttemptFailure::Limited { message, .. }
            | AttemptFailure::Failed { message, .. }
            | AttemptFailure::Unusable { message, .. } => message,
        }
    }

    /// The error to surface once the engine gives up.
    fn into_error(self) -> CruiseError {
        match self {
            AttemptFailure::Limited { message, .. } | AttemptFailure::Failed { message, .. } => {
                CruiseError::CommandError(message)
            }
            AttemptFailure::Unusable { error, .. } => error,
        }
    }
}
/// Run one prompt on an SDK backend, retrying through
/// [`crate::retry::FallbackEngine`].
///
/// `start` opens a stream for the model reference the engine picked; it is
/// called once per attempt, always against a *fresh* backend session (`start`
/// builds from `req.resume`, never from the aborted attempt's session id), so a
/// retry never re-sends the prompt into a partially-answered session. A `start`
/// that rejects the reference itself (an unparseable `provider/model[:effort]`)
/// is a notice and a move to the next chain entry, not a failed run.
///
/// Without an explicit workflow `retry:` policy or a model-array policy, this
/// is exactly the historical loop: only a backend-reported rate limit retries,
/// on the same model, on
/// [`crate::step::command::calculate_backoff`]'s 2s-doubling schedule, up to
/// `req.max_retries` times.
///
/// Under a policy, a 4xx other than 429 never re-sends the same request to the
/// same model: it switches to the next fallback entry, or surfaces unchanged.
#[cfg(test)]
async fn run_with_fallback(
    req: &PromptRun<'_>,
    label: &str,
    format: ModelReferenceFormat,
    policy: Option<Arc<RetryPolicy>>,
    mut start: impl FnMut(Option<&str>) -> Result<std::sync::mpsc::Receiver<StreamChunk>>,
) -> Result<PromptOutcome> {
    run_sdk_with_fallback(
        req,
        Sdk::from_name(label).unwrap_or(Sdk::Jcode),
        format,
        policy,
        |_, model| start(model),
    )
    .await
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep attempt completion and retry decisions in one lifecycle loop"
)]
async fn run_sdk_with_fallback(
    req: &PromptRun<'_>,
    base_sdk: Sdk,
    format: ModelReferenceFormat,
    policy: Option<Arc<RetryPolicy>>,
    mut start: impl FnMut(Sdk, Option<&str>) -> Result<std::sync::mpsc::Receiver<StreamChunk>>,
) -> Result<PromptOutcome> {
    let on_delta = req.stream.and_then(|s| s.on_stdout);
    let candidates = resolve_candidates(req.model, base_sdk);
    let primary = &candidates[0];
    let effective = retry::effective_candidates(
        policy.as_deref().map(retry::RetryPolicy::config),
        &candidates,
        base_sdk,
    );
    for candidate in &effective {
        validate_candidate(req, candidate.sdk)?;
    }
    let mut engine = FallbackEngine::new_candidates(
        format,
        policy,
        primary.sdk,
        base_sdk,
        primary.model.as_deref(),
        &candidates[1..],
        req.max_retries,
    );
    for sdk in engine.candidate_sdks() {
        validate_candidate(req, sdk)?;
    }
    let mut model_reported = false;
    if engine.startup_blocked() {
        return Err(CruiseError::Other(
            "all configured models are cooling down after retryable failures".to_string(),
        ));
    }
    if let Some((from, to)) = engine.take_startup_switch()
        && let Some(cb) = req.on_notice
    {
        let transition = transition_notice(primary.sdk, Some(&from), engine.sdk(), &to);
        cb(&format!(
            "Warning: Fallback: {transition} (still cooling down from an earlier failure)"
        ));
    }

    loop {
        let sdk = engine.sdk();
        let label = sdk.as_str();
        validate_candidate(req, sdk)?;
        let model = engine.model().map(str::to_string);
        let failed = match start(sdk, model.as_deref()) {
            Err(error) => AttemptFailure::Unusable {
                message: error.to_string(),
                error,
            },
            Ok(rx_std) => {
                if !model_reported {
                    notify_model_notice(req.on_notice, model.as_deref(), label);
                    model_reported = true;
                }
                let session_callback = |id: &str| {
                    req.on_session_id.map_or(Ok(()), |callback| {
                        callback(&SdkSessionId {
                            sdk,
                            id: id.to_owned(),
                        })
                    })
                };
                let folded =
                    stream_to_outcome(rx_std, on_delta, Some(&session_callback), req.cancel_token)
                        .await?;
                let streamed = folded.streamed;
                if folded.retry_barrier {
                    match &folded.outcome {
                        ChunkOutcome::Limited { message, .. }
                        | ChunkOutcome::Failed { message, .. } => {
                            return Err(CruiseError::CommandError(message.clone()));
                        }
                        _ => {}
                    }
                }
                match folded.outcome {
                    ChunkOutcome::Done { output, session } => {
                        return Ok(PromptOutcome {
                            result: PromptResult {
                                output,
                                stderr: String::new(),
                            },
                            session_id: session.map(|id| SdkSessionId { sdk, id }),
                        });
                    }
                    ChunkOutcome::Closed { .. } => {
                        return Err(CruiseError::Other(format!(
                            "{label} stream closed before completion"
                        )));
                    }
                    ChunkOutcome::Limited { message, .. } => {
                        AttemptFailure::Limited { message, streamed }
                    }
                    ChunkOutcome::Failed { message, .. } => {
                        AttemptFailure::Failed { message, streamed }
                    }
                }
            }
        };

        let (failure, streamed) = failed.failure();
        let action = engine.next(failure, streamed);
        let delay = match action {
            RetryAction::GiveUp => return Err(failed.into_error()),
            RetryAction::Switch {
                from,
                to,
                detail,
                attempt,
                of,
            } => {
                if let Some(cb) = req.on_notice {
                    let transition = transition_notice(sdk, from.as_deref(), engine.sdk(), &to);
                    cb(&format!(
                        "Warning: Fallback: {transition} ({detail}, attempt {attempt}/{of})\n  Error: {}",
                        failed.message(),
                    ));
                }
                model_reported = false;
                // A switch runs immediately; the wait below is still the
                // cancellation checkpoint every retry passes through.
                Duration::ZERO
            }
            RetryAction::Backoff {
                delay,
                class,
                attempt,
                of,
            } => {
                if let Some(cb) = req.on_notice {
                    cb(&format!(
                        "{} detected. Retrying in {:.1}s... ({attempt}/{of})",
                        class.label(),
                        delay.as_secs_f64(),
                    ));
                }
                delay
            }
        };
        tokio::select! {
            biased;
            () = maybe_cancelled(req.cancel_token) => return Err(CruiseError::Interrupted),
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// Cancels the owned attempt when its caller drops the execution future. The
/// worker remains responsible for stopping and reaping its process tree.
struct SdkAttemptGuard(CancellationToken);

impl Drop for SdkAttemptGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn run_sdk(req: PromptRun<'_>, sdk: Sdk) -> Result<PromptOutcome> {
    use crate::backend::opencode::{OpenCodeRunnerConfig, stream_agent};

    let cancel = req
        .cancel_token
        .map_or_else(CancellationToken::new, CancellationToken::child_token);
    if cancel.is_cancelled() {
        return Err(CruiseError::Interrupted);
    }
    let _guard = SdkAttemptGuard(cancel.clone());
    let req = PromptRun {
        cancel_token: Some(&cancel),
        ..req
    };
    run_sdk_with_fallback(
        &req,
        sdk,
        if sdk == Sdk::OpenCode {
            ModelReferenceFormat::OpenCode
        } else {
            ModelReferenceFormat::LegacyEffort
        },
        retry::active_policy(),
        |candidate_sdk, model_ref| match candidate_sdk {
            Sdk::Jcode => Ok(stream_jcode_agent(
                build_jcode_config(&req, model_ref)?,
                req.prompt.to_string(),
            )),
            Sdk::Claude => Ok(stream_claude_agent(
                build_claude_config(&req, model_ref),
                req.prompt.to_string(),
            )),
            Sdk::OpenCode => Ok(stream_agent(
                OpenCodeRunnerConfig {
                    model_ref: model_ref.map(str::to_string),
                    cwd: req.working_dir.map(Path::to_path_buf),
                    resume_session_id: resume_for(&req, Sdk::OpenCode),
                    tools: req.tools.clone(),
                    env: req.env.clone(),
                    mcp_servers: req.mcp_servers.clone(),
                    permission: req.permission,
                    cancel: req.cancel_token.cloned(),
                    keep_session_home: req.resume.is_some()
                        || req.on_session_id.is_some()
                        || req.tools.iter().any(|tool| {
                            matches!(
                                tool.name.as_str(),
                                crate::sdk_tools::SUBMIT_PLAN_TOOL
                                    | crate::sdk_tools::UPDATE_PLAN_TOOL
                            )
                        }),
                    cli_path: None,
                    computer_use: req.computer_use,
                },
                req.prompt.to_string(),
            )),
        },
    )
    .await
}

fn build_jcode_config(req: &PromptRun<'_>, model_ref: Option<&str>) -> Result<JcodeRunnerConfig> {
    let (provider, model, effort) = jcode::parse_model_ref(model_ref)?;
    if model.is_none() {
        return Err(CruiseError::InvalidStepConfig(format!(
            "model reference '{}' for `sdk: jcode` does not identify a model",
            model_ref.unwrap_or("<unset>")
        )));
    }
    Ok(JcodeRunnerConfig {
        model,
        provider,
        effort,
        cwd: req.working_dir.map(Path::to_path_buf),
        resume_session_id: resume_for(req, Sdk::Jcode),
        tools: req.tools.clone(),
        env: req.env.clone(),
        mcp_servers: req.mcp_servers.clone(),
        computer_use: req.computer_use,
        permission: req.permission,
        cancel: req.cancel_token.cloned(),
        keep_session_home: req.resume.is_some()
            || req.on_session_id.is_some()
            || req.tools.iter().any(|tool| {
                matches!(
                    tool.name.as_str(),
                    crate::sdk_tools::SUBMIT_PLAN_TOOL | crate::sdk_tools::UPDATE_PLAN_TOOL
                )
            }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex};

    use crate::backend::effort::EffortLevel;
    use crate::backend::stream::LimitError;
    use crate::test_support::{EnvGuard, lock_process, prepend_to_path, set_fake_home};

    static EMPTY_MCP_SERVERS: LazyLock<crate::config::McpServers> =
        LazyLock::new(crate::config::McpServers::new);

    fn fixture_model(model: Option<&str>) -> Option<&'static ModelSpec> {
        model.map(|model| &*Box::leak(Box::new(ModelSpec::Single(model.to_owned()))))
    }

    #[tokio::test]
    async fn mixed_sdk_attempts_preserve_order_resume_and_checkpoint_owner() {
        let model: ModelSpec = serde_yaml::from_str("[runtime/order-first:high, {sdk: claude, model: runtime/order-middle:medium}, runtime/order-last:low]").unwrap_or_else(|error| panic!("{error}"));
        let env = HashMap::from([("SDK_ENV".to_owned(), "preserved".to_owned())]);
        let checkpoints = Mutex::new(Vec::new());
        let callback = |session: &SdkSessionId| {
            checkpoints
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(session.clone());
            Ok(())
        };
        let mut req = base_req(&env);
        req.model = Some(&model);
        req.max_retries = 1;
        req.resume = Some(SdkSessionId {
            sdk: Sdk::Jcode,
            id: "caller-session".to_owned(),
        });
        req.on_session_id = Some(&callback);
        let mut attempts = Vec::new();
        let outcome = run_sdk_with_fallback(
            &req,
            Sdk::Jcode,
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[])),
            |sdk, model| {
                attempts.push((sdk, model.map(str::to_owned), resume_for(&req, sdk)));
                let id = format!("attempt-{}", attempts.len());
                let (ready, _rx) = std::sync::mpsc::channel();
                let terminal = if attempts.len() < 3 {
                    StreamChunk::Error("API Error: 404 model not found".to_owned())
                } else {
                    StreamChunk::Done(String::new())
                };
                Ok(canned(vec![
                    StreamChunk::SessionReady { id, ready },
                    terminal,
                ]))
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            attempts,
            vec![
                (
                    Sdk::Jcode,
                    Some("runtime/order-first:high".to_owned()),
                    Some("caller-session".to_owned())
                ),
                (
                    Sdk::Claude,
                    Some("runtime/order-middle:medium".to_owned()),
                    None
                ),
                (
                    Sdk::Jcode,
                    Some("runtime/order-last:low".to_owned()),
                    Some("caller-session".to_owned())
                ),
            ]
        );
        assert_eq!(
            checkpoints
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|id| id.sdk)
                .collect::<Vec<_>>(),
            [Sdk::Jcode, Sdk::Claude, Sdk::Jcode]
        );
        assert_eq!(outcome.session_id.map(|id| id.sdk), Some(Sdk::Jcode));
    }

    #[tokio::test]
    async fn explicit_chain_preflight_ignores_overridden_incompatible_tail() {
        for (sdk, model, computer_use, remote) in [
            (
                Sdk::Jcode,
                "[preflight/main:high, {sdk: claude, model: unreachable}]",
                true,
                false,
            ),
            (
                Sdk::Claude,
                "[preflight/main:high, {sdk: jcode, model: unreachable}]",
                false,
                true,
            ),
            (
                Sdk::Claude,
                "['', {sdk: jcode, model: unreachable}]",
                false,
                true,
            ),
        ] {
            let model: ModelSpec = serde_yaml::from_str(model)
                .unwrap_or_else(|error| panic!("invalid preflight model fixture: {error}"));
            let env = HashMap::new();
            let mut req = base_req(&env);
            req.model = Some(&model);
            req.computer_use = computer_use;
            let servers =
                serde_yaml::from_str("remote:\n  type: http\n  url: https://example.invalid/mcp\n")
                    .unwrap_or_else(|error| panic!("invalid remote MCP fixture: {error}"));
            if remote {
                req.mcp_servers = &servers;
            }
            for entries in [
                &[][..],
                &["preflight/safe"][..],
                &["preflight/main:low"][..],
                &["provider/*"][..],
            ] {
                let result = run_sdk_with_fallback(
                    &req,
                    sdk,
                    ModelReferenceFormat::for_sdk(sdk),
                    Some(fast_policy(&[("default", entries)])),
                    |owner, _| {
                        assert_eq!(owner, sdk);
                        Ok(canned(vec![StreamChunk::Done("ok".into())]))
                    },
                )
                .await;
                assert!(
                    result.is_ok(),
                    "effective candidates must pass preflight: {result:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn fallback_capability_validation_checks_nonprimary_sdk_before_start() {
        let model: ModelSpec = serde_yaml::from_str(
            "[runtime/capability, {sdk: claude, model: runtime/capability-tail}]",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = Some(&model);
        req.computer_use = true;
        let mut started = false;
        let result = run_sdk_with_fallback(
            &req,
            Sdk::Jcode,
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[])),
            |_, _| {
                started = true;
                Ok(canned(vec![StreamChunk::Done(String::new())]))
            },
        )
        .await;
        assert!(!started);
        assert!(result.is_err_and(|error| error.to_string().to_lowercase().contains("claude")));
    }

    // -- Executor dispatch ----------------------------------------------------

    #[tokio::test]
    async fn direct_command_request_rejects_sdk_candidates_before_spawning() {
        let env = HashMap::new();
        let model: ModelSpec = serde_yaml::from_str("[legacy, {sdk: claude, model: fallback}]")
            .unwrap_or_else(|error| panic!("{error}"));
        let mut req = base_req(&env);
        req.model = Some(&model);
        let result = Executor::Command {
            command: vec!["must-not-start".to_owned()],
        }
        .run(req)
        .await;
        assert!(matches!(result, Err(CruiseError::InvalidStepConfig(_))));
    }

    #[tokio::test]
    async fn direct_request_rejects_unknown_candidate_sdk_before_spawning() {
        let env = HashMap::new();
        let model: ModelSpec = serde_yaml::from_str("[{sdk: unknown, model: fallback}]")
            .unwrap_or_else(|error| panic!("{error}"));
        let mut req = base_req(&env);
        req.model = Some(&model);
        assert!(matches!(
            Executor::Jcode.run(req).await,
            Err(CruiseError::InvalidStepConfig(_))
        ));
    }

    fn command_executor() -> Executor {
        Executor::Command {
            command: vec!["claude".to_string(), "-p".to_string()],
        }
    }

    fn claude_executor() -> Executor {
        Executor::Claude
    }

    fn jcode_executor() -> Executor {
        Executor::Jcode
    }

    #[test]
    fn normalized_step_overrides_match_capability_selection() {
        for (sdk, global, step, expected_len) in [
            (
                Sdk::Jcode,
                "[provider/main:high, fallback]",
                "provider/main:low",
                2,
            ),
            (
                Sdk::OpenCode,
                "[provider/main#slow, fallback]",
                "provider/main#fast",
                2,
            ),
            (
                Sdk::Jcode,
                "[provider/main:high, fallback]",
                "provider/other:low",
                1,
            ),
            (
                Sdk::Jcode,
                "[{sdk: claude, model: provider/main:high}, fallback]",
                "provider/main:low",
                1,
            ),
        ] {
            let global: ModelSpec = serde_yaml::from_str(global)
                .unwrap_or_else(|error| panic!("invalid step model fixture: {error}"));
            let executor = Executor::new(Some(sdk.as_str()), &[]);
            let selected = executor
                .step_model(Some(step), Some(&global))
                .unwrap_or_else(|| panic!("step override must select a model"));
            let execution = resolve_candidates(Some(&selected), sdk);
            let capability =
                crate::model_selection::select_step_model(Some(&global), Some(step), sdk);
            assert_eq!(execution, capability);
            assert_eq!(execution.len(), expected_len);
            assert_eq!(execution[0].model.as_deref(), Some(step));
        }
    }

    #[test]
    fn new_picks_claude_when_sdk_is_claude() {
        let e = Executor::new(Some("claude"), &[]);
        assert!(e.is_sdk());
        assert!(matches!(e, Executor::Claude));
    }

    #[test]
    fn new_picks_command_when_sdk_unset() {
        let e = Executor::new(None, &["claude".to_string()]);
        assert!(!e.is_sdk());
        assert!(matches!(e, Executor::Command { .. }));
    }

    /// Neither `sdk` nor `command` set: the workflow runs on the default
    /// backend, which is what makes that combination valid at all (see
    /// [`crate::config::validate_sdk`]).
    #[test]
    fn new_picks_jcode_when_neither_sdk_nor_command_is_set() {
        let e = Executor::new(None, &[]);
        assert!(e.is_sdk());
        assert!(matches!(e, Executor::Jcode));
    }

    /// A removed `sdk:` value must never resolve to a working backend: cruise
    /// rejects it in `validate_sdk`, and this layer refuses to run it too.
    #[tokio::test]
    async fn new_refuses_a_removed_sdk_value_instead_of_defaulting_to_jcode() {
        let e = Executor::new(Some("seher"), &[]);
        assert!(!e.is_sdk());
        assert!(matches!(e, Executor::Unsupported { .. }));

        let env = std::collections::HashMap::new();
        let result = e
            .run(PromptRun {
                prompt: "hi",
                model: None,
                max_retries: 0,
                env: &env,
                mcp_servers: &EMPTY_MCP_SERVERS,
                on_notice: None,
                cancel_token: None,
                working_dir: None,
                stream: None,
                tools: Vec::new(),
                on_session_id: None,
                resume: None,
                computer_use: false,
                permission: crate::config::PermissionMode::Full,
            })
            .await;
        let Err(err) = result else {
            panic!("an unsupported sdk must not execute a prompt");
        };
        assert!(
            err.to_string().contains("seher"),
            "error should name the rejected value: {err}"
        );
    }

    #[test]
    fn command_step_model_passes_through_model_name() {
        let e = command_executor();
        assert_eq!(
            e.step_model(Some("sonnet"), fixture_model(Some("opus"))),
            Some(ModelSpec::Single("sonnet".to_string()))
        );
        assert_eq!(e.step_model(None, None), None);
    }

    #[test]
    fn claude_model_passes_through_model_reference() {
        // Claude mode passes model_or_mode straight through as a
        // `claude --model` name.
        let e = claude_executor();
        assert_eq!(
            e.step_model(Some("claude-sonnet-4-6:high"), fixture_model(Some("opus"))),
            Some(ModelSpec::Single("claude-sonnet-4-6:high".to_string()))
        );
        assert_eq!(e.step_model(None, None), None);
        assert_eq!(
            Executor::plan_model(fixture_model(Some("claude-opus-4-6")), None),
            Some(ModelSpec::Single("claude-opus-4-6".to_string()))
        );
        assert_eq!(Executor::plan_model(None, None), None);
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
        // `provider/model[:effort]` reference in jcode's own namespace.
        let e = jcode_executor();
        assert_eq!(
            e.step_model(
                Some("claude/claude-opus-5:high"),
                fixture_model(Some("openai/gpt-5.6"))
            ),
            Some(ModelSpec::Single("claude/claude-opus-5:high".to_string()))
        );
        assert_eq!(e.step_model(None, None), None);
        assert_eq!(
            Executor::plan_model(
                fixture_model(Some("openai/gpt-5.6")),
                fixture_model(Some("claude/claude-opus-5"))
            ),
            Some(ModelSpec::Single("openai/gpt-5.6".to_string()))
        );
        assert_eq!(Executor::plan_model(None, None), None);
    }

    fn base_req(env: &HashMap<String, String>) -> PromptRun<'_> {
        PromptRun {
            prompt: "hi",
            model: None,
            max_retries: 0,
            env,
            mcp_servers: &EMPTY_MCP_SERVERS,
            on_notice: None,
            cancel_token: None,
            working_dir: None,
            stream: None,
            tools: Vec::new(),
            on_session_id: None,
            resume: None,
            computer_use: false,
            permission: crate::config::PermissionMode::Full,
        }
    }

    #[tokio::test]
    async fn claude_backend_rejects_computer_use_before_running_a_prompt() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.computer_use = true;

        let Err(error) = claude_executor().run(req).await else {
            panic!("Claude backend must reject computer use");
        };
        assert!(matches!(error, CruiseError::InvalidStepConfig(_)));
        assert!(error.to_string().contains("interactive sessions"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_backend_rejects_computer_use_without_spawning_the_command() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let marker = temp.path().join("command-was-spawned");
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.computer_use = true;
        let executor = Executor::Command {
            command: vec!["touch".to_string(), marker.to_string_lossy().into_owned()],
        };

        let Err(error) = executor.run(req).await else {
            panic!("command backend must reject computer use");
        };
        assert!(matches!(error, CruiseError::InvalidStepConfig(_)));
        assert!(error.to_string().contains("jcode backend"));
        assert!(!marker.exists(), "the command must not be spawned");
    }

    // -- build_claude_config ---------------------------------------------------

    #[test]
    fn build_claude_config_splits_effort_suffix_off_the_model() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("claude-sonnet-4-6:xhigh"));
        let config = build_claude_config(&req, req.model.and_then(ModelSpec::primary));
        assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(config.effort, Some(EffortLevel::XHigh));
    }

    #[test]
    fn build_claude_config_leaves_model_unset_so_the_cli_picks_its_default() {
        let env = HashMap::new();
        let req = base_req(&env);
        let config = build_claude_config(&req, req.model.and_then(ModelSpec::primary));
        assert_eq!(config.model, None);
        assert_eq!(config.effort, None);
    }

    #[test]
    fn build_claude_config_forwards_tools_env_working_dir_and_resume() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let mut mcp_servers = crate::config::McpServers::new();
        mcp_servers.insert(
            "local_tool".to_string(),
            crate::config::McpServerConfig {
                command: Some("node".to_string()),
                ..Default::default()
            },
        );
        let tool = CruiseTool::new(
            "echo",
            "Echo",
            serde_json::json!({"type": "object"}),
            std::sync::Arc::new(|_| Ok(String::new())),
        );
        let dir = std::path::PathBuf::from("/tmp/cruise-claude-test");
        let req = PromptRun {
            prompt: "hi",
            model: fixture_model(Some("claude-opus-4-6")),
            max_retries: 0,
            env: &env,
            mcp_servers: &mcp_servers,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(&dir),
            stream: None,
            tools: vec![tool],
            on_session_id: None,
            resume: Some(SdkSessionId {
                sdk: Sdk::Claude,
                id: "sess-1".to_string(),
            }),
            computer_use: false,
            permission: crate::config::PermissionMode::Full,
        };
        let config = build_claude_config(&req, req.model.and_then(ModelSpec::primary));
        assert_eq!(config.cwd, Some(dir));
        assert_eq!(config.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(config.resume_session_id.as_deref(), Some("sess-1"));
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.tools[0].name, "echo");
        assert_eq!(config.mcp_servers, mcp_servers);
    }

    #[test]
    fn build_jcode_config_forwards_mcp_servers() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-provider/test-model"));
        let mut mcp_servers = crate::config::McpServers::new();
        mcp_servers.insert(
            "local_tool".to_string(),
            crate::config::McpServerConfig {
                command: Some("node".to_string()),
                ..Default::default()
            },
        );
        req.mcp_servers = &mcp_servers;

        let config = build_jcode_config(&req, req.model.and_then(ModelSpec::primary))
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(config.mcp_servers, mcp_servers);
        assert_eq!(config.provider.as_deref(), Some("test-provider"));
        assert_eq!(config.model.as_deref(), Some("test-model"));
    }

    #[test]
    fn build_jcode_config_rejects_references_without_a_model() {
        let env = HashMap::new();
        let req = base_req(&env);

        for model in [None, Some(""), Some("  \t"), Some(":high"), Some(":none")] {
            let result = build_jcode_config(&req, model);
            assert!(
                matches!(result, Err(CruiseError::InvalidStepConfig(_))),
                "model reference {model:?} must not produce a runner config"
            );
        }
    }

    #[test]
    fn build_jcode_config_keeps_real_models_with_effort_and_free_suffixes() {
        let env = HashMap::new();
        let req = base_req(&env);

        let effort = build_jcode_config(&req, Some("test-provider/test-model:high"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(effort.provider.as_deref(), Some("test-provider"));
        assert_eq!(effort.model.as_deref(), Some("test-model"));
        assert_eq!(effort.effort, Some(EffortLevel::High));

        let free = build_jcode_config(&req, Some("test-provider/free-model:free"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(free.provider.as_deref(), Some("test-provider"));
        assert_eq!(free.model.as_deref(), Some("free-model:free"));
        assert_eq!(free.effort, None);
    }

    #[tokio::test]
    async fn missing_jcode_model_ignores_backend_default_and_resume_and_creates_user_file() {
        let _process = lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let mut guards = set_fake_home(home.path());
        #[cfg(unix)]
        {
            let bin = home.path().join("bin");
            guards.push(crate::test_support::install_exiting_jcode_stub(&bin));
        }
        guards.push(EnvGuard::set("JCODE_HOME", home.path().join("jcode-home")));
        guards.push(EnvGuard::set("JCODE_MODEL", "backend/default-model"));
        for key in ["CRUISE_MODEL", "CRUISE_PLAN_MODEL"] {
            guards.push(EnvGuard::remove(key));
        }
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.resume = Some(SdkSessionId {
            sdk: Sdk::Jcode,
            id: "existing-session-id".to_string(),
        });

        let result =
            crate::retry::with_active_policy(Some(fast_policy(&[])), jcode_executor().run(req))
                .await;
        assert!(
            matches!(result, Err(CruiseError::MissingJcodeModel(_))),
            "backend defaults and a resume id must not replace a missing Cruise model: {result:?}"
        );
        let user_file = home.path().join(".config/cruise/default.yaml");
        assert!(
            user_file.is_file(),
            "expected user defaults at {}",
            user_file.display()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_jcode_model_reports_user_defaults_creation_failure() {
        let _process = lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let mut guards = set_fake_home(home.path());
        let bin = home.path().join("bin");
        guards.push(crate::test_support::install_exiting_jcode_stub(&bin));
        let config_root = home.path().join("config");
        guards.push(EnvGuard::set("XDG_CONFIG_HOME", &config_root));
        guards.push(EnvGuard::set("JCODE_HOME", home.path().join("jcode-home")));
        for key in ["JCODE_MODEL", "CRUISE_MODEL", "CRUISE_PLAN_MODEL"] {
            guards.push(EnvGuard::remove(key));
        }
        std::fs::write(&config_root, "not a directory").unwrap_or_else(|error| panic!("{error}"));
        let user_file = config_root.join("cruise/default.yaml");
        let env = HashMap::new();

        let result = jcode_executor().run(base_req(&env)).await;
        let Err(error @ CruiseError::MissingJcodeModel(_)) = result else {
            panic!("settings creation failure must preserve the missing-model error: {result:?}");
        };
        let diagnostic = error.to_string().to_ascii_lowercase();
        assert!(
            diagnostic.contains(&user_file.display().to_string().to_ascii_lowercase()),
            "missing-model diagnostic must identify the intended settings path: {diagnostic}"
        );
        assert!(
            diagnostic.contains("not a directory"),
            "missing-model diagnostic must include the file-system cause: {diagnostic}"
        );
        assert!(config_root.is_file());
        assert!(!user_file.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pre_cancelled_missing_jcode_request_has_no_initialization_or_backend_side_effect() {
        use std::os::unix::fs::PermissionsExt as _;

        let _process = lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let mut guards = set_fake_home(home.path());
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("{error}"));
        let marker = home.path().join("jcode-started");
        let executable = bin.join("jcode");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf started >> '{}'\nexit 0\n",
                marker.display()
            ),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let mut permissions = std::fs::metadata(&executable)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions)
            .unwrap_or_else(|error| panic!("{error}"));
        guards.push(prepend_to_path(&bin));
        for key in [
            "JCODE_MODEL",
            "JCODE_HOME",
            "CRUISE_MODEL",
            "CRUISE_PLAN_MODEL",
        ] {
            guards.push(EnvGuard::remove(key));
        }

        let token = CancellationToken::new();
        token.cancel();
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.cancel_token = Some(&token);

        let result = jcode_executor().run(req).await;

        assert!(
            matches!(result, Err(CruiseError::Interrupted)),
            "got {result:?}"
        );
        assert!(
            !home.path().join(".config/cruise/default.yaml").exists(),
            "a cancelled request must not create user defaults"
        );
        assert!(!marker.exists(), "a cancelled request must not start jcode");
    }

    #[tokio::test]
    async fn missing_jcode_model_is_not_replaced_by_read_only_file_tracker_error() {
        let _process = lock_process();
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let mut guards = set_fake_home(root.path());
        #[cfg(unix)]
        {
            let bin = root.path().join("bin");
            guards.push(crate::test_support::install_exiting_jcode_stub(&bin));
        }
        guards.push(EnvGuard::set("JCODE_HOME", root.path().join("jcode-home")));
        let xdg = root.path().join("workspace-config");
        guards.push(EnvGuard::set("XDG_CONFIG_HOME", xdg.as_os_str()));
        for key in ["JCODE_MODEL", "CRUISE_MODEL", "CRUISE_PLAN_MODEL"] {
            guards.push(EnvGuard::remove(key));
        }
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.working_dir = Some(root.path());
        req.permission = crate::config::PermissionMode::ReadOnly;

        let result = jcode_executor().run(req).await;

        assert!(
            matches!(result, Err(CruiseError::MissingJcodeModel(_))),
            "missing model must be reported before read-only tracking sees template creation: {result:?}"
        );
        assert!(xdg.join("cruise/default.yaml").is_file());
    }

    // -- run_with_fallback wiring ---------------------------------------------

    /// A retry policy whose backoff is short enough not to slow the tests.
    fn fast_policy(chains: &[(&str, &[&str])]) -> Arc<RetryPolicy> {
        Arc::new(RetryPolicy::new(crate::config::RetryConfig {
            base_delay_ms: 1,
            max_delay_ms: 300_000,
            model_fallback: true,
            fallback_chains: chains
                .iter()
                .map(|(key, entries)| {
                    (
                        (*key).to_string(),
                        entries.iter().map(|e| (*e).to_string()).collect(),
                    )
                })
                .collect(),
        }))
    }

    /// A backend stub: hands the caller a receiver already holding `chunks`.
    fn canned(chunks: Vec<StreamChunk>) -> std::sync::mpsc::Receiver<StreamChunk> {
        let (tx, rx) = std::sync::mpsc::channel();
        for chunk in chunks {
            let _ = tx.send(chunk);
        }
        rx
    }

    fn limit_chunk() -> StreamChunk {
        StreamChunk::Limit(LimitError {
            provider: "test".to_string(),
            detail: "HTTP status 429 slow down".to_string(),
        })
    }

    #[tokio::test]
    async fn retry_barrier_prevents_replay_without_an_output_sink() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("barrier/model:tag#precise"));
        req.max_retries = 3;
        let mut attempts = 0;
        let result = run_with_fallback(
            &req,
            "opencode",
            ModelReferenceFormat::OpenCode,
            Some(fast_policy(&[("default", &["barrier/spare"])])),
            |_| {
                attempts += 1;
                Ok(canned(vec![StreamChunk::RetryBarrier, limit_chunk()]))
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn durable_checkpoint_acknowledgement_follows_callback() {
        let checkpointed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saved = Arc::clone(&checkpointed);
        let callback = move |id: &str| {
            assert_eq!(id, "ses_checkpoint");
            saved.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (ready, ack) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(StreamChunk::SessionReady {
                id: "ses_checkpoint".to_string(),
                ready,
            })
            .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(ack.recv_timeout(Duration::from_secs(2)), Ok(true));
            assert!(checkpointed.load(std::sync::atomic::Ordering::Acquire));
            let _ = tx.send(StreamChunk::Done("answer".to_string()));
        });
        let result = stream_to_outcome(rx, None, Some(&callback), None)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(result.outcome, ChunkOutcome::Done { .. }));
        worker.join().unwrap_or_else(|_| panic!("worker panicked"));
    }

    #[tokio::test]
    async fn refused_checkpoint_waits_for_worker_cleanup() {
        let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = Arc::clone(&cleaned);
        let callback = |_: &str| Err(CruiseError::Other("checkpoint refused".to_string()));
        let (tx, rx) = std::sync::mpsc::channel();
        let (ready, ack) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(StreamChunk::SessionReady {
                id: "ses_checkpoint".to_string(),
                ready,
            });
            assert_eq!(ack.recv_timeout(Duration::from_secs(2)), Ok(false));
            done.store(true, std::sync::atomic::Ordering::Release);
        });
        let result = stream_to_outcome(rx, None, Some(&callback), None).await;
        assert!(result.is_err());
        assert!(cleaned.load(std::sync::atomic::Ordering::Acquire));
        worker.join().unwrap_or_else(|_| panic!("worker panicked"));
    }

    /// Collects strings a callback under test was handed.
    type Recorder = Arc<Mutex<Vec<String>>>;

    fn recorder() -> Recorder {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn recorded(items: &Recorder) -> Vec<String> {
        items
            .lock()
            .unwrap_or_else(|e| panic!("recorder lock: {e}"))
            .clone()
    }

    fn record(items: &Recorder, value: &str) {
        items
            .lock()
            .unwrap_or_else(|e| panic!("recorder lock: {e}"))
            .push(value.to_string());
    }

    #[tokio::test]
    async fn selected_model_notice_precedes_assistant_output_without_mutating_the_outcome() {
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let events = recorder();
        let events_sink = Arc::clone(&events);
        let on_notice = move |msg: &str| {
            record(&notices_sink, msg);
            record(&events_sink, msg);
        };
        let output_sink = Arc::clone(&events);
        let on_stdout = move |line: &str| record(&output_sink, &format!("assistant:{line}"));
        let stream = StreamCallbacks {
            on_stdout: Some(&on_stdout),
            on_stderr: None,
        };
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("opencode-zen/muse-spark-1.3-contributor-free:xhigh"));
        req.on_notice = Some(&on_notice);
        req.stream = Some(&stream);

        let start_events = Arc::clone(&events);
        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            None,
            |model| {
                assert_eq!(model, req.model.and_then(ModelSpec::primary));
                record(&start_events, "backend-start");
                Ok(canned(vec![
                    StreamChunk::Session("session-1".to_string()),
                    StreamChunk::Delta("assistant output\n".to_string()),
                    StreamChunk::Done("assistant result".to_string()),
                ]))
            },
        )
        .await
        .unwrap_or_else(|e| panic!("expected a successful prompt: {e}"));

        assert_eq!(outcome.result.output, "assistant result");
        assert_eq!(outcome.result.stderr, "");
        assert_eq!(
            outcome
                .session_id
                .as_ref()
                .map(|session| session.id.as_str()),
            Some("session-1")
        );
        assert_eq!(
            recorded(&notices),
            ["Model: opencode-zen/muse-spark-1.3-contributor-free:xhigh"]
        );
        assert_eq!(
            recorded(&events),
            [
                "backend-start",
                "Model: opencode-zen/muse-spark-1.3-contributor-free:xhigh",
                "assistant:assistant output"
            ]
        );
    }

    #[tokio::test]
    async fn default_model_notice_names_the_selected_sdk_backend() {
        for backend in ["jcode", "claude"] {
            let notices = recorder();
            let notices_sink = Arc::clone(&notices);
            let on_notice = move |msg: &str| record(&notices_sink, msg);
            let env = HashMap::new();
            let mut req = base_req(&env);
            req.on_notice = Some(&on_notice);

            run_with_fallback(
                &req,
                backend,
                ModelReferenceFormat::LegacyEffort,
                None,
                |_| Ok(canned(vec![StreamChunk::Done("ok".to_string())])),
            )
            .await
            .unwrap_or_else(|e| panic!("{backend} default run failed: {e}"));

            assert_eq!(
                recorded(&notices),
                [format!("Model: default model ({backend})")]
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_backend_reports_selected_or_default_model_without_mixing_into_output() {
        let _process_lock = crate::test_support::lock_process();

        for (model, expected_notice) in [
            (
                Some("command/provider/model:free:xhigh"),
                "Model: command/provider/model:free:xhigh",
            ),
            (None, "Model: default model (command)"),
        ] {
            let notices = recorder();
            let notices_sink = Arc::clone(&notices);
            let on_notice = move |msg: &str| record(&notices_sink, msg);
            let env = HashMap::new();
            let executor = Executor::Command {
                command: vec!["sh".to_string(), "-c".to_string(), "cat".to_string()],
            };
            let outcome = executor
                .run(PromptRun {
                    prompt: "assistant output",
                    model: fixture_model(model),
                    max_retries: 0,
                    env: &env,
                    mcp_servers: &EMPTY_MCP_SERVERS,
                    on_notice: Some(&on_notice),
                    cancel_token: None,
                    working_dir: None,
                    stream: None,
                    tools: Vec::new(),
                    on_session_id: None,
                    resume: None,
                    computer_use: false,
                    permission: crate::config::PermissionMode::Full,
                })
                .await
                .unwrap_or_else(|e| panic!("command run failed: {e}"));

            assert_eq!(recorded(&notices), [expected_notice]);
            assert_eq!(outcome.result.output, "assistant output");
            assert_eq!(outcome.result.stderr, "");
            assert!(!outcome.result.output.contains("Model:"));
            assert!(outcome.session_id.is_none());
        }
    }

    #[tokio::test]
    async fn resolved_step_and_plan_overrides_are_the_model_notice_source() {
        let executor = jcode_executor();
        let selected_models = [
            executor.step_model(
                Some("step/provider/model:free:xhigh"),
                fixture_model(Some("global/model")),
            ),
            Executor::plan_model(
                fixture_model(Some("plan/provider/model:free:xhigh")),
                fixture_model(Some("global/model")),
            ),
        ];

        for selected in selected_models {
            let selected = selected.unwrap_or_else(|| panic!("an override should be selected"));
            let notices = recorder();
            let notices_sink = Arc::clone(&notices);
            let on_notice = move |msg: &str| record(&notices_sink, msg);
            let env = HashMap::new();
            let mut req = base_req(&env);
            req.model = Some(&selected);
            req.on_notice = Some(&on_notice);

            run_with_fallback(
                &req,
                "jcode",
                ModelReferenceFormat::LegacyEffort,
                None,
                |_| Ok(canned(vec![StreamChunk::Done("ok".to_string())])),
            )
            .await
            .unwrap_or_else(|e| panic!("override run failed: {e}"));

            assert_eq!(
                recorded(&notices),
                [format!(
                    "Model: {}",
                    selected
                        .primary()
                        .unwrap_or_else(|| panic!("selected model must be present"))
                )]
            );
        }
    }

    #[tokio::test]
    async fn same_model_retry_reports_one_model_line_and_keeps_the_retry_notice() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-same-retry/model"));
        req.max_retries = 1;
        req.on_notice = Some(&on_notice);

        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                if recorded(&models).len() == 1 {
                    Ok(canned(vec![limit_chunk()]))
                } else {
                    Ok(canned(vec![StreamChunk::Done("ok".to_string())]))
                }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("same-model retry should succeed: {e}"));

        assert_eq!(outcome.result.output, "ok");
        assert_eq!(
            recorded(&models),
            [
                "test-executor-same-retry/model",
                "test-executor-same-retry/model"
            ]
        );
        let notices = recorded(&notices);
        assert_eq!(
            notices
                .iter()
                .filter(|notice| notice.starts_with("Model:"))
                .count(),
            1
        );
        assert!(
            notices
                .iter()
                .any(|notice| notice.starts_with("Rate limit detected")),
            "retry notice missing: {notices:?}"
        );
        assert!(
            !notices
                .iter()
                .any(|notice| notice.starts_with("Warning: Fallback:")),
            "same-model retry must not report fallback: {notices:?}"
        );
    }

    #[tokio::test]
    async fn multiple_fallbacks_report_full_references_once_in_transition_order() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("opencode-zen/muse-spark-1.3-contributor-free:xhigh"));
        req.max_retries = 1;
        req.on_notice = Some(&on_notice);

        let policy = fast_policy(&[(
            "opencode-zen/muse-spark-1.3-contributor-free:xhigh",
            &[
                "openrouter/z-ai/glm-5.2:free:xhigh",
                "provider/last-resort:free:xhigh",
            ],
        )]);
        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(policy),
            |model| {
                let model = model.unwrap_or_default();
                record(&models_sink, model);
                if model == "provider/last-resort:free:xhigh" {
                    Ok(canned(vec![StreamChunk::Done("ok".to_string())]))
                } else {
                    Ok(canned(vec![limit_chunk()]))
                }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("fallback chain should reach its final model: {e}"));

        assert_eq!(outcome.result.output, "ok");
        let lifecycle = recorded(&notices)
            .into_iter()
            .filter(|notice| {
                notice.starts_with("Model:") || notice.starts_with("Warning: Fallback:")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle.len(),
            5,
            "unexpected model lifecycle notices: {lifecycle:?}"
        );
        assert_eq!(
            lifecycle[0],
            "Model: opencode-zen/muse-spark-1.3-contributor-free:xhigh"
        );
        assert!(lifecycle[1].starts_with(
            "Warning: Fallback: opencode-zen/muse-spark-1.3-contributor-free:xhigh -> \
             openrouter/z-ai/glm-5.2:free:xhigh ("
        ));
        assert_eq!(lifecycle[2], "Model: openrouter/z-ai/glm-5.2:free:xhigh");
        assert!(lifecycle[3].starts_with(
            "Warning: Fallback: openrouter/z-ai/glm-5.2:free:xhigh -> provider/last-resort:free:xhigh ("
        ));
        assert_eq!(lifecycle[4], "Model: provider/last-resort:free:xhigh");
    }

    #[tokio::test]
    async fn client_error_switches_models_without_resending_the_same_request() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-smoke-400/primary"));
        req.max_retries = 2;
        req.on_notice = Some(&on_notice);

        let policy = fast_policy(&[("test-smoke-400/primary", &["test-smoke-400/spare"])]);
        let diagnostic = "jcode: provider error: API Error: 400 invalid_request: unsupported parameter\nrequest_id: req-test";
        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(policy),
            |model| {
                let model = model.unwrap_or_default();
                record(&models_sink, model);
                if model == "test-smoke-400/spare" {
                    Ok(canned(vec![StreamChunk::Done("ok".to_string())]))
                } else {
                    Ok(canned(vec![StreamChunk::Error(diagnostic.to_string())]))
                }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("400 should switch to the fallback model: {e}"));

        assert_eq!(outcome.result.output, "ok");
        assert_eq!(
            recorded(&models),
            ["test-smoke-400/primary", "test-smoke-400/spare"],
            "400 must not be resent to the same model"
        );
        let fallback = recorded(&notices)
            .into_iter()
            .find(|n| n.starts_with("Warning: Fallback:"))
            .unwrap_or_else(|| panic!("no fallback notice: {:?}", recorded(&notices)));
        assert!(
            fallback.contains(diagnostic),
            "fallback notice lost the original multiline backend error: {fallback}"
        );
    }

    #[tokio::test]
    async fn startup_cooldown_reports_only_the_selected_replacement_model() {
        let primary = "test-executor-startup/primary";
        let spare = "test-executor-startup/spare";
        let policy = fast_policy(&[(primary, &[spare])]);
        let env = HashMap::new();
        let mut first = base_req(&env);
        first.model = fixture_model(Some(primary));
        first.max_retries = 1;

        run_with_fallback(
            &first,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(Arc::clone(&policy)),
            |model| {
                if model == Some(spare) {
                    Ok(canned(vec![StreamChunk::Done("first".to_string())]))
                } else {
                    Ok(canned(vec![limit_chunk()]))
                }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("initial fallback run failed: {e}"));

        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let mut second = base_req(&env);
        second.model = fixture_model(Some(primary));
        second.on_notice = Some(&on_notice);

        run_with_fallback(
            &second,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(policy),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(canned(vec![StreamChunk::Done("second".to_string())]))
            },
        )
        .await
        .unwrap_or_else(|e| panic!("cooldown replacement run failed: {e}"));

        assert_eq!(recorded(&models), [spare]);
        let notices = recorded(&notices);
        assert_eq!(
            notices
                .iter()
                .filter(|notice| notice.starts_with("Model:"))
                .collect::<Vec<_>>(),
            ["Model: test-executor-startup/spare"]
        );
        assert!(
            notices.iter().any(|notice| {
                notice.starts_with(
                    "Warning: Fallback: test-executor-startup/primary -> test-executor-startup/spare (still cooling down"
                )
            }),
            "startup fallback notice missing: {notices:?}"
        );
        assert!(
            !notices
                .iter()
                .any(|notice| notice == "Model: test-executor-startup/primary"),
            "a cooling primary must not be reported as started: {notices:?}"
        );
    }

    #[tokio::test]
    async fn all_models_cooling_down_stops_before_any_model_notice_or_backend_start() {
        let primary = "test-executor-all-cooling/primary";
        let spare = "test-executor-all-cooling/spare";
        for model in [primary, spare] {
            let env = HashMap::new();
            let mut cooling_run = base_req(&env);
            cooling_run.model = fixture_model(Some(model));
            let _ = run_with_fallback(
                &cooling_run,
                "jcode",
                ModelReferenceFormat::LegacyEffort,
                Some(fast_policy(&[])),
                |_| Ok(canned(vec![limit_chunk()])),
            )
            .await;
        }
        let policy = fast_policy(&[(primary, &[spare])]);
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some(primary));
        req.on_notice = Some(&on_notice);

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(policy),
            |_| panic!("a fully cooling-down policy must not start a backend"),
        )
        .await
        else {
            panic!("all cooling-down models must prevent execution")
        };

        assert!(
            error
                .to_string()
                .contains("all configured models are cooling down"),
            "unexpected error: {error}"
        );
        assert_eq!(recorded(&notices), [] as [String; 0]);
    }

    #[tokio::test]
    async fn fallback_spends_the_model_budget_then_switches_immediately() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-budget/primary"));
        req.max_retries = 1;
        req.on_notice = Some(&on_notice);

        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-budget/primary",
                &["test-executor-budget/spare"],
            )])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(if model == Some("test-executor-budget/spare") {
                    canned(vec![StreamChunk::Done("ok".to_string())])
                } else {
                    canned(vec![limit_chunk()])
                })
            },
        )
        .await
        .unwrap_or_else(|e| panic!("expected the fallback model to answer: {e}"));

        assert_eq!(outcome.result.output, "ok");
        // The primary model spends its own budget before the chain is used.
        assert_eq!(
            recorded(&models),
            [
                "test-executor-budget/primary",
                "test-executor-budget/primary",
                "test-executor-budget/spare"
            ]
        );
        let notices = recorded(&notices);
        let fallback = notices
            .iter()
            .find(|notice| notice.starts_with("Warning: Fallback:"))
            .unwrap_or_else(|| panic!("no fallback notice: {notices:?}"));
        assert!(
            fallback.contains("HTTP status 429 slow down"),
            "fallback notice lost the original rate-limit detail: {fallback}"
        );
        assert_eq!(
            notices
                .iter()
                .filter(|notice| notice.starts_with("Model:"))
                .collect::<Vec<_>>(),
            [
                &"Model: test-executor-budget/primary".to_string(),
                &"Model: test-executor-budget/spare".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn fallback_is_disabled_by_a_zero_retry_budget() {
        // `--rate-limit-retries 0` means no retry: a fallback chain must not
        // become a second retry count.
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-zero/primary"));

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-zero/primary",
                &["test-executor-zero/spare"],
            )])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(canned(vec![limit_chunk()]))
            },
        )
        .await
        else {
            panic!("a spent budget must surface the failure");
        };

        assert!(error.to_string().contains("429"), "got: {error}");
        assert_eq!(recorded(&models), ["test-executor-zero/primary"]);
    }

    #[tokio::test]
    async fn text_the_user_saw_blocks_a_switch_but_not_a_same_model_retry() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let lines = recorder();
        let lines_sink = Arc::clone(&lines);
        let on_stdout = move |line: &str| record(&lines_sink, line);
        let stream = StreamCallbacks {
            on_stdout: Some(&on_stdout),
            on_stderr: None,
        };
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-streamed/primary"));
        req.max_retries = 1;
        req.stream = Some(&stream);

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-streamed/primary",
                &["test-executor-streamed/spare"],
            )])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(canned(vec![
                    StreamChunk::Delta("partial answer\n".to_string()),
                    limit_chunk(),
                ]))
            },
        )
        .await
        else {
            panic!("a streamed turn must not be replayed on another model");
        };

        assert!(error.to_string().contains("429"), "got: {error}");
        // Retried on the same model (historical behavior), never on the spare.
        assert_eq!(
            recorded(&models),
            [
                "test-executor-streamed/primary",
                "test-executor-streamed/primary"
            ]
        );
        assert_eq!(recorded(&lines), ["partial answer", "partial answer"]);
    }

    #[tokio::test]
    async fn text_no_sink_received_does_not_block_a_switch() {
        // `stream: None` (title generation, PR metadata) shows the user
        // nothing, so a delta must not count as replayed output.
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-no-sink/primary"));
        req.max_retries = 1;

        let outcome = run_with_fallback(
            &req,
            "claude",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-no-sink/primary",
                &["test-executor-no-sink/spare"],
            )])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(if model == Some("test-executor-no-sink/spare") {
                    canned(vec![StreamChunk::Done("ok".to_string())])
                } else {
                    canned(vec![
                        StreamChunk::Delta("partial answer\n".to_string()),
                        limit_chunk(),
                    ])
                })
            },
        )
        .await
        .unwrap_or_else(|e| panic!("expected the fallback model to answer: {e}"));

        assert_eq!(outcome.result.output, "ok");
        assert_eq!(
            recorded(&models),
            [
                "test-executor-no-sink/primary",
                "test-executor-no-sink/primary",
                "test-executor-no-sink/spare"
            ]
        );
    }

    #[tokio::test]
    async fn an_undispatchable_model_reference_moves_to_the_next_candidate() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-unusable/primary"));
        req.on_notice = Some(&on_notice);

        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-unusable/primary",
                &["test-executor-unusable/spare"],
            )])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                if model == Some("test-executor-unusable/spare") {
                    Ok(canned(vec![StreamChunk::Done("ok".to_string())]))
                } else {
                    Err(CruiseError::Other("invalid model reference".to_string()))
                }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("expected the fallback model to answer: {e}"));

        assert_eq!(outcome.result.output, "ok");
        assert_eq!(
            recorded(&models),
            [
                "test-executor-unusable/primary",
                "test-executor-unusable/spare"
            ]
        );
        let notices = recorded(&notices);
        let fallback = notices
            .iter()
            .find(|notice| notice.starts_with("Warning: Fallback:"))
            .unwrap_or_else(|| panic!("no fallback notice: {notices:?}"));
        assert!(
            fallback.contains("invalid model reference"),
            "fallback notice lost the original setup error: {fallback}"
        );
        assert!(
            !notices
                .iter()
                .any(|notice| notice == "Model: test-executor-unusable/primary"),
            "an unusable candidate must not be reported as started: {notices:?}"
        );
        assert!(
            notices
                .iter()
                .any(|notice| notice == "Model: test-executor-unusable/spare"),
            "the usable fallback must be reported: {notices:?}"
        );
    }

    #[tokio::test]
    async fn effort_only_fallback_candidate_is_skipped_before_runner_start() {
        let attempts = recorder();
        let attempts_sink = Arc::clone(&attempts);
        let started = recorder();
        let started_sink = Arc::clone(&started);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-primary/model"));
        req.max_retries = 1;

        let policy = fast_policy(&[("test-primary/model", &[":high", "test-next/model"])]);
        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(policy),
            |model| {
                let model = model.unwrap_or_default();
                record(&attempts_sink, model);
                let config = build_jcode_config(&req, Some(model))?;
                record(&started_sink, model);
                if model == "test-primary/model" {
                    Ok(canned(vec![limit_chunk()]))
                } else if config.model.is_none() {
                    Ok(canned(vec![StreamChunk::Done(
                        "effort-only model".to_string(),
                    )]))
                } else {
                    Ok(canned(vec![StreamChunk::Done("next model".to_string())]))
                }
            },
        )
        .await
        .unwrap_or_else(|error| {
            panic!("the valid model after the empty candidate should run: {error}")
        });

        assert_eq!(outcome.result.output, "next model");
        assert_eq!(
            recorded(&attempts),
            [
                "test-primary/model",
                "test-primary/model",
                ":high",
                "test-next/model",
            ]
        );
        assert_eq!(
            recorded(&started),
            [
                "test-primary/model",
                "test-primary/model",
                "test-next/model",
            ],
            "the effort-only fallback must not start a runner"
        );
    }

    #[tokio::test]
    async fn malformed_model_reference_keeps_existing_fallback_error_path_without_initializing_defaults()
     {
        let _process = lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let mut guards = set_fake_home(home.path());
        for key in ["JCODE_MODEL", "CRUISE_MODEL", "CRUISE_PLAN_MODEL"] {
            guards.push(EnvGuard::remove(key));
        }
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("provider/"));

        let result = jcode_executor().run(req).await;

        assert!(
            matches!(result, Err(CruiseError::InvalidStepConfig(_))),
            "an invalid reference must retain its existing parse/fallback error: {result:?}"
        );
        assert!(
            !home.path().join(".config/cruise/default.yaml").exists(),
            "an invalid reference is not an omitted model"
        );
    }

    #[tokio::test]
    async fn an_undispatchable_model_reference_without_a_chain_surfaces_its_error() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-no-chain/primary"));

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            None,
            |_| Err(CruiseError::Other("invalid model reference".to_string())),
        )
        .await
        else {
            panic!("an undispatchable model reference must fail the run");
        };

        assert!(
            error.to_string().contains("invalid model reference"),
            "got: {error}"
        );
    }

    #[tokio::test]
    async fn a_switch_still_observes_cancellation_before_the_next_attempt() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let token = CancellationToken::new();
        let cancel_on_switch = token.clone();
        let on_notice = move |msg: &str| {
            if msg.starts_with("Warning: Fallback:") {
                cancel_on_switch.cancel();
            }
        };
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-cancel/primary"));
        req.on_notice = Some(&on_notice);
        req.cancel_token = Some(&token);

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-cancel/primary",
                &["test-executor-cancel/spare"],
            )])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Err(CruiseError::Other("invalid model reference".to_string()))
            },
        )
        .await
        else {
            panic!("a cancelled run must not start another attempt");
        };

        assert!(
            matches!(error, CruiseError::Interrupted),
            "expected Interrupted, got: {error}"
        );
        // The spare was decided on but never spawned.
        assert_eq!(recorded(&models), ["test-executor-cancel/primary"]);
    }

    #[tokio::test]
    async fn no_notice_callback_preserves_fallback_result_without_direct_executor_output() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-no-callback/primary"));
        req.max_retries = 1;

        let outcome = run_with_fallback(
            &req,
            "jcode",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-no-callback/primary",
                &["test-executor-no-callback/spare"],
            )])),
            |model| {
                let model = model.unwrap_or_default();
                record(&models_sink, model);
                if model.ends_with("/spare") {
                    Ok(canned(vec![StreamChunk::Done("ok".to_string())]))
                } else {
                    Ok(canned(vec![limit_chunk()]))
                }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("fallback without a notice callback should succeed: {e}"));

        assert_eq!(outcome.result.output, "ok");
        assert_eq!(
            recorded(&models),
            [
                "test-executor-no-callback/primary",
                "test-executor-no-callback/primary",
                "test-executor-no-callback/spare"
            ]
        );
    }

    #[tokio::test]
    async fn a_stream_that_closes_without_a_terminal_chunk_is_not_retried() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model = fixture_model(Some("test-executor-closed/primary"));
        req.max_retries = 3;

        let Err(error) = run_with_fallback(
            &req,
            "claude",
            ModelReferenceFormat::LegacyEffort,
            Some(fast_policy(&[(
                "test-executor-closed/primary",
                &["test-executor-closed/spare"],
            )])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(canned(Vec::new()))
            },
        )
        .await
        else {
            panic!("a closed stream is not a retryable failure");
        };

        assert!(
            error
                .to_string()
                .contains("claude stream closed before completion"),
            "got: {error}"
        );
        assert_eq!(recorded(&models), ["test-executor-closed/primary"]);
    }

    // -- read-only workspace guard -------------------------------------------

    #[cfg(unix)]
    fn sh_executor(script: &str) -> Executor {
        Executor::Command {
            command: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
        }
    }

    #[cfg(unix)]
    async fn run_in(
        executor: &Executor,
        dir: &Path,
        permission: crate::config::PermissionMode,
        cancel: Option<&CancellationToken>,
    ) -> Result<PromptOutcome> {
        let env = HashMap::new();
        executor
            .run(PromptRun {
                prompt: "go",
                model: None,
                max_retries: 0,
                env: &env,
                mcp_servers: &EMPTY_MCP_SERVERS,
                on_notice: None,
                cancel_token: cancel,
                working_dir: Some(dir),
                stream: None,
                tools: Vec::new(),
                on_session_id: None,
                resume: None,
                computer_use: false,
                permission,
            })
            .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_run_returns_backend_result_when_workspace_is_unchanged() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.path().join("a.txt"), "a").unwrap_or_else(|e| panic!("{e}"));
        let executor = sh_executor("cat >/dev/null; printf ok");
        let outcome = run_in(
            &executor,
            dir.path(),
            crate::config::PermissionMode::ReadOnly,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("unchanged workspace must succeed: {e}"));
        assert_eq!(outcome.result.output, "ok");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_run_fails_when_a_file_is_created_modified_or_deleted() {
        for script in [
            "cat >/dev/null; echo new > created.txt",
            "cat >/dev/null; echo changed > existing.txt",
            "cat >/dev/null; rm existing.txt",
        ] {
            let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(dir.path().join("existing.txt"), "old")
                .unwrap_or_else(|e| panic!("{e}"));
            let result = run_in(
                &sh_executor(script),
                dir.path(),
                crate::config::PermissionMode::ReadOnly,
                None,
            )
            .await;
            assert!(
                matches!(result, Err(CruiseError::ReadOnlyWorkspaceChanged)),
                "{script}: {result:?}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_run_prefers_permission_error_over_backend_failure() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let executor = sh_executor("cat >/dev/null; echo new > created.txt; exit 3");
        let result = run_in(
            &executor,
            dir.path(),
            crate::config::PermissionMode::ReadOnly,
            None,
        )
        .await;
        assert!(
            matches!(result, Err(CruiseError::ReadOnlyWorkspaceChanged)),
            "{result:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_run_keeps_backend_failure_when_workspace_is_unchanged() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let executor = sh_executor("cat >/dev/null; exit 3");
        let result = run_in(
            &executor,
            dir.path(),
            crate::config::PermissionMode::ReadOnly,
            None,
        )
        .await;
        assert!(result.is_err());
        assert!(!matches!(
            result,
            Err(CruiseError::ReadOnlyWorkspaceChanged)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_run_prefers_permission_error_over_cancellation() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let token = CancellationToken::new();
        let executor = sh_executor("cat >/dev/null; echo new > created.txt; sleep 30");
        let canceller = token.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            canceller.cancel();
        });
        let result = run_in(
            &executor,
            dir.path(),
            crate::config::PermissionMode::ReadOnly,
            Some(&token),
        )
        .await;
        handle.await.unwrap_or_else(|e| panic!("{e}"));
        assert!(
            matches!(result, Err(CruiseError::ReadOnlyWorkspaceChanged)),
            "{result:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn edit_and_full_runs_may_change_the_workspace() {
        for mode in [
            crate::config::PermissionMode::Edit,
            crate::config::PermissionMode::Full,
        ] {
            let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
            let executor = sh_executor("cat >/dev/null; echo new > created.txt; printf done");
            let outcome = run_in(&executor, dir.path(), mode, None)
                .await
                .unwrap_or_else(|e| panic!("{mode:?} must not be guarded: {e}"));
            assert_eq!(outcome.result.output, "done");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_guard_ignores_excluded_directories() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let executor = sh_executor(
            "cat >/dev/null; mkdir -p target node_modules; echo x > target/o; echo y > node_modules/m; printf ok",
        );
        let outcome = run_in(
            &executor,
            dir.path(),
            crate::config::PermissionMode::ReadOnly,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("excluded dirs are outside the snapshot: {e}"));
        assert_eq!(outcome.result.output, "ok");
    }
}
