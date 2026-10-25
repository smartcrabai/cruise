//! Wire types for `OpenCode` V2, pinned to 74f8d4860f49c563909e6b039b0cc8b454ebce72.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelInfo {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub id: String,
    pub variants: Vec<ModelVariant>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct ModelVariant {
    pub id: String,
    pub settings: Option<VariantSettings>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct VariantSettings {
    #[serde(rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    pub data: T,
}
#[derive(Debug, Deserialize)]
pub(crate) struct ServerInfo {
    pub version: String,
}
#[derive(Debug, Deserialize)]
pub(crate) struct SessionInfo {
    pub id: String,
}
#[derive(Debug, Serialize)]
pub(crate) struct CreateSession<'a> {
    pub id: &'a str,
    pub location: Value,
    pub permissions: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'a ModelRef>,
}
#[derive(Debug, Serialize)]
pub(crate) struct Prompt<'a> {
    pub id: &'a str,
    pub text: &'a str,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub durable: Option<Durable>,
    #[serde(rename = "aggregateID", default)]
    pub aggregate_id: Option<String>,
    #[serde(default)]
    pub seq: Option<u64>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Durable {
    #[serde(rename = "aggregateID")]
    pub aggregate_id: String,
    pub seq: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ModelFailure {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
    pub status: Option<u16>,
    #[serde(rename = "retryAfterMs")]
    pub retry_after_ms: Option<u64>,
}
impl ModelFailure {
    pub fn is_limit(&self) -> bool {
        self.status == Some(429)
            || matches!(
                self.kind.as_str(),
                "provider.rate-limit" | "provider.quota" | "provider.free-limit"
            )
    }
}
impl std::fmt::Display for ModelFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OpenCode model execution failed [{}]: {}",
            self.kind, self.message
        )?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        if let Some(ms) = self.retry_after_ms {
            write!(f, " Retry-After: {} seconds", ms.div_ceil(1000))?;
        }
        Ok(())
    }
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("OpenCode local API HTTP {status}: {detail}")]
    Http {
        status: u16,
        detail: String,
        retry_after: Option<String>,
    },
    #[error("OpenCode transport failure: {0}")]
    Transport(String),
    #[error("OpenCode V2 protocol failure: {0}")]
    Protocol(String),
}
pub(crate) type Result<T> = std::result::Result<T, Error>;
