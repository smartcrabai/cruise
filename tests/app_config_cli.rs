#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("create fixture: {error}"));
        let home = root.path().join("home");
        let project = root.path().join("project");
        std::fs::create_dir_all(&home).unwrap_or_else(|error| panic!("create home: {error}"));
        std::fs::create_dir_all(&project).unwrap_or_else(|error| panic!("create project: {error}"));
        Self {
            root,
            home,
            project,
        }
    }

    fn app_config(&self, extension: &str) -> PathBuf {
        self.root
            .path()
            .join("config")
            .join("cruise")
            .join(format!("config.{extension}"))
    }

    fn write_app_config(&self, extension: &str, contents: &str) -> PathBuf {
        let path = self.app_config(extension);
        let parent = path
            .parent()
            .unwrap_or_else(|| panic!("app config parent missing: {}", path.display()));
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("create app config directory: {error}"));
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("write app config {}: {error}", path.display()));
        path
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.starts_with("CRUISE_") || name.starts_with("JCODE_") {
                command.env_remove(name.as_ref());
            }
        }
        command
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("JCODE_HOME", self.root.path().join("jcode"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("CRUISE_UI_LANGUAGE", "en")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .env_remove("JCODE_MODEL")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("run cruise {args:?}: {error}"))
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn displayed_notifications(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = stdout
        .lines()
        .find_map(|line| line.strip_prefix("notifications = "))
        .unwrap_or_else(|| panic!("missing notifications line: {stdout}"));
    serde_json::from_str(value)
        .unwrap_or_else(|error| panic!("invalid notifications JSON: {error}"))
}

#[test]
fn notification_defaults_remain_invisible_and_are_not_saved()
-> Result<(), Box<dyn std::error::Error>> {
    for input in [
        r#"{"runAllParallelism":2}"#,
        r#"{"runAllParallelism":2,"notifications":{}}"#,
    ] {
        let fixture = Fixture::new();
        let path = fixture.write_app_config("json", input);
        let shown = fixture.run(&["config"]);
        assert!(shown.status.success(), "{}", output_text(&shown));
        assert_eq!(
            String::from_utf8_lossy(&shown.stdout),
            "run_all_parallelism = 2\nrun_all_auto_requeue_max_attempts = 0\nprevent_sleep = false\nui_language = en (source: env)\n"
        );
        let updated = fixture.run(&["config", "--set-parallelism", "3"]);
        assert!(updated.status.success(), "{}", output_text(&updated));
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        assert!(saved.get("notifications").is_none(), "{saved}");
    }
    Ok(())
}

#[test]
fn notification_settings_accept_aliases_and_survive_config_updates() {
    let cases = [
        (
            "json",
            r#"{"notifications":{"sound":true,"events":{"plan_ready":{"sound":false}}}}"#,
        ),
        (
            "json",
            r#"{"notifications":{"sound":true,"events":{"planReady":{"sound":false}}}}"#,
        ),
        (
            "yaml",
            "notifications:\n  sound: true\n  events:\n    plan_ready:\n      sound: false\n",
        ),
    ];
    for (extension, input) in cases {
        let fixture = Fixture::new();
        fixture.write_app_config(extension, input);
        let shown = fixture.run(&["config"]);
        assert!(shown.status.success(), "{}", output_text(&shown));
        let before = displayed_notifications(&shown);
        assert_eq!(before["sound"], true);
        assert_eq!(before["events"]["planReady"]["sound"], false);
        let updated = fixture.run(&["config", "--set-parallelism", "7"]);
        assert!(updated.status.success(), "{}", output_text(&updated));
        let after = fixture.run(&["config"]);
        assert!(after.status.success(), "{}", output_text(&after));
        assert!(String::from_utf8_lossy(&after.stdout).contains("run_all_parallelism = 7\n"));
        assert_eq!(displayed_notifications(&after), before);
    }
}

#[test]
fn notification_yaml_failed_override_is_displayed() {
    let fixture = Fixture::new();
    fixture.write_app_config(
        "yaml",
        "notifications:\n  events:\n    failed:\n      webhook: false\n",
    );
    let shown = fixture.run(&["config"]);
    assert!(shown.status.success(), "{}", output_text(&shown));
    assert_eq!(
        displayed_notifications(&shown)["events"]["failed"]["webhook"],
        false
    );
}

#[test]
fn notification_unknown_keys_are_rejected() {
    let fixture = Fixture::new();
    fixture.write_app_config("json", r#"{"notifications":{"sond":true}}"#);
    let invalid = fixture.run(&["config"]);
    assert!(!invalid.status.success(), "{}", output_text(&invalid));
    assert!(
        output_text(&invalid).contains("sond"),
        "{}",
        output_text(&invalid)
    );
}

#[test]
fn config_reports_webhook_error_without_exposing_webhook_url()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new();
    fixture.write_app_config("json", r#"{"notifications":{"sound":true}}"#);
    let state = fixture.root.path().join("state/cruise");
    std::fs::create_dir_all(&state)?;
    std::fs::write(
        state.join("notification-webhook-error"),
        "1700000000 transport error\n",
    )?;
    let secret = "https://hooks.example.test/services/SECRET123";
    let output = fixture
        .command()
        .env("CRUISE_NOTIFY_WEBHOOK", secret)
        .arg("config")
        .output()?;
    assert!(output.status.success(), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(
        text.contains("notification_webhook_error = transport error at 1700000000"),
        "{text}"
    );
    assert_eq!(displayed_notifications(&output)["sound"], true);
    assert!(
        !text.contains(secret) && !text.contains("SECRET123"),
        "{text}"
    );
    assert!(!std::fs::read_to_string(fixture.app_config("json"))?.contains("SECRET123"));
    Ok(())
}

#[test]
fn auto_requeue_config_defaults_to_zero_and_persists_updates() {
    let fixture = Fixture::new();
    let shown = fixture.run(&["config"]);
    assert!(shown.status.success(), "{}", output_text(&shown));
    assert!(output_text(&shown).contains("run_all_auto_requeue_max_attempts = 0"));

    let updated = fixture.run(&["config", "--set-auto-requeue-max-attempts", "3"]);
    assert!(updated.status.success(), "{}", output_text(&updated));
    assert!(output_text(&updated).contains("run_all_auto_requeue_max_attempts = 3"));
    let shown = fixture.run(&["config"]);
    assert!(shown.status.success(), "{}", output_text(&shown));
    assert!(output_text(&shown).contains("run_all_auto_requeue_max_attempts = 3"));
    let zero = fixture.run(&["config", "--set-auto-requeue-max-attempts", "0"]);
    assert!(zero.status.success(), "{}", output_text(&zero));
    assert!(output_text(&zero).contains("run_all_auto_requeue_max_attempts = 0"));
}

#[test]
fn auto_requeue_config_accepts_both_names_and_rejects_negative_values()
-> Result<(), Box<dyn std::error::Error>> {
    for extension in ["json", "yaml"] {
        for key in [
            "runAllAutoRequeueMaxAttempts",
            "run_all_auto_requeue_max_attempts",
        ] {
            let fixture = Fixture::new();
            let value = serde_json::json!({key: 2});
            let contents = if extension == "yaml" {
                serde_yaml::to_string(&value)?
            } else {
                value.to_string()
            };
            fixture.write_app_config(extension, &contents);
            let shown = fixture.run(&["config"]);
            assert!(
                shown.status.success(),
                "{extension} {key}: {}",
                output_text(&shown)
            );
            assert!(output_text(&shown).contains("run_all_auto_requeue_max_attempts = 2"));
        }
        let fixture = Fixture::new();
        let contents = if extension == "yaml" {
            "runAllAutoRequeueMaxAttempts: -1"
        } else {
            "{\"runAllAutoRequeueMaxAttempts\": -1}"
        };
        fixture.write_app_config(extension, contents);
        let shown = fixture.run(&["config"]);
        assert!(
            !shown.status.success(),
            "{extension}: {}",
            output_text(&shown)
        );
    }
    Ok(())
}

#[test]
fn auto_requeue_run_flag_requires_all_and_unsigned_integer() {
    let fixture = Fixture::new();
    let output = fixture.run(&["run", "some-session", "--auto-requeue-max-attempts", "2"]);
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        output_text(&output).contains("--all"),
        "{}",
        output_text(&output)
    );
    for value in ["-1", "abc"] {
        let output = fixture.run(&["run", "--all", "--auto-requeue-max-attempts", value]);
        assert!(
            !output.status.success(),
            "{value}: {}",
            output_text(&output)
        );
        assert!(
            output_text(&output).contains("auto-requeue-max-attempts"),
            "{}",
            output_text(&output)
        );
    }
    let help = fixture.run(&["run", "--help"]);
    assert!(help.status.success(), "{}", output_text(&help));
    assert!(output_text(&help).contains("--auto-requeue-max-attempts"));
}

#[test]
fn yaml_config_show_and_set_parallelism_preserve_model_lists() {
    let fixture = Fixture::new();
    let yaml_path = fixture.write_app_config(
        "yaml",
        "run_all_parallelism: 3\nmodel:\n  - app/primary\n  - app/fallback\nplan_model:\n  - plan/primary\n  - plan/fallback\n",
    );

    let shown = fixture.run(&["config"]);
    assert!(shown.status.success(), "{}", output_text(&shown));
    assert_eq!(
        String::from_utf8_lossy(&shown.stdout),
        "run_all_parallelism = 3\nrun_all_auto_requeue_max_attempts = 0\nprevent_sleep = false\nui_language = en (source: env)\nmodel = [\"app/primary\",\"app/fallback\"]\nplan_model = [\"plan/primary\",\"plan/fallback\"]\n"
    );

    let updated = fixture.run(&["config", "--set-parallelism", "7"]);
    assert!(updated.status.success(), "{}", output_text(&updated));
    assert_eq!(
        String::from_utf8_lossy(&updated.stdout),
        "run_all_parallelism = 7\n"
    );
    assert!(yaml_path.is_file(), "selected YAML config was removed");
    assert!(
        !fixture.app_config("json").exists(),
        "updating YAML config must not create config.json"
    );

    let shown_after_update = fixture.run(&["config"]);
    assert!(
        shown_after_update.status.success(),
        "{}",
        output_text(&shown_after_update)
    );
    assert_eq!(
        String::from_utf8_lossy(&shown_after_update.stdout),
        "run_all_parallelism = 7\nrun_all_auto_requeue_max_attempts = 0\nprevent_sleep = false\nui_language = en (source: env)\nmodel = [\"app/primary\",\"app/fallback\"]\nplan_model = [\"plan/primary\",\"plan/fallback\"]\n"
    );
}

#[test]
fn parallelism_update_preserves_workflow_categories_in_yaml_and_json() {
    for extension in ["yaml", "json"] {
        let fixture = Fixture::new();
        let expected = serde_json::json!({
            "Development": {
                "workflows": ["tdd", {"refactor": "Review changes"}],
                "Backend": {"workflows": ["api"]}
            }
        });
        let input = serde_json::json!({
            "run_all_parallelism": 3,
            "workflow_categories": expected,
            "show_others_category": false,
            "others_category_name": "Misc"
        });
        let contents = if extension == "yaml" {
            serde_yaml::to_string(&input).unwrap_or_else(|e| panic!("{e}"))
        } else {
            input.to_string()
        };
        let path = fixture.write_app_config(extension, &contents);
        let updated = fixture.run(&["config", "--set-parallelism", "7"]);
        assert!(
            updated.status.success(),
            "{extension}: {}",
            output_text(&updated)
        );
        let saved = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{e}"));
        let actual: serde_json::Value = if extension == "yaml" {
            serde_yaml::from_str(&saved).unwrap_or_else(|e| panic!("{e}"))
        } else {
            serde_json::from_str(&saved).unwrap_or_else(|e| panic!("{e}"))
        };
        // Existing app config serializes using camelCase; the accepted aliases
        // must survive a read-modify-write regardless of spelling.
        let field = |snake: &str, camel: &str| actual.get(snake).or_else(|| actual.get(camel));
        assert_eq!(
            field("workflow_categories", "workflowCategories"),
            Some(&expected),
            "{extension}: {saved}"
        );
        assert_eq!(
            field("show_others_category", "showOthersCategory"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(
            field("others_category_name", "othersCategoryName"),
            Some(&serde_json::json!("Misc"))
        );
    }
}

#[test]
fn config_show_and_parallelism_update_preserve_mixed_sdk_candidates_in_both_formats() {
    let expected_model = serde_json::json!([
        "openai/gpt-6.1-sol:medium",
        {"sdk": "claude", "model": "claude-sonnet-5-5:medium"},
        "openrouter/z-ai/glm-5.3-flash:high"
    ]);
    let expected_plan = serde_json::json!([
        {"sdk": "claude", "model": "planning-primary:high"},
        "openai/planning-fallback:medium"
    ]);
    for extension in ["json", "yaml"] {
        let fixture = Fixture::new();
        let input = serde_json::json!({
            "run_all_parallelism": 3,
            "model": expected_model,
            "planModel": expected_plan
        });
        let contents = if extension == "yaml" {
            serde_yaml::to_string(&input).unwrap_or_else(|error| panic!("YAML fixture: {error}"))
        } else {
            input.to_string()
        };
        let path = fixture.write_app_config(extension, &contents);

        // Config output embeds JSON values. Compare those values, not incidental
        // whitespace or object key ordering in the human-readable display.
        let shown = fixture.run(&["config"]);
        assert!(
            shown.status.success(),
            "{extension}: {}",
            output_text(&shown)
        );
        for (field, expected) in [("model", &expected_model), ("plan_model", &expected_plan)] {
            let prefix = format!("{field} = ");
            let stdout = String::from_utf8_lossy(&shown.stdout);
            let value = stdout
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .unwrap_or_else(|| panic!("missing {field} in {stdout}"));
            let actual: serde_json::Value = serde_json::from_str(value)
                .unwrap_or_else(|error| panic!("{field} display must be JSON: {error}"));
            assert_eq!(&actual, expected, "{extension} {field}");
        }

        let updated = fixture.run(&["config", "--set-parallelism", "7"]);
        assert!(updated.status.success(), "{}", output_text(&updated));
        let saved = cruise::app_config::AppConfig::load_from(&path)
            .unwrap_or_else(|error| panic!("reload {extension}: {error}"));
        assert_eq!(saved.run_all_parallelism, 7);
        assert_eq!(
            serde_json::to_value(&saved.model)
                .unwrap_or_else(|error| panic!("serialize model: {error}")),
            expected_model
        );
        assert_eq!(
            serde_json::to_value(&saved.plan_model)
                .unwrap_or_else(|error| panic!("serialize plan_model: {error}")),
            expected_plan
        );
        let other_extension = if extension == "yaml" { "json" } else { "yaml" };
        assert!(!fixture.app_config(other_extension).exists());
    }
}

#[cfg(unix)]
#[test]
fn command_workflow_rejects_inherited_app_sdk_fallback_before_spawning() {
    for field in ["model", "plan_model"] {
        let fixture = Fixture::new();
        cruise::test_support::init_git_repo(&fixture.project);
        let mut app = serde_json::json!({});
        app[field] = serde_json::json!([
            "legacy-primary",
            {"sdk": "claude", "model": "explicit-sdk-fallback"}
        ]);
        fixture.write_app_config("json", &app.to_string());
        let marker = fixture.root.path().join("command-started");
        let executable = fixture.root.path().join("command-recorder");
        cruise::test_support::write_executable_script(
            &executable,
            &format!("#!/bin/sh\nprintf started > {}\n", shell_quote(&marker)),
        );
        let workflow = serde_json::json!({
            "command": [executable.to_string_lossy()],
            "steps": {"main": {"prompt": "must reject command and SDK candidate"}}
        });
        std::fs::write(fixture.project.join("workflow.yaml"), workflow.to_string())
            .unwrap_or_else(|error| panic!("write workflow: {error}"));

        let output = fixture.run(&["exec", "--config", "workflow.yaml"]);
        assert!(!output.status.success(), "{}", output_text(&output));
        let diagnostic = output_text(&output);
        assert!(
            diagnostic.contains("command") && diagnostic.to_ascii_lowercase().contains("sdk"),
            "composed {field} collision must identify command and SDK: {diagnostic}"
        );
        assert!(!marker.exists(), "invalid composition spawned its command");
    }
}

#[test]
fn json_config_precedes_yaml_and_malformed_json_does_not_fall_back() {
    let fixture = Fixture::new();
    let json_path = fixture.write_app_config("json", r#"{"model":"json/model"}"#);
    fixture.write_app_config("yaml", "model: yaml/model\n");

    let shown = fixture.run(&["config"]);
    assert!(shown.status.success(), "{}", output_text(&shown));
    assert!(
        output_text(&shown).contains("model = \"json/model\""),
        "JSON should win when both application config files exist: {}",
        output_text(&shown)
    );
    assert!(!output_text(&shown).contains("yaml/model"));

    std::fs::write(&json_path, "{")
        .unwrap_or_else(|error| panic!("malform selected JSON config: {error}"));
    let malformed_json = fixture.run(&["config"]);
    let json_path_text = json_path.display().to_string();
    assert!(
        !malformed_json.status.success(),
        "{}",
        output_text(&malformed_json)
    );
    assert!(
        output_text(&malformed_json).contains(json_path_text.as_str()),
        "selected JSON parse error should name {}: {}",
        json_path.display(),
        output_text(&malformed_json)
    );
}

#[test]
fn invalid_selected_yaml_reports_yaml_and_its_path() {
    let fixture = Fixture::new();
    let yaml_path = fixture.write_app_config("yaml", "model: [\n");

    let output = fixture.run(&["config"]);
    assert!(!output.status.success(), "{}", output_text(&output));
    let message = output_text(&output);
    let yaml_path_text = yaml_path.display().to_string();
    assert!(
        message.to_ascii_lowercase().contains("yaml"),
        "YAML parse error should identify its format: {message}"
    );
    assert!(
        message.contains(yaml_path_text.as_str()),
        "YAML parse error should name {}: {message}",
        yaml_path.display()
    );
}

#[test]
fn missing_config_updates_create_readable_json() {
    let fixture = Fixture::new();

    let updated = fixture.run(&["config", "--set-parallelism", "4"]);
    assert!(updated.status.success(), "{}", output_text(&updated));
    assert!(
        fixture.app_config("json").is_file(),
        "default path should be JSON"
    );
    assert!(
        !fixture.app_config("yaml").exists(),
        "missing config should not create config.yaml"
    );
    let saved = cruise::app_config::AppConfig::load_from(&fixture.app_config("json"))
        .unwrap_or_else(|error| panic!("load saved default JSON config: {error}"));
    assert_eq!(saved.run_all_parallelism, 4);
}

#[cfg(unix)]
#[test]
fn yaml_model_list_reaches_execution_primary() {
    let fixture = Fixture::new();
    cruise::test_support::init_git_repo(&fixture.project);
    fixture.write_app_config("yaml", "model:\n  - app/primary\n  - app/fallback\n");
    std::fs::write(
        fixture.project.join("workflow.yaml"),
        "command: [model-recorder, '{model}']\nsteps:\n  main-step:\n    prompt: perform the task\n",
    )
    .unwrap_or_else(|error| panic!("write workflow: {error}"));

    let bin = fixture.root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("create fake bin: {error}"));
    let marker = fixture.root.path().join("selected-model");
    let executable = bin.join("model-recorder");
    cruise::test_support::write_executable_script(
        &executable,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"${{1-<unset>}}\" >> {}\ncat >/dev/null\nprintf '# Plan\\n\\n- complete\\n'\n",
            shell_quote(&marker)
        ),
    );

    let mut command = fixture.command();
    prepend_command_path(&mut command, &bin);
    let output = command
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run YAML-configured exec: {error}"));
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(
        std::fs::read_to_string(marker)
            .unwrap_or_else(|error| panic!("read selected model: {error}")),
        "app/primary\n"
    );
}

#[test]
fn exec_dry_run_discovers_config_yaml_candidates() {
    let cases = [
        (
            "project .cruise/config.yaml",
            true,
            ".cruise/config.yaml",
            "local-config-yaml-marker",
            "local_config_yaml_step_marker",
        ),
        (
            "user workflows/config.yaml",
            false,
            "config/cruise/workflows/config.yaml",
            "user-workflows-config-yaml-marker",
            "user_workflows_config_yaml_step_marker",
        ),
    ];

    for (description, project_relative, relative_path, command_marker, step_marker) in cases {
        let fixture = Fixture::new();
        cruise::test_support::init_git_repo(&fixture.project);
        let path = if project_relative {
            fixture.project.join(relative_path)
        } else {
            fixture.root.path().join(relative_path)
        };
        let parent = path
            .parent()
            .unwrap_or_else(|| panic!("{description} path has no parent: {}", path.display()));
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("create {description} directory: {error}"));
        std::fs::write(
            &path,
            format!(
                "command: [{command_marker}]\nsteps:\n  {step_marker}:\n    prompt: perform the task\n"
            ),
        )
        .unwrap_or_else(|error| panic!("write {description} {}: {error}", path.display()));

        let output = fixture.run(&["exec", "--dry-run"]);
        let text = output_text(&output);
        assert!(
            output.status.success(),
            "{description} dry-run failed: {text}"
        );
        assert!(
            text.contains(&format!("command: {command_marker}")),
            "dry-run did not select {description}: {text}"
        );
        assert!(
            text.contains(step_marker),
            "dry-run omitted the {description} step: {text}"
        );
    }
}

#[cfg(unix)]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(unix)]
fn prepend_command_path(command: &mut Command, bin: &Path) {
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env(
        "PATH",
        std::env::join_paths(paths).unwrap_or_else(|error| panic!("join PATH: {error}")),
    );
}
