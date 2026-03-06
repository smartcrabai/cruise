use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};

/// Phase of a session's lifecycle.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum SessionPhase {
    Planned,
    Running,
    Completed,
    Failed(String),
}

impl SessionPhase {
    pub fn label(&self) -> &str {
        match self {
            Self::Planned => "Planned",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed(_) => "Failed",
        }
    }
}

/// Persisted state for a single session.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionState {
    /// Session ID (format: YYYYMMDDHHmmss).
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
}

impl SessionState {
    pub fn new(id: String, base_dir: PathBuf, config_source: String, input: String) -> Self {
        Self {
            id,
            base_dir,
            phase: SessionPhase::Planned,
            config_source,
            input,
            current_step: None,
            created_at: current_iso8601(),
            completed_at: None,
            worktree_path: None,
            worktree_branch: None,
        }
    }

    /// Absolute path to the plan file for this session.
    pub fn plan_path(&self, sessions_dir: &Path) -> PathBuf {
        sessions_dir.join(&self.id).join("plan.md")
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
        let path = self.sessions_dir().join(id).join("state.json");
        let json = std::fs::read_to_string(&path)
            .map_err(|e| CruiseError::SessionError(format!("failed to load session {id}: {e}")))?;
        serde_json::from_str(&json)
            .map_err(|e| CruiseError::SessionError(format!("failed to parse session {id}: {e}")))
    }

    /// Persist a session state to disk.
    pub fn save(&self, state: &SessionState) -> Result<()> {
        let session_dir = self.sessions_dir().join(&state.id);
        std::fs::create_dir_all(&session_dir)?;
        let path = session_dir.join("state.json");
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| CruiseError::SessionError(format!("serialize error: {e}")))?;
        std::fs::write(&path, json)?;
        Ok(())
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
                Err(e) => eprintln!("warning: {}", e),
            }
        }
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(sessions)
    }

    /// Return sessions in Planned or Failed phase (pending execution).
    pub fn pending(&self) -> Result<Vec<SessionState>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| matches!(s.phase, SessionPhase::Planned | SessionPhase::Failed(_)))
            .collect())
    }

    /// Delete a session directory.
    pub fn delete(&self, id: &str) -> Result<()> {
        let session_dir = self.sessions_dir().join(id);
        if session_dir.exists() {
            std::fs::remove_dir_all(&session_dir)?;
        }
        Ok(())
    }

    /// Remove Completed sessions older than `days` days (and their worktrees).
    pub fn cleanup_old(&self, days: u64) -> Result<CleanupReport> {
        let now_secs = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let threshold_secs = days * 86400;
        let sessions = self.list()?;
        let mut report = CleanupReport::default();

        for session in sessions {
            if !matches!(session.phase, SessionPhase::Completed) {
                continue;
            }
            let completed_secs = session
                .completed_at
                .as_deref()
                .and_then(parse_iso8601_secs)
                .unwrap_or(0);
            if now_secs.saturating_sub(completed_secs) < threshold_secs {
                continue;
            }

            // Remove the git worktree if it still exists.
            if let (Some(wt_path), Some(wt_branch)) =
                (&session.worktree_path, &session.worktree_branch)
            {
                if wt_path.exists() {
                    let ctx = crate::worktree::WorktreeContext {
                        path: wt_path.clone(),
                        branch: wt_branch.clone(),
                        original_dir: session.base_dir.clone(),
                    };
                    if let Err(e) = crate::worktree::cleanup_worktree(&ctx) {
                        eprintln!(
                            "warning: failed to remove worktree for {}: {}",
                            session.id, e
                        );
                    }
                }
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
    format!("{:04}{:02}{:02}{:02}{:02}{:02}", year, month, day, h, m, s)
}

/// Format current UTC time as ISO 8601 (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn current_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, h, m, s) = seconds_to_datetime(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, h, m, s
    )
}

/// Parse an ISO 8601 string (`YYYY-MM-DDTHH:MM:SSZ`) to Unix seconds.
fn parse_iso8601_secs(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let mut parts = s.splitn(2, 'T');
    let date_str = parts.next()?;
    let time_str = parts.next()?;
    let mut dp = date_str.split('-');
    let year: u16 = dp.next()?.parse().ok()?;
    let month: u8 = dp.next()?.parse().ok()?;
    let day: u8 = dp.next()?.parse().ok()?;
    let mut tp = time_str.split(':');
    let h: u64 = tp.next()?.parse().ok()?;
    let m: u64 = tp.next()?.parse().ok()?;
    let s_val: u64 = tp.next()?.parse().ok()?;
    let days = date_to_days(year, month, day) as u64;
    Some(days * 86400 + h * 3600 + m * 60 + s_val)
}

fn date_to_days(year: u16, month: u8, day: u8) -> u32 {
    let mut days = 0u32;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    let months = months_in_year(year);
    for m in 0..(month as usize - 1) {
        days += months[m] as u32;
    }
    days + day as u32 - 1
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
        if days < dim as u64 {
            break;
        }
        days -= dim as u64;
        month += 1;
    }
    let day = (days + 1) as u8;
    (year, month, day, hour, min, sec)
}

fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
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
        let secs = 1741270200u64;
        let (year, month, day, h, m, s) = seconds_to_datetime(secs);
        let iso = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            year, month, day, h, m, s
        );
        let parsed = parse_iso8601_secs(&iso).unwrap();
        assert_eq!(parsed, secs);
    }

    #[test]
    fn test_parse_iso8601_known_date() {
        // 2026-03-06T00:00:00Z = days from 1970-01-01 × 86400
        let secs = parse_iso8601_secs("2026-03-06T00:00:00Z").unwrap();
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
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306143000".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "add hello world".to_string(),
        );
        manager.create(&state).unwrap();
        let loaded = manager.load(&id).unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.input, "add hello world");
        assert!(matches!(loaded.phase, SessionPhase::Planned));
        assert!(loaded.current_step.is_none());
    }

    #[test]
    fn test_session_save_updates_state() {
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306150000".to_string();
        let mut state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        manager.create(&state).unwrap();

        state.phase = SessionPhase::Running;
        state.current_step = Some("implement".to_string());
        manager.save(&state).unwrap();

        let loaded = manager.load(&id).unwrap();
        assert!(matches!(loaded.phase, SessionPhase::Running));
        assert_eq!(loaded.current_step, Some("implement".to_string()));
    }

    #[test]
    fn test_session_list_sorted() {
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());
        for id in ["20260306100000", "20260306120000", "20260306090000"] {
            let state = SessionState::new(
                id.to_string(),
                PathBuf::from("/repo"),
                "cruise.yaml".to_string(),
                "task".to_string(),
            );
            manager.create(&state).unwrap();
        }
        let sessions = manager.list().unwrap();
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].id, "20260306090000");
        assert_eq!(sessions[1].id, "20260306100000");
        assert_eq!(sessions[2].id, "20260306120000");
    }

    #[test]
    fn test_session_list_empty() {
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let sessions = manager.list().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_session_pending_filters() {
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());

        let mut planned = SessionState::new(
            "20260306100000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task1".to_string(),
        );
        planned.phase = SessionPhase::Planned;
        manager.create(&planned).unwrap();

        let mut completed = SessionState::new(
            "20260306110000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task2".to_string(),
        );
        completed.phase = SessionPhase::Completed;
        manager.create(&completed).unwrap();

        let mut failed = SessionState::new(
            "20260306120000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task3".to_string(),
        );
        failed.phase = SessionPhase::Failed("some error".to_string());
        manager.create(&failed).unwrap();

        let pending = manager.pending().unwrap();
        assert_eq!(pending.len(), 2);
        let ids: Vec<&str> = pending.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"20260306100000"));
        assert!(ids.contains(&"20260306120000"));
    }

    #[test]
    fn test_session_delete() {
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "20260306100000".to_string();
        let state = SessionState::new(
            id.clone(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        manager.create(&state).unwrap();
        assert!(manager.sessions_dir().join(&id).exists());

        manager.delete(&id).unwrap();
        assert!(!manager.sessions_dir().join(&id).exists());

        let sessions = manager.list().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_cleanup_old_completed() {
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());

        // Old session completed 10 days ago (relative to actual current time).
        let now_secs = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let old_secs: u64 = now_secs - 10 * 86400;
        let (y, mo, d, h, mi, s) = seconds_to_datetime(old_secs);
        let old_time = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s);

        let mut old = SessionState::new(
            "20260101000000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "old task".to_string(),
        );
        old.phase = SessionPhase::Completed;
        old.completed_at = Some(old_time);
        manager.create(&old).unwrap();

        let report = manager.cleanup_old(3).unwrap();
        assert_eq!(report.deleted, 1);
        assert!(manager.list().unwrap().is_empty());
    }

    #[test]
    fn test_cleanup_old_keeps_recent() {
        let tmp = TempDir::new().unwrap();
        let manager = SessionManager::new(tmp.path().to_path_buf());

        // Recent session completed 1 day ago (relative to actual current time).
        let now_secs = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let recent_secs: u64 = now_secs - 86400;
        let (y, mo, d, h, mi, s) = seconds_to_datetime(recent_secs);
        let recent_time = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s);

        let mut recent = SessionState::new(
            "20260306100000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "task".to_string(),
        );
        recent.phase = SessionPhase::Completed;
        recent.completed_at = Some(recent_time);
        manager.create(&recent).unwrap();

        let report = manager.cleanup_old(3).unwrap();
        assert_eq!(report.deleted, 0);
        assert_eq!(manager.list().unwrap().len(), 1);
    }
}
