use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    ApplicationEvent, ApplicationEventSink, BatchRunContext, BatchRunState, CruiseApplication,
    LogSink, Result, SessionState, batch_parallelism, run_batch_loop,
};
use crate::error::CruiseError;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAllRequest {
    #[serde(default)]
    pub parallelism: Option<usize>,
    #[serde(default)]
    pub auto_requeue_max_attempts: Option<u32>,
    #[serde(default)]
    pub context: Option<crate::request_context::RequestContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TellRequest {
    pub content: String,
    #[serde(default = "default_tell_origin")]
    pub origin: String,
    #[serde(default)]
    pub dedupe_key: Option<String>,
    #[serde(default)]
    pub context: Option<crate::request_context::RequestContext>,
}

fn default_tell_origin() -> String {
    "cli".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TellReceipt {
    pub id: u64,
    pub duplicate: bool,
    pub running_step: Option<String>,
    pub pending_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionHistoryEntry {
    pub id: u64,
    pub at: String,
    pub origin: String,
    pub content: String,
    pub status: String,
    pub dedupe_key: Option<String>,
}

impl CruiseApplication {
    /// Run a batch with per-invocation CLI overrides.
    ///
    /// # Errors
    /// Returns an error if the batch cannot be claimed, configured, or run.
    pub async fn run_all_with_request(
        &self,
        request: RunAllRequest,
        sink: Arc<dyn ApplicationEventSink>,
        log_sink: Option<Arc<dyn LogSink>>,
    ) -> Result<Vec<SessionState>> {
        let config = self.app_config()?;
        let parallelism = request.parallelism.unwrap_or(config.run_all_parallelism);
        let max_attempts = request
            .auto_requeue_max_attempts
            .unwrap_or(config.run_all_auto_requeue_max_attempts);
        run_all_with_overrides(
            self,
            move || Ok(parallelism),
            max_attempts,
            request.context,
            sink,
            log_sink,
        )
        .await
    }

    /// Accept an instruction using the daemon's session and inbox state.
    ///
    /// # Errors
    /// Returns an error for an invalid session, origin, or instruction inbox.
    pub fn submit_instruction(&self, id: &str, request: TellRequest) -> Result<TellReceipt> {
        let origin = match request.origin.as_str() {
            "cli" => crate::instruction_inbox::Origin::Cli,
            "github" => crate::instruction_inbox::Origin::Github,
            "list" => crate::instruction_inbox::Origin::List,
            other => {
                return Err(CruiseError::SessionError(format!(
                    "unsupported instruction origin: {other}"
                )));
            }
        };
        let receipt = crate::instruction_inbox::accept(
            &self.manager.sessions_dir(),
            id,
            crate::instruction_inbox::AcceptRequest {
                content: request.content,
                origin,
                dedupe_key: request.dedupe_key,
            },
        )?;
        Ok(TellReceipt {
            id: receipt.id,
            duplicate: receipt.duplicate,
            running_step: receipt.running_step,
            pending_count: receipt.pending_count,
        })
    }

    /// Read the instruction history for one session.
    ///
    /// # Errors
    /// Returns an error if the session or instruction ledger cannot be read.
    pub fn instruction_history(&self, id: &str) -> Result<Vec<InstructionHistoryEntry>> {
        self.read_session(id)?;
        Ok(
            crate::instruction_inbox::summary(&self.manager.sessions_dir(), id)?
                .history
                .into_iter()
                .map(|entry| InstructionHistoryEntry {
                    id: entry.id,
                    at: entry.at,
                    origin: entry.origin,
                    content: entry.content,
                    status: entry.status,
                    dedupe_key: entry.dedupe_key,
                })
                .collect(),
        )
    }
}

pub(super) async fn run_all_with_overrides<P>(
    app: &CruiseApplication,
    parallelism_provider: P,
    max_attempts: u32,
    context: Option<crate::request_context::RequestContext>,
    sink: Arc<dyn ApplicationEventSink>,
    log_sink: Option<Arc<dyn LogSink>>,
) -> Result<Vec<SessionState>>
where
    P: Fn() -> Result<usize> + Send + Sync + 'static,
{
    let parallelism = batch_parallelism(&parallelism_provider)?;
    let batch = app.runtime.try_begin_batch()?;
    let _sleep_guard = crate::sleep_guard::prevent_sleep_for_run();
    let candidates = app.manager.run_all_candidates()?;
    let candidate_map = candidates
        .iter()
        .map(|state| (state.id.clone(), state.clone()))
        .collect::<std::collections::HashMap<_, _>>();
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
            phase: "Busy".to_owned(),
            error: Some("session is already busy".to_owned()),
        })?;
    }
    for claim in reservation.reserved {
        if let Some(state) = candidate_map.get(claim.session_id()).cloned() {
            run.queued.push_back((claim, state));
        }
    }
    let batch_context = BatchRunContext {
        app,
        batch: &batch,
        parallelism_provider: &parallelism_provider,
        max_attempts,
        context: context.as_ref(),
        sink: &sink,
        log_sink: log_sink.as_ref(),
    };
    run_batch_loop(&batch_context, &mut run).await?;
    sink.send(ApplicationEvent::BatchFinished {
        cancelled: batch.token().is_cancelled(),
    })?;
    run.finished.sort_by_key(|(index, _)| *index);
    Ok(run.finished.into_iter().map(|(_, state)| state).collect())
}
