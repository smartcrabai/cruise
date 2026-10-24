//! Workflow flags: sparse on/off switches; absent values are off.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::workflow::CompiledWorkflow;

/// Sparse flag values keyed by name; an absent value is off.
pub type FlagValues = BTreeMap<String, bool>;

/// An empty sparse flag-value map.
pub static NO_FLAGS: FlagValues = BTreeMap::new();

/// A declared flag as presented to UIs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FlagChoice {
    pub name: String,
    pub description: Option<String>,
}

/// Apply an explicit selection to declared flags, preserving undeclared values.
#[must_use]
pub fn apply_enabled(
    current: &FlagValues,
    choices: &[FlagChoice],
    enabled: &[String],
) -> FlagValues {
    let mut values = current.clone();
    for choice in choices {
        values.insert(choice.name.clone(), enabled.contains(&choice.name));
    }
    values
}

/// Parse a flag name as enabled, or a `NAME=on|off|true|false` assignment.
///
/// # Errors
///
/// Returns a message when the assignment is malformed or the name is empty.
pub fn parse_assignment(raw: &str) -> std::result::Result<(String, bool), String> {
    if raw.is_empty() {
        return Err("missing flag name".to_string());
    }
    let Some((name, value)) = raw.split_once('=') else {
        return Ok((raw.to_string(), true));
    };
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

/// Whether a step carrying `flag` is disabled. Any enabled listed flag enables it.
#[must_use]
pub fn is_disabled(flag: Option<&crate::config::StringOrVec>, values: &FlagValues) -> bool {
    flag.is_some_and(|flag| {
        !flag
            .as_slice()
            .iter()
            .any(|name| values.get(name).copied().unwrap_or(false))
    })
}

/// User-skipped steps plus steps disabled by flags (including absent values).
#[must_use]
pub fn preflight_skips(
    compiled: &CompiledWorkflow,
    values: &FlagValues,
    user_skipped: &[String],
) -> Vec<String> {
    let mut out = user_skipped.to_vec();
    for (name, step) in compiled.steps.iter().chain(&compiled.after_pr) {
        if is_disabled(step.flag.as_ref(), values) && !out.contains(name) {
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
