//! Real production preflight and execution with a deterministic backend double.
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]

#[cfg(unix)]
mod contracts {
    use std::{fs, path::Path, process::Command, time::Duration};

    use cruise::{
        artifacts,
        cancellation::CancellationToken,
        config::{WorkflowConfig, validate_config},
        engine::{ExecutionContext, ExecutionResult, execute_steps_with_graph},
        error::{CruiseError, Result},
        file_tracker::FileTracker,
        flags::{FlagValues, preflight_skips},
        graph::{build_graph, validation::validate_workflow},
        option_handler::OptionHandler,
        variable::VariableStore,
        workflow::compile,
    };
    use cucumber::{World as _, given, then, when};

    const ADVICE: &str = "advisor/implementation.md";
    const STEPS: &str = r"flags:
  advisor:
    description: Run the advisor pass
steps:
  advise:
    prompt: ADVISE
    advisor: true
    flag: advisor
    output_file: advisor/implementation.md
    if: { fail: later }
  apply-advice:
    prompt: 'APPLY {file:advisor/implementation.md}'
    flag: advisor
  later:
    command: printf x >> ../later.visits
";

    struct NoOptions;
    impl OptionHandler for NoOptions {
        fn select_option(
            &self,
            _: &[cruise::step::OptionChoice],
            _: Option<&str>,
        ) -> Result<cruise::step::option::OptionResult> {
            panic!("advisor must not open an option picker")
        }
    }

    #[derive(Debug, Default, cucumber::World)]
    struct World {
        yaml: String,
        dir: Option<tempfile::TempDir>,
        interrupt: bool,
        preflight: Option<Result<()>>,
        runtime: Option<Result<ExecutionResult>>,
    }

    impl World {
        fn root(&self) -> &Path {
            self.dir.as_ref().unwrap().path()
        }
    }

    fn flags(setting: &str) -> FlagValues {
        match setting {
            "enabled" | "user-skip" | "skip-advisor-only" | "skip-apply-only" => {
                [("advisor".to_owned(), true)].into_iter().collect()
            }
            "flag-off" => FlagValues::new(),
            _ => panic!("unknown advisor setting: {setting}"),
        }
    }

    #[given(expr = "an advisor workflow on the {string} backend")]
    fn workflow(w: &mut World, backend: String) {
        let backend = match backend.as_str() {
            "command" => "command: [echo]\n",
            "jcode" => "sdk: jcode\n",
            _ => panic!("unknown backend: {backend}"),
        };
        w.yaml = format!("{backend}{STEPS}");
    }

    #[given("a jcode workflow without an advisor")]
    fn plain_workflow(w: &mut World) {
        w.yaml = "sdk: jcode\nsteps:\n  implement: { prompt: implement }\n".into();
    }

    #[when(expr = "production preflight runs with advisor {string}")]
    fn preflight(w: &mut World, setting: String) {
        let config = WorkflowConfig::from_yaml(&w.yaml).unwrap();
        validate_config(&config).unwrap(); // Acceptability must not depend on static flags.
        let compiled = compile(config).unwrap();
        let graph = build_graph(&compiled, 0).unwrap();
        let user_skips = if setting == "user-skip" {
            vec!["advise".into()]
        } else {
            vec![]
        };
        let skipped = preflight_skips(&compiled, &flags(&setting), &user_skips);
        w.preflight = Some(validate_workflow(&compiled, &graph, &graph.start, &skipped));
    }

    #[then(expr = "preflight reports {string}")]
    fn expect_preflight(w: &mut World, expected: String) {
        let observed = w.preflight.as_ref().unwrap();
        match expected.as_str() {
            "accepted" => assert!(observed.is_ok(), "{observed:?}"),
            "rejected" => assert!(
                matches!(observed, Err(CruiseError::InvalidStepConfig(message))
                    if message.contains("advise") && message.contains("advisor") && message.to_lowercase().contains("jcode")),
                "advisor preflight rejection mismatch: {observed:?}"
            ),
            _ => panic!("unknown preflight expectation: {expected}"),
        }
    }

    #[given(expr = "a jcode workflow with advisor placement {string}")]
    fn placement_fixture(w: &mut World, placement: String) {
        w.yaml = match placement.as_str() {
            "top-prompt" => "steps:\n  advise: { prompt: inspect, advisor: true, output_file: advice }\n",
            "after-pr" => "steps:\n  main: { prompt: implement }\nafter-pr:\n  advise: { prompt: inspect, advisor: true, output_file: advice }\n",
            "command-step" => "steps:\n  advise: { command: 'true', advisor: true, output_file: advice }\n",
            "missing-output" => "steps:\n  advise: { prompt: inspect, advisor: true }\n",
            "allow-commit" => "steps:\n  advise: { prompt: inspect, advisor: true, output_file: advice, allow_commit: true }\n",
            "full-access" => "steps:\n  advise: { prompt: inspect, advisor: true, output_file: advice, permission: full }\n",
            _ => panic!("unknown placement: {placement}"),
        }.to_owned();
    }

    #[when("production static validation runs")]
    fn static_validation(w: &mut World) {
        w.preflight = Some(validate_config(
            &WorkflowConfig::from_yaml(&w.yaml).unwrap(),
        ));
    }

    #[then(expr = "static advisor validation reports {string}")]
    fn expect_static_validation(w: &mut World, expected: String) {
        let observed = w.preflight.as_ref().unwrap();
        match expected.as_str() {
            "accepted" => assert!(observed.is_ok(), "{observed:?}"),
            "rejected" => assert!(
                matches!(observed, Err(error) if error.to_string().contains("advise") && error.to_string().contains("advisor")),
                "advisor static rejection mismatch: {observed:?}"
            ),
            _ => panic!("unknown validation expectation: {expected}"),
        }
    }

    #[given(expr = "an advisor pass with stale advice and a {string} backend double")]
    fn runtime_fixture(w: &mut World, effect: String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = root.join("repo");
        fs::create_dir(&repo).unwrap();
        fs::write(repo.join("README.md"), "initial\n").unwrap();
        for args in [
            &["init"][..],
            &["config", "user.email", "advisor@example.com"],
            &["config", "user.name", "Advisor Fixture"],
            &["add", "README.md"],
            &["commit", "-m", "initial"],
        ] {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(output.status.success(), "fixture git {args:?}: {output:?}");
        }
        fs::create_dir(root.join("artifacts")).unwrap();
        artifacts::write(&root.join("artifacts"), ADVICE, "STALE ADVICE").unwrap();
        let task = match effect.as_str() {
            "success" => "printf 'FRESH ADVICE'",
            "no-findings" => "printf 'NO FINDINGS'",
            "fails" => "exit 7",
            "mutates" => "touch modified-by-advisor; exit 7",
            "mutates-then-sleeps" => "touch modified-by-advisor; exec sleep 30",
            "times-out" => "exec sleep 30",
            // The post-run WalkDir cannot read its missing root, even under root privileges.
            "loses-root" => "mv ../repo ../moved-repo; exit 7",
            "interrupted" => "exec sleep 30",
            "forbidden" => "exit 91",
            _ => panic!("unknown backend effect: {effect}"),
        };
        write_backend(root, task);
        let steps = if matches!(effect.as_str(), "mutates-then-sleeps" | "times-out") {
            STEPS.replace("    prompt: ADVISE", "    timeout: 1s\n    prompt: ADVISE")
        } else {
            STEPS.to_owned()
        };
        w.yaml = format!("command: [sh, ../backend.sh, '{{model}}']\npermission: full\n{steps}");
        w.interrupt = effect == "interrupted";
        w.dir = Some(dir);
    }

    #[given(expr = "the pass models are step {string}, advisor {string}, and plan {string}")]
    fn models(w: &mut World, step_model: String, advisor: String, plan: String) {
        w.yaml.push_str("model: openai/normal\n");
        for (key, value) in [
            ("advisor_model", advisor.as_str()),
            ("plan_model", plan.as_str()),
        ] {
            if value != "absent" {
                w.yaml.push_str(&format!("{key}: {value}\n"));
            }
        }
        if step_model != "absent" {
            w.yaml = w.yaml.replace(
                "    prompt: ADVISE",
                &format!("    model: {step_model}\n    prompt: ADVISE"),
            );
        }
    }

    fn write_backend(root: &Path, task: &str) {
        // Markers are outside the guarded worktree. The entire prompt is inspected
        // before producing advice or recording exactly what the apply step consumed.
        fs::write(
            root.join("backend.sh"),
            format!(
                r#"prompt=$(cat)
case "$prompt" in
  ADVISE*)
    if [ -e ../artifacts/{ADVICE} ]; then printf x >> ../stale-at-dispatch; fi
    printf x >> ../advisor.visits
    printf '%s\n' "${{1-}}" >> ../advisor.model
    {task} ;;
  APPLY\ *)
    printf x >> ../apply.visits
    printf '%s\n' "$prompt" >> ../apply.prompt
    printf '%s\n' "${{1-}}" >> ../apply.model ;;
  *) exit 92 ;;
esac
"#
            ),
        )
        .unwrap();
    }

    #[when(expr = "the production execution engine runs with advisor {string}")]
    async fn execute(w: &mut World, setting: String) {
        // Even the first sequential edge is budgeted (workflow-control.md).
        const EDGE_BUDGET: usize = 1;
        let config = WorkflowConfig::from_yaml(&w.yaml).unwrap();
        let compiled = compile(config).unwrap();
        let mut graph = build_graph(&compiled, EDGE_BUDGET).unwrap();
        let repo = w.root().join("repo");
        let cancel = CancellationToken::new();
        let marker = w.root().join("advisor.visits");
        let interrupter = w.interrupt.then(|| {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                while !marker.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                cancel.cancel();
            })
        });
        let skipped_steps = match setting.as_str() {
            "user-skip" => vec!["advise".into(), "apply-advice".into()],
            "skip-advisor-only" => vec!["advise".into()],
            "skip-apply-only" => vec!["apply-advice".into()],
            _ => vec![],
        };
        let ctx = ExecutionContext {
            usage_session_id: None,
            compiled: &compiled,
            max_retries: EDGE_BUDGET,
            rate_limit_retries: 0,
            on_step_start: &|_| Ok(()),
            cancel_token: Some(&cancel),
            option_handler: &NoOptions,
            ask_handler: None,
            config_reloader: None,
            working_dir: Some(&repo),
            skipped_steps: &skipped_steps,
            flags: &flags(&setting),
            on_step_log: None,
            instructions: None,
        };
        let mut vars = VariableStore::new(String::new());
        vars.set_artifacts_root(w.root().join("artifacts"));
        // Deliberately enter below preflight, so shell doubles can fault-inject
        // into the real engine and readonly guard without provider credentials.
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            execute_steps_with_graph(
                &ctx,
                &mut vars,
                &mut FileTracker::with_root(repo.clone()),
                &mut graph,
                &|_, _| Ok(()),
            ),
        )
        .await;
        if let Some(interrupter) = interrupter {
            interrupter.abort();
        }
        w.runtime = Some(result.unwrap());
    }

    #[then(expr = "the engine reports {string}")]
    fn expect_runtime(w: &mut World, expected: String) {
        let observed = w.runtime.as_ref().unwrap();
        let correct = match expected.as_str() {
            "continued" => observed.is_ok(),
            "changed" => matches!(observed, Err(CruiseError::ReadOnlyWorkspaceChanged)),
            "unverifiable" => matches!(observed, Err(CruiseError::ReadOnlyVerificationFailed(_))),
            "interrupted" => matches!(observed, Err(CruiseError::Interrupted)),
            "missing-advice" => {
                matches!(observed, Err(error) if error.to_string().contains(ADVICE) && error.to_string().contains("failed to read artifact"))
            }
            _ => panic!("unknown runtime expectation: {expected}"),
        };
        assert!(
            correct,
            "advisor runtime outcome mismatch: expected {expected}, got {observed:?}"
        );
    }

    fn read_marker(root: &Path, name: &str) -> String {
        match fs::read_to_string(root.join(name)) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => panic!("read marker {name}: {error}"),
        }
    }

    #[then(expr = "the backend received advisor model {string} and apply model {string}")]
    fn observed_models(w: &mut World, expected_advisor: String, expected_apply: String) {
        assert_eq!(
            read_marker(w.root(), "advisor.model").trim(),
            expected_advisor
        );
        assert_eq!(read_marker(w.root(), "apply.model").trim(), expected_apply);
    }

    #[then(expr = "the advisor ran {string} times")]
    fn advisor_visits(w: &mut World, count: String) {
        assert_eq!(
            read_marker(w.root(), "advisor.visits"),
            "x".repeat(count.parse().unwrap())
        );
        assert!(
            read_marker(w.root(), "stale-at-dispatch").is_empty(),
            "stale advice was visible when the advisor started"
        );
    }

    #[then(expr = "the apply step ran {string} times with {string} advice")]
    fn apply_visits(w: &mut World, count: String, advice: String) {
        let visits = read_marker(w.root(), "apply.visits");
        assert_eq!(visits, "x".repeat(count.parse().unwrap()));
        let root = w.root().join("artifacts");
        match advice.as_str() {
            "absent" => assert!(!root.join(ADVICE).exists(), "stale advice survived"),
            "fresh" | "none" => {
                let expected = if advice == "fresh" {
                    "FRESH ADVICE"
                } else {
                    "NO FINDINGS"
                };
                assert_eq!(artifacts::read(&root, ADVICE).unwrap().trim(), expected);
                // The engine appends the commit-guard notice to ordinary prompts.
                let prompt = read_marker(w.root(), "apply.prompt");
                let expected_prompt = (!visits.is_empty()).then(|| format!("APPLY {expected}"));
                assert_eq!(prompt.lines().next(), expected_prompt.as_deref());
            }
            _ => panic!("unknown advice expectation: {advice}"),
        }
    }

    #[then(expr = "the later step ran {string} times")]
    fn later_visits(w: &mut World, count: String) {
        assert_eq!(
            read_marker(w.root(), "later.visits"),
            "x".repeat(count.parse().unwrap())
        );
    }

    pub async fn run() {
        use cucumber::writer::Stats as _;
        let writer = World::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/advisor.feature"
            ))
            .await;
        assert!(
            !writer.execution_has_failed(),
            "{} step failed, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
        assert!(writer.passed_steps() > 0, "no advisor scenarios executed");
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    #[cfg(unix)]
    contracts::run().await;
    #[cfg(not(unix))]
    panic!("advisor backend-double contracts require Unix");
}
