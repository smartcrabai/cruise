//! Drive a command in a real pseudo-terminal and save what the user saw.
//!
//! Usage (from the repository root):
//!
//! ```sh
//! cargo +1.99.1 run --locked --quiet --example verify_pty -- \
//!   --out DIR [--rows 40] [--cols 120] [--timeout 60] \
//!   [--wait TEXT | --send KEYS | --sleep MS | --snap NAME]... -- PROGRAM [ARGS]...
//! ```
//!
//! Operations run in order. `--wait` blocks until TEXT is on the emulated screen
//! or anywhere in the transcript. `--send` writes KEYS, where `\r`, `\n`, `\t`,
//! `\e` (Escape) and `\\` are decoded. `--snap` saves the current screen. After
//! the last operation the command must exit before the timeout; the final screen
//! is saved as `final`. The child inherits the caller's environment (source
//! `env.sh` first) with `TERM=xterm-256color`.
//!
//! DIR receives `transcript.raw` (every byte the program wrote), one
//! `NN-NAME.txt` (plain screen) and `NN-NAME.html` (colored screen) per snapshot,
//! and `result.json` with the argv, exit code and snapshot list. The helper exits
//! 0 when every operation completed and the program exited, whatever its status;
//! read `exit_code` from `result.json`. It exits 1 on a timeout or I/O error,
//! after writing what it captured.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::json;

enum Op {
    Wait(String),
    Send(Vec<u8>),
    Sleep(u64),
    Snap(String),
}

struct Args {
    out: PathBuf,
    rows: NonZeroU16,
    cols: NonZeroU16,
    timeout: Duration,
    ops: Vec<Op>,
    argv: Vec<String>,
}

// Terminal queries Cruise's prompt library sends, with the replies a terminal gives.
const QUERIES: [(&str, &str); 3] = [
    ("\x1b[?u", "\x1b[?1u"),
    ("\x1b[c", "\x1b[?1;2c"),
    ("\x1b[6n", "\x1b[1;1R"),
];

fn main() {
    let args = match parse(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("verify_pty: {message}");
            std::process::exit(2);
        }
    };
    if let Err(message) = run(&args) {
        eprintln!("verify_pty: {message}");
        std::process::exit(1);
    }
}

fn parse(raw: Vec<String>) -> Result<Args, String> {
    let mut args = Args {
        out: PathBuf::new(),
        rows: NonZeroU16::new(40).unwrap_or(NonZeroU16::MIN),
        cols: NonZeroU16::new(120).unwrap_or(NonZeroU16::MIN),
        timeout: Duration::from_secs(60),
        ops: Vec::new(),
        argv: Vec::new(),
    };
    let mut iter = raw.into_iter();
    while let Some(flag) = iter.next() {
        if flag == "--" {
            args.argv = iter.collect();
            break;
        }
        let value = iter.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|err| format!("{flag} {value}: {err}"))
        };
        match flag.as_str() {
            "--out" => args.out = PathBuf::from(&value),
            "--rows" => args.rows = size(&flag, number()?)?,
            "--cols" => args.cols = size(&flag, number()?)?,
            "--timeout" => args.timeout = Duration::from_secs(number()?),
            "--sleep" => args.ops.push(Op::Sleep(number()?)),
            "--wait" => args.ops.push(Op::Wait(value)),
            "--send" => args.ops.push(Op::Send(decode_keys(&value))),
            "--snap" => args.ops.push(Op::Snap(value)),
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if args.out.as_os_str().is_empty() || args.argv.is_empty() {
        return Err("usage: --out DIR [ops]... -- PROGRAM [ARGS]...".into());
    }
    Ok(args)
}

fn size(flag: &str, value: u64) -> Result<NonZeroU16, String> {
    u16::try_from(value)
        .ok()
        .and_then(NonZeroU16::new)
        .ok_or_else(|| format!("{flag} must be 1..=65535"))
}

fn decode_keys(keys: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(keys.len());
    let mut chars = keys.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('r') => bytes.push(b'\r'),
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('e') => bytes.push(0x1b),
            Some('\\') | None => bytes.push(b'\\'),
            Some(other) => {
                bytes.push(b'\\');
                let mut buf = [0; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    bytes
}

struct Session {
    output: Arc<Mutex<Vec<u8>>>,
    writer: Box<dyn Write + Send>,
    answered: [usize; QUERIES.len()],
    rows: NonZeroU16,
    cols: NonZeroU16,
}

impl Session {
    fn bytes(&self) -> Vec<u8> {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn screen(&self) -> vt100::Parser {
        render(&self.bytes(), self.rows, self.cols)
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.writer
            .write_all(bytes)
            .and_then(|()| self.writer.flush())
            .map_err(|err| format!("write to PTY: {err}"))
    }

    fn answer_queries(&mut self) -> Result<(), String> {
        let text = String::from_utf8_lossy(&self.bytes()).into_owned();
        for (index, (query, reply)) in QUERIES.iter().enumerate() {
            while self.answered[index] < text.matches(query).count() {
                self.send(reply.as_bytes())?;
                self.answered[index] += 1;
            }
        }
        Ok(())
    }
}

fn run(args: &Args) -> Result<(), String> {
    std::fs::create_dir_all(&args.out)
        .map_err(|err| format!("create {}: {err}", args.out.display()))?;
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: args.rows.get(),
            cols: args.cols.get(),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| format!("open PTY: {err}"))?;
    let mut command = CommandBuilder::new(&args.argv[0]);
    command.args(&args.argv[1..]);
    command.cwd(std::env::current_dir().map_err(|err| err.to_string())?);
    command.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|err| format!("spawn {:?}: {err}", args.argv))?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| format!("clone PTY reader: {err}"))?;
    let output = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&output);
    let reader_thread = std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(&buffer[..count]);
        }
    });
    let mut session = Session {
        output,
        writer: pair
            .master
            .take_writer()
            .map_err(|err| format!("take PTY writer: {err}"))?,
        answered: [0; QUERIES.len()],
        rows: args.rows,
        cols: args.cols,
    };
    let mut snapshots = Vec::new();
    let (outcome, exit_code) = play(args, &mut session, &mut *child, &mut snapshots);
    if exit_code.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(session.writer);
    drop(pair.master);
    let _ = reader_thread.join();
    let session_bytes = session
        .output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    std::fs::write(args.out.join("transcript.raw"), &session_bytes)
        .map_err(|err| format!("write transcript: {err}"))?;
    snap(args, &session_bytes, "final", &mut snapshots)?;
    let result = json!({
        "argv": args.argv,
        "rows": args.rows.get(),
        "cols": args.cols.get(),
        "exit_code": exit_code,
        "completed": outcome.is_ok(),
        "error": outcome.as_ref().err(),
        "snapshots": snapshots,
    });
    std::fs::write(
        args.out.join("result.json"),
        serde_json::to_vec_pretty(&result).map_err(|err| err.to_string())?,
    )
    .map_err(|err| format!("write result: {err}"))?;
    outcome
}

/// Run the operations, then wait for the program to exit.
fn play(
    args: &Args,
    session: &mut Session,
    child: &mut (dyn portable_pty::Child + Send + Sync),
    snapshots: &mut Vec<String>,
) -> (Result<(), String>, Option<u32>) {
    let deadline = Instant::now() + args.timeout;
    for op in &args.ops {
        let step = match op {
            Op::Wait(text) => wait_for(session, child, text, deadline),
            Op::Send(bytes) => session.send(bytes),
            Op::Sleep(ms) => {
                std::thread::sleep(Duration::from_millis(*ms));
                Ok(())
            }
            Op::Snap(name) => snap(args, &session.bytes(), name, snapshots),
        };
        if let Err(message) = step {
            return (Err(message), None);
        }
    }
    loop {
        if let Err(message) = session.answer_queries() {
            return (Err(message), None);
        }
        match child.try_wait() {
            Ok(Some(status)) => return (Ok(()), Some(status.exit_code())),
            Ok(None) if Instant::now() >= deadline => {
                return (
                    Err("timed out waiting for the program to exit".into()),
                    None,
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => return (Err(format!("poll child: {err}")), None),
        }
    }
}

fn wait_for(
    session: &mut Session,
    child: &mut (dyn portable_pty::Child + Send + Sync),
    text: &str,
    deadline: Instant,
) -> Result<(), String> {
    loop {
        session.answer_queries()?;
        if session.screen().screen().contents().contains(text)
            || String::from_utf8_lossy(&session.bytes()).contains(text)
        {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "program exited ({status}) before {text:?} appeared"
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {text:?}"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn snap(args: &Args, bytes: &[u8], name: &str, snapshots: &mut Vec<String>) -> Result<(), String> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(format!(
            "snapshot name {name:?} must be ASCII letters, digits, or '-'"
        ));
    }
    let stem = format!("{:02}-{name}", snapshots.len() + 1);
    let parser = render(bytes, args.rows, args.cols);
    let screen = parser.screen();
    std::fs::write(args.out.join(format!("{stem}.txt")), screen.contents())
        .map_err(|err| format!("write {stem}.txt: {err}"))?;
    std::fs::write(
        args.out.join(format!("{stem}.html")),
        html(screen, &args.argv),
    )
    .map_err(|err| format!("write {stem}.html: {err}"))?;
    snapshots.push(stem);
    Ok(())
}

fn render(bytes: &[u8], rows: NonZeroU16, cols: NonZeroU16) -> vt100::Parser {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(bytes);
    parser
}

fn html(screen: &vt100::Screen, argv: &[String]) -> String {
    let (rows, cols) = screen.size();
    // Drop trailing blank rows so the image is the size of what was shown.
    let used = (0..rows.get())
        .rev()
        .find(|&row| {
            (0..cols.get()).any(|col| {
                screen
                    .cell(row, col)
                    .is_some_and(|cell| cell.has_contents() && cell.contents() != " ")
            })
        })
        .map_or(1, |row| row + 1);
    let mut body = String::new();
    for row in 0..used {
        for col in 0..cols.get() {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let (mut fg, mut bg) = (
                color(cell.fgcolor(), "#d4d4d4"),
                color(cell.bgcolor(), "transparent"),
            );
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
                if fg == "transparent" {
                    fg = "#1e1e1e".into();
                }
            }
            let mut style = format!("color:{fg};background:{bg}");
            if cell.bold() {
                style.push_str(";font-weight:bold");
            }
            if cell.italic() {
                style.push_str(";font-style:italic");
            }
            if cell.underline() {
                style.push_str(";text-decoration:underline");
            }
            if cell.dim() {
                style.push_str(";opacity:.6");
            }
            let text = if cell.has_contents() {
                cell.contents()
            } else {
                " "
            };
            let _ = write!(body, "<span style=\"{style}\">{}</span>", escape(text));
        }
        body.push('\n');
    }
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>cruise PTY</title>\
<style>body{{margin:0;background:#111;font:14px/1.35 Menlo,Monaco,'DejaVu Sans Mono',monospace}}\
main{{display:inline-block;background:#1e1e1e}}\
header{{color:#9aa4b2;background:#2b2b2b;padding:6px 10px}}\
pre{{margin:0;padding:8px 10px;color:#d4d4d4;font:inherit}}</style>\
<main><header>$ {}</header><pre>{body}</pre></main>",
        escape(&argv.join(" "))
    )
}

fn color(color: vt100::Color, default: &str) -> String {
    const BASE: [&str; 16] = [
        "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
        "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
    ];
    match color {
        vt100::Color::Default => default.into(),
        vt100::Color::Idx(i) if i < 16 => BASE[usize::from(i)].into(),
        vt100::Color::Idx(i) if i < 232 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            format!(
                "#{:02x}{:02x}{:02x}",
                level(i / 36),
                level(i / 6 % 6),
                level(i % 6)
            )
        }
        vt100::Color::Idx(i) => {
            let v = 8 + (i - 232) * 10;
            format!("#{v:02x}{v:02x}{v:02x}")
        }
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
