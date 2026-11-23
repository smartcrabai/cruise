//! Cookie-authenticated Manager pages and form actions.

use std::path::{Path, PathBuf};

use axum::extract::{Form, Path as UrlPath, rejection::FormRejection};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

use crate::daemon::api::manager::{
    Action, dispatch_action, enriched_snapshot, startup_action_error,
};
use crate::error::Result;
use crate::manager::{self, GoalStatus, Task};
use crate::step::option::OptionResult;

use super::handlers::{Fields, Form as FormPairs};
use super::pages::{self, render, respond_page};
use super::partials::{by_id, targeted};
use super::templates::{RawHtml, escape_html};
use super::{UiState, WebState};

pub(super) fn routes() -> Router<WebState> {
    let router = Router::new()
        .route("/manager", get(list))
        .route("/manager/{repo}", get(detail))
        .route("/webui/manager/open", post(open))
        .route("/webui/manager/{repo}/sync", get(sync));
    [
        "message",
        "propose",
        "approve",
        "preview-corrective",
        "approve-corrective",
        "pause",
        "resume",
        "resume-task",
        "retry",
        "answer",
        "answer-option",
    ]
    .into_iter()
    .fold(router, |router, name| {
        router.route(
            &format!("/webui/manager/{{repo}}/{name}"),
            post(action).layer(Extension(name)),
        )
    })
}

fn encode(repository: &Path) -> String {
    URL_SAFE_NO_PAD.encode(repository.to_string_lossy().as_bytes())
}

fn url(repository: &Path) -> String {
    format!("/manager/{}", encode(repository))
}

pub(super) fn goal_url(goal_id: &str) -> Option<String> {
    manager::readable_snapshots()
        .map_err(|error| eprintln!("webui: cannot resolve Manager child owner: {error}"))
        .ok()?
        .into_iter()
        .find(|record| record.goals.iter().any(|goal| goal.id == goal_id))
        .map(|record| url(&record.repository))
}

fn repository(key: &str) -> Result<Option<PathBuf>> {
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(key) else {
        return Ok(None);
    };
    Ok(manager::readable_snapshots()?
        .into_iter()
        .map(|record| record.repository)
        .find(|path| path.to_str().is_some_and(|path| path.as_bytes() == decoded)))
}

async fn blocking(build: impl FnOnce() -> Result<Response> + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(move || render(build)).await {
        Ok(response) => response,
        Err(error) => render(|| Err(crate::error::CruiseError::Other(error.to_string()))),
    }
}

async fn list(UiState(state): UiState, headers: HeaderMap) -> Response {
    blocking(move || list_page(&state, &headers, None)).await
}

fn list_page(state: &WebState, headers: &HeaderMap, error: Option<&str>) -> Result<Response> {
    let records: Vec<_> = manager::readable_snapshots()?
            .into_iter()
            .map(|record| {
                let counts: Vec<_> = [GoalStatus::Active, GoalStatus::Paused, GoalStatus::Blocked,
                    GoalStatus::Verifying, GoalStatus::AwaitingMerge, GoalStatus::Completed]
                    .into_iter()
                    .map(|status| json!({"status": status, "count": record.goals.iter().filter(|goal| goal.status == status).count()}))
                    .collect();
                json!({
                    "url": url(&record.repository), "repository": record.repository,
                    "counts": counts, "pending": record.proposal.is_some(),
                    "running": crate::manager_provider::active_turn(&record.repository).is_some(),
                })
            })
            .collect();
    let main = state
        .templates
        .render("manager-list", &json!({"records": records, "error": error}))?;
    respond_page(state, headers, main, None, Some("/manager"), String::new())
}

async fn open(
    UiState(state): UiState,
    headers: HeaderMap,
    form: std::result::Result<FormPairs, FormRejection>,
) -> Response {
    blocking(move || {
        let invalid = |message: &str| {
            if pages::is_fragment_request(&headers) {
                Ok(inline_error(
                    StatusCode::BAD_REQUEST,
                    "manager-open-error",
                    message,
                ))
            } else {
                Ok((
                    StatusCode::BAD_REQUEST,
                    list_page(&state, &headers, Some(message))?,
                )
                    .into_response())
            }
        };
        let fields = match form {
            Ok(Form(pairs)) => Fields::from(pairs),
            Err(error) => return invalid(&error.body_text()),
        };
        let Some(path) = fields.get("path").filter(|path| !path.is_empty()) else {
            return invalid("Missing form field: path");
        };
        match manager::snapshot(Path::new(path)) {
            Ok(snapshot) => {
                if repository(&encode(&snapshot.repository))?.is_none() {
                    return invalid("No Manager record exists for this repository.");
                }
                if let Err(error) = manager::checkout_for(&snapshot.repository) {
                    return invalid(&error.to_string());
                }
                let destination = url(&snapshot.repository);
                if pages::is_fragment_request(&headers) {
                    Ok(([("HX-Redirect", destination)], "").into_response())
                } else {
                    Ok(Redirect::to(&destination).into_response())
                }
            }
            Err(error) => invalid(&error.to_string()),
        }
    })
    .await
}

async fn detail(
    UiState(state): UiState,
    UrlPath(key): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    blocking(move || {
        let Some(repository) = repository(&key)? else {
            let main = pages::error_page_html(&state, "Page not found")?;
            return Ok((
                StatusCode::NOT_FOUND,
                respond_page(&state, &headers, main, None, None, String::new())?,
            )
                .into_response());
        };
        let cwd = manager::checkout_for(&repository)?;
        let region = region_html(&state, &key, &cwd)?;
        let main = state.templates.render(
            "manager-page",
            &json!({
                "key": key, "repository": repository, "region": RawHtml::new(region),
            }),
        )?;
        respond_page(
            &state,
            &headers,
            main,
            None,
            Some(&format!("/manager/{key}")),
            String::new(),
        )
    })
    .await
}

fn region_html(state: &WebState, key: &str, cwd: &Path) -> Result<String> {
    let mut snapshot = enriched_snapshot(state, cwd)?;
    snapshot["running"] = json!(snapshot["repository"].as_str().is_some_and(|repository| {
        crate::manager_provider::active_turn(Path::new(repository)).is_some()
    }));
    // Keep the exact revision when it crosses the TSX number boundary.
    if let Some(revision) = snapshot["proposal"]["revision"].as_u64() {
        snapshot["proposal"]["revision"] = json!(revision.to_string());
    }
    if let Some(goals) = snapshot["goals"].as_array_mut() {
        for goal in goals {
            goal_controls(goal)?;
            let preview = &mut goal["execution"]["corrective_preview"];
            if let Some(revision) = preview["revision"].as_u64() {
                preview["revision"] = json!(revision.to_string());
            }
        }
    }
    let props = Value::Object(
        [
            ("key".into(), Value::String(key.into())),
            ("snapshot".into(), snapshot),
        ]
        .into_iter()
        .collect(),
    );
    state.templates.render("manager-region", &props)
}

fn goal_controls(value: &mut Value) -> Result<()> {
    let goal: manager::Goal = serde::Deserialize::deserialize(&*value)?;
    let execution = &goal.execution;
    let blocked = goal.status == GoalStatus::Blocked;
    let active = goal.status == GoalStatus::Active;
    let can_retry_verification =
        crate::manager_publication::validate_verification_retry(&goal).is_ok();
    value["execution"]["can_retry_verification"] = json!(can_retry_verification);
    value["execution"]["can_preview_corrective"] =
        json!(crate::manager_publication::corrective_tip(&goal).is_ok());
    if let Some(attempts) = value["execution"]["attempts"].as_array_mut() {
        for (attempt, data) in execution.attempts.iter().zip(attempts) {
            let current = !execution.attempts.iter().any(|other| {
                other.task_id == attempt.task_id
                    && (other.attempt > attempt.attempt
                        || other.integrated
                        || other.integration_intent.is_some())
            });
            let phase = data["child_state"]["phase"].as_str();
            let interrupted = matches!(phase, Some("Failed" | "Suspended"));
            let ready = current && !attempt.resume_pending;
            let can_resume = ready && active && interrupted && !attempt.resume_starting;
            let can_retry = ready
                && ((active
                    && !attempt.resume_starting
                    && interrupted
                    && attempt.result_sha.is_none()
                    && attempt.finalization_intent.is_none())
                    || (blocked
                        && phase == Some("Completed")
                        && goal.reason.as_deref().is_some_and(|reason| {
                            reason.starts_with(&format!(
                                "Child {} integration requires attention:",
                                attempt.session_id
                            ))
                        })));
            data["can_resume"] = json!(can_resume);
            data["can_retry"] = json!(can_retry);
        }
    }
    Ok(())
}

fn region_partial(state: &WebState, key: &str, cwd: &Path) -> Result<String> {
    Ok(targeted(
        "#manager-region",
        "innerMorph",
        &region_html(state, key, cwd)?,
    ))
}

async fn sync(UiState(state): UiState, UrlPath(key): UrlPath<String>) -> Response {
    blocking(move || {
        let Some(repository) = repository(&key)? else {
            return Ok(StatusCode::NOT_FOUND.into_response());
        };
        let cwd = manager::checkout_for(&repository)?;
        Ok(Html(region_partial(&state, &key, &cwd)?).into_response())
    })
    .await
}

fn inline_error(status: StatusCode, id: &str, message: &str) -> Response {
    (
        status,
        Html(by_id(
            id,
            &format!("<p role=\"alert\">{}</p>", escape_html(message)),
        )),
    )
        .into_response()
}

fn action_error(status: StatusCode, message: &str) -> Response {
    inline_error(status, "manager-action-error", message)
}

async fn action(
    UiState(state): UiState,
    UrlPath(key): UrlPath<String>,
    Extension(name): Extension<&'static str>,
    form: std::result::Result<FormPairs, FormRejection>,
) -> Response {
    blocking(move || {
        if let Some((status, message)) = startup_action_error(&state) {
            return Ok(action_error(status, &message));
        }
        let Some(repository) = repository(&key)? else {
            return Ok(action_error(
                StatusCode::NOT_FOUND,
                "Unknown Manager repository",
            ));
        };
        let fields = match form {
            Ok(Form(pairs)) => Fields::from(pairs),
            Err(error) => return Ok(action_error(StatusCode::BAD_REQUEST, &error.body_text())),
        };
        let action = match parse_action(name, &fields) {
            Ok(action) => action,
            Err(message) => return Ok(action_error(StatusCode::BAD_REQUEST, &message)),
        };
        let cwd = match manager::checkout_for(&repository) {
            Ok(cwd) => cwd,
            Err(error) => return Ok(action_error(StatusCode::NOT_FOUND, &error.to_string())),
        };
        match dispatch_action(&state, &cwd, action) {
            Ok((status, _)) if status.is_success() => {
                let mut html = by_id("manager-action-error", "");
                html.push_str(&region_partial(&state, &key, &cwd)?);
                Ok((status, Html(html)).into_response())
            }
            Ok((status, value)) => Ok(action_error(
                status,
                value["error"].as_str().unwrap_or_default(),
            )),
            Err(error) => {
                let (mut status, message) = crate::daemon::api::error_details(&error);
                if let crate::error::CruiseError::Other(reason) = error {
                    status = if reason.starts_with("unknown Manager goal")
                        || reason.starts_with("unknown Manager task")
                    {
                        StatusCode::NOT_FOUND
                    } else if matches!(
                        reason.as_str(),
                        "stale or unknown Manager ask prompt"
                            | "stale or unknown Manager option prompt"
                    ) {
                        StatusCode::CONFLICT
                    } else if reason == "invalid corrective task or dependencies" {
                        StatusCode::BAD_REQUEST
                    } else {
                        status
                    };
                }
                Ok(action_error(status, &message))
            }
        }
    })
    .await
}

fn parse_action(name: &str, fields: &Fields) -> std::result::Result<Action, String> {
    let required = |name: &str| {
        fields
            .get(name)
            .ok_or_else(|| format!("Missing form field: {name}"))
    };
    let field = |name: &str| required(name).map(str::to_owned);
    let goal_id = || field("goal_id");
    let task_id = || field("task_id");
    let prompt_id = || field("prompt_id");
    let revision = || {
        required("revision")?
            .parse::<u64>()
            .map_err(|_| "Invalid revision".to_string())
    };
    let optional = |name: &str| {
        fields
            .get(name)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Ok(match name {
        "message" => Action::Message {
            text: field("text")?,
        },
        "propose" => Action::Propose,
        "approve" => Action::Approve {
            proposal_id: field("proposal_id")?,
            revision: revision()?,
        },
        "preview-corrective" => Action::PreviewCorrective {
            goal_id: goal_id()?,
            expected_tip: field("expected_tip")?,
            task: Task {
                id: task_id()?,
                instructions: field("instructions")?,
                dependencies: Vec::new(),
            },
        },
        "approve-corrective" => Action::ApproveCorrective {
            goal_id: goal_id()?,
            preview_id: field("preview_id")?,
            revision: revision()?,
        },
        "pause" => Action::Pause {
            goal_id: goal_id()?,
        },
        "resume" => Action::Resume {
            goal_id: goal_id()?,
        },
        "resume-task" => Action::ResumeTask {
            goal_id: goal_id()?,
            task_id: task_id()?,
        },
        "retry" => Action::Retry {
            goal_id: goal_id()?,
            task_id: optional("task_id"),
        },
        "answer" => Action::Answer {
            goal_id: goal_id()?,
            prompt_id: prompt_id()?,
            answer: field("answer")?,
        },
        "answer-option" => Action::AnswerOption {
            goal_id: goal_id()?,
            prompt_id: prompt_id()?,
            option: OptionResult {
                next_step: optional("nextStep"),
                text_input: fields.get("textInput").map(str::to_owned),
            },
        },
        _ => return Err("Unknown Manager action".into()),
    })
}
