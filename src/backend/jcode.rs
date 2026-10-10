//! `sdk: jcode` backend, implemented through the official Rust SDK.
//!
//! Each prompt attempt launches an isolated jcode runtime with that prompt's
//! environment. Its session home is private to the Cruise conversation, so
//! concurrent conversations never contend for the user's daemon socket or
//! mutable home files. Session homes live below the effective ambient
//! `JCODE_HOME` (or `~/.jcode`) and are reused by session id for follow-up turns.
//!
//! The SDK is blocking and thread-based. [`stream_agent`] runs it on a worker
//! thread, subscribes before sending the prompt, and answers session-tool calls
//! with the existing in-process handlers.

use jcode_sdk::{
    ApiEvent, JcodeClient, LaunchOptions, ModelRouteInfo, SessionToolDefinition, ToolConfiguration,
    TurnStopReason,
};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use crate::backend::effort::{EffortLevel, effort_from_suffix, split_thinking_suffix};
use crate::backend::stream::{LimitError, StreamChunk};
use crate::backend::tool::CruiseTool;
use crate::cancellation::CancellationToken;
use crate::error::{CruiseError, Result};
const JCODE_HOME_ENV: &str = "JCODE_HOME";
const NO_TELEMETRY_ENV: &str = "JCODE_NO_TELEMETRY";
const JCODE_CHECK_UPDATES_ENV: &str = "JCODE_CHECK_UPDATES";
const OPENAI_SERVICE_TIER_ENV: &str = "JCODE_OPENAI_SERVICE_TIER";
const JCODE_PROVIDER_ENV: &str = "JCODE_PROVIDER";
const JCODE_MODEL_ENV: &str = "JCODE_MODEL";
const JCODE_DISABLED_TOOLS_ENV: &str = "JCODE_DISABLED_TOOLS";
const COMPUTER_USE_TOOL: &str = "macos_computer_use";
const JCODE_CONFIG_FILE: &str = "config.toml";
const OPENAI_SERVICE_TIER_DEFAULT: &str = "off";
const SESSION_HOME_DIR: &str = ".cruise-sdk-sessions";
const SESSION_ID_FILE: &str = ".cruise-session-id";
const LEGACY_CRUISE_MCP_NAME: &str = "cruise";
const CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_SESSION_HOME_AGE: Duration = Duration::from_hours(24);
const HOME_LOCK_FILE: &str = ".cruise-session.lock";
const MODEL_CATALOG_CACHE_FILES: &[&str] = &[
    "openai_model_catalog_cache.json",
    "anthropic_model_catalog_cache.json",
    "copilot_models_cache.json",
    "gemini_models_cache.json",
    "cursor_models_cache.json",
    "antigravity_models_cache.json",
    "bedrock_models_cache.json",
];

struct MaterializedExternalCredential {
    original_canonical_path: String,
    copy_canonical_path: String,
}

const ROOT_LOCK_FILE: &str = ".cruise-session-prune.lock";
/// One `sdk: jcode` prompt attempt. Deliberately not `Debug`: `env` may contain
/// provider credentials.
#[derive(Default)]
pub(crate) struct JcodeRunnerConfig {
    pub(crate) permission: crate::config::PermissionMode,
    pub(crate) model: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) effort: Option<EffortLevel>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) resume_session_id: Option<String>,
    pub(crate) tools: Vec<CruiseTool>,
    pub(crate) mcp_servers: crate::config::McpServers,
    pub(crate) env: HashMap<String, String>,
    pub(crate) cancel: Option<CancellationToken>,
    pub(crate) keep_session_home: bool,
    pub(crate) computer_use: bool,
}

/// Cruise's `provider/model[:effort]` syntax, parsed once for each fallback
/// attempt. `provider` is kept separate so known aliases retain their previous
/// route-selection semantics.
pub(crate) type ModelRef = (Option<String>, Option<String>, Option<EffortLevel>);

pub(crate) fn parse_model_ref(model_ref: Option<&str>) -> Result<ModelRef> {
    let Some(raw) = model_ref.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((None, None, None));
    };
    let (base, suffix) = split_thinking_suffix(raw);
    let effort = suffix.and_then(effort_from_suffix);
    let base = base.trim();
    if base.is_empty() {
        return Ok((None, None, effort));
    }
    match base.split_once('/') {
        None => Ok((None, Some(base.to_string()), effort)),
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
            Ok((Some(provider.to_string()), Some(model.to_string()), effort))
        }
        Some(_) => Err(CruiseError::InvalidStepConfig(format!(
            "invalid model reference '{raw}' for `sdk: jcode`: expected 'provider/model[:effort]', 'model[:effort]', or no value"
        ))),
    }
}

/// Run one attempt on a dedicated thread, keeping blocking SDK calls off the
/// async executor. The zero-capacity channel ensures the session id reaches the
/// caller before a tool handler can block waiting for user input.
pub(crate) fn stream_agent(config: JcodeRunnerConfig, prompt: String) -> Receiver<StreamChunk> {
    let (tx, rx) = sync_channel(0);
    std::thread::spawn(move || {
        let terminal = match run_attempt(&config, &prompt, &tx) {
            Ok(terminal) => terminal,
            Err(message) => StreamChunk::Error(message),
        };
        let _ = tx.send(terminal);
    });
    rx
}

fn run_attempt(
    config: &JcodeRunnerConfig,
    prompt: &str,
    tx: &SyncSender<StreamChunk>,
) -> std::result::Result<StreamChunk, String> {
    let source_home = resolve_source_home(config.cwd.as_deref());
    let session_key = config.resume_session_id.as_deref().map_or_else(
        || uuid::Uuid::new_v4().simple().to_string(),
        session_storage_key,
    );
    let lock_key = source_home.join(SESSION_HOME_DIR).join(&session_key);
    let lock = session_lock(&lock_key);
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut home = prepare_session_home(
        source_home,
        &session_key,
        config.resume_session_id.as_deref(),
        &config.mcp_servers,
    )?;
    let mut session_id = None;
    let run_result = match launch_client(config, &home) {
        Ok(client) => {
            let result = run_client_turn(&client, config, prompt, tx, &mut home, &mut session_id);
            drop(client);
            let cleanup_result = clear_private_daemon(&home.storage_home);
            match (result, cleanup_result) {
                (Ok(terminal), Ok(())) => Ok(terminal),
                (Err(message), Ok(())) => Err(message),
                (Ok(_), Err(error)) => {
                    Err(format!("could not stop private jcode runtime: {error}"))
                }
                (Err(message), Err(error)) => Err(format!(
                    "{message}; could not stop private jcode runtime: {error}"
                )),
            }
        }
        Err(error) => Err(error),
    };
    let finalize_result =
        finish_session_home(&mut home, session_id.as_deref(), config.keep_session_home);
    match (run_result, finalize_result) {
        (Ok(terminal), Ok(())) => Ok(terminal),
        (Err(message), Ok(())) => Err(message),
        (Ok(_), Err(error)) => Err(error.to_string()),
        (Err(message), Err(error)) => Err(format!(
            "{message}; could not persist jcode session home: {error}"
        )),
    }
}

fn launch_client(
    config: &JcodeRunnerConfig,
    home: &SessionHome,
) -> std::result::Result<JcodeClient, String> {
    #[cfg(unix)]
    validate_sdk_socket_paths(&home.sdk_home)?;
    clear_private_daemon(&home.storage_home)
        .map_err(|error| format!("could not clean up stale private jcode runtime: {error}"))?;
    let mut options = LaunchOptions {
        jcode_home: Some(home.sdk_home.clone()),
        working_dir: config.cwd.clone(),
        inherit_logins: false,
        ..Default::default()
    };
    let skip_user_lists =
        config.computer_use && config.permission == crate::config::PermissionMode::Full;
    let process_disabled_tools = if skip_user_lists {
        None
    } else {
        std::env::var(JCODE_DISABLED_TOOLS_ENV).ok()
    };
    let config_toml = if skip_user_lists
        || config.env.contains_key(JCODE_DISABLED_TOOLS_ENV)
        || process_disabled_tools.is_some()
    {
        None
    } else {
        fs::read_to_string(home.storage_home.join(JCODE_CONFIG_FILE)).ok()
    };
    options.env = launch_environment(
        &config.env,
        config.provider.as_deref(),
        config.model.as_deref(),
        config.computer_use,
        process_disabled_tools.as_deref(),
        config_toml.as_deref(),
        config.permission,
    );
    JcodeClient::launch(options).map_err(|error| {
        let hint = if matches!(
            &error.kind,
            &jcode_sdk::ErrorKind::JcodeNotFound
                | &jcode_sdk::ErrorKind::HandshakeFailed
                | &jcode_sdk::ErrorKind::Harness(jcode_sdk::api::ErrorCode::UnsupportedVersion)
        ) {
            "; install or upgrade jcode to 0.88.0 or newer"
        } else {
            ""
        };
        format!("could not start the jcode SDK runtime{hint}: {error}")
    })
}

fn launch_environment(
    workflow_env: &HashMap<String, String>,
    provider: Option<&str>,
    model: Option<&str>,
    computer_use: bool,
    process_disabled_tools: Option<&str>,
    config_toml: Option<&str>,
    permission: crate::config::PermissionMode,
) -> HashMap<OsString, OsString> {
    let mut env: HashMap<OsString, OsString> = workflow_env
        .iter()
        .filter(|(key, _)| key.as_str() != JCODE_HOME_ENV)
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect();
    env.insert(OsString::from(NO_TELEMETRY_ENV), OsString::from("1"));
    env.insert(OsString::from(JCODE_CHECK_UPDATES_ENV), OsString::from("0"));
    if !workflow_env.contains_key(OPENAI_SERVICE_TIER_ENV)
        && std::env::var_os(OPENAI_SERVICE_TIER_ENV).is_none()
    {
        env.insert(
            OsString::from(OPENAI_SERVICE_TIER_ENV),
            OsString::from(OPENAI_SERVICE_TIER_DEFAULT),
        );
    }
    if let Some(provider) = provider {
        env.insert(OsString::from(JCODE_PROVIDER_ENV), OsString::from(provider));
    }
    if let Some(model) = model {
        env.insert(
            OsString::from(JCODE_MODEL_ENV),
            OsString::from(routed_model_arg(provider, model).as_ref()),
        );
    }
    let restricted = permission != crate::config::PermissionMode::Full;
    if restricted || !computer_use {
        let mut disabled_tools = computer_use_disabled_tools(
            workflow_env
                .get(JCODE_DISABLED_TOOLS_ENV)
                .map(String::as_str),
            process_disabled_tools,
            config_toml,
        );
        for tool in permission_disabled_tools(permission) {
            if !disabled_tools.split(',').any(|existing| existing == *tool) {
                disabled_tools.push(',');
                disabled_tools.push_str(tool);
            }
        }
        env.insert(
            OsString::from(JCODE_DISABLED_TOOLS_ENV),
            OsString::from(disabled_tools),
        );
    }
    env
}

/// Tools a permission mode removes on top of the user's disabled list.
fn permission_disabled_tools(permission: crate::config::PermissionMode) -> &'static [&'static str] {
    match permission {
        crate::config::PermissionMode::ReadOnly => &["bash", "edit", "write", "apply_patch"],
        crate::config::PermissionMode::Edit => &["bash"],
        crate::config::PermissionMode::Full => &[],
    }
}

fn computer_use_disabled_tools(
    workflow_value: Option<&str>,
    process_value: Option<&str>,
    config_toml: Option<&str>,
) -> String {
    let mut disabled_tools = workflow_value.or(process_value).map_or_else(
        || config_toml.map_or_else(Vec::new, configured_disabled_tools),
        |value| {
            value
                .split([',', '\n'])
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(str::to_string)
                .collect()
        },
    );

    let mut unique_tools = Vec::with_capacity(disabled_tools.len() + 1);
    for tool in disabled_tools.drain(..) {
        if !unique_tools.contains(&tool) {
            unique_tools.push(tool);
        }
    }
    if !unique_tools.iter().any(|tool| tool == COMPUTER_USE_TOOL) {
        unique_tools.push(COMPUTER_USE_TOOL.to_string());
    }
    unique_tools.join(",")
}

fn configured_disabled_tools(config_toml: &str) -> Vec<String> {
    let Ok(config) = toml::from_str::<toml::Table>(config_toml) else {
        return Vec::new();
    };
    config
        .get("tools")
        .and_then(toml::Value::as_table)
        .and_then(|tools| tools.get("disabled"))
        .and_then(toml::Value::as_array)
        .map(|disabled| {
            disabled
                .iter()
                .filter_map(toml::Value::as_str)
                .flat_map(|value| value.split([',', '\n']))
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[expect(
    clippy::too_many_lines,
    reason = "one ordered loop handles text, tools, stops, and cancellation"
)]
fn run_client_turn(
    client: &JcodeClient,
    config: &JcodeRunnerConfig,
    prompt: &str,
    tx: &SyncSender<StreamChunk>,
    home: &mut SessionHome,
    session_id: &mut Option<String>,
) -> std::result::Result<StreamChunk, String> {
    if !client.supports("sessions") {
        return Err(
            "the jcode harness does not support sessions; upgrade jcode to 0.88.0 or newer"
                .to_string(),
        );
    }
    if !config.tools.is_empty() && !client.supports("session_tools") {
        return Err(
            "the jcode harness does not support session tools; upgrade jcode to 0.88.0 or newer"
                .to_string(),
        );
    }
    let working_dir = config
        .cwd
        .as_deref()
        .map(|path| path.to_string_lossy().into_owned());
    let session = match config.resume_session_id.as_deref() {
        Some(id) => match client.attach_session(id) {
            Ok(session) => session,
            Err(error) if !home.resume_session_found && error.code() == "unknown_session" => client
                .create_session(working_dir)
                .map_err(|error| error.to_string())?,
            Err(error) => return Err(error.to_string()),
        },
        None => client
            .create_session(working_dir)
            .map_err(|error| error.to_string())?,
    };
    *session_id = Some(session.session_id.clone());
    write_session_id(home, &session.session_id).map_err(|error| error.to_string())?;
    tx.send(StreamChunk::Session(session.session_id.clone()))
        .map_err(|_| "jcode output stream was closed".to_string())?;

    if !config.tools.is_empty() {
        let custom = config
            .tools
            .iter()
            .map(|tool| {
                let parameters = tool.parameters.as_object().cloned().ok_or_else(|| {
                    format!("tool '{}' parameters must be a JSON object", tool.name)
                })?;
                Ok(SessionToolDefinition {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters,
                })
            })
            .collect::<std::result::Result<Vec<_>, String>>()?;
        client
            .configure_tools(
                &session.session_id,
                ToolConfiguration {
                    custom,
                    ..Default::default()
                },
            )
            .map_err(|error| error.to_string())?;
    }

    if let Some(model) = config.model.as_deref() {
        let runtime = client
            .get_runtime_info(&session.session_id)
            .map_err(|error| error.to_string())?;
        let model_request =
            model_switch_request(config.provider.as_deref(), model, &runtime.routes);
        client
            .set_model(&session.session_id, &model_request)
            .map_err(|error| error.to_string())?;
    }
    if let Some(effort) = config.effort {
        client
            .set_reasoning_effort(&session.session_id, effort.as_str())
            .map_err(|error| error.to_string())?;
    }

    // Subscribe before sending so early text and tool events cannot be missed.
    let events = client.events(Some(&session.session_id));
    client
        .send_message(&session.session_id, prompt, Vec::new(), None)
        .map_err(|error| error.to_string())?;

    let mut text = TextCollector::default();
    let mut stopped = None;
    let mut cancel_deadline = None;
    loop {
        if cancel_deadline.is_none()
            && config
                .cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            cancel_deadline = Some(Instant::now() + CANCEL_SETTLE_TIMEOUT);
            client
                .cancel(&session.session_id)
                .map_err(|error| error.to_string())?;
        }
        if cancel_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err("jcode did not stop after the session cancellation request".to_string());
        }
        let Some(event) = events.next_timeout(Duration::from_millis(50)) else {
            if client.is_closed() {
                return Err("jcode harness disconnected before the turn finished".to_string());
            }
            continue;
        };
        match event {
            ApiEvent::TextDelta {
                text: delta,
                message_id,
                ..
            } => {
                text.append(message_id, &delta);
                if tx.send(StreamChunk::Delta(delta)).is_err() {
                    let _ = client.cancel(&session.session_id);
                    return Err(
                        "jcode output stream was closed; the active session was cancelled"
                            .to_string(),
                    );
                }
            }
            ApiEvent::TextReplace {
                text: replacement,
                message_id,
                ..
            } => text.replace(message_id, replacement),
            ApiEvent::TextDone { message_id, .. } => text.finish_message(message_id),
            ApiEvent::ToolCall {
                session_id: tool_session,
                call_id,
                name,
                input,
            } => {
                let tool = config.tools.iter().find(|tool| tool.name == name);
                let (output, error) = match tool {
                    Some(tool) => match (tool.handler)(input) {
                        Ok(output) => (output, None),
                        Err(message) => (String::new(), Some(message)),
                    },
                    None => (
                        String::new(),
                        Some(format!("unknown cruise session tool '{name}'")),
                    ),
                };
                client
                    .submit_tool_result(&tool_session, &call_id, &output, error)
                    .map_err(|error| error.to_string())?;
            }
            ApiEvent::TurnStopped {
                reason, message, ..
            } => {
                stopped = Some((reason, message));
            }
            ApiEvent::TurnDone { .. } => {
                return Ok(match stopped {
                    Some((reason, message)) => {
                        stop_chunk(reason, message, config.provider.as_deref())
                    }
                    None => StreamChunk::Done(text.final_text()),
                });
            }
            ApiEvent::Error { code, message } => {
                if let Some((reason, stop_message)) = stopped {
                    let detail = if stop_message.trim().is_empty() {
                        message.clone()
                    } else {
                        stop_message
                    };
                    if crate::retry::is_limit_message(&detail) {
                        return Ok(StreamChunk::Limit(LimitError {
                            provider: config.provider.as_deref().unwrap_or("jcode").to_string(),
                            detail,
                        }));
                    }
                    if reason == TurnStopReason::LimitReached {
                        return Ok(StreamChunk::Error(format!(
                            "jcode turn stopped ({reason:?}): {detail}"
                        )));
                    }
                }
                return Ok(error_chunk(
                    format!("{code:?}: {message}"),
                    config.provider.as_deref(),
                ));
            }
            _ => {}
        }
    }
}

fn stop_chunk(reason: TurnStopReason, message: String, provider: Option<&str>) -> StreamChunk {
    if crate::retry::is_limit_message(&message) {
        StreamChunk::Limit(LimitError {
            provider: provider.unwrap_or("jcode").to_string(),
            detail: message,
        })
    } else {
        let detail = if message.trim().is_empty() {
            format!("jcode turn stopped: {reason:?}")
        } else {
            format!("jcode turn stopped ({reason:?}): {message}")
        };
        StreamChunk::Error(detail)
    }
}

fn error_chunk(message: String, provider: Option<&str>) -> StreamChunk {
    if crate::retry::is_limit_message(&message) {
        StreamChunk::Limit(LimitError {
            provider: provider.unwrap_or("jcode").to_string(),
            detail: message,
        })
    } else {
        StreamChunk::Error(message)
    }
}

#[derive(Default)]
struct TextCollector {
    parts: Vec<(Option<String>, String, bool)>,
}

impl TextCollector {
    fn index(&self, id: Option<&String>) -> Option<usize> {
        self.parts
            .iter()
            .rposition(|(message_id, _, done)| message_id.as_ref() == id && (id.is_some() || !done))
    }

    fn message(&mut self, id: Option<String>) -> &mut (Option<String>, String, bool) {
        let index = self.index(id.as_ref()).unwrap_or_else(|| {
            self.parts.push((id, String::new(), false));
            self.parts.len() - 1
        });
        &mut self.parts[index]
    }

    fn append(&mut self, id: Option<String>, delta: &str) {
        self.message(id).1.push_str(delta);
    }

    fn replace(&mut self, id: Option<String>, text: String) {
        self.message(id).1 = text;
    }

    fn finish_message(&mut self, id: Option<String>) {
        self.message(id).2 = true;
    }

    fn final_text(&self) -> String {
        self.parts
            .iter()
            .rev()
            .find(|(_, text, done)| *done && !text.is_empty())
            .map_or_else(
                || {
                    self.parts
                        .iter()
                        .map(|(_, text, _)| text.as_str())
                        .collect()
                },
                |(_, text, _)| text.clone(),
            )
    }
}

fn model_switch_request(provider: Option<&str>, model: &str, routes: &[ModelRouteInfo]) -> String {
    let Some(provider) = provider else {
        return model.to_string();
    };
    let legacy_route = routed_model_arg(Some(provider), model);
    if legacy_route != model {
        return legacy_route.into_owned();
    }
    let route = routes
        .iter()
        .find(|route| route.model == model && route_matches_provider(route, provider));
    let prefix = route.and_then(route_prefix).unwrap_or(provider);
    format!("{prefix}:{model}")
}

fn route_matches_provider(route: &ModelRouteInfo, requested: &str) -> bool {
    let requested = normalized_provider(requested);
    let profile = route.api_method.strip_prefix("openai-compatible:");
    [
        Some(route.provider.as_str()),
        Some(route.api_method.as_str()),
        profile,
    ]
    .into_iter()
    .flatten()
    .any(|candidate| normalized_provider(candidate) == requested)
}

fn normalized_provider(provider: &str) -> String {
    provider
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn route_prefix(route: &ModelRouteInfo) -> Option<&str> {
    let api_method = route.api_method.as_str();
    if let Some(profile) = api_method.strip_prefix("openai-compatible:") {
        return (!profile.is_empty()).then_some(profile);
    }
    Some(match api_method {
        "anthropic-api-key" | "claude-api" => "claude-api",
        "claude-oauth" => "claude-oauth",
        "openai-api-key" | "openai-api" => "openai-api",
        "openai-oauth" => "openai-oauth",
        "code-assist-oauth" => "gemini",
        "antigravity-https" => "antigravity",
        "https" if normalized_provider(&route.provider) == "antigravity" => "antigravity",
        "openrouter" | "copilot" | "cursor" | "bedrock" | "antigravity" | "gemini" => api_method,
        _ if !api_method.is_empty() && !api_method.contains(':') => api_method,
        _ => return None,
    })
}

/// Retains the legacy CLI's explicit provider-prefix semantics. Those prefixes
/// are the harness's model route ids; the provider and model are no longer
/// separate SDK arguments.
fn routed_model_arg<'a>(provider: Option<&str>, model: &'a str) -> Cow<'a, str> {
    let prefix = match provider {
        Some("anthropic-api") => "claude-api",
        Some("claude-subprocess") => "claude",
        Some(
            provider @ ("claude" | "openai" | "openai-api" | "copilot" | "openrouter" | "bedrock"),
        ) => provider,
        _ => return Cow::Borrowed(model),
    };
    Cow::Owned(format!("{prefix}:{model}"))
}

struct SessionHome {
    source_home: PathBuf,
    storage_root: PathBuf,
    storage_home: PathBuf,
    expected_home: Option<PathBuf>,
    sdk_home: PathBuf,
    temporary_alias: Option<PathBuf>,
    stable_alias_root: Option<PathBuf>,
    fresh: bool,
    resume_session_found: bool,
    active_lock: Option<std::fs::File>,
}

impl Drop for SessionHome {
    fn drop(&mut self) {
        // A fork can retain the open file description until exec; explicitly unlock when its owner ends.
        if let Some(file) = self.active_lock.as_ref() {
            let _ = fs2::FileExt::unlock(file);
        }
    }
}

fn prepare_session_home(
    source_home: PathBuf,
    session_key: &str,
    resume_session_id: Option<&str>,
    mcp_servers: &crate::config::McpServers,
) -> std::result::Result<SessionHome, String> {
    let storage_root = source_home.join(SESSION_HOME_DIR);
    ensure_private_dir(&storage_root).map_err(|error| error.to_string())?;
    let root_lock = lock_session_root(&storage_root).map_err(|error| error.to_string())?;
    prune_unclaimed_session_homes_locked(&storage_root, SystemTime::now())
        .map_err(|error| error.to_string())?;
    let expected_home = resume_session_id.map(|_| storage_root.join(session_key));
    let existing_home = match (resume_session_id, expected_home.as_ref()) {
        (Some(id), Some(expected)) => find_session_home(&storage_root, expected, id),
        _ => None,
    };
    let resume_session_found = existing_home.is_some();
    let storage_home = existing_home.unwrap_or_else(|| {
        expected_home
            .clone()
            .unwrap_or_else(|| storage_root.join(session_key))
    });
    let fresh = !storage_home.exists();
    ensure_private_dir(&storage_home).map_err(|error| error.to_string())?;
    let active_lock = lock_session_home(&storage_home).map_err(|error| error.to_string())?;
    drop(root_lock);
    let linked_credentials = jcode_sdk::inherit_credentials(&source_home, &storage_home)
        .map_err(|error| error.to_string())?;
    copy_private_model_catalog_caches(&source_home, &storage_home)
        .map_err(|error| error.to_string())?;
    let copied_external_credentials =
        materialize_external_credentials(&storage_home, &linked_credentials)
            .map_err(|error| error.to_string())?;
    copy_external_auth_trust(&storage_home, &copied_external_credentials)
        .map_err(|error| error.to_string())?;
    write_session_mcp(&source_home, &storage_home, mcp_servers)
        .map_err(|error| error.to_string())?;

    #[cfg(unix)]
    let (sdk_home, temporary_alias, stable_alias_root) = {
        let alias_root = short_home_alias_root();
        ensure_private_alias_root(&alias_root).map_err(|error| error.to_string())?;
        let alias_key = resume_session_id.map_or_else(
            || session_key[..16].to_string(),
            |id| stable_alias_key(&source_home, id),
        );
        let alias = alias_root.join(alias_key);
        ensure_home_alias(&alias, &storage_home).map_err(|error| error.to_string())?;
        (alias.clone(), Some(alias), Some(alias_root))
    };
    #[cfg(not(unix))]
    let (sdk_home, temporary_alias, stable_alias_root) = { (storage_home.clone(), None, None) };

    Ok(SessionHome {
        source_home,
        storage_root,
        storage_home,
        expected_home,
        sdk_home,
        temporary_alias,
        stable_alias_root,
        fresh,
        resume_session_found,
        active_lock,
    })
}
fn copy_private_model_catalog_caches(
    source_home: &Path,
    storage_home: &Path,
) -> std::io::Result<()> {
    let source_dir = if std::env::var_os(JCODE_HOME_ENV).is_some() {
        source_home.join("config/jcode")
    } else {
        jcode_sdk::user_app_config_dir()
    };
    let destination_dir = storage_home.join("config/jcode");
    ensure_private_dir(&destination_dir)?;
    for name in MODEL_CATALOG_CACHE_FILES {
        let source = source_dir.join(name);
        if !source.is_file() {
            continue;
        }
        let destination = destination_dir.join(name);
        let _ = fs::remove_file(&destination);
        fs::copy(&source, &destination)?;
        set_private_file(&destination)?;
    }
    Ok(())
}

fn materialize_external_credentials(
    storage_home: &Path,
    linked_credentials: &[PathBuf],
) -> std::io::Result<Vec<MaterializedExternalCredential>> {
    let mut copied = Vec::new();
    for linked_path in linked_credentials {
        if !linked_path.starts_with("external") {
            continue;
        }
        copied.push(materialize_external_credential(
            &storage_home.join(linked_path),
        )?);
    }
    Ok(copied)
}

fn materialize_external_credential(
    link_path: &Path,
) -> std::io::Result<MaterializedExternalCredential> {
    let original = fs::canonicalize(link_path)?;
    let _ = fs::remove_file(link_path);
    fs::copy(&original, link_path)?;
    set_private_file(link_path)?;
    let copy = fs::canonicalize(link_path)?;
    Ok(MaterializedExternalCredential {
        original_canonical_path: original.to_string_lossy().to_ascii_lowercase(),
        copy_canonical_path: copy.to_string_lossy().to_ascii_lowercase(),
    })
}

fn copy_external_auth_trust(
    storage_home: &Path,
    copied: &[MaterializedExternalCredential],
) -> std::io::Result<()> {
    if copied.is_empty() {
        return Ok(());
    }
    let config_path = storage_home.join(JCODE_CONFIG_FILE);
    let config = match fs::read_to_string(&config_path) {
        Ok(config) => config,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let Ok(mut instance_config) = toml::from_str::<toml::Table>(&config) else {
        return Ok(());
    };
    // jcode ignores path trust when the inherited config is unparsable.
    let trusted_source_paths = instance_config
        .get("auth")
        .and_then(toml::Value::as_table)
        .and_then(|auth| auth.get("trusted_external_source_paths"))
        .and_then(toml::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        });
    let Some(trusted_source_paths) = trusted_source_paths else {
        return Ok(());
    };
    let auth = instance_config
        .entry("auth")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let Some(auth) = auth.as_table_mut() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "jcode config auth setting must be a table",
        ));
    };
    let trusted_instance_paths = auth
        .entry("trusted_external_source_paths")
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let Some(trusted_instance_paths) = trusted_instance_paths.as_array_mut() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "jcode trusted_external_source_paths setting must be an array",
        ));
    };
    let mut changed = false;
    for credential in copied {
        for source_entry in &trusted_source_paths {
            let Some((source_id, source_path)) = source_entry.trim().split_once('|') else {
                continue;
            };
            if source_id.trim().is_empty()
                || !source_path
                    .trim()
                    .eq_ignore_ascii_case(&credential.original_canonical_path)
            {
                continue;
            }
            let instance_entry = format!("{}|{}", source_id.trim(), credential.copy_canonical_path);
            if !trusted_instance_paths.iter().any(|entry| {
                entry
                    .as_str()
                    .is_some_and(|entry| entry.trim().eq_ignore_ascii_case(&instance_entry))
            }) {
                trusted_instance_paths.push(toml::Value::String(instance_entry));
                changed = true;
            }
        }
    }
    if changed {
        let serialized = toml::to_string(&instance_config)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        fs::write(&config_path, serialized)?;
        set_private_file(&config_path)?;
    }
    Ok(())
}

fn finish_session_home(
    home: &mut SessionHome,
    session_id: Option<&str>,
    keep: bool,
) -> std::io::Result<()> {
    if !keep {
        remove_home_aliases_for_target(&home.storage_home)?;
        if home.storage_home.exists() {
            fs::remove_dir_all(&home.storage_home)?;
        }
        return Ok(());
    }
    if let Some(session_id) = session_id {
        write_session_id(home, session_id)?;
        let expected = home
            .expected_home
            .clone()
            .unwrap_or_else(|| home.storage_root.join(session_storage_key(session_id)));
        if home.storage_home != expected && !expected.exists() {
            fs::rename(&home.storage_home, &expected)?;
            home.storage_home.clone_from(&expected);
        }
        #[cfg(unix)]
        if let Some(alias_root) = &home.stable_alias_root {
            let stable_alias = alias_root.join(stable_alias_key(&home.source_home, session_id));
            ensure_home_alias(&stable_alias, &home.storage_home)?;
            if home.temporary_alias.as_ref() != Some(&stable_alias)
                && let Some(alias) = &home.temporary_alias
            {
                remove_home_alias(alias)?;
            }
        }
    } else if home.fresh {
        remove_home_aliases_for_target(&home.storage_home)?;
        fs::remove_dir_all(&home.storage_home)?;
    }
    Ok(())
}

pub(crate) fn cleanup_session_home_at(source_home: &Path, session_id: &str) -> std::io::Result<()> {
    let root = source_home.join(SESSION_HOME_DIR);
    let expected = root.join(session_storage_key(session_id));
    let Some(home) = find_session_home(&root, &expected, session_id) else {
        return Ok(());
    };
    let _active_lock = lock_session_home(&home)?;
    stop_private_daemon(&home)?;
    remove_home_aliases_for_target(&home)?;
    fs::remove_dir_all(home)
}

#[cfg(all(test, any(unix, windows)))]
fn prune_unclaimed_session_homes_at(root: &Path, now: SystemTime) -> std::io::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let _root_lock = lock_session_root(root)?;
    prune_unclaimed_session_homes_locked(root, now)
}

fn prune_unclaimed_session_homes_locked(root: &Path, now: SystemTime) -> std::io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries.flatten() {
        let home = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&home) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        if home.join(SESSION_ID_FILE).is_file() {
            continue;
        }
        let stale = metadata.modified().is_ok_and(|modified| {
            now.duration_since(modified).unwrap_or_default() >= STALE_SESSION_HOME_AGE
        });
        if stale && let Some(_home_lock) = lock_existing_session_home(&home)? {
            stop_private_daemon(&home)?;
            remove_home_aliases_for_target(&home)?;
            fs::remove_dir_all(home)?;
        }
    }
    Ok(())
}

fn lock_session_root(root: &Path) -> std::io::Result<Option<File>> {
    let path = root.join(ROOT_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    set_private_file(&path)?;
    fs2::FileExt::lock_exclusive(&file)?;
    Ok(Some(file))
}

fn lock_session_home(home: &Path) -> std::io::Result<Option<File>> {
    let path = home.join(HOME_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    set_private_file(&path)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error) if is_lock_contended(&error) => Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "jcode session home is already active",
        )),
        Err(error) => Err(error),
    }
}

fn lock_existing_session_home(home: &Path) -> std::io::Result<Option<File>> {
    let path = home.join(HOME_LOCK_FILE);
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error) if is_lock_contended(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_lock_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn find_session_home(root: &Path, expected: &Path, session_id: &str) -> Option<PathBuf> {
    let matches_session = |path: &Path| {
        fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            && fs::read_to_string(path.join(SESSION_ID_FILE))
                .is_ok_and(|id| id.trim() == session_id)
    };
    if matches_session(expected) {
        return Some(expected.to_path_buf());
    }
    let entries = fs::read_dir(root).ok()?;
    entries
        .filter_map(std::result::Result::ok)
        .find_map(|entry| {
            let path = entry.path();
            matches_session(&path).then_some(path)
        })
}

#[cfg(unix)]
fn home_aliases_for_target(home: &Path) -> std::io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(short_home_alias_root()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(entries
        .flatten()
        .filter_map(|entry| {
            let alias = entry.path();
            (fs::symlink_metadata(&alias).is_ok_and(|metadata| metadata.file_type().is_symlink())
                && fs::read_link(&alias).is_ok_and(|target| target == home))
            .then_some(alias)
        })
        .collect())
}

fn remove_home_aliases_for_target(home: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    for alias in home_aliases_for_target(home)? {
        fs::remove_file(alias)?;
    }
    #[cfg(not(unix))]
    let _ = home;
    Ok(())
}

#[cfg(unix)]
fn stop_private_daemon(home: &Path) -> std::io::Result<()> {
    stop_private_daemons(&private_daemon_sockets(home)?)
}

#[cfg(unix)]
fn private_daemon_sockets(home: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut sockets = Vec::new();
    let physical_socket = home.join("run/jcode.sock");
    sockets.push(physical_socket);
    if let Ok(runtime_dir) = fs::canonicalize(home.join("run")) {
        let raw = match fs::read(home.join("servers.json")) {
            Ok(raw) => Some(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(raw) = raw
            && let Ok(registry) = serde_json::from_slice::<serde_json::Value>(&raw)
            && let Some(entries) = registry.as_object()
        {
            for entry in entries.values() {
                let Some(socket) = entry
                    .get("socket")
                    .and_then(serde_json::Value::as_str)
                    .map(PathBuf::from)
                else {
                    continue;
                };
                if socket.file_name().and_then(|name| name.to_str()) == Some("jcode.sock")
                    && socket.parent().is_some_and(|parent| {
                        fs::canonicalize(parent).is_ok_and(|p| p == runtime_dir)
                    })
                    && !sockets.contains(&socket)
                {
                    sockets.push(socket);
                }
            }
        }
    }
    let aliases = home_aliases_for_target(home)?;
    for alias in aliases {
        let socket = alias.join("run/jcode.sock");
        if !sockets.contains(&socket) {
            sockets.push(socket);
        }
    }
    Ok(sockets)
}

#[cfg(unix)]
fn stop_private_daemons(sockets: &[PathBuf]) -> std::io::Result<()> {
    for socket in sockets {
        for pid in private_daemon_pids(socket)? {
            signal_private_process(pid, socket, libc::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline && private_daemon_matches(pid, socket) {
                std::thread::sleep(Duration::from_millis(50));
            }
            if private_daemon_matches(pid, socket) {
                signal_private_process(pid, socket, libc::SIGKILL);
                let deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() < deadline && private_daemon_matches(pid, socket) {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn private_daemon_pids(socket: &Path) -> std::io::Result<Vec<i32>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other("could not inspect jcode processes"));
    }
    let mut pids = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|pid| pid.parse::<i32>().ok()) else {
            continue;
        };
        let command = fields.collect::<Vec<_>>().join(" ");
        if jcode_server_command(&command) && private_daemon_matches(pid, socket) {
            pids.push(pid);
        }
    }
    Ok(pids)
}

#[cfg(windows)]
fn stop_private_daemon(home: &Path) -> std::io::Result<()> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut sockets = vec![home.join("run").join("jcode.sock")];
    if let Ok(raw) = fs::read(home.join("servers.json"))
        && let Ok(registry) = serde_json::from_slice::<serde_json::Value>(&raw)
        && let Some(entries) = registry.as_object()
    {
        for entry in entries.values() {
            if let Some(socket) = entry.get("socket").and_then(serde_json::Value::as_str) {
                sockets.push(PathBuf::from(socket));
            }
        }
    }
    let identifiers: Vec<String> = std::iter::once(home.to_string_lossy().into_owned())
        .chain(sockets.iter().map(|s| s.to_string_lossy().into_owned()))
        .collect();

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    // Identity is the executable plus `serve` plus this private home, runtime
    // or socket. A PID from `servers.json` alone is never trusted.
    let matched: Vec<sysinfo::Pid> = system
        .processes()
        .iter()
        .filter(|(_, process)| {
            let is_jcode = process
                .exe()
                .and_then(Path::file_stem)
                .or_else(|| {
                    process
                        .cmd()
                        .first()
                        .map(Path::new)
                        .and_then(Path::file_stem)
                })
                .is_some_and(|stem| stem.eq_ignore_ascii_case("jcode"));
            let serves = process.cmd().iter().any(|arg| arg == "serve");
            let private = process
                .cmd()
                .iter()
                .chain(process.environ())
                .map(|value| value.to_string_lossy())
                .any(|value| identifiers.iter().any(|id| value.contains(id.as_str())));
            is_jcode && serves && private
        })
        .map(|(pid, _)| *pid)
        .collect();
    for root in matched {
        // Stop the whole tree: descendants first, then the daemon.
        let mut tree = vec![root];
        let mut index = 0;
        while index < tree.len() {
            let parent = tree[index];
            for (pid, process) in system.processes() {
                if process.parent() == Some(parent) && !tree.contains(pid) {
                    tree.push(*pid);
                }
            }
            index += 1;
        }
        for pid in tree.into_iter().rev() {
            if let Some(process) = system.process(pid) {
                let _ = process.kill_and_wait();
            }
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn stop_private_daemon(_home: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Stop only daemons whose command and private runtime identify the Cruise home.
fn clear_private_daemon(home: &Path) -> std::io::Result<()> {
    let stopped = stop_private_daemon(home);
    let cleared = match fs::remove_file(home.join("servers.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    stopped.and(cleared)
}

#[cfg(unix)]
fn signal_private_process(pid: i32, socket: &Path, signal: i32) {
    if !private_daemon_matches(pid, socket) {
        return;
    }
    // SAFETY: the process was identified as the jcode daemon serving this
    // private socket immediately before signaling.
    unsafe {
        libc::kill(pid, signal);
    }
}

/// `servers.json` can retain a recycled PID, so verify the running process is
/// jcode serving this private socket before signaling it.
#[cfg(unix)]
fn private_daemon_matches(pid: i32, socket: &Path) -> bool {
    let pid = pid.to_string();
    let command = Command::new("ps")
        .args(["-p", &pid, "-o", "command="])
        .output();
    let environment = Command::new("ps").args(["eww", "-p", &pid]).output();
    let (Ok(command), Ok(environment)) = (command, environment) else {
        return false;
    };
    if !command.status.success() || !environment.status.success() {
        return false;
    }
    private_daemon_identity_matches(
        &String::from_utf8_lossy(&command.stdout),
        &String::from_utf8_lossy(&environment.stdout),
        socket,
    )
}

#[cfg(unix)]
fn private_daemon_identity_matches(command: &str, environment: &str, socket: &Path) -> bool {
    if !jcode_server_command(command) {
        return false;
    }
    let socket_arg =
        command.contains("--socket") && socket.to_str().is_some_and(|path| command.contains(path));
    let Some(runtime_dir) = socket.parent() else {
        return false;
    };
    let Some(home) = runtime_dir.parent() else {
        return false;
    };
    socket_arg
        || (environment_has_path(environment, "JCODE_HOME", home)
            && environment_has_path(environment, "JCODE_RUNTIME_DIR", runtime_dir)
            && environment_has_path(environment, "JCODE_SOCKET", socket))
}

#[cfg(unix)]
fn jcode_server_command(command: &str) -> bool {
    let mut arguments = command.split_whitespace();
    arguments.next().is_some_and(|executable| {
        Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            == Some("jcode")
    }) && arguments.any(|argument| argument == "serve")
}

#[cfg(unix)]
fn environment_has_path(environment: &str, key: &str, value: &Path) -> bool {
    let assignment = format!("{key}={}", value.display());
    environment.match_indices(&assignment).any(|(start, _)| {
        let before_matches = start == 0 || environment.as_bytes()[start - 1].is_ascii_whitespace();
        let end = start + assignment.len();
        let after_matches = environment
            .as_bytes()
            .get(end)
            .is_none_or(u8::is_ascii_whitespace);
        before_matches && after_matches
    })
}

fn write_session_id(home: &SessionHome, session_id: &str) -> std::io::Result<()> {
    let path = home.storage_home.join(SESSION_ID_FILE);
    fs::write(&path, session_id)?;
    set_private_file(&path)
}

fn write_session_mcp(
    source_home: &Path,
    storage_home: &Path,
    workflow: &crate::config::McpServers,
) -> std::io::Result<()> {
    let source = source_home.join("mcp.json");
    let destination = storage_home.join("mcp.json");
    if workflow.is_empty() {
        let mut contents = match fs::read(&source) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::symlink_metadata(&destination) {
                    Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                        fs::remove_file(&destination)?;
                    }
                    Ok(_) => {
                        return Err(std::io::Error::other(
                            "jcode mcp.json destination is not a file",
                        ));
                    }
                    Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(missing) => return Err(missing),
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&contents)
            && let Some(servers) = value
                .get_mut("mcpServers")
                .and_then(serde_json::Value::as_object_mut)
            && servers.remove(LEGACY_CRUISE_MCP_NAME).is_some()
        {
            contents = serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?;
        }
        fs::write(&destination, contents)?;
        return set_private_file(&destination);
    }

    let contents = match fs::read(&source) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut value = match contents {
        Some(contents) => {
            serde_json::from_slice::<serde_json::Value>(&contents).map_err(|error| {
                std::io::Error::other(format!(
                    "cannot merge workflow mcp_servers into {}: {error}",
                    source.display()
                ))
            })?
        }
        None => serde_json::json!({}),
    };
    let Some(root) = value.as_object_mut() else {
        return Err(std::io::Error::other(format!(
            "cannot merge workflow mcp_servers into {}: source must be a JSON object",
            source.display()
        )));
    };
    let server_key = if root.contains_key("mcpServers") {
        "mcpServers"
    } else if root.contains_key("servers") {
        "servers"
    } else {
        "mcpServers"
    };
    if server_key == "mcpServers" {
        root.remove("servers");
    } else {
        root.remove("mcpServers");
    }
    let Some(servers) = root
        .entry(server_key.to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    else {
        return Err(std::io::Error::other(format!(
            "cannot merge workflow mcp_servers into {}: `{server_key}` must be a JSON object",
            source.display()
        )));
    };
    servers.remove(LEGACY_CRUISE_MCP_NAME);
    for (name, server) in workflow {
        servers.insert(name.clone(), server.to_backend_json());
    }
    let contents = serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?;
    fs::write(&destination, contents)?;
    set_private_file(&destination)
}

pub(crate) fn resolve_source_home(cwd: Option<&Path>) -> PathBuf {
    let configured = std::env::var_os(JCODE_HOME_ENV).filter(|value| !value.is_empty());
    let path = configured.map_or_else(
        || {
            home::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".jcode")
        },
        PathBuf::from,
    );
    if path.is_absolute() {
        return path;
    }
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let base = cwd.map_or_else(
        || current_dir.clone(),
        |cwd| {
            if cwd.is_absolute() {
                cwd.to_path_buf()
            } else {
                current_dir.join(cwd)
            }
        },
    );
    base.join(path)
}

fn session_storage_key(session_id: &str) -> String {
    sha256_hex(session_id.as_bytes())
}

fn stable_alias_key(source_home: &Path, session_id: &str) -> String {
    let mut key = source_home.as_os_str().as_encoded_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(session_id.as_bytes());
    sha256_hex(&key)[..16].to_string()
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn session_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: LazyLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut locks = LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

#[cfg(unix)]
fn short_home_alias_root() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/cruise-jcode-{uid}"))
}

#[cfg(not(unix))]
fn short_home_alias_root() -> PathBuf {
    std::env::temp_dir().join("cruise-jcode")
}

#[cfg(unix)]
fn ensure_private_alias_root(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    ensure_private_dir(path)?;
    let uid = unsafe { libc::getuid() };
    if fs::symlink_metadata(path)?.uid() != uid {
        return Err(std::io::Error::other(format!(
            "jcode home alias root must be owned by uid {uid}: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_sdk_socket_paths(sdk_home: &Path) -> std::result::Result<(), String> {
    let path = sdk_home.join("run/jcode-debug.sock");
    std::os::unix::net::SocketAddr::from_pathname(&path)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "jcode SDK socket path is not supported: {}: {error}",
                path.display()
            )
        })
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::other(format!(
            "jcode session path must be a real directory: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(path)?;
    Ok(())
}

#[cfg(unix)]
fn ensure_home_alias(alias: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::symlink;
    match fs::symlink_metadata(alias) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(alias)? == target {
                return Ok(());
            }
            fs::remove_file(alias)?;
        }
        Ok(_) => {
            return Err(std::io::Error::other(
                "jcode session alias is not a symlink",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    symlink(target, alias)
}

#[cfg(unix)]
fn remove_home_alias(alias: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(alias) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::remove_file(alias),
        Ok(_) => Err(std::io::Error::other(
            "jcode session alias is not a symlink",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
fn remove_home_alias(_alias: &Path) -> std::io::Result<()> {
    Ok(())
}

fn set_private_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(path)?;
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    const LOCK_PROBE_ENV: &str = "CRUISE_ROOT_LOCK_PROBE";

    /// Child half of the cross-process lock test. A no-op unless spawned by it.
    #[test]
    fn windows_root_lock_child_probe() {
        let Ok(spec) = std::env::var(LOCK_PROBE_ENV) else {
            return;
        };
        let (mode, dir) = spec
            .split_once('|')
            .unwrap_or_else(|| panic!("bad probe spec"));
        let file =
            File::open(Path::new(dir).join(ROOT_LOCK_FILE)).unwrap_or_else(|e| panic!("{e}"));
        let locked = fs2::FileExt::try_lock_exclusive(&file).is_ok();
        println!("PROBE_RAN:{mode}:{locked}");
        std::process::exit(i32::from(locked != (mode == "free")));
    }

    fn run_lock_probe(mode: &str, dir: &Path) -> bool {
        let output =
            std::process::Command::new(std::env::current_exe().unwrap_or_else(|e| panic!("{e}")))
                .args([
                    "--exact",
                    "backend::jcode::windows_tests::windows_root_lock_child_probe",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(LOCK_PROBE_ENV, format!("{mode}|{}", dir.display()))
                .output()
                .unwrap_or_else(|e| panic!("{e}"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&format!("PROBE_RAN:{mode}:")),
            "probe did not run: {stdout}"
        );
        output.status.success()
    }

    #[test]
    fn windows_root_lock_excludes_second_process() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let first = lock_session_root(dir.path())
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("expected lock"));
        assert!(
            run_lock_probe("blocked", dir.path()),
            "child must not acquire a held lock"
        );
        drop(first);
        assert!(
            run_lock_probe("free", dir.path()),
            "child must acquire a released lock"
        );
    }

    #[test]
    fn windows_runtime_directories_have_owner_only_dacl() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let nested = root.path().join("a").join("b");
        ensure_private_dir(&nested).unwrap_or_else(|e| panic!("{e}"));
        crate::platform::assert_owner_only_dacl(&nested);
    }

    #[test]
    fn windows_private_files_have_owner_only_dacl() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = root.path().join("private.txt");
        fs::write(&file, b"x").unwrap_or_else(|e| panic!("{e}"));
        set_private_file(&file).unwrap_or_else(|e| panic!("{e}"));
        crate::platform::assert_owner_only_dacl(&file);
        let _held = lock_session_home(root.path()).unwrap_or_else(|e| panic!("{e}"));
        crate::platform::assert_owner_only_dacl(&root.path().join(HOME_LOCK_FILE));
    }

    #[test]
    fn windows_home_lock_is_released_when_session_home_drops() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let home = root.path().to_path_buf();
        let lock = lock_session_home(&home).unwrap_or_else(|e| panic!("{e}"));
        let session_home = SessionHome {
            source_home: home.clone(),
            storage_root: home.clone(),
            storage_home: home.clone(),
            expected_home: None,
            sdk_home: home.clone(),
            temporary_alias: None,
            stable_alias_root: None,
            fresh: true,
            resume_session_found: false,
            active_lock: lock,
        };
        assert!(lock_session_home(&home).is_err());
        drop(session_home);
        assert!(lock_session_home(&home).is_ok());
    }

    #[test]
    fn windows_active_session_home_lock_returns_would_block() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let _held = lock_session_home(dir.path()).unwrap_or_else(|e| panic!("{e}"));
        let error = lock_session_home(dir.path()).expect_err("second lock must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn windows_stale_prune_skips_locked_home() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let home = root.path().join("home");
        fs::create_dir(&home).unwrap_or_else(|e| panic!("{e}"));
        let held = lock_session_home(&home).unwrap_or_else(|e| panic!("{e}"));
        let later = SystemTime::now() + STALE_SESSION_HOME_AGE * 2;
        prune_unclaimed_session_homes_at(root.path(), later).unwrap_or_else(|e| panic!("{e}"));
        assert!(home.exists(), "locked home must not be pruned");
        drop(held);
        prune_unclaimed_session_homes_at(root.path(), later).unwrap_or_else(|e| panic!("{e}"));
        assert!(!home.exists(), "unlocked stale home must be pruned");
    }

    #[test]
    fn windows_stale_home_cleanup_ignores_unrelated_registry_pid() {
        let home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let pid = std::process::id();
        fs::write(
            home.path().join("servers.json"),
            format!(
                "{{\"x\":{{\"pid\":{pid},\"socket\":\"{}\"}}}}",
                "C:/nope/jcode.sock"
            ),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        clear_private_daemon(home.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(!home.path().join("servers.json").exists());
    }
}
