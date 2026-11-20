use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

#[test]
fn manager_requires_an_interactive_terminal() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap();
    assert!(status.success());
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    let output = command
        .current_dir(root.path())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("JCODE_HOME", home.join("jcode"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .arg("manager")
        .output()
        .unwrap();
    let _ = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .current_dir(root.path())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CRUISE_DAEMON_PORT")
        .args(["daemon", "stop", "--force"])
        .output();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "manager accepted non-TTY input: {text}"
    );
    let text = text.to_lowercase();
    assert!(
        text.contains("terminal") || text.contains("tty") || text.contains("interactive"),
        "expected actionable terminal error: {text}"
    );
}
