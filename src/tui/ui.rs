use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap,
};

#[cfg(test)]
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::application::{OptionChoiceKind, SessionAction};
use crate::session::{SessionPhase, WorkspaceMode};

use super::app::{DetailTab, Modal, SidebarStatus, TuiApp, View, action_label};
use super::forms::{Editor, Launch, SourceKind, Step};
pub fn draw(frame: &mut Frame<'_>, app: &mut TuiApp) {
    let area = frame.area();
    if area.width < 80 || area.height < 24 {
        frame.render_widget(
            Paragraph::new("Terminal too small — resize to at least 80×24")
                .style(warning(app))
                .block(panel(app, " Cruise TUI ", true)),
            area,
        );
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(area);
    render_header(frame, app, chunks[0]);
    match app.view {
        View::Sessions => render_sessions(frame, app, chunks[1]),
        View::NewSession => render_new_session(frame, app, chunks[1]),
        View::RunAll => render_run_all(frame, app, chunks[1]),
    }
    render_footer(frame, app, chunks[2]);
    if let Some(modal) = app.modal.as_ref() {
        render_modal(frame, app, area, modal);
    }
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let mut spans = vec![Span::styled(" CRUISE ", selection(app)), Span::raw("  ")];
    for (view, label) in [
        (View::Sessions, " 1 Sessions "),
        (View::NewSession, " 2/n New Session "),
        (View::RunAll, " 3 Run All "),
    ] {
        spans.push(Span::styled(
            label,
            if app.view == view {
                active_nav(app)
            } else {
                muted(app)
            },
        ));
        spans.push(Span::raw(" "));
    }
    if app.is_busy() {
        let spinner = ["⠋", "⠙", "⠹", "⠸"][app.spinner_frame];
        spans.push(Span::styled(format!(" {spinner} working"), warning(app)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(border(app, false)),
        ),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(border(app, false));
    if app.view == View::Sessions {
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let status = Line::from(footer_status_spans(app));
        let hints = Line::from(footer_spans(app, Vec::new()));
        let hint_width = u16::try_from(hints.width())
            .unwrap_or(u16::MAX)
            .min(inner.width.saturating_sub(1));
        let split =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(hint_width)]).split(inner);
        frame.render_widget(Paragraph::new(status), split[0]);
        frame.render_widget(Paragraph::new(hints), split[1]);
    } else {
        frame.render_widget(Paragraph::new(footer_line(app)).block(block), area);
    }
}

fn footer_status_spans(app: &TuiApp) -> Vec<Span<'_>> {
    let status = app.status.as_deref().unwrap_or("Ready");
    let busy = app.is_busy();
    let mut spans = vec![
        Span::styled(
            if busy { " ◉ " } else { " ● " },
            if busy { warning(app) } else { success(app) },
        ),
        Span::raw(status),
    ];
    let prompt_count = app.prompts.len() + app.plan_prompts.len();
    if prompt_count > 0 {
        spans.push(Span::styled(
            format!("  {prompt_count} prompt(s)"),
            warning(app),
        ));
    }
    if app.dropped_logs > 0 {
        spans.push(Span::styled(
            format!("  {} logs dropped", app.dropped_logs),
            error_style(app),
        ));
    }
    spans
}

fn footer_spans<'a>(app: &TuiApp, mut spans: Vec<Span<'a>>) -> Vec<Span<'a>> {
    let hints: &[(&str, &str)] = match app.view {
        View::Sessions if app.sessions.is_empty() => {
            &[("n", "new"), ("c", "clean"), ("?", "help"), ("q", "quit")]
        }
        View::Sessions => &[
            ("Enter", "actions"),
            ("c", "clean"),
            ("Tab", "detail"),
            ("?", "help"),
            ("q", "quit"),
        ],
        View::NewSession => &[
            ("Enter", "next"),
            ("Shift-Tab", "back"),
            ("Ctrl+P/G/U", "start now"),
            ("?", "help"),
        ],
        View::RunAll => &[("Enter", "run/stop"), ("?", "help"), ("q", "quit")],
    };
    spans.reserve(hints.len() * 4);
    for &(shortcut, description) in hints {
        spans.extend([
            Span::raw("   "),
            Span::styled(shortcut, key(app)),
            Span::raw(" "),
            Span::styled(description, muted(app)),
        ]);
    }
    spans
}

fn footer_line(app: &TuiApp) -> Line<'_> {
    Line::from(footer_spans(app, footer_status_spans(app)))
}
fn render_sessions(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    app.load_tab_data();
    let layout = if area.width >= 120 {
        Layout::horizontal([Constraint::Length(34), Constraint::Min(1)])
    } else {
        Layout::vertical([Constraint::Length(7), Constraint::Min(1)])
    }
    .spacing(1);
    let sections = layout.split(area);
    render_sidebar(frame, app, sections[0]);
    render_detail(frame, app, sections[1]);
}

fn render_sidebar(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let row_width = usize::from(area.width)
        .saturating_sub(2)
        .saturating_sub(Line::from("▸ ").width());
    let items = app.sessions.iter().map(|session| {
        let status = app.sidebar_status(session);
        let prefix = format!("{} ", status.symbol());
        let suffix = format!(" · {}", status.label());
        let status_style = sidebar_status_style(app, status);
        let title_width = row_width
            .saturating_sub(Line::from(prefix.as_str()).width())
            .saturating_sub(Line::from(suffix.as_str()).width());
        ListItem::new(Line::from(vec![
            Span::styled(prefix, status_style),
            Span::raw(truncate_sidebar_title(
                session.title_or_input(),
                title_width,
            )),
            Span::styled(suffix, status_style),
        ]))
    });
    let mut state =
        ListState::default().with_selected((!app.sessions.is_empty()).then_some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(
                app,
                format!(" Sessions  {} ", app.sessions.len()),
                true,
            ))
            .highlight_style(sidebar_selection(app))
            .highlight_symbol("▸ "),
        area,
        &mut state,
    );
}

fn render_detail(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(session) = app.active_session() else {
        frame.render_widget(
            Paragraph::new(
                "\n  No sessions yet\n\n  Press n, type a task, then Tab through the questions\n  or press Ctrl+P/G/U to start right away.",
            )
            .block(panel(app, " Detail ", false)),
            area,
        );
        return;
    };
    let tabs = Tabs::new(
        [
            DetailTab::Info,
            DetailTab::Dag,
            DetailTab::Plan,
            DetailTab::Log,
        ]
        .into_iter()
        .map(|tab| Line::from(format!(" {} ", tab.label()))),
    )
    .select(match app.tab {
        DetailTab::Info => 0,
        DetailTab::Dag => 1,
        DetailTab::Plan => 2,
        DetailTab::Log => 3,
    })
    .block(panel(app, format!(" {} ", session.title_or_input()), false))
    .style(muted(app))
    .highlight_style(active_nav(app))
    .divider(Span::styled(" │ ", border(app, false)));
    let vertical = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);
    frame.render_widget(tabs, vertical[0]);
    match app.tab {
        DetailTab::Info => render_info(frame, app, session, vertical[1]),
        DetailTab::Dag => render_dag(frame, app, vertical[1]),
        DetailTab::Plan => render_plan(frame, app, vertical[1]),
        DetailTab::Log => render_log(frame, app, vertical[1]),
    }
}

fn render_info(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    session: &crate::session::SessionState,
    area: Rect,
) {
    let mut lines = vec![
        labeled_line(app, "ID       ", Span::raw(session.id.as_str())),
        labeled_line(
            app,
            "Phase    ",
            Span::styled(
                app.display_phase(session),
                if app.display_phase(session) == "Planning" {
                    accent(app, false)
                } else {
                    phase_style(app, &session.phase)
                },
            ),
        ),
        labeled_line(app, "Source   ", Span::raw(session.config.display_label())),
        labeled_line(
            app,
            "Directory",
            Span::raw(format!(" {}", session.base_dir.display())),
        ),
        labeled_line(
            app,
            "Workspace",
            Span::raw(workspace_label(session.workspace_mode)),
        ),
        labeled_line(
            app,
            "Step     ",
            Span::raw(session.current_step.as_deref().unwrap_or("—")),
        ),
        labeled_line(
            app,
            "PR       ",
            Span::raw(session.pr_url.as_deref().unwrap_or("—")),
        ),
        labeled_line(
            app,
            "Issue    ",
            Span::raw(session.published_issue_url.as_deref().unwrap_or("—")),
        ),
        labeled_line(app, "Input    ", Span::raw(session.input.as_str())),
    ];
    if let SessionPhase::Failed(error) = &session.phase {
        lines.push(Line::from(vec![
            Span::styled("Run error", error_style(app)),
            Span::raw(format!(" {error}")),
        ]));
    }
    if let Some(error) = session.plan_error.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("Plan error", error_style(app)),
            Span::raw(format!(" {error}")),
        ]));
    }
    let actions = app
        .application
        .capabilities(session)
        .iter()
        .map(|action| action_label(*action))
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(labeled_line(app, "Actions  ", Span::raw(actions)));
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, " Session information ", false))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn labeled_line<'a>(app: &TuiApp, name: &'a str, value: Span<'a>) -> Line<'a> {
    Line::from(vec![Span::styled(name, label(app)), value])
}

fn error_style(app: &TuiApp) -> Style {
    colored(app, Color::Rgb(248, 113, 113)).add_modifier(Modifier::BOLD)
}

fn render_dag(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(dag) = app.active_dag() else {
        frame.render_widget(
            Paragraph::new("\n  Graph unavailable. Check the session workflow configuration.")
                .style(muted(app))
                .block(panel(app, " Graph ", false)),
            area,
        );
        return;
    };
    let split = Layout::horizontal([Constraint::Length(32), Constraint::Min(1)])
        .spacing(1)
        .split(area);
    let items = dag.nodes.values().map(|node| {
        ListItem::new(Line::from(vec![
            Span::styled(node.id.as_str(), accent(app, true)),
            Span::styled("  ", muted(app)),
            Span::raw(node.step_name.as_str()),
        ]))
    });
    let mut state = ListState::default().with_selected(
        (!dag.nodes.is_empty()).then_some(app.dag_selected.min(dag.nodes.len().saturating_sub(1))),
    );
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(app, format!(" Nodes  {} ", dag.nodes.len()), true))
            .highlight_style(selection(app))
            .highlight_symbol("▸ "),
        split[0],
        &mut state,
    );
    let mut lines = Vec::new();
    if let Some(node) = state
        .selected()
        .and_then(|index| dag.nodes.values().nth(index))
    {
        lines.push(Line::from(vec![
            Span::styled("NODE  ", label(app)),
            Span::styled(node.id.as_str(), accent(app, true)),
            Span::raw(format!("  {}", node.step_name)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("INCOMING", label(app))));
        if let Some(predecessors) = dag.predecessors.get(&node.id) {
            for predecessor_id in predecessors {
                if let Some(predecessor) = dag.nodes.get(predecessor_id) {
                    lines.push(Line::from(format!(
                        "  {}  ←  {}",
                        predecessor.id, predecessor.step_name
                    )));
                }
            }
        }
        lines.push(Line::from(Span::styled("TRANSITIONS", label(app))));
        for successor in &node.successors {
            lines.push(Line::from(format!(
                "  {:?}  →  {}{}",
                successor.reason,
                successor.target.as_deref().unwrap_or("end"),
                successor
                    .target
                    .as_ref()
                    .and_then(|to| dag.state.edge_counts.get(&(node.id.clone(), to.clone())))
                    .map_or_else(String::new, |count| format!(
                        "  [{} traversals, {} budgeted]",
                        count.traversals, count.budgeted_traversals
                    ))
            )));
        }
        if let Some(visited) = node.runtime.visited_at.as_deref() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("LAST VISITED  ", label(app)),
                Span::raw(visited),
            ]));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, " Selected node ", false))
            .wrap(Wrap { trim: false }),
        split[1],
    );
}
fn render_plan(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    if let Some(prompt) = app.active_plan_prompt() {
        render_plan_prompt(frame, app, area, prompt);
        return;
    }
    if let Some(question) = app
        .active_external_plan_input()
        .map(|session| session.pending_ask_question.clone())
    {
        render_external_plan_input(frame, app, area, question.as_deref());
        return;
    }
    let text = app
        .active_plan()
        .unwrap_or("No plan has been generated for this session.");
    let parsed = tui_markdown::from_str(text);
    frame.render_widget(
        Paragraph::new(if app.display.no_color {
            strip_text_styles(parsed)
        } else {
            parsed
        })
        .scroll((u16::try_from(app.plan_scroll).unwrap_or(u16::MAX), 0))
        .block(panel(app, " Plan  Markdown ", false))
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_external_plan_input(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    area: Rect,
    question: Option<&str>,
) {
    let block = panel(app, " Plan  Awaiting Input ", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let areas = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    let question = question.unwrap_or("No question was persisted by the owning process.");
    let question = question
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .map(|line| Line::from(Span::styled(line, accent(app, true))))
        .collect::<Vec<_>>();
    let question_block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(app, false))
        .title(" Question  read-only ");
    frame.render_widget(
        Paragraph::new(question)
            .block(question_block)
            .wrap(Wrap { trim: false }),
        areas[0],
    );
    frame.render_widget(
        Paragraph::new("Read-only: answer this request in the process that owns it.")
            .style(muted(app))
            .wrap(Wrap { trim: false }),
        areas[1],
    );
}

fn render_plan_prompt(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    area: Rect,
    prompt: &super::prompts::PlanPromptItem,
) {
    let block = panel(app, " Plan  Awaiting Input ", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let constraints = if prompt.error.is_some() {
        vec![
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ]
    };
    let areas = Layout::vertical(constraints).split(inner);
    let question = prompt
        .question
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .map(|line| Line::from(Span::styled(line, accent(app, true))))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(question)
            .scroll((u16::try_from(prompt.question_scroll).unwrap_or(u16::MAX), 0))
            .wrap(Wrap { trim: false }),
        areas[0],
    );
    collapse_wide_placeholders(frame, areas[0]);

    let answer_title = if prompt.editing {
        " Answer  editing "
    } else {
        " Answer  press Enter to edit "
    };
    let answer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if prompt.editing {
            accent(app, true)
        } else {
            border(app, false)
        })
        .title(answer_title);
    let answer_inner = answer_block.inner(areas[1]);
    frame.render_widget(answer_block, areas[1]);
    frame.render_widget(prompt.answer.widget(), answer_inner);

    let mut guide = if prompt.editing {
        "Enter submit   Esc leave editing"
    } else {
        "Enter edit   scroll ↑↓ PgUp/PgDn Home/End   Esc leave"
    };
    if prompt.error.is_some() {
        guide = "Enter edit/submit   Esc leave editing";
    }
    let guide_area = areas[areas.len() - 1];
    if let Some(error) = prompt.error.as_deref() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(error, error_style(app))))
                .wrap(Wrap { trim: false }),
            areas[areas.len() - 2],
        );
    }
    frame.render_widget(Paragraph::new(guide).style(muted(app)), guide_area);
}

fn collapse_wide_placeholders(frame: &mut Frame<'_>, area: Rect) {
    let mut y = area.y;
    while y < area.bottom() {
        let mut x = area.x;
        while x < area.right() {
            let symbol = frame
                .buffer_mut()
                .cell((x, y))
                .map(|cell| cell.symbol().to_string())
                .unwrap_or_default();
            let width = u16::try_from(Line::from(symbol.as_str()).width()).unwrap_or(u16::MAX);
            if width > 1 {
                if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
                    cell.set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(
                        std::num::NonZeroU16::new(1).unwrap_or_else(|| unreachable!()),
                    ));
                }
                for offset in 1..width {
                    if let Some(cell) = frame.buffer_mut().cell_mut((x + offset, y)) {
                        cell.set_symbol("");
                    }
                }
                x = x.saturating_add(width);
            } else {
                x = x.saturating_add(1);
            }
        }
        y = y.saturating_add(1);
    }
}

fn strip_text_styles(text: Text<'_>) -> Text<'_> {
    text.lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| Span::raw(span.content))
                .collect::<Line<'_>>()
        })
        .collect()
}

fn render_log(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let height = area.height.saturating_sub(2) as usize;
    let visible = app.visible_lines(height);
    let title = if app.display.follow_log {
        "Log (following)"
    } else {
        "Log (paused)"
    };
    frame.render_widget(
        Paragraph::new(visible.into_iter().map(Line::from).collect::<Vec<_>>())
            .block(panel(app, format!(" {title} "), false))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_new_session(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    let steps = form.steps().collect::<Vec<_>>();
    let current = steps
        .iter()
        .position(|step| *step == form.step)
        .unwrap_or(0);
    let answered = &steps[..current];
    let upcoming = &steps[current + 1..];
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::try_from(answered.len()).unwrap_or(u16::MAX)),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(u16::try_from(upcoming.len()).unwrap_or(u16::MAX)),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" NEW SESSION  ", label(app)),
            Span::styled(
                format!("question {} of {}", current + 1, steps.len()),
                muted(app),
            ),
        ])),
        rows[0],
    );
    let answer_width = usize::from(area.width).saturating_sub(26);
    frame.render_widget(
        Paragraph::new(
            answered
                .iter()
                .map(|step| {
                    Line::from(vec![
                        Span::styled("  ✓ ", success(app)),
                        Span::styled(format!("{:<20}  ", step.label()), muted(app)),
                        Span::raw(truncate(&form.answer(*step), answer_width)),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ▸ ", accent(app, true)),
            Span::styled(step_question(form.step), accent(app, true)),
        ])),
        rows[2],
    );
    let [_, control] =
        Layout::horizontal([Constraint::Length(4), Constraint::Min(1)]).areas(rows[3]);
    render_step_control(frame, app, control);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("    {}", step_hint(app)),
            muted(app),
        ))),
        rows[4],
    );
    frame.render_widget(
        Paragraph::new(
            upcoming
                .iter()
                .map(|step| Line::from(Span::styled(format!("    {}", step.label()), muted(app))))
                .collect::<Vec<_>>(),
        ),
        rows[5],
    );
}

fn step_question(step: Step) -> &'static str {
    match step {
        Step::Task => "What should cruise do?",
        Step::Attachments => "Any images to attach? (optional; one path per line)",
        Step::Source => "Where is the code?",
        Step::WorkingDirectory => "Which directory? (blank = current directory)",
        Step::Repository => "Which GitHub repository? (owner/name)",
        Step::Config => "Which workflow config? (blank = auto-detect)",
        Step::SkippedSteps => "Skip any workflow steps?",
        Step::Workspace => "Where should cruise execute?",
        Step::DirtyTree => "Run on the current branch even with uncommitted changes?",
        Step::FormalSpec => "Include Quint and Alloy formal specifications in the plan?",
        Step::Launch => "How should this session start?",
    }
}

fn step_hint(app: &TuiApp) -> String {
    const BACK: &str = "Shift-Tab/Esc back";
    match app.form.step {
        Step::Task => {
            "Enter newline · Tab or Ctrl+Enter next · Ctrl+P/G/U start now · Esc leave".to_string()
        }
        Step::Attachments => format!("Enter newline · Tab complete path, then next · {BACK}"),
        Step::WorkingDirectory => {
            let recent = app
                .history_summary
                .as_ref()
                .map_or(0, |summary| summary.recent_working_dirs.len());
            if recent == 0 {
                format!("Enter next · Tab complete path · {BACK}")
            } else {
                format!("Enter next · Tab complete path · ↑↓ recent ({recent}) · {BACK}")
            }
        }
        Step::Repository => {
            let found = app.github_repositories.len();
            if found == 0 {
                format!("Enter next · {BACK}")
            } else {
                format!("Enter next · ↑↓ gh repos ({found}) · {BACK}")
            }
        }
        Step::Config => format!("↑↓ select · type path · Tab complete · Enter next · {BACK}"),
        Step::SkippedSteps if app.skip_choices().is_empty() => {
            format!("Comma-separated step ids · Enter next · {BACK}")
        }
        Step::SkippedSteps => format!("↑↓ move · Space toggle · Enter next · {BACK}"),
        Step::Launch => format!("↑↓ choose · Enter go · {BACK}"),
        Step::Source | Step::Workspace | Step::DirtyTree | Step::FormalSpec => {
            format!("↑↓ or Space choose · Enter next · {BACK}")
        }
    }
}

fn render_step_control(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    let two_way = |first: &'static str, second: &'static str, second_selected: bool| {
        vec![
            choice_line(app, first, !second_selected),
            choice_line(app, second, second_selected),
        ]
    };
    let lines = match form.step {
        Step::Task => return render_step_editor(frame, app, area, &form.input),
        Step::Attachments => return render_step_editor(frame, app, area, &form.attachments),
        Step::WorkingDirectory => return render_step_editor(frame, app, area, &form.working_dir),
        Step::Repository => return render_step_editor(frame, app, area, &form.repository),
        Step::Config => return render_config_choices(frame, app, area),
        Step::SkippedSteps => return render_skip_choices(frame, app, area),
        Step::Source => two_way(
            "Directory",
            "GitHub repository (cloned with gh)",
            form.source == SourceKind::GitHub,
        ),
        Step::Workspace => two_way(
            "Worktree (isolated branch checkout)",
            "Current branch (in place)",
            form.workspace_mode == WorkspaceMode::CurrentBranch,
        ),
        Step::DirtyTree => two_way(
            "No, require a clean working tree",
            "Yes, allow uncommitted changes",
            form.options.allow_dirty_working_tree,
        ),
        Step::FormalSpec => two_way(
            "No",
            "Yes, include Quint and Alloy specifications",
            form.options.planning.formal_spec,
        ),
        Step::Launch => Launch::ALL
            .iter()
            .map(|launch| {
                choice_line(
                    app,
                    format!("{:<46}{}", launch.label(), launch.shortcut()),
                    *launch == form.launch,
                )
            })
            .collect(),
    };
    frame.render_widget(Paragraph::new(lines), area);
}

fn choice_line(app: &TuiApp, text: impl Into<String>, selected: bool) -> Line<'static> {
    let text = text.into();
    Line::from(vec![
        Span::styled(if selected { "▸ " } else { "  " }, accent(app, true)),
        Span::styled(
            text,
            if selected {
                selection(app)
            } else {
                Style::default()
            },
        ),
    ])
}

fn render_step_editor(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, editor: &Editor) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(border(app, true));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(editor.widget(), inner);
}

fn render_config_choices(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let [editor_area, choices_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    render_step_editor(frame, app, editor_area, &app.form.config);

    let current = app.form.config.text();
    let current = current.trim();
    let mut selected = None;
    let mut lines = Vec::with_capacity(app.config_sources.len() + 1);
    lines.push(choice_line(app, "Auto-detect", current.is_empty()));
    for (index, source) in app.config_sources.iter().enumerate() {
        let source_selected = selected.is_none() && source.selection_value() == current;
        selected = selected.or(source_selected.then_some(index + 1));
        lines.push(choice_line(app, source.label(), source_selected));
    }
    let selected = current.is_empty().then_some(0).or(selected);
    let visible = usize::from(choices_area.height.max(1));
    let offset = selected
        .unwrap_or(0)
        .saturating_sub(visible.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
        choices_area,
    );
}
fn render_skip_choices(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let choices = app.skip_choices();
    if choices.is_empty() {
        render_step_editor(frame, app, area, &app.form.skipped);
        return;
    }
    let selected = app
        .form
        .selected_skipped_steps()
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let cursor = app.skip_cursor % choices.len();
    let lines = choices
        .iter()
        .enumerate()
        .map(|(index, (label, ids))| {
            let checked = ids.iter().all(|id| selected.contains(id));
            Line::from(vec![
                Span::styled(if index == cursor { "▸ " } else { "  " }, accent(app, true)),
                Span::styled(
                    if checked { "[x] " } else { "[ ] " },
                    if checked { success(app) } else { muted(app) },
                ),
                Span::styled(
                    label.clone(),
                    if index == cursor {
                        selection(app)
                    } else {
                        Style::default()
                    },
                ),
            ])
        })
        .collect::<Vec<_>>();
    let visible = usize::from(area.height.max(1));
    let offset = cursor.saturating_sub(visible - 1);
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
        area,
    );
}

fn render_run_all(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let split = if area.width >= 120 {
        Layout::horizontal([Constraint::Percentage(48), Constraint::Min(1)])
    } else {
        Layout::vertical([Constraint::Percentage(55), Constraint::Min(1)])
    }
    .spacing(1)
    .split(area);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("SESSIONS  ", label(app)),
            Span::styled(app.batch_rows.len().to_string(), accent(app, true)),
            Span::styled("    FINISHED  ", label(app)),
            Span::styled(
                format!("{}/{}", app.batch_finished, app.batch_total),
                success(app),
            ),
        ]),
        Line::from(vec![
            Span::styled("PARALLELISM  ", label(app)),
            Span::styled(app.batch_parallelism.to_string(), warning(app)),
        ]),
        Line::from(""),
    ];
    if app.batch_rows.is_empty() {
        let message = if app
            .status
            .as_deref()
            .is_some_and(|status| status.starts_with("Run All"))
        {
            "No Planned or Suspended sessions were ready."
        } else {
            "Press a to start Run All."
        };
        lines.push(Line::from(Span::styled(message, muted(app))));
    }
    for row in &app.batch_rows {
        let (marker, marker_style, phase_style) = if row.finished {
            ("✓ ", success(app), success(app))
        } else {
            ("● ", warning(app), accent(app, false))
        };
        lines.push(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(row.id.as_str(), label(app)),
            Span::raw(format!("  {}  ", truncate(&row.title, 36))),
            Span::styled(row.phase.as_str(), phase_style),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel(
            app,
            if app.operation_state.batch_cancelled {
                " Run All  cancelled "
            } else {
                " Run All "
            },
            true,
        )),
        split[0],
    );
    let height = split[1].height.saturating_sub(2) as usize;
    let logs = app
        .batch_logs
        .iter()
        .rev()
        .take(height)
        .rev()
        .map(|line| Line::from(line.as_str()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(logs)
            .block(panel(app, " Batch log  latest 2,000 lines ", false))
            .wrap(Wrap { trim: false }),
        split[1],
    );
}

fn render_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, modal: &Modal) {
    match modal {
        Modal::Help => render_help_modal(frame, app, area),
        Modal::Error(error) => render_text_modal(
            frame,
            app,
            area,
            70,
            8,
            "Error  Enter/Esc to close",
            Text::styled(error.as_str(), error_style(app)),
        ),
        Modal::Resize => render_text_modal(
            frame,
            app,
            area,
            60,
            5,
            "Resize",
            "Terminal too small — resize to at least 80×24",
        ),
        Modal::Confirm { message, .. } => render_text_modal(
            frame,
            app,
            area,
            70,
            7,
            "Confirm",
            format!("{message}\n\nEnter confirm   Esc cancel"),
        ),
        Modal::Publish { trigger_cruise } => {
            render_publish_modal(frame, app, area, *trigger_cruise);
        }
        Modal::Palette { actions, selected } => {
            render_palette_modal(frame, app, area, actions, *selected);
        }
        Modal::Prompt => render_prompt_modal(frame, app, area),
        Modal::Input {
            title,
            editor,
            regenerate,
            ..
        } => render_input_modal(frame, app, area, title, editor, *regenerate),
    }
}

fn render_help_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let body = "n  new session, one question at a time     1/2/3  switch views
Ctrl+P  planning     Ctrl+G  grill     Ctrl+U  input plan
Ctrl+S  save draft   (each starts from any question)
Tab / Shift-Tab  next / previous question, or detail tab
Enter  next question; newline in the task and image editors
Ctrl+Enter  next question from a multiline editor
↑↓ / j/k  choose, recall history, or navigate
Space  toggle the current choice     PgUp/PgDn/Home/End  jump
←→ / [ ]  detail tabs
a / Enter  actions   o  selected Ask → Plan; Option modal or URL
c  clean sessions (Sessions only; asks for confirmation)
Ctrl+Enter  save multiline input in action dialogs
Ctrl+R  toggle save/regenerate
Plan Ask: Enter edit/submit   Esc leave editing; Option stays modal
Esc  close, or back one question   ?  help   q/Ctrl-C  quit

Fast path: n → type the task → Ctrl+P/G/U.
Keyboard-only; no mouse or child-owned TTY.";
    render_text_modal(frame, app, area, 72, 20, "Keyboard map", body);
}
fn render_publish_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, trigger_cruise: bool) {
    let text = format!(
        "Publish this plan as an Issue?\n\nFollow-up @cruise run comment: {}\n\nSpace/↑↓ toggle   Enter publish   Esc cancel",
        if trigger_cruise { "yes" } else { "no" }
    );
    render_text_modal(frame, app, area, 72, 8, "Publish", text);
}

fn render_palette_modal(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    area: Rect,
    actions: &[SessionAction],
    selected: usize,
) {
    let items = actions
        .iter()
        .enumerate()
        .map(|(idx, action)| {
            Line::from(Span::styled(
                format!(
                    "{}{}",
                    if idx == selected { "▸ " } else { "  " },
                    action_label(*action)
                ),
                if idx == selected {
                    selection(app)
                } else {
                    Style::default()
                },
            ))
        })
        .collect::<Vec<_>>();
    let height = u16::try_from(actions.len())
        .unwrap_or(u16::MAX)
        .saturating_add(4)
        .min(area.height.saturating_sub(2));
    render_text_modal(frame, app, area, 52, height, "Actions  ↑↓ Enter Esc", items);
}

fn render_prompt_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(prompt) = app.prompts.active.as_ref() else {
        return;
    };
    let mut lines = prompt
        .question
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .map(|line| Line::from(Span::styled(line, accent(app, true))))
        .collect::<Vec<_>>();
    if !prompt.choices.is_empty() {
        lines.push(Line::from(""));
        for (idx, choice) in prompt.choices.iter().enumerate() {
            lines.push(Line::from(Span::styled(
                format!(
                    "{}{}{}",
                    if idx == app.prompts.choice {
                        "▸ "
                    } else {
                        "  "
                    },
                    choice.label,
                    if choice.kind == OptionChoiceKind::TextInput {
                        " (text)"
                    } else {
                        ""
                    }
                ),
                if idx == app.prompts.choice {
                    selection(app)
                } else {
                    Style::default()
                },
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Answer: {}", app.prompts.answer.text())));
    lines.push(Line::from(
        "Enter submit   Esc dismiss (request remains queued)",
    ));
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(4)
        .max(14)
        .min(area.height.saturating_sub(2));
    render_text_modal(frame, app, area, 78, height, "Prompt", lines);
}

fn render_input_modal(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    area: Rect,
    title: &str,
    editor: &Editor,
    regenerate: bool,
) {
    let rect = centered(area, 78, 8);
    frame.render_widget(Clear, rect);
    let block = modal_block(app, title);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let split = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    frame.render_widget(editor.widget(), split[0]);
    frame.render_widget(
        Paragraph::new(if regenerate {
            "Ctrl+Enter save and regenerate   Ctrl+R save only   Esc cancel"
        } else {
            "Ctrl+Enter save   Ctrl+R save and regenerate   Esc cancel"
        }),
        split[1],
    );
}

fn render_text_modal<'a>(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    area: Rect,
    width: u16,
    height: u16,
    title: &str,
    text: impl Into<Text<'a>>,
) {
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(text)
            .block(modal_block(app, title))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

fn modal_block(app: &TuiApp, title: &str) -> Block<'static> {
    panel(app, format!(" {title} "), true)
        .border_type(BorderType::Double)
        .border_style(accent(app, true))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    area.centered(
        Constraint::Length(width.min(area.width.saturating_sub(2))),
        Constraint::Length(height.min(area.height.saturating_sub(2))),
    )
}

fn colored(app: &TuiApp, color: Color) -> Style {
    if app.display.no_color {
        Style::default()
    } else {
        Style::default().fg(color)
    }
}

fn accent(app: &TuiApp, bold: bool) -> Style {
    colored(app, Color::Rgb(94, 234, 212)).add_modifier(if bold {
        Modifier::BOLD
    } else {
        Modifier::empty()
    })
}

fn active_nav(app: &TuiApp) -> Style {
    accent(app, true).add_modifier(Modifier::UNDERLINED)
}

fn selection(app: &TuiApp) -> Style {
    if app.display.no_color {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Rgb(15, 23, 42))
            .bg(Color::Rgb(94, 234, 212))
            .add_modifier(Modifier::BOLD)
    }
}

fn muted(app: &TuiApp) -> Style {
    colored(app, Color::Rgb(148, 163, 184))
}

fn label(app: &TuiApp) -> Style {
    muted(app).add_modifier(Modifier::BOLD)
}

fn border(app: &TuiApp, focused: bool) -> Style {
    if focused {
        accent(app, false)
    } else {
        colored(app, Color::Rgb(71, 85, 105))
    }
}

fn key(app: &TuiApp) -> Style {
    warning(app).add_modifier(Modifier::BOLD)
}

fn success(app: &TuiApp) -> Style {
    colored(app, Color::Rgb(74, 222, 128))
}

fn warning(app: &TuiApp) -> Style {
    colored(app, Color::Rgb(251, 191, 36))
}

fn panel<'a>(app: &TuiApp, title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .title_style(if focused {
            accent(app, true)
        } else {
            label(app)
        })
        .border_style(border(app, focused))
}

fn phase_style(app: &TuiApp, phase: &SessionPhase) -> Style {
    match phase {
        SessionPhase::Completed => success(app),
        SessionPhase::Failed(_) => error_style(app),
        SessionPhase::Running => warning(app),
        SessionPhase::Suspended => colored(app, Color::Rgb(192, 132, 252)),
        _ => muted(app),
    }
}

#[must_use]
pub(crate) fn sidebar_status_style(app: &TuiApp, status: SidebarStatus) -> Style {
    match status {
        SidebarStatus::AwaitingInput | SidebarStatus::AwaitingApproval => {
            colored(app, Color::Rgb(125, 211, 252))
        }
        SidebarStatus::Running | SidebarStatus::Planning => warning(app),
        SidebarStatus::Draft | SidebarStatus::Completed => success(app),
        SidebarStatus::Planned => colored(app, Color::Rgb(96, 165, 250)),
        SidebarStatus::Failed | SidebarStatus::PlanFailed => error_style(app),
        SidebarStatus::Suspended => colored(app, Color::Rgb(192, 132, 252)),
    }
}

fn sidebar_selection(app: &TuiApp) -> Style {
    if app.display.no_color {
        selection(app)
    } else {
        Style::default()
            .bg(Color::Rgb(30, 41, 59))
            .add_modifier(Modifier::BOLD)
    }
}

fn workspace_label(mode: WorkspaceMode) -> &'static str {
    match mode {
        WorkspaceMode::Worktree => "worktree",
        WorkspaceMode::CurrentBranch => "current branch",
    }
}
fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        format!(
            "{}…",
            value
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn truncate_sidebar_title(value: &str, max: usize) -> String {
    let value_width = Line::from(value).width();
    if value_width <= max {
        return value.to_string();
    }

    let ellipsis = "…";
    let ellipsis_width = Line::from(ellipsis).width();
    if max < ellipsis_width {
        return String::new();
    }

    let mut result = String::new();
    let mut width = 0;
    for (start, character) in value.char_indices() {
        let end = start + character.len_utf8();
        let character_width = Line::from(&value[start..end]).width();
        if width + character_width + ellipsis_width > max {
            break;
        }
        result.push(character);
        width += character_width;
    }
    result.push_str(ellipsis);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;

    fn rendered_lines_with(
        width: u16,
        height: u16,
        no_color: bool,
        view: View,
        configure: impl FnOnce(&mut TuiApp),
    ) -> Vec<String> {
        rendered_lines_with_lock(
            width,
            height,
            no_color,
            view,
            Some(crate::test_support::lock_process()),
            configure,
        )
    }

    fn rendered_lines_with_lock(
        width: u16,
        height: u16,
        no_color: bool,
        view: View,
        process_lock: Option<crate::test_support::ProcessLock>,
        configure: impl FnOnce(&mut TuiApp),
    ) -> Vec<String> {
        rendered_buffer_with_lock(width, height, no_color, view, process_lock, configure)
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    fn rendered_buffer_with(
        width: u16,
        height: u16,
        no_color: bool,
        view: View,
        configure: impl FnOnce(&mut TuiApp),
    ) -> ratatui::buffer::Buffer {
        rendered_buffer_with_lock(
            width,
            height,
            no_color,
            view,
            Some(crate::test_support::lock_process()),
            configure,
        )
    }

    fn rendered_buffer_with_lock(
        width: u16,
        height: u16,
        no_color: bool,
        view: View,
        process_lock: Option<crate::test_support::ProcessLock>,
        configure: impl FnOnce(&mut TuiApp),
    ) -> ratatui::buffer::Buffer {
        let temp = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let application = crate::application::CruiseApplication::new(
            crate::session::SessionManager::new(temp.path().to_path_buf()),
        );
        let (event_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (log_tx, _) = tokio::sync::mpsc::channel(4);
        let mut app = TuiApp::new_for_test_with_lock(application, event_tx, log_tx, process_lock);
        app.display.no_color = no_color;
        app.view = view;
        configure(&mut app);
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal =
            ratatui::Terminal::new(backend).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| draw(frame, &mut app))
            .unwrap_or_else(|error| panic!("{error}"));
        terminal.backend().buffer().clone()
    }

    fn rendered_view_with(
        width: u16,
        height: u16,
        no_color: bool,
        view: View,
        configure: impl FnOnce(&mut TuiApp),
    ) -> String {
        rendered_lines_with(width, height, no_color, view, configure).join("\n")
    }

    fn configure_ask_sessions(app: &mut TuiApp, tab: DetailTab, selected: usize) {
        configure_ask_sessions_with_question(app, tab, selected, "Which provider should be used?");
    }

    fn configure_ask_sessions_with_question(
        app: &mut TuiApp,
        tab: DetailTab,
        selected: usize,
        question: &str,
    ) {
        let mut target = SessionState::new(
            "session-a".to_string(),
            std::path::PathBuf::from("."),
            crate::session_config::SessionConfigRef::File {
                path: std::path::PathBuf::from("cruise.yaml"),
            },
            "task session-a".to_string(),
        );
        target.phase = SessionPhase::AwaitingInput;
        let other = SessionState::new(
            "session-b".to_string(),
            std::path::PathBuf::from("."),
            crate::session_config::SessionConfigRef::File {
                path: std::path::PathBuf::from("cruise.yaml"),
            },
            "task session-b".to_string(),
        );
        app.sessions = vec![target, other];
        app.selected = selected;
        app.tab = tab;
        app.plan_cache.insert(
            "session-a".to_string(),
            "# Existing plan\n\n- keep the generated markdown\n".to_string(),
        );
        app.plan_cache
            .insert("session-b".to_string(), "# Other plan\n".to_string());
        app.plan_prompts.enqueue(
            "session-a".to_string(),
            "ask-a".to_string(),
            question.to_string(),
        );
    }

    fn ask_plan_app() -> (tempfile::TempDir, TuiApp) {
        let temp = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let application = crate::application::CruiseApplication::new(
            crate::session::SessionManager::new(temp.path().to_path_buf()),
        );
        let (event_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (log_tx, _) = tokio::sync::mpsc::channel(4);
        let mut app = TuiApp::new_for_test_with_lock(
            application,
            event_tx,
            log_tx,
            Some(crate::test_support::lock_process()),
        );
        configure_ask_sessions(&mut app, DetailTab::Plan, 0);
        (temp, app)
    }

    fn screen_for_app(width: u16, height: u16, app: &mut TuiApp) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal =
            ratatui::Terminal::new(backend).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| draw(frame, app))
            .unwrap_or_else(|error| panic!("{error}"));
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| {
                row.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn style_test_app(no_color: bool) -> TuiApp {
        let temp = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let application = crate::application::CruiseApplication::new(
            crate::session::SessionManager::new(temp.path().to_path_buf()),
        );
        let (event_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (log_tx, _) = tokio::sync::mpsc::channel(4);
        let mut app = TuiApp::new_for_test_with_lock(
            application,
            event_tx,
            log_tx,
            Some(crate::test_support::lock_process()),
        );
        app.display.no_color = no_color;
        app
    }

    fn sidebar_session(id: &str, phase: SessionPhase) -> SessionState {
        let mut state = SessionState::new(
            id.to_string(),
            std::path::PathBuf::from("."),
            crate::session_config::SessionConfigRef::File {
                path: std::path::PathBuf::from("cruise.yaml"),
            },
            format!("{id} title"),
        );
        state.phase = phase;
        state
    }

    fn row_text(buffer: &ratatui::buffer::Buffer, width: u16, row: usize) -> String {
        buffer
            .content
            .chunks(usize::from(width))
            .nth(row)
            .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
            .unwrap_or_default()
    }

    fn row_containing(buffer: &ratatui::buffer::Buffer, width: u16, text: &str) -> usize {
        (0..usize::from(buffer.area.height))
            .rev()
            .find(|row| {
                let rendered = row_text(buffer, width, *row);
                let compact = rendered.replace(' ', "");
                let expected = text.replace(' ', "");
                (rendered.contains(text) || compact.contains(&expected))
                    && (rendered.contains("●") || rendered.contains("◯"))
            })
            .unwrap_or_else(|| panic!("missing {text:?} in rendered buffer"))
    }

    fn foreground_for_symbol(
        buffer: &ratatui::buffer::Buffer,
        width: u16,
        row: usize,
        symbol: &str,
    ) -> Color {
        buffer
            .content
            .chunks(usize::from(width))
            .nth(row)
            .and_then(|cells| cells.iter().find(|cell| cell.symbol() == symbol))
            .map_or_else(
                || panic!("missing symbol {symbol:?} on row {row}"),
                |cell| cell.fg,
            )
    }

    fn cells_for_text<'a>(
        buffer: &'a ratatui::buffer::Buffer,
        width: u16,
        row: usize,
        text: &str,
    ) -> &'a [ratatui::buffer::Cell] {
        let cells = buffer
            .content
            .chunks(usize::from(width))
            .nth(row)
            .unwrap_or_else(|| panic!("missing row {row}"));
        let symbols = text
            .chars()
            .map(|character| character.to_string())
            .collect::<Vec<_>>();
        let start = cells
            .windows(symbols.len())
            .position(|window| {
                window
                    .iter()
                    .zip(&symbols)
                    .all(|(cell, symbol)| cell.symbol() == symbol.as_str())
            })
            .unwrap_or_else(|| panic!("missing {text:?} on row {row}"));
        &cells[start..start + symbols.len()]
    }

    fn assert_sidebar_row_status(
        index: usize,
        status: SidebarStatus,
        phase: SessionPhase,
        plan_available: bool,
        color: Color,
    ) {
        let selected_id = format!("s{index}");
        let unselected_id = format!("u{index}");
        let mut selected = sidebar_session(&selected_id, phase.clone());
        let mut unselected = sidebar_session(&unselected_id, phase);
        if status == SidebarStatus::PlanFailed {
            selected.plan_error = Some("planner failed".to_string());
            unselected.plan_error = Some("planner failed".to_string());
        }
        let buffer = rendered_buffer_with(120, 24, false, View::Sessions, |app| {
            app.sessions = vec![selected, unselected];
            app.selected = 0;
            if plan_available {
                app.sidebar_plan_available.insert(selected_id.clone());
                app.sidebar_plan_available.insert(unselected_id.clone());
            }
        });
        let selected_row = row_containing(&buffer, 120, &format!("{selected_id} title"));
        let unselected_row = row_containing(&buffer, 120, &format!("{unselected_id} title"));
        let symbol = status.symbol();
        assert_eq!(
            foreground_for_symbol(&buffer, 120, selected_row, symbol),
            color
        );
        assert_eq!(
            foreground_for_symbol(&buffer, 120, unselected_row, symbol),
            color
        );
        for row in [selected_row, unselected_row] {
            for cell in cells_for_text(&buffer, 120, row, status.label()) {
                assert_eq!(cell.fg, color, "status {status:?} label cell");
            }
        }
        let selected_title =
            cells_for_text(&buffer, 120, selected_row, &format!("{selected_id} title"));
        assert!(
            selected_title
                .iter()
                .all(|cell| cell.bg == Color::Rgb(30, 41, 59))
        );
        assert!(
            selected_title
                .iter()
                .all(|cell| cell.modifier.contains(Modifier::BOLD))
        );
        let unselected_title = cells_for_text(
            &buffer,
            120,
            unselected_row,
            &format!("{unselected_id} title"),
        );
        assert!(unselected_title.iter().all(|cell| cell.bg == Color::Reset));
        assert!(
            unselected_title
                .iter()
                .all(|cell| !cell.modifier.contains(Modifier::BOLD))
        );
        let expected_symbol = if status == SidebarStatus::Draft {
            "◯"
        } else {
            "●"
        };
        assert_eq!(symbol, expected_symbol);
    }

    #[test]
    fn sidebar_status_styles_match_the_approved_palette_and_no_color() {
        let app = style_test_app(false);
        let cases = [
            (SidebarStatus::AwaitingInput, Color::Rgb(125, 211, 252)),
            (SidebarStatus::AwaitingApproval, Color::Rgb(125, 211, 252)),
            (SidebarStatus::Running, Color::Rgb(251, 191, 36)),
            (SidebarStatus::Planning, Color::Rgb(251, 191, 36)),
            (SidebarStatus::Draft, Color::Rgb(74, 222, 128)),
            (SidebarStatus::Planned, Color::Rgb(96, 165, 250)),
            (SidebarStatus::Completed, Color::Rgb(74, 222, 128)),
            (SidebarStatus::Failed, Color::Rgb(248, 113, 113)),
            (SidebarStatus::PlanFailed, Color::Rgb(248, 113, 113)),
            (SidebarStatus::Suspended, Color::Rgb(192, 132, 252)),
        ];

        for (status, color) in cases {
            assert_eq!(sidebar_status_style(&app, status).fg, Some(color));
        }

        drop(app);
        let no_color_app = style_test_app(true);
        for (status, _) in cases {
            let style = sidebar_status_style(&no_color_app, status);
            assert_eq!(style.fg, None);
            assert_eq!(style.bg, None);
        }
    }

    #[test]
    fn sidebar_rows_render_all_statuses_with_selection_and_palette() {
        let cases = [
            (
                SidebarStatus::AwaitingInput,
                SessionPhase::AwaitingInput,
                false,
                Color::Rgb(125, 211, 252),
            ),
            (
                SidebarStatus::AwaitingApproval,
                SessionPhase::AwaitingApproval,
                true,
                Color::Rgb(125, 211, 252),
            ),
            (
                SidebarStatus::Running,
                SessionPhase::Running,
                false,
                Color::Rgb(251, 191, 36),
            ),
            (
                SidebarStatus::Planning,
                SessionPhase::AwaitingApproval,
                false,
                Color::Rgb(251, 191, 36),
            ),
            (
                SidebarStatus::Draft,
                SessionPhase::Draft,
                false,
                Color::Rgb(74, 222, 128),
            ),
            (
                SidebarStatus::Planned,
                SessionPhase::Planned,
                false,
                Color::Rgb(96, 165, 250),
            ),
            (
                SidebarStatus::Completed,
                SessionPhase::Completed,
                false,
                Color::Rgb(74, 222, 128),
            ),
            (
                SidebarStatus::Failed,
                SessionPhase::Failed("run failed".to_string()),
                false,
                Color::Rgb(248, 113, 113),
            ),
            (
                SidebarStatus::PlanFailed,
                SessionPhase::AwaitingApproval,
                false,
                Color::Rgb(248, 113, 113),
            ),
            (
                SidebarStatus::Suspended,
                SessionPhase::Suspended,
                false,
                Color::Rgb(192, 132, 252),
            ),
        ];

        for (index, (status, phase, plan_available, color)) in cases.into_iter().enumerate() {
            assert_sidebar_row_status(index, status, phase, plan_available, color);
        }
    }

    #[test]
    fn sidebar_uses_the_draft_circle_and_keeps_long_status_labels_visible() {
        let draft_buffer = rendered_buffer_with(120, 24, false, View::Sessions, |app| {
            app.sessions = vec![
                sidebar_session("draft", SessionPhase::Draft),
                sidebar_session("completed", SessionPhase::Completed),
            ];
            app.selected = 1;
        });
        let draft_row = row_containing(&draft_buffer, 120, "draft title");
        assert!(row_text(&draft_buffer, 120, draft_row).contains("◯"));

        for width in [80, 120] {
            let buffer = rendered_buffer_with(width, 24, false, View::Sessions, |app| {
                // sakoku-ignore-next-line
                let title = "日本語の長いタイトルを含むセッション";
                let mut state = sidebar_session(title, SessionPhase::AwaitingApproval);
                state.plan_error = Some("planner failed".to_string());
                app.sessions = vec![state];
                app.selected = 0;
            });
            // sakoku-ignore-next-line
            let needle = "日本語の長い";
            let row = row_containing(&buffer, width, needle);
            let text = row_text(&buffer, width, row);
            assert!(text.contains("Plan Failed"), "width {width}: {text:?}");
            assert!(text.contains("▸ "), "width {width}: {text:?}");
        }
    }

    #[test]
    fn no_color_keeps_sidebar_content_and_selection_without_rgb_styles() {
        let buffer = rendered_buffer_with(120, 24, true, View::Sessions, |app| {
            app.sessions = vec![sidebar_session("completed", SessionPhase::Completed)];
            app.selected = 0;
        });
        let row = row_containing(&buffer, 120, "completed title");
        let cells = buffer
            .content
            .chunks(120)
            .nth(row)
            .unwrap_or_else(|| panic!("missing row {row}"));
        let marker = cells
            .iter()
            .find(|cell| cell.symbol() == "●")
            .unwrap_or_else(|| panic!("missing status symbol"));
        assert_eq!(marker.fg, Color::Reset);
        assert_eq!(marker.bg, Color::Reset);
        assert!(row_text(&buffer, 120, row).contains("▸ "));
        assert!(row_text(&buffer, 120, row).contains("Completed"));
    }

    #[test]
    fn info_tab_keeps_the_persisted_phase_when_sidebar_shows_plan_failed() {
        let view = rendered_view_with(120, 24, false, View::Sessions, |app| {
            let mut state = sidebar_session("approval", SessionPhase::AwaitingApproval);
            state.plan_error = Some("planner failed".to_string());
            app.sessions = vec![state];
            app.selected = 0;
            app.tab = DetailTab::Info;
        });

        assert!(view.contains("Plan Failed"));
        assert!(view.contains("Phase    Awaiting Approval"));
    }

    #[test]
    fn config_step_shows_auto_and_cli_candidates_at_minimum_and_wide_sizes() {
        for width in [80, 120] {
            let view = rendered_view_with(width, 24, true, View::NewSession, |app| {
                let config_dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
                std::fs::write(
                    config_dir.path().join("cruise.yaml"),
                    "command: [echo]\nsteps:\n  local:\n    command: echo local\n",
                )
                .unwrap_or_else(|error| panic!("{error}"));
                app.form.step = Step::Config;
                app.form.config.set_text("");
                app.form
                    .working_dir
                    .set_text(&config_dir.path().to_string_lossy());
                app.refresh();
            });

            assert!(
                view.contains("Auto-detect"),
                "missing Auto candidate at width {width}"
            );
            assert!(
                view.contains("cruise.yaml"),
                "missing CLI candidate at width {width}"
            );
            assert!(
                view.contains("Built-in default"),
                "missing built-in candidate at width {width}"
            );
            assert!(
                view.contains("▸ Auto-detect"),
                "Auto candidate is not visibly selected at width {width}"
            );
            assert!(
                view.contains("↑↓"),
                "missing arrow-key hint at width {width}"
            );
            assert!(
                view.contains("Tab complete"),
                "missing Tab hint at width {width}"
            );
        }
    }

    #[test]
    fn config_step_keeps_an_arbitrary_path_visible_in_the_editor() {
        let view = rendered_view_with(80, 24, true, View::NewSession, |app| {
            app.form.step = Step::Config;
            app.form.config.set_text("custom/path/workflow.yaml");
        });

        assert!(view.contains("custom/path/workflow.yaml"));
    }

    #[test]
    fn config_candidate_list_scrolls_to_the_selected_late_entry() {
        let view = rendered_view_with(80, 24, true, View::NewSession, |app| {
            let config_dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
            let cruise_dir = config_dir.path().join(".cruise");
            std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|error| panic!("{error}"));
            for index in 0..10 {
                std::fs::write(
                    cruise_dir.join(format!("config-{index:02}.yaml")),
                    format!(
                        "command: [echo]\nsteps:\n  step_{index}:\n    command: echo {index}\n"
                    ),
                )
                .unwrap_or_else(|error| panic!("{error}"));
            }
            let selected = cruise_dir.join("config-09.yaml");
            app.form.step = Step::Config;
            app.form
                .working_dir
                .set_text(&config_dir.path().to_string_lossy());
            app.form.config.set_text(&selected.to_string_lossy());
            app.refresh();
        });

        assert!(view.contains("config-09.yaml"));
        assert!(
            view.contains("config-08.yaml"),
            "the visible window should include entries near the selected entry"
        );
        assert!(
            !view.contains("config-00.yaml"),
            "the list should scroll instead of rendering every candidate at once"
        );
    }

    #[test]
    fn duplicate_env_and_local_config_candidates_have_one_selection_marker() {
        let _lock = crate::test_support::lock_process();
        let fake_home = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let config_dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let config_path = config_dir.path().join("cruise.yaml");
        std::fs::write(
            &config_path,
            "command: [echo]\nsteps:\n  local:\n    command: echo local\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let _env_guard = crate::test_support::EnvGuard::set("CRUISE_CONFIG", &config_path);

        let view = rendered_lines_with_lock(80, 24, true, View::NewSession, None, |app| {
            app.form.step = Step::Config;
            app.form
                .working_dir
                .set_text(&config_dir.path().to_string_lossy());
            app.form.config.set_text(&config_path.to_string_lossy());
            app.refresh();
        })
        .join("\n");

        let selected_config_lines = view
            .lines()
            .filter(|line| line.contains("cruise.yaml") && line.contains("▸ "))
            .count();
        assert_eq!(selected_config_lines, 1);
    }

    #[test]
    fn minimum_and_wide_layouts_render_without_small_terminal_notice() {
        let minimum = rendered_view_with(80, 24, false, View::Sessions, |_| {});
        assert!(minimum.contains("CRUISE"));
        assert!(minimum.contains("Sessions"));
        assert!(!minimum.contains("Terminal too small"));

        let wide = rendered_view_with(120, 24, false, View::Sessions, |_| {});
        assert!(wide.contains("CRUISE"));
        assert!(wide.contains("Sessions"));
        assert!(wide.contains("Detail"));
    }

    #[test]
    fn minimum_new_session_layout_shows_one_question_with_the_remaining_ones_listed() {
        let view = rendered_view_with(80, 24, false, View::NewSession, |_| {});
        assert!(view.contains("question 1 of 9"));
        assert!(view.contains("What should cruise do?"));
        for upcoming in [
            "Images",
            "Source",
            "Working directory",
            "Workflow config",
            "Skipped steps",
            "Workspace",
            "Formal specification",
            "Launch",
        ] {
            assert!(
                view.contains(upcoming),
                "missing upcoming question: {upcoming}"
            );
        }
        assert!(
            !view.contains("GitHub repository"),
            "directory sessions do not ask for a repository"
        );
        assert!(view.contains("Tab or Ctrl+Enter next"));
    }

    #[test]
    fn answered_questions_are_summarised_above_the_current_one() {
        let view = rendered_view_with(80, 24, false, View::NewSession, |app| {
            app.form.input.set_text("add dark mode\nwith a toggle");
            app.form.working_dir.set_text("~/apps/demo");
            app.form.config.set_text("");
            app.form.step = Step::Config;
        });
        assert!(view.contains("question 5 of 9"));
        assert!(view.contains("✓ Task"));
        assert!(view.contains("add dark mode …"));
        assert!(view.contains("✓ Working directory"));
        assert!(view.contains("~/apps/demo"));
        assert!(view.contains("Which workflow config?"));
    }

    #[test]
    fn launch_question_lists_every_mode_with_its_shortcut() {
        let view = rendered_view_with(80, 24, false, View::NewSession, |app| {
            app.form.input.set_text("task");
            app.form.step = Step::Launch;
            app.form.launch = Launch::Grill;
        });
        assert!(view.contains("question 9 of 9"));
        assert!(view.contains("How should this session start?"));
        assert!(view.contains("▸ Grill planning (interview first)"));
        for expected in [
            "Start planning",
            "Use the input as the plan (no LLM planning)",
            "Save as draft",
            "Ctrl+P",
            "Ctrl+G",
            "Ctrl+U",
            "Ctrl+S",
        ] {
            assert!(view.contains(expected), "missing launch choice: {expected}");
        }
    }

    #[test]
    fn new_session_footer_shows_dialogue_controls_instead_of_session_actions() {
        let view = rendered_view_with(80, 24, false, View::NewSession, |_| {});
        assert!(view.contains("Enter next"));
        assert!(view.contains("Shift-Tab back"));
        assert!(view.contains("Ctrl+P/G/U start now"));
        assert!(!view.contains("c clean"));
        assert!(!view.contains("F5"));
        assert!(!view.contains("a actions"));
    }

    #[test]
    fn sessions_footer_shows_clean_hint_for_empty_and_nonempty_lists_at_supported_widths() {
        let mut missing = Vec::new();
        for width in [80, 120] {
            for no_color in [false, true] {
                let empty = rendered_view_with(width, 24, no_color, View::Sessions, |_| {});
                if !empty.contains("c clean") {
                    missing.push(format!("empty width={width}, no_color={no_color}"));
                }

                let populated = rendered_view_with(width, 24, no_color, View::Sessions, |app| {
                    app.sessions.push(crate::session::SessionState::new(
                        "session-1".to_string(),
                        std::path::PathBuf::from("."),
                        crate::session_config::SessionConfigRef::BuiltinSnapshot,
                        "task".to_string(),
                    ));
                });
                if !populated.contains("c clean") {
                    missing.push(format!("populated width={width}, no_color={no_color}"));
                }
            }
        }
        assert!(missing.is_empty(), "missing Clean hint in {missing:?}");
    }

    #[test]
    fn sessions_footer_keeps_clean_hint_visible_after_a_long_status() {
        let view = rendered_view_with(80, 24, true, View::Sessions, |app| {
            app.status = Some(
                "status begins with a message that is much longer than the terminal width"
                    .to_string(),
            );
            app.sessions.push(crate::session::SessionState::new(
                "session-1".to_string(),
                std::path::PathBuf::from("."),
                crate::session_config::SessionConfigRef::BuiltinSnapshot,
                "task".to_string(),
            ));
        });

        let missing = ["status begins", "Enter actions", "c clean"]
            .into_iter()
            .filter(|expected| !view.contains(expected))
            .collect::<Vec<_>>();
        assert!(missing.is_empty(), "missing footer content: {missing:?}");
    }

    #[test]
    fn run_all_footer_keeps_its_controls_without_the_sessions_clean_hint() {
        let view = rendered_view_with(80, 24, false, View::RunAll, |_| {});
        assert!(view.contains("Enter run/stop"));
        assert!(view.contains("? help"));
        assert!(view.contains("q quit"));
        assert!(!view.contains("c clean"));
    }

    #[test]
    fn help_modal_keeps_every_control_visible_at_minimum_size() {
        let help = rendered_view_with(80, 24, false, View::Sessions, |app| {
            app.modal = Some(Modal::Help);
        });
        for expected in [
            "switch views",
            "next / previous question, or detail tab",
            "save multiline input",
            "Ctrl+U  input plan",
            "toggle save/regenerate",
            "back one question",
            "Keyboard-only; no mouse or child-owned TTY.",
        ] {
            assert!(help.contains(expected), "missing help text: {expected}");
        }
        let help_lower = help.to_ascii_lowercase();
        for expected in ["ask", "plan", "option"] {
            assert!(
                help_lower.contains(expected),
                "help does not distinguish Ask and Option behavior: {help}"
            );
        }
        assert!(help.lines().any(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("clean")
                && line.contains("sessions only")
                && line.contains("confirmation")
        }));
    }

    #[test]
    fn narrow_run_all_stacks_summary_above_logs() {
        let lines = rendered_lines_with(80, 24, false, View::RunAll, |_| {});
        let summary_row = lines
            .iter()
            .position(|line| line.contains("SESSIONS"))
            .unwrap_or_else(|| panic!("missing session summary: {lines:?}"));
        let log_row = lines
            .iter()
            .position(|line| line.contains("Batch log"))
            .unwrap_or_else(|| panic!("missing batch log: {lines:?}"));
        assert!(
            log_row > summary_row + 4,
            "Run All did not stack: {lines:?}"
        );
    }

    #[test]
    fn plan_ask_panel_is_only_visible_for_the_selected_session_plan() {
        let info = rendered_view_with(80, 24, true, View::Sessions, |app| {
            configure_ask_sessions(app, DetailTab::Info, 0);
        });
        assert!(info.contains("Awaiting Input"), "info screen:\n{info}");
        assert!(!info.contains("Which provider should be used?"));
        assert!(!info.contains("Prompt"));

        let selected_plan = rendered_view_with(120, 24, true, View::Sessions, |app| {
            configure_ask_sessions(app, DetailTab::Plan, 0);
        });
        assert!(selected_plan.contains("Which provider should be used?"));
        assert!(selected_plan.contains("Answer"));
        assert!(!selected_plan.contains("Prompt"));

        let other_plan = rendered_view_with(120, 24, true, View::Sessions, |app| {
            configure_ask_sessions(app, DetailTab::Plan, 1);
        });
        assert!(!other_plan.contains("Which provider should be used?"));
        assert!(!other_plan.contains("Answer"));
        assert!(!other_plan.contains("Prompt"));
    }

    #[test]
    fn persisted_external_ask_renders_read_only_plan_guidance() {
        let view = rendered_view_with(120, 24, true, View::Sessions, |app| {
            let mut target = SessionState::new(
                "session-a".to_string(),
                std::path::PathBuf::from("."),
                crate::session_config::SessionConfigRef::File {
                    path: std::path::PathBuf::from("cruise.yaml"),
                },
                "task session-a".to_string(),
            );
            target.phase = SessionPhase::AwaitingInput;
            target.awaiting_input = true;
            target.pending_ask_question = Some("Question owned by another process".to_string());
            app.sessions = vec![target];
            app.selected = 0;
            app.tab = DetailTab::Plan;
        });

        assert!(view.contains("Question owned by another process"));
        assert!(view.contains("read-only"));
        assert!(view.contains("process that owns it"));
        assert!(!view.contains("Answer  press Enter to edit"));
        assert!(!view.contains("Enter edit"));
    }

    #[test]
    fn plan_ask_panel_handles_multiline_crlf_japanese_and_bottom_controls_at_all_sizes() {
        for (width, no_color) in [(80, false), (80, true), (120, false), (120, true)] {
            let lines = rendered_lines_with(width, 24, no_color, View::Sessions, |app| {
                configure_ask_sessions_with_question(
                    app,
                    DetailTab::Plan,
                    0,
                    // sakoku-ignore-next-line
                    "first line\r\nsecond line\n日本語の質問と長い説明を折り返す",
                );
            });
            let first_row = lines
                .iter()
                .position(|line| line.contains("first line"))
                .unwrap_or_else(|| {
                    panic!("missing first question line at width {width}: {lines:?}")
                });
            let second_row = lines
                .iter()
                .position(|line| line.contains("second line"))
                .unwrap_or_else(|| {
                    panic!("missing second question line at width {width}: {lines:?}")
                });
            assert_eq!(second_row, first_row + 1, "question lines must be adjacent");
            // sakoku-ignore-next-line
            assert!(lines.join("\n").contains("日本語の質問"));
            assert!(lines.join("\n").contains("Answer"));
            assert!(lines.join("\n").contains("Enter"));
            assert!(lines.join("\n").contains("Esc"));
            assert!(!lines.join("\n").contains("Prompt"));
        }
    }

    #[test]
    fn plan_ask_editor_keeps_shortcut_characters_and_draft_when_focus_leaves() {
        let (_temp, mut app) = ask_plan_app();
        assert!(app.modal.is_none());
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(app.modal.is_none());

        for character in "q1n[]jk".chars() {
            assert!(!app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE,)));
        }
        assert!(!app.operation_state.quit_requested);
        let draft_screen = screen_for_app(120, 24, &mut app);
        assert!(
            draft_screen.contains("q1n[]jk"),
            "draft was not rendered: {draft_screen}"
        );

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.modal.is_none());
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(app.tab, DetailTab::Log);
        assert!(!app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)));
        assert_eq!(app.tab, DetailTab::Plan);
        let restored_screen = screen_for_app(120, 24, &mut app);
        assert!(restored_screen.contains("q1n[]jk"));
    }

    #[test]
    fn plan_ask_empty_submission_is_an_inline_error_without_a_modal() {
        let (_temp, mut app) = ask_plan_app();
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

        let screen = screen_for_app(120, 24, &mut app);
        assert!(!matches!(app.modal, Some(Modal::Prompt | Modal::Error(_))));
        assert!(screen.to_ascii_lowercase().contains("empty"));
        assert!(screen.contains("Which provider should be used?"));
    }

    #[test]
    fn active_plan_with_no_valid_ask_is_displayed_as_planning() {
        let screen = rendered_view_with(120, 24, true, View::Sessions, |app| {
            let mut session = SessionState::new(
                "session-a".to_string(),
                std::path::PathBuf::from("."),
                crate::session_config::SessionConfigRef::File {
                    path: std::path::PathBuf::from("cruise.yaml"),
                },
                "task session-a".to_string(),
            );
            session.phase = SessionPhase::AwaitingInput;
            app.sessions.push(session);
            app.apply_event(crate::tui::registry::UiEvent::Control(
                crate::application::ApplicationEvent::PlanStarted {
                    session_id: "session-a".to_string(),
                    operation: crate::application::OperationKind::Generate,
                },
            ));
        });

        assert!(screen.contains("Planning"));
        assert!(!screen.contains("Awaiting Input"));
    }

    #[test]
    fn skipped_step_question_lists_checkboxes_with_the_cursor_on_the_current_choice() {
        let view = rendered_view_with(80, 24, false, View::NewSession, |app| {
            let step = |id: &str| crate::workflow::SkippableStepNode {
                id: id.to_string(),
                expanded_step_ids: vec![id.to_string()],
                children: Vec::new(),
            };
            app.config_defaults = Some(crate::application::NewSessionConfigDefaults {
                steps: vec![step("build"), step("review")],
                after_pr_steps: Vec::new(),
                default_skipped_steps: Vec::new(),
                resolved_config_key: "test".to_string(),
            });
            app.form.skipped.set_text("review");
            app.form.step = Step::SkippedSteps;
            app.skip_cursor = 1;
        });

        assert!(view.contains("Skip any workflow steps?"));
        assert!(view.contains("  [ ] build"));
        assert!(view.contains("▸ [x] review"));
        assert!(view.contains("Space toggle"));
    }

    #[test]
    fn skipped_step_question_falls_back_to_typing_without_a_step_list() {
        let view = rendered_view_with(80, 24, false, View::NewSession, |app| {
            app.config_defaults = None;
            app.form.skipped.set_text("lint, verify");
            app.form.step = Step::SkippedSteps;
        });
        assert!(view.contains("lint, verify"));
        assert!(view.contains("Comma-separated step ids"));
    }

    #[test]
    fn completed_empty_run_all_reports_no_eligible_sessions() {
        let run_all = rendered_view_with(160, 24, false, View::RunAll, |app| {
            app.status = Some("Run All: 0 sessions".to_string());
        });

        assert!(run_all.contains("No Planned or Suspended sessions were ready."));
        assert!(!run_all.contains("Press a to start Run All."));
    }

    #[test]
    fn undersized_layout_reports_resize_notice() {
        assert!(
            rendered_view_with(79, 24, false, View::Sessions, |_| {})
                .contains("Terminal too small")
        );
        assert!(
            rendered_view_with(80, 23, false, View::Sessions, |_| {})
                .contains("Terminal too small")
        );
    }

    #[test]
    fn no_color_markdown_has_no_styles() {
        let parsed = tui_markdown::from_str("# Heading\n\n- item\n\n```rust\ncode\n```");
        let stripped = strip_text_styles(parsed);
        assert!(
            stripped
                .lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style == Style::default())
        );
    }
}
