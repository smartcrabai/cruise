//! Owned `OpenCode` V2 runner. HTTP DTOs and process policy remain private here.
mod client;
mod events;
mod runtime;
#[cfg(test)]
mod tests;
mod tools;
mod types;
use crate::{
    backend::{stream::StreamChunk, tool::CruiseTool},
    cancellation::CancellationToken,
    config::{McpServers, PermissionMode},
    error::{CruiseError, Result},
};
use futures::StreamExt;
pub(crate) use runtime::cleanup_conversation;
use serde_json::json;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    time::Duration,
};
pub(crate) use types::ModelRef;

#[derive(Default)]
pub(crate) struct OpenCodeRunnerConfig {
    pub permission: PermissionMode,
    pub model_ref: Option<String>,
    pub cli_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub resume_session_id: Option<String>,
    pub tools: Vec<CruiseTool>,
    pub mcp_servers: McpServers,
    pub env: HashMap<String, String>,
    pub cancel: Option<CancellationToken>,
    pub keep_session_home: bool,
    pub computer_use: bool,
}
pub(crate) fn parse_model_ref(raw: Option<&str>) -> Result<Option<ModelRef>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let (base, variant) = raw
        .split_once('#')
        .map_or((raw, None), |(base, variant)| (base, Some(variant)));
    let (provider, id) = base.split_once('/').ok_or_else(|| {
        CruiseError::InvalidStepConfig("OpenCode model requires provider/model#variant".into())
    })?;
    if provider.is_empty()
        || id.is_empty()
        || variant.is_some_and(|v| v.is_empty() || v.contains('#'))
        || raw.trim() != raw
    {
        return Err(CruiseError::InvalidStepConfig(
            "invalid OpenCode provider/model#variant reference".into(),
        ));
    }
    Ok(Some(ModelRef {
        provider_id: provider.into(),
        id: id.into(),
        variant: variant.map(str::to_owned),
    }))
}
struct AttemptError {
    message: String,
    provider_failure: Option<types::ModelFailure>,
}
impl From<String> for AttemptError {
    fn from(message: String) -> Self {
        Self {
            message,
            provider_failure: None,
        }
    }
}
impl From<&str> for AttemptError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

pub(crate) fn stream_agent(config: OpenCodeRunnerConfig, prompt: String) -> Receiver<StreamChunk> {
    let (tx, rx) = sync_channel(0);
    std::thread::spawn(move || {
        let result = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(runtime) => {
                let result = runtime.block_on(run_attempt(&config, &prompt, &tx));
                runtime.shutdown_timeout(Duration::from_millis(100));
                result
            }
            Err(error) => Err(AttemptError::from(format!(
                "OpenCode runtime initialization failed: {error}"
            ))),
        };
        let terminal = match result {
            Ok(text) => StreamChunk::Done(text),
            Err(error) => {
                if error.provider_failure.is_none() {
                    let _ = tx.send(StreamChunk::RetryBarrier);
                }
                if error
                    .provider_failure
                    .as_ref()
                    .is_some_and(types::ModelFailure::is_limit)
                {
                    StreamChunk::Limit(crate::backend::stream::LimitError {
                        provider: config
                            .model_ref
                            .as_deref()
                            .and_then(|raw| raw.split_once('/'))
                            .map_or("opencode", |(provider, _)| provider)
                            .to_owned(),
                        detail: error.message,
                    })
                } else {
                    StreamChunk::Error(error.message)
                }
            }
        };
        let _ = tx.send(terminal);
    });
    rx
}
#[expect(
    clippy::too_many_lines,
    reason = "The attempt owns process, bridge and storage cleanup in one explicit lifecycle scope"
)]
async fn run_attempt(
    config: &OpenCodeRunnerConfig,
    prompt: &str,
    tx: &SyncSender<StreamChunk>,
) -> std::result::Result<String, AttemptError> {
    let cancel = config.cancel.clone().unwrap_or_default();
    if cancel.is_cancelled() {
        return Err("OpenCode interrupted".into());
    }
    if config.computer_use {
        return Err("OpenCode does not support computer_use".into());
    }
    let model = parse_model_ref(config.model_ref.as_deref()).map_err(|e| e.to_string())?;
    let cwd = match &config.cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    }
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let id = config
        .resume_session_id
        .clone()
        .unwrap_or_else(|| format!("ses_{}", uuid::Uuid::new_v4().simple()));
    if !id.starts_with("ses")
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("OpenCode session ID is invalid".into());
    }
    let storage = runtime::Storage::prepare(config, id.clone(), cwd.clone())?;
    let mut safe_failure = None;
    let mut conversation_clean = storage.initial_clean;
    let mut checkpointed = config.resume_session_id.is_some();
    let result = async {
        let mut bridge =
            tools::Bridge::start(config, &storage.root, &id, cancel.child_token(), tx.clone())
                .await?;
        if let Err(error) = storage.mark(false) {
            bridge.stop().await?;
            return Err(error);
        }
        let mut runtime = match runtime::Runtime::spawn(config, &storage, &bridge.overlay) {
            Ok(runtime) => {
                conversation_clean = false;
                runtime
            }
            Err(error) => {
                // Unix group_spawn fails only at process creation. Windows can
                // also fail assigning an already-created child to its job.
                conversation_clean &= cfg!(unix);
                bridge.stop().await?;
                storage.mark(conversation_clean)?;
                return Err(error);
            }
        };
        let turn_result = async {
            let url = runtime.startup(&cancel).await?;
            let client =
                client::Client::new(url, runtime.password.clone()).map_err(|e| e.to_string())?;
            let setup = tokio::time::timeout(Duration::from_secs(7), async {
                let info = client.info().await.map_err(|e| e.to_string())?;
                if !info.version.starts_with("2.") {
                    return Err(format!("OpenCode V2 required, received {}", info.version));
                }
                let rules = tools::permissions(config);
                let session = if config.resume_session_id.is_some() {
                    let session = client.session(&id).await.map_err(|e| e.to_string())?;
                    if !client
                        .inbox(&id)
                        .await
                        .map_err(|e| e.to_string())?
                        .is_empty()
                    {
                        return Err("OpenCode resume refused: pending inbox inputs".into());
                    }
                    client
                        .permissions(&id, &rules)
                        .await
                        .map_err(|e| e.to_string())?;
                    if let Some(model) = &model {
                        client.model(&id, model).await.map_err(|e| e.to_string())?;
                    }
                    session
                } else {
                    client
                        .create(&types::CreateSession {
                            id: &id,
                            location: json!({"directory":cwd}),
                            permissions: rules,
                            model: model.as_ref(),
                        })
                        .await
                        .map_err(|e| e.to_string())?
                };
                if session.id != id {
                    return Err("OpenCode create returned unexpected session ID".into());
                }
                let mut variables: HashMap<String, String> = std::env::vars_os()
                    .filter_map(|(key, value)| {
                        Some((key.into_string().ok()?, value.into_string().ok()?))
                    })
                    .collect();
                variables.extend(config.env.clone());
                for key in [
                    "OPENCODE_SERVER_PASSWORD",
                    "OPENCODE_PASSWORD",
                    "OPENCODE_CONFIG_CONTENT",
                    "OPENCODE_DB",
                ] {
                    variables.remove(key);
                }
                client
                    .environment(&id, &variables)
                    .await
                    .map_err(|e| e.to_string())?;
                // The canonical endpoint initializes location plugins asynchronously.
                // Its snapshot can be empty before setup has completed, so only the
                // authenticated bridge handshake establishes native tool readiness.
                client
                    .plugins(&cwd)
                    .await
                    .map_err(|error| error.to_string())?;
                bridge.wait_ready(&cancel).await?;
                Ok(())
            })
            .await
            .map_err(|_| "OpenCode V2 setup timed out".to_owned())?;
            setup?;
            conversation_clean = true;
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            tx.send(StreamChunk::SessionReady {
                id: id.clone(),
                ready: ready_tx,
            })
            .map_err(|_| "OpenCode receiver closed".to_owned())?;
            loop {
                match ready_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(true) => {
                        checkpointed = true;
                        break;
                    }
                    Ok(false) => return Err("OpenCode session checkpoint was rejected".into()),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("OpenCode checkpoint receiver closed".into());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) if cancel.is_cancelled() => {
                        return Err("OpenCode checkpoint interrupted".into());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            if cancel.is_cancelled() {
                return Err("OpenCode interrupted before prompt".into());
            }
            let mut failure = None;
            let inbox = format!("msg_{}", uuid::Uuid::new_v4().simple());
            conversation_clean = false;
            let result = execute(
                &client,
                TurnInput {
                    session: &id,
                    inbox: &inbox,
                    text: prompt,
                },
                &cancel,
                tx,
                &mut failure,
                &mut conversation_clean,
            )
            .await;
            if !bridge.had_side_effects() {
                safe_failure = failure;
            }
            if result.is_err() && !conversation_clean {
                conversation_clean = tokio::time::timeout(
                    Duration::from_secs(3),
                    settle_interrupted(&client, &id, &inbox),
                )
                .await
                .is_ok_and(|result| result.is_ok());
            }
            result
        }
        .await;
        let stop = runtime.stop().await;
        let bridge_stop = bridge.stop().await;
        if stop.is_err() || bridge_stop.is_err() {
            conversation_clean = false;
            safe_failure = None;
        }
        stop?;
        bridge_stop?;
        turn_result
    }
    .await;
    if !conversation_clean {
        safe_failure = None;
    }
    storage.finish(conversation_clean, checkpointed)?;
    result.map_err(|message| AttemptError {
        message,
        provider_failure: safe_failure,
    })
}
#[derive(Clone, Copy)]
struct TurnInput<'a> {
    session: &'a str,
    inbox: &'a str,
    text: &'a str,
}
async fn execute(
    client: &client::Client,
    input: TurnInput<'_>,
    cancel: &CancellationToken,
    tx: &SyncSender<StreamChunk>,
    failure: &mut Option<types::ModelFailure>,
    clean: &mut bool,
) -> std::result::Result<String, String> {
    let TurnInput {
        session: id,
        inbox,
        text: prompt,
    } = input;
    // Establishing the subscription cannot admit or replay an input.
    *clean = true;
    let response = tokio::select! {
        ()=cancel.cancelled()=>return Err("OpenCode event subscription interrupted".into()),
        response=tokio::time::timeout(Duration::from_secs(5),client.events())=>response.map_err(|_|"OpenCode event subscription timed out")?.map_err(|e|e.to_string())?,
    };
    let mut live = response.bytes_stream();
    let mut decoder = events::Decoder::default();
    // Observe the subscription handshake before admitting a durable input.
    loop {
        let chunk = tokio::select! {()=cancel.cancelled()=>return Err("OpenCode interrupted".into()),value=tokio::time::timeout(Duration::from_secs(5),live.next())=>value.map_err(|_|"OpenCode event subscription timed out")?};
        let bytes = chunk
            .ok_or("OpenCode event subscription closed")?
            .map_err(|e| e.to_string())?;
        let values = decoder.push(&bytes).map_err(|e| e.to_string())?;
        if values.iter().any(|e| e.kind == "server.connected") {
            break;
        }
    }
    let boundary = tokio::select! {
        ()=cancel.cancelled()=>return Err("OpenCode turn boundary interrupted".into()),
        result=tokio::time::timeout(Duration::from_secs(5), durable_boundary(client, id))=>result.map_err(|_|"OpenCode turn boundary timed out")??,
    };
    let mut turn = events::Turn::default();
    turn.sequence = boundary;
    let input = types::Prompt {
        id: inbox,
        text: prompt,
    };
    *clean = false;
    tokio::select! {
        ()=cancel.cancelled()=>return Err("OpenCode interrupted".into()),
        result=tokio::time::timeout(Duration::from_secs(15),client.prompt(id,&input))=>{result.map_err(|_|"OpenCode prompt acceptance uncertain: timeout")?.map_err(|e|format!("OpenCode prompt acceptance uncertain: {e}"))?;}
    }
    loop {
        let chunk = tokio::select! {()=cancel.cancelled()=>return Err("OpenCode execution interrupted".into()),value=live.next()=>value};
        if let Some(Ok(bytes)) = chunk {
            for event in decoder.push(&bytes).map_err(|e| e.to_string())? {
                match turn.apply(event, id, inbox) {
                    Ok(Some(delta)) => tx
                        .send(StreamChunk::Delta(delta))
                        .map_err(|_| "OpenCode receiver closed")?,
                    Ok(None) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
        } else {
            reconcile(client, id, inbox, &mut turn, tx).await?;
            if turn.terminal.is_none() {
                return Err("OpenCode execution outcome uncertain after event disconnect".into());
            }
        }
        if let Some(terminal) = turn.terminal.take() {
            *clean = turn.delivered;
            let output = turn.output();
            if output.is_empty()
                && turn
                    .failure
                    .as_ref()
                    .is_some_and(|error| error.kind.starts_with("provider."))
            {
                *failure = turn.failure.take();
            }
            terminal?;
            return Ok(output);
        }
    }
}
// Snapshot history without reducing it into this attempt. The subscription is
// already established, so records after this fence remain available live or in
// reconciliation, while earlier executions cannot supply terminal evidence.
async fn durable_boundary(client: &client::Client, id: &str) -> std::result::Result<u64, String> {
    let response = client
        .log(id, 0)
        .await
        .map_err(|error| format!("OpenCode turn boundary failed: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut decoder = events::Decoder::default();
    let mut sequence = 0;
    while let Some(bytes) = stream.next().await {
        let bytes = bytes.map_err(|error| error.to_string())?;
        for event in decoder.push(&bytes).map_err(|error| error.to_string())? {
            if event.kind == "log.synced" {
                if event.aggregate_id.as_deref() != Some(id) {
                    return Err("OpenCode turn boundary synchronization aggregate mismatch".into());
                }
                let boundary = event.seq.unwrap_or(sequence);
                if boundary < sequence {
                    return Err("OpenCode turn boundary sequence regressed".into());
                }
                return Ok(boundary);
            }
            if let Some(durable) = event.durable {
                if durable.aggregate_id != id {
                    return Err("OpenCode turn boundary aggregate mismatch".into());
                }
                sequence = sequence.max(durable.seq);
            }
        }
    }
    Err("OpenCode turn boundary ended before synchronization".into())
}
async fn reconcile(
    client: &client::Client,
    id: &str,
    inbox: &str,
    turn: &mut events::Turn,
    tx: &SyncSender<StreamChunk>,
) -> std::result::Result<(), String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let response = client
            .log(id, turn.sequence)
            .await
            .map_err(|e| format!("OpenCode durable reconciliation failed: {e}"))?;
        let mut stream = response.bytes_stream();
        let mut decoder = events::Decoder::default();
        while let Some(bytes) = stream.next().await {
            let bytes = bytes.map_err(|e| e.to_string())?;
            for event in decoder.push(&bytes).map_err(|e| e.to_string())? {
                if event.kind == "log.synced" {
                    if event.aggregate_id.as_deref() != Some(id) {
                        return Err("OpenCode durable synchronization aggregate mismatch".into());
                    }
                    return Ok(());
                }
                if let Some(delta) = turn.apply(event, id, inbox).map_err(|e| e.to_string())? {
                    tx.send(StreamChunk::Delta(delta))
                        .map_err(|_| "OpenCode receiver closed")?;
                }
            }
        }
        Err("OpenCode durable log ended before synchronization".into())
    })
    .await
    .map_err(|_| "OpenCode durable reconciliation timed out".to_owned())?
}

async fn settle_interrupted(
    client: &client::Client,
    id: &str,
    inbox: &str,
) -> std::result::Result<(), String> {
    client
        .interrupt(id)
        .await
        .map_err(|error| error.to_string())?;
    let pending = client.inbox(id).await.map_err(|error| error.to_string())?;
    if pending
        .iter()
        .any(|item| item["id"].as_str() == Some(inbox))
    {
        client
            .cancel_inbox(id, inbox)
            .await
            .map_err(|error| error.to_string())?;
    }
    if !client
        .inbox(id)
        .await
        .map_err(|error| error.to_string())?
        .is_empty()
    {
        return Err("OpenCode interrupted conversation still has pending inbox inputs".into());
    }
    let response = client.log(id, 0).await.map_err(|error| error.to_string())?;
    let mut stream = response.bytes_stream();
    let mut decoder = events::Decoder::default();
    let mut delivered = false;
    let mut ended = false;
    let mut cancelled = false;
    while let Some(bytes) = stream.next().await {
        let bytes = bytes.map_err(|error| error.to_string())?;
        for event in decoder.push(&bytes).map_err(|error| error.to_string())? {
            if event.kind == "log.synced" {
                if event.aggregate_id.as_deref() != Some(id) {
                    return Err("OpenCode interrupted synchronization aggregate mismatch".into());
                }
                return if (delivered && ended) || cancelled {
                    Ok(())
                } else {
                    Err("OpenCode interrupted execution result remains uncertain".into())
                };
            }
            if event.data["sessionID"].as_str() != Some(id) {
                continue;
            }
            if event.data["inboxID"].as_str() == Some(inbox) {
                if event.kind == "session.inbox.delivered" {
                    delivered = true;
                }
                if event.kind == "session.inbox.cancelled" {
                    cancelled = true;
                }
            }
            if delivered
                && matches!(
                    event.kind.as_str(),
                    "session.execution.succeeded"
                        | "session.execution.failed"
                        | "session.execution.interrupted"
                )
            {
                ended = true;
            }
        }
    }
    Err("OpenCode interrupted durable log ended without synchronization".into())
}
