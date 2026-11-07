//! End-to-end Gherkin contracts for the production formal verification boundary.
#![expect(clippy::unwrap_used, reason = "isolated test fixtures")]

#[cfg(unix)]
mod contracts {
    use cruise::{
        application::{ApplicationEvent, ApplicationEventSink, CruiseApplication, NewSessionRequest},
        cancellation::CancellationToken,
        error::CruiseError,
        formal_verify::{Verdict, VerifyReport, extract_blocks, verify_blocks},
        session::{SessionManager, SessionPhase, WorkspaceMode},
    };
    use cucumber::{World as _, given, then, when};
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, sync::{Arc, Mutex}, time::Duration};

    #[derive(Debug, Default, cucumber::World)]
    pub struct World {
        dir: Option<tempfile::TempDir>,
        source: String,
        report: Option<VerifyReport>,
        interrupted: bool,
        application_output: String,
        application_preserved: bool,
    }

    impl World {
        fn root(&self) -> &Path {
            self.dir.as_ref().unwrap().path()
        }
        fn setup(&mut self) {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            fs::create_dir(root.join("bin")).unwrap();
            script(
                &root.join("bin/quint"),
                r##"#!/bin/sh
 echo "$1" >> "$FIXTURE_ROOT/calls"
 case "$1" in
 --version) echo '0.33.0' ;;
 parse) printf '%s' '{"modules":[{"name":"Main","declarations":[{"kind":"def","qualifier":"action","name":"init"},{"kind":"def","qualifier":"action","name":"step"},{"kind":"def","qualifier":"val","name":"invSafe"}]}]}' > "$3" ;;
 typecheck) if [ -f "$FIXTURE_ROOT/type_error" ]; then echo 'type error' >&2; exit 1; fi ;;
 run) if [ -f "$FIXTURE_ROOT/block" ]; then (sleep 2; touch "$FIXTURE_ROOT/child_survived") & echo ready > "$FIXTURE_ROOT/started"; wait; fi ;;
 esac
"##,
            );
            script(
                &root.join("bin/java"),
                r##"#!/bin/sh
 if [ "$1" = '-version' ]; then echo 'openjdk version "17.0.1"' >&2; exit 0; fi
 for arg in "$@"; do
   if [ "$arg" = commands ]; then echo '0 . Check Safety for 3'; exit 0; fi
 done
 while [ "$#" -gt 0 ]; do
   if [ "$1" = '--output' ]; then shift; out="$1"; fi
   shift
 done
 mkdir -p "$out"
 printf '%s' '{"commands":{"Safety":{"type":"check","solution":[{"instances":[{}]}]}}}' > "$out/receipt.json"
 exit 0
"##,
            );
            fs::write(root.join("dummy.jar"), "fixture").unwrap();
            // This integration binary has one test, and Cucumber runs scenarios serially.
            unsafe {
                std::env::set_var(
                    "PATH",
                    format!(
                        "{}:{}",
                        root.join("bin").display(),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                );
                std::env::set_var("XDG_DATA_HOME", root.join("data"));
                std::env::set_var("CRUISE_ALLOY_JAR", root.join("dummy.jar"));
                std::env::set_var("FIXTURE_ROOT", root);
            }
            self.dir = Some(dir);
        }
        fn run(&mut self) {
            self.report = Some(
                verify_blocks(
                    extract_blocks(&self.source),
                    self.root(),
                    Duration::from_secs(10),
                    None,
                )
                .unwrap(),
            );
        }
    }
    fn script(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[given("a saved plan and a response without formal specifications")]
    fn no_specifications(w: &mut World) {
        w.setup();
        fs::write(w.root().join("plan.md"), "# Saved plan\nOriginal task\n").unwrap();
        w.source = "No model was generated.\n```rust\nfn main() {}\n```\n".into();
    }
    #[when("the generated response is verified")]
    fn verify_response(w: &mut World) {
        w.run();
    }
    #[then("the report explains that nothing was verified")]
    fn nothing_verified(w: &mut World) {
        let report = w.report.as_ref().unwrap();
        assert!(report.blocks.is_empty());
        let markdown = fs::read_to_string(report.artifact.join("report.md")).unwrap();
        assert!(markdown.contains("nothing was verified"));
        assert!(!w.root().join("calls").exists());
    }
    #[then("the saved plan is unchanged")]
    fn saved_plan_unchanged(w: &mut World) {
        assert_eq!(
            fs::read_to_string(w.root().join("plan.md")).unwrap(),
            "# Saved plan\nOriginal task\n"
        );
    }
    #[given("a Quint specification with a verification target")]
    fn quint(w: &mut World) {
        w.setup();
        w.source = "```quint\nmodule Main { action init = true action step = true val invSafe = true }\n```\n".into();
    }
    #[when("parse, typecheck, run and verify pass")]
    fn pass(w: &mut World) {
        w.run();
    }
    #[when("typecheck returns an error")]
    fn type_error(w: &mut World) {
        fs::write(w.root().join("type_error"), "").unwrap();
        w.run();
    }
    #[then("run and verify are not invoked")]
    fn short_circuit(w: &mut World) {
        let calls = fs::read_to_string(w.root().join("calls")).unwrap();
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            ["--version", "parse", "typecheck"]
        );
    }
    #[then("the verification report is passed")]
    fn passed(w: &mut World) {
        let report = w.report.as_ref().unwrap();
        assert_eq!(report.verdict(), Verdict::Passed);
        assert_eq!(
            report.blocks[0]
                .stages
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["parse", "typecheck", "run", "verify-invariants"]
        );
        assert_eq!(
            fs::read_to_string(w.root().join("calls"))
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["--version", "parse", "typecheck", "run", "verify"]
        );
    }
    #[then("the verification report is error")]
    fn error(w: &mut World) {
        assert_eq!(w.report.as_ref().unwrap().verdict(), Verdict::Error);
    }
    #[given("an Alloy check with a satisfying counterexample receipt")]
    fn alloy(w: &mut World) {
        w.setup();
        w.source = "```alloy\nassert Safety { some univ }\ncheck Safety for 3\n```\n".into();
    }
    #[when("the Alloy command is verified")]
    fn verify_alloy(w: &mut World) {
        w.run();
    }
    #[then("the verification report is failed")]
    fn failed(w: &mut World) {
        assert_eq!(w.report.as_ref().unwrap().verdict(), Verdict::Failed);
    }
    #[given("a running formal verification process")]
    fn running(w: &mut World) {
        quint(w);
        fs::write(w.root().join("block"), "").unwrap();
    }
    #[when("verification is cancelled")]
    fn cancel(w: &mut World) {
        let token = CancellationToken::new();
        let other = token.clone();
        let marker = w.root().join("started");
        let thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !marker.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(marker.exists(), "verification process did not start");
            other.cancel();
        });
        let result = verify_blocks(
            extract_blocks(&w.source),
            w.root(),
            Duration::from_secs(10),
            Some(&token),
        );
        thread.join().unwrap();
        w.interrupted = matches!(result, Err(CruiseError::Interrupted));
    }
    #[then("verification is interrupted and its process group is stopped")]
    fn interrupted(w: &mut World) {
        assert!(w.interrupted);
        std::thread::sleep(Duration::from_millis(2300));
        assert!(
            !w.root().join("child_survived").exists(),
            "child process survived cancellation"
        );
    }

    #[given("a saved application plan and a backend response without specifications")]
    fn application_no_specs(w: &mut World) {
        no_specifications(w);
    }

    #[given("a saved application plan and a backend response with a Quint specification")]
    fn application_quint(w: &mut World) {
        quint(w);
    }

    #[when("the application verifies the saved plan")]
    async fn application_verify(w: &mut World) {
        let root = w.root();
        let backend = root.join("backend.sh");
        fs::write(root.join("response"), &w.source).unwrap();
        script(&backend, "#!/bin/sh\necho call >> \"$FIXTURE_ROOT/backend-calls\"\ncat \"$FIXTURE_ROOT/response\"\n");
        let manager = SessionManager::new(root.join("sessions"));
        let app = CruiseApplication::new(manager.clone());
        let mut state = app.create_session(NewSessionRequest {
            input: "verify test".into(),
            base_dir: root.to_path_buf(),
            config_path: None,
            config_yaml: Some(format!("command: [sh, '{}']\nsteps:\n  s1:\n    prompt: test\n", backend.display())),
            repo: None,
            pr_number: None,
            workspace_mode: WorkspaceMode::Worktree,
            allow_dirty_working_tree: false,
            attachments: vec![],
            skipped_steps: vec![],
            flags: BTreeMap::default(),
        }).unwrap();
        state.phase = SessionPhase::Planned;
        manager.save(&state).unwrap();
        let plan = state.plan_path(&manager.sessions_dir());
        let original = b"# Saved plan\nOriginal task\n";
        fs::write(&plan, original).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let sink: Arc<dyn ApplicationEventSink> = Arc::new(move |event| {
            captured.lock().unwrap().push(event);
            Ok(())
        });
        let result = app.verify(&state.id, sink).await.unwrap();
        let saved = manager.load(&state.id).unwrap();
        let preserved = result.phase == SessionPhase::Planned
            && saved.phase == SessionPhase::Planned
            && saved.plan_error.is_none()
            && fs::read(plan).unwrap() == original;
        let output = events.lock().unwrap().iter().filter_map(|event| match event {
            ApplicationEvent::PlanChunk { text, .. } => Some(text.as_str()),
            _ => None,
        }).collect::<String>();
        w.application_preserved = preserved;
        w.application_output = output;
    }

    #[then("the application reports that nothing was verified")]
    fn application_nothing(w: &mut World) {
        assert!(w.application_output.contains("nothing was verified"));
        assert!(!w.root().join("calls").exists());
    }

    #[then("the application runs all Quint verification stages")]
    fn application_quint_stages(w: &mut World) {
        assert_eq!(fs::read_to_string(w.root().join("calls")).unwrap().lines().collect::<Vec<_>>(),
            ["--version", "parse", "typecheck", "run", "verify"]);
    }

    #[then("the application preserves the saved plan and makes one backend call")]
    fn application_preserves(w: &mut World) {
        assert!(w.application_preserved);
        assert_eq!(fs::read_to_string(w.root().join("backend-calls")).unwrap(), "call\n");
    }
    #[then("the application preserves the saved plan and interprets the report")]
    fn application_preserves_with_interpretation(w: &mut World) {
        assert!(w.application_preserved);
        assert_eq!(fs::read_to_string(w.root().join("backend-calls")).unwrap(), "call\ncall\n");
    }
    #[tokio::test(flavor = "current_thread")]
    async fn plan_verify_bdd() {
        World::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run_and_exit(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/plan_verify.feature"
            ))
            .await;
    }
}
