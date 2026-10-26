//! Task-grounded contracts: real config validation, compilation and CLI execution.
//! Only the external agent process is replaced, using the SDK stream protocol.
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]
#[cfg(unix)]
mod contracts {
    use cruise::config::{WorkflowConfig, validate_config};
    use cucumber::{World as _, given, then, when};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

    #[derive(Default, cucumber::World)]
    pub struct World {
        root: Option<tempfile::TempDir>,
        yaml: String,
        output: Option<Output>,
        error: Option<String>,
        compiled: Option<cruise::workflow::CompiledWorkflow>,
    }
    impl std::fmt::Debug for World {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SubagentWorld")
                .field("yaml", &self.yaml)
                .finish_non_exhaustive()
        }
    }
    fn quote(p: &Path) -> String {
        format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
    }
    impl World {
        fn path(&self, p: &str) -> PathBuf {
            self.root.as_ref().unwrap().path().join(p)
        }
        fn setup(&mut self) {
            self.root = Some(tempfile::tempdir().unwrap());
            std::fs::create_dir_all(self.path("bin")).unwrap();
            std::fs::create_dir_all(self.path("repo")).unwrap();
            cruise::test_support::init_git_repo(&self.path("repo"));
            self.claude(false);
            cruise::test_support::write_executable_script(
                &self.path("bin/jcode"),
                &format!(
                    "#!/bin/sh\nprintf 'jcode\\n' >> {}\nprintf 'API Error: 404 model not found\\n' >&2\nexit 73\n",
                    quote(&self.path("dispatch"))
                ),
            );
        }
        fn claude(&self, fail: bool) {
            let response = if fail {
                r#"{"type":"result","subtype":"success","is_error":true,"session_id":"refused","result":"API Error: 404 model not found"}"#
            } else {
                concat!(
                    r#"{"type":"system","subtype":"init","session_id":"subagent-contract"}"#,
                    "\n",
                    r##"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"# Contract plan\n\n- SUBAGENT_OUTPUT"}]},"session_id":"subagent-contract"}"##,
                    "\n",
                    r##"{"type":"result","subtype":"success","is_error":false,"session_id":"subagent-contract","result":"# Contract plan\n\n- SUBAGENT_OUTPUT"}"##
                )
            };
            self.install_response(response, fail);
        }
        fn install_response(&self, response: &str, fail: bool) {
            cruise::test_support::write_executable_script(
                &self.path("bin/claude"),
                &format!(
                    "#!/bin/sh\nread -r frame\nprintf 'claude\\n' >> {}\nprintf '%s\\t%s\\t%s\\n' \"${{CLAUDE_CODE_SUBAGENT_MODEL-}}\" \"${{CLAUDE_CODE_SUBAGENT_MODEL_FORCE-}}\" \"$frame\" >> {}\nprintf '%s\\n' '{}'\nexit {}\n",
                    quote(&self.path("dispatch")),
                    quote(&self.path("requests")),
                    response,
                    i32::from(fail)
                ),
            );
        }
        fn terminal(&self) -> String {
            let o = self.output.as_ref().unwrap();
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        }
        fn records(&self) -> Vec<(String, String, String)> {
            std::fs::read_to_string(self.path("requests"))
                .unwrap()
                .lines()
                .map(|line| {
                    let p: Vec<_> = line.splitn(3, '\t').collect();
                    assert_eq!(p.len(), 3);
                    (p[0].into(), p[1].into(), p[2].into())
                })
                .collect()
        }
    }
    #[given("this subagent workflow")]
    fn workflow(w: &mut World, step: &cucumber::gherkin::Step) {
        w.setup();
        w.yaml = step.docstring.clone().unwrap();
        std::fs::write(w.path("repo/workflow.yaml"), &w.yaml).unwrap();
    }
    #[given("Claude refuses the selected primary model")]
    fn refuse(w: &mut World) {
        w.claude(true);
    }
    #[given("Claude returns a valid generated workflow")]
    fn generator(w: &mut World) {
        w.install_response(concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"command: [cat]\nsteps:\n  build:\n    command: echo generated\n"}]},"session_id":"generator"}"#, "\n",
            r#"{"type":"result","subtype":"success","is_error":false,"session_id":"generator","result":"command: [cat]\nsteps:\n  build:\n    command: echo generated\n"}"#
        ), false);
    }
    #[when(expr = "the production CLI runs {word}")]
    fn cli(w: &mut World, purpose: String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if ["CRUISE_", "JCODE_", "CLAUDE", "ANTHROPIC_"]
                .iter()
                .any(|p| name.to_string_lossy().starts_with(p))
            {
                c.env_remove(name);
            }
        }
        let mut paths = vec![w.path("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        c.current_dir(w.path("repo"))
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("HOME", w.path("home"))
            .env("USERPROFILE", w.path("home"))
            .env("XDG_CONFIG_HOME", w.path("config"))
            .env("XDG_DATA_HOME", w.path("data"))
            .env("XDG_STATE_HOME", w.path("state"))
            .env("JCODE_HOME", w.path("jh"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env("NO_COLOR", "1");
        if purpose == "generate" {
            c.args(["workflow", "generate", "--name", "generated"]);
        } else {
            c.arg(&purpose);
        }
        if purpose == "plan" {
            c.arg("--no-interactive-planning");
        }
        w.output = Some(
            c.args(["--config", "workflow.yaml", "SUBAGENT_REQUEST"])
                .stdin(Stdio::null())
                .output()
                .unwrap(),
        );
    }
    #[then("the CLI succeeds")]
    fn succeeds(w: &mut World) {
        assert!(
            w.output.as_ref().unwrap().status.success(),
            "{}",
            w.terminal()
        );
    }
    #[then(expr = "task requests use model {word} and force {word}")]
    fn selection(w: &mut World, model: String, force: String) {
        let records = w.records();
        let mut tasks = records
            .iter()
            .filter(|(_, _, input)| !input.contains("generate_title"))
            .peekable();
        assert!(tasks.peek().is_some(), "no task reached the SDK");
        for (m, f, _) in tasks {
            assert_eq!(m, if model == "unset" { "" } else { &model });
            assert_eq!(f, if force == "unset" { "" } else { &force });
        }
    }
    #[then("a validated generated workflow is written")]
    fn generated(w: &mut World) {
        let yaml = std::fs::read_to_string(w.path("repo/.cruise/generated.yaml")).unwrap();
        let config: WorkflowConfig = serde_yaml::from_str(&yaml).unwrap();
        validate_config(&config).unwrap();
        assert_eq!(
            serde_json::to_value(&config.steps["build"].command).unwrap(),
            "echo generated"
        );
    }
    #[then("title requests exclude the subagent override")]
    fn titles(w: &mut World) {
        let records = w.records();
        let mut titles = records
            .iter()
            .filter(|(_, _, input)| input.contains("generate_title"))
            .peekable();
        assert!(titles.peek().is_some(), "title path was not exercised");
        for (m, f, _) in titles {
            assert_eq!(m, "");
            assert_eq!(f, "");
        }
    }
    #[then(expr = "SDK dispatch order is {word}")]
    fn dispatch(w: &mut World, expected: String) {
        assert_eq!(
            std::fs::read_to_string(w.path("dispatch"))
                .unwrap()
                .lines()
                .collect::<Vec<_>>()
                .join(","),
            expected,
            "{}",
            w.terminal()
        );
    }
    #[when("production validation and compilation run")]
    fn compile(w: &mut World) {
        let result = serde_yaml::from_str::<WorkflowConfig>(&w.yaml)
            .map_err(|e| e.to_string())
            .and_then(|c| {
                validate_config(&c).map_err(|e| e.to_string())?;
                cruise::workflow::compile(c).map_err(|e| e.to_string())
            });
        match result {
            Ok(c) => w.compiled = Some(c),
            Err(e) => w.error = Some(e),
        }
    }
    #[then("subagent validation rejects it before SDK dispatch")]
    fn rejected(w: &mut World) {
        let e = w
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("invalid configuration was accepted"));
        assert!(
            e.contains("subagent_model") || e.contains("untagged enum ModelEntry"),
            "wrong rejection: {e}"
        );
        assert!(!w.path("dispatch").exists());
        // Negative control: removing only the offending field makes the same
        // workflow valid, proving rejection isn't an unrelated fixture error.
        fn strip(v: &mut serde_yaml::Value) {
            match v {
                serde_yaml::Value::Mapping(m) => {
                    m.remove(serde_yaml::Value::String("subagent_model".into()));
                    for v in m.values_mut() {
                        strip(v);
                    }
                }
                serde_yaml::Value::Sequence(s) => {
                    for v in s {
                        strip(v);
                    }
                }
                _ => {}
            }
        }
        let mut v: serde_yaml::Value = serde_yaml::from_str(&w.yaml).unwrap();
        strip(&mut v);
        let c: WorkflowConfig = serde_yaml::from_value(v).unwrap();
        validate_config(&c).unwrap();
        cruise::workflow::compile(c).unwrap();
    }
    #[when("the Executor directly runs the command backend")]
    async fn executor_command(w: &mut World) {
        use cruise::executor::{Executor, PromptRun};
        let c: WorkflowConfig = serde_yaml::from_str(&w.yaml).unwrap();
        let marker = w.path("command-started");
        let executor = Executor::Command {
            command: vec![
                "sh".into(),
                "-c".into(),
                format!("touch {}", quote(&marker)),
            ],
        };
        let result = executor
            .run(PromptRun {
                prompt: "SUBAGENT_COMMAND_DEFENSE",
                model: None,
                subagent_model: c.subagent_model.as_ref(),
                max_retries: 0,
                env: &Default::default(),
                mcp_servers: &Default::default(),
                on_notice: None,
                cancel_token: None,
                working_dir: Some(&w.path("repo")),
                stream: None,
                tools: vec![],
                on_session_id: None,
                resume: None,
                computer_use: false,
                permission: Default::default(),
                tool_policy: Default::default(),
            })
            .await;
        w.error = Some(
            result
                .err()
                .unwrap_or_else(|| panic!("Executor accepted command subagent_model"))
                .to_string(),
        );
        assert!(
            !marker.exists(),
            "Executor spawned before rejecting subagent_model"
        );
    }
    #[then("compiled group and after-pr prompts preserve explicit selections")]
    fn compiled(w: &mut World) {
        let c = w.compiled.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&c.subagent_model).unwrap(), "haiku");
        assert_eq!(
            serde_json::to_value(&c.steps["check/inner"].subagent_model).unwrap(),
            "opus"
        );
        let after = c.to_after_pr_compiled();
        assert_eq!(
            serde_json::to_value(&after.subagent_model).unwrap(),
            "haiku"
        );
        assert_eq!(
            serde_json::to_value(&after.steps["review"].subagent_model).unwrap(),
            "sonnet"
        );
    }
    pub async fn main() {
        use cucumber::writer::Stats as _;
        let writer = World::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/subagent_model.feature"
            ))
            .await;
        assert!(
            !writer.execution_has_failed(),
            "{} step failed, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
        assert!(
            writer.passed_steps() + writer.failed_steps() > 0,
            "no subagent model scenarios executed"
        );
    }
}
#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::main().await;
}
#[cfg(not(unix))]
fn main() {
    panic!("subagent_model_bdd requires Unix subprocess fixtures");
}
