//! View models handed to the TSX templates.
//!
//! Templates cannot call functions, test list lengths, or use optional
//! chaining, so every derived value — labels, hrefs, Tailwind tone selectors,
//! emptiness flags — is computed here.

use serde::Serialize;

use super::actions::SessionActionsVm;
use super::dto::SessionDto;
use super::templates::RawHtml;
use crate::application::{DirectoryEntry, OperationKind};

/// Document shell rendered by `layout.tsx`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellVm {
    pub(crate) title: String,
    pub(crate) version: &'static str,
    pub(crate) main: RawHtml,
    pub(crate) sidebar: RawHtml,
    pub(crate) dialogs: RawHtml,
}

/// Phase badge, mirroring `ui/src/components/PhaseBadge.tsx`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhaseBadgeVm {
    pub(crate) label: String,
    /// Selects the Tailwind colour classes inside `components/phase-badge.tsx`.
    pub(crate) tone: String,
    /// Render the blue "needs attention" dot.
    pub(crate) dot: bool,
    pub(crate) dot_label: String,
}

pub(crate) const PLANNING_LABEL: &str = "Planning";
pub(crate) const FIXING_LABEL: &str = "Fixing";

impl PhaseBadgeVm {
    pub(crate) fn new(session: &SessionDto, fixing: bool) -> Self {
        let phase = session.phase.as_str();
        let is_awaiting = phase == "Awaiting Approval";
        let is_awaiting_input = phase == "Awaiting Input";
        let has_pending_input = session.pending_ask_question.is_some();
        let is_draft_planning = phase == "Draft" && fixing;
        let is_awaiting_input_planning = is_awaiting_input && fixing && !has_pending_input;
        let show_approve_ready = is_awaiting && session.plan_available && !fixing;
        let show_input_required = is_awaiting_input && has_pending_input;
        let label = if is_draft_planning || is_awaiting_input_planning {
            PLANNING_LABEL.to_string()
        } else if is_awaiting && fixing {
            FIXING_LABEL.to_string()
        } else {
            phase.to_string()
        };
        let tone = match phase {
            "Awaiting Approval" => {
                if show_approve_ready {
                    "approvalReady"
                } else {
                    "approval"
                }
            }
            "Awaiting Input" => "input",
            "Planned" => "planned",
            "Running" => "running",
            "Completed" => "completed",
            "Failed" | "PR Failed" => "failed",
            "Suspended" => "suspended",
            _ => "draft",
        };
        let dot_label = if show_approve_ready {
            "plan ready for approval"
        } else {
            "user input required"
        };
        Self {
            label,
            tone: tone.to_string(),
            dot: show_approve_ready || show_input_required,
            dot_label: dot_label.to_string(),
        }
    }
}

/// One sidebar row.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionRowVm {
    pub(crate) id: String,
    pub(crate) href: String,
    /// `title` when present, otherwise the raw input.
    pub(crate) title: String,
    /// The raw input, shown as a second line only when `title` is set.
    pub(crate) subtitle: Option<String>,
    pub(crate) dir_label: String,
    pub(crate) time_label: String,
    pub(crate) selected: bool,
    pub(crate) badge: PhaseBadgeVm,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSyncVm {
    pub(crate) id: String,
    pub(crate) url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunAllControlVm {
    pub(crate) run_all_active: bool,
    pub(crate) runnable_count: usize,
    pub(crate) run_all_confirm: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidebarVm {
    pub(crate) version: &'static str,
    pub(crate) selected_id: Option<String>,
    pub(crate) sessions: Vec<SessionRowVm>,
    pub(crate) empty: bool,
    pub(crate) runnable_count: usize,
    pub(crate) run_all_active: bool,
    pub(crate) run_all_confirm: String,
    /// Pre-rendered `sidebar-rows` fragment; empty when rendering the rows
    /// template itself.
    pub(crate) rows: RawHtml,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionHeaderVm {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) input: Option<String>,
    pub(crate) badge: PhaseBadgeVm,
    pub(crate) current_step: Option<String>,
    pub(crate) phase_error: Option<String>,
    pub(crate) auto_requeue_count: u32,
    pub(crate) plan_error: Option<String>,
    pub(crate) pr_url: Option<String>,
    pub(crate) pr_is_link: bool,
    pub(crate) actions: SessionActionsVm,
    pub(crate) busy: bool,
    pub(crate) awaiting_input: bool,
    pub(crate) base_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabHrefs {
    pub(crate) info: String,
    pub(crate) dag: String,
    pub(crate) plan: String,
    pub(crate) log: String,
    pub(crate) diff: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionDetailVm {
    pub(crate) id: String,
    pub(crate) header: RawHtml,
    pub(crate) sync: RawHtml,
    pub(crate) ask_panel: RawHtml,
    pub(crate) settings: RawHtml,
    pub(crate) editor: RawHtml,
    pub(crate) active_tab: String,
    pub(crate) tab_panel: RawHtml,
    pub(crate) tab_hrefs: TabHrefs,
    pub(crate) push_hrefs: TabHrefs,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabInfoVm {
    pub(crate) auto_requeue_count: u32,
    pub(crate) id: String,
    pub(crate) pending_instructions: usize,
    pub(crate) config_source: String,
    /// Declared workflow flags with this session's values; absent values are off.
    pub(crate) flags: Vec<FlagRowVm>,
    /// Displayed when there are no declared flags or the config is unavailable.
    pub(crate) flags_note: Option<String>,
    /// "Repository" when the session targets a remote repo, else "Base dir".
    pub(crate) location_label: String,
    pub(crate) location: String,
    pub(crate) worktree_branch: Option<String>,
    pub(crate) created_at: String,
    pub(crate) completed_at: Option<String>,
    pub(crate) pr_url: Option<String>,
    pub(crate) pr_is_link: bool,
    pub(crate) phase_error: Option<String>,
    pub(crate) plan_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabPlanVm {
    pub(crate) id: String,
    pub(crate) html: RawHtml,
    pub(crate) available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabDiffVm {
    pub(crate) id: String,
    pub(crate) lines: Vec<DiffLineVm>,
    pub(crate) message: Option<String>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiffLineVm {
    pub(crate) text: String,
    pub(crate) class: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabDagVm {
    pub(crate) id: String,
    pub(crate) mermaid_source: Option<String>,
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabLogVm {
    pub(crate) id: String,
    pub(crate) saved_log: String,
    pub(crate) empty: bool,
    pub(crate) running: bool,
    pub(crate) poll_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigOptionVm {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) selected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigGroupVm {
    pub(crate) label: String,
    pub(crate) options: Vec<ConfigOptionVm>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigSelectVm {
    /// The two ungrouped sentinel options: Auto and Built-in tdd.
    pub(crate) sentinels: Vec<ConfigOptionVm>,
    pub(crate) groups: Vec<ConfigGroupVm>,
}

/// Sentinel `<option>` value meaning "let cruise resolve the config".
pub(crate) const AUTO_CONFIG_VALUE: &str = "";

impl ConfigSelectVm {
    fn sentinels(selected: &str) -> Vec<ConfigOptionVm> {
        vec![
            ConfigOptionVm {
                value: AUTO_CONFIG_VALUE.to_string(),
                label: "Auto (base dir / user workflows / builtin)".to_string(),
                selected: selected == AUTO_CONFIG_VALUE,
            },
            ConfigOptionVm {
                value: super::dto::BUILTIN_CONFIG_PATH.to_string(),
                label: format!(
                    "Built-in tdd \u{2014} {}",
                    crate::builtin_workflows::find_builtin(
                        crate::builtin_workflows::DEFAULT_BUILTIN_NAME
                    )
                    .map_or("", |w| w.description)
                ),
                selected: selected == super::dto::BUILTIN_CONFIG_PATH,
            },
        ]
    }

    fn append(
        nodes: &[crate::workflow_categories::PickerNode],
        candidates: &[crate::workflow_categories::PickerCandidate],
        path: &str,
        base_dir: &str,
        selected: &str,
        already_selected: &mut bool,
        groups: &mut Vec<ConfigGroupVm>,
    ) {
        use crate::workflow_categories::PickerNode;
        let mut options = Vec::new();
        for node in nodes {
            if let PickerNode::Workflow {
                candidate,
                description,
            } = node
            {
                let item = &candidates[*candidate];
                let chosen = !*already_selected && selected == item.value;
                *already_selected |= chosen;
                let desc = description.as_ref().or(item.description.as_ref());
                options.push(ConfigOptionVm {
                    value: item.value.clone(),
                    label: desc.map_or_else(
                        || item.label.clone(),
                        |d| format!("{} \u{2014} {d}", item.label),
                    ),
                    selected: chosen,
                });
            }
        }
        if !options.is_empty() {
            let label = if path.is_empty() {
                format!(
                    "Base dir ({})",
                    if base_dir.is_empty() { "." } else { base_dir }
                )
            } else {
                path.to_string()
            };
            groups.push(ConfigGroupVm { label, options });
        }
        for node in nodes {
            if let PickerNode::Category { name, nodes } = node {
                let child = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path} › {name}")
                };
                Self::append(
                    nodes,
                    candidates,
                    &child,
                    base_dir,
                    selected,
                    already_selected,
                    groups,
                );
            }
        }
    }

    pub(crate) fn new(
        nodes: &[crate::workflow_categories::PickerNode],
        candidates: &[crate::workflow_categories::PickerCandidate],
        selected: Option<&str>,
        base_dir: &str,
    ) -> Self {
        let selected = selected.unwrap_or(AUTO_CONFIG_VALUE);
        let sentinels = Self::sentinels(selected);
        let mut groups = Vec::new();
        let mut already_selected = sentinels.iter().any(|option| option.selected);
        Self::append(
            nodes,
            candidates,
            "",
            base_dir,
            selected,
            &mut already_selected,
            &mut groups,
        );
        if !already_selected && !selected.is_empty() {
            let label = candidates
                .iter()
                .find(|item| item.value == selected)
                .map_or_else(|| selected.to_string(), |item| item.label.clone());
            groups.push(ConfigGroupVm {
                label: "Current workflow".to_string(),
                options: vec![ConfigOptionVm {
                    value: selected.to_string(),
                    label,
                    selected: true,
                }],
            });
        }
        Self { sentinels, groups }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StepRowVm {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) indent_style: String,
    pub(crate) checked: bool,
    pub(crate) after_pr: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StepTreeVm {
    pub(crate) steps: Vec<StepRowVm>,
    pub(crate) empty: bool,
    pub(crate) flags: Vec<FlagRowVm>,
}

/// One declared workflow flag as an on/off selector, defaulting to off.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FlagRowVm {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) on: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent piece of view state"
)]
pub(crate) struct NewSessionVm {
    pub(crate) source_mode: String,
    pub(crate) directory_selected: bool,
    pub(crate) input: String,
    pub(crate) base_dir: String,
    pub(crate) repo: String,
    pub(crate) recent_working_dirs: Vec<String>,
    pub(crate) skip_planning: bool,
    pub(crate) grill: bool,
    pub(crate) no_interactive_planning: bool,
    pub(crate) workspace_mode: String,
    pub(crate) config_select: RawHtml,
    pub(crate) steps: RawHtml,
    pub(crate) attachments: RawHtml,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSettingsVm {
    pub(crate) id: String,
    pub(crate) base_dir: String,
    pub(crate) repo: String,
    pub(crate) config_select: RawHtml,
    pub(crate) steps: RawHtml,
    pub(crate) current_step: Option<String>,
    pub(crate) step_options: Vec<ConfigOptionVm>,
    pub(crate) open: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirectorySuggestionsVm {
    pub(crate) entries: Vec<DirectoryEntry>,
    pub(crate) empty: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentVm {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) preview_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentListVm {
    pub(crate) items: Vec<AttachmentVm>,
    pub(crate) empty: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoOptionsVm {
    pub(crate) repos: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditorVm {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) submit_url: String,
    pub(crate) field: String,
    pub(crate) label: String,
    pub(crate) placeholder: String,
    pub(crate) submit_label: String,
    pub(crate) required: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AskPanelVm {
    pub(crate) session_id: String,
    pub(crate) request_id: String,
    pub(crate) question: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChoiceVm {
    pub(crate) label: String,
    pub(crate) selector: bool,
    pub(crate) next_step: Option<String>,
    pub(crate) vals: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OptionDialogVm {
    pub(crate) session_id: String,
    pub(crate) session_title: String,
    pub(crate) request_id: String,
    pub(crate) prompt: String,
    pub(crate) choices: Vec<ChoiceVm>,
    pub(crate) has_text_input: bool,
    pub(crate) text_label: String,
    pub(crate) text_next_step: Option<String>,
    pub(crate) submit_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishDialogVm {
    pub(crate) id: String,
    pub(crate) submit_url: String,
    pub(crate) supports_trigger: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MergeCheckVm {
    pub(crate) name: String,
    pub(crate) status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MergePrDialogVm {
    pub(crate) id: String,
    pub(crate) submit_url: String,
    pub(crate) state: String,
    pub(crate) mergeable: String,
    pub(crate) review_decision: String,
    pub(crate) checks: Vec<MergeCheckVm>,
    pub(crate) no_checks: bool,
    pub(crate) can_merge: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    InputRequired,
    Completed,
    Failed,
    PlanReady,
}

impl ToastKind {
    pub(crate) fn tone(self) -> &'static str {
        match self {
            Self::InputRequired => "inputRequired",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::PlanReady => "planReady",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::InputRequired => "Action required",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::PlanReady => "Plan ready",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToastVm {
    pub(crate) tone: String,
    pub(crate) label: String,
    pub(crate) session_input: String,
    pub(crate) detail: Option<String>,
}

impl ToastVm {
    pub(crate) fn new(
        kind: ToastKind,
        session_input: impl Into<String>,
        detail: Option<String>,
    ) -> Self {
        Self {
            tone: kind.tone().to_string(),
            label: kind.label().to_string(),
            session_input: session_input.into(),
            detail,
        }
    }

    pub(crate) fn failed(context: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(ToastKind::Failed, context, Some(detail.into()))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsVm {
    pub(crate) run_all_parallelism: usize,
    pub(crate) effective_run_all_parallelism: usize,
    pub(crate) run_all_auto_requeue_max_attempts: u32,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunAllRunningVm {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) phase: String,
    pub(crate) step: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunAllResultVm {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) phase: String,
    pub(crate) error: Option<String>,
    pub(crate) failed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent piece of view state"
)]
pub(crate) struct RunAllVm {
    pub(crate) active: bool,
    pub(crate) status: String,
    pub(crate) total: usize,
    pub(crate) finished: usize,
    pub(crate) parallelism: Option<usize>,
    pub(crate) running: Vec<RunAllRunningVm>,
    pub(crate) running_count: usize,
    pub(crate) running_empty: bool,
    pub(crate) results: Vec<RunAllResultVm>,
    pub(crate) results_empty: bool,
    pub(crate) log: String,
    pub(crate) run_error: Option<String>,
    pub(crate) cancelled: bool,
    pub(crate) runnable_count: usize,
    pub(crate) run_all_confirm: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorPageVm {
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmptyStateVm {
    pub(crate) message: String,
}

/// Port of `ui/src/lib/format.ts` `formatLocalTime`, rendered in the server's
/// local timezone because the browser cannot reformat server-rendered text.
pub(crate) fn format_local_time(iso: &str) -> String {
    let Some(parsed) = parse_iso8601(iso) else {
        return "--".to_string();
    };
    let (year, month, day, hour, minute) = parsed;
    format!("{month}/{day}/{:02} {hour:02}:{minute:02}", year % 100)
}

type CivilDateTime = (i64, u32, u32, u32, u32);

/// Parse the `YYYY-MM-DDTHH:MM:SS` prefix cruise writes for timestamps.
fn parse_iso8601(iso: &str) -> Option<CivilDateTime> {
    let bytes = iso.as_bytes();
    if bytes.len() < 16 {
        return None;
    }
    let year: i64 = iso.get(0..4)?.parse().ok()?;
    let month: u32 = iso.get(5..7)?.parse().ok()?;
    let day: u32 = iso.get(8..10)?.parse().ok()?;
    let hour: u32 = iso.get(11..13)?.parse().ok()?;
    let minute: u32 = iso.get(14..16)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    Some((year, month, day, hour, minute))
}

/// Last path segment of a base directory, as the sidebar shows it.
pub(crate) fn dir_label(base_dir: &str) -> String {
    base_dir
        .replace('\\', "/")
        .split('/')
        .rfind(|part| !part.is_empty())
        .map_or_else(|| base_dir.to_string(), ToString::to_string)
}

/// Truncate a session title for the sidebar and toasts.
pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// Sidebar ordering: sessions needing attention first, then most recent.
pub(crate) fn sort_sessions(sessions: &mut [SessionDto]) {
    sessions.sort_by(|a, b| {
        let a_input = a.awaiting_input || a.phase == "Awaiting Approval";
        let b_input = b.awaiting_input || b.phase == "Awaiting Approval";
        b_input.cmp(&a_input).then_with(|| {
            let a_time = a.updated_at.as_ref().unwrap_or(&a.created_at);
            let b_time = b.updated_at.as_ref().unwrap_or(&b.created_at);
            b_time.cmp(a_time)
        })
    });
}

/// Sessions the Run All button would start.
pub(crate) fn runnable_count(sessions: &[SessionDto]) -> usize {
    sessions
        .iter()
        .filter(|session| {
            !session.exec && (session.phase == "Planned" || session.phase == "Suspended")
        })
        .count()
}

/// Whether the session is claimed by a plan-style operation.
pub(crate) fn is_planning(operation: Option<OperationKind>) -> bool {
    matches!(
        operation,
        Some(
            OperationKind::Fix
                | OperationKind::Ask
                | OperationKind::Replan
                | OperationKind::Generate
        )
    )
}

pub(crate) fn session_row(
    session: &SessionDto,
    selected_id: Option<&str>,
    fixing: bool,
) -> SessionRowVm {
    let title = session
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&session.input);
    SessionRowVm {
        id: session.id.clone(),
        href: format!("/sessions/{}", session.id),
        title: truncate(title, 80),
        subtitle: session
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .filter(|_| !session.input.trim().is_empty())
            .map(|_| truncate(&session.input, 80)),
        dir_label: dir_label(&session.base_dir),
        time_label: format_local_time(session.updated_at.as_ref().unwrap_or(&session.created_at)),
        selected: selected_id == Some(session.id.as_str()),
        badge: PhaseBadgeVm::new(session, fixing),
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigSelectVm, dir_label, format_local_time, truncate};
    use crate::webui::dto::{ConfigEntryDto, ConfigEntrySource};

    #[test]
    fn config_select_groups_by_category_path_and_selects_first_duplicate() {
        use crate::workflow_categories::PickerNode;
        let candidates = crate::webui::dto::picker_candidates(&[], "");
        let leaf = || PickerNode::Workflow {
            candidate: 0,
            description: Some("Override".into()),
        };
        let nodes = vec![PickerNode::Category {
            name: "Development".into(),
            nodes: vec![
                leaf(),
                PickerNode::Category {
                    name: "Backend".into(),
                    nodes: vec![leaf()],
                },
            ],
        }];
        let view = ConfigSelectVm::new(&nodes, &candidates, Some("__builtin__"), "");
        assert_eq!(
            view.groups
                .iter()
                .map(|g| g.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Development", "Development › Backend"]
        );
        assert!(view.groups[0].options[0].label.contains("Override"));
        assert_eq!(
            view.sentinels
                .iter()
                .chain(view.groups.iter().flat_map(|g| &g.options))
                .filter(|o| o.selected)
                .count(),
            1
        );
        assert!(view.sentinels[1].selected);
    }

    #[test]
    fn package_group_and_duplicate_option_select_only_first_occurrence() {
        use crate::workflow_categories::PickerNode;
        let entry = ConfigEntryDto {
            path: "/work/package.yaml".into(),
            name: "package".into(),
            description: None,
            source: Some(ConfigEntrySource::User),
        };
        let candidates = crate::webui::dto::picker_candidates(&[entry], "");
        let package = candidates
            .iter()
            .position(|candidate| candidate.value == "/work/package.yaml")
            .unwrap_or_else(|| panic!("missing package candidate"));
        let leaf = || PickerNode::Workflow {
            candidate: package,
            description: None,
        };
        let nodes = vec![
            PickerNode::Category {
                name: "Packages".into(),
                nodes: vec![leaf()],
            },
            PickerNode::Category {
                name: "Again".into(),
                nodes: vec![leaf()],
            },
        ];
        let view = ConfigSelectVm::new(&nodes, &candidates, Some("/work/package.yaml"), "");
        assert_eq!(view.groups[0].label, "Packages");
        assert!(view.groups[0].options[0].selected);
        assert!(!view.groups[1].options[0].selected);
    }

    #[test]
    fn builtin_catalog_entries_are_selectable_in_config_menu() {
        let entries: Vec<ConfigEntryDto> = crate::builtin_workflows::BUILTIN_WORKFLOWS
            .iter()
            .filter(|workflow| workflow.name != crate::builtin_workflows::DEFAULT_BUILTIN_NAME)
            .map(|workflow| ConfigEntryDto {
                path: format!("builtin:{}", workflow.name),
                name: workflow.name.to_string(),
                description: Some(workflow.description.to_string()),
                source: Some(ConfigEntrySource::Builtin),
            })
            .collect();
        let candidates = crate::webui::dto::picker_candidates(&entries, "");
        let nodes = crate::workflow_categories::build_picker_tree(
            &candidates,
            &indexmap::IndexMap::default(),
            true,
            "Others",
        );
        let view = ConfigSelectVm::new(&nodes, &candidates, Some("builtin:research"), "");
        let values: Vec<&str> = view
            .groups
            .iter()
            .flat_map(|group| &group.options)
            .map(|option| option.value.as_str())
            .collect();
        for entry in &entries {
            assert!(values.contains(&entry.path.as_str()));
        }

        for name in ["research", "security"] {
            let value = format!("builtin:{name}");
            let selected_view = ConfigSelectVm::new(&nodes, &candidates, Some(&value), "");
            let option = selected_view
                .groups
                .iter()
                .flat_map(|group| &group.options)
                .find(|option| option.selected)
                .unwrap_or_else(|| panic!("{name} option not selected"));
            assert_eq!(option.value, value);
            assert!(option.label.contains(name));
        }
    }

    #[test]
    fn format_local_time_rejects_unparseable_input() {
        assert_eq!(format_local_time("not-a-date"), "--");
        assert_eq!(format_local_time("2026-02-03T04:05:06Z"), "2/3/26 04:05");
    }

    #[test]
    fn dir_label_uses_the_last_path_segment() {
        assert_eq!(dir_label("/home/me/projects/cruise/"), "cruise");
        assert_eq!(dir_label("C:\\work\\cruise"), "cruise");
        assert_eq!(dir_label("/"), "/");
    }

    #[test]
    fn truncate_adds_an_ellipsis_only_when_needed() {
        assert_eq!(truncate("  short  ", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd\u{2026}");
    }
}
