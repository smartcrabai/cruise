use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn command(home: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_cruise"))
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_DAEMON_PORT")
        .stdin(Stdio::null())
        .args(args)
        .output()?)
}
struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = command(self.0, &["daemon", "stop", "--force"]);
    }
}
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

// These are persisted recovery fixtures, exercised through the authenticated production API.
#[tokio::test]
async fn retry_refuses_unsafe_recovery_states_without_mutating_them() -> Result<()> {
    let root = TempDir::new()?;
    let home = root.path();
    let repo = home.join("repo");
    fs::create_dir(&repo)?;
    git(&repo, &["init", "-q"])?;
    git(&repo, &["config", "user.name", "Test"])?;
    git(&repo, &["config", "user.email", "test@example.test"])?;
    fs::write(repo.join("file"), "base\n")?;
    git(&repo, &["add", "file"])?;
    git(&repo, &["commit", "-qm", "base"])?;
    let sha = git(&repo, &["rev-parse", "HEAD"])?;
    let common = repo.join(".git").canonicalize()?;
    let anchor = repo.canonicalize()?;
    let origin = "https://github.com/example/project.git";
    let workflow: cruise::config::WorkflowConfig = serde_json::from_value(json!({"steps":{}}))?;
    let workflow = serde_json::to_value(workflow)?;
    let proposal = json!({"id":"p", "revision":1, "objective":"goal", "target_branch":"main", "target_sha":sha,
        "exclusions":[], "criteria":[{"id":"c","description":"works"}],
        "checks":[{"id":"check","criterion_ids":["c"],"argv":["true"]}],
        "tasks":[{"id":"task","instructions":"work","dependencies":[]}], "workflow":"builtin:tdd",
        "contract":{"anchor":anchor,"origin":origin,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project","workflow":workflow}});
    let publication = json!({"tip":sha,"worktree":home.join("verification"),"branch":"cruise/goal/g", "phase":"CheckIntent", "checks":[], "pending_check":null,"request":null,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project"});
    let attempt = json!({"task_id":"task","attempt":1,"session_id":"s","base_sha":sha,"launch_started":true,"result_sha":sha,"integrated":true,"integration_intent":null});
    let make_goal = |id: &str| {
        json!({"id":id,"proposal":proposal,"status":"blocked","reason":"blocked",
        "execution":{"anchor":anchor,"origin":origin,"workflow":workflow,"integrated_tip":sha,"attempts":[attempt],"paused_from":null,"paused_reason":null,"publication":publication,"publication_history":[]}})
    };
    let mut goals = vec![];
    let mut g = make_goal("missing-attempt");
    g["execution"]["attempts"] = json!([]);
    goals.push(g);
    let mut g = make_goal("missing-publication");
    g["execution"]["publication"] = Value::Null;
    goals.push(g);
    let mut g = make_goal("tip-mismatch");
    g["execution"]["publication"]["tip"] = json!("different");
    goals.push(g);
    let mut g = make_goal("changed-contract");
    g["execution"]["origin"] = json!("different");
    goals.push(g);
    for phase in [
        "Verified",
        "PushIntent",
        "Pushed",
        "CreateIntent",
        "Published",
    ] {
        let mut g = make_goal(phase);
        g["execution"]["publication"]["phase"] = json!(phase);
        goals.push(g);
    }
    goals.push(make_goal("eligible"));
    let key: String = Sha256::digest(common.to_string_lossy().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let record = home.join("data/cruise/manager").join(format!("{key}.json"));
    fs::create_dir_all(record.parent().ok_or("record has no parent")?)?;
    fs::write(
        &record,
        serde_json::to_vec_pretty(
            &json!({"repository":common,"transcript":[],"proposal":null,"goals":goals}),
        )?,
    )?;
    let started = command(home, &["daemon", "start", "--port", "0"])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let _stop = Stop(home);
    let instance: Value =
        serde_json::from_slice(&fs::read(home.join("state/cruise/daemon/instance.json"))?)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let before: Value = serde_json::from_slice(&fs::read(&record)?)?;
    for goal in before["goals"].as_array().ok_or("goals is not an array")? {
        if goal["id"] == "eligible" {
            continue;
        }
        for _ in 0..2 {
            let response = client
                .post(format!(
                    "{}api/manager/action",
                    instance["origin"]
                        .as_str()
                        .ok_or("missing daemon credential")?
                ))
                .bearer_auth(
                    instance["token"]
                        .as_str()
                        .ok_or("missing daemon credential")?,
                )
                .json(&json!({"cwd":repo,"action":{"type":"retry","goal_id":goal["id"]}}))
                .send()
                .await?;
            assert!(
                !response.status().is_success(),
                "unsafe retry accepted for {}",
                goal["id"]
            );
        }
    }
    let after: Value = serde_json::from_slice(&fs::read(&record)?)?;
    assert_eq!(after, before);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"])?, sha);
    let response = client
        .post(format!(
            "{}api/manager/action",
            instance["origin"]
                .as_str()
                .ok_or("missing daemon credential")?
        ))
        .bearer_auth(
            instance["token"]
                .as_str()
                .ok_or("missing daemon credential")?,
        )
        .json(&json!({"cwd":repo,"action":{"type":"retry","goal_id":"eligible"}}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let goal: Value = response.json().await?;
    assert_eq!(goal["status"], "active");
    assert!(goal["execution"]["publication"].is_null());
    assert_eq!(
        goal["execution"]["publication_history"],
        json!([publication])
    );
    Ok(())
}
