use serde::Deserialize;

use super::DaemonClient;
use crate::application::ExecStartRequest;
use crate::error::Result;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartedExec {
    session_id: String,
}

impl DaemonClient {
    /// Start an exec operation and return its newly created session ID.
    pub(crate) async fn start_exec(&self, request: &ExecStartRequest) -> Result<String> {
        let started: StartedExec = self.post_json("exec", request).await?;
        Ok(started.session_id)
    }
}
