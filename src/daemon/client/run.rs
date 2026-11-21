use serde::Deserialize;
use serde_json::Value;

use crate::application::{
    InstructionHistoryEntry, RunAllRequest, RunRequest, TellReceipt, TellRequest,
};
use crate::daemon::client::DaemonClient;
use crate::error::Result;
use crate::request_context::RequestContext;
use crate::session::SessionState;

#[derive(Debug, Deserialize)]
pub(crate) struct RunSessionSummary {
    pub id: String,
    pub phase: String,
    pub input: String,
    pub title: Option<String>,
    pub plan_available: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunAllResponse {
    #[serde(default)]
    no_active_batch: bool,
}

#[derive(Deserialize)]
struct CancelledResponse {
    cancelled: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionHistoryRequest<'a> {
    context: &'a RequestContext,
}

impl DaemonClient {
    /// Request a run; an already-active operation is attached by the daemon.
    pub(crate) async fn start_run(&self, id: &str, request: &RunRequest) -> Result<()> {
        let _: Value = self
            .post_json(&format!("sessions/{id}/run"), request)
            .await?;
        Ok(())
    }

    /// Start Run All or attach to its currently active batch.
    pub(crate) async fn start_run_all(&self, request: &RunAllRequest) -> Result<bool> {
        let response: RunAllResponse = self.post_json("run-all", request).await?;
        Ok(!response.no_active_batch)
    }

    /// Submit an instruction to a running session.
    pub(crate) async fn tell(&self, id: &str, request: &TellRequest) -> Result<TellReceipt> {
        self.post_json(&format!("sessions/{id}/instructions"), request)
            .await
    }

    /// Read the daemon-owned instruction history.
    pub(crate) async fn instructions(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<Vec<InstructionHistoryEntry>> {
        self.post_json(
            &format!("sessions/{id}/instructions/history"),
            &InstructionHistoryRequest { context },
        )
        .await
    }

    /// Read a session and whether the daemon currently owns an operation for it.
    pub(crate) async fn run_session(&self, id: &str) -> Result<(SessionState, bool)> {
        self.read_session_state(id).await
    }

    /// List sessions eligible to display in the run picker or batch dashboard.
    pub(crate) async fn run_sessions(&self) -> Result<Vec<RunSessionSummary>> {
        self.get_json("sessions").await
    }

    /// Cancel the currently active Run All batch.
    pub(crate) async fn cancel_run_all(&self) -> Result<bool> {
        let response: CancelledResponse = self.post_json("run-all/cancel", &Value::Null).await?;
        Ok(response.cancelled)
    }
}
