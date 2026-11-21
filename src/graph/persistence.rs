use super::ExecutionGraph;
use crate::error::{CruiseError, Result};
use std::path::Path;
/// Atomically persist graph topology and execution state as minified JSON.
///
/// # Errors
///
/// Returns an error if the file cannot be created or serialization fails.
pub fn save_graph(dag: &ExecutionGraph, path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().map_or_else(
        || "dag.json".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let tmp = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let file = std::fs::File::create(&tmp)?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, dag)?;
        std::io::Write::flush(&mut writer)?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(CruiseError::from)
}

/// Load a checkpoint or read-only legacy graph.
///
/// # Errors
///
/// Returns an error if the file cannot be read or deserialized.
pub fn load_graph(path: &Path) -> Result<ExecutionGraph> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let value: serde_json::Value = serde_json::from_reader(reader).map_err(|e| {
        CruiseError::Other(format!(
            "failed to deserialize DAG at {}: {e}",
            path.display()
        ))
    })?;
    if let Some(version) = value.get("version") {
        if version.as_u64() != Some(u64::from(super::CHECKPOINT_VERSION)) {
            return Err(CruiseError::Other(format!(
                "unknown graph checkpoint version {version} at {}",
                path.display()
            )));
        }
        if value.get("state").is_none() {
            return Err(CruiseError::Other(
                "graph checkpoint is missing its execution state".into(),
            ));
        }
    }
    // Decode legacy display data separately. Never manufacture resumable counts.
    let mut dag = if value.get("version").is_none() {
        let legacy: LegacyGraph = serde_json::from_value(value)?;
        ExecutionGraph {
            version: 0,
            start: legacy.start,
            nodes: legacy.nodes,
            predecessors: indexmap::IndexMap::new(),
            max_retries: legacy.max_retries,
            group_sites: std::collections::HashSet::default(),
            state: super::ExecutionState::default(),
        }
    } else {
        serde_json::from_value(value)?
    };
    dag.rebuild_indexes();
    if dag.version == super::CHECKPOINT_VERSION {
        validate_checkpoint(&dag)?;
    }
    Ok(dag)
}

#[derive(serde::Deserialize)]
struct LegacyGraph {
    start: super::NodeId,
    nodes: indexmap::IndexMap<super::NodeId, super::GraphNode>,
    max_retries: usize,
}

fn validate_checkpoint(graph: &ExecutionGraph) -> Result<()> {
    let invalid = || {
        CruiseError::Other(
            "inconsistent graph checkpoint; preserve the file and explicitly restart".into(),
        )
    };
    if !graph.nodes.contains_key(&graph.start) {
        return Err(invalid());
    }
    for (id, node) in &graph.nodes {
        if id != &node.id || id != &node.step_name {
            return Err(invalid());
        }
        if node.successors.iter().any(|edge| {
            edge.target
                .as_ref()
                .is_some_and(|to| !graph.nodes.contains_key(to))
        }) {
            return Err(invalid());
        }
    }
    if graph
        .state
        .current
        .as_ref()
        .is_some_and(|id| !graph.nodes.contains_key(id))
        || (graph.state.completed
            && (graph.state.current.is_some() || graph.state.pending.is_some()))
        || (graph.state.pending.is_some() && graph.state.current.is_none())
        || (!graph.state.completed
            && graph.state.current.is_none()
            && (!graph.state.edge_counts.is_empty()
                || !graph.state.group_counts.is_empty()
                || graph.state.runtime != super::NodeRuntime::default()))
        || graph
            .state
            .group_counts
            .keys()
            .any(|site| !graph.group_sites.contains(site))
    {
        return Err(invalid());
    }
    if let (Some(current), Some(pending)) = (&graph.state.current, &graph.state.pending)
        && !graph.valid_pending_transition(current, pending)
    {
        return Err(invalid());
    }
    let pairs = graph.edge_pairs();
    for (pair, count) in &graph.state.edge_counts {
        if count.budgeted_traversals > count.traversals || !pairs.contains(pair) {
            return Err(invalid());
        }
    }
    graph.validate_exec_failure(graph)?;
    Ok(())
}

/// Default file name for a persisted DAG inside a session directory.
pub const DAG_FILE_NAME: &str = "dag.json";

pub(crate) struct ExecResume {
    pub node: super::NodeId,
    pub rewound: bool,
}

/// Select an exec retry point in memory. Only the run admission path commits it.
pub(crate) fn prepare_exec_resume(
    graph: &mut ExecutionGraph,
    path: &Path,
    has_checkpoint: bool,
    failed: bool,
    execution_id: Option<&str>,
) -> Result<ExecResume> {
    let mut node = prepare_resume(graph, path, has_checkpoint, None, false, execution_id)?;
    let positioned = graph.state.completed || graph.state.current.is_some();
    if (has_checkpoint && !positioned)
        || (positioned && graph.state.execution_id.as_deref() != execution_id)
    {
        return Err(CruiseError::Other(
            "started exec has no reliable execution checkpoint".into(),
        ));
    }
    let mut rewound = false;
    if failed || graph.state.completed {
        if let Some(failure) = graph.state.exec_failure.take() {
            node = failure.node;
            graph.state.current = Some(node.clone());
            graph.state.runtime = failure.runtime;
            graph.state.edge_counts = failure.edge_counts;
            graph.state.group_counts = failure.group_counts;
            graph.state.pending = None;
            graph.state.completed = false;
            rewound = true;
        } else if failed && graph.state.completed {
            return Err(CruiseError::Other(
                "completed exec checkpoint has no reliable retry point".into(),
            ));
        }
    }
    Ok(ExecResume { node, rewound })
}

/// Restore checkpoint authority before workspace creation. A step-name override
/// is an explicit reposition operation, not a fresh retry budget.
///
/// # Errors
/// Missing, corrupt, legacy and incompatible checkpoints cannot silently restart.
pub fn prepare_resume(
    graph: &mut ExecutionGraph,
    path: &Path,
    has_checkpoint: bool,
    saved_step: Option<&str>,
    is_node_id: bool,
    execution_id: Option<&str>,
) -> Result<String> {
    graph.state.execution_id = execution_id.map(str::to_owned);
    let mut restored = false;
    if has_checkpoint || path.exists() {
        let mut saved = load_graph(path)?;
        let no_position = !saved.state.completed && saved.state.current.is_none();
        let saved_execution_id = saved.state.execution_id.as_deref();
        let topology_only = no_position
            && saved.state.pending.is_none()
            && saved.state.edge_counts.is_empty()
            && saved.state.group_counts.is_empty()
            && saved.state.runtime == super::NodeRuntime::default()
            && saved_execution_id.is_none();
        // A topology-only graph created before execution has no execution ID
        // and is a valid fresh start. Once a checkpoint claims an execution,
        // however, a missing position is incomplete state and must not silently
        // restart with unknown side effects or counters.
        if no_position && saved_execution_id.is_some() {
            return Err(CruiseError::Other(
                "incomplete execution checkpoint; explicitly restart the session".into(),
            ));
        }
        if let Some(step) = saved_step.filter(|_| !is_node_id) {
            let target = graph
                .first_node_for_step(step)
                .cloned()
                .ok_or_else(|| CruiseError::StepNotFound(step.into()))?;
            saved.state.current = Some(target);
            saved.state.pending = None;
            saved.state.completed = false;
        }
        if saved.version != super::CHECKPOINT_VERSION {
            return Err(CruiseError::Other(
                "legacy graph has no reliable traversal counters; explicitly restart this session from the beginning".into(),
            ));
        }
        if saved_execution_id.is_some_and(|saved_id| Some(saved_id) != execution_id) {
            return Err(CruiseError::Other(
                "execution checkpoint belongs to a different execution; explicitly restart the session".into(),
            ));
        }
        graph.restore(&saved)?;
        if topology_only {
            graph.state.execution_id = execution_id.map(str::to_owned);
        }
        restored = true;
    } else if is_node_id || (saved_step.is_some() && execution_id.is_none()) {
        // A legacy session may store only a human-readable step name. Without
        // the graph checkpoint there is no reliable way to recover accepted
        // edge/group counts, so do not silently continue with zero counters.
        return Err(CruiseError::Other(
            "missing execution checkpoint; explicitly restart the session".into(),
        ));
    }
    if let Some(step) = saved_step.filter(|_| !restored || !is_node_id) {
        let id = graph
            .first_node_for_step(step)
            .cloned()
            .ok_or_else(|| CruiseError::StepNotFound(step.into()))?;
        graph.state.current = Some(id);
        graph.state.pending = None;
        graph.state.completed = false;
    }
    Ok(graph
        .state
        .current
        .clone()
        .unwrap_or_else(|| graph.start.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeCounter, ExecFailure, NodeRuntime, build_graph};
    use std::collections::HashMap;

    #[test]
    fn exec_retry_prefix_rejects_zero_traversals_on_load_and_reload() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(DAG_FILE_NAME);
        let config = crate::config::WorkflowConfig::from_yaml(
            "command: [echo]\ngroups:\n  checks:\n    max_retries: 1\n    if:\n      file-changed: prefix\n    steps:\n      verify:\n        command: 'true'\nsteps:\n  prefix:\n    command: 'true'\n  checks:\n    group: checks\n  repair:\n    command: 'false'\n",
        )
        .expect("valid retry prefix fixture");
        let fresh = build_graph(&crate::workflow::compile(config)?, 3)?;
        let pair = ("prefix".to_owned(), "checks/verify".to_owned());
        let mut saved = fresh.clone();
        saved.state.execution_id = Some("exec".into());
        saved.state.completed = true;
        saved.state.edge_counts.insert(pair.clone(), EdgeCounter {
            traversals: 1,
            budgeted_traversals: 0,
        });
        saved.state.group_counts.insert("checks".into(), 1);
        saved.state.exec_failure = Some(ExecFailure {
            unresolved: 1,
            node: "repair".into(),
            runtime: NodeRuntime::default(),
            edge_counts: saved.state.edge_counts.clone(),
            group_counts: saved.state.group_counts.clone(),
        });
        save_graph(&saved, &path)?;
        load_graph(&path)?;
        fresh.clone().restore_for_reload(&saved)?;
        for zero_edge in [true, false] {
            let mut damaged = saved.clone();
            let failure = damaged.state.exec_failure.as_mut().expect("retry prefix");
            if zero_edge {
                failure.edge_counts.get_mut(&pair).expect("known edge").traversals = 0;
            } else {
                *failure.group_counts.get_mut("checks").expect("known group") = 0;
            }
            save_graph(&damaged, &path)?;
            assert!(load_graph(&path).is_err(), "zero_edge={zero_edge}");
            assert!(fresh.clone().restore_for_reload(&damaged).is_err(), "zero_edge={zero_edge}");
        }
        Ok(())
    }

    #[test]
    fn failed_exec_rejects_missing_corrupt_or_unrecoverable_started_checkpoint() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(DAG_FILE_NAME);
        let compile = |steps| {
            let config = crate::config::WorkflowConfig::from_yaml(steps)
                .unwrap_or_else(|error| panic!("invalid fixture: {error}"));
            crate::workflow::compile(config)
        };
        let compiled = compile("command: [echo]\nsteps:\n  first:\n    command: 'true'\n")?;
        let fresh = build_graph(&compiled, 3)?;
        assert_eq!(
            prepare_exec_resume(&mut fresh.clone(), &path, false, true, Some("exec"))?.node,
            "first"
        );
        assert!(prepare_exec_resume(&mut fresh.clone(), &path, true, true, Some("exec")).is_err());
        let mut completed = fresh.clone();
        completed.state.execution_id = Some("exec".into());
        completed.state.completed = true;
        let no_retry_point = serde_json::to_string(&completed)?;
        completed.state.exec_failure = Some(ExecFailure {
            unresolved: 1,
            node: "removed".into(),
            runtime: NodeRuntime::default(),
            edge_counts: HashMap::new(),
            group_counts: HashMap::new(),
        });
        let removed_retry_point = serde_json::to_string(&completed)?;
        if let Some(failure) = &mut completed.state.exec_failure {
            failure.node = "first".into();
        }
        let reload = compile("command: [echo]\nsteps:\n  new:\n    command: 'true'\n")?;
        assert!(
            build_graph(&reload, 3)?
                .restore_for_reload(&completed)
                .is_err()
        );
        if let Some(failure) = &mut completed.state.exec_failure {
            failure.edge_counts.insert(
                ("first".into(), "removed".into()),
                EdgeCounter {
                    traversals: 1,
                    budgeted_traversals: 1,
                },
            );
        }
        let corrupt_prefix = serde_json::to_string(&completed)?;
        for bad in [
            "{corrupt".into(),
            serde_json::to_string(&fresh)?,
            no_retry_point,
            removed_retry_point,
            corrupt_prefix,
        ] {
            std::fs::write(&path, &bad)?;
            assert!(
                prepare_exec_resume(&mut fresh.clone(), &path, true, true, Some("exec")).is_err()
            );
            assert_eq!(std::fs::read_to_string(&path)?, bad);
        }
        Ok(())
    }
}
