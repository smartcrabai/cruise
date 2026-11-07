//! Best-effort desktop notifications shared by the CLI and TUI.

const NOTIFICATION_TITLE: &str = "Cruise";
const MAX_DETAIL_CHARS: usize = 60;
const DISABLE_NOTIFICATIONS_ENV: &str = "CRUISE_DISABLE_NOTIFICATIONS";

use crate::app_config::{AppConfig, NotificationSettings};
use crate::application::ApplicationEvent;
#[cfg(not(test))]
use crate::notification_sound;
use crate::notification_webhook;

#[derive(Clone, Copy)]
pub(crate) enum NotificationChannel {
    Desktop,
    Sound,
    Webhook,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ChannelSet {
    pub(crate) desktop: bool,
    pub(crate) sound: bool,
    pub(crate) webhook: bool,
}

pub(crate) fn event_channel_enabled(
    settings: &NotificationSettings,
    kind: WorkflowNotificationKind,
    channel: NotificationChannel,
) -> bool {
    let event = match kind {
        WorkflowNotificationKind::ActionRequired => &settings.events.action_required,
        WorkflowNotificationKind::PlanReady => &settings.events.plan_ready,
        WorkflowNotificationKind::Completed => &settings.events.completed,
        WorkflowNotificationKind::Failed => &settings.events.failed,
    };
    let (master, override_value) = match channel {
        NotificationChannel::Desktop => (settings.desktop, event.desktop),
        NotificationChannel::Sound => (settings.sound, event.sound),
        NotificationChannel::Webhook => (settings.webhook, event.webhook),
    };
    master && override_value.unwrap_or(true)
}

pub(crate) fn resolve_channels(
    kind: WorkflowNotificationKind,
    settings: &NotificationSettings,
    globally_disabled: bool,
    webhook_configured: bool,
) -> ChannelSet {
    if globally_disabled {
        return ChannelSet::default();
    }
    let supported = cfg!(any(target_os = "macos", target_os = "linux"));
    ChannelSet {
        desktop: supported && event_channel_enabled(settings, kind, NotificationChannel::Desktop),
        sound: supported && event_channel_enabled(settings, kind, NotificationChannel::Sound),
        webhook: webhook_configured
            && event_channel_enabled(settings, kind, NotificationChannel::Webhook),
    }
}

pub(crate) fn load_settings() -> NotificationSettings {
    AppConfig::load()
        .map(|config| config.notifications)
        .unwrap_or_default()
}

pub(crate) trait DeliverySinks {
    fn desktop(&self, payload: &NotificationPayload);
    fn sound(&self, kind: WorkflowNotificationKind);
    fn webhook(&self, url: &str, text: &str);
}

struct SystemSinks;
impl DeliverySinks for SystemSinks {
    fn desktop(&self, payload: &NotificationPayload) {
        #[cfg(all(not(test), any(target_os = "macos", target_os = "linux")))]
        let _ = notify_rust::Notification::new()
            .summary(&payload.title)
            .body(&payload.body)
            .show();
        #[cfg(any(test, not(any(target_os = "macos", target_os = "linux"))))]
        let _ = payload;
    }
    fn sound(&self, kind: WorkflowNotificationKind) {
        #[cfg(not(test))]
        notification_sound::play(kind);
        #[cfg(test)]
        let _ = kind;
    }
    fn webhook(&self, url: &str, text: &str) {
        let result = notification_webhook::post(
            url,
            &notification_webhook::request_body(text),
            notification_webhook::WEBHOOK_TIMEOUT,
        );
        notification_webhook::record_outcome(&result);
    }
}

pub(crate) fn deliver(
    payload: &NotificationPayload,
    channels: ChannelSet,
    webhook_url: Option<&str>,
    sinks: &dyn DeliverySinks,
) {
    if channels.desktop {
        sinks.desktop(payload);
    }
    if channels.sound {
        sinks.sound(payload.kind);
    }
    if channels.webhook
        && let Some(url) = webhook_url
    {
        sinks.webhook(
            url,
            &notification_webhook::slack_text(&payload.title, &payload.body),
        );
    }
}

pub(crate) fn application_event_notification(
    event: &ApplicationEvent,
) -> Option<(WorkflowNotificationKind, &str, Option<&str>)> {
    match event {
        ApplicationEvent::AskUserRequired {
            session_id,
            question,
            ..
        } => Some((
            WorkflowNotificationKind::ActionRequired,
            session_id,
            Some(question),
        )),
        ApplicationEvent::OptionRequired {
            session_id, prompt, ..
        } => Some((
            WorkflowNotificationKind::ActionRequired,
            session_id,
            Some(prompt),
        )),
        ApplicationEvent::PlanFinished { session_id, phase } if phase == "Awaiting Approval" => {
            Some((WorkflowNotificationKind::PlanReady, session_id, None))
        }
        ApplicationEvent::PlanFailed { session_id, error }
        | ApplicationEvent::RunFailed { session_id, error } => {
            Some((WorkflowNotificationKind::Failed, session_id, Some(error)))
        }
        ApplicationEvent::RunFinished { session_id, phase } if phase == "Completed" => {
            Some((WorkflowNotificationKind::Completed, session_id, None))
        }
        _ => None,
    }
}

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

pub(crate) fn sanitize_text(text: &str) -> String {
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
    dispatch(payload, false);
}

pub(crate) fn send_server_side_best_effort(payload: &NotificationPayload) {
    dispatch(payload, true);
}

fn dispatch(payload: &NotificationPayload, server_side: bool) {
    if notifications_disabled() {
        return;
    }
    let settings = load_settings();
    let url = notification_webhook::url_from_env();
    let mut channels = resolve_channels(payload.kind, &settings, false, url.is_some());
    if server_side {
        channels.desktop = false;
    }
    deliver(payload, channels, url.as_deref(), &SystemSinks);
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
    fn channel_gates_combine_masters_overrides_and_environment() {
        let mut settings = NotificationSettings::default();
        for kind in [
            WorkflowNotificationKind::ActionRequired,
            WorkflowNotificationKind::PlanReady,
            WorkflowNotificationKind::Completed,
            WorkflowNotificationKind::Failed,
        ] {
            assert!(event_channel_enabled(
                &settings,
                kind,
                NotificationChannel::Desktop
            ));
            assert!(!event_channel_enabled(
                &settings,
                kind,
                NotificationChannel::Sound
            ));
            assert!(event_channel_enabled(
                &settings,
                kind,
                NotificationChannel::Webhook
            ));
        }
        settings.events.failed.desktop = Some(false);
        settings.events.plan_ready.sound = Some(true);
        assert!(!event_channel_enabled(
            &settings,
            WorkflowNotificationKind::Failed,
            NotificationChannel::Desktop
        ));
        assert!(event_channel_enabled(
            &settings,
            WorkflowNotificationKind::Completed,
            NotificationChannel::Desktop
        ));
        assert!(!event_channel_enabled(
            &settings,
            WorkflowNotificationKind::PlanReady,
            NotificationChannel::Sound
        ));
        let disabled = resolve_channels(WorkflowNotificationKind::Completed, &settings, true, true);
        assert!(!disabled.desktop && !disabled.sound && !disabled.webhook);
        let no_url = resolve_channels(WorkflowNotificationKind::Completed, &settings, false, false);
        assert!(!no_url.webhook);
        assert_eq!(
            no_url.desktop,
            cfg!(any(target_os = "macos", target_os = "linux"))
        );
    }

    #[test]
    fn delivery_is_ordered_and_escapes_webhook_text() {
        struct Recorder(std::cell::RefCell<Vec<String>>);
        impl DeliverySinks for Recorder {
            fn desktop(&self, _: &NotificationPayload) {
                self.0.borrow_mut().push("desktop".into());
            }
            fn sound(&self, _: WorkflowNotificationKind) {
                self.0.borrow_mut().push("sound".into());
            }
            fn webhook(&self, url: &str, text: &str) {
                self.0.borrow_mut().push(format!("{url} {text}"));
            }
        }
        let recorder = Recorder(std::cell::RefCell::default());
        let payload = build_payload(WorkflowNotificationKind::Failed, None, Some("a<b>"), "s");
        deliver(
            &payload,
            ChannelSet {
                desktop: true,
                sound: true,
                webhook: true,
            },
            Some("https://u"),
            &recorder,
        );
        assert_eq!(
            *recorder.0.borrow(),
            ["desktop", "sound", "https://u Cruise: Failed -- a&lt;b&gt;"]
        );
        recorder.0.borrow_mut().clear();
        deliver(
            &payload,
            ChannelSet {
                desktop: false,
                sound: false,
                webhook: true,
            },
            None,
            &recorder,
        );
        assert!(recorder.0.borrow().is_empty());
    }

    #[test]
    fn application_events_map_only_expected_notifications() {
        let sid = "session".to_string();
        let cases = [
            (
                ApplicationEvent::AskUserRequired {
                    session_id: sid.clone(),
                    request_id: "r".into(),
                    question: "question".into(),
                },
                Some((
                    WorkflowNotificationKind::ActionRequired,
                    "session",
                    Some("question"),
                )),
            ),
            (
                ApplicationEvent::OptionRequired {
                    session_id: sid.clone(),
                    request_id: "r".into(),
                    prompt: "prompt".into(),
                    choices: vec![],
                },
                Some((
                    WorkflowNotificationKind::ActionRequired,
                    "session",
                    Some("prompt"),
                )),
            ),
            (
                ApplicationEvent::PlanFinished {
                    session_id: sid.clone(),
                    phase: "Awaiting Approval".into(),
                },
                Some((WorkflowNotificationKind::PlanReady, "session", None)),
            ),
            (
                ApplicationEvent::PlanFinished {
                    session_id: sid.clone(),
                    phase: "Planned".into(),
                },
                None,
            ),
            (
                ApplicationEvent::PlanFailed {
                    session_id: sid.clone(),
                    error: "failure".into(),
                },
                Some((WorkflowNotificationKind::Failed, "session", Some("failure"))),
            ),
            (
                ApplicationEvent::RunFailed {
                    session_id: sid.clone(),
                    error: "failure".into(),
                },
                Some((WorkflowNotificationKind::Failed, "session", Some("failure"))),
            ),
            (
                ApplicationEvent::RunFinished {
                    session_id: sid.clone(),
                    phase: "Completed".into(),
                },
                Some((WorkflowNotificationKind::Completed, "session", None)),
            ),
            (
                ApplicationEvent::RunFinished {
                    session_id: sid,
                    phase: "Failed".into(),
                },
                None,
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(application_event_notification(&event), expected);
        }
    }

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
