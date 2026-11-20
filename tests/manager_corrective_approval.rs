use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tempfile::TempDir;

fn cruise(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cruise"))
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_DAEMON_PORT")
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap()
}
struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = cruise(self.0, &["daemon", "stop", "--force"]);
    }
}
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
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
async fn corrective_approval_requires_current_review_and_preserves_evidence() {
    let root = TempDir::new().unwrap();
    let home = root.path();
    let repo = home.join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n").unwrap();
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    let common = repo.join(".git").canonicalize().unwrap();
    let anchor = repo.canonicalize().unwrap();
    let origin = "https://github.com/example/project.git";
    let workflow: cruise::config::WorkflowConfig =
        serde_json::from_value(json!({"steps":{}})).unwrap();
    let workflow = serde_json::to_value(workflow).unwrap();
    let proposal = json!({"id":"p","revision":1,"objective":"goal","target_branch":"main","target_sha":sha,
        "exclusions":[],"criteria":[{"id":"c","description":"works"}],
        "checks":[{"id":"check","criterion_ids":["c"],"argv":["false"]}],
        "tasks":[{"id":"task","instructions":"work","dependencies":[]}],"workflow":"builtin:tdd",
        "contract":{"anchor":anchor,"origin":origin,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project","workflow":workflow}});
    let evidence = json!({"check_id":"check","verified_tip":sha,"argv":["false"],"exit_code":1,"cancelled":false,"stdout_log":"out","stderr_log":"err","state_error":null});
    let publication = json!({"tip":sha,"worktree":home.join("verification"),"branch":"cruise/goal/g","phase":"Ready","checks":[evidence],"pending_check":null,"request":null,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project"});
    let attempts = json!([{"task_id":"task","attempt":1,"session_id":"s","base_sha":sha,"launch_started":true,"resume_pending":false,"resume_starting":false,"result_sha":sha,"finalization_intent":null,"integrated":true,"integration_intent":null}]);
    let goal = json!({"id":"g","proposal":proposal,"status":"blocked","reason":"failed",
        "execution":{"anchor":anchor,"origin":origin,"workflow":workflow,"integrated_tip":sha,"attempts":attempts,"paused_from":null,"paused_reason":null,"publication":publication,"publication_history":[]}});
    let key: String = Sha256::digest(common.to_string_lossy().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let record = home.join("data/cruise/manager").join(format!("{key}.json"));
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(
        &record,
        serde_json::to_vec(
            &json!({"repository":common,"transcript":[],"proposal":null,"goals":[goal]}),
        )
        .unwrap(),
    )
    .unwrap();
    let start = cruise(home, &["daemon", "start", "--port", "0"]);
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let _stop = Stop(home);
    let instance: Value =
        serde_json::from_slice(&fs::read(home.join("state/cruise/daemon/instance.json")).unwrap())
            .unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let url = format!("{}api/manager/action", instance["origin"].as_str().unwrap());
    let token = instance["token"].as_str().unwrap();
    let post = |action: Value| {
        client
            .post(&url)
            .bearer_auth(token)
            .json(&json!({"cwd":repo,"action":action}))
            .send()
    };
    let preview: Value = post(json!({"type":"preview_corrective","goal_id":"g","expected_tip":sha,"task":{"id":"fix","instructions":"fix it","dependencies":["task"]}})).await.unwrap().json().await.unwrap();
    let id = preview["id"].as_str().unwrap();
    let rev = preview["revision"].as_u64().unwrap();
    let stale =
        post(json!({"type":"approve_corrective","goal_id":"g","preview_id":id,"revision":rev + 1}))
            .await
            .unwrap();
    assert!(!stale.status().is_success());
    let approved =
        post(json!({"type":"approve_corrective","goal_id":"g","preview_id":id,"revision":rev}))
            .await
            .unwrap();
    assert_eq!(approved.status(), StatusCode::ACCEPTED);
    let approved: Value = approved.json().await.unwrap();
    assert_eq!(approved["proposal"]["criteria"], proposal["criteria"]);
    assert_eq!(approved["proposal"]["checks"], proposal["checks"]);
    assert_eq!(approved["execution"]["attempts"], attempts);
    assert_eq!(
        approved["execution"]["publication_history"],
        json!([publication])
    );
    assert_eq!(
        approved["execution"]["corrective_approvals"],
        json!([preview])
    );
    assert_eq!(approved["proposal"]["tasks"].as_array().unwrap().len(), 2);
    let repeated =
        post(json!({"type":"approve_corrective","goal_id":"g","preview_id":id,"revision":rev}))
            .await
            .unwrap();
    assert_eq!(repeated.status(), StatusCode::ACCEPTED);
    let repeated: Value = repeated.json().await.unwrap();
    assert_eq!(repeated["proposal"]["tasks"].as_array().unwrap().len(), 2);
}
