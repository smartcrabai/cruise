use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use futures::Stream;
use serde::Deserialize;

use crate::application::ApplicationEvent;
use crate::daemon::server::WebState;

pub(crate) fn routes() -> Router<WebState> {
    Router::new().route("/events", get(events))
}

#[derive(Deserialize)]
struct EventFilter {
    session: Option<String>,
    batch: Option<String>,
}

async fn events(
    State(state): State<WebState>,
    Query(filter): Query<EventFilter>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let batch = filter
        .batch
        .is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    let session = filter.session;
    let receiver = state.hub.subscribe_reliable();
    let stream = futures::stream::unfold(
        (receiver, session, batch),
        |(mut receiver, session, batch)| async move {
            loop {
                let event = receiver.recv().await?;
                if let Some(session_id) = session.as_deref()
                    && event_session(&event) != Some(session_id)
                    && !(batch && is_batch_event(&event))
                {
                    continue;
                }
                let encoded = match Event::default().event("app").json_data(event) {
                    Ok(encoded) => encoded,
                    Err(error) => {
                        eprintln!("daemon API: failed to encode application event: {error}");
                        continue;
                    }
                };
                return Some((Ok(encoded), (receiver, session, batch)));
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn event_session(event: &ApplicationEvent) -> Option<&str> {
    match event {
        ApplicationEvent::PlanStarted { session_id, .. }
        | ApplicationEvent::PlanChunk { session_id, .. }
        | ApplicationEvent::AskUserRequired { session_id, .. }
        | ApplicationEvent::PlanFinished { session_id, .. }
        | ApplicationEvent::PlanFailed { session_id, .. }
        | ApplicationEvent::PlanCancelled { session_id }
        | ApplicationEvent::RunStarted { session_id }
        | ApplicationEvent::RunPhase { session_id, .. }
        | ApplicationEvent::StepStarted { session_id, .. }
        | ApplicationEvent::OptionRequired { session_id, .. }
        | ApplicationEvent::PrCreated { session_id, .. }
        | ApplicationEvent::RunFinished { session_id, .. }
        | ApplicationEvent::RunFailed { session_id, .. }
        | ApplicationEvent::RunCancelled { session_id }
        | ApplicationEvent::AutoRequeued { session_id, .. } => Some(session_id),
        ApplicationEvent::BatchSessionStarted { id }
        | ApplicationEvent::BatchSessionFinished { id, .. } => Some(id),
        ApplicationEvent::LogChunk { session_id, .. } => session_id.as_deref(),
        ApplicationEvent::BatchStarted { .. }
        | ApplicationEvent::BatchTotalChanged { .. }
        | ApplicationEvent::BatchFinished { .. }
        | ApplicationEvent::BatchFailed { .. } => None,
    }
}

fn is_batch_event(event: &ApplicationEvent) -> bool {
    matches!(
        event,
        ApplicationEvent::BatchStarted { .. }
            | ApplicationEvent::BatchTotalChanged { .. }
            | ApplicationEvent::BatchSessionStarted { .. }
            | ApplicationEvent::BatchSessionFinished { .. }
            | ApplicationEvent::BatchFinished { .. }
            | ApplicationEvent::BatchFailed { .. }
            | ApplicationEvent::LogChunk { batch: true, .. }
    )
}
