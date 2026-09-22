#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// A git repo, a workflow, and a fake `herdr` binary that logs its argv.
struct Fixture {
    root: TempDir,
    repo: PathBuf,
    config: PathBuf,
    herdr_bin: PathBuf,
    herdr_log: PathBuf,
}

impl Fixture {
    fn new(yaml: &str) -> Self {
        let root = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e}"));
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
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
        ] {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, yaml).unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = root.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap_or_else(|e| panic!("{e}"));
        let herdr_bin = bin_dir.join("herdr");
        std::fs::write(
            &herdr_bin,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${FAKE_HERDR_LOG:?}\"\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&herdr_bin)
                .unwrap_or_else(|e| panic!("{e}"))
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&herdr_bin, permissions).unwrap_or_else(|e| panic!("{e}"));
        }
        let herdr_log = root.path().join("herdr.log");

        Self {
            root,
            repo,
            config,
            herdr_bin,
            herdr_log,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .arg("exec")
            .arg("--config")
            .arg(&self.config)
            .arg("review")
            .current_dir(&self.repo)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env("HERDR_ENV", "1")
            .env("HERDR_PANE_ID", "w1:p1")
            .env("HERDR_BIN_PATH", &self.herdr_bin)
            .env("FAKE_HERDR_LOG", &self.herdr_log)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn herdr_invocations(&self) -> Vec<String> {
        std::fs::read_to_string(&self.herdr_log)
            .unwrap_or_else(|e| panic!("no herdr invocations logged: {e}"))
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Split `… --seq <n>` off the end of a logged argv line.
fn split_seq(line: &str) -> (String, u64) {
    let (head, seq) = line
        .rsplit_once(" --seq ")
        .unwrap_or_else(|| panic!("missing --seq in herdr invocation: {line}"));
    let seq = seq
        .parse::<u64>()
        .unwrap_or_else(|e| panic!("unparsable --seq in {line}: {e}"));
    (head.to_string(), seq)
}

#[test]
fn exec_reports_working_then_idle_and_releases_the_pane() {
    let fixture = Fixture::new(
        r"
command: [sh, -c, cat]
steps:
  check:
    command: 'echo ok'
",
    );
    let output = fixture.command().output().unwrap_or_else(|e| panic!("{e}"));
    let terminal = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{terminal}");

    let invocations = fixture.herdr_invocations();
    assert_eq!(
        invocations.len(),
        3,
        "unexpected herdr invocations: {invocations:?}"
    );
    let (working, first_seq) = split_seq(&invocations[0]);
    let (idle, second_seq) = split_seq(&invocations[1]);
    let (released, third_seq) = split_seq(&invocations[2]);
    assert_eq!(
        working,
        "pane report-agent w1:p1 --source custom:cruise --agent cruise --state working"
    );
    assert_eq!(
        idle,
        "pane report-agent w1:p1 --source custom:cruise --agent cruise --state idle"
    );
    assert_eq!(
        released,
        "pane release-agent w1:p1 --source custom:cruise --agent cruise"
    );
    assert!(
        first_seq < second_seq && second_seq < third_seq,
        "herdr sequence numbers must increase: {first_seq} {second_seq} {third_seq}"
    );
}
