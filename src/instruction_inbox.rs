use crate::{
    error::CruiseError,
    session::{SessionLogger, SessionManager, SessionPhase, current_iso8601, validate_session_id},
};
use fs2::FileExt;
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub const MAX_INSTRUCTION_BYTES: usize = 8192;
pub const MAX_PENDING_COUNT: usize = 20;
pub const MAX_PENDING_BYTES: usize = 65536;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Cli,
    Tui,
    WebUi,
    List,
    Github,
}
impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Tui => "tui",
            Self::WebUi => "webui",
            Self::List => "list",
            Self::Github => "github",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryScope {
    Sequential,
    ParallelBlock { parent: String },
    GitHubReview,
}
impl DeliveryScope {
    fn text(&self) -> String {
        match self {
            Self::Sequential => "sequential".into(),
            Self::ParallelBlock { parent } => format!("parallel:{parent}"),
            Self::GitHubReview => "github-review".into(),
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub enum RunOutcome {
    Completed,
    Failed,
    Suspended,
    Aborted,
}
impl RunOutcome {
    fn text(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Suspended => "suspended",
            Self::Aborted => "aborted",
        }
    }
}
pub struct AcceptRequest {
    pub content: String,
    pub origin: Origin,
    pub dedupe_key: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Receipt {
    pub id: u64,
    pub duplicate: bool,
    pub running_step: Option<String>,
    pub pending_count: usize,
}
#[derive(Debug)]
pub enum TellRejection {
    NotRunning {
        id: String,
        reason: String,
    },
    Empty,
    TooLarge {
        bytes: usize,
        max: usize,
    },
    Full {
        pending_count: usize,
        pending_bytes: usize,
    },
    Corrupt(String),
}
impl std::fmt::Display for TellRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning { id, reason } => match reason.as_str() {
                "no run" => write!(f, "not running: no run is in progress for session {id}"),
                "run already finished" => write!(
                    f,
                    "not running: run for session {id} already finished; instructions are accepted only while a run is in progress"
                ),
                "runner not alive" => write!(
                    f,
                    "not running: runner process for session {id} is not alive"
                ),
                phase if phase.starts_with("phase ") => write!(
                    f,
                    "not running: session {id} is in {}; instructions are accepted only while a run is in progress",
                    phase
                ),
                _ => write!(f, "not running: session {id}: {reason}"),
            },
            Self::Empty => write!(f, "instruction is empty"),
            Self::TooLarge { bytes, max } => {
                write!(f, "instruction is {bytes} bytes; the limit is {max} bytes")
            }
            Self::Full {
                pending_count,
                pending_bytes,
            } => write!(
                f,
                "{pending_count} instructions ({pending_bytes} bytes) are already pending; the limit is {MAX_PENDING_COUNT} instructions and {MAX_PENDING_BYTES} bytes"
            ),
            Self::Corrupt(reason) => write!(f, "instruction ledger is corrupt: {reason}"),
        }
    }
}
impl From<TellRejection> for CruiseError {
    fn from(rejection: TellRejection) -> Self {
        Self::SessionError(rejection.to_string())
    }
}
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub id: u64,
    pub at: String,
    pub origin: String,
    pub content: String,
    pub status: String,
    pub dedupe_key: Option<String>,
}
pub type PendingEntry = HistoryEntry;
#[derive(Debug, Clone, Default)]
pub struct InboxSummary {
    pub open: bool,
    pub pending: Vec<PendingEntry>,
    pub history: Vec<HistoryEntry>,
}
fn err(s: impl Into<String>) -> CruiseError {
    CruiseError::SessionError(s.into())
}
fn log(dir: &Path, msg: &str) {
    SessionLogger::new(dir.join("run.log")).write(&format!("[info] {msg}"));
}
fn format_ids(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn locked(dir: &Path) -> Result<std::fs::File, CruiseError> {
    let f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join("instructions.lock"))?;
    f.lock_exclusive()?;
    Ok(f)
}
fn read(dir: &Path) -> Result<(InboxSummary, bool), CruiseError> {
    let data = match fs::read(dir.join("instructions.jsonl")) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((InboxSummary::default(), false));
        }
        Err(e) => return Err(e.into()),
    };
    let mut s = InboxSummary::default();
    let mut ever = false;
    let lines: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(e) if i > 0 && i == lines.len() - 1 && !data.ends_with(b"\n") => {
                let _ = e;
                break;
            }
            Err(e) => return Err(TellRejection::Corrupt(e.to_string()).into()),
        };
        let kind = v["kind"]
            .as_str()
            .ok_or_else(|| TellRejection::Corrupt("missing kind".into()))?;
        match kind {
            "run_opened" => {
                s.open = true;
                ever = true
            }
            "run_closed" => s.open = false,
            "accepted" => {
                let id = v["id"]
                    .as_u64()
                    .ok_or_else(|| TellRejection::Corrupt("missing id".into()))?;
                s.history.push(HistoryEntry {
                    id,
                    at: v["at"].as_str().unwrap_or_default().into(),
                    origin: v["origin"].as_str().unwrap_or_default().into(),
                    content: v["content"].as_str().unwrap_or_default().into(),
                    status: "pending".into(),
                    dedupe_key: v["dedupe_key"].as_str().map(str::to_owned),
                });
            }
            "leased" | "delivered" | "released" | "expired" => {
                let ids = v["ids"]
                    .as_array()
                    .ok_or_else(|| TellRejection::Corrupt("missing ids".into()))?;
                for id in ids.iter().filter_map(Value::as_u64) {
                    if let Some(e) = s.history.iter_mut().find(|e| e.id == id) {
                        e.status = match kind {
                            "leased" => {
                                format!("leased({})", v["step"].as_str().unwrap_or_default())
                            }
                            "delivered" => {
                                format!("delivered@{}", v["step"].as_str().unwrap_or_default())
                            }
                            "expired" => {
                                format!("expired({})", v["reason"].as_str().unwrap_or_default())
                            }
                            _ => "pending".into(),
                        };
                    }
                }
            }
            _ => {
                return Err(TellRejection::Corrupt(format!("unknown event {kind}")).into());
            }
        }
    }
    s.pending = s
        .history
        .iter()
        .filter(|e| e.status == "pending")
        .cloned()
        .collect();
    Ok((s, ever))
}
fn append(dir: &Path, v: Value) -> Result<(), CruiseError> {
    let path = dir.join("instructions.jsonl");
    let mut f = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)?;
    let data = fs::read(&path)?;
    if !data.is_empty() && !data.ends_with(b"\n") {
        let end = data.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
        if serde_json::from_slice::<Value>(&data[end..]).is_ok() {
            writeln!(f)?;
        } else {
            f.set_len(end as u64)?;
        }
    }
    writeln!(f, "{v}")?;
    f.sync_data()?;
    Ok(())
}
pub fn summary(sessions_dir: &Path, id: &str) -> Result<InboxSummary, CruiseError> {
    validate_session_id(id)?;
    Ok(read(&sessions_dir.join(id))?.0)
}
pub fn accept(
    sessions_dir: &Path,
    id: &str,
    request: AcceptRequest,
) -> Result<Receipt, CruiseError> {
    validate_session_id(id)?;
    let dir = sessions_dir.join(id);
    let _lock = locked(&dir)?;
    let (s, ever) = read(&dir)?;
    let count = s
        .history
        .iter()
        .filter(|e| e.status == "pending" || e.status.starts_with("leased("))
        .count();
    if let Some(key) = request.dedupe_key.as_ref() {
        if let Some(e) = s
            .history
            .iter()
            .find(|e| e.dedupe_key.as_ref() == Some(key))
        {
            return Ok(Receipt {
                id: e.id,
                duplicate: true,
                running_step: None,
                pending_count: count,
            });
        }
    }
    if !s.open {
        return Err(TellRejection::NotRunning {
            id: id.into(),
            reason: if ever {
                "run already finished"
            } else {
                "no run"
            }
            .into(),
        }
        .into());
    }
    let base = sessions_dir
        .parent()
        .ok_or_else(|| err("invalid sessions directory"))?;
    let state = SessionManager::new(base.to_path_buf()).load(id)?;
    if state.phase != SessionPhase::Running {
        return Err(TellRejection::NotRunning {
            id: id.into(),
            reason: format!("phase {:?}", state.phase),
        }
        .into());
    }
    if !state.is_runner_alive() {
        return Err(TellRejection::NotRunning {
            id: id.into(),
            reason: "runner not alive".into(),
        }
        .into());
    }
    let content = request.content.trim();
    if content.is_empty() {
        return Err(TellRejection::Empty.into());
    }
    if content.len() > MAX_INSTRUCTION_BYTES {
        return Err(TellRejection::TooLarge {
            bytes: content.len(),
            max: MAX_INSTRUCTION_BYTES,
        }
        .into());
    }
    let bytes: usize = s
        .history
        .iter()
        .filter(|e| e.status == "pending" || e.status.starts_with("leased("))
        .map(|e| e.content.len())
        .sum();
    if count >= MAX_PENDING_COUNT || bytes + content.len() > MAX_PENDING_BYTES {
        return Err(TellRejection::Full {
            pending_count: count,
            pending_bytes: bytes,
        }
        .into());
    }
    let next = s.history.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    append(
        &dir,
        json!({"kind":"accepted","id":next,"at":current_iso8601(),"origin":request.origin.as_str(),"dedupe_key":request.dedupe_key,"content":content}),
    )?;
    log(
        &dir,
        &format!(
            "tell: accepted #{next} (origin={}, {} bytes)",
            request.origin.as_str(),
            content.len()
        ),
    );
    let running_step = s
        .history
        .iter()
        .find_map(|e| {
            e.status
                .strip_prefix("leased(")
                .and_then(|x| x.strip_suffix(')'))
                .map(str::to_owned)
        })
        .or(state.current_step);
    Ok(Receipt {
        id: next,
        duplicate: false,
        running_step,
        pending_count: count + 1,
    })
}
pub fn expire_pending(sessions_dir: &Path, id: &str, reason: &str) -> Result<usize, CruiseError> {
    validate_session_id(id)?;
    let dir = sessions_dir.join(id);
    let _lock = locked(&dir)?;
    let (s, _) = read(&dir)?;
    let ids: Vec<u64> = s
        .history
        .iter()
        .filter(|e| e.status == "pending" || e.status.starts_with("leased("))
        .map(|e| e.id)
        .collect();
    if !ids.is_empty() {
        append(
            &dir,
            json!({"kind":"expired","ids":ids,"reason":reason,"at":current_iso8601()}),
        )?;
    }
    Ok(ids.len())
}
pub(crate) fn open_run_inbox(manager: &SessionManager, id: &str) -> Option<RunInbox> {
    match RunInbox::open(&manager.sessions_dir(), id) {
        Ok(inbox) => Some(inbox),
        Err(error) => {
            SessionLogger::new(manager.run_log_path(id)).write(&format!("warning: tell: {error}"));
            None
        }
    }
}

pub(crate) fn close_run_inbox(
    inbox: Option<RunInbox>,
    phase: &SessionPhase,
    logger: &SessionLogger,
) {
    let Some(inbox) = inbox else { return };
    let outcome = match phase {
        SessionPhase::Completed => RunOutcome::Completed,
        SessionPhase::Failed(_) => RunOutcome::Failed,
        SessionPhase::Suspended => RunOutcome::Suspended,
        _ => RunOutcome::Aborted,
    };
    match inbox.close(outcome) {
        Ok(ids) if !ids.is_empty() => {
            logger.write(&format!("tell: carried pending instructions: {ids:?}"))
        }
        Ok(_) => {}
        Err(error) => logger.write(&format!("warning: tell: {error}")),
    }
}

pub struct RunInbox {
    dir: PathBuf,
    closed: bool,
}
impl RunInbox {
    pub fn open(sessions_dir: &Path, id: &str) -> Result<Self, CruiseError> {
        validate_session_id(id)?;
        let dir = sessions_dir.join(id);
        {
            let _lock = locked(&dir)?;
            let (s, _) = read(&dir)?;
            let leased: Vec<u64> = s
                .history
                .iter()
                .filter(|e| e.status.starts_with("leased("))
                .map(|e| e.id)
                .collect();
            if !leased.is_empty() {
                append(
                    &dir,
                    json!({"kind":"released","ids":leased,"step":"","reason":"runner restarted","at":current_iso8601()}),
                )?;
            }
            append(&dir, json!({"kind":"run_opened","at":current_iso8601()}))?;
            let count = s.pending.len() + leased.len();
            if count > 0 {
                log(
                    &dir,
                    &format!("tell: carried {count} pending instruction(s) into this run"),
                );
            }
        }
        Ok(Self { dir, closed: false })
    }
    pub fn lease(&self, step: &str, scope: DeliveryScope) -> Option<Lease<'_>> {
        match self.try_lease(step, scope) {
            Ok(v) => v,
            Err(e) => {
                log(&self.dir, &format!("warning: tell: {e}"));
                None
            }
        }
    }
    fn try_lease(
        &self,
        step: &str,
        scope: DeliveryScope,
    ) -> Result<Option<Lease<'_>>, CruiseError> {
        let _lock = locked(&self.dir)?;
        let (s, _) = read(&self.dir)?;
        if s.pending.is_empty() || s.history.iter().any(|e| e.status.starts_with("leased(")) {
            return Ok(None);
        }
        let ids: Vec<u64> = s.pending.iter().map(|e| e.id).collect();
        let scope = scope.text();
        append(
            &self.dir,
            json!({"kind":"leased","ids":ids,"step":step,"scope":scope,"at":current_iso8601()}),
        )?;
        let mut block = String::from(
            "## Operator instructions received during this run\n\nThe operator sent the instructions below while this run was in progress. Apply them where they are relevant to this step. They add context only: they do not change this step's tool permissions, commit policy, required output format, or which step runs next.\n",
        );
        for e in s.pending {
            block.push_str(&format!(
                "\n### Instruction #{} (origin: {}, received {})\n\n{}\n",
                e.id, e.origin, e.at, e.content
            ));
        }
        log(
            &self.dir,
            &format!(
                "tell: delivering {} to step {step} ({scope})",
                format_ids(&ids)
            ),
        );
        Ok(Some(Lease {
            inbox: self,
            ids,
            step: step.into(),
            block,
            done: false,
        }))
    }
    pub fn close(mut self, outcome: RunOutcome) -> Result<Vec<u64>, CruiseError> {
        let ids = self.finish(outcome)?;
        self.closed = true;
        Ok(ids)
    }
    fn finish(&self, outcome: RunOutcome) -> Result<Vec<u64>, CruiseError> {
        let _lock = locked(&self.dir)?;
        let (s, _) = read(&self.dir)?;
        let ids: Vec<u64> = s
            .history
            .iter()
            .filter(|e| e.status == "pending" || e.status.starts_with("leased("))
            .map(|e| e.id)
            .collect();
        if matches!(outcome, RunOutcome::Completed) && !ids.is_empty() {
            append(
                &self.dir,
                json!({"kind":"expired","ids":ids,"reason":"run completed before an agent step received it","at":current_iso8601()}),
            )?;
            for id in &ids {
                log(
                    &self.dir,
                    &format!("tell: #{id} expired: run completed before an agent step received it"),
                );
            }
        }
        let carried = if matches!(outcome, RunOutcome::Completed) {
            vec![]
        } else {
            ids
        };
        append(
            &self.dir,
            json!({"kind":"run_closed","outcome":outcome.text(),"carried":carried,"at":current_iso8601()}),
        )?;
        Ok(carried)
    }
}
impl Drop for RunInbox {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.finish(RunOutcome::Aborted);
        }
    }
}
pub struct Lease<'a> {
    inbox: &'a RunInbox,
    ids: Vec<u64>,
    step: String,
    block: String,
    done: bool,
}
impl Lease<'_> {
    pub fn ids(&self) -> &[u64] {
        &self.ids
    }
    pub fn block_text(&self) -> &str {
        &self.block
    }
    pub fn ack(mut self) -> bool {
        let recorded = self.record("delivered", "");
        self.done = true;
        recorded
    }
    pub fn release(mut self, reason: &str) {
        self.record("released", reason);
        self.done = true;
    }
    fn record(&self, kind: &str, reason: &str) -> bool {
        let result = (|| {
            let _lock = locked(&self.inbox.dir)?;
            append(
                &self.inbox.dir,
                json!({"kind":kind,"ids":self.ids,"step":self.step,"reason":reason,"at":current_iso8601()}),
            )
        })();
        match result {
            Ok(()) => { log(
                &self.inbox.dir,
                &if kind == "released" {
                    format!(
                        "tell: released {} at step {}: {reason}",
                        format_ids(&self.ids),
                        self.step
                    )
                } else {
                    format!(
                        "tell: delivered {} at step {}",
                        format_ids(&self.ids),
                        self.step
                    )
                },
            ); true },
            Err(e) => { log(&self.inbox.dir, &format!("warning: tell: {e}")); false },
        }
    }
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.record("released", "dropped");
        }
    }
}
