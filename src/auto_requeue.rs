use crate::error::{CruiseError, Result};
use crate::retry::{RetryClass, classify_retryable, is_auth_failure};
use crate::session::{SessionManager, SessionPhase, SessionState};

pub(crate) fn is_eligible_error(error: &CruiseError) -> bool {
    match error {
        CruiseError::CommandError(message) => !is_auth_failure(message) && !matches!(
            classify_retryable(message),
            Some(RetryClass::ClientError | RetryClass::ModelMissing | RetryClass::ModelUnusable)
        ),
        CruiseError::StepMadeNoFileChanges(_) => true,
        CruiseError::ConfigNotFound(_)
        | CruiseError::ConfigParseError(_)
        | CruiseError::StepNotFound(_)
        | CruiseError::InvalidStepConfig(_)
        | CruiseError::MissingJcodeModel(_)
        | CruiseError::AgentAuthExpired(_)
        | CruiseError::UndefinedVariable(_)
        | CruiseError::EmptyVariableReference
        | CruiseError::InvalidTemplateSyntax(_)
        | CruiseError::ProcessSpawnError(_)
        | CruiseError::LoopProtection { .. }
        | CruiseError::IoError(_)
        | CruiseError::SerializationError(_)
        | CruiseError::NotGitRepository
        | CruiseError::WorktreeError(_)
        | CruiseError::Busy(_)
        | CruiseError::SessionError(_)
        | CruiseError::SessionStateConflict(_)
        | CruiseError::SessionStateConflictAborted(_)
        | CruiseError::StepTimeout { .. }
        | CruiseError::GitHubReviewFailed { .. }
        | CruiseError::Interrupted
        | CruiseError::SquashMergeFailed(_)
        | CruiseError::CommitGuardViolation(_)
        | CruiseError::ReadOnlyWorkspaceChanged
        | CruiseError::Other(_)
        | CruiseError::StepPaused => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AutoRequeueSkip {
    Disabled,
    NotFailed,
    Cancelled,
    IneligibleError,
    BudgetExhausted,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AutoRequeueDecision {
    Requeue { attempt: u32 },
    Skip(AutoRequeueSkip),
}

pub(crate) fn decide(
    state: &SessionState,
    error: &CruiseError,
    max_attempts: u32,
    cancelled: bool,
) -> AutoRequeueDecision {
    use AutoRequeueSkip::*;
    let skip = if max_attempts == 0 {
        Some(Disabled)
    } else if !matches!(state.phase, SessionPhase::Failed(_)) {
        Some(NotFailed)
    } else if cancelled {
        Some(Cancelled)
    } else if !is_eligible_error(error) {
        Some(IneligibleError)
    } else if state.auto_requeue_count >= max_attempts {
        Some(BudgetExhausted)
    } else {
        None
    };
    match skip {
        Some(reason) => AutoRequeueDecision::Skip(reason),
        None => AutoRequeueDecision::Requeue {
            attempt: state.auto_requeue_count + 1,
        },
    }
}

pub(crate) fn try_record_requeue(
    manager: &SessionManager,
    session_id: &str,
    error: &CruiseError,
    max_attempts: u32,
    cancelled: bool,
) -> Result<Option<u32>> {
    let mut state = manager.load(session_id)?;
    match decide(&state, error, max_attempts, cancelled) {
        AutoRequeueDecision::Skip(_) => Ok(None),
        AutoRequeueDecision::Requeue { attempt } => {
            state.auto_requeue_count = attempt;
            manager.save(&mut state)?;
            Ok(Some(attempt))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_config::SessionConfigRef;

    fn state() -> SessionState {
        let mut state = SessionState::new("test".into(), "/tmp/repo".into(), SessionConfigRef::InlineSnapshot, "test".into());
        state.phase = SessionPhase::Failed("failed".into());
        state
    }

    #[test]
    fn eligibility_classifies_auth_and_other_errors() {
        for message in ["HTTP 401", "status 403", "error 407", "authentication failed", "invalid API key", "unauthorized", "forbidden", "permission denied", "HTTP 400 bad request", "unknown model"] {
            assert!(!is_eligible_error(&CruiseError::CommandError(message.into())), "{message}");
        }
        for message in ["unclassified command failure", "HTTP 503", "connection reset"] {
            assert!(is_eligible_error(&CruiseError::CommandError(message.into())), "{message}");
        }
        assert!(is_eligible_error(&CruiseError::StepMadeNoFileChanges("work".into())));
        assert!(!is_eligible_error(&CruiseError::Interrupted));
        assert!(!is_eligible_error(&CruiseError::Other("failure".into())));
    }

    #[test]
    fn decisions_are_ordered_and_skips_do_not_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(tmp.path().into());
        let mut state = state();
        manager.create(&state).unwrap();
        let eligible = CruiseError::CommandError("unclassified".into());
        let auth = CruiseError::CommandError("HTTP 401".into());
        state.phase = SessionPhase::AwaitingApproval;
        state.auto_requeue_count = 2;
        manager.save(&state).unwrap();
        let cases = [(0, true, &auth, AutoRequeueSkip::Disabled), (1, true, &auth, AutoRequeueSkip::NotFailed)];
        for (max, cancelled, error, reason) in cases {
            assert_eq!(decide(&state, error, max, cancelled), AutoRequeueDecision::Skip(reason));
            let before = std::fs::read(tmp.path().join("sessions/test/state.json")).unwrap();
            assert_eq!(try_record_requeue(&manager, "test", error, max, cancelled).unwrap(), None);
            assert_eq!(before, std::fs::read(tmp.path().join("sessions/test/state.json")).unwrap());
        }
        state.phase = SessionPhase::Failed("failed".into());
        manager.save(&state).unwrap();
        for (cancelled, error, reason) in [(true, &auth, AutoRequeueSkip::Cancelled), (false, &auth, AutoRequeueSkip::IneligibleError), (false, &eligible, AutoRequeueSkip::BudgetExhausted)] {
            assert_eq!(decide(&state, error, 2, cancelled), AutoRequeueDecision::Skip(reason));
            let before = std::fs::read(tmp.path().join("sessions/test/state.json")).unwrap();
            assert_eq!(try_record_requeue(&manager, "test", error, 2, cancelled).unwrap(), None);
            assert_eq!(before, std::fs::read(tmp.path().join("sessions/test/state.json")).unwrap());
        }
        assert_eq!(decide(&state, &eligible, 3, false), AutoRequeueDecision::Requeue { attempt: 3 });
        assert_eq!(try_record_requeue(&manager, "test", &eligible, 3, false).unwrap(), Some(3));
        assert_eq!(manager.load("test").unwrap().auto_requeue_count, 3);
        assert!(try_record_requeue(&manager, "missing", &eligible, 3, false).is_err());
    }
}
