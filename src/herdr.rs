//! Best-effort lifecycle-state reports to a surrounding herdr pane.
//!
//! herdr (<https://herdr.dev>) marks every pane `working` / `blocked` / `idle`.
//! Cruise reports through herdr's CLI when its complete legacy environment is
//! available, or through Core's terminal OSC protocol when running in a
//! recognized Core terminal. A dedicated worker keeps output off callers'
//! critical paths and drains reports before cruise exits.
//!
//! Every failure is ignored on purpose — reporting must never change cruise's
//! own behaviour or exit status.

use base64::Engine as _;
use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal, Write};
use std::process::Stdio;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Source id reported to herdr; identifies cruise's lifecycle authority.
const SOURCE: &str = "custom:cruise";
/// Agent label shown by herdr.
const AGENT: &str = "cruise";

/// herdr sets this to `1` inside pane processes.
const ENV_FLAG: &str = "HERDR_ENV";
const PANE_ENV: &str = "HERDR_PANE_ID";
const BIN_ENV: &str = "HERDR_BIN_PATH";
/// Herdr Core sets this in its terminal processes.
const HC_TERMINAL_ENV: &str = "HC_TERMINAL_ID";
/// Opt-out, mirroring `CRUISE_DISABLE_NOTIFICATIONS`.
const DISABLE_ENV: &str = "CRUISE_DISABLE_HERDR";

/// herdr caps presentation text at 80 characters.
const MAX_MESSAGE_CHARS: usize = 80;
/// Enough space for the OSC frame and base64 of the longest sanitized reason.
const CORE_FRAME_BUFFER_SIZE: usize = 512;
/// Bounds how long process exit waits for the queued reports to drain.
const RELEASE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Working,
    Idle,
    Blocked,
}

impl AgentState {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Blocked => "blocked",
        }
    }
}

struct LegacyTarget {
    bin: OsString,
    pane_id: String,
}

#[derive(Clone, Copy)]
enum TerminalStream {
    Stderr,
    Stdout,
}

enum Target {
    Legacy(LegacyTarget),
    Core(TerminalStream),
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Resolve the surrounding herdr integration, or `None` when disabled.
///
/// A complete legacy CLI target takes precedence. Core is used only when its
/// terminal id is present and an output stream is an actual foreground terminal.
fn target_from_env(
    get: &dyn Fn(&str) -> Option<OsString>,
    terminal: impl FnOnce() -> Option<TerminalStream>,
) -> Option<Target> {
    let one = OsStr::new("1");
    if get(DISABLE_ENV).as_deref() == Some(one) {
        return None;
    }
    if get(ENV_FLAG).as_deref() == Some(one) {
        let pane_id = get(PANE_ENV)
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.is_empty());
        let bin = get(BIN_ENV).filter(|value| !value.is_empty());
        if let (Some(pane_id), Some(bin)) = (pane_id, bin) {
            return Some(Target::Legacy(LegacyTarget { bin, pane_id }));
        }
    }
    if get(HC_TERMINAL_ENV).is_some_and(|terminal_id| !terminal_id.is_empty()) {
        return Some(Target::Core(terminal()?));
    }
    None
}

fn terminal_stream() -> Option<TerminalStream> {
    if std::io::stderr().is_terminal() && terminal_is_foreground(TerminalStream::Stderr) {
        Some(TerminalStream::Stderr)
    } else if std::io::stdout().is_terminal() && terminal_is_foreground(TerminalStream::Stdout) {
        Some(TerminalStream::Stdout)
    } else {
        None
    }
}

fn terminal_is_foreground(terminal: TerminalStream) -> bool {
    #[cfg(unix)]
    {
        let fd = match terminal {
            TerminalStream::Stderr => libc::STDERR_FILENO,
            TerminalStream::Stdout => libc::STDOUT_FILENO,
        };
        // SAFETY: These libc calls use only an inherited standard file descriptor and process state.
        let foreground_group = unsafe { libc::tcgetpgrp(fd) };
        let process_group = unsafe { libc::getpgrp() };
        foreground_group != -1 && foreground_group == process_group
    }
    #[cfg(not(unix))]
    {
        let _ = terminal;
        true
    }
}

fn report_args(
    target: &LegacyTarget,
    state: AgentState,
    message: Option<&str>,
    seq: u64,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "pane".into(),
        "report-agent".into(),
        target.pane_id.clone().into(),
        "--source".into(),
        SOURCE.into(),
        "--agent".into(),
        AGENT.into(),
        "--state".into(),
        state.as_arg().into(),
        "--seq".into(),
        seq.to_string().into(),
    ];
    if let Some(message) = message {
        args.push("--message".into());
        args.push(message.into());
    }
    args
}

fn release_args(target: &LegacyTarget, seq: u64) -> Vec<OsString> {
    vec![
        "pane".into(),
        "release-agent".into(),
        target.pane_id.clone().into(),
        "--source".into(),
        SOURCE.into(),
        "--agent".into(),
        AGENT.into(),
        "--seq".into(),
        seq.to_string().into(),
    ]
}

fn sanitize_message(text: &str) -> Option<String> {
    let mut sanitized: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let start = sanitized.len() - sanitized.trim_start().len();
    let end = sanitized.trim_end().len();
    if start >= end {
        return None;
    }
    drop(sanitized.drain(..start));
    sanitized.truncate(end - start);
    if let Some((end, _)) = sanitized.char_indices().nth(MAX_MESSAGE_CHARS) {
        sanitized.truncate(end);
    }
    Some(sanitized)
}

enum Msg {
    Report {
        state: AgentState,
        message: Option<String>,
    },
    Release,
    Drain(Sender<()>),
}

struct Reporter {
    core_enabled: bool,
    tx: Sender<Msg>,
    last: Mutex<Option<(AgentState, Option<String>)>>,
}

impl Reporter {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }
}

static REPORTER: LazyLock<Option<Reporter>> = LazyLock::new(init_reporter);

/// Whether the Core terminal reporter was selected from the current environment.
#[must_use]
pub fn core_enabled() -> bool {
    REPORTER
        .as_ref()
        .is_some_and(|reporter| reporter.core_enabled)
}

/// Seed `--seq` from wall-clock microseconds so a later cruise process in the
/// same pane never loses to the sequence numbers of an earlier one.
fn initial_seq() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_micros()).ok())
        .unwrap_or(0)
}

fn init_reporter() -> Option<Reporter> {
    let target = target_from_env(&|key| std::env::var_os(key), terminal_stream)?;
    let core_enabled = matches!(&target, Target::Core(_));
    let seq = if core_enabled { 0 } else { initial_seq() };
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    std::thread::Builder::new()
        .name("herdr-reporter".to_string())
        .spawn(move || worker(&target, rx, seq))
        .ok()?;
    Some(Reporter {
        core_enabled,
        tx,
        last: Mutex::new(None),
    })
}

fn worker(target: &Target, rx: Receiver<Msg>, mut seq: u64) {
    let mut core_claimed = false;
    for msg in rx {
        match msg {
            Msg::Report { state, message } => match target {
                Target::Legacy(target) => {
                    run_command(
                        &target.bin,
                        &report_args(target, state, message.as_deref(), take_seq(&mut seq)),
                    );
                }
                Target::Core(terminal) => {
                    if write_core_report(*terminal, state, message.as_deref()) {
                        core_claimed = true;
                    }
                }
            },
            Msg::Release => match target {
                Target::Legacy(target) => {
                    run_command(&target.bin, &release_args(target, take_seq(&mut seq)));
                }
                Target::Core(terminal) => {
                    if core_claimed && write_core_clear(*terminal) {
                        core_claimed = false;
                    }
                }
            },
            Msg::Drain(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn take_seq(seq: &mut u64) -> u64 {
    let current = *seq;
    *seq = current.wrapping_add(1);
    current
}

fn write_core_report(terminal: TerminalStream, state: AgentState, message: Option<&str>) -> bool {
    let mut frame = [0; CORE_FRAME_BUFFER_SIZE];
    let mut length = 0;
    append_frame_part(&mut frame, &mut length, b"\x1b]7501;");
    match state {
        AgentState::Working => {
            append_frame_part(&mut frame, &mut length, b"state=working:app=cruise");
        }
        AgentState::Blocked => {
            append_frame_part(
                &mut frame,
                &mut length,
                b"state=blocked:app=cruise:kind=question",
            );
            if let Some(message) = message {
                append_frame_part(&mut frame, &mut length, b":msg=");
                let encoded_end = frame.len() - 2;
                let Ok(encoded_length) = base64::engine::general_purpose::STANDARD
                    .encode_slice(message.as_bytes(), &mut frame[length..encoded_end])
                else {
                    return false;
                };
                length += encoded_length;
            }
        }
        AgentState::Idle => return false,
    }
    append_frame_part(&mut frame, &mut length, b"\x1b\\");
    write_terminal_frame(terminal, &frame[..length])
}

fn append_frame_part(frame: &mut [u8], length: &mut usize, part: &[u8]) {
    let end = *length + part.len();
    frame[*length..end].copy_from_slice(part);
    *length = end;
}

fn write_core_clear(terminal: TerminalStream) -> bool {
    write_terminal_frame(terminal, b"\x1b]7501;state=clear\x1b\\")
}

fn write_terminal_frame(terminal: TerminalStream, frame: &[u8]) -> bool {
    match terminal {
        TerminalStream::Stderr => {
            let mut output = std::io::stderr().lock();
            if !terminal_is_foreground(terminal) {
                return false;
            }
            let write_ok = output.write_all(frame).is_ok();
            let flush_ok = output.flush().is_ok();
            write_ok && flush_ok
        }
        TerminalStream::Stdout => {
            let mut output = std::io::stdout().lock();
            if !terminal_is_foreground(terminal) {
                return false;
            }
            let write_ok = output.write_all(frame).is_ok();
            let flush_ok = output.flush().is_ok();
            write_ok && flush_ok
        }
    }
}

fn run_command(bin: &OsStr, args: &[OsString]) {
    let child = std::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        let _ = child.wait();
    }
}

fn enqueue_report(
    reporter: &Reporter,
    state: AgentState,
    message: Option<String>,
    last: &mut Option<(AgentState, Option<String>)>,
) {
    if last
        .as_ref()
        .is_some_and(|previous| previous.0 == state && previous.1 == message)
    {
        return;
    }
    *last = Some((state, message.clone()));
    reporter.send(Msg::Report { state, message });
}

/// Report `idle` and hand lifecycle authority back to herdr's CLI, or clear
/// the Core status on the outermost release.
///
/// Blocks until the reporter thread has drained the queued commands (normally
/// milliseconds) so the reports survive process exit.
pub fn release() {
    let Some(reporter) = REPORTER.as_ref() else {
        return;
    };
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    {
        let mut last = lock(&reporter.last);
        if !reporter.core_enabled {
            enqueue_report(reporter, AgentState::Idle, None, &mut last);
        }
        reporter.send(Msg::Release);
        reporter.send(Msg::Drain(done_tx));
        // A later `start()` in the same process must report again.
        *last = None;
    }
    let _ = done_rx.recv_timeout(RELEASE_DRAIN_TIMEOUT);
}

/// Nesting-aware view of what the CLI is currently doing.
#[derive(Default)]
struct Lifecycle {
    active: usize,
    blocked: Vec<(u64, Option<String>)>,
    next_id: u64,
}

impl Lifecycle {
    /// The state to report, or `None` while no [`ActiveGuard`] is alive.
    fn desired(&self) -> Option<(AgentState, Option<String>)> {
        if self.active == 0 {
            return None;
        }
        self.blocked
            .last()
            .map_or(Some((AgentState::Working, None)), |(_, message)| {
                Some((AgentState::Blocked, message.clone()))
            })
    }

    fn begin(&mut self) {
        self.active += 1;
    }

    /// Returns `true` when the outermost guard was dropped.
    fn end(&mut self) -> bool {
        self.active = self.active.saturating_sub(1);
        self.active == 0
    }

    fn block(&mut self, message: Option<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.blocked.push((id, message));
        id
    }

    /// Guards may drop out of order across parallel `run --all` workers.
    fn unblock(&mut self, id: u64) {
        self.blocked.retain(|(entry, _)| *entry != id);
    }
}

static LIFECYCLE: Mutex<Lifecycle> = Mutex::new(Lifecycle {
    active: 0,
    blocked: Vec::new(),
    next_id: 0,
});

/// Queue the current state after dropping the lifecycle lock.
fn publish_lifecycle() {
    let desired = lock(&LIFECYCLE).desired();
    if let Some((state, message)) = desired
        && let Some(reporter) = REPORTER.as_ref()
    {
        let mut last = lock(&reporter.last);
        enqueue_report(reporter, state, message, &mut last);
    }
}

/// Reports `working` for as long as it is alive; releases on the outermost drop.
///
/// Deliberately not `Clone`: the depth counter, not the guard, models nesting
/// (`cruise plan` → "Execute now" → `cruise run`).
pub struct ActiveGuard(());

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let finished = lock(&LIFECYCLE).end();
        if finished {
            release();
        } else {
            publish_lifecycle();
        }
    }
}

/// Mark cruise busy until the returned guard drops.
#[must_use]
pub fn start() -> ActiveGuard {
    lock(&LIFECYCLE).begin();
    publish_lifecycle();
    ActiveGuard(())
}

/// Reports `blocked` with its message for as long as it is alive.
pub struct BlockedGuard(u64);

impl Drop for BlockedGuard {
    fn drop(&mut self) {
        lock(&LIFECYCLE).unblock(self.0);
        publish_lifecycle();
    }
}

/// Mark cruise blocked on a user decision until the returned guard drops.
#[must_use]
pub fn blocked(message: &str) -> BlockedGuard {
    let message = sanitize_message(message);
    let id = lock(&LIFECYCLE).block(message);
    publish_lifecycle();
    BlockedGuard(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), OsString::from(*value)))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    fn target(pane_id: &str) -> LegacyTarget {
        LegacyTarget {
            bin: OsString::from("/usr/bin/herdr"),
            pane_id: pane_id.to_string(),
        }
    }

    fn resolve_target(pairs: &[(&str, &str)]) -> Option<Target> {
        target_from_env(&env(pairs), || None)
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn target_resolves_only_inside_an_enabled_herdr_pane() {
        let complete = [
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_BIN_PATH", "/usr/bin/herdr"),
        ];
        let Some(Target::Legacy(resolved)) = resolve_target(&complete) else {
            panic!("expected a legacy target inside a herdr pane");
        };
        assert_eq!(resolved.pane_id, "w1:p1");
        assert_eq!(resolved.bin, OsString::from("/usr/bin/herdr"));

        assert!(
            resolve_target(&[
                ("HERDR_PANE_ID", "w1:p1"),
                ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ])
            .is_none(),
            "HERDR_ENV unset must disable reporting"
        );
        assert!(
            resolve_target(&[
                ("HERDR_ENV", "0"),
                ("HERDR_PANE_ID", "w1:p1"),
                ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ])
            .is_none(),
            "HERDR_ENV=0 must disable reporting"
        );
        assert!(
            resolve_target(&[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1"),]).is_none(),
            "a missing HERDR_BIN_PATH must disable reporting"
        );
        assert!(
            resolve_target(&[
                ("CRUISE_DISABLE_HERDR", "1"),
                ("HERDR_ENV", "1"),
                ("HERDR_PANE_ID", "w1:p1"),
                ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ])
            .is_none(),
            "CRUISE_DISABLE_HERDR=1 must disable reporting"
        );
    }

    #[test]
    fn core_target_requires_terminal_and_yields_to_complete_legacy_target() {
        let core_env = [("HC_TERMINAL_ID", "t4")];
        assert!(matches!(
            target_from_env(&env(&core_env), || Some(TerminalStream::Stderr)),
            Some(Target::Core(TerminalStream::Stderr))
        ));
        assert!(target_from_env(&env(&core_env), || None).is_none());
        assert!(
            target_from_env(&env(&[("HC_TERMINAL_ID", "")]), || {
                Some(TerminalStream::Stderr)
            })
            .is_none()
        );
        assert!(
            target_from_env(
                &env(&[("CRUISE_DISABLE_HERDR", "1"), ("HC_TERMINAL_ID", "t4")]),
                || Some(TerminalStream::Stderr)
            )
            .is_none()
        );

        let partial_legacy_env = [("HERDR_ENV", "1"), ("HC_TERMINAL_ID", "t4")];
        assert!(matches!(
            target_from_env(&env(&partial_legacy_env), || Some(TerminalStream::Stderr)),
            Some(Target::Core(TerminalStream::Stderr))
        ));

        let legacy_and_core_env = [
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ("HC_TERMINAL_ID", "t4"),
        ];
        assert!(matches!(
            target_from_env(&env(&legacy_and_core_env), || Some(TerminalStream::Stderr)),
            Some(Target::Legacy(_))
        ));
    }

    #[test]
    fn report_args_carry_the_message_only_when_present() {
        assert_eq!(
            strings(&report_args(
                &target("w1:p1"),
                AgentState::Blocked,
                Some("Plan ready"),
                7
            )),
            vec![
                "pane",
                "report-agent",
                "w1:p1",
                "--source",
                "custom:cruise",
                "--agent",
                "cruise",
                "--state",
                "blocked",
                "--seq",
                "7",
                "--message",
                "Plan ready",
            ]
        );
        assert_eq!(
            strings(&report_args(&target("w1:p1"), AgentState::Idle, None, 8)),
            vec![
                "pane",
                "report-agent",
                "w1:p1",
                "--source",
                "custom:cruise",
                "--agent",
                "cruise",
                "--state",
                "idle",
                "--seq",
                "8",
            ]
        );
    }

    #[test]
    fn release_args_target_the_pane_source() {
        assert_eq!(
            strings(&release_args(&target("w1:p1"), 9)),
            vec![
                "pane",
                "release-agent",
                "w1:p1",
                "--source",
                "custom:cruise",
                "--agent",
                "cruise",
                "--seq",
                "9",
            ]
        );
    }

    #[test]
    fn lifecycle_tracks_nested_blocks_and_reports_nothing_when_inactive() {
        let mut lifecycle = Lifecycle::default();
        assert_eq!(lifecycle.desired(), None);

        // A prompt outside any run must not claim the pane.
        let stray = lifecycle.block(Some("stray".to_string()));
        assert_eq!(lifecycle.desired(), None);
        lifecycle.unblock(stray);

        lifecycle.begin();
        assert_eq!(lifecycle.desired(), Some((AgentState::Working, None)));

        let first = lifecycle.block(Some("q1".to_string()));
        assert_eq!(
            lifecycle.desired(),
            Some((AgentState::Blocked, Some("q1".to_string())))
        );
        let second = lifecycle.block(Some("q2".to_string()));
        assert_eq!(
            lifecycle.desired(),
            Some((AgentState::Blocked, Some("q2".to_string())))
        );

        lifecycle.unblock(second);
        assert_eq!(
            lifecycle.desired(),
            Some((AgentState::Blocked, Some("q1".to_string())))
        );
        lifecycle.unblock(first);
        assert_eq!(lifecycle.desired(), Some((AgentState::Working, None)));

        assert!(lifecycle.end());
        assert_eq!(lifecycle.desired(), None);
    }

    #[test]
    fn lifecycle_releases_only_on_the_outermost_end() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.begin();
        lifecycle.begin();
        assert!(!lifecycle.end());
        assert_eq!(lifecycle.desired(), Some((AgentState::Working, None)));
        assert!(lifecycle.end());
    }

    #[test]
    fn sanitize_message_strips_control_characters_and_truncates() {
        assert_eq!(
            sanitize_message("  line\nbreak\ttab  "),
            Some("line break tab".to_string())
        );
        let long = "x".repeat(100);
        assert_eq!(
            sanitize_message(&long).map(|message| message.chars().count()),
            Some(MAX_MESSAGE_CHARS)
        );
        assert_eq!(
            sanitize_message("before\u{85}after"),
            Some("before after".to_string())
        );
        assert_eq!(sanitize_message("   \n  "), None);
    }
}
