use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};

use crate::application::{OptionChoiceKind, SessionAction};
use crate::session::{SessionPhase, WorkspaceMode};

use super::app::{DetailTab, Modal, TuiApp, View, action_label};
use super::forms::{Editor, FormField, NewSessionForm, SourceKind};
pub fn draw(frame: &mut Frame<'_>, app: &mut TuiApp) {
    let area = frame.area();
    if area.width < 80 || area.height < 24 {
        let block = Block::default().borders(Borders::ALL).title("Cruise TUI");
        frame.render_widget(
            Paragraph::new("Terminal too small — resize to at least 80×24").block(block),
            area,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
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
    if app.modal.is_some() {
        render_modal(frame, app, area);
    }
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let title = match app.view {
        View::Sessions => "Cruise  /  Sessions",
        View::NewSession => "Cruise  /  New Session",
        View::RunAll => "Cruise  /  Run All",
    };
    let spinner = if app.is_busy() {
        ["⠋", "⠙", "⠹", "⠸"][app.spinner_frame]
    } else {
        ""
    };
    let text = if spinner.is_empty() {
        title.to_string()
    } else {
        format!("{spinner} {title}")
    };
    frame.render_widget(Paragraph::new(text).style(accent(app, true)), area);
}

fn render_footer(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let status = app.status.as_deref().unwrap_or("Ready");
    let prompt_badge = if app.prompts.is_empty() {
        String::new()
    } else {
        format!("  prompts:{}", app.prompts.len())
    };
    let dropped = if app.dropped_logs == 0 {
        String::new()
    } else {
        format!("  dropped logs:{}", app.dropped_logs)
    };
    let text = format!(
        "{status}{prompt_badge}{dropped}    1 Sessions  2 New  3 Run All  a Actions  ? Help  q Quit"
    );
    frame.render_widget(Paragraph::new(text).style(accent(app, false)), area);
}

fn render_sessions(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    app.load_tab_data();
    let sidebar_width = if area.width >= 120 {
        34
    } else {
        area.width.saturating_sub(2).min(40)
    };
    let sections = Layout::default()
        .direction(if area.width >= 120 {
            Direction::Horizontal
        } else {
            Direction::Vertical
        })
        .constraints(if area.width >= 120 {
            vec![Constraint::Length(sidebar_width), Constraint::Min(1)]
        } else {
            vec![Constraint::Length(8), Constraint::Min(1)]
        })
        .split(area);
    render_sidebar(frame, app, sections[0]);
    render_detail(frame, app, sections[1]);
}

fn render_sidebar(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let items = app
        .sessions
        .iter()
        .map(|session| {
            let title = truncate(
                session.title_or_input(),
                area.width.saturating_sub(5) as usize,
            );
            let phase = session.phase.label();
            ListItem::new(Line::from(vec![
                Span::raw(title),
                Span::raw(" "),
                Span::styled(format!("[{phase}]"), phase_style(app, &session.phase)),
            ]))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select((!app.sessions.is_empty()).then_some(app.selected));
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Sessions ({})", items.len()));
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(accent(app, true))
            .highlight_symbol("▶ "),
        area,
        &mut state,
    );
}

fn render_detail(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(session) = app.active_session() else {
        frame.render_widget(
            Paragraph::new("No sessions yet. Press 2 to create one.")
                .block(Block::default().borders(Borders::ALL).title("Detail")),
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
        .map(|tab| Line::from(tab.label()))
        .collect::<Vec<_>>(),
    )
    .select(match app.tab {
        DetailTab::Info => 0,
        DetailTab::Dag => 1,
        DetailTab::Plan => 2,
        DetailTab::Log => 3,
    })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(session.title_or_input()),
    )
    .highlight_style(accent(app, true));
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
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
        Line::from(vec![
            Span::styled("ID       ", accent(app, true)),
            Span::raw(session.id.clone()),
        ]),
        Line::from(vec![
            Span::styled("Phase    ", accent(app, true)),
            Span::styled(session.phase.label(), phase_style(app, &session.phase)),
        ]),
        Line::from(vec![
            Span::styled("Source   ", accent(app, true)),
            Span::raw(session.config_source.clone()),
        ]),
        Line::from(vec![
            Span::styled("Directory", accent(app, true)),
            Span::raw(format!(" {}", session.base_dir.display())),
        ]),
        Line::from(vec![
            Span::styled("Workspace", accent(app, true)),
            Span::raw(workspace_label(session.workspace_mode)),
        ]),
        Line::from(vec![
            Span::styled("Step     ", accent(app, true)),
            Span::raw(
                session
                    .current_step
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            ),
        ]),
        Line::from(vec![
            Span::styled("PR       ", accent(app, true)),
            Span::raw(session.pr_url.clone().unwrap_or_else(|| "—".to_string())),
        ]),
        Line::from(vec![
            Span::styled("Issue    ", accent(app, true)),
            Span::raw(
                session
                    .published_issue_url
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Input    ", accent(app, true)),
            Span::raw(session.input.clone()),
        ]),
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
    lines.push(Line::from(vec![
        Span::styled("Actions  ", accent(app, true)),
        Span::raw(actions),
    ]));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Session information"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn error_style(app: &TuiApp) -> Style {
    if app.display.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    }
}

fn render_dag(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(dag) = app.active_dag() else {
        frame.render_widget(
            Paragraph::new("DAG is not available until the workflow is prepared.")
                .block(Block::default().borders(Borders::ALL).title("DAG")),
            area,
        );
        return;
    };
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(1)])
        .split(area);
    let items = dag
        .nodes
        .values()
        .map(|node| {
            ListItem::new(Line::from(vec![
                Span::styled(node.id.clone(), accent(app, true)),
                Span::raw(format!(" {}", node.step_name)),
            ]))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state
        .select((!items.is_empty()).then_some(app.dag_selected.min(items.len().saturating_sub(1))));
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Nodes ({})", dag.nodes.len())),
            )
            .highlight_style(accent(app, true))
            .highlight_symbol("▶ "),
        split[0],
        &mut state,
    );
    let mut lines = Vec::new();
    if let Some(node) = dag
        .nodes
        .values()
        .nth(app.dag_selected.min(dag.nodes.len().saturating_sub(1)))
    {
        lines.push(Line::from(vec![
            Span::styled("Node ", accent(app, true)),
            Span::raw(format!("{}  {}", node.id, node.step_name)),
        ]));
        lines.push(Line::from("Incoming dependencies:"));
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
        lines.push(Line::from("Transitions:"));
        for successor in &node.successors {
            lines.push(Line::from(format!(
                "  {:?}  →  {}",
                successor.reason,
                successor.target.as_deref().unwrap_or("end")
            )));
        }
        if let Some(visited) = node.runtime.visited_at.as_deref() {
            lines.push(Line::from(format!("Last visited: {visited}")));
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Selected node"),
            )
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
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Plan (Markdown)"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn strip_text_styles(text: Text<'_>) -> Text<'_> {
    Text::from(
        text.lines
            .into_iter()
            .map(|line| {
                Line::from(
                    line.spans
                        .into_iter()
                        .map(|span| Span::raw(span.content))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
    )
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
        Paragraph::new(Text::from(
            visible.into_iter().map(Line::from).collect::<Vec<_>>(),
        ))
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_new_session(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(area);
    render_session_source(frame, app, rows[1]);
    render_session_editors(frame, app, &rows);
    render_session_options(frame, app, rows[6]);
    render_session_skips(frame, app, rows[7]);
    render_session_actions(frame, app, rows[8]);
}

fn render_session_source(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    let source = format!(
        "{} Source: {}  (Space toggles)",
        if form.field == FormField::Source {
            "▶"
        } else {
            " "
        },
        match form.source {
            SourceKind::Directory => "Directory",
            SourceKind::GitHub => "GitHub",
        }
    );
    frame.render_widget(
        Paragraph::new(source).block(Block::default().borders(Borders::ALL).title("Source")),
        area,
    );
}

fn render_session_editors(frame: &mut Frame<'_>, app: &TuiApp, rows: &[Rect]) {
    let form = &app.form;
    render_editor(
        frame,
        form,
        FormField::Input,
        rows[0],
        "Task description (Enter inserts a new line)",
        &form.input.text(),
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
        form,
        FormField::WorkingDirectory,
        rows[2],
        &recent_title,
        &form.working_dir.text(),
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
        form,
        FormField::Repository,
        rows[3],
        &repository_title,
        &form.repository.text(),
    );
    render_editor(
        frame,
        form,
        FormField::Config,
        rows[4],
        "Workflow config (Space cycles discovered entries)",
        &form.config.text(),
    );
    render_editor(
        frame,
        form,
        FormField::Attachments,
        rows[5],
        "Image paths (one per line)",
        &form.attachments.text(),
    );
}

fn render_session_options(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    let options = vec![
        (
            FormField::Workspace,
            format!("Workspace: {}", workspace_label(form.workspace_mode)),
        ),
        (
            FormField::DirtyTree,
            format!(
                "Allow dirty current branch: {}",
                yes_no(form.options.allow_dirty_working_tree)
            ),
        ),
        (
            FormField::Grill,
            format!("Grill planning: {}", yes_no(form.options.planning.grill)),
        ),
        (
            FormField::SkipPlanning,
            format!(
                "Use input as plan: {}",
                yes_no(form.options.planning.skip_planning)
            ),
        ),
        (
            FormField::Noninteractive,
            format!(
                "No interactive planning: {}",
                yes_no(form.options.planning.noninteractive)
            ),
        ),
    ];
    let option_lines = options
        .into_iter()
        .map(|(field, line)| {
            Line::from(Span::styled(
                format!("{}{}", if form.field == field { "▶ " } else { "  " }, line),
                if form.field == field {
                    accent(app, true)
                } else {
                    Style::default()
                },
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(option_lines)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Options — Space toggles, Tab moves"),
        ),
        area,
    );
}

fn render_session_skips(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let form = &app.form;
    let available_steps = app
        .config_defaults
        .as_ref()
        .map(|defaults| {
            defaults
                .steps
                .iter()
                .flat_map(|node| {
                    std::iter::once(node.id.as_str())
                        .chain(node.children.iter().map(|child| child.id.as_str()))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let skip_title = if available_steps.is_empty() {
        "Skipped steps (comma-separated; explicit blank means none)".to_string()
    } else {
        format!("Skipped steps (choices: {available_steps})")
    };
    render_editor(
        frame,
        form,
        FormField::SkippedSteps,
        area,
        &skip_title,
        &form.skipped.text(),
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
    let actions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    let draft = format!(
        "{} [ Save as draft ]",
        if form.field == FormField::SaveDraft {
            "▶"
        } else {
            " "
        }
    );
    frame.render_widget(
        Paragraph::new(draft).block(Block::default().borders(Borders::ALL)),
        actions[0],
    );
    let submit = format!(
        "{} [ Create session and start ]  configs: {}",
        if form.field == FormField::Submit {
            "▶"
        } else {
            " "
        },
        if config_names.is_empty() {
            "none"
        } else {
            &config_names
        }
    );
    frame.render_widget(
        Paragraph::new(submit).block(Block::default().borders(Borders::ALL)),
        actions[1],
    );
}

fn render_editor(
    frame: &mut Frame<'_>,
    form: &NewSessionForm,
    field: FormField,
    area: Rect,
    title: &str,
    value: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(if form.field == field {
            format!("▶ {title}")
        } else {
            title.to_string()
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match field {
        FormField::Input => frame.render_widget(form.input.widget(), inner),
        FormField::WorkingDirectory => frame.render_widget(form.working_dir.widget(), inner),
        FormField::Repository => frame.render_widget(form.repository.widget(), inner),
        FormField::Config => frame.render_widget(form.config.widget(), inner),
        FormField::Attachments => frame.render_widget(form.attachments.widget(), inner),
        FormField::SkippedSteps => frame.render_widget(form.skipped.widget(), inner),
        _ => frame.render_widget(Paragraph::new(value), inner),
    }
}

fn render_run_all(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);
    let mut lines = vec![
        Line::from(format!(
            "Sessions: {}   Finished: {}/{}",
            app.batch_rows.len(),
            app.batch_finished,
            app.batch_total
        )),
        Line::from(format!("Parallelism: {}", app.batch_parallelism)),
        Line::from(""),
    ];
    if app.batch_rows.is_empty() {
        lines.push(Line::from("No Planned or Suspended sessions are ready."));
    }
    for row in &app.batch_rows {
        lines.push(Line::from(format!(
            "{}  {}  [{}]{}",
            row.id,
            truncate(&row.title, 36),
            row.phase,
            if row.finished { " done" } else { "" }
        )));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::default().borders(Borders::ALL).title(
            if app.operation_state.batch_cancelled {
                "Run All (cancelled)"
            } else {
                "Run All"
            },
        )),
        split[0],
    );
    let height = split[1].height.saturating_sub(2) as usize;
    let logs = app
        .batch_logs
        .iter()
        .rev()
        .take(height)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(Line::from)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(logs))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Latest 2,000 batch log lines"),
            )
            .wrap(Wrap { trim: false }),
        split[1],
    );
}

fn render_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(modal) = app.modal.as_ref() else {
        return;
    };
    frame.render_widget(Clear, area);
    match modal {
        Modal::Help => render_help_modal(frame, app, area),
        Modal::Error(error) => frame.render_widget(
            Paragraph::new(error.as_str())
                .block(modal_block(app, "Error — Enter/Esc to close"))
                .wrap(Wrap { trim: false }),
            centered(area, 70, 8),
        ),
        Modal::Resize => frame.render_widget(
            Paragraph::new("Terminal too small — resize to at least 80×24")
                .block(modal_block(app, "Resize"))
                .wrap(Wrap { trim: false }),
            centered(area, 60, 5),
        ),
        Modal::Confirm { message, .. } => frame.render_widget(
            Paragraph::new(format!("{message}\n\nEnter confirm   Esc cancel"))
                .block(modal_block(app, "Confirm"))
                .wrap(Wrap { trim: false }),
            centered(area, 70, 7),
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
    let body = "Keyboard-only controls\n\n1/2/3  Sessions / New Session / Run All\nTab/Shift-Tab  focus or detail tabs\n↑↓ or j/k  navigate (and move text cursors while editing)   PgUp/PgDn  page\n[/]  detail tabs   a  action palette\no  open next prompt or PR/Issue URL   f  follow log   r  refresh\nEnter  edit/commit/submit single-line fields   Enter  newline in multiline editors\nCtrl+Enter  submit multiline settings   Ctrl+R  toggle Save+Regenerate\nEsc  close edit/modal   ?  help   q/Ctrl-C  quit\n\nNo mouse, clipboard, daemon, or child-owned TTY is used.";
    frame.render_widget(
        Paragraph::new(body)
            .block(modal_block(app, "Help"))
            .wrap(Wrap { trim: false }),
        centered(area, 64, 18),
    );
}
fn render_publish_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, trigger_cruise: bool) {
    let text = format!(
        "Publish this plan as an Issue?\n\nFollow-up @cruise run comment: {}\n\nSpace/↑↓ toggle   Enter publish   Esc cancel",
        if trigger_cruise { "yes" } else { "no" }
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(modal_block(app, "Publish"))
            .wrap(Wrap { trim: false }),
        centered(area, 72, 8),
    );
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
                    if idx == selected { "▶ " } else { "  " },
                    action_label(*action)
                ),
                if idx == selected {
                    accent(app, true)
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
    frame.render_widget(
        Paragraph::new(Text::from(items))
            .block(modal_block(app, "Actions — ↑↓ Enter Esc"))
            .wrap(Wrap { trim: false }),
        centered(area, 52, height),
    );
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
                        "▶ "
                    } else {
                        "  "
                    },
                    choice.label,
                    if matches!(&choice.kind, &OptionChoiceKind::TextInput) {
                        " (text)"
                    } else {
                        ""
                    }
                ),
                if idx == app.prompts.choice {
                    accent(app, true)
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
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(modal_block(app, "Prompt"))
            .wrap(Wrap { trim: false }),
        centered(area, 78, 14),
    );
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
    let block = modal_block(app, title);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
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

fn modal_block(app: &TuiApp, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(accent(app, true))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn accent(app: &TuiApp, bold: bool) -> Style {
    if app.display.no_color {
        Style::default().add_modifier(if bold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        })
    } else {
        Style::default().fg(Color::Cyan).add_modifier(if bold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        })
    }
}

fn phase_style(app: &TuiApp, phase: &SessionPhase) -> Style {
    if app.display.no_color {
        return Style::default();
    }
    let color = match phase {
        SessionPhase::Completed => Color::Green,
        SessionPhase::Failed(_) => Color::Red,
        SessionPhase::Running => Color::Yellow,
        SessionPhase::Suspended => Color::Magenta,
        _ => Color::White,
    };
    Style::default().fg(color)
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

    fn rendered(width: u16, height: u16, no_color: bool) -> String {
        let temp = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let application = crate::application::CruiseApplication::new(
            crate::session::SessionManager::new(temp.path().to_path_buf()),
        );
        let (event_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (log_tx, _) = tokio::sync::mpsc::channel(4);
        let mut app = TuiApp::new(application, event_tx, log_tx);
        app.display.no_color = no_color;
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

    #[test]
    fn minimum_and_wide_layouts_render_without_small_terminal_notice() {
        let minimum = rendered(80, 24, false);
        assert!(minimum.contains("Cruise"));
        assert!(minimum.contains("Sessions"));
        assert!(!minimum.contains("Terminal too small"));

        let wide = rendered(120, 24, false);
        assert!(wide.contains("Cruise"));
        assert!(wide.contains("Sessions"));
        assert!(wide.contains("Detail"));
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
