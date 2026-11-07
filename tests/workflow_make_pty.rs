#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(20);
const YAML: &str = "command: [cat]\nsteps:\n  build:\n    command: echo hi\n";

struct Fixture {
    _root: TempDir,
    project: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let project = root.path().join("project");
        let home = root.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let response = root.path().join("response.txt");
        fs::write(
            &response,
            format!("Here is a draft.\n<cruise-workflow>\n{YAML}</cruise-workflow>\n"),
        )
        .unwrap();
        let backend = root.path().join("backend.sh");
        fs::write(
            &backend,
            format!("#!/bin/sh\ncat '{}'\n", response.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();
        let config = root.path().join("config.yaml");
        fs::write(
            &config,
            format!(
                "command: ['{}']\nsteps:\n  check:\n    command: echo check\n",
                backend.display()
            ),
        )
        .unwrap();
        Self {
            _root: root,
            project,
            home,
            config,
        }
    }

    fn start(&self) -> Terminal {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
        command.args(["workflow", "make", "--name", "ci", "--config"]);
        command.arg(&self.config);
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
        command.env("GIT_CONFIG_COUNT", "0");
        command.env("TERM", "xterm-256color");
        command.env("NO_COLOR", "1");
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                captured.lock().unwrap().extend_from_slice(&buf[..n]);
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

    fn destination(&self) -> PathBuf {
        self.project.join(".cruise/ci.yaml")
    }

    fn draft(&self) -> PathBuf {
        let dir = self.home.join("data/cruise/workflow-make");
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "expected one staging directory in {}",
            dir.display()
        );
        entries[0].join("ci.yaml")
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
    fn send(&mut self, text: &[u8]) {
        self.writer.write_all(text).unwrap();
        self.writer.flush().unwrap();
    }

    fn screen(&self) -> String {
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(40).unwrap(),
            NonZeroU16::new(160).unwrap(),
            1000,
        );
        parser.process(&self.output.lock().unwrap());
        parser.screen().contents()
    }

    fn answer_queries(&mut self) {
        let raw = String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned();
        for (i, (query, response)) in [
            ("\x1b[?u", "\x1b[?1u"),
            ("\x1b[c", "\x1b[?1;2c"),
            ("\x1b[6n", "\x1b[1;1R"),
        ]
        .iter()
        .enumerate()
        {
            let count = raw.matches(query).count();
            while self.answered[i] < count {
                self.send(response.as_bytes());
                self.answered[i] += 1;
            }
        }
    }

    fn wait_for(&mut self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.answer_queries();
            let screen = self.screen();
            if screen.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; screen:\n{screen}\nraw:\n{}",
                String::from_utf8_lossy(&self.output.lock().unwrap())
            );
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("process exited {status:?} before {needle:?}; screen:\n{screen}");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.answer_queries();
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "exit {status:?}; screen:\n{}",
                    self.screen()
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for exit; screen:\n{}",
                self.screen()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn request_draft(terminal: &mut Terminal) {
    // Allow the line editor to initialize and respond to terminal capability queries.
    thread::sleep(Duration::from_millis(300));
    terminal.answer_queries();
    terminal.send(b"make a build workflow\r");
    terminal.wait_for("Draft updated:");
}

#[test]
fn write_approved_draft() {
    let fixture = Fixture::new();
    let mut terminal = fixture.start();
    request_draft(&mut terminal);
    assert!(!fixture.destination().exists());
    terminal.send(b"/accept\r");
    terminal.wait_for("Write workflow");
    terminal.send(b"\r");
    terminal.finish();
    assert_eq!(
        fs::read(fixture.destination()).unwrap(),
        fs::read(fixture.draft()).unwrap()
    );
    let screen = terminal.screen();
    assert!(screen.contains("Created"), "{screen}");
    assert!(
        screen.contains(&fixture.destination().display().to_string()),
        "{screen}"
    );
}

#[test]
fn cancel_approval_preserves_staging_without_writing() {
    let fixture = Fixture::new();
    let mut terminal = fixture.start();
    request_draft(&mut terminal);
    terminal.send(b"/accept\r");
    terminal.wait_for("Cancel");
    terminal.send(b"\x1b[B\x1b[B\r");
    terminal.finish();
    assert!(!fixture.destination().exists());
    assert!(fixture.draft().is_file());
}

#[test]
fn escape_before_request_cancels_without_writing() {
    let fixture = Fixture::new();
    let mut terminal = fixture.start();
    thread::sleep(Duration::from_millis(300));
    terminal.answer_queries();
    terminal.send(b"\x1b");
    terminal.finish();
    assert!(!fixture.destination().exists());
}
