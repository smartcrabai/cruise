use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use console::{Term, measure_text_width, style};
use indexmap::IndexMap;

use crate::error::{CruiseError, Result};
use crate::option_handler::{OptionHandler, prompt_lock_guard};
use crate::run_observer::{RunObserver, RunPhase};
use crate::session::{SessionPhase, SessionState};
use crate::step::OptionChoice;
use crate::step::option::OptionResult;

const DASHBOARD_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_TITLE_CHARS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) title: String,
    pub(crate) status: RowStatus,
    pub(crate) started: Instant,
    pub(crate) finished_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowStatus {
    Preparing,
    Step(String),
    CreatingPr,
    WaitingInput,
    Completed { pr_url: Option<String> },
    Failed(String),
    Suspended,
    Paused,
}

/// Live terminal dashboard for a `run --all` batch.
pub struct BatchDashboard {
    pub(crate) rows: Arc<Mutex<IndexMap<String, Row>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl BatchDashboard {
    fn lock_rows(&self) -> std::sync::MutexGuard<'_, IndexMap<String, Row>> {
        self.rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn start() -> Self {
        let rows = Arc::new(Mutex::new(IndexMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_rows = Arc::clone(&rows);
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let term = Term::stderr();
            let mut drawn_lines = 0usize;
            let mut last_prompt_epoch = crate::option_handler::prompt_epoch();

            while !thread_stop.load(Ordering::Relaxed) {
                repaint(
                    &term,
                    &thread_rows,
                    &mut drawn_lines,
                    &mut last_prompt_epoch,
                );
                thread::sleep(DASHBOARD_REFRESH_INTERVAL);
            }

            // The last paint is performed by the render thread, while its
            // dashboard state and terminal bookkeeping are still available.
            repaint(
                &term,
                &thread_rows,
                &mut drawn_lines,
                &mut last_prompt_epoch,
            );
            if drawn_lines > 0 {
                let _ = term.write_line("");
            }
        });

        Self {
            rows,
            stop,
            handle: Some(handle),
        }
    }

    /// Add a row when the scheduler starts a worker.
    pub fn add(&self, session: &SessionState) {
        let mut rows = self.lock_rows();
        rows.insert(
            session.id.clone(),
            Row {
                title: crate::display::truncate(
                    &sanitize_terminal_text(session.title_or_input()),
                    MAX_TITLE_CHARS,
                ),
                status: RowStatus::Preparing,
                started: Instant::now(),
                finished_at: None,
            },
        );
    }

    /// Mark a worker's terminal state and retain its elapsed time.
    pub fn finish(&self, id: &str, state: &SessionState, outcome: &Result<()>) {
        let mut rows = self.lock_rows();
        let Some(row) = rows.get_mut(id) else {
            return;
        };
        row.finished_at = Some(Instant::now());
        row.status = match outcome {
            Err(CruiseError::Interrupted) => RowStatus::Suspended,
            Err(CruiseError::StepPaused) => RowStatus::Paused,
            Err(error) => match &state.phase {
                SessionPhase::Failed(reason) => RowStatus::Failed(reason.clone()),
                _ => RowStatus::Failed(error.detailed_message()),
            },
            Ok(()) => match &state.phase {
                SessionPhase::Suspended => RowStatus::Suspended,
                SessionPhase::Failed(reason) => RowStatus::Failed(reason.clone()),
                _ => RowStatus::Completed {
                    pr_url: state.pr_url.clone(),
                },
            },
        };
    }

    /// Render a dashboard block without touching terminal state.
    pub(crate) fn render(rows: &[Row], width: usize, now: Instant) -> Vec<String> {
        let running = rows.iter().filter(|row| !is_finished(&row.status)).count();
        let done = rows.len() - running;
        let mut lines = vec![truncate_visible(
            &format!("run --all: {running} running, {done} done"),
            width,
        )];

        for (index, row) in rows.iter().enumerate() {
            let (icon, status) = status_parts(&row.status);
            let elapsed_at = row.finished_at.unwrap_or(now);
            let elapsed = elapsed_at.saturating_duration_since(row.started).as_secs();
            // A failed command can carry newlines in its error text. Keep
            // each dashboard row to one physical terminal line so redraw
            // bookkeeping remains correct.
            let line = format!(
                "[{}] {} {}  {}  {}s",
                index + 1,
                icon,
                row.title,
                status,
                elapsed
            )
            .replace(['\r', '\n'], " ");
            lines.push(truncate_visible(&line, width));
        }
        lines
    }
}

fn is_finished(status: &RowStatus) -> bool {
    matches!(
        status,
        RowStatus::Completed { .. }
            | RowStatus::Failed(_)
            | RowStatus::Suspended
            | RowStatus::Paused
    )
}

impl RunObserver for BatchDashboard {
    fn on_phase(&self, session_id: &str, phase: RunPhase) {
        let mut rows = self.lock_rows();
        let Some(row) = rows.get_mut(session_id) else {
            return;
        };
        row.status = match phase {
            RunPhase::Preparing => RowStatus::Preparing,
            RunPhase::Step(step) => RowStatus::Step(step),
            RunPhase::CreatingPr => RowStatus::CreatingPr,
            RunPhase::WaitingInput => RowStatus::WaitingInput,
        };
    }
}

impl Drop for BatchDashboard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn status_parts(status: &RowStatus) -> (console::StyledObject<&'static str>, String) {
    match status {
        RowStatus::Preparing => (style(">").cyan(), "Preparing".to_string()),
        RowStatus::Step(step) => (style(">").cyan(), sanitize_terminal_text(step)),
        RowStatus::CreatingPr => (style(">").cyan(), "Creating PR".to_string()),
        RowStatus::WaitingInput => (style("?").yellow().bold(), "Waiting input".to_string()),
        RowStatus::Completed { pr_url } => {
            let status = pr_url.as_deref().map_or_else(
                || "Completed".to_string(),
                |url| format!("Completed -> {}", sanitize_terminal_text(url)),
            );
            (style("v").green().bold(), status)
        }
        RowStatus::Failed(reason) => (
            style("x").red().bold(),
            format!("Failed: {}", sanitize_terminal_text(reason)),
        ),
        RowStatus::Suspended => (style("||").yellow().bold(), "Suspended".to_string()),
        RowStatus::Paused => (style("||").yellow().bold(), "Paused".to_string()),
    }
}

fn sanitize_terminal_text(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn truncate_visible(line: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if measure_text_width(line) <= width {
        line.to_string()
    } else {
        // `console::truncate_str` keeps an ellipsis even when it is wider
        // than the requested width, so omit it for very narrow terminals.
        let tail = if width >= measure_text_width("...") {
            "..."
        } else {
            ""
        };
        console::truncate_str(line, width, tail).into_owned()
    }
}

fn repaint(
    term: &Term,
    rows: &Arc<Mutex<IndexMap<String, Row>>>,
    drawn_lines: &mut usize,
    last_prompt_epoch: &mut u64,
) {
    let Some(_prompt_guard) = crate::option_handler::try_prompt_lock_guard() else {
        return;
    };
    let epoch = crate::option_handler::prompt_epoch();
    let reset_position = epoch != *last_prompt_epoch;
    let snapshot = rows
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if snapshot.is_empty() {
        return;
    }
    let values: Vec<Row> = snapshot.values().cloned().collect();
    let lines = BatchDashboard::render(&values, term.size().1 as usize, Instant::now());
    drop(snapshot);

    if reset_position {
        *drawn_lines = 0;
    } else if *drawn_lines > 0 {
        let _ = term.clear_last_lines(*drawn_lines);
    }
    for line in &lines {
        let _ = term.write_line(line);
    }
    *drawn_lines = lines.len();
    *last_prompt_epoch = epoch;
}

/// Option handler that coordinates an interactive option step with the dashboard.
pub struct DashboardOptionHandler {
    pub(crate) session_id: String,
    pub(crate) dashboard: Arc<BatchDashboard>,
}

impl OptionHandler for DashboardOptionHandler {
    fn select_option(&self, choices: &[OptionChoice], plan: Option<&str>) -> Result<OptionResult> {
        self.dashboard
            .on_phase(&self.session_id, RunPhase::WaitingInput);
        let _guard = prompt_lock_guard();
        let context = self.dashboard.rows.lock().ok().and_then(|rows| {
            rows.get_full(&self.session_id)
                .map(|(index, _, row)| (index + 1, row.title.clone()))
        });
        if let Some((index, title)) = context {
            eprintln!("[{index}] {title} needs input:");
        }
        // The CLI handler also acquires PROMPT_LOCK, so call the leaf prompt
        // directly while this handler owns the lock.
        crate::step::option::run_option(choices, plan)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::{BatchDashboard, DashboardOptionHandler, Row, RowStatus};
    use crate::error::CruiseError;
    use crate::option_handler::OptionHandler;
    use crate::run_observer::{RunObserver, RunPhase};
    use crate::session::{SessionPhase, SessionState};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn session(id: &str, title: Option<&str>, input: &str) -> SessionState {
        let mut state = SessionState::new(
            id.to_string(),
            PathBuf::from("/tmp/repo"),
            "cruise.yaml".to_string(),
            input.to_string(),
        );
        state.title = title.map(str::to_string);
        state.phase = SessionPhase::Planned;
        state
    }

    fn row(title: &str, status: RowStatus, started: Instant) -> Row {
        Row {
            title: title.to_string(),
            status,
            started,
            finished_at: None,
        }
    }

    fn dashboard_with_session(
        id: &str,
        title: Option<&str>,
        input: &str,
    ) -> (BatchDashboard, SessionState) {
        let dashboard = BatchDashboard::start();
        let scheduled = session(id, title, input);
        dashboard.add(&scheduled);
        (dashboard, scheduled)
    }

    fn plain(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|line| console::strip_ansi_codes(line).to_string())
            .collect()
    }

    #[test]
    fn add_uses_session_title_for_the_dashboard_row() {
        let (dashboard, scheduled) =
            dashboard_with_session("20260827000005", Some("Readable title"), "raw prompt");
        let rows = dashboard.rows.lock().unwrap_or_else(|e| panic!("{e}"));
        let row = rows
            .get(&scheduled.id)
            .unwrap_or_else(|| panic!("missing row"));
        assert_eq!(row.title, "Readable title");
    }

    #[test]
    fn render_preserves_scheduling_order_and_display_indexes() {
        let now = Instant::now();
        let rows = vec![
            row("first session", RowStatus::Preparing, now),
            row(
                "second session",
                RowStatus::Step("implement".to_string()),
                now,
            ),
            row("third session", RowStatus::Completed { pr_url: None }, now),
        ];
        let lines = plain(&BatchDashboard::render(&rows, 120, now));
        assert_eq!(lines[0], "run --all: 2 running, 1 done");
        let first = lines
            .iter()
            .position(|line| line.contains("first session"))
            .unwrap();
        let second = lines
            .iter()
            .position(|line| line.contains("second session"))
            .unwrap();
        let third = lines
            .iter()
            .position(|line| line.contains("third session"))
            .unwrap();
        assert!(first < second && second < third);
        assert!(lines[first].starts_with("[1] "));
        assert!(lines[second].starts_with("[2] "));
        assert!(lines[third].starts_with("[3] "));
    }

    #[test]
    fn render_shows_each_phase_status_and_elapsed_time() {
        let now = Instant::now();
        let rows = vec![
            row("preparing", RowStatus::Preparing, now),
            row("step", RowStatus::Step("review".to_string()), now),
            row("pr", RowStatus::CreatingPr, now),
            row("input", RowStatus::WaitingInput, now),
            row("complete", RowStatus::Completed { pr_url: None }, now),
            row("failed", RowStatus::Failed("build failed".to_string()), now),
            row("suspended", RowStatus::Suspended, now),
            row("paused", RowStatus::Paused, now),
        ];
        let lines = plain(&BatchDashboard::render(
            &rows,
            120,
            now + Duration::from_secs(2),
        ));
        assert!(lines.iter().any(|line| line.contains("Preparing")));
        assert!(lines.iter().any(|line| line.contains("review")));
        assert!(lines.iter().any(|line| line.contains("Creating PR")));
        assert!(lines.iter().any(|line| line.contains("Waiting input")));
        assert!(lines.iter().any(|line| line.contains("Completed")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Failed: build failed"))
        );
        assert!(lines.iter().any(|line| line.contains("Suspended")));
        assert!(lines.iter().any(|line| line.contains("Paused")));
        assert!(lines.iter().skip(1).all(|line| line.contains("2s")));
    }

    #[test]
    fn render_uses_finish_time_for_completed_elapsed_duration() {
        let started = Instant::now();
        let finished = started + Duration::from_secs(1);
        let mut completed = row(
            "finished task",
            RowStatus::Completed { pr_url: None },
            started,
        );
        completed.finished_at = Some(finished);
        let lines = plain(&BatchDashboard::render(
            &[completed],
            120,
            started + Duration::from_secs(10),
        ));
        assert!(lines[1].contains("1s"));
        assert!(!lines[1].contains("10s"));
    }

    #[test]
    fn render_replaces_terminal_control_characters_in_session_data() {
        let now = Instant::now();
        let rows = vec![row(
            "title\u{1b}[2J",
            RowStatus::Failed("failure\u{7}".to_string()),
            now,
        )];
        let lines = plain(&BatchDashboard::render(&rows, 120, now));
        assert!(!lines[1].contains('\u{1b}'));
        assert!(!lines[1].contains('\u{7}'));
    }

    #[test]
    fn render_keeps_error_details_on_one_physical_line() {
        let now = Instant::now();
        let rows = vec![row(
            "multiline failure",
            RowStatus::Failed("first line\nsecond line".to_string()),
            now,
        )];
        let lines = plain(&BatchDashboard::render(&rows, 120, now));
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("first line second line"));
    }

    #[test]
    fn render_includes_pull_request_url_for_completed_rows() {
        let now = Instant::now();
        let rows = vec![row(
            "publish change",
            RowStatus::Completed {
                pr_url: Some("https://github.com/acme/app/pull/42".to_string()),
            },
            now,
        )];
        let lines = plain(&BatchDashboard::render(&rows, 120, now));
        assert!(lines[1].contains("Completed"));
        assert!(lines[1].contains("https://github.com/acme/app/pull/42"));
    }

    #[test]
    fn render_truncates_every_line_to_terminal_width() {
        let now = Instant::now();
        let rows = vec![row(
            &"long title ".repeat(30),
            RowStatus::Failed("long failure detail ".repeat(30)),
            now,
        )];
        for width in 1..=32 {
            let lines = BatchDashboard::render(&rows, width, now);
            assert!(
                lines
                    .iter()
                    .all(|line| console::measure_text_width(line) <= width),
                "rendered line exceeds terminal width {width}"
            );
        }
    }

    #[test]
    fn observer_maps_run_phases_to_the_matching_row_status() {
        let (dashboard, scheduled) = dashboard_with_session("20260827000000", None, "phase input");
        let observer: &dyn RunObserver = &dashboard;
        let status = || {
            dashboard
                .rows
                .lock()
                .unwrap()
                .get(&scheduled.id)
                .map(|row| row.status.clone())
        };
        observer.on_phase(&scheduled.id, RunPhase::Preparing);
        assert_eq!(status(), Some(RowStatus::Preparing));
        observer.on_phase(&scheduled.id, RunPhase::Step("review".to_string()));
        assert_eq!(status(), Some(RowStatus::Step("review".to_string())));
        observer.on_phase(&scheduled.id, RunPhase::CreatingPr);
        assert_eq!(status(), Some(RowStatus::CreatingPr));
        observer.on_phase(&scheduled.id, RunPhase::WaitingInput);
        assert_eq!(status(), Some(RowStatus::WaitingInput));
    }

    #[test]
    fn finish_maps_completed_state_and_pr_url_to_completed_row() {
        let (dashboard, scheduled) =
            dashboard_with_session("20260827000001", Some("publish"), "publish input");
        let mut completed = scheduled.clone();
        completed.phase = SessionPhase::Completed;
        completed.pr_url = Some("https://github.com/acme/app/pull/7".to_string());
        dashboard.finish(&scheduled.id, &completed, &Ok(()));
        assert!(
            matches!(dashboard.rows.lock().unwrap().get(&scheduled.id).map(|row| &row.status), Some(RowStatus::Completed { pr_url }) if pr_url.as_deref() == Some("https://github.com/acme/app/pull/7"))
        );
    }

    #[test]
    fn finish_maps_interrupted_state_to_suspended_row() {
        let (dashboard, scheduled) = dashboard_with_session("20260827000002", None, "cancel input");
        let mut suspended = scheduled.clone();
        suspended.phase = SessionPhase::Suspended;
        dashboard.finish(&scheduled.id, &suspended, &Err(CruiseError::Interrupted));
        assert!(matches!(
            dashboard
                .rows
                .lock()
                .unwrap()
                .get(&scheduled.id)
                .map(|row| &row.status),
            Some(RowStatus::Suspended)
        ));
    }

    #[test]
    fn finish_maps_step_paused_to_paused_row() {
        let (dashboard, scheduled) = dashboard_with_session("20260827000006", None, "paused input");
        let mut paused = scheduled.clone();
        paused.phase = SessionPhase::Running;
        dashboard.finish(&scheduled.id, &paused, &Err(CruiseError::StepPaused));
        assert!(matches!(
            dashboard
                .rows
                .lock()
                .unwrap()
                .get(&scheduled.id)
                .map(|row| &row.status),
            Some(RowStatus::Paused)
        ));
    }

    #[test]
    fn finish_maps_failed_state_to_failed_row_with_reason() {
        let (dashboard, scheduled) =
            dashboard_with_session("20260827000003", None, "failure input");
        let mut failed = scheduled.clone();
        failed.phase = SessionPhase::Failed("compile error".to_string());
        dashboard.finish(
            &scheduled.id,
            &failed,
            &Err(CruiseError::Other("compile error".to_string())),
        );
        assert!(
            matches!(dashboard.rows.lock().unwrap().get(&scheduled.id).map(|row| &row.status), Some(RowStatus::Failed(reason)) if reason == "compile error")
        );
    }

    #[test]
    fn option_handler_marks_waiting_input_and_delegates_empty_selection() {
        let (dashboard, scheduled) =
            dashboard_with_session("20260827000004", Some("choose"), "choose input");
        let dashboard = Arc::new(dashboard);
        let handler = DashboardOptionHandler {
            session_id: scheduled.id.clone(),
            dashboard: Arc::clone(&dashboard),
        };
        let result = handler.select_option(&[], Some("context")).unwrap();
        assert!(result.next_step.is_none());
        assert!(result.text_input.is_none());
        assert!(matches!(
            dashboard
                .rows
                .lock()
                .unwrap()
                .get(&scheduled.id)
                .map(|row| &row.status),
            Some(RowStatus::WaitingInput)
        ));
    }
}
