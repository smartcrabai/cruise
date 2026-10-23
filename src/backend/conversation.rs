//! Ownership-aware cleanup for retained planning conversations.
//!
//! Missing ownership is the legacy jcode contract. Never infer ownership from
//! an identifier or from the workflow currently selected by the user.
use std::path::Path;

use crate::error::{CruiseError, Result};

/// Keep every superseded checkpoint alive until the caller's request finishes.
/// The final saved owner, not the last attempted backend, determines retention.
pub(crate) struct DeferredCleanup {
    manager: crate::session::SessionManager,
    session_id: String,
    superseded: std::sync::Mutex<Vec<crate::session::SessionState>>,
}

impl DeferredCleanup {
    pub(crate) fn new(manager: crate::session::SessionManager, session_id: String) -> Self {
        Self { manager, session_id, superseded: std::sync::Mutex::new(Vec::new()) }
    }

    pub(crate) fn defer(&self, state: &crate::session::SessionState) {
        self.superseded.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(state.clone());
    }
}

impl Drop for DeferredCleanup {
    fn drop(&mut self) {
        let current = match self.manager.load(&self.session_id) {
            Ok(state) => Some(state),
            Err(CruiseError::SessionError(_)) => None,
            // If final persistence cannot be read, retaining storage is safer.
            Err(error) => {
                eprintln!("warning: deferred conversation cleanup skipped: {error}");
                return;
            }
        };
        for previous in self.superseded.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner) {
            let Some(id) = previous.plan_conversation_id.as_deref() else { continue };
            if current.as_ref().is_some_and(|state| {
                state.plan_conversation_id.as_deref() == Some(id)
                    && state.plan_conversation_backend.as_deref().unwrap_or("jcode")
                        == previous.plan_conversation_backend.as_deref().unwrap_or("jcode")
                    && state.plan_conversation_home == previous.plan_conversation_home
            }) {
                continue;
            }
            if let Err(error) = cleanup(
                previous.plan_conversation_backend.as_deref(), id,
                previous.plan_conversation_home.as_deref(),
                previous.worktree_path.as_deref().or(Some(&previous.base_dir)),
            ) {
                eprintln!("warning: deferred conversation cleanup failed: {error}");
            }
        }
    }
}

pub(crate) fn cleanup(
    backend: Option<&str>,
    session_id: &str,
    jcode_home: Option<&Path>,
    working_dir: Option<&Path>,
) -> Result<()> {
    match backend {
        None | Some("jcode") => {
            let home = jcode_home.map_or_else(
                || super::jcode::resolve_source_home(working_dir),
                Path::to_path_buf,
            );
            super::jcode::cleanup_session_home_at(&home, session_id).map_err(|error| {
                CruiseError::Other(format!("could not clean jcode session home: {error}"))
            })
        }
        Some("opencode") => {
            super::opencode::cleanup_conversation(session_id, working_dir).map_err(|error| {
                CruiseError::Other(format!("could not clean opencode conversation: {error}"))
            })
        }
        // Claude does not have Cruise-owned retained conversation storage.
        Some("claude") => Ok(()),
        Some(owner) => Err(CruiseError::Other(format!(
            "cannot clean conversation owned by unknown backend '{owner}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_owner_never_falls_back_to_jcode_even_for_matching_id() {
        let tmp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = tmp.path().join("jcode");
        let private = home.join(".cruise-sdk-sessions").join("owned");
        std::fs::create_dir_all(&private).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(private.join(".cruise-session-id"), "ses_unknown")
            .unwrap_or_else(|error| panic!("{error}"));
        let result = cleanup(
            Some("future-backend"),
            "ses_unknown",
            Some(&home),
            Some(tmp.path()),
        );
        let error = result
            .err()
            .unwrap_or_else(|| panic!("unknown owner must be rejected"));
        assert!(error.to_string().contains("future-backend"));
        assert!(
            private.is_dir(),
            "unknown ownership must not delete jcode storage"
        );
        assert_eq!(
            std::fs::read_to_string(private.join(".cruise-session-id"))
                .unwrap_or_else(|error| panic!("{error}")),
            "ses_unknown"
        );
    }
}
