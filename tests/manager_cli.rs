#![cfg(unix)]

#[path = "support/manager_cli.rs"]
mod support;

use serde_json::json;
use std::process::{Command, Output, Stdio};
use support::{Fixture, success};

async fn register_goal(
    fixture: &Fixture,
    proposal: cruise::manager::Proposal,
) -> serde_json::Value {
    fixture
        .api(Some(json!({
            "type":"approve", "proposal_id":proposal.id, "revision":proposal.revision
        })))
        .await
}

fn failure(output: Output) -> String {
    assert!(!output.status.success(), "unexpected success: {output:?}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn help_lists_manager_subcommands() {
    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["manager", "--help"])
        .output()
        .unwrap();
    let help = success(output);
    for subcommand in [
        "status",
        "say",
        "propose",
        "approve",
        "pause",
        "resume",
        "retry",
        "answer",
        "corrective",
        "approve-corrective",
    ] {
        assert!(
            help.lines()
                .any(|line| line.split_whitespace().next() == Some(subcommand)),
            "missing {subcommand}: {help}"
        );
    }
}

#[test]
fn status_json_exposes_the_enriched_snapshot() {
    let fixture = Fixture::new();
    let snapshot = fixture.snapshot();
    assert_eq!(
        snapshot["repository"],
        json!(fixture.repo.join(".git").canonicalize().unwrap())
    );
    for key in ["transcript", "goals", "prompts"] {
        assert_eq!(snapshot[key], json!([]), "{key}: {snapshot}");
    }
}

#[test]
fn default_manager_prints_human_status_without_a_terminal() {
    let fixture = Fixture::new();
    let text = success(fixture.manager(&[]));
    assert!(text.contains("Pending prompts:"), "{text}");
    assert!(!text.contains("/help"), "obsolete REPL hint: {text}");
}

#[test]
fn manager_requires_a_local_repository() {
    let fixture = Fixture::new();
    let text = failure(
        fixture
            .command()
            .current_dir(&fixture.home)
            .args(["manager", "status"])
            .output()
            .unwrap(),
    );
    assert!(
        text.contains("manager requires a local Git repository"),
        "{text}"
    );
}

#[test]
fn say_without_text_rejects_non_git_before_stdin_eof() {
    let fixture = Fixture::new();
    let mut child = fixture.command().current_dir(&fixture.home)
        .args(["manager", "say"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("say waited for stdin outside Git");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let text = failure(child.wait_with_output().unwrap());
    drop(stdin);
    assert!(text.contains("manager requires a local Git repository"), "{text}");
}

#[test]
fn approval_without_a_pending_proposal_fails() {
    let fixture = Fixture::new();
    let text = failure(fixture.manager(&["approve"]));
    assert!(text.contains("no pending proposal"), "{text}");
}

#[tokio::test]
async fn noninteractive_approval_requires_yes_and_a_matching_revision() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_proposal();
    let revision = proposal.revision.to_string();
    let review = success(fixture.manager(&["status"]));
    assert!(review.contains("Non-interactive approval requires both --yes and --revision."));
    let wrong = (proposal.revision + 1).to_string();
    for (args, stale) in [
        (&["approve", "--revision", &revision][..], false),
        (&["approve", "--yes"][..], false),
        (&["approve", "--yes", "--revision", &wrong][..], true),
    ] {
        let text = failure(fixture.manager(args));
        assert!(
            text.contains(&proposal.objective),
            "missing proposal review: {text}"
        );
        if stale {
            assert!(text.to_lowercase().contains("stale"), "{text}");
        }
        let snapshot = fixture.api(None).await;
        assert_eq!(snapshot["goals"], json!([]));
        assert_eq!(snapshot["proposal"]["id"], proposal.id);
    }
    let output = success(fixture.manager(&["approve", "--yes", "--revision", &revision]));
    for reviewed in [&proposal.objective[..], "review-check", "contract"] {
        assert!(
            output.contains(reviewed),
            "missing reviewed field {reviewed}: {output}"
        );
    }
    let snapshot = fixture.snapshot();
    let goals = snapshot["goals"].as_array().unwrap();
    assert_eq!(goals.len(), 1, "{snapshot}");
    assert_eq!(goals[0]["status"], "active");
    assert_eq!(goals[0]["proposal"]["revision"], proposal.revision);

    let replacement = fixture.seed_proposal();
    let text = failure(fixture.manager(&["approve", "--yes", "--revision", &revision]));
    assert!(text.contains("Stale proposal revision"), "{text}");
    let pending = fixture.snapshot();
    assert_eq!(pending["proposal"]["id"], replacement.id);
    assert_eq!(pending["goals"].as_array().unwrap().len(), 1);
    assert_eq!(pending["goals"][0]["proposal"]["id"], proposal.id);
    success(fixture.manager(&["approve", "--yes", "--revision", &replacement.revision.to_string()]));
    let approved = fixture.snapshot();
    assert!(approved["proposal"].is_null());
    assert!(approved["goals"].as_array().unwrap().iter().any(|goal| goal["proposal"]["id"] == replacement.id));
}

#[tokio::test]
async fn pause_and_resume_change_the_persisted_goal_status() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_proposal();
    let active = register_goal(&fixture, proposal).await;
    let goal = active["id"].as_str().unwrap();
    let output = success(fixture.manager(&["pause", goal]));
    assert!(output.to_lowercase().contains("paused"), "{output}");
    assert!(
        serde_json::from_str::<serde_json::Value>(&output).is_err(),
        "expected human acknowledgement: {output}"
    );
    assert_eq!(fixture.api(None).await["goals"][0]["status"], "paused");
    success(fixture.manager(&["resume", goal]));
    assert_eq!(
        fixture.api(None).await["goals"][0]["status"],
        active["status"]
    );
}

#[test]
fn unknown_goal_is_a_command_failure() {
    let fixture = Fixture::new();
    let text = failure(fixture.manager(&["pause", "unregistered-goal"]));
    assert!(text.to_lowercase().contains("goal"), "{text}");
    assert!(
        !text.contains("Usage:"),
        "expected a daemon error, not a parser error: {text}"
    );
}

#[tokio::test]
async fn answer_rejects_unknown_prompt() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_proposal();
    let registered = register_goal(&fixture, proposal).await;
    let goal = registered["id"].as_str().unwrap();
    let text = failure(fixture.manager(&["answer", goal, "unregistered-prompt", "x"]));
    assert!(text.contains("Stale or unknown Manager prompt"), "{text}");
}

#[tokio::test]
async fn answer_selects_a_pending_option_by_label_and_submits_its_text() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_option_proposal();
    let goal = register_goal(&fixture, proposal).await;
    let goal_id = goal["id"].as_str().unwrap();
    let request = wait_for_prompt(&fixture, goal_id, "option").await;
    let id = request["prompt"]["requestId"].as_str().unwrap();
    for flags in [
        &[][..],
        &["--choice", "Provide a value"][..],
        &["unwanted positional text", "--choice", "Provide a value", "--text", "value"][..],
    ] {
        let mut args = vec!["answer", goal_id, id];
        args.extend_from_slice(flags);
        failure(fixture.manager(&args));
    }
    assert!(fixture.api(None).await["prompts"].as_array().unwrap().contains(&request));
    let output = success(fixture.manager(&[
        "answer", goal_id, id, "--choice", "Provide a value", "--text", "selected text",
    ]));
    assert!(output.contains(&format!("Accepted: {goal_id} [")), "{output}");
    let marker = fixture.home.join("option-answer");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(&marker)
            && text == "selected text"
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "option text did not reach the owned child");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn answer_rejects_blank_text_for_a_live_ask() {
    let fixture = Fixture::new();
    let goal = register_goal(&fixture, fixture.seed_ask_proposal()).await;
    let goal_id = goal["id"].as_str().unwrap();
    let request = wait_for_prompt(&fixture, goal_id, "ask").await;
    let id = request["prompt"]["requestId"].as_str().unwrap();
    for answer in ["", " \t "] {
        let text = failure(fixture.manager(&["answer", goal_id, id, answer]));
        assert!(text.contains("Answer must not be empty."), "{text}");
        assert!(fixture.api(None).await["prompts"].as_array().unwrap().contains(&request));
    }
    let output = success(fixture.manager(&["answer", goal_id, id, "Implement it"]));
    assert!(output.contains(&format!("Accepted: {goal_id} [")), "{output}");
}

async fn wait_for_prompt(fixture: &Fixture, goal_id: &str, kind: &str) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let snapshot = fixture.api(None).await;
        if let Some(request) = snapshot["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["goal_id"] == goal_id && entry["prompt"]["kind"] == kind)
        {
            return request.clone();
        }
        assert!(
            snapshot["goals"][0]["status"] != "blocked" && std::time::Instant::now() < deadline,
            "owned child did not reach its {kind} fixture: {snapshot}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
