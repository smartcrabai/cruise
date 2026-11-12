//! Compatibility contract for sessions saved before execution stages existed.
use cruise::session::{SessionExecutionStage, SessionState};
use cruise::session_config::SessionConfigRef;

#[test]
fn old_failed_session_without_execution_stage_defaults_to_main() {
    let mut session = SessionState::new(
        "legacy".into(),
        "/tmp/repo".into(),
        SessionConfigRef::File {
            path: "/tmp/workflow.yaml".into(),
        },
        "legacy".into(),
    );
    session.phase = cruise::session::SessionPhase::Failed("old after-pr failure".into());
    session.current_step = Some("main-last".into());
    session.pr_url = Some("https://github.com/example/repo/pull/42".into());
    let mut value = serde_json::to_value(&session)
        .unwrap_or_else(|error| panic!("serialize legacy fixture: {error}"));
    value
        .as_object_mut()
        .unwrap_or_else(|| panic!("serialized session is not an object"))
        .remove("execution_stage");
    let loaded: SessionState = serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("load legacy session: {error}"));
    assert_eq!(loaded.execution_stage, SessionExecutionStage::Main);
    assert_eq!(loaded.current_step.as_deref(), Some("main-last"));
    assert_eq!(
        loaded.pr_url.as_deref(),
        Some("https://github.com/example/repo/pull/42")
    );
}
