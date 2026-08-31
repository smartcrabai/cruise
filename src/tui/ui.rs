use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap,
};

use crate::application::{OptionChoiceKind, SessionAction};
use crate::session::{SessionPhase, WorkspaceMode};

use super::app::{DetailTab, Modal, TuiApp, View, action_label};
use super::forms::{Editor, FormField, NewSessionForm, SourceKind};
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
        (View::NewSession, " 2 New Session "),
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
    spans.extend([
        Span::styled("    a", key(app)),
        Span::styled(" actions", muted(app)),
        Span::styled("   ?", key(app)),
        Span::styled(" help", muted(app)),
        Span::styled("   q", key(app)),
        Span::styled(" quit", muted(app)),
    ]);
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
            .saturating_sub(7) as usize;
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
            Paragraph::new("\n  No sessions yet\n\n  Press 2 to create one.")
                .style(muted(app))
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
    let rendered = if app.display.no_color {
        strip_text_styles(parsed)
    } else {
        parsed
    };
    frame.render_widget(
        Paragraph::new(rendered)
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
    let rows = Layout::vertical([
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .split(area);
    render_session_editors(frame, app, &rows);
    render_session_source(frame, app, rows[1]);
    render_session_options(frame, app, rows[6]);
    render_session_skips(frame, app, rows[7]);
    render_session_actions(frame, app, rows[8]);
}

fn render_session_source(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            field_marker(app, form.field == FormField::Source),
            Span::styled("SOURCE  ", label(app)),
            Span::styled(
                " DIRECTORY ",
                if form.source == SourceKind::Directory {
                    active_nav(app)
                } else {
                    muted(app)
                },
            ),
            Span::styled(
                " GITHUB ",
                if form.source == SourceKind::GitHub {
                    active_nav(app)
                } else {
                    muted(app)
                },
            ),
            Span::styled("   Space to switch", muted(app)),
        ])),
        area,
    );
}

fn render_session_editors(frame: &mut Frame<'_>, app: &TuiApp, rows: &[Rect]) {
    let form = &app.form;
    render_editor(
        frame,
        app,
        form,
        FormField::Input,
        rows[0],
        "Task description (Enter inserts a new line)",
        &form.input,
    );
    let recent_title = app
        .history_summary
        .as_ref()
        .filter(|summary| !summary.recent_working_dirs.is_empty())
        .map_or_else(
            || "Working directory".to_string(),
            |summary| {
                format!(
                    "Working directory (Space cycles recent: {})",
                    summary
                        .recent_working_dirs
                        .iter()
                        .take(3)
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        );
    render_editor(
        frame,
        app,
        form,
        FormField::WorkingDirectory,
        rows[2],
        &recent_title,
        &form.working_dir,
    );
    let repository_title =
        if form.source == SourceKind::GitHub && !app.github_repositories.is_empty() {
            format!(
                "GitHub repository (Space/source; {} found)",
                app.github_repositories.len()
            )
        } else {
            "GitHub repository owner/name".to_string()
        };
    render_editor(
        frame,
        app,
        form,
        FormField::Repository,
        rows[3],
        &repository_title,
        &form.repository,
    );
    render_editor(
        frame,
        app,
        form,
        FormField::Config,
        rows[4],
        "Workflow config (Space cycles discovered entries)",
        &form.config,
    );
    render_editor(
        frame,
        app,
        form,
        FormField::Attachments,
        rows[5],
        "Image paths (one per line)",
        &form.attachments,
    );
}

fn render_session_options(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    let option = |field, name, value, enabled| option_line(app, form, field, name, value, enabled);
    let columns =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    frame.render_widget(
        Paragraph::new(vec![
            option(
                FormField::Workspace,
                "Workspace",
                workspace_label(form.workspace_mode),
                None,
            ),
            option(
                FormField::DirtyTree,
                "Allow dirty current branch",
                yes_no(form.options.allow_dirty_working_tree),
                Some(form.options.allow_dirty_working_tree),
            ),
            option(
                FormField::Grill,
                "Grill planning",
                yes_no(form.options.planning.grill),
                Some(form.options.planning.grill),
            ),
        ]),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            option(
                FormField::SkipPlanning,
                "Use input as plan",
                yes_no(form.options.planning.skip_planning),
                Some(form.options.planning.skip_planning),
            ),
            option(
                FormField::Noninteractive,
                "No interactive planning",
                yes_no(form.options.planning.noninteractive),
                Some(form.options.planning.noninteractive),
            ),
            Line::from(Span::styled("  Tab moves  ·  Space toggles", muted(app))),
        ]),
        columns[1],
    );
}

fn render_session_skips(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    fn collect_labels<'a>(
        nodes: &'a [crate::workflow::SkippableStepNode],
        labels: &mut Vec<&'a str>,
    ) {
        for node in nodes {
            labels.push(&node.id);
            collect_labels(&node.children, labels);
        }
    }

    let form = &app.form;
    let mut labels = Vec::new();
    if let Some(defaults) = app.config_defaults.as_ref() {
        collect_labels(&defaults.steps, &mut labels);
        collect_labels(&defaults.after_pr_steps, &mut labels);
    }
    let title = if labels.is_empty() {
        "Skipped steps  ·  Enter edit".to_string()
    } else {
        let selected = app.skip_cursor % labels.len();
        format!(
            "Skipped steps  ·  choice: {}  ·  ↑↓ choose  ·  Space toggle  ·  Enter edit",
            labels[selected]
        )
    };
    render_editor(
        frame,
        app,
        form,
        FormField::SkippedSteps,
        area,
        &title,
        &form.skipped,
    );
}

fn render_session_actions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    let config_names = app
        .config_sources
        .iter()
        .take(3)
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    let actions =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(rows[0]);
    for (area, title, field) in [
        (actions[0], "Save draft", FormField::SaveDraft),
        (actions[1], "Create session and start", FormField::Submit),
    ] {
        frame.render_widget(
            Paragraph::new(title)
                .alignment(Alignment::Center)
                .style(if form.field == field {
                    selection(app)
                } else {
                    active_nav(app)
                }),
            area,
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  CONFIG SOURCES  ", label(app)),
            Span::styled(
                if config_names.is_empty() {
                    "none"
                } else {
                    &config_names
                },
                muted(app),
            ),
        ])),
        rows[1],
    );
}

fn render_editor(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    form: &NewSessionForm,
    field: FormField,
    area: Rect,
    title: &str,
    editor: &Editor,
) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let focused = form.field == field;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            field_marker(app, focused),
            Span::styled(
                title,
                if focused {
                    accent(app, true)
                } else {
                    muted(app)
                },
            ),
        ])),
        rows[0],
    );
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(border(app, focused));
    let inner = block.inner(rows[1]);
    frame.render_widget(block, rows[1]);
    frame.render_widget(editor.widget(), inner);
}

fn render_run_all(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let split = Layout::horizontal([Constraint::Percentage(48), Constraint::Min(1)])
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
            "Use Actions to start Run All."
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
        Modal::Error(error) => {
            let rect = centered(area, 70, 8);
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(error.as_str())
                    .style(error_style(app))
                    .block(modal_block(app, "Error  Enter/Esc to close"))
                    .wrap(Wrap { trim: false }),
                rect,
            );
        }
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
            render_publish_modal(frame, app, area, *trigger_cruise)
        }
        Modal::Palette { actions, selected } => {
            render_palette_modal(frame, app, area, actions, *selected)
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
    let body = "Keyboard-only controls\n\n1/2/3  Sessions / New Session / Run All\nTab/Shift-Tab  focus or detail tabs\n↑↓ or j/k  navigate (and move text cursors while editing)   PgUp/PgDn  page\n[/]  detail tabs   a  action palette\no  open next prompt or PR/Issue URL   f  follow log   r  refresh\nEnter  edit/commit/submit single-line fields   Enter  newline in multiline editors\nSpace (not editing)  toggle options/source, cycle choices, or toggle a skipped step\nCtrl+Enter  submit multiline settings   Ctrl+R  toggle Save+Regenerate\nEsc  close edit/modal   ?  help   q/Ctrl-C  quit\n\nNo mouse, clipboard, daemon, or child-owned TTY is used.";
    render_text_modal(frame, app, area, 68, 18, "Keyboard map", body);
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
    let mut lines = vec![Line::from(Span::styled(
        prompt.question.as_str(),
        accent(app, true),
    ))];
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
    render_text_modal(frame, app, area, 78, 14, "Prompt", lines);
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
    Block::bordered()
        .border_type(BorderType::Double)
        .title(format!(" {title} "))
        .title_style(accent(app, true))
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

fn field_marker(app: &TuiApp, focused: bool) -> Span<'static> {
    Span::styled(
        if focused { "▸ " } else { "  " },
        if focused {
            accent(app, true)
        } else {
            muted(app)
        },
    )
}

fn option_line<'a>(
    app: &TuiApp,
    form: &NewSessionForm,
    field: FormField,
    name: &'a str,
    value: &'a str,
    enabled: Option<bool>,
) -> Line<'a> {
    let focused = form.field == field;
    let value_style = match enabled {
        Some(true) => success(app),
        Some(false) => muted(app),
        None => accent(app, false),
    };
    Line::from(vec![
        field_marker(app, focused),
        Span::styled(
            name,
            if focused {
                accent(app, true)
            } else {
                Style::default()
            },
        ),
        Span::raw("  "),
        Span::styled(value, value_style),
    ])
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
fn yes_no(value: bool) -> &'static str {
    if value { "on" } else { "off" }
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

    fn rendered_view_with(
        width: u16,
        height: u16,
        no_color: bool,
        view: View,
        configure: impl FnOnce(&mut TuiApp),
    ) -> String {
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
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    fn rendered(width: u16, height: u16, no_color: bool) -> String {
        rendered_view_with(width, height, no_color, View::Sessions, |_| {})
    }

    #[test]
    fn minimum_and_wide_layouts_render_without_small_terminal_notice() {
        let minimum = rendered(80, 24, false);
        assert!(minimum.contains("CRUISE"));
        assert!(minimum.contains("Sessions"));
        assert!(!minimum.contains("Terminal too small"));

        let wide = rendered(120, 24, false);
        assert!(wide.contains("CRUISE"));
        assert!(wide.contains("Sessions"));
        assert!(wide.contains("Detail"));
    }

    #[test]
    fn minimum_new_session_layout_keeps_every_control_visible() {
        let form = rendered_view_with(80, 24, false, View::NewSession, |_| {});
        for expected in [
            "Task description",
            "SOURCE",
            "Working directory",
            "GitHub repository",
            "Workflow config",
            "Image paths",
            "Workspace",
            "Allow dirty current branch",
            "Grill planning",
            "Use input as plan",
            "No interactive planning",
            "Skipped steps",
            "Save draft",
            "Create session and start",
        ] {
            assert!(form.contains(expected), "missing control: {expected}");
        }
    }

    #[test]
    fn skipped_step_navigation_shows_the_current_choice() {
        let form = rendered_view_with(80, 24, false, View::NewSession, |app| {
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
            app.form.field = FormField::SkippedSteps;
            app.skip_cursor = 1;
        });

        assert!(form.contains("choice: review"));
    }

    #[test]
    fn completed_empty_run_all_reports_no_eligible_sessions() {
        let run_all = rendered_view_with(160, 24, false, View::RunAll, |app| {
            app.status = Some("Run All: 0 sessions".to_string());
        });

        assert!(run_all.contains("No Planned or Suspended sessions were ready."));
        assert!(!run_all.contains("Use Actions to start Run All."));
    }

    #[test]
    fn undersized_layout_reports_resize_notice() {
        assert!(rendered(79, 24, false).contains("Terminal too small"));
        assert!(rendered(80, 23, false).contains("Terminal too small"));
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

    #[test]
    fn centered_rect_stays_inside_parent() {
        let parent = Rect::new(0, 0, 100, 30);
        let rect = centered(parent, 80, 10);
        assert!(rect.x + rect.width <= parent.width);
        assert!(rect.y + rect.height <= parent.height);
    }
}
