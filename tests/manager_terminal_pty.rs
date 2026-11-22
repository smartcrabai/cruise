#![cfg(any(target_os = "macos", target_os = "linux"))]

#[path = "support/manager_cli.rs"]
mod support;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use support::Fixture;

const CONFIRM: &str = "Type approve to authorize this exact proposal";

struct Terminal {
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Terminal {
    fn spawn(fixture: &Fixture, args: &[&str]) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap_or_else(|err| panic!("open PTY: {err}"));
        let isolated = fixture.command();
        let mut cmd = CommandBuilder::new(isolated.get_program());
        cmd.env_clear();
        for (name, value) in isolated.get_envs() {
            if let Some(value) = value {
                cmd.env(name, value);
            }
        }
        cmd.env("TERM", "xterm-256color");
        cmd.cwd(&fixture.repo);
        cmd.arg("manager");
        cmd.args(args);
        let child = pair
            .slave
            .spawn_command(cmd)
            .unwrap_or_else(|err| panic!("spawn manager approve: {err}"));
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
                captured.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Self {
            _master: pair.master,
            child,
            writer,
            output,
        }
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }

    fn status(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child
            .try_wait()
            .unwrap_or_else(|err| panic!("poll manager approve: {err}"))
    }

    fn wait_for_review(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let text = self.output();
            if let Some((review, prompt)) = text.split_once(CONFIRM) {
                // Wait for the line editor, not just the message printed before it enters raw mode.
                if prompt.contains("> ") {
                    return review.to_owned();
                }
            }
            assert!(
                self.status().is_none() && Instant::now() < deadline,
                "manager approve never reached the confirmation prompt: {text}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer
            .write_all(bytes)
            .unwrap_or_else(|err| panic!("write to manager approve: {err}"));
        self.writer.flush().unwrap();
    }

    fn finish(&mut self) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.status() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "manager approve did not exit: {}",
                self.output()
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

fn assert_pending(fixture: &Fixture, reviewed: &cruise::manager::Proposal) {
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot["proposal"]["id"], reviewed.id);
    assert_eq!(snapshot["proposal"]["revision"], reviewed.revision);
    assert_eq!(snapshot["goals"], serde_json::json!([]));
}

#[test]
fn manager_terminal_requires_exact_confirmation() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_proposal();

    let mut declined = Terminal::spawn(&fixture, &["approve"]);
    declined.wait_for_review();
    declined.send(b"Approve\r");
    declined.finish();
    assert_pending(&fixture, &proposal);

    let mut approved = Terminal::spawn(&fixture, &["approve"]);
    let review = approved.wait_for_review();
    for expected in [&proposal.id, &proposal.objective, &proposal.checks[0].id] {
        assert!(review.contains(expected), "missing {expected:?}: {review}");
    }
    assert!(
        review.contains("\"contract\""),
        "missing frozen contract: {review}"
    );
    assert_pending(&fixture, &proposal);
    approved.send(b"approve\r");
    let status = approved.finish();
    assert!(status.success(), "approval failed: {}", approved.output());

    let snapshot = fixture.snapshot();
    let goals = snapshot["goals"].as_array().unwrap();
    assert_eq!(goals.len(), 1, "{snapshot}");
    assert_eq!(goals[0]["proposal"]["id"], proposal.id);
    assert_eq!(goals[0]["proposal"]["revision"], proposal.revision);
    assert!(snapshot["proposal"].is_null(), "{snapshot}");
}

#[test]
fn manager_terminal_rejects_revision_replaced_during_confirmation() {
    let fixture = Fixture::new();
    let reviewed = fixture.seed_proposal();
    let mut terminal = Terminal::spawn(&fixture, &["approve", "--revision", &reviewed.revision.to_string()]);
    assert!(terminal.wait_for_review().contains(&reviewed.id));

    let replacement = fixture.seed_proposal();
    assert!(replacement.revision > reviewed.revision);
    assert_pending(&fixture, &replacement);
    terminal.send(b"approve\r");
    let status = terminal.finish();
    assert!(
        !status.success(),
        "stale approval succeeded: {}",
        terminal.output()
    );
    assert_pending(&fixture, &replacement);
}

#[tokio::test]
async fn manager_terminal_option_requires_the_menu() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_option_proposal();
    let goal = fixture.api(Some(serde_json::json!({
        "type":"approve", "proposal_id":proposal.id, "revision":proposal.revision
    }))).await;
    let goal_id = goal["id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let request = loop {
        let snapshot = fixture.api(None).await;
        if let Some(prompt) = snapshot["prompts"].as_array().unwrap().iter()
            .find(|entry| entry["goal_id"] == goal_id && entry["prompt"]["kind"] == "option")
        {
            break prompt["prompt"]["requestId"].as_str().unwrap().to_owned();
        }
        assert!(Instant::now() < deadline, "missing Option: {snapshot}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let mut scripted = Terminal::spawn(&fixture, &[
        "answer", goal_id, &request, "--choice", "Provide a value", "--text", "value",
    ]);
    assert!(!scripted.finish().success());

    let mut menu = Terminal::spawn(&fixture, &["answer", goal_id, &request]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !menu.output().contains("Provide a value") {
        assert!(menu.status().is_none() && Instant::now() < deadline, "missing menu: {}", menu.output());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
