use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use cruise::application::{CruiseApplication, NewSessionRequest};
use cruise::config::WorkflowConfig;
use cruise::planning::plan_conversation_key;
use cruise::session::SessionManager;
use cruise::session_edit::{CurrentStepUpdate, SessionSettingsUpdate, update_session_settings};
use serde_json::{Value, json};
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &OsStr) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: every test using this guard holds ENV_LOCK for its full lifetime.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: every test using this guard holds ENV_LOCK for its full lifetime.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is only created while ENV_LOCK is held.
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn file_ref(path: &Path) -> Value {
    json!({
        "kind": "file",
        "path": path.to_string_lossy(),
    })
}

fn builtin_snapshot_ref() -> Value {
    json!({"kind": "builtin_snapshot"})
}

fn repo_snapshot_ref(relative_path: &str) -> Value {
    json!({
        "kind": "repo_snapshot",
        "relative_path": relative_path,
    })
}

fn inline_snapshot_ref() -> Value {
    json!({"kind": "inline_snapshot"})
}

fn state_value(id: &str, base_dir: &Path, config: &Value) -> Value {
    let phase = json!("Planned");
    state_value_with_phase(id, base_dir, &phase, config)
}

fn state_value_with_phase(id: &str, base_dir: &Path, phase: &Value, config: &Value) -> Value {
    json!({
        "id": id,
        "base_dir": base_dir.to_string_lossy(),
        "phase": phase,
        "config": config,
        "input": "session config contract",
        "created_at": "2026-09-21T00:00:00Z",
    })
}

fn write_state(manager: &SessionManager, id: &str, state: &Value) {
    let path = manager.state_path(id);
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|error| panic!("create state directory: {error}"));
    let bytes = serde_json::to_vec_pretty(state)
        .unwrap_or_else(|error| panic!("serialize state fixture: {error}"));
    fs::write(path, bytes).unwrap_or_else(|error| panic!("write state fixture: {error}"));
}

fn read_state_json(manager: &SessionManager, id: &str) -> Value {
    let bytes = fs::read(manager.state_path(id))
        .unwrap_or_else(|error| panic!("read persisted state: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse persisted state: {error}"))
}

fn write_session_snapshot(manager: &SessionManager, id: &str, yaml: &str) {
    let path = manager.sessions_dir().join(id).join("config.yaml");
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|error| panic!("create snapshot directory: {error}"));
    fs::write(path, yaml).unwrap_or_else(|error| panic!("write snapshot fixture: {error}"));
}

fn request_value(
    input: &str,
    base_dir: &Path,
    config_path: Option<&Path>,
    config_yaml: Option<&str>,
) -> Value {
    let mut value = json!({
        "input": input,
        "baseDir": base_dir.to_string_lossy(),
        "skippedSteps": [],
    });
    if let Some(path) = config_path {
        value["configPath"] = json!(path.to_string_lossy());
    }
    if let Some(yaml) = config_yaml {
        value["configYaml"] = json!(yaml);
    }
    value
}

fn request_from_value(value: Value) -> NewSessionRequest {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("decode request fixture: {error}"))
}

fn assert_file_reference(manager: &SessionManager, id: &str, expected_path: &Path) {
    let state = read_state_json(manager, id);
    assert_eq!(state["config"]["kind"], "file");
    assert_eq!(
        state["config"]["path"],
        expected_path.to_string_lossy().as_ref()
    );
    assert!(state.get("config_source").is_none());
    assert!(state.get("config_path").is_none());
    assert!(
        !manager.sessions_dir().join(id).join("config.yaml").exists(),
        "a live File reference must not be replaced with a session snapshot"
    );
}

fn load_config_error(manager: &SessionManager, id: &str) -> String {
    let state = manager
        .load(id)
        .unwrap_or_else(|error| panic!("load state before config read: {error}"));
    match manager.load_config(&state) {
        Ok(config) => panic!("expected config load to fail, got {config:?}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn session_state_round_trips_all_tagged_config_references_without_legacy_fields() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let base_dir = temp.path().join("repo");
    let refs = [
        ("file", file_ref(&temp.path().join("explicit.yaml"))),
        ("builtin", builtin_snapshot_ref()),
        ("repo", repo_snapshot_ref(".cruise/review.yaml")),
        ("inline", inline_snapshot_ref()),
    ];

    for (index, (label, config_ref)) in refs.into_iter().enumerate() {
        let id = format!("2026092100{index:04}");
        write_state(&manager, &id, &state_value(&id, &base_dir, &config_ref));

        let state = manager
            .load(&id)
            .unwrap_or_else(|error| panic!("{label} reference must deserialize: {error}"));
        manager
            .save(&state)
            .unwrap_or_else(|error| panic!("{label} reference must serialize: {error}"));

        let saved = read_state_json(&manager, &id);
        assert_eq!(
            saved["config"], config_ref,
            "reference kind changed for {label}"
        );
        assert!(
            saved.get("config_source").is_none(),
            "legacy config_source leaked into persisted {label} state"
        );
        assert!(
            saved.get("config_path").is_none(),
            "legacy config_path leaked into persisted {label} state"
        );
    }
}

#[test]
fn state_rejects_missing_unknown_and_incomplete_config_references() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let base_dir = temp.path().join("repo");
    let cases = [
        ("missing", Value::Null, "config"),
        ("unknown", json!({"kind": "future_reference"}), "unknown"),
        ("file_without_path", json!({"kind": "file"}), "path"),
    ];

    for (index, (label, config_ref, expected_fragment)) in cases.into_iter().enumerate() {
        let id = format!("2026092101{index:04}");
        let mut state = state_value(&id, &base_dir, &config_ref);
        if label == "missing" {
            state
                .as_object_mut()
                .unwrap_or_else(|| panic!("state fixture must be an object"))
                .remove("config");
        }
        write_state(&manager, &id, &state);

        let error = manager.load(&id).map_or_else(
            |error| error.to_string(),
            |value| panic!("accepted {label}: {value:?}"),
        );
        assert!(
            error.to_lowercase().contains(expected_fragment),
            "{label} error should identify {expected_fragment}, got: {error}"
        );
        assert!(
            !error.contains("config_source"),
            "old state fields must not be used as a compatibility fallback: {error}"
        );
    }
}

#[test]
fn application_creation_persists_file_references_for_explicit_env_auto_and_user_sources() {
    let _env_lock = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let application = CruiseApplication::new(manager.clone());
    fs::create_dir_all(temp.path().join("xdg-state").join("cruise"))
        .unwrap_or_else(|error| panic!("history directory: {error}"));
    let _xdg_config = EnvGuard::set(
        "XDG_CONFIG_HOME",
        temp.path().join("xdg-config").as_os_str(),
    );
    let _xdg_state = EnvGuard::set("XDG_STATE_HOME", temp.path().join("xdg-state").as_os_str());
    let _cruise_config = EnvGuard::remove("CRUISE_CONFIG");

    let explicit_base = temp.path().join("explicit-base");
    let explicit_path = temp.path().join("explicit.yaml");
    fs::create_dir_all(&explicit_base).unwrap_or_else(|error| panic!("explicit base: {error}"));
    fs::write(
        &explicit_path,
        "command: [explicit]\nsteps:\n  s:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("explicit config: {error}"));
    let explicit = application
        .create_session(request_from_value(request_value(
            "explicit",
            &explicit_base,
            Some(&explicit_path),
            None,
        )))
        .unwrap_or_else(|error| panic!("explicit session: {error}"));
    assert_file_reference(&manager, &explicit.id, &explicit_path);

    let auto_base = temp.path().join("auto-base");
    let auto_path = auto_base.join("cruise.yaml");
    fs::create_dir_all(&auto_base).unwrap_or_else(|error| panic!("auto base: {error}"));
    fs::write(
        &auto_path,
        "command: [auto]\nsteps:\n  s:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("auto config: {error}"));
    let auto = application
        .create_session(request_from_value(request_value(
            "auto", &auto_base, None, None,
        )))
        .unwrap_or_else(|error| panic!("auto session: {error}"));
    assert_file_reference(&manager, &auto.id, &auto_path);

    let env_base = temp.path().join("env-base");
    let env_path = temp.path().join("env.yaml");
    fs::create_dir_all(&env_base).unwrap_or_else(|error| panic!("env base: {error}"));
    fs::write(
        &env_path,
        "command: [env]\nsteps:\n  s:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("env config: {error}"));
    let env_config = EnvGuard::set("CRUISE_CONFIG", env_path.as_os_str());
    let env_session = application
        .create_session(request_from_value(request_value(
            "env", &env_base, None, None,
        )))
        .unwrap_or_else(|error| panic!("env session: {error}"));
    drop(env_config);
    assert_file_reference(&manager, &env_session.id, &env_path);

    let user_base = temp.path().join("user-base");
    let user_path = temp
        .path()
        .join("xdg-config")
        .join("cruise")
        .join("workflows")
        .join("user.yaml");
    fs::create_dir_all(&user_base).unwrap_or_else(|error| panic!("user base: {error}"));
    fs::create_dir_all(user_path.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|error| panic!("user workflow directory: {error}"));
    fs::write(
        &user_path,
        "command: [user]\nsteps:\n  s:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("user config: {error}"));
    let user = application
        .create_session(request_from_value(request_value(
            "user", &user_base, None, None,
        )))
        .unwrap_or_else(|error| panic!("user session: {error}"));
    assert_file_reference(&manager, &user.id, &user_path);
}

#[test]
fn new_session_rejects_config_path_and_inline_yaml_together_without_publishing_state() {
    let _env_lock = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let application = CruiseApplication::new(manager.clone());
    let base_dir = temp.path().join("repo");
    let file_path = temp.path().join("file.yaml");
    fs::create_dir_all(&base_dir).unwrap_or_else(|error| panic!("base dir: {error}"));
    fs::write(
        &file_path,
        "command: [file]\nsteps:\n  s:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("file config: {error}"));

    let result = application.create_session(request_from_value(request_value(
        "ambiguous",
        &base_dir,
        Some(&file_path),
        Some("command: [inline]\nsteps:\n  s:\n    command: \"true\"\n"),
    )));

    assert!(
        result.is_err(),
        "two config inputs must be rejected as ambiguous"
    );
    assert!(
        application
            .list_sessions()
            .unwrap_or_else(|error| panic!("list sessions: {error}"))
            .is_empty(),
        "an ambiguous request must not publish a new session"
    );
}

#[test]
fn inline_yaml_is_saved_as_an_inline_snapshot_and_is_not_reclassified_as_builtin() {
    let _env_lock = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let application = CruiseApplication::new(manager.clone());
    fs::create_dir_all(temp.path().join("xdg-state").join("cruise"))
        .unwrap_or_else(|error| panic!("history directory: {error}"));
    let _xdg_state = EnvGuard::set("XDG_STATE_HOME", temp.path().join("xdg-state").as_os_str());
    let base_dir = temp.path().join("repo");
    fs::create_dir_all(&base_dir).unwrap_or_else(|error| panic!("base dir: {error}"));
    fs::write(
        base_dir.join("cruise.yaml"),
        "command: [local]\nsteps:\n  local:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("local config: {error}"));
    let inline_yaml = "command: [inline]\nsteps:\n  inline:\n    command: \"true\"\n";

    let state = application
        .create_session(request_from_value(request_value(
            "inline",
            &base_dir,
            None,
            Some(inline_yaml),
        )))
        .unwrap_or_else(|error| panic!("inline session: {error}"));
    let saved = read_state_json(&manager, &state.id);
    assert_eq!(saved["config"]["kind"], "inline_snapshot");
    assert!(saved.get("config_source").is_none());
    assert!(saved.get("config_path").is_none());

    let snapshot = manager.sessions_dir().join(&state.id).join("config.yaml");
    assert!(
        snapshot.is_file(),
        "inline creation must persist config.yaml"
    );
    let loaded = manager
        .load_config(
            &manager
                .load(&state.id)
                .unwrap_or_else(|error| panic!("load state: {error}")),
        )
        .unwrap_or_else(|error| panic!("load inline snapshot: {error}"));
    assert_eq!(loaded.command, vec!["inline".to_string()]);

    let history = application
        .history()
        .unwrap_or_else(|error| panic!("load history: {error}"));
    let entry = history
        .entries
        .iter()
        .find(|entry| entry.input == "inline")
        .unwrap_or_else(|| panic!("inline selection was not recorded"));
    assert_ne!(entry.resolved_config_key, "__builtin__");
    assert!(!entry.resolved_config_key.contains("config.yaml"));
}

#[test]
fn snapshot_reference_stays_self_contained_after_clone_removal_and_source_changes() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let id = "20260921020001";
    let clone_dir = temp.path().join("clone");
    fs::create_dir_all(clone_dir.join(".cruise"))
        .unwrap_or_else(|error| panic!("clone directory: {error}"));
    fs::write(
        clone_dir.join(".cruise/review.yaml"),
        "command: [source]\nsteps:\n  source:\n    prompt: \"source\"\n",
    )
    .unwrap_or_else(|error| panic!("source config: {error}"));
    let snapshot_yaml =
        "command: [snapshot]\nsteps:\n  preserved:\n    prompt: \"inlined prompt\"\n";
    write_state(&manager, id, &{
        let repo_ref = repo_snapshot_ref(".cruise/review.yaml");
        let mut value = state_value(id, &clone_dir, &repo_ref);
        value["repo"] = json!("owner/repository");
        value
    });
    write_session_snapshot(&manager, id, snapshot_yaml);

    let before_removal = manager
        .load_config(
            &manager
                .load(id)
                .unwrap_or_else(|error| panic!("load state: {error}")),
        )
        .unwrap_or_else(|error| panic!("load repo snapshot: {error}"));
    assert_eq!(before_removal.command, vec!["snapshot".to_string()]);
    fs::write(
        clone_dir.join(".cruise/review.yaml"),
        "command: [changed-source]\nsteps:\n  changed:\n    prompt: \"changed\"\n",
    )
    .unwrap_or_else(|error| panic!("change source config: {error}"));
    fs::remove_dir_all(&clone_dir).unwrap_or_else(|error| panic!("remove clone: {error}"));

    let after_removal = manager
        .load_config(
            &manager
                .load(id)
                .unwrap_or_else(|error| panic!("reload state: {error}")),
        )
        .unwrap_or_else(|error| panic!("reload repo snapshot: {error}"));
    assert_eq!(after_removal.command, vec!["snapshot".to_string()]);
    assert_eq!(
        after_removal.steps["preserved"].prompt.as_deref(),
        Some("inlined prompt")
    );
}

#[test]
fn missing_or_invalid_file_and_snapshot_references_never_fallback_to_local_config() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let base_dir = temp.path().join("repo");
    fs::create_dir_all(&base_dir).unwrap_or_else(|error| panic!("base dir: {error}"));
    fs::write(
        base_dir.join("cruise.yaml"),
        "command: [local-fallback]\nsteps:\n  local:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("fallback config: {error}"));

    let missing_file = temp.path().join("missing.yaml");
    let file_id = "20260921030001";
    write_state(
        &manager,
        file_id,
        &state_value(file_id, &base_dir, &file_ref(&missing_file)),
    );
    let file_error = load_config_error(&manager, file_id);
    assert!(file_error.contains(missing_file.to_string_lossy().as_ref()));
    assert!(!file_error.contains("local-fallback"));

    let invalid_file = temp.path().join("invalid.yaml");
    fs::write(&invalid_file, "command: [invalid")
        .unwrap_or_else(|error| panic!("invalid file: {error}"));
    let invalid_file_id = "20260921030002";
    write_state(
        &manager,
        invalid_file_id,
        &state_value(invalid_file_id, &base_dir, &file_ref(&invalid_file)),
    );
    let invalid_file_error = load_config_error(&manager, invalid_file_id);
    assert!(invalid_file_error.contains(invalid_file.to_string_lossy().as_ref()));
    assert!(!invalid_file_error.contains("local-fallback"));

    for (index, config_ref) in [
        builtin_snapshot_ref(),
        repo_snapshot_ref(".cruise/review.yaml"),
        inline_snapshot_ref(),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("2026092103{index:04}");
        write_state(&manager, &id, &state_value(&id, &base_dir, &config_ref));
        let error = load_config_error(&manager, &id);
        assert!(
            error.contains("config.yaml"),
            "snapshot path missing from error: {error}"
        );
        assert!(!error.contains("local-fallback"));
    }
}

#[test]
fn file_reference_reads_updated_yaml_on_the_next_load() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let id = "20260921040001";
    let config_path = temp.path().join("live.yaml");
    fs::write(
        &config_path,
        "command: [first]\nsteps:\n  first:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("initial live config: {error}"));
    write_state(
        &manager,
        id,
        &state_value(id, temp.path(), &file_ref(&config_path)),
    );

    let first = manager
        .load_config(
            &manager
                .load(id)
                .unwrap_or_else(|error| panic!("load state: {error}")),
        )
        .unwrap_or_else(|error| panic!("load first live config: {error}"));
    assert_eq!(first.command, vec!["first".to_string()]);

    fs::write(
        &config_path,
        "command: [second]\nsteps:\n  second:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("updated live config: {error}"));
    let second = manager
        .load_config(
            &manager
                .load(id)
                .unwrap_or_else(|error| panic!("reload state: {error}")),
        )
        .unwrap_or_else(|error| panic!("load updated live config: {error}"));
    assert_eq!(second.command, vec!["second".to_string()]);
}

#[test]
fn application_graph_entry_point_and_manager_load_the_same_file_reference() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let application = CruiseApplication::new(manager.clone());
    let id = "20260921050001";
    let config_path = temp.path().join("graph.yaml");
    fs::write(
        &config_path,
        "command: [echo]\nsteps:\n  shared:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("graph config: {error}"));
    write_state(
        &manager,
        id,
        &state_value(id, temp.path(), &file_ref(&config_path)),
    );

    let direct = manager
        .load_config(
            &manager
                .load(id)
                .unwrap_or_else(|error| panic!("load state: {error}")),
        )
        .unwrap_or_else(|error| panic!("direct config load: {error}"));
    assert!(direct.steps.contains_key("shared"));

    let graph = application
        .session_dag(id)
        .unwrap_or_else(|error| panic!("application graph load: {error}"))
        .unwrap_or_else(|| panic!("application returned no graph"));
    assert!(graph.nodes.contains_key("shared"));
}

#[test]
fn editing_without_a_config_selection_preserves_snapshot_and_reports_no_config_change() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let id = "20260921060001";
    let base_dir = temp.path().join("removed-clone");
    let snapshot = "command: [repo-snapshot]\nsteps:\n  shared:\n    command: \"true\"\n";
    let repo_ref = repo_snapshot_ref(".cruise/review.yaml");
    let state = state_value(id, &base_dir, &repo_ref);
    write_state(&manager, id, &state);
    write_session_snapshot(&manager, id, snapshot);
    let before_snapshot = fs::read(manager.sessions_dir().join(id).join("config.yaml"))
        .unwrap_or_else(|error| panic!("read initial snapshot: {error}"));

    let (updated, config_changed) = update_session_settings(
        &manager,
        id,
        SessionSettingsUpdate {
            config_path: None,
            skipped_steps: vec!["shared".to_string()],
            current_step_update: CurrentStepUpdate::Unchanged,
        },
    )
    .unwrap_or_else(|error| panic!("preserving snapshot during edit: {error}"));

    assert!(!config_changed);
    assert_eq!(
        serde_json::to_value(&updated).unwrap_or_else(|error| panic!("serialize state: {error}"))["config"],
        state["config"]
    );
    let after_snapshot = fs::read(manager.sessions_dir().join(id).join("config.yaml"))
        .unwrap_or_else(|error| panic!("read preserved snapshot: {error}"));
    assert_eq!(after_snapshot, before_snapshot);
}

#[test]
fn explicit_empty_config_selection_reselects_auto_file_instead_of_creating_a_builtin_snapshot() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let id = "20260921070001";
    let base_dir = temp.path().join("repo");
    let auto_path = base_dir.join("cruise.yaml");
    fs::create_dir_all(&base_dir).unwrap_or_else(|error| panic!("base dir: {error}"));
    fs::write(
        &auto_path,
        "command: [auto]\nsteps:\n  auto:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("auto config: {error}"));
    write_state(
        &manager,
        id,
        &state_value(id, &base_dir, &builtin_snapshot_ref()),
    );
    write_session_snapshot(
        &manager,
        id,
        "command: [old-builtin]\nsteps:\n  old:\n    command: \"true\"\n",
    );

    let (_, config_changed) = update_session_settings(
        &manager,
        id,
        SessionSettingsUpdate {
            config_path: Some(String::new()),
            skipped_steps: vec![],
            current_step_update: CurrentStepUpdate::Unchanged,
        },
    )
    .unwrap_or_else(|error| panic!("explicit Auto selection: {error}"));

    assert!(config_changed);
    assert_file_reference(&manager, id, &auto_path);
}

#[test]
fn failed_session_rejects_a_config_reference_switch() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let id = "20260921080001";
    let original = temp.path().join("original.yaml");
    let replacement = temp.path().join("replacement.yaml");
    for (path, command) in [(&original, "original"), (&replacement, "replacement")] {
        fs::write(
            path,
            format!("command: [{command}]\nsteps:\n  s:\n    command: \\\"true\\\"\n"),
        )
        .unwrap_or_else(|error| panic!("write config {}: {error}", path.display()));
    }
    write_state(
        &manager,
        id,
        &state_value_with_phase(
            id,
            temp.path(),
            &json!({"Failed": "workflow failed"}),
            &file_ref(&original),
        ),
    );

    let error = update_session_settings(
        &manager,
        id,
        SessionSettingsUpdate {
            config_path: Some(replacement.to_string_lossy().into_owned()),
            skipped_steps: vec![],
            current_step_update: CurrentStepUpdate::Unchanged,
        },
    )
    .map_or_else(
        |error| error.to_string(),
        |value| panic!("accepted config switch: {value:?}"),
    );
    assert!(
        error.contains("Cannot change config"),
        "unexpected phase error: {error}"
    );
}

#[test]
fn conversation_keys_reuse_stable_identity_and_change_for_content_or_backend() {
    let command_config =
        WorkflowConfig::from_yaml("command: [echo]\nsteps:\n  shared:\n    command: \"true\"\n")
            .unwrap_or_else(|error| panic!("command config: {error}"));
    let changed_config =
        WorkflowConfig::from_yaml("command: [printf]\nsteps:\n  shared:\n    command: \"true\"\n")
            .unwrap_or_else(|error| panic!("changed config: {error}"));
    let sdk_config =
        WorkflowConfig::from_yaml("sdk: jcode\nsteps:\n  shared:\n    prompt: \"use sdk\"\n")
            .unwrap_or_else(|error| panic!("sdk config: {error}"));

    let stable_identity = "owner/repository::.cruise/review.yaml";
    let same_identity = plan_conversation_key(&command_config, stable_identity);
    assert_eq!(
        same_identity,
        plan_conversation_key(&command_config, stable_identity),
        "the same resolved reference must reuse its conversation key"
    );
    assert_ne!(
        same_identity,
        plan_conversation_key(&changed_config, stable_identity),
        "effective config content changes must invalidate the conversation"
    );
    assert_ne!(
        same_identity,
        plan_conversation_key(&sdk_config, stable_identity),
        "backend changes must invalidate the conversation"
    );
}

#[test]
fn cli_json_lists_a_new_state_and_derives_config_fields_without_reading_legacy_state_fields() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let data_home = temp.path().join("xdg-data");
    let config_path = temp.path().join("cli.yaml");
    let id = "20260921090001";
    fs::write(
        &config_path,
        "command: [cli]\nsteps:\n  cli:\n    command: \"true\"\n",
    )
    .unwrap_or_else(|error| panic!("cli config: {error}"));
    let manager = SessionManager::new(data_home.join("cruise"));
    write_state(
        &manager,
        id,
        &state_value(id, temp.path(), &file_ref(&config_path)),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["list", "--json"])
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CONFIG_HOME", temp.path().join("xdg-config"))
        .env("XDG_STATE_HOME", temp.path().join("xdg-state"))
        .output()
        .unwrap_or_else(|error| panic!("run cruise list: {error}"));
    assert!(
        output.status.success(),
        "list command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse list JSON: {error}; stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let sessions = json
        .as_array()
        .unwrap_or_else(|| panic!("list JSON must be an array: {json}"));
    assert_eq!(sessions.len(), 1);
    let listed = &sessions[0];
    assert_eq!(listed["id"], id);
    assert_eq!(
        listed["config_path"],
        config_path.to_string_lossy().as_ref()
    );
    assert!(
        listed["config_source"]
            .as_str()
            .is_some_and(|label| label.contains(config_path.to_string_lossy().as_ref()))
    );
}
