#![cfg(unix)]

use cruise::manager::{self, Check, Criterion, Proposal, Task};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
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
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap_or_else(|error| panic!("operation failed: {error}"))
        .trim()
        .to_string()
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
        .env(
            "CRUISE_COMMAND",
            serde_json::to_string(&[home.join("bin/manager-agent")])
                .unwrap_or_else(|error| panic!("serialize command: {error}")),
        )
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
    let proposal = manager::save_proposal(repo, proposal)
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    fs::write(
        std::env::var_os("CORRECTIVE_OUTPUT")
            .unwrap_or_else(|| panic!("CORRECTIVE_OUTPUT must be set")),
        serde_json::to_vec(&proposal).unwrap_or_else(|error| panic!("operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("operation failed: {error}"));
}

fn setup_corrective_fixture() -> (TempDir, PathBuf, Value) {
    let root = TempDir::new().unwrap_or_else(|error| panic!("operation failed: {error}"));
    let home = root.path();
    fs::create_dir_all(home.join("config/cruise"))
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    let default_command = serde_json::to_string(&[home.join("bin/manager-agent")])
        .unwrap_or_else(|error| panic!("serialize command: {error}"));
    fs::write(
        home.join("config/cruise/default.yaml"),
        format!("command: {default_command}\ncheck_base_ci: false\n"),
    )
    .unwrap_or_else(|error| panic!("operation failed: {error}"));
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap_or_else(|error| panic!("operation failed: {error}"));
    // Gates must run the daemon's own executable, never this stale PATH one.
    fs::write(
        bin.join("cruise"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/stale-cruise\"\nexit 97\n",
    )
    .unwrap_or_else(|error| panic!("operation failed: {error}"));
    fs::set_permissions(bin.join("cruise"), fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    fs::write(
        bin.join("manager-agent"),
        r#"#!/bin/sh
frame=$(cat)
id=${PWD##*/}
cat "$HOME/data/cruise/sessions/$id/manager-owner.json" >> "$HOME/agent-calls"
printf '\n' >> "$HOME/agent-calls"
case "$(cat "$HOME/data/cruise/sessions/$id/manager-owner.json")" in
  *'"task_id":"fixed"'*) task=fixed ;;
  *) task=original ;;
esac
case "$frame" in
  *'write tests only'*) echo test > "$task-test" ;;
  *'implement production code'*) echo fixed > "$task-implementation" ;;
esac
if [ -n "${CRUISE_TDD_LEDGER_PLAN-}" ]; then
  result=$(printf '%s\n' '## Ledger Records' '```text' 'iteration: 1' '' '```')
else
  result='NO_CHANGES_INTENTIONAL: fixture completed'
fi
printf '%s\n' "$result"
"#,
    )
    .unwrap_or_else(|error| panic!("write command agent: {error}"));
    fs::set_permissions(bin.join("manager-agent"), fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("set agent permissions: {error}"));
    let repo = home.join("repo");
    fs::create_dir(&repo).unwrap_or_else(|error| panic!("operation failed: {error}"));
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n")
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    let origin = "https://github.com/example/project.git";
    git(&repo, &["remote", "add", "origin", origin]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &sha]);
    let output = home.join("proposal.json");
    let mut fixture = Command::new(
        std::env::current_exe().unwrap_or_else(|error| panic!("operation failed: {error}")),
    );
    let result = fixture
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("CORRECTIVE_REPO", &repo)
        .env("CORRECTIVE_OUTPUT", &output)
        .args(["--exact", "corrective_preview_fixture", "--nocapture"])
        .output()
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    assert!(
        result.status.success(),
        "fixture: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let proposal: Value = serde_json::from_slice(
        &fs::read(output).unwrap_or_else(|error| panic!("operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("operation failed: {error}"));
    (root, repo, proposal)
}

#[tokio::test]
async fn approved_correction_executes_and_reverifies() {
    let (root, repo, proposal) = setup_corrective_fixture();
    let home = root.path();
    let started = command(home)
        .args(["daemon", "start", "--port", "0"])
        .output()
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    assert!(
        started.status.success(),
        "daemon: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let _stop = Stop(home);
    let instance: Value = serde_json::from_slice(
        &fs::read(home.join("state/cruise/daemon/instance.json"))
            .unwrap_or_else(|error| panic!("operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("operation failed: {error}"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    let origin_url = instance["origin"]
        .as_str()
        .unwrap_or_else(|| panic!("daemon origin missing"));
    let token = instance["token"]
        .as_str()
        .unwrap_or_else(|| panic!("daemon token missing"));
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
    .unwrap_or_else(|error| panic!("operation failed: {error}"));
    assert!(
        response.status().is_success(),
        "initial approval: {}",
        response
            .text()
            .await
            .unwrap_or_else(|error| panic!("operation failed: {error}"))
    );
    let initial: Value = response
        .json()
        .await
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    let goal_id = initial["id"]
        .as_str()
        .unwrap_or_else(|| panic!("goal id missing"))
        .to_owned();
    let mut url = reqwest::Url::parse(&format!("{origin_url}api/manager/snapshot"))
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    url.query_pairs_mut().append_pair(
        "cwd",
        repo.to_str()
            .unwrap_or_else(|| panic!("repository path is not UTF-8")),
    );
    let (publication, sha) = wait_for_blocked_goal(&client, &url, token, home).await;
    let response = post(json!({"type":"preview_corrective","goal_id":goal_id,"expected_tip":sha,"task":{"id":"fixed","instructions":"Fix the failed check","dependencies":["original"]}})).await.unwrap_or_else(|error| panic!("operation failed: {error}"));
    assert!(
        response.status().is_success(),
        "preview: {}",
        response
            .text()
            .await
            .unwrap_or_else(|error| panic!("operation failed: {error}"))
    );
    let preview: Value = response
        .json()
        .await
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    let response = post(json!({"type":"approve_corrective","goal_id":goal_id,"preview_id":preview["id"],"revision":preview["revision"]})).await.unwrap_or_else(|error| panic!("operation failed: {error}"));
    assert!(
        response.status().is_success(),
        "approval: {}",
        response
            .text()
            .await
            .unwrap_or_else(|error| panic!("operation failed: {error}"))
    );
    let approved: Value = response
        .json()
        .await
        .unwrap_or_else(|error| panic!("operation failed: {error}"));
    assert_eq!(
        approved["execution"]["publication_history"],
        json!([publication])
    );
    assert_eq!(
        approved["execution"]["corrective_approvals"],
        json!([preview])
    );
    verify_corrective_completion(&client, &url, token, home, &sha, &publication).await;
}

async fn wait_for_blocked_goal(
    client: &reqwest::Client,
    url: &reqwest::Url,
    token: &str,
    home: &Path,
) -> (Value, String) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let snapshot: Value = client
            .get(url.clone())
            .bearer_auth(token)
            .send()
            .await
            .unwrap_or_else(|error| panic!("operation failed: {error}"))
            .error_for_status()
            .unwrap_or_else(|error| panic!("operation failed: {error}"))
            .json()
            .await
            .unwrap_or_else(|error| panic!("operation failed: {error}"));
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
                    .unwrap_or_else(|| panic!("integrated tip missing"))
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
    }
}

async fn verify_corrective_completion(
    client: &reqwest::Client,
    url: &reqwest::Url,
    token: &str,
    home: &Path,
    sha: &str,
    publication: &Value,
) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let snapshot: Value = client
            .get(url.clone())
            .bearer_auth(token)
            .send()
            .await
            .unwrap_or_else(|error| panic!("operation failed: {error}"))
            .error_for_status()
            .unwrap_or_else(|error| panic!("operation failed: {error}"))
            .json()
            .await
            .unwrap_or_else(|error| panic!("operation failed: {error}"));
        let goal = &snapshot["goals"][0];
        let attempt = goal["execution"]["attempts"]
            .as_array()
            .unwrap_or_else(|| panic!("attempts missing"))
            .iter()
            .find(|a| a["task_id"] == "fixed");
        if let Some(check) = goal["execution"]["publication"]["checks"]
            .as_array()
            .and_then(|c| c.first())
        {
            let attempt =
                attempt.unwrap_or_else(|| panic!("verification without corrective attempt"));
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
                fs::read_to_string(home.join("agent-calls"))
                    .unwrap_or_else(|error| panic!("operation failed: {error}"))
                    .contains("\"task_id\":\"fixed\"")
            );
            break;
        }
        assert!(
            goal["status"] != "blocked" && Instant::now() < deadline,
            "corrective lifecycle failed: goal={goal} agent-calls={} daemon={} stale-cruise={}",
            fs::read_to_string(home.join("agent-calls")).unwrap_or_default(),
            fs::read_to_string(home.join("state/cruise/daemon/daemon.log")).unwrap_or_default(),
            fs::read_to_string(home.join("stale-cruise")).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
