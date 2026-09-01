//! Keyboard-only Ratatui client for Cruise.
//!
//! The client owns rendering, input, and spawned task handles. Domain state,
//! claims, prompt identity, cancellation, and persistence stay in the shared
//! `crate::application::CruiseApplication` façade.

mod app;
mod forms;
mod input;
mod prompts;
mod registry;
mod terminal;
mod ui;

use crossterm::event::{Event as CrosstermEvent, EventStream};
use futures::StreamExt;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::application::CruiseApplication;
use crate::error::{CruiseError, Result};
use crate::session::SessionManager;

use app::TuiApp;
use registry::UiEvent;

/// Start the interactive TUI against the normal Cruise data directory.
pub async fn run() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(CruiseError::Other(
            "cruise requires an interactive TTY".to_string(),
        ));
    }
    let data_dir = crate::paths::data_dir()?;
    run_with_application(CruiseApplication::new(SessionManager::new(data_dir))).await
}

async fn run_with_application(application: CruiseApplication) -> Result<()> {
    let mut terminal =
        terminal::TerminalGuard::new().map_err(|error| CruiseError::Other(error.to_string()))?;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<UiEvent>();
    let (log_tx, mut log_rx) = mpsc::channel::<UiEvent>(256);
    let mut app = TuiApp::new(application, event_tx.clone(), log_tx.clone());
    let mut input = EventStream::new();
    let mut spinner = tokio::time::interval(Duration::from_millis(100));
    spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut autosave = tokio::time::interval(Duration::from_millis(500));
    autosave.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut external_poll = tokio::time::interval(Duration::from_secs(3));
    external_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    #[cfg(unix)]
    let mut signals = Signals::new().map_err(|error| CruiseError::Other(error.to_string()))?;
    let mut input_closed = false;
    let mut input_error: Option<String> = None;
    let mut redraw = true;
    loop {
        for payload in app.take_notifications() {
            std::mem::drop(tokio::task::spawn_blocking(move || {
                crate::desktop_notifications::send_payload_best_effort(&payload);
            }));
        }
        if redraw {
            if let Err(error) = terminal.terminal().draw(|frame| ui::draw(frame, &mut app)) {
                app.cancel_and_quit();
                app.registry.shutdown().await;
                let _ = terminal.restore();
                return Err(CruiseError::Other(format!("failed to draw TUI: {error}")));
            }
            if app.take_bell() {
                let _ = io::stdout().write_all(b"\x07");
                let _ = io::stdout().flush();
            }
            redraw = false;
        }
        if app.operation_state.quit_requested && !app.is_busy() {
            break;
        }
        tokio::select! {
            _ = spinner.tick(), if app.is_busy() => { app.tick_spinner(); redraw = true; }
            _ = autosave.tick(), if app.form.dirty => { app.autosave_draft(std::time::Instant::now()); redraw = true; }
            _ = external_poll.tick() => { app.refresh_if_due(); redraw = true; }
            Some(event) = event_rx.recv() => { app.apply_event(event); redraw = true; }
            Some(event) = log_rx.recv() => { app.apply_event(event); redraw = true; }
            event = input.next(), if !input_closed => {
                match event {
                    Some(Ok(CrosstermEvent::Key(key))) => { let _ = app.handle_key(key); redraw = true; }
                    Some(Ok(CrosstermEvent::Resize(width, height))) => { app.on_resize(width, height); redraw = true; }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => { let message = format!("terminal input failed: {error}"); app.set_error(message.clone()); input_error = Some(message); app.cancel_and_quit(); input_closed = true; redraw = true; }
                    None => { app.cancel_and_quit(); input_closed = true; redraw = true; }
                }
            }
            _ = signals.interrupt.recv() => { app.cancel_and_quit(); redraw = true; }
            _ = signals.term.recv() => { app.cancel_and_quit(); redraw = true; }
            _ = signals.hup.recv() => { app.cancel_and_quit(); redraw = true; }
            else => { app.cancel_and_quit(); break; }
        }
    }
    app.registry.shutdown().await;
    terminal
        .restore()
        .map_err(|error| CruiseError::Other(format!("failed to restore terminal: {error}")))?;
    if let Some(error) = input_error {
        return Err(CruiseError::Other(error));
    }
    Ok(())
}

#[cfg(unix)]
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    term: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> io::Result<Self> {
        Ok(Self {
            interrupt: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
            term: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
            hup: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::IsTerminal;

    #[test]
    fn non_tty_error_is_descriptive() {
        let error = io::Error::new(
            io::ErrorKind::NotConnected,
            "cruise requires an interactive TTY",
        );
        assert_eq!(error.kind(), io::ErrorKind::NotConnected);
        assert!(error.to_string().contains("interactive TTY"));
    }

    #[tokio::test]
    async fn run_rejects_non_tty_without_touching_application_state() {
        assert!(
            !io::stdin().is_terminal() || !io::stdout().is_terminal(),
            "test must run without an interactive TTY"
        );
        let error = match run().await {
            Ok(()) => panic!("TUI unexpectedly accepted non-TTY input/output"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("interactive TTY"));
    }
}
