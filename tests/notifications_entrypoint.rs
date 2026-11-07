#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::process::Command;

use tempfile::TempDir;

#[test]
fn cli_plan_entrypoint_continues_with_notification_opt_out_enabled() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("failed to create fixture: {error}"));
    let config = temp.path().join("cruise.yaml");
    std::fs::write(
        &config,
        "command: [echo]\nsteps:\n  check:\n    command: echo check\n",
    )
    .unwrap_or_else(|error| panic!("failed to write fixture config: {error}"));

    let home = temp.path().join("home");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if (name.starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
                || name.starts_with("JCODE_")
                || name.starts_with("OTEL_")
                || name.starts_with("CLAUDE_")
                || name.starts_with("ANTHROPIC_")
            {
                command.env_remove(name.as_ref());
            }
        }
        command
            .current_dir(temp.path())
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("JCODE_HOME", temp.path().join("jcode"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV");
        command
    };

    let output = command()
        .args(["plan", "--skip-planning", "--config"])
        .arg(&config)
        .arg("test notification opt-out")
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise plan: {error}"));
    assert!(
        output.status.success(),
        "plan entry point failed with notification opt-out: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output = command()
        .args(["list", "--json"])
        .output()
        .unwrap_or_else(|error| panic!("failed to list cruise sessions: {error}"));
    assert!(
        output.status.success(),
        "list entry point failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("list --json output was not valid JSON: {error}"));
    assert_eq!(
        sessions.len(),
        1,
        "expected one planned session: {sessions:?}"
    );
    assert_eq!(sessions[0]["phase"], "Planned", "{sessions:?}");
}

struct WebhookFixture {
    temp: TempDir,
    config: std::path::PathBuf,
}

#[test]
fn cli_plan_starts_sound_before_immediate_exit() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let fixture = WebhookFixture::new();
    std::fs::write(
        fixture.temp.path().join("config/cruise/config.json"),
        r#"{"notifications":{"desktop":false,"sound":true}}"#,
    )
    .unwrap_or_else(|error| panic!("{error:?}"));
    let bin = fixture.temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap_or_else(|error| panic!("{error:?}"));
    let player = bin.join(if cfg!(target_os = "macos") {
        "afplay"
    } else {
        "paplay"
    });
    std::fs::write(&player, "#!/bin/sh\necho played >> \"$SOUND_MARKER\"\n")
        .unwrap_or_else(|error| panic!("{error:?}"));
    std::fs::set_permissions(&player, std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|error| panic!("{error:?}"));
    let marker = fixture.temp.path().join("sound-marker");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    for _ in 0..5 {
        let output = fixture
            .command("")
            .env_remove("CRUISE_NOTIFY_WEBHOOK")
            .env("PATH", &path)
            .env("SOUND_MARKER", &marker)
            .args(["plan", "--skip-planning", "--config"])
            .arg(&fixture.config)
            .arg("sound test")
            .output()
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = std::fs::read_to_string(&marker)
            .unwrap_or_default()
            .lines()
            .count();
        if count == 5 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected 5 player invocations, got {count}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl WebhookFixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error:?}"));
        let config = temp.path().join("cruise.yaml");
        std::fs::write(
            &config,
            "command: [echo]\nsteps:\n  check:\n    command: echo check\n",
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        let config_dir = temp.path().join("config/cruise");
        std::fs::create_dir_all(&config_dir).unwrap_or_else(|error| panic!("{error:?}"));
        std::fs::write(
            config_dir.join("config.json"),
            r#"{"notifications":{"desktop":false}}"#,
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        Self { temp, config }
    }

    fn command(&self, url: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if (name.starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
                || name.starts_with("JCODE_")
                || name.starts_with("OTEL_")
                || name.starts_with("CLAUDE_")
                || name.starts_with("ANTHROPIC_")
            {
                command.env_remove(name.as_ref());
            }
        }
        let root = self.temp.path();
        command
            .current_dir(root)
            .env("HOME", root.join("home"))
            .env("USERPROFILE", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("JCODE_HOME", root.join("jcode"))
            .env("CRUISE_NOTIFY_WEBHOOK", url)
            .env_remove("HERDR_ENV");
        command
    }

    fn plan(&self, url: &str, disabled: bool) -> std::process::Output {
        let mut command = self.command(url);
        if disabled {
            command.env("CRUISE_DISABLE_NOTIFICATIONS", "1");
        }
        let output = command
            .args(["plan", "--skip-planning", "--config"])
            .arg(&self.config)
            .arg("webhook test title")
            .output()
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert!(
            output.status.success(),
            "plan failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

#[test]
fn cli_plan_posts_plan_ready_to_webhook_and_exits_zero() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let fixture = WebhookFixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error:?}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let url = format!(
        "http://{}/hook",
        listener
            .local_addr()
            .unwrap_or_else(|error| panic!("{error:?}"))
    );
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap_or_else(|error| panic!("{error:?}"));
                    let mut request = Vec::new();
                    let mut buffer = [0; 4096];
                    loop {
                        let count = stream
                            .read(&mut buffer)
                            .unwrap_or_else(|error| panic!("{error:?}"));
                        assert!(count > 0, "connection closed before request was complete");
                        request.extend_from_slice(&buffer[..count]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&request[..end]);
                            let length: usize = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length").then(|| {
                                        value
                                            .trim()
                                            .parse()
                                            .unwrap_or_else(|error| panic!("{error:?}"))
                                    })
                                })
                                .unwrap_or_else(|| panic!("missing content length"));
                            if request.len() >= end + 4 + length {
                                let body = request[end + 4..end + 4 + length].to_vec();
                                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap_or_else(|error| panic!("{error:?}"));
                                return Some((body, listener));
                            }
                        }
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return None,
                Err(error) => panic!("listener failed: {error}"),
            }
        }
    });
    fixture.plan(&url, false);
    let (body, listener) = server
        .join()
        .unwrap_or_else(|error| panic!("{error:?}"))
        .unwrap_or_else(|| panic!("webhook request not received"));
    assert_eq!(
        listener.accept().err().map(|error| error.kind()),
        Some(std::io::ErrorKind::WouldBlock)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|error| panic!("{error:?}"));
    assert!(
        json["text"]
            .as_str()
            .unwrap_or_else(|| panic!("missing text"))
            .contains("Plan ready -- webhook test title"),
        "{json}"
    );
    let output = fixture
        .command(&url)
        .args(["list", "--json"])
        .output()
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert!(output.status.success());
    let sessions: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["phase"], "Planned");
}

#[test]
fn cli_plan_survives_unreachable_webhook() {
    let fixture = WebhookFixture::new();
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error:?}"));
    let url = format!(
        "http://{}/hook",
        listener
            .local_addr()
            .unwrap_or_else(|error| panic!("{error:?}"))
    );
    drop(listener);
    let output = fixture.plan(&url, false);
    let error = std::fs::read_to_string(
        fixture
            .temp
            .path()
            .join("state/cruise/notification-webhook-error"),
    )
    .unwrap_or_else(|error| panic!("{error:?}"));
    let (timestamp, classification) = error
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("missing newline"))
        .split_once(' ')
        .unwrap_or_else(|| panic!("missing separator"));
    assert!(!timestamp.is_empty() && timestamp.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(classification, "transport error");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("/hook"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("/hook"));
}

#[test]
fn cli_disable_notifications_suppresses_webhook() {
    let fixture = WebhookFixture::new();
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error:?}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let url = format!(
        "http://{}/hook",
        listener
            .local_addr()
            .unwrap_or_else(|error| panic!("{error:?}"))
    );
    fixture.plan(&url, true);
    assert_eq!(
        listener.accept().err().map(|error| error.kind()),
        Some(std::io::ErrorKind::WouldBlock)
    );
}
