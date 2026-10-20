//! Workflow flags: user-resolved on/off switches for chunks of steps.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::workflow::CompiledWorkflow;

/// Resolved flag values keyed by flag name.
pub type FlagValues = BTreeMap<String, bool>;

/// Empty flag values for contexts that declare no flags.
pub static NO_FLAGS: FlagValues = BTreeMap::new();

/// A declared flag as presented to UIs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FlagChoice {
    pub name: String,
    pub description: Option<String>,
}

/// Parse a `NAME=on|off|true|false` assignment.
///
/// # Errors
///
/// Returns a message when the assignment is malformed.
pub fn parse_assignment(raw: &str) -> std::result::Result<(String, bool), String> {
    let (name, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected NAME=on|off, got '{raw}'"))?;
    if name.is_empty() {
        return Err(format!("missing flag name in '{raw}'"));
    }
    let value = match value {
        "on" | "true" => true,
        "off" | "false" => false,
        _ => {
            return Err(format!(
                "invalid value '{value}' for flag '{name}'; use on or off"
            ));
        }
    };
    Ok((name.to_string(), value))
}

/// Build flag values from CLI assignments, rejecting duplicates.
///
/// # Errors
///
/// Returns an error when a name is assigned more than once.
pub fn from_cli(assignments: &[(String, bool)]) -> Result<FlagValues> {
    let mut values = FlagValues::new();
    for (name, value) in assignments {
        if values.insert(name.clone(), *value).is_some() {
            return Err(CruiseError::Other(format!(
                "flag '{name}' was specified more than once"
            )));
        }
    }
    Ok(values)
}

/// Reject names the config does not declare.
///
/// # Errors
///
/// Returns an error naming the unknown flag and the declared ones.
pub fn validate_names(config: &WorkflowConfig, values: &FlagValues) -> Result<()> {
    for name in values.keys() {
        if !config.flags.contains_key(name) {
            let declared: Vec<&str> = config.flags.keys().map(String::as_str).collect();
            let declared = if declared.is_empty() {
                "none".to_string()
            } else {
                declared.join(", ")
            };
            return Err(CruiseError::Other(format!(
                "unknown flag '{name}'; declared flags: {declared}"
            )));
        }
    }
    Ok(())
}

/// Declared flags without a value, in declaration order.
#[must_use]
pub fn missing<'a>(
    config: &'a WorkflowConfig,
    values: &FlagValues,
) -> Vec<(&'a str, Option<&'a str>)> {
    config
        .flags
        .iter()
        .filter(|(name, _)| !values.contains_key(name.as_str()))
        .map(|(name, flag)| (name.as_str(), flag.description.as_deref()))
        .collect()
}

/// Fail when any declared flag has no value.
///
/// # Errors
///
/// Returns an error listing the unresolved flags.
pub fn require_complete(config: &WorkflowConfig, values: &FlagValues) -> Result<()> {
    let unset = missing(config, values);
    if unset.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = unset.iter().map(|(name, _)| *name).collect();
    Err(CruiseError::Other(format!(
        "workflow flags must be set explicitly before running: {} (pass --flag NAME=on|off)",
        names.join(", ")
    )))
}

/// Whether a step carrying `flag` is disabled.
///
/// # Errors
///
/// Returns an error when the flag has no value.
pub fn is_disabled(flag: Option<&str>, values: &FlagValues) -> Result<bool> {
    let Some(flag) = flag else {
        return Ok(false);
    };
    values
        .get(flag)
        .map(|on| !*on)
        .ok_or_else(|| CruiseError::Other(format!("flag '{flag}' has no value")))
}

/// Flags referenced by a compiled workflow that have no value.
#[must_use]
pub fn missing_in_compiled(compiled: &CompiledWorkflow, values: &FlagValues) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for step in compiled.steps.values().chain(compiled.after_pr.values()) {
        if let Some(flag) = step.flag.as_deref()
            && !values.contains_key(flag)
            && !out.iter().any(|n| n == flag)
        {
            out.push(flag.to_string());
        }
    }
    out
}

/// User-skipped steps plus steps disabled by flags (main and after-pr).
#[must_use]
pub fn preflight_skips(
    compiled: &CompiledWorkflow,
    values: &FlagValues,
    user_skipped: &[String],
) -> Vec<String> {
    let mut out = user_skipped.to_vec();
    for (name, step) in compiled.steps.iter().chain(&compiled.after_pr) {
        if let Some(flag) = step.flag.as_deref()
            && values.get(flag) == Some(&false)
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    }
    out
}

/// Declared flags for UIs.
#[must_use]
pub fn choices(config: &WorkflowConfig) -> Vec<FlagChoice> {
    config
        .flags
        .iter()
        .map(|(name, flag)| FlagChoice {
            name: name.clone(),
            description: flag.description.clone(),
        })
        .collect()
}

/// Ask for every missing flag. Returns false when the user cancels.
///
/// # Errors
///
/// Propagates errors from `ask`.
pub fn prompt_missing(
    config: &WorkflowConfig,
    values: &mut FlagValues,
    mut ask: impl FnMut(&str, Option<&str>) -> Result<Option<bool>>,
) -> Result<bool> {
    for (name, description) in missing(config, values) {
        match ask(name, description)? {
            Some(value) => {
                values.insert(name.to_string(), value);
            }
            None => return Ok(false),
        }
    }
    Ok(true)
}

/// Interactively ask for one flag with no default, forcing an explicit answer.
///
/// # Errors
///
/// Returns an error when the terminal prompt fails.
pub fn prompt_flag_cli(name: &str, description: Option<&str>) -> Result<Option<bool>> {
    let message = format!("Enable workflow flag '{name}'?");
    let mut prompt = inquire::Confirm::new(&message);
    if let Some(description) = description {
        prompt = prompt.with_help_message(description);
    }
    match prompt.prompt() {
        Ok(value) => Ok(Some(value)),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => Ok(None),
        Err(error) => Err(CruiseError::Other(error.to_string())),
    }
}
