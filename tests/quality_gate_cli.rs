#![cfg(unix)]

//! End-to-end quality gate contracts through the real CLI and a fake agent.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    repo: PathBuf,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
}

impl Fixture {
    fn new(gate: &str, retries: usize, extra: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let home = root.path().join("home");
        let data = root.path().join("data");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
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
        );
        std::fs::write(home.join("agent.sh"), "echo attempt >> \"$HOME/attempts\"\nn=$(wc -l < \"$HOME/attempts\" | tr -d ' ')\ncat > \"$HOME/prompt-$n\"\npath=$(awk '/^- Sanitized log \\(may be truncated\\): / { sub(/^- Sanitized log \\(may be truncated\\): /, \"\"); print; exit }' \"$HOME/prompt-$n\")\nif [ -n \"$path\" ] && [ -f \"$path\" ]; then cp \"$path\" \"$HOME/gate-log-$n\"; fi\necho done\n").unwrap();
        std::fs::write(home.join("gate.sh"), gate).unwrap();
        let config = root.path().join("workflow.yaml");
        // The command array is executed directly, not by a shell.
        let yaml = format!("command: [sh, \"$HOME/agent.sh\"]\nmax_retries: {retries}\nsteps:\n  implement:\n    prompt: implement the task\n    quality_gates:\n      - command: 'sh \"$HOME/gate.sh\"'\n{extra}").replace(
            "$HOME/agent.sh",
            &home.join("agent.sh").display().to_string(),
        );
        std::fs::write(&config, yaml).unwrap();
        Self {
            _root: root,
            repo,
            home,
            config,
            data,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self._root.path().join("config"))
            .env("XDG_DATA_HOME", &self.data)
            .env("XDG_STATE_HOME", self._root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .args(["exec", "--config"])
            .arg(&self.config)
            .arg("task");
        cmd
    }

    fn exec(&self) -> Output {
        self.command().output().unwrap()
    }

    fn attempts(&self) -> usize {
        std::fs::read_to_string(self.home.join("attempts"))
            .unwrap_or_default()
            .lines()
            .count()
    }
    fn prompt(&self, n: usize) -> String {
        std::fs::read_to_string(self.home.join(format!("prompt-{n}"))).unwrap()
    }
}
fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn passing_gate_runs_once() {
    let f = Fixture::new("exit 0\n", 2, "");
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 1);
    assert!(!text(&out).to_lowercase().contains("quality gate failed"));
}

#[test]
fn failure_retries_with_feedback_and_clears_it_for_next_step() {
    let f = Fixture::new(
        "if [ ! -f \"$HOME/gate-ran\" ]; then touch \"$HOME/gate-ran\"; echo gate-diagnostic; exit 1; fi\n",
        2,
        "  next-step:\n    prompt: subsequent step\n",
    );
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 3);
    let retry = f.prompt(2);
    assert!(retry.contains("## Quality gate failed"), "{retry}");
    assert!(retry.contains("exit code 1"), "{retry}");
    assert!(retry.contains("gate-diagnostic"), "{retry}");
    assert!(!f.prompt(3).contains("## Quality gate failed"));
}

#[test]
fn exhaustion_stops_without_running_the_next_step() {
    let f = Fixture::new(
        "echo broken; exit 1\n",
        2,
        "  later:\n    command: 'touch later-ran'\n",
    );
    let out = f.exec();
    assert!(!out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 3, "{}", text(&out));
    assert!(!f.repo.join("later-ran").exists());
    let message = text(&out).to_lowercase();
    assert!(
        message.contains("quality gate") && message.contains("implement"),
        "{message}"
    );
}

#[test]
fn zero_budget_fails_without_retry() {
    let f = Fixture::new("exit 1\n", 0, "");
    let out = f.exec();
    assert!(!out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 1, "{}", text(&out));
    assert!(text(&out).to_lowercase().contains("quality gate"));
}

#[test]
fn exhausted_gate_uses_fail_goto() {
    let f = Fixture::new(
        "exit 1\n",
        2,
        "    if:\n      fail: recover\n  recover:\n    command: 'touch recovered'\n",
    );
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 3);
    assert!(f.repo.join("recovered").exists());
}

#[test]
fn feedback_masks_secrets() {
    let f = Fixture::new(
        "if [ ! -f \"$HOME/gate-ran\" ]; then touch \"$HOME/gate-ran\"; echo 'password=hunter2'; echo 'Authorization: Bearer abc.def'; exit 1; fi\n",
        1,
        "",
    );
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    let retry = f.prompt(2);
    assert!(retry.contains("[REDACTED]"), "{retry}");
    assert!(
        !retry.contains("hunter2") && !retry.contains("abc.def"),
        "{retry}"
    );
}

#[test]
fn large_output_preserves_both_ends_in_feedback_and_artifact() {
    let f = Fixture::new(
        "if [ ! -f \"$HOME/gate-ran\" ]; then touch \"$HOME/gate-ran\"; echo BEGIN_MARKER; echo 'password=hunter2'; head -c 5000000 /dev/zero | tr '\\0' x; echo; echo END_MARKER; exit 1; fi\n",
        1,
        "",
    );
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    let retry = f.prompt(2);
    let path = retry
        .lines()
        .find_map(|line| line.strip_prefix("- Sanitized log (may be truncated): "))
        .unwrap();
    let path = Path::new(path);
    assert!(!path.starts_with(&f.repo));
    let log = std::fs::read_to_string(f.home.join("gate-log-2")).unwrap();
    assert!(log.len() <= 1024 * 1024);
    for output in [&retry, &log] {
        assert!(output.contains("BEGIN_MARKER") && output.contains("END_MARKER"));
        assert!(output.contains("omitted"));
        assert!(!output.contains("hunter2"));
    }
    assert!(
        String::from_utf8_lossy(
            &Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(&f.repo)
                .output()
                .unwrap()
                .stdout
        )
        .is_empty()
    );
}

#[test]
fn omitted_private_key_context_is_redacted_in_feedback_and_artifact() {
    let f = Fixture::new(
        "if [ ! -f \"$HOME/gate-ran\" ]; then touch \"$HOME/gate-ran\"; head -c 2500000 /dev/zero | tr '\\0' x; echo; echo '-----BEGIN PRIVATE KEY-----'; head -c 2500000 /dev/zero | tr '\\0' y; echo KEY_BODY_MUST_NOT_ESCAPE; echo '-----END PRIVATE KEY-----'; echo FINAL_SAFE_DIAGNOSTIC; exit 1; fi\n",
        1,
        "",
    );
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    let retry = f.prompt(2);
    let log = std::fs::read_to_string(f.home.join("gate-log-2")).unwrap();
    for output in [&retry, &log] {
        assert!(!output.contains("KEY_BODY_MUST_NOT_ESCAPE"));
        assert!(output.contains("FINAL_SAFE_DIAGNOSTIC"));
    }
}

#[test]
fn seconds_timeout_kills_gate_and_reports_feedback() {
    let f = Fixture::new(
        "if [ ! -f \"$HOME/gate-ran\" ]; then touch \"$HOME/gate-ran\"; sleep 5; touch \"$HOME/leaked\"; else exit 0; fi\n",
        1,
        "",
    );
    let yaml = std::fs::read_to_string(&f.config).unwrap().replace(
        "      - command: 'sh \"$HOME/gate.sh\"'",
        "      - command: 'sh \"$HOME/gate.sh\"'\n        timeout: '1s'",
    );
    std::fs::write(&f.config, yaml).unwrap();
    let start = std::time::Instant::now();
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    assert!(start.elapsed() < std::time::Duration::from_secs(4));
    assert!(f.prompt(2).contains("timed out after 1s"));
    std::thread::sleep(std::time::Duration::from_secs(5));
    assert!(!f.home.join("leaked").exists());
}

#[test]
fn no_changes_retries_before_running_gate() {
    let f = Fixture::new(
        "echo gate >> \"$HOME/gates\"\n",
        2,
        "    if:\n      no-file-changes: retry\n",
    );
    std::fs::write(f.home.join("agent.sh"), "echo attempt >> \"$HOME/attempts\"\nn=$(wc -l < \"$HOME/attempts\" | tr -d ' ')\ncat > \"$HOME/prompt-$n\"\nif [ \"$n\" -gt 1 ]; then touch changed; fi\necho done\n").unwrap();
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 2);
    assert_eq!(
        std::fs::read_to_string(f.home.join("gates"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn failed_gate_suppresses_file_changed_route() {
    let f = Fixture::new(
        "if [ ! -f \"$HOME/gate-ran\" ]; then touch \"$HOME/gate-ran\"; exit 1; fi\n",
        1,
        "    if:\n      file-changed: other\n  other:\n    command: 'touch routed'\n",
    );
    std::fs::write(f.home.join("agent.sh"), "echo attempt >> \"$HOME/attempts\"\nn=$(wc -l < \"$HOME/attempts\" | tr -d ' ')\ncat > \"$HOME/prompt-$n\"\necho \"$n\" >> changed\necho done\n").unwrap();
    let out = f.exec();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 2);
    assert!(f.prompt(2).contains("Quality gate failed"));
    assert!(f.repo.join("routed").exists());
}

#[test]
fn cancellation_cleans_up_gate_process_group() {
    let f = Fixture::new(
        "echo $$ > \"$HOME/gate-pid\"; sleep 5; touch \"$HOME/leaked\"\n",
        1,
        "",
    );
    let mut child = f
        .command()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while !f.home.join("gate-pid").exists() && start.elapsed() < std::time::Duration::from_secs(10)
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(f.home.join("gate-pid").exists());
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    assert!(!child.wait().unwrap().success());
    std::thread::sleep(std::time::Duration::from_secs(5));
    assert!(!f.home.join("leaked").exists());
}
