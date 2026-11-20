//! Host-owned inputs and resources for an isolated jcode turn.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use super::{JCODE_CONFIG_FILE, JcodeClient, JcodeRunnerConfig, SessionHome, linux_git};
use super::linux_proxy::{self, ProviderProxy, Route};
use super::linux_sandbox::{Sandbox, SandboxSpec};
use crate::config::PermissionMode;

pub(super) struct Runtime {
    sandbox: Sandbox,
    _proxy: ProviderProxy,
    _scratch: Scratch,
}

impl Runtime {
    pub(super) fn stop(self) -> Result<(), String> {
        self.sandbox.stop()
    }
}

pub(super) fn launch(
    request: &JcodeRunnerConfig,
    home: &SessionHome,
    env: HashMap<OsString, OsString>,
) -> Result<(JcodeClient, Runtime), String> {
    if request.cancel.as_ref().is_some_and(|token| token.is_cancelled()) {
        return Err("jcode sandbox cancelled before setup".into());
    }
    if request.computer_use {
        return Err("computer_use is not available in the Bubblewrap jcode backend".into());
    }
    let binary = canonical(
        request.binary.as_deref().ok_or("Bubblewrap requires a resolved jcode executable")?,
        "jcode executable",
    )?;
    let helper = canonical(&std::env::current_exe().map_err(|error| error.to_string())?, "Cruise executable")?;
    let cwd = match request.cwd.as_ref() {
        Some(path) => canonical(path, "workspace")?,
        None => std::env::current_dir().and_then(|path| path.canonicalize())
            .map_err(|error| format!("could not resolve workspace: {error}"))?,
    };
    let source = canonical(&home.source_home, "source jcode home")?;
    if source.starts_with(&cwd)
        || cwd.starts_with(&source)
        || ::home::home_dir()
            .and_then(|path| path.canonicalize().ok())
            .is_some_and(|path| path.starts_with(&cwd))
    {
        return Err("jcode workspace must not contain the operator or source jcode home".into());
    }
    let envelope = canonical(&home.storage_home, "host-owned jcode session directory")?;
    let state = envelope.join("bubblewrap");
    let guest_home = state.join("jcode");
    if home.resume_session_found && !guest_home.is_dir() {
        return Err("the saved jcode conversation predates the Bubblewrap backend; start a new conversation from the saved plan".into());
    }
    super::ensure_private_dir(&state).map_err(|error| format!("private jcode state: {error}"))?;
    super::ensure_private_dir(&guest_home)
        .map_err(|error| format!("private jcode home: {error}"))?;
    let (route, env) = stage_provider(request, &source, &envelope, &guest_home, env)?;
    let port = match &route { Route::Connect(_) => None, Route::Loopback(port) => Some(*port) };

    let scratch = Scratch::create().map_err(|error| format!("could not create private jcode sockets: {error}"))?;
    let runtime = scratch.0.join("run");
    let egress = scratch.0.join("egress");
    for path in [&runtime, &egress] {
        fs::DirBuilder::new().mode(0o700).create(path)
            .map_err(|error| format!("could not create jcode runtime directory: {error}"))?;
    }
    let egress_socket = egress.join("model.sock");
    let git = linux_git::prepare(&cwd, &scratch.0)?;
    let mut inputs = Vec::with_capacity(request.sandbox_access.inputs.len() + 1);
    for path in &request.sandbox_access.inputs {
        inputs.push(unalias(path, "requested jcode input")?);
    }
    inputs.push(guest_home.join(JCODE_CONFIG_FILE));
    let output = request.sandbox_access.output_file.as_deref().map(reserve_output).transpose()?;
    let proxy = linux_proxy::start(&egress_socket, route)
        .map_err(|error| format!("could not start the private jcode model gateway: {error}"))?;
    let mut sandbox = Sandbox::launch(SandboxSpec {
        workspace: &cwd,
        read_only: request.sandbox_access.read_only || request.permission == PermissionMode::ReadOnly,
        binary: &binary,
        helper: &helper,
        state: &state,
        runtime: &runtime,
        egress_socket: &egress_socket,
        local_port: port,
        git: &git,
        inputs: &inputs,
        output_file: output.as_deref(),
        env: &env,
    }, request.cancel.as_ref())?;
    let client = sandbox.connect(request.cancel.as_ref())?;
    Ok((client, Runtime { sandbox, _proxy: proxy, _scratch: scratch }))
}

fn canonical(path: &Path, purpose: &str) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|error| format!("could not resolve {purpose}: {error}"))
}

fn reserve_output(path: &Path) -> Result<PathBuf, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => OpenOptions::new()
            .create_new(true).write(true).mode(0o600).open(path)
            .and_then(|file| file.metadata())
            .map_err(|error| format!("could not reserve the jcode plan output: {error}"))?,
        Err(error) => return Err(format!("could not inspect the jcode plan output: {error}")),
    };
    if !metadata.file_type().is_file() || metadata.nlink() != 1 {
        return Err("the writable jcode plan must be a regular file with one link".into());
    }
    unalias(path, "jcode plan output")
}

fn unalias(path: &Path, purpose: &str) -> Result<PathBuf, String> {
    let physical = canonical(path, purpose)?;
    if physical != path {
        return Err(format!("{purpose} must use its canonical path inside Bubblewrap"));
    }
    Ok(physical)
}

struct Scratch(PathBuf);

impl Scratch {
    fn create() -> io::Result<Self> {
        // Linux Unix-domain addresses must fit in sockaddr_un.sun_path.
        let root = std::env::temp_dir()
            .canonicalize()?
            .join(format!("cr-bw-{}", uuid::Uuid::new_v4().simple()));
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        Ok(Self(root))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("cruise: could not remove private jcode runtime directory {}: {error}", self.0.display());
            }
        }
    }
}

fn stage_provider(
    request: &JcodeRunnerConfig,
    source: &Path,
    envelope: &Path,
    state: &Path,
    mut env: HashMap<OsString, OsString>,
) -> Result<(Route, HashMap<OsString, OsString>), String> {
    let provider = request.provider.as_deref().ok_or("Bubblewrap jcode requires an explicit provider/model")?;
    if request.subagent_provider.as_deref().is_some_and(|worker| worker != provider) {
        return Err("Bubblewrap jcode requires the same provider for the main turn and subagents".into());
    }
    let settings = match fs::read_to_string(source.join(JCODE_CONFIG_FILE)) {
        Ok(value) => value.parse::<toml::Table>().map_err(|_| "invalid source jcode config.toml".to_string())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => return Err(format!("could not read source jcode settings: {error}")),
    };
    let mut guest = toml::Table::new();
    if let Some(codemode) = settings.get("tools").and_then(|value| value.get("codemode")).and_then(toml::Value::as_bool) {
        guest.insert("tools".into(), toml::Value::Table(toml::Table::from_iter([("codemode".into(), toml::Value::Boolean(codemode))])));
    }
    let custom = settings.get("providers").and_then(|value| value.get(provider)).and_then(toml::Value::as_table);
    let (route, key, auth_file) = if let Some(profile) = custom {
        let route = custom_route(profile)?;
        if profile.contains_key("env_file") || profile.contains_key("api_key_env") {
            return Err("Bubblewrap jcode does not import custom provider env files or arbitrary API-key environment names".into());
        }
        let profile: toml::Table = profile.iter()
            .filter(|(name, _)| matches!(name.as_str(),
                "provider_type" | "base_url" | "api" | "auth" | "auth_header" | "api_key"
                | "default_model" | "requires_api_key" | "provider_routing" | "model_catalog"
                | "allow_provider_pinning" | "models" | "supports_reasoning_effort"))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        guest.insert("providers".into(), toml::Value::Table(toml::Table::from_iter([
            (provider.to_string(), toml::Value::Table(profile)),
        ])));
        (route, None, None)
    } else {
        let (hosts, key, file): (&[&str], &str, Option<&str>) = match provider {
            "openai" | "openai-api" => (&["api.openai.com", "chatgpt.com", "auth.openai.com"], "OPENAI_API_KEY", Some("openai-auth.json")),
            "claude" | "anthropic" | "anthropic-api" => (&["api.anthropic.com", "platform.claude.com"], "ANTHROPIC_API_KEY", Some("auth.json")),
            "openrouter" => (&["openrouter.ai"], "OPENROUTER_API_KEY", None),
            "kimi" => (&["api.kimi.com"], "KIMI_API_KEY", None),
            _ => return Err(format!("provider '{provider}' is not supported by the Bubblewrap jcode backend")),
        };
        (Route::Connect(hosts.iter().map(|host| (*host).to_string()).collect()), Some(key), file)
    };
    env.retain(|name, _| name.to_str().is_some_and(|name| Some(name) == key || matches!(name,
        "JCODE_PROVIDER" | "JCODE_MODEL" | "JCODE_OPENAI_SERVICE_TIER" | "JCODE_NO_TELEMETRY"
        | "JCODE_CHECK_UPDATES" | "JCODE_DISABLED_TOOLS" | "JCODE_HOOK_PRE_TOOL"
        | "JCODE_HOOKS_DISABLED" | "CRUISE_JCODE_SHELL_DENY")));
    if let Some(key) = key && !env.contains_key(OsStr::new(key)) && let Some(value) = std::env::var_os(key) {
        env.insert(key.into(), value);
    }
    if let Some(value) = std::env::var_os("JCODE_HOOKS_DISABLED") {
        env.entry("JCODE_HOOKS_DISABLED".into()).or_insert(value);
    }
    env.insert("JCODE_DISABLE_CLAUDE_MCP".into(), "1".into());
    for name in ["auth.json", "openai-auth.json", "mcp.json"] {
        let destination = state.join(name);
        if Some(name) == auth_file {
            let original = source.join(name);
            match fs::symlink_metadata(&original) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    let credential = fs::read(original).map_err(|error| format!("could not read jcode login: {error}"))?;
                    write_private(envelope, &destination, &credential)
                        .map_err(|error| format!("could not stage jcode login: {error}"))?;
                    continue;
                }
                Ok(_) => return Err(format!("jcode login file '{name}' must be a regular file")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("could not inspect jcode login: {error}")),
            }
        }
        if let Err(error) = fs::remove_file(destination) && error.kind() != io::ErrorKind::NotFound {
            return Err(format!("could not revoke previous guest input: {error}"));
        }
    }
    let guest = toml::to_string(&guest).map_err(|_| "could not serialize private jcode settings".to_string())?;
    write_private(envelope, &state.join(JCODE_CONFIG_FILE), guest.as_bytes())
        .map_err(|error| format!("could not stage private jcode settings: {error}"))?;
    Ok((route, env))
}

fn custom_route(profile: &toml::Table) -> Result<Route, String> {
    let endpoint = profile.get("base_url").and_then(toml::Value::as_str)
        .and_then(|value| url::Url::parse(value).ok()).ok_or("invalid jcode provider base_url")?;
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err("jcode provider URL must not contain user information".into());
    }
    match (endpoint.scheme(), endpoint.host(), endpoint.port_or_known_default()) {
        ("http", Some(url::Host::Ipv4(ip)), Some(port)) if ip == std::net::Ipv4Addr::LOCALHOST && port != 0 => Ok(Route::Loopback(port)),
        ("https", Some(url::Host::Domain(host)), Some(443)) if !["github.com", "gitlab.com", "bitbucket.org"].iter().any(|forge| host == *forge || host.strip_suffix(forge).is_some_and(|prefix| prefix.ends_with('.'))) => Ok(Route::Connect(vec![host.to_string()])),
        _ => Err("jcode provider requires HTTPS on port 443 or an explicit IPv4 loopback HTTP endpoint".into()),
    }
}

fn write_private(envelope: &Path, destination: &Path, bytes: &[u8]) -> io::Result<()> {
    let staging = envelope.join(uuid::Uuid::new_v4().simple().to_string());
    let mut file: File = OpenOptions::new().create_new(true).write(true).mode(0o600).open(&staging)?;
    let result = file.write_all(bytes).and_then(|()| fs::rename(&staging, destination));
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_only_selected_provider_credentials() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let state = fixture.path().join("guest");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&state).unwrap();
        fs::write(source.join("openai-auth.json"), "selected provider login").unwrap();
        fs::write(source.join("auth.json"), "other provider login").unwrap();
        fs::write(state.join("auth.json"), "stale login").unwrap();
        let request = JcodeRunnerConfig {
            provider: Some("openai".into()),
            ..Default::default()
        };
        let (_, env) = stage_provider(
            &request,
            &source,
            fixture.path(),
            &state,
            HashMap::from([
                ("OPENAI_API_KEY".into(), "selected-key".into()),
                ("GH_TOKEN".into(), "forge-token".into()),
                ("ANTHROPIC_API_KEY".into(), "other-key".into()),
                ("GIT_DIR".into(), "/not/the/worktree".into()),
            ]),
        )
        .unwrap();
        assert_eq!(env.get(OsStr::new("OPENAI_API_KEY")), Some(&"selected-key".into()));
        for name in ["GH_TOKEN", "ANTHROPIC_API_KEY", "GIT_DIR"] {
            assert!(!env.contains_key(OsStr::new(name)));
        }
        assert!(!state.join("auth.json").exists());
        assert_eq!(fs::read_to_string(state.join("openai-auth.json")).unwrap(), "selected provider login");
        assert_eq!(fs::read_to_string(source.join("auth.json")).unwrap(), "other provider login");
    }

    #[test]
    fn custom_route_cannot_redirect_to_a_forge_subdomain() {
        let profile: toml::Table = "base_url = 'https://api.github.com/v1'".parse().unwrap();
        assert!(custom_route(&profile).is_err());
    }
}
