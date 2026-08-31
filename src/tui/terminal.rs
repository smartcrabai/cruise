use std::io::{self, IsTerminal, Stdout};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Idempotent terminal ownership for the interactive client. Restoration is
/// retried by Drop when one of the independent cleanup operations fails.
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
    previous_hook: Option<PanicHook>,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "cruise tui requires an interactive TTY",
            ));
        }
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = restore_terminal_result();
            return Err(error);
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = restore_terminal_result();
                return Err(error);
            }
        };
        let previous_hook = std::panic::take_hook();
        // Worker panics are caught at their task boundary. Root/event-loop
        // panics still drop this guard and restore the terminal; a worker must
        // never tear down a terminal that the event loop is still drawing.
        std::panic::set_hook(Box::new(|panic| eprintln!("cruise tui panic: {panic}")));
        Ok(Self {
            terminal,
            restored: false,
            previous_hook: Some(previous_hook),
        })
    }

    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        let result = restore_terminal_result();
        if result.is_ok() {
            self.restored = true;
            if let Some(previous) = self.previous_hook.take() {
                std::panic::set_hook(previous);
            }
        }
        result
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn restore_terminal_result() -> io::Result<()> {
    let raw_result = disable_raw_mode();
    let screen_result = execute!(io::stdout(), Show, LeaveAlternateScreen);
    raw_result
        .err()
        .or_else(|| screen_result.err())
        .map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_is_safe_when_raw_mode_was_not_enabled() {
        let _ = restore_terminal_result();
    }
}
