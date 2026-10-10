//! Graceful shutdown for `cruise webui`.

use std::time::Duration;

use super::WebState;
use crate::application::CruiseApplication;
use crate::error::{CruiseError, Result};

/// Hard deadline between the first signal and process exit.
pub(crate) const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(10);

/// Begin shutdown: cancel active operations and close every SSE stream.
pub(crate) fn begin(state: &WebState) {
    let signalled = state.application.runtime().begin_shutdown();
    state.hub.close();
    eprintln!(
        "cruise webui: shutting down (cancelling {signalled} active operation(s)); press Ctrl+C again to quit immediately"
    );
}

/// Wait until every cancelled operation has released its claim.
pub(crate) async fn wait_for_operations(application: &CruiseApplication) {
    while !application.runtime().is_idle() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(unix)]
pub(crate) struct ShutdownSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    pub(crate) fn new() -> Result<Self> {
        let register = |kind| {
            tokio::signal::unix::signal(kind).map_err(|error| {
                CruiseError::Other(format!("failed to register signal handler: {error}"))
            })
        };
        Ok(Self {
            interrupt: register(tokio::signal::unix::SignalKind::interrupt())?,
            terminate: register(tokio::signal::unix::SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
    }
}

#[cfg(not(unix))]
pub(crate) struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Production signal future: the first signal starts graceful shutdown and
/// arms a hard-deadline watchdog. A second signal quits immediately.
pub(crate) async fn production_signal(mut signals: ShutdownSignals) {
    signals.recv().await;
    std::thread::spawn(|| {
        std::thread::sleep(SHUTDOWN_GRACE_PERIOD);
        eprintln!(
            "cruise webui: shutdown did not finish within {}s; exiting",
            SHUTDOWN_GRACE_PERIOD.as_secs()
        );
        std::process::exit(1);
    });
    tokio::spawn(async move {
        signals.recv().await;
        eprintln!("cruise webui: forced exit");
        std::process::exit(130);
    });
}
