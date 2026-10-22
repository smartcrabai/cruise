use std::path::Path;

use cruise::config::{ModelSpec, WorkflowConfig};
use cruise::retry::active_policy;
use cruise::test_support::{EnvGuard, ProcessLock, lock_process};
use cruise::workflow;
use cruise::workflow_call::resolve_workflow_calls_with_defaults;
use cruise::workflow_defaults::WorkflowDefaults;
use tempfile::TempDir;

struct ResolutionEnv {
    temp: TempDir,
    _guards: Vec<EnvGuard>,
    _lock: ProcessLock,
}

impl ResolutionEnv {
    fn new() -> Self {
        let lock = lock_process();
        let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let config_dir = temp.path().join("config");
        let guards = vec![
            EnvGuard::set("XDG_CONFIG_HOME", config_dir.as_os_str()),
            EnvGuard::remove("CRUISE_MODEL"),
            EnvGuard::remove("CRUISE_PLAN_MODEL"),
        ];
        Self {
            temp,
            _guards: guards,
            _lock: lock,
        }
    }

    fn write_app_config(&self, content: &str) {
        let path = self.temp.path().join("config/cruise/config.json");
        std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
            .unwrap_or_else(|error| panic!("create app config directory: {error}"));
        std::fs::write(path, content).unwrap_or_else(|error| panic!("write app config: {error}"));
    }
}

fn resolve(yaml: &str) -> WorkflowConfig {
    let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
    resolve_workflow_calls_with_defaults(config, ".", &WorkflowDefaults::default())
        .unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn resolving_array_models_uses_primaries_and_builds_the_fallback_chain() {
    let _env = ResolutionEnv::new();
    let config = resolve(
        r"
 sdk: jcode
 model:
   - anthropic/primary
   - openai/fallback
   - google/last-resort
 plan_model:
   - anthropic/planning
   - openai/planning-fallback
 steps:
   s1:
     prompt: hi
 ",
    );

    let compiled = workflow::compile(config.clone()).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(compiled.model.as_deref(), Some("anthropic/primary"));
    assert_eq!(compiled.plan_model.as_deref(), Some("anthropic/planning"));

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("array models must create a retry policy"));
    assert!(
        retry.model_fallback,
        "array models must enable model fallback"
    );
    assert_eq!(
        retry
            .fallback_chains
            .get("anthropic/primary")
            .map(Vec::as_slice),
        Some(
            [
                "openai/fallback".to_string(),
                "google/last-resort".to_string(),
            ]
            .as_slice()
        )
    );
    assert_eq!(
        retry
            .fallback_chains
            .get("anthropic/planning")
            .map(Vec::as_slice),
        Some(["openai/planning-fallback".to_string()].as_slice())
    );
    assert!(
        active_policy().is_some(),
        "resolved policy must be published"
    );
}

#[test]
fn explicit_retry_chain_takes_precedence_over_the_array_tail() {
    let _env = ResolutionEnv::new();
    let config = resolve(
        r"
 sdk: jcode
 model:
   - anthropic/primary
   - openai/array-fallback
 retry:
   base_delay_ms: 250
   fallback_chains:
     anthropic/primary:
       - google/explicit-fallback
 steps:
   s1:
     prompt: hi
 ",
    );

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("retry block must be preserved"));
    assert_eq!(retry.base_delay_ms, 250);
    assert_eq!(
        retry
            .fallback_chains
            .get("anthropic/primary")
            .map(Vec::as_slice),
        Some(["google/explicit-fallback".to_string()].as_slice())
    );
}

#[test]
fn app_config_model_lists_build_primary_and_fallbacks_for_exec_and_planning() {
    let env = ResolutionEnv::new();
    env.write_app_config(
        r#"{"model":["anthropic/app-primary","openai/app-fallback","google/app-last"],"plan_model":["google/plan-primary","openai/plan-fallback"]}"#,
    );
    let config = resolve(
        r"
 sdk: jcode
 steps:
   s1:
     prompt: hi
 ",
    );

    assert_eq!(
        config.model,
        Some(ModelSpec::Multiple(vec![
            "anthropic/app-primary".to_string(),
            "openai/app-fallback".to_string(),
            "google/app-last".to_string(),
        ]))
    );
    assert_eq!(
        config.plan_model,
        Some(ModelSpec::Multiple(vec![
            "google/plan-primary".to_string(),
            "openai/plan-fallback".to_string(),
        ]))
    );
    let compiled = workflow::compile(config.clone()).unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(compiled.model.as_deref(), Some("anthropic/app-primary"));
    assert_eq!(compiled.plan_model.as_deref(), Some("google/plan-primary"));

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("app model lists must create a retry policy"));
    assert!(retry.model_fallback);
    assert_eq!(
        retry
            .fallback_chains
            .get("anthropic/app-primary")
            .map(Vec::as_slice),
        Some(
            [
                "openai/app-fallback".to_string(),
                "google/app-last".to_string(),
            ]
            .as_slice()
        )
    );
    assert_eq!(
        retry
            .fallback_chains
            .get("google/plan-primary")
            .map(Vec::as_slice),
        Some(["openai/plan-fallback".to_string()].as_slice())
    );
}

#[test]
fn explicit_retry_chain_overrides_app_config_model_list_fallback() {
    let env = ResolutionEnv::new();
    env.write_app_config(r#"{"model":["anthropic/app-primary","openai/app-fallback"]}"#);
    let config = resolve(
        r"
 sdk: jcode
 retry:
   fallback_chains:
     anthropic/app-primary:
       - google/explicit-fallback
 steps:
   s1:
     prompt: hi
 ",
    );

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("explicit retry block must be preserved"));
    assert_eq!(
        retry
            .fallback_chains
            .get("anthropic/app-primary")
            .map(Vec::as_slice),
        Some(["google/explicit-fallback".to_string()].as_slice())
    );
}
