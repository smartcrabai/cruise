#![cfg(unix)]

//! `{cruise.bin}` runs the Cruise executable executing the workflow, not a
//! possibly different `cruise` found on `PATH`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;

/// A git repository plus a stale `cruise` first on `PATH` that records calls.
struct Fixture {
    root: tempfile::TempDir,
    repo: PathBuf,
    stale_log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let repo = root.path().join("repo");
        let bin = root.path().join("bin");
        std::fs::create_dir(&repo).unwrap_or_else(|error| panic!("repo mkdir failed: {error}"));
        std::fs::create_dir(&bin).unwrap_or_else(|error| panic!("bin mkdir failed: {error}"));
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        ] {
            let status = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_COUNT", "0")
                .status()
                .unwrap_or_else(|error| panic!("git failed to start: {error}"));
            assert!(status.success(), "git {args:?} failed");
        }
        let stale_log = root.path().join("stale.log");
        let stale = bin.join("cruise");
        std::fs::write(
            &stale,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 97\n",
                stale_log.display()
            ),
        )
        .unwrap_or_else(|error| panic!("stale cruise write failed: {error}"));
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("stale cruise chmod failed: {error}"));
        let defaults = root.path().join("config/cruise");
        std::fs::create_dir_all(&defaults)
            .unwrap_or_else(|error| panic!("defaults mkdir failed: {error}"));
        std::fs::write(defaults.join("default.yaml"), "check_base_ci: false\n")
            .unwrap_or_else(|error| panic!("defaults write failed: {error}"));
        Self {
            root,
            repo,
            stale_log,
        }
    }

    fn cruise(&self, args: &[&str]) -> Output {
        let path = std::env::join_paths(std::iter::once(self.root.path().join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap_or_else(|error| panic!("PATH join failed: {error}"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        let output = command
            .args(args)
            .current_dir(&self.repo)
            .env("PATH", path)
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|error| panic!("cruise failed to start: {error}"));
        let text = text(&output);
        assert!(output.status.success(), "{text}");
        assert!(
            !self.stale_log.exists(),
            "the stale PATH cruise ran: {text}"
        );
        output
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn cruise_bin_ignores_a_stale_cruise_on_path() {
    let fixture = Fixture::new();
    let config = fixture.root.path().join("workflow.yaml");
    std::fs::write(
        &config,
        "command: [cat]\nsteps:\n  probe:\n    command: '{cruise.bin} --version > version.txt'\n",
    )
    .unwrap_or_else(|error| panic!("workflow write failed: {error}"));
    let config = config.to_string_lossy().into_owned();
    let output = fixture.cruise(&["exec", "--config", &config, "probe"]);
    let version = std::fs::read_to_string(fixture.repo.join("version.txt"))
        .unwrap_or_else(|error| panic!("version.txt missing ({error}): {}", text(&output)));
    assert_eq!(
        version.trim(),
        format!("cruise {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn builtin_snapshot_saved_before_cruise_bin_ignores_a_stale_cruise_on_path() {
    let fixture = Fixture::new();
    let manager = SessionManager::new(fixture.root.path().join("data/cruise"));
    let id = SessionManager::new_session_id();
    let mut state = SessionState::new(
        id.clone(),
        fixture.repo.clone(),
        SessionConfigRef::BuiltinSnapshot { name: None },
        "probe".into(),
    );
    state.phase = SessionPhase::Planned;
    manager
        .create(&state)
        .unwrap_or_else(|error| panic!("create session: {error}"));
    std::fs::write(state.plan_path(&manager.sessions_dir()), "# Probe\n")
        .unwrap_or_else(|error| panic!("write plan: {error}"));
    // Built-in snapshots saved before `{cruise.bin}` call `cruise internal` by name.
    let help = fixture.root.path().join("internal-help.txt");
    std::fs::write(
        cruise::session_config::snapshot_path(&manager.sessions_dir(), &id),
        format!(
            "command: [cat]\ncreate_pr: false\nsteps:\n  probe:\n    command: cruise internal --help > '{}'\n",
            help.display()
        ),
    )
    .unwrap_or_else(|error| panic!("write legacy snapshot: {error}"));
    let output = fixture.cruise(&["run", &id]);
    let help = std::fs::read_to_string(&help)
        .unwrap_or_else(|error| panic!("internal help missing ({error}): {}", text(&output)));
    assert!(help.contains("Usage"), "{help}");
}
