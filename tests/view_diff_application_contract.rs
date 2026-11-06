//! Contract tests for exposing session branch diffs through the application.

use cruise::application::{CruiseApplication, SessionAction};
use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn view_diff_capability_depends_only_on_branch_and_is_first() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let app = CruiseApplication::new(SessionManager::new(temp.path().join("sessions")));
    for phase in [
        SessionPhase::Draft,
        SessionPhase::AwaitingInput,
        SessionPhase::AwaitingApproval,
        SessionPhase::Planned,
        SessionPhase::Running,
        SessionPhase::Completed,
        SessionPhase::Failed("x".into()),
        SessionPhase::Suspended,
    ] {
        let mut state = SessionState::new(
            "20260101000000000".into(),
            temp.path().to_path_buf(),
            SessionConfigRef::BuiltinSnapshot { name: None },
            "task".into(),
        );
        state.phase = phase.clone();
        assert!(
            !app.capabilities(&state).contains(&SessionAction::ViewDiff),
            "{phase:?}"
        );
        state.worktree_branch = Some("session-branch".into());
        let capabilities = app.capabilities(&state);
        assert_eq!(
            capabilities.first(),
            Some(&SessionAction::ViewDiff),
            "{phase:?}: {capabilities:?}"
        );
    }
    Ok(())
}

#[test]
fn diff_for_missing_session_fails() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let app = CruiseApplication::new(SessionManager::new(temp.path().join("sessions")));
    assert!(app.session_branch_diff("missing-id").is_err());
    Ok(())
}

#[test]
fn view_diff_action_serializes_as_camel_case() -> Result<()> {
    assert_eq!(
        serde_json::to_string(&SessionAction::ViewDiff)?,
        "\"viewDiff\""
    );
    assert_eq!(
        serde_json::from_str::<SessionAction>("\"viewDiff\"")?,
        SessionAction::ViewDiff
    );
    Ok(())
}
