#![cfg(unix)]
//! Real executable contracts for plan C-1 through C-5.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::{Value, json};
use tempfile::TempDir;

const ID: &str = "20261024000002";
const AT: &str = "2026-10-24T00:00:00Z";

struct Fixture {
    root: TempDir,
    manager: SessionManager,
}

impl Fixture {
    fn new(phase: SessionPhase) -> Self {
        let root = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(root.path().join("data/cruise"));
        let mut state = SessionState::new(
            ID.into(),
            root.path().to_path_buf(),
            SessionConfigRef::BuiltinSnapshot { name: None },
            "contract fixture".into(),
        );
        state.phase = phase;
        state.set_runner_to_current_process();
        manager.create(&state).unwrap();
        Self { root, manager }
    }

    fn ledger(&self) -> PathBuf {
        self.manager
            .sessions_dir()
            .join(ID)
            .join("instructions.jsonl")
    }

    fn seed(&self, events: &[Value]) {
        let mut data = String::new();
        for event in events {
            data.push_str(&format!("{event}\n"));
        }
        fs::write(self.ledger(), data).unwrap();
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(self.root.path())
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut cmd = self.command();
        cmd.args(args);
        if stdin.is_some() {
            cmd.stdin(Stdio::piped());
        }
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }
}

fn opened() -> Value {
    json!({"kind":"run_opened", "at":AT})
}
fn accepted(id: u64, content: &str) -> Value {
    json!({"kind":"accepted", "id":id, "at":AT, "origin":"cli", "dedupe_key":null, "content":content})
}
fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn c1_accepts_argument_dash_and_implicit_stdin() {
    for (args, input) in [
        (vec!["tell", ID, "hello"], None),
        (vec!["tell", ID, "-"], Some("hello")),
        (vec!["tell", ID], Some("hello")),
    ] {
        let f = Fixture::new(SessionPhase::Running);
        f.seed(&[opened()]);
        let result = f.run(&args, input);
        assert!(result.status.success(), "{}", err(&result));
        assert_eq!(
            out(&result).trim(),
            format!("instruction #1 accepted for {ID}; queued for the next agent prompt, if one runs.")
        );
        let events: Vec<Value> = fs::read_to_string(f.ledger())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["kind"], "accepted");
        assert_eq!(events[1]["id"], 1);
        assert_eq!(events[1]["content"], "hello");
        assert_eq!(events[1]["origin"], "cli");
    }
}

#[test]
fn c1_missing_text_on_tty_is_rejected() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::Read;
    let f = Fixture::new(SessionPhase::Running);
    f.seed(&[opened()]);
    let pair = native_pty_system().openpty(PtySize::default()).unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
    cmd.args(["tell", ID]);
    cmd.cwd(f.root.path());
    cmd.env("HOME", f.root.path().join("home"));
    cmd.env("XDG_DATA_HOME", f.root.path().join("data"));
    cmd.env("XDG_CONFIG_HOME", f.root.path().join("config"));
    cmd.env("XDG_STATE_HOME", f.root.path().join("state"));
    cmd.env("NO_COLOR", "1");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("tell without text blocked on a TTY");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let mut text = String::new();
    let _ = pair
        .master
        .try_clone_reader()
        .unwrap()
        .read_to_string(&mut text);
    assert!(!status.success(), "{text}");
    assert!(text.contains("instruction text is required"), "{text}");
}

#[test]
fn c2_log_formats_history_without_mutating_ledger() {
    let f = Fixture::new(SessionPhase::Running);
    f.seed(&[
        opened(),
        accepted(1, "first line\nsecond line"),
        accepted(2, "second"),
        json!({"kind":"leased","ids":[2],"step":"build","scope":"sequential","at":AT}),
        json!({"kind":"delivered","ids":[2],"step":"build","at":AT}),
    ]);
    let before = fs::read(f.ledger()).unwrap();
    let result = f.run(&["tell", ID, "--log"], None);
    assert!(result.status.success(), "{}", err(&result));
    let text = out(&result);
    assert!(
        text.lines()
            .any(|l| l == format!("#1  {AT}  cli  pending  first line")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l == format!("#2  {AT}  cli  delivered@build  second")),
        "{text}"
    );
    assert_eq!(before, fs::read(f.ledger()).unwrap());
}

#[test]
fn c3_planned_session_rejects_instruction() {
    let f = Fixture::new(SessionPhase::Planned);
    f.seed(&[opened()]);
    let before = fs::read(f.ledger()).unwrap();
    let result = f.run(&["tell", ID, "x"], None);
    assert_eq!(result.status.code(), Some(1), "{}", err(&result));
    assert!(err(&result).contains(&format!("not running: session {ID} is in phase Planned; instructions are accepted only while a run is in progress")), "{}", err(&result));
    assert_eq!(before, fs::read(f.ledger()).unwrap());
}

#[test]
fn c4_tell_is_a_subcommand() {
    let f = Fixture::new(SessionPhase::Planned);
    let result = f.run(&["tell", "--help"], None);
    assert!(result.status.success(), "{}", err(&result));
    assert!(
        out(&result).contains("Usage: cruise tell"),
        "{}",
        out(&result)
    );
}

#[test]
fn c5_list_json_adds_pending_count_without_changing_existing_fields() {
    let f = Fixture::new(SessionPhase::Running);
    f.seed(&[opened()]);
    let list = || -> Value {
        let result = f.run(&["list", "--json"], None);
        assert!(result.status.success(), "{}", err(&result));
        let items: Vec<Value> = serde_json::from_slice(&result.stdout).unwrap();
        items.into_iter().find(|item| item["id"] == ID).unwrap()
    };
    let mut zero = list();
    assert_eq!(zero["pending_instructions"], 0);
    f.seed(&[opened(), accepted(1, "one"), accepted(2, "two")]);
    let mut two = list();
    assert_eq!(two["pending_instructions"], 2);
    zero.as_object_mut().unwrap().remove("pending_instructions");
    two.as_object_mut().unwrap().remove("pending_instructions");
    assert_eq!(zero, two, "existing list fields changed when inbox changed");
    assert_eq!(zero["id"], ID);
    assert_eq!(zero["phase"], "Running");
    assert_eq!(zero["input"], "contract fixture");
}
