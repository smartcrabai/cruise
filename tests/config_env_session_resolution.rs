use cruise::config::resolve_effective_max_retries;
use cruise::session::{SessionManager, SessionState};
use cruise::session_config::SessionConfigRef;

#[test]
fn file_session_reresolves_environment_but_snapshot_stays_frozen()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let workflow = temp.path().join("workflow.yaml");
    std::fs::write(
        &workflow,
        "command: [echo]\nmax_retries: 5\nsteps:\n  main:\n    command: echo main\n",
    )?;
    let manager = SessionManager::new(temp.path().join("data"));
    let file = SessionState::new(
        "file".into(),
        temp.path().into(),
        SessionConfigRef::File { path: workflow },
        "test".into(),
    );
    let snapshot = SessionState::new(
        "snapshot".into(),
        temp.path().into(),
        SessionConfigRef::InlineSnapshot,
        "test".into(),
    );
    // This integration-test binary has only one test, so no other thread mutates its environment.
    for name in cruise::config_env::all_override_names() {
        unsafe { std::env::remove_var(name) };
    }
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", temp.path().join("config"));
        std::env::set_var("CRUISE_MAX_RETRIES", "7");
    }
    let resolved = manager.load_config(&file)?;
    assert_eq!(resolved.max_retries, Some(7));
    assert_eq!(resolve_effective_max_retries(None, &resolved), 7);
    assert_eq!(resolve_effective_max_retries(Some(2), &resolved), 2);
    manager.create_with_config(&snapshot, &resolved)?;
    unsafe { std::env::set_var("CRUISE_MAX_RETRIES", "9") };
    assert_eq!(manager.load_config(&file)?.max_retries, Some(9));
    assert_eq!(manager.load_config(&snapshot)?.max_retries, Some(7));
    Ok(())
}
