//! In-process fan-out of [`ApplicationEvent`]s to every open SSE connection.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::application::{ApplicationEvent, ApplicationEventSink, EventStream, LogEvent, LogSink};
use crate::error::Result;

/// Buffered events per SSE connection, matching the TUI's `UiEvent` channel.
const SUBSCRIBER_CAPACITY: usize = 256;

/// Broadcast hub. Sinks never fail: a browser that navigated away must not
/// turn a running session into `Failed`
/// (`CruiseApplication` maps `sink.send` errors to a failed phase).
///
/// Subscriber channels are bounded: an SSE connection that stopped reading
/// (sleeping laptop, dropped VPN, killed `curl`) drops events instead of
/// buffering a whole run's output in memory.
#[derive(Debug, Default)]
pub(crate) struct EventHub {
    subscribers: Mutex<Vec<mpsc::Sender<ApplicationEvent>>>,
}

impl EventHub {
    pub(crate) fn subscribe(&self) -> mpsc::Receiver<ApplicationEvent> {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_CAPACITY);
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(tx);
        rx
    }

    pub(crate) fn broadcast(&self, event: &ApplicationEvent) {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|tx| match tx.try_send(event.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            });
    }

    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Event and log sinks that publish into this hub.
    pub(crate) fn sinks(self: &Arc<Self>) -> (Arc<dyn ApplicationEventSink>, Arc<dyn LogSink>) {
        (
            Arc::new(HubEventSink(Arc::clone(self))),
            Arc::new(HubLogSink(Arc::clone(self))),
        )
    }
}

#[derive(Debug)]
struct HubEventSink(Arc<EventHub>);

impl ApplicationEventSink for HubEventSink {
    fn send(&self, event: ApplicationEvent) -> Result<()> {
        self.0.broadcast(&event);
        Ok(())
    }
}

#[derive(Debug)]
struct HubLogSink(Arc<EventHub>);

impl LogSink for HubLogSink {
    fn try_send(&self, event: LogEvent) -> bool {
        self.0.broadcast(&ApplicationEvent::LogChunk {
            session_id: event.session_id,
            stream: event.stream,
            text: event.text,
            batch: event.batch,
        });
        true
    }
}

/// An `ApplicationEventSink` that records Run All progress before forwarding.
pub(crate) struct RunAllSink {
    pub(crate) hub: Arc<EventHub>,
    pub(crate) state: Arc<super::run_all_state::RunAllState>,
}

impl ApplicationEventSink for RunAllSink {
    fn send(&self, event: ApplicationEvent) -> Result<()> {
        self.state.apply(&event);
        self.hub.broadcast(&event);
        Ok(())
    }
}

/// A `LogSink` that records Run All log lines before forwarding.
pub(crate) struct RunAllLogSink {
    pub(crate) hub: Arc<EventHub>,
    pub(crate) state: Arc<super::run_all_state::RunAllState>,
}

impl LogSink for RunAllLogSink {
    fn try_send(&self, event: LogEvent) -> bool {
        let event = ApplicationEvent::LogChunk {
            session_id: event.session_id,
            stream: event.stream,
            text: event.text,
            batch: event.batch,
        };
        self.state.apply(&event);
        self.hub.broadcast(&event);
        true
    }
}

/// Stream name used by log partial styling.
pub(crate) fn stream_name(stream: EventStream) -> &'static str {
    match stream {
        EventStream::Stdout => "stdout",
        EventStream::Stderr => "stderr",
        EventStream::Info => "info",
    }
}

#[cfg(test)]
mod tests {
    use super::EventHub;
    use crate::application::ApplicationEvent;
    use std::sync::Arc;

    #[test]
    fn sink_keeps_succeeding_after_all_subscribers_drop() {
        let hub = Arc::new(EventHub::default());
        let rx = hub.subscribe();
        let (sink, _log) = hub.sinks();
        drop(rx);
        let result = sink.send(ApplicationEvent::RunStarted {
            session_id: "s1".to_string(),
        });
        assert!(result.is_ok());
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn broadcast_reaches_every_live_subscriber() {
        let hub = Arc::new(EventHub::default());
        let mut a = hub.subscribe();
        let mut b = hub.subscribe();
        hub.broadcast(&ApplicationEvent::RunCancelled {
            session_id: "s1".to_string(),
        });
        assert!(a.try_recv().is_ok());
        assert!(b.try_recv().is_ok());
    }
}
