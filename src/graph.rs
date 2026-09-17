//! Fixed-size workflow topology and its authoritative execution checkpoint.
use crate::config::{FailAction, NoFileChangesAction};
use crate::error::{CruiseError, Result};
use crate::workflow::CompiledWorkflow;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
pub mod persistence;
pub mod validation;
pub type NodeId = String;
pub const CHECKPOINT_VERSION: u32 = 1;

/// One node per compiled step, independent of retry policy and visit count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionGraph {
    #[serde(default)]
    pub version: u32,
    pub start: NodeId,
    pub nodes: IndexMap<NodeId, GraphNode>,
    /// Reverse topology index used by graph clients to render incoming edges
    /// without rescanning every node on each refresh.
    #[serde(default)]
    pub predecessors: IndexMap<NodeId, Vec<NodeId>>,
    pub max_retries: usize,
    #[serde(default)]
    pub group_sites: std::collections::HashSet<String>,
    #[serde(default)]
    pub state: ExecutionState,
}

/// Accepted traversals only. Rejected requests never consume a traversal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EdgeCounter {
    pub traversals: usize,
    pub budgeted_traversals: usize,
}

/// A resolved transition, shared by execution, checkpointing and graph matching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transition {
    pub target: Option<NodeId>,
    pub reason: TransitionReason,
    pub budgeted: bool,
    pub group_retry: Option<String>,
}

/// Runtime state is independent of node identity and retained across resumes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionState {
    pub execution_id: Option<String>,
    pub current: Option<NodeId>,
    pub completed: bool,
    pub pending: Option<Transition>,
    #[serde(with = "edge_counter_records")]
    pub edge_counts: HashMap<(String, String), EdgeCounter>,
    pub group_counts: HashMap<String, usize>,
    pub runtime: NodeRuntime,
}

mod edge_counter_records {
    use super::{Deserialize, EdgeCounter, HashMap, Serialize};
    #[derive(Serialize, Deserialize)]
    struct Record {
        from: String,
        to: String,
        #[serde(flatten)]
        counts: EdgeCounter,
    }
    pub fn serialize<S: serde::Serializer>(
        counts: &HashMap<(String, String), EdgeCounter>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut records: Vec<_> = counts
            .iter()
            .map(|((from, to), counts)| Record {
                from: from.clone(),
                to: to.clone(),
                counts: counts.clone(),
            })
            .collect();
        records.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        records.serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<HashMap<(String, String), EdgeCounter>, D::Error> {
        let records = Vec::<Record>::deserialize(deserializer)?;
        let mut counts = HashMap::new();
        for record in records {
            if record.counts.budgeted_traversals > record.counts.traversals
                || counts
                    .insert((record.from, record.to), record.counts)
                    .is_some()
            {
                return Err(serde::de::Error::custom(
                    "invalid or duplicate edge counter",
                ));
            }
        }
        Ok(counts)
    }
}

/// A single stable node in the execution graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphNode {
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
    /// `if.no-file-changes: retry` re-executes the current step.
    IfNoFileChangesRetry,
    /// `if.no-file-changes: failed` terminates the workflow.
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

impl TransitionReason {
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Sequential => "sequential",
            Self::Next => "next",
            Self::IfFileChanged { .. } | Self::GroupRetry { .. } => "if.file-changed",
            Self::IfNoFileChangesRetry => "if.no-file-changes: retry",
            Self::IfNoFileChangesFail => "if.no-file-changes: failed",
            Self::IfFailGoto { .. } => "if.fail",
            Self::IfFailRetry => "if.fail.retry",
            Self::OptionChoice { .. } => "option",
            Self::GroupRetryExhausted => "group max retries",
            Self::SkipFallback => "skip",
        }
    }

    /// Options sharing a destination intentionally share the same budget.
    #[must_use]
    pub fn matches_execution(&self, reason: &Self) -> bool {
        self == reason
            || matches!(
                (self, reason),
                (Self::OptionChoice { .. }, Self::OptionChoice { .. })
            )
    }
}

/// Execution context stored once in the authoritative state. The same shape is
/// decoded on legacy nodes for display compatibility. New nodes only retain
/// their last-visit diagnostic timestamp.
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

fn build_predecessor_index(nodes: &IndexMap<NodeId, GraphNode>) -> IndexMap<NodeId, Vec<NodeId>> {
    let mut predecessors: IndexMap<NodeId, Vec<NodeId>> =
        nodes.keys().map(|id| (id.clone(), Vec::new())).collect();
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for node in nodes.values() {
        for edge in &node.successors {
            let Some(target) = edge.target.as_ref() else {
                continue;
            };
            if !seen.insert((node.id.as_str(), target.as_str())) {
                continue;
            }
            if let Some(incoming) = predecessors.get_mut(target) {
                incoming.push(node.id.clone());
            }
        }
    }
    predecessors
}

/// Resolve group call sites exactly once, before graph lookup.
///
/// # Errors
/// Returns an error if the target is not a compiled step or group call site.
pub fn resolve_target(compiled: &CompiledWorkflow, target: &str) -> Result<String> {
    let target = compiled
        .invocations
        .get(target)
        .map_or(target, |meta| meta.first_step.as_str());
    if !compiled.steps.contains_key(target) {
        return Err(CruiseError::StepNotFound(target.to_string()));
    }
    Ok(target.to_string())
}

/// Build static topology in declaration order. Never unfold loop histories.
///
/// # Errors
/// Returns an error for an empty workflow or an unresolved transition.
#[expect(
    clippy::too_many_lines,
    reason = "enumerates the existing workflow transition kinds"
)]
pub fn build_graph(compiled: &CompiledWorkflow, max_retries: usize) -> Result<ExecutionGraph> {
    let start = compiled
        .steps
        .first()
        .ok_or_else(|| CruiseError::InvalidStepConfig("workflow has no steps".into()))?
        .0
        .clone();
    let mut nodes = IndexMap::new();
    for (index, (name, step)) in compiled.steps.iter().enumerate() {
        let sequential = compiled
            .steps
            .get_index(index + 1)
            .map(|(name, _)| name.clone());
        let normal = step.next.clone().or_else(|| sequential.clone());
        let mut successors = Vec::new();
        let mut push = |reason: TransitionReason, target: Option<String>| -> Result<()> {
            let target = target
                .map(|target| resolve_target(compiled, &target))
                .transpose()?;
            successors.push(NodeSuccessor { reason, target });
            Ok(())
        };
        if let Some(options) = &step.option {
            if options.is_empty() {
                // An empty option list is a valid no-op option step. The
                // runtime falls through to the ordinary next step, so the
                // fixed graph must expose the same transition kind.
                push(
                    TransitionReason::OptionChoice {
                        selector: String::new(),
                    },
                    normal.clone(),
                )?;
            }
            for item in options {
                push(
                    TransitionReason::OptionChoice {
                        selector: item
                            .selector
                            .clone()
                            .or_else(|| item.text_input.clone())
                            .unwrap_or_default(),
                    },
                    item.next.clone().or_else(|| normal.clone()),
                )?;
            }
        } else {
            push(
                if step.next.is_some() {
                    TransitionReason::Next
                } else {
                    TransitionReason::Sequential
                },
                normal,
            )?;
        }
        // Always represent user-skippable edges; validation filters them using
        // actual skip input, rather than inventing a normal exit for every node.
        push(TransitionReason::SkipFallback, sequential)?;
        if let Some(cond) = &step.if_condition {
            if cond.no_file_changes.is_none()
                && let Some(target) = &cond.file_changed
            {
                let target = resolve_target(compiled, target)?;
                push(
                    TransitionReason::IfFileChanged {
                        target: target.clone(),
                    },
                    Some(target),
                )?;
            }
            match cond.no_file_changes {
                Some(NoFileChangesAction::Retry) => {
                    push(TransitionReason::IfNoFileChangesRetry, Some(name.clone()))?;
                }
                Some(NoFileChangesAction::Failed) => {
                    push(TransitionReason::IfNoFileChangesFail, None)?;
                }
                None => {}
            }
            match &cond.fail {
                Some(FailAction::Goto(target)) => {
                    let target = resolve_target(compiled, target)?;
                    push(
                        TransitionReason::IfFailGoto {
                            target: target.clone(),
                        },
                        Some(target),
                    )?;
                }
                Some(FailAction::Detailed(detail)) if detail.retry => {
                    push(TransitionReason::IfFailRetry, Some(name.clone()))?;
                }
                _ => {}
            }
        }
        if let Some(meta) = compiled
            .step_to_invocation
            .get(name)
            .and_then(|site| compiled.invocations.get(site))
        {
            if meta.group_retry_exhaustion_is_reachable(name) {
                let last = compiled
                    .steps
                    .get_index_of(&meta.last_step)
                    .ok_or_else(|| CruiseError::StepNotFound(meta.last_step.clone()))?;
                push(
                    TransitionReason::GroupRetryExhausted,
                    compiled
                        .steps
                        .get_index(last + 1)
                        .map(|(name, _)| name.clone()),
                )?;
            }
            if meta.last_step == *name
                && let Some(target) = meta
                    .if_condition
                    .as_ref()
                    .and_then(|cond| cond.file_changed.as_ref())
            {
                let target = resolve_target(compiled, target)?;
                push(
                    TransitionReason::GroupRetry {
                        target: target.clone(),
                    },
                    Some(target),
                )?;
            }
        }
        nodes.insert(
            name.clone(),
            GraphNode {
                id: name.clone(),
                step_name: name.clone(),
                successors,
                runtime: NodeRuntime::default(),
            },
        );
    }
    let predecessors = build_predecessor_index(&nodes);
    Ok(ExecutionGraph {
        version: CHECKPOINT_VERSION,
        start,
        nodes,
        predecessors,
        max_retries,
        group_sites: compiled.invocations.keys().cloned().collect(),
        state: ExecutionState::default(),
    })
}

impl ExecutionGraph {
    /// Rebuild derived topology indexes after decoding a checkpoint written by
    /// an earlier graph version that did not persist them.
    pub(crate) fn rebuild_indexes(&mut self) {
        self.predecessors = build_predecessor_index(&self.nodes);
    }

    #[must_use]
    pub fn step_name_for_node(&self, id: &str) -> Option<&str> {
        self.nodes.get(id).map(|node| node.step_name.as_str())
    }
    #[must_use]
    pub fn first_node_for_step(&self, name: &str) -> Option<&NodeId> {
        if self.version == CHECKPOINT_VERSION {
            self.nodes.get_key_value(name).map(|(id, _)| id)
        } else {
            self.nodes
                .iter()
                .find(|(_, node)| node.step_name == name)
                .map(|(id, _)| id)
        }
    }
    /// Carry surviving stable identities forward, including their accepted counts.
    pub fn adopt_runtime_from(&mut self, other: &Self) {
        for (id, node) in &mut self.nodes {
            if let Some(previous) = other.nodes.get(id) {
                node.runtime = previous.runtime.clone();
            }
        }
        self.state = other.state.clone();
        let pairs = self.edge_pairs();
        self.state
            .edge_counts
            .retain(|pair, _| pairs.contains(pair));
        self.state
            .group_counts
            .retain(|site, _| self.group_sites.contains(site));
        self.state.runtime.file_snapshots.retain(|key, _| {
            self.nodes.contains_key(key)
                || key
                    .strip_prefix("__nfc__")
                    .is_some_and(|step| self.nodes.contains_key(step))
                || key
                    .strip_prefix("__group__")
                    .is_some_and(|site| self.group_sites.contains(site))
        });
    }
    pub(crate) fn edge_pairs(&self) -> std::collections::HashSet<(String, String)> {
        self.nodes
            .values()
            .flat_map(|node| {
                node.successors
                    .iter()
                    .filter_map(|edge| edge.target.as_ref().map(|to| (node.id.clone(), to.clone())))
            })
            .collect()
    }

    pub(crate) fn valid_pending_transition(&self, current: &str, pending: &Transition) -> bool {
        let Some(node) = self.nodes.get(current) else {
            return false;
        };
        let expected_budgeted = !matches!(
            pending.reason,
            TransitionReason::SkipFallback | TransitionReason::GroupRetryExhausted
        );
        let is_group_retry = matches!(pending.reason, TransitionReason::GroupRetry { .. });
        pending.budgeted == expected_budgeted
            && pending.group_retry.is_some() == is_group_retry
            && pending
                .group_retry
                .as_ref()
                .is_none_or(|site| self.group_sites.contains(site))
            && node.successors.iter().any(|edge| {
                edge.target == pending.target && edge.reason.matches_execution(&pending.reason)
            })
    }

    fn control_structure_matches(&self, other: &Self) -> bool {
        self.start == other.start
            && self.group_sites == other.group_sites
            && self.nodes.len() == other.nodes.len()
            && self.nodes.iter().all(|(id, node)| {
                other.nodes.get(id).is_some_and(|previous| {
                    node.id == previous.id
                        && node.step_name == previous.step_name
                        && node.successors == previous.successors
                })
            })
    }

    fn validate_restore_position(&self, other: &Self) -> Result<()> {
        if let Some(current) = &other.state.current {
            self.nodes.get(current).ok_or_else(|| {
                CruiseError::Other(format!(
                    "checkpoint current step '{current}' no longer exists; explicitly restart or choose a resume position"
                ))
            })?;
            if let Some(pending) = &other.state.pending
                && !self.valid_pending_transition(current, pending)
            {
                return Err(CruiseError::Other(
                    "checkpoint pending transition no longer exists".into(),
                ));
            }
        }
        Ok(())
    }

    /// Restore runtime state after a config hot reload.
    ///
    /// A hot reload may add or remove future control-flow nodes, but it cannot
    /// remove the current node or a pending transition that must be resumed.
    /// Counts for transitions that still exist are retained; obsolete pairs are
    /// discarded by [`Self::adopt_runtime_from`].
    pub(crate) fn restore_for_reload(&mut self, other: &Self) -> Result<()> {
        if other.version != CHECKPOINT_VERSION {
            return Err(CruiseError::Other("legacy graph has no reliable traversal counters; explicitly restart this session from the beginning".into()));
        }
        self.validate_restore_position(other)?;
        self.adopt_runtime_from(other);
        Ok(())
    }

    /// Reject incomplete or incompatible checkpoints instead of guessing counts.
    ///
    /// # Errors
    /// Legacy formats, removed current nodes and removed pending edges cannot resume.
    pub fn restore(&mut self, other: &Self) -> Result<()> {
        if other.version != CHECKPOINT_VERSION {
            return Err(CruiseError::Other("legacy graph has no reliable traversal counters; explicitly restart this session from the beginning".into()));
        }
        let has_execution_state = other.state.current.is_some()
            || other.state.completed
            || other.state.pending.is_some()
            || !other.state.edge_counts.is_empty()
            || !other.state.group_counts.is_empty()
            || other.state.execution_id.is_some();
        if has_execution_state && !self.control_structure_matches(other) {
            return Err(CruiseError::Other(
                "checkpoint control structure no longer matches the workflow; explicitly restart or continue with the existing workflow".into(),
            ));
        }
        self.validate_restore_position(other)?;
        self.adopt_runtime_from(other);
        Ok(())
    }
}
