use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::Duration,
};
use tempfile::TempDir;

fn cruise(home: &Path, args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    cruise_command(home).args(args).output()
}

fn manager_command(home: &Path, repo: &Path) -> Command {
    let mut command = cruise_command(home);
    command.current_dir(repo).arg("manager");
    command
}

fn cruise_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    command
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_DAEMON_PORT")
        .stdin(Stdio::null());
    command
}
struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = cruise(self.0, &["daemon", "stop", "--force"]);
    }
}
fn git(repo: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

fn repository(home: &Path) -> Result<(std::path::PathBuf, String), Box<dyn std::error::Error>> {
    let repo = home.join("repo");
    fs::create_dir(&repo)?;
    git(&repo, &["init", "-q"])?;
    git(&repo, &["config", "user.name", "Test"])?;
    git(&repo, &["config", "user.email", "test@example.test"])?;
    fs::write(repo.join("file"), "base\n")?;
    git(&repo, &["add", "file"])?;
    git(&repo, &["commit", "-qm", "base"])?;
    let sha = git(&repo, &["rev-parse", "HEAD"])?;
    Ok((repo, sha))
}

fn seed_failed_goal(home: &Path) -> Result<(PathBuf, String, Value), Box<dyn std::error::Error>> {
    seed_legacy_failed_goal(home, false)
}

fn seed_legacy_failed_goal(
    home: &Path,
    with_preview: bool,
) -> Result<(PathBuf, String, Value), Box<dyn std::error::Error>> {
    let (repo, sha) = repository(home)?;
    let anchor = repo.canonicalize()?;
    let origin = "https://github.com/example/project.git";
    let workflow: cruise::config::WorkflowConfig = serde_json::from_value(json!({"steps":{}}))?;
    let workflow = serde_json::to_value(workflow)?;
    let proposal = json!({"id":"p","revision":1,"objective":"goal","target_branch":"main","target_sha":sha,
        "exclusions":[],"criteria":[{"id":"c","description":"works"}],
        "checks":[{"id":"check","criterion_ids":["c"],"argv":["false"]}],
        "tasks":[{"id":"task","instructions":"work","dependencies":[]}],"workflow":"builtin:tdd",
        "contract":{"anchor":anchor,"origin":origin,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project","workflow":workflow}});
    let evidence = json!({"check_id":"check","verified_tip":sha,"argv":["false"],"exit_code":1,"cancelled":false,"stdout_log":"out","stderr_log":"err","state_error":null});
    let publication = json!({"tip":sha,"worktree":home.join("verification"),"branch":"cruise/goal/g","phase":"Ready","checks":[evidence],"pending_check":null,"request":null,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project"});
    let attempts = json!([{"task_id":"task","attempt":1,"session_id":"s","base_sha":sha,"launch_started":true,"resume_pending":false,"resume_starting":false,"result_sha":sha,"finalization_intent":null,"integrated":true,"integration_intent":null}]);
    let mut goal = json!({"id":"g","proposal":proposal,"status":"blocked","reason":"failed",
        "execution":{"anchor":anchor,"origin":origin,"workflow":workflow,"integrated_tip":sha,"attempts":attempts,"paused_from":null,"paused_reason":null,"publication":publication,"publication_history":[]}});
    if with_preview {
        goal["execution"]["corrective_preview"] = json!({
            "id":"legacy-preview", "revision":1, "goal_id":"g", "integrated_tip":sha,
            "task":{"id":"fix","instructions":"Repair the check","dependencies":["task"]}
        });
    }
    write_goal(home, &repo, &goal)?;
    Ok((repo, sha, goal))
}

fn write_goal(home: &Path, repo: &Path, goal: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let common = repo.join(".git").canonicalize()?;
    let mut key = String::new();
    for byte in Sha256::digest(common.to_string_lossy().as_bytes()) {
        write!(key, "{byte:02x}")?;
    }
    let record = home.join("data/cruise/manager").join(format!("{key}.json"));
    fs::create_dir_all(record.parent().ok_or("record has no parent")?)?;
    fs::write(
        &record,
        serde_json::to_vec(
            &json!({"repository":common,"transcript":[],"proposal":null,"goals":[goal]}),
        )?,
    )?;
    Ok(())
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{output:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap_or_else(|err| panic!("decode Manager stdout: {err}"))
}

fn assert_attempts_preserved(after: &Value, before: &Value) {
    let actual = after
        .as_array()
        .unwrap_or_else(|| panic!("read actual attempts"));
    let expected = before
        .as_array()
        .unwrap_or_else(|| panic!("read expected attempts"));
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        // The daemon enriches these records with live child state in status output.
        for (field, value) in expected
            .as_object()
            .unwrap_or_else(|| panic!("read expected attempt fields"))
        {
            assert_eq!(&actual[field], value, "changed attempt field {field}");
        }
    }
}

#[test]
fn manager_retry_goal_preserves_failed_verification_without_retrying_tasks()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let home = root.path();
    let (repo, _, before) = seed_failed_goal(home)?;
    success(cruise(home, &["daemon", "start", "--port", "0"])?);
    let _stop = Stop(home);
    let manager = |args: &[&str]| manager_command(home, &repo).args(args).output();
    success(manager(&["retry", "g"])?);
    let snapshot: Value = serde_json::from_str(&success(manager(&["status", "--json"])?))?;
    let after = &snapshot["goals"][0]["execution"];
    assert_attempts_preserved(&after["attempts"], &before["execution"]["attempts"]);
    assert_eq!(
        after["publication_history"],
        json!([before["execution"]["publication"]])
    );
    assert_eq!(after["corrective_discarded"], json!([]));
    Ok(())
}

#[test]
fn manager_retry_clears_legacy_corrective_preview_from_status_and_audits_it()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let home = root.path();
    let (repo, _, before) = seed_legacy_failed_goal(home, true)?;
    success(cruise(home, &["daemon", "start", "--port", "0"])?);
    let _stop = Stop(home);
    let manager = |args: &[&str]| manager_command(home, &repo).args(args).output();
    let notice = "Corrective preview is not approved";
    assert!(success(manager(&["status"])?).contains(notice));

    success(manager(&["retry", "g"])?);
    let text = success(manager(&["status"])?);
    let snapshot: Value = serde_json::from_str(&success(manager(&["status", "--json"])?))?;
    let after = &snapshot["goals"][0]["execution"];
    assert_eq!(
        (
            text.contains(notice),
            after.get("corrective_preview"),
            after.get("corrective_discarded"),
        ),
        (
            false,
            Some(&Value::Null),
            Some(&json!([before["execution"]["corrective_preview"]])),
        ),
        "retry must retire the preview from status while preserving its audit entry"
    );
    Ok(())
}

#[test]
fn manager_retry_appends_to_corrective_discard_history() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let home = root.path();
    let (repo, sha, mut goal) = seed_legacy_failed_goal(home, true)?;
    let prior = goal["execution"]["corrective_preview"].clone();
    let pending = json!({
        "id":"newer-preview", "revision":2, "goal_id":"g", "integrated_tip":sha,
        "task":{"id":"fix-again","instructions":"Repair the check again","dependencies":["task"]}
    });
    goal["execution"]["corrective_discarded"] = json!([prior]);
    goal["execution"]["corrective_preview"] = pending.clone();
    write_goal(home, &repo, &goal)?;

    success(cruise(home, &["daemon", "start", "--port", "0"])?);
    let _stop = Stop(home);
    let manager = |args: &[&str]| manager_command(home, &repo).args(args).output();
    success(manager(&["retry", "g"])?);
    let snapshot: Value = serde_json::from_str(&success(manager(&["status", "--json"])?))?;
    let after = &snapshot["goals"][0]["execution"];
    assert_eq!(after.get("corrective_preview"), Some(&Value::Null));
    assert_eq!(after["corrective_discarded"], json!([prior, pending]));
    Ok(())
}

#[tokio::test]
async fn corrective_approval_requires_current_review_and_preserves_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let home = root.path();
    let (repo, sha, goal) = seed_failed_goal(home)?;
    let proposal = &goal["proposal"];
    let attempts = &goal["execution"]["attempts"];
    let publication = &goal["execution"]["publication"];
    let start = cruise(home, &["daemon", "start", "--port", "0"])?;
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let _stop = Stop(home);
    let instance: Value =
        serde_json::from_slice(&fs::read(home.join("state/cruise/daemon/instance.json"))?)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let url = format!(
        "{}api/manager/action",
        instance["origin"].as_str().ok_or("missing origin")?
    );
    let token = instance["token"].as_str().ok_or("missing token")?;
    let post = |action: Value| {
        client
            .post(&url)
            .bearer_auth(token)
            .json(&json!({"cwd":repo,"action":action}))
            .send()
    };
    let preview: Value = post(json!({"type":"preview_corrective","goal_id":"g","expected_tip":sha,"task":{"id":"fix","instructions":"fix it","dependencies":["task"]}})).await?.json().await?;
    let id = preview["id"].as_str().ok_or("missing preview id")?;
    let rev = preview["revision"]
        .as_u64()
        .ok_or("missing preview revision")?;
    let stale =
        post(json!({"type":"approve_corrective","goal_id":"g","preview_id":id,"revision":rev + 1}))
            .await?;
    assert!(!stale.status().is_success());
    let approved =
        post(json!({"type":"approve_corrective","goal_id":"g","preview_id":id,"revision":rev}))
            .await?;
    assert_eq!(approved.status(), StatusCode::ACCEPTED);
    let approved: Value = approved.json().await?;
    assert_eq!(approved["proposal"]["criteria"], proposal["criteria"]);
    assert_eq!(approved["proposal"]["checks"], proposal["checks"]);
    assert_eq!(&approved["execution"]["attempts"], attempts);
    assert_eq!(
        approved["execution"]["publication_history"],
        json!([publication])
    );
    assert_eq!(
        approved["execution"]["corrective_approvals"],
        json!([preview])
    );
    assert_eq!(
        approved["proposal"]["tasks"]
            .as_array()
            .ok_or("missing approved tasks")?
            .len(),
        2
    );
    let repeated =
        post(json!({"type":"approve_corrective","goal_id":"g","preview_id":id,"revision":rev}))
            .await?;
    assert_eq!(repeated.status(), StatusCode::ACCEPTED);
    let repeated: Value = repeated.json().await?;
    assert_eq!(
        repeated["proposal"]["tasks"]
            .as_array()
            .ok_or("missing repeated tasks")?
            .len(),
        2
    );
    Ok(())
}

#[test]
fn corrective_cli_requires_explicit_approval_of_the_current_preview()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let home = root.path();
    let (repo, sha, original) = seed_failed_goal(home)?;
    success(cruise(home, &["daemon", "start", "--port", "0"])?);
    let _stop = Stop(home);
    let manager = |args: &[&str]| manager_command(home, &repo).args(args).output();
    let goal = || -> Result<Value, Box<dyn std::error::Error>> {
        let snapshot: Value = serde_json::from_str(&success(manager(&["status", "--json"])?))?;
        assert_eq!(snapshot["goals"][0]["id"], "g");
        Ok(snapshot["goals"][0].clone())
    };
    let review = |instructions: &str| -> Result<Value, Box<dyn std::error::Error>> {
        Ok(assert_corrective_preview(&goal()?, &original, &sha, instructions).clone())
    };

    let flag_instructions = "Repair the failed check from a flag";
    let review_output = success(manager(&[
        "corrective",
        "g",
        "fix",
        "--instructions",
        flag_instructions,
    ])?);
    let first = review(flag_instructions)?;
    let saved = review_output
        .split_once("Saved corrective preview:")
        .unwrap_or_else(|| panic!("find saved corrective preview"))
        .1;
    for expected in [
        format!("\"revision\": {}", first["revision"]),
        format!("\"integrated_tip\": \"{sha}\""),
        "\"id\": \"fix\"".into(),
        flag_instructions.into(),
        "Corrective preview is not approved".into(),
        "Accepted: g [blocked]".into(),
    ] {
        assert!(saved.contains(&expected), "missing {expected}: {saved}");
    }

    let stdin_instructions = "Repair the failed check from stdin\nKeep the existing criteria";
    let mut child = manager_command(home, &repo)
        .args(["corrective", "g", "fix"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing stdin pipe")?
        .write_all(stdin_instructions.as_bytes())?;
    success(child.wait_with_output()?);
    let preview = review(stdin_instructions)?;
    let revision = preview["revision"]
        .as_u64()
        .ok_or("missing preview revision")?;
    let stale = first["revision"].as_u64().ok_or("missing first revision")?;
    assert!(revision > stale);
    let revision = revision.to_string();
    let stale = stale.to_string();
    for flags in [
        &["--revision", &revision][..],
        &["--yes"][..],
        &["--yes", "--revision", &stale][..],
    ] {
        let mut args = vec!["approve-corrective", "g"];
        args.extend_from_slice(flags);
        let output = manager(&args)?;
        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert_eq!(review(stdin_instructions)?, preview);
    }

    success(manager(&[
        "approve-corrective",
        "g",
        "--yes",
        "--revision",
        &revision,
    ])?);
    let approved = goal()?;
    assert!(approved["execution"]["corrective_preview"].is_null());
    assert_eq!(
        approved["execution"]["corrective_approvals"],
        json!([preview])
    );
    assert_eq!(
        approved["proposal"]["tasks"],
        json!([original["proposal"]["tasks"][0], preview["task"]])
    );

    verify_corrective_revision_cannot_be_replayed(
        home, &manager, &goal, &original, &preview, &revision,
    )
}

fn assert_corrective_preview<'a>(
    current: &'a Value,
    original: &Value,
    sha: &str,
    instructions: &str,
) -> &'a Value {
    assert_eq!(current["status"], original["status"]);
    assert_eq!(current["proposal"]["tasks"], original["proposal"]["tasks"]);
    assert_attempts_preserved(
        &current["execution"]["attempts"],
        &original["execution"]["attempts"],
    );
    assert_eq!(
        current["execution"]["publication"],
        original["execution"]["publication"]
    );
    let approvals = &current["execution"]["corrective_approvals"];
    assert!(approvals.is_null() || *approvals == json!([]), "{current}");
    let preview = &current["execution"]["corrective_preview"];
    assert_eq!(preview["task"]["id"], "fix");
    assert_eq!(preview["task"]["instructions"], instructions);
    assert_eq!(preview["integrated_tip"], sha);
    preview
}

fn verify_corrective_revision_cannot_be_replayed(
    home: &Path,
    manager: &impl Fn(&[&str]) -> std::io::Result<Output>,
    goal: &impl Fn() -> Result<Value, Box<dyn std::error::Error>>,
    original: &Value,
    preview: &Value,
    revision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Advance the durable fixture to the next failed check, preserving approval history.
    success(cruise(home, &["daemon", "stop", "--force"])?);
    let record = fs::read_dir(home.join("data/cruise/manager"))?
        .next()
        .ok_or("missing record")??
        .path();
    let mut persisted: Value = serde_json::from_slice(&fs::read(&record)?)?;
    let next = &mut persisted["goals"][0];
    next["status"] = json!("blocked");
    next["execution"]["publication"] = original["execution"]["publication"].clone();
    let mut attempt = original["execution"]["attempts"][0].clone();
    attempt["task_id"] = json!("fix");
    attempt["session_id"] = json!("second-session");
    next["execution"]["attempts"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("read seeded attempts"))
        .retain(|attempt| attempt["task_id"] != "fix");
    next["execution"]["attempts"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("read seeded attempts"))
        .push(attempt);
    fs::write(record, serde_json::to_vec(&persisted)?)?;
    success(cruise(home, &["daemon", "start", "--port", "0"])?);
    success(manager(&[
        "corrective",
        "g",
        "fix-again",
        "--instructions",
        "Try a different repair",
    ])?);
    let pending = goal()?;
    let next_preview = &pending["execution"]["corrective_preview"];
    let replay = manager(&["approve-corrective", "g", "--yes", "--revision", revision])?;
    assert!(!replay.status.success());
    assert!(String::from_utf8_lossy(&replay.stderr).contains("Stale proposal revision"));
    let after_replay = goal()?;
    assert_eq!(
        &after_replay["execution"]["corrective_preview"],
        next_preview
    );
    assert_eq!(
        after_replay["execution"]["corrective_approvals"],
        json!([preview])
    );
    let next_revision = next_preview["revision"].to_string();
    success(manager(&[
        "approve-corrective",
        "g",
        "--yes",
        "--revision",
        &next_revision,
    ])?);
    let after_approval = goal()?;
    assert!(after_approval["execution"]["corrective_preview"].is_null());
    assert_eq!(
        after_approval["execution"]["corrective_approvals"],
        json!([preview, next_preview])
    );
    Ok(())
}
