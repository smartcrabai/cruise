#![cfg(unix)]

//! `{cruise.bin}` runs the Cruise executable executing the workflow, not a
//! possibly different `cruise` found on `PATH`.

use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Stdio};

#[test]
fn cruise_bin_ignores_a_stale_cruise_on_path() {
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
    let config = root.path().join("workflow.yaml");
    std::fs::write(
        &config,
        "command: [cat]\nsteps:\n  probe:\n    command: '{cruise.bin} --version > version.txt'\n",
    )
    .unwrap_or_else(|error| panic!("workflow write failed: {error}"));
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap_or_else(|error| panic!("PATH join failed: {error}"));

    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    let output = command
        .args(["exec", "--config"])
        .arg(&config)
        .arg("probe")
        .current_dir(&repo)
        .env("PATH", path)
        .env("HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env_remove("HERDR_ENV")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("exec failed to start: {error}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{text}");
    assert!(!stale_log.exists(), "the stale PATH cruise ran: {text}");
    let version = std::fs::read_to_string(repo.join("version.txt"))
        .unwrap_or_else(|error| panic!("version.txt missing ({error}): {text}"));
    assert_eq!(
        version.trim(),
        format!("cruise {}", env!("CARGO_PKG_VERSION"))
    );
}
