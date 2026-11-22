#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    io::BufReader,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

use jcode_sdk::api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame, read_frame, write_frame,
};

const FAKE_JCODE: &str = "CRUISE_TEST_MANAGER_FAKE_JCODE";

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn command(home: &Path, sdk: bool, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
    c.env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .env("JCODE_HOME", home.join("jcode"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .current_dir(home)
        .stdin(Stdio::null());
    if sdk {
        c.env("CRUISE_SDK", "jcode")
            .env("CRUISE_PLAN_MODEL", "fixture/model");
    } else {
        c.env(
            "CRUISE_COMMAND",
            serde_json::to_string(&[home.join("forbidden-command")])
                .unwrap_or_else(|error| panic!("test operation failed: {error}")),
        );
    }
    c.args(args)
        .output()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
}

struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = command(self.0, true, &["daemon", "stop", "--force"]);
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .trim()
        .into()
}

fn start(home: &Path, sdk: bool) -> (String, String) {
    let out = command(home, sdk, &["daemon", "start", "--port", "0"]);
    assert!(
        out.status.success(),
        "start: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(
        &fs::read(home.join("state/cruise/daemon/instance.json"))
            .unwrap_or_else(|error| panic!("test operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    (
        v["origin"]
            .as_str()
            .unwrap_or_else(|| panic!("expected string"))
            .into(),
        v["token"]
            .as_str()
            .unwrap_or_else(|| panic!("expected string"))
            .into(),
    )
}

async fn snapshot(client: &reqwest::Client, origin: &str, token: &str, repo: &Path) -> Value {
    let mut url = reqwest::Url::parse(&format!("{origin}api/manager/snapshot"))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    url.query_pairs_mut().append_pair(
        "cwd",
        repo.to_str()
            .unwrap_or_else(|| panic!("expected UTF-8 path")),
    );
    client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .error_for_status()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .json()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
}

async fn action(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    repo: &Path,
    action: Value,
) -> Value {
    let response = client
        .post(format!("{origin}api/manager/action"))
        .bearer_auth(token)
        .json(&json!({"cwd":repo,"action":action}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "action: {body}");
    serde_json::from_str(&body).unwrap_or_else(|error| panic!("test operation failed: {error}"))
}

async fn wait_for(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    repo: &Path,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = snapshot(client, origin, token, repo).await;
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "snapshot timeout: {snapshot}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn no_children(home: &Path, snapshot: &Value) {
    assert!(
        snapshot["goals"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .is_empty(),
        "unexpected goals: {snapshot}"
    );
    let dir = home.join("data/cruise/sessions");
    assert!(
        !dir.exists()
            || fs::read_dir(dir)
                .unwrap_or_else(|error| panic!("test operation failed: {error}"))
                .next()
                .is_none(),
        "unauthorized child session"
    );
}

fn install_jcode(home: &Path) {
    fs::create_dir(home.join("bin"))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let jcode = home.join("bin/jcode");
    let executable = std::env::current_exe().unwrap_or_else(|error| panic!("{error}"));
    let log = home.join("fake-jcode.log");
    fs::write(
        &jcode,
        format!(
            "#!/bin/sh\n[ \"$1\" = --no-update ] && [ \"$2\" = api-bridge ] && [ \"$3\" = --api-socket ] || exit 91\nexport {FAKE_JCODE}=1\nexport CRUISE_TEST_SOCKET=\"$4\"\nexport CRUISE_TEST_RELEASE_FILE={}\nexec {} --ignored --exact manager_fake_jcode_runtime --nocapture > {} 2>&1\n",
            shell_quote(&home.join("release-jcode")),
            shell_quote(&executable),
            shell_quote(&log)
        )
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::set_permissions(&jcode, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
}

fn setup_repo(home: &Path) -> std::path::PathBuf {
    let repo = home.join("repo");
    fs::create_dir(&repo).unwrap_or_else(|error| panic!("test operation failed: {error}"));
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &sha]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    repo
}

fn event(value: Value) -> ApiEvent {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
}

#[test]
#[ignore = "subprocess-only fake jcode runtime"]
fn manager_fake_jcode_runtime() {
    if std::env::var(FAKE_JCODE).as_deref() != Ok("1") {
        return;
    }
    let listener = UnixListener::bind(
        std::env::var_os("CRUISE_TEST_SOCKET").unwrap_or_else(|| panic!("missing API socket")),
    )
    .unwrap_or_else(|error| panic!("bind fake Jcode socket: {error}"));
    listener
        .set_nonblocking(false)
        .unwrap_or_else(|error| panic!("configure fake Jcode socket: {error}"));
    let (mut socket, _) = listener
        .accept()
        .unwrap_or_else(|error| panic!("accept fake Jcode connection: {error}"));
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap_or_else(|error| panic!("set fake Jcode read timeout: {error}"));
    socket
        .set_write_timeout(Some(Duration::from_secs(15)))
        .unwrap_or_else(|error| panic!("set fake Jcode write timeout: {error}"));
    let mut reader = BufReader::new(
        socket
            .try_clone()
            .unwrap_or_else(|error| panic!("clone fake Jcode socket: {error}")),
    );
    while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
        let serialized = serde_json::to_string(&frame.request)
            .unwrap_or_else(|error| panic!("serialize fake Jcode request: {error}"));
        if let ApiRequest::SendMessage { session_id, .. } = &frame.request {
            let release_file = std::env::var_os("CRUISE_TEST_RELEASE_FILE")
                .unwrap_or_else(|| panic!("missing fake Jcode release file"));
            let release_path = Path::new(&release_file);
            let deadline = Instant::now() + Duration::from_secs(15);
            while !release_path.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                release_path.exists(),
                "timed out waiting to release fake Jcode"
            );
            let output = if serialized.contains("Create a proposal for human review") {
                r#"{"objective":"Deliver goal","target_branch":"main","exclusions":[],"criteria":[{"id":"c","description":"Works"}],"checks":[{"id":"check","criterion_ids":["c"],"argv":["true"]}],"tasks":[{"id":"task","instructions":"Implement","dependencies":[]}]}"#
            } else {
                "Discussed goal"
            };
            for value in [
                json!({"ev":"message_accepted","session_id":session_id}),
                json!({"ev":"text_delta","session_id":session_id,"text":output}),
                json!({"ev":"text_done","session_id":session_id}),
                json!({"ev":"turn_done","session_id":session_id}),
            ] {
                write_frame(
                    &mut socket,
                    &ServerFrame {
                        v: API_VERSION_MAJOR,
                        reply_to: None,
                        event: event(value),
                    },
                )
                .unwrap_or_else(|error| panic!("write fake Jcode event: {error}"));
            }
            continue;
        }
        let reply = match &frame.request {
            ApiRequest::Ping => ApiEvent::Pong,
            ApiRequest::Hello { .. } => event(json!({
                "ev":"hello_ok",
                "version":API_VERSION_MAJOR,
                "server":"fake-jcode/0.99.0",
                "capabilities":["sessions","session_tools","runtime_commands"]
            })),
            ApiRequest::CreateSession { .. } => event(json!({
                "ev":"attached",
                "session":{"session_id":"manager-fixture","status":"idle","saved":false,"user_connected":false,"archived":false}
            })),
            ApiRequest::GetRuntimeInfo { session_id } => event(json!({
                "ev":"runtime_info",
                "session_id":session_id,
                "routes":[{"model":"model","provider":"fixture","api_method":"openai-compatible:fixture-route","available":true,"detail":"test route"}]
            })),
            ApiRequest::ConfigureTools { .. }
            | ApiRequest::SetModel { .. }
            | ApiRequest::SetReasoningEffort { .. } => ApiEvent::Ok,
            other => panic!("unexpected fake Jcode request: {other:?}"),
        };
        write_frame(
            &mut socket,
            &ServerFrame {
                v: API_VERSION_MAJOR,
                reply_to: Some(frame.id),
                event: reply,
            },
        )
        .unwrap_or_else(|error| panic!("write fake Jcode response: {error}"));
    }
}

#[tokio::test]
async fn conversation_and_preview_survive_disconnect_and_restart_and_command_is_rejected() {
    let root = tempfile::Builder::new()
        .prefix("cm-")
        .tempdir()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let home = root.path();
    install_jcode(home);
    let repo = setup_repo(home);
    let forbidden = home.join("forbidden-command");
    fs::write(&forbidden, "#!/bin/sh\ntouch \"$HOME/command-ran\"\n")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let _stop = Stop(home);
    let (origin, token) = start(home, true);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let turn = action(
        &client,
        &origin,
        &token,
        &repo,
        json!({"type":"message","text":"Plan a goal"}),
    )
    .await;
    drop(client);
    fs::write(home.join("release-jcode"), "release")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let completed_snapshot = wait_for(&client, &origin, &token, &repo, |snapshot| {
        snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|turn| turn["text"] == "Discussed goal")
    })
    .await;
    assert!(
        completed_snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|entry| entry["id"] == turn["id"] && entry["status"] == "completed")
    );
    no_children(home, &completed_snapshot);
    action(&client, &origin, &token, &repo, json!({"type":"propose"})).await;
    let proposed_snapshot = wait_for(&client, &origin, &token, &repo, |snapshot| {
        snapshot["proposal"].is_object()
    })
    .await;
    let proposal = proposed_snapshot["proposal"].clone();
    no_children(home, &proposed_snapshot);
    let out = command(home, true, &["daemon", "stop", "--force"]);
    assert!(
        out.status.success(),
        "stop: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (origin, token) = start(home, false);
    let snapshot = snapshot(&client, &origin, &token, &repo).await;
    assert_eq!(snapshot["proposal"], proposal);
    assert!(
        snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|entry| entry["id"] == turn["id"] && entry["status"] == "completed")
    );
    no_children(home, &snapshot);
    action(
        &client,
        &origin,
        &token,
        &repo,
        json!({"type":"message","text":"Must not execute command"}),
    )
    .await;
    let snapshot = wait_for(&client, &origin, &token, &repo, |snapshot| {
        snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|turn| turn["text"].as_str().unwrap_or("").contains("not command"))
    })
    .await;
    assert_eq!(snapshot["proposal"], proposal);
    no_children(home, &snapshot);
    assert!(!home.join("command-ran").exists());
}
