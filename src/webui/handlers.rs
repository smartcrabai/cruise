//! Fragment and action routes under `/webui`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

use crate::application::{
    CurrentStepUpdateDto, Interactive, NewSessionRequest, OperationKind, PlanRequest, RunRequest,
    SessionSettingsRequest,
};
use crate::error::{CruiseError, Result};
use crate::session::WorkspaceMode;
use crate::step::option::OptionResult;

use super::WebState;
use super::partials;
use super::templates::RawHtml;
use super::view::{
    AttachmentListVm, AttachmentVm, DirectorySuggestionsVm, EditorVm, PublishDialogVm,
    RepoOptionsVm, SessionSettingsVm, SettingsVm, ToastVm,
};

const HX_PUSH_URL: HeaderName = HeaderName::from_static("hx-push-url");

/// Directory uploaded attachments are staged in before a session exists.
pub(crate) fn uploads_dir() -> PathBuf {
    std::env::temp_dir().join("cruise-webui-uploads")
}

// --- form/query helpers ----------------------------------------------------

/// Raw urlencoded pairs, which keep repeated fields such as `skippedSteps`.
#[derive(Debug, Default)]
pub(crate) struct Fields(Vec<(String, String)>);

impl Fields {
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn text(&self, name: &str) -> String {
        self.get(name).unwrap_or_default().to_string()
    }

    /// Non-empty value, trimmed.
    pub(crate) fn opt(&self, name: &str) -> Option<String> {
        self.get(name)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        self.get(name)
            .is_some_and(|value| value != "false" && value != "off" && !value.is_empty())
    }

    pub(crate) fn all(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }
}

impl From<Vec<(String, String)>> for Fields {
    fn from(pairs: Vec<(String, String)>) -> Self {
        Self(pairs)
    }
}

type Form = axum::extract::Form<Vec<(String, String)>>;
type Params = Query<Vec<(String, String)>>;

fn html(body: String) -> Response {
    Html(body).into_response()
}

fn push_url(mut response: Response, url: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(url) {
        response.headers_mut().insert(HX_PUSH_URL, value);
    }
    response
}

/// 400 whose body is only a toast partial, leaving the page untouched.
fn fail(state: &WebState, context: &str, error: &CruiseError) -> Response {
    let toast = partials::toast(state, &ToastVm::failed(context, error.to_string()));
    (StatusCode::BAD_REQUEST, Html(toast)).into_response()
}

fn run<F>(state: &WebState, context: &str, build: F) -> Response
where
    F: FnOnce() -> Result<Response>,
{
    match build() {
        Ok(response) => response,
        Err(error) => fail(state, context, &error),
    }
}

fn fragment<F>(state: &WebState, context: &str, build: F) -> Response
where
    F: FnOnce() -> Result<String>,
{
    run(state, context, || build().map(html))
}

// --- fragment routes -------------------------------------------------------

pub(crate) async fn sidebar(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let selected = super::pages::current_session_id(&headers);
    fragment(&state, "Sidebar", || {
        partials::sidebar_rows_html(&state, selected.as_deref())
    })
}

pub(crate) async fn tab(
    State(state): State<WebState>,
    Path((id, tab)): Path<(String, String)>,
) -> Response {
    let tab = super::pages::normalize_tab(Some(tab.as_str()));
    fragment(&state, "Tab", || {
        super::pages::tab_panel_html(&state, &id, tab)
    })
}

pub(crate) async fn session_log(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    fragment(&state, "Log", || partials::log_tab_html(&state, &id, None))
}

pub(crate) async fn session_settings(
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Response {
    fragment(&state, "Settings", || settings_html(&state, &id, true))
}

fn settings_html(state: &WebState, id: &str, open: bool) -> Result<String> {
    let session = super::dto::session_dto(
        &state.application,
        state.application.reconcile_session(id)?,
        false,
    );
    let defaults = state.application.new_session_config_defaults(
        std::path::Path::new(&session.base_dir),
        session.config_path.as_deref(),
        session.repo.as_deref(),
    );
    let (steps_vm, step_options) = match defaults {
        Ok(defaults) => {
            let tree = super::pages::step_tree_vm(
                &defaults.steps,
                &defaults.after_pr_steps,
                &session.skipped_steps,
            );
            let options = tree
                .steps
                .iter()
                .map(|step| super::view::ConfigOptionVm {
                    value: step.id.clone(),
                    label: step.id.clone(),
                    selected: session.current_step.as_deref() == Some(step.id.as_str()),
                })
                .collect();
            (tree, options)
        }
        Err(_) => (
            super::view::StepTreeVm {
                steps: Vec::new(),
                empty: true,
            },
            Vec::new(),
        ),
    };
    let vm = SessionSettingsVm {
        id: id.to_string(),
        base_dir: session.base_dir.clone(),
        repo: session.repo.clone().unwrap_or_default(),
        config_select: RawHtml::new(super::pages::config_select_html(
            state,
            &session.base_dir,
            session.repo.as_deref(),
            session.config_path.as_deref(),
        )?),
        steps: RawHtml::new(state.templates.render("step-tree", &steps_vm)?),
        current_step: session.current_step.clone(),
        step_options,
        open,
    };
    state.templates.render("session-settings", &vm)
}

pub(crate) async fn editor(
    State(state): State<WebState>,
    Path((id, kind)): Path<(String, String)>,
) -> Response {
    run(&state, "Editor", || {
        if kind == "publish" {
            let vm = PublishDialogVm {
                id: id.clone(),
                submit_url: format!("/webui/sessions/{id}/publish"),
            };
            return Ok(html(state.templates.render("publish-dialog", &vm)?));
        }
        let (field, label, placeholder, submit_label, required) = match kind.as_str() {
            "fix" => (
                "feedback",
                "Fix the plan",
                "Describe what to change in the plan...",
                "Submit fix",
                true,
            ),
            "ask" => (
                "question",
                "Ask about the plan",
                "Ask a question about the plan...",
                "Submit",
                true,
            ),
            "replan" => (
                "feedback",
                "Replan",
                "Optional guidance for the new plan...",
                "Replan",
                false,
            ),
            other => {
                return Err(CruiseError::Other(format!("unknown editor '{other}'")));
            }
        };
        let vm = EditorVm {
            id: id.clone(),
            kind: kind.clone(),
            submit_url: format!("/webui/sessions/{id}/{kind}"),
            field: field.to_string(),
            label: label.to_string(),
            placeholder: placeholder.to_string(),
            submit_label: submit_label.to_string(),
            required,
        };
        Ok(html(state.templates.render("editor", &vm)?))
    })
}

pub(crate) async fn directories(State(state): State<WebState>, Query(params): Params) -> Response {
    let params = Fields::from(params);
    // The picker input is named `baseDir` and includes itself in the request.
    let path = params
        .opt("path")
        .or_else(|| params.opt("baseDir"))
        .unwrap_or_default();
    // The input holds a partially typed path: list its parent directory and
    // keep the entries whose name starts with the typed leaf, as the React
    // `DirectoryPicker` did client-side.
    let (dir, prefix) = match path.rfind('/') {
        Some(index) => (&path[..=index], &path[index + 1..]),
        None => ("", path.as_str()),
    };
    fragment(&state, "Directories", || {
        let lowered = prefix.to_lowercase();
        let entries: Vec<_> = state
            .application
            .list_directory(dir)
            .into_iter()
            .filter(|entry| entry.name.to_lowercase().starts_with(&lowered))
            .collect();
        let vm = DirectorySuggestionsVm {
            empty: entries.is_empty(),
            entries,
        };
        state.templates.render("directory-suggestions", &vm)
    })
}

pub(crate) async fn configs(State(state): State<WebState>, Query(params): Params) -> Response {
    let params = Fields::from(params);
    fragment(&state, "Configs", || {
        super::pages::config_select_html(
            &state,
            &params.text("baseDir"),
            params.opt("repo").as_deref(),
            params.get("configPath"),
        )
    })
}

pub(crate) async fn new_steps(State(state): State<WebState>, Query(params): Params) -> Response {
    let params = Fields::from(params);
    fragment(&state, "Steps", || {
        super::pages::step_tree_html(
            &state,
            &params.text("baseDir"),
            params.get("configPath"),
            params.opt("repo").as_deref(),
            &[],
        )
    })
}

pub(crate) async fn repos(State(state): State<WebState>) -> Response {
    let repos = state
        .application
        .list_github_repositories()
        .await
        .unwrap_or_default();
    fragment(&state, "Repositories", || {
        state
            .templates
            .render("repo-options", &RepoOptionsVm { repos })
    })
}

pub(crate) async fn settings_modal(State(state): State<WebState>) -> Response {
    fragment(&state, "Settings", || {
        let config = state.application.app_config()?;
        state.templates.render(
            "settings-modal",
            &SettingsVm {
                run_all_parallelism: config.run_all_parallelism,
                error: None,
            },
        )
    })
}

pub(crate) async fn attachment_preview(Query(params): Params, request: Request) -> Response {
    let params = Fields::from(params);
    let Some(raw) = params.opt("path") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(path) = std::fs::canonicalize(&raw) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let root = std::fs::canonicalize(uploads_dir()).unwrap_or_else(|_| uploads_dir());
    if !path.starts_with(&root) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(error) => {
            eprintln!("webui: attachment preview failed: {error}");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

// --- action routes ---------------------------------------------------------

/// Steps the user unchecked.
///
/// The step tree posts one hidden `stepIds` per row and one `runSteps` value
/// per still-enabled row, because an unchecked checkbox submits nothing.
fn skipped_steps(fields: &Fields) -> Vec<String> {
    let ids = fields.all("stepIds");
    if ids.is_empty() {
        return fields.all("skippedSteps");
    }
    let enabled: std::collections::HashSet<String> = fields.all("runSteps").into_iter().collect();
    ids.into_iter().filter(|id| !enabled.contains(id)).collect()
}

fn plan_request(fields: &Fields) -> PlanRequest {
    PlanRequest {
        grill: fields.flag("grill"),
        formal_spec: fields.flag("formalSpec"),
        no_interactive_planning: fields.flag("noInteractivePlanning"),
        interactive: Interactive::new(true),
        ..PlanRequest::default()
    }
}

/// Requested workspace mode, or `None` when the form omitted the field so the
/// session keeps the mode it was created with.
fn workspace_mode(value: Option<&str>) -> Option<WorkspaceMode> {
    value.map(|value| match value {
        "CurrentBranch" => WorkspaceMode::CurrentBranch,
        _ => WorkspaceMode::Worktree,
    })
}

fn new_session_request(fields: &Fields) -> NewSessionRequest {
    let source_mode = fields.text("sourceMode");
    let repo = if source_mode == "repo" {
        fields.opt("repo")
    } else {
        None
    };
    let base_dir = if repo.is_some() {
        PathBuf::new()
    } else {
        super::dto::expanded_path(std::path::Path::new(&fields.text("baseDir")))
    };
    let config_path =
        super::dto::normalize_config_path(fields.opt("configPath")).map(PathBuf::from);
    let attachments = fields
        .all("attachments")
        .iter()
        .map(|path| super::dto::expanded_path(std::path::Path::new(path)))
        .collect();
    NewSessionRequest {
        input: fields.text("input"),
        base_dir,
        config_path,
        config_yaml: None,
        repo,
        workspace_mode: workspace_mode(fields.get("workspaceMode")).unwrap_or_default(),
        allow_dirty_working_tree: false,
        attachments,
        skipped_steps: skipped_steps(fields),
    }
}

fn discard_uploads(request: &NewSessionRequest) {
    let root = uploads_dir();
    for path in &request.attachments {
        if path.starts_with(&root) {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(crate) async fn create_session(
    State(state): State<WebState>,
    axum::extract::Form(pairs): Form,
) -> Response {
    create_session_inner(&state, &Fields::from(pairs), true)
}

pub(crate) async fn create_draft_session(
    State(state): State<WebState>,
    axum::extract::Form(pairs): Form,
) -> Response {
    create_session_inner(&state, &Fields::from(pairs), false)
}

fn create_session_inner(state: &WebState, fields: &Fields, plan: bool) -> Response {
    run(state, "Create session", || {
        let request = new_session_request(fields);
        let skip_planning = fields.flag("skipPlanning");
        let session = state.application.create_session(request.clone())?;
        discard_uploads(&request);
        let _ = state.application.clear_draft();
        let id = session.id.clone();
        if plan {
            let application = state.application.clone();
            let (sink, _log) = state.hub.sinks();
            if skip_planning {
                let id = id.clone();
                super::ops::spawn(async move { application.use_input_as_plan(&id, &*sink) });
            } else {
                let plan_request = plan_request(fields);
                let id = id.clone();
                super::ops::spawn(
                    async move { application.generate(&id, plan_request, sink).await },
                );
            }
        }
        let assumed = plan.then_some(OperationKind::Generate);
        let main = super::pages::session_detail_html(state, &id, "info")?;
        let mut body = main;
        body.push_str(&partials::sidebar_list_partial(state, Some(&id))?);
        if assumed.is_some() {
            body.push_str(&partials::replace(
                &format!("session-header-{id}"),
                &partials::header_html(state, &id, assumed)?,
            ));
        }
        Ok(push_url(html(body), &format!("/sessions/{id}")))
    })
}

pub(crate) async fn save_draft(
    State(state): State<WebState>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Save draft", || {
        let draft = crate::new_session_draft::NewSessionDraft {
            input: fields.text("input"),
            requested_config_path: fields.opt("configPath"),
            working_dir: fields.text("baseDir"),
            repo: fields.opt("repo"),
            skipped_steps: skipped_steps(&fields),
            updated_at: String::new(),
        };
        state.application.save_draft(&draft)?;
        Ok(html(partials::by_id("draft-status", "Draft saved")))
    })
}

pub(crate) async fn upload_attachments(
    State(state): State<WebState>,
    mut multipart: Multipart,
) -> Response {
    let mut items: Vec<AttachmentVm> = Vec::new();
    let dir = uploads_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return fail(
            &state,
            "Attachments",
            &CruiseError::Other(format!("cannot create upload directory: {error}")),
        );
    }
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().map(ToString::to_string);
        let Some(name) = field.file_name().map(sanitize_file_name) else {
            // Text part: a hidden `attachments` input carrying an already
            // uploaded path. The list is swapped wholesale, so it has to be
            // re-emitted or the earlier uploads are dropped and orphaned.
            if field_name.as_deref() == Some("attachments")
                && let Ok(path) = field.text().await
                && !path.trim().is_empty()
            {
                items.push(attachment_vm(path));
            }
            continue;
        };
        if !crate::attachments::is_image_path(std::path::Path::new(&name)) {
            return fail(
                &state,
                "Attachments",
                &CruiseError::Other(format!("{name} is not a supported image")),
            );
        }
        let Ok(bytes) = field.bytes().await else {
            continue;
        };
        let target = dir.join(format!("{}-{name}", uuid::Uuid::new_v4()));
        if let Err(error) = std::fs::write(&target, &bytes) {
            return fail(
                &state,
                "Attachments",
                &CruiseError::Other(format!("cannot save {name}: {error}")),
            );
        }
        items.push(attachment_vm(target.to_string_lossy().into_owned()));
    }
    fragment(&state, "Attachments", || {
        state.templates.render(
            "attachment-list",
            &AttachmentListVm {
                empty: items.is_empty(),
                items,
            },
        )
    })
}

fn sanitize_file_name(name: &str) -> String {
    std::path::Path::new(name).file_name().map_or_else(
        || "upload".to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn attachment_vm(path: String) -> AttachmentVm {
    AttachmentVm {
        preview_url: format!("/webui/attachments/preview?path={}", encode_query(&path)),
        name: sanitize_file_name(&path),
        path,
    }
}

fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    out
}

pub(crate) async fn approve(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    run(&state, "Approve", || {
        state.application.approve(&id)?;
        Ok(html(partials::header_and_row(&state, &id, None)?))
    })
}

pub(crate) async fn use_input_as_plan(
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Response {
    run(&state, "Use input as plan", || {
        let application = state.application.clone();
        let (sink, _log) = state.hub.sinks();
        let spawn_id = id.clone();
        super::ops::spawn(async move { application.use_input_as_plan(&spawn_id, &*sink) });
        Ok(html(partials::header_and_row(
            &state,
            &id,
            Some(OperationKind::Generate),
        )?))
    })
}

pub(crate) async fn generate(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Generate plan", || {
        let application = state.application.clone();
        let (sink, _log) = state.hub.sinks();
        let request = plan_request(&fields);
        let spawn_id = id.clone();
        super::ops::spawn(async move { application.generate(&spawn_id, request, sink).await });
        Ok(html(partials::header_and_row(
            &state,
            &id,
            Some(OperationKind::Generate),
        )?))
    })
}

pub(crate) async fn replan(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Replan", || {
        let application = state.application.clone();
        let (sink, _log) = state.hub.sinks();
        let request = PlanRequest {
            feedback: fields.opt("feedback"),
            interactive: Interactive::new(true),
            ..PlanRequest::default()
        };
        let spawn_id = id.clone();
        super::ops::spawn(async move { application.replan(&spawn_id, request, sink).await });
        let mut body = partials::header_and_row(&state, &id, Some(OperationKind::Replan))?;
        body.push_str(&partials::clear(&format!("editor-{id}")));
        Ok(html(body))
    })
}

pub(crate) async fn fix(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Fix plan", || {
        let feedback = fields
            .opt("feedback")
            .ok_or_else(|| CruiseError::Other("feedback must not be empty".to_string()))?;
        let application = state.application.clone();
        let (sink, _log) = state.hub.sinks();
        let spawn_id = id.clone();
        super::ops::spawn(async move { application.fix(&spawn_id, feedback, sink).await });
        let mut body = partials::header_and_row(&state, &id, Some(OperationKind::Fix))?;
        body.push_str(&partials::clear(&format!("editor-{id}")));
        Ok(html(body))
    })
}

pub(crate) async fn ask(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Ask", || {
        let question = fields
            .opt("question")
            .ok_or_else(|| CruiseError::Other("question must not be empty".to_string()))?;
        let application = state.application.clone();
        let (sink, _log) = state.hub.sinks();
        let spawn_id = id.clone();
        super::ops::spawn(async move { application.ask(&spawn_id, question, sink).await });
        let mut body = partials::header_and_row(&state, &id, Some(OperationKind::Ask))?;
        body.push_str(&partials::clear(&format!("editor-{id}")));
        Ok(html(body))
    })
}

pub(crate) async fn run_session(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Run", || {
        let application = state.application.clone();
        let (sink, log_sink) = state.hub.sinks();
        let request = RunRequest {
            workspace_mode: workspace_mode(fields.get("workspaceMode")),
            max_retries: None,
            ..RunRequest::default()
        };
        let spawn_id = id.clone();
        super::ops::spawn(async move {
            application
                .run_with_log_sink(&spawn_id, request, sink, Some(log_sink))
                .await
        });
        let mut body = partials::header_and_row(&state, &id, Some(OperationKind::Run))?;
        body.push_str(&partials::by_id(
            &format!("tab-panel-{id}"),
            &partials::log_tab_html(&state, &id, Some(OperationKind::Run))?,
        ));
        Ok(push_url(html(body), &format!("/sessions/{id}?tab=log")))
    })
}

pub(crate) async fn cancel(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    run(&state, "Cancel", || {
        let _ = state.application.cancel_session(&id);
        Ok(html(partials::header_and_row(&state, &id, None)?))
    })
}

pub(crate) async fn reset(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    run(&state, "Reset", || {
        state.application.reset_to_planned(&id)?;
        let mut body = partials::header_and_row(&state, &id, None)?;
        body.push_str(&partials::by_id(
            &format!("tab-panel-{id}"),
            &super::pages::tab_panel_html(&state, &id, "info")?,
        ));
        Ok(html(body))
    })
}

fn settings_request(fields: &Fields) -> SessionSettingsRequest {
    let current_step_update = match fields.get("currentStep") {
        None | Some("__unchanged__") => CurrentStepUpdateDto::Unchanged,
        Some("__clear__") => CurrentStepUpdateDto::Clear,
        Some(name) => CurrentStepUpdateDto::Set(name.to_string()),
    };
    SessionSettingsRequest {
        // An empty value is the "Auto" sentinel and must reach the session
        // edit layer: dropping it reads as "no selection submitted", which
        // re-pins a builtin session to the builtin config.
        config_path: super::dto::normalize_config_path(
            fields.get("configPath").map(ToString::to_string),
        ),
        skipped_steps: skipped_steps(fields),
        current_step_update,
    }
}

pub(crate) async fn update_settings(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Save settings", || {
        state
            .application
            .update_settings(&id, settings_request(&fields))?;
        let mut body = partials::header_and_row(&state, &id, None)?;
        body.push_str(&partials::by_id(&format!("settings-{id}"), ""));
        Ok(html(body))
    })
}

pub(crate) async fn regenerate_settings(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Save and regenerate", || {
        state
            .application
            .update_settings(&id, settings_request(&fields))?;
        let application = state.application.clone();
        let (sink, _log) = state.hub.sinks();
        let spawn_id = id.clone();
        super::ops::spawn(async move {
            application
                .replan(
                    &spawn_id,
                    PlanRequest {
                        interactive: Interactive::new(true),
                        ..PlanRequest::default()
                    },
                    sink,
                )
                .await
        });
        let mut body = partials::header_and_row(&state, &id, Some(OperationKind::Replan))?;
        body.push_str(&partials::by_id(&format!("settings-{id}"), ""));
        Ok(html(body))
    })
}

pub(crate) async fn ask_answer(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Answer", || {
        let request_id = fields.text("requestId");
        state
            .application
            .respond_to_ask(&id, &request_id, fields.text("answer"))?;
        let mut body = partials::clear(&format!("ask-panel-{id}"));
        body.push_str(&partials::header_and_row(&state, &id, None)?);
        Ok(html(body))
    })
}

pub(crate) async fn option_answer(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Option", || {
        let request_id = fields.text("requestId");
        state.application.respond_to_option(
            &id,
            &request_id,
            OptionResult {
                next_step: fields.opt("nextStep"),
                text_input: fields.opt("textInput"),
            },
        )?;
        Ok(html(partials::targeted(
            &format!("#dialog-{request_id}"),
            "delete",
            "",
        )))
    })
}

pub(crate) async fn publish(
    State(state): State<WebState>,
    Path(id): Path<String>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Publish issue", || {
        let issue = state
            .application
            .publish(&id, fields.flag("triggerCruise"))?;
        let mut body = partials::targeted(&format!("#dialog-publish-{id}"), "delete", "");
        body.push_str(&partials::toast(
            &state,
            &super::view::ToastVm::new(
                super::view::ToastKind::Completed,
                "Issue published",
                Some(issue.url.clone()),
            ),
        ));
        // Publishing deletes the session, so the detail pane has to go with
        // it; the publish form swaps nothing, hence the explicit partial.
        body.push_str(&partials::by_id(
            "main",
            &super::pages::empty_state_html(&state)?,
        ));
        body.push_str(&partials::sidebar_list_partial(&state, None)?);
        Ok(push_url(html(body), "/"))
    })
}

pub(crate) async fn discard(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    run(&state, "Discard", || {
        state.application.discard_session(&id)?;
        remove_session_response(&state)
    })
}

pub(crate) async fn delete(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    run(&state, "Delete", || {
        state.application.delete_session(&id)?;
        remove_session_response(&state)
    })
}

fn remove_session_response(state: &WebState) -> Result<Response> {
    let mut body = super::pages::empty_state_html(state)?;
    body.push_str(&partials::sidebar_list_partial(state, None)?);
    Ok(push_url(html(body), "/"))
}

pub(crate) async fn start_run_all(
    State(state): State<WebState>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Run All", || {
        if state.application.runtime().batch_active() {
            return Err(CruiseError::Other("Run All is already active".to_string()));
        }
        let titles = state
            .application
            .run_all_candidates()
            .unwrap_or_default()
            .into_iter()
            .map(|session| (session.id.clone(), session.input))
            .collect();
        state.run_all.start(titles);
        let sink: Arc<dyn crate::application::ApplicationEventSink> =
            Arc::new(super::events::RunAllSink {
                hub: Arc::clone(&state.hub),
                state: Arc::clone(&state.run_all),
            });
        let log_sink: Arc<dyn crate::application::LogSink> =
            Arc::new(super::events::RunAllLogSink {
                hub: Arc::clone(&state.hub),
                state: Arc::clone(&state.run_all),
            });
        let application = state.application.clone();
        let requested = fields
            .opt("parallelism")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value >= 1);
        let config_application = application.clone();
        let provider = move || -> Result<usize> {
            requested.map_or_else(
                || Ok(config_application.app_config()?.run_all_parallelism),
                Ok,
            )
        };
        let failure_sink = Arc::clone(&sink);
        tokio::spawn(async move {
            if let Err(error) = application
                .run_all_with_parallelism_provider(provider, sink, Some(log_sink))
                .await
                && !matches!(error, CruiseError::Interrupted)
            {
                let _ = failure_sink.send(crate::application::ApplicationEvent::BatchFailed {
                    error: error.to_string(),
                });
            }
        });
        let vm = super::pages::run_all_vm(&state);
        let mut body = state.templates.render("run-all", &vm)?;
        body.push_str(&partials::sidebar_list_partial(&state, None)?);
        Ok(push_url(html(body), "/run-all"))
    })
}

pub(crate) async fn cancel_run_all(State(state): State<WebState>) -> Response {
    run(&state, "Cancel Run All", || {
        let _ = state.application.cancel_run_all();
        let vm = super::pages::run_all_vm(&state);
        Ok(html(partials::by_id(
            "run-all-progress",
            &state.templates.render("run-all-progress", &vm)?,
        )))
    })
}

pub(crate) async fn clean(State(state): State<WebState>) -> Response {
    let application = state.application.clone();
    let report = tokio::task::spawn_blocking(move || application.clean()).await;
    run(&state, "Clean", || {
        let report = report
            .map_err(|error| CruiseError::Other(error.to_string()))?
            .map_err(|error| CruiseError::Other(error.to_string()))?;
        let mut body = partials::by_id(
            "clean-message",
            &super::templates::escape_html(&format!(
                "{} deleted (no-PR: {}, skipped: {})",
                report.deleted, report.no_pr_deleted, report.skipped
            )),
        );
        body.push_str(&partials::sidebar_list_partial(&state, None)?);
        Ok(html(body))
    })
}

pub(crate) async fn save_settings(
    State(state): State<WebState>,
    axum::extract::Form(pairs): Form,
) -> Response {
    let fields = Fields::from(pairs);
    run(&state, "Settings", || {
        let parallelism = fields
            .opt("runAllParallelism")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value >= 1)
            .ok_or_else(|| CruiseError::Other("Must be at least 1".to_string()))?;
        state
            .application
            .save_app_config(&crate::app_config::AppConfig {
                run_all_parallelism: parallelism,
            })?;
        Ok(html(partials::by_id("dialogs", "")))
    })
}

#[cfg(test)]
mod tests {
    use super::{Fields, encode_query};

    #[test]
    fn fields_reads_flags_and_repeated_values() {
        let fields = Fields::from(vec![
            ("skippedSteps".to_string(), "a".to_string()),
            ("skippedSteps".to_string(), "b".to_string()),
            ("grill".to_string(), "on".to_string()),
            ("repo".to_string(), "  ".to_string()),
        ]);
        assert_eq!(fields.all("skippedSteps"), vec!["a", "b"]);
        assert!(fields.flag("grill"));
        assert!(!fields.flag("missing"));
        assert_eq!(fields.opt("repo"), None);
    }

    #[test]
    fn encode_query_percent_encodes_path_separators() {
        assert_eq!(encode_query("/tmp/a b.png"), "%2Ftmp%2Fa%20b.png");
    }
}
