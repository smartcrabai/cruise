use cruise::application::{CruiseApplication, OperationKind};
use cruise::error::CruiseError;
use cruise::instruction_inbox::Origin;
use cruise::session::{ManagedChildOwner, SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use tempfile::TempDir;

fn session(root: &std::path::Path, id: &str, phase: SessionPhase, managed: bool) -> SessionState {
    let mut state = SessionState::new(
        id.into(),
        root.into(),
        SessionConfigRef::InlineSnapshot,
        "input".into(),
    );
    state.phase = phase;
    if managed {
        state.managed_child = Some(ManagedChildOwner {
            goal_id: "goal".into(),
            task_id: "task".into(),
            attempt: 1,
            revision: 1,
            base_sha: "base".into(),
        });
    }
    state
}

#[test]
fn managed_children_are_excluded_from_run_all_without_excluding_ordinary_sessions() {
    let dir = TempDir::new().unwrap();
    let manager = SessionManager::new(dir.path().into());
    for (id, phase, managed) in [
        ("ordinary-planned", SessionPhase::Planned, false),
        ("ordinary-suspended", SessionPhase::Suspended, false),
        ("owned-planned", SessionPhase::Planned, true),
        ("owned-suspended", SessionPhase::Suspended, true),
    ] {
        manager
            .create(&session(dir.path(), id, phase, managed))
            .unwrap();
    }
    let app = CruiseApplication::new(manager);
    let ids: Vec<_> = app
        .run_all_candidates()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, vec!["ordinary-planned", "ordinary-suspended"]);
}

#[test]
fn ordinary_mutation_boundaries_reject_owned_children_and_preserve_state() {
    let dir = TempDir::new().unwrap();
    let manager = SessionManager::new(dir.path().into());
    let child = session(dir.path(), "owned", SessionPhase::Suspended, true);
    manager.create(&child).unwrap();
    let before = manager.load("owned").unwrap();
    let app = CruiseApplication::new(manager.clone());
    assert!(matches!(
        app.reset_to_planned("owned"),
        Err(CruiseError::Other(_))
    ));
    assert!(matches!(
        app.delete_session("owned"),
        Err(CruiseError::Other(_))
    ));
    assert!(matches!(
        app.tell("owned", "change behavior".into(), Origin::Cli),
        Err(CruiseError::Other(_))
    ));
    assert!(matches!(
        app.runtime().try_begin("owned", OperationKind::Run),
        Err(CruiseError::Other(_))
    ));
    assert!(!app.cancel_session("owned"));
    assert!(!app.pause_session("owned"));
    assert!(app.capabilities(&before).is_empty());
    assert_eq!(manager.load("owned").unwrap(), before);

    let ordinary = session(dir.path(), "ordinary", SessionPhase::Suspended, false);
    manager.create(&ordinary).unwrap();
    assert_eq!(
        app.reset_to_planned("ordinary").unwrap().phase,
        SessionPhase::Planned
    );
    app.delete_session("ordinary").unwrap();
    assert!(!manager.sessions_dir().join("ordinary").exists());
}

#[test]
fn reservation_marker_blocks_ordinary_create_delete_and_claim_even_without_state() {
    let dir = TempDir::new().unwrap();
    let manager = SessionManager::new(dir.path().into());
    let reserved = manager.sessions_dir().join("reserved");
    std::fs::create_dir_all(&reserved).unwrap();
    let marker = reserved.join("manager-owner.json");
    std::fs::write(&marker, b"reservation").unwrap();
    let app = CruiseApplication::new(manager.clone());
    assert!(matches!(
        manager.create(&session(
            dir.path(),
            "reserved",
            SessionPhase::Planned,
            false
        )),
        Err(CruiseError::Other(_))
    ));
    assert!(matches!(
        manager.delete("reserved"),
        Err(CruiseError::Other(_))
    ));
    assert!(matches!(
        app.runtime().try_begin("reserved", OperationKind::Run),
        Err(CruiseError::Other(_))
    ));
    assert_eq!(std::fs::read(marker).unwrap(), b"reservation");
    assert!(!reserved.join("state.json").exists());
}
