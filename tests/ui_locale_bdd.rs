#![expect(clippy::unwrap_used, reason = "isolated BDD fixtures")]
#![expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber requires owned String captures for step parameters"
)]
use cucumber::{World as _, given, then, when};
use std::process::{Command, Output, Stdio};

#[derive(Debug, Default, cucumber::World)]
struct LocaleWorld {
    root: Option<tempfile::TempDir>,
    saved: Option<String>,
    env: Option<String>,
    system: Option<String>,
    output: Option<Output>,
}
impl LocaleWorld {
    fn command(&self) -> Command {
        let root = self.root.as_ref().unwrap().path();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(name);
            }
        }
        for name in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
            cmd.env_remove(name);
        }
        cmd.current_dir(root)
            .env("HOME", root)
            .env("USERPROFILE", root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        cmd
    }
}
#[given(expr = "a saved UI language {string}")]
fn saved(w: &mut LocaleWorld, value: String) {
    w.saved = Some(value);
}
#[given(expr = "the environment UI language is {string}")]
fn env(w: &mut LocaleWorld, value: String) {
    w.env = Some(value);
}
#[given(expr = "the system locale is {string}")]
fn system(w: &mut LocaleWorld, value: String) {
    w.system = Some(value);
}
#[when("I inspect the UI language configuration")]
fn inspect(w: &mut LocaleWorld) {
    w.root = Some(tempfile::tempdir().unwrap());
    if let Some(value) = &w.saved {
        let out = w
            .command()
            .args(["config", "--set-language", value])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    inspect_again(w);
}
#[when("I inspect the UI language configuration again")]
fn inspect_again(w: &mut LocaleWorld) {
    let mut cmd = w.command();
    cmd.arg("config");
    if let Some(value) = &w.env {
        cmd.env("CRUISE_UI_LANGUAGE", value);
    }
    if let Some(value) = &w.system {
        cmd.env("LANG", value);
    }
    w.output = Some(cmd.output().unwrap());
}
#[when(expr = "I set the UI language to {string}")]
fn set_language(w: &mut LocaleWorld, value: String) {
    let out = w
        .command()
        .args(["config", "--set-language"])
        .arg(value)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    w.output = Some(out);
}
#[when(expr = "I attempt to set the UI language to {string}")]
fn attempt_language(w: &mut LocaleWorld, value: String) {
    w.output = Some(
        w.command()
            .args(["config", "--set-language"])
            .arg(value)
            .output()
            .unwrap(),
    );
}
#[then("the language setting is rejected")]
fn rejected(w: &mut LocaleWorld) {
    let out = w.output.as_ref().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("zh-CN"));
}
#[then(expr = "a warning mentions {string}")]
fn warning(w: &mut LocaleWorld, value: String) {
    let stderr = String::from_utf8_lossy(&w.output.as_ref().unwrap().stderr);
    assert!(
        stderr.contains("warning: ignoring") && stderr.contains(&value),
        "{stderr}"
    );
}
#[when("I list sessions")]
fn list(w: &mut LocaleWorld) {
    w.root = Some(tempfile::tempdir().unwrap());
    let mut cmd = w.command();
    cmd.arg("list");
    if let Some(value) = &w.system {
        cmd.env("LANG", value);
    }
    w.output = Some(cmd.output().unwrap());
}
#[then(expr = "the output contains {string}")]
fn output_contains(w: &mut LocaleWorld, expected: String) {
    let out = w.output.as_ref().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success() && text.contains(&expected), "{text}");
}
#[then(expr = "the effective UI language is {string} from {string}")]
fn effective(w: &mut LocaleWorld, language: String, source: String) {
    let out = w.output.as_ref().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(&format!("ui_language = {language} (source: {source})")),
        "{stdout}"
    );
}

#[then("the effective UI language is the platform fallback")]
fn platform_fallback(w: &mut LocaleWorld) {
    let out = w.output.as_ref().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let system_locale = cruise::i18n::resolve::system_locale_windows()
        .and_then(|value| cruise::i18n::resolve::from_system_locale(&value));
    let expected = system_locale.map_or_else(
        || "ui_language = en (source: default)".to_string(),
        |locale| format!("ui_language = {} (source: system)", locale.tag()),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&expected), "{stdout}");
}
#[tokio::test(flavor = "current_thread")]
async fn ui_locale_bdd() {
    LocaleWorld::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/ui_locale.feature"
        ))
        .await;
}
