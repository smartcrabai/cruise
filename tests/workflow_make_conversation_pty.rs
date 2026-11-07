#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::TempDir;

const VALID: &str = "command: [cat]\nsteps:\n  build:\n    command: echo hi\n";
const INVALID: &str = "steps: [\n";

struct Fixture {
    root: TempDir,
    project: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new(responses: &[String]) -> Self {
        let root =
            TempDir::new().unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        let project = root.path().join("project");
        let home = root.path().join("home");
        fs::create_dir_all(&project)
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        fs::create_dir_all(&home)
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        for (i, response) in responses.iter().enumerate() {
            fs::write(root.path().join(format!("response-{}", i + 1)), response)
                .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        }
        let backend = root.path().join("backend.sh");
        fs::write(
            &backend,
            format!(
                "#!/bin/sh\ndir='{}'\nn=0\n[ ! -f \"$dir/count\" ] || n=$(cat \"$dir/count\")\nn=$((n + 1))\nprintf '%s' \"$n\" > \"$dir/count\"\ncat > \"$dir/prompt-$n\"\ncat \"$dir/response-$n\"\n",
                root.path().display()
            ),
        ).unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        fs::set_permissions(&backend, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        let config = root.path().join("config.yaml");
        fs::write(
            &config,
            format!(
                "command: ['{}']\nsteps:\n  check:\n    command: echo check\n",
                backend.display()
            ),
        )
        .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        Self {
            root,
            project,
            home,
            config,
        }
    }

    fn start(&self, extra: &[&str]) -> Terminal {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
        command.args(["workflow", "make", "--name", "ci", "--config"]);
        command.arg(&self.config);
        command.args(extra);
        command.cwd(&self.project);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command.env("HOME", &self.home);
        command.env("XDG_CONFIG_HOME", self.home.join("config"));
        command.env("XDG_DATA_HOME", self.home.join("data"));
        command.env("XDG_STATE_HOME", self.home.join("state"));
        command.env("TERM", "xterm-256color");
        command.env("NO_COLOR", "1");
        let child = pair
            .slave
            .spawn_command(command)
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        let writer = pair
            .master
            .take_writer()
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                captured
                    .lock()
                    .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"))
                    .extend_from_slice(&buf[..n]);
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

    fn draft(&self) -> Option<PathBuf> {
        let dir = self.home.join("data/cruise/workflow-make");
        fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path().join("ci.yaml"))
            .find(|p| p.is_file())
    }

    fn prompt(&self, n: usize) -> String {
        fs::read_to_string(self.root.path().join(format!("prompt-{n}")))
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"))
    }

    fn wait_for_event(&self, terminal: &mut Terminal, event: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            terminal.answer_queries();
            let dir = self.home.join("data/cruise/workflow-make");
            let found = fs::read_dir(dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|entry| {
                    fs::read_to_string(entry.path().join("conversation.jsonl"))
                        .unwrap_or_default()
                        .lines()
                        .any(|line| {
                            serde_json::from_str::<serde_json::Value>(line)
                                .is_ok_and(|value| value["event"] == event)
                        })
                });
            if found {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {event}:\n{}",
                terminal.raw()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

struct Terminal {
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    answered: [usize; 3],
}

impl Terminal {
    fn raw(&self) -> String {
        String::from_utf8_lossy(
            &self
                .output
                .lock()
                .unwrap_or_else(|error| panic!("PTY test operation failed: {error}")),
        )
        .into_owned()
    }
    fn send(&mut self, text: &[u8]) {
        self.writer
            .write_all(text)
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
        self.writer
            .flush()
            .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
    }
    fn answer_queries(&mut self) {
        let raw = self.raw();
        for (i, (query, response)) in [
            ("\x1b[?u", "\x1b[?1u"),
            ("\x1b[c", "\x1b[?1;2c"),
            ("\x1b[6n", "\x1b[1;1R"),
        ]
        .iter()
        .enumerate()
        {
            while self.answered[i] < raw.matches(query).count() {
                self.send(response.as_bytes());
                self.answered[i] += 1;
            }
        }
    }
    fn wait_for(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.answer_queries();
            let raw = self.raw();
            if raw.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}:\n{raw}"
            );
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"))
            {
                panic!("exited {status:?} waiting for {needle:?}:\n{raw}");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn request(&mut self, text: &str) {
        thread::sleep(Duration::from_millis(300));
        self.answer_queries();
        self.send(format!("{text}\r").as_bytes());
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn candidate(yaml: &str) -> String {
    format!("<cruise-workflow>\n{yaml}</cruise-workflow>\n")
}
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("PTY test operation failed: {error}"))
}

#[test]
fn repairs_invalid_candidate_before_staging_valid_workflow() {
    let fixture = Fixture::new(&[candidate(INVALID), candidate(VALID)]);
    let mut terminal = fixture.start(&[]);
    terminal.request("build the project");
    terminal.wait_for("Draft updated:");
    assert_eq!(
        read(
            &fixture
                .draft()
                .unwrap_or_else(|| panic!("expected a staged draft"))
        )
        .trim(),
        VALID.trim()
    );
    let repair = fixture.prompt(2);
    assert!(
        repair.contains("steps: ["),
        "repair omitted invalid candidate: {repair}"
    );
    assert!(
        repair.contains("## Previous attempt"),
        "missing repair context: {repair}"
    );
    assert!(!fixture.project.join(".cruise/ci.yaml").exists());
}

#[test]
fn question_does_not_create_a_draft() {
    let fixture = Fixture::new(&["Which build system should I use?\n".into()]);
    let mut terminal = fixture.start(&[]);
    terminal.request("make a workflow");
    terminal.wait_for("Which build system should I use?");
    fixture.wait_for_event(&mut terminal, "message");
    assert!(fixture.draft().is_none());
    assert!(!fixture.project.join(".cruise/ci.yaml").exists());
}

#[test]
fn repeated_invalid_candidates_preserve_previous_draft() {
    let mut responses = vec![candidate(VALID)];
    responses.extend((0..4).map(|_| candidate(INVALID)));
    let fixture = Fixture::new(&responses);
    let mut terminal = fixture.start(&[]);
    terminal.request("create a workflow");
    terminal.wait_for("Draft updated:");
    let draft = fixture
        .draft()
        .unwrap_or_else(|| panic!("expected a staged draft"));
    let before =
        fs::read(&draft).unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
    terminal.request("change the workflow");
    fixture.wait_for_event(&mut terminal, "rejected");
    assert_eq!(
        fs::read(&draft).unwrap_or_else(|error| panic!("PTY test operation failed: {error}")),
        before
    );
    assert_eq!(read(&fixture.root.path().join("count")), "5");
    assert!(fixture.prompt(5).contains("steps: ["));
    assert!(!fixture.project.join(".cruise/ci.yaml").exists());
}

#[test]
fn reference_is_included_without_modifying_the_source() {
    let fixture = Fixture::new(&[candidate(VALID)]);
    let reference = fixture.root.path().join("reference.yaml");
    let original = "command: [cat]\nsteps:\n  reference_marker:\n    command: echo original\n";
    fs::write(&reference, original)
        .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
    let mut terminal = fixture.start(&[
        "--from",
        reference
            .to_str()
            .unwrap_or_else(|| panic!("reference path is not UTF-8")),
    ]);
    terminal.request("adapt this workflow");
    terminal.wait_for("Draft updated:");
    assert!(fixture.prompt(1).contains("reference_marker"));
    assert_eq!(read(&reference), original);
    assert!(!fixture.project.join(".cruise/ci.yaml").exists());
}

#[test]
fn second_turn_contains_prior_request_message_and_draft() {
    let fixture = Fixture::new(&[
        format!("First response marker\n{}", candidate(VALID)),
        "Second response\n".into(),
    ]);
    let mut terminal = fixture.start(&[]);
    terminal.request("first request marker");
    terminal.wait_for("Draft updated:");
    terminal.request("second request marker");
    terminal.wait_for("Second response");
    let prompt = fixture.prompt(2);
    assert!(prompt.contains("User: first request marker"));
    assert!(prompt.contains("Assistant: First response marker"));
    assert!(prompt.contains(&format!("## Current draft\n{}", VALID.trim())));
    assert!(prompt.contains("second request marker"));
}

#[test]
fn existing_destination_is_rejected_before_staging_or_backend() {
    let fixture = Fixture::new(&[candidate(VALID)]);
    let dest = fixture.project.join(".cruise/ci.yaml");
    fs::create_dir_all(
        dest.parent()
            .unwrap_or_else(|| panic!("destination must have a parent")),
    )
    .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
    fs::write(&dest, "existing")
        .unwrap_or_else(|error| panic!("PTY test operation failed: {error}"));
    let mut terminal = fixture.start(&[]);
    terminal.wait_for("workflow already exists");
    assert_eq!(read(&dest), "existing");
    assert!(!fixture.home.join("data/cruise/workflow-make").exists());
    assert!(!fixture.root.path().join("count").exists());
}

#[test]
fn user_flag_writes_user_destination() {
    let fixture = Fixture::new(&[candidate(VALID)]);
    let mut terminal = fixture.start(&["--user"]);
    terminal.request("create workflow");
    terminal.wait_for("Draft updated:");
    terminal.send(b"/accept\r");
    terminal.wait_for("Write workflow");
    terminal.send(b"\r");
    terminal.wait_for("Created ");
    let dest = fixture.home.join("config/cruise/workflows/ci.yaml");
    assert_eq!(read(&dest).trim(), VALID.trim());
    assert!(!fixture.project.join(".cruise/ci.yaml").exists());
}
