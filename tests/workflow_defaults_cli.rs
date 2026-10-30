use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

fn cruise(home: &std::path::Path, cwd: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("JCODE_MODEL")
        .env("JCODE_HOME", home.join("jcode-home"))
        .current_dir(cwd);
    command
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn project_with_workflow(defaults: &str) -> (TempDir, TempDir) {
    let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let project = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    std::fs::create_dir_all(project.path().join(".cruise")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(project.path().join(".cruise/default.yaml"), defaults)
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        "command: [echo]\nsteps:\n  main-step:\n    command: echo main\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    (home, project)
}

fn workflow_for_jcode(sdk: Option<&str>) -> String {
    format!(
        "{}steps:\n  main-step:\n    prompt: perform the requested task\n",
        sdk.map_or_else(String::new, |sdk| format!("sdk: {sdk}\n"))
    )
}

#[cfg(unix)]
fn sh_quote(path: &Path) -> String {
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
        std::env::join_paths(paths).unwrap_or_else(|error| panic!("{error}")),
    );
}

#[cfg(unix)]
fn install_jcode_marker(command: &mut Command, home: &Path, marker: &Path) {
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("{error}"));
    let executable = bin.join("jcode");
    cruise::test_support::write_executable_script(
        &executable,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"${{JCODE_MODEL-<unset>}}\" >> {}\nexit 0\n",
            sh_quote(marker)
        ),
    );

    prepend_command_path(command, &bin);
}

#[cfg(unix)]
fn write_app_config(home: &Path, contents: &str) {
    let path = home.join("config/cruise/config.json");
    std::fs::create_dir_all(path.parent().unwrap_or_else(|| panic!("app config parent")))
        .unwrap_or_else(|error| panic!("create app config directory: {error}"));
    std::fs::write(path, contents).unwrap_or_else(|error| panic!("write app config: {error}"));
}

#[cfg(unix)]
fn install_model_recorder(command: &mut Command, home: &Path, marker: &Path) {
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("{error}"));
    let executable = bin.join("model-recorder");
    cruise::test_support::write_executable_script(
        &executable,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"${{1-<unset>}}\" >> {}\ncat >/dev/null\nprintf '# Plan\\n\\n- complete\\n'\n",
            sh_quote(marker)
        ),
    );
    prepend_command_path(command, &bin);
}

#[cfg(unix)]
fn write_model_workflow(project: &Path, model: Option<&str>) {
    let model_line = model.map_or_else(String::new, |model| format!("model: {model}\n"));
    let workflow = format!(
        "command: [model-recorder, '{{model}}']\n{model_line}steps:\n  main-step:\n    prompt: perform the main task\n    next: step-override\n  step-override:\n    model: step/model\n    prompt: perform the step-specific task\n"
    );
    std::fs::write(project.join("workflow.yaml"), workflow)
        .unwrap_or_else(|error| panic!("write model workflow: {error}"));
}

#[cfg(unix)]
fn install_claude_marker(command: &mut Command, home: &Path, marker: &Path) {
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("{error}"));
    let executable = bin.join("claude");
    let response = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"test-session","result":"ok"}"#;
    cruise::test_support::write_executable_script(
        &executable,
        &format!(
            "#!/bin/sh\nread -r _user_frame\nprintf started >> {}\nprintf '%s\\n' '{}'\n",
            sh_quote(marker),
            response
        ),
    );
    prepend_command_path(command, &bin);
}

fn assert_missing_model_diagnostic(output: &Output, user_file: &Path) {
    assert!(
        !output.status.success(),
        "execution must stop before the prompt"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostic = stderr.to_ascii_lowercase().replace('\\', "/");
    let user_path = user_file.display().to_string().replace('\\', "/");
    for required in [
        "jcode",
        "model",
        "default.yaml",
        user_path.as_str(),
        "provider/model",
        "cruise_model",
        "plan_model",
    ] {
        assert!(
            diagnostic.contains(&required.to_ascii_lowercase()),
            "missing required model-setup guidance {required:?} in stderr:\n{stderr}"
        );
    }
}

fn assert_null_model_template(path: &Path) {
    let content = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        content.contains("provider/model"),
        "format comment missing: {content}"
    );
    assert!(
        content.contains("model: null"),
        "null placeholder missing: {content}"
    );
    assert!(
        !content.contains("<your-model>"),
        "template must not contain an executable placeholder"
    );
    let parsed: serde_yaml::Value = serde_yaml::from_str(&content)
        .unwrap_or_else(|error| panic!("template is not valid YAML: {error}"));
    let mapping = parsed
        .as_mapping()
        .unwrap_or_else(|| panic!("template must be a YAML mapping: {parsed:?}"));
    assert!(
        parsed["model"].is_null(),
        "model placeholder must be YAML null"
    );
    assert_eq!(
        mapping.len(),
        1,
        "shared defaults template must not select a backend or contain workflow-only keys"
    );
}

#[test]
fn exec_dry_run_lists_pre_steps_before_main_steps() {
    let (home, project) =
        project_with_workflow("pre_steps:\n  setup:\n    command: mise trust || true\n");
    let output = cruise(home.path(), project.path())
        .args(["exec", "--dry-run", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|e| panic!("run cruise: {e}"));
    let terminal = text(&output);
    assert!(output.status.success(), "{terminal}");
    let pre = terminal
        .find("pre/setup")
        .unwrap_or_else(|| panic!("pre/setup missing:\n{terminal}"));
    let main = terminal
        .find("main-step")
        .unwrap_or_else(|| panic!("main-step missing:\n{terminal}"));
    assert!(pre < main, "pre step must come first:\n{terminal}");
}

#[test]
fn defaults_file_with_workflow_only_key_fails_with_path_and_key() {
    let (home, project) = project_with_workflow("steps:\n  x:\n    command: echo\n");
    let output = cruise(home.path(), project.path())
        .args(["exec", "--dry-run", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|e| panic!("run cruise: {e}"));
    let terminal = text(&output);
    assert!(!output.status.success(), "{terminal}");
    assert!(terminal.contains("default.yaml"), "{terminal}");
    assert!(terminal.contains("steps"), "{terminal}");
}

#[test]
fn workflow_eject_tdd_to_project_writes_named_workflow_file() {
    let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let project = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let output = cruise(home.path(), project.path())
        .args(["workflow", "eject", "tdd", "--to", "project"])
        .output()
        .unwrap_or_else(|e| panic!("run cruise: {e}"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(project.path().join(".cruise/tdd.yaml").is_file());
    assert!(!project.path().join(".cruise/default.yaml").exists());
}

#[test]
fn missing_jcode_model_creates_user_template_and_stops_for_explicit_and_implicit_jcode() {
    for sdk in [Some("jcode"), None] {
        let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let project_defaults = project.path().join(".cruise/default.yaml");
        std::fs::create_dir_all(project.path().join(".cruise"))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            project.path().join("workflow.yaml"),
            workflow_for_jcode(sdk),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cruise::test_support::init_git_repo(project.path());
        let user_file = home.path().join("config/cruise/default.yaml");
        let marker = home.path().join("jcode-started");
        let mut command = cruise(home.path(), project.path());
        #[cfg(unix)]
        {
            install_jcode_marker(&mut command, home.path(), &marker);
            // Match Windows' main-stack budget without changing the test process.
            let mut bounded = Command::new("/bin/sh");
            bounded.args(["-c", "ulimit -s 1024; exec \"$@\"", "cruise-stack"]);
            bounded.arg(command.get_program());
            if let Some(cwd) = command.get_current_dir() {
                bounded.current_dir(cwd);
            }
            for (key, value) in command.get_envs() {
                if let Some(value) = value {
                    bounded.env(key, value);
                } else {
                    bounded.env_remove(key);
                }
            }
            command = bounded;
        }
        let output = command
            .args(["exec", "--config", "workflow.yaml"])
            .env("JCODE_MODEL", "backend/default-model")
            .output()
            .unwrap_or_else(|error| panic!("run cruise: {error}"));

        assert_missing_model_diagnostic(&output, &user_file);
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .to_ascii_lowercase()
                .contains("creat"),
            "first-run diagnostic must report template creation"
        );
        assert_null_model_template(&user_file);
        assert!(
            !project_defaults.exists(),
            "project defaults must not be auto-created"
        );
        #[cfg(unix)]
        assert!(
            !marker.exists(),
            "a missing-model request must not launch jcode"
        );
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        assert!(
            !stderr.contains("model: default model (jcode)"),
            "missing Cruise model must not be reported as a selected backend default: {stderr}"
        );
    }
}

#[test]
fn unedited_null_template_rejects_again_without_rewriting_or_starting_jcode() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let user_file = home.path().join("config/cruise/default.yaml");
    std::fs::create_dir_all(
        user_file
            .parent()
            .unwrap_or_else(|| panic!("user config parent")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let original = "# keep this comment\nmodel: null\n";
    std::fs::write(&user_file, original).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        workflow_for_jcode(Some("jcode")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    cruise::test_support::init_git_repo(project.path());
    let marker = home.path().join("jcode-started");
    let mut command = cruise(home.path(), project.path());
    #[cfg(unix)]
    install_jcode_marker(&mut command, home.path(), &marker);

    let output = command
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run cruise: {error}"));

    assert_missing_model_diagnostic(&output, &user_file);
    let diagnostic = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        diagnostic.contains("exist") || diagnostic.contains("keep"),
        "{diagnostic}"
    );
    assert_eq!(
        std::fs::read_to_string(&user_file).unwrap_or_default(),
        original
    );
    #[cfg(unix)]
    assert!(
        !marker.exists(),
        "the SDK must not start while model remains null"
    );
}

#[test]
fn existing_user_defaults_are_preserved_when_the_jcode_model_is_missing() {
    for original in [
        "",
        "# user comment\n",
        "env:\n  KEEP: value\n",
        "pre_steps:\n  setup:\n    command: echo setup\n",
    ] {
        let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let user_file = home.path().join("config/cruise/default.yaml");
        std::fs::create_dir_all(
            user_file
                .parent()
                .unwrap_or_else(|| panic!("user config parent")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&user_file, original).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            project.path().join("workflow.yaml"),
            workflow_for_jcode(Some("jcode")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cruise::test_support::init_git_repo(project.path());
        let mut command = cruise(home.path(), project.path());
        #[cfg(unix)]
        install_jcode_marker(
            &mut command,
            home.path(),
            &home.path().join("jcode-started"),
        );

        let output = command
            .args(["exec", "--config", "workflow.yaml"])
            .output()
            .unwrap_or_else(|error| panic!("run cruise: {error}"));

        assert_missing_model_diagnostic(&output, &user_file);
        assert_eq!(
            std::fs::read_to_string(&user_file).unwrap_or_default(),
            original,
            "existing defaults must remain byte-for-byte unchanged"
        );
    }
}

#[cfg(unix)]
#[test]
fn existing_user_defaults_symlink_is_not_replaced() {
    use std::os::unix::fs::symlink;

    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let user_dir = home.path().join("config/cruise");
    std::fs::create_dir_all(&user_dir).unwrap_or_else(|error| panic!("{error}"));
    let target = home.path().join("external-defaults.yaml");
    let original = "# symlink target stays intact\n";
    std::fs::write(&target, original).unwrap_or_else(|error| panic!("{error}"));
    let user_file = user_dir.join("default.yaml");
    symlink(&target, &user_file).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        workflow_for_jcode(Some("jcode")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    cruise::test_support::init_git_repo(project.path());
    let mut command = cruise(home.path(), project.path());
    install_jcode_marker(
        &mut command,
        home.path(),
        &home.path().join("jcode-started"),
    );

    let output = command
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run cruise: {error}"));

    assert_missing_model_diagnostic(&output, &user_file);
    assert!(
        std::fs::symlink_metadata(&user_file)
            .unwrap_or_else(|error| panic!("{error}"))
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(target).unwrap_or_default(),
        original
    );
}

#[test]
fn malformed_existing_user_defaults_keeps_the_yaml_parse_error_without_replacement() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let user_file = home.path().join("config/cruise/default.yaml");
    std::fs::create_dir_all(
        user_file
            .parent()
            .unwrap_or_else(|| panic!("user config parent")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let original = "model: [broken\n";
    std::fs::write(&user_file, original).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        workflow_for_jcode(Some("jcode")),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let output = cruise(home.path(), project.path())
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run cruise: {error}"));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("parse"),
        "expected the existing YAML parse error: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(user_file).unwrap_or_default(),
        original
    );
}

#[test]
fn concurrent_missing_model_runs_create_one_complete_user_template() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        workflow_for_jcode(Some("jcode")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    cruise::test_support::init_git_repo(project.path());
    let user_file = home.path().join("config/cruise/default.yaml");
    let mut children = Vec::new();
    for index in 0..2 {
        let mut command = cruise(home.path(), project.path());
        #[cfg(unix)]
        install_jcode_marker(
            &mut command,
            home.path(),
            &home.path().join(format!("jcode-started-{index}")),
        );
        children.push(
            command
                .args(["exec", "--config", "workflow.yaml"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| panic!("spawn cruise: {error}")),
        );
    }

    let mut diagnostics = Vec::new();
    for child in children {
        let output = child
            .wait_with_output()
            .unwrap_or_else(|error| panic!("wait for cruise: {error}"));
        assert_missing_model_diagnostic(&output, &user_file);
        diagnostics.push(String::from_utf8_lossy(&output.stderr).to_ascii_lowercase());
    }

    assert_null_model_template(&user_file);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|message| message.contains("creat"))
            .count(),
        1,
        "exactly one racing request must report creating the template: {diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|message| message.contains("exist")),
        "the other racing request must report preserving the file: {diagnostics:?}"
    );
    #[cfg(unix)]
    for index in 0..2 {
        assert!(!home.path().join(format!("jcode-started-{index}")).exists());
    }
}

#[test]
fn dry_run_workflow_list_and_draft_do_not_create_model_defaults() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        workflow_for_jcode(Some("jcode")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let user_file = home.path().join("config/cruise/default.yaml");

    let dry_run = cruise(home.path(), project.path())
        .args(["exec", "--dry-run", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run dry-run: {error}"));
    assert!(dry_run.status.success(), "{}", text(&dry_run));
    assert!(!user_file.exists());

    let listing = cruise(home.path(), project.path())
        .args(["workflow", "list"])
        .output()
        .unwrap_or_else(|error| panic!("run workflow list: {error}"));
    assert!(listing.status.success(), "{}", text(&listing));
    assert!(!user_file.exists());

    let draft = cruise(home.path(), project.path())
        .args(["draft", "draft input", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run draft: {error}"));
    assert!(draft.status.success(), "{}", text(&draft));
    assert!(
        !user_file.exists(),
        "drafting must not initialize model defaults"
    );
}

#[cfg(unix)]
#[test]
fn command_backend_without_a_model_executes_without_creating_user_defaults() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        "command: [sh, -c, 'cat >/dev/null; printf command-ran']\nsteps:\n  main-step:\n    prompt: perform task\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    cruise::test_support::init_git_repo(project.path());

    let output = cruise(home.path(), project.path())
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run command backend: {error}"));

    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("command-ran"));
    assert!(!home.path().join("config/cruise/default.yaml").exists());
    assert!(!project.path().join(".cruise/default.yaml").exists());
}

#[cfg(unix)]
#[test]
fn claude_backend_without_a_model_executes_without_creating_user_defaults() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        workflow_for_jcode(Some("claude")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    cruise::test_support::init_git_repo(project.path());
    let user_file = home.path().join("config/cruise/default.yaml");
    let marker = home.path().join("claude-started");
    let mut command = cruise(home.path(), project.path());
    install_claude_marker(&mut command, home.path(), &marker);

    let output = command
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run Claude backend: {error}"));

    assert!(output.status.success(), "{}", text(&output));
    assert!(marker.is_file(), "Claude prompt should be executed");
    assert!(!user_file.exists());
    assert!(!project.path().join(".cruise/default.yaml").exists());
}

#[cfg(unix)]
#[test]
fn configured_user_model_recovers_a_fresh_request_without_mutating_defaults() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        workflow_for_jcode(Some("jcode")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    cruise::test_support::init_git_repo(project.path());
    let user_file = home.path().join("config/cruise/default.yaml");

    let missing_model = cruise(home.path(), project.path())
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run missing-model request: {error}"));
    assert_missing_model_diagnostic(&missing_model, &user_file);
    assert_null_model_template(&user_file);

    let template = std::fs::read_to_string(&user_file).unwrap_or_else(|error| panic!("{error}"));
    let configured = template.replacen("model: null", "model: anthropic/recovery-test", 1);
    assert_ne!(
        configured, template,
        "the generated model placeholder must be replaced"
    );
    std::fs::write(&user_file, configured.as_bytes()).unwrap_or_else(|error| panic!("{error}"));
    let configured_bytes = std::fs::read(&user_file).unwrap_or_else(|error| panic!("{error}"));

    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("{error}"));
    let marker = home.path().join("model-received");
    cruise::test_support::write_executable_script(
        &bin.join("model-recorder"),
        &format!(
            "#!/bin/sh\nprintf '%s' \"$1\" > {}\nexit 0\n",
            sh_quote(&marker)
        ),
    );
    std::fs::write(
        project.path().join("workflow.yaml"),
        "command: [model-recorder, '{model}']\nsteps:\n  main-step:\n    prompt: perform the requested task\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mut command = cruise(home.path(), project.path());
    prepend_command_path(&mut command, &bin);
    let recovered = command
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run recovered request: {error}"));

    assert!(recovered.status.success(), "{}", text(&recovered));
    assert_eq!(
        std::fs::read(&marker).unwrap_or_else(|error| panic!("{error}")),
        b"anthropic/recovery-test",
        "the resolved user-default model must reach the non-network runner"
    );
    assert_eq!(
        std::fs::read(&user_file).unwrap_or_else(|error| panic!("{error}")),
        configured_bytes,
        "a recovered request must leave the user defaults byte-for-byte unchanged"
    );
}

#[cfg(unix)]
#[test]
fn app_config_models_reach_exec_and_planning_with_existing_precedence() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    cruise::test_support::init_git_repo(project.path());
    let marker = home.path().join("selected-models");
    write_model_workflow(project.path(), None);
    write_app_config(
        home.path(),
        r#"{"model":["app/primary","app/fallback"],"plan_model":["plan/app-primary","plan/app-fallback"]}"#,
    );

    let run_exec = |env_model: Option<&str>| {
        let mut command = cruise(home.path(), project.path());
        install_model_recorder(&mut command, home.path(), &marker);
        if let Some(model) = env_model {
            command.env("CRUISE_MODEL", model);
        }
        command
            .args(["exec", "--config", "workflow.yaml"])
            .output()
            .unwrap_or_else(|error| panic!("run model exec: {error}"))
    };
    let recorded_models = || {
        std::fs::read_to_string(&marker)
            .unwrap_or_else(|error| panic!("read selected models: {error}"))
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    let app_list = run_exec(None);
    assert!(app_list.status.success(), "{}", text(&app_list));
    assert_eq!(recorded_models(), ["app/primary", "step/model"]);

    write_app_config(home.path(), r#"{"model":"app/scalar"}"#);
    std::fs::remove_file(&marker).unwrap_or_else(|error| panic!("clear model marker: {error}"));
    let app_scalar = run_exec(None);
    assert!(app_scalar.status.success(), "{}", text(&app_scalar));
    assert_eq!(recorded_models(), ["app/scalar", "step/model"]);

    let user_defaults = home.path().join("config/cruise/default.yaml");
    std::fs::create_dir_all(
        user_defaults
            .parent()
            .unwrap_or_else(|| panic!("user config parent")),
    )
    .unwrap_or_else(|error| panic!("create user config directory: {error}"));
    std::fs::write(&user_defaults, "model: user/default\n")
        .unwrap_or_else(|error| panic!("write user defaults: {error}"));
    let project_defaults = project.path().join(".cruise/default.yaml");
    std::fs::create_dir_all(
        project_defaults
            .parent()
            .unwrap_or_else(|| panic!("project config parent")),
    )
    .unwrap_or_else(|error| panic!("create project config directory: {error}"));
    std::fs::write(&project_defaults, "model: project/default\n")
        .unwrap_or_else(|error| panic!("write project defaults: {error}"));
    write_app_config(home.path(), r#"{"model":["app/primary","app/fallback"]}"#);
    write_model_workflow(project.path(), None);
    std::fs::remove_file(&marker).unwrap_or_else(|error| panic!("clear model marker: {error}"));
    let project_default = run_exec(None);
    assert!(
        project_default.status.success(),
        "{}",
        text(&project_default)
    );
    assert_eq!(recorded_models(), ["project/default", "step/model"]);

    write_model_workflow(project.path(), Some("workflow/model"));
    std::fs::remove_file(&marker).unwrap_or_else(|error| panic!("clear model marker: {error}"));
    let workflow_model = run_exec(None);
    assert!(workflow_model.status.success(), "{}", text(&workflow_model));
    assert_eq!(recorded_models(), ["workflow/model", "step/model"]);

    std::fs::remove_file(&marker).unwrap_or_else(|error| panic!("clear model marker: {error}"));
    let environment_model = run_exec(Some("env/model"));
    assert!(
        environment_model.status.success(),
        "{}",
        text(&environment_model)
    );
    assert_eq!(recorded_models(), ["env/model", "step/model"]);

    write_app_config(
        home.path(),
        r#"{"model":"app/model","plan_model":["plan/app-primary","plan/app-fallback"]}"#,
    );
    write_model_workflow(project.path(), None);
    std::fs::remove_file(&marker).unwrap_or_else(|error| panic!("clear model marker: {error}"));
    let mut planning = cruise(home.path(), project.path());
    install_model_recorder(&mut planning, home.path(), &marker);
    let plan = planning
        .args([
            "plan",
            "--no-interactive-planning",
            "--config",
            "workflow.yaml",
            "create a plan",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("run model planning: {error}"));
    assert!(plan.status.success(), "{}", text(&plan));
    assert_eq!(recorded_models(), ["plan/app-primary"]);
}

#[cfg(unix)]
#[test]
fn app_config_model_only_json_exec_selects_the_primary() {
    let home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    cruise::test_support::init_git_repo(project.path());
    let marker = home.path().join("selected-model");
    write_model_workflow(project.path(), None);
    write_app_config(home.path(), r#"{"model":["app/primary","app/fallback"]}"#);

    let mut command = cruise(home.path(), project.path());
    install_model_recorder(&mut command, home.path(), &marker);
    let output = command
        .args(["exec", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run model-only config exec: {error}"));

    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        std::fs::read_to_string(marker)
            .unwrap_or_else(|error| panic!("read selected model: {error}")),
        "app/primary\nstep/model\n"
    );
}

#[test]
fn removed_builtin_names_are_rejected_by_cli() {
    let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let project = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    for name in ["default", "simple", "review"] {
        let output = cruise(home.path(), project.path())
            .args(["workflow", "eject", name, "--to", "user"])
            .output()
            .unwrap_or_else(|e| panic!("run cruise: {e}"));
        let terminal = text(&output);
        assert!(!output.status.success(), "{name} unexpectedly ejected");
        assert!(
            terminal.contains("unknown built-in workflow"),
            "{name}: {terminal}"
        );
    }
}

#[cfg(unix)]
fn builtin_wait_ci_command(name: &str) -> String {
    builtin_wait_ci_command_for_url(name, "https://github.com/owner/repo/pull/42")
}

#[cfg(unix)]
fn builtin_wait_ci_command_for_url(name: &str, pr_url: &str) -> String {
    let workflow = cruise::builtin_workflows::find_builtin(name)
        .unwrap_or_else(|| panic!("missing builtin workflow {name}"));
    let source_dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let config = cruise::resolver::resolve_workflow_config(
        workflow.yaml,
        &cruise::resolver::ConfigSource::NamedBuiltin(workflow.name),
        source_dir.path(),
        None,
    )
    .unwrap_or_else(|error| panic!("load builtin workflow {name}: {error}"));
    let command = config
        .after_pr
        .get("follow-up/wait-ci")
        .and_then(|step| step.command.as_ref())
        .unwrap_or_else(|| panic!("{name} has no wait-ci command"));
    let mut variables = cruise::variable::VariableStore::new(String::new());
    variables.set_named_value("pr.number", "42".to_owned());
    variables.set_named_value("pr.url", pr_url.to_owned());
    variables
        .resolve(&command.as_slice().join("\n"))
        .unwrap_or_else(|error| panic!("resolve {name} wait-ci command: {error}"))
}

#[cfg(unix)]
fn wait_ci_marker(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .find(|line| line.starts_with("CRUISE_CI_DIAGNOSTIC_V1 "))
        .unwrap_or_else(|| panic!("missing wait-ci diagnostic marker: {}", text(output)))
        .to_owned()
}

#[cfg(unix)]
fn run_gitlab_wait_ci_command(command: &str, status_response: &str, status_exit: i32) -> Output {
    let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let repo = root.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("create repo: {error}"));
    cruise::test_support::init_git_repo(&repo);
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("create fake bin: {error}"));
    cruise::test_support::write_executable_script(
        &bin.join("glab"),
        r#"#!/bin/sh
case "$*" in
  *--status=failed*) printf '{"id":1,"jobs":[]}\n'; exit 0 ;;
  *) printf '%s\n' "$WAIT_CI_STATUS_RESPONSE"; exit "$WAIT_CI_STATUS_EXIT" ;;
esac
"#,
    );
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths)
        .unwrap_or_else(|error| panic!("join fake command PATH: {error}"));
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(repo)
        .env("PATH", path)
        .env("WAIT_CI_STATUS_RESPONSE", status_response)
        .env("WAIT_CI_STATUS_EXIT", status_exit.to_string())
        .output()
        .unwrap_or_else(|error| panic!("run builtin GitLab wait-ci command: {error}"))
}

#[cfg(unix)]
const FAKE_GH_SCRIPT: &str = r#"#!/bin/sh
if [ "$1" = repo ]; then
  printf '123/true\n'
  exit 0
fi
if [ "$1" != api ] || [ "$2" != graphql ]; then
  printf 'unexpected fake gh call: %s\n' "$*" >&2
  exit 91
fi
case " $* " in
  *" head=$WAIT_CI_EXPECTED_HEAD "*) ;;
  *) printf 'GraphQL request was not pinned to local HEAD: %s\n' "$*" >&2; exit 92 ;;
esac
case " $* " in
  *" number=42 "*) ;;
  *) printf 'GraphQL request did not identify PR 42: %s\n' "$*" >&2; exit 93 ;;
esac
case "$*" in
  *'pullRequest(number:$number)'*'object(oid:$head)'*statusCheckRollup*) ;;
  *) printf 'GraphQL request did not correlate the PR head with its commit rollup: %s\n' "$*" >&2; exit 94 ;;
esac
typed_owner=false
typed_name=false
typed_head=false
typed_number=false
previous=
for arg in "$@"; do
  case "$previous:$arg" in
    -F:owner=123)
      printf '{"errors":[{"message":"owner was coerced to a number"}]}\n'
      exit 0 ;;
    -F:head=*)
      printf '{"errors":[{"message":"head was passed using typed field coercion"}]}\n'
      exit 0 ;;
    -F:name=true)
      printf '{"errors":[{"message":"name was coerced to a boolean"}]}\n'
      exit 0 ;;
    -F:number=42) typed_number=true ;;
    -f:number=42)
      printf '{"errors":[{"message":"number was passed as a raw string"}]}\n'
      exit 0 ;;
    -f:owner=123) typed_owner=true ;;
    -f:name=true) typed_name=true ;;
    -f:head=$WAIT_CI_EXPECTED_HEAD) typed_head=true ;;
  esac
  previous=$arg
done
if [ "$typed_owner" != true ] || [ "$typed_name" != true ] || [ "$typed_head" != true ] || [ "$typed_number" != true ]; then
  printf '{"errors":[{"message":"owner/name/head were not passed as strings or number was not typed"}]}\n'
  exit 0
fi
count=$(cat "$WAIT_CI_GH_COUNT" 2>/dev/null || printf 0)
count=$((count + 1))
printf '%s\n' "$count" > "$WAIT_CI_GH_COUNT"
if [ "${WAIT_CI_GH_API_FAILURE-0}" = 1 ]; then
  printf 'fake GitHub API failure\n' >&2
  exit 95
fi
response="$WAIT_CI_RESPONSES/$count.json"
if [ ! -f "$response" ]; then
  printf 'no fake GitHub response for poll %s\n' "$count" >&2
  exit 94
fi
cat "$response"
"#;

#[cfg(unix)]
struct GithubWaitCiFixture {
    _root: TempDir,
    repo: PathBuf,
    bin: PathBuf,
    responses: PathBuf,
    gh_count: PathBuf,
    date_count: PathBuf,
    head: String,
}

#[cfg(unix)]
impl GithubWaitCiFixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("create repo: {error}"));
        cruise::test_support::init_git_repo(&repo);
        let head_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap_or_else(|error| panic!("read test HEAD: {error}"));
        assert!(head_output.status.success(), "read test HEAD failed");
        let head = String::from_utf8(head_output.stdout)
            .unwrap_or_else(|error| panic!("HEAD is not UTF-8: {error}"))
            .trim()
            .to_owned();

        let bin = root.path().join("bin");
        let responses = root.path().join("responses");
        std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("create fake bin: {error}"));
        std::fs::create_dir_all(&responses)
            .unwrap_or_else(|error| panic!("create fake responses: {error}"));
        let gh_count = root.path().join("gh-count");
        let date_count = root.path().join("date-count");
        cruise::test_support::write_executable_script(&bin.join("gh"), FAKE_GH_SCRIPT);
        cruise::test_support::write_executable_script(
            &bin.join("date"),
            r#"#!/bin/sh
count=$(cat "$WAIT_CI_DATE_COUNT" 2>/dev/null || printf 0)
count=$((count + 1))
printf '%s\n' "$count" > "$WAIT_CI_DATE_COUNT"
if [ "${WAIT_CI_TIMEOUT-0}" = 1 ] && [ "$count" -gt 1 ]; then
  printf '1900\n'
elif [ "$count" -gt 1 ]; then
  printf '1101\n'
else
  printf '100\n'
fi
"#,
        );
        cruise::test_support::write_executable_script(&bin.join("sleep"), "#!/bin/sh\nexit 0\n");
        Self {
            _root: root,
            repo,
            bin,
            responses,
            gh_count,
            date_count,
            head,
        }
    }

    fn run(&self, command: &str, responses: &[serde_json::Value], timeout: bool) -> Output {
        self.run_with_api_error(command, responses, timeout, false)
    }

    fn run_api_error(&self, command: &str) -> Output {
        self.run_with_api_error(command, &[], false, true)
    }

    fn run_with_api_error(
        &self,
        command: &str,
        responses: &[serde_json::Value],
        timeout: bool,
        api_error: bool,
    ) -> Output {
        let _ = std::fs::remove_file(&self.gh_count);
        let _ = std::fs::remove_file(&self.date_count);
        for (index, response) in responses.iter().enumerate() {
            let bytes = serde_json::to_vec(response)
                .unwrap_or_else(|error| panic!("serialize fake GitHub response: {error}"));
            std::fs::write(self.responses.join(format!("{}.json", index + 1)), bytes)
                .unwrap_or_else(|error| panic!("write fake GitHub response: {error}"));
        }
        let mut paths = vec![self.bin.clone()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(paths)
            .unwrap_or_else(|error| panic!("join fake command PATH: {error}"));
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.repo)
            .env("PATH", path)
            .env("WAIT_CI_EXPECTED_HEAD", &self.head)
            .env("WAIT_CI_GH_COUNT", &self.gh_count)
            .env("WAIT_CI_DATE_COUNT", &self.date_count)
            .env("WAIT_CI_RESPONSES", &self.responses)
            .env("WAIT_CI_TIMEOUT", if timeout { "1" } else { "0" })
            .env("WAIT_CI_GH_API_FAILURE", if api_error { "1" } else { "0" })
            .output()
            .unwrap_or_else(|error| panic!("run builtin wait-ci command: {error}"))
    }

    fn gh_calls(&self) -> usize {
        std::fs::read_to_string(&self.gh_count)
            .unwrap_or_else(|error| panic!("read fake gh call count: {error}"))
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("parse fake gh call count: {error}"))
    }
    fn date_calls(&self) -> usize {
        std::fs::read_to_string(&self.date_count)
            .unwrap_or_else(|error| panic!("read fake date call count: {error}"))
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("parse fake date call count: {error}"))
    }
}

#[cfg(unix)]
fn github_ci_response(
    pr_head: &str,
    rollup_head: &str,
    state: Option<&str>,
    check_count: usize,
) -> serde_json::Value {
    let rollup = state.map_or(serde_json::Value::Null, |state| {
        serde_json::json!({
            "state": state,
            "contexts": {"totalCount": check_count}
        })
    });
    serde_json::json!({
        "data": {
            "repository": {
                "pullRequest": {"headRefOid": pr_head},
                "object": {
                    "oid": rollup_head,
                    "statusCheckRollup": rollup
                }
            }
        }
    })
}

#[cfg(unix)]
#[test]
fn builtin_github_wait_ci_pins_local_head_and_waits_for_registered_checks() {
    let old_head = "1111111111111111111111111111111111111111";
    for workflow in cruise::builtin_workflows::BUILTIN_WORKFLOWS {
        let fixture = GithubWaitCiFixture::new();
        let command = builtin_wait_ci_command(workflow.name);
        let output = fixture.run(
            &command,
            &[
                github_ci_response(&fixture.head, old_head, Some("SUCCESS"), 1),
                github_ci_response(&fixture.head, &fixture.head, None, 0),
                github_ci_response(&fixture.head, &fixture.head, Some("SUCCESS"), 0),
                github_ci_response(&fixture.head, &fixture.head, Some("PENDING"), 1),
                github_ci_response(&fixture.head, &fixture.head, Some("SUCCESS"), 1),
            ],
            false,
        );
        let terminal = text(&output);
        assert!(output.status.success(), "{}: {terminal}", workflow.name);
        assert_eq!(fixture.gh_calls(), 5, "{}", workflow.name);
    }

    let command = builtin_wait_ci_command("tdd");
    let fixture = GithubWaitCiFixture::new();
    let output = fixture.run(
        &command,
        &[github_ci_response(
            &fixture.head,
            &fixture.head,
            Some("FAILURE"),
            1,
        )],
        false,
    );
    let terminal = text(&output);
    assert!(!output.status.success(), "{terminal}");
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 github ci_failed FAILURE true 1"
    );
    assert_eq!(fixture.gh_calls(), 1);

    let fixture = GithubWaitCiFixture::new();
    let changed_head = "2222222222222222222222222222222222222222";
    let output = fixture.run(
        &command,
        &[github_ci_response(
            changed_head,
            &fixture.head,
            Some("SUCCESS"),
            1,
        )],
        false,
    );
    let terminal = text(&output);
    assert!(!output.status.success(), "{terminal}");
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 github head_mismatch unobserved false unknown"
    );
    assert_eq!(fixture.gh_calls(), 1);

    let fixture = GithubWaitCiFixture::new();
    let output = fixture.run(
        &command,
        &[serde_json::json!({"errors": [{"message": "query rejected"}]})],
        false,
    );
    let terminal = text(&output);
    assert!(!output.status.success(), "{terminal}");
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 github graphql_error unobserved unknown unknown"
    );
    assert!(!terminal.contains("query rejected"), "{terminal}");
    assert_eq!(fixture.gh_calls(), 1);
    let fixture = GithubWaitCiFixture::new();
    let output = fixture.run_api_error(&command);
    let terminal = text(&output);
    assert!(!output.status.success(), "{terminal}");
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 github api_error unobserved unknown unknown"
    );
    assert_eq!(fixture.gh_calls(), 1);

    for (state, check_count) in [(None, 0), (Some("SUCCESS"), 0)] {
        let fixture = GithubWaitCiFixture::new();
        let output = fixture.run(
            &command,
            &[github_ci_response(
                &fixture.head,
                &fixture.head,
                state,
                check_count,
            )],
            true,
        );
        let terminal = text(&output);
        assert!(!output.status.success(), "{terminal}");
        assert_eq!(fixture.gh_calls(), 1);
        assert_eq!(fixture.date_calls(), 2);
    }
}
#[cfg(unix)]
#[test]
fn builtin_gitlab_wait_ci_markers_classify_failure_and_untrusted_responses() {
    let command =
        builtin_wait_ci_command_for_url("tdd", "https://gitlab.com/owner/repo/-/merge_requests/42");

    let output = run_gitlab_wait_ci_command(&command, r#"{"status":"failed"}"#, 0);
    assert!(!output.status.success(), "{}", text(&output));
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 gitlab ci_failed failed unknown unknown"
    );

    let output = run_gitlab_wait_ci_command(&command, "", 1);
    assert!(!output.status.success(), "{}", text(&output));
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 gitlab api_error unknown unknown unknown"
    );

    let output = run_gitlab_wait_ci_command(&command, "not-json", 0);
    assert!(!output.status.success(), "{}", text(&output));
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 gitlab invalid_ci_state unknown unknown unknown"
    );

    let output = run_gitlab_wait_ci_command(&command, r#"{"status":"secret raw status"}"#, 0);
    assert!(!output.status.success(), "{}", text(&output));
    assert_eq!(
        wait_ci_marker(&output),
        "CRUISE_CI_DIAGNOSTIC_V1 gitlab unknown_state unknown unknown unknown"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret raw status"));
}
