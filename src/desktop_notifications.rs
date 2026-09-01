//! Best-effort desktop notifications shared by the CLI and TUI.

#![expect(
    dead_code,
    reason = "the library registers the binary-facing notification helper module"
)]

const NOTIFICATION_TITLE: &str = "Cruise";
const MAX_DETAIL_CHARS: usize = 60;
const DISABLE_NOTIFICATIONS_ENV: &str = "CRUISE_DISABLE_NOTIFICATIONS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkflowNotificationKind {
    ActionRequired,
    PlanReady,
    Completed,
    Failed,
}

impl WorkflowNotificationKind {
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ActionRequired => "Action required",
            Self::PlanReady => "Plan ready",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotificationPayload {
    pub(crate) kind: WorkflowNotificationKind,
    pub(crate) title: String,
    pub(crate) body: String,
}

#[cfg(test)]
pub(crate) trait NotificationBackend {
    fn send(&self, payload: &NotificationPayload);
}

fn sanitize_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character <= '\u{1f}' || character == '\u{7f}' {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn truncate_detail(detail: &str) -> String {
    detail.chars().take(MAX_DETAIL_CHARS).collect()
}

pub(crate) fn build_payload(
    kind: WorkflowNotificationKind,
    subject: Option<&str>,
    detail: Option<&str>,
    session_id: &str,
) -> NotificationPayload {
    let detail = detail.map(sanitize_text).filter(|text| !text.is_empty());
    let subject = subject.map(sanitize_text).filter(|text| !text.is_empty());
    let fallback = sanitize_text(session_id);
    let body_detail = detail.or(subject).unwrap_or_else(|| {
        if fallback.is_empty() {
            "Workflow event".to_string()
        } else {
            fallback
        }
    });
    let body_detail = truncate_detail(&body_detail);

    NotificationPayload {
        kind,
        title: NOTIFICATION_TITLE.to_string(),
        body: format!("{} -- {body_detail}", kind.label()),
    }
}

#[cfg(test)]
pub(crate) fn send_best_effort_with_backend(
    kind: WorkflowNotificationKind,
    subject: Option<&str>,
    detail: Option<&str>,
    session_id: &str,
    backend: &dyn NotificationBackend,
) {
    if notifications_disabled() {
        return;
    }
    let payload = build_payload(kind, subject, detail, session_id);
    backend.send(&payload);
}

pub(crate) fn send_best_effort(
    kind: WorkflowNotificationKind,
    subject: Option<&str>,
    detail: Option<&str>,
    session_id: &str,
) {
    if notifications_disabled() {
        return;
    }
    let payload = build_payload(kind, subject, detail, session_id);
    send_payload_best_effort(&payload);
}

pub(crate) fn send_payload_best_effort(payload: &NotificationPayload) {
    if notifications_disabled() {
        return;
    }

    #[cfg(all(not(test), any(target_os = "macos", target_os = "linux")))]
    {
        let _ = notify_rust::Notification::new()
            .summary(&payload.title)
            .body(&payload.body)
            .show();
    }

    #[cfg(any(test, all(not(target_os = "macos"), not(target_os = "linux"))))]
    let _ = payload;
}

fn notifications_disabled() -> bool {
    std::env::var(DISABLE_NOTIFICATIONS_ENV).is_ok_and(|value| value == "1")
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::test_support::{EnvGuard, lock_process};

    #[test]
    fn workflow_notification_kinds_use_gui_labels() {
        let cases = [
            (WorkflowNotificationKind::ActionRequired, "Action required"),
            (WorkflowNotificationKind::PlanReady, "Plan ready"),
            (WorkflowNotificationKind::Completed, "Completed"),
            (WorkflowNotificationKind::Failed, "Failed"),
        ];

        for (kind, expected_label) in cases {
            assert_eq!(kind.label(), expected_label);
        }
    }

    #[test]
    fn detail_is_preferred_over_subject_in_notification_body() {
        let payload = build_payload(
            WorkflowNotificationKind::ActionRequired,
            Some("Implement authentication"),
            Some("Which provider should be used?"),
            "session-1",
        );

        assert_eq!(payload.title, "Cruise");
        assert_eq!(
            payload.body,
            "Action required -- Which provider should be used?"
        );
    }

    #[test]
    fn subject_is_used_when_notification_detail_is_absent() {
        let payload = build_payload(
            WorkflowNotificationKind::Completed,
            Some("Implement authentication"),
            None,
            "session-1",
        );

        assert_eq!(payload.body, "Completed -- Implement authentication");
    }

    #[test]
    fn session_id_is_used_when_subject_and_detail_are_empty() {
        let payload = build_payload(
            WorkflowNotificationKind::Failed,
            Some("  "),
            Some(""),
            "session-42",
        );

        assert_eq!(payload.body, "Failed -- session-42");
    }

    #[test]
    fn control_characters_and_del_are_replaced_in_notification_text() {
        let payload = build_payload(
            WorkflowNotificationKind::Failed,
            None,
            Some("line\nnext\tpart\0hidden\u{7f}end"),
            "session-1",
        );

        assert_eq!(payload.body, "Failed -- line next part hidden end");
    }

    #[test]
    fn truncation_preserves_utf8_character_boundaries() {
        let detail = "🙂x".repeat(31);
        let payload = build_payload(
            WorkflowNotificationKind::PlanReady,
            None,
            Some(&detail),
            "session-1",
        );

        assert_eq!(payload.body, format!("Plan ready -- {}", "🙂x".repeat(30)));
    }

    #[test]
    fn long_notification_text_is_limited_to_the_short_body_budget() {
        let detail = "x".repeat(200);
        let payload = build_payload(
            WorkflowNotificationKind::Completed,
            None,
            Some(&detail),
            "session-1",
        );

        let prefix = "Completed -- ";
        assert!(payload.body.starts_with(prefix));
        assert_eq!(payload.body.chars().count(), prefix.chars().count() + 60);
        assert_eq!(payload.body, format!("{prefix}{}", "x".repeat(60)));
    }

    struct RecordingBackend {
        calls: Arc<AtomicUsize>,
    }

    impl NotificationBackend for RecordingBackend {
        fn send(&self, _payload: &NotificationPayload) {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn opt_out_prevents_backend_dispatch() {
        let _lock = lock_process();
        let _disabled = EnvGuard::set(DISABLE_NOTIFICATIONS_ENV, "1");
        let calls = Arc::new(AtomicUsize::new(0));
        let backend = RecordingBackend {
            calls: Arc::clone(&calls),
        };

        send_best_effort_with_backend(
            WorkflowNotificationKind::Completed,
            Some("task"),
            None,
            "session-1",
            &backend,
        );

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
