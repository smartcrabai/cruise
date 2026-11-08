use std::path::Path;

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};

/// Run the full exec preflight on a candidate YAML string.
///
/// # Errors
///
/// Returns the first parse, reference, validation, compile or graph error.
pub fn validate_generated_yaml(yaml: &str, base_dir: &Path) -> Result<()> {
    let config = WorkflowConfig::from_yaml(yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    let config = crate::workflow_call::resolve_workflow_calls_without_app_model_defaults(
        config,
        base_dir,
        &crate::workflow_defaults::WorkflowDefaults::default(),
    )?;
    crate::config::validate_config(&config)?;
    let max_retries = crate::config::resolve_effective_max_retries(None, &config);
    crate::config::validate_group_retry_budget(&config, max_retries)?;
    let compiled = crate::workflow::compile(config)?;
    let graph = crate::graph::build_graph(&compiled, max_retries)?;
    crate::graph::validation::validate_workflow(&compiled, &graph, &graph.start, &[])?;
    Ok(())
}
