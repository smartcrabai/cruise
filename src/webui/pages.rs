//! Page routes: full documents for normal requests, `#main` fragments for
//! htmx navigations.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::application::PendingPromptKind;
use crate::error::Result;

use super::WebState;
use super::partials;
use super::templates::RawHtml;
use super::view::{
    EmptyStateVm, ErrorPageVm, NewSessionVm, RunAllResultVm, RunAllRunningVm, RunAllVm,
    SessionDetailVm, ShellVm, StepRowVm, StepTreeVm, TabHrefs, TabInfoVm, format_local_time,
    runnable_count, truncate,
};

const HX_PUSH_URL: HeaderName = HeaderName::from_static("hx-push-url");

#[derive(Debug, Deserialize)]
pub(crate) struct TabQuery {
    #[serde(default)]
    pub(crate) tab: Option<String>,
}

/// True for an htmx navigation that wants only the `#main` fragment. History
/// restores replay a cached full document instead.
pub(crate) fn is_fragment_request(headers: &HeaderMap) -> bool {
    headers.get("hx-request").is_some() && headers.get("hx-history-restore-request").is_none()
}

/// Session id from the client's current URL, used by the sidebar poll.
pub(crate) fn current_session_id(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("hx-current-url")?.to_str().ok()?;
    let path = raw
        .split_once("://")
        .map_or(raw, |(_, rest)| rest.split_once('/').map_or("", |(_, p)| p));
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let path = path.split(['?', '#']).next().unwrap_or(&path);
    let id = path.strip_prefix("/sessions/")?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Compose a page response: fragment plus sidebar refresh, or a full document.
pub(crate) fn respond_page(
    state: &WebState,
    headers: &HeaderMap,
    main_html: String,
    selected_id: Option<&str>,
    push_url: Option<&str>,
    dialogs_html: String,
) -> Result<Response> {
    if is_fragment_request(headers) {
        let mut body = main_html;
        body.push_str(&partials::sidebar_list_partial(state, selected_id)?);
        if !dialogs_html.is_empty() {
            body.push_str(&partials::targeted("#dialogs", "beforeend", &dialogs_html));
        }
        let mut response = Html(body).into_response();
        if let Some(url) = push_url
            && let Ok(value) = HeaderValue::from_str(url)
        {
            response.headers_mut().insert(HX_PUSH_URL, value);
        }
        return Ok(response);
    }
    let sidebar_vm = partials::sidebar_vm(state, selected_id)?;
    let rows = state.templates.render("sidebar-rows", &sidebar_vm)?;
    let sidebar_vm = super::view::SidebarVm {
        rows: RawHtml::new(rows),
        ..sidebar_vm
    };
    let shell = ShellVm {
        title: "Cruise".to_string(),
        version: env!("CARGO_PKG_VERSION"),
        main: RawHtml::new(main_html),
        sidebar: RawHtml::new(state.templates.render("sidebar", &sidebar_vm)?),
        dialogs: RawHtml::new(dialogs_html),
    };
    Ok(Html(state.templates.render_document(&shell)?).into_response())
}

pub(crate) fn empty_state_html(state: &WebState) -> Result<String> {
    state.templates.render(
        "empty-state",
        &EmptyStateVm {
            message: "Select a session or create a new one.".to_string(),
        },
    )
}

fn error_page_html(state: &WebState, message: &str) -> Result<String> {
    state.templates.render(
        "error-page",
        &ErrorPageVm {
            message: message.to_string(),
        },
    )
}

pub(crate) async fn root(State(state): State<WebState>, headers: HeaderMap) -> Response {
    render(|| {
        let main = empty_state_html(&state)?;
        respond_page(&state, &headers, main, None, None, String::new())
    })
}

pub(crate) async fn not_found(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let body = render(|| {
        let main = error_page_html(&state, "Page not found")?;
        respond_page(&state, &headers, main, None, None, String::new())
    });
    (StatusCode::NOT_FOUND, body).into_response()
}

pub(crate) async fn session(
    State(state): State<WebState>,
    Path(id): Path<String>,
    Query(query): Query<TabQuery>,
    headers: HeaderMap,
) -> Response {
    if state.application.read_session(&id).is_err() {
        let body = render(|| {
            let main = error_page_html(&state, "Session not found")?;
            respond_page(&state, &headers, main, None, None, String::new())
        });
        return (StatusCode::NOT_FOUND, body).into_response();
    }
    render(|| {
        let tab = normalize_tab(query.tab.as_deref());
        let main = session_detail_html(&state, &id, tab)?;
        let dialogs = pending_option_dialog_html(&state, &id)?;
        respond_page(&state, &headers, main, Some(&id), None, dialogs)
    })
}

pub(crate) async fn new_session(State(state): State<WebState>, headers: HeaderMap) -> Response {
    render(|| {
        let main = new_session_html(&state)?;
        respond_page(&state, &headers, main, None, None, String::new())
    })
}

pub(crate) async fn run_all(State(state): State<WebState>, headers: HeaderMap) -> Response {
    render(|| {
        let vm = run_all_vm(&state);
        let main = state.templates.render("run-all", &vm)?;
        respond_page(&state, &headers, main, None, None, String::new())
    })
}

fn render<F>(build: F) -> Response
where
    F: FnOnce() -> Result<Response>,
{
    match build() {
        Ok(response) => response,
        Err(error) => {
            eprintln!("webui: page render failed: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(format!(
                    "<p class=\"p-6 text-sm text-red-600 dark:text-red-400\">{}</p>",
                    super::templates::escape_html(&error.to_string())
                )),
            )
                .into_response()
        }
    }
}

pub(crate) fn normalize_tab(tab: Option<&str>) -> &'static str {
    match tab {
        Some("dag") => "dag",
        Some("plan") => "plan",
        Some("log") => "log",
        _ => "info",
    }
}

pub(crate) fn tab_panel_html(state: &WebState, id: &str, tab: &str) -> Result<String> {
    match tab {
        "dag" => {
            let dag = super::dto::dag_dto_for_session(&state.application, id).unwrap_or(None);
            let vm = super::view::TabDagVm {
                id: id.to_string(),
                mermaid_source: dag.as_ref().map(super::dag_mermaid::build_mermaid_source),
                message: if dag.is_some() {
                    None
                } else {
                    Some("No Graph available.".to_string())
                },
            };
            state.templates.render("tab-dag", &vm)
        }
        "plan" => partials::plan_tab_html(state, id),
        "log" => partials::log_tab_html(state, id, None),
        _ => {
            let session = super::dto::session_dto(
                &state.application,
                state.application.reconcile_session(id)?,
                true,
            );
            let pr_is_link = session
                .pr_url
                .as_deref()
                .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"));
            let vm = TabInfoVm {
                id: id.to_string(),
                config_source: session.config_source.clone(),
                location_label: if session.repo.is_some() {
                    "Repository".to_string()
                } else {
                    "Base dir".to_string()
                },
                location: session
                    .repo
                    .clone()
                    .unwrap_or_else(|| session.base_dir.clone()),
                worktree_branch: session.worktree_branch.clone(),
                created_at: format_local_time(&session.created_at),
                completed_at: session.completed_at.as_deref().map(format_local_time),
                pr_url: session.pr_url.clone(),
                pr_is_link,
                phase_error: session.phase_error.clone(),
                plan_error: session
                    .plan_error
                    .clone()
                    .filter(|error| Some(error) != session.phase_error.as_ref()),
            };
            state.templates.render("tab-info", &vm)
        }
    }
}

pub(crate) fn session_detail_html(state: &WebState, id: &str, tab: &str) -> Result<String> {
    let header = partials::header_html(state, id, None)?;
    let ask_panel = partials::ask_panel_html(state, id)?;
    let tab_panel = tab_panel_html(state, id, tab)?;
    let vm = SessionDetailVm {
        id: id.to_string(),
        header: RawHtml::new(header),
        ask_panel: RawHtml::new(ask_panel),
        settings: RawHtml::empty(),
        editor: RawHtml::empty(),
        active_tab: tab.to_string(),
        tab_panel: RawHtml::new(tab_panel),
        tab_hrefs: TabHrefs {
            info: format!("/webui/sessions/{id}/tab/info"),
            dag: format!("/webui/sessions/{id}/tab/dag"),
            plan: format!("/webui/sessions/{id}/tab/plan"),
            log: format!("/webui/sessions/{id}/tab/log"),
        },
        push_hrefs: TabHrefs {
            info: format!("/sessions/{id}?tab=info"),
            dag: format!("/sessions/{id}?tab=dag"),
            plan: format!("/sessions/{id}?tab=plan"),
            log: format!("/sessions/{id}?tab=log"),
        },
    };
    state.templates.render("session-detail", &vm)
}

pub(crate) fn pending_option_dialog_html(state: &WebState, id: &str) -> Result<String> {
    let Some(prompt) = state
        .application
        .pending_prompts(id)
        .into_iter()
        .find(|prompt| matches!(prompt.kind, PendingPromptKind::Option))
    else {
        return Ok(String::new());
    };
    partials::option_dialog_html(
        state,
        id,
        &prompt.request_id,
        prompt.question.as_deref().unwrap_or("Choose an option"),
        &prompt.choices,
    )
}

/// Flatten the skippable-step tree into indented checkbox rows.
pub(crate) fn step_tree_vm(
    steps: &[crate::workflow::SkippableStepNode],
    after_pr: &[crate::workflow::SkippableStepNode],
    skipped: &[String],
) -> StepTreeVm {
    let mut rows = Vec::new();
    push_steps(steps, 0, skipped, false, &mut rows);
    push_steps(after_pr, 0, skipped, true, &mut rows);
    StepTreeVm {
        empty: rows.is_empty(),
        steps: rows,
    }
}

fn push_steps(
    nodes: &[crate::workflow::SkippableStepNode],
    depth: usize,
    skipped: &[String],
    after_pr: bool,
    out: &mut Vec<StepRowVm>,
) {
    for node in nodes {
        out.push(StepRowVm {
            id: node.id.clone(),
            label: node.id.clone(),
            indent_style: format!("padding-left:{depth}rem"),
            checked: !skipped.contains(&node.id),
            after_pr,
        });
        push_steps(&node.children, depth + 1, skipped, after_pr, out);
    }
}

pub(crate) fn step_tree_html(
    state: &WebState,
    base_dir: &str,
    config_path: Option<&str>,
    repo: Option<&str>,
    skipped: &[String],
) -> Result<String> {
    let defaults = state.application.new_session_config_defaults(
        std::path::Path::new(base_dir),
        config_path.filter(|path| !path.is_empty()),
        repo.filter(|repo| !repo.trim().is_empty()),
    );
    let vm = match defaults {
        Ok(defaults) => {
            let effective = if skipped.is_empty() {
                defaults.default_skipped_steps.clone()
            } else {
                skipped.to_vec()
            };
            step_tree_vm(&defaults.steps, &defaults.after_pr_steps, &effective)
        }
        Err(_) => StepTreeVm {
            steps: Vec::new(),
            empty: true,
        },
    };
    state.templates.render("step-tree", &vm)
}

pub(crate) fn config_select_html(
    state: &WebState,
    base_dir: &str,
    repo: Option<&str>,
    selected: Option<&str>,
) -> Result<String> {
    let entries = super::dto::config_entries(
        &state.application,
        Some(base_dir),
        repo.filter(|repo| !repo.trim().is_empty()),
    );
    let vm = super::view::ConfigSelectVm::new(&entries, selected, base_dir);
    state.templates.render("components/config-select", &vm)
}

pub(crate) fn new_session_html(state: &WebState) -> Result<String> {
    let draft = state.application.draft().ok().flatten();
    let history = state.application.new_session_history_summary().ok();
    let base_dir = draft.as_ref().map_or_else(
        || {
            history
                .as_ref()
                .and_then(|history| history.last_working_dir.clone())
                .unwrap_or_default()
        },
        |draft| draft.working_dir.clone(),
    );
    let repo = draft
        .as_ref()
        .and_then(|draft| draft.repo.clone())
        .unwrap_or_default();
    let config_path = draft
        .as_ref()
        .and_then(|draft| draft.requested_config_path.clone())
        .or_else(|| {
            history
                .as_ref()
                .and_then(|history| history.last_requested_config_path.clone())
        });
    let skipped = draft
        .as_ref()
        .map(|draft| draft.skipped_steps.clone())
        .unwrap_or_default();
    let source_mode = if repo.trim().is_empty() {
        "directory"
    } else {
        "repo"
    };
    let vm = NewSessionVm {
        source_mode: source_mode.to_string(),
        directory_selected: source_mode == "directory",
        input: draft
            .as_ref()
            .map(|draft| draft.input.clone())
            .unwrap_or_default(),
        base_dir: base_dir.clone(),
        repo: repo.clone(),
        recent_working_dirs: history
            .map(|history| history.recent_working_dirs)
            .unwrap_or_default(),
        skip_planning: false,
        grill: false,
        formal_spec: false,
        no_interactive_planning: false,
        workspace_mode: "Worktree".to_string(),
        config_select: RawHtml::new(config_select_html(
            state,
            &base_dir,
            Some(&repo),
            config_path.as_deref(),
        )?),
        steps: RawHtml::new(step_tree_html(
            state,
            &base_dir,
            config_path.as_deref(),
            Some(&repo),
            &skipped,
        )?),
        attachments: RawHtml::new(state.templates.render(
            "attachment-list",
            &super::view::AttachmentListVm {
                items: Vec::new(),
                empty: true,
            },
        )?),
        error: None,
    };
    state.templates.render("new-session", &vm)
}

/// Build the Run All view model from the shared snapshot.
pub(crate) fn run_all_vm(state: &WebState) -> RunAllVm {
    let snapshot = state.run_all.snapshot();
    let sessions: Vec<super::dto::SessionDto> = state
        .application
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .map(|session| super::dto::session_dto(&state.application, session, false))
        .collect();
    let runnable = runnable_count(&sessions);
    let running: Vec<RunAllRunningVm> = snapshot
        .running
        .iter()
        .map(|(id, info)| RunAllRunningVm {
            id: id.clone(),
            title: truncate(&info.title, 80),
            phase: "Running".to_string(),
            step: info.step.clone(),
        })
        .collect();
    let results: Vec<RunAllResultVm> = snapshot
        .results
        .iter()
        .map(|result| RunAllResultVm {
            id: result.id.clone(),
            title: truncate(&result.title, 80),
            phase: result.phase.clone(),
            error: result.error.clone(),
            failed: result.phase == "Failed" || result.phase == "Busy",
        })
        .collect();
    let finished = results.len();
    RunAllVm {
        active: snapshot.status == super::run_all_state::RunAllStatus::Running,
        status: snapshot.status.as_str().to_string(),
        total: snapshot.total.max(finished + running.len()),
        finished,
        parallelism: snapshot.parallelism,
        running_count: running.len(),
        running_empty: running.is_empty(),
        running,
        results_empty: results.is_empty(),
        results,
        log: if snapshot.log.is_empty() {
            "Waiting for events...".to_string()
        } else {
            snapshot.log.iter().cloned().collect::<Vec<_>>().join("\n")
        },
        run_error: snapshot.run_error.clone(),
        cancelled: snapshot.status == super::run_all_state::RunAllStatus::Cancelled,
        runnable_count: runnable,
        run_all_confirm: format!(
            "Run {runnable} pending session(s) in parallel? Already-completed sessions will be skipped."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::current_session_id;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn current_session_id_reads_the_session_path() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "hx-current-url",
            HeaderValue::from_static("http://127.0.0.1:8484/sessions/abc?tab=log"),
        );
        assert_eq!(current_session_id(&headers).as_deref(), Some("abc"));

        headers.insert(
            "hx-current-url",
            HeaderValue::from_static("http://127.0.0.1:8484/new"),
        );
        assert_eq!(current_session_id(&headers), None);
    }
}
