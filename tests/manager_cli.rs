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

fn failure(output: &Output) -> String {
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
        .unwrap_or_else(|err| panic!("run manager help: {err}"));
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
        json!(
            fixture
                .repo
                .join(".git")
                .canonicalize()
                .unwrap_or_else(|err| panic!("canonicalize fixture Git directory: {err}"))
        )
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
        &fixture
            .command()
            .current_dir(&fixture.home)
            .args(["manager", "status"])
            .output()
            .unwrap_or_else(|err| panic!("run status outside Git: {err}")),
    );
    assert!(
        text.contains("manager requires a local Git repository"),
        "{text}"
    );
}

#[test]
fn say_without_text_rejects_non_git_before_stdin_eof() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .current_dir(&fixture.home)
        .args(["manager", "say"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("spawn manager say: {err}"));
    let stdin = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("open child stdin"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child
        .try_wait()
        .unwrap_or_else(|err| panic!("poll manager process: {err}"))
        .is_none()
    {
        if std::time::Instant::now() >= deadline {
            child
                .kill()
                .unwrap_or_else(|err| panic!("kill stalled manager: {err}"));
            child
                .wait()
                .unwrap_or_else(|err| panic!("reap stalled manager: {err}"));
            panic!("say waited for stdin outside Git");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let text = failure(
        &child
            .wait_with_output()
            .unwrap_or_else(|err| panic!("collect manager output: {err}")),
    );
    drop(stdin);
    assert!(
        text.contains("manager requires a local Git repository"),
        "{text}"
    );
}

#[test]
fn approval_without_a_pending_proposal_fails() {
    let fixture = Fixture::new();
    let text = failure(&fixture.manager(&["approve"]));
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
        let text = failure(&fixture.manager(args));
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
    let goals = snapshot["goals"]
        .as_array()
        .unwrap_or_else(|| panic!("read approved goals"));
    assert_eq!(goals.len(), 1, "{snapshot}");
    assert_eq!(goals[0]["status"], "active");
    assert_eq!(goals[0]["proposal"]["revision"], proposal.revision);

    let replacement = fixture.seed_proposal();
    let text = failure(&fixture.manager(&["approve", "--yes", "--revision", &revision]));
    assert!(text.contains("Stale proposal revision"), "{text}");
    let pending = fixture.snapshot();
    assert_eq!(pending["proposal"]["id"], replacement.id);
    assert_eq!(
        pending["goals"]
            .as_array()
            .unwrap_or_else(|| panic!("read pending goals"))
            .len(),
        1
    );
    assert_eq!(pending["goals"][0]["proposal"]["id"], proposal.id);
    success(fixture.manager(&[
        "approve",
        "--yes",
        "--revision",
        &replacement.revision.to_string(),
    ]));
    let approved = fixture.snapshot();
    assert!(approved["proposal"].is_null());
    assert!(
        approved["goals"]
            .as_array()
            .unwrap_or_else(|| panic!("read replacement goals"))
            .iter()
            .any(|goal| goal["proposal"]["id"] == replacement.id)
    );
}

#[tokio::test]
async fn pause_and_resume_change_the_persisted_goal_status() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_proposal();
    let active = register_goal(&fixture, proposal).await;
    let goal = active["id"]
        .as_str()
        .unwrap_or_else(|| panic!("find active goal ID"));
    let output = success(fixture.manager(&["pause", goal]));
    assert!(output.to_lowercase().contains("paused"), "{output}");
    assert!(
        serde_json::from_str::<serde_json::Value>(&output).is_err(),
        "expected human acknowledgement: {output}"
    );
    let paused = fixture.api(None).await;
    assert_eq!(paused["goals"][0]["status"], "paused");
    // Pause records the status it interrupted. The coordinator may already
    // have blocked the approved Goal by then, and wakes again on resume, so
    // the persisted status can move on after resume returns.
    let interrupted = paused["goals"][0]["execution"]["paused_from"]
        .as_str()
        .unwrap_or_else(|| panic!("find paused_from: {paused}"))
        .to_owned();
    assert!(
        ["active", "blocked"].contains(&interrupted.as_str()),
        "{paused}"
    );
    let resumed = success(fixture.manager(&["resume", goal]));
    assert!(
        resumed.contains(&format!("[{interrupted}]")),
        "resume must restore {interrupted}: {resumed}"
    );
}

#[test]
fn unknown_goal_is_a_command_failure() {
    let fixture = Fixture::new();
    let text = failure(&fixture.manager(&["pause", "unregistered-goal"]));
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
    let goal = registered["id"]
        .as_str()
        .unwrap_or_else(|| panic!("find registered goal ID"));
    let text = failure(&fixture.manager(&["answer", goal, "unregistered-prompt", "x"]));
    assert!(text.contains("Stale or unknown Manager prompt"), "{text}");
}

#[tokio::test]
async fn answer_selects_a_pending_option_by_label_and_submits_its_text() {
    let fixture = Fixture::new();
    let proposal = fixture.seed_option_proposal();
    let goal = register_goal(&fixture, proposal).await;
    let goal_id = goal["id"]
        .as_str()
        .unwrap_or_else(|| panic!("find Option goal ID"));
    let request = wait_for_prompt(&fixture, goal_id, "option").await;
    let id = request["prompt"]["requestId"]
        .as_str()
        .unwrap_or_else(|| panic!("find Option request ID"));
    for flags in [
        &[][..],
        &["--choice", "Provide a value"][..],
        &[
            "unwanted positional text",
            "--choice",
            "Provide a value",
            "--text",
            "value",
        ][..],
    ] {
        let mut args = vec!["answer", goal_id, id];
        args.extend_from_slice(flags);
        failure(&fixture.manager(&args));
    }
    assert!(
        fixture.api(None).await["prompts"]
            .as_array()
            .unwrap_or_else(|| panic!("read pending prompts"))
            .contains(&request)
    );
    let output = success(fixture.manager(&[
        "answer",
        goal_id,
        id,
        "--choice",
        "Provide a value",
        "--text",
        "selected text",
    ]));
    assert!(
        output.contains(&format!("Accepted: {goal_id} [")),
        "{output}"
    );
    let marker = fixture.home.join("option-answer");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(&marker)
            && text == "selected text"
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "option text did not reach the owned child"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test(worker_threads = 2, flavor = "multi_thread")]
async fn answer_rejects_blank_text_for_a_pending_ask() {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    let fixture = Fixture::new();
    // Owned children always start with the approved plan as input. That makes
    // an instruction step skip asking, so supply the pending Ask at the HTTP
    // boundary rather than relying on an unreachable workflow fixture.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|err| panic!("bind Ask fixture: {err}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|err| panic!("read Ask fixture address: {err}"));
    let state = fixture.home.join("ask-client-state");
    let record = "cruise/daemon/instance.json";
    let mut instance: Value = serde_json::from_slice(
        &std::fs::read(fixture.home.join("state").join(record))
            .unwrap_or_else(|err| panic!("read daemon instance: {err}")),
    )
    .unwrap_or_else(|err| panic!("decode daemon instance: {err}"));
    instance["origin"] = json!(format!("http://{addr}/"));
    instance["port"] = json!(addr.port());
    std::fs::create_dir_all(state.join("cruise/daemon"))
        .unwrap_or_else(|err| panic!("create Ask client registry: {err}"));
    std::fs::write(state.join(record), instance.to_string())
        .unwrap_or_else(|err| panic!("write Ask fixture registry: {err}"));

    let submissions = Arc::new(Mutex::new(Vec::<Value>::new()));
    let received = Arc::clone(&submissions);
    let pending = Arc::clone(&submissions);
    let router = Router::new()
        .route("/api/v1/status", get(|| async {
            Json(json!({"version":env!("CARGO_PKG_VERSION"), "activeOperations":0, "batchActive":false}))
        }))
        .route("/api/manager/snapshot", get(move || async move {
            let prompts = if pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty() {
                json!([{"goal_id":"goal", "prompt":{
                    "kind":"ask", "requestId":"ask", "sessionId":"child",
                    "question":"What should the Goal do?", "choices":[]
                }}])
            } else {
                json!([])
            };
            Json(json!({"goals":[{"id":"goal", "status":"active"}], "prompts":prompts}))
        }))
        .route("/api/manager/action", post(move |Json(body): Json<Value>| async move {
            received.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(body);
            Json(json!({}))
        }));
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let answer = |args: &[&str]| {
        fixture
            .command()
            .env("XDG_STATE_HOME", &state)
            .args(["manager", "answer", "goal", "ask"])
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("run Manager answer: {err}"))
    };
    for args in [&[][..], &[""][..], &[" \t "][..]] {
        let text = failure(&answer(args));
        assert!(text.contains("Answer must not be empty."), "{text}");
        assert!(
            submissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }
    let text = failure(&answer(&["--choice", "Implement it"]));
    assert!(text.contains("apply only to Option prompts"), "{text}");
    assert!(
        submissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    let output = success(answer(&["Implement it"]));
    assert!(output.contains("Accepted: goal ["), "{output}");
    assert_eq!(
        *submissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [
            json!({"cwd":fixture.repo.canonicalize().unwrap_or_else(|err| panic!("resolve repo: {err}")), "action":{
                "type":"answer", "goal_id":"goal", "prompt_id":"ask", "answer":"Implement it"
            }})
        ]
    );
    server.abort();
}

async fn wait_for_prompt(fixture: &Fixture, goal_id: &str, kind: &str) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let snapshot = fixture.api(None).await;
        if let Some(request) = snapshot["prompts"]
            .as_array()
            .unwrap_or_else(|| panic!("read snapshot prompts"))
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
