use cruise::config::WorkflowConfig;
use cruise::retry::active_policy;
use cruise::workflow;
use cruise::workflow_call::resolve_workflow_calls;

fn resolve(yaml: &str) -> WorkflowConfig {
    let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
    resolve_workflow_calls(config, ".").unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn resolving_array_models_uses_primaries_and_builds_the_fallback_chain() {
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
