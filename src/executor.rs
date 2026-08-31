//! Prompt-execution backend abstraction.
//!
//! Cruise drives prompts through one of three backends: an external `command`
//! (the classic `claude -p` path), the **`jcode` CLI** (`sdk: jcode`, also the
//! default when a workflow names neither `sdk` nor `command`), or the
//! **`claude` CLI** (`sdk: claude`). [`Executor`] hides that choice behind a
//! single [`Executor::run`] call so that `planning.rs`, `engine.rs`, and the
//! GUI command layer don't need to branch on the backend.
//!
//! Every backend reads the cruise `model` / `plan_model` / per-step `model`
//! fields as a plain model reference:
//!
//! - `command` — the model name substituted into the command line.
//! - `sdk: jcode` — a `provider/model[:effort]` reference in jcode's own
//!   provider/model namespace, driven as a `jcode run --ndjson` subprocess
//!   under cruise's private `JCODE_HOME` -- see [`run_jcode`] and
//!   [`crate::backend::jcode`].
//! - `sdk: claude` — a plain `claude --model` name with an optional `:effort`
//!   suffix, driven in-process through `claude-agent-sdk` -- see
//!   [`run_claude`] and [`crate::backend::claude`].

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
use crate::error::{CruiseError, Result};
use crate::retry::{self, Failure, FallbackEngine, RetryAction, RetryPolicy};
use crate::step::prompt::{PromptResult, StreamCallbacks, run_prompt};
use crate::tool_bridge::ToolBridge;

/// Callback invoked when an SDK backend reports its session identifier.
pub type SessionIdCallback<'a> = dyn Fn(&str) -> Result<()> + Send + Sync + 'a;

/// A single prompt execution request, backend-agnostic.
///
/// Built by the caller and handed to [`Executor::run`]. `model_or_mode` carries
/// the model reference the selected backend takes (compute it with
/// [`Executor::step_model_or_mode`] / [`Executor::plan_model_or_mode`]).
/// `tools` and `resume` are honored by the SDK backends and ignored in command
/// mode.
pub struct PromptRun<'a> {
    /// The fully-resolved prompt text to send.
    pub prompt: &'a str,
    /// Model name (command mode) or model reference (`sdk: jcode`,
    /// `sdk: claude`).
    pub model_or_mode: Option<&'a str>,
    /// Maximum rate-limit retries.
    pub max_retries: usize,
    /// Environment variables applied to the prompt run.
    ///
    /// Command mode passes these to the spawned process; `sdk: jcode` and
    /// `sdk: claude` pass them to the `jcode` / `claude` child process.
    pub env: &'a HashMap<String, String>,
    /// Callback invoked with human-readable progress notices: rate-limit
    /// retries and model fallbacks.
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
    pub resume: Option<String>,
}

/// Outcome of [`Executor::run`]: the prompt result plus, in SDK mode, the
/// backend session id (for a follow-up `resume`). `session_id` is `None` in
/// command mode.
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
    /// Drive the `jcode` CLI as an NDJSON subprocess (`sdk: jcode`) under
    /// cruise's private `JCODE_HOME`, exposing cruise's tools over the
    /// [`ToolBridge`]. See [`run_jcode`].
    Jcode,
    /// Drive the `claude` CLI in-process through `claude-agent-sdk`
    /// (`sdk: claude`). See [`run_claude`].
    Claude,
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
    /// Build an executor from the workflow's backend selection.
    ///
    /// `sdk: jcode` -> [`Executor::Jcode`]; `sdk: claude` -> [`Executor::Claude`];
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
            Some(other) => Executor::Unsupported {
                sdk: other.to_string(),
            },
            None if command.is_empty() => Executor::Jcode,
            None => Executor::Command {
                command: command.to_vec(),
            },
        }
    }

    /// Whether this executor drives prompts through an agent backend (`Jcode`
    /// or `Claude`) rather than spawning an external `command`.
    #[must_use]
    pub fn is_sdk(&self) -> bool {
        matches!(self, Executor::Jcode | Executor::Claude)
    }

    /// Resolve the model reference for an ordinary prompt step: a per-step
    /// value wins over the workflow default.
    ///
    /// Matched exhaustively even though every backend reads the same plain
    /// reference, so adding a backend has to revisit this.
    #[must_use]
    pub fn step_model_or_mode(
        &self,
        step_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. }
            | Executor::Jcode
            | Executor::Claude
            | Executor::Unsupported { .. } => step_model.or(global_model).map(str::to_string),
        }
    }

    /// Resolve the model reference for the built-in planning step: the
    /// dedicated `plan_model` wins, falling back to the workflow `model`.
    #[must_use]
    pub fn plan_model_or_mode(
        &self,
        plan_model: Option<&str>,
        global_model: Option<&str>,
    ) -> Option<String> {
        match self {
            Executor::Command { .. }
            | Executor::Jcode
            | Executor::Claude
            | Executor::Unsupported { .. } => plan_model.or(global_model).map(str::to_string),
        }
    }

    /// Execute one prompt on the selected backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to spawn / exits non-zero, if the
    /// `jcode` run or the `claude` CLI run fails, or if `sdk` named a backend
    /// cruise does not implement.
    pub async fn run(&self, req: PromptRun<'_>) -> Result<PromptOutcome> {
        match self {
            Executor::Command { command } => run_command(command, req).await,
            Executor::Jcode => run_jcode(req).await,
            Executor::Claude => run_claude(req).await,
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

/// Bridge a blocking `std::sync::mpsc::Receiver` of backend chunks (as returned
/// by [`stream_claude_agent`] and [`stream_jcode_agent`]) into a
/// [`ChunkOutcome`], forwarding text deltas to `on_delta` line-buffered (the
/// SDK backends emit token-level deltas; `StreamCallbacks::on_stdout` is
/// line-oriented like the command backend) and returning `Err(Interrupted)` as
/// soon as `cancel_token` fires.
async fn stream_to_outcome(
    rx_std: std::sync::mpsc::Receiver<StreamChunk>,
    on_delta: Option<&(dyn Fn(&str) + Send + Sync)>,
    on_session_id: Option<&SessionIdCallback<'_>>,
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
            () = maybe_cancelled(cancel_token) => return Err(CruiseError::Interrupted),
            maybe = rx.recv() => match maybe {
                Some(chunk) => {
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
    Ok(Folded { outcome, streamed })
}

/// `Claude`-backend execution: drive the `claude` CLI in-process through
/// `claude-agent-sdk` ([`crate::backend::claude`]).
///
/// Retryable failures go through [`run_with_fallback`], so a rate limit backs
/// off on the same model unless the workflow's `retry:` policy names a
/// fallback model to switch to.
///
/// Retries deliberately start from `req.resume` (the caller's session), not the
/// aborted attempt's session id: re-sending the same prompt into a
/// partially-answered session would duplicate context.
///
/// `req.cancel_token` is forwarded to the backend thread, which stops reading
/// the CLI's output and closes the transport when it fires — that is what
/// terminates the `claude` child, since the SDK offers no interrupt.
async fn run_claude(req: PromptRun<'_>) -> Result<PromptOutcome> {
    run_with_fallback(&req, "claude", retry::active_policy(), |model| {
        Ok(stream_claude_agent(
            build_claude_config(&req, model),
            req.prompt.to_string(),
        ))
    })
    .await
}

/// Build a [`ClaudeRunnerConfig`] straight from `req`. `model_ref` is the model
/// this attempt runs on — the caller's plain `claude --model` name with an
/// optional `:effort` suffix (see [`Executor::step_model_or_mode`] /
/// [`Executor::plan_model_or_mode`]), or a fallback the retry policy switched
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
        resume_session_id: req.resume.clone(),
        tools: req.tools.clone(),
        env: req.env.clone(),
        cancel: req.cancel_token.cloned(),
        cli_path: None,
    }
}

/// How one attempt of [`run_with_fallback`] failed. Keeps the failure text
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
/// Without a workflow `retry:` policy this is exactly the historical loop: only
/// a backend-reported rate limit retries, on the same model, on
/// [`crate::step::command::calculate_backoff`]'s 2s-doubling schedule, up to
/// `req.max_retries` times.
async fn run_with_fallback(
    req: &PromptRun<'_>,
    label: &str,
    policy: Option<Arc<RetryPolicy>>,
    mut start: impl FnMut(Option<&str>) -> Result<std::sync::mpsc::Receiver<StreamChunk>>,
) -> Result<PromptOutcome> {
    let on_delta = req.stream.and_then(|s| s.on_stdout);
    let mut engine = FallbackEngine::new(policy, req.model_or_mode, req.max_retries);
    if let Some((from, to)) = engine.take_startup_switch()
        && let Some(cb) = req.on_notice
    {
        cb(&format!(
            "fallback: {from} -> {to} (still cooling down from an earlier failure)"
        ));
    }

    loop {
        let failed = match start(engine.model()) {
            Err(error) => AttemptFailure::Unusable {
                message: error.to_string(),
                error,
            },
            Ok(rx_std) => {
                let folded =
                    stream_to_outcome(rx_std, on_delta, req.on_session_id, req.cancel_token)
                        .await?;
                let streamed = folded.streamed;
                match folded.outcome {
                    ChunkOutcome::Done { output, session } => {
                        return Ok(PromptOutcome {
                            result: PromptResult {
                                output,
                                stderr: String::new(),
                            },
                            session_id: session,
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
                    cb(&format!(
                        "fallback: {} -> {to} ({detail}, attempt {attempt}/{of})",
                        from.as_deref().unwrap_or("default model")
                    ));
                }
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

/// `Jcode`-backend execution: run the prompt as a `jcode run --ndjson`
/// subprocess ([`crate::backend::jcode`]) under cruise's private `JCODE_HOME`.
///
/// jcode has no in-process tool registration, so cruise's tools are served to
/// it over a per-run Unix socket by a [`ToolBridge`]: the `cruise mcp-bridge`
/// server jcode spawns relays every call back here, which is what keeps the
/// handlers' in-process state (the terminal `ask_user` prompt, the plan-persist
/// flag, the title / PR-metadata stores) authoritative. The bridge is started
/// once for the whole run, including its retries, and torn down on return.
///
/// Retryable failures go through [`run_with_fallback`], so a rate limit backs
/// off on the same model unless the workflow's `retry:` policy names a
/// fallback model to switch to.
///
/// Retries deliberately start from `req.resume` (the caller's session), not the
/// aborted attempt's session id: re-sending the same prompt into a
/// partially-answered session would duplicate context.
async fn run_jcode(req: PromptRun<'_>) -> Result<PromptOutcome> {
    let home = jcode::preflight(None, req.working_dir, req.env, req.on_notice)?;
    let bridge = ToolBridge::start(req.tools.clone())?;

    run_with_fallback(&req, "jcode", retry::active_policy(), |model_ref| {
        let (provider, model, effort) = jcode::parse_model_ref(model_ref)?;
        let config = JcodeRunnerConfig {
            model,
            provider,
            effort,
            cwd: req.working_dir.map(Path::to_path_buf),
            resume_session_id: req.resume.clone(),
            home: home.clone(),
            tool_socket: bridge.socket_path().to_path_buf(),
            env: req.env.clone(),
            cancel: req.cancel_token.cloned(),
            binary: None,
        };
        Ok(stream_jcode_agent(config, req.prompt.to_string()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::backend::effort::EffortLevel;
    use crate::backend::stream::LimitError;

    // -- Executor dispatch ----------------------------------------------------

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
                model_or_mode: None,
                max_retries: 0,
                env: &env,
                on_notice: None,
                cancel_token: None,
                working_dir: None,
                stream: None,
                tools: Vec::new(),
                on_session_id: None,
                resume: None,
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
            e.step_model_or_mode(Some("sonnet"), Some("opus")),
            Some("sonnet".to_string())
        );
        assert_eq!(e.step_model_or_mode(None, None), None);
    }

    #[test]
    fn claude_model_passes_through_model_reference() {
        // Claude mode passes model_or_mode straight through as a
        // `claude --model` name.
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
        // `provider/model[:effort]` reference in jcode's own namespace.
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
            on_session_id: None,
            resume: None,
        }
    }

    // -- build_claude_config ---------------------------------------------------

    #[test]
    fn build_claude_config_splits_effort_suffix_off_the_model() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model_or_mode = Some("claude-sonnet-4-6:xhigh");
        let config = build_claude_config(&req, req.model_or_mode);
        assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(config.effort, Some(EffortLevel::XHigh));
    }

    #[test]
    fn build_claude_config_leaves_model_unset_so_the_cli_picks_its_default() {
        let env = HashMap::new();
        let req = base_req(&env);
        let config = build_claude_config(&req, req.model_or_mode);
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
            on_session_id: None,
            resume: Some("sess-1".to_string()),
        };
        let config = build_claude_config(&req, req.model_or_mode);
        assert_eq!(config.cwd, Some(dir));
        assert_eq!(config.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(config.resume_session_id.as_deref(), Some("sess-1"));
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.tools[0].name, "echo");
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
    async fn fallback_spends_the_model_budget_then_switches_immediately() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let notices = recorder();
        let notices_sink = Arc::clone(&notices);
        let on_notice = move |msg: &str| record(&notices_sink, msg);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model_or_mode = Some("p/primary");
        req.max_retries = 1;
        req.on_notice = Some(&on_notice);

        let outcome = run_with_fallback(
            &req,
            "jcode",
            Some(fast_policy(&[("p/primary", &["p/spare"])])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(if model == Some("p/spare") {
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
        assert_eq!(recorded(&models), ["p/primary", "p/primary", "p/spare"]);
        let notices = recorded(&notices);
        assert!(
            notices
                .iter()
                .any(|n| n == "fallback: p/primary -> p/spare (429, attempt 2/3)"),
            "got: {notices:?}"
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
        req.model_or_mode = Some("p/primary");

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            Some(fast_policy(&[("p/primary", &["p/spare"])])),
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
        assert_eq!(recorded(&models), ["p/primary"]);
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
        req.model_or_mode = Some("p/primary");
        req.max_retries = 1;
        req.stream = Some(&stream);

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            Some(fast_policy(&[("p/primary", &["p/spare"])])),
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
        assert_eq!(recorded(&models), ["p/primary", "p/primary"]);
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
        req.model_or_mode = Some("p/primary");
        req.max_retries = 1;

        let outcome = run_with_fallback(
            &req,
            "claude",
            Some(fast_policy(&[("p/primary", &["p/spare"])])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                Ok(if model == Some("p/spare") {
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
        assert_eq!(recorded(&models), ["p/primary", "p/primary", "p/spare"]);
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
        req.model_or_mode = Some("p/primary");
        req.on_notice = Some(&on_notice);

        let outcome = run_with_fallback(
            &req,
            "jcode",
            Some(fast_policy(&[("p/primary", &["p/spare"])])),
            |model| {
                record(&models_sink, model.unwrap_or_default());
                if model == Some("p/spare") {
                    Ok(canned(vec![StreamChunk::Done("ok".to_string())]))
                } else {
                    Err(CruiseError::Other("invalid model reference".to_string()))
                }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("expected the fallback model to answer: {e}"));

        assert_eq!(outcome.result.output, "ok");
        assert_eq!(recorded(&models), ["p/primary", "p/spare"]);
        let notices = recorded(&notices);
        assert!(
            notices
                .iter()
                .any(|n| n == "fallback: p/primary -> p/spare (unusable model, attempt 1/1)"),
            "got: {notices:?}"
        );
    }

    #[tokio::test]
    async fn an_undispatchable_model_reference_without_a_chain_surfaces_its_error() {
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model_or_mode = Some("p/primary");

        let Err(error) = run_with_fallback(&req, "jcode", None, |_| {
            Err(CruiseError::Other("invalid model reference".to_string()))
        })
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
            if msg.starts_with("fallback:") {
                cancel_on_switch.cancel();
            }
        };
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model_or_mode = Some("p/primary");
        req.on_notice = Some(&on_notice);
        req.cancel_token = Some(&token);

        let Err(error) = run_with_fallback(
            &req,
            "jcode",
            Some(fast_policy(&[("p/primary", &["p/spare"])])),
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
        assert_eq!(recorded(&models), ["p/primary"]);
    }

    #[tokio::test]
    async fn a_stream_that_closes_without_a_terminal_chunk_is_not_retried() {
        let models = recorder();
        let models_sink = Arc::clone(&models);
        let env = HashMap::new();
        let mut req = base_req(&env);
        req.model_or_mode = Some("p/primary");
        req.max_retries = 3;

        let Err(error) = run_with_fallback(
            &req,
            "claude",
            Some(fast_policy(&[("p/primary", &["p/spare"])])),
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
        assert_eq!(recorded(&models), ["p/primary"]);
    }
}
