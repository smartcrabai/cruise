//! Executable list contract against the shipped CLI, a real PTY, and its daemon.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "daemon_list_api.rs"]
mod daemon_list_api;
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "list_actions_pty.rs"]
mod list_actions_pty;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod spec {
    use super::daemon_list_api::{Daemon, fetch_sessions, write_cli_session};
    use super::list_actions_pty::{DOWN, ENTER, ESC, Fixture, Terminal, UP, checked};
    use cruise::session::SessionPhase;
    use cucumber::{World as _, given, then, when};
    use serde_json::{Value, json};
    use std::{fmt, fs};

    const CLEAN: &str = "terminal-clean-witness";
    const DRAFT: &str = "zzz-ineligible-draft-witness";
    const PLANNED: &str = "eligible-run-witness";

    #[derive(Default, cucumber::World)]
    struct ListWorld {
        // Drop the terminal before stopping its daemon, including on step failures.
        terminal: Option<Terminal>,
        fixture: Option<Fixture>,
        since: usize,
        session_id: Option<String>,
        daemon: Option<Daemon>,
        instance: Value,
        cli_json: Value,
        cli_stderr: String,
        api_json: Value,
    }

    impl fmt::Debug for ListWorld {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            // Do not print the daemon's bearer token on failures.
            f.debug_struct("ListWorld")
                .field("since", &self.since)
                .field("session_id", &self.session_id)
                .field("cli_json", &self.cli_json)
                .field("cli_stderr", &self.cli_stderr)
                .field("api_json", &self.api_json)
                .finish_non_exhaustive()
        }
    }

    impl ListWorld {
        fn fixture(&self) -> &Fixture {
            self.fixture
                .as_ref()
                .unwrap_or_else(|| panic!("missing workflow fixture"))
        }

        fn pty(&mut self) -> &mut Terminal {
            self.terminal
                .as_mut()
                .unwrap_or_else(|| panic!("missing terminal"))
        }

        fn daemon(&self) -> &Daemon {
            self.daemon
                .as_ref()
                .unwrap_or_else(|| panic!("missing daemon fixture"))
        }

        fn prompts(&mut self, text: &str, visible: &[&str]) {
            let since = self.since;
            self.pty().prompt(since, text, visible);
        }

        fn task_editor(&mut self, mode: &str) {
            let mode_picker = self.pty().send(ENTER);
            let editor = self.pty().creation_mode(mode_picker, mode);
            self.pty().task_editor(editor);
        }

        fn phase(&self, id: &str, phase: SessionPhase) {
            assert_eq!(checked(self.fixture().manager.load(id)).phase, phase);
        }
    }

    #[given("an isolated workflow with no sessions")]
    fn workflow(w: &mut ListWorld) {
        w.fixture = Some(Fixture::new());
    }

    #[given("two planned sessions with identical row labels but different plans")]
    fn duplicate_rows(w: &mut ListWorld) {
        w.fixture().seed_duplicate_planned_rows();
    }

    #[given("a completed cleanup-eligible session")]
    fn cleanup_session(w: &mut ListWorld) {
        w.fixture().seed(CLEAN, SessionPhase::Completed);
    }

    #[given("an ineligible draft session")]
    fn draft_session(w: &mut ListWorld) {
        w.fixture().seed(DRAFT, SessionPhase::Draft);
    }

    #[given("an eligible planned session")]
    fn planned_session(w: &mut ListWorld) {
        w.fixture().seed(PLANNED, SessionPhase::Planned);
    }

    #[when(expr = "I open {string} in a terminal")]
    fn open(w: &mut ListWorld, command: String) {
        let args: &[&str] = match command.as_str() {
            "cruise" => &[],
            "cruise list" => &["list"],
            _ => panic!("unsupported command {command}"),
        };
        w.terminal = Some(w.fixture().start(args));
    }

    #[then("the empty picker offers New Session, Clean, Run All, and Quit")]
    fn empty_picker(w: &mut ListWorld) {
        w.prompts(
            "Action:",
            &[
                "No sessions found.",
                "New Session",
                "Clean",
                "Run All",
                "Quit",
            ],
        );
    }

    #[then(expr = "the picker refreshes with {string}")]
    fn refreshed(w: &mut ListWorld, row: String) {
        let since = w.since;
        w.pty().picker(since, &row);
    }

    #[when(expr = "I exit by {string}")]
    fn exit(w: &mut ListWorld, method: String) {
        let keys = match method.as_str() {
            "Quit" => [DOWN, DOWN, DOWN, ENTER].concat(),
            "Escape" => ESC.to_vec(),
            _ => panic!("unsupported exit {method}"),
        };
        w.terminal
            .take()
            .unwrap_or_else(|| panic!("missing terminal"))
            .exit(&keys);
    }

    #[then("no session or workflow activity exists")]
    fn no_activity(w: &mut ListWorld) {
        let fixture = w.fixture();
        assert!(checked(fixture.manager.list()).is_empty());
        assert!(!fixture.file("planning-started").exists());
        assert!(!fixture.file("run-started").exists());
    }

    #[when(expr = "I choose {string} and cancel task entry")]
    fn cancel_task(w: &mut ListWorld, mode: String) {
        w.task_editor(&mode);
        w.since = w.pty().send(b"discard this task\x03");
    }

    #[when(expr = "I choose {string} and submit {string}")]
    fn submit_task(w: &mut ListWorld, mode: String, task: String) {
        w.task_editor(&mode);
        w.since = w.pty().send(format!("{task}\r").as_bytes());
    }

    #[then("the only session is a Draft without planner or runner activity")]
    fn saved_draft(w: &mut ListWorld) {
        let fixture = w.fixture();
        let session = fixture.only_session();
        assert_eq!(session.input, "menu draft witness");
        assert_eq!(session.phase, SessionPhase::Draft);
        assert!(!fixture.file("planning-started").exists());
        assert!(!fixture.file("run-started").exists());
    }

    #[when("I open the initially selected session")]
    fn open_current(w: &mut ListWorld) {
        w.since = w.pty().send(ENTER);
    }

    #[when("I open the next session row")]
    fn open_next(w: &mut ListWorld) {
        w.since = w.pty().choose(&[DOWN]);
    }

    #[then("its submenu offers Generate Plan")]
    fn draft_actions(w: &mut ListWorld) {
        w.prompts("Action:", &["Generate Plan", "Back"]);
    }

    #[then("the submenu displays the second session's plan")]
    fn second_plan(w: &mut ListWorld) {
        w.prompts("Action:", &["Second plan witness", "Back"]);
    }

    #[when("I go back from the session submenu")]
    fn go_back(w: &mut ListWorld) {
        w.since = w.pty().send(ESC);
    }

    #[when("I release the fixture planner")]
    fn release(w: &mut ListWorld) {
        let started = w.fixture().file("planning-started");
        w.pty().wait("planner start", |_| started.exists());
        checked(fs::write(w.fixture().file("planning-release"), b""));
    }

    #[then("the plan awaits approval without direct execution")]
    fn approval(w: &mut ListWorld) {
        w.prompts("Approve", &["Approve", "Execute now", "Menu Plan"]);
        let session = w.fixture().only_session();
        assert_eq!(session.phase, SessionPhase::AwaitingApproval);
        assert!(!session.exec);
        assert!(!w.fixture().file("run-started").exists());
        w.session_id = Some(session.id);
    }

    #[when("I approve the plan without skipping any steps")]
    fn approve(w: &mut ListWorld) {
        w.since = w.pty().send(ENTER);
        w.prompts("Steps to skip", &["Steps to skip"]);
        w.since = w.pty().send(ENTER);
    }

    #[then("the approved session is Planned without runner activity")]
    fn approved(w: &mut ListWorld) {
        assert_eq!(
            w.session_id.as_deref(),
            Some(w.fixture().only_session().id.as_str())
        );
        assert_eq!(w.fixture().only_session().phase, SessionPhase::Planned);
        assert!(!w.fixture().file("run-started").exists());
    }

    #[then("the held background task is literally stdin and has no plan yet")]
    fn held(w: &mut ListWorld) {
        let started = w.fixture().file("planning-started");
        w.pty().wait("held planner start", |_| started.exists());
        let fixture = w.fixture();
        let session = fixture.only_session();
        assert_eq!(session.input, "stdin");
        assert_eq!(session.phase, SessionPhase::Draft);
        assert!(!session.exec);
        assert!(!fixture.file("planning-release").exists());
        assert!(!fixture.file("run-started").exists());
        assert!(!session.plan_path(&fixture.manager.sessions_dir()).exists());
        w.session_id = Some(session.id);
    }

    #[then("that session's plan is written and awaits approval")]
    fn background_done(w: &mut ListWorld) {
        let id = w
            .session_id
            .as_deref()
            .unwrap_or_else(|| panic!("missing held session"));
        let fixture = w
            .fixture
            .as_ref()
            .unwrap_or_else(|| panic!("missing workflow fixture"));
        let manager = &fixture.manager;
        let plan = checked(manager.load(id)).plan_path(&manager.sessions_dir());
        w.terminal
            .as_mut()
            .unwrap_or_else(|| panic!("missing terminal"))
            .wait("background plan", |_| {
                fs::read_to_string(&plan).is_ok_and(|text| text.contains("Menu Plan"))
                    && manager
                        .load(id)
                        .is_ok_and(|session| session.phase == SessionPhase::AwaitingApproval)
            });
        assert!(!fixture.file("run-started").exists());
    }

    #[when(regex = r"^I select Clean and (accept the default answer|confirm)$")]
    fn clean(w: &mut ListWorld, answer: String) {
        w.since = w.pty().choose(&[UP, UP]);
        w.prompts("Clean", &["Clean", "y/N"]);
        w.since = w
            .pty()
            .send(if answer == "confirm" { b"y\r" } else { ENTER });
    }

    #[then("the completed session still exists")]
    fn completed_exists(w: &mut ListWorld) {
        w.phase(CLEAN, SessionPhase::Completed);
    }

    #[when("I select Run All")]
    fn run_all(w: &mut ListWorld) {
        w.since = w.pty().choose(&[UP]);
    }

    #[then("there is a no-work message and the picker returns")]
    fn no_work(w: &mut ListWorld) {
        w.prompts("No Planned or Suspended sessions to run.", &[]);
        refreshed(w, DRAFT.to_owned());
    }

    #[when("I accept the default Run All answer")]
    fn decline_run(w: &mut ListWorld) {
        w.prompts("Run All", &["y/N"]);
        w.since = w.pty().send(ENTER);
    }

    #[when("I confirm Run All")]
    fn confirm_run(w: &mut ListWorld) {
        w.prompts("Run All", &["y/N"]);
        w.since = w.pty().send(b"y\r");
    }

    #[then("the planned session is unchanged")]
    fn planned_unchanged(w: &mut ListWorld) {
        w.phase(PLANNED, SessionPhase::Planned);
    }

    #[then("the draft is unchanged and no workflow started")]
    fn draft_unchanged(w: &mut ListWorld) {
        w.phase(DRAFT, SessionPhase::Draft);
        assert!(!w.fixture().file("run-started").exists());
    }

    #[then("the batch summary appears and the picker refreshes")]
    fn batch_summary(w: &mut ListWorld) {
        w.prompts("Run All Summary", &[]);
        refreshed(w, PLANNED.to_owned());
    }

    #[then("only the planned session ran to completion")]
    fn ran_eligible(w: &mut ListWorld) {
        w.phase(PLANNED, SessionPhase::Completed);
        w.phase(DRAFT, SessionPhase::Draft);
        assert_eq!(checked(fs::read(w.fixture().file("run-started"))), b"x");
    }

    #[given(expr = "an isolated daemon whose inventory is {string}")]
    fn inventory(w: &mut ListWorld, variant: String) {
        let daemon = Daemon::new();
        w.instance = daemon.start();
        match variant.as_str() {
            "populated" => write_cli_session(daemon.home.path()),
            "empty" => (),
            _ => panic!("unsupported inventory {variant}"),
        }
        w.daemon = Some(daemon);
    }

    #[when("I run headless cruise list --json")]
    fn headless(w: &mut ListWorld) {
        let output = w.daemon().success(&["list", "--json"]);
        w.cli_json = checked(serde_json::from_slice(&output.stdout));
        w.cli_stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    }

    #[when("I request the authenticated sessions API")]
    async fn list_api(w: &mut ListWorld) {
        w.api_json = fetch_sessions(&w.instance).await;
    }

    #[then("stdout is an empty JSON array")]
    fn empty_json(w: &mut ListWorld) {
        assert_eq!(w.cli_json, json!([]));
    }

    #[then("the CLI and API expose the same session rows without action rows")]
    fn matching_json(w: &mut ListWorld) {
        assert_eq!(w.cli_json, w.api_json);
        let rows = w
            .cli_json
            .as_array()
            .unwrap_or_else(|| panic!("expected session array"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "list-json-session");
        assert_eq!(rows[0]["phase"], "Planned");
        assert_eq!(rows[0]["title"], "List JSON shape");
        assert_eq!(rows[0]["plan_available"], true);
    }

    #[then("stderr contains no picker prompts")]
    fn no_prompt(w: &mut ListWorld) {
        assert!(
            ![
                "Select a session",
                "Action:",
                "No sessions found",
                "New Session",
                "Run All"
            ]
            .iter()
            .any(|prompt| w.cli_stderr.contains(prompt)),
            "unexpected interactive output: {}",
            w.cli_stderr
        );
    }

    pub async fn run() {
        ListWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run_and_exit(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/list_actions.feature"
            ))
            .await;
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::main]
async fn main() {
    spec::run().await;
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    panic!("list_actions_bdd requires a macOS or Linux PTY");
}
