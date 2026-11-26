use crate::daemon::server::WebState;
use crate::error::Result;
use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/snapshot", get(snapshot))
        .route("/action", post(action))
}
#[derive(Deserialize)]
struct SnapshotRequest {
    cwd: PathBuf,
}
async fn snapshot(
    State(state): State<WebState>,
    Query(request): Query<SnapshotRequest>,
) -> Response {
    match tokio::task::spawn_blocking(move || enriched_snapshot(&state, &request.cwd)).await {
        Ok(Ok(snapshot)) => axum::Json(snapshot).into_response(),
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

pub(crate) fn enriched_snapshot(state: &WebState, cwd: &Path) -> Result<Value> {
    let (mut snapshot, active) = crate::manager_provider::snapshot_with_active_turn(cwd)?;
    if let Some(id) = active
        && let Some(turn) = snapshot
            .transcript
            .iter_mut()
            .find(|turn| turn.id == id && turn.status == "interrupted")
    {
        turn.status = "running".into();
    }
    let mut prompts = Vec::new();
    for goal in &snapshot.goals {
        for prompt in crate::manager_coordinator::prompts(&state.application, cwd, &goal.id)? {
            prompts.push(serde_json::json!({"goal_id": goal.id, "prompt": prompt}));
        }
    }
    let mut value = serde_json::to_value(&snapshot)?;
    if let Some(goals) = value["goals"].as_array_mut() {
        for (goal_value, goal) in goals.iter_mut().zip(&snapshot.goals) {
            if let Some(attempts) = goal_value["execution"]["attempts"].as_array_mut() {
                for (attempt_value, attempt) in attempts.iter_mut().zip(&goal.execution.attempts) {
                    attempt_value["child_state"] =
                        match state.application.read_session(&attempt.session_id) {
                            Ok(child) => {
                                let failure = match &child.phase {
                                    crate::session::SessionPhase::Failed(message)
                                    | crate::session::SessionPhase::PrFailed(message) => {
                                        Some(message.clone())
                                    }
                                    _ => None,
                                };
                                serde_json::json!({
                                    "phase": child.phase.label(),
                                    "failure": failure,
                                })
                            }
                            Err(error) => serde_json::json!({
                                "phase": "Unavailable",
                                "failure": format!(
                                    "Could not inspect child {}: {error}",
                                    attempt.session_id
                                ),
                            }),
                        };
                }
            }
        }
    }
    value["prompts"] = serde_json::Value::Array(prompts);
    Ok(value)
}

#[derive(Deserialize)]
struct ActionRequest {
    cwd: PathBuf,
    action: Action,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Action {
    Message {
        text: String,
    },
    Propose,
    PreviewCorrective {
        goal_id: String,
        expected_tip: String,
        #[serde(flatten)]
        request: crate::manager_coordinator::corrections::CorrectionRequest,
    },
    Approve {
        proposal_id: String,
        revision: u64,
    },
    ApproveCorrective {
        goal_id: String,
        preview_id: String,
        revision: u64,
    },
    Pause {
        goal_id: String,
    },
    Resume {
        goal_id: String,
    },
    ResumeTask {
        goal_id: String,
        task_id: String,
    },
    Retry {
        goal_id: String,
        task_id: Option<String>,
    },
    Answer {
        goal_id: String,
        prompt_id: String,
        answer: String,
    },
    AnswerOption {
        goal_id: String,
        prompt_id: String,
        option: crate::step::option::OptionResult,
    },
}

async fn action(
    State(state): State<WebState>,
    axum::Json(request): axum::Json<ActionRequest>,
) -> Response {
    match tokio::task::spawn_blocking(move || dispatch_action(&state, &request.cwd, request.action))
        .await
    {
        Ok(Ok((status, value))) => (status, axum::Json(value)).into_response(),
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

pub(crate) fn startup_action_error(state: &WebState) -> Option<(StatusCode, String)> {
    state.manager_startup_error.as_ref().map(|error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("Manager disabled after startup recovery failure: {error}"),
        )
    })
}

pub(crate) fn dispatch_action(
    state: &WebState,
    cwd: &Path,
    action: Action,
) -> Result<(StatusCode, Value)> {
    use crate::{manager, manager_coordinator as coordinator, manager_provider as provider};

    if let Some((status, message)) = startup_action_error(state) {
        return Ok((status, json!({"error": message})));
    }
    let value =
        match action {
            Action::Message { text } => serde_json::to_value(provider::message(cwd, text)?)?,
            Action::Propose => serde_json::to_value(provider::propose(cwd)?)?,
            Action::PreviewCorrective {
                goal_id,
                expected_tip,
                request,
            } => serde_json::to_value(coordinator::corrections::preview(
                &state.application,
                cwd,
                &goal_id,
                &expected_tip,
                request,
            )?)?,
            Action::Approve {
                proposal_id,
                revision,
            } => {
                let goal = manager::approve(cwd, &proposal_id, revision)?;
                coordinator::wake();
                serde_json::to_value(goal)?
            }
            Action::ApproveCorrective {
                goal_id,
                preview_id,
                revision,
            } => {
                let goal = coordinator::corrections::approve(
                    &state.application,
                    cwd,
                    &goal_id,
                    &preview_id,
                    revision,
                )?;
                coordinator::wake();
                serde_json::to_value(goal)?
            }
            Action::Pause { goal_id } => {
                serde_json::to_value(coordinator::pause(&state.application, cwd, &goal_id)?)?
            }
            Action::Resume { goal_id } => {
                let goal = coordinator::resume(cwd, &goal_id)?;
                coordinator::wake();
                serde_json::to_value(goal)?
            }
            Action::ResumeTask { goal_id, task_id } => serde_json::to_value(
                coordinator::resume_task(&state.application, cwd, &goal_id, &task_id)?,
            )?,
            Action::Retry { goal_id, task_id } => {
                let goal = if let Some(task_id) = task_id {
                    coordinator::retry_task(&state.application, cwd, &goal_id, &task_id)?
                } else {
                    coordinator::retry_goal(cwd, &goal_id, &state.manager_stop)?
                };
                coordinator::wake();
                serde_json::to_value(goal)?
            }
            Action::Answer {
                goal_id,
                prompt_id,
                answer,
            } => {
                coordinator::answer(&state.application, cwd, &goal_id, &prompt_id, answer)?;
                json!({"accepted": true})
            }
            Action::AnswerOption {
                goal_id,
                prompt_id,
                option,
            } => {
                coordinator::answer_option(&state.application, cwd, &goal_id, &prompt_id, option)?;
                json!({"accepted": true})
            }
        };
    Ok((StatusCode::ACCEPTED, value))
}

#[cfg(test)]
mod prompt_action_tests {
    use super::Action;

    #[test]
    fn manager_prompt_actions_accept_ask_and_typed_option() -> Result<(), serde_json::Error> {
        let ask: Action = serde_json::from_value(serde_json::json!({
            "type": "answer", "goal_id": "g", "prompt_id": "p", "answer": "yes"
        }))?;
        assert!(matches!(ask, Action::Answer { answer, .. } if answer == "yes"));
        let option: Action = serde_json::from_value(serde_json::json!({
            "type": "answer_option", "goal_id": "g", "prompt_id": "p",
            "option": {"nextStep": "next", "textInput": "details"}
        }))?;
        assert!(matches!(option, Action::AnswerOption { option, .. }
            if option.next_step.as_deref() == Some("next") && option.text_input.as_deref() == Some("details")));
        Ok(())
    }
}
