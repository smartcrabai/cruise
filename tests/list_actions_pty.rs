#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use cruise::session_config::SessionConfigRef;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::{NamedTempFile, TempDir};

const TIMEOUT: Duration = Duration::from_secs(25);
const PICKER: &str = "Action:";
const TASK: &str = "What would you like to implement?";
pub(crate) const ENTER: &[u8] = b"\r";
pub(crate) const ESC: &[u8] = b"\x1b";
pub(crate) const UP: &[u8] = b"\x1b[A";
pub(crate) const DOWN: &[u8] = b"\x1b[B";

pub(crate) fn checked<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("list PTY fixture: {error}"))
}

fn run_fixture_command(command: &mut Command, timeout: Duration) -> Result<(), String> {
    let description = format!("{command:?}");
    let io_error = |error| format!("{description}: {error}");
    // Regular files let the child write without pipe backpressure and let us
    // read diagnostics without waiting for daemon descendants to close stdio.
    let stdout = NamedTempFile::new().map_err(io_error)?;
    let stderr = NamedTempFile::new().map_err(io_error)?;
    let mut child = command
        .stdout(stdout.reopen().map_err(io_error)?)
        .stderr(stderr.reopen().map_err(io_error)?)
        .spawn()
        .map_err(io_error)?;
    let deadline = Instant::now() + timeout;
    let failure = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => break status.to_string(),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            outcome => {
                let reason = match outcome {
                    Ok(_) => format!("timed out after {timeout:?}"),
                    Err(error) => format!("could not poll child: {error}"),
                };
                let killed = child.kill();
                let reaped = child.wait();
                break format!("{reason} (kill: {killed:?}, wait: {reaped:?})");
            }
        }
    };
    let capture = |file: NamedTempFile| {
        let mut bytes = Vec::new();
        let file = file.into_file();
        match file
            .metadata()
            .and_then(|metadata| file.take(metadata.len()).read_to_end(&mut bytes))
        {
            Ok(_) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => format!("could not read captured output: {error}"),
        }
    };
    Err(format!(
        "{description}: {failure}\nstdout: {}\nstderr: {}",
        capture(stdout),
        capture(stderr)
    ))
}

#[test]
fn fixture_command_times_out_with_diagnostics_and_reaps_child() {
    let mut command = Command::new("sh");
    command.args(["-c", "echo child=$$; echo stalled >&2; sleep 10 & wait"]);
    let start = Instant::now();
    let Err(error) = run_fixture_command(&mut command, Duration::from_secs(2)) else {
        panic!("stalled fixture command unexpectedly succeeded");
    };
    assert!(start.elapsed() < Duration::from_secs(8), "{error}");
    assert!(
        error.contains("timed out") && error.contains("stderr: stalled"),
        "{error}"
    );
    let pid = error
        .lines()
        .find_map(|line| line.strip_prefix("stdout: child="))
        .unwrap_or_else(|| panic!("missing child PID: {error}"));
    let pid: libc::pid_t = checked(pid.parse());
    assert!(pid > 0);
    // SAFETY: Signal zero only checks whether the positive child PID exists.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

pub(crate) struct Fixture {
    root: TempDir,
    repo: PathBuf,
    home: PathBuf,
    config: PathBuf,
    pub(crate) manager: SessionManager,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let root = checked(TempDir::new());
        let repo = root.path().join("repo");
        let home = root.path().join("home");
        let config = repo.join("cruise.yaml");
        let manager = SessionManager::new(home.join("data/cruise"));
        checked(fs::create_dir(&repo));
        checked(fs::create_dir(&home));
        let fixture = Self {
            root,
            repo,
            home,
            config,
            manager,
        };
        fixture.git(&["init", "-b", "main"]);

        // The planner and workflow have independent witnesses. Holding the
        // planner open makes a synchronous background implementation fail.
        let planning = format!(
            "touch '{}'; i=0; while [ ! -f '{}' ]; do i=$((i + 1)); [ \"$i\" -lt 1500 ] || exit 1; sleep 0.02; done; printf '# Menu Plan\\n\\n- deterministic plan\\n' > \"$PLAN_FILE\"",
            fixture.file("planning-started").display(),
            fixture.file("planning-release").display(),
        );
        let running = format!(
            "printf x >> '{}'; while [ -f '{}' ]; do sleep 0.02; done",
            fixture.file("run-started").display(),
            fixture.file("hold-run").display(),
        );
        checked(fs::write(
            &fixture.config,
            format!(
                "force_exec: true\ninteractive_planning: false\ncreate_pr: false\ncleanup_after_pr: false\ncommand: [sh, -c, {}]\nenv:\n  PLAN_FILE: '{{plan}}'\nsteps:\n  main:\n    command: {}\n",
                checked(serde_json::to_string(&planning)),
                checked(serde_json::to_string(&running)),
            ),
        ));
        fixture.git(&["add", "cruise.yaml"]);
        fixture.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-m",
            "fixture workflow",
        ]);
        fixture.cli(&["daemon", "start", "--port", "0"]);
        fixture
    }

    fn environment(&self) -> [(&'static str, PathBuf); 7] {
        [
            ("HOME", self.home.clone()),
            ("XDG_CONFIG_HOME", self.home.join("config")),
            ("XDG_DATA_HOME", self.home.join("data")),
            ("XDG_STATE_HOME", self.home.join("state")),
            ("CRUISE_UI_LANGUAGE", "en".into()),
            ("CRUISE_DISABLE_NOTIFICATIONS", "1".into()),
            ("GIT_CONFIG_COUNT", "0".into()),
        ]
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") || key == "HERDR_ENV" {
                command.env_remove(key);
            }
        }
        command
            .envs(self.environment())
            .current_dir(&self.repo)
            .stdin(Stdio::null());
        command
    }

    fn git(&self, args: &[&str]) {
        self.run("git", args);
    }

    fn cli(&self, args: &[&str]) {
        self.run(env!("CARGO_BIN_EXE_cruise"), args);
    }

    fn run(&self, program: &str, args: &[&str]) {
        checked(run_fixture_command(
            self.command(program).args(args),
            TIMEOUT,
        ));
    }

    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn observe_requests(&self) -> RequestObserver {
        let registry = self.home.join("state/cruise/daemon/instance.json");
        let original = checked(fs::read(&registry));
        let mut record: serde_json::Value = checked(serde_json::from_slice(&original));
        let origin = record["origin"]
            .as_str()
            .unwrap_or_else(|| panic!("missing daemon origin"))
            .trim_end_matches('/')
            .to_owned();
        let runtime = checked(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build(),
        );
        let listener =
            runtime.block_on(async { checked(tokio::net::TcpListener::bind("127.0.0.1:0").await) });
        record["origin"] = format!("http://{}/", checked(listener.local_addr())).into();
        let posts = Arc::new(AtomicUsize::new(0));
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let observed_posts = Arc::clone(&posts);
        let observed_outcomes = Arc::clone(&outcomes);
        let http = checked(reqwest::Client::builder().no_proxy().build());
        let router = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let url = format!("{origin}{}", request.uri());
            let run_all = request.method() == axum::http::Method::POST
                && request.uri().path() == "/api/v1/run-all";
            if run_all {
                observed_posts.fetch_add(1, Ordering::SeqCst);
            }
            let outcomes = Arc::clone(&observed_outcomes);
            let http = http.clone();
            async move {
                let (mut parts, body) = request.into_parts();
                parts.headers.remove(axum::http::header::HOST);
                let upstream = checked(
                    http.request(parts.method, url)
                        .headers(parts.headers)
                        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
                        .send()
                        .await,
                );
                let mut response = axum::http::Response::new(());
                *response.status_mut() = upstream.status();
                *response.headers_mut() = upstream.headers().clone();
                let body = if run_all {
                    let bytes = checked(upstream.bytes().await);
                    checked(outcomes.lock()).push(checked(serde_json::from_slice(&bytes)));
                    axum::body::Body::from(bytes)
                } else {
                    axum::body::Body::from_stream(upstream.bytes_stream())
                };
                response.map(|()| body)
            }
        });
        runtime.spawn(async move { checked(axum::serve(listener, router).await) });
        checked(fs::write(&registry, checked(serde_json::to_vec(&record))));
        RequestObserver {
            registry,
            original,
            _runtime: runtime,
            posts,
            outcomes,
        }
    }

    pub(crate) fn only_session(&self) -> SessionState {
        let mut sessions = checked(self.manager.list());
        assert_eq!(sessions.len(), 1);
        sessions.remove(0)
    }

    pub(crate) fn seed(&self, id: &str, phase: SessionPhase) {
        let mut state = SessionState::new(
            id.to_owned(),
            self.repo.clone(),
            SessionConfigRef::File {
                path: self.config.clone(),
            },
            id.to_owned(),
        );
        state.title = Some(id.to_owned());
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.target_branch = Some("main".to_owned());
        state.phase = phase;
        checked(self.manager.create(&state));
        if state.phase == SessionPhase::Planned {
            checked(fs::write(
                state.plan_path(&self.manager.sessions_dir()),
                "# Seed Plan\n\nRun the fixture.\n",
            ));
        }
    }

    pub(crate) fn seed_duplicate_planned_rows(&self) {
        for (id, witness) in [
            ("20260101120001", "First plan witness"),
            ("20260101120002", "Second plan witness"),
        ] {
            self.seed(id, SessionPhase::Planned);
            let mut state = checked(self.manager.load(id));
            state.title = Some("Duplicate title".to_owned());
            checked(self.manager.save(&state));
            checked(fs::write(
                state.plan_path(&self.manager.sessions_dir()),
                format!("# {witness}\n"),
            ));
        }
    }

    pub(crate) fn start(&self, args: &[&str]) -> Terminal {
        self.start_with_config(args, &self.config)
    }

    fn start_with_config(&self, args: &[&str], config: &Path) -> Terminal {
        let pair = checked(native_pty_system().openpty(PtySize {
            rows: 40,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        }));
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
        command.args(args);
        command.cwd(&self.repo);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") || key == "HERDR_ENV" {
                command.env_remove(key);
            }
        }
        for (key, value) in self.environment() {
            command.env(key, value);
        }
        // The shared interactive resolver otherwise offers bundled workflows too.
        command.env("CRUISE_CONFIG", config);
        command.env("TERM", "xterm-256color");
        command.env("NO_COLOR", "1");
        let child = checked(pair.slave.spawn_command(command));
        drop(pair.slave);
        let mut reader = checked(pair.master.try_clone_reader());
        let writer = checked(pair.master.take_writer());
        let output = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&output);
        thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                checked(capture.lock()).extend_from_slice(&buf[..n]);
            }
        });
        Terminal {
            _master: pair.master,
            child,
            writer,
            output,
            answered: [0; 3],
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.file("hold-run"));
        let _ = fs::write(self.file("planning-release"), b"");
        if self.home.join("state/cruise/daemon/instance.json").exists()
            && let Err(error) = run_fixture_command(
                self.command(env!("CARGO_BIN_EXE_cruise"))
                    .args(["daemon", "stop", "--force"]),
                TIMEOUT,
            )
        {
            if thread::panicking() {
                eprintln!("list PTY fixture teardown: {error}");
            } else {
                panic!("list PTY fixture teardown: {error}");
            }
        }
    }
}

struct RequestObserver {
    registry: PathBuf,
    original: Vec<u8>,
    _runtime: tokio::runtime::Runtime,
    posts: Arc<AtomicUsize>,
    outcomes: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Drop for RequestObserver {
    fn drop(&mut self) {
        checked(fs::write(&self.registry, &self.original));
    }
}

pub(crate) struct Terminal {
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    answered: [usize; 3],
}

impl Terminal {
    pub(crate) fn send(&mut self, bytes: &[u8]) -> usize {
        let before = checked(self.output.lock()).len();
        checked(self.writer.write_all(bytes));
        checked(self.writer.flush());
        before
    }

    fn raw(&self) -> String {
        String::from_utf8_lossy(&checked(self.output.lock())).into_owned()
    }

    fn answer_queries(&mut self) {
        let raw = self.raw();
        for (index, (query, answer)) in [
            ("\x1b[?u", &b"\x1b[?1u"[..]),
            ("\x1b[c", &b"\x1b[?1;2c"[..]),
            ("\x1b[6n", &b"\x1b[1;1R"[..]),
        ]
        .into_iter()
        .enumerate()
        {
            let count = raw.matches(query).count();
            while self.answered[index] < count {
                self.send(answer);
                self.answered[index] += 1;
            }
        }
    }

    pub(crate) fn wait(&mut self, description: &str, mut predicate: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if predicate(self) {
                return;
            }
            assert!(
                checked(self.child.try_wait()).is_none(),
                "cruise exited while waiting for {description}. PTY transcript:\n{}",
                self.raw()
            );
            self.answer_queries();
            assert!(
                Instant::now() < deadline,
                "waiting for {description}; PTY transcript:\n{}",
                self.raw()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn screen(&self) -> String {
        let dimension = |n| NonZeroU16::new(n).unwrap_or_else(|| panic!("zero PTY dimension"));
        let mut parser = vt100::Parser::new(dimension(40), dimension(160), 0);
        parser.process(&checked(self.output.lock()));
        parser.screen().contents()
    }

    pub(crate) fn prompt(&mut self, after: usize, fresh: &str, visible: &[&str]) {
        self.wait(fresh, |pty| {
            let raw = checked(pty.output.lock());
            let recent = String::from_utf8_lossy(&raw[after..]);
            let rendered = recent.contains(fresh);
            drop(raw);
            rendered && visible.iter().all(|needle| pty.screen().contains(needle))
        });
    }

    pub(crate) fn picker(&mut self, after: usize, session_or_empty: &str) {
        self.prompt(after, PICKER, &["New Session", "Run All", session_or_empty]);
    }

    pub(crate) fn task_editor(&mut self, after: usize) {
        self.prompt(after, TASK, &[TASK]);
        self.wait("task editor ready", |pty| {
            pty.screen()
                .lines()
                .last()
                .is_some_and(|line| line.trim() == ">")
        });
    }

    pub(crate) fn creation_mode(&mut self, after: usize, mode: &str) -> usize {
        self.wait("creation mode picker", |pty| {
            let recent = {
                let output = checked(pty.output.lock());
                String::from_utf8_lossy(&output[after..]).to_ascii_lowercase()
            };
            recent.contains("draft") && pty.screen().to_ascii_lowercase().contains(mode)
        });
        self.send(format!("{mode}\r").as_bytes())
    }

    pub(crate) fn choose(&mut self, moves: &[&[u8]]) -> usize {
        let mut keys = moves.concat();
        keys.extend_from_slice(ENTER);
        self.send(&keys)
    }

    pub(crate) fn exit(mut self, selection: &[u8]) {
        self.send(selection);
        self.finish();
    }

    fn finish(mut self) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = checked(self.child.try_wait()) {
                assert!(status.success(), "cruise failed: {}", self.raw());
                return;
            }
            self.answer_queries();
            assert!(
                Instant::now() < deadline,
                "cruise did not exit: {}",
                self.raw()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
fn empty_entry_points_stay_interactive_and_exit_explicitly() {
    let fixture = Fixture::new();
    for (args, exit) in [
        (&["list"][..], &b"\x1b[B\x1b[B\x1b[B\r"[..]),
        (&[][..], ESC),
    ] {
        let mut pty = fixture.start(args);
        pty.picker(0, "No sessions found.");
        assert!(checked(pty.child.try_wait()).is_none());
        pty.exit(exit);
    }
    assert_eq!(checked(fixture.manager.list()).len(), 0);
}

#[test]
fn identical_session_rows_open_the_selected_sessions_plan() {
    let fixture = Fixture::new();
    fixture.seed_duplicate_planned_rows();

    let mut pty = fixture.start(&["list"]);
    pty.picker(0, "Duplicate title");
    let submenu = pty.choose(&[DOWN]);
    pty.prompt(submenu, "Action:", &["Second plan witness", "Back"]);
    let back = pty.send(ESC);
    pty.picker(back, "Duplicate title");
    pty.exit(ESC);
}

#[test]
fn task_cancel_writes_nothing_and_draft_bypasses_the_planner() {
    let fixture = Fixture::new();
    let override_config = fixture.file("override.yaml");
    checked(fs::write(
        &override_config,
        "command: [echo]\nsteps:\n  main:\n    command: echo override\n",
    ));
    let override_config = checked(override_config.canonicalize());
    let mut pty = fixture.start_with_config(&["list"], &override_config);
    pty.picker(0, "No sessions found.");
    let mode = pty.send(ENTER);
    let task = pty.creation_mode(mode, "draft");
    pty.task_editor(task);
    let cancelled = pty.send(b"discard this task\x03");
    pty.picker(cancelled, "No sessions found.");
    assert_eq!(checked(fixture.manager.list()).len(), 0);
    assert!(!fixture.file("planning-started").exists());

    let mode = pty.send(ENTER);
    let task = pty.creation_mode(mode, "draft");
    pty.task_editor(task);
    let submitted = pty.send(b"menu draft witness\r");
    pty.picker(submitted, "menu draft witness");
    let state = fixture.only_session();
    assert_eq!(state.input, "menu draft witness");
    assert_eq!(state.phase, SessionPhase::Draft);
    assert_eq!(
        checked(state.base_dir.canonicalize()),
        checked(fixture.repo.canonicalize())
    );
    assert_eq!(
        state.config,
        SessionConfigRef::File {
            path: override_config
        }
    );
    assert!(!fixture.file("planning-started").exists());
    assert!(!fixture.file("run-started").exists());

    // After refresh the initial cursor must select the new session, not New Session.
    let row = pty.send(ENTER);
    pty.prompt(row, "Action:", &["Generate Plan", "Back"]);
    let back = pty.send(ESC);
    pty.picker(back, "menu draft witness");
    pty.exit(ESC);
}

#[test]
fn interactive_plan_ignores_force_exec_and_approval_refreshes_picker() {
    let fixture = Fixture::new();
    let mut pty = fixture.start(&["list"]);
    pty.picker(0, "No sessions found.");
    let mode = pty.send(ENTER);
    let task = pty.creation_mode(mode, "interactive");
    pty.task_editor(task);
    let submitted = pty.send(b"interactive plan witness\r");
    pty.wait("planner start", |_| {
        fixture.file("planning-started").exists()
    });
    checked(fs::write(fixture.file("planning-release"), b""));
    pty.prompt(
        submitted,
        "Approve",
        &["Approve", "Execute now", "Menu Plan"],
    );
    let reviewing = fixture.only_session();
    assert_eq!(reviewing.phase, SessionPhase::AwaitingApproval);
    assert!(!reviewing.exec);
    let approve = pty.send(ENTER);
    pty.prompt(approve, "Steps to skip", &["Steps to skip"]);
    let skips = pty.send(ENTER);
    pty.picker(skips, "Planned");
    let state = fixture.only_session();
    assert_eq!(state.id, reviewing.id);
    assert_eq!(state.phase, SessionPhase::Planned);
    assert!(!fixture.file("run-started").exists());
    pty.exit(ESC);
}

#[test]
fn background_plan_returns_before_completion_and_preserves_literal_stdin() {
    let fixture = Fixture::new();
    let mut pty = fixture.start(&["list"]);
    pty.picker(0, "No sessions found.");
    let mode = pty.send(ENTER);
    let task = pty.creation_mode(mode, "background");
    pty.task_editor(task);
    let submitted = pty.send(b"stdin\r");
    pty.wait("held background planner", |_| {
        fixture.file("planning-started").exists()
    });
    pty.picker(submitted, "stdin");
    assert!(!fixture.file("planning-release").exists());
    let state = fixture.only_session();
    assert_eq!(state.input, "stdin");
    assert_eq!(state.phase, SessionPhase::Draft);
    let plan = state.plan_path(&fixture.manager.sessions_dir());
    assert!(!plan.exists());
    assert!(!state.exec);
    assert!(!fixture.file("run-started").exists());
    checked(fs::write(fixture.file("planning-release"), b""));
    pty.wait("background plan written", |_| {
        fs::read_to_string(&plan).is_ok_and(|text| text.contains("Menu Plan"))
            && fixture
                .manager
                .load(&state.id)
                .is_ok_and(|state| state.phase == SessionPhase::AwaitingApproval)
    });
    pty.exit(ESC);
}

#[test]
fn clean_defaults_to_no_then_removes_the_last_visible_session() {
    let fixture = Fixture::new();
    fixture.seed("terminal-clean-witness", SessionPhase::Completed);
    let mut pty = fixture.start(&["list"]);
    pty.picker(0, "terminal-clean-witness");
    let clean = pty.choose(&[UP, UP]);
    pty.prompt(clean, "Clean", &["Clean", "y/N"]);
    let decline = pty.send(ENTER);
    pty.picker(decline, "terminal-clean-witness");
    assert_eq!(fixture.only_session().phase, SessionPhase::Completed);

    let clean = pty.choose(&[UP, UP]);
    pty.prompt(clean, "Clean", &["Clean", "y/N"]);
    let confirm = pty.send(b"y\r");
    pty.picker(confirm, "No sessions found.");
    assert_eq!(checked(fixture.manager.list()).len(), 0);
    pty.exit(ESC);
}

#[test]
fn run_all_requires_eligible_work_and_confirmation() {
    let fixture = Fixture::new();
    let observer = fixture.observe_requests();
    fixture.seed("zzz-ineligible-draft-witness", SessionPhase::Draft);
    let mut pty = fixture.start(&["list"]);
    pty.picker(0, "zzz-ineligible-draft-witness");
    let before = observer.posts.load(Ordering::SeqCst);
    let no_work = pty.choose(&[UP]);
    pty.picker(no_work, "zzz-ineligible-draft-witness");
    assert!(!fixture.file("run-started").exists());
    assert_eq!(fixture.only_session().phase, SessionPhase::Draft);
    assert_eq!(
        observer.posts.load(Ordering::SeqCst),
        before,
        "Draft-only picker sent POST /run-all"
    );
    pty.exit(ESC);

    fixture.seed("eligible-run-witness", SessionPhase::Planned);
    let mut pty = fixture.start(&["list"]);
    pty.picker(0, "eligible-run-witness");
    let run = pty.choose(&[UP]);
    pty.prompt(run, "Run All", &["y/N"]);
    let decline = pty.send(ENTER);
    pty.picker(decline, "eligible-run-witness");
    assert_eq!(
        checked(fixture.manager.load("eligible-run-witness")).phase,
        SessionPhase::Planned
    );
    assert!(!fixture.file("run-started").exists());

    let run = pty.choose(&[UP]);
    pty.prompt(run, "Run All", &["y/N"]);
    let confirm = pty.send(b"y\r");
    pty.prompt(confirm, "Run All Summary", &[]);
    pty.picker(confirm, "eligible-run-witness");
    assert_eq!(checked(fs::read(fixture.file("run-started"))), b"x");
    assert_eq!(
        checked(fixture.manager.load("eligible-run-witness")).phase,
        SessionPhase::Completed
    );
    assert_eq!(
        checked(fixture.manager.load("zzz-ineligible-draft-witness")).phase,
        SessionPhase::Draft
    );
    pty.exit(ESC);
}

#[test]
fn run_all_attach_confirmation_never_starts_a_new_batch() {
    for still_active in [true, false] {
        let fixture = Fixture::new();
        let observer = fixture.observe_requests();
        fixture.seed("active-batch-witness", SessionPhase::Planned);
        checked(fs::write(fixture.file("hold-run"), b""));
        let mut standalone = fixture.start(&["run", "--all"]);
        standalone.wait("standalone workflow start", |_| {
            fixture.file("run-started").exists()
        });
        assert_eq!(checked(observer.outcomes.lock())[0]["attached"], false);
        let mut standalone = Some(standalone);

        let mut picker = fixture.start(&["list"]);
        picker.picker(0, "active-batch-witness");
        let run = picker.choose(&[UP]);
        picker.prompt(run, "Attach to the active batch?", &["y/N"]);
        if !still_active {
            checked(fs::remove_file(fixture.file("hold-run")));
            standalone
                .take()
                .unwrap_or_else(|| panic!("missing standalone client"))
                .finish();
            fixture.seed("new-planned-witness", SessionPhase::Planned);
        }

        let confirm = picker.send(b"y\r");
        picker.wait("attach request outcome", |_| {
            checked(observer.outcomes.lock()).len() == 2
        });
        if still_active {
            assert_eq!(checked(observer.outcomes.lock())[1]["attached"], true);
            checked(fs::remove_file(fixture.file("hold-run")));
            standalone
                .take()
                .unwrap_or_else(|| panic!("missing standalone client"))
                .finish();
            picker.picker(confirm, "active-batch-witness");
        } else {
            assert_eq!(checked(observer.outcomes.lock())[1]["noActiveBatch"], true);
            picker.picker(confirm, "new-planned-witness");
            assert_eq!(
                checked(fixture.manager.load("new-planned-witness")).phase,
                SessionPhase::Planned
            );
        }
        assert_eq!(checked(fs::read(fixture.file("run-started"))), b"x");
        assert_eq!(
            checked(fixture.manager.load("active-batch-witness")).phase,
            SessionPhase::Completed
        );
        picker.exit(ESC);
    }
}
