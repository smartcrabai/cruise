use super::{
    events::{Decoder, Turn},
    parse_model_ref,
    types::{Error, Event},
};
use serde_json::json;

#[tokio::test]
async fn boundary_uses_synchronization_high_water_without_reducing_history() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("listener: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("address: {error}"));
    let app = axum::Router::new().fallback(|| async {
        ([ ("content-type", "text/event-stream") ], concat!(
            "data: {\"type\":\"session.execution.failed\",\"data\":{},\"durable\":{\"aggregateID\":\"ses_test\",\"seq\":4}}\n\n",
            "data: {\"type\":\"log.synced\",\"aggregateID\":\"ses_test\",\"seq\":12}\n\n"
        ))
    });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = super::client::Client::new(
        format!("http://{address}")
            .parse()
            .unwrap_or_else(|error| panic!("URL: {error}")),
        "password".into(),
    )
    .unwrap_or_else(|error| panic!("client: {error}"));
    assert_eq!(
        super::durable_boundary(&client, "ses_test")
            .await
            .unwrap_or_else(|error| panic!("boundary: {error}")),
        12
    );
    server.abort();
}

#[tokio::test]
async fn cancelling_boundary_acquisition_never_submits_prompt() {
    use axum::response::IntoResponse as _;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let cancel = crate::cancellation::CancellationToken::new();
    let prompts = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("listener: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("address: {error}"));
    let token = cancel.clone();
    let count = prompts.clone();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let token = token.clone();
        let count = count.clone();
        async move {
            if uri.path() == "/api/event" {
                (
                    [("content-type", "text/event-stream")],
                    "data: {\"type\":\"server.connected\"}\n\n",
                )
                    .into_response()
            } else if uri.path().ends_with("/log") {
                token.cancel();
                let pending =
                    futures::stream::pending::<std::result::Result<String, std::io::Error>>();
                axum::body::Body::from_stream(pending).into_response()
            } else {
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::NO_CONTENT.into_response()
            }
        }
    });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = super::client::Client::new(
        format!("http://{address}")
            .parse()
            .unwrap_or_else(|error| panic!("URL: {error}")),
        "password".into(),
    )
    .unwrap_or_else(|error| panic!("client: {error}"));
    let (tx, _rx) = std::sync::mpsc::sync_channel(128);
    let mut failure = None;
    let mut clean = false;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::execute(
            &client,
            super::TurnInput {
                session: "ses_test",
                inbox: "msg_test",
                text: "test",
            },
            &cancel,
            &tx,
            &mut failure,
            &mut clean,
        ),
    )
    .await
    .unwrap_or_else(|error| panic!("cancellation: {error}"));
    assert!(result.is_err());
    assert!(clean);
    assert!(failure.is_none());
    assert_eq!(prompts.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn empty_log_cannot_prove_an_admitted_input_was_settled() {
    use axum::response::IntoResponse as _;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("fixture listener: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("fixture address: {error}"));
    let app = axum::Router::new().fallback(|uri: axum::http::Uri| async move {
        if uri.path().ends_with("/inbox") {
            axum::Json(json!({"data":[]})).into_response()
        } else if uri.path().ends_with("/log") {
            (
                [("content-type", "text/event-stream")],
                "data: {\"type\":\"log.synced\",\"aggregateID\":\"ses_test\",\"seq\":0}\n\n",
            )
                .into_response()
        } else {
            axum::Json(json!({"interrupted":true})).into_response()
        }
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .unwrap_or_else(|error| panic!("fixture server: {error}"));
    });
    let client = super::client::Client::new(
        format!("http://{address}")
            .parse()
            .unwrap_or_else(|error| panic!("fixture URL: {error}")),
        "private-fixture-password".to_owned(),
    )
    .unwrap_or_else(|error| panic!("fixture client: {error}"));
    assert!(
        super::settle_interrupted(&client, "ses_test", "msg_test")
            .await
            .is_err()
    );
    server.abort();
}

#[test]
fn mandatory_inbox_delivery_requires_an_input_identifier() {
    let mut turn = Turn::default();
    assert!(matches!(
        turn.apply(
            event(
                "session.inbox.delivered",
                Some(1),
                json!({"sessionID":"ses_test"})
            ),
            "ses_test",
            "msg_test"
        ),
        Err(Error::Protocol(_))
    ));
}

fn event(kind: &str, seq: Option<u64>, data: serde_json::Value) -> Event {
    let aggregate = data["sessionID"].as_str().unwrap_or("ses_test").to_owned();
    let mut value =
        json!({"type":kind,"durable":seq.map(|seq|json!({"seq":seq,"aggregateID":aggregate}))});
    value["data"] = data;
    serde_json::from_value(value).unwrap_or_else(|error| panic!("fixture event: {error}"))
}

#[test]
fn boundary_rejects_stale_history_but_still_validates_its_aggregate() {
    let mut turn = Turn::default();
    turn.sequence = 10;
    for seq in [1, 10] {
        assert!(
            turn.apply(
                event(
                    "session.execution.started",
                    Some(seq),
                    json!({"sessionID":"ses_test"})
                ),
                "ses_test",
                "msg_test"
            )
            .is_ok()
        );
        assert!(!turn.started);
    }
    let mismatched: Event = serde_json::from_value(json!({
        "type":"session.execution.started", "data":{"sessionID":"ses_test"},
        "durable":{"aggregateID":"ses_other","seq":1}
    }))
    .unwrap_or_else(|error| panic!("event: {error}"));
    let error = turn
        .apply(mismatched, "ses_test", "msg_test")
        .err()
        .unwrap_or_else(|| panic!("stale aggregate mismatch accepted"));
    assert_eq!(
        error.to_string(),
        "OpenCode V2 protocol failure: durable event aggregate does not match session"
    );
    assert!(
        turn.apply(
            event(
                "session.execution.started",
                Some(15),
                json!({"sessionID":"ses_test"})
            ),
            "ses_test",
            "msg_test"
        )
        .is_ok()
    );
    assert!(turn.started);
    assert_eq!(turn.sequence, 15);
}

#[test]
fn current_started_failure_does_not_require_delivery_but_success_does() {
    let mut turn = Turn::default();
    let failure = || {
        json!({"sessionID":"ses_test","error":{
            "type":"provider.api","message":"context preparation failed","status":503
        }})
    };
    for (kind, seq, data) in [
        ("session.execution.failed", 1, failure()),
        (
            "session.execution.started",
            2,
            json!({"sessionID":"ses_test"}),
        ),
        ("session.execution.failed", 1, failure()),
        (
            "session.execution.succeeded",
            3,
            json!({"sessionID":"ses_test"}),
        ),
    ] {
        assert!(
            turn.apply(event(kind, Some(seq), data), "ses_test", "msg_test")
                .is_ok()
        );
        assert!(turn.terminal.is_none());
    }
    assert!(
        turn.apply(
            event("session.execution.failed", Some(4), failure()),
            "ses_test",
            "msg_test",
        )
        .is_ok()
    );
    assert!(matches!(turn.terminal, Some(Err(_))));
    assert!(!turn.delivered);
    assert_eq!(
        turn.failure.as_ref().and_then(|error| error.status),
        Some(503)
    );
}

#[test]
fn complete_text_appends_only_suffix_and_duplicate_endings_emit_nothing() {
    let mut turn = Turn::default();
    turn.started = true;
    turn.delivered = true;
    let text = |kind, seq, value| event(kind, seq, value);
    assert!(turn.apply(text("session.text.delta", None,
        json!({"sessionID":"ses_test","assistantMessageID":"answer","ordinal":0,"delta":"\u{65e5}"})),
        "ses_test", "msg_test").is_ok());
    let ended = || json!({"sessionID":"ses_test","assistantMessageID":"answer","ordinal":0,"text":"\u{65e5}\u{672c}\u{8a9e}"});
    assert_eq!(
        turn.apply(
            text("session.text.ended", Some(1), ended()),
            "ses_test",
            "msg_test"
        )
        .unwrap_or_else(|error| panic!("{error}"))
        .as_deref(),
        Some("\u{672c}\u{8a9e}")
    );
    assert!(
        turn.apply(
            text("session.text.ended", Some(2), ended()),
            "ses_test",
            "msg_test"
        )
        .is_ok_and(|delta| delta.is_none())
    );
    assert_eq!(turn.output(), "\u{65e5}\u{672c}\u{8a9e}");
    assert!(turn.apply(text("session.text.delta", None,
        json!({"sessionID":"ses_test","assistantMessageID":"other","ordinal":0,"delta":"prefix"})),
        "ses_test", "msg_test").is_ok());
    assert!(matches!(turn.apply(text("session.text.ended", Some(3),
        json!({"sessionID":"ses_test","assistantMessageID":"other","ordinal":0,"text":"different"})),
        "ses_test", "msg_test"), Err(Error::Protocol(_))));
}
#[test]
fn model_reference_keeps_punctuation_and_requires_provider() {
    let model = parse_model_ref(Some("provider/team/model:2026#precise"))
        .unwrap_or_else(|error| panic!("model: {error}"));
    let value = serde_json::to_value(model).unwrap_or_else(|error| panic!("model JSON: {error}"));
    assert_eq!(
        value,
        json!({"providerID":"provider","id":"team/model:2026","variant":"precise"})
    );
    for raw in [
        "model",
        "/model",
        "provider/",
        "provider/model#",
        "provider/model#a#b",
        " provider/model",
    ] {
        assert!(parse_model_ref(Some(raw)).is_err(), "{raw}");
    }
    assert!(parse_model_ref(None).is_ok_and(|model| model.is_none()));
}
#[test]
fn sse_every_byte_boundary_preserves_utf8_crlf_and_multiline_data() {
    let text = ":heartbeat\r\ndata: {\"type\":\"future.event\",\r\ndata: \"data\":{\"text\":\"\u{65e5}\u{672c}\u{8a9e}\"}}\r\n\r\n";
    for split in 0..=text.len() {
        let mut decoder = Decoder::default();
        let mut values = decoder
            .push(&text.as_bytes()[..split])
            .unwrap_or_else(|e| panic!("first: {e}"));
        values.extend(
            decoder
                .push(&text.as_bytes()[split..])
                .unwrap_or_else(|e| panic!("second: {e}")),
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].data["text"], "\u{65e5}\u{672c}\u{8a9e}");
    }
}
#[test]
fn sse_bounds_and_invalid_json_are_errors() {
    assert!(
        Decoder::default()
            .push(&vec![b'x'; 2 * 1024 * 1024 + 1])
            .is_err()
    );
    assert!(Decoder::default().push(b"data: {broken}\n\n").is_err());
}
#[test]
fn turn_reconciles_missing_suffix_and_never_answers_with_reasoning_or_foreign_events() {
    let mut turn = Turn::default();
    let session = "ses_test";
    let inbox = "msg_test";
    for value in [
        event(
            "session.execution.started",
            Some(1),
            json!({"sessionID":session}),
        ),
        event(
            "session.inbox.delivered",
            Some(2),
            json!({"sessionID":session,"inboxID":inbox}),
        ),
    ] {
        assert!(turn.apply(value, session, inbox).is_ok());
    }
    assert!(
        turn.apply(
            event(
                "session.reasoning.delta",
                None,
                json!({"sessionID":session,"delta":"secret"})
            ),
            session,
            inbox
        )
        .is_ok_and(|value| value.is_none())
    );
    assert!(
        turn.apply(
            event(
                "session.text.delta",
                None,
                json!({"sessionID":"ses_foreign","delta":"foreign"})
            ),
            session,
            inbox
        )
        .is_ok_and(|value| value.is_none())
    );
    let delta=turn.apply(event("session.text.delta",None,json!({"sessionID":session,"assistantMessageID":"msg_answer","ordinal":0,"delta":"\u{65e5}"})),session,inbox).unwrap_or_else(|e|panic!("delta: {e}"));
    assert_eq!(delta.as_deref(), Some("\u{65e5}"));
    let suffix=turn.apply(event("session.text.ended",Some(3),json!({"sessionID":session,"assistantMessageID":"msg_answer","ordinal":0,"text":"\u{65e5}\u{672c}\u{8a9e}"})),session,inbox).unwrap_or_else(|e|panic!("full: {e}"));
    assert_eq!(suffix.as_deref(), Some("\u{672c}\u{8a9e}"));
    assert!(turn.terminal.is_none());
    assert!(
        turn.apply(
            event(
                "session.execution.succeeded",
                Some(4),
                json!({"sessionID":session})
            ),
            session,
            inbox
        )
        .is_ok()
    );
    assert!(matches!(turn.terminal, Some(Ok(()))));
    assert_eq!(turn.output(), "\u{65e5}\u{672c}\u{8a9e}");
}
#[test]
fn public_sequence_holes_are_valid_and_missing_required_fields_fail_closed() {
    let mut turn = Turn::default();
    assert!(
        turn.apply(
            event(
                "session.execution.started",
                Some(1),
                json!({"sessionID":"ses_test"})
            ),
            "ses_test",
            "msg_test"
        )
        .is_ok()
    );
    assert!(
        turn.apply(
            event(
                "session.inbox.delivered",
                Some(3),
                json!({"sessionID":"ses_test","inboxID":"msg_test"})
            ),
            "ses_test",
            "msg_test"
        )
        .is_ok()
    );
    assert!(
        turn.apply(
            event(
                "session.execution.succeeded",
                Some(5),
                json!({"sessionID":"ses_test"})
            ),
            "ses_test",
            "msg_test"
        )
        .is_ok()
    );
    assert!(matches!(turn.terminal, Some(Ok(()))));
    assert_eq!(turn.sequence, 5);
    assert!(
        turn.apply(
            event("session.execution.succeeded", None, json!({})),
            "ses_test",
            "msg_test"
        )
        .is_err()
    );
}
