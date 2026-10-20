//! Workflow defaults (`default.yaml`) layered under every resolved workflow.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;

use crate::config::{
    LanguagesConfig, McpServers, ModelSpec, PermissionMode, RetryConfig, SkipCondition, StepConfig,
    StringOrVec, WhenCondition, WorkflowConfig,
};
use crate::error::{CruiseError, Result};

/// File name of the reserved workflow-defaults file.
pub(crate) const DEFAULTS_FILE_NAME: &str = "default.yaml";

/// Top-level keys that only make sense in a workflow.
const WORKFLOW_ONLY_KEYS: [&str; 4] = ["description", "steps", "groups", "after-pr"];

/// Path of the user defaults file (`<config_dir>/default.yaml`), when a config
/// directory can be determined.
pub(crate) fn user_defaults_path() -> Option<PathBuf> {
    crate::paths::config_dir()
        .ok()
        .map(|dir| dir.join(DEFAULTS_FILE_NAME))
}

/// Path of the project defaults file (`<project_dir>/.cruise/default.yaml`).
pub(crate) fn project_defaults_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".cruise").join(DEFAULTS_FILE_NAME)
}

/// A pre-step definition from `pre_steps:` (command steps only).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreStepConfig {
    command: StringOrVec,
    #[serde(default)]
    env: HashMap<String, String>,
    timeout: Option<String>,
    skip: Option<SkipCondition>,
    when: Option<WhenCondition>,
}

impl PreStepConfig {
    fn into_step(self) -> StepConfig {
        StepConfig {
            command: Some(self.command),
            env: self.env,
            timeout: self.timeout,
            skip: self.skip,
            when: self.when,
            ..Default::default()
        }
    }
}

/// One `default.yaml` file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultsFile {
    command: Option<Vec<String>>,
    sdk: Option<String>,
    model: Option<ModelSpec>,
    plan_model: Option<ModelSpec>,
    max_retries: Option<usize>,
    interactive_planning: Option<bool>,
    languages: Option<LanguagesConfig>,
    cleanup_after_pr: Option<bool>,
    force_exec: Option<bool>,
    computer_use: Option<bool>,
    permission: Option<PermissionMode>,
    #[serde(default)]
    env: HashMap<String, String>,
    retry: Option<RetryConfig>,
    #[serde(default)]
    mcp_servers: McpServers,
    #[serde(default)]
    pre_steps: IndexMap<String, PreStepConfig>,
}

/// A value together with the defaults file it came from.
#[derive(Debug, Clone)]
struct Sourced<T> {
    value: T,
    path: PathBuf,
}

fn sourced<T>(value: Option<T>, path: &Path) -> Option<Sourced<T>> {
    value.map(|value| Sourced {
        value,
        path: path.to_path_buf(),
    })
}

fn parse_error(path: &Path, detail: impl std::fmt::Display) -> CruiseError {
    CruiseError::ConfigParseError(format!("{}: {detail}", path.display()))
}

/// Merged user + project defaults.
#[derive(Debug, Clone, Default)]
pub struct WorkflowDefaults {
    backend: Option<Sourced<(Vec<String>, Option<String>)>>,
    model: Option<Sourced<ModelSpec>>,
    plan_model: Option<Sourced<ModelSpec>>,
    max_retries: Option<usize>,
    interactive_planning: Option<bool>,
    languages: Option<LanguagesConfig>,
    cleanup_after_pr: Option<bool>,
    force_exec: Option<bool>,
    computer_use: Option<Sourced<bool>>,
    permission: Option<Sourced<PermissionMode>>,
    env: HashMap<String, String>,
    retry: Option<Sourced<RetryConfig>>,
    mcp_servers: IndexMap<String, Sourced<crate::config::McpServerConfig>>,
    pre_steps: IndexMap<String, StepConfig>,
}

/// Resolution state for fields whose public `WorkflowConfig` values
/// are materialized before defaults have been applied.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "these field-presence flags represent independent workflow settings"
)]
pub(crate) struct WorkflowConfigPresence {
    interactive_planning: bool,
    cleanup_after_pr: bool,
    force_exec: bool,
    computer_use: bool,
    permission: bool,
}

impl WorkflowConfigPresence {
    pub(crate) fn from_yaml(yaml: &str) -> std::result::Result<Self, serde_yaml::Error> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
        let mut presence = Self::default();
        if let Some(mapping) = value.as_mapping() {
            for key in mapping.keys() {
                match key.as_str() {
                    Some("interactive_planning") => presence.interactive_planning = true,
                    Some("cleanup_after_pr") => presence.cleanup_after_pr = true,
                    Some("force_exec") => presence.force_exec = true,
                    Some("computer_use") => presence.computer_use = true,
                    Some("permission") => presence.permission = true,
                    _ => {}
                }
            }
        }
        Ok(presence)
    }

    fn materialized() -> Self {
        Self {
            interactive_planning: true,
            cleanup_after_pr: true,
            force_exec: true,
            computer_use: true,
            permission: true,
        }
    }
}

/// Config plus field-presence information, retained until defaults have been
/// applied and then discarded in favor of the effective public config.
#[derive(Debug)]
pub struct WorkflowConfigResolution {
    pub config: WorkflowConfig,
    presence: WorkflowConfigPresence,
}

impl WorkflowConfigResolution {
    /// Parse a workflow config while retaining which fields were explicitly set.
    ///
    /// # Errors
    ///
    /// Returns an error when `yaml` is not valid workflow configuration.
    pub fn from_yaml(yaml: &str) -> std::result::Result<Self, serde_yaml::Error> {
        Ok(Self {
            config: WorkflowConfig::from_yaml(yaml)?,
            presence: WorkflowConfigPresence::from_yaml(yaml)?,
        })
    }

    pub(crate) fn from_materialized(config: WorkflowConfig) -> Self {
        Self {
            config,
            presence: WorkflowConfigPresence::materialized(),
        }
    }
}

fn load_file(path: &Path) -> Result<Option<DefaultsFile>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(parse_error(path, e)),
    };
    let value: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| parse_error(path, e))?;
    if value.is_null() {
        return Ok(None);
    }
    if let Some(map) = value.as_mapping() {
        for key in WORKFLOW_ONLY_KEYS {
            if map.contains_key(key) {
                return Err(parse_error(
                    path,
                    format!(
                        "'{key}' is workflow-only and cannot be set in default.yaml; move this file to .cruise/<name>.yaml (project) or <config_dir>/workflows/<name>.yaml (user) to use it as a workflow"
                    ),
                ));
            }
        }
    }
    let file: DefaultsFile = serde_yaml::from_value(value).map_err(|e| parse_error(path, e))?;
    for name in file.pre_steps.keys() {
        if name.is_empty() || name.contains('/') {
            return Err(parse_error(
                path,
                format!(
                    "invalid pre_steps name '{name}': must be non-empty and must not contain '/'"
                ),
            ));
        }
    }
    Ok(Some(file))
}

impl WorkflowDefaults {
    /// Load the user and project defaults layers.
    ///
    /// # Errors
    ///
    /// Returns an error when a defaults file is unreadable or invalid.
    pub fn load(project_dir: Option<&Path>) -> Result<Self> {
        let mut merged = Self::default();
        let layers = [user_defaults_path(), project_dir.map(project_defaults_path)];
        for path in layers.into_iter().flatten() {
            if let Some(file) = load_file(&path)? {
                merged.overlay(file, &path);
            }
        }
        Ok(merged)
    }

    fn overlay(&mut self, file: DefaultsFile, path: &Path) {
        if file.command.is_some() || file.sdk.is_some() {
            self.backend = sourced(Some((file.command.unwrap_or_default(), file.sdk)), path);
        }
        self.model = sourced(file.model, path).or(self.model.take());
        self.plan_model = sourced(file.plan_model, path).or(self.plan_model.take());
        self.max_retries = file.max_retries.or(self.max_retries);
        self.interactive_planning = file.interactive_planning.or(self.interactive_planning);
        self.languages = file.languages.or(self.languages.take());
        self.cleanup_after_pr = file.cleanup_after_pr.or(self.cleanup_after_pr);
        self.force_exec = file.force_exec.or(self.force_exec);
        self.computer_use = sourced(file.computer_use, path).or(self.computer_use.take());
        self.permission = sourced(file.permission, path).or(self.permission.take());
        self.retry = sourced(file.retry, path).or(self.retry.take());
        self.env.extend(file.env);
        for (name, server) in file.mcp_servers {
            self.mcp_servers.insert(
                name,
                Sourced {
                    value: server,
                    path: path.to_path_buf(),
                },
            );
        }
        for (name, step) in file.pre_steps {
            self.pre_steps.insert(name, step.into_step());
        }
    }

    /// Fill unset fields of `resolution.config` and insert `pre_steps`, using
    /// the field-presence information retained while parsing the workflow YAML.
    ///
    /// # Errors
    ///
    /// Returns an error on backend collision or `pre/<name>` key collision.
    pub fn apply_to(&self, resolution: &mut WorkflowConfigResolution) -> Result<()> {
        self.apply_to_with_presence(&mut resolution.config, resolution.presence)
    }

    #[cfg(test)]
    fn apply_to_materialized_config(&self, config: &mut WorkflowConfig) -> Result<()> {
        self.apply_to_with_presence(config, WorkflowConfigPresence::materialized())
    }

    fn apply_to_with_presence(
        &self,
        config: &mut WorkflowConfig,
        presence: WorkflowConfigPresence,
    ) -> Result<()> {
        self.check_backend_collision(config, presence)?;
        self.fill_fields(config, presence);
        self.insert_pre_steps(config)
    }

    fn check_backend_collision(
        &self,
        config: &WorkflowConfig,
        presence: WorkflowConfigPresence,
    ) -> Result<()> {
        let Some(backend) = &self.backend else {
            return Ok(());
        };
        if config.command.is_empty() && config.sdk.is_none() {
            return Ok(());
        }
        let (command, sdk) = &backend.value;
        let (defaults_backend, defaults_label) = classify_backend(sdk.as_deref(), command);
        let (workflow_backend, workflow_label) =
            classify_backend(config.sdk.as_deref(), &config.command);
        if defaults_backend == workflow_backend {
            return Ok(());
        }

        let conflicts: [(&str, Option<&Path>); 6] = [
            (
                "model",
                self.model
                    .as_ref()
                    .filter(|_| config.model.is_none())
                    .map(|s| s.path.as_path()),
            ),
            (
                "plan_model",
                self.plan_model
                    .as_ref()
                    .filter(|_| config.plan_model.is_none())
                    .map(|s| s.path.as_path()),
            ),
            (
                "retry",
                self.retry
                    .as_ref()
                    .filter(|_| config.retry.is_none())
                    .map(|s| s.path.as_path()),
            ),
            (
                "mcp_servers",
                self.mcp_servers
                    .iter()
                    .find(|(name, _)| !config.mcp_servers.contains_key(*name))
                    .map(|(_, s)| s.path.as_path()),
            ),
            (
                "computer_use",
                self.computer_use
                    .as_ref()
                    .filter(|_| !presence.computer_use)
                    .map(|s| s.path.as_path()),
            ),
            (
                "permission",
                self.permission
                    .as_ref()
                    .filter(|_| !presence.permission)
                    .map(|s| s.path.as_path()),
            ),
        ];
        if let Some((field, Some(path))) = conflicts.into_iter().find(|(_, p)| p.is_some()) {
            return Err(CruiseError::InvalidStepConfig(format!(
                "{field} from {} targets the {} backend, but the workflow uses {}; set {field} in the workflow or remove it from default.yaml",
                path.display(),
                defaults_label,
                workflow_label,
            )));
        }
        Ok(())
    }

    fn fill_fields(&self, config: &mut WorkflowConfig, presence: WorkflowConfigPresence) {
        if config.command.is_empty()
            && config.sdk.is_none()
            && let Some(backend) = &self.backend
        {
            config.command.clone_from(&backend.value.0);
            config.sdk.clone_from(&backend.value.1);
        }
        if config.model.is_none() {
            config.model = self.model.as_ref().map(|s| s.value.clone());
        }
        if config.plan_model.is_none() {
            config.plan_model = self.plan_model.as_ref().map(|s| s.value.clone());
        }
        if config.max_retries.is_none() {
            config.max_retries = self.max_retries;
        }
        if !presence.interactive_planning
            && let Some(value) = self.interactive_planning
        {
            config.interactive_planning = value;
        }
        if config.languages.is_none()
            && config.pr_language.is_none()
            && config.plan_language.is_none()
        {
            config.languages.clone_from(&self.languages);
        }
        if !presence.cleanup_after_pr
            && let Some(value) = self.cleanup_after_pr
        {
            config.cleanup_after_pr = value;
        }
        if !presence.force_exec
            && let Some(value) = self.force_exec
        {
            config.force_exec = value;
        }
        if !presence.computer_use
            && let Some(value) = &self.computer_use
        {
            config.computer_use = value.value;
        }
        if !presence.permission
            && let Some(value) = &self.permission
        {
            config.permission = value.value;
        }
        if config.retry.is_none() {
            config.retry = self.retry.as_ref().map(|s| s.value.clone());
        }
        for (key, value) in &self.env {
            config
                .env
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        for (name, server) in &self.mcp_servers {
            config
                .mcp_servers
                .entry(name.clone())
                .or_insert_with(|| server.value.clone());
        }
    }

    fn insert_pre_steps(&self, config: &mut WorkflowConfig) -> Result<()> {
        if self.pre_steps.is_empty() {
            return Ok(());
        }
        let mut steps = IndexMap::with_capacity(self.pre_steps.len() + config.steps.len());
        for (name, step) in &self.pre_steps {
            steps.insert(format!("pre/{name}"), step.clone());
        }
        for (name, step) in std::mem::take(&mut config.steps) {
            match steps.entry(name) {
                indexmap::map::Entry::Vacant(entry) => {
                    entry.insert(step);
                }
                indexmap::map::Entry::Occupied(entry) => {
                    return Err(CruiseError::InvalidStepConfig(format!(
                        "pre step key '{}' collides with an existing step name",
                        entry.key()
                    )));
                }
            }
        }
        config.steps = steps;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum BackendClass {
    Jcode,
    Claude,
    Command,
    Unsupported,
}

fn classify_backend<'a>(sdk: Option<&'a str>, command: &[String]) -> (BackendClass, &'a str) {
    match sdk {
        Some("jcode") => (BackendClass::Jcode, "jcode"),
        Some("claude") => (BackendClass::Claude, "claude"),
        Some(other) => (BackendClass::Unsupported, other),
        None if command.is_empty() => (BackendClass::Jcode, "jcode"),
        None => (BackendClass::Command, "command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelSpec, StringOrVec, WorkflowConfig};
    use crate::error::CruiseError;
    use crate::test_support::{EnvGuard, lock_process, set_fake_home};
    use std::path::PathBuf;

    struct Env {
        _guards: Vec<EnvGuard>,
        _lock: crate::test_support::ProcessLock,
        _home: tempfile::TempDir,
        project: tempfile::TempDir,
        user_file: PathBuf,
        project_file: PathBuf,
    }

    fn env() -> Env {
        let lock = lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let mut guards = set_fake_home(home.path());
        for key in [
            "CRUISE_MODEL",
            "CRUISE_PLAN_MODEL",
            "CRUISE_SDK",
            "CRUISE_LANGUAGE_PR",
            "CRUISE_LANGUAGE_PLAN",
            "CRUISE_CLEANUP_AFTER_PR",
            "CRUISE_INTERACTIVE_PLANNING",
            "CRUISE_FORCE_EXEC",
        ] {
            guards.push(EnvGuard::remove(key));
        }
        let project = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let user_dir = crate::paths::config_dir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::create_dir_all(&user_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::create_dir_all(project.path().join(".cruise")).unwrap_or_else(|e| panic!("{e:?}"));
        Env {
            _lock: lock,
            _guards: guards,
            _home: home,
            user_file: user_dir.join("default.yaml"),
            project_file: project.path().join(".cruise").join("default.yaml"),
            project,
        }
    }

    fn write(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap_or_else(|e| panic!("{e:?}"));
    }

    fn workflow(extra: &str) -> WorkflowConfig {
        WorkflowConfig::from_yaml(&workflow_yaml(extra)).unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn workflow_yaml(extra: &str) -> String {
        format!("{extra}\nsteps:\n  a:\n    command: echo a\n  b:\n    command: echo b\n")
    }

    fn applied(env: &Env, config: &mut WorkflowConfig) {
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));
        defaults
            .apply_to_materialized_config(config)
            .unwrap_or_else(|e| panic!("{e:?}"));
    }

    fn applied_with_presence(env: &Env, yaml: &str) -> WorkflowConfig {
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));
        let mut resolution =
            WorkflowConfigResolution::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        defaults
            .apply_to(&mut resolution)
            .unwrap_or_else(|e| panic!("{e:?}"));
        resolution.config
    }

    fn parse_error(env: &Env) -> String {
        match WorkflowDefaults::load(Some(env.project.path())) {
            Err(CruiseError::ConfigParseError(message)) => message,
            other => panic!("expected ConfigParseError, got {other:?}"),
        }
    }

    fn command_text(step: &crate::config::StepConfig) -> String {
        match &step.command {
            Some(StringOrVec::Single(s)) => s.clone(),
            other => panic!("unexpected command {other:?}"),
        }
    }

    // ---- load ----

    #[test]
    fn missing_files_leave_config_unchanged() {
        let env = env();
        let mut config = workflow("command: [echo]");
        let before = serde_yaml::to_string(&config).unwrap_or_else(|e| panic!("{e:?}"));
        applied(&env, &mut config);
        assert_eq!(
            serde_yaml::to_string(&config).unwrap_or_else(|e| panic!("{e:?}")),
            before
        );
    }

    #[test]
    fn empty_file_is_empty_defaults() {
        let env = env();
        write(&env.project_file, "");
        let mut config = workflow("command: [echo]");
        let before = serde_yaml::to_string(&config).unwrap_or_else(|e| panic!("{e:?}"));
        applied(&env, &mut config);
        assert_eq!(
            serde_yaml::to_string(&config).unwrap_or_else(|e| panic!("{e:?}")),
            before
        );
    }

    #[test]
    fn workflow_only_keys_are_rejected_with_path_and_key() {
        for (key, body) in [
            ("steps", "steps:\n  x:\n    command: echo\n"),
            ("description", "description: hi\n"),
            ("groups", "groups: {}\n"),
            ("after-pr", "after-pr: {}\n"),
        ] {
            let env = env();
            write(&env.project_file, body);
            let message = parse_error(&env);
            assert!(message.contains(key), "{key}: {message}");
            assert!(message.contains("default.yaml"), "{key}: {message}");
        }
    }

    #[test]
    fn unknown_key_is_rejected_with_path() {
        let env = env();
        write(&env.user_file, "modle: x\n");
        let message = parse_error(&env);
        assert!(message.contains("default.yaml"), "{message}");
    }

    #[test]
    fn pre_step_with_prompt_is_rejected() {
        let env = env();
        write(
            &env.project_file,
            "pre_steps:\n  setup:\n    prompt: hello\n",
        );
        assert!(WorkflowDefaults::load(Some(env.project.path())).is_err());
    }

    #[test]
    fn pre_step_name_with_slash_is_rejected() {
        let env = env();
        write(
            &env.project_file,
            "pre_steps:\n  \"a/b\":\n    command: echo\n",
        );
        assert!(WorkflowDefaults::load(Some(env.project.path())).is_err());
    }

    #[test]
    fn pre_step_without_command_is_rejected() {
        let env = env();
        write(&env.project_file, "pre_steps:\n  setup:\n    timeout: 5s\n");
        assert!(WorkflowDefaults::load(Some(env.project.path())).is_err());
    }

    // ---- layering ----

    #[test]
    fn project_scalar_wins_over_user() {
        let env = env();
        write(&env.user_file, "model: user/m\nmax_retries: 3\n");
        write(&env.project_file, "model: project/m\n");
        let mut config = workflow("command: [echo]");
        applied(&env, &mut config);
        assert_eq!(config.model, Some(ModelSpec::Single("project/m".into())));
        assert_eq!(config.max_retries, Some(3));
    }

    #[test]
    fn env_and_mcp_servers_merge_per_key_with_project_winning() {
        let env = env();
        write(
            &env.user_file,
            "env:\n  A: user\n  B: user\nmcp_servers:\n  one:\n    command: user-one\n",
        );
        write(
            &env.project_file,
            "env:\n  B: project\nmcp_servers:\n  one:\n    command: project-one\n  two:\n    command: two\n",
        );
        let mut config = workflow("");
        applied(&env, &mut config);
        assert_eq!(config.env.get("A").map(String::as_str), Some("user"));
        assert_eq!(config.env.get("B").map(String::as_str), Some("project"));
        assert_eq!(config.mcp_servers.len(), 2);
        assert_eq!(
            config.mcp_servers["one"].command.as_deref(),
            Some("project-one")
        );
    }

    #[test]
    fn project_sdk_replaces_user_command_pair() {
        let env = env();
        write(&env.user_file, "command: [mytool, -p]\n");
        write(&env.project_file, "sdk: claude\n");
        let mut config = workflow("");
        applied(&env, &mut config);
        assert_eq!(config.sdk.as_deref(), Some("claude"));
        assert_eq!(config.command.len(), 0);
    }

    #[test]
    fn pre_steps_concatenate_user_then_project_with_project_override_in_place() {
        let env = env();
        write(
            &env.user_file,
            "pre_steps:\n  a:\n    command: user-a\n  b:\n    command: user-b\n",
        );
        write(
            &env.project_file,
            "pre_steps:\n  b:\n    command: project-b\n  c:\n    command: project-c\n",
        );
        let mut config = workflow("command: [echo]");
        applied(&env, &mut config);
        let keys: Vec<&str> = config.steps.keys().map(String::as_str).collect();
        assert_eq!(keys, ["pre/a", "pre/b", "pre/c", "a", "b"]);
        assert_eq!(command_text(&config.steps["pre/b"]), "project-b");
        assert!(config.after_pr.is_empty());
    }

    // ---- apply_to ----

    #[test]
    fn defaults_fill_unset_model_but_workflow_wins() {
        let env = env();
        write(&env.project_file, "model: default/x\n");
        let mut unset = workflow("command: [echo]");
        applied(&env, &mut unset);
        assert_eq!(unset.model, Some(ModelSpec::Single("default/x".into())));
        let mut set = workflow("command: [echo]\nmodel: wf/y");
        applied(&env, &mut set);
        assert_eq!(set.model, Some(ModelSpec::Single("wf/y".into())));
    }

    #[test]
    fn env_override_beats_defaults_and_workflow() {
        let env = env();
        write(&env.project_file, "model: default/x\n");
        let _model = EnvGuard::set("CRUISE_MODEL", "env/z");
        let mut config = workflow("command: [echo]\nmodel: wf/y");
        applied(&env, &mut config);
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.model, Some(ModelSpec::Single("env/z".into())));
    }

    #[test]
    fn bool_and_permission_defaults_fill_only_unset_fields() {
        let env = env();
        write(
            &env.project_file,
            "interactive_planning: false\ncleanup_after_pr: true\nforce_exec: true\ncomputer_use: true\npermission: read-only\n",
        );
        let unset_yaml = workflow_yaml("");
        let unset = applied_with_presence(&env, &unset_yaml);
        assert!(!unset.interactive_planning());
        assert!(unset.cleanup_after_pr());
        assert!(unset.force_exec());
        assert!(unset.computer_use());
        assert_eq!(unset.permission(), crate::config::PermissionMode::ReadOnly);

        let set_yaml = workflow_yaml(
            "interactive_planning: true\ncleanup_after_pr: false\nforce_exec: false\ncomputer_use: false\npermission: full",
        );
        let set = applied_with_presence(&env, &set_yaml);
        assert!(set.interactive_planning());
        assert!(!set.cleanup_after_pr());
        assert!(!set.force_exec());
        assert!(!set.computer_use());
        assert_eq!(set.permission(), crate::config::PermissionMode::Full);
    }

    #[test]
    fn unset_everywhere_uses_builtin_accessor_defaults() {
        let env = env();
        let mut config = workflow("");
        applied(&env, &mut config);
        assert!(config.interactive_planning());
        assert!(!config.cleanup_after_pr());
        assert!(!config.force_exec());
        assert!(!config.computer_use());
        assert_eq!(
            config.permission(),
            crate::config::PermissionMode::default()
        );
    }

    #[test]
    fn workflow_env_wins_per_key() {
        let env = env();
        write(&env.project_file, "env:\n  A: \"2\"\n  B: \"3\"\n");
        let mut config = workflow("env:\n  A: \"1\"");
        applied(&env, &mut config);
        assert_eq!(config.env.get("A").map(String::as_str), Some("1"));
        assert_eq!(config.env.get("B").map(String::as_str), Some("3"));
    }

    #[test]
    fn legacy_language_in_workflow_blocks_defaults_languages() {
        let env = env();
        write(&env.project_file, "languages:\n  pr: ja\n");
        let mut config = workflow("pr_language: en");
        applied(&env, &mut config);
        assert!(config.languages.is_none());
    }

    #[test]
    fn pre_steps_are_prepended_and_after_pr_untouched() {
        let env = env();
        write(
            &env.project_file,
            "pre_steps:\n  setup:\n    command: mise trust || true\n",
        );
        let mut config = workflow("command: [echo]");
        applied(&env, &mut config);
        let keys: Vec<&str> = config.steps.keys().map(String::as_str).collect();
        assert_eq!(keys, ["pre/setup", "a", "b"]);
        assert_eq!(
            command_text(&config.steps["pre/setup"]),
            "mise trust || true"
        );
        assert!(config.after_pr.is_empty());
    }

    #[test]
    fn existing_pre_prefixed_step_collides() {
        let env = env();
        write(&env.project_file, "pre_steps:\n  setup:\n    command: x\n");
        let mut config =
            WorkflowConfig::from_yaml("command: [echo]\nsteps:\n  pre/setup:\n    command: echo\n")
                .unwrap_or_else(|e| panic!("{e:?}"));
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(defaults.apply_to_materialized_config(&mut config).is_err());
    }

    #[test]
    fn backend_collision_is_an_invalid_step_config_error() {
        let env = env();
        write(&env.project_file, "sdk: jcode\nmodel: p/x\n");
        let mut config = workflow("command: [claude, -p]");
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));
        match defaults.apply_to_materialized_config(&mut config) {
            Err(CruiseError::InvalidStepConfig(message)) => {
                assert!(message.contains("model"), "{message}");
                assert!(message.contains("default.yaml"), "{message}");
                assert!(message.contains("jcode"), "{message}");
            }
            other => panic!("expected InvalidStepConfig, got {other:?}"),
        }
    }

    #[test]
    fn cruise_sdk_override_matching_defaults_does_not_avoid_backend_collision() {
        let env = env();
        write(&env.project_file, "sdk: jcode\nmodel: p/x\n");
        let _sdk = EnvGuard::set("CRUISE_SDK", "jcode");
        let mut config = workflow("sdk: claude");
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));

        match defaults.apply_to_materialized_config(&mut config) {
            Err(CruiseError::InvalidStepConfig(message)) => {
                assert!(message.contains("model"), "{message}");
                assert!(message.contains("default.yaml"), "{message}");
                assert!(message.contains("jcode"), "{message}");
                assert!(message.contains("claude"), "{message}");
            }
            other => panic!("expected InvalidStepConfig, got {other:?}"),
        }
    }

    #[test]
    fn cruise_sdk_override_does_not_create_backend_collision() {
        let env = env();
        write(&env.project_file, "sdk: jcode\nmodel: p/x\n");
        let _sdk = EnvGuard::set("CRUISE_SDK", "claude");
        let mut config = workflow("sdk: jcode");
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));

        defaults
            .apply_to_materialized_config(&mut config)
            .unwrap_or_else(|e| panic!("{e:?}"));
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(config.sdk.as_deref(), Some("claude"));
        assert_eq!(config.model, Some(ModelSpec::Single("p/x".into())));
    }

    #[test]
    fn model_environment_override_does_not_suppress_sourced_backend_collision() {
        let env = env();
        write(&env.project_file, "sdk: jcode\nmodel: p/x\n");
        let _model = EnvGuard::set("CRUISE_MODEL", "env/model");
        let mut config = workflow("sdk: claude");
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));

        match defaults.apply_to_materialized_config(&mut config) {
            Err(CruiseError::InvalidStepConfig(message)) => {
                assert!(message.contains("model"), "{message}");
                assert!(message.contains("default.yaml"), "{message}");
                assert!(message.contains("jcode"), "{message}");
                assert!(message.contains("claude"), "{message}");
            }
            other => panic!("expected InvalidStepConfig, got {other:?}"),
        }
    }

    #[test]
    fn plan_model_environment_override_does_not_suppress_sourced_backend_collision() {
        let env = env();
        write(&env.project_file, "sdk: jcode\nplan_model: p/y\n");
        let _plan_model = EnvGuard::set("CRUISE_PLAN_MODEL", "env/plan");
        let mut config = workflow("sdk: claude");
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));

        match defaults.apply_to_materialized_config(&mut config) {
            Err(CruiseError::InvalidStepConfig(message)) => {
                assert!(message.contains("plan_model"), "{message}");
                assert!(message.contains("default.yaml"), "{message}");
                assert!(message.contains("jcode"), "{message}");
                assert!(message.contains("claude"), "{message}");
            }
            other => panic!("expected InvalidStepConfig, got {other:?}"),
        }
    }

    #[test]
    fn false_computer_use_default_conflicts_with_different_backend() {
        let env = env();
        write(&env.project_file, "sdk: jcode\ncomputer_use: false\n");
        let yaml = workflow_yaml("command: [claude, -p]");
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));
        let mut resolution =
            WorkflowConfigResolution::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"));
        match defaults.apply_to(&mut resolution) {
            Err(CruiseError::InvalidStepConfig(message)) => {
                assert!(message.contains("computer_use"), "{message}");
                assert!(message.contains("default.yaml"), "{message}");
            }
            other => panic!("expected InvalidStepConfig, got {other:?}"),
        }
    }

    #[test]
    fn full_permission_default_conflicts_with_different_backend() {
        let env = env();
        write(&env.project_file, "sdk: jcode\npermission: full\n");
        let yaml = workflow_yaml("command: [claude, -p]");
        let defaults =
            WorkflowDefaults::load(Some(env.project.path())).unwrap_or_else(|e| panic!("{e:?}"));
        let mut resolution =
            WorkflowConfigResolution::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"));
        match defaults.apply_to(&mut resolution) {
            Err(CruiseError::InvalidStepConfig(message)) => {
                assert!(message.contains("permission"), "{message}");
                assert!(message.contains("default.yaml"), "{message}");
            }
            other => panic!("expected InvalidStepConfig, got {other:?}"),
        }
    }

    #[test]
    fn no_collision_when_workflow_sets_the_field_itself() {
        let env = env();
        write(&env.project_file, "sdk: jcode\nmodel: p/x\n");
        let mut config = workflow("command: [claude, -p]\nmodel: own");
        applied(&env, &mut config);
        assert_eq!(config.model, Some(ModelSpec::Single("own".into())));
    }

    #[test]
    fn no_collision_when_defaults_name_no_backend() {
        let env = env();
        write(&env.project_file, "model: p/x\n");
        let mut config = workflow("command: [claude, -p]");
        applied(&env, &mut config);
        assert_eq!(config.model, Some(ModelSpec::Single("p/x".into())));
    }

    #[test]
    fn backend_independent_fields_never_collide() {
        let env = env();
        write(&env.project_file, "sdk: jcode\nenv:\n  K: v\n");
        let mut config = workflow("command: [claude, -p]");
        applied(&env, &mut config);
        assert_eq!(config.env.get("K").map(String::as_str), Some("v"));
    }

    #[test]
    fn defaults_sdk_fills_workflow_without_backend() {
        let env = env();
        write(&env.project_file, "sdk: claude\n");
        let mut config = workflow("");
        applied(&env, &mut config);
        assert_eq!(config.sdk.as_deref(), Some("claude"));
    }
}
