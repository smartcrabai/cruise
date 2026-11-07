//! Executable environment override contracts against the production CLI.
use std::collections::BTreeMap;
use std::process::{Command, Output, Stdio};

use cucumber::{World as _, given, then, when};

#[derive(Debug, Default, cucumber::World)]
struct ConfigEnvWorld {
    root: Option<tempfile::TempDir>,
    env: BTreeMap<String, String>,
    output: Option<Output>,
}

impl ConfigEnvWorld {
    fn run_cli(&mut self, args: &[&str]) {
        let root = self
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("isolated project"))
            .path();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(root.join("project"))
            .env("HOME", root.join("home"))
            .env("USERPROFILE", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("NO_COLOR", "1")
            .env_remove("JCODE_MODEL")
            .env_remove("HERDR_ENV")
            .envs(&self.env)
            .stdin(Stdio::null())
            .args(args);
        self.output = Some(
            cmd.output()
                .unwrap_or_else(|error| panic!("run production cruise CLI: {error}")),
        );
    }

    fn output(&self) -> &Output {
        self.output
            .as_ref()
            .unwrap_or_else(|| panic!("command was run"))
    }

    fn text(&self) -> String {
        let out = self.output();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

#[given("an isolated Cruise project")]
fn isolated(world: &mut ConfigEnvWorld) {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap_or_else(|error| panic!("create project: {error}"));
    std::fs::write(
        project.join("workflow.yaml"),
        "command: [echo]\nsteps:\n  main:\n    command: echo main\n",
    )
    .unwrap_or_else(|error| panic!("write workflow: {error}"));
    world.root = Some(root);
}

#[given(regex = r#"^(CRUISE_[A-Z_]+) is set to "(.*)"$"#)]
fn set_env(world: &mut ConfigEnvWorld, key: String, value: String) {
    world.env.insert(key, value);
}

#[when(regex = r"^I run cruise (config|exec)$")]
fn run_command(world: &mut ConfigEnvWorld, matches: &[String]) {
    match matches[0].as_str() {
        "config" => world.run_cli(&["config"]),
        "exec" => world.run_cli(&["exec", "--config", "workflow.yaml"]),
        _ => unreachable!(),
    }
}

#[when(regex = r#"^I run cruise config setting parallelism to "([^"]+)"$"#)]
fn set_parallelism(world: &mut ConfigEnvWorld, matches: &[String]) {
    world.run_cli(&["config", "--set-parallelism", &matches[0]]);
}

#[when("I clear the environment override and run cruise config")]
fn clear_and_show(world: &mut ConfigEnvWorld) {
    world.env.clear();
    world.run_cli(&["config"]);
}

#[then("the command succeeds")]
fn succeeds(world: &mut ConfigEnvWorld) {
    assert!(world.output().status.success(), "{}", world.text());
}

#[then(regex = r#"^the command (succeeds|fails) and reports "([^"]+)"$"#)]
fn status_and_text(world: &mut ConfigEnvWorld, matches: &[String]) {
    assert_eq!(
        world.output().status.success(),
        matches[0] == "succeeds",
        "{}",
        world.text()
    );
    assert!(world.text().contains(&matches[1]), "{}", world.text());
}

#[then(regex = r#"^the output mentions "([^"]+)"$"#)]
fn mentions(world: &mut ConfigEnvWorld, matches: &[String]) {
    assert!(world.text().contains(&matches[0]), "{}", world.text());
}

#[tokio::main]
async fn main() {
    use cucumber::writer::Stats as _;
    let writer = ConfigEnvWorld::cucumber()
        .fail_on_skipped()
        .run(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/config_env.feature"
        ))
        .await;
    assert!(
        !writer.execution_has_failed(),
        "{} steps failed, {} parsing errors, {} hook errors",
        writer.failed_steps(),
        writer.parsing_errors(),
        writer.hook_errors()
    );
    assert!(
        writer.passed_steps() + writer.failed_steps() > 0,
        "no config environment scenarios executed"
    );
}
