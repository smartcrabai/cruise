#![cfg(windows)]

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::TempDir;

const EXIT_TIMEOUT: Duration = Duration::from_secs(20);
const TRANSCRIPT_TAIL: usize = 4000;

struct Tui {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    _root: TempDir,
    output: Arc<Mutex<Vec<u8>>>,
    cursor_queries_answered: usize,
}

fn start_tui() -> Tui {
    let root = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap_or_else(|error| panic!("openpty failed: {error}"));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_cruise"));
    for (key, dir) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
    ] {
        let path = root.path().join(dir);
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{error}"));
        command.env(key, path);
    }
    command.env("CRUISE_DISABLE_NOTIFICATIONS", "1");
    let child = pair
        .slave
        .spawn_command(command)
        .unwrap_or_else(|error| panic!("spawn failed: {error}"));
    let writer = pair
        .master
        .take_writer()
        .unwrap_or_else(|error| panic!("writer failed: {error}"));
    let mut reader = pair
        .master
        .try_clone_reader()
        .unwrap_or_else(|error| panic!("reader failed: {error}"));
    let output = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&output);
    // Drain output so the ConPTY never blocks on a full pipe, and keep it for
    // failure messages.
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(n) if n > 0 => {
                    if let Ok(mut bytes) = sink.lock() {
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                }
                _ => break,
            }
        }
    });
    let mut tui = Tui {
        child,
        writer,
        _master: pair.master,
        _root: root,
        output,
        cursor_queries_answered: 0,
    };
    // crossterm blocks on the `ESC[6n` cursor-position query during startup, and
    // ConPTY has no emulator to answer it, so answer as a terminal would.
    let settle = Instant::now() + Duration::from_secs(3);
    while Instant::now() < settle {
        answer_cursor_queries(&mut tui);
        std::thread::sleep(Duration::from_millis(25));
    }
    tui
}

fn answer_cursor_queries(tui: &mut Tui) {
    let queries = {
        let bytes = tui.output.lock().unwrap_or_else(|error| error.into_inner());
        String::from_utf8_lossy(&bytes).matches("\x1b[6n").count()
    };
    while tui.cursor_queries_answered < queries {
        let _ = tui.writer.write_all(b"\x1b[1;1R");
        let _ = tui.writer.flush();
        tui.cursor_queries_answered += 1;
    }
}

fn transcript(tui: &Tui) -> String {
    let bytes = tui
        .output
        .lock()
        .map(|bytes| bytes.clone())
        .unwrap_or_default();
    let start = bytes.len().saturating_sub(TRANSCRIPT_TAIL);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn wait_for_exit(tui: &mut Tui) -> bool {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while Instant::now() < deadline {
        answer_cursor_queries(tui);
        if matches!(tui.child.try_wait(), Ok(Some(_))) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn windows_tui_ctrl_c_cancels_busy_operation_and_restores_terminal() {
    let mut tui = start_tui();
    tui.writer
        .write_all(&[0x03])
        .unwrap_or_else(|error| panic!("{error}"));
    tui.writer.flush().unwrap_or_else(|error| panic!("{error}"));
    assert!(
        wait_for_exit(&mut tui),
        "TUI did not exit after Ctrl+C; recent output: {}",
        transcript(&tui)
    );
}

#[test]
fn windows_tui_quit_restores_raw_mode_after_input_close() {
    let mut tui = start_tui();
    tui.writer
        .write_all(b"q")
        .unwrap_or_else(|error| panic!("{error}"));
    tui.writer.flush().unwrap_or_else(|error| panic!("{error}"));
    drop(std::mem::replace(
        &mut tui.writer,
        Box::new(std::io::sink()),
    ));
    if !wait_for_exit(&mut tui) {
        let _ = tui.child.kill();
        panic!("TUI did not exit after quit/input close");
    }
}
