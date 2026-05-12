use std::path::{Path, PathBuf};

#[cfg(unix)]
use users::os::unix::UserExt;

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};
use crate::session::current_iso8601;

/// Sentinel value used when the built-in default config is in effect.
pub const BUILTIN_CONFIG_KEY: &str = "__builtin__";

/// A single recorded New Session selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewSessionHistoryEntry {
    /// When the selection was recorded.
    #[serde(default)]
    pub selected_at: String,
    /// User-typed task description. Empty string for legacy entries.
    #[serde(default)]
    pub input: String,
    /// The raw config selection shown in the GUI dropdown.
    ///
    /// `None` means "auto resolve".
    #[serde(default)]
    pub requested_config_path: Option<String>,
    /// The selected working directory after normalization.
    #[serde(default)]
    pub working_dir: String,
    /// The effective config key after resolution.
    ///
    /// For file-based configs this is the absolute path string.
    /// For the built-in default this is [`BUILTIN_CONFIG_KEY`].
    pub resolved_config_key: String,
    /// Step names the user explicitly chose to skip.
    #[serde(default)]
    pub skipped_steps: Vec<String>,
}

/// Persistent ring-buffer of per-config skip-step selections.
///
/// Stored at `~/.cruise/history.json`. Missing file is treated as empty history.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NewSessionHistory {
    /// Entries in most-recent-first order.
    pub entries: Vec<NewSessionHistoryEntry>,
}

impl NewSessionHistory {
    /// Maximum number of history entries to retain.
    pub const MAX_ENTRIES: usize = 50;

    /// Return the canonical path to the history file: `~/.cruise/history.json`.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined.
    fn history_path() -> Result<PathBuf> {
        crate::session::get_cruise_home().map(|h| h.join("history.json"))
    }

    /// Load history from the canonical [`Self::history_path`].
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined or the file is corrupt.
    pub fn load() -> Result<Self> {
        let path = Self::history_path()?;
        Self::load_from(&path)
    }

    /// Load history, returning empty history on any error (logged as a warning).
    #[must_use]
    pub fn load_best_effort() -> Self {
        match Self::load() {
            Ok(history) => history,
            Err(e) => {
                eprintln!("warning: failed to load history: {e}");
                Self::default()
            }
        }
    }

    /// Save history, logging any error as a warning.
    pub fn save_best_effort(&self) {
        if let Err(e) = self.save() {
            eprintln!("warning: failed to save history: {e}");
        }
    }

    /// Load history from an explicit path.
    ///
    /// - File absent returns empty history.
    /// - File present but unparseable returns an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    fn load_from(path: &Path) -> Result<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => {
                return Err(CruiseError::Other(format!(
                    "failed to read history {}: {e}",
                    path.display()
                )));
            }
        };
        serde_json::from_str(&content).map_err(|e| {
            CruiseError::Other(format!("invalid history JSON in {}: {e}", path.display()))
        })
    }

    /// Save history to the canonical [`Self::history_path`].
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        let path = Self::history_path()?;
        self.save_to(&path)
    }

    /// Save history to an explicit path.
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
                    "failed to create history dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let tmp_path = path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| CruiseError::Other(format!("failed to serialize history: {e}")))?;
        std::fs::write(&tmp_path, content).map_err(|e| {
            CruiseError::Other(format!(
                "failed to write history to {}: {e}",
                tmp_path.display()
            ))
        })?;
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(CruiseError::Other(format!(
                "failed to rename history file: {e}"
            )));
        }
        Ok(())
    }

    /// Record a new selection at the front of the list.
    ///
    /// Caps the list at [`Self::MAX_ENTRIES`] by dropping the oldest entry when needed.
    pub fn record_selection(&mut self, mut entry: NewSessionHistoryEntry) {
        if entry.selected_at.is_empty() {
            entry.selected_at = current_iso8601();
        }
        if entry.requested_config_path.as_deref() == Some("") {
            entry.requested_config_path = None;
        }
        entry.working_dir = normalize_working_dir(&entry.working_dir);
        self.entries.insert(0, entry);
        self.entries.truncate(Self::MAX_ENTRIES);
    }

    /// Record skipped-step defaults for a config without creating redundant
    /// skip-only entries when the config already has history.
    pub fn record_skip_selection_for_config(
        &mut self,
        resolved_config_key: &str,
        skipped_steps: Vec<String>,
    ) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.resolved_config_key == resolved_config_key)
        {
            entry.selected_at = current_iso8601();
            entry.skipped_steps = skipped_steps;
            return;
        }

        self.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir: String::new(),
            resolved_config_key: resolved_config_key.to_string(),
            skipped_steps,
        });
    }

    /// Return the most recently recorded entry whose `resolved_config_key` equals
    /// `resolved_config_key`, or `None` if no matching entry exists.
    #[must_use]
    pub fn latest_entry_for_config(
        &self,
        resolved_config_key: &str,
    ) -> Option<&NewSessionHistoryEntry> {
        self.entries
            .iter()
            .find(|e| e.resolved_config_key == resolved_config_key)
    }
}

/// Return the resolved config key for a session.
///
/// File-based configs use their absolute path string; the built-in default
/// uses [`BUILTIN_CONFIG_KEY`].
#[must_use]
pub fn resolved_config_key_for_session(config_path: Option<&PathBuf>) -> String {
    config_path.map_or_else(
        || BUILTIN_CONFIG_KEY.to_string(),
        |path| path.to_string_lossy().into_owned(),
    )
}

/// Normalize a working-directory string for history storage and deduplication.
#[must_use]
pub fn normalize_working_dir(value: &str) -> String {
    let expanded = expand_tilde(value.trim());
    let normalized = Path::new(&expanded)
        .components()
        .as_path()
        .to_string_lossy()
        .into_owned();
    if normalized.is_empty() {
        expanded
    } else {
        normalized
    }
}

#[must_use]
pub fn expand_tilde(path: &str) -> String {
    if path == "~"
        && let Some(home) = home::home_dir()
    {
        return home.to_string_lossy().to_string();
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home::home_dir()
    {
        return format!("{}/{}", home.to_string_lossy(), rest);
    }
    #[cfg(unix)]
    if let Some(rest) = path.strip_prefix("~") {
        if rest.is_empty() || rest.starts_with('/') {
            return path.to_string();
        }
        if let Some(slash_idx) = rest.find('/') {
            let username = &rest[..slash_idx];
            let remainder = &rest[slash_idx..];
            if let Some(user) = users::get_user_by_name(username) {
                let home = user.home_dir();
                return format!("{}{}", home.to_string_lossy(), remainder);
            }
        } else if let Some(user) = users::get_user_by_name(rest) {
            let home = user.home_dir();
            return home.to_string_lossy().to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    use tempfile::TempDir;

    fn skipped_steps_to_default_indices(
        all_steps: &[&str],
        saved_skipped: &[String],
    ) -> Vec<usize> {
        all_steps
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                if saved_skipped.iter().any(|saved| saved == *name) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect()
    }

    fn make_entry(resolved_config_key: &str, skipped_steps: Vec<&str>) -> NewSessionHistoryEntry {
        NewSessionHistoryEntry {
            selected_at: "2026-04-07T00:00:00Z".to_string(),
            input: String::new(),
            requested_config_path: None,
            working_dir: "/tmp/project".to_string(),
            resolved_config_key: resolved_config_key.to_string(),
            skipped_steps: skipped_steps.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_history_path_ends_with_cruise_history_json() {
        let path =
            NewSessionHistory::history_path().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with(".cruise/history.json")
                || path_str.ends_with(".cruise\\history.json"),
            "expected path to end with .cruise/history.json, got: {path_str}"
        );
    }

    #[test]
    fn test_load_from_returns_empty_when_file_absent() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let missing = tmp.path().join("nonexistent").join("history.json");
        let history = NewSessionHistory::load_from(&missing)
            .unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        assert!(
            history.entries.is_empty(),
            "absent history file should yield empty entries"
        );
    }

    #[test]
    fn test_load_from_returns_error_for_invalid_json() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("history.json");
        std::fs::write(&path, "not valid json at all").unwrap_or_else(|e| panic!("{e:?}"));
        let result = NewSessionHistory::load_from(&path);
        assert!(result.is_err(), "expected error for invalid JSON, got Ok");
    }

    #[test]
    fn test_save_and_load_round_trip_through_default_cruise_home() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());

        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/config/a.yaml", vec!["step1"]));
        history
            .save()
            .unwrap_or_else(|e| panic!("save failed: {e}"));

        let loaded = NewSessionHistory::load().unwrap_or_else(|e| panic!("load failed: {e}"));
        assert_eq!(loaded.entries, history.entries);
        assert!(
            tmp.path().join(".cruise").join("history.json").exists(),
            "history.json should be written under the fake cruise home"
        );
    }

    #[test]
    fn test_record_selection_prepends_newest_to_front() {
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/path/a.yaml", vec!["step1"]));
        history.record_selection(make_entry("/path/b.yaml", vec!["step2"]));
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].resolved_config_key, "/path/b.yaml");
        assert_eq!(history.entries[1].resolved_config_key, "/path/a.yaml");
    }

    #[test]
    fn test_record_selection_caps_at_max_entries() {
        let mut history = NewSessionHistory::default();
        for i in 0..NewSessionHistory::MAX_ENTRIES {
            history.record_selection(make_entry(&format!("/path/config-{i}.yaml"), vec![]));
        }
        history.record_selection(make_entry("/path/new.yaml", vec![]));
        assert_eq!(history.entries.len(), NewSessionHistory::MAX_ENTRIES);
        assert_eq!(history.entries[0].resolved_config_key, "/path/new.yaml");
    }

    #[test]
    fn test_latest_entry_for_config_returns_none_when_key_not_found() {
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/config/a.yaml", vec![]));
        assert!(
            history
                .latest_entry_for_config("/config/nonexistent.yaml")
                .is_none()
        );
    }

    #[test]
    fn test_latest_entry_for_config_returns_most_recent_skipped_steps() {
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/config/a.yaml", vec!["step1"]));
        history.record_selection(make_entry("/config/a.yaml", vec!["step2", "step3"]));
        let entry = history
            .latest_entry_for_config("/config/a.yaml")
            .unwrap_or_else(|| panic!("expected Some, got None"));
        assert_eq!(entry.skipped_steps, vec!["step2", "step3"]);
    }

    #[test]
    fn test_latest_entry_for_config_builtin_and_path_do_not_collide() {
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry(BUILTIN_CONFIG_KEY, vec!["builtin_step"]));
        history.record_selection(make_entry("/config/a.yaml", vec!["file_step"]));
        let builtin = history
            .latest_entry_for_config(BUILTIN_CONFIG_KEY)
            .unwrap_or_else(|| panic!("expected Some for builtin, got None"));
        let file_based = history
            .latest_entry_for_config("/config/a.yaml")
            .unwrap_or_else(|| panic!("expected Some for file config, got None"));
        assert_eq!(builtin.skipped_steps, vec!["builtin_step"]);
        assert_eq!(file_based.skipped_steps, vec!["file_step"]);
    }

    #[test]
    fn test_save_to_and_load_from_round_trip_preserves_fields() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("history.json");
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/config/a.yaml", vec!["step1"]));
        history.record_selection(make_entry(BUILTIN_CONFIG_KEY, vec!["step2", "step3"]));
        history
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save failed: {e}"));
        let loaded =
            NewSessionHistory::load_from(&path).unwrap_or_else(|e| panic!("load failed: {e}"));
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].resolved_config_key, BUILTIN_CONFIG_KEY);
        assert_eq!(loaded.entries[0].skipped_steps, vec!["step2", "step3"]);
        assert_eq!(loaded.entries[1].resolved_config_key, "/config/a.yaml");
    }

    #[test]
    fn test_save_to_creates_parent_directories() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("a").join("b").join("history.json");
        let history = NewSessionHistory::default();
        history
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save failed: {e}"));
        assert!(
            path.exists(),
            "history.json should exist at {}",
            path.display()
        );
    }

    #[test]
    fn test_save_to_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("history.json");
        let mut old_history = NewSessionHistory::default();
        old_history.record_selection(make_entry("/old.yaml", vec!["old_step"]));
        old_history.save_to(&path).unwrap_or_else(|e| panic!("{e}"));

        let mut new_history = NewSessionHistory::default();
        new_history.record_selection(make_entry("/new.yaml", vec!["new_step"]));
        new_history.save_to(&path).unwrap_or_else(|e| panic!("{e}"));

        let loaded = NewSessionHistory::load_from(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].resolved_config_key, "/new.yaml");
        assert_eq!(loaded.entries[0].skipped_steps, vec!["new_step"]);
    }

    #[test]
    fn test_entries_remain_most_recent_first() {
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/old.yaml", vec!["plan"]));
        history.record_selection(make_entry("/new.yaml", vec!["review"]));
        let latest = history
            .entries
            .first()
            .unwrap_or_else(|| panic!("expected latest entry"));
        assert_eq!(latest.resolved_config_key, "/new.yaml");
        assert_eq!(latest.skipped_steps, vec!["review"]);
    }

    #[test]
    fn test_record_selection_normalizes_working_dir_and_empty_config_selection() {
        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: Some(String::new()),
            working_dir: "/tmp/project/".to_string(),
            resolved_config_key: BUILTIN_CONFIG_KEY.to_string(),
            skipped_steps: vec![],
        });
        let entry = history
            .entries
            .first()
            .unwrap_or_else(|| panic!("expected latest entry"));
        assert!(!entry.selected_at.is_empty());
        assert_eq!(entry.requested_config_path, None);
        assert_eq!(entry.working_dir, "/tmp/project");
    }

    #[test]
    fn test_record_skip_selection_updates_existing_gui_entry_in_place() {
        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: "2026-04-07T00:00:00Z".to_string(),
            input: String::new(),
            requested_config_path: Some("/config/a.yaml".to_string()),
            working_dir: "/tmp/project".to_string(),
            resolved_config_key: "/config/a.yaml".to_string(),
            skipped_steps: vec!["plan".to_string()],
        });

        history.record_skip_selection_for_config("/config/a.yaml", vec!["review".to_string()]);

        assert_eq!(history.entries.len(), 1);
        assert_eq!(
            history.entries[0].requested_config_path.as_deref(),
            Some("/config/a.yaml")
        );
        assert_eq!(history.entries[0].working_dir, "/tmp/project");
        assert_eq!(history.entries[0].skipped_steps, vec!["review"]);
    }

    #[test]
    fn test_record_skip_selection_inserts_skip_only_entry_when_config_is_new() {
        let mut history = NewSessionHistory::default();

        history.record_skip_selection_for_config("/config/a.yaml", vec!["review".to_string()]);

        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].resolved_config_key, "/config/a.yaml");
        assert_eq!(history.entries[0].requested_config_path, None);
        assert_eq!(history.entries[0].working_dir, "");
        assert_eq!(history.entries[0].skipped_steps, vec!["review"]);
    }

    #[test]
    fn test_builtin_and_path_config_keys_coexist_within_cap() {
        let mut history = NewSessionHistory::default();
        for _ in 0..25 {
            history.record_selection(make_entry(BUILTIN_CONFIG_KEY, vec![]));
        }
        for _ in 0..25 {
            history.record_selection(make_entry("/config/custom.yaml", vec![]));
        }
        assert_eq!(history.entries.len(), NewSessionHistory::MAX_ENTRIES);
        assert!(
            history
                .latest_entry_for_config(BUILTIN_CONFIG_KEY)
                .is_some()
        );
        assert!(
            history
                .latest_entry_for_config("/config/custom.yaml")
                .is_some()
        );
    }

    #[test]
    fn test_default_indices_empty_when_no_saved_steps() {
        let all_steps = &["plan", "review", "implement", "test"];
        let saved_skipped: Vec<String> = vec![];
        let indices = skipped_steps_to_default_indices(all_steps, &saved_skipped);
        assert!(indices.is_empty());
    }

    #[test]
    fn test_default_indices_returns_correct_positions() {
        let all_steps = &["plan", "review", "implement", "test", "deploy"];
        let saved_skipped = vec!["review".to_string(), "deploy".to_string()];
        let indices = skipped_steps_to_default_indices(all_steps, &saved_skipped);
        assert_eq!(indices, vec![1, 4]);
    }

    #[test]
    fn test_default_indices_silently_drops_stale_step_names() {
        let all_steps = &["plan", "implement", "test"];
        let saved_skipped = vec!["review".to_string(), "implement".to_string()];
        let indices = skipped_steps_to_default_indices(all_steps, &saved_skipped);
        assert_eq!(indices, vec![1]);
    }

    #[test]
    fn test_default_indices_empty_when_all_steps_list_is_empty() {
        let all_steps: &[&str] = &[];
        let saved_skipped = vec!["step1".to_string()];
        let indices = skipped_steps_to_default_indices(all_steps, &saved_skipped);
        assert!(indices.is_empty());
    }

    #[test]
    fn test_default_indices_all_steps_match_saved() {
        let all_steps = &["a", "b", "c"];
        let saved_skipped = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let indices = skipped_steps_to_default_indices(all_steps, &saved_skipped);
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_default_indices_preserves_order_from_all_steps() {
        let all_steps = &["alpha", "beta", "gamma", "delta"];
        let saved_skipped = vec!["delta".to_string(), "alpha".to_string()];
        let indices = skipped_steps_to_default_indices(all_steps, &saved_skipped);
        assert_eq!(indices, vec![0, 3]);
    }

    #[test]
    fn test_resolved_config_key_for_session_uses_builtin_for_none() {
        assert_eq!(resolved_config_key_for_session(None), BUILTIN_CONFIG_KEY);
    }

    #[test]
    fn test_resolved_config_key_for_session_returns_path_string() {
        let path = PathBuf::from("/tmp/cruise.yaml");
        assert_eq!(
            resolved_config_key_for_session(Some(&path)),
            "/tmp/cruise.yaml"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_resolved_config_key_for_session_keeps_non_utf8_paths_distinct_from_builtin() {
        let path = PathBuf::from(OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0x80]));
        let key = resolved_config_key_for_session(Some(&path));
        assert_ne!(key, BUILTIN_CONFIG_KEY);
        assert_eq!(key, path.to_string_lossy());
    }

    #[test]
    fn test_normalize_working_dir_removes_trailing_slash() {
        assert_eq!(normalize_working_dir("/tmp/project/"), "/tmp/project");
    }
}
