//! Executable contracts grounded in plan.md, using real CLI, Executor and AppConfig.
//! Fake executables replace only the final SDK process boundary, never dispatch or retry.
#![expect(clippy::unwrap_used, reason = "isolated executable contract fixtures")]

#[cfg(unix)]
mod contracts {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::sync::Mutex;

    use cruise::app_config::AppConfig;
    use cruise::config::ModelSpec;
    use cruise::executor::{Executor, PromptOutcome, PromptRun};
    use cruise::model_selection::{Sdk, SdkSessionId};
    use cucumber::{World as _, given, then, when};
    use serde_json::{Value, json};

    #[derive(Default, cucumber::World)]
    pub struct SdkWorld {
        root: Option<tempfile::TempDir>,
        output: Option<Output>,
        owner: Option<Sdk>,
        outcome: Option<PromptOutcome>,
        checkpoints: Vec<SdkSessionId>,
        config_path: Option<PathBuf>,
        expected: Option<Value>,
    }

    impl std::fmt::Debug for SdkWorld {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SdkWorld")
                .field("root", &self.root)
                .finish_non_exhaustive()
        }
    }

    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    impl SdkWorld {
        fn path(&self, name: &str) -> PathBuf {
            self.root.as_ref().unwrap().path().join(name)
        }
        fn recorded(&self, name: &str) -> String {
            std::fs::read_to_string(self.path(name)).unwrap()
        }
        fn terminal(&self) -> String {
            let output = self.output.as_ref().unwrap();
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        }
        fn setup(&mut self, fail_claude: bool) {
            self.root = Some(tempfile::tempdir().unwrap());
            std::fs::create_dir_all(self.path("bin")).unwrap();
            std::fs::create_dir_all(self.path("repo")).unwrap();
            cruise::test_support::init_git_repo(&self.path("repo"));
            // No runtime starts and no credentials are loaded. A named 404 refusal
            // exercises production classification and immediate candidate switching.
            cruise::test_support::write_executable_script(
                &self.path("bin/jcode"),
                &format!(
                    "#!/bin/sh\nprintf 'jcode\\n' >> {}\nprintf '%s\\n' \"${{JCODE_MODEL-}}\" >> {}\nprintf '%s\\n' \"$@\" >> {}\nprintf 'API Error: 404 model not found\\n' >&2\nexit 73\n",
                    quote(&self.path("dispatch")),
                    quote(&self.path("jcode-models")),
                    quote(&self.path("jcode-args"))
                ),
            );
            let response = if fail_claude {
                r#"{"type":"result","subtype":"success","is_error":true,"session_id":"failed-claude-session","result":"API Error: 404 model not found"}"#
            } else {
                concat!(
                    r#"{"type":"system","subtype":"init","session_id":"actual-claude-session"}"#,
                    "\n",
                    r#"{"type":"result","subtype":"success","is_error":false,"session_id":"actual-claude-session","result":"SDK_BDD_OUTPUT"}"#
                )
            };
            cruise::test_support::write_executable_script(
                &self.path("bin/claude"),
                &format!(
                    "#!/bin/sh\nread -r frame\nprintf 'claude\\n' >> {}\nprintf '%s\\n' \"$@\" >> {}\nprintf '%s\\n' \"$frame\" >> {}\nprintf '%s\\n' '{}'\nexit {}\n",
                    quote(&self.path("dispatch")),
                    quote(&self.path("claude-args")),
                    quote(&self.path("claude-input")),
                    response,
                    i32::from(fail_claude)
                ),
            );
        }
        fn workflow(&self, yaml: &str) {
            std::fs::write(self.path("repo/workflow.yaml"), yaml).unwrap();
        }
        fn child_path(&self) -> std::ffi::OsString {
            let mut paths = vec![self.path("bin")];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            std::env::join_paths(paths).unwrap()
        }
        fn run(&mut self, purpose: &str) {
            let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
            for (name, _) in std::env::vars_os() {
                let text = name.to_string_lossy();
                if ["CRUISE_", "JCODE_", "CLAUDE", "ANTHROPIC_"]
                    .iter()
                    .any(|p| text.starts_with(p))
                {
                    command.env_remove(name);
                }
            }
            command
                .current_dir(self.path("repo"))
                .env("PATH", self.child_path())
                .env("HOME", self.path("home"))
                .env("USERPROFILE", self.path("home"))
                .env("XDG_CONFIG_HOME", self.path("config"))
                .env("XDG_DATA_HOME", self.path("data"))
                .env("XDG_STATE_HOME", self.path("state"))
                .env("JCODE_HOME", self.path("jh"))
                .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
                .env("NO_COLOR", "1")
                .env("GIT_CONFIG_COUNT", "0")
                .arg(purpose);
            if purpose == "plan" {
                command.arg("--no-interactive-planning");
            }
            self.output = Some(
                command
                    .args(["--config", "workflow.yaml", "SDK_BDD_REQUEST"])
                    .stdin(Stdio::null())
                    .output()
                    .unwrap(),
            );
        }
    }

    #[given(expr = "an isolated workflow with a mixed SDK list for {word}")]
    fn mixed(w: &mut SdkWorld, purpose: String) {
        w.setup(true);
        let field = if purpose == "plan" {
            "plan_model"
        } else {
            "model"
        };
        let ordinary = if purpose == "plan" {
            "model: unused-execution-model\n"
        } else {
            ""
        };
        w.workflow(&format!("sdk: jcode\n{ordinary}{field}:\n  - bdd-first:medium\n  - sdk: claude\n    model: bdd-middle:high\n  - bdd-last:low\nretry:\n  base_delay_ms: 1\n  max_delay_ms: 1\nsteps:\n  answer:\n    prompt: SDK_BDD_REQUEST\n"));
    }
    #[when(expr = "the real CLI runs {word} against refusing local SDK processes")]
    fn run_cli(w: &mut SdkWorld, purpose: String) {
        w.run(&purpose);
    }
    #[then("SDK subprocesses ran in order jcode claude jcode")]
    fn order(w: &mut SdkWorld) {
        assert_eq!(
            w.recorded("dispatch"),
            "jcode\nclaude\njcode\n",
            "{}",
            w.terminal()
        );
        assert!(
            !w.output.as_ref().unwrap().status.success(),
            "refusing fixtures must fail"
        );
    }
    #[then("each subprocess received its selected model and Claude received the original request")]
    fn arguments(w: &mut SdkWorld) {
        assert_eq!(w.recorded("jcode-models"), "bdd-first\nbdd-last\n");
        let args = w.recorded("claude-args");
        let args: Vec<_> = args.lines().collect();
        assert!(args.windows(2).any(|p| p == ["--model", "bdd-middle"]));
        assert!(args.windows(2).any(|p| p == ["--effort", "high"]));
        assert!(w.recorded("claude-input").contains("SDK_BDD_REQUEST"));
        assert!(
            !args.contains(&"--resume"),
            "failed attempt ID must not leak"
        );
        assert!(!w.recorded("jcode-args").contains("failed-claude-session"));
    }
    #[then("both SDK switch notices identify the source and destination")]
    fn notices(w: &mut SdkWorld) {
        let terminal = w.terminal();
        for pair in [
            ("jcode:bdd-first", "claude:bdd-middle"),
            ("claude:bdd-middle", "jcode:bdd-last"),
        ] {
            assert!(
                terminal.lines().any(|line| line.contains("Fallback:")
                    && line.contains(pair.0)
                    && line.contains(pair.1)),
                "{terminal}"
            );
        }
    }

    #[given("an isolated workflow with a single successful Claude candidate")]
    fn single_claude(w: &mut SdkWorld) {
        w.setup(false);
        w.workflow("sdk: jcode\nmodel:\n  - sdk: claude\n    model: bdd-control\nsteps:\n  answer:\n    prompt: SDK_BDD_REQUEST\n");
    }

    #[given("a mixed SDK list sharing model names with different effort suffixes")]
    fn same_name(w: &mut SdkWorld) {
        w.setup(true);
        w.workflow("sdk: jcode\nmodel:\n  - bdd-shared:medium\n  - sdk: claude\n    model: bdd-shared:high\n  - bdd-shared:low\n  - bdd-final:low\nretry:\n  base_delay_ms: 1\n  max_delay_ms: 1\nsteps:\n  answer:\n    prompt: SDK_BDD_REQUEST\n");
    }
    #[then("different SDK owners execute but the repeated jcode effort variant does not")]
    fn same_name_identity(w: &mut SdkWorld) {
        assert!(!w.output.as_ref().unwrap().status.success());
        assert_eq!(
            w.recorded("dispatch"),
            "jcode\nclaude\njcode\n",
            "{}",
            w.terminal()
        );
        assert_eq!(w.recorded("jcode-models"), "bdd-shared\nbdd-final\n");
        let args = w.recorded("claude-args");
        let args: Vec<_> = args.lines().collect();
        assert!(args.windows(2).any(|p| p == ["--model", "bdd-shared"]));
        assert!(args.windows(2).any(|p| p == ["--effort", "high"]));
        let terminal = w.terminal();
        assert!(
            terminal.lines().any(|line| line.contains("Fallback:")
                && line.contains("jcode:bdd-shared")
                && line.contains("claude:bdd-shared")),
            "{terminal}"
        );
        assert!(
            terminal.contains("404"),
            "fixture must fail through actual refusal: {terminal}"
        );
    }

    #[given(expr = "a local Claude process and a saved {word} conversation")]
    fn saved(w: &mut SdkWorld, owner: String) {
        w.setup(false);
        w.owner = Sdk::from_name(&owner);
    }
    #[when("the production Executor runs a Claude candidate")]
    async fn resume(w: &mut SdkWorld) {
        let _lock = cruise::test_support::lock_process();
        let _path = cruise::test_support::EnvGuard::set("PATH", w.child_path());
        let _home = cruise::test_support::EnvGuard::set("HOME", w.path("home"));
        let checkpoints = Mutex::new(Vec::new());
        let callback = |id: &SdkSessionId| {
            checkpoints.lock().unwrap().push(id.clone());
            Ok(())
        };
        let model: ModelSpec =
            serde_json::from_value(json!([{"sdk":"claude", "model":"bdd-resume:high"}])).unwrap();
        w.outcome = Some(
            Executor::Jcode
                .run(PromptRun {
                    prompt: "SDK_BDD_RESUME_REQUEST",
                    model: Some(&model),
                    subagent_model: None,
                    max_retries: 0,
                    env: &HashMap::new(),
                    mcp_servers: &Default::default(),
                    on_notice: None,
                    cancel_token: None,
                    working_dir: Some(&w.path("repo")),
                    stream: None,
                    tools: vec![],
                    on_session_id: Some(&callback),
                    resume: Some(SdkSessionId {
                        sdk: w.owner.unwrap(),
                        id: "saved-owner-session".into(),
                    }),
                    computer_use: false,
                    permission: Default::default(),
                })
                .await
                .unwrap(),
        );
        w.checkpoints = checkpoints.into_inner().unwrap();
    }
    #[then(expr = "the Claude resume argument is {word}")]
    fn resume_arg(w: &mut SdkWorld, behavior: String) {
        let args = w.recorded("claude-args");
        let args: Vec<_> = args.lines().collect();
        assert_eq!(
            args.windows(2)
                .any(|p| p == ["--resume", "saved-owner-session"]),
            behavior == "present"
        );
        if behavior == "absent" {
            assert!(!args.contains(&"saved-owner-session"));
        }
        assert_eq!(w.recorded("dispatch"), "claude\n");
    }
    #[then("the outcome and checkpoint callback identify the actual Claude conversation")]
    fn checkpoint(w: &mut SdkWorld) {
        let expected = SdkSessionId {
            sdk: Sdk::Claude,
            id: "actual-claude-session".into(),
        };
        assert_eq!(
            w.outcome.as_ref().unwrap().session_id.as_ref(),
            Some(&expected)
        );
        assert!(!w.checkpoints.is_empty());
        assert!(w.checkpoints.iter().all(|id| id == &expected));
        assert!(
            w.recorded("claude-input")
                .contains("SDK_BDD_RESUME_REQUEST")
        );
    }

    #[given(expr = "an app config in {word} with mixed execution and planning candidates")]
    fn app(w: &mut SdkWorld, format: String) {
        w.root = Some(tempfile::tempdir().unwrap());
        let value = json!({"model":["provider/first:medium", {"sdk":"claude","model":"middle:high"}, "provider/last:low"],
            "planModel":[{"sdk":"claude","model":"planner"}, "provider/plan-tail"], "runAllParallelism":2});
        let path = w.path(&format!("config.{format}"));
        std::fs::write(
            &path,
            if format == "json" {
                serde_json::to_string(&value).unwrap()
            } else {
                serde_yaml::to_string(&value).unwrap()
            },
        )
        .unwrap();
        w.config_path = Some(path);
        w.expected = Some(value);
    }
    #[when("production app config loading and atomic saving update parallelism")]
    fn save(w: &mut SdkWorld) {
        let path = w.config_path.as_ref().unwrap();
        let mut config = AppConfig::load_from(path).unwrap();
        config.run_all_parallelism = 3;
        config.save_to(path).unwrap();
    }
    #[then("reloading preserves candidate order SDK and original entry forms")]
    fn reload(w: &mut SdkWorld) {
        let loaded = AppConfig::load_from(w.config_path.as_ref().unwrap()).unwrap();
        let actual = serde_json::to_value(loaded).unwrap();
        let expected = w.expected.as_ref().unwrap();
        assert_eq!(actual["model"], expected["model"]);
        assert_eq!(actual["plan_model"], expected["planModel"]);
        assert_eq!(actual["runAllParallelism"], 3);
    }

    #[given(expr = "a command workflow with an SDK object in {word} at {word}")]
    fn conflict(w: &mut SdkWorld, field: String, position: String) {
        w.setup(false);
        let object = json!({"sdk":"claude", "model":"bdd-command"});
        let candidates = if position == "primary" {
            json!([object, "provider/tail"])
        } else {
            json!(["provider/main", object])
        };
        let mut value = json!({"command":["sh", "-c", format!("touch {}", quote(&w.path("command-started")))], "steps":{"answer":{"prompt":"SDK_BDD_REQUEST"}}});
        value[&field] = candidates;
        w.workflow(&serde_yaml::to_string(&value).unwrap());
    }
    #[when("the real CLI validates and executes the conflicting workflow")]
    fn reject(w: &mut SdkWorld) {
        w.run("exec");
    }
    #[then("the command conflict is reported without launching any subprocess")]
    fn rejected(w: &mut SdkWorld) {
        assert!(!w.output.as_ref().unwrap().status.success());
        let terminal = w.terminal();
        assert!(
            terminal.contains("command") && (terminal.contains("SDK") || terminal.contains("sdk")),
            "{terminal}"
        );
        assert!(!w.path("command-started").exists());
        assert!(!w.path("dispatch").exists());
    }

    // cucumber 0.23.0 has fail_on_skipped but no built-in fail_on_empty.
    // Check the native writer's executed step statistics, including CLI filters.
    fn fail_on_empty(writer: &impl cucumber::writer::Stats<SdkWorld>) {
        assert!(
            writer.passed_steps() + writer.failed_steps() > 0,
            "no SDK model scenarios executed"
        );
    }
    pub async fn main() {
        use cucumber::writer::Stats as _;
        let writer = SdkWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/sdk_model.feature"
            ))
            .await;
        assert!(
            !writer.execution_has_failed(),
            "{} step failed, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
        fail_on_empty(&writer);
    }
}

#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::main().await;
}

#[cfg(not(unix))]
fn main() {
    panic!("sdk_model_bdd requires Unix subprocess fixtures");
}
