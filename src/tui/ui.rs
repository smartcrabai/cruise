use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap,
};

use crate::application::{OptionChoiceKind, SessionAction};
use crate::session::{SessionPhase, WorkspaceMode};

use super::app::{DetailTab, Modal, TuiApp, View, action_label};
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
    let status = app.status.as_deref().unwrap_or("Ready");
    let busy = app.is_busy();
    let mut spans = vec![
        Span::styled(
            if busy { " ◉ " } else { " ● " },
            if busy { warning(app) } else { success(app) },
        ),
        Span::raw(status),
    ];
    if !app.prompts.is_empty() {
        spans.push(Span::styled(
            format!("  {} prompt(s)", app.prompts.len()),
            warning(app),
        ));
    }
    if app.dropped_logs > 0 {
        spans.push(Span::styled(
            format!("  {} logs dropped", app.dropped_logs),
            error_style(app),
        ));
    }
    let hints: &[(&str, &str)] = match app.view {
        View::Sessions if app.sessions.is_empty() => &[("n", "new"), ("?", "help"), ("q", "quit")],
        View::Sessions => &[
            ("Enter", "actions"),
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
    for &(shortcut, description) in hints {
        spans.extend([
            Span::raw("   "),
            Span::styled(shortcut, key(app)),
            Span::raw(" "),
            Span::styled(description, muted(app)),
        ]);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(border(app, false)),
        ),
        area,
    );
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
    let items = app.sessions.iter().map(|session| {
        let phase = session.phase.label();
        let title_width = usize::from(area.width)
            .saturating_sub(phase.chars().count())
            .saturating_sub(7);
        ListItem::new(Line::from(vec![
            Span::styled("● ", phase_style(app, &session.phase)),
            Span::raw(truncate(session.title_or_input(), title_width)),
            Span::styled(format!(" · {phase}"), phase_style(app, &session.phase)),
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
            .highlight_style(selection(app))
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
            Span::styled(session.phase.label(), phase_style(app, &session.phase)),
        ),
        labeled_line(app, "Source   ", Span::raw(session.config_source.as_str())),
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
            Paragraph::new("\n  DAG unavailable. Check the session workflow configuration.")
                .style(muted(app))
                .block(panel(app, " DAG ", false)),
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
        for predecessor in dag.nodes.values().filter(|candidate| {
            candidate
                .successors
                .iter()
                .any(|edge| edge.target.as_deref() == Some(node.id.as_str()))
        }) {
            lines.push(Line::from(format!(
                "  {}  ←  {}",
                predecessor.id, predecessor.step_name
            )));
        }
        lines.push(Line::from(Span::styled("TRANSITIONS", label(app))));
        for successor in &node.successors {
            lines.push(Line::from(format!(
                "  {:?}  →  {}",
                successor.reason,
                successor.target.as_deref().unwrap_or("end")
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
    let mut selected = current.is_empty().then_some(0);
    let mut matched = false;
    let mut lines = Vec::with_capacity(app.config_sources.len() + 1);
    lines.push(choice_line(app, "Auto-detect", current.is_empty()));
    for (index, source) in app.config_sources.iter().enumerate() {
        let source_selected = !matched && source.selection_value() == current;
        matched |= source_selected;
        selected = selected.or(source_selected.then_some(index + 1));
        lines.push(choice_line(app, source.label(), source_selected));
    }
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
a / Enter  actions   o  prompt/link   f  follow log   r  refresh
Ctrl+Enter  save multiline input in action dialogs
Ctrl+R  toggle save/regenerate
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_lines_with(
        width: u16,
        height: u16,
        no_color: bool,
        view: View,
        configure: impl FnOnce(&mut TuiApp),
    ) -> Vec<String> {
        let temp = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let application = crate::application::CruiseApplication::new(
            crate::session::SessionManager::new(temp.path().to_path_buf()),
        );
        let (event_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (log_tx, _) = tokio::sync::mpsc::channel(4);
        let mut app = TuiApp::new(application, event_tx, log_tx);
        app.display.no_color = no_color;
        app.view = view;
        configure(&mut app);
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal =
            ratatui::Terminal::new(backend).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| draw(frame, &mut app))
            .unwrap_or_else(|error| panic!("{error}"));
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
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

        let view = rendered_view_with(80, 24, true, View::NewSession, |app| {
            app.form.step = Step::Config;
            app.form
                .working_dir
                .set_text(&config_dir.path().to_string_lossy());
            app.form.config.set_text(&config_path.to_string_lossy());
            app.refresh();
        });

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
        assert!(!view.contains("F5"));
        assert!(!view.contains("a actions"));
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
    fn prompt_modal_preserves_multiline_questions_and_bottom_controls_at_minimum_size() {
        let lines = rendered_lines_with(80, 24, true, View::Sessions, |app| {
            app.prompts.active = Some(crate::tui::prompts::PromptItem {
                request_id: "ask-1".to_string(),
                session_id: "session-1".to_string(),
                kind: crate::application::PendingPromptKind::Ask,
                question: "first line\nsecond line".to_string(),
                choices: Vec::new(),
            });
            app.modal = Some(Modal::Prompt);
        });
        let first_row = lines
            .iter()
            .position(|line| line.contains("first line"))
            .unwrap_or_else(|| panic!("missing first question line: {lines:?}"));
        let second_row = lines
            .iter()
            .position(|line| line.contains("second line"))
            .unwrap_or_else(|| panic!("missing second question line: {lines:?}"));
        assert_eq!(second_row, first_row + 1, "question lines must be adjacent");
        assert!(!lines[first_row].contains("second line"));
        let screen = lines.join("\n");
        assert!(screen.contains("Answer:"));
        assert!(screen.contains("Enter submit   Esc dismiss (request remains queued)"));
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
