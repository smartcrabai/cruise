#![cfg(unix)]
//! Black-box ledger contracts. No inbox implementation APIs are required.
use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
use tempfile::TempDir;

const ID: &str = "20261024000003";
const AT: &str = "2026-10-24T00:00:00Z";

struct Fixture {
    root: TempDir,
    ledger: PathBuf,
}
impl Fixture {
    fn new(phase: SessionPhase, alive: bool) -> Self {
        let root = tempfile::tempdir()
            .unwrap_or_else(|e| panic!("create fixture temporary directory: {e}"));
        let manager = SessionManager::new(root.path().join("data/cruise"));
        let mut state = SessionState::new(
            ID.into(),
            root.path().to_owned(),
            SessionConfigRef::BuiltinSnapshot { name: None },
            "fixture".into(),
        );
        state.phase = phase;
        if alive {
            state.set_runner_to_current_process();
        }
        manager
            .create(&state)
            .unwrap_or_else(|e| panic!("create fixture session: {e}"));
        let ledger = manager.sessions_dir().join(ID).join("instructions.jsonl");
        Self { root, ledger }
    }
    fn seed(&self, events: &[Value]) {
        let mut data = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        if !events.is_empty() {
            data.push('\n');
        }
        fs::write(&self.ledger, data).unwrap_or_else(|e| panic!("write fixture ledger: {e}"));
    }
    fn raw(&self) -> String {
        fs::read_to_string(&self.ledger).unwrap_or_default()
    }
    fn events(&self) -> Vec<Value> {
        self.raw()
            .lines()
            .map(|s| {
                serde_json::from_str(s)
                    .unwrap_or_else(|e| panic!("parse fixture ledger event: {e}"))
            })
            .collect()
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") {
                c.env_remove(key);
            }
        }
        c.current_dir(self.root.path())
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("NO_COLOR", "1")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run cruise CLI: {e}"))
    }
    fn tell(&self, text: &str) -> Output {
        self.run(&["tell", ID, text])
    }
}
fn opened() -> Value {
    json!({"kind":"run_opened","at":AT})
}
fn accepted(id: u64, content: &str, key: Option<&str>) -> Value {
    json!({"kind":"accepted","id":id,"at":AT,"origin":"cli","dedupe_key":key,"content":content})
}
fn error(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}
fn assert_rejected(out: &Output, expected: &str) {
    assert!(!out.status.success());
    assert!(
        error(out).contains(expected),
        "expected {expected:?}, got {}",
        error(out)
    );
}

#[test]
fn i1_i6_accept_trims_and_audits_and_empty_is_rejected() {
    let f = Fixture::new(SessionPhase::Running, true);
    f.seed(&[opened()]);
    let out = f.tell("  a  ");
    assert!(out.status.success(), "{}", error(&out));
    assert_eq!(f.events()[1]["content"], "a");
    let log = fs::read_to_string(
        f.ledger
            .parent()
            .unwrap_or_else(|| panic!("ledger has no parent directory"))
            .join("run.log"),
    )
    .unwrap_or_else(|e| panic!("read run log: {e}"));
    assert!(log.contains("tell: accepted #1 (origin=cli, 1 bytes)"));
    let before = f.raw();
    assert_rejected(&f.tell(" \n "), "instruction is empty");
    assert_eq!(f.raw(), before);
}

#[test]
fn i2_i3_i4_i5_rejections_leave_ledger_unchanged() {
    let f = Fixture::new(SessionPhase::Running, true);
    f.seed(&[]);
    let before = f.raw();
    assert_rejected(&f.tell("x"), "not running");
    assert_eq!(f.raw(), before);
    f.seed(&[
        opened(),
        json!({"kind":"run_closed","outcome":"completed","carried":[],"at":AT}),
    ]);
    let before = f.raw();
    assert_rejected(
        &f.tell("x"),
        &format!(
            "not running: run for session {ID} already finished; instructions are accepted only while a run is in progress"
        ),
    );
    assert_eq!(f.raw(), before);
    let f = Fixture::new(SessionPhase::Planned, true);
    f.seed(&[opened()]);
    let before = f.raw();
    assert_rejected(
        &f.tell("x"),
        &format!(
            "not running: session {ID} is in phase Planned; instructions are accepted only while a run is in progress"
        ),
    );
    assert_eq!(f.raw(), before);
    let f = Fixture::new(SessionPhase::Running, false);
    f.seed(&[opened()]);
    let before = f.raw();
    assert_rejected(
        &f.tell("x"),
        &format!("not running: runner process for session {ID} is not alive"),
    );
    assert_eq!(f.raw(), before);
}

#[test]
fn i7_byte_limit_counts_trimmed_utf8_bytes() {
    let f = Fixture::new(SessionPhase::Running, true);
    f.seed(&[opened()]);
    let valid = format!("  {}  ", "é".repeat(4096));
    let out = f.tell(&valid);
    assert!(out.status.success(), "{}", error(&out));
    assert_eq!(
        f.events()[1]["content"]
            .as_str()
            .unwrap_or_else(|| panic!("content is not a string"))
            .len(),
        8192
    );
    let before = f.raw();
    assert_rejected(
        &f.tell(&("é".repeat(4096) + "x")),
        "instruction is 8193 bytes; the limit is 8192 bytes",
    );
    assert_eq!(f.raw(), before);
}

#[test]
fn i8_count_and_byte_limits_include_leased_items() {
    let f = Fixture::new(SessionPhase::Running, true);
    let mut events = vec![opened()];
    for id in 1..=20 {
        events.push(accepted(id, "x", None));
    }
    events.push(json!({"kind":"leased","ids":[1],"step":"work","scope":"sequential","at":AT}));
    f.seed(&events);
    let before = f.raw();
    assert_rejected(
        &f.tell("x"),
        "20 instructions (20 bytes) are already pending; the limit is 20 instructions and 65536 bytes",
    );
    assert_eq!(f.raw(), before);
    let mut events = vec![opened()];
    for id in 1..=8 {
        events.push(accepted(id, &"x".repeat(8192), None));
    }
    f.seed(&events);
    let before = f.raw();
    assert_rejected(
        &f.tell("x"),
        "8 instructions (65536 bytes) are already pending; the limit is 20 instructions and 65536 bytes",
    );
    assert_eq!(f.raw(), before);
}

#[test]
fn i9_dedupe_precedes_run_and_payload_validation_even_after_delivery() {
    let f = Fixture::new(SessionPhase::Planned, false);
    f.seed(&[
        opened(),
        accepted(1, "original", Some("key")),
        json!({"kind":"leased","ids":[1],"step":"work","scope":"sequential","at":AT}),
        json!({"kind":"delivered","ids":[1],"step":"work","at":AT}),
        json!({"kind":"run_closed","outcome":"completed","carried":[],"at":AT}),
    ]);
    let before = f.raw();
    let out = f.run(&["tell", ID, "--dedupe-key", "key", " "]);
    assert!(out.status.success(), "{}", error(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains(&format!("instruction #1 was already accepted for {ID}."))
    );
    assert_eq!(f.raw(), before);
}

#[test]
fn i15_corrupt_middle_is_rejected_but_truncated_tail_can_be_read() {
    let f = Fixture::new(SessionPhase::Running, true);
    fs::write(
        &f.ledger,
        format!("{}\n{{bad\n{}\n", opened(), accepted(1, "x", None)),
    )
    .unwrap_or_else(|e| panic!("write corrupt ledger: {e}"));
    let before = f.raw();
    assert_rejected(&f.tell("y"), "instruction ledger is corrupt");
    assert_eq!(f.raw(), before);
    fs::write(
        &f.ledger,
        format!("{}\n{}\n{{\"kind\":", opened(), accepted(1, "x", None)),
    )
    .unwrap_or_else(|e| panic!("write truncated ledger: {e}"));
    let out = f.run(&["tell", ID, "--log"]);
    assert!(out.status.success(), "{}", error(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("#1"));
}

#[test]
fn i18_log_reflects_ledger_history_without_mutating_it() {
    let f = Fixture::new(SessionPhase::Running, true);
    f.seed(&[
        opened(),
        accepted(1, "first\nsecond", None),
        accepted(2, "delivered", None),
        json!({"kind":"leased","ids":[2],"step":"work","scope":"sequential","at":AT}),
        json!({"kind":"delivered","ids":[2],"step":"work","at":AT}),
        accepted(3, "expired", None),
        json!({"kind":"expired","ids":[3],"reason":"reset to planned","at":AT}),
    ]);
    let before = f.raw();
    let out = f.run(&["tell", ID, "--log"]);
    assert!(out.status.success(), "{}", error(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "#1",
        "pending",
        "first",
        "#2",
        "delivered@work",
        "#3",
        "expired(reset to planned)",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_eq!(f.raw(), before);
}
