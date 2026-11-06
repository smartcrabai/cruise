use std::path::PathBuf;

use cruise::app_config::AppConfig;
use cruise::session::{SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::{Value, json};

fn session() -> SessionState {
    SessionState::new(
        "test-session".into(),
        PathBuf::from("/tmp/repo"),
        SessionConfigRef::InlineSnapshot,
        "test input".into(),
    )
}

#[test]
fn legacy_session_defaults_counter_to_zero_and_omits_it_when_serialized()
-> Result<(), Box<dyn std::error::Error>> {
    let mut legacy = serde_json::to_value(session())?;
    legacy
        .as_object_mut()
        .ok_or("session must serialize to an object")?
        .remove("auto_requeue_count");
    let loaded: SessionState = serde_json::from_value(legacy)?;
    let serialized = serde_json::to_value(loaded)?;
    assert!(serialized.get("auto_requeue_count").is_none());
    Ok(())
}

#[test]
fn session_counter_round_trips_and_reset_clears_it() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = serde_json::to_value(session())?;
    value["auto_requeue_count"] = json!(3);
    let mut state: SessionState = serde_json::from_value(value)?;
    state.phase = SessionPhase::Failed("temporary failure".into());
    state.current_step = Some("build".into());
    let saved = serde_json::to_value(&state)?;
    assert_eq!(saved["auto_requeue_count"], 3);
    let mut state: SessionState = serde_json::from_value(saved)?;
    state.reset_to_planned();
    assert_eq!(state.phase, SessionPhase::Planned);
    assert_eq!(state.current_step, None);
    assert!(
        serde_json::to_value(state)?
            .get("auto_requeue_count")
            .is_none()
    );
    Ok(())
}

#[test]
fn app_config_defaults_and_accepts_both_names() -> Result<(), Box<dyn std::error::Error>> {
    let default = serde_json::to_value(AppConfig::default())?;
    assert_eq!(default["runAllAutoRequeueMaxAttempts"], 0);
    for key in [
        "runAllAutoRequeueMaxAttempts",
        "run_all_auto_requeue_max_attempts",
    ] {
        let mut input = serde_json::Map::new();
        input.insert(key.into(), json!(7));
        let config: AppConfig = serde_json::from_value(Value::Object(input))?;
        let output = serde_json::to_value(config)?;
        assert_eq!(output["runAllAutoRequeueMaxAttempts"], 7);
        assert!(output.get("run_all_auto_requeue_max_attempts").is_none());
    }
    Ok(())
}

#[test]
fn app_config_rejects_invalid_counter_values() {
    for value in [json!(-1), json!(4_294_967_296_u64), json!(1.5), json!("3")] {
        let input = json!({"runAllAutoRequeueMaxAttempts": value});
        assert!(serde_json::from_value::<AppConfig>(input).is_err());
    }
}
