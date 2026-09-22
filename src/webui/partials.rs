//! `<hx-partial>` fragments: the shared vocabulary of action responses and the
//! SSE stream.
//!
//! htmx 4 rewrites `<hx-partial hx-target=… hx-swap=…>` into an out-of-band
//! swap task and silently drops a partial whose target is absent from the
//! page, so every connection can receive every event.

use std::collections::BTreeMap;

use crate::application::{ApplicationEvent, OperationKind, PendingPromptKind};
use crate::error::Result;

use super::WebState;
use super::templates::escape_html;
use super::view::{
    AskPanelVm, ChoiceVm, OptionDialogVm, PhaseBadgeVm, SessionHeaderVm, SidebarVm, TabLogVm,
    TabPlanVm, ToastKind, ToastVm, is_planning, runnable_count, session_row, sort_sessions,
    truncate,
};

/// One message pushed over the SSE connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SseMessage {
    /// Unnamed event: htmx swaps the `<hx-partial>` elements it contains.
    Html(String),
    /// Named `notify` event consumed by `app.notify` in `app.js`.
    Notify { title: String, body: String },
}

/// Wrap `html` in a partial addressed by element id (default `innerHTML` swap).
pub(crate) fn by_id(id: &str, html: &str) -> String {
    format!("<hx-partial id=\"{}\">{html}</hx-partial>", escape_html(id))
}

/// Wrap `html` in a partial with an explicit target selector and swap style.
pub(crate) fn targeted(target: &str, swap: &str, html: &str) -> String {
    format!(
        "<hx-partial hx-target=\"{}\" hx-swap=\"{}\">{html}</hx-partial>",
        escape_html(target),
        escape_html(swap)
    )
}

/// Replace an element wholesale; `html` must carry the same id.
pub(crate) fn replace(id: &str, html: &str) -> String {
    format!(
        "<hx-partial hx-target=\"#{}\" hx-swap=\"outerHTML\">{html}</hx-partial>",
        escape_html(id)
    )
}

pub(crate) fn clear(id: &str) -> String {
    by_id(id, "")
}

// --- view-model builders ---------------------------------------------------

/// Build the header view model, optionally pretending an operation is already
/// claimed so a just-spawned task shows Cancel immediately.
pub(crate) fn header_vm(
    state: &WebState,
    session_id: &str,
    assumed: Option<OperationKind>,
) -> Result<SessionHeaderVm> {
    let runtime = state.application.runtime();
    let operation = runtime.active_operation(session_id).or(assumed);
    let session = super::dto::session_dto(
        &state.application,
        state.application.reconcile_session(session_id)?,
        true,
    );
    let prompts = state.application.pending_prompts(session_id);
    let has_pending_prompt = !prompts.is_empty();
    let fixing = is_planning(operation) || session.fix_in_progress;
    let badge = PhaseBadgeVm::new(&session, fixing);
    let actions = super::actions::session_actions(&session, operation, has_pending_prompt);
    let title = session
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| session.input.clone());
    let pr_is_link = session
        .pr_url
        .as_deref()
        .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"));
    Ok(SessionHeaderVm {
        id: session.id.clone(),
        title,
        input: session.input.clone(),
        badge,
        current_step: session.current_step.clone(),
        phase_error: session.phase_error.clone(),
        plan_error: session.plan_error.clone(),
        pr_url: session.pr_url.clone(),
        pr_is_link,
        actions,
        busy: operation.is_some(),
        awaiting_input: session.awaiting_input,
        base_url: format!("/webui/sessions/{}", session.id),
    })
}

pub(crate) fn header_html(
    state: &WebState,
    session_id: &str,
    assumed: Option<OperationKind>,
) -> Result<String> {
    let vm = header_vm(state, session_id, assumed)?;
    state.templates.render("components/session-header", &vm)
}

/// The sidebar view model, with rows already rendered when `with_rows`.
pub(crate) fn sidebar_vm(state: &WebState, selected_id: Option<&str>) -> Result<SidebarVm> {
    let runtime = state.application.runtime();
    let mut sessions: Vec<super::dto::SessionDto> = state
        .application
        .list_sessions()?
        .into_iter()
        .map(|session| super::dto::session_dto(&state.application, session, false))
        .collect();
    sort_sessions(&mut sessions);
    let runnable = runnable_count(&sessions);
    let rows = sessions
        .iter()
        .map(|session| {
            let fixing =
                is_planning(runtime.active_operation(&session.id)) || session.fix_in_progress;
            session_row(session, selected_id, fixing)
        })
        .collect::<Vec<_>>();
    Ok(SidebarVm {
        version: env!("CARGO_PKG_VERSION"),
        selected_id: selected_id.map(ToString::to_string),
        empty: rows.is_empty(),
        sessions: rows,
        runnable_count: runnable,
        run_all_active: runtime.batch_active(),
        run_all_confirm: format!(
            "Run {runnable} pending session(s) in parallel? Already-completed sessions will be skipped."
        ),
        rows: super::templates::RawHtml::empty(),
    })
}

pub(crate) fn sidebar_rows_html(state: &WebState, selected_id: Option<&str>) -> Result<String> {
    let vm = sidebar_vm(state, selected_id)?;
    state.templates.render("sidebar-rows", &vm)
}

/// `<hx-partial>` refreshing the sidebar list container.
pub(crate) fn sidebar_list_partial(state: &WebState, selected_id: Option<&str>) -> Result<String> {
    Ok(by_id(
        "session-list",
        &sidebar_rows_html(state, selected_id)?,
    ))
}

pub(crate) fn row_partial(state: &WebState, session_id: &str) -> Result<String> {
    let runtime = state.application.runtime();
    let session = super::dto::session_dto(
        &state.application,
        state.application.reconcile_session(session_id)?,
        false,
    );
    let fixing = is_planning(runtime.active_operation(session_id)) || session.fix_in_progress;
    let vm = session_row(&session, None, fixing);
    let html = state
        .templates
        .render("components/session-row", &RowProps { row: vm })?;
    Ok(replace(&format!("session-row-{session_id}"), &html))
}

#[derive(serde::Serialize)]
struct RowProps {
    row: super::view::SessionRowVm,
}

/// Header + sidebar row, the response shape of nearly every action route.
pub(crate) fn header_and_row(
    state: &WebState,
    session_id: &str,
    assumed: Option<OperationKind>,
) -> Result<String> {
    let header = header_html(state, session_id, assumed)?;
    let mut out = replace(&format!("session-header-{session_id}"), &header);
    out.push_str(&row_partial(state, session_id)?);
    Ok(out)
}

pub(crate) fn toast(state: &WebState, vm: &ToastVm) -> String {
    match state.templates.render("toast", vm) {
        Ok(html) => targeted("#toasts", "beforeend", &html),
        Err(error) => {
            eprintln!("webui: failed to render toast: {error}");
            String::new()
        }
    }
}

pub(crate) fn log_tab_html(
    state: &WebState,
    session_id: &str,
    assumed: Option<OperationKind>,
) -> Result<String> {
    let session = state.application.reconcile_session(session_id)?;
    let saved_log = state.application.session_log(session_id, None)?;
    let running = state
        .application
        .runtime()
        .active_operation(session_id)
        .or(assumed)
        .is_some()
        || matches!(session.phase, crate::session::SessionPhase::Running);
    let vm = TabLogVm {
        id: session_id.to_string(),
        empty: saved_log.trim().is_empty(),
        saved_log,
        running,
        poll_url: format!("/webui/sessions/{session_id}/log"),
    };
    state.templates.render("tab-log", &vm)
}

pub(crate) fn plan_tab_html(state: &WebState, session_id: &str) -> Result<String> {
    let session = super::dto::session_dto(
        &state.application,
        state.application.reconcile_session(session_id)?,
        false,
    );
    let markdown = if session.plan_available {
        state
            .application
            .session_plan(session_id)
            .unwrap_or_default()
    } else {
        String::new()
    };
    let vm = TabPlanVm {
        id: session_id.to_string(),
        available: session.plan_available && !markdown.trim().is_empty(),
        html: super::templates::RawHtml::new(super::markdown::render_markdown(&markdown)),
    };
    state.templates.render("tab-plan", &vm)
}

pub(crate) fn ask_panel_html(state: &WebState, session_id: &str) -> Result<String> {
    let prompt = state
        .application
        .pending_prompts(session_id)
        .into_iter()
        .find(|prompt| matches!(prompt.kind, PendingPromptKind::Ask));
    let Some(prompt) = prompt else {
        return Ok(String::new());
    };
    let vm = AskPanelVm {
        session_id: session_id.to_string(),
        request_id: prompt.request_id,
        question: prompt.question.unwrap_or_default(),
    };
    state.templates.render("ask-panel", &vm)
}

pub(crate) fn option_dialog_html(
    state: &WebState,
    session_id: &str,
    request_id: &str,
    prompt: &str,
    choices: &[crate::application::OptionChoicePayload],
) -> Result<String> {
    let session_title = state.application.read_session(session_id).ok().map_or_else(
        || session_id.to_string(),
        |state| truncate(&state.input, 80),
    );
    let text_choice = choices
        .iter()
        .find(|choice| matches!(choice.kind, crate::application::OptionChoiceKind::TextInput));
    let selectors: Vec<ChoiceVm> = choices
        .iter()
        .filter(|choice| matches!(choice.kind, crate::application::OptionChoiceKind::Selector))
        .map(|choice| ChoiceVm {
            label: choice.label.clone(),
            selector: true,
            next_step: choice.next_step.clone(),
            vals: option_vals(request_id, choice.next_step.as_deref()),
        })
        .collect();
    let vm = OptionDialogVm {
        session_id: session_id.to_string(),
        session_title,
        request_id: request_id.to_string(),
        prompt: prompt.to_string(),
        choices: selectors,
        has_text_input: text_choice.is_some(),
        text_label: text_choice.map_or_else(|| "Answer".to_string(), |choice| choice.label.clone()),
        text_next_step: text_choice.and_then(|choice| choice.next_step.clone()),
        submit_url: format!("/webui/sessions/{session_id}/option"),
    };
    state.templates.render("option-dialog", &vm)
}

fn option_vals(request_id: &str, next_step: Option<&str>) -> String {
    let mut map = BTreeMap::new();
    map.insert("requestId".to_string(), request_id.to_string());
    if let Some(next) = next_step {
        map.insert("nextStep".to_string(), next.to_string());
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

// --- event -> partials -----------------------------------------------------

fn notify(kind: ToastKind, detail: &str) -> SseMessage {
    let sanitized: String = detail
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(60)
        .collect();
    SseMessage::Notify {
        title: "Cruise".to_string(),
        body: format!("{} -- {sanitized}", kind.label()),
    }
}

fn session_input(state: &WebState, session_id: &str) -> String {
    state
        .application
        .read_session(session_id)
        .map_or_else(|_| session_id.to_string(), |session| session.input)
}

fn run_all_partials(state: &WebState) -> Result<String> {
    let vm = super::pages::run_all_vm(state);
    let mut out = by_id(
        "run-all-progress",
        &state.templates.render("run-all-progress", &vm)?,
    );
    out.push_str(&by_id(
        "run-all-results",
        &state.templates.render("run-all-results", &vm)?,
    ));
    Ok(out)
}

/// Translate one application event into the partials every connected page
/// should receive.
pub(crate) fn for_event(state: &WebState, event: &ApplicationEvent) -> Vec<SseMessage> {
    match try_for_event(state, event) {
        Ok(messages) => messages,
        Err(error) => {
            eprintln!("webui: failed to render event partial: {error}");
            Vec::new()
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per application event keeps the SSE contract readable in one place"
)]
fn try_for_event(state: &WebState, event: &ApplicationEvent) -> Result<Vec<SseMessage>> {
    let mut out = Vec::new();
    match event {
        ApplicationEvent::LogChunk {
            session_id,
            stream,
            text,
            ..
        } => {
            let class = match super::events::stream_name(*stream) {
                "stderr" => "text-red-600 dark:text-red-400",
                "info" => "text-gray-500 dark:text-gray-400",
                _ => "",
            };
            let line = session_id
                .as_ref()
                .map_or_else(|| text.clone(), |id| format!("[{id}] {text}"));
            out.push(SseMessage::Html(targeted(
                "#run-all-log",
                "beforeend",
                &format!(
                    "<span class=\"{class}\">{}\n</span>",
                    escape_html(line.trim_end_matches('\n'))
                ),
            )));
        }
        ApplicationEvent::PlanChunk {
            session_id, text, ..
        } => {
            out.push(SseMessage::Html(targeted(
                &format!("#plan-progress-{session_id}"),
                "beforeend",
                &escape_html(text),
            )));
        }
        ApplicationEvent::PlanStarted { session_id, .. } => {
            out.push(SseMessage::Html(format!(
                "{}{}",
                header_and_row(state, session_id, None)?,
                clear(&format!("plan-progress-{session_id}"))
            )));
        }
        ApplicationEvent::PlanFinished { session_id, phase } => {
            let mut html = header_and_row(state, session_id, None)?;
            html.push_str(&replace(
                &format!("tab-plan-{session_id}"),
                &plan_tab_html(state, session_id)?,
            ));
            out.push(SseMessage::Html(html));
            if phase == "Awaiting Approval" {
                let input = session_input(state, session_id);
                out.push(SseMessage::Html(toast(
                    state,
                    &ToastVm::new(ToastKind::PlanReady, truncate(&input, 80), None),
                )));
                out.push(notify(ToastKind::PlanReady, &input));
            }
        }
        ApplicationEvent::PlanFailed { session_id, error }
        | ApplicationEvent::RunFailed { session_id, error } => {
            let input = session_input(state, session_id);
            let mut html = header_and_row(state, session_id, None)?;
            html.push_str(&toast(
                state,
                &ToastVm::new(
                    ToastKind::Failed,
                    truncate(&input, 80),
                    Some(truncate(error, 80)),
                ),
            ));
            out.push(SseMessage::Html(html));
            out.push(notify(ToastKind::Failed, error));
        }
        ApplicationEvent::PlanCancelled { session_id }
        | ApplicationEvent::RunCancelled { session_id }
        | ApplicationEvent::RunStarted { session_id }
        | ApplicationEvent::PrCreated { session_id, .. } => {
            out.push(SseMessage::Html(header_and_row(state, session_id, None)?));
        }
        ApplicationEvent::RunPhase { session_id, .. }
        | ApplicationEvent::StepStarted { session_id, .. } => {
            let mut html = header_and_row(state, session_id, None)?;
            if state.application.runtime().batch_active() {
                html.push_str(&run_all_partials(state)?);
            }
            out.push(SseMessage::Html(html));
        }
        ApplicationEvent::AskUserRequired {
            session_id,
            request_id,
            question,
        } => {
            let vm = AskPanelVm {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                question: question.clone(),
            };
            let input = session_input(state, session_id);
            let mut html = by_id(
                &format!("ask-panel-{session_id}"),
                &state.templates.render("ask-panel", &vm)?,
            );
            html.push_str(&header_and_row(state, session_id, None)?);
            html.push_str(&toast(
                state,
                &ToastVm::new(
                    ToastKind::InputRequired,
                    truncate(&input, 80),
                    Some(truncate(question, 80)),
                ),
            ));
            out.push(SseMessage::Html(html));
            out.push(notify(ToastKind::InputRequired, question));
        }
        ApplicationEvent::OptionRequired {
            session_id,
            request_id,
            prompt,
            choices,
        } => {
            let dialog = option_dialog_html(state, session_id, request_id, prompt, choices)?;
            let input = session_input(state, session_id);
            let mut html = targeted("#dialogs", "beforeend", &dialog);
            html.push_str(&toast(
                state,
                &ToastVm::new(
                    ToastKind::InputRequired,
                    truncate(&input, 80),
                    Some(truncate(prompt, 80)),
                ),
            ));
            out.push(SseMessage::Html(html));
            out.push(notify(ToastKind::InputRequired, prompt));
        }
        ApplicationEvent::RunFinished { session_id, phase } => {
            let mut html = header_and_row(state, session_id, None)?;
            html.push_str(&replace(
                &format!("tab-log-{session_id}"),
                &log_tab_html(state, session_id, None)?,
            ));
            if phase == "Completed" {
                let input = session_input(state, session_id);
                html.push_str(&toast(
                    state,
                    &ToastVm::new(ToastKind::Completed, truncate(&input, 80), None),
                ));
                out.push(SseMessage::Html(html));
                out.push(notify(ToastKind::Completed, &input));
            } else {
                out.push(SseMessage::Html(html));
            }
        }
        ApplicationEvent::BatchStarted { .. }
        | ApplicationEvent::BatchTotalChanged { .. }
        | ApplicationEvent::BatchSessionStarted { .. } => {
            out.push(SseMessage::Html(run_all_partials(state)?));
        }
        ApplicationEvent::BatchSessionFinished { id, .. } => {
            let mut html = run_all_partials(state)?;
            html.push_str(&sidebar_list_partial(state, None)?);
            let _ = id;
            out.push(SseMessage::Html(html));
        }
        ApplicationEvent::BatchFinished { cancelled } => {
            let vm = super::pages::run_all_vm(state);
            let mut html = replace("run-all-view", &state.templates.render("run-all", &vm)?);
            html.push_str(&sidebar_list_partial(state, None)?);
            let kind = if *cancelled {
                ToastKind::Failed
            } else {
                ToastKind::Completed
            };
            let detail = if *cancelled {
                "Run All cancelled"
            } else {
                "Run All finished"
            };
            html.push_str(&toast(
                state,
                &ToastVm::new(kind, "Run All", Some(detail.to_string())),
            ));
            out.push(SseMessage::Html(html));
            out.push(notify(kind, detail));
        }
        ApplicationEvent::BatchFailed { error } => {
            let vm = super::pages::run_all_vm(state);
            let mut html = replace("run-all-view", &state.templates.render("run-all", &vm)?);
            html.push_str(&toast(state, &ToastVm::failed("Run All", error.clone())));
            out.push(SseMessage::Html(html));
            out.push(notify(ToastKind::Failed, error));
        }
    }
    Ok(out)
}
