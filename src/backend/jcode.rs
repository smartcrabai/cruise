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
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
const OPENAI_SERVICE_TIER_ENV: &str = "JCODE_OPENAI_SERVICE_TIER";
const JCODE_PROVIDER_ENV: &str = "JCODE_PROVIDER";
const JCODE_MODEL_ENV: &str = "JCODE_MODEL";
const OPENAI_SERVICE_TIER_DEFAULT: &str = "off";
const SESSION_HOME_DIR: &str = ".cruise-sdk-sessions";
const SESSION_ID_FILE: &str = ".cruise-session-id";
const LEGACY_CRUISE_MCP_NAME: &str = "cruise";
const MIN_JCODE_VERSION: (u64, u64, u64) = (0, 88, 0);
const CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

const STALE_SESSION_HOME_AGE: Duration = Duration::from_hours(24);
/// One `sdk: jcode` prompt attempt. Deliberately not `Debug`: `env` may contain
/// provider credentials.
#[derive(Default)]
pub(crate) struct JcodeRunnerConfig {
    pub(crate) model: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) effort: Option<EffortLevel>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) resume_session_id: Option<String>,
    pub(crate) tools: Vec<CruiseTool>,
    pub(crate) env: HashMap<String, String>,
    pub(crate) cancel: Option<CancellationToken>,
    pub(crate) keep_session_home: bool,
}

/// Check the binary floor. The SDK handshake only reports the API bridge crate
/// version (`jcode-harness-api-bridge/...`), not the jcode binary version, so
/// this one CLI probe has no SDK equivalent.
pub(crate) fn check_runtime_version() -> Result<()> {
    let output = Command::new("jcode")
        .args(["--no-update", "version", "--json"])
        .env(NO_TELEMETRY_ENV, "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            CruiseError::Other(format!(
                "`sdk: jcode` requires jcode {} or newer, but `jcode version --json` failed: {error}",
                format_version(MIN_JCODE_VERSION)
            ))
        })?;
    let version = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value.get("semver")?.as_str().map(str::to_string))
        .ok_or_else(|| {
            CruiseError::Other(format!(
                "could not read jcode's version from `jcode version --json`; `sdk: jcode` requires jcode {} or newer",
                format_version(MIN_JCODE_VERSION)
            ))
        })?;
    let parsed = parse_version(&version).ok_or_else(|| {
        CruiseError::Other(format!(
            "jcode version --json reported an unparseable version '{version}'; `sdk: jcode` requires jcode {} or newer",
            format_version(MIN_JCODE_VERSION)
        ))
    })?;
    if parsed < MIN_JCODE_VERSION {
        return Err(CruiseError::Other(format!(
            "jcode {version} is too old for `sdk: jcode`, which requires {} or newer; upgrade jcode and retry",
            format_version(MIN_JCODE_VERSION)
        )));
    }
    Ok(())
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text
        .trim()
        .trim_start_matches('v')
        .split(['-', '+', ' '])
        .next()?;
    let mut parts = core.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
    ))
}

fn format_version((major, minor, patch): (u64, u64, u64)) -> String {
    format!("{major}.{minor}.{patch}")
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
    )?;
    let mut session_id = None;
    let run_result = match launch_client(config, &home) {
        Ok(client) => {
            let result = run_client_turn(&client, config, prompt, tx, &mut home, &mut session_id);
            drop(client);
            result
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
    let mut options = LaunchOptions {
        jcode_home: Some(home.sdk_home.clone()),
        working_dir: config.cwd.clone(),
        inherit_logins: false,
        ..Default::default()
    };
    options.env = launch_environment(
        &config.env,
        config.provider.as_deref(),
        config.model.as_deref(),
    );
    JcodeClient::launch(options).map_err(|error| error.to_string())
}

fn launch_environment(
    workflow_env: &HashMap<String, String>,
    provider: Option<&str>,
    model: Option<&str>,
) -> HashMap<OsString, OsString> {
    let mut env: HashMap<OsString, OsString> = workflow_env
        .iter()
        .filter(|(key, _)| key.as_str() != JCODE_HOME_ENV)
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect();
    env.insert(OsString::from(NO_TELEMETRY_ENV), OsString::from("1"));
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
    env
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
    if !config.tools.is_empty() && !client.supports("session_tools") {
        return Err(
            "the jcode harness does not support session tools; upgrade jcode to 0.88.0 or newer"
                .to_string(),
        );
    }
    let session = match config.resume_session_id.as_deref() {
        Some(id) => client.attach_session(id),
        None => client.create_session(
            config
                .cwd
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned()),
        ),
    }
    .map_err(|error| error.to_string())?;
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
                    enabled: None,
                    disabled: Vec::new(),
                    custom,
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
                let _ = tx.send(StreamChunk::Delta(delta));
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
                if let Some((reason, stop_message)) = stopped
                    && (reason == TurnStopReason::LimitReached
                        || crate::retry::is_limit_message(&stop_message))
                {
                    let detail = if message.trim().is_empty() {
                        stop_message
                    } else {
                        message
                    };
                    return Ok(StreamChunk::Limit(LimitError {
                        provider: config.provider.as_deref().unwrap_or("jcode").to_string(),
                        detail,
                    }));
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
    if reason == TurnStopReason::LimitReached || crate::retry::is_limit_message(&message) {
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
}

fn prepare_session_home(
    source_home: PathBuf,
    session_key: &str,
    resume_session_id: Option<&str>,
) -> std::result::Result<SessionHome, String> {
    let storage_root = source_home.join(SESSION_HOME_DIR);
    ensure_private_dir(&storage_root).map_err(|error| error.to_string())?;
    prune_unclaimed_session_homes(&storage_root).map_err(|error| error.to_string())?;
    let expected_home = resume_session_id.map(|_| storage_root.join(session_key));
    let storage_home = match (resume_session_id, expected_home.as_ref()) {
        (Some(id), Some(expected)) => {
            find_session_home(&storage_root, expected, id).unwrap_or_else(|| expected.clone())
        }
        _ => storage_root.join(session_key),
    };
    let fresh = !storage_home.exists();
    ensure_private_dir(&storage_home).map_err(|error| error.to_string())?;
    jcode_sdk::inherit_credentials(&source_home, &storage_home)
        .map_err(|error| error.to_string())?;
    disable_private_updates(&storage_home).map_err(|error| error.to_string())?;
    copy_global_mcp(&source_home, &storage_home).map_err(|error| error.to_string())?;

    #[cfg(unix)]
    let (sdk_home, temporary_alias, stable_alias_root) = {
        let alias_root = short_home_alias_root();
        ensure_private_dir(&alias_root).map_err(|error| error.to_string())?;
        let alias_key = resume_session_id.map_or_else(
            || session_key.to_string(),
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
    })
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

/// Remove a persistent SDK home when its owning Cruise session is deleted.
pub(crate) fn cleanup_session_home(session_id: &str) -> Result<()> {
    let source_home = resolve_source_home(None);
    cleanup_session_home_at(&source_home, session_id)
        .map_err(|error| CruiseError::Other(format!("could not clean jcode session home: {error}")))
}

fn cleanup_session_home_at(source_home: &Path, session_id: &str) -> std::io::Result<()> {
    let root = source_home.join(SESSION_HOME_DIR);
    let expected = root.join(session_storage_key(session_id));
    let Some(home) = find_session_home(&root, &expected, session_id) else {
        return Ok(());
    };
    stop_private_daemon(&home)?;
    remove_home_aliases_for_target(&home)?;
    fs::remove_dir_all(home)
}

fn prune_unclaimed_session_homes(root: &Path) -> std::io::Result<()> {
    prune_unclaimed_session_homes_at(root, SystemTime::now())
}

fn prune_unclaimed_session_homes_at(root: &Path, now: SystemTime) -> std::io::Result<()> {
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
        if stale {
            stop_private_daemon(&home)?;
            remove_home_aliases_for_target(&home)?;
            fs::remove_dir_all(home)?;
        }
    }
    Ok(())
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

fn remove_home_aliases_for_target(home: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let root = short_home_alias_root();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries.flatten() {
            let alias = entry.path();
            if fs::symlink_metadata(&alias).is_ok_and(|metadata| metadata.file_type().is_symlink())
                && fs::read_link(&alias).is_ok_and(|target| target == home)
            {
                fs::remove_file(alias)?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = home;
    Ok(())
}

#[cfg(unix)]
fn stop_private_daemon(home: &Path) -> std::io::Result<()> {
    let registry_path = home.join("servers.json");
    let raw = match fs::read(&registry_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let Ok(registry) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return Ok(());
    };
    let Ok(runtime_dir) = fs::canonicalize(home.join("run")) else {
        return Ok(());
    };
    let Some(pid) = registry
        .as_object()
        .and_then(|entries| {
            entries.values().find_map(|entry| {
                let entry = entry.as_object()?;
                let socket = Path::new(entry.get("socket")?.as_str()?);
                let socket_runtime = socket.parent()?;
                if fs::canonicalize(socket_runtime).ok()?.as_path() != runtime_dir.as_path() {
                    return None;
                }
                i32::try_from(entry.get("pid")?.as_i64()?).ok()
            })
        })
        .filter(|pid| *pid > 1)
    else {
        return Ok(());
    };
    signal_private_process_group(pid, libc::SIGTERM);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && private_process_exists(pid) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if private_process_exists(pid) {
        signal_private_process_group(pid, libc::SIGKILL);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && private_process_exists(pid) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn stop_private_daemon(_home: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn signal_private_process_group(pid: i32, signal: i32) {
    // The PID comes from this Cruise-owned private home's jcode server registry.
    unsafe {
        if libc::kill(-pid, signal) != 0 {
            libc::kill(pid, signal);
        }
    }
}

#[cfg(unix)]
fn private_process_exists(pid: i32) -> bool {
    // Signal 0 only probes whether the private daemon PID is still live.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn write_session_id(home: &SessionHome, session_id: &str) -> std::io::Result<()> {
    let path = home.storage_home.join(SESSION_ID_FILE);
    fs::write(&path, session_id)?;
    set_private_file(&path)
}

fn copy_global_mcp(source_home: &Path, storage_home: &Path) -> std::io::Result<()> {
    let source = source_home.join("mcp.json");
    let destination = storage_home.join("mcp.json");
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
    set_private_file(&destination)
}

fn disable_private_updates(storage_home: &Path) -> std::io::Result<()> {
    let path = storage_home.join("config.toml");
    let mut config = match fs::read_to_string(&path) {
        Ok(contents) => toml::from_str::<toml::Value>(&contents).map_err(std::io::Error::other)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        Err(error) => return Err(error),
    };
    let config = config
        .as_table_mut()
        .ok_or_else(|| std::io::Error::other("jcode config.toml must contain a table"))?;
    let features = config
        .entry("features")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| std::io::Error::other("jcode config [features] must be a table"))?;
    features.insert("check_updates".to_string(), toml::Value::Boolean(false));
    let contents = toml::to_string(&config).map_err(std::io::Error::other)?;
    fs::write(&path, contents)?;
    set_private_file(&path)
}

fn resolve_source_home(cwd: Option<&Path>) -> PathBuf {
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
    let base = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(path)
}

fn session_storage_key(session_id: &str) -> String {
    sha256_hex(session_id.as_bytes())
}

fn stable_alias_key(source_home: &Path, session_id: &str) -> String {
    let mut key = source_home.as_os_str().as_encoded_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(session_id.as_bytes());
    sha256_hex(&key)
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
    PathBuf::from("/tmp/cruise-jcode")
}

#[cfg(not(unix))]
fn short_home_alias_root() -> PathBuf {
    std::env::temp_dir().join("cruise-jcode")
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
    Ok(())
}

#[cfg(test)]
mod tests;
