#![allow(dead_code)]

use indexmap::IndexMap;
use std::collections::HashMap;

use crate::config::{IfCondition, StepConfig, WorkflowConfig};
use crate::error::Result;

/// Metadata for a single group invocation (one call site in top-level steps).
/// Keyed by the call-site step name (e.g. "review-pass").
#[derive(Debug, Clone)]
pub struct InvocationMeta {
    /// Retry-trigger condition inherited from the group definition.
    pub if_condition: Option<IfCondition>,
    /// Maximum retry count inherited from the group definition.
    pub max_retries: Option<usize>,
    /// ID of the first expanded step in this invocation.
    pub first_step: String,
    /// ID of the last expanded step in this invocation.
    pub last_step: String,
    /// Number of steps in this invocation.
    pub step_count: usize,
}

/// Flat, executable representation of a workflow after group-call expansion.
///
/// Group call steps (e.g. `review-pass: {group: review}`) are replaced by
/// their expanded sub-steps using the convention `{call_site}/{step_name}`
/// (e.g. `review-pass/simplify`, `review-pass/coderabbit`).
#[derive(Debug, Clone)]
pub struct CompiledWorkflow {
    pub command: Vec<String>,
    pub model: Option<String>,
    pub plan_model: Option<String>,
    pub env: HashMap<String, String>,
    /// Flat steps after group-call expansion. Order matches the original YAML.
    pub steps: IndexMap<String, StepConfig>,
    /// Flat after-pr steps after group-call expansion.
    pub after_pr: IndexMap<String, StepConfig>,
    /// Invocation metadata for group calls in `steps`, keyed by call-site name.
    pub invocations: HashMap<String, InvocationMeta>,
    /// Invocation metadata for group calls in `after_pr`, keyed by call-site name.
    pub after_pr_invocations: HashMap<String, InvocationMeta>,
}

/// Compile a [`WorkflowConfig`] into a flat [`CompiledWorkflow`].
///
/// Validates the config (undefined groups, migration errors, empty groups,
/// nested calls, individual `if` in group steps) and expands all group call
/// steps into their constituent sub-steps.
pub fn compile(config: WorkflowConfig) -> Result<CompiledWorkflow> {
    let (steps, invocations) = expand_steps(&config.steps, &config.groups)?;
    let (after_pr, after_pr_invocations) = expand_steps(&config.after_pr, &config.groups)?;

    Ok(CompiledWorkflow {
        command: config.command,
        model: config.model,
        plan_model: config.plan_model,
        env: config.env,
        steps,
        after_pr,
        invocations,
        after_pr_invocations,
    })
}

fn expand_steps(
    steps: &IndexMap<String, StepConfig>,
    groups: &HashMap<String, crate::config::GroupConfig>,
) -> Result<(
    IndexMap<String, StepConfig>,
    HashMap<String, InvocationMeta>,
)> {
    let mut flat: IndexMap<String, StepConfig> = IndexMap::new();
    let mut invocations: HashMap<String, InvocationMeta> = HashMap::new();

    for (step_name, step) in steps {
        if let Some(group_name) = &step.group {
            // Old membership style: has group + prompt/command → migration error
            if step.prompt.is_some() || step.command.is_some() {
                return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                    "step '{}' uses old membership style (group + prompt/command). \
                     Please migrate to groups.<name>.steps block style.",
                    step_name
                )));
            }

            // Look up group definition
            let group_def = groups.get(group_name).ok_or_else(|| {
                crate::error::CruiseError::InvalidStepConfig(format!(
                    "step '{}' references undefined group '{}'",
                    step_name, group_name
                ))
            })?;

            // Empty group check
            if group_def.steps.is_empty() {
                return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                    "group '{}' is empty (no steps defined)",
                    group_name
                )));
            }

            // Validate and expand sub-steps
            let step_count = group_def.steps.len();
            // Non-empty is guaranteed by the is_empty check above.
            let first_step = format!("{}/{}", step_name, group_def.steps.keys().next().unwrap());
            let last_step = format!("{}/{}", step_name, group_def.steps.keys().last().unwrap());
            for (sub_name, sub_step) in &group_def.steps {
                // Nested group call check
                if sub_step.group.is_some() {
                    return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                        "nested group call inside group '{}' at step '{}' is not allowed",
                        group_name, sub_name
                    )));
                }
                // Individual `if` inside group step check
                if sub_step.if_condition.is_some() {
                    return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                        "group step '{}/{}' has an individual 'if' condition, \
                         which is not allowed inside group steps",
                        group_name, sub_name
                    )));
                }

                let key = format!("{}/{}", step_name, sub_name);
                if flat.contains_key(&key) {
                    return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                        "expanded step key '{}' collides with an existing step name",
                        key
                    )));
                }
                flat.insert(key, sub_step.clone());
            }

            invocations.insert(
                step_name.clone(),
                InvocationMeta {
                    if_condition: group_def.if_condition.clone(),
                    max_retries: group_def.max_retries,
                    first_step,
                    last_step,
                    step_count,
                },
            );
        } else {
            // Regular step: pass through unchanged
            flat.insert(step_name.clone(), step.clone());
        }
    }

    Ok((flat, invocations))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    fn parsed(yaml: &str) -> WorkflowConfig {
        WorkflowConfig::from_yaml(yaml).unwrap()
    }

    fn compiled(yaml: &str) -> CompiledWorkflow {
        compile(parsed(yaml)).unwrap()
    }

    // -----------------------------------------------------------------------
    // Happy-path: compile expands group calls correctly
    // -----------------------------------------------------------------------

    #[test]
    fn test_compile_non_group_steps_pass_through_unchanged() {
        // Given: workflow with no group calls
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: echo hello
  step2:
    command: echo world
"#;
        // When: compiled
        let c = compiled(yaml);
        // Then: steps are identical to the source
        let keys: Vec<&str> = c.steps.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["step1", "step2"]);
        assert!(c.invocations.is_empty());
    }

    #[test]
    fn test_compile_group_call_expands_to_prefixed_steps() {
        // Given: workflow with one group call
        let yaml = r#"
command: [claude, -p]
groups:
  review:
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
"#;
        // When: compiled
        let c = compiled(yaml);
        // Then: group call is expanded with call-site prefix
        let keys: Vec<&str> = c.steps.keys().map(|s| s.as_str()).collect();
        assert_eq!(
            keys,
            vec!["test", "review-pass/simplify", "review-pass/coderabbit"]
        );
    }

    #[test]
    fn test_compile_group_call_step_order_preserved() {
        // Given: group with three steps in a specific order
        let yaml = r#"
command: [claude, -p]
groups:
  review:
    steps:
      alpha:
        command: echo alpha
      beta:
        command: echo beta
      gamma:
        command: echo gamma
steps:
  call:
    group: review
"#;
        // When: compiled
        let c = compiled(yaml);
        // Then: expanded steps appear in definition order
        let keys: Vec<&str> = c.steps.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["call/alpha", "call/beta", "call/gamma"]);
    }

    #[test]
    fn test_compile_invocation_metadata_populated() {
        // Given: group with max_retries and if condition
        let yaml = r#"
command: [claude, -p]
groups:
  review:
    max_retries: 3
    if:
      file-changed: test
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
"#;
        // When: compiled
        let c = compiled(yaml);
        // Then: invocation metadata reflects the group definition
        let meta = c.invocations.get("review-pass").unwrap();
        assert_eq!(meta.max_retries, Some(3));
        assert!(meta.if_condition.is_some());
        assert_eq!(meta.first_step, "review-pass/simplify");
        assert_eq!(meta.last_step, "review-pass/coderabbit");
        assert_eq!(meta.step_count, 2);
    }

    #[test]
    fn test_compile_same_group_two_call_sites_independent_invocations() {
        // Given: same group invoked from two separate call sites
        let yaml = r#"
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
"#;
        // When: compiled
        let c = compiled(yaml);
        // Then: each call site has its own invocation metadata entry
        assert!(c.invocations.contains_key("review-after-lib"));
        assert!(c.invocations.contains_key("review-after-doc"));
        // And: steps are interleaved in YAML order with per-call-site prefixes
        let keys: Vec<&str> = c.steps.keys().map(|s| s.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "test1",
                "review-after-lib/simplify",
                "test2",
                "review-after-doc/simplify",
            ]
        );
    }

    #[test]
    fn test_compile_after_pr_group_call_expands() {
        // Given: after-pr contains a group call
        let yaml = r#"
command: [claude, -p]
groups:
  notify:
    steps:
      slack:
        command: echo slack
      email:
        command: echo email
steps:
  build:
    command: cargo build
after-pr:
  post-notify:
    group: notify
"#;
        // When: compiled
        let c = compiled(yaml);
        // Then: after_pr steps are expanded
        let keys: Vec<&str> = c.after_pr.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["post-notify/slack", "post-notify/email"]);
        // And: invocation metadata exists for after-pr call site
        assert!(c.after_pr_invocations.contains_key("post-notify"));
    }

    #[test]
    fn test_compile_non_group_step_not_in_invocations() {
        // Given: workflow with no group calls
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: echo hello
"#;
        // When: compiled
        let c = compiled(yaml);
        // Then: invocations map is empty
        assert!(c.invocations.is_empty());
        assert!(c.after_pr_invocations.is_empty());
    }

    // -----------------------------------------------------------------------
    // Error cases: validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_compile_undefined_group_returns_error() {
        // Given: a top-level step calls a group that is not defined
        let yaml = r#"
command: [echo]
groups: {}
steps:
  bad:
    group: nonexistent
"#;
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: error mentions undefined group
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("undefined group"));
    }

    #[test]
    fn test_compile_old_membership_style_returns_migration_error() {
        // Given: top-level step has both `group` and `prompt` (old membership style)
        let yaml = r#"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  step1:
    group: review
    prompt: /something
"#;
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: migration error pointing users to groups.<name>.steps
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("migration")
                || msg.contains("groups.<name>.steps")
                || msg.contains("move"),
            "expected migration hint in: {msg}"
        );
    }

    #[test]
    fn test_compile_empty_group_returns_error() {
        // Given: a group is defined with no inner steps
        let yaml = r#"
command: [echo]
groups:
  review:
    steps: {}
steps:
  call-review:
    group: review
"#;
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: error mentions empty group
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("empty"),
            "expected 'empty' in error"
        );
    }

    #[test]
    fn test_compile_nested_group_call_returns_error() {
        // Given: a step inside group.steps itself references another group
        let yaml = r#"
command: [claude, -p]
groups:
  inner:
    steps:
      step-a:
        command: echo inner
  outer:
    steps:
      nested:
        group: inner
steps:
  call-outer:
    group: outer
"#;
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: nested group call is rejected
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nested") || msg.contains("group call") || msg.contains("group"),
            "expected nested-group-call error in: {msg}"
        );
    }

    #[test]
    fn test_compile_group_step_individual_if_returns_error() {
        // Given: a step inside group.steps has its own `if` condition
        let yaml = r#"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
        if:
          file-changed: test
steps:
  call-review:
    group: review
"#;
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: individual `if` inside group step is rejected
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("if"),
            "expected 'if' in error message"
        );
    }
}
