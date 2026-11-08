use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("create fixture");
        std::fs::create_dir_all(root.path().join("home")).expect("create home");
        std::fs::create_dir_all(root.path().join("project")).expect("create project");
        Self { root }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        for name in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
            command.env_remove(name);
        }
        command
            .current_dir(self.root.path().join("project"))
            .env("HOME", self.root.path().join("home"))
            .env("USERPROFILE", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        command
    }
}

fn text(output: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run(command: &mut Command) -> Output {
    command.output().expect("run cruise")
}

#[test]
fn environment_language_has_priority_and_invalid_value_warns() {
    let fixture = Fixture::new();
    let output = run(fixture
        .command()
        .args(["config"])
        .env("CRUISE_UI_LANGUAGE", "ja"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("ui_language = ja (source: env)"),
        "{}",
        text(&output)
    );

    let output = run(fixture
        .command()
        .args(["config"])
        .env("CRUISE_UI_LANGUAGE", "xx"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .starts_with("warning: ignoring CRUISE_UI_LANGUAGE='xx'"),
        "{}",
        text(&output)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("ui_language = en (source: default)"),
        "{}",
        text(&output)
    );
}

#[test]
fn system_locale_localizes_empty_session_list() {
    let fixture = Fixture::new();
    let output = run(fixture.command().arg("list").env("LANG", "ko_KR.UTF-8"));
    assert!(output.status.success(), "{}", text(&output));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n/ko.toml");
    let contents = std::fs::read_to_string(path).expect("read Korean catalog");
    let catalog: toml::Value = toml::from_str(&contents).expect("parse Korean catalog");
    let expected = catalog["cli"]["list"]["empty"]
        .as_str()
        .expect("Korean empty-list translation");
    assert!(!expected.is_empty());
    assert!(
        text(&output).contains(expected),
        "expected {expected:?}, {}",
        text(&output)
    );
}

#[test]
fn language_setting_persists_and_auto_clears_it() {
    let fixture = Fixture::new();
    let output = run(fixture.command().args(["config", "--set-language", "ja"]));
    assert!(output.status.success(), "{}", text(&output));
    let output = run(fixture.command().arg("config"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("ui_language = ja (source: config)"),
        "{}",
        text(&output)
    );
    let output = run(fixture.command().args(["config", "--set-language", "auto"]).env("CRUISE_UI_LANGUAGE", "ko"));
    assert!(output.status.success(), "{}", text(&output));
    let output = run(fixture.command().arg("config").env("CRUISE_UI_LANGUAGE", "ko"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("ui_language = ko (source: env)"), "{}", text(&output));
    let output = run(fixture.command().arg("config"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("ui_language = en (source: default)"),
        "{}",
        text(&output)
    );
}

#[test]
fn invalid_language_is_rejected_before_writing() {
    let fixture = Fixture::new();
    let output = run(fixture.command().args(["config", "--set-language", "xx"]));
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("zh-CN"),
        "{}",
        text(&output)
    );
    assert!(
        !fixture
            .root
            .path()
            .join("config/cruise/config.json")
            .exists()
    );
}

#[test]
fn ui_language_does_not_change_other_config_output_or_pr_language_override() {
    let fixture = Fixture::new();
    let invoke = |language| {
        run(fixture
            .command()
            .arg("config")
            .env("CRUISE_LANGUAGE_PR", "French")
            .env("CRUISE_UI_LANGUAGE", language))
    };
    let en = invoke("en");
    let ja = invoke("ja");
    assert!(
        en.status.success() && ja.status.success(),
        "{}\n{}",
        text(&en),
        text(&ja)
    );
    let without_ui = |output: &Output| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.starts_with("ui_language = "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(without_ui(&en), without_ui(&ja));
}
