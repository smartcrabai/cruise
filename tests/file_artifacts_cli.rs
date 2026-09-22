#![cfg(unix)]

//! Contract tests for prompt file artifacts.
//!
//! These tests deliberately exercise the public configuration and CLI paths.
//! They do not inspect private helpers or prescribe a storage implementation.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use cruise::config::WorkflowConfig;
use cruise::workflow;
use cruise::workflow_call::resolve_workflow_calls;
use tempfile::TempDir;

struct Fixture {
    _process_lock: MutexGuard<'static, ()>,
    root: TempDir,
    repo: PathBuf,
    config: PathBuf,
}

static PROCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

impl Fixture {
    fn new(yaml: &str) -> Self {
        let process_lock = PROCESS_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| panic!("fixture process lock poisoned: {error}"));
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|error| panic!("repo mkdir failed: {error}"));
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        );
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, yaml)
            .unwrap_or_else(|error| panic!("workflow write failed: {error}"));
        Self {
            _process_lock: process_lock,
            root,
            repo,
            config,
        }
    }

    fn write_file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.root.path().join(name);
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
        path
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .current_dir(&self.repo)
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn exec(&self, input: &str) -> Output {
        self.command()
            .arg("exec")
            .arg("--config")
            .arg(&self.config)
            .arg(input)
            .output()
            .unwrap_or_else(|error| panic!("exec failed to start: {error}"))
    }

    fn dry_run(&self) -> Output {
        self.command()
            .arg("exec")
            .arg("--config")
            .arg(&self.config)
            .arg("--dry-run")
            .output()
            .unwrap_or_else(|error| panic!("dry-run failed to start: {error}"))
    }

    fn run_session(&self, id: &str) -> Output {
        self.command()
            .arg("run")
            .arg(id)
            .output()
            .unwrap_or_else(|error| panic!("resume failed to start: {error}"))
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.path().join("data/cruise/sessions")
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .unwrap_or_else(|error| panic!("git failed to start: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn terminal(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn parse_config(yaml: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(yaml).unwrap_or_else(|error| panic!("config parse failed: {error}"))
}

fn compile_config(yaml: &str) -> Result<cruise::workflow::CompiledWorkflow, String> {
    let config = WorkflowConfig::from_yaml(yaml).map_err(|error| error.to_string())?;
    workflow::compile(config).map_err(|error| error.to_string())
}

fn resolve_and_compile_config(
    yaml: &str,
    base_dir: &Path,
) -> Result<cruise::workflow::CompiledWorkflow, String> {
    let config = WorkflowConfig::from_yaml(yaml).map_err(|error| error.to_string())?;
    let config = resolve_workflow_calls(config, base_dir).map_err(|error| error.to_string())?;
    workflow::compile(config).map_err(|error| error.to_string())
}

fn wait_for_path(child: &mut Child, path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() {
            return;
        }
        if let Some(status) = child
            .try_wait()
            .unwrap_or_else(|error| panic!("failed to poll cruise process: {error}"))
        {
            panic!(
                "cruise exited before {} appeared ({status})",
                path.display()
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child
                .wait()
                .unwrap_or_else(|error| panic!("failed to collect timed-out status: {error}"));
            panic!("timed out waiting for {} ({status})", path.display());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn suspended_session_id(fixture: &Fixture) -> String {
    let entries = std::fs::read_dir(fixture.sessions_dir())
        .unwrap_or_else(|error| panic!("sessions directory unavailable: {error}"));
    let mut ids = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("session entry unavailable: {error}"))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(
        ids.len(),
        1,
        "expected one retained exec session, got {ids:?}"
    );
    ids.remove(0)
}

fn compile_error(yaml: &str) -> String {
    compile_config(yaml)
        .err()
        .unwrap_or_else(|| panic!("expected configuration to be rejected"))
}

fn resolve_and_compile_error(yaml: &str, base_dir: &Path) -> String {
    resolve_and_compile_config(yaml, base_dir)
        .err()
        .unwrap_or_else(|| panic!("expected configuration to be rejected"))
}

fn validate_error(yaml: &str) -> String {
    let config = WorkflowConfig::from_yaml(yaml)
        .map_err(|error| error.to_string())
        .unwrap_or_else(|error| panic!("expected validation to run after parsing: {error}"));
    cruise::config::validate_config(&config).map_or_else(
        |error| error.to_string(),
        |()| panic!("expected configuration to be rejected"),
    )
}

#[test]
fn output_file_is_optional_and_round_trips_for_prompt_steps() {
    let config = parse_config(
        r"
command: [cat]
steps:
  summarize:
    prompt: summarize
    output_file: initial-state.md
  legacy:
    prompt: unchanged
",
    );

    let serialized = serde_json::to_value(&config.steps["summarize"])
        .unwrap_or_else(|error| panic!("step serialization failed: {error}"));
    assert_eq!(serialized["output_file"], "initial-state.md");

    let legacy_serialized = serde_json::to_value(&config.steps["legacy"])
        .unwrap_or_else(|error| panic!("legacy step serialization failed: {error}"));
    assert!(
        legacy_serialized.get("output_file").is_none(),
        "omitted output_file must remain omitted when serialized"
    );
}

#[test]
fn output_file_is_preserved_through_prompt_file_resolution() {
    let fixture = Fixture::new(
        r"
command: [cat]
steps:
  summarize:
    prompt_file: prompt.md
    output_file: initial-state.md
",
    );
    fixture.write_file("prompt.md", "prompt loaded from disk");

    let resolved = cruise::workflow_call::resolve_workflow_calls_from_path(&fixture.config)
        .unwrap_or_else(|error| panic!("workflow resolution failed: {error}"));
    let step = resolved
        .steps
        .get("summarize")
        .unwrap_or_else(|| panic!("resolved summarize step is missing"));
    let serialized = serde_json::to_value(step)
        .unwrap_or_else(|error| panic!("resolved step serialization failed: {error}"));
    assert_eq!(serialized["prompt"], "prompt loaded from disk");
    assert_eq!(serialized["output_file"], "initial-state.md");
    assert!(serialized.get("prompt_file").is_none());
}

#[test]
fn output_file_is_preserved_when_a_group_is_expanded() {
    let config = parse_config(
        r"
command: [cat]
groups:
  review:
    steps:
      summarize:
        prompt: group prompt
        output_file: group-summary.md
steps:
  review-group:
    group: review
",
    );
    let compiled = workflow::compile(config)
        .unwrap_or_else(|error| panic!("group workflow compilation failed: {error}"));
    let serialized = serde_json::to_value(
        compiled
            .steps
            .get("review-group/summarize")
            .unwrap_or_else(|| panic!("expanded group step is missing")),
    )
    .unwrap_or_else(|error| panic!("expanded step serialization failed: {error}"));
    assert_eq!(serialized["output_file"], "group-summary.md");
}

#[test]
fn output_file_is_rejected_on_a_command_step() {
    let error = compile_error(
        r"
command: [cat]
steps:
  write:
    command: printf command
    output_file: command-output.md
",
    );
    assert!(error.contains("output_file"), "unexpected error: {error}");
    assert!(error.contains("command"), "unexpected error: {error}");
}

#[test]
fn output_file_rejects_a_prompt_mixed_with_another_executable_kind() {
    let error = compile_error(
        r"
command: [cat]
steps:
  mixed:
    prompt: prompt
    command: printf command
    output_file: result.md
",
    );
    assert!(error.contains("output_file"), "unexpected error: {error}");
    assert!(error.contains("command"), "unexpected error: {error}");
}

#[test]
fn output_file_is_rejected_on_a_parallel_parent() {
    let error = validate_error(
        r"
command: [cat]
steps:
  checks:
    output_file: parent-output.md
    parallel:
      one:
        prompt: one
",
    );
    assert!(error.contains("output_file"), "unexpected error: {error}");
    assert!(error.contains("parallel"), "unexpected error: {error}");
}

#[test]
fn output_file_is_rejected_on_an_option_step() {
    let error = validate_error(
        r"
command: [cat]
steps:
  choose:
    option:
      - selector: continue
    output_file: option-output.md
",
    );
    assert!(error.contains("output_file"), "unexpected error: {error}");
    assert!(error.contains("option"), "unexpected error: {error}");
}

#[test]
fn output_file_is_rejected_on_a_group_call_step() {
    let error = validate_error(
        r"
command: [cat]
groups:
  review:
    steps:
      inner:
        prompt: inner
steps:
  review-call:
    group: review
    output_file: group-output.md
",
    );
    assert!(error.contains("output_file"), "unexpected error: {error}");
    assert!(error.contains("group"), "unexpected error: {error}");
}

#[test]
fn output_file_is_rejected_on_a_workflow_call_site() {
    let fixture = Fixture::new(
        r"
command: [cat]
steps:
  shared:
    workflow_call: child.yaml
    output_file: call-output.md
",
    );
    fixture.write_file(
        "child.yaml",
        "command: [cat]\nsteps:\n  child:\n    prompt: child\n",
    );

    let error = resolve_and_compile_error(
        &std::fs::read_to_string(&fixture.config)
            .unwrap_or_else(|read_error| panic!("config read failed: {read_error}")),
        fixture.root.path(),
    );
    assert!(error.contains("output_file"), "unexpected error: {error}");
    assert!(error.contains("workflow_call"), "unexpected error: {error}");
}

#[test]
fn parallel_prompt_children_accept_distinct_output_files() {
    let config = parse_config(
        r"
command: [cat]
steps:
  checks:
    parallel:
      first:
        prompt: first
        output_file: first.md
      second:
        prompt: second
        output_file: second.md
",
    );
    let compiled = workflow::compile(config)
        .unwrap_or_else(|error| panic!("parallel workflow compilation failed: {error}"));
    let parallel = compiled
        .steps
        .get("checks")
        .unwrap_or_else(|| panic!("parallel step is missing"));
    let serialized = serde_json::to_value(parallel)
        .unwrap_or_else(|error| panic!("parallel step serialization failed: {error}"));
    assert_eq!(serialized["parallel"]["first"]["output_file"], "first.md");
    assert_eq!(serialized["parallel"]["second"]["output_file"], "second.md");
}

#[test]
fn schema_declares_output_file_without_making_it_required() {
    let schema: serde_json::Value = serde_json::from_str(include_str!("../cruise-schema.json"))
        .unwrap_or_else(|error| panic!("schema is not valid JSON: {error}"));
    let step_properties = schema["$defs"]["StepConfig"]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("StepConfig properties are missing"));
    assert!(step_properties.contains_key("output_file"));
    let parallel_properties = schema["$defs"]["ParallelChild"]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("ParallelChild properties are missing"));
    assert!(parallel_properties.contains_key("output_file"));
    if let Some(required) = schema["$defs"]["StepConfig"]["required"].as_array() {
        assert!(!required.iter().any(|field| field == "output_file"));
    }
}

#[test]
fn cli_saves_the_prompt_response_and_reads_it_after_a_command_step() {
    let fixture = Fixture::new(
        r#"
command:
  - sh
  - -c
  - |
      prompt="$(cat)"
      case "$prompt" in
        initial-state)
          printf '%s\n' 'BASELINE: café 🙂' 'line-2' '{{"key":"value"}}' '{{input}} {{file:other.md}} {{{{literal}}}}'
          printf '%s' 'PROMPT_STDERR' >&2
          ;;
        final-check*)
          expected=$(cat <<'EOF'
      final-check
      BASELINE: café 🙂
      line-2
      {{"key":"value"}}
      {{input}} {{file:other.md}} {{{{literal}}}}
      EOF
          )
          case "$prompt" in
            *PROMPT_STDERR*) printf '%s' 'ARTIFACT_MIXED' ;;
            *)
              if [ "$prompt" = "$expected" ]; then
                printf '%s' 'ARTIFACT_EXACT'
              else
                printf '%s' 'ARTIFACT_MISMATCH'
              fi
              ;;
          esac
          ;;
        *)
          printf '%s' "$prompt"
          ;;
      esac
steps:
  baseline:
    prompt: initial-state
    output_file: initial-state.md
  interpose:
    command: printf command-result
  verify:
    prompt: |-
      final-check
      {file:initial-state.md}
"#,
    );
    let output = fixture.exec("task input");
    let text = terminal(&output);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("ARTIFACT_EXACT"),
        "the saved artifact must preserve the exact response without stderr or recursive expansion: {text}"
    );
    assert!(
        !fixture.repo.join("initial-state.md").exists(),
        "artifacts must not be written into the workspace"
    );
}

#[test]
fn cli_resolves_a_file_reference_from_prompt_file_at_execution_time() {
    let fixture = Fixture::new(
        r"
command: [cat]
steps:
  baseline:
    prompt: baseline
    output_file: baseline.md
  verify:
    prompt_file: final-prompt.md
",
    );
    fixture.write_file("final-prompt.md", "{file:baseline.md}");
    let output = fixture.exec("input");
    let text = terminal(&output);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("baseline"),
        "prompt_file content should be resolved after the artifact exists: {text}"
    );
}

#[test]
fn cli_reports_a_missing_file_reference_instead_of_inserting_empty_text() {
    let fixture = Fixture::new(
        r#"
command: [cat]
steps:
  verify:
    prompt: "before {file:not-created.md} after"
"#,
    );
    let output = fixture.exec("input");
    let text = terminal(&output);
    assert!(
        !output.status.success(),
        "missing artifact unexpectedly succeeded: {text}"
    );
    assert!(
        text.contains("not-created.md"),
        "error should identify the missing file: {text}"
    );
    assert!(
        text.contains("read") || text.contains("artifact") || text.contains("file"),
        "error should identify the failed read operation: {text}"
    );
}

#[test]
fn cli_rejects_an_empty_output_file_name() {
    let fixture = Fixture::new(
        r#"
command: [cat]
steps:
  save:
    prompt: save
    output_file: ""
"#,
    );
    let output = fixture.dry_run();
    let text = terminal(&output);
    assert!(
        !output.status.success(),
        "empty artifact name unexpectedly accepted: {text}"
    );
    assert!(
        text.contains("empty") || text.contains("name"),
        "error should explain why an empty artifact name is invalid: {text}"
    );
}

#[test]
fn cli_parallel_children_save_distinct_outputs_and_join_before_reading_them() {
    let fixture = Fixture::new(
        r#"
command:
  - sh
  - -c
  - |
      prompt="$(cat)"
      case "$prompt" in
        first) printf 'FIRST_OUTPUT' ;;
        second) printf 'SECOND_OUTPUT' ;;
        join*) printf '%s' "$prompt" ;;
      esac
steps:
  parallel:
    parallel:
      first:
        prompt: first
        output_file: first.md
      second:
        prompt: second
        output_file: second.md
  join:
    prompt: |-
      join
      {file:first.md}
      {file:second.md}
"#,
    );
    let output = fixture.exec("input");
    let text = terminal(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("FIRST_OUTPUT"), "{text}");
    assert!(text.contains("SECOND_OUTPUT"), "{text}");
}

#[test]
fn cli_rejects_an_absolute_output_file_path() {
    let fixture = Fixture::new(
        r"
command: [cat]
steps:
  save:
    prompt: save
    output_file: /tmp/escape.md
",
    );
    let output = fixture.dry_run();
    let text = terminal(&output);
    assert!(
        !output.status.success(),
        "unsafe path unexpectedly accepted: {text}"
    );
    assert!(
        text.contains("absolute"),
        "error should describe the path boundary: {text}"
    );
}

#[test]
fn cli_rejects_a_parent_directory_output_file_path() {
    let fixture = Fixture::new(
        r"
command: [cat]
steps:
  save:
    prompt: save
    output_file: ../escape.md
",
    );
    let output = fixture.dry_run();
    let text = terminal(&output);
    assert!(
        !output.status.success(),
        "unsafe path unexpectedly accepted: {text}"
    );
    assert!(
        text.contains("parent") || text.contains("outside") || text.contains(".."),
        "error should identify the root escape: {text}"
    );
}

#[test]
fn cli_rejects_duplicate_parallel_output_names_before_children_start() {
    let fixture = Fixture::new(
        r"
command: [cat]
steps:
  checks:
    parallel:
      first:
        command: touch first-ran
        output_file: same.md
      second:
        prompt: second
        output_file: same.md
",
    );
    let output = fixture.dry_run();
    let text = terminal(&output);
    assert!(
        !output.status.success(),
        "duplicate names unexpectedly accepted: {text}"
    );
    assert!(
        text.contains("same.md") && text.contains("parallel"),
        "error should identify the conflicting parallel artifact: {text}"
    );
    assert!(
        !fixture.repo.join("first-ran").exists(),
        "parallel children must not start before conflict validation"
    );
}

#[test]
fn cli_rejects_a_parallel_sibling_dependency_before_children_start() {
    let fixture = Fixture::new(
        r#"
command: [cat]
steps:
  checks:
    parallel:
      producer:
        command: touch producer-ran
        output_file: produced.md
      consumer:
        prompt: "{file:produced.md}"
"#,
    );
    let output = fixture.dry_run();
    let text = terminal(&output);
    assert!(
        !output.status.success(),
        "sibling dependency unexpectedly accepted: {text}"
    );
    assert!(
        text.contains("parallel") && text.contains("produced.md"),
        "error should identify the sibling dependency: {text}"
    );
    assert!(!fixture.repo.join("producer-ran").exists());
}

#[test]
fn cli_rejects_an_artifact_larger_than_one_mib_without_truncating_it() {
    let fixture = Fixture::new(
        r"
command:
  - sh
  - -c
  - |
      cat >/dev/null
      head -c 1048577 /dev/zero
steps:
  oversized:
    prompt: oversized
    output_file: oversized.md
",
    );
    let output = fixture.exec("input");
    let text = terminal(&output);
    assert!(
        !output.status.success(),
        "oversized artifact unexpectedly succeeded: {text}"
    );
    assert!(
        text.contains("1048576") || text.contains("1 MiB") || text.contains("size"),
        "error should identify the artifact size limit: {text}"
    );
}

#[test]
fn cli_dry_run_does_not_create_the_artifact_directory() {
    let fixture = Fixture::new(
        r"
command: [cat]
steps:
  save:
    prompt: save
    output_file: save.md
",
    );
    let output = fixture.dry_run();
    let text = terminal(&output);
    assert!(
        output.status.success(),
        "dry-run should not execute artifact I/O: {text}"
    );
    assert!(
        !fixture
            .sessions_dir()
            .read_dir()
            .is_ok_and(|mut entries| entries.next().is_some()),
        "dry-run must not create a session artifact directory"
    );
}

#[test]
fn cli_keeps_the_artifact_available_across_suspend_and_resume() {
    let fixture = Fixture::new(
        r#"
command:
  - sh
  - -c
  - |
      prompt="$(cat)"
      case "$prompt" in
        initial)
          printf 'RESUME_BASELINE\n'
          touch .baseline-ready
          ;;
        hold)
          touch .hold-ready
          if [ -e .hold-seen ]; then printf 'HOLD_RESUMED' ; else touch .hold-seen; sleep 30; fi
          ;;
        final*)
          case "$prompt" in
            *RESUME_BASELINE*) printf 'RESUME_OK' ;;
            *) printf 'RESUME_BAD' ;;
          esac
          ;;
      esac
steps:
  baseline:
    prompt: initial
    output_file: baseline.md
  hold:
    prompt: hold
  verify:
    prompt: |-
      final
      {file:baseline.md}
"#,
    );
    let mut child = fixture
        .command()
        .arg("exec")
        .arg("--config")
        .arg(&fixture.config)
        .arg("input")
        .spawn()
        .unwrap_or_else(|error| panic!("exec failed to start: {error}"));
    wait_for_path(
        &mut child,
        &fixture.repo.join(".hold-ready"),
        Duration::from_secs(5),
    );
    assert_eq!(
        unsafe {
            libc::kill(
                i32::try_from(child.id()).unwrap_or_else(|error| panic!("{error}")),
                libc::SIGINT,
            )
        },
        0
    );
    let interrupted = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("failed to collect interrupted output: {error}"));
    assert!(
        !interrupted.status.success(),
        "interrupt should stop exec: {}",
        terminal(&interrupted)
    );

    let id = suspended_session_id(&fixture);
    let artifact = fixture
        .sessions_dir()
        .join(&id)
        .join("artifacts")
        .join("baseline.md");
    assert_eq!(
        std::fs::read_to_string(&artifact)
            .unwrap_or_else(|error| panic!("saved artifact unavailable: {error}")),
        "RESUME_BASELINE\n"
    );

    let resumed = fixture.run_session(&id);
    let resumed_text = terminal(&resumed);
    assert!(resumed.status.success(), "resume failed: {resumed_text}");
    assert!(resumed_text.contains("RESUME_OK"), "{resumed_text}");
    assert!(
        !fixture.sessions_dir().join(id).exists(),
        "completed exec sessions and their artifacts are removed after resume"
    );
}
