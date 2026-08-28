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

/// Load a workflow YAML file, resolve any `workflow_call` steps it contains, and
/// inline the contents of any `prompt_file` fields.
///
/// # Errors
///
/// Returns an error when the workflow file cannot be read or parsed, when a
/// referenced workflow or prompt file cannot be loaded, or when resolving calls
/// detects an invalid or cyclic workflow graph.
pub fn resolve_workflow_calls_from_path(path: impl Into<PathBuf>) -> Result<WorkflowConfig> {
    let path = path.into();
    let mut stack = CallStack::default();
    let config = load_local_workflow(&path, &mut stack)?;
    finish_resolved_config(config)
}

/// Resolve `workflow_call` steps and `prompt_file` fields in an already parsed
/// config using the supplied base directory for relative local paths.
///
/// # Errors
///
/// Returns an error when a referenced workflow or prompt file cannot be loaded,
/// when call-site fields are invalid, or when resolving calls detects a cyclic
/// workflow graph.
pub fn resolve_workflow_calls(
    config: WorkflowConfig,
    base_dir: impl Into<PathBuf>,
) -> Result<WorkflowConfig> {
    let base_dir = base_dir.into();
    let mut stack = CallStack::default();
    let config = resolve_workflow_calls_inner(config, &base_dir, &mut stack)?;
    finish_resolved_config(config)
}

fn finish_resolved_config(mut config: WorkflowConfig) -> Result<WorkflowConfig> {
    for warning in config.deprecated_language_warnings() {
        eprintln!("warning: {warning}");
    }
    config.apply_env_overrides()?;
    Ok(config)
}

/// Parse a supported GitHub blob/raw URL into repository, ref, and path parts.
///
/// # Errors
///
/// Returns an error when the URL is not a supported GitHub `blob` URL or
/// `raw.githubusercontent.com` URL.
pub fn parse_github_workflow_url(url: &str) -> Result<GitHubWorkflowRef> {
    parse_github_url(url, "workflow_call")
}

fn parse_github_url(url: &str, field: &str) -> Result<GitHubWorkflowRef> {
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
        "unsupported {field} GitHub URL: {url}"
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
        let mut step = step;
        if step.workflow_call.is_none() {
            inline_prompt_file(&step_name, &mut step, base_dir)?;
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
    let invalid_fields: Vec<&str> = [
        ("model", step.model.is_some()),
        ("prompt", step.prompt.is_some()),
        ("prompt_file", step.prompt_file.is_some()),
        ("instruction", step.instruction.is_some()),
        ("plan", step.plan.is_some()),
        ("option", step.option.is_some()),
        ("command", step.command.is_some()),
        ("group", step.group.is_some()),
        ("if", step.if_condition.is_some()),
        ("timeout", step.timeout.is_some()),
        ("env", !step.env.is_empty()),
        ("fail-if-no-file-changes", step.fail_if_no_file_changes),
    ]
    .into_iter()
    .filter_map(|(field, present)| present.then_some(field))
    .collect();

    if invalid_fields.is_empty() {
        return Ok(());
    }

    Err(CruiseError::InvalidStepConfig(format!(
        "step '{step_name}' uses workflow_call with unsupported field(s): {}",
        invalid_fields.join(", ")
    )))
}

fn inline_prompt_file(step_name: &str, step: &mut StepConfig, base_dir: &Path) -> Result<()> {
    let Some(prompt_file) = step.prompt_file.take() else {
        return Ok(());
    };

    if step.prompt.is_some() {
        return Err(CruiseError::InvalidStepConfig(format!(
            "step '{step_name}' specifies both `prompt` and `prompt_file`; use only one"
        )));
    }
    if prompt_file.trim().is_empty() {
        return Err(CruiseError::InvalidStepConfig(format!(
            "step '{step_name}' has empty prompt_file"
        )));
    }

    step.prompt = Some(load_prompt_file(step_name, &prompt_file, base_dir)?);
    Ok(())
}

fn load_prompt_file(step_name: &str, prompt_file: &str, base_dir: &Path) -> Result<String> {
    if is_github_url(&base_dir.to_string_lossy()) && is_home_prompt_path(prompt_file) {
        return Err(CruiseError::Other(
            "`~` is not supported for prompt_file in GitHub-hosted workflows".to_string(),
        ));
    }
    if let Some(reference) = resolve_github_workflow_ref(prompt_file, base_dir, "prompt_file")? {
        return fetch_github_file(&reference, "prompt_file").map(|(_, content)| content);
    }

    let path = resolve_local_prompt_path(prompt_file, base_dir);
    std::fs::read_to_string(&path).map_err(|e| {
        CruiseError::Other(format!(
            "failed to read prompt_file '{}' for step '{step_name}': {e}",
            path.display()
        ))
    })
}

fn resolve_local_prompt_path(prompt_file: &str, base_dir: &Path) -> PathBuf {
    base_dir.join(crate::new_session_history::expand_tilde(prompt_file))
}

fn is_home_prompt_path(prompt_file: &str) -> bool {
    if prompt_file == "~" || prompt_file.starts_with("~/") {
        return true;
    }

    #[cfg(unix)]
    if let Some(rest) = prompt_file.strip_prefix('~') {
        let username = rest.split('/').next().unwrap_or(rest);
        return !username.is_empty() && users::get_user_by_name(username).is_some();
    }

    false
}

fn is_github_url(value: &str) -> bool {
    value.starts_with(GITHUB_BLOB_PREFIX) || value.starts_with(GITHUB_RAW_PREFIX)
}

fn resolve_github_workflow_ref(
    path: &str,
    base_dir: &Path,
    field: &str,
) -> Result<Option<GitHubWorkflowRef>> {
    let base_dir = base_dir.to_string_lossy();
    let url = if is_github_url(path) {
        path.to_string()
    } else if Path::new(path).is_absolute() {
        return Ok(None);
    } else if is_github_url(&base_dir) {
        github_relative_workflow_url(&base_dir, path)
    } else {
        return Ok(None);
    };
    let reference = parse_github_url(&url, field)?;
    Ok(Some(reference))
}

fn load_called_workflow(
    workflow_call: &str,
    base_dir: &Path,
    stack: &mut CallStack,
) -> Result<WorkflowConfig> {
    if let Some(reference) = resolve_github_workflow_ref(workflow_call, base_dir, "workflow_call")?
    {
        return load_github_workflow(&reference, stack);
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
                // Keep relative remote references inside the same repository and
                // ref. The URL prefix has six components through the ref:
                // `https://raw.githubusercontent.com/{owner}/{repo}/{ref}`.
                if path_parts.len() > 6 {
                    path_parts.pop();
                }
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
    let (resolved_reference, yaml) = fetch_github_file(reference, "workflow_call")?;
    if stack.github[..stack.github.len() - 1].contains(&resolved_reference) {
        stack.github.pop();
        return Err(CruiseError::InvalidStepConfig(format!(
            "workflow_call cycle detected at GitHub workflow '{}', ref '{}'",
            resolved_reference.path, resolved_reference.git_ref
        )));
    }
    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    let remote_base = github_workflow_base_url(&resolved_reference);
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

fn fetch_github_file(
    reference: &GitHubWorkflowRef,
    what: &str,
) -> Result<(GitHubWorkflowRef, String)> {
    match fetch_github_file_once(reference, what) {
        Ok(content) => Ok((reference.clone(), content)),
        Err(first_error) => {
            // GitHub URLs do not delimit a slash-containing ref from the file
            // path. Retry longer ref prefixes only when the conventional
            // single-segment interpretation was not found.
            let mut last_error = first_error;
            for candidate in github_ref_candidates(reference).into_iter().skip(1) {
                match fetch_github_file_once(&candidate, what) {
                    Ok(content) => return Ok((candidate, content)),
                    Err(error) => last_error = error,
                }
            }
            Err(last_error)
        }
    }
}

fn github_ref_candidates(reference: &GitHubWorkflowRef) -> Vec<GitHubWorkflowRef> {
    let path_parts: Vec<&str> = reference.path.split('/').collect();
    let mut candidates = vec![reference.clone()];
    let mut ref_parts = vec![reference.git_ref.as_str()];

    for split in 0..path_parts.len().saturating_sub(1) {
        ref_parts.push(path_parts[split]);
        let path = path_parts[split + 1..].join(STEP_ID_SEPARATOR);
        candidates.push(GitHubWorkflowRef {
            owner: reference.owner.clone(),
            repo: reference.repo.clone(),
            git_ref: ref_parts.join(STEP_ID_SEPARATOR),
            path,
        });
    }

    candidates
}

fn fetch_github_file_once(reference: &GitHubWorkflowRef, what: &str) -> Result<String> {
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
        .map_err(|e| CruiseError::Other(format!("failed to run gh for {what}: {e}")))?;

    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "failed to fetch {what} from GitHub: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let decoded = decode_base64(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| CruiseError::Other(format!("failed to decode GitHub {what} content")))?;
    String::from_utf8(decoded)
        .map_err(|e| CruiseError::Other(format!("GitHub {what} content is not valid UTF-8: {e}")))
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
        debug_assert!(step.prompt_file.is_none());
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
    let mut encoded_chars = 0_usize;

    for byte in input.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        encoded_chars += 1;
        let value = u32::try_from(TABLE.bytes().position(|candidate| candidate == byte)?)
            .unwrap_or_else(|_| unreachable!("base64 alphabet index always fits in u32"));
        buffer = (buffer << 6) | value;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
        // Retain only the unconsumed low bits so the accumulator stays bounded.
        buffer = if bits == 0 {
            0
        } else {
            buffer & ((1_u32 << bits) - 1)
        };
    }

    // A single trailing Base64 character cannot form a complete quantum.
    if encoded_chars % 4 == 1 {
        return None;
    }

    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FailAction, StringOrVec, WorkflowConfig};
    use crate::test_support::{EnvGuard, lock_process, prepend_to_path};
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

    #[cfg(unix)]
    fn install_gh_stub(dir: &TempDir, script: &str) -> EnvGuard {
        use std::os::unix::fs::PermissionsExt;

        let gh = dir.path().join("gh");
        std::fs::write(&gh, script).unwrap_or_else(|e| panic!("write gh stub failed: {e}"));
        let mut permissions = std::fs::metadata(&gh)
            .unwrap_or_else(|e| panic!("stat gh stub failed: {e}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&gh, permissions)
            .unwrap_or_else(|e| panic!("chmod gh stub failed: {e}"));
        prepend_to_path(dir.path())
    }

    fn clear_all_override_envs() -> Vec<EnvGuard> {
        vec![
            EnvGuard::remove("CRUISE_MODEL"),
            EnvGuard::remove("CRUISE_PLAN_MODEL"),
            EnvGuard::remove("CRUISE_SDK"),
            EnvGuard::remove("CRUISE_LANGUAGE_PR"),
            EnvGuard::remove("CRUISE_LANGUAGE_PLAN"),
            EnvGuard::remove("CRUISE_CLEANUP_AFTER_PR"),
            EnvGuard::remove("CRUISE_INTERACTIVE_PLANNING"),
        ]
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
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
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
plan_language: Japanese
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
plan_language: English
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
        assert_eq!(config.pr_language.as_deref(), Some("English"));
        assert_eq!(config.plan_language.as_deref(), Some("English"));
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

    #[cfg(unix)]
    #[test]
    fn test_remote_prompt_file_fetches_and_decodes_content() {
        let _lock = lock_process();
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let _path = install_gh_stub(
            &dir,
            "#!/bin/sh\nprintf 'c2hhcmVkIHJlbW90ZSBwcm9tcHQK\\n'\n",
        );

        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: prompts/impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        let resolved = resolve_workflow_calls(
            config,
            "https://raw.githubusercontent.com/org/repo/main/workflows",
        )
        .unwrap_or_else(|e| panic!("remote prompt fetch failed: {e:?}"));

        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("shared remote prompt\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_remote_prompt_file_supports_slash_containing_ref() {
        let _lock = lock_process();
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let _path = install_gh_stub(
            &dir,
            "#!/bin/sh\ncase \"$*\" in\n  *'contents/prompts/impl.md?ref=feature/foo'*) printf 'c2xhc2ggcmVmIHBhZ2UK\\n' ;;\n  *) exit 1 ;;\nesac\n",
        );

        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: https://raw.githubusercontent.com/org/repo/feature/foo/prompts/impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("slash ref page\n")
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
        assert_eq!(
            github_relative_workflow_url(base, "../../../../other/repo/main/secret"),
            "https://raw.githubusercontent.com/org/repo/main/other/repo/main/secret"
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

    #[test]
    fn test_resolve_workflow_calls_from_path_applies_env_overrides() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _model = EnvGuard::set("CRUISE_MODEL", "opus");

        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let path = write_file(
            &dir,
            "cruise.yaml",
            r"
command: [claude, -p]
model: sonnet
steps:
  s1:
    command: echo hi
",
        );

        // Given: YAML sets model=sonnet, CRUISE_MODEL env var overrides to opus
        // When: workflow is loaded via resolve_workflow_calls_from_path
        let config = resolved_from_path(path);

        // Then: model reflects the env var override, not the YAML value
        assert_eq!(config.model, Some("opus".to_string()));
    }

    #[test]
    fn test_resolve_workflow_calls_applies_env_overrides() {
        let _lock = lock_process();
        let _guards = clear_all_override_envs();
        let _model = EnvGuard::set("CRUISE_MODEL", "opus");

        let config: WorkflowConfig = serde_yaml::from_str(
            r"
command: [claude, -p]
model: sonnet
steps:
  s1:
    command: echo hi
",
        )
        .unwrap_or_else(|e| panic!("yaml parse failed: {e}"));

        // Given: YAML sets model=sonnet, CRUISE_MODEL env var overrides to opus
        // When: workflow is loaded via resolve_workflow_calls (YAML-string path)
        let resolved = resolve_workflow_calls(config, PathBuf::from("."))
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: model reflects the env var override, not the YAML value
        assert_eq!(resolved.model, Some("opus".to_string()));
    }

    // ---- prompt_file: resolve_local_prompt_path unit tests ----

    #[test]
    fn test_resolve_local_prompt_path_joins_relative_forms_onto_base_dir() {
        // Given: a base directory equal to the config file's directory.
        let base = Path::new("/workflows");

        // When: bare file name, ./-prefixed, subdirectory, and parent-relative
        // forms are resolved.
        // Then: all of them join onto the config directory (join keeps `..` unnormalized).
        for (prompt_file, expected) in [
            ("impl.md", "/workflows/impl.md"),
            ("./prompts/impl.md", "/workflows/prompts/impl.md"),
            ("prompts/impl.md", "/workflows/prompts/impl.md"),
            ("../shared/impl.md", "/workflows/../shared/impl.md"),
            ("~prompts.md", "/workflows/~prompts.md"),
        ] {
            assert_eq!(
                resolve_local_prompt_path(prompt_file, base),
                PathBuf::from(expected)
            );
        }
    }

    #[test]
    fn test_resolve_local_prompt_path_absolute_path_ignores_base_dir() {
        // Given: an absolute prompt file path in a different tree than the base dir.
        let elsewhere = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let absolute = elsewhere.path().join("impl.md");
        let absolute_string = absolute.to_string_lossy().to_string();
        let base = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));

        // When: resolved.
        // Then: the absolute path is adopted as-is (base_dir is ignored).
        assert_eq!(
            resolve_local_prompt_path(&absolute_string, base.path()),
            absolute
        );
    }

    #[test]
    fn test_resolve_local_prompt_path_expands_tilde_before_joining_base_dir() {
        let _lock = lock_process();
        // Given: a fake home directory and an unrelated base dir.
        let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let _guards = crate::test_support::set_fake_home(home.path());
        let base = Path::new("/unrelated/base");

        // When: `~/`-prefixed and bare `~` paths are resolved.
        // Then: they expand to the home directory instead of joining under base_dir.
        assert_eq!(
            resolve_local_prompt_path("~/prompts/impl.md", base),
            home.path().join("prompts/impl.md")
        );
        assert_eq!(resolve_local_prompt_path("~", base), home.path());
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_local_prompt_path_unknown_user_stays_unexpanded() {
        let _lock = lock_process();
        // Given: a `~user/` path whose user does not exist.
        let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let _guards = crate::test_support::set_fake_home(home.path());
        let base = Path::new("/workflows");

        // When: resolved.
        // Then: expand_tilde passes it through and it joins under base_dir verbatim.
        assert_eq!(
            resolve_local_prompt_path("~no_such_user_x/p.md", base),
            PathBuf::from("/workflows/~no_such_user_x/p.md")
        );
    }

    // ---- prompt_file: integration through workflow resolution ----

    #[test]
    fn test_prompt_file_bare_name_resolves_next_to_config_file() {
        // Given: a config referencing a prompt file by bare file name.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "impl.md", "implement it\n");
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved with the config directory as base while the process cwd
        // remains elsewhere (cargo test cwd is the crate root, not this tempdir).
        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: the file next to the config file is read, not one relative to cwd.
        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("implement it\n")
        );
    }

    #[test]
    fn test_prompt_file_dot_prefixed_and_plain_relative_paths_resolve_identically() {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "prompts/impl.md", "shared contents\n");
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  dotted:
    prompt_file: ./prompts/impl.md
  plain:
    prompt_file: prompts/impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));
        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        assert_eq!(
            resolved.steps["dotted"].prompt.as_deref(),
            Some("shared contents\n")
        );
        assert_eq!(
            resolved.steps["plain"].prompt.as_deref(),
            Some("shared contents\n")
        );
    }

    #[test]
    fn test_prompt_file_parent_relative_resolves_from_config_directory() {
        // Given: a nested config that reaches a sibling directory via `../`.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "shared/impl.md", "shared prompt\n");
        let nested_config = write_file(
            &dir,
            "nested/cruise.yaml",
            r"
command: [nested]
steps:
  implement:
    prompt_file: ../shared/impl.md
",
        );

        // When: resolved directly from its path.
        let resolved = resolved_from_path(nested_config);

        // Then: the parent-relative path resolves from the nested config's own directory.
        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("shared prompt\n")
        );
    }

    #[test]
    fn test_prompt_file_absolute_path_ignores_resolution_base_directory() {
        // Given: a prompt file addressed by absolute path while both the config
        // location and the resolution base point somewhere else entirely.
        let file_dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let absolute = write_file(&file_dir, "impl.md", "absolute prompt\n");
        let absolute_string = absolute.to_string_lossy().to_string();
        let yaml = format!(
            r#"
command: [parent]
steps:
  implement:
    prompt_file: "{absolute_string}"
"#
        );
        let config =
            WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("parse failed: {e}"));
        let unrelated_base = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));

        // When: resolved with the unrelated base directory.
        let resolved = resolve_workflow_calls(config, unrelated_base.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: the absolute path wins over the base directory.
        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("absolute prompt\n")
        );
    }

    #[test]
    fn test_decode_base64_handles_long_github_content() {
        // Given: more than four Base64 characters, including padding and a newline.
        // When: GitHub content is decoded.
        let decoded = decode_base64("VGhpcyBpcyBhIGxvbmcgcHJvbXB0Lg==\n");

        // Then: the complete content is returned without accumulator overflow.
        assert_eq!(decoded, Some(b"This is a long prompt.".to_vec()));
    }

    #[test]
    fn test_decode_base64_rejects_incomplete_quantum() {
        assert_eq!(decode_base64("A"), None);
    }

    #[cfg(unix)]
    #[test]
    fn test_home_prompt_path_distinguishes_home_forms_from_tilde_filenames() {
        assert!(is_home_prompt_path("~"));
        assert!(is_home_prompt_path("~/prompts.md"));
        assert!(is_home_prompt_path("~root/prompts.md"));
        assert!(!is_home_prompt_path("~prompts.md"));
    }

    #[test]
    fn test_prompt_file_home_relative_expands_tilde() {
        let _lock = lock_process();
        // Given: `$HOME/prompts/impl.md` exists and the config lives in a separate tree.
        let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let _guards = crate::test_support::set_fake_home(home.path());
        write_file(&home, "prompts/impl.md", "home prompt\n");
        let config_dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let config_path = write_file(
            &config_dir,
            "cruise.yaml",
            r"
command: [parent]
steps:
  implement:
    prompt_file: ~/prompts/impl.md
",
        );

        // When: resolved from its path.
        let resolved = resolved_from_path(config_path);

        // Then: the tilde path expands to the fake home regardless of config location.
        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("home prompt\n")
        );
    }

    #[test]
    fn test_prompt_file_tilde_alone_fails_reading_the_home_directory() {
        let _lock = lock_process();
        // Given: a step whose prompt_file is just `~`, which expands to the home directory.
        let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let _guards = crate::test_support::set_fake_home(home.path());
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let config_path = write_file(
            &dir,
            "cruise.yaml",
            r"
command: [parent]
steps:
  implement:
    prompt_file: ~
",
        );

        // When: resolved.
        // Then: reading a directory fails with the expanded full path in the message.
        let Err(err) = resolve_workflow_calls_from_path(config_path) else {
            panic!("expected reading a directory as prompt_file to fail");
        };
        let msg = err.to_string();
        assert!(msg.contains("prompt_file"), "unexpected error: {msg}");
        assert!(msg.contains("implement"), "unexpected error: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn test_prompt_file_unknown_user_path_errors_with_joined_full_path() {
        let _lock = lock_process();
        // Given: a `~nonexistent-user/` prompt_file path.
        let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let _guards = crate::test_support::set_fake_home(home.path());
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: ~no_such_user_x/p.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved from the config directory.
        // Then: the error reports the joined full path (unexpanded `~user` treated
        // as a literal directory name under the config directory).
        let Err(err) = resolve_workflow_calls(config, dir.path()) else {
            panic!("expected unknown-user prompt_file to fail");
        };
        let msg = err.to_string();
        assert!(
            msg.contains(
                dir.path()
                    .join("~no_such_user_x/p.md")
                    .to_string_lossy()
                    .as_ref()
            ),
            "unexpected error: {msg}"
        );
        assert!(msg.contains("implement"), "unexpected error: {msg}");
    }

    #[test]
    fn test_prompt_file_tilde_prefix_file_name_resolves_next_to_config_file() {
        // Given: a file literally named `~prompts.md` next to the config file.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "~prompts.md", "tilde-named file\n");
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: ~prompts.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved.
        // Then: it is treated as a bare file name in the config directory.
        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("tilde-named file\n")
        );
    }

    #[test]
    fn test_prompt_file_inside_nested_workflow_call_resolves_from_callee_directory() {
        // Given: parent -> nested/outer.yaml, where the callee declares prompt_file
        // and same-named files exist in both directories with different contents.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "p.md", "parent version\n");
        write_file(&dir, "nested/p.md", "child version\n");
        write_file(
            &dir,
            "nested/outer.yaml",
            r"
command: [ignored]
steps:
  implement:
    prompt_file: p.md
",
        );
        let parent = write_file(
            &dir,
            "parent.yaml",
            r"
command: [parent]
steps:
  shared:
    workflow_call: ./nested/outer.yaml
",
        );

        // When: workflow calls are resolved.
        let resolved = resolved_from_path(parent);

        // Then: the callee's prompt_file resolves against the callee's directory,
        // so the child version is picked.
        assert_eq!(
            resolved.steps["shared/implement"].prompt.as_deref(),
            Some("child version\n")
        );
    }

    #[test]
    fn test_prompt_file_expands_for_group_steps_and_after_pr_steps() {
        // Given: prompt_file used inside a group step block and in after-pr.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "group-prompt.md", "group prompt\n");
        write_file(&dir, "after-pr-prompt.md", "after-pr prompt\n");
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
groups:
  review:
    if:
      file-changed: '*.rs'
    max_retries: 1
    steps:
      simplify:
        prompt_file: group-prompt.md
steps:
  prep:
    command: echo prep
  review-pass:
    group: review
after-pr:
  cleanup:
    prompt_file: after-pr-prompt.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: workflow calls are resolved.
        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: both locations are inlined.
        assert_eq!(
            resolved.groups["review"].steps["simplify"]
                .prompt
                .as_deref(),
            Some("group prompt\n")
        );
        assert_eq!(
            resolved.after_pr["cleanup"].prompt.as_deref(),
            Some("after-pr prompt\n")
        );
    }

    #[test]
    fn test_resolved_prompt_file_clears_field_and_sets_prompt_exactly_without_trim() {
        // Given: a prompt file whose content ends with a newline.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "impl.md", "line one\nline two\n\n");
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved and serialized back to YAML.
        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
        let serialized = serde_yaml::to_string(&resolved).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the content is kept verbatim (no trim), the field is cleared, and
        // no `prompt_file` key appears in serialized output (session snapshots).
        let step = &resolved.steps["implement"];
        assert_eq!(step.prompt.as_deref(), Some("line one\nline two\n\n"));
        assert_eq!(step.prompt_file, None);
        assert!(
            !serialized.contains("prompt_file"),
            "serialized YAML was: {serialized}"
        );
    }

    #[test]
    fn test_prompt_file_content_keeps_variable_placeholders_unresolved() {
        // Given: a prompt file containing template variables.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "impl.md", "Implement: {input}\nPlan: {plan}\n");
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved at load time.
        let resolved = resolve_workflow_calls(config, dir.path())
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

        // Then: placeholders survive untouched; variable resolution happens later
        // exactly as for inline `prompt`.
        assert_eq!(
            resolved.steps["implement"].prompt.as_deref(),
            Some("Implement: {input}\nPlan: {plan}\n")
        );
    }

    #[test]
    fn test_step_with_both_prompt_and_prompt_file_is_rejected() {
        // Given: a step specifying both `prompt` and `prompt_file`.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(&dir, "impl.md", "contents\n");
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt: inline
    prompt_file: impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved.
        let Err(err) = resolve_workflow_calls(config, dir.path()) else {
            panic!("expected both-specified step to be rejected");
        };

        // Then: the error names both fields and the offending step.
        let msg = err.to_string();
        assert!(msg.contains("both"), "unexpected error: {msg}");
        assert!(msg.contains("prompt_file"), "unexpected error: {msg}");
        assert!(msg.contains("implement"), "unexpected error: {msg}");
    }

    #[test]
    fn test_empty_prompt_file_value_is_rejected() {
        // Given: steps with an empty and a whitespace-only prompt_file value.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let empty = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: ''
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));
        let whitespace = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: '   '
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: each is resolved.
        let empty_result = resolve_workflow_calls(empty, dir.path());
        let whitespace_result = resolve_workflow_calls(whitespace, dir.path());

        // Then: both are rejected as empty prompt_file values.
        for result in [empty_result, whitespace_result] {
            let Err(err) = result else {
                panic!("expected empty prompt_file to be rejected");
            };
            let msg = err.to_string();
            assert!(msg.contains("empty prompt_file"), "unexpected error: {msg}");
        }
    }

    #[test]
    fn test_missing_prompt_file_error_names_step_and_full_path() {
        // Given: a step pointing at a file that does not exist.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: nope.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved.
        let Err(err) = resolve_workflow_calls(config, dir.path()) else {
            panic!("expected missing prompt_file to be rejected");
        };

        // Then: the error contains the step name and the joined full path.
        let msg = err.to_string();
        assert!(msg.contains("implement"), "unexpected error: {msg}");
        assert!(
            msg.contains(dir.path().join("nope.md").to_string_lossy().as_ref()),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_workflow_call_with_prompt_file_is_rejected_as_unsupported_field() {
        // Given: a call-site that also declares prompt_file.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        write_file(
            &dir,
            "callee.yaml",
            "command: [callee]\nsteps:\n  s:\n    command: echo hi\n",
        );
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  shared:
    workflow_call: ./callee.yaml
    prompt_file: impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: workflow calls are resolved.
        let Err(err) = resolve_workflow_calls(config, dir.path()) else {
            panic!("expected workflow_call + prompt_file to be rejected");
        };

        // Then: the error reports prompt_file among the unsupported fields.
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported field(s)"),
            "unexpected error: {msg}"
        );
        assert!(msg.contains("prompt_file"), "unexpected error: {msg}");
    }

    #[test]
    fn test_non_utf8_prompt_file_is_rejected() {
        // Given: a prompt file containing invalid UTF-8 bytes.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        std::fs::write(dir.path().join("binary.md"), [0xff_u8, 0xfe_u8])
            .unwrap_or_else(|e| panic!("write file failed: {e}"));
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: binary.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));

        // When: resolved.
        let Err(err) = resolve_workflow_calls(config, dir.path()) else {
            panic!("expected non-UTF-8 prompt_file to be rejected");
        };

        // Then: the error mentions the failing read and the file path.
        let msg = err.to_string();
        assert!(msg.contains("prompt_file"), "unexpected error: {msg}");
        assert!(
            msg.contains(dir.path().join("binary.md").to_string_lossy().as_ref()),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_prompt_file_tilde_in_github_hosted_workflow_is_rejected() {
        // Given: a step declared in a GitHub-hosted callee context using a `~/` path.
        let config = WorkflowConfig::from_yaml(
            r"
command: [parent]
steps:
  implement:
    prompt_file: ~/prompts/impl.md
",
        )
        .unwrap_or_else(|e| panic!("parse failed: {e}"));
        let github_base = PathBuf::from("https://raw.githubusercontent.com/org/repo/main");

        // When: resolved against the remote base directory.
        // Then: rejected up front because `~` has no meaning on a remote tree
        // (and no `gh` fetch is attempted).
        let Err(err) = resolve_workflow_calls(config, github_base) else {
            panic!("expected ~/ prompt_file in GitHub-hosted workflow to be rejected");
        };
        let msg = err.to_string();
        assert!(msg.contains("prompt_file"), "unexpected error: {msg}");
        assert!(msg.contains('~'), "unexpected error: {msg}");
    }
}
