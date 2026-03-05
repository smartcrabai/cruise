use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowState {
    pub config: WorkflowConfig,
    pub current: String,
}

impl WorkflowState {
    pub fn new(config: WorkflowConfig, current: String) -> Self {
        Self { config, current }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| CruiseError::StateError(e.to_string()))?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json).map_err(|e| CruiseError::StateError(e.to_string()))
    }

    pub fn cleanup(path: &Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}
