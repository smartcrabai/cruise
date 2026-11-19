use serde::Deserialize;
use serde_json::Value;

use crate::application::{ApproveRequest, NewSessionRequest, PlanRequest, PlanStart};
use crate::error::Result;
use crate::session::SessionState;

use super::DaemonClient;

#[derive(Deserialize)]
struct PublishedIssue {
    url: String,
}

impl DaemonClient {
    pub(crate) async fn create_session(&self, request: &NewSessionRequest) -> Result<SessionState> {
        self.post_json("sessions", request).await
    }

    pub(crate) async fn start_plan(&self, id: &str, request: &PlanStart) -> Result<()> {
        let _: Value = self
            .post_json(&format!("sessions/{id}/plan"), request)
            .await?;
        Ok(())
    }

    pub(crate) async fn approve(&self, id: &str, request: &ApproveRequest) -> Result<SessionState> {
        self.post_json(&format!("sessions/{id}/approve"), request)
            .await
    }

    pub(crate) async fn publish(&self, id: &str, trigger_cruise: bool) -> Result<String> {
        let context = crate::request_context::RequestContext::capture()?;
        let response: PublishedIssue = self
            .post_json(
                &format!("sessions/{id}/publish"),
                &serde_json::json!({
                    "triggerCruise": trigger_cruise,
                    "context": context,
                }),
            )
            .await?;
        Ok(response.url)
    }
    pub(crate) async fn session_config(&self, id: &str) -> Result<crate::config::WorkflowConfig> {
        self.get_json(&format!("sessions/{id}/config")).await
    }

    pub(crate) async fn fix_plan(&self, id: &str, request: &PlanRequest) -> Result<()> {
        self.start_plan_operation(id, "fix", request).await
    }

    pub(crate) async fn ask_plan(&self, id: &str, request: &PlanRequest) -> Result<()> {
        self.start_plan_operation(id, "ask", request).await
    }

    pub(crate) async fn verify_plan(&self, id: &str, request: &PlanRequest) -> Result<()> {
        self.start_plan_operation(id, "verify", request).await
    }

    pub(crate) async fn replan_plan(&self, id: &str, request: &PlanRequest) -> Result<()> {
        self.start_plan_operation(id, "replan", request).await
    }

    pub(crate) async fn discard_plan(&self, id: &str) -> Result<()> {
        let context = crate::request_context::RequestContext::capture()?;
        let _: Value = self
            .post_json(
                &format!("sessions/{id}/discard"),
                &serde_json::json!({ "context": context }),
            )
            .await?;
        Ok(())
    }

    async fn start_plan_operation(
        &self,
        id: &str,
        operation: &str,
        request: &PlanRequest,
    ) -> Result<()> {
        let _: Value = self
            .post_json(&format!("sessions/{id}/plan/{operation}"), request)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CruiseError;

    #[tokio::test]
    async fn plan_start_conflict_returns_to_client_without_waiting() -> Result<()> {
        let router = axum::Router::new().route(
            "/api/v1/sessions/busy-session/plan",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::CONFLICT,
                    axum::Json(serde_json::json!({ "error": "session is busy" })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let client = DaemonClient {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| CruiseError::Other(error.to_string()))?,
            origin: format!("http://{address}/"),
            token: "test-token".to_owned(),
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.start_plan("busy-session", &PlanStart::default()),
        )
        .await
        .map_err(|error| CruiseError::Other(format!("busy plan request timed out: {error}")))?;
        server.abort();

        assert!(result.is_err());
        Ok(())
    }
}
