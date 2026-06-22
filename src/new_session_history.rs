use std::path::{Path, PathBuf};

#[cfg(unix)]
use users::os::unix::UserExt;

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};
use crate::session::current_iso8601;

/// Sentinel key used when the session config comes from the built-in default
/// rather than a user-supplied file.
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
    /// GitHub repository spec ("owner/repo") for repository-mode sessions.
    #[serde(default)]
    pub repo: Option<String>,
    /// The effective config key after resolution.
    ///
    /// For file-based configs this is the absolute path string.
    /// For the built-in default this is [`"__builtin__"`].
    pub resolved_config_key: String,
    /// Step names the user explicitly chose to skip.
    #[serde(default)]
    pub skipped_steps: Vec<String>,
}

/// Scope key used for history lookup and recording.
///
/// Directory scope matches on `working_dir`; Repo scope matches on `repo`.
/// Both are used in combination with `resolved_config_key` as a compound key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryScope<'a> {
    /// GitHub repository mode — "owner/repo" spec.
    Repo(&'a str),
    /// Local directory mode — normalized absolute path.
    Directory(&'a str),
}

impl HistoryScope<'_> {
    /// Return `true` if this scope's key is non-empty and the entry's corresponding
    /// field matches.  An empty scope string never matches to prevent dead entries.
    fn matches(self, entry: &NewSessionHistoryEntry) -> bool {
        match self {
            HistoryScope::Repo(r) => {
                !r.is_empty()
                    && entry
                        .repo
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(r))
            }
            HistoryScope::Directory(d) => {
                !d.is_empty()
                    && !entry.working_dir.is_empty()
                    && normalize_working_dir(&entry.working_dir) == normalize_working_dir(d)
            }
        }
    }

    /// Return `true` if the scope key is non-empty (i.e., safe to record).
    fn is_non_empty(self) -> bool {
        match self {
            HistoryScope::Repo(r) => !r.is_empty(),
            HistoryScope::Directory(d) => !d.is_empty(),
        }
    }
}

/// Persistent ring-buffer of per-config skip-step selections.
///
/// Stored at `~/.local/state/cruise/history.json`. Missing file is treated as empty history.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NewSessionHistory {
    /// Entries in most-recent-first order.
    pub entries: Vec<NewSessionHistoryEntry>,
}

impl NewSessionHistory {
    /// Maximum number of history entries to retain.
    pub const MAX_ENTRIES: usize = 50;

    /// Return the canonical path to the history file: `~/.local/state/cruise/history.json`.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined.
    fn history_path() -> Result<PathBuf> {
        crate::paths::state_dir().map(|h| h.join("history.json"))
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
        if is_temp_working_dir(&entry.working_dir) {
            entry.working_dir = String::new();
        }
        self.entries.insert(0, entry);
        // Remove older duplicates with the same compound key so the ring buffer is
        // not filled by repeated settings-saves for the same session context.
        let (wd, repo, ck) = {
            let e = &self.entries[0];
            (
                e.working_dir.clone(),
                e.repo.clone(),
                e.resolved_config_key.clone(),
            )
        };
        let mut first_seen = false;
        self.entries.retain(|e| {
            if e.working_dir == wd && e.repo == repo && e.resolved_config_key == ck {
                if first_seen {
                    return false;
                }
                first_seen = true;
            }
            true
        });
        self.entries.truncate(Self::MAX_ENTRIES);
    }

    /// Return the most recently recorded entry matching the compound key
    /// `(scope, resolved_config_key)`, or `None` if no matching entry exists.
    ///
    /// Matching rules:
    /// - `Repo(r)`: `entry.repo == Some(r)` and `r` is non-empty
    /// - `Directory(d)`: `d` is non-empty and `normalize_working_dir(entry.working_dir) == normalize_working_dir(d)`
    /// - Additionally: `entry.resolved_config_key == resolved_config_key`
    #[must_use]
    pub fn latest_entry_for_scope(
        &self,
        scope: HistoryScope<'_>,
        resolved_config_key: &str,
    ) -> Option<&NewSessionHistoryEntry> {
        self.entries
            .iter()
            .find(|e| e.resolved_config_key == resolved_config_key && scope.matches(e))
            .or_else(|| {
                // Legacy fallback: old history entries (written before scope-based recording
                // was introduced) have working_dir="" and repo=None. Match them by config
                // key alone so existing skip preferences survive an upgrade.
                self.entries.iter().find(|e| {
                    e.resolved_config_key == resolved_config_key
                        && e.working_dir.is_empty()
                        && e.repo.is_none()
                })
            })
    }

    #[cfg(test)]
    fn latest_entry_for_config(
        &self,
        resolved_config_key: &str,
    ) -> Option<&NewSessionHistoryEntry> {
        self.entries
            .iter()
            .find(|e| e.resolved_config_key == resolved_config_key)
    }

    /// Record skipped-step defaults using the compound key `(scope, resolved_config_key)`.
    ///
    /// Updates an existing matching entry in-place, or inserts a new entry at the front.
    /// New entries populate `repo` or `working_dir` according to the scope kind.
    pub fn record_skip_selection_for_scope(
        &mut self,
        scope: HistoryScope<'_>,
        resolved_config_key: &str,
        skipped_steps: Vec<String>,
    ) {
        // Never write a dead entry that can never be looked up again.
        if !scope.is_non_empty() {
            return;
        }
        // Directory scope: temp paths get cleared to "" by record_selection, which would
        // create a dead entry that no scope lookup can ever retrieve.
        if let HistoryScope::Directory(d) = scope
            && is_temp_working_dir(&normalize_working_dir(d))
        {
            return;
        }

        let found = self
            .entries
            .iter_mut()
            .find(|e| e.resolved_config_key == resolved_config_key && scope.matches(e));

        if let Some(entry) = found {
            entry.selected_at = current_iso8601();
            entry.skipped_steps = skipped_steps;
            return;
        }

        let (working_dir, repo) = match scope {
            HistoryScope::Repo(r) => (String::new(), Some(r.to_string())),
            HistoryScope::Directory(d) => (d.to_string(), None),
        };

        self.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir,
            repo,
            resolved_config_key: resolved_config_key.to_string(),
            skipped_steps,
        });
    }
}

/// Return the resolved config key for a session.
///
/// Returns the absolute path string of the config file.
#[must_use]
pub fn resolved_config_key_for_session(config_path: &Path) -> String {
    config_path.to_string_lossy().into_owned()
}

/// Return `true` if `path` is under a known system temp directory.
///
/// Used to filter out paths that come from test fixtures (e.g. `tempfile::TempDir`)
/// so they never leak into the user-visible recent-working-dirs list.
#[must_use]
pub fn is_temp_working_dir(path: &str) -> bool {
    // Fallback: well-known temp prefixes (handles paths that do not exist on disk).
    const TEMP_PREFIXES: &[&str] = &[
        "/var/folders/",         // macOS user TMPDIR
        "/private/var/folders/", // canonicalized macOS TMPDIR
        "/tmp/",                 // POSIX
        "/private/tmp/",         // canonicalized macOS /tmp
    ];
    if path.is_empty() {
        return false;
    }
    // Primary: std::env::temp_dir() — picks up TMPDIR / TMP / TEMP at runtime.
    if let Ok(temp) = std::env::temp_dir().canonicalize() {
        if let Ok(p) = std::path::Path::new(path).canonicalize()
            && p.starts_with(&temp)
        {
            return true;
        }
        // String-comparison fallback when canonicalize fails (e.g. path does not exist).
        let temp_str = temp.to_string_lossy();
        if path.starts_with(temp_str.as_ref()) {
            return true;
        }
    }
    for prefix in TEMP_PREFIXES {
        if path.starts_with(prefix) {
            return true;
        }
    }
    #[cfg(windows)]
    if path.contains("\\Temp\\") || path.contains("/Temp/") {
        return true;
    }
    false
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
            working_dir: "/Users/test/project".to_string(),
            repo: None,
            resolved_config_key: resolved_config_key.to_string(),
            skipped_steps: skipped_steps.into_iter().map(String::from).collect(),
        }
    }

    fn make_dir_entry(
        working_dir: &str,
        resolved_config_key: &str,
        skipped_steps: Vec<&str>,
    ) -> NewSessionHistoryEntry {
        NewSessionHistoryEntry {
            selected_at: "2026-04-07T00:00:00Z".to_string(),
            input: String::new(),
            requested_config_path: None,
            working_dir: working_dir.to_string(),
            repo: None,
            resolved_config_key: resolved_config_key.to_string(),
            skipped_steps: skipped_steps.into_iter().map(String::from).collect(),
        }
    }

    fn make_repo_entry(
        repo: &str,
        resolved_config_key: &str,
        skipped_steps: Vec<&str>,
    ) -> NewSessionHistoryEntry {
        NewSessionHistoryEntry {
            selected_at: "2026-04-07T00:00:00Z".to_string(),
            input: String::new(),
            requested_config_path: None,
            working_dir: String::new(),
            repo: Some(repo.to_string()),
            resolved_config_key: resolved_config_key.to_string(),
            skipped_steps: skipped_steps.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_history_path_ends_with_state_cruise_history_json() {
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = crate::test_support::set_fake_home(tmp.path());
        let path =
            NewSessionHistory::history_path().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with(".local/state/cruise/history.json")
                || path_str.ends_with(".local\\state\\cruise\\history.json"),
            "expected path to end with .local/state/cruise/history.json, got: {path_str}"
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
    fn test_save_and_load_round_trip_through_default_state_dir() {
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
            tmp.path()
                .join(".local")
                .join("state")
                .join("cruise")
                .join("history.json")
                .exists(),
            "history.json should be written under the fake state dir"
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
        history.record_selection(make_entry("__builtin__", vec!["builtin_step"]));
        history.record_selection(make_entry("/config/a.yaml", vec!["file_step"]));
        let builtin = history
            .latest_entry_for_config("__builtin__")
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
        history.record_selection(make_entry("__builtin__", vec!["step2", "step3"]));
        history
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save failed: {e}"));
        let loaded =
            NewSessionHistory::load_from(&path).unwrap_or_else(|e| panic!("load failed: {e}"));
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].resolved_config_key, "__builtin__");
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
            working_dir: "/Users/test/project/".to_string(),
            repo: None,
            resolved_config_key: "__builtin__".to_string(),
            skipped_steps: vec![],
        });
        let entry = history
            .entries
            .first()
            .unwrap_or_else(|| panic!("expected latest entry"));
        assert!(!entry.selected_at.is_empty());
        assert_eq!(entry.requested_config_path, None);
        assert_eq!(entry.working_dir, "/Users/test/project");
    }

    #[test]
    fn test_builtin_and_path_config_keys_coexist() {
        // Repeated recordings with the same (working_dir, repo, config) key are deduped,
        // so both config keys survive rather than one evicting the other.
        let mut history = NewSessionHistory::default();
        for _ in 0..25 {
            history.record_selection(make_entry("__builtin__", vec![]));
        }
        for _ in 0..25 {
            history.record_selection(make_entry("/config/custom.yaml", vec![]));
        }
        assert_eq!(
            history.entries.len(),
            2,
            "dedup keeps exactly one entry per (working_dir, repo, config) key"
        );
        assert!(history.latest_entry_for_config("__builtin__").is_some());
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
    fn test_resolved_config_key_for_session_returns_path_string() {
        let path = PathBuf::from("/tmp/cruise.yaml");
        assert_eq!(resolved_config_key_for_session(&path), "/tmp/cruise.yaml");
    }

    #[cfg(unix)]
    #[test]
    fn test_resolved_config_key_for_session_keeps_non_utf8_paths_distinct_from_builtin() {
        let path = PathBuf::from(OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0x80]));
        let key = resolved_config_key_for_session(&path);
        assert_ne!(key, "__builtin__");
        assert_eq!(key, path.to_string_lossy());
    }

    #[test]
    fn test_normalize_working_dir_removes_trailing_slash() {
        assert_eq!(normalize_working_dir("/tmp/project/"), "/tmp/project");
    }

    // ---- resolved_config_key_for_session: new non-Option API ----

    #[test]
    fn test_resolved_config_key_for_session_with_path_ref_returns_absolute_path_string() {
        // Given: a PathBuf representing an absolute config file path
        let path = PathBuf::from("/home/user/.config/cruise/myconf.yaml");

        // When: resolved_config_key_for_session is called with &path directly (non-Option)
        let key = resolved_config_key_for_session(&path);

        // Then: returns the path string (same semantics, new mandatory parameter)
        assert_eq!(key, "/home/user/.config/cruise/myconf.yaml");
        assert_ne!(key, "__builtin__");
    }

    #[test]
    fn test_history_round_trip_without_builtin_entries() {
        // Given: a history containing only file-based config entries (no builtin)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("history.json");
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/config/a.yaml", vec!["step1"]));
        history.record_selection(make_entry("/config/b.yaml", vec![]));

        // When: saved and loaded
        history.save_to(&path).unwrap_or_else(|e| panic!("{e}"));
        let loaded =
            NewSessionHistory::load_from(&path).unwrap_or_else(|e| panic!("load failed: {e}"));

        // Then: all entries are file-path entries with no builtin keys
        assert_eq!(loaded.entries.len(), 2);
        for entry in &loaded.entries {
            assert_ne!(
                entry.resolved_config_key, "__builtin__",
                "no new session should produce a builtin key"
            );
        }
    }

    #[test]
    fn test_latest_entry_for_config_works_with_only_file_based_entries() {
        // Given: history with only file-based entries (the builtin variant will be removed)
        let mut history = NewSessionHistory::default();
        history.record_selection(make_entry("/config/a.yaml", vec!["step-a"]));
        history.record_selection(make_entry("/config/b.yaml", vec!["step-b"]));

        // When: looking up by file path
        let entry_a = history.latest_entry_for_config("/config/a.yaml");
        let entry_b = history.latest_entry_for_config("/config/b.yaml");
        let entry_missing = history.latest_entry_for_config("__builtin__");

        // Then: file-based lookups work; builtin key is not present (no new sessions produce it)
        let Some(entry_a) = entry_a else {
            panic!("entry_a should be Some for /config/a.yaml");
        };
        assert_eq!(entry_a.skipped_steps, vec!["step-a"]);
        let Some(entry_b) = entry_b else {
            panic!("entry_b should be Some for /config/b.yaml");
        };
        assert_eq!(entry_b.skipped_steps, vec!["step-b"]);
        assert!(
            entry_missing.is_none(),
            "builtin key should not exist in a history populated by new sessions"
        );
    }

    // ---- is_temp_working_dir ----

    #[test]
    fn test_is_temp_working_dir_returns_false_for_empty_string() {
        assert!(!is_temp_working_dir(""));
    }

    #[test]
    fn test_is_temp_working_dir_returns_false_for_normal_user_dir() {
        assert!(!is_temp_working_dir("/Users/takumi/apps/cruise"));
    }

    #[test]
    fn test_is_temp_working_dir_returns_true_for_var_folders() {
        assert!(is_temp_working_dir(
            "/var/folders/4r/cb2pswws7fsctl8ksr1xpk100000gn/T/.tmpqR0urI/repo"
        ));
    }

    #[test]
    fn test_is_temp_working_dir_returns_true_for_private_var_folders() {
        assert!(is_temp_working_dir(
            "/private/var/folders/4r/cb2pswws7fsctl8ksr1xpk100000gn/T/.tmpD8hIwu/repo"
        ));
    }

    #[test]
    fn test_is_temp_working_dir_returns_true_for_tmp_prefix() {
        assert!(is_temp_working_dir("/tmp/foo"));
    }

    #[test]
    fn test_is_temp_working_dir_returns_true_for_private_tmp_prefix() {
        assert!(is_temp_working_dir("/private/tmp/some_dir"));
    }

    #[test]
    fn test_record_selection_clears_working_dir_for_temp_path() {
        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: "task".to_string(),
            requested_config_path: None,
            working_dir: "/var/folders/4r/cb2pswws7fsctl8ksr1xpk100000gn/T/.tmpXYZ/repo"
                .to_string(),
            repo: None,
            resolved_config_key: "__builtin__".to_string(),
            skipped_steps: vec![],
        });
        let entry = history
            .entries
            .first()
            .unwrap_or_else(|| panic!("expected an entry"));
        assert_eq!(
            entry.working_dir, "",
            "temp-dir working_dir should be cleared to empty string"
        );
    }

    #[test]
    fn test_record_selection_keeps_working_dir_for_normal_path() {
        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir: "/Users/takumi/projects/cruise".to_string(),
            repo: None,
            resolved_config_key: "__builtin__".to_string(),
            skipped_steps: vec![],
        });
        let entry = history
            .entries
            .first()
            .unwrap_or_else(|| panic!("expected an entry"));
        assert_eq!(entry.working_dir, "/Users/takumi/projects/cruise");
    }

    // ---- latest_entry_for_scope ----

    #[test]
    fn test_latest_entry_for_scope_directory_same_dir_different_configs() {
        // Given: two entries with the same directory but different configs
        let mut history = NewSessionHistory::default();
        history.entries.push(make_dir_entry(
            "/home/user/proj",
            "/config/a.yaml",
            vec!["step-a"],
        ));
        history.entries.push(make_dir_entry(
            "/home/user/proj",
            "/config/b.yaml",
            vec!["step-b"],
        ));

        // When: looking up by Directory scope + each config
        let entry_a = history
            .latest_entry_for_scope(HistoryScope::Directory("/home/user/proj"), "/config/a.yaml");
        let entry_b = history
            .latest_entry_for_scope(HistoryScope::Directory("/home/user/proj"), "/config/b.yaml");

        // Then: each returns the correct skipped_steps (no cross-contamination)
        let Some(entry_a) = entry_a else {
            panic!("expected Some for /config/a.yaml");
        };
        assert_eq!(entry_a.skipped_steps, vec!["step-a"]);
        let Some(entry_b) = entry_b else {
            panic!("expected Some for /config/b.yaml");
        };
        assert_eq!(entry_b.skipped_steps, vec!["step-b"]);
    }

    #[test]
    fn test_latest_entry_for_scope_directory_same_config_different_dirs() {
        // Given: two entries with the same config but different directories
        let mut history = NewSessionHistory::default();
        history.entries.push(make_dir_entry(
            "/home/user/proj-a",
            "/config/shared.yaml",
            vec!["step-a"],
        ));
        history.entries.push(make_dir_entry(
            "/home/user/proj-b",
            "/config/shared.yaml",
            vec!["step-b"],
        ));

        // When: looking up by Directory scope + config for each directory
        let entry_a = history.latest_entry_for_scope(
            HistoryScope::Directory("/home/user/proj-a"),
            "/config/shared.yaml",
        );
        let entry_b = history.latest_entry_for_scope(
            HistoryScope::Directory("/home/user/proj-b"),
            "/config/shared.yaml",
        );

        // Then: each returns its own skipped_steps — same config does not bleed across dirs
        let Some(entry_a) = entry_a else {
            panic!("expected Some for proj-a");
        };
        assert_eq!(entry_a.skipped_steps, vec!["step-a"]);
        let Some(entry_b) = entry_b else {
            panic!("expected Some for proj-b");
        };
        assert_eq!(entry_b.skipped_steps, vec!["step-b"]);
    }

    #[test]
    fn test_latest_entry_for_scope_repo_same_repo_different_configs() {
        // Given: two entries with the same repo but different configs
        let mut history = NewSessionHistory::default();
        history.entries.push(make_repo_entry(
            "owner/repo",
            "/config/a.yaml",
            vec!["step-a"],
        ));
        history.entries.push(make_repo_entry(
            "owner/repo",
            "/config/b.yaml",
            vec!["step-b"],
        ));

        // When: looking up by Repo scope + each config
        let entry_a =
            history.latest_entry_for_scope(HistoryScope::Repo("owner/repo"), "/config/a.yaml");
        let entry_b =
            history.latest_entry_for_scope(HistoryScope::Repo("owner/repo"), "/config/b.yaml");

        // Then: each returns the correct skipped_steps
        let Some(entry_a) = entry_a else {
            panic!("expected Some for /config/a.yaml");
        };
        assert_eq!(entry_a.skipped_steps, vec!["step-a"]);
        let Some(entry_b) = entry_b else {
            panic!("expected Some for /config/b.yaml");
        };
        assert_eq!(entry_b.skipped_steps, vec!["step-b"]);
    }

    #[test]
    fn test_latest_entry_for_scope_directory_falls_back_to_legacy_entry() {
        // Given: a legacy entry with empty working_dir and no repo (old format written before
        // scope-based recording was introduced).
        let mut history = NewSessionHistory::default();
        let mut legacy = make_entry("/config/a.yaml", vec!["step-legacy"]);
        legacy.working_dir = String::new();
        legacy.repo = None;
        history.entries.push(legacy);

        // When: looking up by Directory scope with a non-empty directory
        let result = history
            .latest_entry_for_scope(HistoryScope::Directory("/home/user/proj"), "/config/a.yaml");

        // Then: returns the legacy entry via the backward-compat fallback so that
        // skip preferences survive an upgrade from the scopeless history format.
        assert!(
            result.is_some(),
            "legacy entry (working_dir='', repo=None) should be returned as a fallback"
        );
        assert_eq!(
            result
                .unwrap_or_else(|| panic!("expected Some, got None"))
                .skipped_steps,
            vec!["step-legacy"]
        );
    }

    #[test]
    fn test_latest_entry_for_scope_legacy_fallback_not_used_when_scope_match_exists() {
        // Given: both a scope-matched entry and a legacy entry for the same config
        let mut history = NewSessionHistory::default();
        let scoped = make_dir_entry("/home/user/proj", "/config/a.yaml", vec!["step-scoped"]);
        history.entries.push(scoped);
        let mut legacy = make_entry("/config/a.yaml", vec!["step-legacy"]);
        legacy.working_dir = String::new();
        legacy.repo = None;
        history.entries.push(legacy);

        // When: looking up by Directory scope
        let result = history
            .latest_entry_for_scope(HistoryScope::Directory("/home/user/proj"), "/config/a.yaml");

        // Then: returns the scoped entry, not the legacy one
        assert_eq!(
            result
                .unwrap_or_else(|| panic!("expected Some, got None"))
                .skipped_steps,
            vec!["step-scoped"],
            "scoped entry should take priority over legacy fallback"
        );
    }

    #[test]
    fn test_latest_entry_for_scope_repo_mode_skips_entry_without_repo_field() {
        // Given: a directory-mode entry with no repo field
        let mut history = NewSessionHistory::default();
        history.entries.push(make_dir_entry(
            "/home/user/proj",
            "/config/a.yaml",
            vec!["step-dir"],
        ));

        // When: looking up by Repo scope
        let result =
            history.latest_entry_for_scope(HistoryScope::Repo("owner/repo"), "/config/a.yaml");

        // Then: returns None — entry without repo does not match Repo scope
        assert!(
            result.is_none(),
            "directory-mode entry should not match Repo scope lookup"
        );
    }

    #[test]
    fn test_latest_entry_for_scope_same_scope_and_config_returns_most_recent() {
        // Given: two entries with the same (scope, config) — entries are in most-recent-first order
        let mut history = NewSessionHistory::default();
        let mut recent = make_dir_entry("/home/user/proj", "/config/a.yaml", vec!["step-new"]);
        recent.selected_at = "2026-06-01T00:00:00Z".to_string();
        let mut older = make_dir_entry("/home/user/proj", "/config/a.yaml", vec!["step-old"]);
        older.selected_at = "2026-01-01T00:00:00Z".to_string();
        // entries[0] is more recent
        history.entries.push(recent);
        history.entries.push(older);

        // When: looking up by scope + config
        let entry = history
            .latest_entry_for_scope(HistoryScope::Directory("/home/user/proj"), "/config/a.yaml");

        // Then: returns the most recent entry (the first one in entries)
        let Some(entry) = entry else {
            panic!("expected Some");
        };
        assert_eq!(
            entry.skipped_steps,
            vec!["step-new"],
            "should return the most-recent-first entry"
        );
    }

    #[test]
    fn test_latest_entry_for_scope_repo_empty_string_never_matches() {
        // Given: an entry with a valid repo
        let mut history = NewSessionHistory::default();
        history
            .entries
            .push(make_repo_entry("owner/repo", "__builtin__", vec!["step-x"]));

        // When: looking up with an empty string as Repo scope
        let result = history.latest_entry_for_scope(HistoryScope::Repo(""), "__builtin__");

        // Then: returns None — empty repo string is always a non-match
        assert!(
            result.is_none(),
            "HistoryScope::Repo(\"\") should never match any entry"
        );
    }

    // ---- record_skip_selection_for_scope ----

    #[test]
    fn test_record_skip_selection_for_scope_updates_in_place_on_directory_match() {
        // Given: history with an existing directory-mode entry
        let mut history = NewSessionHistory::default();
        history.entries.push(make_dir_entry(
            "/home/user/proj",
            "/config/a.yaml",
            vec!["step-old"],
        ));

        // When: recording a new skip selection for the same scope + config
        history.record_skip_selection_for_scope(
            HistoryScope::Directory("/home/user/proj"),
            "/config/a.yaml",
            vec!["step-new".to_string()],
        );

        // Then: the existing entry is updated in-place (no new entry is added)
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].skipped_steps, vec!["step-new"]);
    }

    #[test]
    fn test_record_skip_selection_for_scope_inserts_new_directory_entry_with_working_dir() {
        // Given: empty history
        let mut history = NewSessionHistory::default();

        // When: recording a new skip selection for a new directory scope
        history.record_skip_selection_for_scope(
            HistoryScope::Directory("/home/user/proj"),
            "/config/a.yaml",
            vec!["step-x".to_string()],
        );

        // Then: a new entry is inserted with working_dir set and repo absent
        assert_eq!(history.entries.len(), 1);
        let entry = &history.entries[0];
        assert_eq!(entry.resolved_config_key, "/config/a.yaml");
        assert_eq!(entry.skipped_steps, vec!["step-x"]);
        assert_eq!(
            entry.repo, None,
            "Directory scope entry must not have repo set"
        );
        assert!(
            !entry.working_dir.is_empty(),
            "Directory scope entry must have working_dir set"
        );
    }

    #[test]
    fn test_record_skip_selection_for_scope_inserts_new_repo_entry_with_repo_field() {
        // Given: empty history
        let mut history = NewSessionHistory::default();

        // When: recording a new skip selection for a new repo scope
        history.record_skip_selection_for_scope(
            HistoryScope::Repo("owner/myrepo"),
            "__builtin__",
            vec!["step-y".to_string()],
        );

        // Then: a new entry is inserted with repo set and working_dir empty
        assert_eq!(history.entries.len(), 1);
        let entry = &history.entries[0];
        assert_eq!(entry.resolved_config_key, "__builtin__");
        assert_eq!(entry.skipped_steps, vec!["step-y"]);
        assert_eq!(
            entry.repo.as_deref(),
            Some("owner/myrepo"),
            "Repo scope entry must have repo set"
        );
        assert_eq!(
            entry.working_dir, "",
            "Repo scope entry must have empty working_dir"
        );
    }

    #[test]
    fn test_record_skip_selection_for_scope_ignores_temp_directory() {
        // Given: empty history
        let mut history = NewSessionHistory::default();

        // When: recording skip selection with a temp directory as the Directory scope
        history.record_skip_selection_for_scope(
            HistoryScope::Directory("/tmp/some-session"),
            "/config/a.yaml",
            vec!["step-x".to_string()],
        );

        // Then: no entry is written — temp paths get cleared to "" by record_selection,
        // which would create a dead entry that can never be looked up again.
        assert!(
            history.entries.is_empty(),
            "temp directory scope should not produce any history entry"
        );
    }

    #[test]
    fn test_latest_entry_for_scope_repo_matches_case_insensitively() {
        // Given: an entry recorded with mixed-case repo name
        let mut history = NewSessionHistory::default();
        history.entries.push(make_repo_entry(
            "Owner/Repo",
            "/config/a.yaml",
            vec!["step-x"],
        ));

        // When: looking up with all-lowercase repo name
        let result =
            history.latest_entry_for_scope(HistoryScope::Repo("owner/repo"), "/config/a.yaml");

        // Then: matches because GitHub repo names are case-insensitive
        assert!(
            result.is_some(),
            "Repo scope lookup should match regardless of case"
        );
        assert_eq!(
            result
                .unwrap_or_else(|| panic!("expected Some, got None"))
                .skipped_steps,
            vec!["step-x"]
        );
    }
}
