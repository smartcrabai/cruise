//! Best-effort lifecycle-state reports to a surrounding herdr pane.
//!
//! herdr (<https://herdr.dev>) marks every pane `working` / `blocked` / `idle`.
//! Agents without native detection report their own state from inside the pane
//! by shelling out to herdr's CLI, which is what this module does: it spawns
//! `"$HERDR_BIN_PATH" pane report-agent …` on a dedicated thread so no report
//! ever blocks the caller, and releases lifecycle authority when cruise exits.
//!
//! Every failure is ignored on purpose — reporting must never change cruise's
//! own behaviour or exit status.

use std::ffi::{OsStr, OsString};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
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
/// Opt-out, mirroring `CRUISE_DISABLE_NOTIFICATIONS`.
const DISABLE_ENV: &str = "CRUISE_DISABLE_HERDR";

/// herdr caps presentation text at 80 characters.
const MAX_MESSAGE_CHARS: usize = 80;
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

struct Target {
    bin: OsString,
    pane_id: String,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Resolve the surrounding herdr pane, or `None` when reporting is disabled.
///
/// There is deliberately no `herdr`-on-`PATH` fallback: without
/// `HERDR_BIN_PATH` there is no evidence that the surrounding pane belongs to
/// the herdr server this process would talk to.
fn target_from_env(get: &dyn Fn(&str) -> Option<OsString>) -> Option<Target> {
    let one = OsStr::new("1");
    if get(DISABLE_ENV).as_deref() == Some(one) {
        return None;
    }
    if get(ENV_FLAG).as_deref() != Some(one) {
        return None;
    }
    let pane_id = get(PANE_ENV)?.into_string().ok()?;
    if pane_id.is_empty() {
        return None;
    }
    let bin = get(BIN_ENV)?;
    if bin.is_empty() {
        return None;
    }
    Some(Target { bin, pane_id })
}

fn report_args(
    target: &Target,
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

fn release_args(target: &Target, seq: u64) -> Vec<OsString> {
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
    let sanitized: String = crate::desktop_notifications::sanitize_text(text)
        .chars()
        .take(MAX_MESSAGE_CHARS)
        .collect();
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

enum Msg {
    Run(Vec<OsString>),
    Drain(Sender<()>),
}

struct Reporter {
    target: Target,
    tx: Mutex<Sender<Msg>>,
    seq: AtomicU64,
    last: Mutex<Option<(AgentState, Option<String>)>>,
}

impl Reporter {
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    fn send(&self, msg: Msg) {
        let _ = lock(&self.tx).send(msg);
    }
}

static REPORTER: LazyLock<Option<Reporter>> = LazyLock::new(init_reporter);

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
    let target = target_from_env(&|key| std::env::var_os(key))?;
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    let bin = target.bin.clone();
    std::thread::Builder::new()
        .name("herdr-reporter".to_string())
        .spawn(move || worker(&bin, &rx))
        .ok()?;
    Some(Reporter {
        target,
        tx: Mutex::new(tx),
        seq: AtomicU64::new(initial_seq()),
        last: Mutex::new(None),
    })
}

fn worker(bin: &OsStr, rx: &Receiver<Msg>) {
    for msg in rx {
        match msg {
            Msg::Run(args) => run_command(bin, &args),
            Msg::Drain(done) => {
                let _ = done.send(());
            }
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

/// Report `state` to the surrounding herdr pane; no-op outside one.
///
/// `message` is only forwarded for [`AgentState::Blocked`], where herdr shows
/// it as the reason the pane needs attention. Repeating the current state is
/// free: identical reports are dropped before a process is spawned.
pub fn report(state: AgentState, message: Option<&str>) {
    let Some(reporter) = REPORTER.as_ref() else {
        return;
    };
    let message = match state {
        AgentState::Blocked => message.and_then(sanitize_message),
        AgentState::Working | AgentState::Idle => None,
    };
    {
        let mut last = lock(&reporter.last);
        if last
            .as_ref()
            .is_some_and(|previous| previous.0 == state && previous.1 == message)
        {
            return;
        }
        *last = Some((state, message.clone()));
    }
    let args = report_args(
        &reporter.target,
        state,
        message.as_deref(),
        reporter.next_seq(),
    );
    reporter.send(Msg::Run(args));
}

/// Report `idle` and hand lifecycle authority back to herdr.
///
/// Blocks until the reporter thread has drained the queued commands (normally
/// milliseconds) so the reports survive process exit.
pub fn release() {
    let Some(reporter) = REPORTER.as_ref() else {
        return;
    };
    report(AgentState::Idle, None);
    let args = release_args(&reporter.target, reporter.next_seq());
    reporter.send(Msg::Run(args));
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    reporter.send(Msg::Drain(done_tx));
    let _ = done_rx.recv_timeout(RELEASE_DRAIN_TIMEOUT);
    // A later `start()` in the same process must report again.
    *lock(&reporter.last) = None;
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

/// Publish the lifecycle state. Never called while `LIFECYCLE` is held:
/// [`report`] and [`release`] take their own locks and may block.
fn publish_lifecycle() {
    let desired = lock(&LIFECYCLE).desired();
    if let Some((state, message)) = desired {
        report(state, message.as_deref());
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
    let id = lock(&LIFECYCLE).block(sanitize_message(message));
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

    fn target(pane_id: &str) -> Target {
        Target {
            bin: OsString::from("/usr/bin/herdr"),
            pane_id: pane_id.to_string(),
        }
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
        let resolved = target_from_env(&env(&complete))
            .unwrap_or_else(|| panic!("expected a target inside a herdr pane"));
        assert_eq!(resolved.pane_id, "w1:p1");
        assert_eq!(resolved.bin, OsString::from("/usr/bin/herdr"));

        assert!(
            target_from_env(&env(&[
                ("HERDR_PANE_ID", "w1:p1"),
                ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ]))
            .is_none(),
            "HERDR_ENV unset must disable reporting"
        );
        assert!(
            target_from_env(&env(&[
                ("HERDR_ENV", "0"),
                ("HERDR_PANE_ID", "w1:p1"),
                ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ]))
            .is_none(),
            "HERDR_ENV=0 must disable reporting"
        );
        assert!(
            target_from_env(&env(&[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1"),])).is_none(),
            "a missing HERDR_BIN_PATH must disable reporting"
        );
        assert!(
            target_from_env(&env(&[
                ("CRUISE_DISABLE_HERDR", "1"),
                ("HERDR_ENV", "1"),
                ("HERDR_PANE_ID", "w1:p1"),
                ("HERDR_BIN_PATH", "/usr/bin/herdr"),
            ]))
            .is_none(),
            "CRUISE_DISABLE_HERDR=1 must disable reporting"
        );
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
        assert_eq!(sanitize_message("   \n  "), None);
    }
}
