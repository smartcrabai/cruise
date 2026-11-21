mod exec;
pub(crate) mod list;
mod plan;
mod run;
pub(crate) use run::RunSessionSummary;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt as _};
use reqwest::Response;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

use crate::application::{ApplicationEvent, PendingPrompt};
use crate::daemon::instance::{InstanceRecord, Registry};
use crate::daemon::{ServeOptions, lifecycle};
use crate::error::{CruiseError, Result};
use crate::session::{SessionPhase, SessionState};
use crate::step::option::OptionResult;
pub(crate) type EventStream = Pin<Box<dyn Stream<Item = Result<ApplicationEvent>> + Send>>;

const READY_WAIT: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub(crate) struct DaemonClient {
    http: reqwest::Client,
    origin: String,
    token: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct EventFilter {
    pub session: Option<String>,
    pub batch: bool,
}

pub(crate) enum PromptAnswer {
    Answer(String),
    Option(OptionResult),
}

pub(crate) trait SessionEventHandler {
    fn on_event(&mut self, event: &ApplicationEvent) -> Result<()>;

    fn on_pending_prompts(&mut self, _prompts: &[PendingPrompt]) -> Result<()> {
        Ok(())
    }

    fn is_terminal_state(&self, _state: &SessionState) -> bool {
        true
    }

    fn terminal_failure(&self) -> Option<&str> {
        None
    }
}

impl<F> SessionEventHandler for F
where
    F: FnMut(&ApplicationEvent) -> Result<()>,
{
    fn on_event(&mut self, event: &ApplicationEvent) -> Result<()> {
        self(event)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    version: String,
    active_operations: usize,
    batch_active: bool,
}

#[derive(serde::Deserialize)]
struct CancelResponse {
    cancelled: bool,
}

impl DaemonClient {
    pub(crate) async fn manager_snapshot(&self, cwd: &str) -> Result<Value> {
        let mut url = Url::parse(&self.origin)
            .and_then(|base| base.join("api/manager/snapshot"))
            .map_err(|error| CruiseError::Other(format!("invalid daemon API path: {error}")))?;
        url.query_pairs_mut().append_pair("cwd", cwd);
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|error| CruiseError::Other(format!("daemon request failed: {error}")))?;
        decode_response(response).await
    }

    pub(crate) async fn manager_action(&self, body: &Value) -> Result<Value> {
        let url = Url::parse(&self.origin)
            .and_then(|base| base.join("api/manager/action"))
            .map_err(|error| CruiseError::Other(format!("invalid daemon API path: {error}")))?;
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(|error| CruiseError::Other(format!("daemon request failed: {error}")))?;
        decode_response(response).await
    }

    /// Connect to the registered daemon, starting it when needed and replacing an idle old version.
    pub(crate) async fn connect() -> Result<Self> {
        let registry = Registry::new()?;
        if let Some(record) = registry.read()?
            && record.verify_identity()?
        {
            // A live daemon is checked below for readiness and version compatibility.
        } else {
            lifecycle::start_autostart(&ServeOptions::default())?;
        }

        let deadline = Instant::now() + READY_WAIT;
        loop {
            let record = match registry.read()? {
                Some(record) => record,
                None if Instant::now() < deadline => {
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
                None => {
                    return Err(CruiseError::Other(
                        "daemon did not publish its instance record".to_owned(),
                    ));
                }
            };
            if !record.verify_identity()? {
                if Instant::now() >= deadline {
                    return Err(CruiseError::Other("daemon is not running".to_owned()));
                }
                tokio::time::sleep(RETRY_DELAY).await;
                continue;
            }
            let client = Self::from_record(&record)?;
            match client.get_json::<StatusResponse>("status").await {
                Ok(status) => {
                    if status.version != env!("CARGO_PKG_VERSION") {
                        if status.active_operations != 0 || status.batch_active {
                            return Err(CruiseError::Other(format!(
                                "daemon is running cruise {} with active work; run 'cruise daemon restart' when idle",
                                status.version
                            )));
                        }
                        lifecycle::restart_outdated_autostart(
                            ServeOptions {
                                host: Some(record.host.clone()),
                                port: Some(record.port),
                                port_explicit: Some(record.port_explicit),
                                ..ServeOptions::default()
                            },
                            &record.instance_id,
                        )?;
                        continue;
                    }
                    return Ok(client);
                }
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn from_record(record: &InstanceRecord) -> Result<Self> {
        let parsed = Url::parse(&record.origin)
            .map_err(|error| CruiseError::Other(format!("invalid daemon origin: {error}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CruiseError::Other(
                "invalid daemon origin scheme".to_owned(),
            ));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                CruiseError::Other(format!("failed to build daemon HTTP client: {error}"))
            })?;
        Ok(Self {
            http,
            origin: record.origin.clone(),
            token: record.token.clone(),
        })
    }

    /// Fetch a short-lived, single-use `WebUI` login URL.
    pub(crate) async fn webui_login_url(&self) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct LoginNonceResponse {
            url: String,
        }

        self.post_json::<_, LoginNonceResponse>("login-nonce", &())
            .await
            .map(|response| response.url)
    }

    /// Fetch a typed JSON value from a relative `/api/v1` path.
    pub(crate) async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.api_url(path)?;
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|error| CruiseError::Other(format!("daemon request failed: {error}")))?;
        decode_response(response).await
    }

    /// Post a typed JSON request and decode its JSON response.
    pub(crate) async fn post_json<B, T>(&self, path: &str, body: &B) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let url = self.api_url(path)?;
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(|error| CruiseError::Other(format!("daemon request failed: {error}")))?;
        decode_response(response).await
    }

    /// Open an authenticated SSE stream of application events.
    pub(crate) async fn events(&self, filter: EventFilter) -> Result<EventStream> {
        let mut url = self.api_url("events")?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(session) = filter.session {
                query.append_pair("session", &session);
            }
            if filter.batch {
                query.append_pair("batch", "1");
            }
        }
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|error| CruiseError::Other(format!("daemon event request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(response_error(response).await);
        }
        let chunks = Box::pin(response.bytes_stream());
        let stream = futures::stream::try_unfold(
            (chunks, Vec::<u8>::new()),
            |(mut chunks, mut buffer)| async move {
                loop {
                    if let Some(frame) = take_frame(&mut buffer, false) {
                        if let Some(event) = parse_frame(&frame)? {
                            return Ok(Some((event, (chunks, buffer))));
                        }
                        continue;
                    }
                    match chunks.next().await {
                        Some(Ok(bytes)) => buffer.extend_from_slice(&bytes),
                        Some(Err(error)) => {
                            return Err(CruiseError::Other(format!(
                                "daemon event stream failed: {error}"
                            )));
                        }
                        None => {
                            let Some(frame) = take_frame(&mut buffer, true) else {
                                return Ok(None);
                            };
                            if let Some(event) = parse_frame(&frame)? {
                                return Ok(Some((event, (chunks, buffer))));
                            }
                            return Ok(None);
                        }
                    }
                }
            },
        );
        Ok(Box::pin(stream))
    }

    /// Send an ask or option answer to a pending prompt.
    pub(crate) async fn answer_prompt(
        &self,
        session_id: &str,
        request_id: &str,
        answer: PromptAnswer,
    ) -> Result<()> {
        let path = format!("sessions/{session_id}/prompts/{request_id}");
        let body = match answer {
            PromptAnswer::Answer(answer) => serde_json::json!({ "answer": answer }),
            PromptAnswer::Option(option) => serde_json::json!({ "option": option }),
        };
        let _: Value = self.post_json(&path, &body).await?;
        Ok(())
    }

    /// Cancel the currently active operation for a session.
    pub(crate) async fn cancel(&self, session_id: &str) -> Result<bool> {
        let path = format!("sessions/{session_id}/cancel");
        let response: CancelResponse = self.post_json(&path, &Value::Null).await?;
        Ok(response.cancelled)
    }

    /// Pause a prompt-driven operation after the user leaves its prompt unanswered.
    pub(crate) async fn pause(&self, session_id: &str) -> Result<()> {
        let path = format!("sessions/{session_id}/pause");
        let _: Value = self.post_json(&path, &Value::Null).await?;
        Ok(())
    }

    /// Drive an operation using a stream subscribed before its start request.
    pub(crate) async fn wait_for_terminal_from<H: SessionEventHandler>(
        &self,
        session_id: &str,
        mut stream: EventStream,
        handler: &mut H,
    ) -> Result<SessionState> {
        let filter = EventFilter {
            session: Some(session_id.to_owned()),
            batch: true,
        };
        let mut last_state = self.initial_wait_state(session_id, handler).await?;
        let mut resync = tokio::time::interval(Duration::from_secs(1));
        resync.tick().await;
        resync.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            loop {
                let item = tokio::select! {
                    item = stream.next() => item,
                    _ = resync.tick() => {
                        if let Some(state) = self
                            .resync_open_stream(session_id, &mut last_state, handler)
                            .await?
                        {
                            return Ok(state);
                        }
                        continue;
                    }
                };
                let Some(item) = item else {
                    break;
                };
                let Ok(event) = item else {
                    break;
                };
                let terminal = is_terminal_event(session_id, &event);
                handler.on_event(&event)?;
                if terminal {
                    return match self.read_session_state(session_id).await {
                        Ok((state, _)) => Ok(state),
                        Err(CruiseError::IoError(error))
                            if error.kind() == std::io::ErrorKind::NotFound =>
                        {
                            state_from_terminal_event(last_state, session_id, &event)
                        }
                        Err(error) => Err(error),
                    };
                }
            }
            if let Some(state) = self
                .resync_closed_stream(session_id, &mut last_state, handler)
                .await?
            {
                return Ok(state);
            }
            tokio::time::sleep(RETRY_DELAY).await;
            stream = self.events(filter.clone()).await?;
        }
    }

    async fn initial_wait_state<H: SessionEventHandler>(
        &self,
        session_id: &str,
        handler: &mut H,
    ) -> Result<Option<SessionState>> {
        let state = match self.read_session_state(session_id).await {
            Ok((state, _)) => Some(state),
            Err(CruiseError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                None
            }
            Err(error) => return Err(error),
        };
        let prompts = self.pending_prompts_or_empty(session_id).await?;
        handler.on_pending_prompts(&prompts)?;
        Ok(state)
    }

    async fn resync_open_stream<H: SessionEventHandler>(
        &self,
        session_id: &str,
        last_state: &mut Option<SessionState>,
        handler: &mut H,
    ) -> Result<Option<SessionState>> {
        let (state, active) = match self.read_session_state(session_id).await {
            Ok(result) => result,
            Err(CruiseError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                // The operation may dispose a transient session before releasing
                // its terminal event. An open stream can still deliver that event.
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        *last_state = Some(state.clone());
        if active {
            return Ok(None);
        }

        let prompts = self.pending_prompts_or_empty(session_id).await?;
        handler.on_pending_prompts(&prompts)?;
        Ok((prompts.is_empty() && handler.is_terminal_state(&state)).then_some(state))
    }

    async fn resync_closed_stream<H: SessionEventHandler>(
        &self,
        session_id: &str,
        last_state: &mut Option<SessionState>,
        handler: &mut H,
    ) -> Result<Option<SessionState>> {
        let (state, active) = self.read_session_state(session_id).await?;
        *last_state = Some(state.clone());
        let prompts = self.pending_prompts_or_empty(session_id).await?;
        handler.on_pending_prompts(&prompts)?;
        Ok((!active && prompts.is_empty() && handler.is_terminal_state(&state)).then_some(state))
    }

    async fn pending_prompts_or_empty(&self, session_id: &str) -> Result<Vec<PendingPrompt>> {
        match self.pending_prompts(session_id).await {
            Ok(prompts) => Ok(prompts),
            Err(CruiseError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    fn api_url(&self, path: &str) -> Result<Url> {
        let base = Url::parse(&self.origin)
            .map_err(|error| CruiseError::Other(format!("invalid daemon origin: {error}")))?;
        base.join(&format!("api/v1/{}", path.trim_start_matches('/')))
            .map_err(|error| CruiseError::Other(format!("invalid daemon API path: {error}")))
    }

    async fn pending_prompts(&self, session_id: &str) -> Result<Vec<PendingPrompt>> {
        self.get_json(&format!("sessions/{session_id}/prompts"))
            .await
    }

    async fn read_session_state(&self, session_id: &str) -> Result<(SessionState, bool)> {
        let value: Value = self.get_json(&format!("sessions/{session_id}")).await?;
        let active = value["activeOperation"].as_bool().unwrap_or(false);
        let state = serde_json::from_value(value).map_err(|error| {
            CruiseError::Other(format!("invalid daemon session response: {error}"))
        })?;
        Ok((state, active))
    }
}

fn state_from_terminal_event(
    state: Option<SessionState>,
    session_id: &str,
    event: &ApplicationEvent,
) -> Result<SessionState> {
    let mut state = if let Some(state) = state {
        state
    } else {
        serde_json::from_value(serde_json::json!({
            "id": session_id,
            "base_dir": ".",
            "phase": "Running",
            "config": { "kind": "inline_snapshot" },
            "input": "",
            "created_at": "",
            "exec": true
        }))
        .map_err(|error| {
            CruiseError::Other(format!(
                "could not synthesize terminal session state: {error}"
            ))
        })?
    };
    match event {
        ApplicationEvent::RunFinished { phase, .. } => {
            state.phase = match phase.as_str() {
                "Completed" => SessionPhase::Completed,
                "Suspended" => SessionPhase::Suspended,
                _ => state.phase,
            };
            if state.phase == SessionPhase::Completed {
                state.completed_at = Some(crate::session::current_iso8601());
            }
        }
        ApplicationEvent::RunFailed { error, .. } | ApplicationEvent::PlanFailed { error, .. } => {
            state.phase = SessionPhase::Failed(error.clone());
            state.completed_at = Some(crate::session::current_iso8601());
        }
        ApplicationEvent::RunCancelled { .. } => {
            state.phase = SessionPhase::Suspended;
        }
        ApplicationEvent::PlanFinished { phase, .. } => {
            state.phase = match phase.as_str() {
                "Planned" => SessionPhase::Planned,
                "AwaitingApproval" => SessionPhase::AwaitingApproval,
                "AwaitingInput" => SessionPhase::AwaitingInput,
                _ => state.phase,
            };
        }
        _ => {}
    }
    state.clear_pending_input();
    state.clear_runner();
    Ok(state)
}

async fn decode_response<T: DeserializeOwned>(response: Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    response
        .json::<T>()
        .await
        .map_err(|error| CruiseError::Other(format!("invalid daemon JSON response: {error}")))
}

async fn response_error(response: Response) -> CruiseError {
    let status = response.status();
    let message = response.json::<ErrorBody>().await.map_or_else(
        |_| format!("daemon returned HTTP {status}"),
        |body| body.error,
    );
    match status {
        reqwest::StatusCode::CONFLICT => CruiseError::Busy(message),
        reqwest::StatusCode::NOT_FOUND => {
            CruiseError::IoError(std::io::Error::new(std::io::ErrorKind::NotFound, message))
        }
        _ => CruiseError::Other(message),
    }
}

#[derive(serde::Deserialize)]
struct ErrorBody {
    error: String,
}

fn take_frame(buffer: &mut Vec<u8>, eof: bool) -> Option<Vec<u8>> {
    let line_feed = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|i| (i, 2));
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|i| (i, 4));
    let boundary = match (line_feed, crlf) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(boundary), None) | (None, Some(boundary)) => Some(boundary),
        (None, None) => None,
    };
    if let Some((start, delimiter_len)) = boundary {
        let mut frame: Vec<_> = buffer.drain(..start + delimiter_len).collect();
        frame.truncate(start);
        return Some(frame);
    }
    if eof && !buffer.is_empty() {
        return Some(std::mem::take(buffer));
    }
    None
}

fn parse_frame(frame: &[u8]) -> Result<Option<ApplicationEvent>> {
    let text = std::str::from_utf8(frame)
        .map_err(|error| CruiseError::Other(format!("invalid UTF-8 in daemon event: {error}")))?;
    let mut event_name = "message";
    let mut data = String::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(value) = line.strip_prefix("event:") {
            event_name = value.trim_start();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if event_name != "app" || data.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|error| CruiseError::Other(format!("invalid daemon application event: {error}")))
}

fn is_terminal_event(session_id: &str, event: &ApplicationEvent) -> bool {
    match event {
        ApplicationEvent::PlanFinished { session_id: id, .. }
        | ApplicationEvent::PlanFailed { session_id: id, .. }
        | ApplicationEvent::PlanCancelled { session_id: id }
        | ApplicationEvent::RunFinished { session_id: id, .. }
        | ApplicationEvent::RunFailed { session_id: id, .. }
        | ApplicationEvent::RunCancelled { session_id: id } => id == session_id,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::CruiseApplication;
    use crate::daemon::instance::{self, InstanceRecord};
    use crate::daemon::server::{ControlGate, WebState};
    use crate::session::SessionManager;
    use crate::webui::assets::Assets;

    #[tokio::test]
    async fn connect_uses_registered_instance_and_authenticated_api() -> Result<()> {
        let _process_lock = crate::test_support::lock_process();
        let dir = tempfile::tempdir()?;
        let _home = crate::test_support::EnvGuard::set("HOME", dir.path());
        let _state_home =
            crate::test_support::EnvGuard::set("XDG_STATE_HOME", dir.path().join("state"));
        let _data_home =
            crate::test_support::EnvGuard::set("XDG_DATA_HOME", dir.path().join("data"));

        let registry = Registry::new()?;
        let lock = registry.lock()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let origin = format!("http://{address}/");
        let manager = SessionManager::new(crate::paths::data_dir()?);
        let application = CruiseApplication::new(manager);
        let webui_dir = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"));
        let assets = Assets {
            templates_dir: webui_dir.join("templates"),
            static_dir: webui_dir.join("static"),
            dev_mode: true,
        };
        let mut state = WebState::new(application, &assets)?;
        let token = instance::new_token();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        state.api_token = Some(token.clone());
        state.control = Some(std::sync::Arc::new(ControlGate {
            sender: std::sync::Mutex::new(Some(sender)),
        }));
        let pid = std::process::id();
        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| CruiseError::Other(error.to_string()))?
            .as_secs();
        let instance_id = uuid::Uuid::new_v4().to_string();
        registry.write(&InstanceRecord {
            schema_version: 1,
            instance_id: instance_id.clone(),
            mode: "daemon".to_owned(),
            pid,
            process_started_at: instance::process_start_time(pid)
                .ok_or_else(|| CruiseError::Other("cannot inspect test process".to_owned()))?,
            process_identity: Some(
                instance::precise_identity(pid)
                    .ok_or_else(|| CruiseError::Other("cannot identify test process".to_owned()))?,
            ),
            host: "127.0.0.1".to_owned(),
            port: address.port(),
            origin: origin.clone(),
            token,
            port_explicit: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            executable: std::env::current_exe()?,
            working_dir: std::env::current_dir()?,
            log_path: Some(registry.log_path()),
            started_at,
        })?;
        drop(lock);

        let server_state = state.clone();
        let server = tokio::spawn(async move {
            crate::daemon::server::serve(listener, server_state, async move {
                let _ = receiver.await;
            })
            .await
        });
        let client = DaemonClient::connect().await?;
        assert_eq!(client.origin, origin);
        let status: StatusResponse = client.get_json("status").await?;
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        let sessions: serde_json::Value = client.get_json("sessions").await?;
        assert_eq!(sessions, serde_json::json!([]));
        let _: serde_json::Value = client.post_json("shutdown", &Value::Null).await?;
        let result = server
            .await
            .map_err(|error| CruiseError::Other(error.to_string()))?;
        result?;
        registry.remove_if_owned(&instance_id)?;
        Ok(())
    }
    fn test_session_response(
        id: &str,
        phase: SessionPhase,
        active: bool,
    ) -> Result<serde_json::Value> {
        let mut session = SessionState::new(
            id.to_owned(),
            std::path::PathBuf::from("."),
            crate::session_config::SessionConfigRef::File {
                path: std::path::PathBuf::from("workflow.yaml"),
            },
            String::new(),
        );
        session.phase = phase;
        let mut value = serde_json::to_value(session)?;
        value["activeOperation"] = active.into();
        Ok(value)
    }

    async fn test_client(
        router: axum::Router,
    ) -> Result<(DaemonClient, tokio::task::JoinHandle<std::io::Result<()>>)> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        Ok((
            DaemonClient {
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(3))
                    .build()
                    .map_err(|error| CruiseError::Other(error.to_string()))?,
                origin: format!("http://{address}/"),
                token: "test-token".to_owned(),
            },
            server,
        ))
    }

    fn test_sse(event_json: &serde_json::Value) -> axum::response::Response {
        let mut response = axum::response::Response::new(axum::body::Body::from(format!(
            "event: app\ndata: {event_json}\n\n"
        )));
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/event-stream"),
        );
        response
    }

    #[tokio::test]
    async fn wait_for_terminal_resyncs_state_after_sse_closes_early() -> Result<()> {
        use axum::response::IntoResponse as _;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let state_reads = std::sync::Arc::new(AtomicUsize::new(0));
        let state_reads_route = std::sync::Arc::clone(&state_reads);
        let prompt_reads = std::sync::Arc::new(AtomicUsize::new(0));
        let prompt_reads_route = std::sync::Arc::clone(&prompt_reads);
        let event_reads = std::sync::Arc::new(AtomicUsize::new(0));
        let event_reads_route = std::sync::Arc::clone(&event_reads);
        let router = axum::Router::new()
            .route(
                "/api/v1/sessions/stream-test",
                axum::routing::get(move || {
                    let reads = std::sync::Arc::clone(&state_reads_route);
                    async move {
                        let first = reads.fetch_add(1, Ordering::SeqCst) == 0;
                        let phase = if first {
                            SessionPhase::Running
                        } else {
                            SessionPhase::Completed
                        };
                        match test_session_response("stream-test", phase, first) {
                            Ok(value) => axum::Json(value).into_response(),
                            Err(error) => (
                                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                error.to_string(),
                            )
                                .into_response(),
                        }
                    }
                }),
            )
            .route(
                "/api/v1/sessions/stream-test/prompts",
                axum::routing::get(move || {
                    let reads = std::sync::Arc::clone(&prompt_reads_route);
                    async move {
                        reads.fetch_add(1, Ordering::SeqCst);
                        axum::Json(Vec::<crate::application::PendingPrompt>::new())
                    }
                }),
            )
            .route(
                "/api/v1/events",
                axum::routing::get(move || {
                    let reads = std::sync::Arc::clone(&event_reads_route);
                    async move {
                        reads.fetch_add(1, Ordering::SeqCst);
                        test_sse(&serde_json::json!({
                            "event": "runStarted",
                            "data": { "sessionId": "stream-test" }
                        }))
                    }
                }),
            );
        let (client, server) = test_client(router).await?;
        let stream = client
            .events(EventFilter {
                session: Some("stream-test".to_owned()),
                batch: false,
            })
            .await?;
        let mut events = Vec::new();
        let mut handler = |event: &ApplicationEvent| {
            events.push(event.clone());
            Ok(())
        };
        let state = tokio::time::timeout(
            Duration::from_secs(5),
            client.wait_for_terminal_from("stream-test", stream, &mut handler),
        )
        .await
        .map_err(|error| CruiseError::Other(format!("terminal wait timed out: {error}")))??;
        server.abort();

        assert_eq!(state.phase, SessionPhase::Completed);
        assert_eq!(state_reads.load(Ordering::SeqCst), 2);
        assert_eq!(prompt_reads.load(Ordering::SeqCst), 2);
        assert_eq!(event_reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            events,
            vec![ApplicationEvent::RunStarted {
                session_id: "stream-test".to_owned()
            }]
        );
        Ok(())
    }

    #[tokio::test]
    async fn wait_for_terminal_resyncs_while_sse_stays_open() -> Result<()> {
        use axum::response::IntoResponse as _;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let state_reads = std::sync::Arc::new(AtomicUsize::new(0));
        let state_reads_route = std::sync::Arc::clone(&state_reads);
        let router = axum::Router::new()
            .route(
                "/api/v1/sessions/missed-terminal",
                axum::routing::get(move || {
                    let reads = std::sync::Arc::clone(&state_reads_route);
                    async move {
                        let first = reads.fetch_add(1, Ordering::SeqCst) == 0;
                        let phase = if first {
                            SessionPhase::Running
                        } else {
                            SessionPhase::Suspended
                        };
                        match test_session_response("missed-terminal", phase, first) {
                            Ok(value) => axum::Json(value).into_response(),
                            Err(error) => (
                                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                error.to_string(),
                            )
                                .into_response(),
                        }
                    }
                }),
            )
            .route(
                "/api/v1/sessions/missed-terminal/prompts",
                axum::routing::get(|| async {
                    axum::Json(Vec::<crate::application::PendingPrompt>::new())
                }),
            )
            .route(
                "/api/v1/events",
                axum::routing::get(|| async {
                    axum::response::sse::Sse::new(futures::stream::pending::<
                        std::result::Result<axum::response::sse::Event, std::convert::Infallible>,
                    >())
                }),
            );
        let (client, server) = test_client(router).await?;
        let stream = client
            .events(EventFilter {
                session: Some("missed-terminal".to_owned()),
                batch: false,
            })
            .await?;
        let mut handler = |_: &ApplicationEvent| Ok(());
        let state = tokio::time::timeout(
            Duration::from_secs(5),
            client.wait_for_terminal_from("missed-terminal", stream, &mut handler),
        )
        .await
        .map_err(|error| CruiseError::Other(format!("terminal wait timed out: {error}")))??;
        server.abort();

        assert_eq!(state.phase, SessionPhase::Suspended);
        assert_eq!(state_reads.load(Ordering::SeqCst), 2);
        Ok(())
    }
    #[tokio::test]
    async fn run_failure_event_survives_resync_to_planned_state() -> Result<()> {
        use axum::response::IntoResponse as _;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let state_reads = std::sync::Arc::new(AtomicUsize::new(0));
        let state_reads_route = std::sync::Arc::clone(&state_reads);
        let router = axum::Router::new()
            .route(
                "/api/v1/sessions/failed-resync",
                axum::routing::get(move || {
                    let reads = std::sync::Arc::clone(&state_reads_route);
                    async move {
                        let active = reads.fetch_add(1, Ordering::SeqCst) == 0;
                        match test_session_response("failed-resync", SessionPhase::Planned, active)
                        {
                            Ok(value) => axum::Json(value).into_response(),
                            Err(error) => (
                                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                error.to_string(),
                            )
                                .into_response(),
                        }
                    }
                }),
            )
            .route(
                "/api/v1/sessions/failed-resync/prompts",
                axum::routing::get(|| async {
                    axum::Json(Vec::<crate::application::PendingPrompt>::new())
                }),
            )
            .route(
                "/api/v1/events",
                axum::routing::get(|| async {
                    let payload = serde_json::json!({
                        "event": "runFailed",
                        "data": {
                            "sessionId": "failed-resync",
                            "error": "base CI is failing"
                        }
                    });
                    let event = axum::response::sse::Event::default()
                        .event("app")
                        .data(payload.to_string());
                    let events = futures::stream::once(async move {
                        tokio::time::sleep(Duration::from_millis(1_500)).await;
                        Ok::<_, std::convert::Infallible>(event)
                    });
                    axum::response::sse::Sse::new(events)
                }),
            );
        let (client, server) = test_client(router).await?;
        let stream = client
            .events(EventFilter {
                session: Some("failed-resync".to_owned()),
                batch: false,
            })
            .await?;
        let mut handler = crate::cli_events::TerminalEvents::new(&client, "failed-resync");
        let finished = tokio::time::timeout(
            Duration::from_secs(5),
            crate::cli_events::drive(
                &client,
                "failed-resync",
                stream,
                crate::cli_events::CtrlC::new()?,
                &mut handler,
            ),
        )
        .await
        .map_err(|error| CruiseError::Other(format!("terminal wait timed out: {error}")))??;
        server.abort();

        assert_eq!(finished.state.phase, SessionPhase::Planned);
        assert_eq!(finished.failure.as_deref(), Some("base CI is failing"));
        assert!(state_reads.load(Ordering::SeqCst) >= 2);
        Ok(())
    }
    #[tokio::test]
    async fn run_start_surfaces_synchronous_preflight_error() -> Result<()> {
        let router = axum::Router::new().route(
            "/api/v1/sessions/sync-start/run",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "base CI is failing"})),
                )
            }),
        );
        let (client, server) = test_client(router).await?;
        let result = client
            .start_run("sync-start", &crate::application::RunRequest::default())
            .await;
        let Err(error) = result else {
            server.abort();
            return Err(CruiseError::Other(
                "synchronous run start unexpectedly succeeded".to_owned(),
            ));
        };
        server.abort();

        assert_eq!(error.to_string(), "base CI is failing");
        Ok(())
    }

    #[tokio::test]
    async fn wait_for_terminal_ignores_404_resync_while_stream_is_open() -> Result<()> {
        use axum::response::IntoResponse as _;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let state_reads = std::sync::Arc::new(AtomicUsize::new(0));
        let state_reads_route = std::sync::Arc::clone(&state_reads);
        let terminal = serde_json::json!({
            "event": "runFinished",
            "data": { "sessionId": "late-terminal", "phase": "Completed" }
        });
        let router = axum::Router::new()
            .route(
                "/api/v1/sessions/late-terminal",
                axum::routing::get(move || {
                    let reads = std::sync::Arc::clone(&state_reads_route);
                    async move {
                        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                            match test_session_response(
                                "late-terminal",
                                SessionPhase::Running,
                                true,
                            ) {
                                Ok(value) => axum::Json(value).into_response(),
                                Err(error) => (
                                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                    error.to_string(),
                                )
                                    .into_response(),
                            }
                        } else {
                            (
                                axum::http::StatusCode::NOT_FOUND,
                                axum::Json(serde_json::json!({ "error": "session disposed" })),
                            )
                                .into_response()
                        }
                    }
                }),
            )
            .route(
                "/api/v1/sessions/late-terminal/prompts",
                axum::routing::get(|| async {
                    axum::Json(Vec::<crate::application::PendingPrompt>::new())
                }),
            )
            .route(
                "/api/v1/events",
                axum::routing::get(move || {
                    let terminal = terminal.clone();
                    async move {
                        let event = axum::response::sse::Event::default()
                            .event("app")
                            .data(terminal.to_string());
                        let events = futures::stream::once(async move {
                            tokio::time::sleep(Duration::from_millis(1_500)).await;
                            Ok::<_, std::convert::Infallible>(event)
                        });
                        axum::response::sse::Sse::new(events)
                    }
                }),
            );
        let (client, server) = test_client(router).await?;
        let stream = client
            .events(EventFilter {
                session: Some("late-terminal".to_owned()),
                batch: false,
            })
            .await?;
        let mut events = Vec::new();
        let mut handler = |event: &ApplicationEvent| {
            events.push(event.clone());
            Ok(())
        };
        let state = tokio::time::timeout(
            Duration::from_secs(5),
            client.wait_for_terminal_from("late-terminal", stream, &mut handler),
        )
        .await
        .map_err(|error| CruiseError::Other(format!("terminal wait timed out: {error}")))??;
        server.abort();

        assert_eq!(state.phase, SessionPhase::Completed);
        assert!(state_reads.load(Ordering::SeqCst) >= 2);
        assert_eq!(
            events,
            vec![ApplicationEvent::RunFinished {
                session_id: "late-terminal".to_owned(),
                phase: "Completed".to_owned(),
            }]
        );
        Ok(())
    }

    #[tokio::test]
    async fn wait_for_terminal_returns_404_when_session_is_deleted_during_resync() -> Result<()> {
        use axum::response::IntoResponse as _;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let state_reads = std::sync::Arc::new(AtomicUsize::new(0));
        let state_reads_route = std::sync::Arc::clone(&state_reads);
        let router = axum::Router::new()
            .route(
                "/api/v1/sessions/deleted-resync",
                axum::routing::get(move || {
                    let reads = std::sync::Arc::clone(&state_reads_route);
                    async move {
                        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                            match test_session_response(
                                "deleted-resync",
                                SessionPhase::Running,
                                true,
                            ) {
                                Ok(value) => axum::Json(value).into_response(),
                                Err(error) => (
                                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                    error.to_string(),
                                )
                                    .into_response(),
                            }
                        } else {
                            (
                                axum::http::StatusCode::NOT_FOUND,
                                axum::Json(serde_json::json!({ "error": "session deleted" })),
                            )
                                .into_response()
                        }
                    }
                }),
            )
            .route(
                "/api/v1/sessions/deleted-resync/prompts",
                axum::routing::get(|| async {
                    axum::Json(Vec::<crate::application::PendingPrompt>::new())
                }),
            )
            .route(
                "/api/v1/events",
                axum::routing::get(|| async {
                    test_sse(&serde_json::json!({
                        "event": "runStarted",
                        "data": { "sessionId": "deleted-resync" }
                    }))
                }),
            );
        let (client, server) = test_client(router).await?;
        let stream = client
            .events(EventFilter {
                session: Some("deleted-resync".to_owned()),
                batch: false,
            })
            .await?;
        let mut handler = |_: &ApplicationEvent| Ok(());
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.wait_for_terminal_from("deleted-resync", stream, &mut handler),
        )
        .await
        .map_err(|error| CruiseError::Other(format!("terminal wait timed out: {error}")))?;
        server.abort();

        assert!(matches!(
            result,
            Err(CruiseError::IoError(error))
                if error.kind() == std::io::ErrorKind::NotFound
        ));
        assert_eq!(state_reads.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn wait_for_terminal_uses_terminal_event_when_disposed_session_returns_404() -> Result<()>
    {
        use axum::response::IntoResponse as _;

        let router = axum::Router::new()
            .route(
                "/api/v1/sessions/disposed-exec",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        axum::Json(serde_json::json!({ "error": "session disposed" })),
                    )
                        .into_response()
                }),
            )
            .route(
                "/api/v1/sessions/disposed-exec/prompts",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        axum::Json(serde_json::json!({ "error": "session disposed" })),
                    )
                        .into_response()
                }),
            )
            .route(
                "/api/v1/events",
                axum::routing::get(|| async {
                    test_sse(&serde_json::json!({
                        "event": "runFinished",
                        "data": {
                            "sessionId": "disposed-exec",
                            "phase": "Completed"
                        }
                    }))
                }),
            );
        let (client, server) = test_client(router).await?;
        let stream = client
            .events(EventFilter {
                session: Some("disposed-exec".to_owned()),
                batch: false,
            })
            .await?;
        let mut handler = |_: &ApplicationEvent| Ok(());
        let state = tokio::time::timeout(
            Duration::from_secs(5),
            client.wait_for_terminal_from("disposed-exec", stream, &mut handler),
        )
        .await
        .map_err(|error| CruiseError::Other(format!("terminal wait timed out: {error}")))??;
        server.abort();

        assert_eq!(state.id, "disposed-exec");
        assert_eq!(state.phase, SessionPhase::Completed);
        assert!(state.completed_at.is_some());
        assert!(state.exec);
        Ok(())
    }
}
