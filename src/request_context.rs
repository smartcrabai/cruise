//! Client-captured inputs carried with every daemon request.
//!
//! The daemon outlives the shells that talk to it, so inputs that differ
//! between clients (working directory and workflow override variables) travel
//! with each request instead of being read from the daemon process.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestContext {
    /// Absolute working directory of the requesting client.
    pub cwd: PathBuf,
    /// Client values of the `config_env::WORKFLOW_ENV_KEYS` variables; absent keys are unset.
    #[serde(default)]
    pub config_env: BTreeMap<String, String>,
}

impl RequestContext {
    /// Capture this process's working directory and workflow override variables.
    ///
    /// # Errors
    ///
    /// Returns an error when the current directory cannot be read.
    pub fn capture() -> std::io::Result<Self> {
        let cwd = std::env::current_dir()?;
        let config_env = crate::config_env::WORKFLOW_ENV_KEYS
            .iter()
            .filter_map(|key| {
                std::env::var(key.var)
                    .ok()
                    .map(|value| (key.var.to_owned(), value))
            })
            .collect();
        Ok(Self { cwd, config_env })
    }

    /// The client's raw value for one workflow override variable.
    #[must_use]
    pub fn config_env_var(&self, var: &str) -> Option<&str> {
        self.config_env.get(var).map(String::as_str)
    }
}
