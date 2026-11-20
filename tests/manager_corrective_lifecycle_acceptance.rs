#![cfg(unix)]

use cruise::manager::{self, Check, Criterion, Proposal, Task};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}
fn command(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
    c.current_dir(home)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("CRUISE_SDK", "claude")
        .stdin(Stdio::null());
    c
}
struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = command(self.0).args(["daemon", "stop", "--force"]).output();
    }
}

// The subprocess freezes the production workflow contract, not the execution.
#[test]
fn corrective_preview_fixture() {
    let Some(repo) = std::env::var_os("CORRECTIVE_REPO") else {
        return;
    };
    let repo = Path::new(&repo);
    let proposal = Proposal {
        id: "fixture".into(),
        revision: 0,
        objective: "Fix goal".into(),
        target_branch: "main".into(),
        target_sha: git(repo, &["rev-parse", "HEAD"]),
        exclusions: vec![],
        criteria: vec![Criterion {
            id: "c".into(),
            description: "fixed".into(),
        }],
        checks: vec![Check {
            id: "check".into(),
            criterion_ids: vec!["c".into()],
            argv: vec!["test".into(), "-f".into(), "fixed-implementation".into()],
        }],
        tasks: vec![Task {
            id: "original".into(),
            instructions: "original".into(),
            dependencies: vec![],
        }],
        workflow: "builtin:tdd".into(),
        contract: None,
    };
    let proposal = manager::save_proposal(repo, proposal).unwrap();
    fs::write(
        std::env::var_os("CORRECTIVE_OUTPUT").unwrap(),
        serde_json::to_vec(&proposal).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn approved_correction_executes_and_reverifies() {
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
    fs::write(bin.join("claude"), r##"#!/bin/sh
read -r frame
id=${PWD##*/}
cat "$HOME/data/cruise/sessions/$id/manager-owner.json" >> "$HOME/sdk-owners"
printf '\n' >> "$HOME/sdk-owners"
case "$(cat "$HOME/data/cruise/sessions/$id/manager-owner.json")" in
  *'"task_id":"fixed"'*) task=fixed ;;
  *) task=original ;;
esac
case "$frame" in
  *'write tests only'*) echo test > "$task-test" ;;
  *'implement production code'*) echo fixed > "$task-implementation" ;;
esac
case "$frame" in
  *'<consolidated-worklist>'*) result='## Ledger Records\n```text\niteration: 1\n\n```' ;;
  *) result='NO_CHANGES_INTENTIONAL: fixture completed' ;;
esac
printf '%s\n' '{"type":"system","subtype":"init","session_id":"fixture"}'
printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]},"session_id":"fixture"}\n' "$result"
printf '{"type":"result","subtype":"success","is_error":false,"session_id":"fixture","result":"%s"}\n' "$result"
"##).unwrap();
    fs::set_permissions(bin.join("claude"), fs::Permissions::from_mode(0o700)).unwrap();
    let repo = home.join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n").unwrap();
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    let origin = "https://github.com/example/project.git";
    git(&repo, &["remote", "add", "origin", origin]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &sha]);
    let output = home.join("proposal.json");
    let mut fixture = Command::new(std::env::current_exe().unwrap());
    let result = fixture
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("CRUISE_SDK", "claude")
        .env("CORRECTIVE_REPO", &repo)
        .env("CORRECTIVE_OUTPUT", &output)
        .args(["--exact", "corrective_preview_fixture", "--nocapture"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let proposal: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    let started = command(home)
        .args(["daemon", "start", "--port", "0"])
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "daemon: {}",
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
    let origin_url = instance["origin"].as_str().unwrap();
    let token = instance["token"].as_str().unwrap();
    let post = |action: Value| {
        client
            .post(format!("{origin_url}api/manager/action"))
            .bearer_auth(token)
            .json(&json!({"cwd":repo,"action":action}))
            .send()
    };
    let response = post(
        json!({"type":"approve","proposal_id":proposal["id"],"revision":proposal["revision"]}),
    )
    .await
    .unwrap();
    assert!(
        response.status().is_success(),
        "initial approval: {}",
        response.text().await.unwrap()
    );
    let initial: Value = response.json().await.unwrap();
    let goal_id = initial["id"].as_str().unwrap().to_owned();
    let mut url = reqwest::Url::parse(&format!("{origin_url}api/manager/snapshot")).unwrap();
    url.query_pairs_mut()
        .append_pair("cwd", repo.to_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(90);
    let (publication, sha) = loop {
        let snapshot: Value = client
            .get(url.clone())
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
        if goal["status"] == "blocked" {
            let publication = goal["execution"]["publication"].clone();
            assert_eq!(
                publication["checks"][0]["exit_code"], 1,
                "initial goal blocked before failed check: {}",
                goal["reason"]
            );
            break (
                publication,
                goal["execution"]["integrated_tip"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "initial goal timed out: status={} reason={} daemon={}",
            goal["status"],
            goal["reason"],
            fs::read_to_string(home.join("state/cruise/daemon/daemon.log")).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let response = post(json!({"type":"preview_corrective","goal_id":goal_id,"expected_tip":sha,"task":{"id":"fixed","instructions":"Fix the failed check","dependencies":["original"]}})).await.unwrap();
    assert!(
        response.status().is_success(),
        "preview: {}",
        response.text().await.unwrap()
    );
    let preview: Value = response.json().await.unwrap();
    let response = post(json!({"type":"approve_corrective","goal_id":goal_id,"preview_id":preview["id"],"revision":preview["revision"]})).await.unwrap();
    assert!(
        response.status().is_success(),
        "approval: {}",
        response.text().await.unwrap()
    );
    let approved: Value = response.json().await.unwrap();
    assert_eq!(
        approved["execution"]["publication_history"],
        json!([publication])
    );
    assert_eq!(
        approved["execution"]["corrective_approvals"],
        json!([preview])
    );
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let snapshot: Value = client
            .get(url.clone())
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
        let attempt = goal["execution"]["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["task_id"] == "fixed");
        if let Some(check) = goal["execution"]["publication"]["checks"]
            .as_array()
            .and_then(|c| c.first())
        {
            let attempt = attempt.expect("verification without corrective attempt");
            assert_eq!(attempt["base_sha"], sha);
            assert_eq!(attempt["integrated"], true);
            assert_ne!(goal["execution"]["integrated_tip"], sha);
            assert_eq!(check["verified_tip"], goal["execution"]["integrated_tip"]);
            assert_eq!(check["exit_code"], 0);
            assert_eq!(
                goal["execution"]["publication_history"],
                json!([publication])
            );
            assert!(
                fs::read_to_string(home.join("sdk-owners"))
                    .unwrap()
                    .contains("\"task_id\":\"fixed\"")
            );
            break;
        }
        assert!(
            goal["status"] != "blocked" && Instant::now() < deadline,
            "corrective lifecycle failed: goal={goal} sdk={} daemon={} gates={}",
            fs::read_to_string(home.join("sdk-owners")).unwrap_or_default(),
            fs::read_to_string(home.join("state/cruise/daemon/daemon.log")).unwrap_or_default(),
            fs::read_to_string(home.join("gate-errors")).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
