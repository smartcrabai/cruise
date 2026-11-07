//! Authoring-time workflow YAML generation (`cruise workflow generate`).

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::{DEFAULT_RATE_LIMIT_RETRIES, WorkflowGenerateArgs};
use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::executor::{Executor, PromptRun};
use crate::resolver::{load_config_from_source, resolve_config};

const PROMPT_TEMPLATE: &str = include_str!("../prompts/workflow-generate.md");
const SCHEMA: &str = include_str!("../cruise-schema.json");
const MAX_REPAIRS: usize = 3;

/// Distinguish backend failures from exhausted candidate validation.
pub(crate) enum RepairError {
    Backend(CruiseError),
    Rejected(CruiseError),
}

/// Run the initial prompt and up to three repairs, validating each reply.
pub(crate) async fn run_with_repairs<T, Run, Fut, Validate, Repair>(
    mut prompt: String,
    mut run: Run,
    mut validate: Validate,
    mut repair: Repair,
) -> std::result::Result<T, RepairError>
where
    Run: FnMut(String) -> Fut,
    Fut: Future<Output = Result<String>>,
    Validate: FnMut(&str) -> Result<T>,
    Repair: FnMut(&str, &CruiseError) -> String,
{
    for attempt in 0..=MAX_REPAIRS {
        let candidate = run(prompt).await.map_err(RepairError::Backend)?;
        match validate(&candidate) {
            Ok(value) => return Ok(value),
            Err(err) if attempt == MAX_REPAIRS => {
                return Err(RepairError::Rejected(CruiseError::InvalidStepConfig(
                    format!(
                        "generated workflow is still invalid after {MAX_REPAIRS} repairs: {err}"
                    ),
                )));
            }
            Err(err) => prompt = repair(&candidate, &err),
        }
    }
    unreachable!("repair loop always returns on its final attempt")
}

/// Run one stateless authoring prompt with caller-selected permissions.
pub(crate) async fn run_prompt(
    config: &WorkflowConfig,
    prompt: &str,
    permission: crate::config::PermissionMode,
    working_dir: Option<&Path>,
    cancel_token: Option<&crate::cancellation::CancellationToken>,
) -> Result<String> {
    run_prompt_with_usage(config, prompt, permission, working_dir, cancel_token, None).await
}

async fn run_prompt_with_usage(
    config: &WorkflowConfig,
    prompt: &str,
    permission: crate::config::PermissionMode,
    working_dir: Option<&Path>,
    cancel_token: Option<&crate::cancellation::CancellationToken>,
    usage: Option<crate::usage::UsageScope<'_>>,
) -> Result<String> {
    let executor = Executor::new(config.sdk.as_deref(), &config.command);
    let model = Executor::plan_model(config.plan_model.as_ref(), config.model.as_ref());
    let outcome = executor
        .run(PromptRun {
            usage,
            prompt,
            model: model.as_ref(),
            subagent_model: None,
            max_retries: DEFAULT_RATE_LIMIT_RETRIES,
            env: &config.env,
            mcp_servers: &config.mcp_servers,
            on_notice: None,
            cancel_token,
            working_dir,
            stream: None,
            tools: Vec::new(),
            on_session_id: None,
            resume: None,
            permission,
            tool_policy: crate::tool_policy::ToolPolicy::default(),
            computer_use: false,
        })
        .await?;
    Ok(outcome.result.output)
}

/// Generate YAML for `description` through the configured backend, repairing up to three times.
///
/// # Errors
///
/// Returns the backend error, or the last validation error once the initial
/// attempt and three repairs all failed.
pub(crate) async fn generate_yaml(
    config: &WorkflowConfig,
    description: &str,
    destination_parent: &Path,
) -> Result<String> {
    let base_prompt = PROMPT_TEMPLATE
        .replace("{schema}", SCHEMA)
        .replace("{description}", description);

    run_with_repairs(
        base_prompt.clone(),
        |prompt| async move {
            run_prompt_with_usage(
                config,
                &prompt,
                crate::config::PermissionMode::Full,
                None,
                None,
                Some(crate::usage::UsageScope {
                    session_id: None,
                    step: None,
                    purpose: crate::usage::UsagePurpose::WorkflowGenerate,
                }),
            )
            .await
        },
        |candidate| {
            validate_generated_yaml(candidate, destination_parent)?;
            Ok(candidate.to_owned())
        },
        |candidate, err| format!(
            "{base_prompt}\n\n## Previous attempt\n\nYour previous reply was rejected. Fix it and reply with the complete corrected YAML only, with no Markdown fence.\n\nPrevious YAML:\n\n{candidate}\n\nDiagnostic:\n\n{err}\n"
        ),
    )
    .await
    .map_err(|err| match err {
        RepairError::Backend(err) | RepairError::Rejected(err) => err,
    })
}

/// Run doctor static checks with flags off and isolated defaults on candidate YAML.
///
/// # Errors
///
/// Returns the first parse, reference, validation, compile or graph error.
pub(crate) fn validate_generated_yaml(yaml: &str, base_dir: &Path) -> Result<()> {
    crate::workflow_doctor::check_generated_yaml(yaml, base_dir)
}

/// Check that `name` is a plain file stem (ASCII alphanumerics, `-`, `_`).
///
/// # Errors
///
/// Returns a message describing the rule when `name` is not allowed.
pub(crate) fn validate_name(name: &str) -> std::result::Result<String, String> {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(name.to_string())
    } else {
        Err(format!(
            "invalid workflow name {name:?}: use only letters, digits, '-' and '_'"
        ))
    }
}

/// Resolve the destination path for a new workflow.
///
/// # Errors
///
/// Returns an error for an invalid name or when the user workflow directory is unavailable.
pub(crate) fn destination_path(name: &str, user: bool, cwd: &Path) -> Result<PathBuf> {
    validate_name(name).map_err(CruiseError::Other)?;
    if !user && name == "default" {
        return Err(CruiseError::Other(
            "./.cruise/default.yaml is reserved for workflow defaults; choose another --name or use --user"
                .to_string(),
        ));
    }
    let dir = if user {
        crate::paths::workflows_dir()?
    } else {
        cwd.join(".cruise")
    };
    Ok(dir.join(format!("{name}.yaml")))
}

/// Create `path` exclusively and write `yaml`.
///
/// # Errors
///
/// Returns an I/O error (`AlreadyExists` when the file exists). A partially
/// written file is removed.
pub(crate) fn write_new_workflow(path: &Path, yaml: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(e) = file.write_all(yaml.as_bytes()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}

/// Entry point for `cruise workflow generate`.
///
/// # Errors
///
/// Returns an error when the name or config is invalid, the destination
/// exists, generation fails validation, or the file cannot be written.
pub(crate) async fn run(args: WorkflowGenerateArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let dest = destination_path(&args.name, args.user, &cwd)?;
    if dest.exists() {
        return Err(CruiseError::Other(format!(
            "workflow already exists: {}",
            dest.display()
        )));
    }
    let (yaml, source) = resolve_config(args.config.as_deref())?;
    let config = load_config_from_source(&yaml, &source)?;
    let parent = dest.parent().unwrap_or(&cwd);
    let generated = generate_yaml(&config, &args.description, parent).await?;
    write_new_workflow(&dest, &generated)?;
    println!("Created {}", dest.display());
    println!(
        "Review the YAML before running it. Validation does not prove the commands are safe or that the workflow succeeds."
    );
    Ok(())
}

#[cfg(all(test, unix))]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::error::CruiseError;
    use crate::test_support::workflow_authoring::{
        backend_config, fake_backend, prompt, run_env, turns,
    };
    use crate::test_support::{EnvGuard, lock_process, set_fake_home, write_executable_script};
    use std::fs;
    use tempfile::TempDir;

    const VALID: &str = "command: [cat]\nsteps:\n  build:\n    command: echo hi\n";
    const BROKEN: &str = "steps: [";

    async fn mixed_sdk_yaml_generation(
        repair: bool,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let _lock = lock_process();
        let tmp = TempDir::new()?;
        let _home = set_fake_home(tmp.path());
        let _path = crate::test_support::install_exiting_jcode_stub(tmp.path());
        let _jcode_home = EnvGuard::set("JCODE_HOME", tmp.path().join("jcode-home"));
        let _sdk = EnvGuard::remove("CRUISE_SDK");
        let _model = EnvGuard::remove("CRUISE_MODEL");
        let _plan_model = EnvGuard::remove("CRUISE_PLAN_MODEL");
        let _subagent_model = EnvGuard::remove("CLAUDE_CODE_SUBAGENT_MODEL");
        let _subagent_force = EnvGuard::remove("CLAUDE_CODE_SUBAGENT_MODEL_FORCE");
        for (name, text) in [("valid", VALID), ("broken", BROKEN)] {
            let frame = serde_json::json!({"type":"assistant","session_id":"mixed-yaml","message":{"role":"assistant","content":[{"type":"text","text":text}]}});
            fs::write(tmp.path().join(name), format!("{frame}\n"))?;
        }
        let script = format!(
            r#"#!/bin/sh
case "$1" in --version) echo '2.0.0'; exit 0;; --cruise-exec-probe) exit 0;; esac
d='{}'
printf '%s\n' "$@" >> "$d/args"
printf 'model=%s force=%s\n' "${{CLAUDE_CODE_SUBAGENT_MODEL-unset}}" "${{CLAUDE_CODE_SUBAGENT_MODEL_FORCE-unset}}" >> "$d/subagent-env"
read -r input
printf '%s\n' "$input" >> "$d/prompts"
if [ '{}' = true ] && [ ! -f "$d/attempted" ]; then
  touch "$d/attempted"
  cat "$d/broken"
else
  cat "$d/valid"
fi
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"session_id":"mixed-yaml"}}'
"#,
            tmp.path().display(),
            repair
        );
        write_executable_script(&tmp.path().join("claude"), &script);
        // Prove the stream fixture works before parsing the new mixed syntax.
        let control = WorkflowConfig::from_yaml(
            "sdk: claude\nmodel: mixed-yaml-model\nsteps:\n  build:\n    command: echo hi\n",
        )?;
        let control_output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            generate_yaml(&control, "legacy SDK control", tmp.path()),
        )
        .await??;
        assert_eq!(control_output.trim_end(), VALID.trim_end());
        for name in ["args", "prompts", "attempted", "subagent-env"] {
            let path = tmp.path().join(name);
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        let config = WorkflowConfig::from_yaml(
            "sdk: jcode\nsubagent_model: {sdk: claude, model: haiku}\nplan_model: []\nmodel:\n  - sdk: claude\n    model: mixed-yaml-model\n  - unused/jcode\nsteps:\n  build:\n    command: echo hi\n",
        )?;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            generate_yaml(&config, "mixed SDK workflow", tmp.path()),
        )
        .await??;
        assert_eq!(output.trim_end(), VALID.trim_end());
        // Check the actual SDK subprocess, including every repair dispatch.
        assert_eq!(
            fs::read_to_string(tmp.path().join("subagent-env"))?,
            "model=unset force=unset\n".repeat(if repair { 2 } else { 1 })
        );
        let args = fs::read_to_string(tmp.path().join("args"))?;
        assert_eq!(
            args.lines()
                .filter(|line| *line == "mixed-yaml-model")
                .count(),
            if repair { 2 } else { 1 }
        );
        assert!(args.lines().any(|line| line == "--model"));
        let prompts = fs::read_to_string(tmp.path().join("prompts"))?;
        assert_eq!(prompts.lines().count(), if repair { 2 } else { 1 });
        if repair {
            assert!(prompts.contains("Previous attempt"));
            assert!(prompts.contains(BROKEN));
        }
        Ok(())
    }

    #[tokio::test]
    async fn mixed_sdk_generate_yaml_uses_first_claude_candidate_under_jcode()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        mixed_sdk_yaml_generation(false).await
    }

    #[tokio::test]
    async fn mixed_sdk_generate_yaml_repair_preserves_first_claude_candidate()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        mixed_sdk_yaml_generation(true).await
    }

    #[test]
    fn destination_path_local_and_user() {
        let _lock = lock_process();
        let home = TempDir::new().unwrap();
        let _env = set_fake_home(home.path());
        let cwd = Path::new("/work/proj");
        assert_eq!(
            destination_path("ci", false, cwd).unwrap(),
            cwd.join(".cruise").join("ci.yaml")
        );
        assert_eq!(
            destination_path("ci", true, cwd).unwrap(),
            crate::paths::workflows_dir().unwrap().join("ci.yaml")
        );
    }

    #[test]
    fn destination_path_reserves_project_default_but_not_user_default() {
        let _lock = lock_process();
        let home = TempDir::new().unwrap();
        let _env = set_fake_home(home.path());
        let cwd = Path::new("/work/proj");
        assert!(destination_path("default", false, cwd).is_err());
        assert_eq!(
            destination_path("default", true, cwd).unwrap(),
            crate::paths::workflows_dir().unwrap().join("default.yaml")
        );
    }

    #[test]
    fn destination_path_rejects_non_filename_names() {
        let cwd = Path::new("/work/proj");
        for bad in [
            "", ".", "..", "a/b", "a\\b", "../x", ".hidden", "a.yaml", "a b", "/abs",
        ] {
            for user in [false, true] {
                assert!(
                    matches!(destination_path(bad, user, cwd), Err(CruiseError::Other(_))),
                    "name {bad:?} (user={user}) must be rejected"
                );
            }
        }
    }

    #[test]
    fn destination_path_accepts_alnum_dash_underscore() {
        let cwd = Path::new("/work/proj");
        assert!(destination_path("My-flow_2", false, cwd).is_ok());
    }

    #[test]
    fn generated_workflow_resolution_ignores_app_model_defaults() {
        let _lock = lock_process();
        let home = TempDir::new().unwrap();
        let _home = set_fake_home(home.path());
        let _model = EnvGuard::remove("CRUISE_MODEL");
        let _plan_model = EnvGuard::remove("CRUISE_PLAN_MODEL");
        let app_config_dir = home.path().join(".config/cruise");
        fs::create_dir_all(&app_config_dir).unwrap();
        fs::write(
            app_config_dir.join("config.json"),
            r#"{"model":"host/model","plan_model":["host/plan","host/plan-fallback"]}"#,
        )
        .unwrap();

        let config = WorkflowConfig::from_yaml(VALID).unwrap();
        let resolved = crate::workflow_call::resolve_workflow_calls_without_app_model_defaults(
            config,
            home.path(),
            &crate::workflow_defaults::WorkflowDefaults::default(),
        )
        .unwrap();
        assert_eq!(resolved.model, None);
        assert_eq!(resolved.plan_model, None);
        validate_generated_yaml(VALID, home.path()).unwrap();
    }

    #[test]
    fn validate_accepts_minimal_workflow() {
        let tmp = TempDir::new().unwrap();
        validate_generated_yaml(VALID, tmp.path()).unwrap();
    }

    #[test]
    fn validate_reports_parse_errors_as_config_parse_error() {
        let tmp = TempDir::new().unwrap();
        for bad in [BROKEN, "command: [cat]\nsteps: 3\n"] {
            assert!(
                matches!(
                    validate_generated_yaml(bad, tmp.path()),
                    Err(CruiseError::ConfigParseError(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn validate_generated_yaml_runs_full_preflight() {
        let tmp = TempDir::new().unwrap();
        // config validation: sdk and command are mutually exclusive
        assert!(
            validate_generated_yaml(
                "sdk: claude\ncommand: [cat]\nsteps:\n  a:\n    command: echo\n",
                tmp.path()
            )
            .is_err()
        );
        // undefined transition target
        assert!(
            validate_generated_yaml(
                "command: [cat]\nsteps:\n  a:\n    command: echo\n    next: missing\n",
                tmp.path()
            )
            .is_err()
        );
        // exitless cycle
        assert!(matches!(
            validate_generated_yaml(
                "command: [cat]\nsteps:\n  loop:\n    command: echo\n    next: loop\n",
                tmp.path()
            ),
            Err(CruiseError::InvalidStepConfig(_))
        ));
        // group retry budget beyond the effective ceiling
        assert!(
            validate_generated_yaml(
                &crate::test_support::group_retry_budget_config_with(5),
                tmp.path()
            )
            .is_err()
        );
    }

    #[test]
    fn generated_yaml_ignores_inactive_file_changed_target() {
        let tmp = TempDir::new().unwrap();
        let yaml = "command: [cat]\nsteps:\n  a:\n    command: echo\n    if:\n      file-changed: missing\n";
        assert!(validate_generated_yaml(yaml, tmp.path()).is_err());
        assert!(
            validate_generated_yaml(
                &format!("{yaml}      no-file-changes: failed\n"),
                tmp.path()
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_resolves_references_relative_to_base_dir() {
        let tmp = TempDir::new().unwrap();
        let yaml = "command: [cat]\nsteps:\n  a:\n    prompt_file: p.md\n";
        assert!(validate_generated_yaml(yaml, tmp.path()).is_err());
        fs::write(tmp.path().join("p.md"), "hello").unwrap();
        validate_generated_yaml(yaml, tmp.path()).unwrap();

        let call = "command: [cat]\nsteps:\n  a:\n    workflow_call: ./sub.yaml\n";
        assert!(validate_generated_yaml(call, tmp.path()).is_err());
        fs::write(tmp.path().join("sub.yaml"), VALID).unwrap();
        validate_generated_yaml(call, tmp.path()).unwrap();
    }

    #[tokio::test]
    async fn generate_yaml_returns_valid_first_output_verbatim_in_one_turn() {
        let tmp = TempDir::new().unwrap();
        let cfg = fake_backend(tmp.path(), &[VALID]);
        let out = generate_yaml(&cfg, "build and test the thing", tmp.path())
            .await
            .unwrap();
        assert_eq!(out.trim_end(), VALID.trim_end());
        assert_eq!(turns(tmp.path()), 1);
        assert!(prompt(tmp.path(), 1).contains("build and test the thing"));
    }

    #[tokio::test]
    async fn generate_yaml_repairs_invalid_candidate_up_to_three_turns() {
        let tmp = TempDir::new().unwrap();
        let cfg = fake_backend(tmp.path(), &[BROKEN, BROKEN, BROKEN, VALID]);
        let out = generate_yaml(&cfg, "unique-description-xyz", tmp.path())
            .await
            .unwrap();
        assert_eq!(out.trim_end(), VALID.trim_end());
        assert_eq!(turns(tmp.path()), 4);
        let diag = validate_generated_yaml(BROKEN, tmp.path())
            .unwrap_err()
            .to_string();
        let repair = prompt(tmp.path(), 2);
        assert!(repair.contains("unique-description-xyz"));
        assert!(repair.contains(BROKEN));
        assert!(repair.contains(&diag));
    }

    #[tokio::test]
    async fn generate_yaml_repairs_once_for_two_turns() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = fake_backend(tmp.path(), &[BROKEN, VALID]);
        cfg.plan_model = Some(crate::config::ModelSpec::Multiple(Vec::new()));
        cfg.model = Some(crate::config::ModelSpec::Single("command-primary".into()));
        generate_yaml(&cfg, "d", tmp.path()).await.unwrap();
        assert_eq!(turns(tmp.path()), 2);
    }

    #[tokio::test]
    async fn generate_yaml_exhausts_three_repairs_without_writing() {
        let tmp = TempDir::new().unwrap();
        let cfg = fake_backend(tmp.path(), &[BROKEN]);
        let err = generate_yaml(&cfg, "d", tmp.path()).await.unwrap_err();
        assert!(matches!(err, CruiseError::InvalidStepConfig(_)), "{err:?}");
        assert_eq!(turns(tmp.path()), 4);
        let diag = validate_generated_yaml(BROKEN, tmp.path())
            .unwrap_err()
            .to_string();
        assert!(err.to_string().contains(&diag), "{err}");
        assert!(!tmp.path().join(".cruise").exists());
    }

    #[tokio::test]
    async fn generate_yaml_rejects_markdown_fenced_output_without_recovery() {
        let tmp = TempDir::new().unwrap();
        let fenced = format!("```yaml\n{VALID}```\n");
        let cfg = fake_backend(tmp.path(), &[&fenced]);
        assert!(generate_yaml(&cfg, "d", tmp.path()).await.is_err());
        assert_eq!(turns(tmp.path()), 4);
    }

    #[tokio::test]
    async fn generate_yaml_propagates_backend_failure() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("fail.sh");
        write_executable_script(&script, "#!/bin/sh\ncat >/dev/null\nexit 1\n");
        let cfg = backend_config(&script);
        assert!(generate_yaml(&cfg, "d", tmp.path()).await.is_err());
    }

    #[test]
    fn write_new_workflow_creates_parent_and_writes_exact_content() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".cruise").join("ci.yaml");
        write_new_workflow(&path, VALID).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), VALID);
    }

    #[test]
    fn write_new_workflow_refuses_existing_destination() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("ci.yaml");
        fs::write(&path, "original").unwrap();
        let err = write_new_workflow(&path, VALID).unwrap_err();
        match err {
            CruiseError::IoError(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
            }
            other => panic!("expected AlreadyExists io error, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    }

    fn args(name: &str, user: bool) -> WorkflowGenerateArgs {
        WorkflowGenerateArgs {
            description: "make a workflow".to_string(),
            name: name.to_string(),
            user,
            config: None,
        }
    }

    #[tokio::test]
    async fn workflow_generate_does_not_run_generated_config() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap();
        let generated = "command: [cat]\nsteps:\n  build:\n    command: touch ran.txt\n";
        let (_g, _c, proj, _rec) = run_env(&tmp, &[generated]);
        run(args("ci", false)).await.unwrap();
        let dest = proj.join(".cruise").join("ci.yaml");
        assert_eq!(fs::read_to_string(&dest).unwrap(), generated);
        assert!(!proj.join("ran.txt").exists());
        let entries: Vec<_> = fs::read_dir(proj.join(".cruise")).unwrap().collect();
        assert_eq!(entries.len(), 1, "only the workflow file may be created");
    }

    #[tokio::test]
    async fn workflow_generate_user_flag_saves_to_user_workflows_dir() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap();
        let (_g, _c, proj, _rec) = run_env(&tmp, &[VALID]);
        run(args("shared", true)).await.unwrap();
        let dest = crate::paths::workflows_dir().unwrap().join("shared.yaml");
        assert_eq!(fs::read_to_string(dest).unwrap(), VALID);
        assert!(!proj.join(".cruise").exists());
    }

    #[tokio::test]
    async fn workflow_generate_rejects_existing_file_before_calling_backend() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap();
        let (_g, _c, proj, rec) = run_env(&tmp, &[VALID]);
        fs::create_dir_all(proj.join(".cruise")).unwrap();
        let dest = proj.join(".cruise").join("ci.yaml");
        fs::write(&dest, "original").unwrap();
        assert!(run(args("ci", false)).await.is_err());
        assert_eq!(turns(&rec), 0, "backend must not be called");
        assert_eq!(fs::read_to_string(dest).unwrap(), "original");
    }

    #[tokio::test]
    async fn workflow_generate_writes_nothing_when_repairs_exhausted() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap();
        let (_g, _c, proj, rec) = run_env(&tmp, &[BROKEN]);
        assert!(run(args("ci", false)).await.is_err());
        assert_eq!(turns(&rec), 4);
        assert!(!proj.join(".cruise").join("ci.yaml").exists());
    }
}
