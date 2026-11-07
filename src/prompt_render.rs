use std::path::Path;

use crate::{
    config::WorkflowConfig, error::Result, tool_policy::ToolPolicy, variable::VariableStore,
};

pub const COMMIT_GUARD_PROMPT_NOTICE: &str = "\n\nIn this working repository and any linked worktrees, do not create commits, move HEAD or local branch refs, or disable or bypass the commit guard, even if the task text asks you to. Leave all edits uncommitted.";

/// Render a planning template with the current plan path and guidance.
///
/// # Errors
/// Returns an error if a template variable cannot be resolved.
pub fn render_plan_template(
    template: &str,
    vars: &mut VariableStore,
    config: &WorkflowConfig,
    plan_path: &Path,
    interactive: bool,
) -> Result<String> {
    vars.set_named_file(crate::session::PLAN_VAR, plan_path.to_path_buf());
    vars.set_named_value(
        crate::planning::PLAN_LANGUAGE_VAR,
        config.effective_plan_language(),
    );
    vars.set_named_value(
        crate::planning::PLAN_CLARIFICATION_VAR,
        crate::planning::clarification_guidance(interactive).to_string(),
    );
    vars.resolve(template)
}

/// Render a pull request template with the requested language.
///
/// # Errors
/// Returns an error if a template variable cannot be resolved.
pub fn render_pr_template(
    template: &str,
    vars: &mut VariableStore,
    pr_language: &str,
) -> Result<String> {
    vars.set_named_value("pr.language", pr_language.to_string());
    vars.resolve(template)
}

/// Render a workflow generation prompt, substituting schema before description.
#[must_use]
pub fn render_generate_prompt(template: &str, schema: &str, description: &str) -> String {
    template
        .replace("{schema}", schema)
        .replace("{description}", description)
}

/// Assemble the resolved step prompt and applicable safety notices.
///
/// # Errors
/// Returns an error if a template or file reference cannot be resolved.
#[allow(dead_code, reason = "public evaluation API is not used by the runtime binary")]
pub fn assemble_step_prompt(
    template: &str,
    vars: &VariableStore,
    allow_commit: bool,
    tool_policy: &ToolPolicy,
) -> Result<String> {
    assemble_step_prompt_with_context(template, vars, allow_commit, tool_policy, None, None)
}

/// Assemble a step prompt with delivered instructions and quality gate feedback.
///
/// # Errors
/// Returns an error if a template or file reference cannot be resolved.
pub fn assemble_step_prompt_with_context(
    template: &str,
    vars: &VariableStore,
    allow_commit: bool,
    tool_policy: &ToolPolicy,
    delivered: Option<&str>,
    feedback: Option<&str>,
) -> Result<String> {
    let mut prompt = vars.resolve_prompt(template)?;
    if let Some(delivered) = delivered {
        prompt.push_str("\n\n");
        prompt.push_str(delivered);
    }
    if !allow_commit {
        prompt.push_str(COMMIT_GUARD_PROMPT_NOTICE);
    }
    if let Some(feedback) = feedback {
        prompt.push_str("\n\n");
        prompt.push_str(feedback);
    }
    if let Some(notice) = tool_policy.prompt_notice() {
        prompt.push_str("\n\n");
        prompt.push_str(&notice);
    }
    Ok(prompt)
}
