//! The workflow's `retry:` block reaches the executor.
//!
//! The executor takes its work through `PromptRun`, which carries no policy
//! field, so resolving a workflow config publishes the policy process-wide
//! (`cruise::retry::active_policy`). This lives in its own test binary, as a
//! single test, because that publication is process state: any other test
//! resolving a config would overwrite it.

use cruise::config::WorkflowConfig;
use cruise::test_support::{EnvGuard, lock_process};
use cruise::workflow_call::resolve_workflow_calls_with_defaults;
use cruise::workflow_defaults::WorkflowDefaults;
use tempfile::TempDir;

const RETRY_YAML: &str = r#"
sdk: jcode
retry:
  base_delay_ms: 250
  fallback_chains:
    "anthropic/claude-opus-4-6":
      - openai/gpt-5.5
steps:
  s1:
    prompt: hi
"#;

// `max_delay_ms` below `base_delay_ms` can never serve a delay.
const INVALID_RETRY_YAML: &str = "
sdk: jcode
retry:
  base_delay_ms: 5000
  max_delay_ms: 100
steps:
  s1:
    prompt: hi
";

const PLAIN_YAML: &str = "sdk: jcode\nsteps:\n  s1:\n    prompt: hi\n";

fn resolve(yaml: &str) -> Result<WorkflowConfig, cruise::error::CruiseError> {
    let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
    resolve_workflow_calls_with_defaults(config, ".", &WorkflowDefaults::default())
}

fn resolve_ok(yaml: &str) {
    resolve(yaml).unwrap_or_else(|e| panic!("{e:?}"));
}

#[test]
fn resolving_a_workflow_publishes_validates_and_clears_its_retry_policy() {
    let _lock = lock_process();
    let config_home = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().as_os_str());
    let _model = EnvGuard::remove("CRUISE_MODEL");
    let _plan_model = EnvGuard::remove("CRUISE_PLAN_MODEL");

    resolve_ok(RETRY_YAML);
    assert!(
        cruise::retry::active_policy().is_some(),
        "declared policy not published"
    );

    // An unusable policy is rejected before it can become the active one, so
    // the running workflow keeps the policy it was resolved with.
    let Err(err) = resolve(INVALID_RETRY_YAML) else {
        panic!("expected an unusable retry policy to be rejected");
    };
    assert!(
        err.to_string().contains("retry.max_delay_ms"),
        "unexpected error: {err}"
    );
    assert!(
        cruise::retry::active_policy().is_some(),
        "a rejected config must not clear the published policy"
    );

    // A workflow without `retry:` returns the process to the historical
    // same-model rate-limit behavior.
    resolve_ok(PLAIN_YAML);
    assert!(cruise::retry::active_policy().is_none());
}
