//! Terminal rendering of daemon events for foreground CLI commands.
//!
//! Work runs inside the daemon; a foreground command subscribes to the
//! session's events, prints its output, answers Ask/Option prompts on the
//! terminal, and turns Ctrl+C into a daemon-side cancellation.

use std::collections::HashSet;
use std::io::IsTerminal as _;

use crate::application::{
    ApplicationEvent, EventStream as OutputStream, OptionChoiceKind, OptionChoicePayload,
    PendingPrompt, PendingPromptKind,
};
use crate::daemon::client::{DaemonClient, EventStream, PromptAnswer, SessionEventHandler};
use crate::error::{CruiseError, Result};
use crate::session::{SessionPhase, SessionState};
use crate::step::OptionChoice;

/// Run CLI setup with Ctrl+C handling before Core reports the command working.
pub async fn interruptible_startup<F, T>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if crate::herdr::core_enabled() {
        tokio::pin!(future);
        tokio::select! {
            biased;
            signal = tokio::signal::ctrl_c() => {
                if signal.is_ok() {
                    Err(CruiseError::Interrupted)
                } else {
                    future.await
                }
            }
            result = &mut future => result,
        }
    } else {
        future.await
    }
}

/// Final state of a driven session operation.
#[derive(Debug)]
pub struct Finished {
    pub state: SessionState,
    /// The user pressed Ctrl+C and the daemon was asked to cancel.
    pub cancelled_by_user: bool,
    /// A terminal failure event, which may not be reflected in persisted state.
    pub failure: Option<String>,
}

/// Consume `stream` until the session's operation ends. Ctrl+C requests
/// cancellation and keeps waiting for the resulting terminal event. A second
/// Ctrl+C exits immediately. Create `ctrl_c` before starting the operation.
///
/// When stdin is not a terminal, a prompt remains active in the daemon so a
/// WebUI/API client can answer it after this client exits.
///
/// # Errors
///
/// Returns handler errors (e.g. [`CruiseError::StepPaused`] when the user
/// leaves a prompt unanswered) and daemon transport errors.
pub async fn drive<H: SessionEventHandler>(
    client: &DaemonClient,
    session_id: &str,
    stream: EventStream,
    mut ctrl_c: CtrlC,
    handler: &mut H,
) -> Result<Finished> {
    let mut cancelled_by_user = false;
    let mut retry_cancellation = false;
    let mut retry = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_millis(100),
        std::time::Duration::from_millis(100),
    );
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = {
        let wait = client.wait_for_terminal_from(session_id, stream, handler);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                result = &mut wait => break result,
                () = ctrl_c.recv() => {
                    match ctrl_c_action(&mut cancelled_by_user) {
                        CtrlCAction::Cancel => {
                            eprintln!("Cancelling session {session_id}...");
                            retry_cancellation = !client.cancel(session_id).await?;
                        }
                        CtrlCAction::Exit => return Err(CruiseError::Interrupted),
                    }
                }
                _ = retry.tick(), if retry_cancellation => {
                    if client.run_session(session_id).await?.1 {
                        retry_cancellation = !client.cancel(session_id).await?;
                    }
                }
            }
        }
    };
    match result {
        Err(CruiseError::StepPaused) => {
            // A non-interactive client cannot render the pending prompt, but
            // the daemon can keep waiting for a WebUI/API answer after this
            // client exits. Only an interactive dismissal should suspend the
            // run and require a later `cruise run`.
            if !cancelled_by_user && !std::io::stdin().is_terminal() {
                return Err(CruiseError::StepPaused);
            }
            if !cancelled_by_user {
                client.pause(session_id).await?;
            }
            let stream = client
                .events(crate::daemon::client::EventFilter {
                    session: Some(session_id.to_owned()),
                    batch: true,
                })
                .await?;
            client
                .wait_for_terminal_from(session_id, stream, &mut IgnoreSessionEvents)
                .await?;
            if cancelled_by_user {
                Err(CruiseError::Interrupted)
            } else {
                Err(CruiseError::StepPaused)
            }
        }
        result => result.map(|state| Finished {
            state,
            cancelled_by_user,
            failure: handler.terminal_failure().map(str::to_owned),
        }),
    }
}

/// Ctrl+C listener that keeps a signal delivered while the caller is busy.
///
/// A fresh `tokio::signal::ctrl_c()` future only sees signals after its first
/// poll, so recreating it on every `select!` iteration loses a Ctrl+C that
/// arrives while another branch runs. Until a listener exists, Ctrl+C kills
/// the CLI, so create one before asking the daemon to start work.
pub(crate) struct CtrlC {
    #[cfg(unix)]
    signal: tokio::signal::unix::Signal,
    #[cfg(windows)]
    signal: tokio::signal::windows::CtrlC,
}

impl CtrlC {
    pub(crate) fn new() -> Result<Self> {
        #[cfg(unix)]
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
        #[cfg(windows)]
        let signal = tokio::signal::windows::ctrl_c();
        signal
            .map(|signal| Self { signal })
            .map_err(|error| CruiseError::Other(format!("failed to listen for Ctrl+C: {error}")))
    }

    pub(crate) async fn recv(&mut self) {
        self.signal.recv().await;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CtrlCAction {
    Cancel,
    Exit,
}

fn ctrl_c_action(cancelled_by_user: &mut bool) -> CtrlCAction {
    if std::mem::replace(cancelled_by_user, true) {
        CtrlCAction::Exit
    } else {
        CtrlCAction::Cancel
    }
}

struct IgnoreSessionEvents;

impl SessionEventHandler for IgnoreSessionEvents {
    fn on_event(&mut self, _event: &ApplicationEvent) -> Result<()> {
        Ok(())
    }

    fn on_pending_prompts(&mut self, _prompts: &[PendingPrompt]) -> Result<()> {
        Ok(())
    }
}

/// Default terminal handler: prints the session's log output and answers its
/// prompts interactively.
pub struct TerminalEvents<'a> {
    client: &'a DaemonClient,
    session_id: String,
    plan_chunks: bool,
    failure: Option<String>,
    handled_prompts: HashSet<String>,
}

impl<'a> TerminalEvents<'a> {
    #[must_use]
    pub fn new(client: &'a DaemonClient, session_id: impl Into<String>) -> Self {
        Self {
            client,
            session_id: session_id.into(),
            plan_chunks: false,
            failure: None,
            handled_prompts: HashSet::new(),
        }
    }

    /// Also print streamed planning output.
    #[must_use]
    pub fn with_plan_chunks(mut self, enabled: bool) -> Self {
        self.plan_chunks = enabled;
        self
    }

    fn owns(&self, session_id: &str) -> bool {
        session_id == self.session_id
    }

    fn ask(&mut self, request_id: &str, question: &str) -> Result<()> {
        if !self.handled_prompts.insert(request_id.to_owned()) {
            return Ok(());
        }
        let answer = prompt_ask(&self.session_id, question)?;
        answer_or_note(
            self.client,
            &self.session_id,
            request_id,
            PromptAnswer::Answer(answer),
        )
    }

    fn option(
        &mut self,
        request_id: &str,
        prompt: Option<&str>,
        choices: &[OptionChoicePayload],
        has_image: bool,
    ) -> Result<()> {
        if !self.handled_prompts.insert(request_id.to_owned()) {
            return Ok(());
        }
        let result = prompt_option_with_image(
            self.client,
            &self.session_id,
            request_id,
            prompt,
            choices,
            has_image,
        )?;
        answer_or_note(
            self.client,
            &self.session_id,
            request_id,
            PromptAnswer::Option(result),
        )
    }
}

impl SessionEventHandler for TerminalEvents<'_> {
    fn on_event(&mut self, event: &ApplicationEvent) -> Result<()> {
        match event {
            ApplicationEvent::LogChunk {
                session_id: Some(id),
                stream,
                text,
                ..
            } if self.owns(id) => {
                print_output(*stream, text);
                Ok(())
            }
            ApplicationEvent::PlanChunk {
                session_id,
                stream,
                text,
            } if self.plan_chunks && self.owns(session_id) => {
                print_output(*stream, text);
                Ok(())
            }
            ApplicationEvent::RunFailed { session_id, error }
            | ApplicationEvent::PlanFailed { session_id, error }
                if self.owns(session_id) =>
            {
                self.failure = Some(error.clone());
                Ok(())
            }
            ApplicationEvent::RunPhase { session_id, phase }
                if self.owns(session_id) && phase == "Creating PR" =>
            {
                crate::status_eprintln!("{} {phase}", console::style("▶").cyan().bold());
                Ok(())
            }
            ApplicationEvent::PrCreated { session_id, url } if self.owns(session_id) => {
                crate::status_eprintln!("{} PR created: {url}", console::style("->").cyan());
                Ok(())
            }
            ApplicationEvent::AskUserRequired {
                session_id,
                request_id,
                question,
            } if self.owns(session_id) => self.ask(request_id, question),
            ApplicationEvent::OptionRequired {
                session_id,
                request_id,
                prompt,
                choices,
                has_image,
            } if self.owns(session_id) => {
                self.option(request_id, Some(prompt), choices, *has_image)
            }
            _ => Ok(()),
        }
    }

    fn is_terminal_state(&self, state: &SessionState) -> bool {
        if self.plan_chunks {
            state.plan_error.is_some()
                || matches!(
                    &state.phase,
                    SessionPhase::AwaitingApproval
                        | SessionPhase::AwaitingInput
                        | SessionPhase::Suspended
                        | SessionPhase::Completed
                        | SessionPhase::Failed(_)
                        | SessionPhase::PrFailed(_)
                )
        } else {
            matches!(
                &state.phase,
                SessionPhase::Completed
                    | SessionPhase::Failed(_)
                    | SessionPhase::PrFailed(_)
                    | SessionPhase::Suspended
            )
        }
    }

    fn terminal_failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn on_pending_prompts(&mut self, prompts: &[PendingPrompt]) -> Result<()> {
        for prompt in prompts {
            if !self.owns(&prompt.session_id) {
                continue;
            }
            match prompt.kind {
                PendingPromptKind::Ask => {
                    self.ask(&prompt.request_id, prompt.question.as_deref().unwrap_or(""))?;
                }
                PendingPromptKind::Option => {
                    self.option(
                        &prompt.request_id,
                        prompt.question.as_deref(),
                        &prompt.choices,
                        prompt.has_image,
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Prompt renderer and de-duplicator shared by batch runs.
pub struct BatchPrompts<'a> {
    client: &'a DaemonClient,
    sessions: HashSet<String>,
    handled: HashSet<(String, String)>,
}

impl<'a> BatchPrompts<'a> {
    #[must_use]
    pub fn new(client: &'a DaemonClient) -> Self {
        Self {
            client,
            sessions: HashSet::new(),
            handled: HashSet::new(),
        }
    }

    pub fn include_session(&mut self, session_id: &str) {
        self.sessions.insert(session_id.to_owned());
    }

    pub fn handle_event(&mut self, event: &ApplicationEvent) -> Result<()> {
        match event {
            ApplicationEvent::BatchSessionStarted { id } => self.include_session(id),
            ApplicationEvent::AskUserRequired {
                session_id,
                request_id,
                question,
            } if accepts_batch_prompt(&self.sessions, session_id) => {
                return self.ask(session_id, request_id, question);
            }
            ApplicationEvent::OptionRequired {
                session_id,
                request_id,
                prompt,
                choices,
                has_image,
            } if accepts_batch_prompt(&self.sessions, session_id) => {
                return self.option(session_id, request_id, Some(prompt), choices, *has_image);
            }
            _ => {}
        }
        Ok(())
    }

    pub fn handle_pending_prompts(&mut self, prompts: &[PendingPrompt]) -> Result<()> {
        for prompt in prompts {
            if !accepts_batch_prompt(&self.sessions, &prompt.session_id) {
                continue;
            }
            match prompt.kind {
                PendingPromptKind::Ask => self.ask(
                    &prompt.session_id,
                    &prompt.request_id,
                    prompt.question.as_deref().unwrap_or(""),
                )?,
                PendingPromptKind::Option => self.option(
                    &prompt.session_id,
                    &prompt.request_id,
                    prompt.question.as_deref(),
                    &prompt.choices,
                    prompt.has_image,
                )?,
            }
        }
        Ok(())
    }

    fn ask(&mut self, session_id: &str, request_id: &str, question: &str) -> Result<()> {
        let key = (session_id.to_owned(), request_id.to_owned());
        if !self.handled.insert(key) {
            return Ok(());
        }
        let answer = prompt_ask(session_id, question)?;
        answer_or_note(
            self.client,
            session_id,
            request_id,
            PromptAnswer::Answer(answer),
        )
    }

    fn option(
        &mut self,
        session_id: &str,
        request_id: &str,
        prompt: Option<&str>,
        choices: &[OptionChoicePayload],
        has_image: bool,
    ) -> Result<()> {
        let key = (session_id.to_owned(), request_id.to_owned());
        if !self.handled.insert(key) {
            return Ok(());
        }
        let answer = prompt_option_with_image(
            self.client,
            session_id,
            request_id,
            prompt,
            choices,
            has_image,
        )?;
        answer_or_note(
            self.client,
            session_id,
            request_id,
            PromptAnswer::Option(answer),
        )
    }
}

fn accepts_batch_prompt(sessions: &HashSet<String>, session_id: &str) -> bool {
    sessions.contains(session_id)
}

/// Print one daemon output chunk using the pre-daemon CLI formatting.
pub fn print_output(stream: OutputStream, text: &str) {
    for line in text.lines() {
        if is_parallel_child_line(line) {
            crate::status_eprintln!("{line}");
            continue;
        }
        match stream {
            OutputStream::Info => crate::status_eprintln!("{line}"),
            OutputStream::Stdout => crate::status_eprintln!("  {line}"),
            OutputStream::Stderr => {
                crate::status_eprintln!("  {} {line}", console::style("stderr:").dim());
            }
        }
    }
}

fn is_parallel_child_line(line: &str) -> bool {
    let Some((name, _)) = line
        .strip_prefix('[')
        .and_then(|line| line.split_once("] "))
    else {
        return false;
    };
    name.contains('/')
}

/// Read an Ask answer on the terminal. A cancelled editor or a non-terminal
/// stdin leaves the prompt pending in the daemon and pauses this client.
///
/// # Errors
///
/// Returns [`CruiseError::StepPaused`] when the prompt is left unanswered.
pub fn prompt_ask(session_id: &str, question: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        return Err(waiting_for_input(session_id));
    }
    let _blocked = crate::herdr::blocked(question);
    tokio::task::block_in_place(|| {
        crate::multiline_input::prompt_multiline(question)?.into_result()
    })
}

/// Render an Option menu on the terminal.
///
/// # Errors
///
/// Returns [`CruiseError::StepPaused`] when the menu is dismissed or stdin is
/// not a terminal.
pub fn prompt_option(
    session_id: &str,
    prompt: Option<&str>,
    choices: &[OptionChoicePayload],
) -> Result<crate::step::option::OptionResult> {
    run_option_menu(session_id, prompt, choices, None)
}

fn run_option_menu(
    session_id: &str,
    prompt: Option<&str>,
    choices: &[OptionChoicePayload],
    image: Option<&LocalOptionImage>,
) -> Result<crate::step::option::OptionResult> {
    if !std::io::stdin().is_terminal() {
        return Err(waiting_for_input(session_id));
    }
    let choices: Vec<OptionChoice> = choices.iter().map(option_choice).collect();
    let _blocked = crate::herdr::blocked(prompt.unwrap_or("Select an option"));
    tokio::task::block_in_place(|| {
        let _guard = crate::option_handler::prompt_lock_guard();
        if let Some(image) = image {
            image.show();
        }
        crate::step::option::run_option(&choices, prompt.filter(|text| !text.is_empty()))
    })
}

fn option_choice(payload: &OptionChoicePayload) -> OptionChoice {
    let label = payload.label.clone();
    let next = payload.next_step.clone();
    match payload.kind {
        OptionChoiceKind::Selector => OptionChoice::Selector { label, next },
        OptionChoiceKind::TextInput => OptionChoice::TextInput { label, next },
    }
}

struct LocalOptionImage {
    dir: std::path::PathBuf,
    png: Vec<u8>,
}

impl LocalOptionImage {
    fn show(&self) {
        let path = self.dir.join("quint.png");
        eprintln!("Quint PNG: {}", path.display());
        eprintln!("Open this file manually while the menu is active.");
        if !std::io::stderr().is_terminal()
            || std::env::var_os("TMUX").is_some()
            || std::env::var_os("STY").is_some()
        {
            return;
        }
        let term = std::env::var("TERM");
        let kitty =
            term.as_deref() == Ok("xterm-kitty") || std::env::var_os("KITTY_WINDOW_ID").is_some();
        let iterm = std::env::var("TERM_PROGRAM").as_deref() == Ok("iTerm.app");
        if term.as_deref() == Ok("dumb") || (!kitty && !iterm) {
            return;
        }
        let mut stderr = std::io::stderr().lock();
        let _ = write_terminal_png(&mut stderr, &self.png, kitty);
    }
}

fn write_terminal_png(
    output: &mut impl std::io::Write,
    png: &[u8],
    kitty: bool,
) -> std::io::Result<()> {
    use base64::{display::Base64Display, engine::general_purpose::STANDARD};
    let encode = |bytes| Base64Display::new(bytes, &STANDARD);
    if kitty {
        let mut chunks = png.chunks(3072).peekable();
        let mut first = true;
        while let Some(chunk) = chunks.next() {
            let control = if first { "a=T,f=100,q=2,r=12," } else { "" };
            write!(
                output,
                "\x1b_G{control}m={};{}\x1b\\",
                u8::from(chunks.peek().is_some()),
                encode(chunk)
            )?;
            first = false;
        }
    } else {
        const ARGS: &str = "inline=1;width=60%;height=12;preserveAspectRatio=1";
        if png.len() > 750_000 {
            write!(output, "\x1b]1337;MultipartFile={ARGS}\x07")?;
            for chunk in png.chunks(750_000) {
                write!(output, "\x1b]1337;FilePart={}\x07", encode(chunk))?;
            }
            output.write_all(b"\x1b]1337;FileEnd\x07")?;
        } else {
            write!(output, "\x1b]1337;File={ARGS}:{}\x07", encode(png))?;
        }
    }
    writeln!(output)?;
    output.flush()
}

impl Drop for LocalOptionImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn prompt_option_with_image(
    client: &DaemonClient,
    session_id: &str,
    request_id: &str,
    prompt: Option<&str>,
    choices: &[OptionChoicePayload],
    has_image: bool,
) -> Result<crate::step::option::OptionResult> {
    if !has_image {
        return prompt_option(session_id, prompt, choices);
    }
    if !std::io::stdin().is_terminal() {
        return Err(waiting_for_input(session_id));
    }
    let result: Result<_> = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let bytes = client.prompt_image(session_id, request_id).await?;
            let dir = std::env::temp_dir().join(format!("cruise-quint-{}", uuid::Uuid::new_v4()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
            }
            #[cfg(not(unix))]
            std::fs::create_dir(&dir)?;
            let guard = LocalOptionImage { dir, png: bytes };
            #[cfg(windows)]
            crate::platform::set_owner_only_acl(&guard.dir)?;
            std::fs::write(guard.dir.join("quint.png"), &guard.png)?;
            Ok(guard)
        })
    });
    let image = result.map_err(|error| {
        eprintln!("Could not download the Quint PNG: {error}. Use the WebUI to review it.");
        waiting_for_input(session_id)
    })?;
    run_option_menu(session_id, prompt, choices, Some(&image))
}

fn waiting_for_input(session_id: &str) -> CruiseError {
    eprintln!(
        "Session {session_id} is waiting for input. Answer it in the WebUI (`cruise webui`) or run `cruise run {session_id}` in a terminal."
    );
    CruiseError::StepPaused
}

/// Send an answer; a prompt already answered elsewhere (`WebUI`, another
/// terminal) is reported and otherwise ignored.
fn answer_or_note(
    client: &DaemonClient,
    session_id: &str,
    request_id: &str,
    answer: PromptAnswer,
) -> Result<()> {
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(client.answer_prompt(session_id, request_id, answer))
    });
    match result {
        Err(CruiseError::Busy(_)) => {
            eprintln!("This prompt was already answered elsewhere.");
            Ok(())
        }
        Err(CruiseError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("This prompt is no longer pending.");
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_png_preserves_image_bytes_across_protocol_chunks()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let png = vec![0x8f_u8; 800_000];
        for kitty in [true, false] {
            let mut output = Vec::new();
            write_terminal_png(&mut output, &png, kitty)?;
            let output = String::from_utf8(output)?;
            let mut payload = String::new();
            if kitty {
                let chunks: Vec<_> = output.trim_end().split_terminator("\x1b\\").collect();
                for (index, chunk) in chunks.iter().enumerate() {
                    let (control, data) = chunk.split_once(';').ok_or("missing Kitty separator")?;
                    assert_eq!(control.ends_with("m=0"), index + 1 == chunks.len());
                    assert!(data.len() <= 4096);
                    payload.push_str(data);
                }
            } else {
                assert!(output.starts_with("\x1b]1337;MultipartFile=inline=1;"));
                assert!(output.ends_with("\x1b]1337;FileEnd\x07\n"));
                for part in output
                    .split('\x07')
                    .filter_map(|part| part.strip_prefix("\x1b]1337;FilePart="))
                {
                    assert!(part.len() <= 1_000_000);
                    payload.push_str(part);
                }
            }
            assert_eq!(STANDARD.decode(payload)?, png);
        }
        Ok(())
    }

    #[test]
    fn selector_payload_keeps_the_selected_route() {
        let choice = option_choice(&OptionChoicePayload {
            label: "Take alternate path".to_owned(),
            kind: OptionChoiceKind::Selector,
            next_step: Some("alternate".to_owned()),
        });
        let OptionChoice::Selector { label, next } = choice else {
            panic!("selector payload became a text-input choice");
        };
        assert_eq!(label, "Take alternate path");
        assert_eq!(next.as_deref(), Some("alternate"));
    }

    #[test]
    fn text_input_payload_keeps_its_label_and_route() {
        let choice = option_choice(&OptionChoicePayload {
            label: "Provide a value".to_owned(),
            kind: OptionChoiceKind::TextInput,
            next_step: Some("capture".to_owned()),
        });
        let OptionChoice::TextInput { label, next } = choice else {
            panic!("text-input payload became a selector choice");
        };
        assert_eq!(label, "Provide a value");
        assert_eq!(next.as_deref(), Some("capture"));
    }

    #[test]
    fn second_ctrl_c_exits_without_another_cancel_request() {
        let mut cancelled_by_user = false;
        assert_eq!(ctrl_c_action(&mut cancelled_by_user), CtrlCAction::Cancel);
        assert_eq!(ctrl_c_action(&mut cancelled_by_user), CtrlCAction::Exit);
    }

    #[test]
    fn batch_prompt_filter_rejects_foreign_sessions() {
        let sessions = HashSet::from(["batch-session".to_owned()]);
        assert!(accepts_batch_prompt(&sessions, "batch-session"));
        assert!(!accepts_batch_prompt(&sessions, "foreign-session"));
    }
}
