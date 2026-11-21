#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn command(home: &Path, sdk: bool, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
    c.env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .current_dir(home)
        .stdin(Stdio::null());
    if sdk {
        c.env("CRUISE_SDK", "claude");
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
        let s = snapshot(client, origin, token, repo).await;
        if predicate(&s) {
            return s;
        }
        assert!(Instant::now() < deadline, "snapshot timeout: {s}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn no_children(home: &Path, s: &Value) {
    assert!(
        s["goals"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .is_empty(),
        "unexpected goals: {s}"
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

fn install_sdk(home: &Path) {
    fs::create_dir(home.join("bin"))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let sdk = home.join("bin/claude");
    fs::write(&sdk, r#"#!/bin/sh
read -r frame
printf '%s\n' invoked >> "$HOME/sdk-calls"
while [ ! -f "$HOME/release-sdk" ]; do /bin/sleep 0.1; done
case "$frame" in
  *'Create a proposal for human review'*) result='{\"objective\":\"Deliver goal\",\"target_branch\":\"main\",\"exclusions\":[],\"criteria\":[{\"id\":\"c\",\"description\":\"Works\"}],\"checks\":[{\"id\":\"check\",\"criterion_ids\":[\"c\"],\"argv\":[\"true\"]}],\"tasks\":[{\"id\":\"task\",\"instructions\":\"Implement\",\"dependencies\":[]}]}' ;;
  *) result='Discussed goal' ;;
esac
printf '%s\n' '{"type":"system","subtype":"init","session_id":"fixture"}'
printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]},"session_id":"fixture"}\n' "$result"
printf '{"type":"result","subtype":"success","is_error":false,"session_id":"fixture","result":"%s"}\n' "$result"
"#).unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::set_permissions(&sdk, fs::Permissions::from_mode(0o700))
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

#[tokio::test]
async fn conversation_and_preview_survive_disconnect_and_restart_and_command_is_rejected() {
    let root = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let home = root.path();
    install_sdk(home);
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
    fs::write(home.join("release-sdk"), "release")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let s = wait_for(&client, &origin, &token, &repo, |s| {
        s["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|t| t["text"] == "Discussed goal")
    })
    .await;
    assert!(
        s["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|t| t["id"] == turn["id"] && t["status"] == "completed")
    );
    no_children(home, &s);
    action(&client, &origin, &token, &repo, json!({"type":"propose"})).await;
    let s = wait_for(&client, &origin, &token, &repo, |s| {
        s["proposal"].is_object()
    })
    .await;
    let proposal = s["proposal"].clone();
    no_children(home, &s);
    let out = command(home, true, &["daemon", "stop", "--force"]);
    assert!(
        out.status.success(),
        "stop: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (origin, token) = start(home, false);
    let s = snapshot(&client, &origin, &token, &repo).await;
    assert_eq!(s["proposal"], proposal);
    assert!(
        s["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|t| t["id"] == turn["id"] && t["status"] == "completed")
    );
    no_children(home, &s);
    action(
        &client,
        &origin,
        &token,
        &repo,
        json!({"type":"message","text":"Must not execute command"}),
    )
    .await;
    let s = wait_for(&client, &origin, &token, &repo, |s| {
        s["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|t| t["text"].as_str().unwrap_or("").contains("not command"))
    })
    .await;
    assert_eq!(s["proposal"], proposal);
    no_children(home, &s);
    assert!(!home.join("command-ran").exists());
}
