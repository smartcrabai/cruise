//! Executable contracts for planning input and client parity.
#[cfg(unix)]
mod contracts {

    use std::{
        io::{BufRead, Read, Write},
        net::TcpStream,
        path::Path,
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex},
        thread,
        time::{Duration, Instant},
    };

    use cruise::{
        application::{
            ApplicationEvent, ApplicationEventSink, CruiseApplication, NewSessionRequest,
            PlanRequest, SessionAction,
        },
        error::CruiseError,
        session::{SessionManager, SessionPhase, WorkspaceMode},
    };
    use cucumber::{World as _, given, then, when};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    const CONFIG_YAML: &str = "command: [cat]\nsteps:\n  s1:\n    prompt: planning fixture\n";
    const SENTINEL: &str = "revision-sentinel-9f3a";
    const USER_TEXT_ERROR: &str = "replan does not accept feedback; use fix";

    #[derive(Debug, Default, cucumber::World)]
    pub struct PlanRevisionWorld {
        dir: Option<tempfile::TempDir>,
        session_id: Option<String>,
        feedback_error: Option<String>,
        question_error: Option<String>,
        replan_phase: Option<SessionPhase>,
        client_phases: Vec<(SessionPhase, bool, bool)>,
        fix_phase: Option<SessionPhase>,
        ask_phase: Option<SessionPhase>,
    }

    impl PlanRevisionWorld {
        fn root(&self) -> &Path {
            self.dir
                .as_ref()
                .unwrap_or_else(|| panic!("planning fixture is not initialized"))
                .path()
        }

        fn initialize(&mut self) {
            if self.dir.is_some() {
                return;
            }
            let dir =
                tempfile::tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
            let root = dir.path();
            for name in ["home", "config", "data", "state", "base"] {
                std::fs::create_dir_all(root.join(name))
                    .unwrap_or_else(|error| panic!("create fixture directory: {error}"));
            }
            unsafe {
                std::env::set_var("HOME", root.join("home"));
                std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
                std::env::set_var("XDG_DATA_HOME", root.join("data"));
                std::env::set_var("XDG_STATE_HOME", root.join("state"));
                std::env::set_var("CRUISE_DISABLE_NOTIFICATIONS", "1");
                for key in [
                    "CRUISE_CONFIG",
                    "CRUISE_MODEL",
                    "CRUISE_PLAN_MODEL",
                    "CRUISE_SDK",
                ] {
                    std::env::remove_var(key);
                }
            }
            self.dir = Some(dir);
        }

        fn manager(&self) -> SessionManager {
            SessionManager::new(self.root().join("data/cruise"))
        }

        fn session_id(&self) -> &str {
            self.session_id
                .as_deref()
                .unwrap_or_else(|| panic!("session fixture is not initialized"))
        }

        async fn create_planned_session(&mut self) {
            self.initialize();
            let manager = self.manager();
            let app = CruiseApplication::new(manager);
            let state = app
                .create_session(NewSessionRequest {
                    input: "planning revision fixture task".to_string(),
                    base_dir: self.root().join("base"),
                    config_path: None,
                    config_yaml: Some(CONFIG_YAML.to_string()),
                    repo: None,
                    pr_number: None,
                    workspace_mode: WorkspaceMode::Worktree,
                    allow_dirty_working_tree: false,
                    attachments: vec![],
                    skipped_steps: vec![],
                    flags: Default::default(),
                })
                .unwrap_or_else(|error| panic!("create planning session: {error}"));
            let sink: Arc<dyn ApplicationEventSink> = Arc::new(|_| Ok(()));
            app.generate(
                &state.id,
                PlanRequest {
                    skip_planning: true,
                    ..PlanRequest::default()
                },
                sink,
            )
            .await
            .unwrap_or_else(|error| panic!("seed planning session: {error}"));
            self.session_id = Some(state.id);
        }

        fn set_phase(&self, phase: SessionPhase) -> cruise::session::SessionState {
            let manager = self.manager();
            let mut state = manager
                .load(self.session_id())
                .unwrap_or_else(|error| panic!("load planning session: {error}"));
            state.phase = phase;
            manager
                .save(&state)
                .unwrap_or_else(|error| panic!("save planning session: {error}"));
            state
        }
    }

    fn event_sink() -> Arc<dyn ApplicationEventSink> {
        Arc::new(|_| Ok(()))
    }

    #[given("a session with a usable plan")]
    async fn session_with_plan(world: &mut PlanRevisionWorld) {
        world.create_planned_session().await;
    }

    #[when("I replan with feedback and with a question")]
    async fn replan_with_text(world: &mut PlanRevisionWorld) {
        let app = CruiseApplication::new(world.manager());
        let id = world.session_id().to_string();
        let feedback = app
            .replan(
                &id,
                PlanRequest {
                    feedback: Some("tighten the scope".to_string()),
                    ..PlanRequest::default()
                },
                event_sink(),
            )
            .await;
        let question = app
            .replan(
                &id,
                PlanRequest {
                    question: Some("what changed?".to_string()),
                    ..PlanRequest::default()
                },
                event_sink(),
            )
            .await;
        world.feedback_error = Some(expected_replan_error(feedback));
        world.question_error = Some(expected_replan_error(question));
    }

    fn expected_replan_error(
        result: cruise::error::Result<cruise::session::SessionState>,
    ) -> String {
        match result {
            Err(CruiseError::Other(message)) => message,
            Err(error) => panic!("unexpected replan error variant: {error}"),
            Ok(_) => panic!("replan unexpectedly accepted user text"),
        }
    }

    #[then("both requests are rejected with guidance to use fix")]
    fn replan_text_rejected(world: &mut PlanRevisionWorld) {
        assert_eq!(world.feedback_error.as_deref(), Some(USER_TEXT_ERROR));
        assert_eq!(world.question_error.as_deref(), Some(USER_TEXT_ERROR));
    }

    #[when("I replan without user text")]
    async fn replan_without_text(world: &mut PlanRevisionWorld) {
        let app = CruiseApplication::new(world.manager());
        let state = app
            .replan(world.session_id(), PlanRequest::default(), event_sink())
            .await
            .unwrap_or_else(|error| panic!("replan without text: {error}"));
        world.replan_phase = Some(state.phase);
    }

    #[then("regeneration succeeds and preserves the Planned phase")]
    fn no_text_replan_preserves_phase(world: &mut PlanRevisionWorld) {
        assert_eq!(world.replan_phase, Some(SessionPhase::Planned));
        let state = world
            .manager()
            .load(world.session_id())
            .unwrap_or_else(|error| panic!("load regenerated session: {error}"));
        assert_eq!(state.phase, SessionPhase::Planned);
    }

    #[when("I inspect planning actions in AwaitingApproval and Planned")]
    fn inspect_client_actions(world: &mut PlanRevisionWorld) {
        let root = world.root().to_path_buf();
        let manager = world.manager();
        let app = CruiseApplication::new(manager.clone());
        let mut webui = WebUiProcess::start(&root);
        for phase in [SessionPhase::AwaitingApproval, SessionPhase::Planned] {
            let state = world.set_phase(phase.clone());
            let tui = app.capabilities(&state);
            let tui_has_actions = [
                SessionAction::Fix,
                SessionAction::Ask,
                SessionAction::Verify,
            ]
            .iter()
            .all(|action| tui.contains(action))
                && !format!("{tui:?}").contains("Replan");
            let response = webui.get(&format!("/sessions/{}", world.session_id()));
            let webui_has_actions = ["/editor/fix", "/editor/ask", "/verify"]
                .iter()
                .all(|action| response.contains(action))
                && !response.contains("Replan")
                && !response.contains("/replan");
            world
                .client_phases
                .push((phase, tui_has_actions, webui_has_actions));
        }
        world.set_phase(SessionPhase::Planned);
    }

    #[then("TUI and WebUI offer Fix, Ask, and Verify in both phases without Replan")]
    fn clients_offer_review_actions(world: &mut PlanRevisionWorld) {
        assert_eq!(world.client_phases.len(), 2);
        for (phase, tui, webui) in &world.client_phases {
            assert!(*tui, "TUI planning action parity failed in {phase:?}");
            assert!(*webui, "WebUI planning action parity failed in {phase:?}");
        }
    }

    #[when("I inspect the CLI list for Replan in Planned as the Fix alias")]
    fn cli_list_offers_replan(world: &mut PlanRevisionWorld) {
        cli_replan_menu_visible(world.root());
    }

    #[when("I Fix and Ask the Planned session")]
    async fn fix_and_ask_planned(world: &mut PlanRevisionWorld) {
        world.set_phase(SessionPhase::Planned);
        let app = CruiseApplication::new(world.manager());
        let id = world.session_id().to_string();
        let fixed = app
            .fix(&id, SENTINEL.to_string(), event_sink())
            .await
            .unwrap_or_else(|error| panic!("fix planned session: {error}"));
        let plan = app
            .session_plan(&id)
            .unwrap_or_else(|error| panic!("read fixed plan: {error}"));
        assert!(
            plan.contains(SENTINEL),
            "sentinel missing from plan: {plan}"
        );

        let ask_events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = Arc::clone(&ask_events);
        let ask_sink: Arc<dyn ApplicationEventSink> = Arc::new(move |event| {
            captured_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
            Ok(())
        });
        let asked = app
            .ask(&id, SENTINEL.to_string(), ask_sink)
            .await
            .unwrap_or_else(|error| panic!("ask planned session: {error}"));
        assert!(
        ask_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|event| matches!(event, ApplicationEvent::PlanChunk { text, .. } if text.contains(SENTINEL))),
        "sentinel missing from Ask PlanChunk events"
    );
        world.fix_phase = Some(fixed.phase);
        world.ask_phase = Some(asked.phase);
    }

    #[then("both operations preserve the Planned phase")]
    fn fix_and_ask_preserve_phase(world: &mut PlanRevisionWorld) {
        assert_eq!(world.fix_phase, Some(SessionPhase::Planned));
        assert_eq!(world.ask_phase, Some(SessionPhase::Planned));
    }

    struct WebUiProcess {
        child: Child,
        address: String,
    }

    impl WebUiProcess {
        fn start(root: &Path) -> Self {
            let mut command = Command::new(env!("CARGO_BIN_EXE_cruise-runtime"));
            command
                .args(["webui", "--host", "127.0.0.1", "--port", "0", "--webui-dir"])
                .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"))
                .current_dir(root)
                .env("HOME", root.join("home"))
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("XDG_STATE_HOME", root.join("state"))
                .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
                .env_remove("CRUISE_CONFIG")
                .env_remove("CRUISE_MODEL")
                .env_remove("CRUISE_PLAN_MODEL")
                .env_remove("CRUISE_SDK")
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = command
                .spawn()
                .unwrap_or_else(|error| panic!("start production WebUI: {error}"));
            let mut line = String::new();
            std::io::BufReader::new(
                child
                    .stdout
                    .take()
                    .unwrap_or_else(|| panic!("WebUI stdout missing")),
            )
            .read_line(&mut line)
            .unwrap_or_else(|error| panic!("read WebUI endpoint: {error}"));
            let address = line
                .trim()
                .strip_prefix("cruise webui listening on http://")
                .and_then(|value| value.strip_suffix('/'))
                .unwrap_or_else(|| panic!("unexpected WebUI startup output: {line:?}"))
                .to_string();
            Self { child, address }
        }

        fn get(&mut self, path: &str) -> String {
            let mut stream = TcpStream::connect(&self.address)
                .unwrap_or_else(|error| panic!("connect production WebUI: {error}"));
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap_or_else(|error| panic!("set WebUI timeout: {error}"));
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .unwrap_or_else(|error| panic!("request WebUI page: {error}"));
            let mut response = String::new();
            stream
                .read_to_string(&mut response)
                .unwrap_or_else(|error| panic!("read WebUI page: {error}"));
            assert!(
                response.starts_with("HTTP/1.1 200"),
                "WebUI response: {response}"
            );
            response
        }
    }

    impl Drop for WebUiProcess {
        fn drop(&mut self) {
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }

    struct CliPty {
        child: Box<dyn portable_pty::Child + Send + Sync>,
        writer: Option<Box<dyn Write + Send>>,
        master: Option<Box<dyn portable_pty::MasterPty + Send>>,
        reader: Option<thread::JoinHandle<()>>,
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl Drop for CliPty {
        fn drop(&mut self) {
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
            drop(self.writer.take());
            drop(self.master.take());
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    fn cli_replan_menu_visible(root: &Path) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap_or_else(|error| panic!("open CLI PTY: {error}"));
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
        command.arg("list");
        command.cwd(root);
        command.env("HOME", root.join("home"));
        command.env("XDG_CONFIG_HOME", root.join("config"));
        command.env("XDG_DATA_HOME", root.join("data"));
        command.env("XDG_STATE_HOME", root.join("state"));
        command.env("CRUISE_DISABLE_NOTIFICATIONS", "1");
        command.env("LC_ALL", "C");
        command.env_remove("CRUISE_CONFIG");
        command.env_remove("CRUISE_MODEL");
        command.env_remove("CRUISE_PLAN_MODEL");
        command.env_remove("CRUISE_SDK");
        command.env("TERM", "xterm-256color");
        let writer = pair
            .master
            .take_writer()
            .unwrap_or_else(|error| panic!("open CLI PTY writer: {error}"));
        let reader = pair
            .master
            .try_clone_reader()
            .unwrap_or_else(|error| panic!("open CLI PTY reader: {error}"));
        let child = pair
            .slave
            .spawn_command(command)
            .unwrap_or_else(|error| panic!("spawn CLI list: {error}"));
        drop(pair.slave);
        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_output = Arc::clone(&output);
        let reader = thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = [0; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                reader_output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(&buffer[..count]);
            }
        });
        let mut pty = CliPty {
            child,
            writer: Some(writer),
            master: Some(pair.master),
            reader: Some(reader),
            output,
        };
        pty.wait_for("Select a session:");
        pty.writer
            .as_mut()
            .unwrap_or_else(|| panic!("CLI PTY writer missing"))
            .write_all(b"\r")
            .unwrap_or_else(|error| panic!("select CLI session: {error}"));
        let transcript = pty.wait_for("Replan");
        assert!(
            transcript.contains("Action:"),
            "CLI action prompt missing: {transcript}"
        );
    }

    impl CliPty {
        fn wait_for(&mut self, needle: &str) -> String {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let output = self
                    .output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let transcript = String::from_utf8_lossy(&output).into_owned();
                drop(output);
                if transcript.contains(needle) {
                    return transcript;
                }
                if let Some(status) = self
                    .child
                    .try_wait()
                    .unwrap_or_else(|error| panic!("wait for CLI list: {error}"))
                {
                    panic!("CLI list exited before {needle:?}: {status}\n{transcript}");
                }
                assert!(
                    Instant::now() < deadline,
                    "CLI list did not display {needle:?}:\n{transcript}"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    pub async fn run() {
        PlanRevisionWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run_and_exit(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/plan_revision.feature"
            ))
            .await;
    }
}

#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::run().await;
}

#[cfg(not(unix))]
fn main() {}
