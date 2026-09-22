//! Deterministic props for every template, used by `templates_render_all_fixtures`.
//!
//! A fixture exercises the branches a template must handle, so a template that
//! reads a prop the view model never provides fails the test instead of a live
//! request.

use serde_json::{Value, json};

use super::actions::SessionActionsVm;
use super::templates::RawHtml;
use super::view::{
    AskPanelVm, AttachmentListVm, AttachmentVm, ChoiceVm, ConfigGroupVm, ConfigOptionVm,
    ConfigSelectVm, DirectorySuggestionsVm, EditorVm, EmptyStateVm, ErrorPageVm, NewSessionVm,
    OptionDialogVm, PhaseBadgeVm, PublishDialogVm, RepoOptionsVm, RunAllResultVm, RunAllRunningVm,
    RunAllVm, SessionDetailVm, SessionHeaderVm, SessionRowVm, SessionSettingsVm, SettingsVm,
    ShellVm, SidebarVm, StepRowVm, StepTreeVm, TabDagVm, TabHrefs, TabInfoVm, TabLogVm, TabPlanVm,
    ToastKind, ToastVm,
};
use crate::application::DirectoryEntry;

fn value(props: &impl serde::Serialize) -> Value {
    serde_json::to_value(props).unwrap_or(Value::Null)
}

fn badge() -> PhaseBadgeVm {
    PhaseBadgeVm {
        label: "Awaiting Approval".to_string(),
        tone: "approvalReady".to_string(),
        dot: true,
        dot_label: "plan ready for approval".to_string(),
    }
}

fn row() -> SessionRowVm {
    SessionRowVm {
        id: "s1".to_string(),
        href: "/sessions/s1".to_string(),
        title: "Add a WebUI".to_string(),
        subtitle: Some("add a webui".to_string()),
        dir_label: "cruise".to_string(),
        time_label: "2/3/26 04:05".to_string(),
        selected: true,
        badge: badge(),
    }
}

fn actions() -> SessionActionsVm {
    SessionActionsVm {
        show_approve: true,
        show_publish_issue: true,
        show_fix: true,
        show_ask: true,
        show_create_worktree: true,
        show_run: true,
        run_label: "Retry".to_string(),
        show_reset: true,
        show_replan: true,
        show_discard: true,
        show_delete: true,
        show_cancel: true,
        show_generate_plan: true,
        status: "idle".to_string(),
    }
}

fn sidebar() -> SidebarVm {
    SidebarVm {
        version: "0.0.0-test",
        selected_id: Some("s1".to_string()),
        sessions: vec![row()],
        empty: false,
        runnable_count: 2,
        run_all_active: false,
        run_all_confirm: "Run 2 pending session(s)?".to_string(),
        rows: RawHtml::new("<a id=\"session-row-s1\"></a>"),
    }
}

fn config_select() -> ConfigSelectVm {
    ConfigSelectVm {
        sentinels: vec![
            ConfigOptionVm {
                value: String::new(),
                label: "Auto (base dir / user workflows / builtin)".to_string(),
                selected: true,
            },
            ConfigOptionVm {
                value: "__builtin__".to_string(),
                label: "Built-in default".to_string(),
                selected: false,
            },
        ],
        groups: vec![ConfigGroupVm {
            label: "Base dir (/tmp/x)".to_string(),
            options: vec![ConfigOptionVm {
                value: "/tmp/x/cruise.yaml".to_string(),
                label: "cruise.yaml".to_string(),
                selected: false,
            }],
        }],
    }
}

fn step_tree() -> StepTreeVm {
    StepTreeVm {
        steps: vec![
            StepRowVm {
                id: "implement".to_string(),
                label: "implement".to_string(),
                indent_style: "padding-left:0rem".to_string(),
                checked: true,
                after_pr: false,
            },
            StepRowVm {
                id: "review/simplify".to_string(),
                label: "review/simplify".to_string(),
                indent_style: "padding-left:1rem".to_string(),
                checked: false,
                after_pr: true,
            },
        ],
        empty: false,
    }
}

fn run_all() -> RunAllVm {
    RunAllVm {
        active: true,
        status: "running".to_string(),
        total: 3,
        finished: 1,
        parallelism: Some(2),
        running: vec![RunAllRunningVm {
            id: "s2".to_string(),
            title: "second task".to_string(),
            phase: "Running".to_string(),
            step: Some("implement".to_string()),
        }],
        running_count: 1,
        running_empty: false,
        results: vec![RunAllResultVm {
            id: "s1".to_string(),
            title: "first task".to_string(),
            phase: "Failed".to_string(),
            error: Some("boom".to_string()),
            failed: true,
        }],
        results_empty: false,
        log: "--- Run All started ---".to_string(),
        run_error: Some("partial failure".to_string()),
        cancelled: false,
        runnable_count: 2,
        run_all_confirm: "Run 2 pending session(s)?".to_string(),
    }
}

/// Template name (relative to `webui/templates`, without extension) and props.
pub(crate) fn all() -> Vec<(&'static str, Value)> {
    let mut fixtures = shell_fixtures();
    fixtures.extend(tab_fixtures());
    fixtures.extend(form_fixtures());
    fixtures.extend(run_all_fixtures());
    fixtures
}

fn shell_fixtures() -> Vec<(&'static str, Value)> {
    vec![
        (
            "layout",
            value(&ShellVm {
                title: "Cruise".to_string(),
                version: "0.0.0-test",
                main: RawHtml::new("<p>main</p>"),
                sidebar: RawHtml::new("<p>sidebar</p>"),
                dialogs: RawHtml::empty(),
            }),
        ),
        ("sidebar", value(&sidebar())),
        ("sidebar-rows", value(&sidebar())),
        ("components/session-row", json!({ "row": value(&row()) })),
        (
            "components/phase-badge",
            json!({ "badge": value(&badge()) }),
        ),
        (
            "components/session-header",
            value(&SessionHeaderVm {
                id: "s1".to_string(),
                title: "Add a WebUI".to_string(),
                input: "add a webui".to_string(),
                badge: badge(),
                current_step: Some("implement".to_string()),
                phase_error: Some("failed to run".to_string()),
                plan_error: Some("plan failed".to_string()),
                pr_url: Some("https://example.com/pr/1".to_string()),
                pr_is_link: true,
                actions: actions(),
                busy: true,
                awaiting_input: true,
                base_url: "/webui/sessions/s1".to_string(),
            }),
        ),
        (
            "session-detail",
            value(&SessionDetailVm {
                id: "s1".to_string(),
                header: RawHtml::new("<header id=\"session-header-s1\"></header>"),
                ask_panel: RawHtml::empty(),
                settings: RawHtml::empty(),
                editor: RawHtml::empty(),
                active_tab: "plan".to_string(),
                tab_panel: RawHtml::new("<p>panel</p>"),
                tab_hrefs: TabHrefs {
                    info: "/webui/sessions/s1/tab/info".to_string(),
                    dag: "/webui/sessions/s1/tab/dag".to_string(),
                    plan: "/webui/sessions/s1/tab/plan".to_string(),
                    log: "/webui/sessions/s1/tab/log".to_string(),
                },
                push_hrefs: TabHrefs {
                    info: "/sessions/s1?tab=info".to_string(),
                    dag: "/sessions/s1?tab=dag".to_string(),
                    plan: "/sessions/s1?tab=plan".to_string(),
                    log: "/sessions/s1?tab=log".to_string(),
                },
            }),
        ),
    ]
}

fn tab_fixtures() -> Vec<(&'static str, Value)> {
    vec![
        (
            "tab-info",
            value(&TabInfoVm {
                id: "s1".to_string(),
                config_source: "builtin".to_string(),
                location_label: "Base dir".to_string(),
                location: "/tmp/x".to_string(),
                worktree_branch: Some("cruise/s1".to_string()),
                created_at: "2/3/26 04:05".to_string(),
                completed_at: Some("2/3/26 05:05".to_string()),
                pr_url: Some("https://example.com/pr/1".to_string()),
                pr_is_link: true,
                phase_error: Some("boom".to_string()),
                plan_error: Some("plan boom".to_string()),
            }),
        ),
        (
            "tab-plan",
            value(&TabPlanVm {
                id: "s1".to_string(),
                html: RawHtml::new("<h1>Plan</h1>"),
                available: true,
            }),
        ),
        (
            "tab-dag",
            value(&TabDagVm {
                id: "s1".to_string(),
                mermaid_source: Some("flowchart TD\n  a --> b".to_string()),
                message: None,
            }),
        ),
        (
            "tab-log",
            value(&TabLogVm {
                id: "s1".to_string(),
                saved_log: "[stdout] hello".to_string(),
                empty: false,
                running: true,
                poll_url: "/webui/sessions/s1/log".to_string(),
            }),
        ),
        (
            "session-settings",
            value(&SessionSettingsVm {
                id: "s1".to_string(),
                base_dir: "/tmp/x".to_string(),
                repo: String::new(),
                config_select: RawHtml::new("<select></select>"),
                steps: RawHtml::new("<ul></ul>"),
                current_step: Some("implement".to_string()),
                step_options: vec![ConfigOptionVm {
                    value: "implement".to_string(),
                    label: "implement".to_string(),
                    selected: true,
                }],
                open: true,
            }),
        ),
        (
            "new-session",
            value(&NewSessionVm {
                source_mode: "directory".to_string(),
                directory_selected: true,
                input: "add a webui".to_string(),
                base_dir: "/tmp/x".to_string(),
                repo: String::new(),
                recent_working_dirs: vec!["/tmp/x".to_string()],
                skip_planning: false,
                grill: false,
                no_interactive_planning: false,
                formal_spec: false,
                workspace_mode: "Worktree".to_string(),
                config_select: RawHtml::new("<select></select>"),
                steps: RawHtml::new("<ul></ul>"),
                attachments: RawHtml::empty(),
                error: Some("something went wrong".to_string()),
            }),
        ),
    ]
}

fn form_fixtures() -> Vec<(&'static str, Value)> {
    vec![
        ("components/config-select", value(&config_select())),
        ("step-tree", value(&step_tree())),
        (
            "directory-suggestions",
            value(&DirectorySuggestionsVm {
                entries: vec![DirectoryEntry {
                    name: "cruise".to_string(),
                    path: "/tmp/x/cruise".to_string(),
                }],
                empty: false,
            }),
        ),
        (
            "attachment-list",
            value(&AttachmentListVm {
                items: vec![AttachmentVm {
                    path: "/tmp/cruise-webui-uploads/a.png".to_string(),
                    name: "a.png".to_string(),
                    preview_url: "/webui/attachments/preview?path=%2Ftmp%2Fa.png".to_string(),
                }],
                empty: false,
            }),
        ),
        (
            "repo-options",
            value(&RepoOptionsVm {
                repos: vec!["owner/repo".to_string()],
            }),
        ),
        (
            "editor",
            value(&EditorVm {
                id: "s1".to_string(),
                kind: "fix".to_string(),
                submit_url: "/webui/sessions/s1/fix".to_string(),
                field: "feedback".to_string(),
                label: "Fix the plan".to_string(),
                placeholder: "Describe what to change...".to_string(),
                submit_label: "Submit fix".to_string(),
                required: true,
            }),
        ),
        (
            "ask-panel",
            value(&AskPanelVm {
                session_id: "s1".to_string(),
                request_id: "r1".to_string(),
                question: "Which database?".to_string(),
            }),
        ),
        (
            "option-dialog",
            value(&OptionDialogVm {
                session_id: "s1".to_string(),
                session_title: "Add a WebUI".to_string(),
                request_id: "r1".to_string(),
                prompt: "Pick a branch".to_string(),
                choices: vec![ChoiceVm {
                    label: "Continue".to_string(),
                    selector: true,
                    next_step: Some("implement".to_string()),
                    vals: "{\"requestId\":\"r1\",\"nextStep\":\"implement\"}".to_string(),
                }],
                has_text_input: true,
                text_label: "Other".to_string(),
                text_next_step: Some("implement".to_string()),
                submit_url: "/webui/sessions/s1/option".to_string(),
            }),
        ),
        (
            "publish-dialog",
            value(&PublishDialogVm {
                id: "s1".to_string(),
                submit_url: "/webui/sessions/s1/publish".to_string(),
            }),
        ),
    ]
}

fn run_all_fixtures() -> Vec<(&'static str, Value)> {
    vec![
        ("run-all", value(&run_all())),
        ("run-all-progress", value(&run_all())),
        ("run-all-results", value(&run_all())),
        (
            "settings-modal",
            value(&SettingsVm {
                run_all_parallelism: 4,
                error: Some("Must be at least 1".to_string()),
            }),
        ),
        (
            "toast",
            value(&ToastVm::new(
                ToastKind::Failed,
                "add a webui",
                Some("boom".to_string()),
            )),
        ),
        (
            "empty-state",
            value(&EmptyStateVm {
                message: "Select a session or create a new one.".to_string(),
            }),
        ),
        (
            "error-page",
            value(&ErrorPageVm {
                message: "Session not found".to_string(),
            }),
        ),
    ]
}
