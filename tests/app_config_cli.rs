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
            .join("config/cruise")
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
        "run_all_parallelism = 3\nmodel = [\"app/primary\",\"app/fallback\"]\nplan_model = [\"plan/primary\",\"plan/fallback\"]\n"
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
        "run_all_parallelism = 7\nmodel = [\"app/primary\",\"app/fallback\"]\nplan_model = [\"plan/primary\",\"plan/fallback\"]\n"
    );
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
