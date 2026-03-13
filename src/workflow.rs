#![allow(dead_code)]

use indexmap::IndexMap;
use std::collections::HashMap;

use crate::config::{IfCondition, StepConfig, WorkflowConfig};
use crate::error::Result;

type ExpandedSteps = (
    IndexMap<String, StepConfig>,
    HashMap<String, InvocationMeta>,
    HashMap<String, String>,
);

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
    /// Language to use for PR title/body generation.
    pub pr_language: String,
    /// Flat steps after group-call expansion. Order matches the original YAML.
    pub steps: IndexMap<String, StepConfig>,
    /// Flat after-pr steps after group-call expansion.
    pub after_pr: IndexMap<String, StepConfig>,
    /// Invocation metadata for group calls in `steps`, keyed by call-site name.
    pub invocations: HashMap<String, InvocationMeta>,
    /// Invocation metadata for group calls in `after_pr`, keyed by call-site name.
    pub after_pr_invocations: HashMap<String, InvocationMeta>,
    /// Precomputed mapping from expanded step name → call-site name, for `steps`.
    pub step_to_invocation: HashMap<String, String>,
    /// Precomputed mapping from expanded step name → call-site name, for `after_pr`.
    pub after_pr_step_to_invocation: HashMap<String, String>,
}

impl CompiledWorkflow {
    /// Create a new `CompiledWorkflow` that runs the `after_pr` phase as its main steps.
    pub fn to_after_pr_compiled(&self) -> Self {
        Self {
            command: self.command.clone(),
            model: self.model.clone(),
            plan_model: self.plan_model.clone(),
            env: self.env.clone(),
            pr_language: self.pr_language.clone(),
            steps: self.after_pr.clone(),
            invocations: self.after_pr_invocations.clone(),
            step_to_invocation: self.after_pr_step_to_invocation.clone(),
            after_pr: IndexMap::new(),
            after_pr_invocations: HashMap::new(),
            after_pr_step_to_invocation: HashMap::new(),
        }
    }
}

/// Compile a [`WorkflowConfig`] into a flat [`CompiledWorkflow`].
///
/// Validates the config (undefined groups, migration errors, empty groups,
/// nested calls, individual `if` in group steps) and expands all group call
/// steps into their constituent sub-steps.
pub fn compile(config: WorkflowConfig) -> Result<CompiledWorkflow> {
    let (steps, invocations, step_to_invocation) = expand_steps(&config.steps, &config.groups)?;
    let (after_pr, after_pr_invocations, after_pr_step_to_invocation) =
        expand_steps(&config.after_pr, &config.groups)?;

    Ok(CompiledWorkflow {
        command: config.command,
        model: config.model,
        plan_model: config.plan_model,
        env: config.env,
        pr_language: config.pr_language,
        steps,
        after_pr,
        invocations,
        after_pr_invocations,
        step_to_invocation,
        after_pr_step_to_invocation,
    })
}

fn expand_steps(
    steps: &IndexMap<String, StepConfig>,
    groups: &HashMap<String, crate::config::GroupConfig>,
) -> Result<ExpandedSteps> {
    let mut flat: IndexMap<String, StepConfig> = IndexMap::new();
    let mut invocations: HashMap<String, InvocationMeta> = HashMap::new();
    let mut step_to_invocation: HashMap<String, String> = HashMap::new();

    for (step_name, step) in steps {
        if let Some(group_name) = &step.group {
            // Old membership style: has group + prompt/command → migration error
            if step.prompt.is_some() || step.command.is_some() {
                return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                    "step '{step_name}' uses old membership style (group + prompt/command). \
                     Please migrate to groups.<name>.steps block style."
                )));
            }

            // Look up group definition
            let group_def = groups.get(group_name).ok_or_else(|| {
                crate::error::CruiseError::InvalidStepConfig(format!(
                    "step '{step_name}' references undefined group '{group_name}'"
                ))
            })?;

            // Empty group check
            if group_def.steps.is_empty() {
                return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                    "group '{group_name}' is empty (no steps defined)"
                )));
            }

            // Validate and expand sub-steps
            let step_count = group_def.steps.len();
            // Non-empty is guaranteed by the is_empty check above.
            let first_sub = group_def.steps.keys().next().ok_or_else(|| {
                crate::error::CruiseError::InvalidStepConfig(format!(
                    "group '{group_name}' unexpectedly empty"
                ))
            })?;
            let last_sub = group_def.steps.keys().last().ok_or_else(|| {
                crate::error::CruiseError::InvalidStepConfig(format!(
                    "group '{group_name}' unexpectedly empty"
                ))
            })?;
            let first_step = format!("{step_name}/{first_sub}");
            let last_step = format!("{step_name}/{last_sub}");
            for (sub_name, sub_step) in &group_def.steps {
                // Nested group call check
                if sub_step.group.is_some() {
                    return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                        "nested group call inside group '{group_name}' at step '{sub_name}' is not allowed"
                    )));
                }
                // Individual `if` inside group step check
                if sub_step.if_condition.is_some() {
                    return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                        "group step '{group_name}/{sub_name}' has an individual 'if' condition, \
                         which is not allowed inside group steps"
                    )));
                }

                let key = format!("{step_name}/{sub_name}");
                if flat.contains_key(&key) {
                    return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                        "expanded step key '{key}' collides with an existing step name"
                    )));
                }
                step_to_invocation.insert(key.clone(), step_name.clone());
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

    Ok((flat, invocations, step_to_invocation))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    fn parsed(yaml: &str) -> WorkflowConfig {
        WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn compiled(yaml: &str) -> CompiledWorkflow {
        compile(parsed(yaml)).unwrap_or_else(|e| panic!("{e:?}"))
    }

    // -----------------------------------------------------------------------
    // Happy-path: compile expands group calls correctly
    // -----------------------------------------------------------------------

    #[test]
    fn test_compile_non_group_steps_pass_through_unchanged() {
        // Given: workflow with no group calls
        let yaml = r"
command: [echo]
steps:
  step1:
    command: echo hello
  step2:
    command: echo world
";
        // When: compiled
        let c = compiled(yaml);
        // Then: steps are identical to the source
        let keys: Vec<&str> = c.steps.keys().map(std::string::String::as_str).collect();
        assert_eq!(keys, vec!["step1", "step2"]);
        assert!(c.invocations.is_empty());
    }

    #[test]
    fn test_compile_group_call_expands_to_prefixed_steps() {
        // Given: workflow with one group call
        let yaml = r"
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
";
        // When: compiled
        let c = compiled(yaml);
        // Then: group call is expanded with call-site prefix
        let keys: Vec<&str> = c.steps.keys().map(std::string::String::as_str).collect();
        assert_eq!(
            keys,
            vec!["test", "review-pass/simplify", "review-pass/coderabbit"]
        );
    }

    #[test]
    fn test_compile_group_call_step_order_preserved() {
        // Given: group with three steps in a specific order
        let yaml = r"
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
";
        // When: compiled
        let c = compiled(yaml);
        // Then: expanded steps appear in definition order
        let keys: Vec<&str> = c.steps.keys().map(std::string::String::as_str).collect();
        assert_eq!(keys, vec!["call/alpha", "call/beta", "call/gamma"]);
    }

    #[test]
    fn test_compile_invocation_metadata_populated() {
        // Given: group with max_retries and if condition
        let yaml = r"
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
";
        // When: compiled
        let c = compiled(yaml);
        // Then: invocation metadata reflects the group definition
        let meta = c
            .invocations
            .get("review-pass")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(meta.max_retries, Some(3));
        assert!(meta.if_condition.is_some());
        assert_eq!(meta.first_step, "review-pass/simplify");
        assert_eq!(meta.last_step, "review-pass/coderabbit");
        assert_eq!(meta.step_count, 2);
    }

    #[test]
    fn test_compile_same_group_two_call_sites_independent_invocations() {
        // Given: same group invoked from two separate call sites
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
        // When: compiled
        let c = compiled(yaml);
        // Then: each call site has its own invocation metadata entry
        assert!(c.invocations.contains_key("review-after-lib"));
        assert!(c.invocations.contains_key("review-after-doc"));
        // And: steps are interleaved in YAML order with per-call-site prefixes
        let keys: Vec<&str> = c.steps.keys().map(std::string::String::as_str).collect();
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
        let yaml = r"
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
";
        // When: compiled
        let c = compiled(yaml);
        // Then: after_pr steps are expanded
        let keys: Vec<&str> = c.after_pr.keys().map(std::string::String::as_str).collect();
        assert_eq!(keys, vec!["post-notify/slack", "post-notify/email"]);
        // And: invocation metadata exists for after-pr call site
        assert!(c.after_pr_invocations.contains_key("post-notify"));
    }

    #[test]
    fn test_compile_non_group_step_not_in_invocations() {
        // Given: workflow with no group calls
        let yaml = r"
command: [echo]
steps:
  step1:
    command: echo hello
";
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
        let yaml = r"
command: [echo]
groups: {}
steps:
  bad:
    group: nonexistent
";
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: error mentions undefined group
        assert!(result.is_err());
        assert!(
            result
                .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
                .to_string()
                .contains("undefined group")
        );
    }

    #[test]
    fn test_compile_old_membership_style_returns_migration_error() {
        // Given: top-level step has both `group` and `prompt` (old membership style)
        let yaml = r"
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
";
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: migration error pointing users to groups.<name>.steps
        assert!(result.is_err());
        let msg = result
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
            .to_string();
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
        let yaml = r"
command: [echo]
groups:
  review:
    steps: {}
steps:
  call-review:
    group: review
";
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: error mentions empty group
        assert!(result.is_err());
        assert!(
            result
                .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
                .to_string()
                .contains("empty"),
            "expected 'empty' in error"
        );
    }

    #[test]
    fn test_compile_nested_group_call_returns_error() {
        // Given: a step inside group.steps itself references another group
        let yaml = r"
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
";
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: nested group call is rejected
        assert!(result.is_err());
        let msg = result
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
            .to_string();
        assert!(
            msg.contains("nested") || msg.contains("group call") || msg.contains("group"),
            "expected nested-group-call error in: {msg}"
        );
    }

    #[test]
    fn test_compile_group_step_individual_if_returns_error() {
        // Given: a step inside group.steps has its own `if` condition
        let yaml = r"
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
";
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: individual `if` inside group step is rejected
        assert!(result.is_err());
        assert!(
            result
                .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
                .to_string()
                .contains("if"),
            "expected 'if' in error message"
        );
    }

    #[test]
    fn test_compile_step_key_collision_returns_error() {
        // Given: a regular step named "call/simplify" and a group call "call" that expands to
        // "call/simplify" — the expanded key collides with the existing regular step.
        let yaml = r"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: echo simplify
steps:
  call/simplify:
    command: echo manual
  call:
    group: review
";
        // When: compile is called
        let result = compile(parsed(yaml));
        // Then: error mentions collision
        assert!(result.is_err());
        let msg = result
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
            .to_string();
        assert!(
            msg.contains("collides"),
            "expected 'collides' in error message, got: {msg}"
        );
    }

    #[test]
    fn test_compile_group_step_preserves_fail_if_no_file_changes() {
        // Given: a group whose sub-step has fail-if-no-file-changes: true
        let yaml = r"
command: [echo]
groups:
  review:
    steps:
      implement:
        command: cargo build
        fail-if-no-file-changes: true
steps:
  run-review:
    group: review
";
        // When: compiled
        let c = compiled(yaml);
        // Then: the expanded step preserves fail_if_no_file_changes
        let step = c
            .steps
            .get("run-review/implement")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert!(
            step.fail_if_no_file_changes,
            "fail_if_no_file_changes should be preserved after compilation"
        );
    }
}
