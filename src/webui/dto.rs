use std::path::{Path, PathBuf};

use crate::application::{CruiseApplication, OperationKind};
use crate::config::SkipCondition;
use crate::error::{CruiseError, Result};
use crate::session::{SessionPhase, SessionState, WorkspaceMode};
use serde::Serialize;

pub(crate) const BUILTIN_CONFIG_PATH: &str = crate::new_session_history::BUILTIN_CONFIG_KEY;

pub(crate) fn expanded_path(path: &Path) -> PathBuf {
    PathBuf::from(crate::new_session_history::expand_tilde(
        &path.to_string_lossy(),
    ))
}

pub(crate) fn normalize_config_path(path: Option<String>) -> Option<String> {
    path.map(|path| crate::new_session_history::expand_tilde(&path))
}

/// Serializable session representation used by the `WebUI`. Paths and phase
/// errors are flattened here so the client never needs to understand Rust-only
/// domain types.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent piece of view state"
)]
pub struct SessionDto {
    pub id: String,
    pub phase: String,
    pub phase_error: Option<String>,
    pub config_source: String,
    pub config_path: Option<String>,
    pub base_dir: String,
    pub repo: Option<String>,
    pub input: String,
    pub title: Option<String>,
    pub current_step: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub worktree_branch: Option<String>,
    pub workspace_mode: WorkspaceMode,
    pub pr_url: Option<String>,
    pub updated_at: Option<String>,
    pub awaiting_input: bool,
    pub pending_ask_question: Option<String>,
    pub plan_error: Option<String>,
    pub exec: bool,
    pub plan_available: bool,
    pub fix_in_progress: bool,
    pub skipped_steps: Vec<String>,
}

pub(crate) fn session_plan_available(state: &SessionState) -> bool {
    let Ok(data_dir) = crate::paths::data_dir() else {
        return false;
    };
    let sessions_dir = data_dir.join("sessions");
    crate::metadata::plan_markdown_available(&state.plan_path(&sessions_dir))
}

pub(crate) fn session_dto(
    application: &CruiseApplication,
    state: SessionState,
    resolve_current_step: bool,
) -> SessionDto {
    let plan_available = session_plan_available(&state);
    let (phase, phase_error) = match &state.phase {
        SessionPhase::Failed(message) => ("Failed".to_string(), Some(message.clone())),
        phase => (phase.label().to_string(), None),
    };
    let config_path = state.config.selection_value();
    let config_source = state.config.display_label();
    let current_step = if resolve_current_step && state.current_step_is_node_id {
        state.current_step.as_deref().and_then(|node| {
            application
                .session_dag(&state.id)
                .ok()
                .flatten()
                .and_then(|dag| dag.step_name_for_node(node).map(str::to_owned))
        })
    } else {
        state.current_step.clone()
    };
    let fix_in_progress = matches!(
        application.runtime().active_operation(&state.id),
        Some(OperationKind::Fix)
    );
    SessionDto {
        id: state.id,
        phase,
        phase_error,
        config_source,
        config_path,
        base_dir: state.base_dir.to_string_lossy().into_owned(),
        repo: state.repo,
        input: state.input,
        title: state.title,
        current_step,
        created_at: state.created_at,
        completed_at: state.completed_at,
        worktree_branch: state.worktree_branch,
        workspace_mode: state.workspace_mode,
        pr_url: state.pr_url,
        updated_at: state.updated_at,
        awaiting_input: state.awaiting_input,
        pending_ask_question: state.pending_ask_question,
        plan_error: state.plan_error,
        exec: state.exec,
        plan_available,
        fix_in_progress,
        skipped_steps: state.skipped_steps,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DagDto {
    pub start_step: String,
    pub steps: Vec<DagStepDto>,
    pub edges: Vec<DagEdgeDto>,
    pub current_step: Option<String>,
}

pub(crate) fn step_kind(config: &crate::config::StepConfig) -> String {
    if config.parallel.is_some() {
        "parallel".to_string()
    } else if config.prompt.is_some() {
        "prompt".to_string()
    } else if config.option.is_some() {
        "option".to_string()
    } else if config.command.is_some() {
        "command".to_string()
    } else {
        "unknown".to_string()
    }
}

pub(crate) fn transition_reason(
    reason: &crate::graph::TransitionReason,
) -> (String, Option<String>) {
    use crate::graph::TransitionReason;
    match reason {
        TransitionReason::Sequential => ("sequential".to_string(), None),
        TransitionReason::Next => ("next".to_string(), None),
        TransitionReason::IfFileChanged { target } => {
            ("ifFileChanged".to_string(), Some(target.clone()))
        }
        TransitionReason::IfNoFileChangesRetry => ("ifNoFileChangesRetry".to_string(), None),
        TransitionReason::IfNoFileChangesFail => ("ifNoFileChangesFail".to_string(), None),
        TransitionReason::IfFailGoto { target } => ("ifFail".to_string(), Some(target.clone())),
        TransitionReason::IfFailRetry => ("ifFailRetry".to_string(), None),
        TransitionReason::OptionChoice { selector } => {
            ("optionChoice".to_string(), Some(selector.clone()))
        }
        TransitionReason::GroupRetry { target } => ("groupRetry".to_string(), Some(target.clone())),
        TransitionReason::GroupRetryExhausted => ("groupRetryExhausted".to_string(), None),
        TransitionReason::SkipFallback => ("skipFallback".to_string(), None),
    }
}

pub(crate) fn build_dag_dto(
    compiled: &crate::workflow::CompiledWorkflow,
    dag: &crate::graph::ExecutionGraph,
    current_step: Option<&str>,
    current_step_is_node_id: bool,
    skipped_steps: &[String],
) -> std::result::Result<DagDto, String> {
    let start_step = dag
        .step_name_for_node(&dag.start)
        .ok_or_else(|| {
            format!(
                "start node '{}' does not map to any workflow step",
                dag.start
            )
        })?
        .to_string();
    let current_step = dag
        .state
        .current
        .as_deref()
        .or(current_step.filter(|_| !dag.state.completed))
        .and_then(|step| {
            if current_step_is_node_id {
                dag.step_name_for_node(step).map(str::to_owned)
            } else {
                Some(step.to_string())
            }
        });
    let mut terminals = std::collections::HashMap::new();
    for node in dag.nodes.values() {
        let is_terminal = node.successors.iter().any(|edge| {
            edge.target.is_none()
                && match edge.reason {
                    crate::graph::TransitionReason::IfNoFileChangesFail => false,
                    crate::graph::TransitionReason::SkipFallback => {
                        compiled.steps.get(&node.step_name).is_some_and(|step| {
                            step.when.is_some()
                                || matches!(
                                    step.skip,
                                    Some(SkipCondition::Static(true) | SkipCondition::Variable(_))
                                )
                                || skipped_steps.contains(&node.step_name)
                        })
                    }
                    _ => true,
                }
        });
        terminals
            .entry(node.step_name.as_str())
            .and_modify(|terminal| *terminal |= is_terminal)
            .or_insert(is_terminal);
    }
    let mut steps = Vec::new();
    for (name, config) in &compiled.steps {
        let Some(&is_terminal) = terminals.get(name.as_str()) else {
            continue;
        };
        steps.push(DagStepDto {
            name: name.clone(),
            kind: step_kind(config),
            is_terminal,
        });
    }
    let mut edges = Vec::new();
    for node in dag.nodes.values() {
        for successor in &node.successors {
            let to = successor
                .target
                .as_deref()
                .and_then(|id| dag.step_name_for_node(id))
                .map(str::to_owned);
            let (reason, selector) = transition_reason(&successor.reason);
            let counts = to.as_ref().and_then(|to| {
                dag.state
                    .edge_counts
                    .get(&(node.step_name.clone(), to.clone()))
            });
            edges.push(DagEdgeDto {
                from: node.step_name.clone(),
                to,
                reason,
                selector,
                traversals: counts.map_or(0, |count| count.traversals),
                budgeted_traversals: counts.map_or(0, |count| count.budgeted_traversals),
            });
        }
    }
    Ok(DagDto {
        start_step,
        steps,
        edges,
        current_step,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DagStepDto {
    pub name: String,
    pub kind: String,
    pub is_terminal: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct DagEdgeDto {
    pub from: String,
    pub to: Option<String>,
    pub reason: String,
    pub selector: Option<String>,
    pub traversals: usize,
    pub budgeted_traversals: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigEntrySource {
    Local,
    User,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntryDto {
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    pub source: Option<ConfigEntrySource>,
}

pub(crate) fn config_entries(
    application: &CruiseApplication,
    base_dir: Option<&str>,
    repo: Option<&str>,
) -> Vec<ConfigEntryDto> {
    let is_repo = repo.is_some();
    let entries = if is_repo {
        application.discover_configs()
    } else {
        let base = expanded_path(Path::new(base_dir.unwrap_or(".")));
        application.discover_config_sources(&base)
    };
    let user_dir = crate::paths::workflows_dir()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok());
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter_map(|entry| {
            if entry.path == BUILTIN_CONFIG_PATH {
                return None;
            }
            let path = PathBuf::from(&entry.path);
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(canonical.clone()) {
                return None;
            }
            let source = if is_repo
                || user_dir
                    .as_ref()
                    .is_some_and(|dir| canonical.starts_with(dir))
            {
                ConfigEntrySource::User
            } else {
                ConfigEntrySource::Local
            };
            Some(ConfigEntryDto {
                source: Some(source),
                path: entry.path,
                name: entry.name,
                description: entry.description,
            })
        })
        .collect()
}

pub(crate) fn dag_dto_for_session(
    application: &CruiseApplication,
    session_id: &str,
) -> Result<Option<DagDto>> {
    let session = application
        .read_session(session_id)
        .map_err(|error| CruiseError::Other(error.to_string()))?;
    let compiled = crate::workflow::compile(
        application
            .session_config(&session)
            .map_err(|error| CruiseError::Other(error.to_string()))?,
    )
    .map_err(|error| CruiseError::Other(error.to_string()))?;
    let dag = application
        .session_dag(session_id)
        .map_err(|error| CruiseError::Other(error.to_string()))?;
    dag.map(|dag| {
        build_dag_dto(
            &compiled,
            &dag,
            session.current_step.as_deref(),
            session.current_step_is_node_id,
            &session.skipped_steps,
        )
        .map_err(CruiseError::Other)
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_dto_preserves_cycles_and_accepted_counts() {
        let config = crate::config::WorkflowConfig::from_yaml(
            "steps:\n  test:\n    command: echo test\n  review:\n    command: echo review\n    if:\n      file-changed: test\n",
        )
        .unwrap_or_else(|error| panic!("valid graph fixture: {error}"));
        let compiled = crate::workflow::compile(config)
            .unwrap_or_else(|error| panic!("compiled fixture: {error}"));
        let mut graph = crate::graph::build_graph(&compiled, 1_000_000)
            .unwrap_or_else(|error| panic!("fixed topology: {error}"));
        graph.state.current = Some("review".into());
        graph.state.edge_counts.insert(
            ("review".into(), "test".into()),
            crate::graph::EdgeCounter {
                traversals: 3,
                budgeted_traversals: 2,
            },
        );
        let dto = build_dag_dto(&compiled, &graph, Some("test"), true, &[])
            .unwrap_or_else(|error| panic!("graph DTO: {error}"));
        assert_eq!(dto.steps.len(), 2);
        assert_eq!(dto.current_step.as_deref(), Some("review"));
        let edge = dto
            .edges
            .iter()
            .find(|edge| edge.from == "review" && edge.to.as_deref() == Some("test"))
            .unwrap_or_else(|| panic!("back edge"));
        assert_eq!((edge.traversals, edge.budgeted_traversals), (3, 2));
        let value =
            serde_json::to_value(edge).unwrap_or_else(|error| panic!("edge serializes: {error}"));
        assert_eq!(value["budgetedTraversals"], 2);
        assert_eq!(value["traversals"], 3);
    }

    #[test]
    fn graph_dto_does_not_mark_error_only_exit_as_normal_terminal() {
        let config = crate::config::WorkflowConfig::from_yaml(
            "steps:\n  loop:\n    command: 'true'\n    next: loop\n    if:\n      no-file-changes: failed\n",
        )
        .unwrap_or_else(|error| panic!("valid graph fixture: {error}"));
        let compiled = crate::workflow::compile(config)
            .unwrap_or_else(|error| panic!("compiled fixture: {error}"));
        let graph = crate::graph::build_graph(&compiled, 3)
            .unwrap_or_else(|error| panic!("fixed topology: {error}"));

        let dto = build_dag_dto(&compiled, &graph, None, false, &[])
            .unwrap_or_else(|error| panic!("graph DTO: {error}"));

        assert_eq!(dto.steps.len(), 1);
        assert!(!dto.steps[0].is_terminal);
        assert!(
            dto.edges
                .iter()
                .any(|edge| { edge.reason == "ifNoFileChangesFail" && edge.to.is_none() })
        );
    }

    #[test]
    fn graph_dto_does_not_mark_skip_false_fallback_as_normal_terminal() {
        let config = crate::config::WorkflowConfig::from_yaml(
            "steps:\n  loop:\n    command: 'true'\n    next: loop\n    skip: false\n",
        )
        .unwrap_or_else(|error| panic!("valid graph fixture: {error}"));
        let compiled = crate::workflow::compile(config)
            .unwrap_or_else(|error| panic!("compiled fixture: {error}"));
        let graph = crate::graph::build_graph(&compiled, 3)
            .unwrap_or_else(|error| panic!("fixed topology: {error}"));

        let dto = build_dag_dto(&compiled, &graph, None, false, &[])
            .unwrap_or_else(|error| panic!("graph DTO: {error}"));

        assert_eq!(dto.steps.len(), 1);
        assert!(!dto.steps[0].is_terminal);
        assert!(
            dto.edges
                .iter()
                .any(|edge| { edge.reason == "skipFallback" && edge.to.is_none() })
        );
    }

    #[test]
    fn session_dto_serializes_nullable_fields_and_camel_case() {
        let dto = SessionDto {
            id: "session-1".to_string(),
            phase: "Planned".to_string(),
            phase_error: None,
            config_source: "config: (builtin default)".to_string(),
            config_path: Some(BUILTIN_CONFIG_PATH.to_string()),
            base_dir: "/tmp/project".to_string(),
            repo: None,
            input: "task".to_string(),
            title: None,
            current_step: Some("build".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: None,
            worktree_branch: None,
            workspace_mode: WorkspaceMode::Worktree,
            pr_url: None,
            updated_at: None,
            awaiting_input: false,
            pending_ask_question: None,
            plan_error: None,
            exec: false,
            plan_available: true,
            fix_in_progress: false,
            skipped_steps: vec![],
        };
        let Ok(value) = serde_json::to_value(dto) else {
            panic!("session DTO serializes");
        };
        assert_eq!(value["configPath"], BUILTIN_CONFIG_PATH);
        assert!(value["planError"].is_null());
        assert_eq!(value["planAvailable"], true);
        assert_eq!(value["fixInProgress"], false);
        assert_eq!(value["currentStep"], "build");
    }

    #[test]
    fn auto_config_values_preserve_explicit_auto_selection() {
        assert_eq!(
            normalize_config_path(Some(String::new())),
            Some(String::new())
        );
        assert_eq!(normalize_config_path(None), None);
        assert_eq!(
            normalize_config_path(Some(BUILTIN_CONFIG_PATH.to_string())),
            Some(BUILTIN_CONFIG_PATH.to_string())
        );
    }
}
