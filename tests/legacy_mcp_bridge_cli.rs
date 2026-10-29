use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn assert_removed_bridge(args: &[&str]) {
    let root = TempDir::new().unwrap_or_else(|e| panic!("create fixture: {e}"));
    let home = root.path().join("home");
    let project = root.path().join("project");
    let jcode = root.path().join("jcode");
    let data = root.path().join("data");
    let state = root.path().join("state");
    for dir in [&home, &project, &jcode] {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create fixture dir: {e}"));
    }
    let config = jcode.join("mcp.json");
    let original =
        b"{\"mcpServers\":{\"cruise\":{\"command\":\"cruise\",\"args\":[\"mcp-bridge\"]}}}";
    std::fs::write(&config, original).unwrap_or_else(|e| panic!("write config: {e}"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if name.starts_with("CRUISE_") || name.starts_with("JCODE_") {
            command.env_remove(name.as_ref());
        }
    }
    let mut child = command
        .args(args)
        .current_dir(&project)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", &data)
        .env("XDG_STATE_HOME", &state)
        .env("JCODE_HOME", &jcode)
        .env("GIT_CONFIG_COUNT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn cruise: {e}"));
    let input = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("missing stdin"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child
            .try_wait()
            .unwrap_or_else(|e| panic!("poll cruise: {e}"))
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("bridge did not exit with stdin held open");
        }
        thread::sleep(Duration::from_millis(20));
    }
    drop(input);
    let output = child
        .wait_with_output()
        .unwrap_or_else(|e| panic!("collect output: {e}"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "bridge must fail");
    assert!(
        output.stdout.is_empty(),
        "stdout must be empty: {:?}",
        output.stdout
    );
    assert!(
        stderr.contains(&config.to_string_lossy().into_owned()),
        "missing source path: {stderr}"
    );
    assert!(
        stderr.contains("mcpServers.cruise"),
        "missing stale entry: {stderr}"
    );
    assert!(
        stderr.contains("cruise plan mcp-bridge"),
        "missing explicit plan escape: {stderr}"
    );
    assert!(!data.exists(), "data directory created");
    assert!(!state.exists(), "state directory created");
    assert_eq!(
        std::fs::read(&config).unwrap_or_else(|e| panic!("read config: {e}")),
        original
    );
}

#[test]
fn stale_bridge_exits_without_side_effects_with_stdin_open() {
    assert_removed_bridge(&["mcp-bridge"]);
}

#[test]
fn stale_bridge_with_legacy_flags_gives_migration_guidance() {
    assert_removed_bridge(&["mcp-bridge", "--socket", "/tmp/x.sock"]);
}

#[test]
fn stale_bridge_help_gives_migration_guidance() {
    assert_removed_bridge(&["mcp-bridge", "--help"]);
}
