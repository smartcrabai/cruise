use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_PR_LANGUAGE: &str = "English";
pub const DEFAULT_PLAN_LANGUAGE: &str = "English";

/// Default global loop-protection ceiling ("G") when neither an explicit CLI
/// flag nor a workflow config `max_retries` is set.
///
/// Lives here (rather than in the CLI-only `cli` module) because this file is
/// shared by both the `cruise` binary and the `cruise` library crate (used by
/// the Tauri GUI), and [`resolve_effective_max_retries`] must be callable from
/// both.
pub const DEFAULT_MAX_RETRIES: usize = 3;

/// Nested language configuration (new style).
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct LanguagesConfig {
    /// Language for built-in PR title/body generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,

    /// Language for built-in planning prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// Top-level workflow configuration.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkflowConfig {
    /// LLM invocation command (e.g. `["claude", "--model", "{model}", "-p"]`).
    ///
    /// Mutually exclusive with `sdk`. Defaults to empty; exactly one of `command`
    /// or `sdk` must be set (validated by [`validate_sdk`]).
    #[serde(default)]
    pub command: Vec<String>,

    /// SDK to drive prompt execution instead of an external `command`.
    /// Mutually exclusive with `command`. Accepted values (validated by
    /// [`validate_sdk`]):
    ///
    /// - `"seher"` — routes through seher's provider-resolution layer
    ///   (`~/.config/seher/config.yaml`), which picks a concrete
    ///   provider/model. `model` / `plan_model` / per-step `model` are
    ///   reinterpreted as seher `mode_key`s (default: `model` -> `build`,
    ///   `plan_model` -> `plan`).
    /// - `"pi"` — drives `pi_agent_rust` directly in-process, bypassing
    ///   seher's provider resolution and config file entirely. `model` /
    ///   `plan_model` / per-step `model` are plain model references
    ///   (`"provider/model[:thinking]"` or a bare `"model"`; unset lets pi
    ///   auto-select), not mode keys.
    /// - `"claude"` — drives the `claude` CLI in-process through
    ///   `claude-agent-sdk`, with no seher provider resolution. `model` /
    ///   `plan_model` / per-step `model` are plain `claude --model` names with
    ///   an optional `:effort` suffix (unset lets the CLI pick its default),
    ///   not mode keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,

    /// Default model for prompt steps (e.g. "sonnet"). Per-step model overrides this.
    pub model: Option<String>,

    /// Model to use for the built-in plan step (falls back to `model`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,

    /// Global loop-protection ceiling; CLI `--max-retries` overrides; defaults to
    /// [`DEFAULT_MAX_RETRIES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<usize>,

    /// Whether SDK-mode planning drives the plan through the interactive custom
    /// tools (`submit_plan` / `update_plan` / `ask_user`).
    ///
    /// When `true` (the default) the planning agent persists and edits the plan
    /// via those tools, which restricts provider resolution to the tool-capable
    /// SDKs (`pi`, `omp`, `pi-rust`, `claude`). When `false`, planning instead
    /// embeds the target plan file path in the prompt and asks the agent to
    /// write `plan.md` directly — exactly like the `command` backend (the file
    /// is read back afterward, falling back to the agent's captured output if
    /// it was not written). No custom tools are registered, so tool-incapable
    /// providers (e.g. `sdk: claude-terminal`, `sdk: claude-headless`) become
    /// eligible.
    /// Has no effect in `command` mode, which is always file-based.
    #[serde(default = "default_true")]
    pub interactive_planning: bool,

    /// Deprecated: use `languages.pr` instead. Kept for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_language: Option<String>,

    /// Deprecated: use `languages.plan` instead. Kept for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_language: Option<String>,

    /// New-style nested language configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub languages: Option<LanguagesConfig>,

    /// Remove the local git worktree and its branch automatically after the PR
    /// is created. Defaults to `false` (non-destructive). Only applies to
    /// worktree-mode sessions that successfully created a PR.
    #[serde(default)]
    pub cleanup_after_pr: bool,
    /// Execute the workflow directly in the current directory for direct plan
    /// entry points: `cruise <input>`, `cruise plan`, and `cruise --plan`.
    /// Defaults to `false`; `--no-force-exec` opts out for one invocation.
    #[serde(default)]
    pub force_exec: bool,

    /// Environment variables applied to all steps.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Group definitions. Groups share if conditions and `max_retries`.
    #[serde(default)]
    pub groups: HashMap<String, GroupConfig>,

    /// Step definitions. `IndexMap` preserves YAML key order.
    pub steps: IndexMap<String, StepConfig>,

    /// Steps to run after PR creation. Same format as `steps`.
    #[serde(default, rename = "after-pr")]
    pub after_pr: IndexMap<String, StepConfig>,

    /// Human-readable description displayed alongside the file name in config selectors
    /// (CLI and GUI both read this via [`crate::yaml_metadata::extract_one_line_description`],
    /// which parses the full `WorkflowConfig` first and falls back to a raw re-parse only
    /// when that fails). Kept as a real field (rather than derived purely from YAML text)
    /// so it round-trips when a config is persisted back to YAML, e.g. into a session's
    /// `config.yaml` snapshot (see `src/plan_cmd.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A command value that can be either a single string or a list of strings.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum StringOrVec {
    Single(String),
    Multiple(Vec<String>),
}

/// Skip condition: static boolean or a variable reference.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum SkipCondition {
    /// Always skip (true) or never skip (false).
    Static(bool),
    /// Skip if the named variable resolves to "true".
    Variable(String),
}

/// Per-step configuration. All fields are optional.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct StepConfig {
    /// Model to use (prompt steps only).
    pub model: Option<String>,

    /// Inline prompt body (prompt steps only; use `prompt` or `prompt_file`).
    pub prompt: Option<String>,

    /// Message displayed to the user before this step runs (prompt steps only).
    pub instruction: Option<String>,

    /// Plan file path to display as context in option steps.
    pub plan: Option<String>,

    /// List of choices (option steps only).
    pub option: Option<Vec<OptionItem>>,

    /// Shell command(s) to run (command steps only).
    pub command: Option<StringOrVec>,

    /// Explicit next step name, overriding sequential order.
    pub next: Option<String>,

    /// Skip condition: static bool or variable reference.
    pub skip: Option<SkipCondition>,

    /// Pre-execution condition: skip the step unless the workspace satisfies the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<WhenCondition>,

    /// Conditional execution rule.
    #[serde(rename = "if")]
    pub if_condition: Option<IfCondition>,

    /// Per-step timeout. Plain digits = seconds, "Nm" = minutes, "Nh" = hours.
    /// Example: "30", "5m", "1h".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,

    /// Environment variables applied to this step (overrides top-level env).
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Group this step belongs to.
    pub group: Option<String>,

    /// Reference to another cruise workflow YAML file to inline at compile/load time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_call: Option<String>,

    /// Path or supported GitHub blob/raw URL whose contents become `prompt`.
    /// Absolute, `~`-prefixed, or relative to the directory of the config file
    /// that declares the step (a bare file name means "next to the config file").
    /// Resolved and inlined at load time by
    /// `workflow_call::resolve_workflow_calls*`; always `None` afterwards.
    #[serde(
        default,
        deserialize_with = "deserialize_prompt_file",
        skip_serializing_if = "Option::is_none"
    )]
    pub prompt_file: Option<String>,
}

fn deserialize_prompt_file<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // YAML uses `~` for null, but prompt_file reserves that spelling for the
    // user's home directory. Missing fields still use the serde default above.
    Ok(Option::<String>::deserialize(deserializer)?.or_else(|| Some("~".to_string())))
}

/// A single item in an option step.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OptionItem {
    /// Selector label shown in the menu.
    pub selector: Option<String>,

    /// Free-text input label (shows a text prompt when selected).
    #[serde(rename = "text-input")]
    pub text_input: Option<String>,

    /// Step to go to when this item is selected (None = end of workflow).
    pub next: Option<String>,
}

/// Action to take when no workspace file changes are detected after a step.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NoFileChangesAction {
    /// `no-file-changes: retry` -- re-execute the current step.
    Retry,
    /// `no-file-changes: failed` -- abort the workflow with an error.
    Failed,
}

/// Action to take when the step fails (including timeout, non-zero exit, prompt error).
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum FailAction {
    /// `if.fail: step-name` -- jump to the named step.
    Goto(String),
    /// `if.fail: { retry: true }` -- retry the current step.
    Detailed(FailDetailed),
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct FailDetailed {
    #[serde(default)]
    pub retry: bool,
}

/// Conditional execution rule.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct IfCondition {
    /// Only execute this step if the given step's snapshot differs from the current state.
    #[serde(rename = "file-changed")]
    pub file_changed: Option<String>,

    /// Action to take when no workspace file changes are detected after this step.
    #[serde(rename = "no-file-changes")]
    pub no_file_changes: Option<NoFileChangesAction>,

    /// Failure handler. Either a step name (jump) or `{ retry: true }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail: Option<FailAction>,
}

/// Pre-execution condition: skip the step unless the workspace satisfies the rule.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct WhenCondition {
    /// Skip the step if no file matches the given glob (relative to the workflow working dir).
    /// Variable references in the glob string are resolved via `VariableStore::resolve()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<String>,
}

/// Group configuration for grouping related steps.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct GroupConfig {
    /// Conditional execution rule applied to the entire group.
    #[serde(rename = "if")]
    pub if_condition: Option<IfCondition>,

    /// Maximum number of retries for this group before skipping.
    pub max_retries: Option<usize>,

    /// Steps that belong to this group (new explicit-block style).
    #[serde(default)]
    pub steps: IndexMap<String, StepConfig>,
}

fn default_true() -> bool {
    true
}

fn normalize_language(value: Option<&str>, default: &str) -> String {
    let trimmed = value.map_or("", str::trim);
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

impl WorkflowConfig {
    /// Parse a workflow config from a YAML string.
    ///
    /// # Errors
    ///
    /// Returns an error if the YAML is invalid or does not match the expected schema.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    /// Resolve the effective PR language.
    ///
    /// After [`WorkflowConfig::apply_env_overrides`], precedence is
    /// `CRUISE_LANGUAGE_PR` > `languages.pr` > `pr_language` > locale >
    /// default (`English`).
    /// Blank/whitespace values fall back to the default.
    #[must_use]
    pub fn effective_pr_language(&self) -> String {
        let from_new = self.languages.as_ref().and_then(|l| l.pr.as_deref());
        let from_old = self.pr_language.as_deref();
        normalize_language(from_new.or(from_old), DEFAULT_PR_LANGUAGE)
    }

    /// Resolve the effective planning language.
    ///
    /// After [`WorkflowConfig::apply_env_overrides`], precedence is
    /// `CRUISE_LANGUAGE_PLAN` > `languages.plan` > `plan_language` > locale >
    /// default (`English`).
    /// Blank/whitespace values fall back to the default.
    #[must_use]
    pub fn effective_plan_language(&self) -> String {
        let from_new = self.languages.as_ref().and_then(|l| l.plan.as_deref());
        let from_old = self.plan_language.as_deref();
        normalize_language(from_new.or(from_old), DEFAULT_PLAN_LANGUAGE)
    }

    /// Return warnings for deprecated language fields.
    ///
    /// Warnings are returned as plain messages; callers should prefix with
    /// `warning: ` and emit to stderr (e.g. `eprintln!`). This keeps the method
    /// testable without side effects.
    #[must_use]
    pub fn deprecated_language_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let new_pr = self.languages.as_ref().and_then(|l| l.pr.as_deref());
        let new_plan = self.languages.as_ref().and_then(|l| l.plan.as_deref());

        if self.pr_language.is_some() {
            warnings.push("'pr_language' is deprecated; use 'languages.pr' instead".to_string());
        }
        if self.plan_language.is_some() {
            warnings
                .push("'plan_language' is deprecated; use 'languages.plan' instead".to_string());
        }
        if self.pr_language.is_some() && new_pr.is_some() {
            warnings.push("'pr_language' is ignored because 'languages.pr' is set".to_string());
        }
        if self.plan_language.is_some() && new_plan.is_some() {
            warnings.push("'plan_language' is ignored because 'languages.plan' is set".to_string());
        }

        warnings
    }
}

/// Built-in default workflow config YAML, embedded at compile time.
///
/// Single source: `builtin/cruise.yaml`. Editing that file changes the
/// built-in default shipped to users with no config file.
pub const BUILTIN_CONFIG_YAML: &str = include_str!("../builtin/cruise.yaml");

impl WorkflowConfig {
    /// Apply environment variable overrides and locale-derived language defaults.
    ///
    /// # Errors
    ///
    /// Returns an error if a boolean env var has a value other than
    /// `true`, `false`, `1`, or `0`.
    pub fn apply_env_overrides(&mut self) -> crate::error::Result<()> {
        if let Some(v) = read_env_string("CRUISE_MODEL") {
            self.model = Some(v);
        }
        if let Some(v) = read_env_string("CRUISE_PLAN_MODEL") {
            self.plan_model = Some(v);
        }
        if let Some(v) = read_env_string("CRUISE_SDK") {
            self.sdk = Some(v);
            self.command = vec![]; // sdk and command are mutually exclusive; validate_sdk enforces this
        }
        if let Some(v) = read_env_string("CRUISE_LANGUAGE_PR") {
            self.languages
                .get_or_insert_with(LanguagesConfig::default)
                .pr = Some(v);
        }
        if let Some(v) = read_env_string("CRUISE_LANGUAGE_PLAN") {
            self.languages
                .get_or_insert_with(LanguagesConfig::default)
                .plan = Some(v);
        }

        // Infer languages only when neither the new nor deprecated field was
        // explicitly configured. Environment overrides above take precedence.
        let languages = self.languages.as_ref();
        if (languages.and_then(|l| l.pr.as_ref()).is_none() && self.pr_language.is_none()
            || languages.and_then(|l| l.plan.as_ref()).is_none() && self.plan_language.is_none())
            && let Some(language) = infer_language_from_locale()
        {
            let languages = self.languages.get_or_insert_with(LanguagesConfig::default);
            if languages.pr.is_none() && self.pr_language.is_none() {
                languages.pr = Some(language.clone());
            }
            if languages.plan.is_none() && self.plan_language.is_none() {
                languages.plan = Some(language);
            }
        }

        if let Some(v) = read_env_bool("CRUISE_CLEANUP_AFTER_PR")? {
            self.cleanup_after_pr = v;
        }
        if let Some(v) = read_env_bool("CRUISE_INTERACTIVE_PLANNING")? {
            self.interactive_planning = v;
        }
        if let Some(v) = read_env_bool("CRUISE_FORCE_EXEC")? {
            self.force_exec = v;
        }
        Ok(())
    }
}

fn read_env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

const LOCALE_ENV_VARS: [&str; 4] = ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"];

fn infer_language_from_locale() -> Option<String> {
    // Locale precedence follows POSIX semantics: once the first non-empty
    // variable is selected, an unsupported value must not fall through to a
    // lower-priority variable.
    let locale = LOCALE_ENV_VARS
        .iter()
        .find_map(|name| read_env_string(name))?;
    locale_to_language_name(&locale)
}

fn locale_to_language_name(locale: &str) -> Option<String> {
    let language = locale
        .trim()
        .split([':', '.', '@', '_', '-'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    let language_name = match language.as_str() {
        "en" => "English",
        "ja" => "Japanese",
        "zh" => "Chinese",
        "ko" => "Korean",
        "de" => "German",
        "fr" => "French",
        "es" => "Spanish",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ru" => "Russian",
        "nl" => "Dutch",
        "sv" => "Swedish",
        "pl" => "Polish",
        "tr" => "Turkish",
        "vi" => "Vietnamese",
        "th" => "Thai",
        "id" => "Indonesian",
        "ar" => "Arabic",
        "hi" => "Hindi",
        "uk" => "Ukrainian",
        "cs" => "Czech",
        "da" => "Danish",
        "fi" => "Finnish",
        "nb" | "no" => "Norwegian",
        "hu" => "Hungarian",
        "el" => "Greek",
        "he" => "Hebrew",
        "ro" => "Romanian",
        _ => return None,
    };
    Some(language_name.to_string())
}

fn read_env_bool(name: &str) -> crate::error::Result<Option<bool>> {
    match std::env::var(name).ok().as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some("true" | "1") => Ok(Some(true)),
        Some("false" | "0") => Ok(Some(false)),
        Some(other) => Err(crate::error::CruiseError::Other(format!(
            "invalid value for {name}: '{other}' (expected true/false/1/0)"
        ))),
    }
}

/// Validate `if.no-file-changes` usage across all steps and groups.
///
/// Enforces:
/// - `if.no-file-changes` in `after-pr` steps is rejected.
/// - `if.no-file-changes` in group-level `if` is rejected.
///
/// # Errors
///
/// Returns an error if any validation rule is violated.
pub fn validate_if_conditions(config: &WorkflowConfig) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    for (group_name, group) in &config.groups {
        if let Some(ref if_cond) = group.if_condition {
            if if_cond.fail.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "group '{group_name}' uses if.fail, which is not supported at the group level",
                )));
            }
            if if_cond.no_file_changes.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "group '{group_name}' uses if.no-file-changes, which is not supported at the group level",
                )));
            }
        }
    }

    for (name, step) in &config.after_pr {
        if let Some(ref if_cond) = step.if_condition {
            if if_cond.fail.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{name}' in after-pr uses if.fail, which is not supported in after-pr steps",
                )));
            }
            if if_cond.no_file_changes.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{name}' in after-pr uses if.no-file-changes, which is not supported in after-pr steps",
                )));
            }
        }
    }

    Ok(())
}

/// Validate `when` conditions across all steps.
///
/// # Errors
///
/// Returns an error if any `when.exists` glob is empty or syntactically invalid.
pub fn validate_when(config: &WorkflowConfig) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    let regular = config.steps.iter();
    let after_pr = config.after_pr.iter();
    let group_steps = config.groups.values().flat_map(|g| g.steps.iter());

    for (name, step) in regular.chain(after_pr).chain(group_steps) {
        if let Some(ref when) = step.when
            && let Some(ref exists) = when.exists
        {
            if exists.is_empty() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{name}' has empty when.exists glob"
                )));
            }
            // Skip static validation for globs containing variable references.
            if !exists.contains('{') {
                glob::Pattern::new(exists).map_err(|e| {
                    CruiseError::InvalidStepConfig(format!(
                        "step '{name}' has invalid when.exists glob '{exists}': {e}"
                    ))
                })?;
            }
        }
    }

    Ok(())
}

/// Run all config validations (groups, if-conditions, timeouts).
///
/// # Errors
///
/// Returns an error if any validation check fails.
pub fn validate_config(config: &WorkflowConfig) -> crate::error::Result<()> {
    validate_sdk(config)?;
    validate_groups(config)?;
    validate_mixed_conditional_cycles(config)?;
    validate_if_conditions(config)?;
    validate_timeouts(config)?;
    validate_when(config)?;
    Ok(())
}

/// SDK values accepted by [`validate_sdk`].
const SUPPORTED_SDKS: &[&str] = &["seher", "pi", "claude"];

/// Validate the top-level execution backend selection.
///
/// Exactly one of `command` or `sdk` must be specified:
/// - both set -> ambiguous, rejected.
/// - neither set -> nothing to run prompts with, rejected.
/// - `sdk` set to anything other than `"seher"` / `"pi"` / `"claude"` -> rejected.
///
/// An empty `command` list counts as "not specified" so that `sdk`-only configs
/// (where `command` defaults to `[]`) are accepted.
///
/// # Errors
///
/// Returns an error if both or neither of `command` / `sdk` are set, or if
/// `sdk` is set to an unsupported value.
pub fn validate_sdk(config: &WorkflowConfig) -> crate::error::Result<()> {
    use crate::error::CruiseError;
    let has_command = !config.command.is_empty();
    match (has_command, config.sdk.as_deref()) {
        (true, Some(_)) => Err(CruiseError::InvalidStepConfig(
            "`sdk` and `command` are mutually exclusive; specify only one".to_string(),
        )),
        (false, None) => Err(CruiseError::InvalidStepConfig(
            "either `command` or `sdk` must be specified".to_string(),
        )),
        (false, Some(sdk)) if !SUPPORTED_SDKS.contains(&sdk) => {
            Err(CruiseError::InvalidStepConfig(format!(
                "unknown `sdk` value '{sdk}'; expected one of: {}",
                SUPPORTED_SDKS.join(", ")
            )))
        }
        _ => Ok(()),
    }
}

/// Validate all timeout strings across steps, after-pr steps, and group inner steps.
///
/// # Errors
///
/// Returns an error if any timeout string fails to parse.
pub fn validate_timeouts(config: &WorkflowConfig) -> crate::error::Result<()> {
    use crate::error::CruiseError;
    for (name, step) in &config.steps {
        if let Some(ref timeout_str) = step.timeout {
            crate::timeout::parse_timeout(timeout_str).map_err(|_| {
                CruiseError::InvalidStepConfig(format!(
                    "step '{name}' has invalid timeout: '{timeout_str}'"
                ))
            })?;
        }
    }
    for (name, step) in &config.after_pr {
        if let Some(ref timeout_str) = step.timeout {
            crate::timeout::parse_timeout(timeout_str).map_err(|_| {
                CruiseError::InvalidStepConfig(format!(
                    "step '{name}' in after-pr has invalid timeout: '{timeout_str}'"
                ))
            })?;
        }
    }
    for group in config.groups.values() {
        for (sub_name, sub_step) in &group.steps {
            if let Some(ref timeout_str) = sub_step.timeout {
                crate::timeout::parse_timeout(timeout_str).map_err(|_| {
                    CruiseError::InvalidStepConfig(format!(
                        "step '{sub_name}' has invalid timeout: '{timeout_str}'"
                    ))
                })?;
            }
        }
    }
    Ok(())
}

/// Validate group configuration:
/// - All step `group` references must point to defined groups.
/// - Steps with a group must not have individual `if` conditions.
/// - Steps inside group definitions must not have nested group references or individual `if` conditions.
///
/// # Errors
///
/// Returns an error if any group configuration is invalid.
pub fn validate_groups(config: &WorkflowConfig) -> crate::error::Result<()> {
    validate_step_groups(&config.steps, &config.groups)?;
    validate_step_groups(&config.after_pr, &config.groups)?;
    validate_group_inner_steps(&config.groups)?;
    Ok(())
}

fn validate_step_groups(
    steps: &IndexMap<String, StepConfig>,
    groups: &std::collections::HashMap<String, GroupConfig>,
) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    for (step_name, step) in steps {
        if let Some(group_name) = step.group.as_deref() {
            if !groups.contains_key(group_name) {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{step_name}' references undefined group '{group_name}'"
                )));
            }
            if step.prompt.is_some() || step.prompt_file.is_some() || step.command.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{step_name}' uses old membership style (group + prompt/prompt_file/command). \
                     Please migrate to groups.<name>.steps block style."
                )));
            }
            if step.if_condition.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{step_name}' has both a group and an individual 'if' condition; use only the group's 'if'"
                )));
            }
        }
    }

    Ok(())
}

fn validate_group_inner_steps(
    groups: &std::collections::HashMap<String, GroupConfig>,
) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    for (group_name, group) in groups {
        if group.steps.is_empty() {
            return Err(CruiseError::InvalidStepConfig(format!(
                "group '{group_name}' is empty (no steps defined)"
            )));
        }
        for (sub_name, sub_step) in &group.steps {
            if sub_step.group.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "nested group call inside group '{group_name}' at step '{sub_name}' is not allowed"
                )));
            }
            if sub_step.if_condition.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "group step '{group_name}/{sub_name}' has an individual 'if' condition, \
                     which is not allowed inside group steps"
                )));
            }
        }
    }

    Ok(())
}

/// Resolve the effective global loop-protection ceiling ("G").
///
/// Precedence: an explicitly-passed CLI value wins, then the workflow config's
/// top-level `max_retries`, then [`DEFAULT_MAX_RETRIES`].
#[must_use]
pub fn resolve_effective_max_retries(cli_value: Option<usize>, config: &WorkflowConfig) -> usize {
    cli_value
        .or(config.max_retries)
        .unwrap_or(DEFAULT_MAX_RETRIES)
}

/// Validate that every group's `max_retries` ("R") can actually take effect
/// under the effective global loop-protection ceiling ("G").
///
/// The lock-step assumption -- "the group retry counter and the edge counter
/// advance together, so `R <= G` is always safe" -- only holds for **one** of
/// two shapes a group's `if.file-changed` retry target can take:
///
/// - **Case 1: the retry target re-enters the group at its own start.** The
///   target is either the call-site step name itself (the step whose
///   `group: <name>` invokes this group) or `"<call-site>/<first-sub-step>"`
///   (the group's first expanded step). Here the *only* edge retraversed
///   each retry cycle is the group-internal edge that the graceful group
///   skip watches (`group_retry_counts >= R`), so the corresponding
///   `edge_counts` entry and `group_retry_counts` move in lock-step: the
///   graceful skip fires once the count reaches `R`, which is always
///   `<= G`, so the hard `LoopProtection` failure (`count > G`) never wins
///   the race. Safe whenever `R <= G`.
///
/// - **Case 2: the retry target is some other step outside the group** (e.g.
///   an earlier step in the workflow). Each retry cycle then *also*
///   retraverses the plain sequential edge(s) leading from that target back
///   to the group's first step -- an edge that the group's graceful skip
///   does not gate at all, only the global `edge_counts` check does. That
///   edge is counted once per cycle *in addition to* the group-internal
///   edges (it includes the initial pass-through, so it reaches `R + 1` by
///   the time the group's own counter reaches `R`). Case 2 is therefore only
///   safe when `R + 1 <= G`; at `R == G` the external edge's count exceeds
///   `G` and triggers `LoopProtection` before the group ever gets to
///   gracefully skip. (This is exactly the failure this function was
///   hardened against: `groups.review` with `max_retries: 3`, retry target
///   `test` outside the group, and an unset/default `G` of 3 hit
///   `LoopProtection` on the `test -> ...` edge on its 4th traversal.)
///
/// When the group has no `if` block, or `if.file-changed` is unset, the
/// group structurally never loops back on itself (nothing increments
/// `group_retry_counts` in that case), so `max_retries` is inert. That case
/// keeps the original `R <= G` check purely as a "this setting can never
/// matter" guard rather than a safety requirement.
///
/// A group may be invoked from more than one call site. A single shared
/// `if.file-changed` target string can structurally match at most one call
/// site's own name, so -- erring on the safe (stricter) side -- case 1's
/// looser `R <= G` bound is only applied when *every* referencing call site
/// would re-enter the group at itself; if even one call site's retry target
/// points elsewhere, case 2's `R + 1 <= G` bound is required for that group.
///
/// Only groups actually referenced by a step (via `StepConfig.group`, in
/// `steps` or `after_pr`) are checked; unreferenced group definitions are
/// harmless and ignored.
///
/// Deliberately not called from [`validate_config`]: some of its callers run
/// at plan/edit time where the effective G is not yet known.
///
/// # Errors
///
/// Returns an error naming the offending group, its configured `max_retries`,
/// its retry target (case 2 only), and the effective global ceiling, when
/// the case-appropriate budget check fails.
pub fn validate_group_retry_budget(
    config: &WorkflowConfig,
    effective_max_retries: usize,
) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    // Group name -> every call-site step name (from `steps` and `after_pr`),
    // in first-seen order.
    let mut group_call_sites: IndexMap<&str, Vec<&str>> = IndexMap::new();
    for (step_name, step) in config.steps.iter().chain(config.after_pr.iter()) {
        if let Some(group_name) = step.group.as_deref() {
            group_call_sites
                .entry(group_name)
                .or_default()
                .push(step_name.as_str());
        }
    }

    for (group_name, call_sites) in group_call_sites {
        let Some(group) = config.groups.get(group_name) else {
            continue;
        };
        let Some(r) = group.max_retries else {
            continue;
        };

        let Some(target) = group
            .if_condition
            .as_ref()
            .and_then(|c| c.file_changed.as_deref())
        else {
            // No retry path exists structurally: keep the original guard.
            if r > effective_max_retries {
                return Err(CruiseError::InvalidStepConfig(unreachable_group_message(
                    group_name,
                    r,
                    effective_max_retries,
                )));
            }
            continue;
        };

        // The group's first expanded sub-step name, if any -- used to detect
        // the `"<call-site>/<first-sub-step>"` re-entry shape of case 1.
        let first_sub = group.steps.keys().next().map(String::as_str);
        let all_call_sites_reenter_group = call_sites.iter().all(|call_site| {
            target == *call_site
                || first_sub.is_some_and(|sub| target == format!("{call_site}/{sub}"))
        });

        let required_budget = if all_call_sites_reenter_group {
            r
        } else {
            let Some(required_budget) = r.checked_add(1) else {
                let max_group_retries = effective_max_retries.saturating_sub(1);
                return Err(CruiseError::InvalidStepConfig(format!(
                    "group '{group_name}' has max_retries: {r}, but its external retry target '{target}' requires one additional loop-protection edge that cannot be represented. Lower groups.{group_name}.max_retries to at most {max_group_retries}"
                )));
            };
            required_budget
        };

        if required_budget > effective_max_retries {
            let message = if all_call_sites_reenter_group {
                unreachable_group_message(group_name, r, effective_max_retries)
            } else {
                unreachable_group_message_external_target(
                    group_name,
                    r,
                    target,
                    effective_max_retries,
                )
            };
            return Err(CruiseError::InvalidStepConfig(message));
        }
    }

    Ok(())
}

/// Error message for case 1 (and the no-retry-path fallback): the group's
/// `max_retries` alone exceeds the effective ceiling.
fn unreachable_group_message(group_name: &str, r: usize, effective_max_retries: usize) -> String {
    format!(
        "group '{group_name}' has max_retries: {r}, which can never take effect under \
         the effective global loop-protection ceiling of {effective_max_retries} \
         (a group's max_retries must not exceed the ceiling). Either lower \
         groups.{group_name}.max_retries to at most {effective_max_retries} or raise \
         the ceiling via `--max-retries {r}` / config `max_retries: {r}`"
    )
}

/// Error message for case 2: the retry target lands outside the group, so
/// one extra sequential edge is counted every retry cycle and the effective
/// required budget is `R + 1`, not `R`.
fn unreachable_group_message_external_target(
    group_name: &str,
    r: usize,
    target: &str,
    effective_max_retries: usize,
) -> String {
    let r_plus_1 = r + 1;
    let g_minus_1 = effective_max_retries.saturating_sub(1);
    format!(
        "group '{group_name}' has max_retries: {r}, but its if.file-changed retry target \
         '{target}' is outside the group, so each retry cycle counts one extra sequential \
         edge (the jump from '{target}' back into the group) on top of the group's own \
         internal edges -- effectively requiring a budget of {r} + 1 = {r_plus_1} under the \
         effective global loop-protection ceiling of {effective_max_retries}. Either lower \
         groups.{group_name}.max_retries to at most {g_minus_1} or raise the ceiling via \
         `--max-retries {r_plus_1}` / config `max_retries: {r_plus_1}`"
    )
}

/// Validate that no step cycle mixes unsafe conditional edges
/// (`if.file-changed` jumps and `if.fail` goto targets) with unconditional
/// sequential edges among the top-level `steps`. After-pr steps run through
/// the same loop protection, but are deliberately out of scope here: the
/// built-in config's own after-pr CI-retry loop is such a mixed cycle and
/// relies on runtime loop protection.
///
/// Such a cycle always deadlocks under loop protection: once the conditional
/// back-edge exhausts its retries (`max_retries`), the unconditional edge needs
/// `max_retries` + 1 traversals, which always exceeds any ceiling G.
///
/// # Errors
///
/// Returns an error naming the witness cycle when a mixed
/// conditional/unconditional cycle exists.
pub fn validate_mixed_conditional_cycles(config: &WorkflowConfig) -> crate::error::Result<()> {
    let steps = &config.steps;
    let groups = &config.groups;
    let names: Vec<&str> = steps.keys().map(String::as_str).collect();
    let index_of: HashMap<&str, usize> = names
        .iter()
        .copied()
        .enumerate()
        .map(|(i, name)| (name, i))
        .collect();
    let edges = build_step_edges(steps, &names, &index_of, groups);

    // Strongly connected components via mutual reachability (step counts are
    // practically small, so this naive computation is sufficient); the
    // component id is the smallest node index in the component.
    let n = names.len();
    let mut reach = vec![vec![false; n]; n];
    for (u, row) in reach.iter_mut().enumerate() {
        let mut stack = vec![u];
        row[u] = true;
        while let Some(v) = stack.pop() {
            for &(w, _) in &edges[v] {
                if !row[w] {
                    row[w] = true;
                    stack.push(w);
                }
            }
        }
    }
    let mut component = vec![usize::MAX; n];
    for (u, row) in reach.iter().enumerate() {
        for (v, &fwd) in row.iter().enumerate().skip(u) {
            if fwd && reach[v][u] {
                component[v] = component[v].min(u);
            }
        }
    }

    for root in 0..n {
        // The component id is the smallest member index, so a root that is not
        // its own id belongs to an already-visited (smaller) component.
        if component[root] != root {
            continue;
        }
        let in_component = |v: usize| component[v] == root;
        let members: Vec<usize> = (root..n).filter(|&v| in_component(v)).collect();
        if members.len() < 2 {
            // Single-node components cannot mix edge kinds here by design:
            // conditional self-edges (if.no-file-changes.retry / if.fail.retry)
            // are excluded from the graph, and an unconditional self-loop is a
            // pure unconditional cycle left to runtime loop protection.
            continue;
        }
        let mut has_unconditional = false;
        let mut cond_edge = None;
        for &v in &members {
            for &(w, kind) in &edges[v] {
                if !in_component(w) {
                    continue;
                }
                match kind {
                    Some(CycleEdgeKind::Unconditional) => has_unconditional = true,
                    Some(CycleEdgeKind::Conditional) if cond_edge.is_none() => {
                        cond_edge = Some((v, w));
                    }
                    _ => {}
                }
            }
        }
        let Some(witness) = cond_edge
            .filter(|_| has_unconditional)
            .and_then(|(u, w)| mixed_cycle_witness(&edges, &names, in_component, u, w))
        else {
            // Not a mixed component (purely unconditional cycles stay under
            // runtime loop protection); a missing witness is unreachable when
            // a conditional edge exists, but never reject-silently.
            continue;
        };
        return Err(crate::error::CruiseError::InvalidStepConfig(format!(
            "top-level steps form a cycle that mixes conditional and unconditional edges: \
             {witness}. Once the conditional back-edge (if.file-changed / if.fail goto) has \
             fired max_retries times under loop protection, the unconditional sequential edge \
             needs one more traversal than the ceiling allows and always fails with \
             LoopProtection, whatever the ceiling is. Confine the cycle inside a group under \
             `groups:` with a `max_retries` so exhausted retries degrade into a graceful skip \
             instead -- see the built-in config's groups.verify-review for an example"
        )));
    }
    Ok(())
}

/// Build the outgoing edge lists for the cycle-detection graph.
///
/// Each entry is `(target, kind)`; a `None` kind marks a group-retry back-edge
/// whose exhaustion degrades into a graceful skip (already budget-checked by
/// [`validate_group_retry_budget`]), so it counts as neither edge kind below.
fn build_step_edges(
    steps: &IndexMap<String, StepConfig>,
    names: &[&str],
    index_of: &HashMap<&str, usize>,
    groups: &HashMap<String, GroupConfig>,
) -> Vec<StepEdgeList> {
    let mut edges: Vec<StepEdgeList> = vec![Vec::new(); names.len()];
    for (i, step) in steps.values().enumerate() {
        // Unconditional edge: explicit `next`, else the next step in YAML order.
        // On an option step this edge is reachable at runtime only when some
        // choice leaves `next` unset (the selected choice takes priority over
        // the sequential/explicit edge); when every choice carries an explicit
        // `next`, emitting it would create false positives.
        let has_open_choice = step
            .option
            .as_ref()
            .is_some_and(|items| items.iter().any(|item| item.next.is_none()));
        if step.option.is_none() || has_open_choice {
            let sequential = step
                .next
                .as_deref()
                .or_else(|| names.get(i + 1).copied())
                .and_then(|target| index_of.get(target).copied());
            if let Some(target) = sequential {
                edges[i].push((target, Some(CycleEdgeKind::Unconditional)));
            }
        }

        // Option-item `next` edges are user-driven interactive choices,
        // out of scope for this check; nothing else on option steps is read.
        if step.option.is_some() {
            continue;
        }

        if let Some(group_name) = step.group.as_deref() {
            // Group call step: its only conditional edge is the group's own
            // if.file-changed back-edge. With `max_retries` set, exhaustion
            // degrades into a graceful skip (budget-checked by
            // [`validate_group_retry_budget`]), so it counts as neither kind;
            // without it the jump fires unboundedly with no skip -- exactly
            // like a plain conditional edge.
            if let Some(group) = groups.get(group_name)
                && let Some(target) = group
                    .if_condition
                    .as_ref()
                    .and_then(|cond| cond.file_changed.as_deref())
                    .and_then(|target| index_of.get(target).copied())
            {
                edges[i].push((
                    target,
                    group
                        .max_retries
                        .map_or(Some(CycleEdgeKind::Conditional), |_| None),
                ));
            }
        } else if let Some(cond) = &step.if_condition {
            if let Some(target) = cond
                .file_changed
                .as_deref()
                .and_then(|target| index_of.get(target).copied())
            {
                edges[i].push((target, Some(CycleEdgeKind::Conditional)));
            }
            if let Some(FailAction::Goto(target)) = &cond.fail
                && let Some(&goto_target) = index_of.get(target.as_str())
            {
                edges[i].push((goto_target, Some(CycleEdgeKind::Conditional)));
            }
            // if.no-file-changes (retry/fail) and if.fail `{retry: true}` are
            // single-node self-retries or aborts; they never form part of a cycle.
        }
    }
    edges
}

/// Classification of a top-level step-graph edge used by
/// [`validate_mixed_conditional_cycles`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CycleEdgeKind {
    /// Sequential fall-through or explicit `next`: consumed every time it is reached.
    Unconditional,
    /// `if.file-changed` jump or `if.fail` goto: only taken while the condition fires,
    /// so its traversals are bounded by loop protection's retry ceiling.
    Conditional,
}

/// Outgoing edges of one step in the cycle-detection graph.
type StepEdgeList = Vec<(usize, Option<CycleEdgeKind>)>;

/// Name a witness cycle containing a conditional edge: an SCC can also embed
/// purely unconditional sub-cycles, and naming one would contradict the error
/// explanation. For the in-component conditional edge `u -> w`, close the
/// cycle with a shortest in-component path `w -> .. -> u` (BFS parents),
/// closed back onto `w`. Returns `None` only if `w` cannot reach `u`, which
/// same-SCC membership makes unreachable.
fn mixed_cycle_witness(
    edges: &[StepEdgeList],
    names: &[&str],
    in_component: impl Fn(usize) -> bool,
    u: usize,
    w: usize,
) -> Option<String> {
    // BFS from w to u within the component; u is guaranteed reachable
    // from w because both sit in the same SCC.
    let mut prev: HashMap<usize, usize> = HashMap::from([(w, w)]);
    let mut queue = std::collections::VecDeque::from([w]);
    while let Some(v) = queue.pop_front() {
        if v == u {
            break;
        }
        for &(x, _) in &edges[v] {
            if in_component(x) && !prev.contains_key(&x) {
                prev.insert(x, v);
                queue.push_back(x);
            }
        }
    }
    // Walk parents from u back to w, print forward [w, .., u], and close the
    // loop onto w.
    let mut walked = vec![u];
    while *walked.last()? != w {
        walked.push(*prev.get(walked.last()?)?);
    }
    walked.reverse(); // [u, ..., w] -> [w, ..., u]
    walked.push(w);
    Some(
        walked
            .iter()
            .map(|&v| names[v])
            .collect::<Vec<_>>()
            .join(" -> "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, err_string, lock_process, mixed_conditional_cycle_config};

    const SAMPLE_YAML: &str = r#"
command:
  - claude
  - -p

steps:
  planning:
    model: claude-opus-4-5
    instruction: "You are a senior engineer."
    prompt: "Plan the implementation of: {input}"

  review_plan:
    plan: "{plan}"
    option:
      - selector: "Approve and continue"
        next: implement
      - selector: "Revise the plan"
        next: planning
      - text-input: "Other (text input)"
        next: planning

  implement:
    prompt: "Implement based on the plan: {plan}"

  run_tests:
    command: cargo test

  commit:
    command: "git commit -am 'feat: {input}'"
    if:
      file-changed: implement
"#;

    #[test]
    fn test_parse_workflow_config() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.command, vec!["claude", "-p"]);
        assert_eq!(config.model, None);
        assert_eq!(config.plan_model, None);
        assert_eq!(config.pr_language, None);
        assert_eq!(config.plan_language, None);
        assert_eq!(config.effective_pr_language(), DEFAULT_PR_LANGUAGE);
        assert_eq!(config.effective_plan_language(), DEFAULT_PLAN_LANGUAGE);
    }

    #[test]
    fn test_plan_model_field() {
        let yaml = r"
command: [claude, -p]
model: sonnet
plan_model: opus
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert_eq!(config.plan_model, Some("opus".to_string()));
    }
    #[test]
    fn test_pr_language_field() {
        let yaml = r"
command: [claude, -p]
pr_language: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.pr_language, Some("Japanese".to_string()));
        assert_eq!(config.effective_pr_language(), "Japanese");
    }

    #[test]
    fn test_pr_language_defaults_to_english_when_omitted() {
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.pr_language, None);
        assert_eq!(config.effective_pr_language(), DEFAULT_PR_LANGUAGE);
    }

    #[test]
    fn test_plan_language_field() {
        // Given: workflow YAML configures a planning language
        let yaml = r"
command: [claude, -p]
plan_language: Japanese
steps:
  s1:
    command: echo hi
";

        // When: the workflow is parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the configured planning language is preserved
        assert_eq!(config.plan_language, Some("Japanese".to_string()));
        assert_eq!(config.effective_plan_language(), "Japanese");
    }

    #[test]
    fn test_plan_language_defaults_to_english_when_omitted() {
        // Given: workflow YAML omits plan_language
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";

        // When: the workflow is parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the built-in English default is used
        assert_eq!(config.plan_language, None);
        assert_eq!(config.effective_plan_language(), DEFAULT_PLAN_LANGUAGE);
    }
    #[test]
    fn test_languages_pr_field() {
        let yaml = r"
command: [claude, -p]
languages:
  pr: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            config.languages.as_ref().and_then(|l| l.pr.as_deref()),
            Some("Japanese")
        );
        assert_eq!(config.effective_pr_language(), "Japanese");
        assert!(config.deprecated_language_warnings().is_empty());
    }

    #[test]
    fn test_languages_plan_field() {
        let yaml = r"
command: [claude, -p]
languages:
  plan: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            config.languages.as_ref().and_then(|l| l.plan.as_deref()),
            Some("Japanese")
        );
        assert_eq!(config.effective_plan_language(), "Japanese");
        assert!(config.deprecated_language_warnings().is_empty());
    }

    #[test]
    fn test_languages_pr_takes_precedence_over_pr_language() {
        let yaml = r"
command: [claude, -p]
pr_language: English
languages:
  pr: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.effective_pr_language(), "Japanese");
        let warnings = config.deprecated_language_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("deprecated") && w.contains("pr_language"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("ignored") && w.contains("pr_language"))
        );
    }

    #[test]
    fn test_languages_plan_takes_precedence_over_plan_language() {
        let yaml = r"
command: [claude, -p]
plan_language: English
languages:
  plan: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.effective_plan_language(), "Japanese");
        let warnings = config.deprecated_language_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("deprecated") && w.contains("plan_language"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("ignored") && w.contains("plan_language"))
        );
    }

    #[test]
    fn test_warn_deprecated_emits_for_legacy_fields() {
        let yaml = r"
command: [claude, -p]
pr_language: Japanese
plan_language: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let warnings = config.deprecated_language_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("pr_language") && w.contains("deprecated"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("plan_language") && w.contains("deprecated"))
        );
        assert!(!warnings.iter().any(|w| w.contains("ignored")));
    }

    #[test]
    fn test_warn_deprecated_silent_when_new_keys_only() {
        let yaml = r"
command: [claude, -p]
languages:
  pr: Japanese
  plan: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.deprecated_language_warnings().is_empty());
    }

    #[test]
    fn test_effective_language_trims_and_defaults_blank() {
        let yaml = r#"
command: [claude, -p]
languages:
  pr: "   "
  plan: "   "
steps:
  s1:
    command: echo hi
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.effective_pr_language(), DEFAULT_PR_LANGUAGE);
        assert_eq!(config.effective_plan_language(), DEFAULT_PLAN_LANGUAGE);
    }

    #[test]
    fn test_cleanup_after_pr_field() {
        // Given: workflow YAML enables post-PR cleanup
        let yaml = r"
command: [claude, -p]
cleanup_after_pr: true
steps:
  s1:
    command: echo hi
";

        // When: the workflow is parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the field is true
        assert!(config.cleanup_after_pr);
    }

    #[test]
    fn test_cleanup_after_pr_defaults_to_false_when_omitted() {
        // Given: workflow YAML omits cleanup_after_pr
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";

        // When: the workflow is parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the field defaults to false (non-destructive)
        assert!(!config.cleanup_after_pr);
    }

    #[test]
    fn test_force_exec_field() {
        // Given: workflow YAML enables direct execution without the exec subcommand
        let yaml = r"
command: [claude, -p]
force_exec: true
steps:
  s1:
    command: echo hi
";

        // When: the workflow is parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: force_exec is enabled
        assert!(config.force_exec);
    }

    #[test]
    fn test_force_exec_defaults_to_false_when_omitted() {
        // Given: workflow YAML omits force_exec
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";

        // When: the workflow is parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: force_exec defaults to false
        assert!(!config.force_exec);
    }

    #[test]
    fn test_builtin_config_yaml_parses_and_validates() {
        // Given / When: the embedded built-in config YAML is parsed
        let config = WorkflowConfig::from_yaml(BUILTIN_CONFIG_YAML)
            .unwrap_or_else(|e| panic!("built-in config YAML must parse: {e}"));

        // Then: it has the expected built-in defaults (source: builtin/cruise.yaml)
        assert_eq!(config.sdk.as_deref(), Some("seher"));
        assert_eq!(config.model.as_deref(), Some("build"));
        assert_eq!(config.plan_model.as_deref(), Some("plan"));
        assert_eq!(
            config.languages.as_ref().and_then(|l| l.plan.as_deref()),
            None
        );
        assert_eq!(
            config.languages.as_ref().and_then(|l| l.pr.as_deref()),
            Some("English")
        );
        assert!(config.cleanup_after_pr);
        // max_retries is unset so DEFAULT_MAX_RETRIES governs
        assert_eq!(config.max_retries, None);
        assert!(config.steps.contains_key("write-test-first"));
        assert!(config.steps.contains_key("implement-after-tests"));
        assert!(config.groups.contains_key("verify-review"));
        // after-pr automation must not auto-merge: merging stays a human action
        assert!(!config.after_pr.contains_key("merge"));

        // And: the review group ends with a fixing review pass after the
        // verification and simplification steps.
        let review = config
            .groups
            .get("verify-review")
            .unwrap_or_else(|| panic!("built-in config must define the 'verify-review' group"));
        let order: Vec<&str> = review
            .steps
            .keys()
            .map(std::string::String::as_str)
            .collect();
        assert_eq!(
            order,
            vec![
                "verify-plan-implementation",
                "verify-wiring",
                "verify-docs",
                "simplify-pass",
                "review-pass"
            ]
        );

        // And: review fixes re-enter the flow at plan verification, so a
        // review-driven change is re-checked against {plan} before wiring.
        assert_eq!(
            review
                .if_condition
                .as_ref()
                .and_then(|c| c.file_changed.as_deref()),
            Some("verify-review-pass/verify-plan-implementation")
        );

        // And: it passes full config validation
        validate_config(&config).unwrap_or_else(|e| panic!("built-in config invalid: {e}"));
    }

    #[test]
    fn test_step_order_preserved() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let step_names: Vec<&str> = config
            .steps
            .keys()
            .map(std::string::String::as_str)
            .collect();
        assert_eq!(
            step_names,
            vec![
                "planning",
                "review_plan",
                "implement",
                "run_tests",
                "commit"
            ]
        );
    }

    #[test]
    fn test_prompt_step_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let planning = config
            .steps
            .get("planning")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(planning.model, Some("claude-opus-4-5".to_string()));
        assert_eq!(
            planning.instruction,
            Some("You are a senior engineer.".to_string())
        );
        assert!(planning.prompt.is_some());
    }

    #[test]
    fn test_command_step_single() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let run_tests = config
            .steps
            .get("run_tests")
            .unwrap_or_else(|| panic!("unexpected None"));
        match run_tests
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"))
        {
            StringOrVec::Single(s) => assert_eq!(s, "cargo test"),
            StringOrVec::Multiple(_) => panic!("Expected Single command"),
        }
    }

    #[test]
    fn test_command_list_field() {
        let yaml = r"
command: [claude, -p]
steps:
  multi:
    command:
      - cargo fmt
      - cargo test
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("multi")
            .unwrap_or_else(|| panic!("unexpected None"));
        match step
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"))
        {
            StringOrVec::Multiple(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds[0], "cargo fmt");
                assert_eq!(cmds[1], "cargo test");
            }
            StringOrVec::Single(_) => panic!("Expected Multiple commands"),
        }
    }

    #[test]
    fn test_option_step_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let review = config
            .steps
            .get("review_plan")
            .unwrap_or_else(|| panic!("unexpected None"));
        let options = review
            .option
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(options.len(), 3);
        assert_eq!(
            options[0].selector,
            Some("Approve and continue".to_string())
        );
        assert_eq!(options[0].next, Some("implement".to_string()));
        assert_eq!(options[1].next, Some("planning".to_string()));
        assert_eq!(
            options[2].text_input,
            Some("Other (text input)".to_string())
        );
        assert_eq!(options[2].next, Some("planning".to_string()));
    }

    #[test]
    fn test_if_condition_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let commit = config
            .steps
            .get("commit")
            .unwrap_or_else(|| panic!("unexpected None"));
        let if_cond = commit
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(if_cond.file_changed, Some("implement".to_string()));
    }

    #[test]
    fn test_skip_static_field() {
        let yaml = r"
command: [claude, -p]
steps:
  optional_step:
    command: cargo fmt
    skip: true
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("optional_step")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert!(matches!(step.skip, Some(SkipCondition::Static(true))));
    }

    #[test]
    fn test_skip_variable_field() {
        let yaml = r"
command: [claude, -p]
steps:
  conditional_skip:
    command: cargo fmt
    skip: prev.success
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("conditional_skip")
            .unwrap_or_else(|| panic!("unexpected None"));
        match &step.skip {
            Some(SkipCondition::Variable(name)) => assert_eq!(name, "prev.success"),
            _ => panic!("Expected Variable skip condition"),
        }
    }

    #[test]
    fn test_top_level_env() {
        let yaml = r"
command: [claude, -p]
env:
  ANTHROPIC_API_KEY: sk-test
  PROJECT_NAME: myproject
steps:
  step1:
    command: echo hello
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            config.env.get("ANTHROPIC_API_KEY"),
            Some(&"sk-test".to_string())
        );
        assert_eq!(
            config.env.get("PROJECT_NAME"),
            Some(&"myproject".to_string())
        );
    }

    #[test]
    fn test_step_level_env() {
        let yaml = r"
command: [claude, -p]
steps:
  build:
    command: cargo build
    env:
      RUST_LOG: debug
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let build = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(build.env.get("RUST_LOG"), Some(&"debug".to_string()));
    }

    #[test]
    fn test_env_defaults_empty() {
        let yaml = r"
command: [claude, -p]
steps:
  step1:
    command: echo hello
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.env.is_empty());
        let step = config
            .steps
            .get("step1")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert!(step.env.is_empty());
    }

    // --- timeout deserialization tests ---

    #[test]
    fn test_step_timeout_parses_plain_digits() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    timeout: '30'
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("step not found"));
        assert_eq!(step.timeout.as_deref(), Some("30"));
    }

    #[test]
    fn test_step_timeout_parses_minutes() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    timeout: 5m
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("step not found"));
        assert_eq!(step.timeout.as_deref(), Some("5m"));
    }

    #[test]
    fn test_step_timeout_parses_hours() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    timeout: 1h
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("step not found"));
        assert_eq!(step.timeout.as_deref(), Some("1h"));
    }

    #[test]
    fn test_step_timeout_defaults_none() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("step not found"));
        assert!(step.timeout.is_none(), "timeout should default to None");
    }

    #[test]
    fn test_minimal_config() {
        let yaml = r#"
command: [claude, -p]
steps:
  only_step:
    prompt: "Hello {input}"
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.steps.len(), 1);
    }

    #[test]
    fn test_parse_cruise_yaml() {
        let yaml = BUILTIN_CONFIG_YAML;
        let config = WorkflowConfig::from_yaml(yaml)
            .unwrap_or_else(|e| panic!("failed to parse cruise.yaml: {e:?}"));
        assert_eq!(config.sdk, Some("seher".to_string()));
        assert!(
            config.command.is_empty(),
            "command should be empty when sdk is set"
        );
        assert_eq!(config.model, Some("build".to_string()));
        assert_eq!(config.plan_model, Some("plan".to_string()));
        assert!(!config.steps.is_empty(), "steps is empty");
        assert!(
            config.steps.contains_key("mise-trust"),
            "expected mise-trust step"
        );
    }

    #[test]
    fn test_empty_steps() {
        let yaml = "command: [echo]\nsteps: {}";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.steps.is_empty());
    }

    #[test]
    fn test_missing_steps_error() {
        let yaml = "command: [echo]";
        let result = WorkflowConfig::from_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_type_mismatch() {
        let yaml = "command: [echo]\nsteps:\n  s1:\n    command: {foo: bar}";
        let result = WorkflowConfig::from_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_fields_ignored() {
        // Old configs with `state` or `worktree` fields should still parse.
        let yaml = "command: [echo]\nworktree: true\nstate: .cruise/state.json\nsteps:\n  s1:\n    command: echo hi";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!config.steps.is_empty());
    }

    #[test]
    fn test_group_config_parse() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: test
    max_retries: 3
steps:
  test:
    command: cargo test
  simplify:
    group: review
    prompt: /simplify
  ai-antipattern:
    group: review
    prompt: /ai-antipattern
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.groups.contains_key("review"));
        let review = &config.groups["review"];
        assert_eq!(review.max_retries, Some(3));
        assert!(review.if_condition.is_some());
        assert_eq!(
            review
                .if_condition
                .as_ref()
                .unwrap_or_else(|| panic!("unexpected None"))
                .file_changed,
            Some("test".to_string())
        );
        let simplify = config
            .steps
            .get("simplify")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(simplify.group, Some("review".to_string()));
    }

    #[test]
    fn test_validate_groups_ok() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 2
    steps:
      simplify:
        prompt: /simplify
      ai-antipattern:
        prompt: /ai-antipattern
steps:
  build:
    command: cargo build
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(validate_groups(&config).is_ok());
    }

    #[test]
    fn test_validate_groups_undefined_group() {
        let yaml = r"
command: [claude, -p]
groups: {}
steps:
  step1:
    group: nonexistent
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(err_string(result).contains("undefined group"));
    }

    #[test]
    fn test_validate_groups_multiple_call_sites_ok() {
        // New-style: same group invoked from multiple non-consecutive call sites is valid
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 2
    steps:
      simplify:
        prompt: /simplify
steps:
  test1:
    command: cargo test --lib
  review-after-lib:
    group: review
  test2:
    command: cargo test --doc
  review-after-doc:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(validate_groups(&config).is_ok());
    }

    #[test]
    fn test_validate_groups_step_has_individual_if() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 2
    steps:
      step1:
        command: echo hi
steps:
  call-review:
    group: review
    if:
      file-changed: step1
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(err_string(result).contains("individual 'if'"));
    }

    #[test]
    fn test_validate_groups_rejects_old_membership_style() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  review-pass:
    group: review
    prompt: /legacy
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_groups(&config);
        assert!(result.is_err());
        let msg = err_string(result);
        assert!(
            msg.contains("old membership style") || msg.contains("groups.<name>.steps"),
            "expected migration hint in: {msg}"
        );
    }

    #[test]
    fn test_validate_groups_rejects_prompt_file_old_membership_style() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  review-pass:
    group: review
    prompt_file: prompts/review.md
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(err_string(result).contains("old membership style"));
    }

    #[test]
    fn test_validate_groups_rejects_empty_group() {
        let yaml = r"
command: [echo]
groups:
  review:
    steps: {}
steps:
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(
            err_string(result).contains("empty"),
            "expected empty-group error"
        );
    }

    #[test]
    fn test_after_pr_field_parse() {
        // Given: YAML with after-pr steps containing pr.number / pr.url placeholders
        let yaml = r#"
command: [claude, -p]
steps:
  implement:
    prompt: "Implement: {input}"
  test:
    command: cargo test
after-pr:
  notify:
    command: "echo 'PR #{pr.number} created: {pr.url}'"
  label:
    command: "gh pr edit {pr.number} --add-label enhancement"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: after_pr has 2 steps in order
        assert_eq!(config.after_pr.len(), 2);
        let keys: Vec<&str> = config
            .after_pr
            .keys()
            .map(std::string::String::as_str)
            .collect();
        assert_eq!(keys, vec!["notify", "label"]);
    }

    #[test]
    fn test_after_pr_field_default_empty() {
        // Given: YAML without after-pr field
        let yaml = r#"
command: [claude, -p]
steps:
  implement:
    prompt: "Implement: {input}"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: after_pr defaults to empty IndexMap
        assert!(config.after_pr.is_empty());
    }

    #[test]
    fn test_after_pr_step_fields() {
        // Given: YAML where after-pr step uses command field
        let yaml = r#"
command: [claude, -p]
steps:
  build:
    command: cargo build
after-pr:
  notify:
    command: "echo done"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: after_pr step has the command field set
        let notify = config
            .after_pr
            .get("notify")
            .unwrap_or_else(|| panic!("unexpected None"));
        match notify
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"))
        {
            StringOrVec::Single(s) => assert_eq!(s, "echo done"),
            StringOrVec::Multiple(_) => panic!("Expected Single command"),
        }
    }

    // --- New group schema: groups.<name>.steps ---

    #[test]
    fn test_group_config_with_steps_parse() {
        // Given: YAML with groups that define steps inside them
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: group has steps with correct count and order
        let review = &config.groups["review"];
        assert_eq!(review.max_retries, Some(3));
        assert_eq!(review.steps.len(), 2);
        let step_names: Vec<&str> = review
            .steps
            .keys()
            .map(std::string::String::as_str)
            .collect();
        assert_eq!(step_names, vec!["simplify", "coderabbit"]);
    }

    #[test]
    fn test_group_call_step_parse() {
        // Given: YAML where a top-level step is a pure group call (no prompt/command)
        let yaml = r"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: group call step only has group set
        let review_pass = config
            .steps
            .get("review-pass")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(review_pass.group, Some("review".to_string()));
        assert!(review_pass.prompt.is_none());
        assert!(review_pass.command.is_none());
    }

    #[test]
    fn test_group_call_same_group_multiple_call_sites_parse() {
        // Given: YAML where same group is invoked from two different top-level steps
        let yaml = r"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  test1:
    command: cargo test --lib
  review-after-lib:
    group: review
  test2:
    command: cargo test --doc
  review-after-doc:
    group: review
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: both call sites reference the same group
        assert_eq!(
            config.steps["review-after-lib"].group,
            Some("review".to_string())
        );
        assert_eq!(
            config.steps["review-after-doc"].group,
            Some("review".to_string())
        );
        // And: step order in top-level steps is preserved
        let keys: Vec<&str> = config
            .steps
            .keys()
            .map(std::string::String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["test1", "review-after-lib", "test2", "review-after-doc"]
        );
    }

    // --- if.no-file-changes parse tests ---

    #[test]
    fn test_if_no_file_changes_failed_parses() {
        // Given: a step with if.no-file-changes: failed
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes: failed
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the no_file_changes condition holds the `failed` action
        let nfc = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("step not found"))
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"))
            .no_file_changes
            .as_ref()
            .unwrap_or_else(|| panic!("no_file_changes not set"));
        assert_eq!(*nfc, NoFileChangesAction::Failed);
    }

    #[test]
    fn test_if_no_file_changes_retry_parses() {
        // Given: a step with if.no-file-changes: retry
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes: retry
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the no_file_changes condition holds the `retry` action
        let nfc = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("step not found"))
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"))
            .no_file_changes
            .as_ref()
            .unwrap_or_else(|| panic!("no_file_changes not set"));
        assert_eq!(*nfc, NoFileChangesAction::Retry);
    }

    #[test]
    fn test_if_no_file_changes_and_file_changed_coexist_in_parse() {
        // Given: a step with both if.file-changed and if.no-file-changes
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      file-changed: implement
      no-file-changes: retry
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: both fields are present
        let implement = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("step not found"));
        let if_cond = implement
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"));
        assert_eq!(if_cond.file_changed, Some("implement".to_string()));
        assert_eq!(if_cond.no_file_changes, Some(NoFileChangesAction::Retry));
    }

    #[test]
    fn test_if_no_file_changes_rejects_legacy_object_form() {
        // Given: a step using the removed object form { fail: true }
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes:
        fail: true
";
        // When: parsed
        let result = WorkflowConfig::from_yaml(yaml);
        // Then: parsing fails -- the object form was removed without backward compatibility
        assert!(
            result.is_err(),
            "legacy object form must be rejected, got: {result:?}"
        );
    }

    #[test]
    fn test_if_no_file_changes_rejects_removed_legacy_field() {
        // Given: a step using the removed top-level legacy field
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    fail-if-no-file-changes: true
";
        // When: parsed
        let result = WorkflowConfig::from_yaml(yaml);
        // Then: parsing fails instead of silently ignoring the removed field
        let err = result.map_or_else(|e| e, |_| panic!("removed legacy field must be rejected"));
        assert!(
            err.to_string().contains("fail-if-no-file-changes"),
            "error should name the removed field, got: {err}"
        );
    }

    #[test]
    fn test_if_no_file_changes_serializes_as_lowercase_string() {
        // Given: each supported action
        // When: serialized for a workflow config
        let retry =
            serde_yaml::to_value(NoFileChangesAction::Retry).unwrap_or_else(|e| panic!("{e:?}"));
        let failed =
            serde_yaml::to_value(NoFileChangesAction::Failed).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: actions use the public YAML strings, not Rust variant names or objects
        assert_eq!(retry.as_str(), Some("retry"));
        assert_eq!(failed.as_str(), Some("failed"));
    }

    #[test]
    fn test_if_no_file_changes_rejects_invalid_value() {
        // Given: a step with an unknown no-file-changes value ('fail' instead of 'failed')
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes: fail
";
        // When: parsed
        let result = WorkflowConfig::from_yaml(yaml);
        // Then: parsing fails because only 'retry' and 'failed' are accepted
        assert!(
            result.is_err(),
            "invalid no-file-changes value must be rejected, got: {result:?}"
        );
    }

    // --- if.fail deserialization tests ---

    #[test]
    fn test_if_fail_string_form_parses() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    if:
      fail: rollback
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("step not found"));
        let if_cond = step
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"));
        match if_cond
            .fail
            .as_ref()
            .unwrap_or_else(|| panic!("fail not set"))
        {
            FailAction::Goto(name) => assert_eq!(name, "rollback"),
            FailAction::Detailed(_) => panic!("Expected FailAction::Goto"),
        }
    }

    #[test]
    fn test_if_fail_retry_object_form_parses() {
        let yaml = r"
command: [echo]
steps:
  flaky:
    command: ./flaky-test.sh
    if:
      fail:
        retry: true
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("flaky")
            .unwrap_or_else(|| panic!("step not found"));
        let if_cond = step
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"));
        match if_cond
            .fail
            .as_ref()
            .unwrap_or_else(|| panic!("fail not set"))
        {
            FailAction::Detailed(d) => {
                assert!(d.retry, "retry should be true");
            }
            FailAction::Goto(_) => panic!("Expected FailAction::Detailed"),
        }
    }

    #[test]
    fn test_if_fail_defaults_none() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    if:
      file-changed: implement
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("step not found"));
        let if_cond = step
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"));
        assert!(if_cond.fail.is_none(), "fail should default to None");
    }

    // --- if.no-file-changes validation tests ---

    #[test]
    fn test_validate_if_conditions_rejects_no_file_changes_in_after_pr() {
        // Given: an after-pr step with if.no-file-changes: failed
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
after-pr:
  notify:
    command: echo done
    if:
      no-file-changes: failed
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: returns an error because no-file-changes in after-pr is unsupported
        assert!(
            result.is_err(),
            "expected Err for after-pr + no-file-changes"
        );
        let msg = err_string(result);
        assert!(
            msg.contains("after-pr") || msg.contains("notify"),
            "error should mention after-pr step, got: {msg}"
        );
    }

    #[test]
    fn test_validate_if_conditions_rejects_no_file_changes_in_group_if() {
        // Given: a group with if.no-file-changes set (group-level no-file-changes is unsupported)
        let yaml = r"
command: [echo]
groups:
  review:
    if:
      no-file-changes: failed
    steps:
      simplify:
        prompt: /simplify
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: returns an error because no-file-changes in group-level if is unsupported
        assert!(
            result.is_err(),
            "expected Err for group-level no-file-changes"
        );
        let msg = err_string(result);
        assert!(
            msg.contains("group") || msg.contains("review"),
            "error should mention group, got: {msg}"
        );
    }

    #[test]
    fn test_validate_if_conditions_ok_for_failed() {
        // Given: a step with if.no-file-changes: failed (valid)
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes: failed
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: no error
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
    }

    #[test]
    fn test_validate_if_conditions_ok_for_retry() {
        // Given: a step with if.no-file-changes: retry (valid)
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes: retry
  done:
    command: echo done
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: no error
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
    }

    // --- timeout validation tests ---

    #[test]
    fn test_validate_rejects_invalid_timeout_string() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    timeout: abc
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_timeouts(&config);
        assert!(result.is_err(), "expected Err for invalid timeout 'abc'");
        let msg = err_string(result);
        assert!(
            msg.contains("timeout"),
            "error should mention timeout, got: {msg}"
        );
    }

    #[test]
    fn test_validate_rejects_zero_timeout() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    timeout: '0'
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_timeouts(&config);
        assert!(result.is_err(), "expected Err for zero timeout");
    }

    #[test]
    fn test_validate_accepts_valid_timeout() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    timeout: '30'
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_timeouts(&config);
        assert!(
            result.is_ok(),
            "expected Ok for valid timeout, got: {result:?}"
        );
    }

    #[test]
    fn test_validate_accepts_timeout_with_suffix() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    timeout: 5m
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_timeouts(&config);
        assert!(result.is_ok(), "expected Ok for '5m', got: {result:?}");
    }

    // --- if.fail validation tests ---

    #[test]
    fn test_validate_rejects_if_fail_in_after_pr() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
after-pr:
  notify:
    command: echo done
    if:
      fail: rollback
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        assert!(result.is_err(), "expected Err for if.fail in after-pr");
        let msg = err_string(result);
        assert!(
            msg.contains("after-pr") || msg.contains("notify"),
            "error should mention after-pr step, got: {msg}"
        );
    }

    #[test]
    fn test_validate_rejects_if_fail_at_group_level() {
        let yaml = r"
command: [echo]
groups:
  review:
    if:
      fail: rollback
    steps:
      simplify:
        prompt: /simplify
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        assert!(result.is_err(), "expected Err for if.fail at group level");
        let msg = err_string(result);
        assert!(
            msg.contains("group"),
            "error should mention group, got: {msg}"
        );
    }

    #[test]
    fn test_validate_accepts_if_fail_retry_only() {
        let yaml = r"
command: [echo]
steps:
  flaky:
    command: ./test.sh
    if:
      fail:
        retry: true
  done:
    command: echo done
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        assert!(
            result.is_ok(),
            "expected Ok for valid if.fail.retry, got: {result:?}"
        );
    }

    #[test]
    fn test_validate_accepts_if_fail_goto_only() {
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
    if:
      fail: rollback
  rollback:
    command: echo rolled back
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        assert!(
            result.is_ok(),
            "expected Ok for valid if.fail string, got: {result:?}"
        );
    }

    // --- JSON Schema tests ---

    fn load_schema() -> &'static serde_json::Value {
        use std::sync::OnceLock;
        static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
        SCHEMA.get_or_init(|| {
            serde_json::from_str(include_str!("../cruise-schema.json"))
                .unwrap_or_else(|e| panic!("cruise-schema.json is not valid JSON: {e}"))
        })
    }

    /// Returns the "properties" object from a `$defs/{def_name}` definition.
    fn def_properties<'a>(
        schema: &'a serde_json::Value,
        def_name: &str,
    ) -> &'a serde_json::Map<String, serde_json::Value> {
        schema["$defs"][def_name]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{def_name} properties not found in schema $defs"))
    }

    /// Asserts that all `expected_fields` exist as keys in `props`.
    fn assert_has_fields(
        props: &serde_json::Map<String, serde_json::Value>,
        expected_fields: &[&str],
        type_name: &str,
    ) {
        for field in expected_fields {
            assert!(
                props.contains_key(*field),
                "{type_name} schema must contain field '{field}'"
            );
        }
    }

    /// Asserts that `field_def` uses `oneOf` containing the given type variants.
    fn assert_oneof_types(
        field_def: &serde_json::Value,
        expected_types: &[&str],
        field_name: &str,
    ) {
        assert!(
            field_def.get("oneOf").is_some(),
            "{field_name} must use 'oneOf'; got: {field_def}"
        );
        let one_of = field_def["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{field_name} oneOf must be a JSON array"));
        for expected in expected_types {
            assert!(
                one_of.iter().any(|v| v["type"].as_str() == Some(expected)),
                "{field_name} oneOf must include '{expected}' variant"
            );
        }
    }

    #[test]
    fn test_schema_is_valid_json() {
        let schema = load_schema();
        assert!(schema.is_object(), "schema root must be a JSON object");
    }

    #[test]
    fn test_schema_has_meta_fields() {
        let schema = load_schema();
        assert!(
            schema.get("$schema").is_some(),
            "schema must have a $schema field"
        );
        assert_eq!(
            schema["type"].as_str(),
            Some("object"),
            "root type must be 'object'"
        );
        assert!(
            schema.get("properties").is_some(),
            "schema must have properties"
        );
    }

    #[test]
    fn test_schema_workflow_config_required_fields() {
        let schema = load_schema();
        let required = schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("schema must have a 'required' array"));
        // `command` is no longer unconditionally required: an `sdk`-backed config
        // omits it. Only `steps` is always required.
        assert!(
            required.iter().any(|v| v.as_str() == Some("steps")),
            "'steps' must be in required"
        );
        assert!(
            schema["properties"].get("sdk").is_some(),
            "schema must expose an 'sdk' property"
        );
    }

    #[test]
    fn test_schema_workflow_config_has_expected_properties() {
        let schema = load_schema();
        let props = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("schema must have a 'properties' object"));
        assert_has_fields(
            props,
            &[
                "command",
                "model",
                "plan_model",
                "interactive_planning",
                "pr_language",
                "plan_language",
                "languages",
                "env",
                "force_exec",
                "groups",
                "steps",
                "after-pr",
            ],
            "WorkflowConfig",
        );
    }

    #[test]
    fn test_schema_command_is_array_of_strings() {
        let schema = load_schema();
        let command_prop = &schema["properties"]["command"];
        assert_eq!(
            command_prop["type"].as_str(),
            Some("array"),
            "command must have type 'array'"
        );
        assert_eq!(
            command_prop["items"]["type"].as_str(),
            Some("string"),
            "command items must have type 'string'"
        );
    }

    fn assert_object_map_property(schema: &serde_json::Value, prop_name: &str) {
        let prop = &schema["properties"][prop_name];
        assert_eq!(
            prop["type"].as_str(),
            Some("object"),
            "{prop_name} must have type 'object'"
        );
        assert!(
            prop.get("additionalProperties").is_some(),
            "{prop_name} must define additionalProperties"
        );
    }

    #[test]
    fn test_schema_steps_is_object_with_step_config() {
        let schema = load_schema();
        assert_object_map_property(schema, "steps");
    }

    #[test]
    fn test_schema_step_config_has_expected_properties() {
        let schema = load_schema();
        let step_props = def_properties(schema, "StepConfig");
        assert_has_fields(
            step_props,
            &[
                "model",
                "prompt",
                "instruction",
                "plan",
                "option",
                "command",
                "next",
                "skip",
                "when",
                "if",
                "env",
                "group",
                "workflow_call",
                "timeout",
                "prompt_file",
            ],
            "StepConfig",
        );
        assert!(
            !step_props.contains_key("fail-if-no-file-changes"),
            "removed fail-if-no-file-changes must not remain in the StepConfig schema"
        );
    }

    #[test]
    fn test_schema_no_file_changes_is_string_enum() {
        // Given: the IfCondition schema definition
        let schema = load_schema();
        let if_props = def_properties(schema, "IfCondition");
        let nfc = &if_props["no-file-changes"];
        // Then: no-file-changes is a string enum restricted to retry|failed
        assert_eq!(
            nfc["type"].as_str(),
            Some("string"),
            "no-file-changes must have type 'string'; got: {nfc}"
        );
        let variants = nfc["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("no-file-changes must declare an enum; got: {nfc}"));
        let mut names: Vec<&str> = variants
            .iter()
            .map(|v| {
                v.as_str()
                    .unwrap_or_else(|| panic!("enum entry must be a string"))
            })
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec!["failed", "retry"]);
    }

    #[test]
    fn test_schema_prompt_file_has_expected_type_and_exclusion_rule() {
        let schema = load_schema();
        let step = &schema["$defs"]["StepConfig"];
        let prompt_file = &step["properties"]["prompt_file"];
        assert_eq!(
            prompt_file["type"].as_array().map(|types| {
                types
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
            }),
            Some(vec!["string", "null"]),
            "prompt_file must accept strings and YAML null (`~`)"
        );

        let exclusions = step["allOf"]
            .as_array()
            .unwrap_or_else(|| panic!("StepConfig allOf must be an array"));
        assert!(exclusions.iter().any(|rule| {
            rule["not"]["required"].as_array().is_some_and(|required| {
                required.iter().any(|v| v.as_str() == Some("prompt"))
                    && required.iter().any(|v| v.as_str() == Some("prompt_file"))
            })
        }));
    }

    #[test]
    fn test_schema_step_command_is_string_or_array() {
        let schema = load_schema();
        let step_props = def_properties(schema, "StepConfig");
        assert_oneof_types(&step_props["command"], &["string", "array"], "step command");
    }

    #[test]
    fn test_schema_step_skip_is_boolean_or_string() {
        let schema = load_schema();
        let step_props = def_properties(schema, "StepConfig");
        assert_oneof_types(&step_props["skip"], &["boolean", "string"], "step skip");
    }

    #[test]
    fn test_schema_when_condition_has_expected_properties() {
        let schema = load_schema();
        let when_props = def_properties(schema, "WhenCondition");
        assert_has_fields(when_props, &["exists"], "WhenCondition");
    }

    #[test]
    fn test_schema_if_condition_has_expected_properties() {
        let schema = load_schema();
        let if_props = def_properties(schema, "IfCondition");
        assert_has_fields(
            if_props,
            &["file-changed", "no-file-changes", "fail"],
            "IfCondition",
        );
    }

    #[test]
    fn test_schema_option_item_has_expected_properties() {
        let schema = load_schema();
        let option_item_props = def_properties(schema, "OptionItem");
        assert_has_fields(
            option_item_props,
            &["selector", "text-input", "next"],
            "OptionItem",
        );
    }

    #[test]
    fn test_schema_group_config_has_expected_properties() {
        let schema = load_schema();
        let group_props = def_properties(schema, "GroupConfig");
        assert_has_fields(group_props, &["if", "max_retries", "steps"], "GroupConfig");
    }

    #[test]
    fn test_schema_after_pr_is_object_with_step_config() {
        let schema = load_schema();
        assert_object_map_property(schema, "after-pr");
    }

    // -- LlmApiConfigYaml ----------------------------------------------------

    // ---- description field ----

    #[test]
    fn test_description_omitted_parses_as_none() {
        // Given: a YAML without description
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: description is None
        assert_eq!(config.description, None);
    }

    #[test]
    fn test_description_field_parses() {
        // Given: a YAML with a description
        let yaml = r"
command: [claude, -p]
description: 'team-shared: parallel implement + auto-PR'
steps:
  s1:
    command: echo hi
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: description is Some with the given value
        assert_eq!(
            config.description,
            Some("team-shared: parallel implement + auto-PR".to_string())
        );
    }

    #[test]
    fn test_when_exists_parses() {
        // Given: a step with when.exists
        let yaml = r#"
command: [claude, -p]
steps:
  format-rust:
    command: cargo fmt
    when:
      exists: "**/*.rs"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: when.exists is Some with the correct pattern
        let step = config
            .steps
            .get("format-rust")
            .unwrap_or_else(|| panic!("step not found"));
        let when = step.when.as_ref().unwrap_or_else(|| panic!("when is None"));
        assert_eq!(when.exists, Some("**/*.rs".to_string()));
    }

    #[test]
    fn test_when_exists_defaults_none() {
        // Given: a step without a when field
        let yaml = r"
command: [claude, -p]
steps:
  build:
    command: cargo build
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: when is None
        let step = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("step not found"));
        assert!(step.when.is_none(), "when should default to None");
    }

    #[test]
    fn test_validate_when_empty_glob_rejects() {
        let yaml = r#"
command: [claude, -p]
steps:
  build:
    command: cargo build
    when:
      exists: ""
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_when(&config);
        assert!(result.is_err(), "empty when.exists glob should be rejected");
    }

    #[test]
    fn test_validate_when_valid_glob_ok() {
        let yaml = r#"
command: [claude, -p]
steps:
  build:
    command: cargo build
    when:
      exists: "**/*.rs"
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_when(&config);
        assert!(result.is_ok(), "valid when.exists glob should be accepted");
    }

    #[test]
    fn test_validate_when_invalid_glob_syntax_rejects() {
        let yaml = r#"
command: [claude, -p]
steps:
  build:
    command: cargo build
    when:
      exists: "[invalid"
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_when(&config);
        assert!(result.is_err(), "invalid glob syntax should be rejected");
    }

    #[test]
    fn test_validate_when_glob_with_variable_skips_static_check() {
        let yaml = r#"
command: [claude, -p]
steps:
  build:
    command: cargo build
    when:
      exists: "{input}/**/*.rs"
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_when(&config);
        assert!(
            result.is_ok(),
            "glob with variable reference should skip static validation"
        );
    }

    // ---- sdk field ----

    #[test]
    fn test_sdk_field_parses_without_command() {
        // Given: a YAML with `sdk` and no `command`
        let yaml = r#"
sdk: seher
steps:
  s1:
    prompt: "Do: {input}"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: sdk is set and command defaults to empty
        assert_eq!(config.sdk.as_deref(), Some("seher"));
        assert!(config.command.is_empty(), "command should default to empty");
    }

    #[test]
    fn test_sdk_field_defaults_none() {
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.sdk.is_none(), "sdk should default to None");
    }

    #[test]
    fn test_validate_sdk_rejects_both_sdk_and_command() {
        // Given: both sdk and command set at the top level
        let yaml = r"
sdk: seher
command: [claude, -p]
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_sdk(&config);
        assert!(
            result.is_err(),
            "expected Err when both sdk and command set"
        );
        let msg = err_string(result);
        assert!(
            msg.contains("sdk") && msg.contains("command"),
            "error should mention both sdk and command, got: {msg}"
        );
    }

    #[test]
    fn test_validate_sdk_rejects_neither() {
        // Given: neither sdk nor command (command defaults to empty)
        let yaml = r"
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_sdk(&config);
        assert!(
            result.is_err(),
            "expected Err when neither sdk nor command set"
        );
    }

    #[test]
    fn test_validate_sdk_ok_sdk_only() {
        let yaml = r"
sdk: seher
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(validate_sdk(&config).is_ok(), "sdk-only should be valid");
    }

    #[test]
    fn test_validate_sdk_ok_command_only() {
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            validate_sdk(&config).is_ok(),
            "command-only should be valid"
        );
    }

    #[test]
    fn test_sdk_pi_field_parses() {
        let yaml = r"
sdk: pi
model: anthropic/claude-sonnet-4-6
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.sdk, Some("pi".to_string()));
        assert_eq!(
            config.model,
            Some("anthropic/claude-sonnet-4-6".to_string())
        );
    }

    #[test]
    fn test_validate_sdk_ok_pi() {
        let yaml = r"
sdk: pi
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(validate_sdk(&config).is_ok(), "sdk: pi should be valid");
    }

    #[test]
    fn test_validate_sdk_ok_claude() {
        let yaml = r"
sdk: claude
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(validate_sdk(&config).is_ok(), "sdk: claude should be valid");
    }

    #[test]
    fn test_validate_sdk_rejects_unknown_value() {
        let yaml = r"
sdk: made-up-sdk
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_sdk(&config);
        assert!(result.is_err(), "unknown sdk value should be rejected");
        let msg = err_string(result);
        assert!(
            msg.contains("made-up-sdk"),
            "error should name the offending value, got: {msg}"
        );
    }

    #[test]
    fn test_validate_config_runs_sdk_check() {
        // validate_config should surface the sdk/command mutual-exclusion error.
        let yaml = r"
sdk: seher
command: [claude, -p]
steps:
  s1:
    prompt: hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            validate_config(&config).is_err(),
            "validate_config should reject sdk+command"
        );
    }

    // --- apply_env_overrides tests ---

    const MINIMAL_YAML: &str = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";

    fn clear_all_override_envs() -> Vec<EnvGuard> {
        vec![
            EnvGuard::remove("CRUISE_MODEL"),
            EnvGuard::remove("CRUISE_PLAN_MODEL"),
            EnvGuard::remove("CRUISE_SDK"),
            EnvGuard::remove("CRUISE_LANGUAGE_PR"),
            EnvGuard::remove("CRUISE_LANGUAGE_PLAN"),
            EnvGuard::remove("LC_ALL"),
            EnvGuard::remove("LC_MESSAGES"),
            EnvGuard::remove("LANG"),
            EnvGuard::remove("LANGUAGE"),
            EnvGuard::remove("CRUISE_CLEANUP_AFTER_PR"),
            EnvGuard::remove("CRUISE_INTERACTIVE_PLANNING"),
            EnvGuard::remove("CRUISE_FORCE_EXEC"),
        ]
    }

    #[test]
    fn test_apply_env_overrides_sets_model() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _model = EnvGuard::set("CRUISE_MODEL", "opus");

        // Given: config has no model set
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.model, None);

        // When: env overrides are applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: model is overridden to the env var value
        assert_eq!(config.model, Some("opus".to_string()));
    }

    #[test]
    fn test_apply_env_overrides_empty_value_is_ignored() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _model = EnvGuard::set("CRUISE_MODEL", "");

        // Given: config has model=sonnet and CRUISE_MODEL is set to empty string
        let yaml = r"
command: [claude, -p]
model: sonnet
steps:
  s1:
    command: echo hi
";
        let mut config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.model, Some("sonnet".to_string()));

        // When: env overrides are applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: model is unchanged (empty env var is treated as unset)
        assert_eq!(config.model, Some("sonnet".to_string()));
    }

    #[test]
    fn test_apply_env_overrides_language_pr_writes_to_languages_struct() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _lang_pr = EnvGuard::set("CRUISE_LANGUAGE_PR", "Japanese");

        // Given: config has old-style pr_language=English in YAML
        let yaml = r"
command: [claude, -p]
pr_language: English
steps:
  s1:
    command: echo hi
";
        let mut config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.effective_pr_language(), "English");

        // When: CRUISE_LANGUAGE_PR env override is applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: effective_pr_language returns env var value (new-style languages.pr beats old pr_language)
        assert_eq!(config.effective_pr_language(), "Japanese");
        assert_eq!(
            config.languages.as_ref().and_then(|l| l.pr.as_deref()),
            Some("Japanese")
        );
    }

    #[test]
    fn test_apply_env_overrides_infers_locale_language() {
        let _lock = lock_process();
        for (variables, expected) in [
            (vec![("LANG", "ja_JP.UTF-8")], "Japanese"),
            (
                vec![
                    ("LC_ALL", "de_DE.UTF-8"),
                    ("LC_MESSAGES", "fr_FR.UTF-8"),
                    ("LANG", "ja_JP.UTF-8"),
                    ("LANGUAGE", "ko:en"),
                ],
                "German",
            ),
            (
                vec![("LC_MESSAGES", "fr_FR.UTF-8"), ("LANG", "ja_JP.UTF-8")],
                "French",
            ),
            (
                vec![("LC_ALL", "   "), ("LC_MESSAGES", "de_DE.UTF-8")],
                "German",
            ),
            (vec![("LANGUAGE", "ko:en")], "Korean"),
            (
                vec![("LC_ALL", "C.UTF-8"), ("LANG", "ja_JP.UTF-8")],
                DEFAULT_PLAN_LANGUAGE,
            ),
            (vec![("LANG", "C.UTF-8")], DEFAULT_PLAN_LANGUAGE),
            (vec![("LANG", "POSIX")], DEFAULT_PLAN_LANGUAGE),
            (vec![("LANG", "xx_YY")], DEFAULT_PLAN_LANGUAGE),
        ] {
            let _guards = clear_all_override_envs();
            let _envs: Vec<_> = variables
                .into_iter()
                .map(|(name, value)| EnvGuard::set(name, value))
                .collect();
            let mut config =
                WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));
            config
                .apply_env_overrides()
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(config.effective_plan_language(), expected);
            assert_eq!(config.effective_pr_language(), expected);
        }
    }

    #[test]
    fn test_apply_env_overrides_preserves_explicit_plan_language() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _lang = EnvGuard::set("LANG", "ja_JP.UTF-8");

        for field in ["languages:\n  plan: English", "plan_language: English"] {
            let yaml =
                format!("command: [claude, -p]\n{field}\nsteps:\n  s1:\n    command: echo hi\n");
            let mut config = WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"));
            config
                .apply_env_overrides()
                .unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(config.effective_plan_language(), "English");
        }
    }

    #[test]
    fn test_apply_env_overrides_explicit_pr_language_prevents_pr_inference() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _lang = EnvGuard::set("LANG", "ja_JP.UTF-8");

        // Given: the PR language is explicitly configured while plan language is not
        let mut config = WorkflowConfig::from_yaml(
            "command: [claude, -p]\nlanguages:\n  pr: English\nsteps:\n  s1:\n    command: echo hi\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: environment overrides are applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the explicit PR language wins and planning still follows the locale
        assert_eq!(config.effective_pr_language(), "English");
        assert_eq!(config.effective_plan_language(), "Japanese");
    }

    #[test]
    fn test_apply_env_overrides_keeps_inferred_languages_out_of_deprecated_fields() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _lang = EnvGuard::set("LANG", "ja_JP.UTF-8");

        // Given: a workflow with no new or deprecated language fields
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));

        // When: locale inference is applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: inferred values are stored only in the new language settings
        assert_eq!(config.pr_language, None);
        assert_eq!(config.plan_language, None);
        assert_eq!(config.effective_pr_language(), "Japanese");
        assert_eq!(config.effective_plan_language(), "Japanese");
    }

    #[test]
    fn test_apply_env_overrides_explicit_pr_env_wins_over_locale() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _lang = EnvGuard::set("LANG", "ja_JP.UTF-8");
        let _pr_language = EnvGuard::set("CRUISE_LANGUAGE_PR", "French");

        // Given: locale inference suggests Japanese and the explicit PR env selects French
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));

        // When: environment overrides are applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: only the PR language is overridden explicitly
        assert_eq!(config.effective_pr_language(), "French");
        assert_eq!(config.effective_plan_language(), "Japanese");
    }

    #[test]
    fn test_apply_env_overrides_explicit_plan_env_wins_over_locale() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _lang = EnvGuard::set("LANG", "ja_JP.UTF-8");
        let _plan_language = EnvGuard::set("CRUISE_LANGUAGE_PLAN", "French");

        // Given: locale inference suggests Japanese and the explicit plan env selects French
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));

        // When: environment overrides are applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the explicit plan environment override wins
        assert_eq!(config.effective_plan_language(), "French");
    }

    #[test]
    fn test_builtin_config_infers_plan_language_but_keeps_pr_language_english() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _lang = EnvGuard::set("LANG", "ja_JP.UTF-8");

        // Given: the built-in config leaves planning language unspecified
        let mut config = WorkflowConfig::from_yaml(BUILTIN_CONFIG_YAML)
            .unwrap_or_else(|e| panic!("built-in config YAML must parse: {e}"));

        // When: environment overrides are applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: planning follows the locale while PR generation remains English
        assert_eq!(config.effective_plan_language(), "Japanese");
        assert_eq!(config.effective_pr_language(), "English");
    }

    #[test]
    fn test_locale_to_language_name_parses_supported_locale_forms() {
        for (locale, expected) in [
            ("en-US", Some("English")),
            ("zh_CN.UTF-8", Some("Chinese")),
            ("pt_BR@latin", Some("Portuguese")),
            ("", None),
        ] {
            assert_eq!(
                locale_to_language_name(locale).as_deref(),
                expected,
                "locale: {locale}"
            );
        }
    }

    #[test]
    fn test_apply_env_overrides_bool_parses_true_false_1_0() {
        for (value, expected) in [("true", true), ("1", true), ("false", false), ("0", false)] {
            let _lock = lock_process();
            let _guards = clear_all_override_envs();
            let _cleanup = EnvGuard::set("CRUISE_CLEANUP_AFTER_PR", value);

            // Given: config with default cleanup_after_pr=false
            let mut config =
                WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));

            // When: bool env override is applied
            config
                .apply_env_overrides()
                .unwrap_or_else(|e| panic!("{e:?}"));

            // Then: cleanup_after_pr reflects the parsed bool value
            assert_eq!(
                config.cleanup_after_pr, expected,
                "CRUISE_CLEANUP_AFTER_PR={value:?} should parse to {expected}"
            );
        }
    }

    #[test]
    fn test_apply_env_overrides_force_exec_parses_true_false_1_0() {
        for (value, expected) in [("true", true), ("1", true), ("false", false), ("0", false)] {
            let _lock = lock_process();
            let _guards = clear_all_override_envs();
            let _force_exec = EnvGuard::set("CRUISE_FORCE_EXEC", value);

            // Given: config with default force_exec=false
            let mut config =
                WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));

            // When: bool env override is applied
            config
                .apply_env_overrides()
                .unwrap_or_else(|e| panic!("{e:?}"));

            // Then: force_exec reflects the parsed bool value
            assert_eq!(
                config.force_exec, expected,
                "CRUISE_FORCE_EXEC={value:?} should parse to {expected}"
            );
        }
    }

    #[test]
    fn test_apply_env_overrides_invalid_bool_returns_error() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _cleanup = EnvGuard::set("CRUISE_CLEANUP_AFTER_PR", "yes");

        // Given: CRUISE_CLEANUP_AFTER_PR is set to an invalid value
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));

        // When: env overrides are applied
        let result = config.apply_env_overrides();

        // Then: an error is returned naming the variable and the invalid value
        assert!(result.is_err(), "invalid bool should return an error");
        let msg = err_string(result);
        assert!(
            msg.contains("CRUISE_CLEANUP_AFTER_PR"),
            "error should name the env var, got: {msg}"
        );
        assert!(
            msg.contains("yes"),
            "error should include the invalid value, got: {msg}"
        );
    }

    #[test]
    fn test_apply_env_overrides_force_exec_invalid_bool_returns_error() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _force_exec = EnvGuard::set("CRUISE_FORCE_EXEC", "yes");

        // Given: CRUISE_FORCE_EXEC is set to an invalid value
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));

        // When: env overrides are applied
        let result = config.apply_env_overrides();

        // Then: an error is returned naming the env var and invalid value
        assert!(result.is_err(), "invalid bool should return an error");
        let msg = err_string(result);
        assert!(
            msg.contains("CRUISE_FORCE_EXEC"),
            "error should name the env var, got: {msg}"
        );
        assert!(
            msg.contains("yes"),
            "error should include the invalid value, got: {msg}"
        );
    }

    #[test]
    fn test_apply_env_overrides_no_env_vars_is_noop() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();

        // Given: a fully configured workflow and no env overrides set
        let yaml = r"
command: [claude, -p]
model: sonnet
plan_model: opus
cleanup_after_pr: true
pr_language: Japanese
steps:
  s1:
    command: echo hi
";
        let mut config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let original = config.clone();

        // When: env overrides are applied with no env vars set
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: all fields remain unchanged
        assert_eq!(config.model, original.model);
        assert_eq!(config.plan_model, original.plan_model);
        assert_eq!(config.sdk, original.sdk);
        assert_eq!(config.cleanup_after_pr, original.cleanup_after_pr);
        assert_eq!(config.interactive_planning, original.interactive_planning);
        assert_eq!(
            config.effective_pr_language(),
            original.effective_pr_language()
        );
        assert_eq!(
            config.effective_plan_language(),
            original.effective_plan_language()
        );
    }

    #[test]
    fn test_apply_env_overrides_cruise_sdk_clears_command() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _sdk = EnvGuard::set("CRUISE_SDK", "seher");

        // Given: config has command set (the default case when loaded from YAML)
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!config.command.is_empty(), "precondition: command is set");

        // When: CRUISE_SDK env var is applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: sdk is set and command is cleared so validate_sdk passes
        assert_eq!(config.sdk, Some("seher".to_string()));
        assert!(
            config.command.is_empty(),
            "command must be cleared when sdk is set via env"
        );
        assert!(
            validate_sdk(&config).is_ok(),
            "validate_sdk must pass after env override"
        );
    }

    #[test]
    fn test_apply_env_overrides_cruise_sdk_pi() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _sdk = EnvGuard::set("CRUISE_SDK", "pi");

        // Given: config has command set (the default case when loaded from YAML)
        let mut config =
            WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!config.command.is_empty(), "precondition: command is set");

        // When: CRUISE_SDK=pi env var is applied
        config
            .apply_env_overrides()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: sdk is set to "pi", command is cleared, and validate_sdk passes
        assert_eq!(config.sdk, Some("pi".to_string()));
        assert!(
            config.command.is_empty(),
            "command must be cleared when sdk is set via env"
        );
        assert!(
            validate_sdk(&config).is_ok(),
            "validate_sdk must accept 'pi' after env override"
        );
    }

    // --- top-level `max_retries` field ---

    #[test]
    fn test_max_retries_field_parses_when_present() {
        // Given: workflow YAML sets a top-level max_retries ceiling
        let yaml = r"
command: [claude, -p]
max_retries: 5
steps:
  s1:
    command: echo hi
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the field is Some(5)
        assert_eq!(config.max_retries, Some(5));
    }

    #[test]
    fn test_max_retries_field_defaults_to_none_when_omitted() {
        // Given: workflow YAML omits max_retries
        let config = WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: the field defaults to None
        assert_eq!(config.max_retries, None);
    }

    #[test]
    fn test_max_retries_field_round_trips_through_serialize() {
        // Given: a parsed config with max_retries set
        let yaml = r"
command: [claude, -p]
max_retries: 7
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When: serialized back to YAML and re-parsed (mirrors session config.yaml snapshots)
        let serialized = serde_yaml::to_string(&config).unwrap_or_else(|e| panic!("{e:?}"));
        let reparsed = WorkflowConfig::from_yaml(&serialized).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the value survives the round trip
        assert_eq!(reparsed.max_retries, Some(7));
    }

    // --- resolve_effective_max_retries ---

    #[test]
    fn test_resolve_effective_max_retries_cli_flag_wins_over_config() {
        // Given: config sets max_retries: 5, and the CLI flag is explicitly 7
        let yaml = r"
command: [claude, -p]
max_retries: 5
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: the explicit CLI value takes precedence
        assert_eq!(resolve_effective_max_retries(Some(7), &config), 7);
    }

    #[test]
    fn test_resolve_effective_max_retries_uses_config_value_when_cli_omitted() {
        // Given: config sets max_retries: 5, and no CLI flag was passed
        let yaml = r"
command: [claude, -p]
max_retries: 5
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: the config value is used
        assert_eq!(resolve_effective_max_retries(None, &config), 5);
    }

    #[test]
    fn test_resolve_effective_max_retries_falls_back_to_default_when_neither_set() {
        // Given: config has no max_retries, and no CLI flag was passed
        let config = WorkflowConfig::from_yaml(MINIMAL_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: DEFAULT_MAX_RETRIES governs
        assert_eq!(
            resolve_effective_max_retries(None, &config),
            DEFAULT_MAX_RETRIES
        );
    }

    // --- validate_group_retry_budget ---

    fn group_config_with_max_retries(max_retries: usize) -> String {
        format!(
            r"
command: [claude, -p]
groups:
  review:
    max_retries: {max_retries}
    steps:
      simplify:
        prompt: /simplify
steps:
  build:
    command: cargo build
  review-pass:
    group: review
"
        )
    }

    #[test]
    fn test_validate_group_retry_budget_rejects_unreachable_group() {
        // Given: group 'review' has max_retries: 4, effective ceiling is 3 (R > G)
        let config = WorkflowConfig::from_yaml(&group_config_with_max_retries(4))
            .unwrap_or_else(|e| panic!("{e:?}"));
        // When
        let result = validate_group_retry_budget(&config, 3);
        // Then: rejected, naming the group and both values
        assert!(result.is_err());
        let msg = err_string(result);
        assert!(msg.contains("review"), "expected group name in: {msg}");
        assert!(
            msg.contains('4'),
            "expected configured max_retries in: {msg}"
        );
        assert!(msg.contains('3'), "expected effective ceiling in: {msg}");
    }

    #[test]
    fn test_validate_group_retry_budget_boundary_equal_is_accepted() {
        // Given: group max_retries exactly equals the effective ceiling (R == G)
        let config = WorkflowConfig::from_yaml(&group_config_with_max_retries(3))
            .unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: the graceful skip at R fires exactly when the edge count reaches R,
        // which is still <= G, so the hard LoopProtection failure never triggers.
        assert!(
            validate_group_retry_budget(&config, 3).is_ok(),
            "R == G should be accepted: the graceful skip at R fires before the hard failure at G+1"
        );
    }

    #[test]
    fn test_validate_group_retry_budget_accepts_value_below_ceiling() {
        // Given: group max_retries is strictly below the effective ceiling (R < G)
        let config = WorkflowConfig::from_yaml(&group_config_with_max_retries(2))
            .unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: reachable, so validation passes
        assert!(validate_group_retry_budget(&config, 3).is_ok());
    }

    #[test]
    fn test_validate_group_retry_budget_ok_when_group_has_no_max_retries() {
        // Given: group 'review' is referenced by a step but sets no max_retries of its own
        let yaml = r"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  build:
    command: cargo build
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: the global ceiling governs; nothing to validate for this group
        assert!(validate_group_retry_budget(&config, 3).is_ok());
    }

    #[test]
    fn test_validate_group_retry_budget_ignores_unreferenced_group() {
        // Given: group 'review' has an unreachable max_retries but no step references it
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 99
    steps:
      simplify:
        prompt: /simplify
steps:
  build:
    command: cargo build
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: unused group definitions are harmless
        assert!(validate_group_retry_budget(&config, 3).is_ok());
    }

    #[test]
    fn test_validate_group_retry_budget_checks_after_pr_referenced_group() {
        // Given: group 'review' is referenced only from an after-pr step
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 4
    steps:
      simplify:
        prompt: /simplify
steps:
  build:
    command: cargo build
after-pr:
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: after-pr references are validated the same way as regular steps
        assert!(validate_group_retry_budget(&config, 3).is_err());
    }

    // --- validate_group_retry_budget: case 1 vs case 2 (retry-target location) ---

    /// Case 1, target == the call-site step name itself (`review-pass`).
    fn group_config_case1_call_site_target(max_retries: usize) -> String {
        format!(
            r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: review-pass
    max_retries: {max_retries}
    steps:
      simplify:
        prompt: /simplify
steps:
  review-pass:
    group: review
"
        )
    }

    #[test]
    fn test_validate_group_retry_budget_case1_boundary_r_equals_g_is_accepted() {
        // Given: case 1 (retry target is the call site itself), R == G.
        // Regression guard: this exact boundary was the site of a past off-by-one bug.
        let config = WorkflowConfig::from_yaml(&group_config_case1_call_site_target(3))
            .unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: lock-step holds for case 1, so R == G is safe.
        assert!(
            validate_group_retry_budget(&config, 3).is_ok(),
            "case 1 with R == G must be accepted (lock-step boundary regression check)"
        );
    }

    #[test]
    fn test_validate_group_retry_budget_case1_r_greater_than_g_is_rejected() {
        // Given: case 1 (retry target is the call site itself), R > G.
        let config = WorkflowConfig::from_yaml(&group_config_case1_call_site_target(4))
            .unwrap_or_else(|e| panic!("{e:?}"));
        // When
        let result = validate_group_retry_budget(&config, 3);
        // Then: rejected, naming the group and both values
        assert!(result.is_err());
        let msg = err_string(result);
        assert!(msg.contains("review"), "expected group name in: {msg}");
        assert!(
            msg.contains('4'),
            "expected configured max_retries in: {msg}"
        );
        assert!(msg.contains('3'), "expected effective ceiling in: {msg}");
    }

    #[test]
    fn test_validate_group_retry_budget_case1_first_substep_target_form_is_accepted() {
        // Given: case 1's other shape -- target is "<call-site>/<first-sub-step>"
        // instead of the bare call-site name -- with R == G.
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: review-pass/simplify
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      ai-antipattern:
        prompt: /ai-antipattern
steps:
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: still case 1 (re-enters the group's own first step), R == G is safe.
        assert!(validate_group_retry_budget(&config, 3).is_ok());
    }

    #[test]
    fn test_validate_group_retry_budget_rejects_external_target_overflow() {
        // Given: the largest representable retry budget with an external target.
        let max = usize::MAX;
        let yaml = format!(
            "command: [claude, -p]\ngroups:\n  review:\n    if:\n      file-changed: build\n    max_retries: {max}\n    steps:\n      simplify:\n        prompt: /simplify\nsteps:\n  build:\n    command: echo build\n  review-pass:\n    group: review\n"
        );
        let config = WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: validation reports the impossible extra edge instead of overflowing.
        let result = validate_group_retry_budget(&config, max);
        assert!(
            result.is_err(),
            "max retry budget with an external target must be rejected"
        );
        assert!(err_string(result).contains("external retry target"));
    }

    #[test]
    fn test_validate_group_retry_budget_case2_r_plus_1_equals_g_is_accepted() {
        // Given: case 2 (retry target is an external step 'build'), R + 1 == G.
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: build
    max_retries: 2
    steps:
      simplify:
        prompt: /simplify
steps:
  build:
    command: cargo build
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: the extra sequential edge back into the group costs one unit of
        // budget, so R + 1 <= G (2 + 1 == 3) is the safe boundary for case 2.
        assert!(
            validate_group_retry_budget(&config, 3).is_ok(),
            "case 2 with R + 1 == G must be accepted"
        );
    }

    #[test]
    fn test_validate_group_retry_budget_case2_r_equals_g_is_rejected() {
        // Given: the real-world failure this function was hardened for --
        // groups.review has max_retries: 3, retry target 'test' is outside the
        // group, and the effective ceiling is 3 (R == G, R + 1 > G).
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When
        let result = validate_group_retry_budget(&config, 3);
        // Then: rejected -- R == G is not enough budget once the extra external
        // edge is accounted for -- and the message names both the group and the
        // external retry target so the user knows exactly which edge is at fault.
        assert!(result.is_err());
        let msg = err_string(result);
        assert!(msg.contains("review"), "expected group name in: {msg}");
        assert!(msg.contains("test"), "expected retry target in: {msg}");
    }

    #[test]
    fn test_validate_group_retry_budget_no_file_changed_keeps_old_rule() {
        // Given: the group has an `if:` block, but it does not set `file-changed`
        // (so no retry can structurally ever happen). R > G would be rejected
        // under the original rule regardless of what the group's steps look like.
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if: {}
    max_retries: 4
    steps:
      simplify:
        prompt: /simplify
steps:
  build:
    command: cargo build
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: falls back to the original `R > G` guard (no case-2 penalty
        // applies since there is no retry target to jump to at all).
        assert!(validate_group_retry_budget(&config, 3).is_err());
        assert!(validate_group_retry_budget(&config, 4).is_ok());
    }

    #[test]
    fn test_validate_group_retry_budget_ignores_unreferenced_group_with_external_target() {
        // Given: an unreferenced group definition that would fail case 2 if it were
        // ever wired up (R == G with an external-looking retry target).
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: nonexistent-external-step
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
steps:
  build:
    command: cargo build
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: unused group definitions are harmless regardless of shape
        assert!(validate_group_retry_budget(&config, 3).is_ok());
    }

    #[test]
    fn test_validate_group_retry_budget_multiple_call_sites_requires_all_to_reenter() {
        // Given: group 'review' is invoked from two call sites, but its single
        // shared if.file-changed target can only match one of their names
        // ('review-after-lib'). From 'review-after-doc's perspective the retry
        // target is an unrelated external step, so the group as a whole must be
        // held to case 2's stricter bound (safe side). R == G should therefore
        // be rejected even though it would pass under case 1.
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: review-after-lib
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
steps:
  test1:
    command: cargo test --lib
  review-after-lib:
    group: review
  test2:
    command: cargo test --doc
  review-after-doc:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // When/Then: rejected under case 2's R + 1 <= G bound, not accepted under case 1
        assert!(
            validate_group_retry_budget(&config, 3).is_err(),
            "a shared target matching only one of several call sites must not get case 1's looser bound"
        );
    }

    // -----------------------------------------------------------------------
    // validate_mixed_conditional_cycles
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_config_rejects_mixed_cycle() {
        // Given: a cycle a -> b -> c ->(if.file-changed) a mixing one unsafe
        // conditional back-edge with unconditional sequential edges (the
        // shared fixture reproduces the failed session's flat step cycle)
        let config = WorkflowConfig::from_yaml(&mixed_conditional_cycle_config())
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: validate_config runs on it
        let message = err_string(validate_config(&config));

        // Then: it is rejected, naming the witness cycle in order, explaining
        // the max_retries exhaustion mechanism, and pointing at groups as fix
        assert!(
            message.contains("a -> b -> c -> a"),
            "error should name the witness cycle, got: {message}"
        );
        assert!(
            message.contains("max_retries"),
            "error should explain that the conditional edge exhausts max_retries, got: {message}"
        );
        assert!(
            message.contains("groups"),
            "error should recommend confining the cycle into groups, got: {message}"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_builtin_config_ok() {
        // Given: the built-in default workflow (its review loop is safely
        // confined inside groups.verify-review with max_retries)
        let config =
            WorkflowConfig::from_yaml(BUILTIN_CONFIG_YAML).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: the mixed-cycle validator accepts it
        assert!(
            validate_mixed_conditional_cycles(&config).is_ok(),
            "the built-in config must not be rejected as a mixed cycle"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_allows_pure_unconditional_cycle() {
        // Given: a cycle made of unconditional `next` edges only (runtime loop
        // protection is the designed safety net for this shape)
        let yaml = r"
command: [claude, -p]
steps:
  a:
    prompt: a
    next: b
  b:
    prompt: b
    next: a
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: the mixed-cycle validator accepts it
        assert!(
            validate_mixed_conditional_cycles(&config).is_ok(),
            "a purely unconditional cycle must not be rejected by the mixed-cycle check"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_allows_group_backedge_cycle() {
        // Given: a group whose if.file-changed retry target is outside the
        // group (x -> group call -> back to x on file-changed), with a
        // max_retries that satisfies validate_group_retry_budget under the
        // default ceiling of 3 (R + 1 <= G)
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: x
    max_retries: 2
    steps:
      review-it:
        prompt: review
steps:
  x:
    command: cargo test
  do-review:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: the grouped back-edge is treated as safe (already guarded
        // by validate_group_retry_budget) and the cycle is accepted
        assert!(
            validate_mixed_conditional_cycles(&config).is_ok(),
            "a group-confined retry cycle must not be rejected as a mixed cycle"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_ignores_self_retry() {
        // Given: a single step whose only conditional edge is a self-retry
        let yaml = r"
command: [claude, -p]
steps:
  implement:
    prompt: implement
    if:
      no-file-changes: retry
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: self-retries fall through sequentially when the condition
        // stops firing, so they are not part of any cycle
        assert!(
            validate_mixed_conditional_cycles(&config).is_ok(),
            "an if.no-file-changes.retry self-edge must be ignored"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_allows_forward_file_changed_jump() {
        // Given: an if.file-changed jump that does not close a cycle (forward
        // edge only, no way back)
        let yaml = r"
command: [claude, -p]
steps:
  a:
    prompt: a
    if:
      file-changed: b
  b:
    prompt: b
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: no SCC contains both kinds of edges, so it is accepted
        assert!(
            validate_mixed_conditional_cycles(&config).is_ok(),
            "a forward if.file-changed jump without a return edge must be accepted"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_rejects_if_fail_goto_cycle() {
        // Given: a cycle whose back-edge is an if.fail goto target
        let yaml = r"
command: [claude, -p]
steps:
  build:
    prompt: build
  test:
    command: exit 1
    if:
      fail: build
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When: the mixed-cycle validator runs on it
        let message = err_string(validate_mixed_conditional_cycles(&config));

        // Then: it is rejected, naming the witness cycle
        assert!(
            message.contains("build -> test -> build"),
            "an if.fail goto back-edge must count as an unsafe conditional edge, got: {message}"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_ignores_option_step_next_edges() {
        // Given: an option step whose interactive choice loops back to an
        // earlier step (`next` on option items)
        let yaml = r"
command: [claude, -p]
steps:
  start:
    prompt: start
  choose:
    plan: '{{plan}}'
    option:
      - selector: redo
        next: start
      - selector: continue
        next: finish
  finish:
    prompt: finish
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: option-step next edges are user-driven choices, out of
        // scope for this check, so the graph is accepted
        assert!(
            validate_mixed_conditional_cycles(&config).is_ok(),
            "option-step next edges must be excluded from cycle detection"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_rejects_group_without_max_retries() {
        // Given: a group whose if.file-changed retry target closes a top-level
        // cycle, but which sets no max_retries -- at runtime the jump fires
        // unboundedly and never degrades into a graceful skip (see
        // check_group_retry_skip), so this is exactly the deadlocking shape
        let yaml = r"
command: [claude, -p]
groups:
  g:
    if:
      file-changed: build
    steps:
      inner:
        prompt: r
steps:
  build:
    prompt: b
  review:
    group: g
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When: the mixed-cycle validator runs on it
        let message = err_string(validate_mixed_conditional_cycles(&config));

        // Then: it is rejected, naming the witness cycle
        assert!(
            message.contains("build -> review -> build"),
            "a group file-changed back-edge without max_retries must count as an \
             unsafe conditional edge, got: {message}"
        );
    }

    #[test]
    fn test_validate_mixed_conditional_cycles_ignores_unreachable_option_fallthrough() {
        // Given: an option step where every choice carries an explicit `next`,
        // so the runtime always follows the selected choice and the sequential
        // fall-through edge can never fire; the only cycle would go through it
        let yaml = r"
command: [claude, -p]
steps:
  start:
    prompt: start
  menu:
    prompt: choose
    option:
      - selector: restart
        next: start
  guard:
    prompt: guard
    if:
      fail:
        goto: start
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When/Then: the unreachable fall-through edge must not create a
        // false-positive mixed cycle
        assert!(
            validate_mixed_conditional_cycles(&config).is_ok(),
            "an option step whose choices all set next has no reachable \
             fall-through edge"
        );
    }
}
