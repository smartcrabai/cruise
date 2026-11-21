use super::*;
use std::path::Path;

fn directory_request(base: &Path, config: Option<&str>, input: &str) -> Result<ExecStartRequest> {
    ExecStartRequest::for_directory(
        base,
        config,
        input,
        std::collections::BTreeMap::default(),
        vec![],
    )
}

#[test]
fn for_directory_rejects_invalid_input_and_directory() {
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("create temp dir: {e}"));
    for input in ["", "  \n "] {
        let error = directory_request(temp.path(), Some("__builtin__"), input)
            .err()
            .unwrap_or_else(|| panic!("expected request rejection"));
        assert!(
            error.to_string().contains("exec input cannot be empty"),
            "{error}"
        );
    }
    let error = directory_request(temp.path(), Some("__builtin__"), "#12")
        .err()
        .unwrap_or_else(|| panic!("expected request rejection"));
    assert!(
        error
            .to_string()
            .contains("exec does not resolve #N issue input"),
        "{error}"
    );
    for base in [Path::new("relative"), temp.path().join("missing").as_path()] {
        let error = directory_request(base, Some("__builtin__"), "task")
            .err()
            .unwrap_or_else(|| panic!("expected request rejection"));
        assert!(
            error
                .to_string()
                .contains("exec requires an existing absolute working directory"),
            "{error}"
        );
    }
}

#[test]
fn for_directory_resolves_file_and_builtin_config() {
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("create temp dir: {e}"));
    let config = temp.path().join("workflow.yml");
    let yaml = "command: [echo]\nsteps:\n  build: {command: 'true'}\n";
    std::fs::write(&config, yaml).unwrap_or_else(|e| panic!("write config: {e}"));
    let request = directory_request(temp.path(), Some("workflow.yml"), "  task  ")
        .unwrap_or_else(|e| panic!("resolve request: {e}"));
    assert_eq!(request.input, "task");
    assert_eq!(request.config_yaml, yaml);
    assert!(matches!(request.config_source, SessionConfigRef::File { path } if path == config));
    assert_eq!(request.context.cwd, temp.path());
    let request = directory_request(temp.path(), Some("__builtin__"), "task")
        .unwrap_or_else(|e| panic!("resolve request: {e}"));
    assert!(matches!(
        request.config_source,
        SessionConfigRef::BuiltinSnapshot { .. }
    ));
    assert_ne!(request.config_yaml, "");
}

#[test]
fn exec_persists_only_applicable_skips() {
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let config = temp.path().join("workflow.yml");
    std::fs::write(&config, "command: [echo]\nflags:\n  extra: {}\nsteps:\n  ordinary: {command: 'echo ordinary'}\n  flagged: {flag: extra, command: 'echo flagged'}\n")
        .unwrap_or_else(|e| panic!("write config: {e}"));
    let manager = crate::session::SessionManager::new(temp.path().join("data"));
    let app = CruiseApplication::new(manager.clone());
    let mut request = directory_request(temp.path(), Some("workflow.yml"), "task")
        .unwrap_or_else(|e| panic!("request: {e}"));
    request.skipped_steps = vec!["ordinary".into(), "flagged".into()];
    let created = app
        .create_exec_session(&request)
        .unwrap_or_else(|e| panic!("create: {e}"));
    let loaded = manager
        .load(&created.id)
        .unwrap_or_else(|e| panic!("load: {e}"));
    assert_eq!(loaded.skipped_steps, vec!["ordinary"]);
}

#[test]
fn exec_rejects_repository_snapshot_config() {
    let source = SessionConfigRef::RepoSnapshot {
        relative_path: "workflow.yml".into(),
    };
    let error = resolver_source(&source)
        .err()
        .unwrap_or_else(|| panic!("expected request rejection"));
    assert!(
        error
            .to_string()
            .contains("exec cannot use a repository snapshot config"),
        "{error}"
    );
}
