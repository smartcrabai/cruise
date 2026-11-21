use std::io::IsTerminal;

use serde_json::{Value, json};

use crate::daemon::client::DaemonClient;
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};

fn text(key: &str) -> String {
    crate::i18n::t(crate::i18n::process_locale(), &format!("cli.manager.{key}"))
}

fn confirm_review() -> Result<bool> {
    let InputResult::Submitted(confirm) = prompt_multiline(&text("confirm"))? else {
        return Ok(false);
    };
    if confirm.trim() == "approve" {
        Ok(true)
    } else {
        println!("{}", text("not_approved"));
        Ok(false)
    }
}

/// A terminal is only a client: closing it never cancels daemon work.
pub(crate) async fn run() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(CruiseError::Other(text("interactive_required")));
    }
    let cwd = std::env::current_dir()?;
    let cwd = cwd
        .to_str()
        .ok_or_else(|| CruiseError::Other(text("utf8_required")))?;
    let client = DaemonClient::connect().await?;
    println!("{}", text("welcome"));
    let mut snapshot = client.manager_snapshot(cwd).await?;
    show_snapshot(&snapshot);
    loop {
        let InputResult::Submitted(input) = prompt_multiline(&text("prompt"))? else {
            return Ok(());
        };
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if matches!(input, "/exit" | "/quit") {
            return Ok(());
        }
        if input == "/help" {
            println!("{}", text("help"));
            println!("{}", text("corrective_usage"));
            println!("{}", text("corrective_approve_usage"));
            continue;
        }
        if input == "/goals" {
            match client.manager_snapshot(cwd).await {
                Ok(value) => {
                    snapshot = value;
                    show_snapshot(&snapshot);
                }
                Err(error) => eprintln!("{}: {}", text("prompt"), error.detailed_message()),
            }
            continue;
        }
        let Some(action) = build_action(&client, cwd, input, &mut snapshot).await? else {
            continue;
        };
        match client
            .manager_action(&json!({"cwd":cwd, "action":action}))
            .await
        {
            Ok(value) => {
                println!(
                    "{}: {}",
                    text("accepted"),
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
                if input.starts_with("/corrective ") {
                    println!("{}", text("corrective_not_authorized"));
                }
                if input == "/make-goal" || !input.starts_with('/') {
                    println!("{}", text("turn_running"));
                }
            }
            Err(error) => eprintln!("{}: {}", text("prompt"), error.detailed_message()),
        }
        match client.manager_snapshot(cwd).await {
            Ok(value) => {
                snapshot = value;
                show_snapshot(&snapshot);
            }
            Err(error) => eprintln!("{}: {}", text("refresh_failed"), error.detailed_message()),
        }
    }
}

async fn build_action(
    client: &DaemonClient,
    cwd: &str,
    input: &str,
    snapshot: &mut Value,
) -> Result<Option<Value>> {
    let action = if input == "/make-goal" {
        json!({"type":"propose"})
    } else if input.starts_with("/corrective ") {
        return preview_corrective(client, cwd, input).await;
    } else if input.starts_with("/approve-corrective ") {
        return approve_corrective(client, cwd, input).await;
    } else if input == "/approve" {
        return approve_proposal(client, cwd, snapshot).await;
    } else if input.starts_with('/') {
        let (command, rest) = take_word(input);
        let (goal, rest) = take_word(rest);
        let (item, rest) = take_word(rest);
        match (command, goal, item) {
            (Some("/pause"), Some(goal), None) => json!({"type":"pause", "goal_id":goal}),
            (Some("/resume"), Some(goal), None) => {
                json!({"type":"resume", "goal_id":goal})
            }
            (Some("/resume"), Some(goal), Some(task)) if rest.is_empty() => {
                json!({"type":"resume_task", "goal_id":goal, "task_id":task})
            }
            (Some("/retry"), Some(goal), None) => {
                json!({"type":"retry", "goal_id":goal})
            }
            (Some("/retry"), Some(goal), Some(task)) if rest.is_empty() => {
                json!({"type":"retry", "goal_id":goal, "task_id":task})
            }
            (Some("/answer"), Some(goal), Some(prompt)) => {
                let current = match client.manager_snapshot(cwd).await {
                    Ok(value) => value,
                    Err(error) => {
                        eprintln!("{}: {}", text("refresh_failed"), error.detailed_message());
                        return Ok(None);
                    }
                };
                let Some(pending) = find_prompt(&current, goal, prompt) else {
                    eprintln!("{}", text("stale_prompt"));
                    return Ok(None);
                };
                match pending.kind {
                    crate::application::PendingPromptKind::Ask => {
                        if rest.is_empty() {
                            println!("{}", text("empty_answer"));
                            return Ok(None);
                        }
                        json!({"type":"answer", "goal_id":goal, "prompt_id":prompt, "answer":rest})
                    }
                    crate::application::PendingPromptKind::Option => {
                        if !rest.is_empty() {
                            eprintln!("{}", text("option_answer_usage"));
                            return Ok(None);
                        }
                        let option = match crate::cli_events::prompt_option(
                            &pending.session_id,
                            pending.question.as_deref(),
                            &pending.choices,
                        ) {
                            Ok(option) => option,
                            Err(CruiseError::StepPaused) => return Ok(None),
                            Err(error) => return Err(error),
                        };
                        json!({"type":"answer_option", "goal_id":goal, "prompt_id":prompt, "option":option})
                    }
                }
            }
            _ => {
                println!("{}", text("unknown_command"));
                return Ok(None);
            }
        }
    } else {
        json!({"type":"message", "text":input})
    };
    Ok(Some(action))
}

async fn preview_corrective(
    client: &DaemonClient,
    cwd: &str,
    input: &str,
) -> Result<Option<Value>> {
    let (_, rest) = take_word(input);
    let (goal_id, rest) = take_word(rest);
    let (task_id, rest) = take_word(rest);
    let (Some(goal_id), Some(task_id)) = (goal_id, task_id) else {
        println!("{}", text("corrective_usage"));
        return Ok(None);
    };
    if !rest.is_empty() {
        println!("{}", text("corrective_usage"));
        return Ok(None);
    }
    let current = match client.manager_snapshot(cwd).await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{}: {}", text("refresh_failed"), error.detailed_message());
            return Ok(None);
        }
    };
    let Some(goal) = current
        .get("goals")
        .and_then(Value::as_array)
        .and_then(|goals| {
            goals
                .iter()
                .find(|goal| goal.get("id").and_then(Value::as_str) == Some(goal_id))
        })
    else {
        println!("{}", text("corrective_unknown_goal"));
        return Ok(None);
    };
    let Some(tip) = goal
        .get("execution")
        .and_then(|execution| execution.get("integrated_tip"))
        .and_then(Value::as_str)
    else {
        println!("{}", text("corrective_missing_tip"));
        return Ok(None);
    };
    println!(
        "{}\n{}",
        text("corrective_review"),
        serde_json::to_string_pretty(goal).unwrap_or_default()
    );
    let InputResult::Submitted(instructions) = prompt_multiline(&text("corrective_instructions"))?
    else {
        return Ok(None);
    };
    if instructions.trim().is_empty() {
        println!("{}", text("corrective_empty"));
        return Ok(None);
    }
    Ok(Some(
        json!({"type":"preview_corrective", "goal_id":goal_id, "expected_tip":tip, "task":{"id":task_id, "instructions":instructions, "dependencies":[]}}),
    ))
}

async fn approve_corrective(
    client: &DaemonClient,
    cwd: &str,
    input: &str,
) -> Result<Option<Value>> {
    let (_, rest) = take_word(input);
    let (goal_id, rest) = take_word(rest);
    let Some(goal_id) = goal_id.filter(|_| rest.is_empty()) else {
        println!("{}", text("corrective_approve_usage"));
        return Ok(None);
    };
    let current = match client.manager_snapshot(cwd).await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{}: {}", text("refresh_failed"), error.detailed_message());
            return Ok(None);
        }
    };
    let Some(goal) = current
        .get("goals")
        .and_then(Value::as_array)
        .and_then(|goals| {
            goals
                .iter()
                .find(|goal| goal.get("id").and_then(Value::as_str) == Some(goal_id))
        })
    else {
        println!("{}", text("corrective_unknown_goal"));
        return Ok(None);
    };
    let Some(preview) = goal
        .get("execution")
        .and_then(|execution| execution.get("corrective_preview"))
        .filter(|preview| !preview.is_null())
    else {
        println!("{}", text("corrective_no_preview"));
        return Ok(None);
    };
    println!(
        "{}\n{}",
        text("proposal_to_approve"),
        serde_json::to_string_pretty(goal).unwrap_or_default()
    );
    let (Some(id), Some(revision)) = (
        preview.get("id").and_then(Value::as_str),
        preview.get("revision").and_then(Value::as_u64),
    ) else {
        eprintln!("{}", text("missing_revision"));
        return Ok(None);
    };
    if !confirm_review()? {
        return Ok(None);
    }
    Ok(Some(
        json!({"type":"approve_corrective", "goal_id":goal_id, "preview_id":id, "revision":revision}),
    ))
}

async fn approve_proposal(
    client: &DaemonClient,
    cwd: &str,
    snapshot: &mut Value,
) -> Result<Option<Value>> {
    match client.manager_snapshot(cwd).await {
        Ok(value) => *snapshot = value,
        Err(error) => {
            eprintln!("{}: {}", text("refresh_failed"), error.detailed_message());
            return Ok(None);
        }
    }
    let Some(proposal) = snapshot.get("proposal").filter(|value| !value.is_null()) else {
        println!("{}", text("no_proposal"));
        return Ok(None);
    };
    // Re-display the full executable contract before asking for explicit approval.
    println!(
        "{}\n{}",
        text("proposal_to_approve"),
        serde_json::to_string_pretty(proposal).unwrap_or_default()
    );
    let (Some(id), Some(revision)) = (
        proposal.get("id").and_then(Value::as_str),
        proposal.get("revision").and_then(Value::as_u64),
    ) else {
        eprintln!("{}", text("missing_revision"));
        return Ok(None);
    };
    if !confirm_review()? {
        return Ok(None);
    }
    Ok(Some(
        json!({"type":"approve", "proposal_id":id, "revision":revision}),
    ))
}

fn find_prompt(
    snapshot: &Value,
    goal_id: &str,
    prompt_id: &str,
) -> Option<crate::application::PendingPrompt> {
    snapshot
        .get("prompts")?
        .as_array()?
        .iter()
        .find_map(|entry| {
            if entry.get("goal_id")?.as_str()? != goal_id {
                return None;
            }
            let prompt: crate::application::PendingPrompt =
                serde::Deserialize::deserialize(entry.get("prompt")?).ok()?;
            (prompt.request_id == prompt_id).then_some(prompt)
        })
}

fn take_word(input: &str) -> (Option<&str>, &str) {
    let input = input.trim_start();
    if input.is_empty() {
        return (None, "");
    }
    match input.find(char::is_whitespace) {
        Some(index) => (Some(&input[..index]), input[index..].trim_start()),
        None => (Some(input), ""),
    }
}

fn show_snapshot(snapshot: &Value) {
    if let Some(transcript) = snapshot.get("transcript").and_then(Value::as_array) {
        for turn in transcript {
            println!(
                "{} [{}]: {}",
                turn["role"].as_str().unwrap_or(&text("unknown")),
                turn["status"].as_str().unwrap_or(&text("unknown")),
                turn["text"].as_str().unwrap_or("")
            );
        }
    }
    if let Some(proposal) = snapshot.get("proposal").filter(|v| !v.is_null()) {
        println!(
            "{}\n{}",
            text("pending_proposal"),
            serde_json::to_string_pretty(proposal).unwrap_or_default()
        );
        println!("{}", text("approve_hint"));
    }
    if let Some(goals) = snapshot.get("goals").and_then(Value::as_array) {
        for goal in goals {
            println!(
                "{}\n{}",
                text("goal"),
                serde_json::to_string_pretty(goal).unwrap_or_default()
            );
            if goal
                .get("execution")
                .and_then(|execution| execution.get("corrective_preview"))
                .is_some_and(|preview| !preview.is_null())
            {
                println!("{}", text("corrective_not_authorized"));
            }
        }
    }
    if let Some(prompts) = snapshot.get("prompts").filter(|v| !v.is_null()) {
        println!(
            "{}\n{}",
            text("pending_prompts"),
            serde_json::to_string_pretty(prompts).unwrap_or_default()
        );
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

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
}
