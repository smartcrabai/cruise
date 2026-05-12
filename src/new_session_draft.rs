use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};
use crate::session::current_iso8601;

/// Persistent draft of the New Session form.
///
/// Stored at `~/.cruise/new_session_draft.json`. Missing file means no draft.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NewSessionDraft {
    /// User-typed task description.
    #[serde(default)]
    pub input: String,
    /// Explicit config path selected in the dropdown, or `None` for auto mode.
    #[serde(default)]
    pub requested_config_path: Option<String>,
    /// Working directory for the session.
    #[serde(default)]
    pub working_dir: String,
    /// Step names the user explicitly chose to skip.
    #[serde(default)]
    pub skipped_steps: Vec<String>,
    /// ISO 8601 timestamp of the last save.
    #[serde(default)]
    pub updated_at: String,
}

impl NewSessionDraft {
    /// Return the canonical path to the draft file: `~/.cruise/new_session_draft.json`.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined.
    fn draft_path() -> Result<PathBuf> {
        crate::session::get_cruise_home().map(|h| h.join("new_session_draft.json"))
    }

    /// Load the draft from the canonical [`Self::draft_path`].
    ///
    /// Returns `Ok(None)` if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load() -> Result<Option<Self>> {
        let path = Self::draft_path()?;
        Self::load_from(&path)
    }

    /// Load the draft, returning `None` on any error (logged as a warning).
    #[must_use]
    pub fn load_best_effort() -> Option<Self> {
        match Self::load() {
            Ok(draft) => draft,
            Err(e) => {
                eprintln!("warning: failed to load new session draft: {e}");
                None
            }
        }
    }

    /// Save the draft, logging any error as a warning.
    pub fn save_best_effort(&self) {
        if let Err(e) = self.save() {
            eprintln!("warning: failed to save new session draft: {e}");
        }
    }

    /// Load the draft from an explicit path.
    ///
    /// - File absent returns `Ok(None)`.
    /// - File present but unparseable returns an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    fn load_from(path: &Path) -> Result<Option<Self>> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(e) => {
                return Err(CruiseError::Other(format!(
                    "failed to read new session draft {}: {e}",
                    path.display()
                )));
            }
        };
        serde_json::from_str(&content).map(Some).map_err(|e| {
            CruiseError::Other(format!(
                "invalid new session draft JSON in {}: {e}",
                path.display()
            ))
        })
    }

    /// Save the draft to the canonical [`Self::draft_path`].
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        let path = Self::draft_path()?;
        self.save_to(&path)
    }

    /// Save the draft to an explicit path.
    ///
    /// Creates parent directories as needed. Uses a temp-file-then-rename pattern for
    /// atomicity on platforms that support it.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or the file cannot be written.
    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CruiseError::Other(format!(
                    "failed to create draft dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let tmp_path = path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| CruiseError::Other(format!("failed to serialize draft: {e}")))?;
        std::fs::write(&tmp_path, content).map_err(|e| {
            CruiseError::Other(format!(
                "failed to write draft to {}: {e}",
                tmp_path.display()
            ))
        })?;
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(CruiseError::Other(format!(
                "failed to rename draft file: {e}"
            )));
        }
        Ok(())
    }

    /// Delete the draft file from the canonical path.
    ///
    /// Returns `Ok(())` even if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined or the file cannot be deleted.
    pub fn clear() -> Result<()> {
        let path = Self::draft_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CruiseError::Other(format!(
                "failed to remove new session draft {}: {e}",
                path.display()
            ))),
        }
    }

    /// Delete the draft file, logging any error as a warning.
    pub fn clear_best_effort() {
        if let Err(e) = Self::clear() {
            eprintln!("warning: failed to clear new session draft: {e}");
        }
    }

    /// Create a new draft with a fresh timestamp, suitable for saving after a form update.
    #[must_use]
    pub fn with_fresh_timestamp(&self) -> Self {
        let mut draft = self.clone();
        draft.updated_at = current_iso8601();
        draft
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_draft() -> NewSessionDraft {
        NewSessionDraft {
            input: "test task".to_string(),
            requested_config_path: Some("/tmp/cruise.yaml".to_string()),
            working_dir: "/tmp/project".to_string(),
            skipped_steps: vec!["review".to_string(), "write-tests".to_string()],
            updated_at: "2026-04-07T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_draft_path_ends_with_cruise_new_session_draft_json() {
        let path =
            NewSessionDraft::draft_path().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with(".cruise/new_session_draft.json")
                || path_str.ends_with(".cruise\\new_session_draft.json"),
            "expected path to end with .cruise/new_session_draft.json, got: {path_str}"
        );
    }

    #[test]
    fn test_load_from_returns_none_when_file_absent() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let missing = tmp.path().join("new_session_draft.json");
        let draft = NewSessionDraft::load_from(&missing)
            .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        assert!(draft.is_none(), "absent file should yield None");
    }

    #[test]
    fn test_load_from_returns_error_for_invalid_json() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("new_session_draft.json");
        std::fs::write(&path, "not valid json at all").unwrap_or_else(|e| panic!("{e:?}"));
        let result = NewSessionDraft::load_from(&path);
        assert!(result.is_err(), "expected error for invalid JSON, got Ok");
    }

    #[test]
    fn test_save_and_load_round_trip_through_default_cruise_home() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());

        let draft = make_draft();
        draft.save().unwrap_or_else(|e| panic!("save failed: {e}"));

        let loaded = NewSessionDraft::load()
            .unwrap_or_else(|e| panic!("load failed: {e}"))
            .unwrap_or_else(|| panic!("expected Some, got None"));
        assert_eq!(loaded.input, "test task");
        assert_eq!(
            loaded.requested_config_path.as_deref(),
            Some("/tmp/cruise.yaml")
        );
        assert_eq!(loaded.working_dir, "/tmp/project");
        assert_eq!(loaded.skipped_steps, vec!["review", "write-tests"]);
        assert!(
            tmp.path()
                .join(".cruise")
                .join("new_session_draft.json")
                .exists(),
            "new_session_draft.json should be written under the fake cruise home"
        );
    }

    #[test]
    fn test_save_to_and_load_from_round_trip_preserves_all_fields() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("new_session_draft.json");
        let draft = make_draft();
        draft
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save failed: {e}"));
        let loaded = NewSessionDraft::load_from(&path)
            .unwrap_or_else(|e| panic!("load failed: {e}"))
            .unwrap_or_else(|| panic!("expected Some, got None"));
        assert_eq!(loaded.input, "test task");
        assert_eq!(
            loaded.requested_config_path.as_deref(),
            Some("/tmp/cruise.yaml")
        );
        assert_eq!(loaded.working_dir, "/tmp/project");
        assert_eq!(loaded.skipped_steps, vec!["review", "write-tests"]);
        assert_eq!(loaded.updated_at, "2026-04-07T00:00:00Z");
    }

    #[test]
    fn test_save_to_creates_parent_directories() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp
            .path()
            .join("a")
            .join("b")
            .join("new_session_draft.json");
        let draft = NewSessionDraft::default();
        draft
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save failed: {e}"));
        assert!(
            path.exists(),
            "new_session_draft.json should exist at {}",
            path.display()
        );
    }

    #[test]
    fn test_save_to_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("new_session_draft.json");
        let old_draft = NewSessionDraft {
            input: "old task".to_string(),
            ..Default::default()
        };
        old_draft.save_to(&path).unwrap_or_else(|e| panic!("{e}"));

        let new_draft = make_draft();
        new_draft.save_to(&path).unwrap_or_else(|e| panic!("{e}"));

        let loaded = NewSessionDraft::load_from(&path)
            .unwrap_or_else(|e| panic!("load failed: {e}"))
            .unwrap_or_else(|| panic!("expected Some, got None"));
        assert_eq!(loaded.input, "test task");
    }

    #[test]
    fn test_clear_removes_file() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());

        let draft = make_draft();
        draft.save().unwrap_or_else(|e| panic!("save failed: {e}"));
        assert!(
            tmp.path()
                .join(".cruise")
                .join("new_session_draft.json")
                .exists()
        );

        NewSessionDraft::clear().unwrap_or_else(|e| panic!("clear failed: {e}"));
        assert!(
            !tmp.path()
                .join(".cruise")
                .join("new_session_draft.json")
                .exists()
        );
    }

    #[test]
    fn test_clear_is_idempotent_when_file_absent() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());

        NewSessionDraft::clear().unwrap_or_else(|e| panic!("first clear failed: {e}"));
        NewSessionDraft::clear().unwrap_or_else(|e| panic!("second clear failed: {e}"));
    }

    #[test]
    fn test_load_returns_none_when_no_file_exists() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());

        let draft = NewSessionDraft::load().unwrap_or_else(|e| panic!("load failed: {e}"));
        assert!(draft.is_none(), "no file should yield None");
    }

    #[test]
    fn test_load_best_effort_returns_none_on_error() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());

        std::fs::create_dir_all(tmp.path().join(".cruise")).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp.path().join(".cruise").join("new_session_draft.json"),
            "bad json",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let draft = NewSessionDraft::load_best_effort();
        assert!(
            draft.is_none(),
            "best_effort should return None on parse error"
        );
    }

    #[test]
    fn test_load_best_effort_returns_some_when_valid() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());

        let draft = make_draft();
        draft.save().unwrap_or_else(|e| panic!("save failed: {e}"));

        let loaded = NewSessionDraft::load_best_effort();
        assert!(
            loaded.is_some(),
            "best_effort should return Some for valid file"
        );
        assert_eq!(
            loaded.unwrap_or_else(|| panic!("expected Some, got None")).input,
            "test task"
        );
    }

    #[test]
    fn test_default_draft_has_empty_fields() {
        let draft = NewSessionDraft::default();
        assert!(draft.input.is_empty());
        assert!(draft.requested_config_path.is_none());
        assert!(draft.working_dir.is_empty());
        assert!(draft.skipped_steps.is_empty());
        assert!(draft.updated_at.is_empty());
    }

    #[test]
    fn test_serde_default_handles_missing_fields() {
        let json = r"{}";
        let draft: NewSessionDraft =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("deserialize failed: {e}"));
        assert!(draft.input.is_empty());
        assert!(draft.requested_config_path.is_none());
        assert!(draft.working_dir.is_empty());
        assert!(draft.skipped_steps.is_empty());
    }

    #[test]
    fn test_serde_default_handles_partial_json_with_legacy_fields() {
        let json = r#"{"input": "hello", "working_dir": "/tmp"}"#;
        let draft: NewSessionDraft =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("deserialize failed: {e}"));
        assert_eq!(draft.input, "hello");
        assert_eq!(draft.working_dir, "/tmp");
        assert!(draft.requested_config_path.is_none());
        assert!(draft.skipped_steps.is_empty());
    }

    #[test]
    fn test_with_fresh_timestamp_updates_time() {
        let draft = make_draft();
        let fresh = draft.with_fresh_timestamp();
        assert_eq!(fresh.input, draft.input);
        assert_eq!(fresh.working_dir, draft.working_dir);
        assert_ne!(fresh.updated_at, draft.updated_at);
        assert!(!fresh.updated_at.is_empty());
    }
}
