use super::types::{
    CreateSession, Envelope, Error, ModelRef, Prompt, Result, ServerInfo, SessionInfo,
};
use reqwest::{Method, Response};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::time::Duration;

pub(crate) struct Client {
    http: reqwest::Client,
    base: url::Url,
    password: String,
}
impl Client {
    pub fn new(base: url::Url, password: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .build()
            .map_err(transport)?;
        Ok(Self {
            http,
            base,
            password,
        })
    }
    async fn request<T: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&T>,
    ) -> Result<Response> {
        let url = self
            .base
            .join(path)
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let mut request = self
            .http
            .request(method, url)
            .basic_auth("opencode", Some(&self.password));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(transport)?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let detail =
                String::from_utf8_lossy(&read_body(response, 8192).await.unwrap_or_else(|error| {
                    format!("response body unavailable: {error}").into_bytes()
                }))
                .into_owned();
            return Err(Error::Http {
                status,
                detail: detail
                    .chars()
                    .take(4096)
                    .collect::<String>()
                    .replace(&self.password, "[redacted]"),
                retry_after,
            });
        }
        Ok(response)
    }
    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let response = self.request(method, path, body).await?;
        let bytes = read_body(response, 1024 * 1024).await?;
        serde_json::from_slice(&bytes).map_err(|error| Error::Protocol(error.to_string()))
    }
    pub async fn info(&self) -> Result<ServerInfo> {
        self.json(Method::GET, "/api/info", None::<&Value>).await
    }
    pub async fn plugins(&self, directory: &std::path::Path) -> Result<Vec<Value>> {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("location[directory]", &directory.to_string_lossy());
        let query = query.finish();
        let value: Envelope<Vec<Value>> = self
            .json(Method::GET, &format!("/api/plugin?{query}"), None::<&Value>)
            .await?;
        Ok(value.data)
    }
    pub async fn create(&self, request: &CreateSession<'_>) -> Result<SessionInfo> {
        let value: Envelope<SessionInfo> = self
            .json(Method::POST, "/api/session", Some(request))
            .await?;
        Ok(value.data)
    }
    pub async fn session(&self, id: &str) -> Result<SessionInfo> {
        let value: Envelope<SessionInfo> = self
            .json(Method::GET, &format!("/api/session/{id}"), None::<&Value>)
            .await?;
        Ok(value.data)
    }
    pub async fn model(&self, id: &str, model: &ModelRef) -> Result<()> {
        self.request(
            Method::POST,
            &format!("/api/session/{id}/model"),
            Some(&json!({"model":model})),
        )
        .await?;
        Ok(())
    }
    pub async fn permissions(&self, id: &str, rules: &Value) -> Result<()> {
        self.request(
            Method::PATCH,
            &format!("/api/session/{id}"),
            Some(&json!({"permissions":rules})),
        )
        .await?;
        Ok(())
    }
    pub async fn environment(
        &self,
        id: &str,
        variables: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        self.request(
            Method::PUT,
            &format!("/api/session/{id}/environment"),
            Some(&json!({"variables":variables})),
        )
        .await?;
        Ok(())
    }
    pub async fn prompt(&self, id: &str, input: &Prompt<'_>) -> Result<Value> {
        let value: Envelope<Value> = self
            .json(
                Method::POST,
                &format!("/api/session/{id}/prompt"),
                Some(input),
            )
            .await?;
        Ok(value.data)
    }
    pub async fn events(&self) -> Result<Response> {
        self.request(Method::GET, "/api/event", None::<&Value>)
            .await
    }
    pub async fn log(&self, id: &str, after: u64) -> Result<Response> {
        self.request(
            Method::GET,
            &format!("/api/experimental/session/{id}/log?after={after}"),
            None::<&Value>,
        )
        .await
    }
    pub async fn interrupt(&self, id: &str) -> Result<()> {
        self.request(
            Method::POST,
            &format!("/api/session/{id}/interrupt"),
            Some(&json!({})),
        )
        .await?;
        Ok(())
    }
    pub async fn cancel_inbox(&self, id: &str, inbox: &str) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/api/session/{id}/inbox/{inbox}"),
            None::<&Value>,
        )
        .await?;
        Ok(())
    }
    pub async fn inbox(&self, id: &str) -> Result<Vec<Value>> {
        let value: Envelope<Vec<Value>> = self
            .json(
                Method::GET,
                &format!("/api/session/{id}/inbox"),
                None::<&Value>,
            )
            .await?;
        Ok(value.data)
    }
}
async fn read_body(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if output.len().saturating_add(chunk.len()) > limit {
            return Err(Error::Protocol(
                "HTTP response exceeds configured size limit".into(),
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}
fn transport(error: reqwest::Error) -> Error {
    Error::Transport(error.without_url().to_string())
}
