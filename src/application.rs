//! Client-neutral application facade shared by the CLI, TUI, and Tauri.
//!
//! This module deliberately owns operation claims, prompt request identity, and
//! the neutral event vocabulary. Presentation adapters only translate these
//! types into their own channels and widgets.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use uuid::Uuid;

use crate::ask_handler::AskHandler;
use crate::cancellation::CancellationToken;
use crate::error::{CruiseError, Result};
use crate::new_session_draft::NewSessionDraft;
use crate::new_session_history::NewSessionHistory;
use crate::option_handler::OptionHandler;
use crate::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use crate::session_edit::{CurrentStepUpdate, SessionSettingsUpdate};
use crate::step::{OptionChoice, option::OptionResult};

/// Operation kinds protected by the process-local claim registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    Generate,
    Fix,
    Ask,
    Replan,
    Run,
    BatchQueued,
    BatchRun,
    Mutate,
}

/// Stable stream names used by plan and run events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventStream {
    Stdout,
    Stderr,
    Info,
}

/// Serializable description of an option exposed to clients. `next_step` is
/// retained so clients can submit a validated `OptionResult` without parsing
/// workflow configuration themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OptionChoiceKind {
    Selector,
    TextInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionChoicePayload {
    pub label: String,
    pub kind: OptionChoiceKind,
    pub next_step: Option<String>,
}

/// A single client-neutral event vocabulary shared by the TUI and Tauri.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ApplicationEvent {
    PlanStarted {
        session_id: String,
        operation: OperationKind,
    },
    PlanChunk {
        session_id: String,
        stream: EventStream,
        text: String,
    },
    AskUserRequired {
        session_id: String,
        request_id: String,
        question: String,
    },
    PlanFinished {
        session_id: String,
        phase: String,
    },
    PlanFailed {
        session_id: String,
        error: String,
    },
    PlanCancelled {
        session_id: String,
    },
    RunStarted {
        session_id: String,
    },
    RunPhase {
        session_id: String,
        phase: String,
    },
    StepStarted {
        session_id: String,
        step: String,
    },
    OptionRequired {
        session_id: String,
        request_id: String,
        prompt: String,
        choices: Vec<OptionChoicePayload>,
    },
    PrCreated {
        session_id: String,
        url: String,
    },
    RunFinished {
        session_id: String,
        phase: String,
    },
    RunFailed {
        session_id: String,
        error: String,
    },
    RunCancelled {
        session_id: String,
    },
    BatchStarted {
        total: usize,
        parallelism: usize,
    },
    BatchTotalChanged {
        total: usize,
    },
    BatchSessionStarted {
        id: String,
    },
    BatchSessionFinished {
        id: String,
        phase: String,
        error: Option<String>,
    },
    BatchFinished {
        cancelled: bool,
    },
    LogChunk {
        session_id: Option<String>,
        stream: EventStream,
        text: String,
        batch: bool,
    },
}

/// Separate output stream for potentially high-volume logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEvent {
    pub session_id: Option<String>,
    pub stream: EventStream,
    pub text: String,
    pub batch: bool,
}

/// Reliable control event sink used by both interactive clients.
pub trait ApplicationEventSink: Send + Sync {
    /// Deliver one control event, returning an error if the receiver is gone.
    ///
    /// # Errors
    ///
    /// Returns an error when the receiver cannot accept the event.
    fn send(&self, event: ApplicationEvent) -> Result<()>;
}

impl<F> ApplicationEventSink for F
where
    F: Fn(ApplicationEvent) -> Result<()> + Send + Sync,
{
    fn send(&self, event: ApplicationEvent) -> Result<()> {
        self(event)
    }
}

/// Optional bounded log sink. Implementations should use `try_send` and
/// increment their own dropped counter when the queue is full.
pub trait LogSink: Send + Sync {
    fn try_send(&self, event: LogEvent) -> bool;
}
impl LogSink for crate::session::SessionLogger {
    fn try_send(&self, event: LogEvent) -> bool {
        let stream = match event.stream {
            EventStream::Stdout => "stdout",
            EventStream::Stderr => "stderr",
            EventStream::Info => "info",
        };
        self.write(&format!("[{stream}] {}", event.text));
        true
    }
}

/// A process-local identity-safe claim for one session operation.
pub struct OperationClaim {
    runtime: Arc<ApplicationRuntime>,
    session_id: String,
    identity: u64,
    token: CancellationToken,
}

impl std::fmt::Debug for OperationClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationClaim")
            .field("session_id", &self.session_id)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl OperationClaim {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    #[must_use]
    pub fn identity(&self) -> u64 {
        self.identity
    }
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for OperationClaim {
    fn drop(&mut self) {
        self.runtime.release_claim(&self.session_id, self.identity);
    }
}

struct ClaimRecord {
    identity: u64,
    operation: OperationKind,
    token: CancellationToken,
    terminal: bool,
}
struct PendingRequest {
    session_id: String,
    claim_identity: u64,
    kind: PendingKind,
    choices: Option<Vec<OptionChoicePayload>>,
    sender: mpsc::Sender<PromptResponse>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Ask,
    Option,
}

enum PromptResponse {
    Ask(String),
    Option(OptionResult),
}

/// Shared process runtime. It is the sole owner of claims, cancellation
/// handles, prompt requests, and the one active Run All claim.
pub struct ApplicationRuntime {
    manager: SessionManager,
    claims: Mutex<HashMap<String, ClaimRecord>>,
    pending: Mutex<HashMap<String, PendingRequest>>,
    batch: Mutex<Option<(u64, CancellationToken)>>,
    /// Serializes cancellation with terminal state commits. A successful
    /// terminal commit marks its claim terminal while holding this gate, so a
    /// late cancellation cannot turn a committed operation into Cancelled.
    commit_gate: Mutex<()>,
    next_identity: AtomicU64,
}

impl std::fmt::Debug for ApplicationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplicationRuntime").finish_non_exhaustive()
    }
}

fn option_choice_payload(choice: &OptionChoice) -> OptionChoicePayload {
    match choice {
        OptionChoice::Selector { label, next } => OptionChoicePayload {
            label: label.clone(),
            kind: OptionChoiceKind::Selector,
            next_step: next.clone(),
        },
        OptionChoice::TextInput { label, next } => OptionChoicePayload {
            label: label.clone(),
            kind: OptionChoiceKind::TextInput,
            next_step: next.clone(),
        },
    }
}

fn read_optional_snapshot(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CruiseError::Other(format!(
            "failed to snapshot {}: {error}",
            path.display()
        ))),
    }
}
fn read_log_tail(path: &std::path::Path, limit: usize) -> std::io::Result<String> {
    if limit == 0 {
        return Ok(String::new());
    }
    let mut file = std::fs::File::open(path)?;
    let mut position = file.seek(SeekFrom::End(0))?;
    let mut newline_count = 0usize;
    let mut chunks = Vec::new();
    let mut buffer = [0_u8; 8192];
    while position > 0 && newline_count <= limit {
        let start = position.saturating_sub(buffer.len() as u64);
        file.seek(SeekFrom::Start(start))?;
        let chunk_len = usize::try_from(position - start).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "log chunk is too large")
        })?;
        let read = file.read(&mut buffer[..chunk_len])?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            if *byte == b'\n' {
                newline_count += 1;
            }
        }
        chunks.push(buffer[..read].to_vec());
        position = start;
    }
    chunks.reverse();
    let size = chunks.iter().map(Vec::len).sum();
    let mut bytes = Vec::with_capacity(size);
    for chunk in chunks {
        bytes.extend_from_slice(&chunk);
    }
    let start = if position > 0 {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    } else {
        0
    };
    let text = String::from_utf8_lossy(&bytes[start..]);
    let mut lines = text.lines().rev().take(limit).collect::<Vec<_>>();
    lines.reverse();
    Ok(lines.join("\n"))
}
fn cleanup_new_execution_workspace(
    manager: &SessionManager,
    state: &mut SessionState,
    workspace: &crate::workspace::ExecutionWorkspace,
    repo_clone_created: bool,
) -> Result<()> {
    if let crate::workspace::ExecutionWorkspace::Worktree { ctx, reused } = workspace
        && !*reused
    {
        crate::worktree::cleanup_worktree(ctx)?;
    }
    if repo_clone_created {
        let clone_path = manager.clones_dir().join(&state.id);
        if clone_path.exists() {
            std::fs::remove_dir_all(clone_path)?;
        }
    }
    if let crate::workspace::ExecutionWorkspace::Worktree { reused, .. } = workspace
        && !*reused
    {
        state.worktree_path = None;
        state.worktree_branch = None;
    }
    Ok(())
}

fn create_session_inner(
    manager: &SessionManager,
    request: NewSessionRequest,
    id: &str,
) -> Result<SessionState> {
    let repo = request
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|repo| !repo.is_empty())
        .map(ToString::to_string);
    if let Some(repo) = &repo {
        crate::repo_clone::validate_repo_spec(repo)?;
    }
    let base_dir = if let Some(repo) = &repo {
        let clone_path = manager.clones_dir().join(id);
        crate::repo_clone::clone_repo(repo, &clone_path)?;
        clone_path
    } else if request.base_dir.as_os_str().is_empty() {
        std::env::current_dir()?
    } else {
        request.base_dir.clone()
    };
    let requested = request
        .config_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .or_else(|| {
            request.config_source.clone().map(|source| {
                if crate::resolver::ConfigSource::is_builtin_source(&source) {
                    crate::new_session_history::BUILTIN_CONFIG_KEY.to_string()
                } else {
                    source
                        .strip_prefix("config: ")
                        .unwrap_or(&source)
                        .to_string()
                }
            })
        })
        .map(|path| crate::new_session_history::expand_tilde(&path))
        .filter(|path| !path.trim().is_empty());
    let (yaml, source) = if let Some(raw) = request.config_yaml.as_deref() {
        let source = crate::resolver::ConfigSource::Builtin;
        (raw.to_string(), source)
    } else {
        crate::resolver::resolve_config_in_dir(requested.as_deref(), &base_dir)?
    };
    let config = crate::resolver::resolve_workflow_config(&yaml, &source, &base_dir)?;
    crate::config::validate_config(&config)?;
    let persistent_path = if repo.is_some() {
        crate::repo_clone::persistent_config_path(&source, &base_dir)
    } else {
        source.path().cloned()
    };
    let source_display = source.display_string();
    let mut state = SessionState::new_draft(
        id.to_string(),
        base_dir,
        source_display,
        request.input.trim().to_string(),
    );
    state.workspace_mode = request.workspace_mode;
    state.allow_dirty_working_tree = request.allow_dirty_working_tree;
    state.config_path = persistent_path;
    state.repo = repo;
    state.skipped_steps = request.skipped_steps;
    manager.create(&state)?;
    let session_dir = manager.sessions_dir().join(id);
    state.attachments =
        crate::attachments::copy_images_into_session(&session_dir, &request.attachments)?;
    if state.config_path.is_none() {
        let snapshot = crate::repo_clone::serialize_resolved_config(&config)?;
        crate::planning::write_plan_atomically(
            &session_dir.join("config.yaml"),
            snapshot.as_bytes(),
        )?;
    }
    manager.save(&state)?;
    let resolved_config_key = source.path().map_or_else(
        || crate::new_session_history::BUILTIN_CONFIG_KEY.to_string(),
        |path| crate::new_session_history::resolved_config_key_for_session(path),
    );
    let mut history = NewSessionHistory::load_best_effort();
    history.record_selection(crate::new_session_history::NewSessionHistoryEntry {
        selected_at: crate::session::current_iso8601(),
        input: state.input.clone(),
        requested_config_path: requested,
        working_dir: if state.repo.is_some() {
            String::new()
        } else {
            state.base_dir.to_string_lossy().into_owned()
        },
        repo: state.repo.clone(),
        resolved_config_key,
        skipped_steps: state.skipped_steps.clone(),
    });
    history.save_best_effort();
    Ok(state)
}

impl ApplicationRuntime {
    #[must_use]
    pub fn new(manager: SessionManager) -> Self {
        Self {
            manager,
            claims: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            batch: Mutex::new(None),
            commit_gate: Mutex::new(()),
            next_identity: AtomicU64::new(1),
        }
    }

    /// Begin one session operation. A duplicate never replaces the owner.
    ///
    /// # Errors
    ///
    /// Returns [`CruiseError::Busy`] when the session already has an active
    /// operation claim.
    pub fn try_begin(
        self: &Arc<Self>,
        session_id: impl Into<String>,
        operation: OperationKind,
    ) -> Result<OperationClaim> {
        let session_id = session_id.into();
        let identity = self.next_identity.fetch_add(1, Ordering::Relaxed);
        let token = CancellationToken::new();
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if claims.contains_key(&session_id) {
            return Err(CruiseError::Busy(format!(
                "session {session_id} is already busy"
            )));
        }
        claims.insert(
            session_id.clone(),
            ClaimRecord {
                identity,
                operation,
                token: token.clone(),
                terminal: false,
            },
        );
        Ok(OperationClaim {
            runtime: Arc::clone(self),
            session_id,
            identity,
            token,
        })
    }

    fn release_claim(&self, session_id: &str, identity: u64) {
        let removed = {
            let mut claims = self
                .claims
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if claims
                .get(session_id)
                .is_some_and(|record| record.identity == identity)
            {
                claims.remove(session_id);
                true
            } else {
                false
            }
        };
        if removed {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pending.retain(|_, request| {
                !(request.session_id == session_id && request.claim_identity == identity)
            });
        }
    }

    /// Cancel only the currently owned operation for `session_id`.
    pub fn cancel_session(&self, session_id: &str) -> bool {
        let _commit_gate = self
            .commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(claim) = claims.get(session_id) else {
            return false;
        };
        if claim.terminal {
            return false;
        }
        claim.token.cancel();
        let identity = claim.identity;
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, request| {
            !(request.session_id == session_id && request.claim_identity == identity)
        });
        true
    }

    fn cancel_batch(&self, identity: Option<u64>) -> bool {
        let _commit_gate = self
            .commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let batch = self
            .batch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((active, token)) = batch.as_ref() else {
            return false;
        };
        if identity.is_some_and(|expected| expected != *active) {
            return false;
        }
        token.cancel();
        true
    }

    /// Commit a terminal operation only while its identity still owns the
    /// session. Cancellation takes the same gate, making the decision and the
    /// durable state write one atomic ownership transition.
    fn commit_if_active<F>(&self, claim: &OperationClaim, commit: F) -> Result<bool>
    where
        F: FnOnce() -> Result<()>,
    {
        let _commit_gate = self
            .commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = claims.get(&claim.session_id).is_some_and(|record| {
            record.identity == claim.identity && !record.terminal && !record.token.is_cancelled()
        });
        if !active {
            return Ok(false);
        }
        commit()?;
        if let Some(record) = claims.get_mut(&claim.session_id)
            && record.identity == claim.identity
        {
            record.terminal = true;
        }
        Ok(true)
    }
    /// Return the active claim identity, useful for status/reconcile views.
    #[must_use]
    pub fn active_identity(&self, session_id: &str) -> Option<u64> {
        self.claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .map(|record| record.identity)
    }
    #[must_use]
    pub fn active_operation(&self, session_id: &str) -> Option<OperationKind> {
        self.claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .map(|record| record.operation)
    }

    /// Begin the process-wide Run All operation.
    ///
    /// # Errors
    ///
    /// Returns [`CruiseError::Busy`] when another Run All operation is active.
    pub fn try_begin_batch(self: &Arc<Self>) -> Result<BatchClaim> {
        let identity = self.next_identity.fetch_add(1, Ordering::Relaxed);
        let token = CancellationToken::new();
        let mut batch = self
            .batch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if batch.is_some() {
            return Err(CruiseError::Busy(
                "a Run All operation is already active".to_string(),
            ));
        }
        *batch = Some((identity, token.clone()));
        Ok(BatchClaim {
            runtime: Arc::clone(self),
            identity,
            token,
        })
    }

    fn release_batch(&self, identity: u64) {
        let mut batch = self
            .batch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if batch
            .as_ref()
            .is_some_and(|(active, _)| *active == identity)
        {
            *batch = None;
        }
    }

    fn register_prompt(
        &self,
        session_id: &str,
        claim_identity: u64,
        kind: PendingKind,
        question: Option<&str>,
        choices: Option<&[OptionChoice]>,
    ) -> Result<(String, mpsc::Receiver<PromptResponse>)> {
        let claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !claims
            .get(session_id)
            .is_some_and(|claim| claim.identity == claim_identity && !claim.token.is_cancelled())
        {
            return Err(CruiseError::Other(
                "operation claim is no longer active".to_string(),
            ));
        }
        let id = format!(
            "prompt-{}-{}",
            self.next_identity.fetch_add(1, Ordering::Relaxed),
            Uuid::new_v4().simple()
        );
        let (sender, receiver) = mpsc::channel();
        let payload = choices.map(|items| items.iter().map(option_choice_payload).collect());
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.insert(
            id.clone(),
            PendingRequest {
                session_id: session_id.to_string(),
                claim_identity,
                kind,
                choices: payload,
                sender,
            },
        );
        drop(pending);
        drop(claims);
        let persist = self.manager.load(session_id).and_then(|mut state| {
            if kind == PendingKind::Ask {
                state.phase = SessionPhase::AwaitingInput;
                state.pending_ask_question = question.map(str::to_string);
            } else {
                state.pending_ask_question = None;
            }
            state.awaiting_input = true;
            self.manager.save(&state)
        });
        if let Err(error) = persist {
            self.unregister_prompt(&id, session_id, claim_identity);
            return Err(error);
        }
        Ok((id, receiver))
    }

    fn unregister_prompt(&self, request_id: &str, session_id: &str, claim_identity: u64) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.get(request_id).is_some_and(|request| {
            request.session_id == session_id && request.claim_identity == claim_identity
        }) {
            pending.remove(request_id);
        }
    }

    fn wait_prompt(
        &self,
        session_id: &str,
        request_id: &str,
        claim_identity: u64,
        receiver: &mpsc::Receiver<PromptResponse>,
        token: Option<&CancellationToken>,
    ) -> Result<PromptResponse> {
        loop {
            if token.is_some_and(CancellationToken::is_cancelled) {
                self.unregister_prompt(request_id, session_id, claim_identity);
                return Err(CruiseError::Interrupted);
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(response) => {
                    self.unregister_prompt(request_id, session_id, claim_identity);
                    if token.is_some_and(CancellationToken::is_cancelled) {
                        return Err(CruiseError::Interrupted);
                    }
                    let mut state = self.manager.load(session_id)?;
                    state.clear_pending_input();
                    self.manager.save(&state)?;
                    return Ok(response);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.unregister_prompt(request_id, session_id, claim_identity);
                    return Err(CruiseError::Other(
                        "prompt response channel closed".to_string(),
                    ));
                }
            }
        }
    }

    fn respond(
        &self,
        session_id: &str,
        request_id: &str,
        expected: PendingKind,
        response: PromptResponse,
    ) -> Result<()> {
        match &response {
            PromptResponse::Ask(answer) if answer.trim().is_empty() => {
                return Err(CruiseError::Other("answer must not be empty".to_string()));
            }
            PromptResponse::Option(result)
                if result
                    .text_input
                    .as_deref()
                    .is_some_and(|text| text.trim().is_empty()) =>
            {
                return Err(CruiseError::Other(
                    "text input must not be empty".to_string(),
                ));
            }
            _ => {}
        }
        let claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(request) = pending.get(request_id) else {
            return Err(CruiseError::Other(
                "stale or unknown prompt request".to_string(),
            ));
        };
        let active = claims.get(session_id).is_some_and(|claim| {
            claim.identity == request.claim_identity && !claim.token.is_cancelled()
        });
        if request.session_id != session_id || request.kind != expected || !active {
            return Err(CruiseError::Other(
                "stale prompt request for another session or operation".to_string(),
            ));
        }
        if let PromptResponse::Option(result) = &response {
            let valid = request.choices.as_ref().is_some_and(|choices| {
                choices.iter().any(|choice| {
                    let next_matches = choice.next_step == result.next_step;
                    match &choice.kind {
                        OptionChoiceKind::Selector => next_matches && result.text_input.is_none(),
                        OptionChoiceKind::TextInput => next_matches && result.text_input.is_some(),
                    }
                })
            });
            if !valid {
                return Err(CruiseError::Other(
                    "invalid option response for prompt request".to_string(),
                ));
            }
        }
        let Some(request) = pending.remove(request_id) else {
            return Err(CruiseError::Other(
                "stale or unknown prompt request".to_string(),
            ));
        };
        drop(pending);
        drop(claims);
        request
            .sender
            .send(response)
            .map_err(|_| CruiseError::Other("prompt worker is no longer waiting".to_string()))
    }

    /// Respond to a pending `ask_user` request with both identity components.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty answer or a stale, unknown, or inactive
    /// prompt request.
    pub fn respond_to_ask(&self, session_id: &str, request_id: &str, answer: String) -> Result<()> {
        self.respond(
            session_id,
            request_id,
            PendingKind::Ask,
            PromptResponse::Ask(answer),
        )
    }

    /// Respond to a pending option request.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid text input or a stale, unknown, inactive,
    /// or otherwise invalid option response.
    pub fn respond_to_option(
        &self,
        session_id: &str,
        request_id: &str,
        result: OptionResult,
    ) -> Result<()> {
        self.respond(
            session_id,
            request_id,
            PendingKind::Option,
            PromptResponse::Option(result),
        )
    }
    pub fn pending_prompts(&self, session_id: &str) -> Vec<PendingPrompt> {
        let requests = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut prompts = requests
            .iter()
            .filter(|&(_request_id, request)| request.session_id == session_id)
            .map(|(request_id, request)| {
                (request_id.clone(), request.kind, request.choices.clone())
            })
            .collect::<Vec<_>>();
        drop(requests);
        prompts.sort_by(|a, b| a.0.cmp(&b.0));
        prompts
            .into_iter()
            .map(|(request_id, kind, choices)| PendingPrompt {
                request_id,
                session_id: session_id.to_string(),
                kind: match kind {
                    PendingKind::Ask => PendingPromptKind::Ask,
                    PendingKind::Option => PendingPromptKind::Option,
                },
                question: (kind == PendingKind::Ask)
                    .then(|| {
                        self.manager
                            .load(session_id)
                            .ok()
                            .and_then(|state| state.pending_ask_question)
                    })
                    .flatten(),
                choices: choices.unwrap_or_default(),
            })
            .collect()
    }
}

/// A batch claim whose child session tokens are linked to its parent.
pub struct BatchClaim {
    runtime: Arc<ApplicationRuntime>,
    identity: u64,
    token: CancellationToken,
}

impl std::fmt::Debug for BatchClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchClaim")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl BatchClaim {
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
    pub fn cancel(&self) {
        self.runtime.cancel_batch(Some(self.identity));
    }

    /// Atomically reserve all currently available candidates. Busy IDs are
    /// returned explicitly and are excluded from the reserved total.
    pub fn reserve(&self, candidates: &[SessionState]) -> BatchReservation {
        let mut claims = self
            .runtime
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut reserved = Vec::new();
        let mut busy = Vec::new();
        for state in candidates {
            if claims.contains_key(&state.id) {
                busy.push(state.id.clone());
                continue;
            }
            let identity = self.runtime.next_identity.fetch_add(1, Ordering::Relaxed);
            let token = self.token.child_token();
            claims.insert(
                state.id.clone(),
                ClaimRecord {
                    identity,
                    operation: OperationKind::BatchRun,
                    token: token.clone(),
                    terminal: false,
                },
            );
            reserved.push(OperationClaim {
                runtime: Arc::clone(&self.runtime),
                session_id: state.id.clone(),
                identity,
                token,
            });
        }
        BatchReservation { reserved, busy }
    }
}

impl Drop for BatchClaim {
    fn drop(&mut self) {
        self.runtime.release_batch(self.identity);
    }
}

/// Result of an atomic batch reservation.
pub struct BatchReservation {
    pub reserved: Vec<OperationClaim>,
    pub busy: Vec<String>,
}

async fn drain_batch_workers(
    batch: &BatchClaim,
    running: &mut tokio::task::JoinSet<(usize, String, SessionState, Result<SessionState>)>,
) {
    batch.cancel();
    while let Some(joined) = running.join_next().await {
        let _ = joined;
    }
}

/// New-session form input understood by all clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionRequest {
    pub input: String,
    pub base_dir: PathBuf,
    #[serde(default)]
    pub config_path: Option<PathBuf>,
    #[serde(default)]
    pub config_source: Option<String>,
    #[serde(default)]
    pub config_yaml: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub workspace_mode: WorkspaceMode,
    #[serde(default)]
    pub allow_dirty_working_tree: bool,
    #[serde(default)]
    pub attachments: Vec<PathBuf>,
    #[serde(default)]
    pub skipped_steps: Vec<String>,
}

pub const DEFAULT_RATE_LIMIT_RETRIES: usize = 5;

fn default_rate_limit_retries() -> usize {
    DEFAULT_RATE_LIMIT_RETRIES
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    #[serde(default)]
    pub workspace_mode: Option<WorkspaceMode>,
    #[serde(default)]
    pub max_retries: Option<usize>,
    #[serde(default = "default_rate_limit_retries")]
    pub rate_limit_retries: usize,
}

impl Default for RunRequest {
    fn default() -> Self {
        Self {
            workspace_mode: None,
            max_retries: None,
            rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
        }
    }
}

/// Whether this planning request may use interactive planning tools.
///
/// The transparent representation keeps the serialized request field as a
/// boolean while giving the request's planning modes a named type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Interactive(bool);

impl Interactive {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self(enabled)
    }

    #[must_use]
    pub const fn is_enabled(self) -> bool {
        self.0
    }
}

impl From<bool> for Interactive {
    fn from(enabled: bool) -> Self {
        Self::new(enabled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "planning modes are independent request flags exposed by the application API"
)]
pub struct PlanRequest {
    #[serde(default)]
    pub grill: bool,
    #[serde(default)]
    pub formal_spec: bool,
    #[serde(default)]
    pub skip_planning: bool,
    /// Disable SDK planning tools for this request, even when the workflow
    /// enables interactive planning. The agent writes the plan file directly.
    #[serde(default)]
    pub no_interactive_planning: bool,
    #[serde(default)]
    pub interactive: Interactive,
    #[serde(default = "default_rate_limit_retries")]
    pub rate_limit_retries: usize,
    #[serde(default)]
    pub feedback: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
}

impl Default for PlanRequest {
    fn default() -> Self {
        Self {
            grill: false,
            formal_spec: false,
            skip_planning: false,
            no_interactive_planning: false,
            interactive: Interactive::default(),
            rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
            feedback: None,
            question: None,
        }
    }
}

/// Exact action policy for one persisted session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionAction {
    Generate,
    Answer,
    Cancel,
    Delete,
    Approve,
    Publish,
    Fix,
    Ask,
    Discard,
    RunWorktree,
    RunCurrentBranch,
    Replan,
    EditSettings,
    Retry,
    ResetToPlanned,
    EditCurrentStep,
    Resume,
    OpenPr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionConfigDefaults {
    pub steps: Vec<crate::workflow::SkippableStepNode>,
    pub after_pr_steps: Vec<crate::workflow::SkippableStepNode>,
    pub default_skipped_steps: Vec<String>,
    pub resolved_config_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionHistorySummary {
    pub last_requested_config_path: Option<String>,
    pub last_working_dir: Option<String>,
    pub recent_working_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PendingPromptKind {
    Ask,
    Option,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPrompt {
    pub request_id: String,
    pub session_id: String,
    pub kind: PendingPromptKind,
    pub question: Option<String>,
    pub choices: Vec<OptionChoicePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettingsRequest {
    #[serde(default)]
    pub config_path: Option<String>,
    #[serde(default)]
    pub skipped_steps: Vec<String>,
    #[serde(default)]
    pub current_step_update: CurrentStepUpdateDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CurrentStepUpdateDto {
    #[default]
    Unchanged,
    Clear,
    Set(String),
}

/// The shared application façade.
#[derive(Clone)]
pub struct CruiseApplication {
    manager: SessionManager,
    runtime: Arc<ApplicationRuntime>,
}

impl std::fmt::Debug for CruiseApplication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CruiseApplication").finish_non_exhaustive()
    }
}

fn restore_input_plan(
    plan_path: &std::path::Path,
    staged_plan_path: &std::path::Path,
    old_plan: Option<&[u8]>,
) -> Result<()> {
    if let Some(bytes) = old_plan {
        crate::planning::write_plan_atomically(plan_path, bytes)
            .map_err(|error| CruiseError::Other(format!("failed to restore plan: {error}")))?;
    } else if let Err(error) = std::fs::remove_file(plan_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(CruiseError::Other(format!(
            "failed to remove partial plan: {error}"
        )));
    }
    if let Err(error) = std::fs::remove_file(staged_plan_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(CruiseError::Other(format!(
            "failed to remove staged plan: {error}"
        )));
    }
    Ok(())
}

fn commit_input_plan(
    manager: &SessionManager,
    mut state: SessionState,
    plan_path: &std::path::Path,
    staged_plan_path: &std::path::Path,
    old_plan: Option<&[u8]>,
) -> Result<()> {
    crate::repo_clone::cleanup_after_approval_checked(manager, &mut state)?;
    std::fs::rename(staged_plan_path, plan_path)
        .map_err(|error| CruiseError::Other(format!("failed to commit generated plan: {error}")))?;
    if let Err(error) = manager.save(&state) {
        if let Some(bytes) = old_plan {
            crate::planning::write_plan_atomically(plan_path, bytes).map_err(|restore_error| {
                CruiseError::Other(format!(
                    "failed to restore plan after state save failure ({error}): {restore_error}"
                ))
            })?;
        } else if let Err(restore_error) = std::fs::remove_file(plan_path)
            && restore_error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(CruiseError::Other(format!(
                "failed to restore plan after state save failure ({error}): {restore_error}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn use_input_as_plan_inner(
    manager: &SessionManager,
    runtime: &ApplicationRuntime,
    id: &str,
    claim: &OperationClaim,
    mut state: SessionState,
    sink: &dyn ApplicationEventSink,
) -> Result<SessionState> {
    let before = state.clone();
    let plan_path = state.plan_path(&manager.sessions_dir());
    let staged_plan_path = manager
        .sessions_dir()
        .join(&state.id)
        .join(format!(".plan.{}.staged", claim.identity()));
    let old_plan = read_optional_snapshot(&plan_path)?;
    let _ = std::fs::remove_file(&staged_plan_path);
    sink.send(ApplicationEvent::PlanStarted {
        session_id: id.to_string(),
        operation: OperationKind::Generate,
    })?;
    if let Err(error) =
        crate::planning::write_input_as_plan(&staged_plan_path, &state.input_with_attachments())
            .and_then(|_| state.use_input_as_plan())
    {
        restore_input_plan(&plan_path, &staged_plan_path, old_plan.as_deref())?;
        state = before.clone();
        state.plan_error = Some(error.to_string());
        manager.save(&state)?;
        sink.send(ApplicationEvent::PlanFailed {
            session_id: id.to_string(),
            error: error.to_string(),
        })?;
        return Err(error);
    }
    if let Ok(content) = std::fs::read_to_string(&staged_plan_path) {
        crate::metadata::refresh_session_title_from_plan(&mut state, &content);
    }
    let state_for_commit = state.clone();
    let plan_path_for_commit = plan_path.clone();
    let staged_path_for_commit = staged_plan_path.clone();
    let old_plan_for_commit = old_plan.clone();
    let committed = runtime.commit_if_active(claim, || {
        commit_input_plan(
            manager,
            state_for_commit,
            &plan_path_for_commit,
            &staged_path_for_commit,
            old_plan_for_commit.as_deref(),
        )
    });
    match committed {
        Ok(true) => {
            if state.repo.is_some() {
                state.worktree_path = None;
            }
            sink.send(ApplicationEvent::PlanFinished {
                session_id: id.to_string(),
                phase: state.phase.label().to_string(),
            })?;
            Ok(state)
        }
        Ok(false) => {
            restore_input_plan(&plan_path, &staged_plan_path, old_plan.as_deref())?;
            state = before;
            manager.save(&state)?;
            sink.send(ApplicationEvent::PlanCancelled {
                session_id: id.to_string(),
            })?;
            Err(CruiseError::Interrupted)
        }
        Err(error) => {
            restore_input_plan(&plan_path, &staged_plan_path, old_plan.as_deref())?;
            state = before;
            state.plan_error = Some(error.to_string());
            manager.save(&state)?;
            sink.send(ApplicationEvent::PlanFailed {
                session_id: id.to_string(),
                error: error.to_string(),
            })?;
            Err(error)
        }
    }
}

struct PlanContext {
    claim: OperationClaim,
    state: SessionState,
    before_state: SessionState,
    plan_path: PathBuf,
    staged_plan_path: PathBuf,
    old_plan: Option<Vec<u8>>,
    key: Option<String>,
    resume: Option<String>,
}

fn planning_operation_allowed(state: &SessionState, operation: OperationKind) -> bool {
    match operation {
        OperationKind::Fix | OperationKind::Ask => matches!(
            state.phase,
            SessionPhase::AwaitingApproval | SessionPhase::Planned
        ),
        OperationKind::Replan => matches!(
            state.phase,
            SessionPhase::Planned | SessionPhase::AwaitingApproval
        ),
        _ => matches!(
            state.phase,
            SessionPhase::Draft
                | SessionPhase::AwaitingInput
                | SessionPhase::AwaitingApproval
                | SessionPhase::Planned
        ),
    }
}

fn start_plan_context(
    manager: &SessionManager,
    runtime: &Arc<ApplicationRuntime>,
    id: &str,
    operation: OperationKind,
    sink: &dyn ApplicationEventSink,
) -> Result<PlanContext> {
    let claim = runtime.try_begin(id, operation)?;
    let state = manager.load(id)?;
    if !planning_operation_allowed(&state, operation) {
        return Err(CruiseError::Other(format!(
            "session {id} is not in a phase for this planning action"
        )));
    }
    let before_state = state.clone();
    let plan_path = state.plan_path(&manager.sessions_dir());
    let staged_plan_path = manager
        .sessions_dir()
        .join(&state.id)
        .join(format!(".plan.{}.staged", claim.identity()));
    let old_plan = read_optional_snapshot(&plan_path)?;
    let _ = std::fs::remove_file(&staged_plan_path);
    sink.send(ApplicationEvent::PlanStarted {
        session_id: id.to_string(),
        operation,
    })?;
    Ok(PlanContext {
        claim,
        state,
        before_state,
        plan_path,
        staged_plan_path,
        old_plan,
        key: None,
        resume: None,
    })
}

fn restore_planning_plan(context: &PlanContext) -> Result<()> {
    if let Some(bytes) = context.old_plan.as_deref() {
        crate::planning::write_plan_atomically(&context.plan_path, bytes).map_err(|error| {
            CruiseError::Other(format!(
                "failed to restore plan after planning failure: {error}"
            ))
        })?;
    } else if let Err(error) = std::fs::remove_file(&context.plan_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(CruiseError::Other(format!(
            "failed to remove partial plan after planning failure: {error}"
        )));
    }
    if let Err(error) = std::fs::remove_file(&context.staged_plan_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(CruiseError::Other(format!(
            "failed to remove staged plan after planning failure: {error}"
        )));
    }
    Ok(())
}

fn cancel_plan(
    manager: &SessionManager,
    id: &str,
    sink: &dyn ApplicationEventSink,
    context: &mut PlanContext,
) -> Result<SessionState> {
    let base_dir = context.state.base_dir.clone();
    restore_planning_plan(context)?;
    context.state = context.before_state.clone();
    context.state.base_dir = base_dir;
    manager.save(&context.state)?;
    sink.send(ApplicationEvent::PlanCancelled {
        session_id: id.to_string(),
    })?;
    Err(CruiseError::Interrupted)
}

fn fail_plan(
    manager: &SessionManager,
    id: &str,
    operation: OperationKind,
    sink: &dyn ApplicationEventSink,
    context: &mut PlanContext,
    error: CruiseError,
) -> Result<SessionState> {
    if context.claim.token().is_cancelled() {
        return cancel_plan(manager, id, sink, context);
    }
    let message = error.to_string();
    let base_dir = context.state.base_dir.clone();
    restore_planning_plan(context)?;
    context.state = context.before_state.clone();
    context.state.base_dir = base_dir;
    if operation != OperationKind::Ask {
        context.state.plan_error = Some(message.clone());
    }
    manager.save(&context.state)?;
    sink.send(ApplicationEvent::PlanFailed {
        session_id: id.to_string(),
        error: message,
    })?;
    Err(error)
}

async fn prepare_plan(
    manager: &SessionManager,
    context: &mut PlanContext,
    request: &PlanRequest,
) -> Result<crate::config::WorkflowConfig> {
    let mut config = manager.load_config(&context.state)?;
    crate::config::validate_config(&config)?;
    if request.no_interactive_planning {
        config.interactive_planning = false;
    }
    let planning_interactive =
        request.interactive.is_enabled() && crate::planning::sdk_plan_tools_enabled(&config);
    if request.grill && !planning_interactive {
        return Err(CruiseError::Other(
            "grill requires an interactive SDK backend with interactive planning enabled"
                .to_string(),
        ));
    }
    let token = context.claim.token();
    crate::repo_clone::ensure_repo_session_workspace_cancelled(manager, &mut context.state, &token)
        .await?;
    manager.save(&context.state)?;
    let key = crate::planning::plan_conversation_key_for_path(
        &config,
        context.state.config_path.as_deref(),
    );
    context.resume = if context.state.plan_conversation_key.as_deref() == Some(key.as_str()) {
        context.state.plan_conversation_id.clone()
    } else {
        None
    };
    context.key = Some(key.clone());
    if !request.skip_planning {
        context.state.plan_conversation_key = Some(key);
        manager.save(&context.state)?;
    }
    Ok(config)
}

fn plan_stream_callback(
    sink: Arc<dyn ApplicationEventSink>,
    id: String,
    stream: EventStream,
    failure: Arc<Mutex<Option<String>>>,
) -> impl Fn(&str) + Send + Sync + 'static {
    move |text: &str| {
        if let Err(error) = sink.send(ApplicationEvent::PlanChunk {
            session_id: id.clone(),
            stream,
            text: text.to_string(),
        }) {
            let mut failure = failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if failure.is_none() {
                *failure = Some(error.to_string());
            }
        }
    }
}

fn plan_checkpoint_callback(
    manager: SessionManager,
    id: String,
    key: String,
    token: CancellationToken,
) -> impl Fn(&str) -> Result<()> + Send + Sync + 'static {
    move |backend_id: &str| {
        if token.is_cancelled() {
            return Err(CruiseError::Interrupted);
        }
        let mut state = manager.load(&id)?;
        state.plan_conversation_id = Some(backend_id.to_string());
        state.plan_conversation_key = Some(key.clone());
        manager.save(&state)
    }
}

async fn run_plan_prompt(
    runtime: &Arc<ApplicationRuntime>,
    manager: &SessionManager,
    context: &mut PlanContext,
    request: &PlanRequest,
    operation: OperationKind,
    config: &crate::config::WorkflowConfig,
    sink: &Arc<dyn ApplicationEventSink>,
) -> Result<()> {
    let ask = Arc::new(RuntimeAskHandler {
        runtime: Arc::clone(runtime),
        sink: Arc::clone(sink),
        session_id: context.state.id.clone(),
        claim_identity: context.claim.identity(),
        token: context.claim.token(),
    });
    let planning_interactive =
        request.interactive.is_enabled() && crate::planning::sdk_plan_tools_enabled(config);
    let mut vars = crate::planning::setup_plan_vars(
        context.state.input_with_attachments(),
        context.staged_plan_path.clone(),
        config,
    );
    if let Some(input) = request.feedback.clone().or(request.question.clone()) {
        vars.set_prev_input(Some(input));
    }
    let stream_failure = Arc::new(Mutex::new(None::<String>));
    let on_stdout = plan_stream_callback(
        Arc::clone(sink),
        context.state.id.clone(),
        EventStream::Stdout,
        Arc::clone(&stream_failure),
    );
    let on_stderr = plan_stream_callback(
        Arc::clone(sink),
        context.state.id.clone(),
        EventStream::Stderr,
        Arc::clone(&stream_failure),
    );
    let streams = crate::step::prompt::StreamCallbacks {
        on_stdout: Some(&on_stdout),
        on_stderr: Some(&on_stderr),
    };
    let token = context.claim.token();
    let on_session_id = plan_checkpoint_callback(
        manager.clone(),
        context.state.id.clone(),
        context.key.clone().unwrap_or_default(),
        token.clone(),
    );
    let ctx = crate::planning::PlanPromptCtx {
        config,
        ask,
        plan_path: &context.staged_plan_path,
        interactive: planning_interactive,
        rate_limit_retries: request.rate_limit_retries,
        working_dir: Some(&context.state.base_dir),
        grill: request.grill,
        // Formal specifications are limited to Generate operations. CLI and
        // official GUI/TUI callers only set the flag for new-session planning,
        // while direct API Generate callers may use it for any eligible phase.
        // Replan, Fix, and Ask callers cannot opt in through this field.
        formal_spec: request.formal_spec && operation == OperationKind::Generate,
        on_session_id: Some(&on_session_id),
        cancel_token: Some(&token),
    };
    let template = match operation {
        OperationKind::Fix => crate::planning::fix_plan_template(config),
        OperationKind::Ask => crate::planning::ask_plan_template(config),
        OperationKind::Replan => crate::planning::plan_template(config),
        _ => crate::planning::initial_plan_template(config, request.grill),
    };
    let label = match operation {
        OperationKind::Fix => "[plan] fixing plan...",
        OperationKind::Ask => "[plan] answering question...",
        OperationKind::Replan => "[plan] replanning...",
        _ => "[plan] creating plan...",
    };
    let output = crate::planning::run_plan_prompt_template(
        &ctx,
        &mut vars,
        template,
        label,
        Some(&streams),
        &mut context.resume,
        operation != OperationKind::Ask,
    )
    .await?;
    if let Some(error) = stream_failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        return Err(CruiseError::Other(format!(
            "planning event delivery failed: {error}"
        )));
    }
    if operation == OperationKind::Ask {
        return Ok(());
    }
    let plan_content = crate::planning::resolve_generated_plan_content(
        &context.staged_plan_path,
        &output.output,
        &output.stderr,
        None,
    )?;
    crate::planning::write_plan_atomically(&context.staged_plan_path, plan_content.as_bytes())
        .map_err(|error| CruiseError::Other(format!("failed to stage generated plan: {error}")))
}

fn commit_generated_plan(
    manager: &SessionManager,
    operation: OperationKind,
    state: &SessionState,
    plan_path: &std::path::Path,
    staged_plan_path: &std::path::Path,
    old_plan: Option<&[u8]>,
) -> Result<()> {
    if operation != OperationKind::Ask {
        std::fs::rename(staged_plan_path, plan_path).map_err(|error| {
            CruiseError::Other(format!("failed to commit generated plan: {error}"))
        })?;
    }
    if let Err(error) = manager.save(state) {
        if let Some(bytes) = old_plan {
            crate::planning::write_plan_atomically(plan_path, bytes).map_err(|restore_error| {
                CruiseError::Other(format!(
                    "failed to restore plan after state save failure ({error}): {restore_error}"
                ))
            })?;
        } else if let Err(restore_error) = std::fs::remove_file(plan_path)
            && restore_error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(CruiseError::Other(format!(
                "failed to restore plan after state save failure ({error}): {restore_error}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn finish_plan_commit(
    manager: &SessionManager,
    runtime: &ApplicationRuntime,
    id: &str,
    operation: OperationKind,
    sink: &dyn ApplicationEventSink,
    context: &mut PlanContext,
) -> Result<SessionState> {
    let state_for_commit = context.state.clone();
    let plan_path = context.plan_path.clone();
    let staged_path = context.staged_plan_path.clone();
    let old_plan = context.old_plan.clone();
    let committed = runtime.commit_if_active(&context.claim, || {
        commit_generated_plan(
            manager,
            operation,
            &state_for_commit,
            &plan_path,
            &staged_path,
            old_plan.as_deref(),
        )
    });
    match committed {
        Ok(true) => {
            let _ = std::fs::remove_file(&context.staged_plan_path);
            sink.send(ApplicationEvent::PlanFinished {
                session_id: id.to_string(),
                phase: context.state.phase.label().to_string(),
            })?;
            Ok(context.state.clone())
        }
        Ok(false) => cancel_plan(manager, id, sink, context),
        Err(error) => fail_plan(manager, id, operation, sink, context, error),
    }
}

async fn execute_plan(
    manager: &SessionManager,
    runtime: &Arc<ApplicationRuntime>,
    context: &mut PlanContext,
    request: &PlanRequest,
    operation: OperationKind,
    sink: &Arc<dyn ApplicationEventSink>,
) -> Result<()> {
    let config = prepare_plan(manager, context, request).await?;
    if request.skip_planning {
        crate::planning::write_input_as_plan(
            &context.staged_plan_path,
            &context.state.input_with_attachments(),
        )
        .and_then(|_| context.state.use_input_as_plan())
    } else {
        run_plan_prompt(runtime, manager, context, request, operation, &config, sink).await
    }
}

fn finish_plan(
    manager: &SessionManager,
    runtime: &ApplicationRuntime,
    operation: OperationKind,
    request: &PlanRequest,
    sink: &dyn ApplicationEventSink,
    context: &mut PlanContext,
    result: Result<()>,
) -> Result<SessionState> {
    let id = context.state.id.clone();
    if let Err(error) = result {
        if matches!(&error, &CruiseError::Interrupted) {
            return cancel_plan(manager, &id, sink, context);
        }
        return fail_plan(manager, &id, operation, sink, context, error);
    }
    if context.claim.token().is_cancelled() {
        return cancel_plan(manager, &id, sink, context);
    }
    if request.skip_planning {
        context.state.plan_conversation_id = None;
        context.state.plan_conversation_key = None;
    } else {
        context.state.phase = if matches!(
            operation,
            OperationKind::Fix | OperationKind::Ask | OperationKind::Replan
        ) || matches!(
            context.before_state.phase,
            SessionPhase::Planned | SessionPhase::AwaitingApproval
        ) {
            context.before_state.phase.clone()
        } else {
            SessionPhase::AwaitingApproval
        };
        context.state.plan_conversation_key = context.key.clone();
        context.state.plan_conversation_id = context.resume.clone();
        context.state.clear_pending_input();
    }
    context.state.plan_error = None;
    if operation != OperationKind::Ask
        && let Ok(content) = std::fs::read_to_string(&context.staged_plan_path)
    {
        crate::metadata::refresh_session_title_from_plan(&mut context.state, &content);
    }
    finish_plan_commit(manager, runtime, &id, operation, sink, context)
}

impl CruiseApplication {
    /// Construct an application with the process session manager.
    #[must_use]
    pub fn new(manager: SessionManager) -> Self {
        let runtime = Arc::new(ApplicationRuntime::new(manager.clone()));
        Self { manager, runtime }
    }
    /// Read the persisted plan for a session.
    ///
    /// # Errors
    ///
    /// Returns an error when the session or its plan cannot be loaded.
    pub fn session_plan(&self, id: &str) -> Result<String> {
        // A planner keeps generated bytes in a private staging path until its
        // state commit. Serialize reads with the final rename/state save so a
        // same-process reader never observes an uncommitted plan.
        let _commit_gate = self
            .runtime
            .commit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = self.manager.load(id)?;
        std::fs::read_to_string(state.plan_path(&self.manager.sessions_dir()))
            .map_err(|e| CruiseError::Other(format!("failed to read plan for {id}: {e}")))
    }
    #[must_use]
    pub fn runtime(&self) -> Arc<ApplicationRuntime> {
        Arc::clone(&self.runtime)
    }

    /// List persisted, non-execution sessions.
    ///
    /// # Errors
    ///
    /// Returns an error when the session store cannot be listed or read.
    pub fn list_sessions(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .manager
            .list()?
            .into_iter()
            .filter(|state| !state.exec)
            .collect())
    }
    /// Read one persisted session.
    ///
    /// # Errors
    ///
    /// Returns an error when the session cannot be loaded.
    pub fn read_session(&self, id: &str) -> Result<SessionState> {
        self.manager.load(id)
    }
    #[must_use]
    pub fn discover_configs(&self) -> Vec<crate::configs::ConfigEntry> {
        crate::configs::list_user_configs()
    }

    #[must_use]
    pub fn discover_config_sources(
        &self,
        base_dir: &std::path::Path,
    ) -> Vec<crate::configs::ConfigEntry> {
        let mut entries = crate::configs::list_configs_in(base_dir);
        entries.extend(crate::configs::list_user_configs());
        entries.push(crate::configs::ConfigEntry {
            name: "Built-in default".to_string(),
            path: crate::new_session_history::BUILTIN_CONFIG_KEY.to_string(),
            description: None,
        });
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }
    /// Load application configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the application configuration cannot be loaded.
    pub fn app_config(&self) -> Result<crate::app_config::AppConfig> {
        crate::app_config::AppConfig::load()
    }
    /// Load the saved new-session draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the draft cannot be loaded.
    pub fn draft(&self) -> Result<Option<NewSessionDraft>> {
        NewSessionDraft::load()
    }
    /// Save a new-session draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the draft cannot be saved.
    pub fn save_draft(&self, draft: &NewSessionDraft) -> Result<()> {
        draft.with_fresh_timestamp().save()
    }
    /// Load new-session history.
    ///
    /// # Errors
    ///
    /// Returns an error when history cannot be loaded.
    pub fn history(&self) -> Result<NewSessionHistory> {
        NewSessionHistory::load()
    }

    /// Clear the saved new-session draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the draft cannot be cleared.
    pub fn clear_draft(&self) -> Result<()> {
        NewSessionDraft::clear()
    }
    /// Save application configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the application configuration cannot be saved.
    pub fn save_app_config(&self, config: &crate::app_config::AppConfig) -> Result<()> {
        config.save()
    }

    /// Load or rebuild a session execution DAG.
    ///
    /// # Errors
    ///
    /// Returns an error when the session, configuration, or workflow cannot be loaded or compiled.
    pub fn session_dag(&self, id: &str) -> Result<Option<crate::dag::ExecutionDag>> {
        let state = self.manager.load(id)?;
        let path = self.manager.dag_path(id);
        if state.has_dag && path.is_file() {
            match crate::dag::load_dag(&path) {
                Ok(dag) => return Ok(Some(dag)),
                Err(_) => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let config = self.manager.load_config(&state)?;
        let max_retries = crate::config::resolve_effective_max_retries(None, &config);
        crate::workflow::compile(config)
            .and_then(|compiled| crate::dag::build_dag(&compiled, max_retries))
            .map(Some)
    }

    /// Read the most recent session log lines.
    ///
    /// # Errors
    ///
    /// Returns an error when the log cannot be read for a reason other than it being absent.
    pub fn session_log(&self, id: &str, max_lines: Option<usize>) -> Result<String> {
        let limit = max_lines.unwrap_or(10_000);
        match read_log_tail(&self.manager.run_log_path(id), limit) {
            Ok(content) => Ok(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(CruiseError::Other(format!(
                "failed to read log for {id}: {error}"
            ))),
        }
    }

    /// List sessions eligible for Run All.
    ///
    /// # Errors
    ///
    /// Returns an error when the session store cannot be queried.
    pub fn run_all_candidates(&self) -> Result<Vec<SessionState>> {
        self.manager.run_all_candidates()
    }

    #[must_use]
    pub fn list_directory(&self, path: &str) -> Vec<DirectoryEntry> {
        let expanded = crate::new_session_history::expand_tilde(path);
        let Ok(entries) = std::fs::read_dir(expanded) else {
            return Vec::new();
        };
        let mut result = entries
            .flatten()
            .filter_map(|entry| {
                let file_type = entry.file_type().ok()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                (file_type.is_dir() && !name.starts_with('.')).then(|| DirectoryEntry {
                    name,
                    path: entry.path().to_string_lossy().into_owned(),
                })
            })
            .collect::<Vec<_>>();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result.truncate(50);
        result
    }

    /// List repositories visible to the GitHub CLI.
    ///
    /// # Errors
    ///
    /// Returns an error when the GitHub CLI cannot run or reports failure.
    pub async fn list_github_repositories(&self) -> Result<Vec<String>> {
        let output = crate::step::command::run_process_output_cancelled(
            "gh",
            &[
                "repo",
                "list",
                "--limit",
                "200",
                "--json",
                "nameWithOwner",
                "--jq",
                ".[].nameWithOwner",
            ],
            None,
            None,
        )
        .await?;
        if !output.status.success() {
            return Err(CruiseError::Other(format!(
                "gh repo list failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect())
    }

    /// Summarize recent new-session history.
    ///
    /// # Errors
    ///
    /// Returns an error when history cannot be loaded.
    pub fn new_session_history_summary(&self) -> Result<NewSessionHistorySummary> {
        let history = NewSessionHistory::load()?;
        let mut seen = std::collections::HashSet::new();
        let mut recent = Vec::new();
        let mut last_requested = None;
        let mut last_dir = None;
        for entry in &history.entries {
            if entry.working_dir.is_empty()
                || crate::new_session_history::is_temp_working_dir(&entry.working_dir)
            {
                continue;
            }
            if last_dir.is_none() {
                last_requested.clone_from(&entry.requested_config_path);
                last_dir = Some(entry.working_dir.clone());
            }
            if seen.insert(entry.working_dir.clone()) && recent.len() < 5 {
                recent.push(entry.working_dir.clone());
            }
        }
        Ok(NewSessionHistorySummary {
            last_requested_config_path: last_requested,
            last_working_dir: last_dir,
            recent_working_dirs: recent,
        })
    }

    /// Resolve workflow configuration and its session defaults.
    ///
    /// # Errors
    ///
    /// Returns an error when the working directory, configuration, or workflow is invalid.
    pub fn new_session_config_defaults(
        &self,
        base_dir: &std::path::Path,
        config_path: Option<&str>,
        repo: Option<&str>,
    ) -> Result<NewSessionConfigDefaults> {
        let base = if base_dir.as_os_str().is_empty() {
            std::env::current_dir()?
        } else {
            PathBuf::from(crate::new_session_history::expand_tilde(
                &base_dir.to_string_lossy(),
            ))
        };
        let config_path = config_path.map(crate::new_session_history::expand_tilde);
        let (yaml, source) = crate::resolver::resolve_config_in_dir(config_path.as_deref(), &base)?;
        let config = crate::resolver::resolve_workflow_config(&yaml, &source, &base)?;
        crate::config::validate_config(&config)?;
        let resolved_key = source.path().map_or_else(
            || crate::new_session_history::BUILTIN_CONFIG_KEY.to_string(),
            |path| crate::new_session_history::resolved_config_key_for_session(path),
        );
        let steps = crate::workflow::list_skippable_steps(&config)?;
        let after_pr_steps = crate::workflow::list_skippable_after_pr_steps(&config)?;
        let history = NewSessionHistory::load_best_effort();
        let base_string = base.to_string_lossy().into_owned();
        let default_skipped_steps =
            if let Some(repo) = repo.filter(|value| !value.trim().is_empty()) {
                history.latest_entry_for_scope(
                    crate::new_session_history::HistoryScope::Repo(repo),
                    &resolved_key,
                )
            } else {
                history.latest_entry_for_scope(
                    crate::new_session_history::HistoryScope::Directory(&base_string),
                    &resolved_key,
                )
            }
            .map(|entry| entry.skipped_steps.clone())
            .unwrap_or_default();
        Ok(NewSessionConfigDefaults {
            steps,
            after_pr_steps,
            default_skipped_steps,
            resolved_config_key: resolved_key,
        })
    }

    #[must_use]
    pub fn pending_prompts(&self, session_id: &str) -> Vec<PendingPrompt> {
        self.runtime.pending_prompts(session_id)
    }

    /// Update persisted session settings.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy, cannot be loaded, or settings cannot be saved.
    pub fn update_settings(
        &self,
        id: &str,
        request: SessionSettingsRequest,
    ) -> Result<SessionState> {
        let _claim = self.runtime.try_begin(id, OperationKind::Mutate)?;
        let current_step_update = match request.current_step_update {
            CurrentStepUpdateDto::Unchanged => CurrentStepUpdate::Unchanged,
            CurrentStepUpdateDto::Clear => CurrentStepUpdate::Clear,
            CurrentStepUpdateDto::Set(step) => CurrentStepUpdate::Set(step),
        };
        crate::session_edit::update_session_settings(
            &self.manager,
            id,
            SessionSettingsUpdate {
                config_path: request.config_path,
                skipped_steps: request.skipped_steps,
                current_step_update,
            },
        )
        .map(|(state, _)| state)
    }

    /// Update persisted session settings through the façade alias.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy or settings cannot be saved.
    pub fn edit_settings(&self, id: &str, request: SessionSettingsRequest) -> Result<SessionState> {
        self.update_settings(id, request)
    }

    /// Update the current workflow step for a session.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy, cannot be loaded, or the update cannot be saved.
    pub fn edit_current_step(
        &self,
        id: &str,
        update: CurrentStepUpdateDto,
    ) -> Result<SessionState> {
        let _claim = self.runtime.try_begin(id, OperationKind::Mutate)?;
        let state = self.manager.load(id)?;
        let current_step_update = match update {
            CurrentStepUpdateDto::Unchanged => CurrentStepUpdate::Unchanged,
            CurrentStepUpdateDto::Clear => CurrentStepUpdate::Clear,
            CurrentStepUpdateDto::Set(step) => CurrentStepUpdate::Set(step),
        };
        let config_path = state
            .config_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        crate::session_edit::update_session_settings(
            &self.manager,
            id,
            SessionSettingsUpdate {
                config_path,
                skipped_steps: state.skipped_steps,
                current_step_update,
            },
        )
        .map(|(state, _)| state)
    }

    /// Create and persist an editable Draft. Initial persistence happens before
    /// any planning Started event can be emitted.
    ///
    /// # Errors
    ///
    /// Returns an error when the input, repository, configuration, attachments,
    /// or session persistence is invalid.
    pub fn create_session(&self, mut request: NewSessionRequest) -> Result<SessionState> {
        if request.input.trim().is_empty() && request.attachments.is_empty() {
            return Err(CruiseError::Other(
                "session input cannot be empty".to_string(),
            ));
        }
        request.base_dir = PathBuf::from(crate::new_session_history::expand_tilde(
            &request.base_dir.to_string_lossy(),
        ));
        request.config_path = request.config_path.map(|path| {
            PathBuf::from(crate::new_session_history::expand_tilde(
                &path.to_string_lossy(),
            ))
        });
        request.attachments = request
            .attachments
            .into_iter()
            .map(|path| {
                PathBuf::from(crate::new_session_history::expand_tilde(
                    &path.to_string_lossy(),
                ))
            })
            .collect();
        let id = SessionManager::new_session_id();
        let manager = self.manager.clone();
        let result = create_session_inner(&manager, request, &id);
        if result.is_err() {
            let _ = manager.delete(&id);
            let clone = manager.clones_dir().join(&id);
            let _ = std::fs::remove_dir_all(clone);
        }
        result
    }

    /// Approve a planned session.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy, not awaiting approval, has no usable plan, or cannot be saved.
    pub fn approve(&self, id: &str) -> Result<SessionState> {
        let _claim = self.runtime.try_begin(id, OperationKind::Mutate)?;
        let mut state = self.manager.load(id)?;
        if !matches!(state.phase, SessionPhase::AwaitingApproval) {
            return Err(CruiseError::Other(format!(
                "session is in '{}' phase and cannot be approved",
                state.phase.label()
            )));
        }
        if state.plan_error.is_some() || !state.has_usable_plan(&self.manager.sessions_dir()) {
            return Err(CruiseError::Other(
                "session has no usable plan to approve".to_string(),
            ));
        }
        state.approve();
        crate::repo_clone::cleanup_after_approval_checked(&self.manager, &mut state)?;
        self.manager.save(&state)?;
        Ok(state)
    }
    /// Respond to a pending ask prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when the prompt is stale, invalid, or no longer active.
    pub fn respond_to_ask(&self, session_id: &str, request_id: &str, answer: String) -> Result<()> {
        self.runtime.respond_to_ask(session_id, request_id, answer)
    }

    /// Respond to a pending option prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when the prompt is stale, invalid, or no longer active.
    pub fn respond_to_option(
        &self,
        session_id: &str,
        request_id: &str,
        result: OptionResult,
    ) -> Result<()> {
        self.runtime
            .respond_to_option(session_id, request_id, result)
    }

    /// Use the session input as its plan.
    ///
    /// # Errors
    ///
    /// Returns an error when the session claim, plan staging, persistence, or
    /// event delivery fails.
    pub fn use_input_as_plan(
        &self,
        id: &str,
        sink: &dyn ApplicationEventSink,
    ) -> Result<SessionState> {
        let claim = self.runtime.try_begin(id, OperationKind::Generate)?;
        let state = self.manager.load(id)?;
        use_input_as_plan_inner(&self.manager, &self.runtime, id, &claim, state, sink)
    }

    /// Reset a session to its planned state.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy, ineligible for reset, or cannot be saved.
    pub fn reset_to_planned(&self, id: &str) -> Result<SessionState> {
        let _claim = self.runtime.try_begin(id, OperationKind::Mutate)?;
        let mut state = self.manager.load(id)?;
        if !matches!(
            state.phase,
            SessionPhase::Failed(_)
                | SessionPhase::Suspended
                | SessionPhase::Completed
                | SessionPhase::Planned
        ) {
            return Err(CruiseError::Other(format!(
                "session is in '{}' phase and cannot be reset",
                state.phase.label()
            )));
        }
        state.reset_to_planned();
        self.manager.save(&state)?;
        Ok(state)
    }

    /// Delete a persisted session and its execution workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy, running, or cannot be cleaned up or deleted.
    pub fn delete_session(&self, id: &str) -> Result<()> {
        let _claim = self.runtime.try_begin(id, OperationKind::Mutate)?;
        let state = self.manager.load(id)?;
        if matches!(state.phase, SessionPhase::Running) {
            return Err(CruiseError::Busy(format!("session {id} is running")));
        }
        self.cleanup_session_workspace(&state)?;
        self.manager.delete(id)
    }

    /// Discard a session awaiting approval.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy, not awaiting approval, or cannot be cleaned up or deleted.
    pub fn discard_session(&self, id: &str) -> Result<()> {
        let _claim = self.runtime.try_begin(id, OperationKind::Mutate)?;
        let state = self.manager.load(id)?;
        if !matches!(state.phase, SessionPhase::AwaitingApproval) {
            return Err(CruiseError::Other(format!(
                "session is in '{}' phase and cannot be discarded",
                state.phase.label()
            )));
        }
        self.cleanup_session_workspace(&state)?;
        self.manager.delete(id)
    }

    fn cleanup_session_workspace(&self, state: &SessionState) -> Result<()> {
        if state.repo.is_some() {
            crate::repo_clone::cleanup_session_workspace(&self.manager, state)
        } else if let (Some(path), Some(branch)) = (&state.worktree_path, &state.worktree_branch) {
            crate::worktree::cleanup_worktree(&crate::worktree::WorktreeContext {
                path: path.clone(),
                branch: branch.clone(),
                original_dir: state.base_dir.clone(),
            })
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn cancel_session(&self, id: &str) -> bool {
        self.runtime.cancel_session(id)
    }
    #[must_use]
    pub fn cancel_run_all(&self) -> bool {
        self.runtime.cancel_batch(None)
    }

    #[must_use]
    pub fn capabilities(&self, state: &SessionState) -> Vec<SessionAction> {
        let usable_plan =
            state.plan_error.is_none() && state.has_usable_plan(&self.manager.sessions_dir());
        let mut actions = Vec::new();
        match &state.phase {
            SessionPhase::Draft => actions.extend([SessionAction::Generate, SessionAction::Delete]),
            SessionPhase::AwaitingInput => {
                if state.awaiting_input {
                    actions.push(SessionAction::Answer);
                }
                actions.push(SessionAction::Generate);
                if self.runtime.active_identity(&state.id).is_some() {
                    actions.push(SessionAction::Cancel);
                }
                actions.push(SessionAction::Delete);
            }
            SessionPhase::AwaitingApproval => {
                if usable_plan {
                    actions.extend([
                        SessionAction::Approve,
                        SessionAction::Publish,
                        SessionAction::Fix,
                        SessionAction::Ask,
                    ]);
                } else {
                    actions.extend([SessionAction::Fix, SessionAction::Generate]);
                }
                actions.push(SessionAction::Discard);
            }
            SessionPhase::Planned => {
                if usable_plan {
                    actions.push(SessionAction::RunWorktree);
                    if state.repo.is_none() {
                        actions.push(SessionAction::RunCurrentBranch);
                    }
                    actions.push(SessionAction::Publish);
                }
                actions.extend([
                    SessionAction::Replan,
                    SessionAction::EditSettings,
                    SessionAction::Delete,
                ]);
            }
            SessionPhase::Running => {
                if self.runtime.active_identity(&state.id).is_some() {
                    actions.push(SessionAction::Cancel);
                }
            }
            SessionPhase::Failed(_) => actions.extend([
                SessionAction::Retry,
                SessionAction::ResetToPlanned,
                SessionAction::EditSettings,
                SessionAction::EditCurrentStep,
                SessionAction::Delete,
            ]),
            SessionPhase::Suspended => actions.extend([
                SessionAction::Resume,
                SessionAction::ResetToPlanned,
                SessionAction::EditSettings,
                SessionAction::EditCurrentStep,
                SessionAction::Delete,
            ]),
            SessionPhase::Completed => {
                if state.pr_url.is_some() {
                    actions.push(SessionAction::OpenPr);
                }
                actions.extend([SessionAction::ResetToPlanned, SessionAction::Delete]);
            }
        }
        let planning_claim = matches!(
            self.runtime.active_operation(&state.id),
            Some(
                OperationKind::Generate
                    | OperationKind::Fix
                    | OperationKind::Ask
                    | OperationKind::Replan
                    | OperationKind::Run
            )
        );
        if planning_claim && !actions.contains(&SessionAction::Cancel) {
            actions.push(SessionAction::Cancel);
        }
        actions
    }

    async fn plan_with_operation(
        &self,
        id: &str,
        request: PlanRequest,
        sink: Arc<dyn ApplicationEventSink>,
        operation: OperationKind,
    ) -> Result<SessionState> {
        let _quiet = crate::console_mode::quiet_guard();
        let mut context = start_plan_context(&self.manager, &self.runtime, id, operation, &*sink)?;
        let result = execute_plan(
            &self.manager,
            &self.runtime,
            &mut context,
            &request,
            operation,
            &sink,
        )
        .await;
        finish_plan(
            &self.manager,
            &self.runtime,
            operation,
            &request,
            &*sink,
            &mut context,
            result,
        )
    }
    /// Apply feedback to a session plan.
    ///
    /// # Errors
    ///
    /// Returns an error when planning, session persistence, or event delivery fails.
    pub async fn fix(
        &self,
        id: &str,
        feedback: String,
        sink: Arc<dyn ApplicationEventSink>,
    ) -> Result<SessionState> {
        let request = PlanRequest {
            feedback: Some(feedback),
            interactive: Interactive::new(true),
            ..PlanRequest::default()
        };
        self.plan_with_operation(id, request, sink, OperationKind::Fix)
            .await
    }

    /// Ask a planning question and update the session plan.
    ///
    /// # Errors
    ///
    /// Returns an error when the question is empty or planning, persistence, or event delivery fails.
    pub async fn ask(
        &self,
        id: &str,
        question: String,
        sink: Arc<dyn ApplicationEventSink>,
    ) -> Result<SessionState> {
        if question.trim().is_empty() {
            return Err(CruiseError::Other("question must not be empty".to_string()));
        }
        let request = PlanRequest {
            question: Some(question),
            ..PlanRequest::default()
        };
        self.plan_with_operation(id, request, sink, OperationKind::Ask)
            .await
    }

    /// Replan a session using the supplied request.
    ///
    /// # Errors
    ///
    /// Returns an error when planning, session persistence, or event delivery fails.
    pub async fn replan(
        &self,
        id: &str,
        request: PlanRequest,
        sink: Arc<dyn ApplicationEventSink>,
    ) -> Result<SessionState> {
        self.plan_with_operation(id, request, sink, OperationKind::Replan)
            .await
    }
    /// Generate a session plan.
    ///
    /// # Errors
    ///
    /// Returns an error when planning, session persistence, or event delivery fails.
    pub async fn generate(
        &self,
        id: &str,
        request: PlanRequest,
        sink: Arc<dyn ApplicationEventSink>,
    ) -> Result<SessionState> {
        self.plan_with_operation(id, request, sink, OperationKind::Generate)
            .await
    }

    /// Reconcile persisted running state with the active operation registry.
    ///
    /// # Errors
    ///
    /// Returns an error when the session cannot be loaded.
    pub fn reconcile_session(&self, id: &str) -> Result<SessionState> {
        let mut state = self.manager.load(id)?;
        self.manager
            .reconcile_running_phase(&mut state, self.runtime.active_identity(id).is_some());
        Ok(state)
    }

    /// Publish a session plan as an issue.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy, not publishable, or publishing fails.
    pub fn publish(
        &self,
        id: &str,
        trigger_cruise: bool,
    ) -> Result<crate::issue_publish::PublishedIssue> {
        let _claim = self.runtime.try_begin(id, OperationKind::Mutate)?;
        let state = self.manager.load(id)?;
        if !matches!(
            state.phase,
            SessionPhase::AwaitingApproval | SessionPhase::Planned
        ) {
            return Err(CruiseError::Other(format!(
                "session is in '{}' phase and cannot be published",
                state.phase.label()
            )));
        }
        if state.plan_error.is_some() || !state.has_usable_plan(&self.manager.sessions_dir()) {
            return Err(CruiseError::Other(
                "session has no usable plan to publish".to_string(),
            ));
        }
        crate::issue_publish::publish_plan_issue_and_delete(&self.manager, state, trigger_cruise)
    }

    /// Clean sessions whose pull requests have been handled.
    ///
    /// # Errors
    ///
    /// Returns an error when sessions cannot be listed or claims cannot be acquired.
    pub fn clean(&self) -> Result<crate::session::CleanupReport> {
        let _quiet = crate::console_mode::quiet_guard();
        let mut skipped = std::collections::HashSet::new();
        let mut claims = Vec::new();
        for state in self.manager.list()? {
            match self.runtime.try_begin(&state.id, OperationKind::Mutate) {
                Ok(claim) => claims.push(claim),
                Err(CruiseError::Busy(_)) => {
                    skipped.insert(state.id);
                }
                Err(error) => return Err(error),
            }
        }
        let report = self.manager.cleanup_by_pr_status_skipping(&skipped);
        drop(claims);
        report
    }

    /// Run one eligible session while forwarding log events to an optional sink.
    ///
    /// # Errors
    ///
    /// Returns an error when parallelism, session setup, execution, or event delivery fails.
    pub async fn run_all_with_parallelism_provider<P>(
        &self,
        parallelism_provider: P,
        sink: Arc<dyn ApplicationEventSink>,
        log_sink: Option<Arc<dyn LogSink>>,
    ) -> Result<Vec<SessionState>>
    where
        P: Fn() -> Result<usize> + Send + Sync + 'static,
    {
        let parallelism = batch_parallelism(&parallelism_provider)?;
        let batch = self.runtime.try_begin_batch()?;
        let candidates = self.manager.run_all_candidates()?;
        let candidate_map = candidates
            .iter()
            .map(|state| (state.id.clone(), state.clone()))
            .collect::<HashMap<_, _>>();
        let reservation = batch.reserve(&candidates);
        let mut run = BatchRunState {
            candidate_total: reservation.reserved.len(),
            queued: std::collections::VecDeque::new(),
            seen: candidates.iter().map(|state| state.id.clone()).collect(),
            running: tokio::task::JoinSet::new(),
            finished: Vec::new(),
            next_index: 0,
        };
        sink.send(ApplicationEvent::BatchStarted {
            total: run.candidate_total,
            parallelism,
        })?;
        for id in &reservation.busy {
            sink.send(ApplicationEvent::BatchSessionFinished {
                id: id.clone(),
                phase: "Busy".to_string(),
                error: Some("session is already busy".to_string()),
            })?;
        }
        for claim in reservation.reserved {
            if let Some(state) = candidate_map.get(claim.session_id()).cloned() {
                run.queued.push_back((claim, state));
            }
        }
        run_batch_loop(
            self,
            &batch,
            &parallelism_provider,
            &sink,
            log_sink.as_ref(),
            &mut run,
        )
        .await?;
        sink.send(ApplicationEvent::BatchFinished {
            cancelled: batch.token().is_cancelled(),
        })?;
        run.finished.sort_by_key(|(index, _)| *index);
        Ok(run.finished.into_iter().map(|(_, state)| state).collect())
    }
    /// Run one session while forwarding log events to an optional sink.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is busy or execution, persistence, or event delivery fails.
    pub async fn run_with_log_sink(
        &self,
        id: &str,
        request: RunRequest,
        sink: Arc<dyn ApplicationEventSink>,
        log_sink: Option<Arc<dyn LogSink>>,
    ) -> Result<SessionState> {
        let claim = self.runtime.try_begin(id, OperationKind::Run)?;
        self.run_claimed(id, &claim, request, sink, log_sink, false)
            .await
    }

    /// Return the pull request URL recorded for a session.
    ///
    /// # Errors
    ///
    /// Returns an error when the session cannot be loaded or has no pull request URL.
    pub fn open_pr(&self, id: &str) -> Result<String> {
        let state = self.manager.load(id)?;
        state
            .pr_url
            .ok_or_else(|| CruiseError::Other(format!("session {id} has no pull request")))
    }
    fn persist_setup_failure(
        &self,
        id: &str,
        error: CruiseError,
        sink: &Arc<dyn ApplicationEventSink>,
    ) -> Result<SessionState> {
        let mut state = self.manager.load(id)?;
        let message = error.to_string();
        if matches!(&error, CruiseError::Interrupted) {
            state.phase = SessionPhase::Suspended;
            state.clear_pending_input();
            state.clear_runner();
            self.manager.save(&state)?;
            sink.send(ApplicationEvent::RunCancelled {
                session_id: id.to_string(),
            })?;
        } else {
            state.phase = SessionPhase::Failed(message.clone());
            state.completed_at = Some(crate::session::current_iso8601());
            state.clear_pending_input();
            state.clear_runner();
            self.manager.save(&state)?;
            sink.send(ApplicationEvent::RunFailed {
                session_id: id.to_string(),
                error: message,
            })?;
        }
        Err(error)
    }

    async fn run_claimed(
        &self,
        id: &str,
        claim: &OperationClaim,
        request: RunRequest,
        sink: Arc<dyn ApplicationEventSink>,
        log_sink: Option<Arc<dyn LogSink>>,
        batch_started: bool,
    ) -> Result<SessionState> {
        let exec_initial = self
            .manager
            .load_with_fingerprint(id)
            .ok()
            .and_then(|(state, fingerprint)| state.exec.then_some((state.phase, fingerprint)));
        let result = self
            .run_claimed_inner(id, claim, request, sink, log_sink, batch_started)
            .await;
        if let Some((initial_phase, fingerprint)) = exec_initial {
            self.manager
                .dispose_exec_session_if_owned(id, &initial_phase, fingerprint);
        }
        result
    }

    async fn run_claimed_inner(
        &self,
        id: &str,
        claim: &OperationClaim,
        request: RunRequest,
        sink: Arc<dyn ApplicationEventSink>,
        log_sink: Option<Arc<dyn LogSink>>,
        batch_started: bool,
    ) -> Result<SessionState> {
        let _quiet = crate::console_mode::quiet_guard();
        let mut setup = match prepare_run_setup(&self.manager, id, claim, &request).await {
            Ok(setup) => setup,
            Err(error) => return self.persist_setup_failure(id, error, &sink),
        };
        if claim.token().is_cancelled() {
            let cleanup = cleanup_new_execution_workspace(
                &self.manager,
                &mut setup.state,
                &setup.workspace,
                setup.repo_clone_created,
            );
            setup.state.phase = SessionPhase::Suspended;
            setup.state.clear_pending_input();
            setup.state.clear_runner();
            self.manager.save(&setup.state)?;
            sink.send(ApplicationEvent::RunCancelled {
                session_id: id.to_string(),
            })?;
            let _ = cleanup;
            return Err(CruiseError::Interrupted);
        }
        let mut inputs = match prepare_run_inputs(&self.manager, &mut setup) {
            Ok(inputs) => inputs,
            Err(error) => {
                let _ = cleanup_new_execution_workspace(
                    &self.manager,
                    &mut setup.state,
                    &setup.workspace,
                    setup.repo_clone_created,
                );
                return self.persist_setup_failure(id, error, &sink);
            }
        };
        emit_run_started(&self.manager, id, &*sink, &mut setup, batch_started)?;
        let result = execute_run(
            &self.manager,
            claim,
            &mut setup,
            &mut inputs,
            sink.clone(),
            log_sink,
            batch_started,
        )
        .await;
        finish_run(
            &self.manager,
            &self.runtime,
            id,
            claim,
            &*sink,
            &mut setup,
            result,
        )
    }
}

struct RunSetup {
    state: SessionState,
    compiled: crate::workflow::CompiledWorkflow,
    workspace: crate::workspace::ExecutionWorkspace,
    repo_clone_created: bool,
    max_retries: usize,
    rate_limit_retries: usize,
    retry_policy: Option<Arc<crate::retry::RetryPolicy>>,
    dag: crate::dag::ExecutionDag,
}

fn prepare_run_workspace(
    manager: &SessionManager,
    runtime_handle: &tokio::runtime::Handle,
    state: &mut SessionState,
    workspace_mode: WorkspaceMode,
    token: &CancellationToken,
) -> Result<(crate::workspace::ExecutionWorkspace, bool)> {
    let repo_clone_created = if state.repo.is_some() && !state.exec {
        runtime_handle.block_on(crate::repo_clone::ensure_repo_session_workspace_cancelled(
            manager, state, token,
        ))?
    } else {
        false
    };
    let workspace =
        match crate::workspace::prepare_execution_workspace(manager, state, workspace_mode) {
            Ok(workspace) => workspace,
            Err(error) => {
                if workspace_mode == WorkspaceMode::Worktree {
                    let _ = std::fs::remove_dir_all(manager.worktrees_dir().join(&state.id));
                }
                if repo_clone_created {
                    let _ = std::fs::remove_dir_all(manager.clones_dir().join(&state.id));
                }
                return Err(error);
            }
        };
    Ok((workspace, repo_clone_created))
}

fn restore_runtime_dag(
    manager: &SessionManager,
    state: &mut SessionState,
    dag: &mut crate::dag::ExecutionDag,
    id: &str,
) {
    let dag_path = manager.dag_path(id);
    if state.has_dag && dag_path.is_file() {
        if let Ok(persisted) = crate::dag::load_dag(&dag_path) {
            dag.adopt_runtime_from(&persisted);
        } else {
            state.has_dag = false;
            let _ = std::fs::remove_file(&dag_path);
            if state.current_step_is_node_id {
                state.current_step = None;
                state.current_step_is_node_id = false;
            }
        }
    } else if state.current_step_is_node_id {
        state.current_step = None;
        state.current_step_is_node_id = false;
    }
}

fn setup_run_blocking(
    manager: &SessionManager,
    runtime_handle: &tokio::runtime::Handle,
    id: &str,
    token: &CancellationToken,
    requested_mode: Option<WorkspaceMode>,
    max_retries_override: Option<usize>,
    rate_limit_retries: usize,
) -> Result<RunSetup> {
    let mut state = manager.load(id)?;
    if !matches!(
        state.phase,
        SessionPhase::Planned | SessionPhase::Suspended | SessionPhase::Failed(_)
    ) {
        return Err(CruiseError::Other(format!("session {id} is not runnable")));
    }
    if state.plan_error.is_some() || !state.has_usable_plan(&manager.sessions_dir()) {
        return Err(CruiseError::Other(format!(
            "session {id} has no usable plan; retry planning first"
        )));
    }
    if token.is_cancelled() {
        return Err(CruiseError::Interrupted);
    }
    let config = manager.load_config(&state)?;
    crate::config::validate_config(&config)?;
    let max_retries = max_retries_override
        .unwrap_or_else(|| crate::config::resolve_effective_max_retries(None, &config));
    crate::config::validate_group_retry_budget(&config, max_retries)?;
    let retry_policy = crate::retry::policy_for_config(config.retry.clone());
    let compiled = crate::workflow::compile(config)?;
    let workspace_mode = if state.exec {
        WorkspaceMode::CurrentBranch
    } else if state.repo.is_some() {
        WorkspaceMode::Worktree
    } else {
        requested_mode.unwrap_or(state.workspace_mode)
    };
    if workspace_mode == WorkspaceMode::Worktree {
        crate::worktree_pr::ensure_gh_available()?;
    }
    let (workspace, repo_clone_created) =
        prepare_run_workspace(manager, runtime_handle, &mut state, workspace_mode, token)?;
    crate::workspace::update_session_workspace(&mut state, &workspace);
    state.workspace_mode = workspace_mode;
    let mut dag = match crate::dag::build_dag(&compiled, max_retries) {
        Ok(dag) => dag,
        Err(error) => {
            let _ = cleanup_new_execution_workspace(
                manager,
                &mut state,
                &workspace,
                repo_clone_created,
            );
            return Err(error);
        }
    };
    restore_runtime_dag(manager, &mut state, &mut dag, id);
    Ok(RunSetup {
        state,
        compiled,
        workspace,
        repo_clone_created,
        max_retries,
        rate_limit_retries,
        retry_policy,
        dag,
    })
}

async fn prepare_run_setup(
    manager: &SessionManager,
    id: &str,
    claim: &OperationClaim,
    request: &RunRequest,
) -> Result<RunSetup> {
    let runtime_handle = tokio::runtime::Handle::current();
    let setup_manager = manager.clone();
    let setup_token = claim.token();
    let setup_id = id.to_string();
    let requested_mode = request.workspace_mode;
    let max_retries_override = request.max_retries;
    let rate_limit_retries = request.rate_limit_retries;
    tokio::task::spawn_blocking(move || {
        setup_run_blocking(
            &setup_manager,
            &runtime_handle,
            &setup_id,
            &setup_token,
            requested_mode,
            max_retries_override,
            rate_limit_retries,
        )
    })
    .await
    .map_err(|error| CruiseError::Other(format!("run setup worker failed: {error}")))?
}
struct RunInputs {
    start: String,
    vars: crate::variable::VariableStore,
    tracker: crate::file_tracker::FileTracker,
    skipped_steps: Vec<String>,
}

fn prepare_run_inputs(manager: &SessionManager, setup: &mut RunSetup) -> Result<RunInputs> {
    let start = match setup.state.current_step.as_deref() {
        Some(step) if setup.state.current_step_is_node_id && setup.dag.nodes.contains_key(step) => {
            step.to_string()
        }
        Some(_) if setup.state.current_step_is_node_id => {
            setup.state.current_step = None;
            setup.state.current_step_is_node_id = false;
            setup.dag.start.clone()
        }
        Some(step) => setup
            .dag
            .first_node_for_step(step)
            .cloned()
            .unwrap_or_else(|| setup.dag.start.clone()),
        None => setup.dag.start.clone(),
    };
    let plan_path = setup.state.plan_path(&manager.sessions_dir());
    let mut vars = crate::variable::VariableStore::new(setup.state.input_with_attachments());
    vars.set_named_file(crate::session::PLAN_VAR, plan_path);
    let mut tracker =
        crate::file_tracker::FileTracker::with_root(setup.workspace.path().to_path_buf());
    if let Some(node) = setup.dag.nodes.get(&start) {
        vars.set_prev_output(node.runtime.prev_output.clone());
        vars.set_prev_input(node.runtime.prev_input.clone());
        vars.set_prev_stderr(node.runtime.prev_stderr.clone());
        vars.set_prev_success(node.runtime.prev_success);
        tracker.restore_snapshots(node.runtime.file_snapshots.clone());
    }
    setup.state.phase = SessionPhase::Running;
    setup.state.set_runner_to_current_process();
    manager.save(&setup.state)?;
    Ok(RunInputs {
        start,
        vars,
        tracker,
        skipped_steps: setup.state.skipped_steps.clone(),
    })
}

fn emit_run_started(
    manager: &SessionManager,
    id: &str,
    sink: &dyn ApplicationEventSink,
    setup: &mut RunSetup,
    batch_started: bool,
) -> Result<()> {
    if batch_started
        && let Err(error) = sink.send(ApplicationEvent::BatchSessionStarted { id: id.to_string() })
    {
        setup.state.phase = SessionPhase::Failed(error.to_string());
        setup.state.completed_at = Some(crate::session::current_iso8601());
        setup.state.clear_pending_input();
        let _ = manager.save(&setup.state);
        return Err(error);
    }
    if let Err(error) = sink.send(ApplicationEvent::RunStarted {
        session_id: id.to_string(),
    }) {
        setup.state.phase = SessionPhase::Failed(error.to_string());
        setup.state.completed_at = Some(crate::session::current_iso8601());
        setup.state.clear_pending_input();
        let _ = manager.save(&setup.state);
        return Err(error);
    }
    Ok(())
}

fn run_log_callback(
    logger: crate::session::SessionLogger,
    id: String,
    log_sink: Option<Arc<dyn LogSink>>,
    batch_started: bool,
) -> impl Fn(&str, &str) + Send + Sync + 'static {
    move |stream: &str, text: &str| {
        let stream_kind = match stream {
            "stdout" => EventStream::Stdout,
            "stderr" => EventStream::Stderr,
            _ => EventStream::Info,
        };
        logger.write(&format!("[{stream}] {text}"));
        if let Some(log_sink) = log_sink.as_ref() {
            let _ = log_sink.try_send(LogEvent {
                session_id: Some(id.clone()),
                stream: stream_kind,
                text: text.to_string(),
                batch: batch_started,
            });
        }
    }
}

fn node_checkpoint_callback(
    manager: SessionManager,
    id: String,
    path: PathBuf,
) -> impl for<'a> Fn(&crate::engine::NodeCheckpoint<'a>, &crate::dag::ExecutionDag) -> Result<()>
+ Send
+ Sync
+ 'static {
    move |checkpoint, checkpoint_dag| {
        crate::dag::save_dag(checkpoint_dag, &path)?;
        let mut state = manager.load(&id)?;
        state.current_step = Some(checkpoint.node_id.clone());
        state.current_step_is_node_id = true;
        state.has_dag = true;
        manager.save(&state)
    }
}

async fn run_engine_steps(
    ctx: &crate::engine::ExecutionContext<'_>,
    retry_policy: Option<&Arc<crate::retry::RetryPolicy>>,
    vars: &mut crate::variable::VariableStore,
    tracker: &mut crate::file_tracker::FileTracker,
    dag: &mut crate::dag::ExecutionDag,
    start: &crate::dag::NodeId,
    on_node_start: &(
         dyn Fn(&crate::engine::NodeCheckpoint<'_>, &crate::dag::ExecutionDag) -> Result<()>
             + Send
             + Sync
     ),
) -> Result<()> {
    crate::retry::with_active_policy(
        retry_policy.cloned(),
        crate::engine::execute_steps_with_dag(ctx, vars, tracker, dag, start, on_node_start),
    )
    .await
    .map(|_| ())
}

struct WorktreeRun<'a> {
    manager: &'a SessionManager,
    sink: &'a Arc<dyn ApplicationEventSink>,
    worktree: &'a crate::worktree::WorktreeContext,
    claim: &'a OperationClaim,
    option: &'a RuntimeOptionHandler,
}

impl WorktreeRun<'_> {
    async fn execute(
        self,
        setup: &mut RunSetup,
        inputs: &mut RunInputs,
        on_log: &crate::step::command::StepLogCallback<'_>,
    ) -> Result<()> {
        self.sink.send(ApplicationEvent::RunPhase {
            session_id: setup.state.id.clone(),
            phase: "Creating PR".to_string(),
        })?;
        let pr_sink = Arc::clone(self.sink);
        let manager_for_pr = self.manager.clone();
        let persist_pr = move |snapshot: &SessionState| {
            manager_for_pr.save(snapshot)?;
            let url = snapshot.pr_url.clone().ok_or_else(|| {
                CruiseError::Other("PR creation completed without a URL".to_string())
            })?;
            pr_sink.send(ApplicationEvent::PrCreated {
                session_id: snapshot.id.clone(),
                url,
            })
        };
        crate::retry::with_active_policy(
            setup.retry_policy.clone(),
            crate::worktree_pr::handle_worktree_pr_with_persistence(
                self.worktree,
                &setup.compiled,
                &mut inputs.vars,
                &mut inputs.tracker,
                &mut setup.state,
                setup.rate_limit_retries,
                setup.max_retries,
                &inputs.skipped_steps,
                Some(&self.claim.token),
                self.option,
                Some(on_log),
                Some(&persist_pr),
            ),
        )
        .await
    }
}

async fn execute_run(
    manager: &SessionManager,
    claim: &OperationClaim,
    setup: &mut RunSetup,
    inputs: &mut RunInputs,
    sink: Arc<dyn ApplicationEventSink>,
    log_sink: Option<Arc<dyn LogSink>>,
    batch_started: bool,
) -> Result<()> {
    let id = claim.session_id();
    let option = RuntimeOptionHandler {
        runtime: Arc::clone(&claim.runtime),
        sink: Arc::clone(&sink),
        session_id: id.to_string(),
        claim_identity: claim.identity(),
        token: claim.token(),
    };
    let on_start = |step: &str| {
        sink.send(ApplicationEvent::StepStarted {
            session_id: id.to_string(),
            step: step.to_string(),
        })
    };
    let logger = crate::session::SessionLogger::new(manager.run_log_path(id));
    let on_log = run_log_callback(logger, id.to_string(), log_sink, batch_started);
    let on_node_start =
        node_checkpoint_callback(manager.clone(), id.to_string(), manager.dag_path(id));
    let execution = {
        let ctx = crate::engine::ExecutionContext {
            compiled: &setup.compiled,
            max_retries: setup.max_retries,
            rate_limit_retries: setup.rate_limit_retries,
            cancel_token: Some(&claim.token),
            option_handler: &option,
            config_reloader: None,
            working_dir: Some(setup.workspace.path()),
            skipped_steps: &inputs.skipped_steps,
            on_step_log: Some(&on_log),
            on_step_start: &on_start,
        };
        run_engine_steps(
            &ctx,
            setup.retry_policy.as_ref(),
            &mut inputs.vars,
            &mut inputs.tracker,
            &mut setup.dag,
            &inputs.start,
            &on_node_start,
        )
        .await
    };
    if let Ok(saved) = manager.load(id) {
        setup.state = saved;
    }
    match execution {
        Err(error) => Err(error),
        Ok(()) if claim.token().is_cancelled() => Err(CruiseError::Interrupted),
        Ok(()) => {
            let worktree = match &setup.workspace {
                crate::workspace::ExecutionWorkspace::CurrentBranch { .. } => return Ok(()),
                crate::workspace::ExecutionWorkspace::Worktree { ctx, .. } => ctx.clone(),
            };
            WorktreeRun {
                manager,
                sink: &sink,
                worktree: &worktree,
                claim,
                option: &option,
            }
            .execute(setup, inputs, &on_log)
            .await
        }
    }
}
fn cancel_run(
    manager: &SessionManager,
    id: &str,
    sink: &dyn ApplicationEventSink,
    setup: &mut RunSetup,
) -> Result<SessionState> {
    setup.state.phase = SessionPhase::Suspended;
    setup.state.clear_pending_input();
    setup.state.clear_runner();
    manager.save(&setup.state)?;
    sink.send(ApplicationEvent::RunCancelled {
        session_id: id.to_string(),
    })?;
    Err(CruiseError::Interrupted)
}

fn complete_run(
    manager: &SessionManager,
    runtime: &ApplicationRuntime,
    id: &str,
    claim: &OperationClaim,
    sink: &dyn ApplicationEventSink,
    setup: &mut RunSetup,
) -> Result<SessionState> {
    setup.state.phase = SessionPhase::Completed;
    setup.state.completed_at = Some(crate::session::current_iso8601());
    setup.state.clear_runner();
    setup.state.current_step = None;
    if setup.state.repo.is_some()
        && setup.state.pr_url.is_some()
        && crate::repo_clone::cleanup_session_workspace(manager, &setup.state).is_ok()
    {
        setup.state.worktree_path = None;
        setup.state.worktree_branch = None;
    }
    if let crate::workspace::ExecutionWorkspace::Worktree { ctx, .. } = &setup.workspace
        && setup.state.repo.is_none()
        && setup.state.pr_url.is_some()
        && setup
            .state
            .cleanup_after_pr_override
            .unwrap_or(setup.compiled.cleanup_after_pr)
        && crate::worktree::cleanup_worktree(ctx).is_ok()
    {
        setup.state.worktree_path = None;
        setup.state.worktree_branch = None;
    }
    let committed = runtime.commit_if_active(claim, || manager.save(&setup.state));
    match committed {
        Ok(true) => {
            sink.send(ApplicationEvent::RunFinished {
                session_id: id.to_string(),
                phase: setup.state.phase.label().to_string(),
            })?;
            Ok(setup.state.clone())
        }
        Ok(false) => cancel_run(manager, id, sink, setup),
        Err(error) => {
            setup.state.phase = SessionPhase::Suspended;
            setup.state.clear_pending_input();
            setup.state.clear_runner();
            let _ = manager.save(&setup.state);
            let _ = sink.send(ApplicationEvent::RunFailed {
                session_id: id.to_string(),
                error: error.to_string(),
            });
            Err(error)
        }
    }
}

fn fail_run(
    manager: &SessionManager,
    id: &str,
    sink: &dyn ApplicationEventSink,
    setup: &mut RunSetup,
    error: CruiseError,
) -> Result<SessionState> {
    setup.state.phase = SessionPhase::Failed(error.to_string());
    setup.state.completed_at = Some(crate::session::current_iso8601());
    setup.state.clear_pending_input();
    setup.state.clear_runner();
    manager.save(&setup.state)?;
    sink.send(ApplicationEvent::RunFailed {
        session_id: id.to_string(),
        error: error.to_string(),
    })?;
    Err(error)
}

fn finish_run(
    manager: &SessionManager,
    runtime: &ApplicationRuntime,
    id: &str,
    claim: &OperationClaim,
    sink: &dyn ApplicationEventSink,
    setup: &mut RunSetup,
    result: Result<()>,
) -> Result<SessionState> {
    if result
        .as_ref()
        .is_err_and(|error| matches!(error, &CruiseError::Interrupted))
        || claim.token().is_cancelled()
    {
        return cancel_run(manager, id, sink, setup);
    }
    match result {
        Ok(()) => {
            if claim.token().is_cancelled() {
                cancel_run(manager, id, sink, setup)
            } else {
                complete_run(manager, runtime, id, claim, sink, setup)
            }
        }
        Err(error) => fail_run(manager, id, sink, setup, error),
    }
}
type BatchWorkerResult = (usize, String, SessionState, Result<SessionState>);

struct BatchRunState {
    queued: std::collections::VecDeque<(OperationClaim, SessionState)>,
    seen: std::collections::HashSet<String>,
    running: tokio::task::JoinSet<BatchWorkerResult>,
    finished: Vec<(usize, SessionState)>,
    candidate_total: usize,
    next_index: usize,
}

fn batch_parallelism<P>(provider: &P) -> Result<usize>
where
    P: Fn() -> Result<usize>,
{
    let parallelism = provider()?;
    if parallelism == 0 {
        return Err(CruiseError::Other(
            "parallelism must be at least 1".to_string(),
        ));
    }
    Ok(parallelism)
}

async fn schedule_batch_workers<P>(
    app: &CruiseApplication,
    batch: &BatchClaim,
    parallelism_provider: &P,
    sink: &Arc<dyn ApplicationEventSink>,
    log_sink: Option<&Arc<dyn LogSink>>,
    run: &mut BatchRunState,
) -> Result<()>
where
    P: Fn() -> Result<usize> + Send + Sync + 'static,
{
    while !batch.token().is_cancelled() {
        let parallelism = match batch_parallelism(parallelism_provider) {
            Ok(parallelism) => parallelism,
            Err(error) => {
                drain_batch_workers(batch, &mut run.running).await;
                return Err(error);
            }
        };
        if run.running.len() >= parallelism {
            break;
        }
        let Some((claim, scheduled)) = run.queued.pop_front() else {
            break;
        };
        let id = claim.session_id().to_string();
        if claim.token().is_cancelled() {
            sink.send(ApplicationEvent::BatchSessionFinished {
                id,
                phase: scheduled.phase.label().to_string(),
                error: Some("session cancelled before start".to_string()),
            })?;
            continue;
        }
        let worker_app = app.clone();
        let worker_sink = Arc::clone(sink);
        let worker_log_sink = log_sink.cloned();
        let index = run.next_index;
        run.next_index += 1;
        run.running.spawn(async move {
            let outcome = if let Ok(outcome) = std::panic::AssertUnwindSafe(worker_app.run_claimed(
                &id,
                &claim,
                RunRequest::default(),
                worker_sink,
                worker_log_sink,
                true,
            ))
            .catch_unwind()
            .await
            {
                outcome
            } else {
                let mut state = worker_app
                    .manager
                    .load(&id)
                    .unwrap_or_else(|_| scheduled.clone());
                state.phase = SessionPhase::Suspended;
                state.clear_pending_input();
                state.clear_runner();
                let _ = worker_app.manager.save(&state);
                Err(CruiseError::Other("batch worker panicked".to_string()))
            };
            (index, id, scheduled, outcome)
        });
    }
    Ok(())
}

async fn cancel_queued_batch_sessions(
    batch: &BatchClaim,
    run: &mut BatchRunState,
    sink: &Arc<dyn ApplicationEventSink>,
) -> Result<()> {
    while let Some((_claim, scheduled)) = run.queued.pop_front() {
        if let Err(error) = sink.send(ApplicationEvent::BatchSessionFinished {
            id: scheduled.id.clone(),
            phase: scheduled.phase.label().to_string(),
            error: Some("session cancelled before start".to_string()),
        }) {
            drain_batch_workers(batch, &mut run.running).await;
            return Err(error);
        }
    }
    Ok(())
}

async fn queue_late_batch_sessions(
    app: &CruiseApplication,
    batch: &BatchClaim,
    run: &mut BatchRunState,
    sink: &Arc<dyn ApplicationEventSink>,
) -> Result<bool> {
    let late = app.manager.run_all_remaining(&run.seen)?;
    if late.is_empty() {
        return Ok(false);
    }
    let late_map = late
        .iter()
        .map(|state| (state.id.clone(), state.clone()))
        .collect::<HashMap<_, _>>();
    for state in &late {
        run.seen.insert(state.id.clone());
    }
    let additional = batch.reserve(&late);
    for id in &additional.busy {
        if let Err(error) = sink.send(ApplicationEvent::BatchSessionFinished {
            id: id.clone(),
            phase: "Busy".to_string(),
            error: Some("session is already busy".to_string()),
        }) {
            drain_batch_workers(batch, &mut run.running).await;
            return Err(error);
        }
    }
    let added = additional.reserved.len();
    if added > 0 {
        run.candidate_total += added;
        if let Err(error) = sink.send(ApplicationEvent::BatchTotalChanged {
            total: run.candidate_total,
        }) {
            drain_batch_workers(batch, &mut run.running).await;
            return Err(error);
        }
    }
    for claim in additional.reserved {
        if let Some(state) = late_map.get(claim.session_id()).cloned() {
            run.queued.push_back((claim, state));
        }
    }
    Ok(!run.queued.is_empty())
}

async fn finish_batch_worker(
    app: &CruiseApplication,
    batch: &BatchClaim,
    run: &mut BatchRunState,
    sink: &Arc<dyn ApplicationEventSink>,
) -> Result<()> {
    let Some(joined) = run.running.join_next().await else {
        return Ok(());
    };
    let (index, id, scheduled, outcome) = match joined {
        Ok(value) => value,
        Err(error) => {
            drain_batch_workers(batch, &mut run.running).await;
            return Err(CruiseError::Other(format!("batch worker failed: {error}")));
        }
    };
    let state = match &outcome {
        Ok(state) => state.clone(),
        Err(error) => app.manager.load(&id).unwrap_or_else(|_| {
            let mut fallback = scheduled.clone();
            fallback.phase = SessionPhase::Failed(error.to_string());
            fallback.completed_at = Some(crate::session::current_iso8601());
            fallback.clear_runner();
            fallback
        }),
    };
    if let Err(error) = sink.send(ApplicationEvent::BatchSessionFinished {
        id,
        phase: state.phase.label().to_string(),
        error: outcome.as_ref().err().map(ToString::to_string),
    }) {
        drain_batch_workers(batch, &mut run.running).await;
        return Err(error);
    }
    run.finished.push((index, state));
    Ok(())
}

async fn run_batch_loop<P>(
    app: &CruiseApplication,
    batch: &BatchClaim,
    parallelism_provider: &P,
    sink: &Arc<dyn ApplicationEventSink>,
    log_sink: Option<&Arc<dyn LogSink>>,
    run: &mut BatchRunState,
) -> Result<()>
where
    P: Fn() -> Result<usize> + Send + Sync + 'static,
{
    loop {
        schedule_batch_workers(app, batch, parallelism_provider, sink, log_sink, run).await?;
        if batch.token().is_cancelled() {
            cancel_queued_batch_sessions(batch, run, sink).await?;
        } else if run.queued.is_empty() && queue_late_batch_sessions(app, batch, run, sink).await? {
            continue;
        }
        if run.running.is_empty() {
            break;
        }
        finish_batch_worker(app, batch, run, sink).await?;
    }
    Ok(())
}
struct RuntimeAskHandler {
    runtime: Arc<ApplicationRuntime>,
    sink: Arc<dyn ApplicationEventSink>,
    session_id: String,
    claim_identity: u64,
    token: CancellationToken,
}
impl AskHandler for RuntimeAskHandler {
    fn ask_user(&self, question: &str) -> Result<String> {
        self.ask_user_with_cancellation(question, Some(&self.token))
    }
    fn ask_user_with_cancellation(
        &self,
        question: &str,
        token: Option<&CancellationToken>,
    ) -> Result<String> {
        let effective = token.unwrap_or(&self.token);
        let (id, receiver) = self.runtime.register_prompt(
            &self.session_id,
            self.claim_identity,
            PendingKind::Ask,
            Some(question),
            None,
        )?;
        self.sink.send(ApplicationEvent::AskUserRequired {
            session_id: self.session_id.clone(),
            request_id: id.clone(),
            question: question.to_string(),
        })?;
        match self.runtime.wait_prompt(
            &self.session_id,
            &id,
            self.claim_identity,
            &receiver,
            Some(effective),
        )? {
            PromptResponse::Ask(answer) => Ok(answer.trim().to_string()),
            PromptResponse::Option(_) => Err(CruiseError::Other(
                "received option response for ask prompt".to_string(),
            )),
        }
    }
}

struct RuntimeOptionHandler {
    runtime: Arc<ApplicationRuntime>,
    sink: Arc<dyn ApplicationEventSink>,
    session_id: String,
    claim_identity: u64,
    token: CancellationToken,
}
impl OptionHandler for RuntimeOptionHandler {
    fn select_option(&self, choices: &[OptionChoice], plan: Option<&str>) -> Result<OptionResult> {
        self.select_option_with_cancellation(choices, plan, Some(&self.token))
    }
    fn select_option_with_cancellation(
        &self,
        choices: &[OptionChoice],
        plan: Option<&str>,
        token: Option<&CancellationToken>,
    ) -> Result<OptionResult> {
        let effective = token.unwrap_or(&self.token);
        if choices.is_empty() {
            return Ok(OptionResult {
                next_step: None,
                text_input: None,
            });
        }
        let prompt = plan.unwrap_or("Select an option").to_string();
        let payload = choices
            .iter()
            .map(option_choice_payload)
            .collect::<Vec<_>>();
        let (id, receiver) = self.runtime.register_prompt(
            &self.session_id,
            self.claim_identity,
            PendingKind::Option,
            None,
            Some(choices),
        )?;
        self.sink.send(ApplicationEvent::OptionRequired {
            session_id: self.session_id.clone(),
            request_id: id.clone(),
            prompt,
            choices: payload,
        })?;
        match self.runtime.wait_prompt(
            &self.session_id,
            &id,
            self.claim_identity,
            &receiver,
            Some(effective),
        )? {
            PromptResponse::Option(result) => Ok(result),
            PromptResponse::Ask(_) => Err(CruiseError::Other(
                "received ask response for option prompt".to_string(),
            )),
        }
    }
    fn select_option_async<'a>(
        &'a self,
        choices: &'a [OptionChoice],
        plan: Option<&'a str>,
        token: Option<&'a CancellationToken>,
    ) -> Pin<Box<dyn Future<Output = Result<OptionResult>> + Send + 'a>> {
        let runtime = Arc::clone(&self.runtime);
        let sink = Arc::clone(&self.sink);
        let session_id = self.session_id.clone();
        let claim_identity = self.claim_identity;
        let effective = token.cloned().unwrap_or_else(|| self.token.clone());
        let prompt = plan.unwrap_or("Select an option").to_string();
        let payload = choices
            .iter()
            .map(option_choice_payload)
            .collect::<Vec<_>>();
        Box::pin(async move {
            if choices.is_empty() {
                return Ok(OptionResult {
                    next_step: None,
                    text_input: None,
                });
            }
            let (id, receiver) = runtime.register_prompt(
                &session_id,
                claim_identity,
                PendingKind::Option,
                None,
                Some(choices),
            )?;
            if let Err(error) = sink.send(ApplicationEvent::OptionRequired {
                session_id: session_id.clone(),
                request_id: id.clone(),
                prompt,
                choices: payload,
            }) {
                runtime.unregister_prompt(&id, &session_id, claim_identity);
                return Err(error);
            }
            let wait = tokio::task::spawn_blocking(move || {
                runtime.wait_prompt(
                    &session_id,
                    &id,
                    claim_identity,
                    &receiver,
                    Some(&effective),
                )
            })
            .await
            .map_err(|error| {
                CruiseError::Other(format!("option prompt worker failed: {error}"))
            })??;
            match wait {
                PromptResponse::Option(result) => Ok(result),
                PromptResponse::Ask(_) => Err(CruiseError::Other(
                    "received ask response for option prompt".to_string(),
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_session_claim_is_busy_and_raii_release_is_identity_safe() {
        let manager = SessionManager::new(
            std::env::temp_dir().join(format!("cruise-{}", Uuid::new_v4().simple())),
        );
        let runtime = Arc::new(ApplicationRuntime::new(manager));
        let first = runtime
            .try_begin("s", OperationKind::Run)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            runtime.try_begin("s", OperationKind::Fix),
            Err(CruiseError::Busy(_))
        ));
        assert!(runtime.cancel_session("s"));
        drop(first);
        assert!(runtime.try_begin("s", OperationKind::Fix).is_ok());
    }
    #[test]
    fn prompt_responses_reject_empty_text_before_lookup() {
        let manager = SessionManager::new(
            std::env::temp_dir().join(format!("cruise-{}", Uuid::new_v4().simple())),
        );
        let runtime = ApplicationRuntime::new(manager);
        let ask = runtime.respond_to_ask("session", "request", "  ".to_string());
        assert!(ask.is_err());
        let option = runtime.respond_to_option(
            "session",
            "request",
            OptionResult {
                next_step: None,
                text_input: Some("\n".to_string()),
            },
        );
        assert!(option.is_err());
    }

    #[test]
    fn plan_request_no_interactive_planning_defaults_and_roundtrips() {
        let request: PlanRequest = serde_json::from_str("{}").unwrap_or_else(|e| panic!("{e}"));
        assert!(!request.no_interactive_planning);
        let request = PlanRequest {
            no_interactive_planning: true,
            ..PlanRequest::default()
        };
        let value = serde_json::to_value(request).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(value["noInteractivePlanning"], true);
    }

    #[test]
    fn plan_request_formal_spec_defaults_to_false_and_roundtrips_camel_case() {
        let request: PlanRequest = serde_json::from_str("{}").unwrap_or_else(|e| panic!("{e}"));
        assert!(!request.formal_spec);

        let request: PlanRequest =
            serde_json::from_str(r#"{"formalSpec":true}"#).unwrap_or_else(|e| panic!("{e}"));
        assert!(request.formal_spec);
        let value = serde_json::to_value(request).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(value["formalSpec"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generate_passes_formal_spec_request_into_the_initial_prompt() {
        let _lock = crate::test_support::lock_process();
        let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap_or_else(|e| panic!("{e}"));
        let _home = crate::test_support::set_fake_home(&home);
        let manager = SessionManager::new(temp.path().join("sessions"));
        let app = CruiseApplication::new(manager);
        let session = app
            .create_session(NewSessionRequest {
                input: "formal planning task".to_string(),
                base_dir: temp.path().to_path_buf(),
                config_path: None,
                config_source: None,
                config_yaml: Some("command: [cat]\nsteps:\n  s1:\n    prompt: plan\n".to_string()),
                repo: None,
                workspace_mode: WorkspaceMode::Worktree,
                allow_dirty_working_tree: false,
                attachments: vec![],
                skipped_steps: vec![],
            })
            .unwrap_or_else(|e| panic!("{e}"));
        let request: PlanRequest =
            serde_json::from_str(r#"{"formalSpec":true}"#).unwrap_or_else(|e| panic!("{e}"));
        let sink: Arc<dyn ApplicationEventSink> = Arc::new(|_event: ApplicationEvent| Ok(()));

        app.generate(&session.id, request, sink)
            .await
            .unwrap_or_else(|e| panic!("formal planning should complete: {e}"));

        let plan = app
            .session_plan(&session.id)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            plan.contains("Quint"),
            "formal plan must include Quint guidance: {plan}"
        );
        assert!(
            plan.contains("Alloy"),
            "formal plan must include Alloy guidance: {plan}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fix_and_ask_keep_formal_spec_disabled_by_default() {
        let _lock = crate::test_support::lock_process();
        let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap_or_else(|e| panic!("{e}"));
        let _home = crate::test_support::set_fake_home(&home);
        let app = CruiseApplication::new(SessionManager::new(temp.path().join("sessions")));
        let session = app
            .create_session(NewSessionRequest {
                input: "lifecycle boundary task".to_string(),
                base_dir: temp.path().to_path_buf(),
                config_path: None,
                config_source: None,
                config_yaml: Some("command: [cat]\nsteps:\n  s1:\n    prompt: plan\n".to_string()),
                repo: None,
                workspace_mode: WorkspaceMode::Worktree,
                allow_dirty_working_tree: false,
                attachments: vec![],
                skipped_steps: vec![],
            })
            .unwrap_or_else(|e| panic!("{e}"));
        let no_op_sink: Arc<dyn ApplicationEventSink> = Arc::new(|_event: ApplicationEvent| Ok(()));
        app.generate(
            &session.id,
            PlanRequest {
                skip_planning: true,
                ..PlanRequest::default()
            },
            Arc::clone(&no_op_sink),
        )
        .await
        .unwrap_or_else(|e| panic!("initial plan should complete: {e}"));

        let events = Arc::new(Mutex::new(Vec::<ApplicationEvent>::new()));
        let sink_events = Arc::clone(&events);
        let sink: Arc<dyn ApplicationEventSink> = Arc::new(move |event: ApplicationEvent| {
            sink_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
            Ok(())
        });
        app.fix(
            &session.id,
            "adjust the plan".to_string(),
            Arc::clone(&sink),
        )
        .await
        .unwrap_or_else(|e| panic!("fix should complete: {e}"));
        let fixed_plan = app
            .session_plan(&session.id)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!fixed_plan.contains("Quint"));
        assert!(!fixed_plan.contains("Alloy"));

        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        app.ask(&session.id, "what changed?".to_string(), sink)
            .await
            .unwrap_or_else(|e| panic!("ask should complete: {e}"));
        let ask_output = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|event| match event {
                ApplicationEvent::PlanChunk { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(!ask_output.contains("Quint"));
        assert!(!ask_output.contains("Alloy"));

        app.replan(
            &session.id,
            PlanRequest {
                formal_spec: true,
                feedback: Some("replan without formal mode".to_string()),
                ..PlanRequest::default()
            },
            Arc::clone(&no_op_sink),
        )
        .await
        .unwrap_or_else(|e| panic!("replan should complete: {e}"));
        let replanned_plan = app
            .session_plan(&session.id)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!replanned_plan.contains("Quint"));
        assert!(!replanned_plan.contains("Alloy"));
    }

    #[test]
    fn application_listing_hides_transient_exec_sessions() {
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let mut ordinary = SessionState::new(
            "20260830000000".to_string(),
            tmp.path().to_path_buf(),
            "cruise.yaml".to_string(),
            "ordinary".to_string(),
        );
        ordinary.phase = SessionPhase::Planned;
        let mut exec = ordinary.clone();
        exec.id = "20260830000001".to_string();
        exec.exec = true;
        manager.create(&ordinary).unwrap_or_else(|e| panic!("{e}"));
        manager.create(&exec).unwrap_or_else(|e| panic!("{e}"));

        let app = CruiseApplication::new(manager);
        let sessions = app.list_sessions().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["20260830000000"]
        );
    }

    #[test]
    fn failed_and_suspended_sessions_allow_current_step_editing() {
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let app = CruiseApplication::new(SessionManager::new(tmp.path().to_path_buf()));
        for phase in [
            SessionPhase::Failed("failed".to_string()),
            SessionPhase::Suspended,
        ] {
            let mut state = SessionState::new(
                "20260830000002".to_string(),
                PathBuf::from("/tmp"),
                "cruise.yaml".to_string(),
                "task".to_string(),
            );
            state.phase = phase;
            assert!(
                app.capabilities(&state)
                    .contains(&SessionAction::EditCurrentStep)
            );
        }
    }

    #[test]
    fn batch_reservations_use_parent_linked_tokens() {
        let manager = SessionManager::new(
            std::env::temp_dir().join(format!("cruise-{}", Uuid::new_v4().simple())),
        );
        let runtime = Arc::new(ApplicationRuntime::new(manager));
        let batch = runtime.try_begin_batch().unwrap_or_else(|e| panic!("{e}"));
        let state = SessionState::new_draft(
            "s".to_string(),
            PathBuf::from("/tmp"),
            "__builtin__".to_string(),
            "task".to_string(),
        );
        let reservation = batch.reserve(&[state]);
        assert_eq!(reservation.reserved.len(), 1);
        batch.cancel();
        assert!(reservation.reserved[0].token().is_cancelled());
    }
    #[test]
    fn request_defaults_keep_rate_limit_retries() {
        assert_eq!(
            RunRequest::default().rate_limit_retries,
            DEFAULT_RATE_LIMIT_RETRIES
        );
        assert_eq!(
            PlanRequest::default().rate_limit_retries,
            DEFAULT_RATE_LIMIT_RETRIES
        );
        let run: RunRequest = serde_json::from_str("{}").unwrap_or_else(|e| panic!("{e}"));
        let plan: PlanRequest = serde_json::from_str("{}").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(run.rate_limit_retries, DEFAULT_RATE_LIMIT_RETRIES);
        assert_eq!(plan.rate_limit_retries, DEFAULT_RATE_LIMIT_RETRIES);
    }

    #[test]
    fn log_tail_reads_only_requested_lines() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("run.log");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read_log_tail(&path, 2).unwrap_or_else(|e| panic!("{e}")),
            "three\nfour"
        );
        assert_eq!(
            read_log_tail(&path, 0).unwrap_or_else(|e| panic!("{e}")),
            ""
        );
    }
}
