#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    repo: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|err| panic!("create fixture directory: {err}"));
        let repo = root.path().join("repo");
        let home = root.path().join("home");
        fs::create_dir_all(&repo).unwrap_or_else(|err| panic!("create repository: {err}"));
        fs::create_dir_all(&home).unwrap_or_else(|err| panic!("create home: {err}"));
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repo)
                .status()
                .unwrap_or_else(|err| panic!("run git init: {err}"))
                .success()
        );
        Self {
            _root: root,
            repo,
            home,
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
            .unwrap_or_else(|err| panic!("open PTY: {err}"));
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            cmd.env_remove(name);
        }
        cmd.env("PATH", std::env::var_os("PATH").unwrap_or_default());
        cmd.env("HOME", &self.home);
        cmd.env("XDG_CONFIG_HOME", self.home.join("config"));
        cmd.env("XDG_DATA_HOME", self.home.join("data"));
        cmd.env("XDG_STATE_HOME", self.home.join("state"));
        cmd.env("JCODE_HOME", self.home.join("jcode"));
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("TERM", "xterm-256color");
        cmd.env("LANG", "en_US.UTF-8");
        cmd.cwd(&self.repo);
        cmd.arg("manager");
        let child = pair
            .slave
            .spawn_command(cmd)
            .unwrap_or_else(|err| panic!("spawn manager: {err}"));
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .unwrap_or_else(|err| panic!("clone PTY reader: {err}"));
        let writer = pair
            .master
            .take_writer()
            .unwrap_or_else(|err| panic!("take PTY writer: {err}"));
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
                    .unwrap_or_else(|err| panic!("lock captured output: {err}"))
                    .extend_from_slice(&buf[..n]);
            }
        });
        Terminal {
            _master: pair.master,
            child,
            writer,
            output,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_cruise"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("JCODE_HOME", self.home.join("jcode"))
            .current_dir(&self.repo)
            .args(["daemon", "stop", "--force"])
            .output();
    }
}

struct Terminal {
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Terminal {
    fn wait_for(&mut self, needle: &str, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let text = String::from_utf8_lossy(
                &self
                    .output
                    .lock()
                    .unwrap_or_else(|err| panic!("lock terminal output: {err}")),
            )
            .into_owned();
            if text.matches(needle).count() >= count {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}: {text}"
            );
            assert!(
                self.child
                    .try_wait()
                    .unwrap_or_else(|err| panic!("poll manager: {err}"))
                    .is_none(),
                "manager exited before {needle:?}: {text}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn send(&mut self, bytes: &[u8]) {
        self.writer
            .write_all(bytes)
            .unwrap_or_else(|err| panic!("write to manager: {err}"));
        self.writer
            .flush()
            .unwrap_or_else(|err| panic!("flush manager input: {err}"));
    }
    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|err| panic!("poll manager: {err}"))
            {
                assert!(status.success(), "manager exit: {status:?}");
                return;
            }
            assert!(Instant::now() < deadline, "manager did not exit");
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

#[test]
fn manager_terminal_disconnect_and_reconnect() {
    let fixture = Fixture::new();
    for _ in 0..2 {
        let mut terminal = fixture.start();
        terminal.wait_for("/goals", 1);
        terminal.wait_for("> ", 1);
        terminal.send(b"/help\r");
        terminal.wait_for("/approve", 1);
        terminal.wait_for("> ", 2);
        terminal.send(b"/goals\r");
        terminal.wait_for("> ", 3);
        terminal.send(b"/exit\r");
        terminal.finish();
    }
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&fixture.repo)
        .output()
        .unwrap_or_else(|err| panic!("run git status: {err}"));
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "manager interaction changed repository: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}
