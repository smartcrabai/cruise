use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};

/// Phase of a session's lifecycle.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum SessionPhase {
    /// Plan has been generated but not yet approved by the user.
    AwaitingApproval,
    Planned,
    Running,
    Completed,
    Failed(String),
    /// Process was interrupted (Ctrl+C or panic) mid-execution; can be resumed.
    Suspended,
}

impl SessionPhase {
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::AwaitingApproval => "Awaiting Approval",
            Self::Planned => "Planned",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed(_) => "Failed",
            Self::Suspended => "Suspended",
        }
    }

    /// Whether this phase allows (re-)execution.
    #[must_use]
    pub fn is_runnable(&self) -> bool {
        matches!(
            self,
            Self::Planned | Self::Running | Self::Failed(_) | Self::Suspended
        )
    }
}

/// Where a session should execute its workflow.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkspaceMode {
    #[default]
    Worktree,
    CurrentBranch,
}

/// Persisted state for a single session.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SessionState {
    /// Session ID (format: `YYYYMMDDHHmmss`).
    pub id: String,
    /// Path to the original repository (base directory).
    pub base_dir: PathBuf,
    /// Current phase of the session.
    pub phase: SessionPhase,
    /// Name of the config file used (display string).
    pub config_source: String,
    /// User input that initiated the session.
    pub input: String,
    /// Generated session title shown in session lists when available.
    #[serde(default)]
    pub title: Option<String>,
    /// The step currently executing (set during run phase).
    pub current_step: Option<String>,
    /// ISO 8601 creation time.
    pub created_at: String,
    /// ISO 8601 completion time (set when Completed or Failed).
    pub completed_at: Option<String>,
    /// Path to the git worktree (set during run phase).
    pub worktree_path: Option<PathBuf>,
    /// Worktree branch name (set during run phase).
    pub worktree_branch: Option<String>,
    /// Where this session should run.
    #[serde(default)]
    pub workspace_mode: WorkspaceMode,
    /// Branch captured for current-branch mode.
    #[serde(default)]
    pub target_branch: Option<String>,
    /// PR URL created after workflow completion.
    #[serde(default)]
    pub pr_url: Option<String>,
    /// Absolute path to the original config file (None for builtin or old sessions).
    #[serde(default)]
    pub config_path: Option<PathBuf>,
    /// ISO 8601 last-updated time (auto-set on every save).
    #[serde(default)]
    pub updated_at: Option<String>,
    /// True when the session is waiting for user input (option step).
    #[serde(default)]
    pub awaiting_input: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStateFingerprint([u8; 32]);

impl SessionStateFingerprint {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self(crate::file_tracker::sha256_digest(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionFileContents {
    Missing,
    Parsed {
        state: Box<SessionState>,
        fingerprint: SessionStateFingerprint,
    },
    Invalid {
        fingerprint: SessionStateFingerprint,
        error: String,
    },
}

impl SessionFileContents {
    #[must_use]
    pub fn fingerprint(&self) -> Option<SessionStateFingerprint> {
        match self {
            Self::Missing => None,
            Self::Parsed { fingerprint, .. } | Self::Invalid { fingerprint, .. } => {
                Some(*fingerprint)
            }
        }
    }
}

impl SessionState {
    #[must_use]
    pub fn new(id: String, base_dir: PathBuf, config_source: String, input: String) -> Self {
        Self {
            id,
            base_dir,
            phase: SessionPhase::AwaitingApproval,
            config_source,
            input,
            title: None,
            current_step: None,
            created_at: current_iso8601(),
            completed_at: None,
            worktree_path: None,
            worktree_branch: None,
            workspace_mode: WorkspaceMode::Worktree,
            target_branch: None,
            pr_url: None,
            config_path: None,
            updated_at: None,
            awaiting_input: false,
        }
    }

    /// Absolute path to the plan file for this session.
    #[must_use]
    pub fn plan_path(&self, sessions_dir: &Path) -> PathBuf {
        sessions_dir.join(&self.id).join("plan.md")
    }

    #[must_use]
    pub fn title_or_input(&self) -> &str {
        self.title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or(&self.input)
    }

    /// Approve the session, transitioning from `AwaitingApproval` to Planned.
    ///
    /// # Panics
    ///
    /// Panics if the session is not in `AwaitingApproval` phase.
    pub fn approve(&mut self) {
        assert!(
            matches!(self.phase, SessionPhase::AwaitingApproval),
            "approve() called on session in '{}' phase",
            self.phase.label()
        );
        self.phase = SessionPhase::Planned;
    }

    /// Resets this session back to `Planned` state so it can be re-executed from scratch.
    ///
    /// Clears: `phase`, `current_step`, `completed_at`, `pr_url`.
    /// Preserves: `worktree_path`, `worktree_branch` (reused on next run).
    pub fn reset_to_planned(&mut self) {
        self.phase = SessionPhase::Planned;
        self.current_step = None;
        self.completed_at = None;
        self.pr_url = None;
    }

    /// Returns a `WorktreeContext` if the session has a valid, existing worktree.
    #[must_use]
    pub fn worktree_context(&self) -> Option<crate::worktree::WorktreeContext> {
        let path = self.worktree_path.as_ref()?;
        let branch = self.worktree_branch.as_ref()?;
        if !path.exists() {
            return None;
        }
        Some(crate::worktree::WorktreeContext {
            path: path.clone(),
            branch: branch.clone(),
            original_dir: self.base_dir.clone(),
        })
    }
}

/// Manages sessions stored under `<base>/sessions/`.
pub struct SessionManager {
    base: PathBuf,
}

impl SessionManager {
    #[must_use]
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    /// Get the sessions directory.
    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.base.join("sessions")
    }

    /// Get the worktrees directory.
    #[must_use]
    pub fn worktrees_dir(&self) -> PathBuf {
        self.base.join("worktrees")
    }

    /// Get the run log path for a session.
    #[must_use]
    pub fn run_log_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(session_id).join("run.log")
    }

    /// Generate a new unique session ID from current UTC time.
    #[must_use]
    pub fn new_session_id() -> String {
        current_timestamp_id()
    }

    /// Create a new session directory and persist the state.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or the state cannot be written.
    pub fn create(&self, state: &SessionState) -> Result<()> {
        let session_dir = self.sessions_dir().join(&state.id);
        std::fs::create_dir_all(&session_dir)?;
        self.save(state)?;
        Ok(())
    }

    /// Load a session by ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the session file does not exist or cannot be parsed.
    pub fn load(&self, id: &str) -> Result<SessionState> {
        let (state, _) = self.load_with_fingerprint(id)?;
        Ok(state)
    }

    /// Persist a session state to disk.
    ///
    /// Automatically sets `updated_at` to the current UTC time before writing.
    ///
    /// # Errors
    ///
    /// Returns an error if the state cannot be serialized or written to disk.
    pub fn save(&self, state: &SessionState) -> Result<()> {
        let mut state = state.clone();
        state.updated_at = Some(current_iso8601());
        self.save_with_fingerprint(&state)?;
        Ok(())
    }

    pub(crate) fn state_path(&self, id: &str) -> PathBuf {
        self.sessions_dir().join(id).join("state.json")
    }

    pub(crate) fn load_with_fingerprint(
        &self,
        id: &str,
    ) -> Result<(SessionState, SessionStateFingerprint)> {
        let path = self.state_path(id);
        let bytes = std::fs::read(&path)
            .map_err(|e| CruiseError::SessionError(format!("failed to load session {id}: {e}")))?;
        let fingerprint = SessionStateFingerprint::from_bytes(&bytes);
        let state = serde_json::from_slice(&bytes)
            .map_err(|e| CruiseError::SessionError(format!("failed to parse session {id}: {e}")))?;
        Ok((state, fingerprint))
    }

    /// Inspect the raw state file for a session without deserializing it fully.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read.
    pub fn inspect_state_file(&self, id: &str) -> Result<SessionFileContents> {
        let path = self.state_path(id);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SessionFileContents::Missing);
            }
            Err(e) => {
                return Err(CruiseError::SessionError(format!(
                    "failed to inspect session {id}: {e}"
                )));
            }
        };
        let fingerprint = SessionStateFingerprint::from_bytes(&bytes);
        match serde_json::from_slice(&bytes) {
            Ok(state) => Ok(SessionFileContents::Parsed {
                state: Box::new(state),
                fingerprint,
            }),
            Err(e) => Ok(SessionFileContents::Invalid {
                fingerprint,
                error: e.to_string(),
            }),
        }
    }

    pub(crate) fn save_with_fingerprint(
        &self,
        state: &SessionState,
    ) -> Result<SessionStateFingerprint> {
        let path = self.state_path(&state.id);
        let json = serde_json::to_vec_pretty(state)
            .map_err(|e| CruiseError::SessionError(format!("serialize error: {e}")))?;
        let fingerprint = SessionStateFingerprint::from_bytes(&json);
        std::fs::write(&path, json)?;
        Ok(fingerprint)
    }

    /// List all sessions sorted by ID ascending (oldest first).
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be read.
    pub fn list(&self) -> Result<Vec<SessionState>> {
        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return Ok(vec![]);
        }
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            match self.load(&id) {
                Ok(state) => sessions.push(state),
                Err(e) => eprintln!("warning: {e}"),
            }
        }
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(sessions)
    }

    /// Return sessions in a runnable phase (pending execution).
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be read.
    pub fn pending(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| s.phase.is_runnable())
            .collect())
    }

    /// Return sessions in the Planned phase only.
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be read.
    #[cfg(test)]
    pub fn planned(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| s.phase == SessionPhase::Planned)
            .collect())
    }

    /// Return sessions eligible for `run --all`: Planned or Suspended.
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be read.
    pub fn run_all_candidates(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| matches!(s.phase, SessionPhase::Planned | SessionPhase::Suspended))
            .collect())
    }

    /// Load the workflow config for a session.
    ///
    /// # Errors
    ///
    /// Returns an error if the config file cannot be read or parsed.
    pub fn load_config(&self, state: &SessionState) -> Result<crate::config::WorkflowConfig> {
        let config_path = state.config_path.clone().unwrap_or_else(|| {
            // Backward-compatible fallback: session-local copy
            self.sessions_dir().join(&state.id).join("config.yaml")
        });
        let yaml = std::fs::read_to_string(&config_path).map_err(|e| {
            CruiseError::Other(format!(
                "failed to read session config {}: {}",
                config_path.display(),
                e
            ))
        })?;
        crate::config::WorkflowConfig::from_yaml(&yaml)
            .map_err(|e| CruiseError::ConfigParseError(e.to_string()))
    }

    /// Delete a session directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be removed.
    pub fn delete(&self, id: &str) -> Result<()> {
        let session_dir = self.sessions_dir().join(id);
        if session_dir.exists() {
            std::fs::remove_dir_all(&session_dir)?;
        }
        Ok(())
    }

    /// Remove Completed sessions whose PR is closed or merged (checked via `gh`).
    ///
    /// # Errors
    ///
    /// Returns an error if the session list cannot be read or a session cannot be deleted.
    pub fn cleanup_by_pr_status(&self) -> Result<CleanupReport> {
        let sessions = self.list()?;
        let mut report = CleanupReport::default();

        for session in sessions {
            if !matches!(session.phase, SessionPhase::Completed) {
                continue;
            }
            let Some(ref pr_url) = session.pr_url else {
                // No PR URL recorded — skip silently.
                continue;
            };

            // Check PR state via gh CLI.
            let output = std::process::Command::new("gh")
                .args(["pr", "view", pr_url, "--json", "state", "--jq", ".state"])
                .output();

            let state = match output {
                Ok(out) if out.status.success() => {
                    let raw = String::from_utf8_lossy(&out.stdout);
                    raw.trim().to_uppercase()
                }
                Ok(out) => {
                    eprintln!(
                        "warning: gh pr view failed for {}: {}",
                        session.id,
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                    report.skipped += 1;
                    continue;
                }
                Err(e) => {
                    eprintln!("warning: failed to run gh for {}: {}", session.id, e);
                    report.skipped += 1;
                    continue;
                }
            };

            if state != "CLOSED" && state != "MERGED" {
                report.skipped += 1;
                continue;
            }

            // Remove the git worktree if it still exists.
            if let Some(ctx) = session.worktree_context()
                && let Err(e) = crate::worktree::cleanup_worktree(&ctx)
            {
                eprintln!(
                    "warning: failed to remove worktree for {}: {}",
                    session.id, e
                );
            }

            self.delete(&session.id)?;
            report.deleted += 1;
        }

        Ok(report)
    }
}

#[derive(Default)]
pub struct CleanupReport {
    pub deleted: usize,
    pub skipped: usize,
}

/// Get the cruise home directory: `~/.cruise/`
#[must_use]
pub fn cruise_home() -> Option<PathBuf> {
    home::home_dir().map(|h| h.join(".cruise"))
}

/// Get the cruise home directory or return an error.
///
/// # Errors
///
/// Returns an error if the home directory cannot be determined.
pub fn get_cruise_home() -> crate::error::Result<PathBuf> {
    cruise_home()
        .ok_or_else(|| crate::error::CruiseError::Other("home directory not found".to_string()))
}

/// Generate a session ID from current UTC time: `YYYYMMDDHHmmss`.
#[must_use]
pub fn current_timestamp_id() -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, h, m, s) = seconds_to_datetime(secs);
    format!("{year:04}{month:02}{day:02}{h:02}{m:02}{s:02}")
}

/// Format current UTC time as ISO 8601 (`YYYY-MM-DDTHH:MM:SSZ`).
#[must_use]
pub fn current_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, h, m, s) = seconds_to_datetime(secs);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Appends timestamped log lines to `<sessions_dir>/<session_id>/run.log`.
pub struct SessionLogger {
    path: std::path::PathBuf,
}

impl SessionLogger {
    #[must_use]
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    pub fn write(&self, line: &str) {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let ts = current_iso8601();
            let _ = writeln!(file, "[{ts}] {line}");
        }
    }
}

/// Parse an ISO 8601 string (`YYYY-MM-DDTHH:MM:SSZ`) to Unix seconds.
#[cfg(test)]
fn parse_iso8601_secs(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let (date_str, time_str) = s.split_once('T')?;
    let mut dp = date_str.split('-');
    let year: u16 = dp.next()?.parse().ok()?;
    let month: u8 = dp.next()?.parse().ok()?;
    let day: u8 = dp.next()?.parse().ok()?;
    let mut tp = time_str.split(':');
    let h: u64 = tp.next()?.parse().ok()?;
    let m: u64 = tp.next()?.parse().ok()?;
    let s_val: u64 = tp.next()?.parse().ok()?;
    let days = u64::from(date_to_days(year, month, day));
    Some(days * 86400 + h * 3600 + m * 60 + s_val)
}

#[cfg(test)]
fn date_to_days(year: u16, month: u8, day: u8) -> u32 {
    let mut days = 0u32;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    let months = months_in_year(year);
    for month_days in months.iter().take(month as usize - 1) {
        days += u32::from(*month_days);
    }
    days + u32::from(day) - 1
}

fn seconds_to_datetime(secs: u64) -> (u16, u8, u8, u8, u8, u8) {
    let sec = (secs % 60) as u8;
    let min = ((secs / 60) % 60) as u8;
    let hour = ((secs / 3600) % 24) as u8;
    let mut days = secs / 86400;
    let mut year = 1970u16;
    loop {
        let days_in_year = if is_leap_year(year) { 366u64 } else { 365u64 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let months = months_in_year(year);
    let mut month = 1u8;
    for &dim in &months {
        if days < u64::from(dim) {
            break;
        }
        days -= u64::from(dim);
        month += 1;
    }
    let day = u8::try_from(days + 1).unwrap_or(31);
    (year, month, day, hour, min, sec)
}

fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn months_in_year(year: u16) -> [u8; 12] {
    [
        31,
        if is_leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CruiseError;
    use tempfile::TempDir;

    #[test]
    fn test_timestamp_id_format() {
        let id = current_timestamp_id();
        assert_eq!(id.len(), 14);
        assert!(id.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_iso8601_format() {
        let ts = current_iso8601();
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
        assert_eq!(ts.len(), 20);
    }

    #[test]
    fn test_parse_iso8601_roundtrip() {
        // 2026-03-06T14:30:00Z
        let secs = 1_741_270_200_u64;
        let (year, month, day, h, m, s) = seconds_to_datetime(secs);
        let iso = format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z");
        let parsed = parse_iso8601_secs(&iso).unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(parsed, secs);
    }

    #[test]
    fn test_parse_iso8601_known_date() {
        // 2026-03-06T00:00:00Z = days from 1970-01-01 × 86400
        let secs =
            parse_iso8601_secs("2026-03-06T00:00:00Z").unwrap_or_else(|| panic!("unexpected None"));
        let (year, month, day, h, m, s) = seconds_to_datetime(secs);
        assert_eq!(year, 2026);
        assert_eq!(month, 3);
        assert_eq!(day, 6);
        assert_eq!(h, 0);
        assert_eq!(m, 0);
        assert_eq!(s, 0);
    }

    #[test]
    fn test_session_create_and_load() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306143000".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "add hello world".to_string(),
        );
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));
        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.input, "add hello world");
        assert!(matches!(loaded.phase, SessionPhase::AwaitingApproval));
        assert!(loaded.current_step.is_none());
        assert_eq!(loaded.title, None);
        assert_eq!(loaded.workspace_mode, WorkspaceMode::Worktree);
        assert_eq!(loaded.target_branch, None);
        assert!(loaded.pr_url.is_none());
    }

    #[test]
    fn test_session_save_updates_state() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306150000".to_string();
        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        state.phase = SessionPhase::Running;
        state.current_step = Some("implement".to_string());
        manager.save(&state).unwrap_or_else(|e| panic!("{e:?}"));

        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(matches!(loaded.phase, SessionPhase::Running));
        assert_eq!(loaded.current_step, Some("implement".to_string()));
    }

    #[test]
    fn test_load_with_fingerprint_matches_inspected_file() {
        // Given: a persisted session state file
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260310130000".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        // When: loading with fingerprint and inspecting the same file
        let (loaded, load_fingerprint) = manager
            .load_with_fingerprint(&id)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let inspected = manager
            .inspect_state_file(&id)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: both APIs observe the same parsed state and fingerprint.
        // Note: save() auto-sets updated_at, so loaded.updated_at is Some(...).
        assert_eq!(loaded.id, state.id);
        assert_eq!(loaded.phase, state.phase);
        assert!(loaded.updated_at.is_some());
        match inspected {
            SessionFileContents::Parsed {
                state: inspected_state,
                fingerprint,
            } => {
                assert_eq!(*inspected_state, loaded);
                assert_eq!(fingerprint, load_fingerprint);
            }
            other => panic!("expected parsed contents, got {other:?}"),
        }
    }

    #[test]
    fn test_save_with_fingerprint_round_trips_through_load_with_fingerprint() {
        // Given: a session state to persist via the fingerprint-aware API
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let mut state = SessionState::new(
            "20260310130001".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        state.phase = SessionPhase::Running;
        state.current_step = Some("write-test-first".to_string());

        // When: saving and then loading with fingerprints
        std::fs::create_dir_all(manager.sessions_dir().join(&state.id))
            .unwrap_or_else(|e| panic!("{e:?}"));
        let saved_fingerprint = manager
            .save_with_fingerprint(&state)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let (loaded, loaded_fingerprint) = manager
            .load_with_fingerprint(&state.id)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the state and fingerprint round-trip exactly
        assert_eq!(loaded, state);
        assert_eq!(loaded_fingerprint, saved_fingerprint);
    }

    #[test]
    fn test_inspect_state_file_returns_invalid_for_malformed_json() {
        // Given: a malformed state.json on disk
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260310130002";
        let session_dir = manager.sessions_dir().join(id);
        std::fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(session_dir.join("state.json"), "{not valid json")
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: inspecting the state file
        let inspected = manager
            .inspect_state_file(id)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: invalid contents are returned with an error and a consistent fingerprint
        match inspected {
            SessionFileContents::Invalid { fingerprint, error } => {
                assert!(
                    !error.is_empty(),
                    "invalid JSON inspection should include a parse error"
                );
                assert_eq!(
                    Some(fingerprint),
                    manager
                        .inspect_state_file(id)
                        .unwrap_or_else(|e| panic!("{e:?}"))
                        .fingerprint()
                );
            }
            other => panic!("expected invalid contents, got {other:?}"),
        }
    }

    #[test]
    fn test_inspect_state_file_returns_missing_for_absent_file() {
        // Given: a session directory without a state.json
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        // When: inspecting a missing state file
        let inspected = manager
            .inspect_state_file("20260310130003")
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the file is reported as missing without error
        assert_eq!(inspected, SessionFileContents::Missing);
        assert_eq!(inspected.fingerprint(), None);
    }

    #[test]
    fn test_session_list_sorted() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        for id in ["20260306100000", "20260306120000", "20260306090000"] {
            let state = SessionState::new(
                id.to_string(),
                PathBuf::from("/repo"),
                "cruise.yaml".to_string(),
                "task".to_string(),
            );
            manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));
        }
        let sessions = manager.list().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].id, "20260306090000");
        assert_eq!(sessions[1].id, "20260306100000");
        assert_eq!(sessions[2].id, "20260306120000");
    }

    #[test]
    fn test_session_list_empty() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let sessions = manager.list().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_session_pending_filters() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let mut planned = SessionState::new(
            "20260306100000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task1".to_string(),
        );
        planned.phase = SessionPhase::Planned;
        manager.create(&planned).unwrap_or_else(|e| panic!("{e:?}"));

        let mut completed = SessionState::new(
            "20260306110000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task2".to_string(),
        );
        completed.phase = SessionPhase::Completed;
        manager
            .create(&completed)
            .unwrap_or_else(|e| panic!("{e:?}"));

        let mut failed = SessionState::new(
            "20260306120000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task3".to_string(),
        );
        failed.phase = SessionPhase::Failed("some error".to_string());
        manager.create(&failed).unwrap_or_else(|e| panic!("{e:?}"));

        let mut running = SessionState::new(
            "20260306130000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task4".to_string(),
        );
        running.phase = SessionPhase::Running;
        manager.create(&running).unwrap_or_else(|e| panic!("{e:?}"));

        let pending = manager.pending().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(pending.len(), 3);
        let ids: Vec<&str> = pending.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"20260306100000"), "Planned should be pending");
        assert!(ids.contains(&"20260306120000"), "Failed should be pending");
        assert!(ids.contains(&"20260306130000"), "Running should be pending");
        assert!(
            !ids.contains(&"20260306110000"),
            "Completed should not be pending"
        );
    }

    #[test]
    fn test_session_delete() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306100000".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(manager.sessions_dir().join(&id).exists());

        manager.delete(&id).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!manager.sessions_dir().join(&id).exists());

        let sessions = manager.list().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_session_state_pr_url_roundtrip() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306160000".to_string();
        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        state.phase = SessionPhase::Completed;
        state.pr_url = Some("https://github.com/owner/repo/pull/42".to_string());
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            loaded.pr_url,
            Some("https://github.com/owner/repo/pull/42".to_string())
        );
    }

    #[test]
    fn test_session_state_title_roundtrip() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306165000".to_string();
        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        state.title = Some("Readable generated title".to_string());
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(loaded.title.as_deref(), Some("Readable generated title"));
    }

    #[test]
    fn test_session_state_backward_compat() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306170000".to_string();

        // Write a state.json without the pr_url field (simulating old format).
        let session_dir = manager.sessions_dir().join(&id);
        std::fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let json = serde_json::json!({
            "id": id,
            "base_dir": "/repo",
            "phase": "Planned",
            "config_source": "cruise.yaml",
            "input": "old task",
            "current_step": null,
            "created_at": "2026-03-06T17:00:00Z",
            "completed_at": null,
            "worktree_path": null,
            "worktree_branch": null
        });
        std::fs::write(session_dir.join("state.json"), json.to_string())
            .unwrap_or_else(|e| panic!("{e:?}"));

        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(loaded.workspace_mode, WorkspaceMode::Worktree);
        assert_eq!(loaded.target_branch, None);
        assert_eq!(loaded.pr_url, None);
        assert_eq!(loaded.title, None);
        assert_eq!(loaded.input, "old task");
    }

    #[test]
    fn test_session_state_target_branch_roundtrip() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306180000".to_string();
        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.target_branch = Some("feature/direct-mode".to_string());
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(loaded.workspace_mode, WorkspaceMode::CurrentBranch);
        assert_eq!(loaded.target_branch.as_deref(), Some("feature/direct-mode"));
    }

    #[test]
    fn test_session_planned_returns_only_planned() {
        // Given: sessions exist in each phase: Planned / Completed / Failed / Running
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let mut planned = SessionState::new(
            "20260308100000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "planned-task".to_string(),
        );
        planned.phase = SessionPhase::Planned;
        manager.create(&planned).unwrap_or_else(|e| panic!("{e:?}"));

        let mut completed = SessionState::new(
            "20260308110000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "completed-task".to_string(),
        );
        completed.phase = SessionPhase::Completed;
        manager
            .create(&completed)
            .unwrap_or_else(|e| panic!("{e:?}"));

        let mut failed = SessionState::new(
            "20260308120000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "failed-task".to_string(),
        );
        failed.phase = SessionPhase::Failed("error".to_string());
        manager.create(&failed).unwrap_or_else(|e| panic!("{e:?}"));

        let mut running = SessionState::new(
            "20260308130000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "running-task".to_string(),
        );
        running.phase = SessionPhase::Running;
        manager.create(&running).unwrap_or_else(|e| panic!("{e:?}"));

        // When: calling planned()
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: only sessions in the Planned phase are returned
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "20260308100000");
        assert!(matches!(result[0].phase, SessionPhase::Planned));
    }

    #[test]
    fn test_session_planned_empty_when_none_planned() {
        // Given: no Planned sessions exist (only Completed)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let mut completed = SessionState::new(
            "20260308200000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "done".to_string(),
        );
        completed.phase = SessionPhase::Completed;
        manager
            .create(&completed)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: calling planned()
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: an empty list is returned
        assert!(result.is_empty());
    }

    #[test]
    fn test_session_planned_multiple_planned() {
        // Given: multiple Planned sessions exist
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        for (id, input) in [
            ("20260308300000", "task-a"),
            ("20260308310000", "task-b"),
            ("20260308320000", "task-c"),
        ] {
            let mut s = SessionState::new(
                id.to_string(),
                PathBuf::from("/repo"),
                "cruise.yaml".to_string(),
                input.to_string(),
            );
            s.phase = SessionPhase::Planned;
            manager.create(&s).unwrap_or_else(|e| panic!("{e:?}"));
        }

        // When: calling planned()
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: all Planned sessions are returned
        assert_eq!(result.len(), 3);
        let ids: Vec<&str> = result.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"20260308300000"));
        assert!(ids.contains(&"20260308310000"));
        assert!(ids.contains(&"20260308320000"));
    }

    #[test]
    fn test_session_load_config_reads_valid_yaml() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260309120000".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        let yaml = "command:\n  - echo\nsteps:\n  test:\n    command: \"true\"\n";
        std::fs::write(manager.sessions_dir().join(&id).join("config.yaml"), yaml)
            .unwrap_or_else(|e| panic!("{e:?}"));

        let config = manager
            .load_config(&state)
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(config.command, vec!["echo".to_string()]);
        assert!(config.steps.contains_key("test"));
    }

    #[test]
    fn test_session_load_config_invalid_yaml_returns_parse_error() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260309120001".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        std::fs::write(
            manager.sessions_dir().join(&id).join("config.yaml"),
            "command:\n  - echo\nsteps: [",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let err = manager
            .load_config(&state)
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));

        assert!(matches!(err, CruiseError::ConfigParseError(_)));
    }

    // -----------------------------------------------------------------------
    // SessionState::reset_to_planned
    // -----------------------------------------------------------------------

    fn make_completed_session() -> SessionState {
        let mut s = SessionState::new(
            "20260309100000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "some task".to_string(),
        );
        s.phase = SessionPhase::Completed;
        s.current_step = Some("final-step".to_string());
        s.completed_at = Some("2026-03-09T10:00:00Z".to_string());
        s.pr_url = Some("https://github.com/owner/repo/pull/42".to_string());
        s.worktree_path = Some(PathBuf::from("/tmp/worktree"));
        s.worktree_branch = Some("cruise/20260309100000-some-task".to_string());
        s
    }

    #[test]
    fn test_reset_to_planned_from_completed() {
        // Given: a fully populated Completed session
        let mut s = make_completed_session();
        let orig_id = s.id.clone();
        let orig_input = s.input.clone();
        let orig_created_at = s.created_at.clone();
        let orig_base_dir = s.base_dir.clone();
        let orig_config_source = s.config_source.clone();

        // When
        s.reset_to_planned();

        // Then: execution state fields are cleared, identity/worktree are preserved
        assert!(matches!(s.phase, SessionPhase::Planned));
        assert!(s.current_step.is_none());
        assert!(s.completed_at.is_none());
        assert!(s.pr_url.is_none());
        assert_eq!(s.worktree_path, Some(PathBuf::from("/tmp/worktree")));
        assert_eq!(
            s.worktree_branch,
            Some("cruise/20260309100000-some-task".to_string())
        );
        assert_eq!(s.id, orig_id);
        assert_eq!(s.input, orig_input);
        assert_eq!(s.created_at, orig_created_at);
        assert_eq!(s.base_dir, orig_base_dir);
        assert_eq!(s.config_source, orig_config_source);
    }

    #[test]
    fn test_reset_to_planned_from_running() {
        // Given: a session in the Running phase
        let mut s = SessionState::new(
            "20260309110000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "running task".to_string(),
        );
        s.phase = SessionPhase::Running;
        s.current_step = Some("implement".to_string());
        s.worktree_path = Some(PathBuf::from("/tmp/wt2"));
        s.worktree_branch = Some("cruise/20260309110000-running-task".to_string());

        // When
        s.reset_to_planned();

        // Then: reverts to Planned, execution state is cleared, worktree is preserved
        assert!(matches!(s.phase, SessionPhase::Planned));
        assert!(s.current_step.is_none());
        assert!(s.completed_at.is_none());
        assert_eq!(s.worktree_path, Some(PathBuf::from("/tmp/wt2")));
        assert_eq!(
            s.worktree_branch,
            Some("cruise/20260309110000-running-task".to_string())
        );
    }

    // -----------------------------------------------------------------------
    // SessionPhase::Suspended — basic properties
    // -----------------------------------------------------------------------

    #[test]
    fn test_suspended_phase_label() {
        // Given: Suspended phase
        let phase = SessionPhase::Suspended;

        // When
        let label = phase.label();

        // Then: returns "Suspended"
        assert_eq!(label, "Suspended");
    }

    #[test]
    fn test_suspended_is_runnable() {
        // Given: Suspended phase
        let phase = SessionPhase::Suspended;

        // When / Then: is_runnable() = true because it can be resumed
        assert!(
            phase.is_runnable(),
            "Suspended should be runnable (resumable)"
        );
    }

    #[test]
    fn test_suspended_serialize_deserialize_roundtrip() {
        // Given: save a session with Suspended phase and a current_step
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260310100000".to_string();
        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        state.phase = SessionPhase::Suspended;
        state.current_step = Some("implement".to_string());
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        // When: reloading from disk
        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the phase and current_step are correctly restored
        assert!(
            matches!(loaded.phase, SessionPhase::Suspended),
            "phase should be Suspended after roundtrip"
        );
        assert_eq!(loaded.current_step, Some("implement".to_string()));
    }

    #[test]
    fn test_pending_includes_suspended() {
        // Given: sessions exist in each phase: Suspended / Completed
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let mut suspended = SessionState::new(
            "20260310110000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "suspended-task".to_string(),
        );
        suspended.phase = SessionPhase::Suspended;
        manager
            .create(&suspended)
            .unwrap_or_else(|e| panic!("{e:?}"));

        let mut completed = SessionState::new(
            "20260310120000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "completed-task".to_string(),
        );
        completed.phase = SessionPhase::Completed;
        manager
            .create(&completed)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: calling pending()
        let pending = manager.pending().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: Suspended is included in pending, Completed is not
        let ids: Vec<&str> = pending.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.contains(&"20260310110000"),
            "Suspended should be in pending"
        );
        assert!(
            !ids.contains(&"20260310120000"),
            "Completed should not be in pending"
        );
    }

    // -----------------------------------------------------------------------
    // SessionManager::run_all_candidates
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_all_candidates_returns_planned_and_suspended_only() {
        // Given: sessions exist in all phases
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        for (id, phase) in [
            ("20260310200000", SessionPhase::Planned),
            ("20260310200001", SessionPhase::Suspended),
            ("20260310200002", SessionPhase::Running),
            ("20260310200003", SessionPhase::Completed),
            ("20260310200004", SessionPhase::Failed("err".to_string())),
        ] {
            let mut s = SessionState::new(
                id.to_string(),
                PathBuf::from("/repo"),
                "cruise.yaml".to_string(),
                "task".to_string(),
            );
            s.phase = phase;
            manager.create(&s).unwrap_or_else(|e| panic!("{e:?}"));
        }

        // When: calling run_all_candidates()
        let candidates = manager
            .run_all_candidates()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: only Planned and Suspended are returned
        assert_eq!(
            candidates.len(),
            2,
            "only Planned and Suspended should be candidates"
        );
        let ids: Vec<&str> = candidates.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.contains(&"20260310200000"),
            "Planned should be included"
        );
        assert!(
            ids.contains(&"20260310200001"),
            "Suspended should be included"
        );
        assert!(
            !ids.contains(&"20260310200002"),
            "Running should NOT be included"
        );
        assert!(
            !ids.contains(&"20260310200003"),
            "Completed should NOT be included"
        );
        assert!(
            !ids.contains(&"20260310200004"),
            "Failed should NOT be included"
        );
    }

    #[test]
    fn test_run_all_candidates_empty_when_none_qualify() {
        // Given: only Completed sessions exist
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let mut s = SessionState::new(
            "20260310210000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "done".to_string(),
        );
        s.phase = SessionPhase::Completed;
        manager.create(&s).unwrap_or_else(|e| panic!("{e:?}"));

        // When: calling run_all_candidates()
        let candidates = manager
            .run_all_candidates()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: an empty list is returned
        assert!(
            candidates.is_empty(),
            "no candidates when only Completed exists"
        );
    }

    #[test]
    fn test_reset_to_planned_preserves_workspace_mode_and_target_branch() {
        // Given: a Running session in CurrentBranch mode with target_branch set
        let mut s = SessionState::new(
            "20260310120000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "direct mode task".to_string(),
        );
        s.phase = SessionPhase::Running;
        s.current_step = Some("implement".to_string());
        s.workspace_mode = WorkspaceMode::CurrentBranch;
        s.target_branch = Some("feature/my-branch".to_string());

        // When
        s.reset_to_planned();

        // Then: execution state is cleared but workspace_mode / target_branch are preserved
        assert!(matches!(s.phase, SessionPhase::Planned));
        assert!(s.current_step.is_none());
        assert_eq!(s.workspace_mode, WorkspaceMode::CurrentBranch);
        assert_eq!(s.target_branch.as_deref(), Some("feature/my-branch"));
    }

    // -----------------------------------------------------------------------
    // SessionPhase::AwaitingApproval
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_new_starts_in_awaiting_approval() {
        // Given / When: creating a new session
        let s = SessionState::new(
            "20260311100000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "some task".to_string(),
        );

        // Then: starts in the AwaitingApproval phase (not Planned)
        assert!(
            matches!(s.phase, SessionPhase::AwaitingApproval),
            "new session should start in AwaitingApproval, got {:?}",
            s.phase
        );
    }

    #[test]
    fn test_awaiting_approval_is_not_runnable() {
        // Given: AwaitingApproval phase
        let phase = SessionPhase::AwaitingApproval;

        // When / Then: is_runnable() returns false
        assert!(
            !phase.is_runnable(),
            "AwaitingApproval should not be runnable"
        );
    }

    #[test]
    fn test_awaiting_approval_label_is_distinct() {
        // Given: AwaitingApproval phase
        let phase = SessionPhase::AwaitingApproval;

        // When / Then: returns a clear label that does not overlap with other phases
        let label = phase.label();
        assert_eq!(label, "Awaiting Approval");
        assert_ne!(label, SessionPhase::Planned.label());
        assert_ne!(label, SessionPhase::Running.label());
        assert_ne!(label, SessionPhase::Completed.label());
        assert_ne!(label, SessionPhase::Failed("x".to_string()).label());
    }

    #[test]
    fn test_pending_excludes_awaiting_approval() {
        // Given: sessions exist in each phase: AwaitingApproval / Planned / Running / Failed / Completed
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let awaiting = SessionState::new(
            "20260311200000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "unapproved".to_string(),
        );
        // SessionState::new creates in AwaitingApproval phase
        manager
            .create(&awaiting)
            .unwrap_or_else(|e| panic!("{e:?}"));

        let mut planned = SessionState::new(
            "20260311200001".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "approved".to_string(),
        );
        planned.phase = SessionPhase::Planned;
        manager.create(&planned).unwrap_or_else(|e| panic!("{e:?}"));

        let mut running = SessionState::new(
            "20260311200002".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "running".to_string(),
        );
        running.phase = SessionPhase::Running;
        manager.create(&running).unwrap_or_else(|e| panic!("{e:?}"));

        // When: calling pending()
        let pending = manager.pending().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: AwaitingApproval is not included; Planned and Running are included
        let ids: Vec<&str> = pending.iter().map(|s| s.id.as_str()).collect();
        assert!(
            !ids.contains(&"20260311200000"),
            "AwaitingApproval should NOT be in pending: {ids:?}"
        );
        assert!(
            ids.contains(&"20260311200001"),
            "Planned should be in pending: {ids:?}"
        );
        assert!(
            ids.contains(&"20260311200002"),
            "Running should be in pending: {ids:?}"
        );
    }

    #[test]
    fn test_planned_excludes_awaiting_approval() {
        // Given: both AwaitingApproval and Planned sessions exist
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        // AwaitingApproval session (default of SessionState::new)
        let awaiting = SessionState::new(
            "20260311300000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "not yet approved".to_string(),
        );
        manager
            .create(&awaiting)
            .unwrap_or_else(|e| panic!("{e:?}"));

        let mut approved = SessionState::new(
            "20260311300001".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "approved task".to_string(),
        );
        approved.phase = SessionPhase::Planned;
        manager
            .create(&approved)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: calling planned()
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: only Planned is returned, AwaitingApproval is not included
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "20260311300001");
        assert!(
            !result.iter().any(|s| s.id == "20260311300000"),
            "AwaitingApproval should NOT appear in planned()"
        );
    }

    #[test]
    fn test_awaiting_approval_session_roundtrip() {
        // Given: persisting a session in the AwaitingApproval phase
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260311400000".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "pending approval".to_string(),
        );
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        // When: loading
        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the AwaitingApproval phase is correctly deserialized
        assert!(
            matches!(loaded.phase, SessionPhase::AwaitingApproval),
            "loaded phase should be AwaitingApproval, got {:?}",
            loaded.phase
        );
    }

    #[test]
    fn test_approve_from_awaiting_approval() {
        // Given: a session in the AwaitingApproval phase
        let mut s = SessionState::new(
            "20260311500000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        assert!(matches!(s.phase, SessionPhase::AwaitingApproval));

        // When: calling approve()
        s.approve();

        // Then: the phase becomes Planned
        assert!(
            matches!(s.phase, SessionPhase::Planned),
            "approve should set phase to Planned, got {:?}",
            s.phase
        );
    }

    // --- tests for the config_path field & load_config changes ---

    #[test]
    fn test_session_state_config_path_defaults_to_none_on_new() {
        // Given/When: creating a SessionState::new() without arguments
        let state = SessionState::new(
            "20260314120000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        // Then: config_path is None
        assert!(state.config_path.is_none());
    }

    #[test]
    fn test_session_state_backward_compat_config_path_none() {
        // Given: legacy JSON format that does not contain the config_path field
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260314120002".to_string();
        let session_dir = manager.sessions_dir().join(&id);
        std::fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        // legacy format without config_path field
        let json = serde_json::json!({
            "id": id,
            "base_dir": "/repo",
            "phase": "Planned",
            "config_source": "cruise.yaml",
            "input": "old task",
            "current_step": null,
            "created_at": "2026-03-14T12:00:00Z",
            "completed_at": null,
            "worktree_path": null,
            "worktree_branch": null
        });
        std::fs::write(session_dir.join("state.json"), json.to_string())
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: loading the legacy JSON format
        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: config_path defaults to None
        assert!(
            loaded.config_path.is_none(),
            "config_path should default to None for old sessions"
        );
    }

    #[test]
    fn test_session_load_config_reads_from_config_path_when_set() {
        // Given: a session with a config_path pointing to an external file
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260314120003".to_string();

        // create an external YAML file
        let config_file = tmp.path().join("external_cruise.yaml");
        let yaml = "command:\n  - cat\nsteps:\n  check:\n    command: \"true\"\n";
        std::fs::write(&config_file, yaml).unwrap_or_else(|e| panic!("{e:?}"));

        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            config_file.display().to_string(),
            "task".to_string(),
        );
        state.config_path = Some(config_file);
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        // When: loading from config_path
        let config = manager
            .load_config(&state)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the contents of the external file are loaded
        assert_eq!(config.command, vec!["cat".to_string()]);
        assert!(config.steps.contains_key("check"));
    }

    #[test]
    fn test_session_load_config_falls_back_to_session_dir_when_config_path_none() {
        // Given: a session with config_path as None (backward-compatible fallback)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260314120004".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        // config_path remains None
        assert!(state.config_path.is_none());
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        // write to config.yaml in the session directory as a fallback
        let yaml = "command:\n  - bash\nsteps:\n  fallback_step:\n    command: \"true\"\n";
        std::fs::write(manager.sessions_dir().join(&id).join("config.yaml"), yaml)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: calling load_config
        let config = manager
            .load_config(&state)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the fallback session directory config.yaml is read
        assert_eq!(config.command, vec!["bash".to_string()]);
        assert!(config.steps.contains_key("fallback_step"));
    }

    #[test]
    fn test_session_load_config_config_path_not_found_returns_error() {
        // Given: a session whose config_path points to a non-existent file
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260314120005".to_string();
        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        state.config_path = Some(PathBuf::from("/nonexistent/cruise.yaml"));
        manager.create(&state).unwrap_or_else(|e| panic!("{e:?}"));

        // When: load_config referencing a non-existent file
        let result = manager.load_config(&state);

        // Then: an error is returned
        assert!(result.is_err());
    }

    #[test]
    fn test_session_logger_creates_file_and_writes_line() {
        // Given: a temp directory and a log path
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let log_path = tmp.path().join("run.log");
        let logger = SessionLogger::new(log_path.clone());

        // When: writing a log line
        logger.write("test message");

        // Then: the file exists and contains the message
        let content = std::fs::read_to_string(&log_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            content.contains("test message"),
            "log should contain 'test message'"
        );
    }

    #[test]
    fn test_session_logger_line_format_has_timestamp_prefix() {
        // Given: a log file path
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let log_path = tmp.path().join("run.log");
        let logger = SessionLogger::new(log_path.clone());

        // When: writing a log line
        logger.write("hello");

        // Then: the line is formatted as "[YYYY-MM-DDTHH:MM:SSZ] hello"
        let content = std::fs::read_to_string(&log_path).unwrap_or_else(|e| panic!("{e:?}"));
        let line = content
            .lines()
            .next()
            .unwrap_or_else(|| panic!("should have at least one line"));
        assert!(line.starts_with('['), "line should start with '['");
        assert!(line.contains("] hello"), "line should contain '] hello'");
    }

    #[test]
    fn test_session_logger_appends_multiple_writes() {
        // Given: a log file path
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let log_path = tmp.path().join("run.log");
        let logger = SessionLogger::new(log_path.clone());

        // When: writing three log lines
        logger.write("line one");
        logger.write("line two");
        logger.write("line three");

        // Then: the file contains all 3 lines in order
        let content = std::fs::read_to_string(&log_path).unwrap_or_else(|e| panic!("{e:?}"));
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "should have 3 lines");
        assert!(
            lines[0].contains("line one"),
            "first line should contain 'line one'"
        );
        assert!(
            lines[1].contains("line two"),
            "second line should contain 'line two'"
        );
        assert!(
            lines[2].contains("line three"),
            "third line should contain 'line three'"
        );
    }

    #[test]
    fn test_session_logger_write_silently_ignores_nonexistent_directory() {
        // Given: a path inside a non-existent directory
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let log_path = tmp.path().join("nonexistent_dir").join("run.log");
        let logger = SessionLogger::new(log_path);

        // When/Then: writing does not panic even if the parent directory doesn't exist
        logger.write("this should not panic");
    }
}
