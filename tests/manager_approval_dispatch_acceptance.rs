#![cfg(unix)]

use cruise::manager::{self, Check, Criterion, Proposal, Task};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap_or_else(|error| panic!("{error}"))
        .trim()
        .to_owned()
}

fn isolated(command: &mut Command, home: &Path) {
    let backend = home.join("inert-agent");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env(
            "CRUISE_COMMAND",
            serde_json::to_string(&[backend]).unwrap_or_else(|error| panic!("{error}")),
        );
}

fn cli(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    isolated(&mut command, home);
    command
        .current_dir(home)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{error}"))
}

struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = cli(self.0, &["daemon", "stop", "--force"]);
    }
}

// This subprocess only saves a preview using the real proposal validator and
// contract freezer. Approval and all execution belong to the real daemon.
#[test]
fn save_dispatch_preview_fixture() {
    let Some(repo) = std::env::var_os("DISPATCH_PREVIEW_REPO") else {
        return;
    };
    let repo = Path::new(&repo);
    let sha = git(repo, &["rev-parse", "HEAD"]);
    let proposal = Proposal {
        id: "provider".into(),
        revision: 0,
        objective: "Deliver the goal".into(),
        target_branch: "main".into(),
        target_sha: sha,
        exclusions: vec![],
        criteria: vec![Criterion {
            id: "c".into(),
            description: "Works".into(),
        }],
        checks: vec![Check {
            id: "check".into(),
            criterion_ids: vec!["c".into()],
            argv: vec!["true".into()],
        }],
        tasks: vec![
            Task {
                id: "a".into(),
                instructions: "Implement A".into(),
                dependencies: vec![],
            },
            Task {
                id: "b".into(),
                instructions: "Implement B".into(),
                dependencies: vec![],
            },
            Task {
                id: "c".into(),
                instructions: "Implement C".into(),
                dependencies: vec!["a".into()],
            },
        ],
        workflow: "builtin:tdd".into(),
        contract: None,
    };
    let proposal = manager::save_proposal(repo, proposal).unwrap_or_else(|error| panic!("{error}"));
    fs::write(
        std::env::var_os("DISPATCH_PREVIEW_OUTPUT")
            .unwrap_or_else(|| panic!("missing preview output path")),
        serde_json::to_vec(&proposal).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

fn prepare_preview(home: &Path) -> (std::path::PathBuf, Value) {
    // Never invoke an installed agent or inherit credentials. The command backend
    // remains alive until daemon shutdown, letting both ready tasks be observed.
    let backend = home.join("inert-agent");
    fs::write(&backend, "#!/bin/sh\nexec /bin/sleep 30\n")
        .unwrap_or_else(|error| panic!("{error}"));
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("{error}"));
    let repo = home.join("repo");
    let bare = home.join("origin.git");
    fs::create_dir(&repo).unwrap_or_else(|error| panic!("{error}"));
    git(
        home,
        &[
            "init",
            "--bare",
            "-q",
            bare.to_str().unwrap_or_else(|| panic!("path is not UTF-8")),
        ],
    );
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n").unwrap_or_else(|error| panic!("{error}"));
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    git(
        &repo,
        &[
            "push",
            "-q",
            bare.to_str().unwrap_or_else(|| panic!("path is not UTF-8")),
            "main",
        ],
    );
    git(&repo, &["update-ref", "refs/remotes/origin/main", &sha]);
    let output_path = home.join("preview.json");
    let mut fixture =
        Command::new(std::env::current_exe().unwrap_or_else(|error| panic!("{error}")));
    isolated(&mut fixture, home);
    let output = fixture
        .args(["--exact", "save_dispatch_preview_fixture", "--nocapture"])
        .env("DISPATCH_PREVIEW_REPO", &repo)
        .env("DISPATCH_PREVIEW_OUTPUT", &output_path)
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.status.success(),
        "preview fixture: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let preview: Value =
        serde_json::from_slice(&fs::read(output_path).unwrap_or_else(|error| panic!("{error}")))
            .unwrap_or_else(|error| panic!("{error}"));
    (repo, preview)
}

async fn snapshot(client: &reqwest::Client, instance: &Value, token: &str, repo: &Path) -> Value {
    let mut snapshot_url = reqwest::Url::parse(&format!(
        "{}api/manager/snapshot",
        instance["origin"]
            .as_str()
            .unwrap_or_else(|| panic!("expected string"))
    ))
    .unwrap_or_else(|error| panic!("{error}"));
    snapshot_url.query_pairs_mut().append_pair(
        "cwd",
        repo.to_str().unwrap_or_else(|| panic!("path is not UTF-8")),
    );
    client
        .get(snapshot_url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .error_for_status()
        .unwrap_or_else(|error| panic!("{error}"))
        .json()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
}

fn assert_no_sessions(sessions: &Path, message: &str) {
    assert!(
        !sessions.exists()
            || fs::read_dir(sessions)
                .unwrap_or_else(|error| panic!("{error}"))
                .next()
                .is_none(),
        "{message}"
    );
}

async fn submit_approval(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    request: &Value,
) -> Value {
    let response = client
        .post(url)
        .bearer_auth(token)
        .json(request)
        .send()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        response.status().is_success(),
        "approval failed: {}",
        response
            .text()
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    );
    response
        .json()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
}

#[tokio::test]
async fn approval_registers_goal_and_is_idempotent() {
    let root = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    let home = root.path();
    let (repo, preview) = prepare_preview(home);
    let started = cli(home, &["daemon", "start", "--port", "0"]);
    assert!(
        started.status.success(),
        "daemon start: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let _stop = Stop(home);
    let instance: Value = serde_json::from_slice(
        &fs::read(home.join("state/cruise/daemon/instance.json"))
            .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("{error}"));
    let url = format!(
        "{}api/manager/action",
        instance["origin"]
            .as_str()
            .unwrap_or_else(|| panic!("expected string"))
    );
    let token = instance["token"]
        .as_str()
        .unwrap_or_else(|| panic!("expected string"));
    let action = |id: Value, revision: Value| json!({"cwd":repo,"action":{"type":"approve","proposal_id":id,"revision":revision}});
    let sessions = home.join("data/cruise/sessions");
    assert_no_sessions(&sessions, "preview started a child");
    let stale = client
        .post(&url)
        .bearer_auth(token)
        .json(&action(
            preview["id"].clone(),
            json!(
                preview["revision"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("expected revision"))
                    + 1
            ),
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!stale.status().is_success(), "stale approval accepted");
    assert_no_sessions(&sessions, "stale approval started a child");

    let request = action(preview["id"].clone(), preview["revision"].clone());
    let first_goal = submit_approval(&client, &url, token, &request).await;
    let second_goal = submit_approval(&client, &url, token, &request).await;
    assert_eq!(first_goal["id"], second_goal["id"]);

    let snapshot = snapshot(&client, &instance, token, &repo).await;
    assert!(snapshot["proposal"].is_null());
    let goals = snapshot["goals"]
        .as_array()
        .unwrap_or_else(|| panic!("expected goals array"));
    assert_eq!(goals.len(), 1, "approval created duplicate Goals");
    let goal = &goals[0];
    assert_eq!(goal["id"], first_goal["id"]);
    assert_eq!(goal["proposal"]["id"], preview["id"]);
    assert_eq!(goal["proposal"]["revision"], preview["revision"]);
    assert_eq!(goal["execution"]["integrated_tip"], preview["target_sha"]);
    assert_eq!(goal["execution"]["anchor"], preview["contract"]["anchor"]);
    assert_eq!(goal["execution"]["origin"], preview["contract"]["origin"]);
    assert_eq!(
        goal["execution"]["workflow"],
        preview["contract"]["workflow"]
    );
}
