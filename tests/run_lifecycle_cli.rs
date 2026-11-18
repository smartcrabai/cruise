#![cfg(unix)]

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use cruise::session_config::SessionConfigRef;
use tempfile::TempDir;

const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const STATE_TIMEOUT: Duration = Duration::from_secs(20);

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    manager: SessionManager,
    output_index: AtomicUsize,
    release_files: Vec<PathBuf>,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let root = tempfile::tempdir()?;
        let repo = root.path().join("repo");
        fs::create_dir_all(&repo)?;
        for directory in ["home", "config", "data", "state", "workflows", "logs"] {
            fs::create_dir_all(root.path().join(directory))?;
        }
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=Run Lifecycle Test",
                "-c",
                "user.email=run-lifecycle@example.test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        ] {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_COUNT", "0")
                .output()?;
            if !output.status.success() {
                return Err(
                    format!("git failed: {}", String::from_utf8_lossy(&output.stderr)).into(),
                );
            }
        }
        Ok(Self {
            manager: SessionManager::new(root.path().join("data/cruise")),
            root,
            repo,
            output_index: AtomicUsize::new(0),
            release_files: Vec::new(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .current_dir(&self.repo)
            .env("HOME", self.root.path().join("home"))
            .env("USERPROFILE", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("CRUISE_UI_LANGUAGE", "en")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null());
        command
    }

    fn spawn(&self, args: &[&str]) -> io::Result<CapturedChild> {
        let index = self.output_index.fetch_add(1, Ordering::Relaxed);
        let stdout_path = self.root.path().join(format!("logs/{index}.stdout"));
        let stderr_path = self.root.path().join(format!("logs/{index}.stderr"));
        let mut command = self.command();
        command
            .args(args)
            .stdout(Stdio::from(File::create(&stdout_path)?))
            .stderr(Stdio::from(File::create(&stderr_path)?));
        Ok(CapturedChild {
            child: command.spawn()?,
            stdout_path,
            stderr_path,
        })
    }

    fn run(&self, args: &[&str]) -> TestResult<Output> {
        let mut child = self.spawn(args)?;
        match child.wait(CHILD_TIMEOUT) {
            Ok(output) => Ok(output),
            Err(error) => Err(format!("{error}; CLI output: {}", child.current_text()?).into()),
        }
    }

    fn start_daemon(&self) -> TestResult<()> {
        let output = self.run(&["daemon", "start", "--port", "0"])?;
        if !output.status.success() {
            return Err(format!("daemon start failed: {}", output_text(&output)).into());
        }
        Ok(())
    }

    fn add_session(&mut self, id: &str, title: &str, script: &str) -> TestResult<()> {
        let workflow = self.root.path().join(format!("workflows/{id}.yaml"));
        let scalar = serde_json::to_string(script)?;
        fs::write(
            &workflow,
            format!(
                "create_pr: false\ncleanup_after_pr: false\nsteps:\n  main:\n    command: {scalar}"
            ),
        )?;
        let mut state = SessionState::new(
            id.to_owned(),
            self.repo.clone(),
            SessionConfigRef::File { path: workflow },
            title.to_owned(),
        );
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.target_branch = Some("main".to_owned());
        state.phase = SessionPhase::Planned;
        state.title = Some(title.to_owned());
        self.manager.create(&state)?;
        fs::write(
            state.plan_path(&self.manager.sessions_dir()),
            "# Plan\n\nRun the test workflow.\n",
        )?;
        Ok(())
    }

    fn add_gated_session(&mut self, id: &str, title: &str) -> TestResult<PathBuf> {
        let marker = self.root.path().join(format!("{id}.started"));
        let release = self.root.path().join(format!("{id}.release"));
        let script = format!(
            "printf x >> '{}'; while [ ! -f '{}' ]; do sleep 0.02; done",
            marker.display(),
            release.display()
        );
        self.add_session(id, title, &script)?;
        self.release_files.push(release.clone());
        Ok(marker)
    }

    fn phase_json(&self, id: &str) -> TestResult<serde_json::Value> {
        Ok(serde_json::from_slice(&fs::read(
            self.manager.state_path(id),
        )?)?)
    }

    fn wait_for_phase(&self, id: &str, expected: &str) -> TestResult<()> {
        let deadline = Instant::now() + STATE_TIMEOUT;
        loop {
            if self
                .phase_json(id)
                .is_ok_and(|state| state["phase"] == expected)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "session {id} did not reach {expected}; persisted state: {:?}",
                    self.phase_json(id)
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_started(markers: &[PathBuf], count: usize) -> TestResult<()> {
        let deadline = Instant::now() + STATE_TIMEOUT;
        loop {
            if markers.iter().filter(|path| path.exists()).count() >= count {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "only {} of {count} expected command markers appeared",
                    markers.iter().filter(|path| path.exists()).count()
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for release in &self.release_files {
            let _release_result = fs::write(release, b"release");
        }
        let _stop_result = self.run(&["daemon", "stop", "--force"]);
    }
}

struct CapturedChild {
    child: Child,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl CapturedChild {
    fn wait(&mut self, timeout: Duration) -> io::Result<Output> {
        let status = wait_for_child(&mut self.child, timeout)?;
        Ok(Output {
            status,
            stdout: fs::read(&self.stdout_path)?,
            stderr: fs::read(&self.stderr_path)?,
        })
    }
    fn current_text(&self) -> io::Result<String> {
        Ok(format!(
            "{}{}",
            fs::read_to_string(&self.stdout_path)?,
            fs::read_to_string(&self.stderr_path)?
        ))
    }

    fn signal(&self, signal: i32) -> io::Result<()> {
        let pid =
            i32::try_from(self.child.id()).map_err(|error| io::Error::other(error.to_string()))?;
        // SAFETY: pid is the live direct child process ID returned by std::process::Child.
        let result = unsafe { libc::kill(pid, signal) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        self.child.kill()?;
        self.child.wait().map(|_| ())
    }
}

impl Drop for CapturedChild {
    fn drop(&mut self) {
        if self.child.try_wait().is_ok_and(|status| status.is_none()) {
            let _kill_result = self.kill();
        }
    }
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _kill_result = child.kill();
            let _wait_result = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "CLI child did not exit before the deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display())
}

#[test]
fn reattaching_after_client_sigkill_joins_the_existing_run() -> TestResult {
    let mut fixture = Fixture::new()?;
    let marker = fixture.add_gated_session("reattach", "reattach session")?;
    fixture.start_daemon()?;

    let mut first = fixture.spawn(&["run", "reattach"])?;
    Fixture::wait_for_started(std::slice::from_ref(&marker), 1)?;
    fixture.wait_for_phase("reattach", "Running")?;
    first.signal(libc::SIGKILL)?;
    let killed = first.wait(CHILD_TIMEOUT)?;
    assert!(!killed.status.success());
    assert_eq!(fixture.phase_json("reattach")?["phase"], "Running");

    let release = fixture.root.path().join("reattach.release");
    let mut attached = fixture.spawn(&["run", "reattach"])?;
    let attach_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = attached.child.try_wait()? {
            let output = attached.wait(CHILD_TIMEOUT)?;
            return Err(format!(
                "reattaching client exited early ({status:?}): {}",
                output_text(&output)
            )
            .into());
        }
        if Instant::now() >= attach_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::write(release, b"release")?;
    let output = attached.wait(CHILD_TIMEOUT)?;
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(fixture.phase_json("reattach")?["phase"], "Completed");
    assert_eq!(
        fs::read_to_string(marker)?.len(),
        1,
        "step ran more than once"
    );
    Ok(())
}

#[test]
fn daemon_completes_a_run_after_its_client_is_sigkilled() -> TestResult {
    let mut fixture = Fixture::new()?;
    let marker = fixture.add_gated_session("detached", "detached session")?;
    fixture.start_daemon()?;

    let mut client = fixture.spawn(&["run", "detached"])?;
    Fixture::wait_for_started(std::slice::from_ref(&marker), 1)?;
    fixture.wait_for_phase("detached", "Running")?;
    client.signal(libc::SIGKILL)?;
    let killed = client.wait(CHILD_TIMEOUT)?;
    assert!(!killed.status.success());
    assert_eq!(fixture.phase_json("detached")?["phase"], "Running");

    fs::write(fixture.root.path().join("detached.release"), b"release")?;
    fixture.wait_for_phase("detached", "Completed")?;
    assert_eq!(fs::read_to_string(marker)?.len(), 1);
    Ok(())
}

#[test]
fn ctrl_c_suspends_a_single_run_and_a_later_run_resumes_it() -> TestResult {
    let mut fixture = Fixture::new()?;
    let marker = fixture.add_gated_session("interrupt", "interrupt session")?;
    fixture.start_daemon()?;

    let mut client = fixture.spawn(&["run", "interrupt"])?;
    Fixture::wait_for_started(std::slice::from_ref(&marker), 1)?;
    fixture.wait_for_phase("interrupt", "Running")?;
    client.signal(libc::SIGINT)?;
    let output = client.wait(CHILD_TIMEOUT)?;
    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    let message = output_text(&output);
    assert!(
        message.contains("Interrupted -- session saved as Suspended."),
        "{message}"
    );
    assert!(
        message.contains("interrupted by user (Ctrl+C)"),
        "{message}"
    );
    fixture.wait_for_phase("interrupt", "Suspended")?;

    fs::write(fixture.root.path().join("interrupt.release"), b"release")?;
    let resumed = fixture.run(&["run", "interrupt"])?;
    assert!(resumed.status.success(), "{}", output_text(&resumed));
    assert_eq!(fixture.phase_json("interrupt")?["phase"], "Completed");
    assert_eq!(
        fs::read_to_string(marker)?.len(),
        2,
        "resume should rerun the interrupted step"
    );
    Ok(())
}

#[test]
fn run_all_parallelism_completes_all_sessions_and_prints_summary() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut titles = Vec::new();
    let mut markers = Vec::new();
    for index in 0..3 {
        let id = format!("batch-{index}");
        let title = format!("batch title {index}");
        markers.push(fixture.add_gated_session(&id, &title)?);
        titles.push((id, title));
    }
    fixture.start_daemon()?;

    let mut client = fixture.spawn(&["run", "--all", "--parallelism", "2"])?;
    if let Err(error) = Fixture::wait_for_started(&markers, 2) {
        return Err(format!("{error}; CLI output: {}", client.current_text()?).into());
    }
    assert_eq!(
        markers.iter().filter(|path| path.exists()).count(),
        2,
        "parallelism 2 should run two blocked command steps before scheduling the third"
    );
    for index in 0..3 {
        fs::write(
            fixture.root.path().join(format!("batch-{index}.release")),
            b"release",
        )?;
    }
    let output = client.wait(CHILD_TIMEOUT)?;
    assert!(output.status.success(), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(
        text.contains("=== Run All Summary (3 sessions) ==="),
        "{text}"
    );
    for (id, title) in titles {
        assert_eq!(fixture.phase_json(&id)?["phase"], "Completed");
        assert!(
            text.contains(&title),
            "missing {title:?} in summary: {text}"
        );
    }
    Ok(())
}

#[test]
fn ctrl_c_run_all_suspends_active_sessions_without_scheduling_queued_ones() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut ids = Vec::new();
    let mut markers = Vec::new();
    for index in 0..3 {
        let id = format!("batch-cancel-{index}");
        let marker = fixture.add_gated_session(&id, &id)?;
        ids.push(id);
        markers.push(marker);
    }
    fixture.start_daemon()?;

    let mut client = fixture.spawn(&["run", "--all", "--parallelism", "2"])?;
    if let Err(error) = Fixture::wait_for_started(&markers, 2) {
        let states = ids
            .iter()
            .map(|id| fixture.phase_json(id).map(|state| state["phase"].clone()))
            .collect::<TestResult<Vec<_>>>()?;
        return Err(format!(
            "{error}; phases: {states:?}; CLI output: {}",
            client.current_text()?
        )
        .into());
    }
    client.signal(libc::SIGINT)?;
    let output = client.wait(CHILD_TIMEOUT)?;
    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    let phases = ids
        .iter()
        .map(|id| fixture.phase_json(id).map(|state| state["phase"].clone()))
        .collect::<TestResult<Vec<_>>>()?;
    assert_eq!(
        phases.iter().filter(|phase| *phase == "Suspended").count(),
        2,
        "active batch sessions should suspend: {phases:?}"
    );
    assert_eq!(
        phases.iter().filter(|phase| *phase == "Planned").count(),
        1,
        "the queued session should remain planned: {phases:?}"
    );
    assert_eq!(markers.iter().filter(|path| path.exists()).count(), 2);
    Ok(())
}

#[test]
fn run_without_id_selects_the_only_eligible_session() -> TestResult {
    let mut fixture = Fixture::new()?;
    let marker = fixture.root.path().join("picker.started");
    fixture.add_session(
        "picker",
        "only eligible session",
        &format!("touch {}", shell_quote(&marker)),
    )?;
    fixture.start_daemon()?;

    let output = fixture.run(&["run"])?;
    assert!(output.status.success(), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(text.contains("Selected session: picker"), "{text}");
    assert!(marker.exists());
    assert_eq!(fixture.phase_json("picker")?["phase"], "Completed");
    Ok(())
}

#[test]
fn run_without_id_reports_when_no_session_is_eligible() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.start_daemon()?;

    let output = fixture.run(&["run"])?;
    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    assert!(
        output_text(&output).contains("No pending sessions. Run `cruise plan` first."),
        "{}",
        output_text(&output)
    );
    Ok(())
}

#[test]
fn run_refuses_a_live_foreign_runner() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.add_session("foreign-runner", "foreign runner", "true")?;
    fixture.start_daemon()?;

    let mut child = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .spawn()?;
    let pid = child.id();
    let mut state = fixture.manager.load("foreign-runner")?;
    state.set_runner_to_process(pid);
    fixture.manager.save(&state)?;
    let before = fs::read(fixture.manager.state_path("foreign-runner"))?;

    let output = fixture.run(&["run", "foreign-runner"]);
    let stopped = child.kill();
    let reaped = child.wait();
    let output = output?;
    stopped?;
    reaped?;

    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(
        text.contains(&format!(
            "session foreign-runner is being executed by another cruise process (PID {pid})"
        )),
        "{text}"
    );
    assert_eq!(
        fs::read(fixture.manager.state_path("foreign-runner"))?,
        before,
        "a rejected run must leave state.json unchanged"
    );
    Ok(())
}
