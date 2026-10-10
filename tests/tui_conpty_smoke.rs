#![cfg(windows)]

use std::io::Write as _;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::TempDir;

const EXIT_TIMEOUT: Duration = Duration::from_secs(20);

struct Tui {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn std::io::Write + Send>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    _root: TempDir,
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
    // Drain output so the ConPTY never blocks on a full pipe.
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while matches!(std::io::Read::read(&mut reader, &mut buffer), Ok(n) if n > 0) {}
    });
    std::thread::sleep(Duration::from_secs(3));
    Tui {
        child,
        writer,
        _master: pair.master,
        _root: root,
    }
}

fn wait_for_exit(tui: &mut Tui) -> bool {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while Instant::now() < deadline {
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
    assert!(wait_for_exit(&mut tui), "TUI did not exit after Ctrl+C");
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
