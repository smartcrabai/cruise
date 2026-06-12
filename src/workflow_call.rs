use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use indexmap::IndexMap;

use crate::config::{FailAction, StepConfig, WorkflowConfig};
use crate::error::{CruiseError, Result};

const GITHUB_BLOB_PREFIX: &str = "https://github.com/";
const GITHUB_RAW_PREFIX: &str = "https://raw.githubusercontent.com/";
const GH_COMMAND: &str = "gh";
const STEP_ID_SEPARATOR: &str = "/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubWorkflowRef {
    pub owner: String,
    pub repo: String,
    pub git_ref: String,
    pub path: String,
}

/// Load a workflow YAML file and resolve any `workflow_call` steps it contains.
///
/// # Errors
///
/// Returns an error when the workflow file cannot be read or parsed, when a
/// referenced workflow cannot be loaded, or when resolving calls detects an
/// invalid or cyclic workflow graph.
pub fn resolve_workflow_calls_from_path(path: impl Into<PathBuf>) -> Result<WorkflowConfig> {
    let path = path.into();
    let mut stack = CallStack::default();
    load_local_workflow(&path, &mut stack)
}

/// Resolve `workflow_call` steps in an already parsed config using the supplied
/// base directory for relative local calls.
///
/// # Errors
///
/// Returns an error when a referenced workflow cannot be loaded, when call-site
/// fields are invalid, or when resolving calls detects a cyclic workflow graph.
pub fn resolve_workflow_calls(
    config: WorkflowConfig,
    base_dir: impl Into<PathBuf>,
) -> Result<WorkflowConfig> {
    let base_dir = base_dir.into();
    let mut stack = CallStack::default();
    resolve_workflow_calls_inner(config, &base_dir, &mut stack)
}

/// Parse a supported GitHub workflow URL into repository, ref, and path parts.
///
/// # Errors
///
/// Returns an error when the URL is not a supported GitHub `blob` URL or
/// `raw.githubusercontent.com` URL.
pub fn parse_github_workflow_url(url: &str) -> Result<GitHubWorkflowRef> {
    if let Some(rest) = url.strip_prefix(GITHUB_BLOB_PREFIX) {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 5 && parts[2] == "blob" {
            return Ok(GitHubWorkflowRef {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
                git_ref: parts[3].to_string(),
                path: parts[4..].join(STEP_ID_SEPARATOR),
            });
        }
    }

    if let Some(rest) = url.strip_prefix(GITHUB_RAW_PREFIX) {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 4 {
            return Ok(GitHubWorkflowRef {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
                git_ref: parts[2].to_string(),
                path: parts[3..].join(STEP_ID_SEPARATOR),
            });
        }
    }

    Err(CruiseError::InvalidStepConfig(format!(
        "unsupported workflow_call GitHub URL: {url}"
    )))
}

#[derive(Default)]
struct CallStack {
    local: Vec<PathBuf>,
    github: Vec<GitHubWorkflowRef>,
}

fn load_local_workflow(path: &Path, stack: &mut CallStack) -> Result<WorkflowConfig> {
    let canonical = path.canonicalize().map_err(|e| {
        CruiseError::Other(format!(
            "failed to resolve workflow_call file '{}': {e}",
            path.display()
        ))
    })?;

    if stack.local.contains(&canonical) {
        return Err(CruiseError::InvalidStepConfig(format!(
            "workflow_call cycle detected at '{}'",
            canonical.display()
        )));
    }

    stack.local.push(canonical.clone());
    let yaml = std::fs::read_to_string(&canonical)?;
    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    let base_dir = canonical
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let resolved = resolve_workflow_calls_inner(config, &base_dir, stack);
    stack.local.pop();
    resolved
}

fn resolve_workflow_calls_inner(
    mut config: WorkflowConfig,
    base_dir: &Path,
    stack: &mut CallStack,
) -> Result<WorkflowConfig> {
    for (group_name, group) in &mut config.groups {
        group.steps = resolve_step_map(
            std::mem::take(&mut group.steps),
            base_dir,
            stack,
            Some(group_name.as_str()),
        )?;
    }
    config.steps = resolve_step_map(config.steps, base_dir, stack, None)?;
    config.after_pr = resolve_step_map(config.after_pr, base_dir, stack, None)?;
    Ok(config)
}

fn resolve_step_map(
    steps: IndexMap<String, StepConfig>,
    base_dir: &Path,
    stack: &mut CallStack,
    group_name: Option<&str>,
) -> Result<IndexMap<String, StepConfig>> {
    let mut resolved = IndexMap::new();

    for (step_name, step) in steps {
        if step.workflow_call.is_none() {
            insert_unique(&mut resolved, step_name, step)?;
            continue;
        }

        if let Some(group_name) = group_name {
            return Err(CruiseError::InvalidStepConfig(format!(
                "group step '{group_name}/{step_name}' uses workflow_call, which is not supported inside groups"
            )));
        }

        validate_call_site(&step_name, &step)?;
        let callee = load_called_workflow(
            step.workflow_call.as_deref().ok_or_else(|| {
                CruiseError::InvalidStepConfig("missing workflow_call".to_string())
            })?,
            base_dir,
            stack,
        )?;
        let expanded = expand_called_workflow(&step_name, &step, callee)?;
        for (expanded_name, expanded_step) in expanded {
            insert_unique(&mut resolved, expanded_name, expanded_step)?;
        }
    }

    Ok(resolved)
}

fn validate_call_site(step_name: &str, step: &StepConfig) -> Result<()> {
    let mut invalid_fields = Vec::new();
    if step.model.is_some() {
        invalid_fields.push("model");
    }
    if step.prompt.is_some() {
        invalid_fields.push("prompt");
    }
    if step.instruction.is_some() {
        invalid_fields.push("instruction");
    }
    if step.plan.is_some() {
        invalid_fields.push("plan");
    }
    if step.option.is_some() {
        invalid_fields.push("option");
    }
    if step.command.is_some() {
        invalid_fields.push("command");
    }
    if step.group.is_some() {
        invalid_fields.push("group");
    }
    if step.if_condition.is_some() {
        invalid_fields.push("if");
    }
    if step.timeout.is_some() {
        invalid_fields.push("timeout");
    }
    if !step.env.is_empty() {
        invalid_fields.push("env");
    }
    if step.fail_if_no_file_changes {
        invalid_fields.push("fail-if-no-file-changes");
    }

    if invalid_fields.is_empty() {
        return Ok(());
    }

    Err(CruiseError::InvalidStepConfig(format!(
        "step '{step_name}' uses workflow_call with unsupported field(s): {}",
        invalid_fields.join(", ")
    )))
}

fn load_called_workflow(
    workflow_call: &str,
    base_dir: &Path,
    stack: &mut CallStack,
) -> Result<WorkflowConfig> {
    if workflow_call.starts_with(GITHUB_BLOB_PREFIX) || workflow_call.starts_with(GITHUB_RAW_PREFIX)
    {
        let parsed = parse_github_workflow_url(workflow_call)?;
        return load_github_workflow(&parsed, stack);
    }

    let base_dir_string = base_dir.to_string_lossy();
    if base_dir_string.starts_with(GITHUB_BLOB_PREFIX)
        || base_dir_string.starts_with(GITHUB_RAW_PREFIX)
    {
        let url = github_relative_workflow_url(&base_dir_string, workflow_call);
        let parsed = parse_github_workflow_url(&url)?;
        return load_github_workflow(&parsed, stack);
    }

    let path = base_dir.join(workflow_call);
    load_local_workflow(&path, stack)
}

fn github_relative_workflow_url(base_dir: &str, workflow_call: &str) -> String {
    let mut path_parts: Vec<&str> = base_dir.trim_end_matches('/').split('/').collect();
    for part in workflow_call.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path_parts.pop();
            }
            _ => path_parts.push(part),
        }
    }
    path_parts.join("/")
}

fn load_github_workflow(
    reference: &GitHubWorkflowRef,
    stack: &mut CallStack,
) -> Result<WorkflowConfig> {
    if stack.github.contains(reference) {
        return Err(CruiseError::InvalidStepConfig(format!(
            "workflow_call cycle detected at GitHub workflow '{}', ref '{}'",
            reference.path, reference.git_ref
        )));
    }

    stack.github.push(reference.clone());
    let yaml = fetch_github_workflow(reference)?;
    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    let remote_base = github_workflow_base_url(reference);
    let resolved = resolve_workflow_calls_inner(config, Path::new(&remote_base), stack);
    stack.github.pop();
    resolved
}

fn github_workflow_base_url(reference: &GitHubWorkflowRef) -> String {
    let parent = Path::new(&reference.path)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("");
    if parent.is_empty() {
        format!(
            "https://raw.githubusercontent.com/{}/{}/{}",
            reference.owner, reference.repo, reference.git_ref
        )
    } else {
        format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            reference.owner, reference.repo, reference.git_ref, parent
        )
    }
}

fn fetch_github_workflow(reference: &GitHubWorkflowRef) -> Result<String> {
    let output = Command::new(GH_COMMAND)
        .args([
            "api",
            &format!(
                "repos/{}/{}/contents/{}?ref={}",
                reference.owner, reference.repo, reference.path, reference.git_ref
            ),
            "--jq",
            ".content",
        ])
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh for workflow_call: {e}")))?;

    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "failed to fetch workflow_call from GitHub: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let encoded = String::from_utf8_lossy(&output.stdout).replace(['\n', '\r'], "");
    let decoded = decode_base64(&encoded).ok_or_else(|| {
        CruiseError::Other("failed to decode GitHub workflow_call content".to_string())
    })?;
    String::from_utf8(decoded).map_err(|e| {
        CruiseError::Other(format!(
            "GitHub workflow_call content is not valid UTF-8: {e}"
        ))
    })
}

fn expand_called_workflow(
    call_site: &str,
    call_step: &StepConfig,
    callee: WorkflowConfig,
) -> Result<IndexMap<String, StepConfig>> {
    if !callee.groups.is_empty() {
        return Err(CruiseError::InvalidStepConfig(format!(
            "workflow_call step '{call_site}' references a workflow that defines groups; groups inside called workflows are not supported"
        )));
    }

    let compiled = crate::workflow::compile(callee)?;
    let original_ids: HashSet<String> = compiled.steps.keys().cloned().collect();
    let first_id = compiled.steps.keys().next().cloned();
    let last_id = compiled.steps.keys().last().cloned();
    let mut expanded = IndexMap::new();

    for (original_id, mut step) in compiled.steps {
        step.workflow_call = None;
        rewrite_internal_references(&mut step, call_site, &original_ids);

        if first_id.as_deref() == Some(original_id.as_str()) {
            step.skip.clone_from(&call_step.skip);
            step.when.clone_from(&call_step.when);
        }
        if last_id.as_deref() == Some(original_id.as_str()) && step.next.is_none() {
            step.next.clone_from(&call_step.next);
        }

        let expanded_id = prefixed_step_id(call_site, &original_id);
        expanded.insert(expanded_id, step);
    }

    Ok(expanded)
}

fn rewrite_internal_references(
    step: &mut StepConfig,
    call_site: &str,
    original_ids: &HashSet<String>,
) {
    rewrite_optional_step_ref(&mut step.next, call_site, original_ids);

    if let Some(options) = step.option.as_mut() {
        for option in options {
            rewrite_optional_step_ref(&mut option.next, call_site, original_ids);
        }
    }

    if let Some(if_condition) = step.if_condition.as_mut() {
        rewrite_optional_step_ref(&mut if_condition.file_changed, call_site, original_ids);
        if let Some(FailAction::Goto(next)) = if_condition.fail.as_mut()
            && original_ids.contains(next)
        {
            *next = prefixed_step_id(call_site, next);
        }
    }
}

fn rewrite_optional_step_ref(
    step_ref: &mut Option<String>,
    call_site: &str,
    original_ids: &HashSet<String>,
) {
    if let Some(value) = step_ref.as_mut()
        && original_ids.contains(value)
    {
        *value = prefixed_step_id(call_site, value);
    }
}

fn prefixed_step_id(call_site: &str, step_id: &str) -> String {
    format!("{call_site}{STEP_ID_SEPARATOR}{step_id}")
}

fn insert_unique(
    steps: &mut IndexMap<String, StepConfig>,
    name: String,
    step: StepConfig,
) -> Result<()> {
    if steps.contains_key(&name) {
        return Err(CruiseError::InvalidStepConfig(format!(
            "expanded workflow_call step key '{name}' collides with an existing step name"
        )));
    }
    steps.insert(name, step);
    Ok(())
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    const TABLE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u8;

    for byte in input.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = u32::try_from(TABLE.bytes().position(|candidate| candidate == byte)?)
            .unwrap_or_else(|_| unreachable!("base64 alphabet index always fits in u32"));
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }

    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FailAction, StringOrVec, WorkflowConfig};
    use tempfile::TempDir;

    fn write_file(dir: &TempDir, relative: &str, contents: &str) -> PathBuf {
        let path = dir.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create dir failed: {e}"));
        }
        std::fs::write(&path, contents).unwrap_or_else(|e| panic!("write file failed: {e}"));
        path
    }

    fn resolved_from_path(path: PathBuf) -> WorkflowConfig {
        resolve_workflow_calls_from_path(path).unwrap_or_else(|e| panic!("unexpected error: {e:?}"))
    }

    #[test]
    fn test_resolve_local_workflow_call_expands_steps_with_call_site_prefix() {
        // Given: a parent workflow calls a relative workflow file.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "workflows/review.yaml",
            r"
command: [ignored-command]
env:
  IGNORED: true
steps:
  simplify:
    prompt: /simplify
  decide:
    command: echo decide
",
        );
        let parent = write_file(
            &dir,
            "cruise.yaml",
            r"
command: [parent-command]
env:
  PARENT: kept
steps:
  build:
    command: cargo build
  shared-review:
    workflow_call: ./workflows/review.yaml
  deploy:
    command: cargo publish
",
        );

        // When: workflow calls are resolved.
        let config = resolved_from_path(parent);

        // Then: the call-site is replaced by the callee steps in order.
        let keys: Vec<&str> = config.steps.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "build",
                "shared-review/simplify",
                "shared-review/decide",
                "deploy"
            ]
        );
        assert_eq!(
            config.steps["shared-review/simplify"].prompt.as_deref(),
            Some("/simplify")
        );
        assert!(config.steps["shared-review/decide"].command.is_some());
    }

    #[test]
    fn test_resolve_workflow_call_ignores_callee_top_level_execution_settings() {
        // Given: the called workflow declares its own execution backend and environment.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "callee.yaml",
            r"
command: [callee-command]
sdk: ignored-sdk
model: ignored-model
plan_model: ignored-plan-model
pr_language: Japanese
env:
  CALLEE_ONLY: ignored
llm:
  endpoint: https://ignored.example.test/v1
steps:
  review:
    prompt: /review
",
        );
        let parent = write_file(
            &dir,
            "parent.yaml",
            r"
command: [parent-command]
model: parent-model
plan_model: parent-plan-model
pr_language: English
env:
  PARENT_ONLY: kept
steps:
  shared:
    workflow_call: ./callee.yaml
",
        );

        // When: workflow calls are resolved.
        let config = resolved_from_path(parent);

        // Then: parent top-level settings are retained and callee settings are ignored.
        assert_eq!(config.command, vec!["parent-command".to_string()]);
        assert_eq!(config.sdk, None);
        assert_eq!(config.model.as_deref(), Some("parent-model"));
        assert_eq!(config.plan_model.as_deref(), Some("parent-plan-model"));
        assert_eq!(config.pr_language, "English");
        assert_eq!(
            config.env.get("PARENT_ONLY").map(String::as_str),
            Some("kept")
        );
        assert!(!config.env.contains_key("CALLEE_ONLY"));
    }

    #[test]
    fn test_resolve_workflow_call_rewrites_internal_transitions_to_expanded_ids() {
        // Given: a callee uses next, option next, if.file-changed, and if.fail references internally.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "review.yaml",
            r"
command: [ignored]
steps:
  first:
    command: echo first
    next: choose
  choose:
    option:
      - selector: retry
        next: first
      - selector: finish
  verify:
    command: echo verify
    if:
      file-changed: first
      fail: choose
",
        );
        let parent = write_file(
            &dir,
            "parent.yaml",
            r"
command: [parent]
steps:
  review-pass:
    workflow_call: ./review.yaml
",
        );

        // When: workflow calls are resolved.
        let config = resolved_from_path(parent);

        // Then: all internal step references point at expanded step IDs.
        assert_eq!(
            config.steps["review-pass/first"].next.as_deref(),
            Some("review-pass/choose")
        );
        let option = config.steps["review-pass/choose"]
            .option
            .as_ref()
            .unwrap_or_else(|| panic!("missing option step"));
        assert_eq!(option[0].next.as_deref(), Some("review-pass/first"));
        assert_eq!(option[1].next, None);
        let if_condition = config.steps["review-pass/verify"]
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("missing if condition"));
        assert_eq!(
            if_condition.file_changed.as_deref(),
            Some("review-pass/first")
        );
        match if_condition
            .fail
            .as_ref()
            .unwrap_or_else(|| panic!("missing fail action"))
        {
            FailAction::Goto(next) => assert_eq!(next, "review-pass/choose"),
            FailAction::Detailed(_) => panic!("expected goto fail action"),
        }
    }

    #[test]
    fn test_resolve_nested_workflow_call_uses_nested_file_base_directory() {
        // Given: parent -> nested/outer.yaml -> inner/leaf.yaml.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "nested/inner/leaf.yaml",
            r"
command: [ignored]
steps:
  leaf:
    command: echo leaf
",
        );
        write_file(
            &dir,
            "nested/outer.yaml",
            r"
command: [ignored]
steps:
  leaf-call:
    workflow_call: ./inner/leaf.yaml
",
        );
        let parent = write_file(
            &dir,
            "parent.yaml",
            r"
command: [parent]
steps:
  outer-call:
    workflow_call: ./nested/outer.yaml
",
        );

        // When: workflow calls are resolved.
        let config = resolved_from_path(parent);

        // Then: nested relative paths resolve from the file that contains the call.
        let keys: Vec<&str> = config.steps.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["outer-call/leaf-call/leaf"]);
    }

    #[test]
    fn test_resolve_workflow_call_detects_cycles() {
        // Given: two local workflows call each other.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let a = write_file(
            &dir,
            "a.yaml",
            r"
command: [a]
steps:
  b:
    workflow_call: ./b.yaml
",
        );
        write_file(
            &dir,
            "b.yaml",
            r"
command: [b]
steps:
  a:
    workflow_call: ./a.yaml
",
        );

        // When: workflow calls are resolved.
        let Err(err) = resolve_workflow_calls_from_path(a) else {
            panic!("expected workflow_call cycle to be rejected");
        };

        // Then: the error explains that a cycle was found.
        let msg = err.to_string();
        assert!(
            msg.contains("cycle") && msg.contains("workflow_call"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn test_parse_github_blob_workflow_url() {
        // Given: a supported github.com blob URL.
        let url = "https://github.com/org/repo/blob/main/workflows/review.yaml";

        // When: parsed.
        let parsed =
            parse_github_workflow_url(url).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: owner, repo, ref, and path are extracted for `gh api` fetching.
        assert_eq!(
            parsed,
            GitHubWorkflowRef {
                owner: "org".to_string(),
                repo: "repo".to_string(),
                git_ref: "main".to_string(),
                path: "workflows/review.yaml".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_github_raw_workflow_url() {
        // Given: a supported raw.githubusercontent.com URL.
        let url = "https://raw.githubusercontent.com/org/repo/feature-branch/workflows/review.yaml";

        // When: parsed.
        let parsed =
            parse_github_workflow_url(url).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: owner, repo, ref, and path are extracted for `gh api` fetching.
        assert_eq!(
            parsed,
            GitHubWorkflowRef {
                owner: "org".to_string(),
                repo: "repo".to_string(),
                git_ref: "feature-branch".to_string(),
                path: "workflows/review.yaml".to_string(),
            }
        );
    }

    #[test]
    fn test_github_relative_workflow_url_resolves_from_remote_directory() {
        let base = "https://raw.githubusercontent.com/org/repo/main/workflows/nested";

        assert_eq!(
            github_relative_workflow_url(base, "./shared.yaml"),
            "https://raw.githubusercontent.com/org/repo/main/workflows/nested/shared.yaml"
        );
        assert_eq!(
            github_relative_workflow_url(base, "../common/shared.yaml"),
            "https://raw.githubusercontent.com/org/repo/main/workflows/common/shared.yaml"
        );
    }

    #[test]
    fn test_resolve_rejects_workflow_call_inside_group_steps() {
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
groups:
  review:
    steps:
      shared:
        workflow_call: ./shared.yaml
steps:
  review-pass:
    group: review
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        let Err(err) = resolve_workflow_calls(config, PathBuf::from(".")) else {
            panic!("expected workflow_call in group to be rejected");
        };
        let msg = err.to_string();
        assert!(msg.contains("workflow_call"), "unexpected error: {msg}");
        assert!(msg.contains("inside groups"), "unexpected error: {msg}");
    }

    #[test]
    fn test_resolve_rejects_called_workflow_that_defines_groups() {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "callee.yaml",
            r"
command: [ignored]
groups:
  review:
    steps:
      one:
        command: echo one
steps:
  review-pass:
    group: review
",
        );
        let parent = write_file(
            &dir,
            "parent.yaml",
            r"
command: [parent]
steps:
  shared:
    workflow_call: ./callee.yaml
",
        );

        let Err(err) = resolve_workflow_calls_from_path(parent) else {
            panic!("expected called workflow with groups to be rejected");
        };
        let msg = err.to_string();
        assert!(msg.contains("groups"), "unexpected error: {msg}");
        assert!(msg.contains("called workflows"), "unexpected error: {msg}");
    }

    #[test]
    fn test_resolve_rejects_call_site_mixed_with_executable_step_fields() {
        // Given: a call-site also declares a command, which would be ambiguous.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  mixed:
    workflow_call: ./callee.yaml
    command: echo ambiguous
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: workflow calls are resolved.
        let Err(err) = resolve_workflow_calls(config, dir.path()) else {
            panic!("expected mixed workflow_call step to be rejected");
        };

        // Then: the error names the ambiguous call-site fields.
        let msg = err.to_string();
        assert!(
            msg.contains("workflow_call"),
            "unexpected error message: {msg}"
        );
        assert!(msg.contains("command"), "unexpected error message: {msg}");
    }

    #[test]
    fn test_resolve_workflow_call_allows_call_site_skip_when_and_next() {
        // Given: a pure call-site uses only workflow_call plus supported orchestration fields.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "callee.yaml",
            r"
command: [ignored]
steps:
  one:
    command: echo one
",
        );
        let parent_yaml = r"
command: [parent]
steps:
  maybe-review:
    workflow_call: ./callee.yaml
    skip: false
    when:
      exists: src/**/*.rs
    next: done
  done:
    command: echo done
";
        let config =
            WorkflowConfig::from_yaml(parent_yaml).unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: workflow calls are resolved from an already parsed config.
        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: the expanded first step inherits call-site skip/when and the expanded last step jumps to parent next.
        let first = &resolved.steps["maybe-review/one"];
        assert!(first.skip.is_some());
        assert!(first.when.is_some());
        assert_eq!(first.next.as_deref(), Some("done"));
    }

    #[test]
    fn test_workflow_call_field_deserializes_and_serializes_as_snake_case() {
        // Given: YAML with the new workflow_call field.
        let yaml = r"
command: [parent]
steps:
  shared:
    workflow_call: ./shared.yaml
";

        // When: parsed and serialized back to YAML.
        let config =
            WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("parse failed: {e}"));
        let serialized =
            serde_yaml::to_string(&config).unwrap_or_else(|e| panic!("serialize failed: {e}"));

        // Then: the field is preserved as workflow_call.
        assert_eq!(
            config.steps["shared"].workflow_call.as_deref(),
            Some("./shared.yaml")
        );
        assert!(
            serialized.contains("workflow_call"),
            "serialized YAML was: {serialized}"
        );
    }

    #[test]
    fn test_resolved_workflow_call_compiles_to_flat_executable_steps() {
        // Given: a parent workflow calls another workflow.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "callee.yaml",
            r"
command: [ignored]
steps:
  test:
    command: cargo test
",
        );
        let parent = write_file(
            &dir,
            "parent.yaml",
            r"
command: [parent]
steps:
  shared:
    workflow_call: ./callee.yaml
",
        );

        // When: workflow calls are resolved and compiled.
        let config = resolved_from_path(parent);
        let compiled =
            crate::workflow::compile(config).unwrap_or_else(|e| panic!("compile failed: {e:?}"));

        // Then: the engine-facing workflow contains only executable steps, not workflow_call placeholders.
        let keys: Vec<&str> = compiled.steps.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["shared/test"]);
        assert!(compiled.steps["shared/test"].workflow_call.is_none());
        assert!(matches!(
            compiled.steps["shared/test"].command,
            Some(StringOrVec::Single(_) | StringOrVec::Multiple(_))
        ));
    }
}
