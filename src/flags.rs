//! Workflow flags: sparse on/off switches; absent values are off.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::workflow::{CompiledWorkflow, SkippableStepNode};

/// Sparse flag values keyed by name; an absent value is off.
pub type FlagValues = BTreeMap<String, bool>;

/// An empty sparse flag-value map.
pub static NO_FLAGS: FlagValues = BTreeMap::new();

/// Skip candidates whose steps can run under `values`.
#[must_use]
pub fn skip_candidates(nodes: &[SkippableStepNode], values: &FlagValues) -> Vec<SkippableStepNode> {
    nodes
        .iter()
        .filter(|node| !is_disabled(node.flag.as_ref(), values))
        .cloned()
        .collect()
}

/// Drop skip entries belonging to flag-disabled nodes, preserving unknown IDs.
#[must_use]
pub fn drop_disabled_skips<'a>(
    skipped: Vec<String>,
    nodes: impl IntoIterator<Item = &'a SkippableStepNode>,
    values: &FlagValues,
) -> Vec<String> {
    let mut disabled = HashSet::new();
    for node in nodes
        .into_iter()
        .filter(|node| is_disabled(node.flag.as_ref(), values))
    {
        disabled.insert(node.id.as_str());
        disabled.extend(node.expanded_step_ids.iter().map(String::as_str));
        disabled.extend(node.children.iter().map(|child| child.id.as_str()));
    }
    skipped
        .into_iter()
        .filter(|id| !disabled.contains(id.as_str()))
        .collect()
}

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

#[cfg(test)]
mod skip_candidate_tests {
    use super::*;
    use crate::workflow::{SkippableStepNode, list_skippable_after_pr_steps, list_skippable_steps};

    fn nodes() -> Vec<SkippableStepNode> {
        let config = WorkflowConfig::from_yaml(
            r"
command: [echo]
flags:
  e2e: {}
  docs: {}
groups:
  checks:
    steps:
      first:
        command: echo first
      second:
        command: echo second
steps:
  build:
    command: echo build
  e2e-test:
    flag: e2e
    command: echo e2e
  either-test:
    flag: [e2e, docs]
    command: echo either
  review:
    flag: docs
    group: checks
after-pr:
  notify:
    flag: docs
    command: echo notify
",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let mut nodes = list_skippable_steps(&config).unwrap_or_else(|e| panic!("{e:?}"));
        nodes.extend(list_skippable_after_pr_steps(&config).unwrap_or_else(|e| panic!("{e:?}")));
        nodes
    }

    fn ids(nodes: &[SkippableStepNode]) -> Vec<&str> {
        nodes.iter().map(|node| node.id.as_str()).collect()
    }

    #[test]
    fn absent_flags_hide_gated_skip_candidates() {
        assert_eq!(
            ids(&skip_candidates(&nodes(), &FlagValues::new())),
            ["build"]
        );
    }

    #[test]
    fn explicit_off_hides_gated_skip_candidates() {
        let values = FlagValues::from([("e2e".into(), false), ("docs".into(), false)]);
        assert_eq!(ids(&skip_candidates(&nodes(), &values)), ["build"]);
    }

    #[test]
    fn enabled_flag_restores_its_skip_candidate_in_workflow_order() {
        let values = FlagValues::from([("e2e".into(), true)]);
        assert_eq!(
            ids(&skip_candidates(&nodes(), &values)),
            ["build", "e2e-test", "either-test"]
        );
    }

    #[test]
    fn any_enabled_flag_in_a_list_restores_the_skip_candidate() {
        let values = FlagValues::from([("e2e".into(), false), ("docs".into(), true)]);
        assert!(ids(&skip_candidates(&nodes(), &values)).contains(&"either-test"));
    }

    #[test]
    fn enabled_group_candidate_preserves_children_and_expanded_ids() {
        let nodes = nodes();
        let values = FlagValues::from([("docs".into(), true)]);
        let candidates = skip_candidates(&nodes, &values);
        let group = candidates
            .iter()
            .find(|node| node.id == "review")
            .unwrap_or_else(|| panic!("enabled group is missing"));
        assert_eq!(group.expanded_step_ids, ["review/first", "review/second"]);
        assert_eq!(ids(&group.children), ["review/first", "review/second"]);
    }

    #[test]
    fn after_pr_candidates_follow_their_flag() {
        let nodes = nodes();
        let after_pr = &nodes[nodes.len() - 1..];
        assert!(skip_candidates(after_pr, &FlagValues::new()).is_empty());
        assert_eq!(
            ids(&skip_candidates(
                after_pr,
                &FlagValues::from([("docs".into(), true)])
            )),
            ["notify"]
        );
    }

    #[test]
    fn disabled_group_skip_pruning_removes_parent_and_every_expanded_child() {
        let skipped = ["review", "review/first", "review/second", "build"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            drop_disabled_skips(skipped, &nodes(), &FlagValues::new()),
            ["build"]
        );
    }

    #[test]
    fn disabled_after_pr_skip_is_removed() {
        assert_eq!(
            drop_disabled_skips(vec!["notify".into()], &nodes(), &FlagValues::new()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn pruning_keeps_enabled_and_ungated_skips_in_input_order() {
        let skipped = ["e2e-test", "build", "either-test"]
            .map(str::to_owned)
            .to_vec();
        let values = FlagValues::from([("e2e".into(), true)]);
        assert_eq!(
            drop_disabled_skips(skipped.clone(), &nodes(), &values),
            skipped
        );
    }

    #[test]
    fn pruning_preserves_unknown_ids_including_unknown_group_children() {
        let skipped = ["custom", "review/unknown", "e2e-test", "build"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            drop_disabled_skips(skipped, &nodes(), &FlagValues::new()),
            ["custom", "review/unknown", "build"]
        );
    }

    #[test]
    fn empty_candidate_catalog_preserves_free_text_skips() {
        let skipped = vec!["custom".into()];
        assert_eq!(
            drop_disabled_skips(skipped.clone(), &[], &FlagValues::new()),
            skipped
        );
    }
}
