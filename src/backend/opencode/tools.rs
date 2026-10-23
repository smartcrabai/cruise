//! Authenticated, attempt-scoped IPC for the bundled native V2 plugin.
#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;

use super::{
    OpenCodeRunnerConfig,
    runtime::{private_dir, private_write},
};
use crate::{
    backend::{stream::StreamChunk, tool::ToolHandler},
    cancellation::CancellationToken,
    config::{McpTransport, PermissionMode},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};

type Reply = (StatusCode, Json<Value>);

struct CallRecord {
    tool: String,
    input: Value,
    result: watch::Receiver<Option<Value>>,
}

#[derive(Default)]
struct Admission {
    closed: bool,
    calls: HashMap<String, CallRecord>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

#[derive(Clone)]
struct BridgeState {
    token: String,
    session: String,
    tools: HashMap<String, ToolHandler>,
    allowed: HashSet<String>,
    restricted: bool,
    admission: Arc<Mutex<Admission>>,
    cancel: CancellationToken,
    chunks: SyncSender<StreamChunk>,
    effects: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
    ready_notify: Arc<Notify>,
}

#[derive(Deserialize)]
struct Identity {
    #[serde(rename = "sessionID")]
    session: String,
}

#[derive(Deserialize)]
struct Call {
    #[serde(rename = "sessionID")]
    session: String,
    #[serde(rename = "callID")]
    id: String,
    tool: String,
    input: Value,
}

#[derive(Deserialize)]
struct Effect {
    #[serde(rename = "sessionID")]
    session: String,
    #[serde(rename = "callID")]
    id: String,
    tool: String,
}

impl BridgeState {
    fn authorized(&self, headers: &HeaderMap, session: &str) -> bool {
        headers.get("authorization").and_then(|h| h.to_str().ok()) == Some(self.token.as_str())
            && session == self.session
            && !self.cancel.is_cancelled()
    }

    fn barrier(&self) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        self.effects.store(true, Ordering::Release);
        self.chunks.send(StreamChunk::RetryBarrier).is_ok() && !self.cancel.is_cancelled()
    }
}

fn reject(status: StatusCode, message: &str) -> Reply {
    (status, Json(json!({"error": message})))
}

async fn ready(
    State(state): State<BridgeState>,
    headers: HeaderMap,
    Json(identity): Json<Identity>,
) -> Reply {
    if !state.authorized(&headers, &identity.session) {
        return reject(StatusCode::FORBIDDEN, "invalid bridge identity");
    }
    state.ready.store(true, Ordering::Release);
    state.ready_notify.notify_one();
    (StatusCode::OK, Json(json!({"ready": true})))
}

async fn effect(
    State(state): State<BridgeState>,
    headers: HeaderMap,
    Json(call): Json<Effect>,
) -> Reply {
    if !state.authorized(&headers, &call.session)
        || call.id.is_empty()
        || call.id.len() > 256
        || (state.restricted && !state.allowed.contains(&call.tool))
    {
        return reject(StatusCode::FORBIDDEN, "invalid tool execution identity");
    }
    if !state.barrier() {
        return reject(StatusCode::CONFLICT, "execution cancelled");
    }
    (StatusCode::OK, Json(json!({"accepted": true})))
}

async fn callback(
    State(state): State<BridgeState>,
    headers: HeaderMap,
    Json(call): Json<Call>,
) -> Reply {
    if !state.authorized(&headers, &call.session) || call.id.is_empty() || call.id.len() > 256 {
        return reject(StatusCode::FORBIDDEN, "invalid bridge identity");
    }
    let Some(tool) = state.tools.get(&call.tool).cloned() else {
        return reject(StatusCode::FORBIDDEN, "tool not allowed");
    };
    let mut result = {
        let mut admission = state
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if admission.closed || state.cancel.is_cancelled() {
            return reject(StatusCode::CONFLICT, "execution cancelled");
        }
        if let Some(record) = admission.calls.get(&call.id) {
            if record.tool != call.tool || record.input != call.input {
                return reject(
                    StatusCode::CONFLICT,
                    "call ID was already used with different input",
                );
            }
            record.result.clone()
        } else {
            if admission.calls.len() >= 4096 {
                return reject(StatusCode::TOO_MANY_REQUESTS, "tool call limit");
            }
            if !state.barrier() {
                return reject(StatusCode::CONFLICT, "execution cancelled");
            }
            let (tx, rx) = watch::channel(None);
            admission.calls.insert(
                call.id,
                CallRecord {
                    tool: call.tool,
                    input: call.input.clone(),
                    result: rx.clone(),
                },
            );
            // Record irreversibility before starting the handler, independent of
            // whether the model emits any assistant text or the HTTP peer stays.
            let input = call.input;
            let error_tx = tx.clone();
            let spawned = std::thread::Builder::new()
                .name("cruise-opencode-tool".to_string())
                .spawn(move || {
                    let value = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        tool(input)
                    })) {
                        Ok(Ok(text)) => json!({"content": text}),
                        Ok(Err(error)) => json!({"error": error}),
                        Err(_) => json!({"error": "tool worker failed"}),
                    };
                    let _ = tx.send(Some(value));
                });
            match spawned {
                Ok(worker) => admission.workers.push(worker),
                Err(error) => {
                    let _ = error_tx.send(Some(
                        json!({"error": format!("could not start tool worker: {error}")}),
                    ));
                }
            }
            rx
        }
    };
    loop {
        if let Some(value) = result.borrow().clone() {
            return (StatusCode::OK, Json(value));
        }
        tokio::select! {
            () = state.cancel.cancelled() => return reject(StatusCode::CONFLICT, "execution cancelled"),
            changed = result.changed() => {
                if changed.is_err() {
                    return reject(StatusCode::INTERNAL_SERVER_ERROR, "tool worker closed without a result");
                }
            }
        }
    }
}

pub(crate) struct Bridge {
    task: Option<tokio::task::JoinHandle<()>>,
    state: BridgeState,
    secret_path: PathBuf,
    pub overlay: String,
}

struct PendingSecret(Option<PathBuf>);

impl Drop for PendingSecret {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Bridge {
    pub async fn start(
        config: &OpenCodeRunnerConfig,
        root: &Path,
        id: &str,
        cancel: CancellationToken,
        chunks: SyncSender<StreamChunk>,
    ) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let token = format!("Bearer {}", uuid::Uuid::new_v4().simple());
        let url = format!(
            "http://{}",
            listener.local_addr().map_err(|e| e.to_string())?
        );
        let state = BridgeState {
            token: token.clone(),
            session: id.to_owned(),
            tools: config
                .tools
                .iter()
                .map(|t| (t.name.clone(), Arc::clone(&t.handler)))
                .collect(),
            allowed: allowed_tools(config),
            restricted: config.permission != PermissionMode::Full,
            admission: Arc::default(),
            cancel: cancel.child_token(),
            chunks,
            effects: Arc::default(),
            ready: Arc::default(),
            ready_notify: Arc::default(),
        };
        if state.tools.len() != config.tools.len() {
            return Err("OpenCode tools must have unique names".to_string());
        }
        let plugin = root.join("plugin");
        private_dir(&plugin).map_err(|e| e.to_string())?;
        private_write(
            &plugin.join("package.json"),
            include_bytes!("plugin/package.json"),
        )
        .map_err(|e| e.to_string())?;
        private_write(&plugin.join("index.js"), include_bytes!("plugin/index.js"))
            .map_err(|e| e.to_string())?;
        let tools: Vec<Value> = config
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name, "description": t.description, "parameters": t.parameters,
                })
            })
            .collect();
        let overlay = configuration_overlay(config, &plugin)?;
        let bridge_config = serde_json::to_vec(&json!({
            "url": url, "token": token, "sessionID": id, "tools": tools,
            "allowed": state.allowed, "restricted": state.restricted,
        }))
        .map_err(|e| e.to_string())?;
        let secret_path = plugin.join("bridge.json");
        let mut pending_secret = PendingSecret(Some(secret_path.clone()));
        private_write(&secret_path, &bridge_config).map_err(|e| e.to_string())?;
        // Spawn only after every fallible preparation step, so an invalid
        // overlay cannot leak an unowned callback listener.
        let router = Router::new()
            .route("/call", post(callback))
            .route("/effect", post(effect))
            .route("/ready", post(ready))
            .layer(DefaultBodyLimit::max(1024 * 1024))
            .with_state(state.clone());
        let shutdown = state.cancel.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await;
        });
        let bridge = Self {
            task: Some(task),
            state,
            secret_path,
            overlay,
        };
        pending_secret.0 = None;
        Ok(bridge)
    }

    pub async fn stop(&mut self) -> Result<(), String> {
        self.state.cancel.cancel();
        // Registration and closure share the lock. A peer disconnect can drop
        // the HTTP future, but cannot drop the ownership of an admitted worker.
        let workers = {
            let mut admission = self.state.admission.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            admission.closed = true;
            std::mem::take(&mut admission.workers)
        };
        let drain = tokio::task::spawn_blocking(move || {
            let mut failed = false;
            for worker in workers {
                failed |= worker.join().is_err();
            }
            if failed { Err("OpenCode tool worker join failed".to_owned()) } else { Ok(()) }
        }).await.map_err(|error| format!("OpenCode tool worker drain failed: {error}"))?;
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(Duration::from_secs(1), &mut task)
                .await
                .is_err()
        {
            task.abort();
            let _ = task.await;
        }
        drain?;
        match std::fs::remove_file(&self.secret_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("OpenCode bridge secret cleanup failed: {error}")),
        }
    }

    pub fn had_side_effects(&self) -> bool {
        self.state.effects.load(Ordering::Acquire)
    }

    pub async fn wait_ready(&self, cancel: &CancellationToken) -> Result<(), String> {
        let wait = async {
            while !self.state.ready.load(Ordering::Acquire) {
                self.state.ready_notify.notified().await;
            }
        };
        tokio::select! {
            () = cancel.cancelled() => Err("OpenCode plugin initialization interrupted".to_string()),
            result = tokio::time::timeout(Duration::from_secs(7), wait) => {
                result.map_err(|_| "OpenCode required native plugin was not loaded or was disabled by policy".to_string())
            }
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.state.cancel.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.secret_path);
    }
}

fn configuration_overlay(config: &OpenCodeRunnerConfig, plugin: &Path) -> Result<String, String> {
    let raw = config
        .env
        .get("OPENCODE_CONFIG_CONTENT")
        .cloned()
        .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok())
        .unwrap_or_else(|| "{}".to_string());
    let mut overlay: Value = json5::from_str(&raw)
        .map_err(|_| "OpenCode OPENCODE_CONFIG_CONTENT contains invalid JSONC".to_string())?;
    let object = overlay
        .as_object_mut()
        .ok_or("OpenCode config must be an object")?;
    let plugins = object
        .entry("plugins")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("OpenCode plugins must be an array")?;
    plugins.push(json!(plugin));
    let mcp = object
        .entry("mcp")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("OpenCode mcp must be an object")?;
    for (name, server) in &config.mcp_servers {
        let value = match server.transport() {
            McpTransport::Sse => {
                return Err(format!(
                    "OpenCode V2 does not support MCP SSE transport for '{name}'"
                ));
            }
            McpTransport::Stdio => {
                let mut command: Vec<&str> = vec![
                    server
                        .command
                        .as_deref()
                        .ok_or("OpenCode MCP local command missing")?,
                ];
                command.extend(server.args.iter().map(String::as_str));
                json!({"type": "local", "command": command, "environment": server.env, "enabled": true})
            }
            McpTransport::Http => {
                json!({"type": "remote", "url": server.url, "headers": server.headers, "oauth": false, "enabled": true})
            }
        };
        mcp.insert(name.clone(), value);
    }
    serde_json::to_string(&overlay).map_err(|e| e.to_string())
}

pub(crate) fn allowed_tools(config: &OpenCodeRunnerConfig) -> HashSet<String> {
    let mut tools: HashSet<String> = ["read", "glob", "grep", "list", "webfetch", "websearch"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if config.permission == PermissionMode::Edit {
        tools.extend(
            ["write", "edit", "patch", "apply_patch"]
                .into_iter()
                .map(str::to_owned),
        );
    }
    tools.extend(config.tools.iter().map(|t| t.name.clone()));
    tools
}

pub(crate) fn permissions(config: &OpenCodeRunnerConfig) -> Value {
    let mut rules = vec![
        json!({"action": "*", "resource": "*", "effect": if config.permission == PermissionMode::Full {"allow"} else {"deny"}}),
    ];
    if config.permission != PermissionMode::Full {
        let mut allowed: Vec<_> = allowed_tools(config).into_iter().collect();
        allowed.sort();
        for name in allowed {
            rules.push(json!({"action": name, "resource": "*", "effect": "allow"}));
        }
    }
    for name in ["question", "ask", "ask_user_question"] {
        rules.push(json!({"action": name, "resource": "*", "effect": "deny"}));
    }
    json!(rules)
}
