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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionStateFingerprint([u8; 32]);

impl SessionStateFingerprint {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self(crate::file_tracker::sha256_digest(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionFileContents {
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
    pub(crate) fn fingerprint(&self) -> Option<SessionStateFingerprint> {
        match self {
            Self::Missing => None,
            Self::Parsed { fingerprint, .. } | Self::Invalid { fingerprint, .. } => {
                Some(*fingerprint)
            }
        }
    }
}

impl SessionState {
    pub fn new(id: String, base_dir: PathBuf, config_source: String, input: String) -> Self {
        Self {
            id,
            base_dir,
            phase: SessionPhase::AwaitingApproval,
            config_source,
            input,
            current_step: None,
            created_at: current_iso8601(),
            completed_at: None,
            worktree_path: None,
            worktree_branch: None,
            workspace_mode: WorkspaceMode::Worktree,
            target_branch: None,
            pr_url: None,
        }
    }

    /// Absolute path to the plan file for this session.
    pub fn plan_path(&self, sessions_dir: &Path) -> PathBuf {
        sessions_dir.join(&self.id).join("plan.md")
    }

    /// Approve the session, transitioning from `AwaitingApproval` to Planned.
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
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    /// Get the sessions directory.
    pub fn sessions_dir(&self) -> PathBuf {
        self.base.join("sessions")
    }

    /// Get the worktrees directory.
    pub fn worktrees_dir(&self) -> PathBuf {
        self.base.join("worktrees")
    }

    /// Generate a new unique session ID from current UTC time.
    pub fn new_session_id() -> String {
        current_timestamp_id()
    }

    /// Create a new session directory and persist the state.
    pub fn create(&self, state: &SessionState) -> Result<()> {
        let session_dir = self.sessions_dir().join(&state.id);
        std::fs::create_dir_all(&session_dir)?;
        self.save(state)?;
        Ok(())
    }

    /// Load a session by ID.
    pub fn load(&self, id: &str) -> Result<SessionState> {
        let (state, _) = self.load_with_fingerprint(id)?;
        Ok(state)
    }

    /// Persist a session state to disk.
    pub fn save(&self, state: &SessionState) -> Result<()> {
        self.save_with_fingerprint(state)?;
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

    pub(crate) fn inspect_state_file(&self, id: &str) -> Result<SessionFileContents> {
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
    pub fn pending(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| s.phase.is_runnable())
            .collect())
    }

    /// Return sessions in the Planned phase only.
    #[cfg(test)]
    pub fn planned(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| s.phase == SessionPhase::Planned)
            .collect())
    }

    /// Return sessions eligible for `run --all`: Planned or Suspended.
    pub fn run_all_candidates(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| matches!(s.phase, SessionPhase::Planned | SessionPhase::Suspended))
            .collect())
    }

    /// Load the workflow config for a session.
    pub fn load_config(&self, id: &str) -> Result<crate::config::WorkflowConfig> {
        let config_path = self.sessions_dir().join(id).join("config.yaml");
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
    pub fn delete(&self, id: &str) -> Result<()> {
        let session_dir = self.sessions_dir().join(id);
        if session_dir.exists() {
            std::fs::remove_dir_all(&session_dir)?;
        }
        Ok(())
    }

    /// Remove Completed sessions whose PR is closed or merged (checked via `gh`).
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
pub fn cruise_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".cruise"))
}

/// Get the cruise home directory or return an error.
pub fn get_cruise_home() -> crate::error::Result<PathBuf> {
    cruise_home().ok_or_else(|| crate::error::CruiseError::Other("HOME not set".to_string()))
}

/// Generate a session ID from current UTC time: `YYYYMMDDHHmmss`.
pub fn current_timestamp_id() -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, h, m, s) = seconds_to_datetime(secs);
    format!("{year:04}{month:02}{day:02}{h:02}{m:02}{s:02}")
}

/// Format current UTC time as ISO 8601 (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn current_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, h, m, s) = seconds_to_datetime(secs);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
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

        // Then: both APIs observe the same parsed state and fingerprint
        assert_eq!(loaded, state);
        match inspected {
            SessionFileContents::Parsed { state, fingerprint } => {
                assert_eq!(*state, loaded);
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
        // Given: Planned / Completed / Failed / Running の各フェーズのセッションが存在する
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

        // When: planned() を呼ぶ
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: Planned フェーズのセッションのみ返される
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "20260308100000");
        assert!(matches!(result[0].phase, SessionPhase::Planned));
    }

    #[test]
    fn test_session_planned_empty_when_none_planned() {
        // Given: Planned セッションが存在しない（Completed のみ）
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

        // When: planned() を呼ぶ
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: 空のリストが返される
        assert!(result.is_empty());
    }

    #[test]
    fn test_session_planned_multiple_planned() {
        // Given: 複数の Planned セッションが存在する
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

        // When: planned() を呼ぶ
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: すべての Planned セッションが返される
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

        let config = manager.load_config(&id).unwrap_or_else(|e| panic!("{e:?}"));

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
            .load_config(&id)
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
        // Given: フル状態の Completed セッション
        let mut s = make_completed_session();
        let orig_id = s.id.clone();
        let orig_input = s.input.clone();
        let orig_created_at = s.created_at.clone();
        let orig_base_dir = s.base_dir.clone();
        let orig_config_source = s.config_source.clone();

        // When
        s.reset_to_planned();

        // Then: 実行状態フィールドがクリアされ、identity/worktree は保持
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
        // Given: Running 中のセッション
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

        // Then: Planned に戻り、実行状態はクリア、worktree は保持
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
    // SessionPhase::Suspended — 基本プロパティ
    // -----------------------------------------------------------------------

    #[test]
    fn test_suspended_phase_label() {
        // Given: Suspended フェーズ
        let phase = SessionPhase::Suspended;

        // When
        let label = phase.label();

        // Then: "Suspended" を返す
        assert_eq!(label, "Suspended");
    }

    #[test]
    fn test_suspended_is_runnable() {
        // Given: Suspended フェーズ
        let phase = SessionPhase::Suspended;

        // When / Then: resume 可能なので is_runnable() = true
        assert!(
            phase.is_runnable(),
            "Suspended should be runnable (resumable)"
        );
    }

    #[test]
    fn test_suspended_serialize_deserialize_roundtrip() {
        // Given: Suspended フェーズと current_step を持つセッションを保存
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

        // When: ディスクから再読み込み
        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: フェーズと current_step が正しく復元される
        assert!(
            matches!(loaded.phase, SessionPhase::Suspended),
            "phase should be Suspended after roundtrip"
        );
        assert_eq!(loaded.current_step, Some("implement".to_string()));
    }

    #[test]
    fn test_pending_includes_suspended() {
        // Given: Suspended / Completed の各フェーズのセッションが存在する
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

        // When: pending() を呼ぶ
        let pending = manager.pending().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: Suspended は pending に含まれ、Completed は含まれない
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
        // Given: 全フェーズのセッションが存在する
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

        // When: run_all_candidates() を呼ぶ
        let candidates = manager
            .run_all_candidates()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: Planned と Suspended のみ返される
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
        // Given: Completed セッションのみ存在する
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

        // When: run_all_candidates() を呼ぶ
        let candidates = manager
            .run_all_candidates()
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: 空リストが返される
        assert!(
            candidates.is_empty(),
            "no candidates when only Completed exists"
        );
    }

    #[test]
    fn test_reset_to_planned_preserves_workspace_mode_and_target_branch() {
        // Given: CurrentBranch モード + target_branch 設定済みの Running セッション
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

        // Then: 実行状態はクリアされるが workspace_mode / target_branch は保持
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
        // Given / When: 新規セッションを作成する
        let s = SessionState::new(
            "20260311100000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "some task".to_string(),
        );

        // Then: AwaitingApproval フェーズで開始する（Planned ではない）
        assert!(
            matches!(s.phase, SessionPhase::AwaitingApproval),
            "new session should start in AwaitingApproval, got {:?}",
            s.phase
        );
    }

    #[test]
    fn test_awaiting_approval_is_not_runnable() {
        // Given: AwaitingApproval フェーズ
        let phase = SessionPhase::AwaitingApproval;

        // When / Then: is_runnable() は false を返す
        assert!(
            !phase.is_runnable(),
            "AwaitingApproval should not be runnable"
        );
    }

    #[test]
    fn test_awaiting_approval_label_is_distinct() {
        // Given: AwaitingApproval フェーズ
        let phase = SessionPhase::AwaitingApproval;

        // When / Then: 他のフェーズと重複しない明確なラベルを返す
        let label = phase.label();
        assert_eq!(label, "Awaiting Approval");
        assert_ne!(label, SessionPhase::Planned.label());
        assert_ne!(label, SessionPhase::Running.label());
        assert_ne!(label, SessionPhase::Completed.label());
        assert_ne!(label, SessionPhase::Failed("x".to_string()).label());
    }

    #[test]
    fn test_pending_excludes_awaiting_approval() {
        // Given: AwaitingApproval / Planned / Running / Failed / Completed の各セッションが存在する
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let awaiting = SessionState::new(
            "20260311200000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "unapproved".to_string(),
        );
        // SessionState::new は AwaitingApproval で作成される
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

        // When: pending() を呼ぶ
        let pending = manager.pending().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: AwaitingApproval は含まれない。Planned・Running は含まれる
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
        // Given: AwaitingApproval と Planned の両セッションが存在する
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());

        // AwaitingApproval セッション（SessionState::new のデフォルト）
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

        // When: planned() を呼ぶ
        let result = manager.planned().unwrap_or_else(|e| panic!("{e:?}"));

        // Then: Planned のみ返され、AwaitingApproval は含まれない
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "20260311300001");
        assert!(
            !result.iter().any(|s| s.id == "20260311300000"),
            "AwaitingApproval should NOT appear in planned()"
        );
    }

    #[test]
    fn test_awaiting_approval_session_roundtrip() {
        // Given: AwaitingApproval フェーズのセッションを永続化する
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

        // When: ロードする
        let loaded = manager.load(&id).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: AwaitingApproval フェーズが正しくデシリアライズされる
        assert!(
            matches!(loaded.phase, SessionPhase::AwaitingApproval),
            "loaded phase should be AwaitingApproval, got {:?}",
            loaded.phase
        );
    }

    #[test]
    fn test_approve_from_awaiting_approval() {
        // Given: AwaitingApproval フェーズのセッション
        let mut s = SessionState::new(
            "20260311500000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        assert!(matches!(s.phase, SessionPhase::AwaitingApproval));

        // When: approve() を呼ぶ
        s.approve();

        // Then: Planned フェーズになる
        assert!(
            matches!(s.phase, SessionPhase::Planned),
            "approve should set phase to Planned, got {:?}",
            s.phase
        );
    }
}
