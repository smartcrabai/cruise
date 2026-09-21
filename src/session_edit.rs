use crate::error::{CruiseError, Result};
use crate::new_session_history::{NewSessionHistory, NewSessionHistoryEntry};
use crate::session::{SessionManager, SessionPhase, SessionState, current_iso8601};
use crate::session_config::SessionConfigRef;

/// How the session's `current_step` field should be modified.
pub enum CurrentStepUpdate {
    /// Leave `current_step` unchanged.
    Unchanged,
    /// Clear `current_step` so execution resumes from the beginning.
    Clear,
    /// Set `current_step` to the given workflow step name.
    Set(String),
}

pub struct SessionSettingsUpdate {
    pub config_path: Option<String>,
    pub skipped_steps: Vec<String>,
    pub current_step_update: CurrentStepUpdate,
}

struct PreparedSettings {
    requested_config_path: Option<String>,
    skipped_steps: Vec<String>,
    current_step_update: CurrentStepUpdate,
    is_failed_or_suspended: bool,
    config_ref: SessionConfigRef,
    config: crate::config::WorkflowConfig,
    config_changed: bool,
}

fn prepare_settings(
    manager: &SessionManager,
    session: &SessionState,
    update: SessionSettingsUpdate,
) -> Result<PreparedSettings> {
    let is_failed_or_suspended = matches!(
        &session.phase,
        SessionPhase::Failed(_) | SessionPhase::Suspended
    );
    match &session.phase {
        SessionPhase::Draft
        | SessionPhase::AwaitingApproval
        | SessionPhase::Planned
        | SessionPhase::Failed(_)
        | SessionPhase::Suspended => {}
        other => {
            return Err(CruiseError::Other(format!(
                "Cannot edit session in '{}' phase. Only 'Draft', 'Awaiting Approval', 'Planned', 'Failed' and 'Suspended' sessions are editable.",
                other.label()
            )));
        }
    }

    let SessionSettingsUpdate {
        config_path: requested_config_path,
        skipped_steps,
        current_step_update,
    } = update;

    let (requested_config_path, config_ref, config) = match requested_config_path {
        None => (None, session.config.clone(), manager.load_config(session)?),
        Some(raw) => {
            let requested_config_path =
                (!raw.trim().is_empty()).then(|| crate::new_session_history::expand_tilde(&raw));
            let (yaml, source) = crate::resolver::resolve_config_in_dir(
                requested_config_path.as_deref(),
                &session.base_dir,
            )?;
            let config =
                crate::resolver::resolve_workflow_config(&yaml, &source, &session.base_dir)?;
            let config_ref = SessionConfigRef::from_source(
                &source,
                session.repo.as_ref().map(|_| session.base_dir.as_path()),
            )?;
            (requested_config_path, config_ref, config)
        }
    };
    let config_changed = config_ref != session.config;
    if is_failed_or_suspended && config_changed {
        return Err(CruiseError::Other(
            "Cannot change config for a Failed or Suspended session. Only skip steps and current step can be edited.".to_string(),
        ));
    }
    if !matches!(&current_step_update, CurrentStepUpdate::Unchanged) && !is_failed_or_suspended {
        return Err(CruiseError::Other(
            "Cannot update current step for a session that is not Failed or Suspended.".to_string(),
        ));
    }

    crate::config::validate_config(&config)?;
    if let CurrentStepUpdate::Set(step_name) = &current_step_update {
        validate_current_step_name(&config, step_name, &skipped_steps)?;
    }
    Ok(PreparedSettings {
        requested_config_path,
        skipped_steps,
        current_step_update,
        is_failed_or_suspended,
        config_ref,
        config,
        config_changed,
    })
}
/// Update config and skip-step settings for an editable session.
///
/// Returns `(updated_state, config_changed)`. `config_changed` is true only when
/// the requested config path differs from what was stored before the call — callers
/// can use this to decide whether to regenerate the plan.
///
/// # Errors
///
/// Returns an error if the session cannot be loaded, the phase is not editable,
/// the config path cannot be resolved, or the config YAML is invalid.
pub fn update_session_settings(
    manager: &SessionManager,
    session_id: &str,
    update: SessionSettingsUpdate,
) -> Result<(SessionState, bool)> {
    let mut session = manager.load(session_id)?;
    let PreparedSettings {
        requested_config_path,
        skipped_steps,
        current_step_update,
        is_failed_or_suspended,
        config_ref,
        config,
        config_changed,
    } = prepare_settings(manager, &session, update)?;

    match current_step_update {
        CurrentStepUpdate::Unchanged => {}
        CurrentStepUpdate::Clear => {
            session.current_step = None;
            session.current_step_is_node_id = false;
            session.has_dag = false;
            session.execution_id = Some(uuid::Uuid::new_v4().to_string());
        }
        CurrentStepUpdate::Set(step_name) => {
            session.current_step = Some(step_name);
            session.current_step_is_node_id = false;
        }
    }
    session.config = config_ref;
    session.skipped_steps = skipped_steps;
    session.plan_error = None;
    session.updated_at = Some(current_iso8601());

    if config_changed {
        manager.save_config(&session, &config)?;
    } else {
        manager.save(&session)?;
    }
    if !is_failed_or_suspended {
        record_history(&session, requested_config_path.as_ref());
    }
    Ok((session, config_changed))
}

fn record_history(session: &SessionState, requested_config_path: Option<&String>) {
    let resolved_config_key = session
        .config
        .stable_identity(session.repo.as_deref(), &session.id);
    let mut history = NewSessionHistory::load_best_effort();
    history.record_selection(NewSessionHistoryEntry {
        selected_at: current_iso8601(),
        input: session.input.clone(),
        requested_config_path: requested_config_path.cloned(),
        // Clone paths are temporary; never expose them as recent directories.
        working_dir: if session.repo.is_some() {
            String::new()
        } else {
            session.base_dir.to_string_lossy().into_owned()
        },
        repo: session.repo.clone(),
        resolved_config_key,
        skipped_steps: session.skipped_steps.clone(),
    });
    history.save_best_effort();
}

fn validate_current_step_name(
    config: &crate::config::WorkflowConfig,
    step_name: &str,
    skipped_steps: &[String],
) -> Result<()> {
    let nodes = crate::workflow::list_skippable_steps(config)
        .map_err(|e| CruiseError::Other(format!("step expansion error: {e}")))?;
    let valid_ids: std::collections::HashSet<&str> = nodes
        .iter()
        .flat_map(|n| n.expanded_step_ids.iter().map(String::as_str))
        .collect();
    if !valid_ids.contains(step_name) {
        return Err(CruiseError::Other(format!(
            "Step '{step_name}' does not exist in the workflow config."
        )));
    }
    if skipped_steps.iter().any(|s| s == step_name) {
        return Err(CruiseError::Other(format!(
            "Cannot set current_step to '{step_name}' because it is in skipped_steps."
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionManager, SessionPhase, SessionState};
    use std::fs;

    fn make_session(id: &str, base_dir: &std::path::Path) -> SessionState {
        let mut s = SessionState::new(
            id.to_string(),
            base_dir.to_path_buf(),
            crate::session_config::SessionConfigRef::File {
                path: base_dir.join("cruise.yaml"),
            },
            "test task".to_string(),
        );
        s.phase = SessionPhase::Planned;
        s
    }

    fn write_minimal_config(dir: &std::path::Path) {
        fs::write(
            dir.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo ok",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
    }

    // --- Phase gating ---

    #[test]
    fn test_update_session_settings_draft_phase_succeeds() {
        // Given: a Draft session with a config file in its base dir
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000001", &repo);
        session.phase = SessionPhase::Draft;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = update_session_settings(
            &manager,
            "20260619000001",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: Draft is an allowed phase
        assert!(
            result.is_ok(),
            "Draft phase should be allowed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_update_session_settings_awaiting_approval_phase_succeeds() {
        // Given: an AwaitingApproval session
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000002", &repo);
        session.phase = SessionPhase::AwaitingApproval;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = update_session_settings(
            &manager,
            "20260619000002",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec!["build".to_string()],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then
        assert!(
            result.is_ok(),
            "AwaitingApproval phase should be allowed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_update_session_settings_planned_phase_succeeds() {
        // Given: a Planned session
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session = make_session("20260619000003", &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = update_session_settings(
            &manager,
            "20260619000003",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec!["test".to_string()],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then
        assert!(
            result.is_ok(),
            "Planned phase should be allowed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_update_session_settings_running_phase_fails_with_phase_message() {
        // Given: a Running session
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000004", &repo);
        session.phase = SessionPhase::Running;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = update_session_settings(
            &manager,
            "20260619000004",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: must reject with an error mentioning the phase
        assert!(result.is_err(), "Running phase should be rejected");
        let msg = result
            .err()
            .unwrap_or_else(|| panic!("expected Err"))
            .to_string();
        assert!(
            msg.contains("Running") || msg.contains("running"),
            "error should mention phase: {msg}"
        );
    }

    #[test]
    fn test_update_session_settings_completed_phase_fails() {
        // Given: a Completed session
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000007", &repo);
        session.phase = SessionPhase::Completed;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = update_session_settings(
            &manager,
            "20260619000007",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then
        assert!(result.is_err(), "Completed phase should be rejected");
    }

    // --- Persistence correctness ---

    #[test]
    fn test_update_session_settings_skipped_steps_persisted_on_disk() {
        // Given: a Planned session with no skipped steps
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000008", &repo);
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = update_session_settings(
            &manager,
            "20260619000008",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec!["build".to_string(), "test".to_string()],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: persisted to disk
        assert!(result.is_ok(), "should succeed: {:?}", result.err());
        let reloaded = manager
            .load("20260619000008")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            reloaded.skipped_steps,
            vec!["build".to_string(), "test".to_string()],
            "skipped_steps should be persisted"
        );
    }

    // --- config_changed flag ---

    #[test]
    fn test_update_session_settings_config_changed_false_for_skip_only_edit() {
        // Given: a Planned session whose config_path is already None (builtin)
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session = make_session("20260619000009", &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: only skipped_steps change, config_path stays None (same auto-resolved result)
        let (_, config_changed) = update_session_settings(
            &manager,
            "20260619000009",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec!["lint".to_string()],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: config did not change — no regen needed
        assert!(
            !config_changed,
            "config_changed should be false when only skipped_steps differ"
        );
    }

    #[test]
    fn test_update_session_settings_config_changed_true_when_explicit_path_given() {
        // Given: a Planned session with no explicit config (uses repo local cruise.yaml)
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        // Second config file to switch to
        let alt_config = tmp.path().join("alt.yaml");
        fs::write(
            &alt_config,
            "command: [local]\nsteps:\n  s:\n    command: echo alt",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session = make_session("20260619000010", &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: an explicit config path is provided (different from the previously resolved one)
        let (_, config_changed) = update_session_settings(
            &manager,
            "20260619000010",
            SessionSettingsUpdate {
                config_path: Some(alt_config.to_string_lossy().into_owned()),
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: config path changed — caller should regenerate plan
        assert!(
            config_changed,
            "config_changed should be true when config path switches"
        );
    }

    // --- Session-local config.yaml for builtin ---

    #[test]
    fn test_update_session_settings_preserves_pinned_builtin_config() {
        // Given: a session explicitly pinned to the builtin config and a local
        // config that would otherwise win automatic discovery
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000012", &repo);
        session.config = crate::session_config::SessionConfigRef::BuiltinSnapshot;
        let builtin_config = crate::resolver::resolve_workflow_config(
            crate::config::BUILTIN_CONFIG_YAML,
            &crate::resolver::ConfigSource::Builtin,
            &repo,
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        manager
            .create_with_config(&session, &builtin_config)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: the CLI edits only skipped steps (it omits config_path)
        let (_, config_changed) = update_session_settings(
            &manager,
            "20260619000012",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec!["lint".to_string()],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the builtin pin is retained rather than replaced by the local file
        let reloaded = manager
            .load("20260619000012")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            reloaded.config,
            crate::session_config::SessionConfigRef::BuiltinSnapshot
        );
        assert!(!config_changed);
        let saved_config = fs::read_to_string(
            manager
                .sessions_dir()
                .join("20260619000012")
                .join("config.yaml"),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let expected_config = crate::resolver::resolve_workflow_config(
            crate::config::BUILTIN_CONFIG_YAML,
            &crate::resolver::ConfigSource::Builtin,
            &repo,
        )
        .and_then(|config| crate::session_config::serialize_resolved_config(&config))
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(saved_config, expected_config);
    }

    #[test]
    fn test_update_session_settings_writes_session_config_yaml_for_builtin_config() {
        // Given: a Planned session using the builtin config (config_path = None)
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session = make_session("20260619000011", &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: explicitly select the builtin snapshot
        let result = update_session_settings(
            &manager,
            "20260619000011",
            SessionSettingsUpdate {
                config_path: Some(crate::new_session_history::BUILTIN_CONFIG_KEY.to_string()),
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: a session-local config.yaml is written next to session.json
        assert!(result.is_ok(), "should succeed: {:?}", result.err());
        let session_dir = manager.sessions_dir().join("20260619000011");
        assert!(
            session_dir.join("config.yaml").exists(),
            "config.yaml should be written for builtin/auto-resolved config"
        );
    }

    #[test]
    fn test_update_repo_session_reuses_resolved_snapshot_after_clone_cleanup() {
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("removed-repo");
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000013", &repo);
        session.repo = Some("owner/repository".to_string());
        session.config = crate::session_config::SessionConfigRef::RepoSnapshot {
            relative_path: std::path::PathBuf::from(".cruise/review.yaml"),
        };
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            manager
                .sessions_dir()
                .join("20260619000013")
                .join("config.yaml"),
            "command: [echo]\nsteps:\n  s:\n    prompt: preserved\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let result = update_session_settings(
            &manager,
            "20260619000013",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        assert!(result.is_ok(), "snapshot edit failed: {result:?}");
        let snapshot = fs::read_to_string(
            manager
                .sessions_dir()
                .join("20260619000013")
                .join("config.yaml"),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(snapshot.contains("prompt: preserved"));
    }

    #[test]
    fn test_update_auto_selected_config_keeps_live_file_reference() {
        // Given: an auto-selected config whose prompt_file is relative to it.
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join("prompts")).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(repo.join("prompts/implement.md"), "Implement from a file\n")
            .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    prompt_file: prompts/implement.md\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session = make_session("20260619000019", &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: Auto is explicitly selected again.
        let (_, config_changed) = update_session_settings(
            &manager,
            "20260619000019",
            SessionSettingsUpdate {
                config_path: Some(String::new()),
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: Auto resolves to the ordinary file and does not create a snapshot.
        assert!(!config_changed);
        let reloaded = manager
            .load("20260619000019")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            reloaded.config,
            crate::session_config::SessionConfigRef::File {
                path: repo.join("cruise.yaml"),
            }
        );
        assert!(
            !manager
                .sessions_dir()
                .join("20260619000019")
                .join("config.yaml")
                .exists()
        );
        let config = manager
            .load_config(&reloaded)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            config.steps["s"].prompt.as_deref(),
            Some("Implement from a file\n")
        );
    }

    #[test]
    fn test_update_repo_session_preserves_resolved_prompt_snapshot() {
        // Given: a repo-backed session whose temporary clone has already gone away
        // and whose snapshot contains an inlined prompt_file.
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000018", &repo);
        session.phase = SessionPhase::AwaitingApproval;
        session.repo = Some("owner/repository".to_string());
        session.config = crate::session_config::SessionConfigRef::RepoSnapshot {
            relative_path: std::path::PathBuf::from(".cruise/review.yaml"),
        };
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            manager
                .sessions_dir()
                .join("20260619000018")
                .join("config.yaml"),
            "command: [local]\nsteps:\n  s:\n    prompt: inlined prompt\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::remove_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        // When: settings are edited without selecting a replacement config.
        let result = update_session_settings(
            &manager,
            "20260619000018",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: the resolved snapshot remains usable after the clone is removed.
        assert!(result.is_ok(), "repo snapshot edit failed: {result:?}");
        let snapshot = fs::read_to_string(
            manager
                .sessions_dir()
                .join("20260619000018")
                .join("config.yaml"),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(snapshot.contains("prompt: inlined prompt"));
        assert!(!snapshot.contains("prompt_file"));
    }

    // --- Failed / Suspended phase gating (new) ---

    #[test]
    fn test_failed_phase_succeeds_for_skip_only() {
        // Given: a Failed session with a config file in its base dir
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000012", &repo);
        session.phase = SessionPhase::Failed("build error".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: skip-only update (current_step=None)
        let result = update_session_settings(
            &manager,
            "20260619000012",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec!["s".to_string()],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: Failed should be allowed for skip-only edits
        assert!(
            result.is_ok(),
            "Failed phase should allow skip-only edits: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_suspended_phase_succeeds_for_skip_only() {
        // Given: a Suspended session with a config file in its base dir
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000013", &repo);
        session.phase = SessionPhase::Suspended;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: skip-only update (current_step=None)
        let result = update_session_settings(
            &manager,
            "20260619000013",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec!["s".to_string()],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: Suspended should be allowed for skip-only edits
        assert!(
            result.is_ok(),
            "Suspended phase should allow skip-only edits: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_failed_phase_updates_current_step() {
        // Given: a Failed session with current_step=None, config with step "s"
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000014", &repo);
        session.phase = SessionPhase::Failed("step s failed".to_string());
        session.current_step = None;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: set current_step to "s" (an existing step in the config)
        let result = update_session_settings(
            &manager,
            "20260619000014",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Set("s".to_string()),
            },
        );

        // Then: should succeed and persist the new current_step
        assert!(
            result.is_ok(),
            "Failed phase should allow current_step update: {:?}",
            result.err()
        );
        let reloaded = manager
            .load("20260619000014")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            reloaded.current_step,
            Some("s".to_string()),
            "current_step should be set to 's'"
        );
    }

    #[test]
    fn test_suspended_phase_clears_current_step() {
        // Given: a Suspended session with current_step already set
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000015", &repo);
        session.phase = SessionPhase::Suspended;
        session.current_step = Some("s".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: clear current_step (Some(None) = clear)
        let result = update_session_settings(
            &manager,
            "20260619000015",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Clear,
            },
        );

        // Then: should succeed and current_step becomes None (from beginning)
        assert!(
            result.is_ok(),
            "Suspended phase should allow current_step clear: {:?}",
            result.err()
        );
        let reloaded = manager
            .load("20260619000015")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            reloaded.current_step, None,
            "current_step should be cleared"
        );
    }

    #[test]
    fn test_failed_phase_rejects_config_swap() {
        // Given: a Failed session with its original config
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let alt_config = tmp.path().join("alt.yaml");
        fs::write(
            &alt_config,
            "command: [local]\nsteps:\n  s:\n    command: echo alt",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000016", &repo);
        session.phase = SessionPhase::Failed("err".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: try to swap to a different config (not allowed for Failed/Suspended)
        let result = update_session_settings(
            &manager,
            "20260619000016",
            SessionSettingsUpdate {
                config_path: Some(alt_config.to_string_lossy().into_owned()),
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Unchanged,
            },
        );

        // Then: must reject — config swap breaks plan/state consistency
        assert!(
            result.is_err(),
            "Failed phase should reject config_path change"
        );
    }

    #[test]
    fn test_failed_phase_rejects_invalid_current_step() {
        // Given: a Failed session with config that only has step "s"
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619000017", &repo);
        session.phase = SessionPhase::Failed("err".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: set current_step to a step that doesn't exist in the compiled workflow
        let result = update_session_settings(
            &manager,
            "20260619000017",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Set("nonexistent_step".to_string()),
            },
        );

        // Then: must reject — step "nonexistent_step" is not in the workflow
        assert!(
            result.is_err(),
            "Should reject current_step that doesn't exist in workflow"
        );
    }

    #[test]
    fn test_planned_phase_rejects_current_step_update() {
        // Given: a Planned session
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        write_minimal_config(&repo);

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session = make_session("20260619000018", &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: try to set current_step on a Planned session (Planned never has a running step)
        let result = update_session_settings(
            &manager,
            "20260619000018",
            SessionSettingsUpdate {
                config_path: None,
                skipped_steps: vec![],
                current_step_update: CurrentStepUpdate::Set("s".to_string()),
            },
        );

        // Then: must reject — current_step editing is only valid for Failed/Suspended
        assert!(
            result.is_err(),
            "Planned phase should reject current_step update"
        );
    }
}
