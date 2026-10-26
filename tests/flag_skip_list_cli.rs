#![cfg(any(target_os = "macos", target_os = "linux"))]
//! Observable contract for flag-dependent skip choices in `cruise list` settings.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use tempfile::TempDir;

const WORKFLOW: &str = "command: [echo]
flags:
  e2e: {}
  docs: {}
steps:
  build:
    command: echo build
  e2e-test:
    flag: e2e
    command: echo e2e
  docs-test:
    flag: docs
    command: echo docs
after-pr:
  e2e-publish:
    flag: e2e
    command: echo publish
  docs-publish:
    flag: docs
    command: echo docs
";

#[test]
fn list_settings_select_flags_before_filtering_skips_but_keep_all_resume_steps() {
    // A plain temporary directory is enough: editing settings must not run a
    // workflow, create branches, or require fixture git commits.
    let root = TempDir::new().unwrap_or_else(|error| panic!("temporary fixture: {error}"));
    let config = root.path().join("workflow.yaml");
    std::fs::write(&config, WORKFLOW).unwrap_or_else(|error| panic!("write workflow: {error}"));
    let manager = SessionManager::new(root.path().join("data/cruise"));
    let id = "20261025000000";
    let mut session = SessionState::new(
        id.into(),
        root.path().to_path_buf(),
        SessionConfigRef::File { path: config },
        "list settings flag filtering".into(),
    );
    session.phase = SessionPhase::Suspended;
    session.flags.insert("e2e".into(), false);
    session.flags.insert("docs".into(), true);
    manager
        .create(&session)
        .unwrap_or_else(|error| panic!("create session: {error}"));

    let (success, text) = list_in_pty(
        &root,
        &[
            ("Select a session:", b"\r"),
            ("Edit Settings", b"\x1b[B\r"),
            // Change both saved values. Filtering must use these new values,
            // not the session's previously persisted flag map.
            ("Flag 'e2e':", b"\x1b[B\r"),
            ("Flag 'docs':", b"\x1b[B\x1b[B\r"),
            ("Steps to skip:", b"\x1b[B \r"),
            // Keep + clear + build + e2e-test + docs-test. The disabled docs
            // step must still be a selectable resume position.
            ("Resume from step:", b"\x1b[B\x1b[B\x1b[B\x1b[B\r"),
            // The save diagnostic is not an input prompt. Wait for the action
            // menu to reopen before sending Back, otherwise inquire may discard
            // an Escape received while it is initializing terminal input.
            ("Action:", b"\x1b"),
            ("Select a session:", b"\x1b"),
        ],
    );
    assert!(success, "{text}");
    let e2e = text
        .find("Flag 'e2e':")
        .unwrap_or_else(|| panic!("missing e2e flag prompt: {text}"));
    let docs = text
        .find("Flag 'docs':")
        .unwrap_or_else(|| panic!("missing docs flag prompt: {text}"));
    let skips = text
        .find("Steps to skip:")
        .unwrap_or_else(|| panic!("missing skip prompt: {text}"));
    let resume = text
        .find("Resume from step:")
        .unwrap_or_else(|| panic!("missing resume prompt: {text}"));
    assert!(e2e < docs && docs < skips && skips < resume, "{text}");
    let skip_output = &text[skips..resume];
    for visible in ["build", "e2e-test", "[after-pr] e2e-publish"] {
        assert!(skip_output.contains(visible), "missing {visible}: {text}");
    }
    for hidden in ["docs-test", "docs-publish"] {
        assert!(!skip_output.contains(hidden), "unexpected {hidden}: {text}");
    }
    let updated = text
        .find("Settings updated.")
        .unwrap_or_else(|| panic!("settings not saved: {text}"));
    let resume_output = &text[resume..updated];
    assert!(resume_output.contains("docs-test"), "{text}");
    assert!(!resume_output.contains("e2e-publish"), "{text}");
    let saved = manager
        .load(id)
        .unwrap_or_else(|error| panic!("load edited session: {error}"));
    assert_eq!(saved.flags.get("e2e"), Some(&true), "{text}");
    assert_eq!(saved.flags.get("docs"), Some(&false), "{text}");
    assert_eq!(saved.skipped_steps, vec!["e2e-test"], "{text}");
    assert_eq!(saved.current_step.as_deref(), Some("docs-test"), "{text}");
    assert_eq!(saved.phase, SessionPhase::Suspended);
}

// Follow flags_cli.rs's script-based PTY conventions, including cursor query
// responses and process-group cleanup on timeout or assertion failure.
struct PtyChild(Child);

impl Drop for PtyChild {
    fn drop(&mut self) {
        let _ = Command::new("pkill")
            .args(["-KILL", "-P", &self.0.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", self.0.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_stream(mut stream: impl Read + Send + 'static, tx: mpsc::SyncSender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut bytes = [0; 4096];
        loop {
            match stream.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(bytes[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn spawn_list_pty(root: &TempDir) -> PtyChild {
    let mut command = Command::new("script");
    #[cfg(target_os = "macos")]
    command.args(["-q", "/dev/null", env!("CARGO_BIN_EXE_cruise"), "list"]);
    #[cfg(target_os = "linux")]
    {
        let binary = env!("CARGO_BIN_EXE_cruise").replace('\'', "'\\''");
        command.args(["-q", "-e", "-c", &format!("'{binary}' list"), "/dev/null"]);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR" {
            command.env_remove(name);
        }
    }
    PtyChild(
        command
            .current_dir(root.path())
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_STATE_HOME", root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("spawn PTY: {error}")),
    )
}

fn list_in_pty(root: &TempDir, responses: &[(&str, &[u8])]) -> (bool, String) {
    let mut child = spawn_list_pty(root);
    let mut input = child
        .0
        .stdin
        .take()
        .unwrap_or_else(|| panic!("missing PTY input"));
    let (tx, rx) = mpsc::sync_channel(32);
    read_stream(
        child
            .0
            .stdout
            .take()
            .unwrap_or_else(|| panic!("missing PTY stdout")),
        tx.clone(),
    );
    read_stream(
        child
            .0
            .stderr
            .take()
            .unwrap_or_else(|| panic!("missing PTY stderr")),
        tx,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut transcript = Vec::new();
    let mut response_index = 0;
    let mut response_start = 0;
    let mut cursor_queries = 0;
    let mut status = None;
    loop {
        assert!(
            Instant::now() < deadline,
            "PTY timed out awaiting response {response_index}: {}",
            String::from_utf8_lossy(&transcript)
        );
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(bytes) => transcript.extend(bytes),
            Err(mpsc::RecvTimeoutError::Disconnected) if status.is_some() => break,
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
        }
        assert!(
            transcript.len() < 2 * 1024 * 1024,
            "PTY transcript exceeded 2 MiB"
        );
        let text = String::from_utf8_lossy(&transcript);
        let queries = text.matches("\x1b[6n").count();
        for _ in cursor_queries..queries {
            input
                .write_all(b"\x1b[1;1R")
                .unwrap_or_else(|error| panic!("cursor response: {error}"));
        }
        cursor_queries = queries;
        // Only match output produced since the preceding response. This is
        // essential when returning to the session picker at the end.
        if let Some((marker, response)) = responses.get(response_index)
            && transcript[response_start..]
                .windows(marker.len())
                .any(|bytes| bytes == marker.as_bytes())
        {
            input
                .write_all(response)
                .unwrap_or_else(|error| panic!("prompt response: {error}"));
            input
                .flush()
                .unwrap_or_else(|error| panic!("flush response: {error}"));
            response_start = transcript.len();
            response_index += 1;
        }
        if status.is_none() {
            status = child
                .0
                .try_wait()
                .unwrap_or_else(|error| panic!("poll PTY: {error}"));
        }
    }
    let text = String::from_utf8_lossy(&transcript).into_owned();
    assert_eq!(
        response_index,
        responses.len(),
        "not all prompts reached: {text}"
    );
    (status.is_some_and(|status| status.success()), text)
}
