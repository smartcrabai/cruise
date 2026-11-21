use crate::daemon::server::WebState;
use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::path::PathBuf;

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
    match tokio::task::spawn_blocking(move || -> crate::error::Result<serde_json::Value> {
        let mut snapshot = crate::manager::snapshot(&request.cwd)?;
        if let Some(id) = crate::manager_provider::active_turn(&snapshot.repository)
            && let Some(turn) = snapshot
                .transcript
                .iter_mut()
                .find(|turn| turn.id == id && turn.status == "interrupted")
        {
            turn.status = "running".into();
        }
        let mut prompts = Vec::new();
        for goal in &snapshot.goals {
            for prompt in
                crate::manager_coordinator::prompts(&state.application, &request.cwd, &goal.id)?
            {
                prompts.push(serde_json::json!({"goal_id": goal.id, "prompt": prompt}));
            }
        }
        let mut value = serde_json::to_value(snapshot)?;
        value["prompts"] = serde_json::Value::Array(prompts);
        Ok(value)
    })
    .await
    {
        Ok(Ok(snapshot)) => axum::Json(snapshot).into_response(),
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

#[derive(Deserialize)]
struct ActionRequest {
    cwd: PathBuf,
    action: Action,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    Message {
        text: String,
    },
    Propose,
    PreviewCorrective {
        goal_id: String,
        expected_tip: String,
        task: crate::manager::Task,
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
    if let Some(error) = &state.manager_startup_error {
        return super::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("Manager disabled after startup recovery failure: {error}"),
        );
    }
    match request.action {
        Action::Message { text } => match crate::manager_provider::message(&request.cwd, text) {
            Ok(turn) => (StatusCode::ACCEPTED, axum::Json(turn)).into_response(),
            Err(error) => super::map_error(&error),
        },
        Action::Propose => match crate::manager_provider::propose(&request.cwd) {
            Ok(turn) => (StatusCode::ACCEPTED, axum::Json(turn)).into_response(),
            Err(error) => super::map_error(&error),
        },
        Action::PreviewCorrective {
            goal_id,
            expected_tip,
            task,
        } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager::preview_corrective(&request.cwd, &goal_id, &expected_tip, task)
            })
            .await
            {
                Ok(Ok(preview)) => (StatusCode::ACCEPTED, axum::Json(preview)).into_response(),
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        Action::Approve {
            proposal_id,
            revision,
        } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager::approve(&request.cwd, &proposal_id, revision)
            })
            .await
            {
                Ok(Ok(goal)) => {
                    crate::manager_coordinator::wake();
                    (StatusCode::ACCEPTED, axum::Json(goal)).into_response()
                }
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        Action::ApproveCorrective {
            goal_id,
            preview_id,
            revision,
        } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager::approve_corrective(&request.cwd, &goal_id, &preview_id, revision)
            })
            .await
            {
                Ok(Ok(goal)) => {
                    crate::manager_coordinator::wake();
                    (StatusCode::ACCEPTED, axum::Json(goal)).into_response()
                }
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        other => {
            action_control(
                state,
                ActionRequest {
                    cwd: request.cwd,
                    action: other,
                },
            )
            .await
        }
    }
}

async fn action_control(state: WebState, request: ActionRequest) -> Response {
    match request.action {
        Action::Pause { goal_id } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager_coordinator::pause(&state.application, &request.cwd, &goal_id)
            })
            .await
            {
                Ok(Ok(goal)) => (StatusCode::ACCEPTED, axum::Json(goal)).into_response(),
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        Action::Resume { goal_id } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager_coordinator::resume(&request.cwd, &goal_id)
            })
            .await
            {
                Ok(Ok(goal)) => {
                    crate::manager_coordinator::wake();
                    (StatusCode::ACCEPTED, axum::Json(goal)).into_response()
                }
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        Action::ResumeTask { goal_id, task_id } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager_coordinator::resume_task(
                    &state.application,
                    &request.cwd,
                    &goal_id,
                    &task_id,
                )
            })
            .await
            {
                Ok(Ok(goal)) => (StatusCode::ACCEPTED, axum::Json(goal)).into_response(),
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        Action::Retry { goal_id, task_id } => {
            match tokio::task::spawn_blocking(move || {
                if let Some(task_id) = task_id {
                    crate::manager_coordinator::retry_task(
                        &state.application,
                        &request.cwd,
                        &goal_id,
                        &task_id,
                    )
                } else {
                    crate::manager_coordinator::retry_verification(&request.cwd, &goal_id)
                }
            })
            .await
            {
                Ok(Ok(goal)) => {
                    crate::manager_coordinator::wake();
                    (StatusCode::ACCEPTED, axum::Json(goal)).into_response()
                }
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        other => {
            action_answer(
                state,
                ActionRequest {
                    cwd: request.cwd,
                    action: other,
                },
            )
            .await
        }
    }
}

async fn action_answer(state: WebState, request: ActionRequest) -> Response {
    match request.action {
        Action::AnswerOption {
            goal_id,
            prompt_id,
            option,
        } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager_coordinator::answer_option(
                    &state.application,
                    &request.cwd,
                    &goal_id,
                    &prompt_id,
                    option,
                )
            })
            .await
            {
                Ok(Ok(())) => (
                    StatusCode::ACCEPTED,
                    axum::Json(serde_json::json!({"accepted": true})),
                )
                    .into_response(),
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        Action::Answer {
            goal_id,
            prompt_id,
            answer,
        } => {
            match tokio::task::spawn_blocking(move || {
                crate::manager_coordinator::answer(
                    &state.application,
                    &request.cwd,
                    &goal_id,
                    &prompt_id,
                    answer,
                )
            })
            .await
            {
                Ok(Ok(())) => (
                    StatusCode::ACCEPTED,
                    axum::Json(serde_json::json!({"accepted": true})),
                )
                    .into_response(),
                Ok(Err(error)) => super::map_error(&error),
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        _ => super::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected manager action",
        ),
    }
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
