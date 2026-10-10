use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::resolver::ConfigSource;

/// The durable configuration reference stored in a session's `state.json`.
///
/// A [`File`] remains a live reference to the source file. The other variants
/// deliberately refer to the session-owned `config.yaml` snapshot and never
/// fall back to a newly discovered configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionConfigRef {
    /// A live, absolute path to a regular workflow file.
    File { path: PathBuf },
    /// The resolved built-in workflow captured for this session.
    BuiltinSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A resolved workflow originating in a temporary repository clone.
    RepoSnapshot { relative_path: PathBuf },
    /// A workflow supplied directly as YAML by a caller.
    InlineSnapshot,
}

impl SessionConfigRef {
    /// Convert a resolver result into the durable reference for a session.
    ///
    /// When `clone_root` is supplied, a source path under that root becomes a
    /// repository snapshot and stores only its clone-relative origin.
    ///
    /// # Errors
    ///
    /// Returns an error when a relative source path cannot be made absolute or
    /// when a clone-relative path cannot be represented.
    pub fn from_source(source: &ConfigSource, clone_root: Option<&Path>) -> Result<Self> {
        match source {
            ConfigSource::Builtin => Ok(Self::BuiltinSnapshot {
                name: Some(crate::builtin_workflows::DEFAULT_BUILTIN_NAME.to_string()),
            }),
            ConfigSource::NamedBuiltin(name) => Ok(Self::BuiltinSnapshot {
                name: Some((*name).to_string()),
            }),
            ConfigSource::Explicit(path)
            | ConfigSource::EnvVar(path)
            | ConfigSource::Local(path)
            | ConfigSource::UserDir(path) => {
                let path = absolute_path(path)?;
                if let Some(clone_root) = clone_root {
                    let clone_root = absolute_path(clone_root)?;
                    if path.starts_with(&clone_root) {
                        let relative_path = path.strip_prefix(&clone_root).map_err(|error| {
                            CruiseError::Other(format!(
                                "failed to derive repository-relative config path for {}: {error}",
                                path.display()
                            ))
                        })?;
                        return Ok(Self::RepoSnapshot {
                            relative_path: relative_path.to_path_buf(),
                        });
                    }
                }
                Ok(Self::File { path })
            }
        }
    }

    /// Construct the reference for caller-provided YAML.
    #[must_use]
    pub const fn inline_snapshot() -> Self {
        Self::InlineSnapshot
    }

    /// Human-readable label for CLI, TUI, and GUI display boundaries.
    #[must_use]
    pub fn display_label(&self) -> String {
        match self {
            Self::File { path } => path.display().to_string(),
            Self::BuiltinSnapshot { .. } => "Built-in snapshot".to_string(),
            Self::RepoSnapshot { relative_path } => {
                format!("Repo snapshot: {}", relative_path.display())
            }
            Self::InlineSnapshot => "Inline snapshot".to_string(),
        }
    }

    /// Return the only reference kind that should be monitored for live edits.
    #[must_use]
    pub fn live_path(&self) -> Option<&Path> {
        match self {
            Self::File { path } => Some(path),
            Self::BuiltinSnapshot { .. } | Self::RepoSnapshot { .. } | Self::InlineSnapshot => None,
        }
    }

    /// Return the value used by a config-selection form, when one exists.
    ///
    /// Snapshots intentionally return `None`: omitting a selection preserves
    /// the current snapshot, while the empty string remains the explicit Auto
    /// operation at the edit boundary.
    #[must_use]
    pub fn selection_value(&self) -> Option<String> {
        match self {
            Self::File { path } => Some(path.to_string_lossy().into_owned()),
            Self::BuiltinSnapshot { name } => Some(builtin_selector(name.as_deref())),
            Self::RepoSnapshot { .. } | Self::InlineSnapshot => None,
        }
    }

    /// Return whether two refs select the same config. Built-in snapshots
    /// compare by selector so legacy unnamed snapshots equal the default.
    #[must_use]
    pub fn same_selection(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::BuiltinSnapshot { .. }, Self::BuiltinSnapshot { .. }) => {
                self.selection_value() == other.selection_value()
            }
            _ => self == other,
        }
    }

    /// Return the stable identity used by history and planning conversations.
    #[must_use]
    pub fn stable_identity(&self, repo: Option<&str>, session_id: &str) -> String {
        match self {
            Self::File { path } => {
                crate::new_session_history::resolved_config_key_for_session(path)
            }
            Self::BuiltinSnapshot { name } => builtin_selector(name.as_deref()),
            Self::RepoSnapshot { relative_path } => {
                let repo = repo
                    .filter(|value| !value.is_empty())
                    .unwrap_or("repo")
                    .to_ascii_lowercase();
                format!("{repo}::{}", relative_path.to_string_lossy())
            }
            Self::InlineSnapshot => format!("inline:{session_id}"),
        }
    }

    /// Whether this reference reads the session-owned snapshot.
    #[must_use]
    pub const fn is_snapshot(&self) -> bool {
        !matches!(self, Self::File { .. })
    }

    /// A concise kind label used in snapshot diagnostics.
    #[must_use]
    pub const fn kind_label(&self) -> &'static str {
        match self {
            Self::File { .. } => "file",
            Self::BuiltinSnapshot { .. } => "builtin snapshot",
            Self::RepoSnapshot { .. } => "repo snapshot",
            Self::InlineSnapshot => "inline snapshot",
        }
    }
}

/// Make a path absolute without canonicalizing it away from a live reference.
///
/// # Errors
///
/// Returns an error when the process working directory cannot be read for a
/// relative path.
pub fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

/// Serialize resolved configuration for a session-owned snapshot.
///
/// # Errors
///
/// Returns an error when the resolved workflow cannot be represented as YAML.
pub fn serialize_resolved_config(config: &WorkflowConfig) -> Result<String> {
    serde_yaml::to_string(config).map_err(|error| {
        CruiseError::Other(format!(
            "failed to serialize resolved workflow config for session: {error}"
        ))
    })
}

/// Return the canonical snapshot path for a session.
#[must_use]
pub fn snapshot_path(sessions_dir: &Path, session_id: &str) -> PathBuf {
    sessions_dir.join(session_id).join("config.yaml")
}

/// Atomically replace a session-owned config snapshot.
///
/// # Errors
///
/// Returns an I/O error when the parent, temporary file, or replacement rename
/// cannot be written.
pub fn write_snapshot_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().map_or_else(
        || "config.yaml".to_string(),
        |value| value.to_string_lossy().into_owned(),
    );
    let tmp = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    if let Err(error) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

/// Build the common live-file reloader used by CLI and application execution.
#[must_use]
pub fn config_reloader_for_reference(
    reference: &SessionConfigRef,
    effective_max_retries: usize,
) -> Option<Box<dyn Fn() -> Result<Option<crate::engine::ReloadedWorkflow>> + Send + Sync>> {
    let path = reference.live_path()?.to_path_buf();
    let last_mtime = Mutex::new(
        std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok(),
    );
    Some(Box::new(move || {
        let current_mtime = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok();
        let mut last = last_mtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current_mtime == *last {
            return Ok(None);
        }
        let config = crate::workflow_call::resolve_workflow_calls_from_path(&path)?;
        crate::config::validate_config(&config)?;
        crate::config::validate_group_retry_budget(&config, effective_max_retries)?;
        let retry_policy = crate::retry::policy_for_config(config.retry.clone());
        let compiled = crate::workflow::compile(config)?;
        *last = current_mtime;
        Ok(Some(crate::engine::ReloadedWorkflow {
            compiled,
            retry_policy,
        }))
    }))
}

/// Selector value for a built-in snapshot: the legacy sentinel for the default.
fn builtin_selector(name: Option<&str>) -> String {
    match name {
        None | Some(crate::builtin_workflows::DEFAULT_BUILTIN_NAME) => {
            crate::new_session_history::BUILTIN_CONFIG_KEY.to_string()
        }
        Some(name) => format!("{}{name}", crate::resolver::BUILTIN_SELECTOR_PREFIX),
    }
}

#[cfg(test)]
mod builtin_name_tests {
    use super::*;

    #[test]
    fn builtin_snapshot_name_round_trips() {
        let named = SessionConfigRef::BuiltinSnapshot {
            name: Some("simple".to_string()),
        };
        let json = serde_json::to_value(&named).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            json,
            serde_json::json!({"kind": "builtin_snapshot", "name": "simple"})
        );
        let back: SessionConfigRef =
            serde_json::from_value(json).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(back, named);
    }

    #[test]
    fn unnamed_builtin_snapshot_omits_name_when_serialized() {
        let json = serde_json::to_value(SessionConfigRef::BuiltinSnapshot { name: None })
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(json, serde_json::json!({"kind": "builtin_snapshot"}));
    }

    #[test]
    fn legacy_builtin_snapshot_without_name_loads() {
        let loaded: SessionConfigRef = serde_json::from_str(r#"{"kind":"builtin_snapshot"}"#)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(loaded, SessionConfigRef::BuiltinSnapshot { name: None });
        assert_eq!(
            loaded.selection_value().as_deref(),
            Some(crate::new_session_history::BUILTIN_CONFIG_KEY)
        );
        assert_eq!(
            loaded.stable_identity(None, "sid"),
            crate::new_session_history::BUILTIN_CONFIG_KEY
        );
    }

    #[test]
    fn named_builtin_snapshot_uses_builtin_selector_identity() {
        let simple = SessionConfigRef::BuiltinSnapshot {
            name: Some("simple".to_string()),
        };
        assert_eq!(simple.selection_value().as_deref(), Some("builtin:simple"));
        assert_eq!(simple.stable_identity(None, "sid"), "builtin:simple");
        assert!(simple.is_snapshot());
        assert!(simple.live_path().is_none());
    }

    #[test]
    fn default_builtin_snapshot_keeps_legacy_identity() {
        let default = SessionConfigRef::BuiltinSnapshot {
            name: Some("default".to_string()),
        };
        assert_eq!(
            default.selection_value().as_deref(),
            Some(crate::new_session_history::BUILTIN_CONFIG_KEY)
        );
        assert_eq!(
            default.stable_identity(None, "sid"),
            crate::new_session_history::BUILTIN_CONFIG_KEY
        );
    }

    #[test]
    fn new_default_builtin_session_records_default_name() {
        let reference = SessionConfigRef::from_source(&ConfigSource::Builtin, None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            reference,
            SessionConfigRef::BuiltinSnapshot {
                name: Some("default".to_string())
            }
        );
    }
}
