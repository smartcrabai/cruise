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

/// A node in the skippable step tree returned by [`list_skippable_steps`].
///
/// This structure is used by both the CLI and GUI to present a hierarchical
/// view of steps, where group call sites appear as parent nodes with their
/// sub-steps as children. The `expanded_step_ids` field contains the actual
/// executable step IDs that should be stored in `session.skipped_steps`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippableStepNode {
    /// UI-stable identifier. For regular steps this is the step name.
    /// For group parent nodes this is the call-site name. For children
    /// of group parents this is the expanded step ID (e.g. `review-pass/simplify`).
    pub id: String,
    /// The executable step IDs that are skipped when this node is selected.
    /// For a regular step this contains one element. For a group parent it
    /// contains all expanded child IDs. For a child node it contains only
    /// its own expanded ID.
    pub expanded_step_ids: Vec<String>,
    /// Child nodes for group parent steps. Empty for regular steps and
    /// for child nodes (which are leaves).
    pub children: Vec<SkippableStepNode>,
}

/// Build a tree of skippable steps from a [`WorkflowConfig`].
///
/// This function mirrors the group-expansion logic in [`compile`] so that
/// the UI and CLI can present parent/child relationships for group calls.
/// The returned nodes are in the same order as the top-level steps in the
/// config, and `after_pr` is excluded.
///
/// # Errors
///
/// Returns an error if the config references undefined groups or uses
/// invalid group configurations.
pub fn list_skippable_steps(config: &WorkflowConfig) -> Result<Vec<SkippableStepNode>> {
    let (expanded_steps, _invocations, step_to_invocation) =
        expand_steps(&config.steps, &config.groups)?;
    let mut expanded_ids_by_call_site: HashMap<String, Vec<String>> = HashMap::new();

    for expanded_id in expanded_steps.keys() {
        if let Some(call_site) = step_to_invocation.get(expanded_id) {
            expanded_ids_by_call_site
                .entry(call_site.clone())
                .or_default()
                .push(expanded_id.clone());
        }
    }

    config
        .steps
        .iter()
        .map(|(step_name, step_config)| {
            if step_config.group.is_some() {
                let expanded_ids =
                    expanded_ids_by_call_site.remove(step_name).ok_or_else(|| {
                        crate::error::CruiseError::InvalidStepConfig(format!(
                            "group call step '{step_name}' produced no expanded steps"
                        ))
                    })?;
                let children = expanded_ids
                    .iter()
                    .cloned()
                    .map(|expanded_id| SkippableStepNode {
                        id: expanded_id.clone(),
                        expanded_step_ids: vec![expanded_id],
                        children: Vec::new(),
                    })
                    .collect();
                Ok(SkippableStepNode {
                    id: step_name.clone(),
                    expanded_step_ids: expanded_ids,
                    children,
                })
            } else {
                Ok(SkippableStepNode {
                    id: step_name.clone(),
                    expanded_step_ids: vec![step_name.clone()],
                    children: Vec::new(),
                })
            }
        })
        .collect()
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
    /// Precomputed mapping from expanded step name -> call-site name, for `steps`.
    pub step_to_invocation: HashMap<String, String>,
    /// Precomputed mapping from expanded step name -> call-site name, for `after_pr`.
    pub after_pr_step_to_invocation: HashMap<String, String>,
    /// Resolved LLM API configuration. `None` when no API key is available.
    pub llm_api: Option<crate::llm_api::LlmApiConfig>,
}

impl CompiledWorkflow {
    /// Create a new `CompiledWorkflow` that runs the `after_pr` phase as its main steps.
    #[must_use]
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
            llm_api: self.llm_api.clone(),
        }
    }
}

/// Compile a [`WorkflowConfig`] into a flat [`CompiledWorkflow`].
///
/// Validates the config (undefined groups, migration errors, empty groups,
/// nested calls, individual `if` in group steps) and expands all group call
/// steps into their constituent sub-steps.
///
/// # Errors
///
/// Returns an error if the config references undefined groups, uses the old
/// membership style, contains empty groups, nested group calls, or
/// individual `if` conditions inside group steps.
pub fn compile(config: WorkflowConfig) -> Result<CompiledWorkflow> {
    let (steps, invocations, step_to_invocation) = expand_steps(&config.steps, &config.groups)?;
    let (after_pr, after_pr_invocations, after_pr_step_to_invocation) =
        expand_steps(&config.after_pr, &config.groups)?;

    let llm_api = crate::llm_api::resolve_llm_api_config(config.llm.as_ref());

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
        llm_api,
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
            // Old membership style: has group + prompt/command -> migration error
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
            if flat.contains_key(step_name) {
                return Err(crate::error::CruiseError::InvalidStepConfig(format!(
                    "expanded step key '{step_name}' collides with an existing step name"
                )));
            }
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
    use crate::test_support::err_string;

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
        assert!(err_string(result).contains("undefined group"));
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
        let msg = err_string(result);
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
            err_string(result).contains("empty"),
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
        let msg = err_string(result);
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
            err_string(result).contains("if"),
            "expected 'if' in error message"
        );
    }

    #[test]
    fn test_compile_step_key_collision_returns_error() {
        // Given: a regular step named "call/simplify" and a group call "call" that expands to
        // "call/simplify" -- the expanded key collides with the existing regular step.
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
        let msg = err_string(result);
        assert!(
            msg.contains("collides"),
            "expected 'collides' in error message, got: {msg}"
        );
    }

    #[test]
    fn test_compile_step_key_collision_returns_error_when_regular_step_follows_group() {
        let yaml = r"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: echo simplify
steps:
  call:
    group: review
  call/simplify:
    command: echo manual
";
        let result = compile(parsed(yaml));
        assert!(result.is_err());
        let msg = err_string(result);
        assert!(
            msg.contains("collides"),
            "expected 'collides' in error message, got: {msg}"
        );
    }

    // -- llm_api field --------------------------------------------------------

    #[test]
    fn test_compile_llm_api_is_none_when_no_api_key_configured() {
        // Given: workflow with no llm section and CRUISE_LLM_API_KEY not set
        let _lock = crate::test_support::lock_process();
        let _env = crate::test_support::EnvGuard::remove("CRUISE_LLM_API_KEY");
        let yaml = r"
command: [echo]
steps:
  s1:
    command: echo hello
";
        // When: compiled
        let c = compiled(yaml);

        // Then: llm_api is None (API mode disabled by default)
        assert!(
            c.llm_api.is_none(),
            "llm_api should be None when no API key is configured"
        );
    }

    #[test]
    fn test_compile_llm_api_propagates_to_after_pr_compiled() {
        // Given: compiled workflow with no llm config (llm_api = None)
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
after-pr:
  notify:
    command: echo done
";
        // When: compiled, then converted to after-pr compiled
        let c = compiled(yaml);
        let after_pr = c.to_after_pr_compiled();

        // Then: llm_api is propagated (same value in both)
        assert_eq!(
            c.llm_api.is_none(),
            after_pr.llm_api.is_none(),
            "llm_api should propagate from compile to to_after_pr_compiled"
        );
    }

    #[test]
    fn test_compile_llm_api_is_some_when_api_key_env_var_set() {
        // Given: CRUISE_LLM_API_KEY is set in the environment
        let _lock = crate::test_support::lock_process();
        let _env = crate::test_support::EnvGuard::remove("CRUISE_LLM_API_KEY");
        let _key = crate::test_support::EnvGuard::set("CRUISE_LLM_API_KEY", "sk-integration-test");

        let yaml = r"
command: [echo]
steps:
  s1:
    command: echo hello
";
        // When: compiled
        let c = compiled(yaml);

        // Then: llm_api is Some with the env key
        let api = c.llm_api.unwrap_or_else(|| {
            panic!("expected llm_api to be Some when CRUISE_LLM_API_KEY is set")
        });
        assert_eq!(api.api_key, "sk-integration-test");
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

    // -----------------------------------------------------------------------
    // list_skippable_steps tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_list_skippable_steps_non_group_steps_unchanged() {
        // Given: workflow with no group calls
        let yaml = r"
command: [echo]
steps:
  step1:
    command: echo hello
  step2:
    command: echo world
";
        // When: list_skippable_steps is called
        let nodes = list_skippable_steps(&parsed(yaml)).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: nodes are flat, each with one expanded ID matching the step name
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].id, "step1");
        assert_eq!(nodes[0].expanded_step_ids, vec!["step1"]);
        assert!(nodes[0].children.is_empty());
        assert_eq!(nodes[1].id, "step2");
        assert_eq!(nodes[1].expanded_step_ids, vec!["step2"]);
        assert!(nodes[1].children.is_empty());
    }

    #[test]
    fn test_list_skippable_steps_group_call_parent_and_children() {
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
        // When: list_skippable_steps is called
        let nodes = list_skippable_steps(&parsed(yaml)).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: nodes include parent and children in correct order
        assert_eq!(nodes.len(), 2);
        // First node is the regular step
        assert_eq!(nodes[0].id, "test");
        assert_eq!(nodes[0].expanded_step_ids, vec!["test"]);
        assert!(nodes[0].children.is_empty());
        // Second node is the group parent
        assert_eq!(nodes[1].id, "review-pass");
        // Parent's expanded IDs are all children
        assert_eq!(
            nodes[1].expanded_step_ids,
            vec!["review-pass/simplify", "review-pass/coderabbit"]
        );
        // Parent has children
        assert_eq!(nodes[1].children.len(), 2);
        // Children have their own expanded IDs
        assert_eq!(nodes[1].children[0].id, "review-pass/simplify");
        assert_eq!(
            nodes[1].children[0].expanded_step_ids,
            vec!["review-pass/simplify"]
        );
        assert_eq!(nodes[1].children[1].id, "review-pass/coderabbit");
        assert_eq!(
            nodes[1].children[1].expanded_step_ids,
            vec!["review-pass/coderabbit"]
        );
    }

    #[test]
    fn test_list_skippable_steps_order_preserved() {
        // Given: group with three steps in specific order
        let yaml = r"
command: [echo]
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
        // When: list_skippable_steps is called
        let nodes = list_skippable_steps(&parsed(yaml)).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: order matches YAML definition
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "call");
        assert_eq!(
            nodes[0].expanded_step_ids,
            vec!["call/alpha", "call/beta", "call/gamma"]
        );
        assert_eq!(nodes[0].children[0].id, "call/alpha");
        assert_eq!(nodes[0].children[1].id, "call/beta");
        assert_eq!(nodes[0].children[2].id, "call/gamma");
    }

    #[test]
    fn test_list_skippable_steps_multiple_call_sites() {
        // Given: same group invoked from two separate call sites
        let yaml = r"
command: [echo]
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
        // When: list_skippable_steps is called
        let nodes = list_skippable_steps(&parsed(yaml)).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: both call sites appear as separate parent nodes
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0].id, "test1");
        assert_eq!(nodes[1].id, "review-after-lib");
        assert_eq!(
            nodes[1].expanded_step_ids,
            vec!["review-after-lib/simplify"]
        );
        assert_eq!(nodes[2].id, "test2");
        assert_eq!(nodes[3].id, "review-after-doc");
        assert_eq!(
            nodes[3].expanded_step_ids,
            vec!["review-after-doc/simplify"]
        );
    }

    #[test]
    fn test_list_skippable_steps_excludes_after_pr() {
        // Given: workflow with steps and after-pr containing a group call
        let yaml = r"
command: [echo]
groups:
  notify:
    steps:
      slack:
        command: echo slack
steps:
  build:
    command: cargo build
after-pr:
  post-notify:
    group: notify
";
        // When: list_skippable_steps is called
        let nodes = list_skippable_steps(&parsed(yaml)).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: only steps are included, after-pr is excluded
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "build");
    }

    #[test]
    fn test_list_skippable_steps_rejects_old_membership_style() {
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
        let err = list_skippable_steps(&parsed(yaml))
            .err()
            .unwrap_or_else(|| panic!("expected Err"));
        let msg = err.to_string();
        assert!(
            msg.contains("old membership style") || msg.contains("groups.<name>.steps"),
            "expected migration hint in: {msg}"
        );
    }

    #[test]
    fn test_list_skippable_steps_rejects_empty_group() {
        let yaml = r"
command: [echo]
groups:
  review:
    steps: {}
steps:
  review-pass:
    group: review
";
        let err = list_skippable_steps(&parsed(yaml))
            .err()
            .unwrap_or_else(|| panic!("expected Err"));
        assert!(
            err.to_string().contains("empty"),
            "expected empty-group error"
        );
    }

    #[test]
    fn test_list_skippable_steps_rejects_expanded_key_collision() {
        let yaml = r"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: echo simplify
steps:
  review-pass:
    group: review
  review-pass/simplify:
    command: echo collision
";
        let err = list_skippable_steps(&parsed(yaml))
            .err()
            .unwrap_or_else(|| panic!("expected Err"));
        assert!(
            err.to_string().contains("collides"),
            "expected collision error"
        );
    }
}
