#![cfg(unix)]

//! Real CLI dispatch contracts. Neither executable can contact an agent service.
//! Claude speaks the stream-json stub protocol used by backend/claude.rs, while
//! jcode deliberately fails before starting a runtime or loading credentials.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("mkdir: {error}"));
        cruise::test_support::init_git_repo(&repo);
        let fixture = Self { root, repo };
        let bin = fixture.root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("bin: {error}"));
        fixture.install_claude(false);
        cruise::test_support::write_executable_script(
            &bin.join("jcode"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' jcode >> {}\nprintf '%s\\n' \"$@\" >> {}\nprintf '%s\\n' \"${{JCODE_MODEL-}}\" >> {}\nprintf 'FAKE_JCODE_REFUSED_SERVICE\\n' >&2\nexit 73\n",
                quote(&fixture.path("dispatch")),
                quote(&fixture.path("jcode-args")),
                quote(&fixture.path("jcode-model")),
            ),
        );
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn install_claude(&self, fail: bool) {
        let response = if fail {
            r#"{"type":"result","subtype":"success","is_error":true,"session_id":"fake-claude-session","result":"API Error: 404 model not found"}"#
        } else {
            concat!(
                r#"{"type":"system","subtype":"init","session_id":"fake-claude-session"}"#,
                "\n",
                r##"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"# Fake SDK plan\n\n- SDK_CLAUDE_OUTPUT"}]},"session_id":"fake-claude-session"}"##,
                "\n",
                r##"{"type":"result","subtype":"success","is_error":false,"session_id":"fake-claude-session","result":"# Fake SDK plan\n\n- SDK_CLAUDE_OUTPUT"}"##,
            )
        };
        cruise::test_support::write_executable_script(
            &self.path("bin/claude"),
            &format!(
                "#!/bin/sh\nread -r user_frame\nprintf '%s\\n' claude >> {}\nprintf '%s\\n' \"$@\" >> {}\nprintf '%s\\n' \"$user_frame\" >> {}\nprintf '%s\\n' '{}'\nexit {}\n",
                quote(&self.path("dispatch")),
                quote(&self.path("claude-args")),
                quote(&self.path("claude-input")),
                response,
                i32::from(fail),
            ),
        );
    }

    fn workflow(&self, yaml: &str) {
        std::fs::write(self.repo.join("workflow.yaml"), yaml)
            .unwrap_or_else(|error| panic!("workflow: {error}"));
    }

    fn app_config(&self, extension: &str, contents: &str) {
        let dir = self.path("config/cruise");
        std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("config dir: {error}"));
        std::fs::write(dir.join(format!("config.{extension}")), contents)
            .unwrap_or_else(|error| panic!("app config: {error}"));
    }

    fn run(&self, subcommand: &str) -> Output {
        self.run_with_env(subcommand, &[])
    }

    fn run_with_env(&self, subcommand: &str, overrides: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        // Child-local isolation allows tests to run concurrently without changing
        // this test process's environment or the coordinator's build environment.
        for (name, _) in std::env::vars_os() {
            let name_text = name.to_string_lossy();
            if name_text.starts_with("CRUISE_")
                || name_text.starts_with("JCODE_")
                || name_text.starts_with("CLAUDE")
                || name_text.starts_with("ANTHROPIC_")
            {
                command.env_remove(name);
            }
        }
        let mut paths = vec![self.path("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command
            .current_dir(&self.repo)
            .env(
                "PATH",
                std::env::join_paths(paths).unwrap_or_else(|error| panic!("PATH: {error}")),
            )
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("JCODE_HOME", self.path("jcode-home"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env("NO_COLOR", "1")
            .arg(subcommand);
        if subcommand == "plan" {
            command.arg("--no-interactive-planning");
        }
        command.envs(overrides.iter().copied());
        command
            .args(["--config", "workflow.yaml", "SDK_DISPATCH_REQUEST"])
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|error| panic!("CLI {subcommand}: {error}"))
    }

    fn recorded(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name))
            .unwrap_or_else(|error| panic!("read {name}: {error}"))
    }

    fn assert_jcode_model(&self, model: &str) {
        let models = self.recorded("jcode-model");
        assert!(
            !models.is_empty() && models.lines().all(|actual| actual == model),
            "unexpected jcode model environment: {models:?}"
        );
    }

    fn assert_claude_model(&self, model: &str) {
        let args = self.recorded("claude-args");
        let args: Vec<_> = args.lines().collect();
        assert!(
            args.windows(2).any(|pair| pair == ["--model", model]),
            "Claude model argument missing: {args:?}"
        );
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn legacy_claude_string_model_is_a_successful_fake_cli_harness_control() {
    let fixture = Fixture::new();
    fixture.workflow("sdk: claude\nmodel: claude-legacy-control:high\nsteps:\n  answer:\n    prompt: SDK_LEGACY_CONTROL_PROMPT\n");
    let output = fixture.run("exec");
    assert!(
        output.status.success(),
        "legacy harness failed: {}",
        text(&output)
    );
    assert_eq!(fixture.recorded("dispatch"), "claude\n");
    fixture.assert_claude_model("claude-legacy-control");
    assert!(
        fixture
            .recorded("claude-input")
            .contains("SDK_LEGACY_CONTROL_PROMPT")
    );
    assert!(
        text(&output).contains("SDK_CLAUDE_OUTPUT"),
        "{}",
        text(&output)
    );
    assert!(!fixture.path("jcode-args").exists());
}

#[test]
fn first_claude_object_dispatches_claude_under_explicit_and_default_jcode() {
    for sdk in ["sdk: jcode\n", ""] {
        let fixture = Fixture::new();
        fixture.workflow(&format!("{sdk}model:\n  - sdk: claude\n    model: claude-dispatch-test:high\nsteps:\n  answer:\n    prompt: SDK_EXEC_PROMPT\n"));
        let output = fixture.run("exec");
        assert!(output.status.success(), "{}", text(&output));
        assert_eq!(fixture.recorded("dispatch"), "claude\n");
        fixture.assert_claude_model("claude-dispatch-test");
        let args = fixture.recorded("claude-args");
        assert!(
            args.lines()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|pair| pair == ["--effort", "high"]),
            "{args}"
        );
        assert!(fixture.recorded("claude-input").contains("SDK_EXEC_PROMPT"));
        assert!(
            text(&output).contains("SDK_CLAUDE_OUTPUT"),
            "{}",
            text(&output)
        );
        assert!(!fixture.path("jcode-args").exists());
    }
}

#[test]
fn parallel_child_dispatches_global_claude_object_despite_workflow_jcode() {
    let fixture = Fixture::new();
    fixture.workflow("sdk: jcode\nmodel:\n  - sdk: claude\n    model: claude-parallel-test\nsteps:\n  checks:\n    parallel:\n      prompt_child:\n        prompt: SDK_PARALLEL_CHILD_PROMPT\n      command_child:\n        command: echo SDK_PARALLEL_COMMAND_OUTPUT\n");
    let output = fixture.run("exec");
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.recorded("dispatch"), "claude\n");
    fixture.assert_claude_model("claude-parallel-test");
    assert!(
        fixture
            .recorded("claude-input")
            .contains("SDK_PARALLEL_CHILD_PROMPT")
    );
    let terminal = text(&output);
    assert!(terminal.contains("[checks/prompt_child]"), "{terminal}");
    assert!(terminal.contains("SDK_CLAUDE_OUTPUT"), "{terminal}");
    assert!(
        terminal.contains("SDK_PARALLEL_COMMAND_OUTPUT"),
        "{terminal}"
    );
    assert!(!fixture.path("jcode-args").exists());
}

#[test]
fn environment_model_replaces_the_whole_object_list_for_actual_exec() {
    let fixture = Fixture::new();
    fixture.workflow("sdk: claude\nmodel:\n  - sdk: jcode\n    model: stale-primary-test\n  - sdk: jcode\n    model: stale-fallback-test\nretry:\n  base_delay_ms: 1\n  max_delay_ms: 1\nsteps:\n  answer:\n    prompt: SDK_ENV_OVERRIDE_PROMPT\n");
    let output = fixture.run_with_env("exec", &[("CRUISE_MODEL", "claude-env-test")]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.recorded("dispatch"), "claude\n");
    fixture.assert_claude_model("claude-env-test");
    assert!(
        fixture
            .recorded("claude-input")
            .contains("SDK_ENV_OVERRIDE_PROMPT")
    );
    assert!(!fixture.path("jcode-args").exists());

    // Fail the replacement model to expose any accidentally retained tail.
    // A 404 cannot retry this same model and must not resurrect stale jcode.
    fixture.install_claude(true);
    let failed = fixture.run_with_env("exec", &[("CRUISE_MODEL", "claude-env-test")]);
    assert!(!failed.status.success(), "{}", text(&failed));
    assert_eq!(fixture.recorded("dispatch"), "claude\nclaude\n");
    assert!(
        !fixture.path("jcode-args").exists(),
        "environment replacement retained stale fallback"
    );
}

#[test]
fn environment_plan_model_replaces_object_list_for_actual_planning() {
    let fixture = Fixture::new();
    fixture.workflow("sdk: claude\nmodel: claude-execution-only-test\nplan_model:\n  - sdk: jcode\n    model: stale-planning-primary-test\n  - sdk: jcode\n    model: stale-planning-fallback-test\nsteps:\n  answer:\n    prompt: SDK_EXEC_PROMPT\n");
    let output = fixture.run_with_env("plan", &[("CRUISE_PLAN_MODEL", "claude-env-planning-test")]);
    assert!(output.status.success(), "{}", text(&output));
    let dispatch = fixture.recorded("dispatch");
    assert!(
        !dispatch.is_empty() && dispatch.lines().all(|sdk| sdk == "claude"),
        "{dispatch}"
    );
    fixture.assert_claude_model("claude-env-planning-test");
    let args = fixture.recorded("claude-args");
    assert!(!args.contains("stale-planning"), "{args}");
    assert!(!args.contains("claude-execution-only-test"), "{args}");
    assert!(!fixture.path("jcode-args").exists());
}

#[test]
fn app_json_and_yaml_object_models_reach_actual_exec_dispatch() {
    for (extension, contents) in [
        (
            "json",
            r#"{"model":[{"sdk":"claude","model":"claude-app-test"}]}"#,
        ),
        (
            "yaml",
            "model:\n  - sdk: claude\n    model: claude-app-test\n",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.app_config(extension, contents);
        fixture.workflow("steps:\n  answer:\n    prompt: SDK_APP_PROMPT\n");
        let output = fixture.run("exec");
        assert!(output.status.success(), "{extension}: {}", text(&output));
        assert_eq!(fixture.recorded("dispatch"), "claude\n");
        fixture.assert_claude_model("claude-app-test");
        assert!(
            text(&output).contains("SDK_CLAUDE_OUTPUT"),
            "{}",
            text(&output)
        );
    }
}

#[test]
fn step_string_override_dispatches_workflow_jcode_not_global_object_sdk() {
    let fixture = Fixture::new();
    fixture.workflow("sdk: jcode\nmodel:\n  - sdk: claude\n    model: claude-global-test\nsteps:\n  answer:\n    model: step-override-test\n    prompt: SDK_STEP_PROMPT\n");
    let output = fixture.run("exec");
    assert!(
        !output.status.success(),
        "fake jcode must refuse service: {}",
        text(&output)
    );
    assert!(
        fixture.path("dispatch").is_file(),
        "CLI failed before SDK subprocess dispatch: {}",
        text(&output)
    );
    let dispatch = fixture.recorded("dispatch");
    assert!(dispatch.lines().all(|sdk| sdk == "jcode"), "{dispatch}");
    fixture.assert_jcode_model("step-override-test");
    assert!(
        !fixture.path("claude-args").exists(),
        "step override inherited object SDK"
    );
}

#[test]
fn planning_uses_its_own_same_named_model_list_without_overwriting_execution() {
    let fixture = Fixture::new();
    fixture.workflow("sdk: jcode\nmodel:\n  - shared-dispatch-test\nplan_model:\n  - sdk: claude\n    model: shared-dispatch-test\nsteps:\n  answer:\n    prompt: SDK_EXEC_PROMPT\n");
    let plan = fixture.run("plan");
    assert!(plan.status.success(), "{}", text(&plan));
    // Foreground approval also requests a title through the planning model.
    // The stub does not implement that tool, so production falls back to the
    // plan heading. Both SDK subprocess requests must remain on Claude.
    let planning_dispatch = fixture.recorded("dispatch");
    let planning_calls = planning_dispatch.lines().count();
    assert!(
        planning_calls > 0 && planning_dispatch.lines().all(|sdk| sdk == "claude"),
        "{planning_dispatch}"
    );
    fixture.assert_claude_model("shared-dispatch-test");
    assert!(
        fixture
            .recorded("claude-input")
            .contains("SDK_DISPATCH_REQUEST")
    );
    let exec = fixture.run("exec");
    assert!(
        !exec.status.success(),
        "execution should select fail-fast jcode: {}",
        text(&exec)
    );
    let dispatch = fixture.recorded("dispatch");
    let remaining: Vec<_> = dispatch.lines().skip(planning_calls).collect();
    assert!(
        !remaining.is_empty() && remaining.iter().all(|sdk| *sdk == "jcode"),
        "{dispatch}"
    );
    fixture.assert_jcode_model("shared-dispatch-test");
}

#[test]
fn same_model_different_sdk_is_not_deduplicated_during_real_fallback() {
    let fixture = Fixture::new();
    fixture.install_claude(true);
    fixture.workflow("sdk: jcode\nmodel:\n  - sdk: claude\n    model: shared-fallback-test\n  - shared-fallback-test\nretry:\n  base_delay_ms: 1\n  max_delay_ms: 1\nsteps:\n  answer:\n    prompt: SDK_FALLBACK_PROMPT\n");
    let output = fixture.run("exec");
    assert!(
        !output.status.success(),
        "fallback jcode refuses service: {}",
        text(&output)
    );
    assert!(
        fixture.path("dispatch").is_file(),
        "CLI failed before primary SDK dispatch: {}",
        text(&output)
    );
    let dispatch = fixture.recorded("dispatch");
    let calls: Vec<_> = dispatch.lines().collect();
    assert_eq!(calls.first(), Some(&"claude"), "{dispatch}");
    assert!(
        calls.len() >= 2 && calls[1..].iter().all(|sdk| *sdk == "jcode"),
        "same-name jcode fallback was skipped: {dispatch}\n{}",
        text(&output)
    );
    fixture.assert_claude_model("shared-fallback-test");
    fixture.assert_jcode_model("shared-fallback-test");
    let terminal = text(&output);
    let notice = terminal
        .lines()
        .find(|line| line.contains("Fallback:") && line.contains("shared-fallback-test"))
        .unwrap_or_else(|| panic!("missing same-model SDK switch notice: {terminal}"));
    assert!(
        notice.contains("claude") && notice.contains("jcode"),
        "switch notice must distinguish both SDK owners: {notice}"
    );
}
