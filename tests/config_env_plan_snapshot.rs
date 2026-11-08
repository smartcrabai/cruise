use std::process::Command;

#[test]
fn plan_builtin_snapshot_captures_environment_retry_override()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let project = root.path().join("project");
    std::fs::create_dir_all(&project)?;
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
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
        let result = Command::new("git")
            .current_dir(&project)
            .env("GIT_CONFIG_COUNT", "0")
            .args(args)
            .output()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    let output = command
        .current_dir(&project)
        .env("HOME", root.path().join("home"))
        .env("USERPROFILE", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("GIT_CONFIG_COUNT", "0")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("CRUISE_MAX_RETRIES", "7")
        .env("CRUISE_COMMAND", "[\"echo\"]")
        .env("NO_COLOR", "1")
        .env_remove("HERDR_ENV")
        .env_remove("JCODE_MODEL")
        .args([
            "plan",
            "--skip-planning",
            "--no-force-exec",
            "-c",
            "__builtin__",
            "test plan",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manager = cruise::session::SessionManager::new(root.path().join("data/cruise"));
    let sessions = manager.list()?;
    assert_eq!(sessions.len(), 1);
    let path = manager
        .sessions_dir()
        .join(&sessions[0].id)
        .join("config.yaml");
    let snapshot: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(path)?)?;
    assert_eq!(snapshot["max_retries"].as_u64(), Some(7));
    Ok(())
}
