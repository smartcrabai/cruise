use crate::error::Result;
use crate::session::SessionState;

impl super::CruiseApplication {
    /// List sessions after reconciling stale running phases against this daemon's runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if listing or reconciling session state fails.
    pub fn reconciled_sessions(&self) -> Result<Vec<SessionState>> {
        let mut sessions = self.manager.list()?;
        sessions.retain(|session| !session.exec);
        for session in &mut sessions {
            let active = self.runtime.active_identity(&session.id).is_some();
            self.manager.reconcile_running_phase(session, active);
        }
        Ok(sessions)
    }

    /// Read a session diff using daemon-side filesystem access.
    ///
    /// # Errors
    ///
    /// Returns an error if session reconciliation or diff generation fails.
    pub fn session_diff(&self, id: &str, color: bool) -> Result<crate::branch_diff::BranchDiff> {
        let session = self.reconcile_session(id)?;
        crate::branch_diff::session_branch_diff(&session, color)
    }
}
