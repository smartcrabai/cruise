//! Authoring-time workflow YAML generation (`cruise workflow generate`).

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::{DEFAULT_RATE_LIMIT_RETRIES, WorkflowGenerateArgs};
use crate::config::{ModelSpec, WorkflowConfig};
use crate::error::{CruiseError, Result};
use crate::executor::{Executor, PromptRun};
use crate::resolver::{load_config_from_source, resolve_config};

const PROMPT_TEMPLATE: &str = include_str!("../prompts/workflow-generate.md");
const SCHEMA: &str = include_str!("../cruise-schema.json");
const MAX_REPAIRS: usize = 3;

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
    let executor = Executor::new(config.sdk.as_deref(), &config.command);
    let model_or_mode = executor.plan_model_or_mode(
        config.plan_model.as_ref().and_then(ModelSpec::primary),
        config.model.as_ref().and_then(ModelSpec::primary),
    );
    let base_prompt = PROMPT_TEMPLATE
        .replace("{schema}", SCHEMA)
        .replace("{description}", description);

    let mut prompt = base_prompt.clone();
    let mut attempt = 0;
    loop {
        let outcome = executor
            .run(PromptRun {
                prompt: &prompt,
                model_or_mode: model_or_mode.as_deref(),
                max_retries: DEFAULT_RATE_LIMIT_RETRIES,
                env: &config.env,
                mcp_servers: &config.mcp_servers,
                on_notice: None,
                cancel_token: None,
                working_dir: None,
                stream: None,
                tools: Vec::new(),
                on_session_id: None,
                resume: None,
                permission: crate::config::PermissionMode::Full,
                computer_use: false,
            })
            .await?;
        let candidate = outcome.result.output;
        let err = match validate_generated_yaml(&candidate, destination_parent) {
            Ok(()) => return Ok(candidate),
            Err(err) => err,
        };
        if attempt >= MAX_REPAIRS {
            return Err(CruiseError::InvalidStepConfig(format!(
                "generated workflow is still invalid after {MAX_REPAIRS} repairs: {err}"
            )));
        }
        attempt += 1;
        prompt = format!(
            "{base_prompt}\n\n## Previous attempt\n\nYour previous reply was rejected. Fix it and reply with the complete corrected YAML only, with no Markdown fence.\n\nPrevious YAML:\n\n{candidate}\n\nDiagnostic:\n\n{err}\n"
        );
    }
}

/// Run the full exec preflight on a candidate YAML string.
///
/// # Errors
///
/// Returns the first parse, reference, validation, compile or graph error.
pub(crate) fn validate_generated_yaml(yaml: &str, base_dir: &Path) -> Result<()> {
    let config = WorkflowConfig::from_yaml(yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    let config = crate::workflow_call::resolve_workflow_calls(config, base_dir)?;
    crate::config::validate_config(&config)?;
    let max_retries = crate::config::resolve_effective_max_retries(None, &config);
    crate::config::validate_group_retry_budget(&config, max_retries)?;
    let compiled = crate::workflow::compile(config)?;
    let graph = crate::graph::build_graph(&compiled, max_retries)?;
    crate::graph::validation::validate_workflow(&compiled, &graph, &graph.start, &[])?;
    Ok(())
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
    use crate::test_support::{EnvGuard, lock_process, set_fake_home, write_executable_script};
    use std::fs;
    use tempfile::TempDir;

    const VALID: &str = "command: [cat]\nsteps:\n  build:\n    command: echo hi\n";
    const BROKEN: &str = "steps: [";

    /// Build a command-backend config whose script records each prompt and
    /// replies with `outputs[n]` on turn n (the last output repeats).
    fn fake_backend(dir: &Path, outputs: &[&str]) -> WorkflowConfig {
        for (i, out) in outputs.iter().enumerate() {
            fs::write(dir.join(format!("out-{}.txt", i + 1)), out).unwrap();
        }
        fs::write(dir.join("out-last.txt"), outputs.last().unwrap()).unwrap();
        let d = dir.display();
        let script = format!(
            "#!/bin/sh\nd='{d}'\nn=$(ls \"$d\" | grep -c '^prompt-')\nn=$((n+1))\ncat > \"$d/prompt-$n.txt\"\nif [ -f \"$d/out-$n.txt\" ]; then cat \"$d/out-$n.txt\"; else cat \"$d/out-last.txt\"; fi\n"
        );
        let path = dir.join("backend.sh");
        write_executable_script(&path, &script);
        backend_config(&path)
    }

    fn backend_config(script: &Path) -> WorkflowConfig {
        WorkflowConfig::from_yaml(&format!(
            "command: ['{}']\nsteps:\n  s:\n    command: echo\n",
            script.display()
        ))
        .unwrap()
    }

    fn turns(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("prompt-")
            })
            .count()
    }

    fn prompt(dir: &Path, n: usize) -> String {
        fs::read_to_string(dir.join(format!("prompt-{n}.txt"))).unwrap()
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
        let cfg = fake_backend(tmp.path(), &[BROKEN, VALID]);
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

    /// Set up cwd + env so `run` resolves a command-backend config via `CRUISE_CONFIG`.
    fn run_env(tmp: &TempDir, outputs: &[&str]) -> (Vec<EnvGuard>, EnvGuard, PathBuf, PathBuf) {
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        let rec = tmp.path().join("rec");
        for d in [&home, &proj, &rec] {
            fs::create_dir_all(d).unwrap();
        }
        let mut guards = set_fake_home(&home);
        let cfg = fake_backend(&rec, outputs);
        let cfg_path = tmp.path().join("backend.yaml");
        fs::write(
            &cfg_path,
            format!(
                "command: ['{}']\nsteps:\n  s:\n    command: echo\n",
                rec.join("backend.sh").display()
            ),
        )
        .unwrap();
        drop(cfg);
        guards.push(EnvGuard::set("CRUISE_CONFIG", &cfg_path));
        let cwd = EnvGuard::set("PWD", &proj);
        std::env::set_current_dir(&proj).unwrap();
        (guards, cwd, proj, rec)
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
