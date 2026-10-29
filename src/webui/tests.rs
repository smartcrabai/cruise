//! Router and template contract tests.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt as _;
use tower::ServiceExt as _;

use super::{WebState, assets, router};
use crate::application::{CruiseApplication, NewSessionRequest};
use crate::session::{SessionManager, SessionState, WorkspaceMode};

const CONFIG_YAML: &str = "command: [cat]\nsteps:\n  s1:\n    prompt: plan\n";

fn webui_root() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"))
}

fn repo_assets() -> assets::Assets {
    let root = webui_root();
    assets::Assets {
        templates_dir: root.join("templates"),
        static_dir: root.join("static"),
        dev_mode: true,
    }
}

struct Harness {
    _xdg_config: crate::test_support::EnvGuard,
    _process_lock: crate::test_support::ProcessLock,
    state: WebState,
    dir: tempfile::TempDir,
    manager: SessionManager,
}

fn harness() -> Harness {
    let process_lock = crate::test_support::lock_process();
    let Ok(dir) = tempfile::tempdir() else {
        panic!("tempdir");
    };
    let config_home = dir.path().join("xdg-config");
    let xdg_config = crate::test_support::EnvGuard::set("XDG_CONFIG_HOME", config_home.as_os_str());
    let manager = SessionManager::new(dir.path().to_path_buf());
    let application = CruiseApplication::new(manager.clone());
    let Ok(state) = WebState::new(application, &repo_assets()) else {
        panic!("web state");
    };
    Harness {
        _xdg_config: xdg_config,
        _process_lock: process_lock,
        state,
        dir,
        manager,
    }
}

fn create_session(harness: &Harness) -> SessionState {
    let base = harness.dir.path().join("work");
    let Ok(()) = std::fs::create_dir_all(&base) else {
        panic!("create base dir");
    };
    let result = harness.state.application.create_session(NewSessionRequest {
        input: "add a webui".to_string(),
        base_dir: base,
        config_path: None,
        config_yaml: Some(CONFIG_YAML.to_string()),
        repo: None,
        workspace_mode: WorkspaceMode::Worktree,
        allow_dirty_working_tree: true,
        attachments: Vec::new(),
        skipped_steps: Vec::new(),
        flags: BTreeMap::default(),
    });
    match result {
        Ok(session) => session,
        Err(error) => panic!("create_session: {error}"),
    }
}

async fn get(
    harness: &Harness,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String, String) {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let Ok(request) = builder.body(Body::empty()) else {
        panic!("request");
    };
    send(harness, request).await
}

async fn post(harness: &Harness, uri: &str, body: &'static str) -> (StatusCode, String, String) {
    let Ok(request) = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
    else {
        panic!("request");
    };
    send(harness, request).await
}

async fn send(harness: &Harness, request: Request<Body>) -> (StatusCode, String, String) {
    let response = match router(harness.state.clone()).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Ok(bytes) = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024).await else {
        panic!("body");
    };
    (
        status,
        content_type,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

const FLAG_CONFIG_YAML: &str = "command: [cat]
flags:
  e2e:
    description: Run browser tests
  docs: {}
  after_pr: {}
steps:
  always:
    command: 'true'
  browser:
    flag: e2e
    command: 'true'
  either:
    flag: [e2e, docs]
    command: 'true'
  docs-only:
    flag: docs
    command: 'true'
after-pr:
  publish:
    flag: after_pr
    command: 'true'
";

fn flag_config(harness: &Harness) -> std::path::PathBuf {
    let path = harness.dir.path().join("flags.yaml");
    std::fs::write(&path, FLAG_CONFIG_YAML)
        .unwrap_or_else(|error| panic!("write flag config: {error}"));
    path
}

fn flag_steps_uri(harness: &Harness, config: &std::path::Path, fields: &str) -> String {
    format!(
        "/webui/new/steps/flags?baseDir={}&configPath={}&{fields}",
        harness.dir.path().display(),
        config.display(),
    )
}

fn input_tag<'a>(html: &'a str, name: &str, value: &str) -> &'a str {
    html.split('<')
        .filter_map(|part| part.split_once('>').map(|(tag, _)| tag))
        .find(|tag| {
            tag.starts_with("input ")
                && tag.contains(&format!("name=\"{name}\""))
                && tag.contains(&format!("value=\"{value}\""))
        })
        .unwrap_or_else(|| panic!("missing input {name}={value}: {html}"))
}

#[tokio::test]
async fn webui_flag_steps_route_filters_off_and_on_trees() {
    let harness = harness();
    let config = flag_config(&harness);
    let (status, _, off) = get(&harness, &flag_steps_uri(&harness, &config, ""), &[]).await;
    assert_eq!(status, StatusCode::OK, "{off}");
    input_tag(&off, "stepIds", "always");
    for id in ["browser", "either", "docs-only", "publish"] {
        assert!(
            !off.contains(&format!("name=\"stepIds\" value=\"{id}\"")),
            "{off}"
        );
    }

    let (status, _, on) = get(
        &harness,
        &flag_steps_uri(
            &harness,
            &config,
            "flag.e2e=on&flag.docs=off&flag.after_pr=on",
        ),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{on}");
    for id in ["always", "browser", "either", "publish"] {
        input_tag(&on, "stepIds", id);
    }
    assert!(!on.contains("name=\"stepIds\" value=\"docs-only\""), "{on}");
    assert!(on.contains("after PR"), "{on}");
}

#[tokio::test]
async fn webui_config_change_restores_history_with_flags_off() {
    let harness = harness();
    let config = flag_config(&harness);
    let _state_home = crate::test_support::EnvGuard::set(
        "XDG_STATE_HOME",
        harness.dir.path().join("state").as_os_str(),
    );
    crate::new_session_history::NewSessionHistory {
        entries: vec![crate::new_session_history::NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir: harness.dir.path().to_string_lossy().into_owned(),
            repo: None,
            resolved_config_key: crate::new_session_history::resolved_config_key_for_session(
                &config,
            ),
            skipped_steps: vec!["always".into(), "browser".into()],
        }],
    }
    .save()
    .unwrap_or_else(|error| panic!("seed history: {error}"));
    // A config change is not a flag change. Stale flag and checkbox values
    // from the old form must not replace the new config's history defaults.
    let uri = format!(
        "/webui/new/steps?baseDir={}&configPath={}&flag.e2e=on&stepIds=always&runSteps=always",
        harness.dir.path().display(),
        config.display(),
    );
    let (status, _, html) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        !input_tag(&html, "runSteps", "always").contains("checked"),
        "{html}"
    );
    for id in ["browser", "either", "docs-only", "publish"] {
        assert!(
            !html.contains(&format!("name=\"stepIds\" value=\"{id}\"")),
            "{html}"
        );
    }
}

#[tokio::test]
async fn webui_flag_rows_precede_steps_and_rerender_the_containing_form() {
    let harness = harness();
    let config = flag_config(&harness);
    // Use the existing config-change route as well as the flag-change route.
    let uri = format!(
        "/webui/new/steps?baseDir={}&configPath={}",
        harness.dir.path().display(),
        config.display()
    );
    let (status, _, html) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let first_step = html
        .find("name=\"runSteps\"")
        .unwrap_or_else(|| panic!("{html}"));
    for name in ["e2e", "docs", "after_pr"] {
        let marker = format!("name=\"flag.{name}\"");
        let position = html.find(&marker).unwrap_or_else(|| panic!("{html}"));
        assert!(
            position < first_step,
            "flags must precede skip choices: {html}"
        );
        let start = html[..position]
            .rfind("<select")
            .unwrap_or_else(|| panic!("{html}"));
        let tag = html[start..].split('>').next().unwrap_or_default();
        for attr in [
            "hx-get=\"/webui/new/steps/flags\"",
            "hx-trigger=\"change\"",
            "hx-target=\"#step-tree\"",
            "hx-include=\"closest form\"",
        ] {
            assert!(tag.contains(attr), "missing {attr}: {tag}");
        }
    }
}

#[tokio::test]
async fn webui_flag_rerender_preserves_explicit_skips_and_flag_choices() {
    let harness = harness();
    let config = flag_config(&harness);
    let _state_home = crate::test_support::EnvGuard::set(
        "XDG_STATE_HOME",
        harness.dir.path().join("state").as_os_str(),
    );
    // Seed a contrary history default. An explicitly checked form must not
    // silently reapply this skip when its calculated skipped list is empty.
    harness
        .state
        .application
        .create_session(NewSessionRequest {
            input: "remember skipped always".to_string(),
            base_dir: harness.dir.path().to_path_buf(),
            config_path: Some(config.clone()),
            config_yaml: None,
            repo: None,
            workspace_mode: WorkspaceMode::Worktree,
            allow_dirty_working_tree: true,
            attachments: Vec::new(),
            skipped_steps: vec!["always".to_string()],
            flags: BTreeMap::default(),
        })
        .unwrap_or_else(|error| panic!("seed skip history: {error}"));
    // Normal recording deliberately forgets temporary working directories.
    // Restore this isolated fixture's scope so the regression really exercises
    // a nonempty history default, rather than merely an empty default.
    let mut history = crate::new_session_history::NewSessionHistory::load_best_effort();
    let entry = history
        .entries
        .first_mut()
        .unwrap_or_else(|| panic!("missing history seed"));
    entry.working_dir = harness.dir.path().to_string_lossy().into_owned();
    history
        .save()
        .unwrap_or_else(|error| panic!("save scoped history: {error}"));
    let defaults = harness
        .state
        .application
        .new_session_config_defaults(harness.dir.path(), config.to_str(), None)
        .unwrap_or_else(|error| panic!("read history defaults: {error}"));
    assert_eq!(defaults.default_skipped_steps, ["always"]);
    for (submitted, always_checked) in [
        (
            "stepIds=always&stepIds=browser&stepIds=either&runSteps=browser&runSteps=either",
            false,
        ),
        (
            "stepIds=always&stepIds=browser&stepIds=either&runSteps=always&runSteps=browser&runSteps=either",
            true,
        ),
    ] {
        let fields = format!("flag.e2e=on&flag.docs=off&flag.after_pr=off&{submitted}");
        let (status, _, html) =
            get(&harness, &flag_steps_uri(&harness, &config, &fields), &[]).await;
        assert_eq!(status, StatusCode::OK, "{html}");
        assert_eq!(
            input_tag(&html, "runSteps", "always").contains("checked"),
            always_checked,
            "{html}"
        );
        for id in ["browser", "either"] {
            assert!(
                input_tag(&html, "runSteps", id).contains("checked"),
                "{html}"
            );
        }
        for (name, choice) in [("e2e", "on"), ("docs", "off"), ("after_pr", "off")] {
            let marker = format!("name=\"flag.{name}\"");
            let select = html
                .split(&marker)
                .nth(1)
                .and_then(|tail| tail.split("</select>").next())
                .unwrap_or_else(|| panic!("missing {marker}: {html}"));
            assert!(
                select.contains(&format!("value=\"{choice}\" selected")),
                "{select}"
            );
        }
    }
}

#[tokio::test]
async fn webui_all_gated_flag_round_trip_does_not_restore_history_skips() {
    let harness = harness();
    let config = harness.dir.path().join("all-gated.yaml");
    std::fs::write(&config, "command: [cat]\nflags:\n  e2e: {}\nsteps:\n  browser:\n    flag: e2e\n    command: 'true'\n")
        .unwrap_or_else(|error| panic!("write all-gated workflow: {error}"));
    let _state_home = crate::test_support::EnvGuard::set(
        "XDG_STATE_HOME",
        harness.dir.path().join("state").as_os_str(),
    );
    crate::new_session_history::NewSessionHistory {
        entries: vec![crate::new_session_history::NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir: harness.dir.path().to_string_lossy().into_owned(),
            repo: None,
            resolved_config_key: crate::new_session_history::resolved_config_key_for_session(
                &config,
            ),
            skipped_steps: vec!["browser".into()],
        }],
    }
    .save()
    .unwrap_or_else(|error| panic!("seed contrary history: {error}"));
    let defaults = harness
        .state
        .application
        .new_session_config_defaults(harness.dir.path(), config.to_str(), None)
        .unwrap_or_else(|error| panic!("load defaults: {error}"));
    assert_eq!(defaults.default_skipped_steps, ["browser"]);
    for (fields, visible) in [
        ("flag.e2e=on&stepIds=browser&runSteps=browser", true),
        ("flag.e2e=off&stepIds=browser&runSteps=browser", false),
        ("flag.e2e=on", true),
    ] {
        let (status, _, html) =
            get(&harness, &flag_steps_uri(&harness, &config, fields), &[]).await;
        assert_eq!(status, StatusCode::OK, "{html}");
        if visible {
            assert!(
                input_tag(&html, "runSteps", "browser").contains("checked"),
                "{html}"
            );
        } else {
            assert!(!html.contains("name=\"stepIds\""), "{html}");
        }
    }
    let body = format!(
        "input=all-gated-round-trip&baseDir={}&configPath={}&flag.e2e=on&stepIds=browser&runSteps=browser",
        harness.dir.path().display(),
        config.display(),
    );
    let request = Request::builder()
        .method("POST")
        .uri("/webui/sessions/draft")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap_or_else(|error| panic!("create request: {error}"));
    let (status, _, html) = send(&harness, request).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let sessions = harness
        .state
        .application
        .list_sessions()
        .unwrap_or_else(|error| panic!("load created session: {error}"));
    let session = sessions
        .iter()
        .find(|s| s.input == "all-gated-round-trip")
        .unwrap_or_else(|| panic!("created session: {html}"));
    assert_eq!(session.skipped_steps, Vec::<String>::new());
    assert_eq!(session.flags.get("e2e"), Some(&true));
}

#[tokio::test]
async fn webui_dag_tab_tracks_external_flag_changes() {
    let harness = harness();
    let config = flag_config(&harness);
    let session = harness
        .state
        .application
        .create_session(NewSessionRequest {
            input: "dag flags".to_string(),
            base_dir: harness.dir.path().to_path_buf(),
            config_path: Some(config),
            config_yaml: None,
            repo: None,
            workspace_mode: WorkspaceMode::Worktree,
            allow_dirty_working_tree: true,
            attachments: Vec::new(),
            skipped_steps: Vec::new(),
            flags: BTreeMap::default(),
        })
        .unwrap_or_else(|error| panic!("create flag session: {error}"));

    for enabled in [false, true, false] {
        external_edit(&harness, &session.id, |stored| {
            stored.flags.insert("e2e".to_string(), enabled);
        });
        let uri = format!("/webui/sessions/{}/tab/dag", session.id);
        let (status, _, html) = get(&harness, &uri, &[]).await;
        assert_eq!(status, StatusCode::OK, "{html}");
        assert!(html.contains("always"), "{html}");
        assert_eq!(html.contains("browser"), enabled, "{html}");
        assert!(!html.contains("docs-only"), "{html}");
    }
}

#[tokio::test]
async fn webui_settings_filter_skips_but_keep_current_step_options() {
    let harness = harness();
    let config = flag_config(&harness);
    let session = harness
        .state
        .application
        .create_session(NewSessionRequest {
            input: "settings flags".to_string(),
            base_dir: harness.dir.path().to_path_buf(),
            config_path: Some(config),
            config_yaml: None,
            repo: None,
            workspace_mode: WorkspaceMode::Worktree,
            allow_dirty_working_tree: true,
            attachments: Vec::new(),
            skipped_steps: Vec::new(),
            flags: BTreeMap::default(),
        })
        .unwrap_or_else(|error| panic!("create flag session: {error}"));
    for enabled in [false, true] {
        external_edit(&harness, &session.id, |stored| {
            stored.flags.insert("e2e".to_string(), enabled);
        });
        let uri = format!("/webui/sessions/{}/settings", session.id);
        let (status, _, html) = get(&harness, &uri, &[]).await;
        assert_eq!(status, StatusCode::OK, "{html}");
        input_tag(&html, "stepIds", "always");
        assert_eq!(
            html.contains("name=\"stepIds\" value=\"browser\""),
            enabled,
            "{html}"
        );
        assert!(
            !html.contains("name=\"stepIds\" value=\"docs-only\""),
            "{html}"
        );
        let options = html
            .split("name=\"currentStep\"")
            .nth(1)
            .and_then(|tail| tail.split("</select>").next())
            .unwrap_or_else(|| panic!("missing currentStep: {html}"));
        for id in ["always", "browser", "either", "docs-only"] {
            assert!(
                options.contains(&format!("<option value=\"{id}\"")),
                "{options}"
            );
        }
    }
}

#[test]
fn publish_dialog_hides_trigger_checkbox_when_unsupported() {
    let harness = harness();
    let render = |supports: bool| {
        harness
            .state
            .templates
            .render(
                "publish-dialog",
                &serde_json::json!({
                    "id": "s1",
                    "submitUrl": "/webui/sessions/s1/publish",
                    "supportsTrigger": supports,
                }),
            )
            .unwrap_or_else(|error| panic!("{error}"))
    };
    assert!(render(true).contains("triggerCruise"));
    assert!(!render(false).contains("triggerCruise"));
}

#[test]
fn templates_render_all_fixtures() {
    let harness = harness();
    let root = webui_root().join("templates");
    let fixtures = super::fixtures::all();
    for (name, props) in &fixtures {
        let rendered = if *name == "layout" {
            harness.state.templates.render_document(props)
        } else {
            harness.state.templates.render(name, props)
        };
        match rendered {
            Ok(html) => assert!(!html.is_empty(), "{name} rendered empty"),
            Err(error) => panic!("{error}"),
        }
    }

    let mut on_disk = Vec::new();
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_file()
            && entry.path().extension().is_some_and(|ext| ext == "tsx")
            && let Ok(relative) = entry.path().strip_prefix(&root)
        {
            let name = relative
                .with_extension("")
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            on_disk.push(name);
        }
    }
    on_disk.sort();
    let mut covered: Vec<String> = fixtures
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    covered.sort();
    assert_eq!(on_disk, covered, "every template needs exactly one fixture");
}

#[test]
fn sidebar_rows_and_direct_session_components_render_badge_contents() {
    let harness = harness();
    let fixtures = super::fixtures::all();
    let render = |name: &str| {
        let (_, props) = fixtures
            .iter()
            .find(|(fixture, _)| *fixture == name)
            .unwrap_or_else(|| panic!("missing fixture for {name}"));
        harness
            .state
            .templates
            .render(name, props)
            .unwrap_or_else(|error| panic!("{error}"))
    };

    for name in ["sidebar-rows", "components/session-row"] {
        let html = render(name);
        assert!(html.contains("Awaiting Approval"), "{name}: {html}");
        assert!(html.contains("Add a WebUI"), "{name}: {html}");
    }

    let header = render("components/session-header");
    assert!(header.contains("Awaiting Approval"), "{header}");
    assert!(header.contains("Add a WebUI"), "{header}");
}

#[tokio::test]
async fn root_page_has_shell_contract() {
    let harness = harness();
    let (status, _, body) = get(&harness, "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!DOCTYPE html>"), "{body}");
    for needle in [
        "id=\"main\"",
        "id=\"toasts\"",
        "id=\"dialogs\"",
        "id=\"sidebar-toggle\"",
        "aria-controls=\"sidebar\"",
        "id=\"sidebar-backdrop\"",
        "data-sidebar-open=\"false\"",
        "hx-sse:connect=\"/webui/events\"",
        "/static/app.css",
        "/static/htmax.min.js",
    ] {
        assert!(body.contains(needle), "missing {needle}");
    }
    assert!(
        !body.contains("style=\"width:"),
        "inline sidebar width would override the mobile drawer width"
    );
}

#[cfg(unix)]
fn collect_step_ids(steps: impl IntoIterator<Item = String>) -> std::collections::HashSet<String> {
    steps.into_iter().collect()
}

#[cfg(unix)]
#[test]
fn repo_snapshot_preview_and_submitted_skips_follow_clone_resolved_config() {
    use crate::test_support::{EnvGuard, init_git_repo, prepend_to_path, write_executable_script};

    let harness = harness();
    let _forge = EnvGuard::remove("CRUISE_FORGE");
    let home = harness.dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap_or_else(|error| panic!("create fake home: {error}"));
    let _home = crate::test_support::set_fake_home(&home);

    let caller = harness.dir.path().join("caller");
    std::fs::create_dir_all(caller.join(".cruise"))
        .unwrap_or_else(|error| panic!("create caller defaults: {error}"));
    std::fs::write(
        caller.join(".cruise/default.yaml"),
        "pre_steps:\n  caller-only:\n    command: echo caller\n",
    )
    .unwrap_or_else(|error| panic!("write caller defaults: {error}"));

    let target = harness.dir.path().join("target-repo");
    std::fs::create_dir_all(target.join(".cruise"))
        .unwrap_or_else(|error| panic!("create target defaults: {error}"));
    std::fs::write(
        target.join(".cruise/default.yaml"),
        "pre_steps:\n  target-only:\n    command: echo target\n",
    )
    .unwrap_or_else(|error| panic!("write target defaults: {error}"));
    init_git_repo(&target);

    let bin = harness.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("create bin: {error}"));
    write_executable_script(
        &bin.join("gh"),
        "#!/bin/sh\nif [ \"${1:-}\" = \"repo\" ] && [ \"${2:-}\" = \"clone\" ]; then\n  exec git clone \"$CRUISE_TEST_REPO_PATH\" \"$4\"\nfi\nexit 1\n",
    );
    let _repo_source = EnvGuard::set("CRUISE_TEST_REPO_PATH", target.as_os_str());
    let _path = prepend_to_path(&bin);

    let repo = "owner/repo";
    let preview = harness
        .state
        .application
        .new_session_config_defaults(&caller, Some("__builtin__"), Some(repo))
        .unwrap_or_else(|error| panic!("preview defaults: {error}"));
    let preview_ids = collect_step_ids(preview.steps.iter().map(|step| step.id.clone()));
    assert!(!preview_ids.contains("pre/caller-only"));
    assert!(preview_ids.contains("write-test-first"));

    let preview_html = super::pages::step_tree_html(
        &harness.state,
        &caller.to_string_lossy(),
        Some("__builtin__"),
        Some(repo),
        None,
        &crate::flags::FlagValues::new(),
    )
    .unwrap_or_else(|error| panic!("render repo preview: {error}"));
    assert!(preview_html.contains("value=\"write-test-first\""));
    assert!(!preview_html.contains("pre/caller-only"));

    let session = harness
        .state
        .application
        .create_session(NewSessionRequest {
            input: "repo defaults preview".to_string(),
            base_dir: std::path::PathBuf::new(),
            config_path: Some(std::path::PathBuf::from("__builtin__")),
            config_yaml: None,
            repo: Some(repo.to_string()),
            workspace_mode: crate::session::WorkspaceMode::Worktree,
            allow_dirty_working_tree: false,
            attachments: Vec::new(),
            skipped_steps: vec!["write-test-first".to_string()],
            flags: BTreeMap::default(),
        })
        .unwrap_or_else(|error| panic!("create repo session: {error}"));
    let persisted = harness
        .manager
        .load_config(&session)
        .unwrap_or_else(|error| panic!("load persisted config: {error}"));
    let persisted_ids = collect_step_ids(
        crate::workflow::list_skippable_steps(&persisted)
            .unwrap_or_else(|error| panic!("list persisted steps: {error}"))
            .into_iter()
            .map(|step| step.id),
    );
    let cloned_preview = harness
        .state
        .application
        .new_session_config_defaults(&session.base_dir, Some("__builtin__"), Some(repo))
        .unwrap_or_else(|error| panic!("clone-resolved preview: {error}"));
    let cloned_preview_ids =
        collect_step_ids(cloned_preview.steps.iter().map(|step| step.id.clone()));
    assert!(persisted_ids.contains("pre/target-only"));
    assert!(!persisted_ids.contains("pre/caller-only"));
    assert!(preview_ids.is_subset(&persisted_ids));
    assert_eq!(cloned_preview_ids, persisted_ids);
    assert_eq!(session.skipped_steps, ["write-test-first"]);
    assert!(
        session
            .skipped_steps
            .iter()
            .all(|step| persisted_ids.contains(step)),
        "submitted skipped IDs must exist in the clone-resolved config"
    );
}

#[tokio::test]
async fn session_page_fragment_vs_full() {
    let harness = harness();
    let session = create_session(&harness);
    let uri = format!("/sessions/{}", session.id);

    let (status, _, fragment) = get(&harness, &uri, &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!fragment.starts_with("<!DOCTYPE"), "{fragment}");
    assert!(fragment.contains("id=\"session-detail\""));
    assert!(fragment.contains("<hx-partial id=\"session-list\""));

    let (status, _, full) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(full.starts_with("<!DOCTYPE html>"));
}

#[tokio::test]
async fn unknown_session_is_404() {
    let harness = harness();
    let (status, _, _) = get(&harness, "/sessions/nope", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(&harness, "/no-such-page", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_all_provider_failure_reaches_connected_sse_client() {
    let harness = harness();
    let config_home = harness.dir.path().join("xdg-config");
    let config_dir = config_home.join("cruise");
    if let Err(error) = std::fs::create_dir_all(&config_dir) {
        panic!("create config dir: {error}");
    }
    create_session(&harness);
    if let Err(error) = std::fs::write(config_dir.join("config.json"), "not-json") {
        panic!("write invalid config: {error}");
    }

    let Ok(request) = Request::builder().uri("/webui/events").body(Body::empty()) else {
        panic!("SSE request");
    };
    let response = match router(harness.state.clone()).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    let (_, _, body) = post(&harness, "/webui/run-all", "").await;
    assert!(body.contains("id=\"run-all-view\""), "{body}");

    let mut payload = String::new();
    while !payload.contains("hx-target=\"#run-all-view\"") || !payload.contains("event: notify") {
        let chunk =
            match tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()).await {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(error))) => panic!("SSE body error: {error}"),
                Ok(None) => panic!("SSE stream closed before Run All failure"),
                Err(error) => panic!("timed out waiting for Run All failure event: {error}"),
            };
        payload.push_str(&String::from_utf8_lossy(&chunk));
    }

    assert!(payload.contains("hx-swap=\"outerHTML\""), "{payload}");
    assert!(payload.contains(">Error</span>"), "{payload}");
    assert!(payload.contains("invalid config JSON"), "{payload}");
    assert!(
        payload.contains("Failed -- invalid config JSON"),
        "{payload}"
    );
}

#[tokio::test]
async fn action_error_is_toast_partial() {
    let harness = harness();
    let (status, _, body) = post(&harness, "/webui/sessions/nope/approve", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("hx-target=\"#toasts\""), "{body}");
    assert!(!body.contains("id=\"session-header"), "{body}");
}

#[tokio::test]
async fn css_endpoint_serves_generated_css() {
    let harness = harness();
    let (status, content_type, body) = get(&harness, "/static/app.css", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/css"), "{content_type}");
    assert!(body.contains(".prose"), "handwritten extras missing");
    assert!(body.contains(".h-dvh"), "dynamic viewport utility missing");
    assert!(
        body.contains(".md\\:hidden"),
        "responsive visibility missing"
    );
    assert!(
        body.contains("#app[data-sidebar-open=\"true\"] #sidebar"),
        "open drawer styling missing"
    );
}

#[tokio::test]
async fn static_htmax_served() {
    let harness = harness();
    let (status, content_type, _) = get(&harness, "/static/htmax.min.js", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.contains("javascript"), "{content_type}");
}

#[tokio::test]
async fn preview_rejects_paths_outside_uploads_dir() {
    let harness = harness();
    let (status, _, _) = get(
        &harness,
        "/webui/attachments/preview?path=%2Fetc%2Fhosts",
        &[],
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
}

#[tokio::test]
async fn new_and_run_all_pages_render() {
    let harness = harness();
    let (status, _, body) = get(&harness, "/new", &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("id=\"new-session-form\""), "{body}");

    let (status, _, body) = get(&harness, "/run-all", &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("id=\"run-all-view\""), "{body}");
}

#[tokio::test]
async fn session_tabs_render_their_contract_ids() {
    let harness = harness();
    let session = create_session(&harness);
    for (tab, needle) in [
        ("info", "id=\"tab-info-"),
        ("plan", "id=\"tab-plan-"),
        ("dag", "id=\"tab-dag-"),
        ("log", "id=\"tab-log-"),
    ] {
        let uri = format!("/webui/sessions/{}/tab/{tab}", session.id);
        let (status, _, body) = get(&harness, &uri, &[]).await;
        assert_eq!(status, StatusCode::OK, "{tab}");
        assert!(body.contains(needle), "{tab}: {body}");
    }
}

#[tokio::test]
async fn info_tab_shows_declared_flags_in_order_with_saved_values() {
    let harness = harness();
    let session = create_session(&harness);
    let config = flag_config(&harness);
    let mut stored = harness
        .manager
        .load(&session.id)
        .unwrap_or_else(|e| panic!("{e}"));
    stored.config = crate::session_config::SessionConfigRef::File { path: config };
    stored.flags = BTreeMap::from([
        ("e2e".to_string(), true),
        ("docs".to_string(), false),
        ("obsolete".to_string(), true),
    ]);
    harness
        .manager
        .save(&stored)
        .unwrap_or_else(|e| panic!("{e}"));

    let uri = format!("/webui/sessions/{}/tab/info", session.id);
    let (status, _, body) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e2e = body.find("e2e=on").unwrap_or_else(|| panic!("{body}"));
    let docs = body.find("docs=off").unwrap_or_else(|| panic!("{body}"));
    let after_pr = body
        .find("after_pr=off")
        .unwrap_or_else(|| panic!("{body}"));
    assert!(e2e < docs && docs < after_pr, "{body}");
    assert!(body.contains("Run browser tests"), "{body}");
    assert!(!body.contains("obsolete"), "{body}");
}

#[tokio::test]
async fn info_tab_shows_none_for_no_declared_flags() {
    let harness = harness();
    let session = create_session(&harness);
    let uri = format!("/webui/sessions/{}/tab/info", session.id);
    let (status, _, body) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let flags = body
        .split_once(">Flags</dt>")
        .unwrap_or_else(|| panic!("{body}"))
        .1;
    let field = flags
        .split_once("</dd>")
        .unwrap_or_else(|| panic!("{body}"))
        .0;
    assert!(field.contains("None"), "{body}");
}

#[tokio::test]
async fn info_tab_shows_unavailable_when_session_config_is_missing() {
    let harness = harness();
    let session = create_session(&harness);
    let mut stored = harness
        .manager
        .load(&session.id)
        .unwrap_or_else(|e| panic!("{e}"));
    stored.config = crate::session_config::SessionConfigRef::File {
        path: harness.dir.path().join("missing.yaml"),
    };
    stored.flags.insert("obsolete".to_string(), true);
    harness
        .manager
        .save(&stored)
        .unwrap_or_else(|e| panic!("{e}"));

    let uri = format!("/webui/sessions/{}/tab/info", session.id);
    let (status, _, body) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let flags = body
        .split_once(">Flags</dt>")
        .unwrap_or_else(|| panic!("{body}"))
        .1;
    let field = flags
        .split_once("</dd>")
        .unwrap_or_else(|| panic!("{body}"))
        .0;
    assert!(field.contains("Unavailable"), "{body}");
    assert!(!field.contains("obsolete"), "{body}");
}

#[test]
fn header_omits_input_paragraph_for_input_as_plan_session() {
    let harness = harness();
    let session = create_session(&harness);
    let manager = SessionManager::new(harness.dir.path().to_path_buf());
    let mut stored = manager.load(&session.id).unwrap_or_else(|e| panic!("{e}"));
    stored.input = String::new();
    stored.input_as_plan = true;
    stored.title = Some("plan title".to_string());
    manager.save(&stored).unwrap_or_else(|e| panic!("{e}"));

    let html = super::partials::header_html(&harness.state, &session.id, None)
        .unwrap_or_else(|e| panic!("{e}"));

    assert!(html.contains("plan title"), "{html}");
    assert!(!html.contains("<p "), "{html}");
}

#[test]
fn header_shows_input_paragraph_for_normal_session() {
    let harness = harness();
    let session = create_session(&harness);

    let html = super::partials::header_html(&harness.state, &session.id, None)
        .unwrap_or_else(|e| panic!("{e}"));

    assert!(html.contains("<p "), "{html}");
    assert!(html.contains("add a webui"), "{html}");
}

mod event_partials {
    use super::{create_session, harness};
    use crate::application::{
        ApplicationEvent, EventStream, OptionChoiceKind, OptionChoicePayload,
    };
    use crate::webui::partials::{SseMessage, for_event};

    fn html_of(messages: &[SseMessage]) -> String {
        messages
            .iter()
            .filter_map(|message| match message {
                SseMessage::Html(html) => Some(html.clone()),
                SseMessage::Notify { .. } => None,
            })
            .collect()
    }

    #[test]
    fn log_chunk_targets_the_run_all_log_and_escapes_markup() {
        let harness = harness();
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::LogChunk {
                session_id: None,
                stream: EventStream::Stdout,
                text: "<script>".to_string(),
                batch: true,
            },
        );
        let html = html_of(&messages);
        assert!(html.contains("hx-target=\"#run-all-log\""), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn option_required_opens_a_dialog_in_the_dialogs_container() {
        let harness = harness();
        let session = create_session(&harness);
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::OptionRequired {
                session_id: session.id.clone(),
                request_id: "r1".to_string(),
                prompt: "Pick one".to_string(),
                choices: vec![OptionChoicePayload {
                    label: "Continue".to_string(),
                    kind: OptionChoiceKind::Selector,
                    next_step: Some("s1".to_string()),
                }],
            },
        );
        let html = html_of(&messages);
        assert!(html.contains("hx-target=\"#dialogs\""), "{html}");
        assert!(html.contains("id=\"dialog-r1\""), "{html}");
    }

    #[test]
    fn ask_user_required_fills_the_ask_panel_and_notifies() {
        let harness = harness();
        let session = create_session(&harness);
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::AskUserRequired {
                session_id: session.id.clone(),
                request_id: "r1".to_string(),
                question: "Which database?".to_string(),
            },
        );
        let html = html_of(&messages);
        assert!(
            html.contains(&format!("id=\"ask-panel-{}\"", session.id)),
            "{html}"
        );
        assert!(messages.iter().any(|message| matches!(
            message,
            SseMessage::Notify { body, .. } if body.contains("Action required")
        )));
    }

    #[test]
    fn plan_finished_awaiting_approval_notifies_plan_ready() {
        let harness = harness();
        let session = create_session(&harness);
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::PlanFinished {
                session_id: session.id.clone(),
                phase: "Awaiting Approval".to_string(),
            },
        );
        assert!(messages.iter().any(|message| matches!(
            message,
            SseMessage::Notify { body, .. } if body.contains("Plan ready")
        )));
    }
}

fn external_edit(harness: &Harness, id: &str, edit: impl FnOnce(&mut SessionState)) {
    let other = SessionManager::new(harness.dir.path().to_path_buf());
    let mut state = other
        .load(id)
        .unwrap_or_else(|error| panic!("load: {error}"));
    edit(&mut state);
    other
        .save(&state)
        .unwrap_or_else(|error| panic!("save: {error}"));
}

fn current_version(harness: &Harness, id: &str) -> String {
    harness
        .state
        .application
        .session_view_version(id)
        .unwrap_or_else(|error| panic!("version: {error}"))
}

#[tokio::test]
async fn session_page_contains_sync_poller() {
    let harness = harness();
    let session = create_session(&harness);
    let uri = format!("/sessions/{}", session.id);
    let (status, _, body) = get(&harness, &uri, &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("id=\"session-sync-{}\"", session.id)),
        "{body}"
    );
    assert!(body.contains("hx-trigger=\"every 3s\""), "{body}");
}

#[tokio::test]
async fn sync_with_current_token_is_no_content() {
    let harness = harness();
    let session = create_session(&harness);
    let token = current_version(&harness, &session.id);
    let uri = format!("/webui/sessions/{}/sync?v={token}", session.id);
    let (status, _, body) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty(), "{body}");
}

#[tokio::test]
async fn sync_after_external_change_returns_fresh_partials() {
    let harness = harness();
    let session = create_session(&harness);
    let id = session.id.clone();
    let old = current_version(&harness, &id);
    external_edit(&harness, &id, |state| {
        state.title = Some("Externally Renamed Title".to_string());
    });

    let uri = format!("/webui/sessions/{id}/sync?v={old}");
    let (status, _, body) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("hx-target=\"#session-header-{id}\"")),
        "{body}"
    );
    assert!(
        body.contains(&format!("hx-target=\"#tab-info-{id}\"")),
        "{body}"
    );
    assert!(body.contains("Externally Renamed Title"), "{body}");
    assert!(body.contains(&format!("session-sync-{id}")), "{body}");
    assert!(
        !body.contains(&format!("v={old}")),
        "poller must carry the new token: {body}"
    );
    assert!(
        body.contains(&format!("v={}", current_version(&harness, &id))),
        "{body}"
    );
}

#[tokio::test]
async fn sync_after_external_run_starts_log_polling() {
    let harness = harness();
    let session = create_session(&harness);
    let id = session.id.clone();
    let old = current_version(&harness, &id);
    external_edit(&harness, &id, |state| {
        state.phase = crate::session::SessionPhase::Running;
        state.set_runner_to_current_process();
    });

    let uri = format!("/webui/sessions/{id}/sync?v={old}");
    let (status, _, body) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("hx-target=\"#tab-log-{id}\"")),
        "{body}"
    );
    assert!(body.contains("hx-trigger=\"every 2s\""), "{body}");
}

#[tokio::test]
async fn sync_reconciles_dead_runner_to_suspended() {
    let harness = harness();
    let session = create_session(&harness);
    let id = session.id.clone();
    let old = current_version(&harness, &id);
    external_edit(&harness, &id, |state| {
        state.phase = crate::session::SessionPhase::Running;
        state.runner_pid = None;
        state.runner_started_at = None;
    });

    let uri = format!("/webui/sessions/{id}/sync?v={old}");
    let (status, _, body) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Suspended"), "{body}");
    assert!(!body.contains(">Running<"), "{body}");
}

#[tokio::test]
async fn sync_for_deleted_session_replaces_main_with_not_found() {
    let harness = harness();
    let session = create_session(&harness);
    let id = session.id.clone();
    let token = current_version(&harness, &id);
    if let Err(error) = std::fs::remove_dir_all(harness.dir.path().join("sessions").join(&id)) {
        panic!("remove session: {error}");
    }

    let uri = format!("/webui/sessions/{id}/sync?v={token}");
    let (_, _, body) = get(&harness, &uri, &[]).await;
    assert!(body.contains("<hx-partial id=\"main\""), "{body}");
    assert!(body.contains("Session not found"), "{body}");
}

#[tokio::test]
async fn sidebar_refreshes_run_all_control() {
    let harness = harness();
    create_session(&harness);
    let (status, _, body) = get(&harness, "/webui/sidebar", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("hx-target=\"#run-all-control\""), "{body}");
    assert!(body.contains("<hx-partial id=\"session-list\""), "{body}");
}

#[tokio::test]
async fn sidebar_reconciles_stale_running_rows() {
    let harness = harness();
    let session = create_session(&harness);
    let id = session.id.clone();
    external_edit(&harness, &id, |state| {
        state.phase = crate::session::SessionPhase::Running;
        state.runner_pid = None;
        state.runner_started_at = None;
    });
    let (status, _, body) = get(&harness, "/webui/sidebar", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Suspended"), "{body}");
}

#[tokio::test]
async fn shutdown_begin_ends_open_sse_stream() {
    let harness = harness();
    let Ok(request) = Request::builder().uri("/webui/events").body(Body::empty()) else {
        panic!("SSE request");
    };
    let response = match router(harness.state.clone()).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    super::shutdown::begin(&harness.state);

    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()).await {
            Ok(None) => break,
            Ok(Some(_)) => {}
            Err(error) => panic!("SSE stream stayed open after shutdown: {error}"),
        }
    }
}

#[tokio::test]
async fn serve_returns_after_signal_even_with_open_sse_client() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let harness = harness();
    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        panic!("bind");
    };
    let Ok(addr) = listener.local_addr() else {
        panic!("local addr");
    };
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(super::serve(listener, harness.state.clone(), async move {
        let _ = rx.await;
    }));

    let Ok(mut client) = tokio::net::TcpStream::connect(addr).await else {
        panic!("connect");
    };
    if let Err(error) = client
        .write_all(b"GET /webui/events HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
    {
        panic!("write request: {error}");
    }
    let mut received = Vec::new();
    let mut buf = [0u8; 1024];
    while !String::from_utf8_lossy(&received).contains("\r\n\r\n") {
        match tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf)).await {
            Ok(Ok(0)) => panic!("closed before headers"),
            Ok(Ok(n)) => received.extend_from_slice(&buf[..n]),
            Ok(Err(error)) => panic!("read headers: {error}"),
            Err(error) => panic!("timed out waiting for headers: {error}"),
        }
    }
    let head = String::from_utf8_lossy(&received).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("text/event-stream"), "{head}");

    let _ = tx.send(());

    match tokio::time::timeout(std::time::Duration::from_secs(5), server).await {
        Ok(Ok(result)) => assert!(result.is_ok(), "{result:?}"),
        Ok(Err(error)) => panic!("server task failed: {error}"),
        Err(error) => panic!("serve did not return after signal: {error}"),
    }
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf)).await {
            Ok(Ok(0) | Err(_)) => break,
            Ok(Ok(_)) => {}
            Err(error) => panic!("client connection stayed open: {error}"),
        }
    }
}

#[tokio::test]
async fn serve_waits_for_operations_to_release_after_cancel() {
    let harness = harness();
    let runtime = harness.state.application.runtime();
    let claim = runtime
        .try_begin("busy", crate::application::OperationKind::Run)
        .unwrap_or_else(|e| panic!("{e}"));
    let token = claim.token();
    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        panic!("bind");
    };
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(super::serve(listener, harness.state.clone(), async move {
        let _ = rx.await;
    }));
    let _ = tx.send(());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !token.is_cancelled() {
        assert!(std::time::Instant::now() < deadline, "claim not cancelled");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), &mut server)
            .await
            .is_err(),
        "serve returned while an operation was still active"
    );
    drop(claim);
    match tokio::time::timeout(std::time::Duration::from_secs(5), server).await {
        Ok(Ok(result)) => assert!(result.is_ok(), "{result:?}"),
        Ok(Err(error)) => panic!("server task failed: {error}"),
        Err(error) => panic!("serve did not return after release: {error}"),
    }
}

#[cfg(unix)]
mod merge_pr {
    use super::*;
    use crate::pr_merge::fake_gh::{FakeGh, view_json};

    const URL: &str = "https://github.com/owner/repo/pull/8";

    struct Merge {
        _path: crate::test_support::EnvGuard,
        gh: FakeGh,
        session: SessionState,
        harness: Harness,
    }

    fn merge_harness(phase: crate::session::SessionPhase, pr_url: Option<&str>) -> Merge {
        let harness = harness();
        let gh = FakeGh::install(harness.dir.path());
        gh.set(
            "view.out",
            r#"{"state":"OPEN","mergeable":"MERGEABLE","reviewDecision":"APPROVED","statusCheckRollup":[{"__typename":"CheckRun","name":"unit-tests","status":"COMPLETED","conclusion":"SUCCESS"}]}"#,
        );
        let path = crate::test_support::prepend_to_path(&gh.bin());
        let mut session = create_session(&harness);
        session.phase = phase;
        session.pr_url = pr_url.map(str::to_string);
        let manager = SessionManager::new(harness.dir.path().to_path_buf());
        let Ok(()) = manager.save(&session) else {
            panic!("save session");
        };
        Merge {
            _path: path,
            gh,
            session,
            harness,
        }
    }

    fn completed() -> Merge {
        merge_harness(crate::session::SessionPhase::Completed, Some(URL))
    }

    fn exists(m: &Merge) -> bool {
        SessionManager::new(m.harness.dir.path().to_path_buf())
            .load(&m.session.id)
            .is_ok()
    }

    #[tokio::test]
    async fn completed_session_header_offers_merge_pr() {
        let m = completed();
        let (status, _, body) = get(&m.harness, &format!("/sessions/{}", m.session.id), &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Merge PR"), "{body}");
    }

    #[tokio::test]
    async fn session_header_without_pr_has_no_merge_pr() {
        let m = merge_harness(crate::session::SessionPhase::Completed, None);
        let (_, _, body) = get(&m.harness, &format!("/sessions/{}", m.session.id), &[]).await;
        assert!(!body.contains("Merge PR"), "{body}");
    }

    #[tokio::test]
    async fn merge_preview_displays_status_before_confirm() {
        let m = completed();

        let (status, _, body) = get(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("MERGEABLE"), "{body}");
        assert!(body.contains("APPROVED"), "{body}");
        assert!(body.contains("unit-tests"), "{body}");
        for method in ["merge", "squash", "rebase"] {
            assert!(body.contains(method), "method {method} missing: {body}");
        }
        assert!(
            body.contains("hx-post"),
            "confirm affordance missing: {body}"
        );
        assert!(m.gh.merge_calls().is_empty(), "preview must not merge");
        assert!(exists(&m));
    }

    #[tokio::test]
    async fn merge_preview_for_non_open_pr_has_no_submit_affordance() {
        let m = completed();
        m.gh.set("view.out", &view_json("MERGED"));

        let (_, _, body) = get(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            &[],
        )
        .await;

        assert!(!body.contains("hx-post"), "{body}");
        assert_eq!(m.gh.merge_calls(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn merge_preview_without_pr_is_rejected() {
        let m = merge_harness(crate::session::SessionPhase::Completed, None);

        let (status, _, body) = get(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(!body.contains("hx-post"), "{body}");
    }

    #[tokio::test]
    async fn merge_post_uses_selected_method_and_removes_cleaned_session() {
        let m = completed();
        m.gh.set("view_after.out", &view_json("MERGED"));

        let (status, _, body) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=rebase",
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(m.gh.merge_calls(), vec![format!("pr merge {URL} --rebase")]);
        assert!(!exists(&m), "session must be cleaned");
        let (_, _, sidebar) = get(&m.harness, "/webui/sidebar", &[]).await;
        assert!(!sidebar.contains(&m.session.id), "{sidebar}");
    }

    #[tokio::test]
    async fn merge_post_pending_keeps_session() {
        let m = completed();

        let (status, _, body) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=squash",
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(m.gh.merge_calls(), vec![format!("pr merge {URL} --squash")]);
        assert!(exists(&m));
        let (_, _, sidebar) = get(&m.harness, "/webui/sidebar", &[]).await;
        assert!(sidebar.contains(&m.session.id), "{sidebar}");
    }

    #[tokio::test]
    async fn merge_post_rejects_unknown_or_missing_method() {
        let m = completed();
        for form in ["method=fast-forward", "method=--auto", ""] {
            let (status, _, body) = post(
                &m.harness,
                &format!("/webui/sessions/{}/merge-pr", m.session.id),
                form,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{form}: {body}");
        }
        assert_eq!(m.gh.merge_calls(), Vec::<String>::new());
        assert!(exists(&m));
    }

    #[tokio::test]
    async fn merge_post_without_pr_is_rejected_without_merging() {
        let m = merge_harness(crate::session::SessionPhase::Completed, None);

        let (status, _, _) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=squash",
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(m.gh.merge_calls(), Vec::<String>::new());
        assert!(exists(&m));
    }

    #[tokio::test]
    async fn merge_post_gh_failure_keeps_session_and_reports_error() {
        let m = completed();
        m.gh.set("merge.exit", "1");

        let (status, _, body) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=squash",
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(exists(&m));
    }
}
