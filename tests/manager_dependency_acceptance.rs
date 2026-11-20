#![cfg(unix)]

use cruise::manager::{self, Check, Criterion, Proposal, Task};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
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
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
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
        .env("CRUISE_COMMAND", serde_json::to_string(&[backend]).unwrap());
}

fn cli(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    isolated(&mut command, home);
    command
        .current_dir(home)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap()
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
    let proposal = manager::save_proposal(repo, proposal).unwrap();
    fs::write(
        std::env::var_os("DISPATCH_PREVIEW_OUTPUT").unwrap(),
        serde_json::to_vec(&proposal).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn dependent_child_starts_from_integrated_prerequisite_sha() {
    let root = TempDir::new().unwrap();
    let home = root.path();
    fs::create_dir_all(home.join("config/cruise")).unwrap();
    fs::write(
        home.join("config/cruise/default.yaml"),
        "check_base_ci: false\n",
    )
    .unwrap();
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("cruise"),
        format!(
            "#!/bin/sh\n'{}' \"$@\" 2>>\"$HOME/gate-errors\"\n",
            env!("CARGO_BIN_EXE_cruise")
        ),
    )
    .unwrap();
    fs::set_permissions(bin.join("cruise"), fs::Permissions::from_mode(0o700)).unwrap();
    let sdk = bin.join("claude");
    fs::write(&sdk, r##"#!/bin/sh
read -r frame
id=${PWD##*/}
owner="$HOME/data/cruise/sessions/$id/manager-owner.json"
case "$(cat "$owner")" in
  *'"task_id":"a"'*) task=a ;;
  *'"task_id":"b"'*) task=b ;;
  *'"task_id":"c"'*) task=c ;;
  *) echo "unknown owner $owner" >&2; exit 1 ;;
esac
printf '%s\n' "$task" >> "$HOME/sdk-calls"
if [ "$task" = a ]; then
  while [ ! -f "$HOME/release-a" ]; do /bin/sleep 0.1; done
fi
if [ "$task" = b ]; then
  while [ ! -f "$HOME/release-b" ]; do /bin/sleep 0.1; done
fi
case "$frame" in
  *'write tests only'*) printf '%s\n' "$task test" > "${task}-test" ;;
  *'implement production code'*) printf '%s\n' "$task implementation" > "${task}-implementation" ;;
esac
case "$frame" in
  *'<consolidated-worklist>'*) result='## Ledger Records\n```text\niteration: 1\n\n```' ;;
  *) result='NO_CHANGES_INTENTIONAL: fixture completed' ;;
esac
printf '%s\n' '{"type":"system","subtype":"init","session_id":"fixture"}'
printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]},"session_id":"fixture"}\n' "$result"
printf '{"type":"result","subtype":"success","is_error":false,"session_id":"fixture","result":"%s"}\n' "$result"
"##).unwrap();
    fs::set_permissions(&sdk, fs::Permissions::from_mode(0o700)).unwrap();
    let repo = home.join("repo");
    let bare = home.join("origin.git");
    fs::create_dir(&repo).unwrap();
    git(home, &["init", "--bare", "-q", bare.to_str().unwrap()]);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n").unwrap();
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    git(&repo, &["push", "-q", bare.to_str().unwrap(), "main"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &base]);
    let output_path = home.join("preview.json");
    let mut fixture = Command::new(std::env::current_exe().unwrap());
    isolated(&mut fixture, home);
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let output = fixture
        .args(["--exact", "save_dispatch_preview_fixture", "--nocapture"])
        .env_remove("CRUISE_COMMAND")
        .env("CRUISE_SDK", "claude")
        .env("PATH", &path)
        .env("DISPATCH_PREVIEW_REPO", &repo)
        .env("DISPATCH_PREVIEW_OUTPUT", &output_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "preview: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let preview: Value = serde_json::from_slice(&fs::read(output_path).unwrap()).unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_cruise"));
    isolated(&mut daemon, home);
    let started = daemon
        .env_remove("CRUISE_COMMAND")
        .env("CRUISE_SDK", "claude")
        .env("PATH", &path)
        .current_dir(home)
        .stdin(Stdio::null())
        .args(["daemon", "start", "--port", "0"])
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "daemon: {}{}",
        String::from_utf8_lossy(&started.stdout),
        String::from_utf8_lossy(&started.stderr)
    );
    let _stop = Stop(home);
    let instance: Value =
        serde_json::from_slice(&fs::read(home.join("state/cruise/daemon/instance.json")).unwrap())
            .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let origin = instance["origin"].as_str().unwrap();
    let token = instance["token"].as_str().unwrap();
    let response = client.post(format!("{origin}api/manager/action")).bearer_auth(token)
        .json(&json!({"cwd":repo,"action":{"type":"approve","proposal_id":preview["id"],"revision":preview["revision"]}})).send().await.unwrap();
    assert!(
        response.status().is_success(),
        "approve: {}",
        response.text().await.unwrap()
    );
    let mut snapshot_url = reqwest::Url::parse(&format!("{origin}api/manager/snapshot")).unwrap();
    snapshot_url
        .query_pairs_mut()
        .append_pair("cwd", repo.to_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let snapshot: Value = client
            .get(snapshot_url.clone())
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        let goal = &snapshot["goals"][0];
        let attempts = goal["execution"]["attempts"].as_array().unwrap();
        let a = attempts.iter().find(|a| a["task_id"] == "a");
        let b = attempts.iter().find(|a| a["task_id"] == "b");
        let c = attempts.iter().find(|a| a["task_id"] == "c");
        if a.is_some() && b.is_some() && !home.join("release-a").exists() {
            assert!(c.is_none(), "C launched before A completed");
            fs::write(home.join("release-a"), "release").unwrap();
        }
        if let Some(c) = c {
            let a = a.expect("C without A");
            let b = b.expect("C without B");
            assert_eq!(
                a["integrated"], true,
                "C launched before A integrated: {goal}"
            );
            assert_eq!(b["base_sha"], base);
            assert_eq!(a["base_sha"], base);
            assert_eq!(
                c["base_sha"], goal["execution"]["integrated_tip"],
                "C did not start from exact integrated tip"
            );
            assert_ne!(c["base_sha"], base, "A made no integrated commit");
            assert_eq!(b["integrated"], false, "B must remain blocked in fixture");
            fs::write(home.join("release-b"), "release").unwrap();
            break;
        }
        if goal["status"] == "blocked" {
            let files = Command::new("find")
                .arg(home.join("data/cruise/sessions"))
                .args(["-type", "f"])
                .output()
                .unwrap();
            panic!(
                "blocked: reason={} attempts={} files={} daemon={}",
                goal["reason"],
                goal["execution"]["attempts"],
                String::from_utf8_lossy(&files.stdout),
                fs::read_to_string(home.join("state/cruise/daemon/daemon.log")).unwrap_or_default()
            );
        }
        assert!(
            Instant::now() < deadline,
            "dependency did not dispatch: status={} reason={} attempts={} sdk={} daemon={}",
            goal["status"],
            goal["reason"],
            goal["execution"]["attempts"],
            fs::read_to_string(home.join("sdk-calls")).unwrap_or_default(),
            format!(
                "{} gate-errors={}",
                fs::read_to_string(home.join("state/cruise/daemon/daemon.log")).unwrap_or_default(),
                fs::read_to_string(home.join("gate-errors")).unwrap_or_default()
            )
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
