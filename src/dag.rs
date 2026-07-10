use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::config::FailAction;
use crate::error::{CruiseError, Result};
use crate::workflow::CompiledWorkflow;

/// Stable identifier for a node in the execution DAG.
pub type NodeId = String;

/// Maximum number of DAG nodes the builder will create before bailing out.
///
/// This protects against pathological workflows where independent branches
/// combine to produce an exponential number of counter states.
/// The fundamental mitigation is lowering `--max-retries`; this budget is a
/// last-resort safety net.
const DAG_NODE_BUDGET: usize = 100_000;

/// An execution DAG is a precomputed, fully-resumable graph of every loop
/// iteration a workflow can take given a `max_retries` budget.
///
/// Each node represents one visit to a compiled step together with the exact
/// counter state that led there.  The node also stores the runtime values
/// (`prev_*` variables and file tracker snapshots) captured when the node was
/// last visited, which makes resumption deterministic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionDag {
    /// Identifier of the entry node.
    pub start: NodeId,
    /// All nodes in the DAG, keyed by id.  Order matches creation order so
    /// listing sessions enumerates nodes in a natural progression.
    pub nodes: IndexMap<NodeId, DagNode>,
    /// `max_retries` value the DAG was built for.  Used to invalidate a cached
    /// DAG when the CLI flag changes.
    pub max_retries: usize,
}

/// A single node in the execution DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DagNode {
    pub id: NodeId,
    /// Original compiled step name.  Used by the UI/CLI to show a human
    /// readable current step even though `current_step` stores a node id.
    pub step_name: String,
    /// All transitions that can follow this node.
    pub successors: Vec<NodeSuccessor>,
    /// Runtime data written back after the node is executed.
    pub runtime: NodeRuntime,
}

/// A possible transition from one node to another.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct NodeSuccessor {
    /// Why this transition is taken.
    pub reason: TransitionReason,
    /// Target node id, or `None` when this transition leaves the workflow.
    pub target: Option<NodeId>,
}

/// Reasons a step can transition to its successor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TransitionReason {
    /// Next step according to the compiled step order.
    Sequential,
    /// Explicit `next:` field.
    Next,
    /// `if.file-changed:` triggered a jump to the named step.
    IfFileChanged { target: String },
    /// `if.no-file-changes.retry: true` re-executes the current step.
    IfNoFileChangesRetry,
    /// `if.no-file-changes.fail: true` terminates the workflow.
    IfNoFileChangesFail,
    /// `if.fail:` jumped to the named step.
    IfFailGoto { target: String },
    /// `if.fail: { retry: true }` re-executes the current step.
    IfFailRetry,
    /// An option item with the given label was selected.
    OptionChoice { selector: String },
    /// Group-level `if.file-changed` triggered a retry jump.
    GroupRetry { target: String },
    /// Group retry budget exhausted; the invocation is skipped.
    GroupRetryExhausted,
    /// Sequential-order fallback used when a step with `next:` is skipped.
    SkipFallback,
}

/// Runtime data stored inside a DAG node.
///
/// The engine writes `prev_output`/`prev_input`/`prev_stderr`/`prev_success`
/// and `file_snapshots` onto a node right *before* it executes: they capture
/// the `{prev.*}` variables and file-tracker snapshots produced by whatever
/// ran immediately before this node. That is exactly the context a caller
/// needs to restore in order to resume execution at this node, and it is
/// written before the node's own step runs so a checkpoint saved at that
/// point (see `on_node_start` in [`crate::engine::execute_steps_with_dag`])
/// is sufficient to resume deterministically -- no in-memory state needs to
/// be reconstructed.
///
/// `visited_at` is written *after* the node's step finishes executing, and
/// is purely a diagnostic marker (a node whose id is saved as the resume
/// point was, by definition, not yet visited when the session was
/// interrupted, so its own `visited_at` is typically unset).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeRuntime {
    pub prev_output: Option<String>,
    pub prev_input: Option<String>,
    pub prev_stderr: Option<String>,
    pub prev_success: Option<bool>,
    /// File tracker snapshots keyed by snapshot name.
    pub file_snapshots: HashMapSnapshot,
    /// ISO-8601 timestamp of the last visit, for debugging.
    pub visited_at: Option<String>,
}

/// Snapshot storage type used inside `NodeRuntime`.
pub type HashMapSnapshot =
    std::collections::HashMap<String, std::collections::HashMap<PathBuf, [u8; 32]>>;

/// Internal state key used while expanding the DAG.
///
/// Two visits to the same compiled step with different loop-counter states are
/// represented by different DAG nodes so that resumption can continue from the
/// exact iteration.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct StateKey {
    step: String,
    /// Number of times each edge has been traversed so far.
    edge_counts: BTreeMap<(String, String), usize>,
    /// Number of file-change retries consumed by each group invocation.
    group_counts: BTreeMap<String, usize>,
}

impl StateKey {
    fn new(step: String) -> Self {
        Self {
            step,
            edge_counts: BTreeMap::new(),
            group_counts: BTreeMap::new(),
        }
    }

    fn for_step(&self, step: &str) -> Self {
        let mut cloned = self.clone();
        cloned.step = step.to_string();
        cloned
    }

    fn with_edge_increment(mut self, from: &str, to: &str) -> Self {
        *self
            .edge_counts
            .entry((from.to_string(), to.to_string()))
            .or_insert(0) += 1;
        self
    }

    fn with_group_increment(mut self, call_site: &str) -> Self {
        *self.group_counts.entry(call_site.to_string()).or_insert(0) += 1;
        self
    }
}

/// Build an execution DAG for a compiled workflow.
///
/// # Errors
///
/// Returns an error if the workflow references unknown steps or if the DAG
/// would exceed the node budget.
pub fn build_dag(compiled: &CompiledWorkflow, max_retries: usize) -> Result<ExecutionDag> {
    let first_step = compiled
        .steps
        .first()
        .map(|(name, _)| name.clone())
        .ok_or_else(|| CruiseError::InvalidStepConfig("workflow has no steps".to_string()))?;

    let mut state_to_id: IndexMap<StateKey, NodeId> = IndexMap::new();
    let mut nodes: IndexMap<NodeId, DagNode> = IndexMap::new();
    let mut worklist: Vec<StateKey> = Vec::new();

    let start_key = StateKey::new(first_step.clone());
    let start_id = allocate_node(&start_key, &mut state_to_id, &mut nodes, &mut worklist);

    while let Some(key) = worklist.pop() {
        if nodes.len() > DAG_NODE_BUDGET {
            return Err(CruiseError::InvalidStepConfig(format!(
                "DAG would exceed {DAG_NODE_BUDGET} nodes"
            )));
        }

        let id = state_to_id.get(&key).cloned().ok_or_else(|| {
            CruiseError::InvalidStepConfig("worklist key missing from state_to_id".to_string())
        })?;
        let successors = compute_successors(compiled, &key, max_retries)?;
        let mut node_successors = Vec::with_capacity(successors.len());

        for (reason, target_step, new_key) in successors {
            let target_id = if let Some(_step_name) = target_step {
                if let Some(existing) = state_to_id.get(&new_key) {
                    existing.clone()
                } else {
                    allocate_node(&new_key, &mut state_to_id, &mut nodes, &mut worklist)
                }
            } else {
                // Terminal transition: no runtime state to persist.
                node_successors.push(NodeSuccessor {
                    reason,
                    target: None,
                });
                continue;
            };

            node_successors.push(NodeSuccessor {
                reason,
                target: Some(target_id),
            });
        }

        nodes
            .get_mut(&id)
            .ok_or_else(|| {
                CruiseError::InvalidStepConfig("node missing from nodes map".to_string())
            })?
            .successors = node_successors;
    }

    Ok(ExecutionDag {
        start: start_id,
        nodes,
        max_retries,
    })
}

fn allocate_node(
    key: &StateKey,
    state_to_id: &mut IndexMap<StateKey, NodeId>,
    nodes: &mut IndexMap<NodeId, DagNode>,
    worklist: &mut Vec<StateKey>,
) -> NodeId {
    let id = format!("n{:04}", state_to_id.len());
    state_to_id.insert(key.clone(), id.clone());
    nodes.insert(
        id.clone(),
        DagNode {
            id: id.clone(),
            step_name: key.step.clone(),
            successors: Vec::new(),
            runtime: NodeRuntime::default(),
        },
    );
    worklist.push(key.clone());
    id
}

#[expect(clippy::too_many_lines)]
fn compute_successors(
    compiled: &CompiledWorkflow,
    key: &StateKey,
    max_retries: usize,
) -> Result<Vec<(TransitionReason, Option<String>, StateKey)>> {
    let step = &key.step;
    let step_config = compiled
        .steps
        .get(step)
        .ok_or_else(|| CruiseError::StepNotFound(step.clone()))?;
    let call_site = compiled
        .step_to_invocation
        .get(step)
        .map(std::string::String::as_str);

    // Group retry exhaustion is checked at the first step of an invocation.
    if let Some(cs) = call_site
        && let Some(meta) = compiled.invocations.get(cs)
        && meta.first_step == *step
        && let Some(max) = meta.max_retries
        && key.group_counts.get(cs).copied().unwrap_or(0) >= max
    {
        let target = sequential_next(&compiled.steps, &meta.last_step).cloned();
        let new_key = target
            .as_ref()
            .map_or_else(|| key.clone(), |t| key.for_step(t));
        return Ok(vec![(
            TransitionReason::GroupRetryExhausted,
            target,
            new_key,
        )]);
    }

    let mut successors = Vec::new();
    let normal_target = explicit_or_sequential_next(compiled, step, step_config.next.as_deref())?;

    // Normal "condition did not trigger" path.
    let sequential_reason = if step_config.next.is_some() {
        TransitionReason::Next
    } else {
        TransitionReason::Sequential
    };
    match normal_target.as_deref() {
        Some(target) => {
            let new_key = key.for_step(target);
            push_transition(
                &mut successors,
                sequential_reason,
                Some(target),
                &new_key,
                step,
                max_retries,
            );

            if step_config.next.is_some()
                && let Some(fallback_target) = sequential_next(&compiled.steps, step)
                && fallback_target != target
            {
                let fallback_key = key.for_step(fallback_target);
                push_unbudgeted_transition(
                    &mut successors,
                    TransitionReason::SkipFallback,
                    Some(fallback_target),
                    &fallback_key,
                );
            }
        }
        None => {
            successors.push((sequential_reason, None, key.clone()));
        }
    }

    let if_cond = step_config.if_condition.as_ref();
    let step_if_file_changed = if_cond.and_then(|c| c.file_changed.as_deref());
    let nfc_cond = if_cond.and_then(|c| c.no_file_changes.as_ref());
    let if_fail = if_cond.and_then(|c| c.fail.as_ref());

    // `if.file-changed:` branch.
    if let Some(target) = step_if_file_changed {
        let mut new_key = key.for_step(target).with_edge_increment(step, target);
        if let Some(cs) = call_site {
            new_key = new_key.with_group_increment(cs);
        }
        push_transition(
            &mut successors,
            TransitionReason::IfFileChanged {
                target: target.to_string(),
            },
            Some(target),
            &new_key,
            step,
            max_retries,
        );
    }

    // `if.no-file-changes:` branches.
    if let Some(nfc) = nfc_cond {
        if nfc.retry {
            let new_key = key.for_step(step).with_edge_increment(step, step);
            push_transition(
                &mut successors,
                TransitionReason::IfNoFileChangesRetry,
                Some(step),
                &new_key,
                step,
                max_retries,
            );
        } else if nfc.fail {
            successors.push((TransitionReason::IfNoFileChangesFail, None, key.clone()));
        }
    }

    // `if.fail:` branches.
    if let Some(fail) = if_fail {
        match fail {
            FailAction::Goto(target) => {
                let new_key = key.for_step(target).with_edge_increment(step, target);
                push_transition(
                    &mut successors,
                    TransitionReason::IfFailGoto {
                        target: target.clone(),
                    },
                    Some(target),
                    &new_key,
                    step,
                    max_retries,
                );
            }
            FailAction::Detailed(d) if d.retry => {
                let new_key = key.for_step(step).with_edge_increment(step, step);
                push_transition(
                    &mut successors,
                    TransitionReason::IfFailRetry,
                    Some(step),
                    &new_key,
                    step,
                    max_retries,
                );
            }
            FailAction::Detailed(_) => {}
        }
    }

    // Option step branches replace the generic sequential path.
    if let Some(options) = step_config.option.as_ref() {
        successors.retain(|(reason, _, _)| *reason != TransitionReason::Sequential);
        for item in options {
            let selector = item
                .selector
                .clone()
                .or_else(|| item.text_input.clone())
                .unwrap_or_default();
            let target = item
                .next
                .clone()
                .or_else(|| sequential_next(&compiled.steps, step).cloned());
            if let Some(ref t) = item.next {
                let new_key = key.for_step(t).with_edge_increment(step, t);
                push_transition(
                    &mut successors,
                    TransitionReason::OptionChoice {
                        selector: selector.clone(),
                    },
                    Some(t),
                    &new_key,
                    step,
                    max_retries,
                );
            } else if let Some(ref t) = target {
                let new_key = key.for_step(t).with_edge_increment(step, t);
                push_transition(
                    &mut successors,
                    TransitionReason::OptionChoice {
                        selector: selector.clone(),
                    },
                    Some(t),
                    &new_key,
                    step,
                    max_retries,
                );
            } else {
                successors.push((
                    TransitionReason::OptionChoice {
                        selector: selector.clone(),
                    },
                    None,
                    key.clone(),
                ));
            }
        }
    }

    // Group-level `if.file-changed:` at the last step of an invocation.
    if let Some(cs) = call_site
        && let Some(meta) = compiled.invocations.get(cs)
        && meta.last_step == *step
        && let Some(ref cond) = meta.if_condition
        && let Some(target) = cond.file_changed.as_deref()
    {
        let current_count = key.group_counts.get(cs).copied().unwrap_or(0);
        if let Some(max) = meta.max_retries
            && current_count < max
        {
            let new_key = key
                .for_step(target)
                .with_group_increment(cs)
                .with_edge_increment(step, target);
            push_transition(
                &mut successors,
                TransitionReason::GroupRetry {
                    target: target.to_string(),
                },
                Some(target),
                &new_key,
                step,
                max_retries,
            );
        }
    }

    Ok(successors)
}

fn push_transition(
    successors: &mut Vec<(TransitionReason, Option<String>, StateKey)>,
    reason: TransitionReason,
    target: Option<&str>,
    new_key: &StateKey,
    from: &str,
    max_retries: usize,
) {
    match target {
        Some(to) => {
            if is_within_budget(new_key, max_retries, from, to) {
                successors.push((reason, Some(to.to_string()), new_key.clone()));
            } else {
                successors.push((reason, None, new_key.clone()));
            }
        }
        None => successors.push((reason, None, new_key.clone())),
    }
}

fn push_unbudgeted_transition(
    successors: &mut Vec<(TransitionReason, Option<String>, StateKey)>,
    reason: TransitionReason,
    target: Option<&str>,
    new_key: &StateKey,
) {
    successors.push((reason, target.map(str::to_string), new_key.clone()));
}

fn is_within_budget(key: &StateKey, max_retries: usize, from: &str, to: &str) -> bool {
    key.edge_counts
        .get(&(from.to_string(), to.to_string()))
        .copied()
        .unwrap_or(0)
        <= max_retries
}

fn explicit_or_sequential_next(
    compiled: &CompiledWorkflow,
    current: &str,
    explicit: Option<&str>,
) -> Result<Option<String>> {
    if let Some(target) = explicit {
        if !compiled.steps.contains_key(target) {
            return Err(CruiseError::StepNotFound(target.to_string()));
        }
        return Ok(Some(target.to_string()));
    }
    Ok(sequential_next(&compiled.steps, current).cloned())
}

fn sequential_next<'a>(
    steps: &'a IndexMap<String, crate::config::StepConfig>,
    current: &str,
) -> Option<&'a String> {
    let mut found = false;
    for name in steps.keys() {
        if found {
            return Some(name);
        }
        if name == current {
            found = true;
        }
    }
    None
}

/// Persist a DAG as minified JSON.
///
/// # Errors
///
/// Returns an error if the file cannot be created or serialization fails.
pub fn save_dag(dag: &ExecutionDag, path: &Path) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, dag)?;
    std::io::Write::flush(&mut writer)?;
    Ok(())
}

/// Load a DAG previously saved with [`save_dag`].
///
/// # Errors
///
/// Returns an error if the file cannot be read or deserialized.
pub fn load_dag(path: &Path) -> Result<ExecutionDag> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let dag = serde_json::from_reader(reader).map_err(|e| {
        CruiseError::Other(format!(
            "failed to deserialize DAG at {}: {e}",
            path.display()
        ))
    })?;
    Ok(dag)
}

/// Default file name for a persisted DAG inside a session directory.
pub const DAG_FILE_NAME: &str = "dag.json";

impl ExecutionDag {
    /// Return the step name associated with `node_id`, or `None` if the id is
    /// not present in the DAG.
    #[must_use]
    pub fn step_name_for_node(&self, node_id: &str) -> Option<&str> {
        self.nodes.get(node_id).map(|n| n.step_name.as_str())
    }

    /// Return the id of the first node (in insertion order) whose `step_name`
    /// matches `step_name`, or `None` if no such node exists.
    ///
    /// When a step appears in multiple nodes (looping workflow), this returns
    /// the earliest node so that "resume by step name" always starts from the
    /// first iteration.
    #[must_use]
    pub fn first_node_for_step(&self, step_name: &str) -> Option<&NodeId> {
        self.nodes
            .iter()
            .find(|(_, n)| n.step_name == step_name)
            .map(|(id, _)| id)
    }

    /// Copy `runtime` data from `other` onto every node id that exists in
    /// both graphs, leaving `self`'s graph structure (`start`, `successors`)
    /// untouched.
    ///
    /// Used when resuming a session: the freshly rebuilt DAG (source of
    /// truth for the current graph structure, since the workflow config may
    /// have changed since the session last ran) receives back the runtime
    /// context recorded in a previously persisted DAG. Node ids that no
    /// longer exist (or are new) are silently skipped, so a structural
    /// mismatch degrades gracefully to a partial (or empty) restore rather
    /// than an error.
    pub fn adopt_runtime_from(&mut self, other: &ExecutionDag) {
        for (id, other_node) in &other.nodes {
            if let Some(node) = self.nodes.get_mut(id) {
                node.runtime = other_node.runtime.clone();
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use crate::workflow::compile;
    use std::collections::HashSet;

    fn compile_yaml(yaml: &str) -> CompiledWorkflow {
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        compile(config).unwrap_or_else(|e| panic!("{e:?}"))
    }

    #[test]
    fn test_dag_linear_workflow() {
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo one
  step2:
    command: echo two
",
        );

        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(dag.nodes.len(), 2);
        assert_eq!(dag.start, "n0000");
        assert_eq!(dag.nodes[&dag.start].step_name, "step1");

        let first = &dag.nodes[&dag.start];
        assert_eq!(first.successors.len(), 1);
        assert_eq!(first.successors[0].reason, TransitionReason::Sequential);
        let second_id = first.successors[0].target.as_ref().unwrap();
        assert_eq!(dag.nodes[second_id].step_name, "step2");

        let second = &dag.nodes[second_id];
        assert_eq!(second.successors.len(), 1);
        assert_eq!(second.successors[0].target, None);
        assert_eq!(second.successors[0].reason, TransitionReason::Sequential);
    }

    #[test]
    fn test_dag_explicit_next() {
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  a:
    command: echo a
  b:
    command: echo b
    next: a
  c:
    command: echo c
",
        );

        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        let a = &dag.nodes[&dag.start];
        assert_eq!(a.successors.len(), 1);
        assert_eq!(a.successors[0].reason, TransitionReason::Sequential);
        let b_id = a.successors[0].target.as_ref().unwrap();

        let b = &dag.nodes[b_id];
        assert_eq!(b.step_name, "b");
        assert_eq!(b.successors.len(), 2);
        assert!(b.successors.iter().any(|s| {
            s.reason == TransitionReason::Next
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "a")
        }));
        assert!(b.successors.iter().any(|s| {
            s.reason == TransitionReason::SkipFallback
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "c")
        }));
    }

    #[test]
    fn test_dag_next_step_has_skip_fallback_edge() {
        // Given: a step with an explicit `next:` jump to a non-sequential target.
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  a:
    command: echo a
  b:
    command: echo b
    next: a
  c:
    command: echo c
",
        );

        // When: the execution DAG is built.
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));
        let a = &dag.nodes[&dag.start];
        let b_id = a.successors[0].target.as_ref().unwrap();
        let b = &dag.nodes[b_id];

        // Then: the step can either follow `next:` or fall back to definition order when skipped.
        assert_eq!(b.step_name, "b");
        assert_eq!(b.successors.len(), 2);
        assert!(b.successors.iter().any(|s| {
            s.reason == TransitionReason::Next
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "a")
        }));
        assert!(b.successors.iter().any(|s| {
            s.reason == TransitionReason::SkipFallback
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "c")
        }));
    }

    #[test]
    fn test_dag_skip_fallback_bypasses_retry_budget() {
        // Given: `b -> c` is a real loop edge that can exhaust its retry budget,
        // and `b` also has a skip-only fallback to sequential step `c`.
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  a:
    command: echo a
  b:
    command: echo b
    next: a
    if:
      file-changed: c
  c:
    command: echo c
    next: b
",
        );

        // When: the execution DAG is built with no retry allowance for real loop edges.
        let dag = build_dag(&compiled, 0).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: even after the real `b -> c` edge count is over budget, skipped-step
        // fallback from b to c remains non-terminal because it is not a retry edge.
        let b_with_exhausted_file_changed_to_c = dag.nodes.values().find(|node| {
            node.step_name == "b"
                && node.successors.iter().any(|s| {
                    matches!(
                        s.reason,
                        TransitionReason::IfFileChanged { ref target } if target == "c"
                    ) && s.target.is_none()
                })
        });
        let b = b_with_exhausted_file_changed_to_c
            .unwrap_or_else(|| panic!("expected b node with exhausted file-changed edge to c"));

        assert!(b.successors.iter().any(|s| {
            s.reason == TransitionReason::SkipFallback
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "c")
        }));
    }

    #[test]
    fn test_dag_next_to_sequential_target_has_single_edge() {
        // Given: `next:` points to the same step as definition-order sequencing.
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  a:
    command: echo a
    next: b
  b:
    command: echo b
",
        );

        // When: the execution DAG is built.
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));
        let a = &dag.nodes[&dag.start];

        // Then: the DAG has only the existing `next:` edge, with no duplicate fallback edge.
        assert_eq!(a.step_name, "a");
        assert_eq!(a.successors.len(), 1);
        assert_eq!(a.successors[0].reason, TransitionReason::Next);
        assert_eq!(
            dag.nodes[a.successors[0].target.as_ref().unwrap()].step_name,
            "b"
        );
    }

    #[test]
    fn test_dag_last_step_with_next_has_no_skip_fallback() {
        // Given: the final step has an explicit `next:` but no definition-order successor.
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  a:
    command: echo a
  b:
    command: echo b
    next: a
",
        );

        // When: the execution DAG is built.
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));
        let a = &dag.nodes[&dag.start];
        let b_id = a.successors[0].target.as_ref().unwrap();
        let b = &dag.nodes[b_id];

        // Then: the final step only has the explicit `next:` edge.
        assert_eq!(b.step_name, "b");
        assert_eq!(b.successors.len(), 1);
        assert_eq!(b.successors[0].reason, TransitionReason::Next);
        assert_eq!(
            dag.nodes[b.successors[0].target.as_ref().unwrap()].step_name,
            "a"
        );
    }

    #[test]
    fn test_dag_if_fail_retry_caps_at_max_retries() {
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: exit 1
    if:
      fail:
        retry: true
  step2:
    command: echo done
",
        );

        let dag = build_dag(&compiled, 2).unwrap_or_else(|e| panic!("{e:?}"));

        // Every path must terminate; there must be at least one node whose
        // retry transition is terminal (loop-protection node).
        let terminal_retries: Vec<&NodeId> = dag
            .nodes
            .values()
            .filter(|n| {
                n.step_name == "step1"
                    && n.successors
                        .iter()
                        .any(|s| s.reason == TransitionReason::IfFailRetry && s.target.is_none())
            })
            .map(|n| &n.id)
            .collect();
        assert!(
            !terminal_retries.is_empty(),
            "expected a step1 node with terminal retry transition"
        );

        // Every reachable node must have at least one outgoing edge.
        for node in dag.nodes.values() {
            assert!(
                !node.successors.is_empty(),
                "node {} has no successors",
                node.id
            );
        }
    }

    #[test]
    fn test_dag_if_file_changed_loop() {
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  edit:
    command: echo edit
  check:
    command: echo check
    if:
      file-changed: edit
",
        );

        let dag = build_dag(&compiled, 2).unwrap_or_else(|e| panic!("{e:?}"));

        let edit = &dag.nodes[&dag.start];
        assert_eq!(edit.step_name, "edit");
        let check_id = edit.successors[0].target.as_ref().unwrap();

        // The check node must offer both a "files changed" jump back to edit
        // and a normal sequential exit.
        let check = &dag.nodes[check_id];
        assert!(check.successors.iter().any(|s| matches!(
            s.reason,
            TransitionReason::IfFileChanged { ref target } if target == "edit"
        )));
        assert!(
            check
                .successors
                .iter()
                .any(|s| s.reason == TransitionReason::Sequential)
        );
    }

    #[test]
    fn test_dag_option_branches() {
        let compiled = compile_yaml(
            r#"
command: [echo]
steps:
  choose:
    option:
      - selector: "Go to a"
        next: a
      - selector: "Go to b"
        next: b
  a:
    command: echo a
  b:
    command: echo b
"#,
        );

        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        let choose = &dag.nodes[&dag.start];
        assert_eq!(choose.step_name, "choose");
        let option_reasons: Vec<_> = choose.successors.iter().map(|s| s.reason.clone()).collect();
        assert!(option_reasons.contains(&TransitionReason::OptionChoice {
            selector: "Go to a".to_string(),
        }));
        assert!(option_reasons.contains(&TransitionReason::OptionChoice {
            selector: "Go to b".to_string(),
        }));
        assert!(
            !choose
                .successors
                .iter()
                .any(|s| s.reason == TransitionReason::Sequential)
        );
    }

    #[test]
    fn test_dag_group_retry_exhaustion() {
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  setup:
    command: echo setup
  review-pass:
    group: review
  finish:
    command: echo finish

groups:
  review:
    max_retries: 1
    if:
      file-changed: setup
    steps:
      simplify:
        command: echo simplify
      coderabbit:
        command: echo coderabbit
",
        );

        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // There must be a node for the first group step where retry budget is
        // exhausted and the only successor skips the group to `finish`.
        let exhausted: Vec<_> = dag
            .nodes
            .values()
            .filter(|n| {
                n.step_name == "review-pass/simplify"
                    && n.successors.iter().any(|s| {
                        s.reason == TransitionReason::GroupRetryExhausted
                            && s.target
                                .as_ref()
                                .is_some_and(|id| dag.nodes[id].step_name == "finish")
                    })
            })
            .collect();
        assert!(
            !exhausted.is_empty(),
            "expected group retry exhaustion node"
        );
    }

    #[test]
    fn test_dag_runtime_roundtrip() {
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo one
",
        );
        let mut dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        let mut snapshots = std::collections::HashMap::new();
        let mut inner = std::collections::HashMap::new();
        inner.insert(PathBuf::from("foo.txt"), [0u8; 32]);
        snapshots.insert("step1".to_string(), inner);

        dag.nodes[&dag.start].runtime = NodeRuntime {
            prev_output: Some("output".to_string()),
            prev_input: Some("input".to_string()),
            prev_stderr: Some("stderr".to_string()),
            prev_success: Some(true),
            file_snapshots: snapshots,
            visited_at: Some("2026-06-23T00:00:00Z".to_string()),
        };

        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join(DAG_FILE_NAME);
        save_dag(&dag, &path).unwrap_or_else(|e| panic!("{e:?}"));
        let loaded = load_dag(&path).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(loaded, dag);
    }

    #[test]
    fn test_load_dag_missing_file_errors() {
        // Given: a path that does not exist
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join(DAG_FILE_NAME);

        // When: loading it
        let result = load_dag(&path);

        // Then: it errors instead of panicking, so callers can fall back gracefully
        assert!(result.is_err(), "expected Err for a missing DAG file");
    }

    #[test]
    fn test_load_dag_corrupt_file_errors() {
        // Given: a file that exists but is not valid DAG JSON
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join(DAG_FILE_NAME);
        std::fs::write(&path, b"not json").unwrap_or_else(|e| panic!("{e:?}"));

        // When: loading it
        let result = load_dag(&path);

        // Then: it errors instead of panicking, so callers can fall back gracefully
        assert!(result.is_err(), "expected Err for a corrupt DAG file");
    }

    #[test]
    fn test_adopt_runtime_from_copies_matching_node_runtime() {
        // Given: two DAGs built from the same workflow (so node ids line up),
        // one of which ("source") has runtime data recorded on its start node.
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo one
  step2:
    command: echo two
",
        );
        let mut source = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));
        let start_id = source.start.clone();
        source.nodes[&start_id].runtime = NodeRuntime {
            prev_success: Some(true),
            prev_stderr: Some("warning".to_string()),
            ..Default::default()
        };
        let mut target = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // When: target adopts source's runtime data
        target.adopt_runtime_from(&source);

        // Then: the matching node's runtime is copied over
        assert_eq!(target.nodes[&start_id].runtime.prev_success, Some(true));
        assert_eq!(
            target.nodes[&start_id].runtime.prev_stderr,
            Some("warning".to_string())
        );
        // And: the graph structure (successors) is untouched
        assert_eq!(
            target.nodes[&start_id].successors,
            source.nodes[&start_id].successors
        );
    }

    #[test]
    fn test_adopt_runtime_from_skips_node_ids_missing_in_target() {
        // Given: a "source" DAG built from a workflow with an extra step, so
        // it has a node id that does not exist in "target"'s smaller graph.
        let small_compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo one
",
        );
        let large_compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo one
  step2:
    command: echo two
",
        );
        let source = build_dag(&large_compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));
        let mut target = build_dag(&small_compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));
        let target_before = target.clone();

        // When: target (fewer nodes) adopts runtime from source (more nodes)
        target.adopt_runtime_from(&source);

        // Then: nothing panics, and node ids present in target are unaffected
        // beyond what genuinely overlaps by id (here, only the start node id
        // is shared, and source never set any runtime data on it).
        assert_eq!(target, target_before);
    }

    #[test]
    fn test_dag_all_node_ids_unique() {
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  a:
    command: exit 1
    if:
      fail:
        retry: true
  b:
    command: echo b
",
        );
        let dag = build_dag(&compiled, 3).unwrap_or_else(|e| panic!("{e:?}"));

        let ids: HashSet<_> = dag.nodes.keys().cloned().collect();
        assert_eq!(ids.len(), dag.nodes.len());
    }

    // -- step_name_for_node ----------------------------------------------------

    #[test]
    fn test_step_name_for_node_returns_name_for_start_node() {
        // Given: a single-step workflow whose only node is "n0000"
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  my_step:
    command: echo hello
",
        );
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // When: we look up the start node id
        let result = dag.step_name_for_node(&dag.start);

        // Then: we get the step name back
        assert_eq!(result, Some("my_step"));
    }

    #[test]
    fn test_step_name_for_node_returns_none_for_unknown_id() {
        // Given: a DAG built from a simple workflow
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo hello
",
        );
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // When: we look up a node id that does not exist
        let result = dag.step_name_for_node("n9999");

        // Then: we get None
        assert_eq!(result, None);
    }

    #[test]
    fn test_step_name_for_node_works_for_non_start_node() {
        // Given: a two-step linear workflow
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  alpha:
    command: echo a
  beta:
    command: echo b
",
        );
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // When: we follow the start node to its successor and look up the name
        let start = &dag.nodes[&dag.start];
        let second_id = start.successors[0].target.as_ref().unwrap();
        let result = dag.step_name_for_node(second_id);

        // Then: we get "beta"
        assert_eq!(result, Some("beta"));
    }

    // -- first_node_for_step ---------------------------------------------------

    #[test]
    fn test_first_node_for_step_returns_start_for_first_step() {
        // Given: a two-step workflow
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo a
  step2:
    command: echo b
",
        );
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // When: we look up the first step by name
        let node_id = dag.first_node_for_step("step1");

        // Then: we get the DAG start node
        assert_eq!(node_id, Some(&dag.start));
    }

    #[test]
    fn test_first_node_for_step_finds_non_start_step() {
        // Given: a two-step workflow
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo a
  step2:
    command: echo b
",
        );
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // When: we look up the second step by name
        let node_id = dag.first_node_for_step("step2").unwrap();

        // Then: the resolved node has the correct step name
        assert_eq!(dag.nodes[node_id].step_name, "step2");
    }

    #[test]
    fn test_first_node_for_step_returns_none_for_unknown_step() {
        // Given: a DAG for a simple workflow
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: echo hello
",
        );
        let dag = build_dag(&compiled, 10).unwrap_or_else(|e| panic!("{e:?}"));

        // When: we look up a step name that does not exist
        let result = dag.first_node_for_step("nonexistent_step");

        // Then: we get None
        assert!(result.is_none());
    }

    #[test]
    fn test_first_node_for_step_returns_first_occurrence_in_loop() {
        // Given: a retry loop where "step1" appears as multiple DAG nodes
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  step1:
    command: exit 1
    if:
      fail:
        retry: true
  step2:
    command: echo done
",
        );
        let dag = build_dag(&compiled, 3).unwrap_or_else(|e| panic!("{e:?}"));

        // When: we look up step1 (which appears several times)
        let node_id = dag.first_node_for_step("step1").unwrap();

        // Then: we get the very first node (DAG start), not a later retry node
        assert_eq!(node_id, &dag.start);
    }
}
