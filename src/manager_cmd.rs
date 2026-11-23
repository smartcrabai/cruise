use std::io::{IsTerminal, Read};
use std::time::Duration;

use serde_json::{Value, json};

use crate::application::{OptionChoiceKind, PendingPrompt, PendingPromptKind};
use crate::cli::{ManagerApprovalArgs, ManagerArgs, ManagerCommand};
use crate::daemon::client::DaemonClient;
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::step::option::OptionResult;

fn text(key: &str) -> String {
    crate::i18n::t(crate::i18n::process_locale(), &format!("cli.manager.{key}"))
}

fn error(key: &str) -> CruiseError {
    CruiseError::Other(text(key))
}

fn confirm_review() -> Result<bool> {
    Ok(
        matches!(prompt_multiline(&text("confirm"))?, InputResult::Submitted(confirm) if confirm == "approve"),
    )
}

pub(crate) async fn run(args: ManagerArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let cwd = cwd.to_str().ok_or_else(|| error("utf8_required"))?;
    let client = DaemonClient::connect().await?;
    let tty = std::io::stdin().is_terminal();
    let action = match args
        .command
        .unwrap_or(ManagerCommand::Status { json: false })
    {
        ManagerCommand::Status { json } => {
            let snapshot = client.manager_snapshot(cwd).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                show_snapshot(&snapshot)?;
            }
            return Ok(());
        }
        ManagerCommand::Say {
            text: message,
            no_wait,
        } => {
            if message.is_none() {
                crate::manager::repository(std::path::Path::new(cwd))?;
            }
            let message = read_input(message, tty, "message_input")?;
            return conversation(
                &client,
                cwd,
                json!({"type":"message", "text":message.trim()}),
                no_wait,
                false,
            )
            .await;
        }
        ManagerCommand::Propose { no_wait } => {
            return conversation(&client, cwd, json!({"type":"propose"}), no_wait, true).await;
        }
        ManagerCommand::Approve(approval) => {
            let snapshot = client.manager_snapshot(cwd).await?;
            approve_proposal(&snapshot, &approval, tty)?
        }
        ManagerCommand::Pause { goal } => json!({"type":"pause", "goal_id":goal}),
        ManagerCommand::Resume { goal, task } => match task {
            Some(task) => json!({"type":"resume_task", "goal_id":goal, "task_id":task}),
            None => json!({"type":"resume", "goal_id":goal}),
        },
        ManagerCommand::Retry { goal, task } => {
            json!({"type":"retry", "goal_id":goal, "task_id":task})
        }
        ManagerCommand::Answer {
            goal,
            prompt,
            text: answer,
            choice,
            option_text,
        } => {
            let snapshot = client.manager_snapshot(cwd).await?;
            let action = answer_action(
                &snapshot,
                &goal,
                &prompt,
                answer,
                choice.as_deref(),
                option_text,
                tty,
            )?;
            send(&client, cwd, action).await?;
            print_goal_accepted(&client, cwd, &goal).await;
            return Ok(());
        }
        ManagerCommand::Corrective {
            goal,
            task,
            instructions,
        } => {
            let snapshot = client.manager_snapshot(cwd).await?;
            let action = preview_corrective(&snapshot, &goal, &task, instructions, tty)?;
            let preview = send(&client, cwd, action).await?;
            println!(
                "{}\n{}",
                text("corrective_saved"),
                serde_json::to_string_pretty(&preview)?
            );
            println!("{}", text("corrective_not_authorized"));
            print_goal_accepted(&client, cwd, &goal).await;
            return Ok(());
        }
        ManagerCommand::ApproveCorrective { goal, approval } => {
            let snapshot = client.manager_snapshot(cwd).await?;
            approve_corrective(&snapshot, &goal, &approval, tty)?
        }
    };
    let result = send(&client, cwd, action).await?;
    print_accepted(&result);
    Ok(())
}

fn read_input(value: Option<String>, tty: bool, label: &str) -> Result<String> {
    if let Some(value) = value {
        return Ok(value);
    }
    if tty {
        return prompt_multiline(&text(label))?.into_result();
    }
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    Ok(value)
}

async fn send(client: &DaemonClient, cwd: &str, action: Value) -> Result<Value> {
    let request = Value::Object(serde_json::Map::from_iter([
        ("cwd".into(), Value::String(cwd.into())),
        ("action".into(), action),
    ]));
    client.manager_action(&request).await
}

fn print_accepted(result: &Value) {
    match (result["id"].as_str(), result["status"].as_str()) {
        (Some(id), Some(status)) => println!("{}: {id} [{status}]", text("accepted")),
        _ => println!("{}", text("accepted")),
    }
}

async fn print_goal_accepted(client: &DaemonClient, cwd: &str, goal_id: &str) {
    if let Ok(snapshot) = client.manager_snapshot(cwd).await
        && let Ok(goal) = find_goal(&snapshot, goal_id)
        && goal["status"].is_string()
    {
        print_accepted(goal);
    } else {
        println!(
            "{}: {goal_id}. {}",
            text("accepted"),
            text("status_refresh_failed")
        );
    }
}

async fn conversation(
    client: &DaemonClient,
    cwd: &str,
    action: Value,
    no_wait: bool,
    propose: bool,
) -> Result<()> {
    let turn = send(client, cwd, action).await?;
    let id = turn["id"].as_str().ok_or_else(|| error("missing_turn"))?;
    if no_wait {
        println!("{}: {id}. {}", text("accepted"), text("turn_running"));
        return Ok(());
    }
    let snapshot = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal?;
            eprintln!("{}", text("waiting_interrupted"));
            return Ok(());
        }
        snapshot = async {
            print_accepted(&turn);
            wait_for_turn(client, cwd, id).await
        } => snapshot?,
    };
    let (turn, replies) = turn_and_replies(&snapshot, id).ok_or_else(|| error("missing_turn"))?;
    for reply in replies
        .iter()
        .take_while(|reply| reply["role"] != "user")
        .filter(|reply| reply["role"] == "assistant")
    {
        print_turn(reply);
    }
    if turn["status"] != "completed" {
        return Err(error("turn_failed"));
    }
    if propose {
        show_turn_proposal(&snapshot, id)?;
    }
    Ok(())
}

fn turn_and_replies<'a>(snapshot: &'a Value, id: &str) -> Option<(&'a Value, &'a [Value])> {
    let transcript = snapshot["transcript"].as_array()?;
    let index = transcript.iter().position(|turn| turn["id"] == id)?;
    Some((&transcript[index], &transcript[index + 1..]))
}

fn turn_running(turn: &Value) -> bool {
    turn["status"] == "running"
}

async fn wait_for_turn(client: &DaemonClient, cwd: &str, id: &str) -> Result<Value> {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let snapshot = client.manager_snapshot(cwd).await?;
        let (turn, _) = turn_and_replies(&snapshot, id).ok_or_else(|| error("missing_turn"))?;
        if !turn_running(turn) {
            return Ok(snapshot);
        }
    }
}

fn answer_action(
    snapshot: &Value,
    goal: &str,
    prompt: &str,
    answer: Option<String>,
    choice: Option<&str>,
    option_text: Option<String>,
    tty: bool,
) -> Result<Value> {
    let pending = find_prompt(snapshot, goal, prompt).ok_or_else(|| error("stale_prompt"))?;
    match pending.kind {
        PendingPromptKind::Ask => {
            let answer = match (answer, tty, choice) {
                (Some(answer), _, None) => answer,
                (None, true, None) => {
                    prompt_multiline(pending.question.as_deref().unwrap_or(&text("answer_input")))?
                        .into_result()?
                }
                _ => return Err(error("empty_answer")),
            };
            if answer.trim().is_empty() {
                return Err(error("empty_answer"));
            }
            Ok(json!({"type":"answer", "goal_id":goal, "prompt_id":prompt, "answer":answer}))
        }
        PendingPromptKind::Option => {
            let option = match (tty, answer, choice) {
                (true, None, None) => crate::cli_events::prompt_option(
                    &pending.session_id,
                    pending.question.as_deref(),
                    &pending.choices,
                )?,
                (false, None, Some(label)) => select_option(pending, label, option_text)?,
                _ => return Err(error("option_choice")),
            };
            Ok(json!({"type":"answer_option", "goal_id":goal, "prompt_id":prompt, "option":option}))
        }
    }
}

fn select_option(
    pending: PendingPrompt,
    label: &str,
    input: Option<String>,
) -> Result<OptionResult> {
    let mut choices = pending
        .choices
        .into_iter()
        .filter(|choice| choice.label == label);
    let choice = choices.next().ok_or_else(|| error("option_choice"))?;
    if choices.next().is_some() {
        return Err(error("option_choice"));
    }
    match (&choice.kind, &input) {
        (OptionChoiceKind::Selector, None) | (OptionChoiceKind::TextInput, Some(_)) => {
            Ok(OptionResult {
                next_step: choice.next_step,
                text_input: input,
            })
        }
        _ => Err(error("option_choice")),
    }
}

fn find_goal<'a>(snapshot: &'a Value, goal: &str) -> Result<&'a Value> {
    snapshot["goals"]
        .as_array()
        .and_then(|goals| goals.iter().find(|entry| entry["id"] == goal))
        .ok_or_else(|| error("corrective_unknown_goal"))
}

fn preview_corrective(
    snapshot: &Value,
    goal_id: &str,
    task_id: &str,
    instructions: Option<String>,
    tty: bool,
) -> Result<Value> {
    let goal = find_goal(snapshot, goal_id)?;
    let tip = goal["execution"]["integrated_tip"]
        .as_str()
        .ok_or_else(|| error("corrective_missing_tip"))?;
    println!(
        "{}\n{}",
        text("corrective_review"),
        serde_json::to_string_pretty(goal)?
    );
    let instructions = read_input(instructions, tty, "corrective_instructions")?;
    if instructions.trim().is_empty() {
        return Err(error("corrective_empty"));
    }
    Ok(
        json!({"type":"preview_corrective", "goal_id":goal_id, "expected_tip":tip, "task":{"id":task_id, "instructions":instructions, "dependencies":[]}}),
    )
}

fn review_revision<'a>(
    review: &'a Value,
    approval: &ManagerApprovalArgs,
    tty: bool,
) -> Result<(&'a str, u64)> {
    let (Some(id), Some(revision)) = (review["id"].as_str(), review["revision"].as_u64()) else {
        return Err(error("missing_revision"));
    };
    if approval
        .revision
        .is_some_and(|expected| expected != revision)
    {
        return Err(error("stale_revision"));
    }
    if !tty && (!approval.yes || approval.revision.is_none()) {
        return Err(error("approval_flags"));
    }
    if !approval.yes && !confirm_review()? {
        return Err(error("not_approved"));
    }
    Ok((id, revision))
}

fn approve_proposal(snapshot: &Value, approval: &ManagerApprovalArgs, tty: bool) -> Result<Value> {
    let proposal = &snapshot["proposal"];
    if proposal.is_null() {
        return Err(error("no_proposal"));
    }
    println!(
        "{}\n{}",
        text("proposal_to_approve"),
        serde_json::to_string_pretty(proposal)?
    );
    let (id, revision) = review_revision(proposal, approval, tty)?;
    Ok(json!({"type":"approve", "proposal_id":id, "revision":revision}))
}

fn approve_corrective(
    snapshot: &Value,
    goal_id: &str,
    approval: &ManagerApprovalArgs,
    tty: bool,
) -> Result<Value> {
    let goal = find_goal(snapshot, goal_id)?;
    let preview = &goal["execution"]["corrective_preview"];
    if preview.is_null() {
        return Err(error("corrective_no_preview"));
    }
    println!(
        "{}\n{}",
        text("proposal_to_approve"),
        serde_json::to_string_pretty(goal)?
    );
    let (id, revision) = review_revision(preview, approval, tty)?;
    Ok(
        json!({"type":"approve_corrective", "goal_id":goal_id, "preview_id":id, "revision":revision}),
    )
}

fn find_prompt(snapshot: &Value, goal_id: &str, prompt_id: &str) -> Option<PendingPrompt> {
    snapshot["prompts"].as_array()?.iter().find_map(|entry| {
        let prompt = entry.get("prompt")?;
        if entry["goal_id"].as_str()? != goal_id || prompt["requestId"].as_str()? != prompt_id {
            return None;
        }
        serde::Deserialize::deserialize(prompt).ok()
    })
}

fn print_turn(turn: &Value) {
    println!(
        "{} [{}]: {}",
        turn["role"].as_str().unwrap_or(&text("unknown")),
        turn["status"].as_str().unwrap_or(&text("unknown")),
        turn["text"].as_str().unwrap_or("")
    );
}

fn show_turn_proposal(snapshot: &Value, id: &str) -> Result<()> {
    if snapshot["proposal"]["id"] != id {
        return Err(error("proposal_unavailable"));
    }
    show_proposal(snapshot)
}

fn show_proposal(snapshot: &Value) -> Result<()> {
    let proposal = &snapshot["proposal"];
    if !proposal.is_null() {
        println!(
            "{}\n{}",
            text("pending_proposal"),
            serde_json::to_string_pretty(proposal)?
        );
        if let Some(revision) = proposal["revision"].as_u64() {
            println!("cruise manager approve --revision {revision}");
            println!("{}", text("approval_flags"));
        }
    }
    Ok(())
}

fn show_snapshot(snapshot: &Value) -> Result<()> {
    if let Some(transcript) = snapshot["transcript"].as_array() {
        for turn in transcript {
            print_turn(turn);
        }
    }
    show_proposal(snapshot)?;
    if let Some(goals) = snapshot["goals"].as_array() {
        for goal in goals {
            println!("{}\n{}", text("goal"), serde_json::to_string_pretty(goal)?);
            if !goal["execution"]["corrective_preview"].is_null() {
                println!("{}", text("corrective_not_authorized"));
            }
        }
    }
    if !snapshot["prompts"].is_null() {
        println!(
            "{}\n{}",
            text("pending_prompts"),
            serde_json::to_string_pretty(&snapshot["prompts"])?
        );
    }
    Ok(())
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn completed_propose_renders_only_its_own_pending_review() -> Result<()> {
        show_turn_proposal(
            &json!({"proposal":{"id":"accepted", "revision":1}}),
            "accepted",
        )?;
        for proposal in [json!({"id":"later", "revision":2}), Value::Null] {
            assert!(show_turn_proposal(&json!({"proposal":proposal}), "accepted").is_err());
        }
        Ok(())
    }

    #[test]
    fn answer_selects_only_exact_goal_and_request() -> Result<()> {
        let snapshot = json!({"prompts": [{"goal_id": "goal-a", "prompt": {
            "requestId": "request-a", "sessionId": "child", "kind": "option",
            "question": "Choose", "choices": [{"label": "Continue", "kind": "selector", "nextStep": "next"}]
        }}]});
        let prompt = find_prompt(&snapshot, "goal-a", "request-a")
            .ok_or_else(|| CruiseError::Other("expected prompt".into()))?;
        assert!(matches!(
            prompt.kind,
            crate::application::PendingPromptKind::Option
        ));
        assert_eq!(prompt.choices[0].next_step.as_deref(), Some("next"));
        assert!(find_prompt(&snapshot, "goal-b", "request-a").is_none());
        assert!(find_prompt(&snapshot, "goal-a", "request-old").is_none());
        Ok(())
    }

    #[test]
    fn waiting_stops_when_the_accepted_turn_is_not_running() -> Result<()> {
        for status in ["running", "completed", "interrupted", "failed"] {
            let snapshot = json!({"transcript": [
                {"id": "accepted", "status": status},
                {"id": "later", "status": "running"}
            ]});
            let (turn, _) = turn_and_replies(&snapshot, "accepted")
                .ok_or_else(|| CruiseError::Other("expected accepted turn".into()))?;
            assert_eq!(turn_running(turn), status == "running");
        }
        Ok(())
    }
}
