//! Conservative normal-exit analysis over the same topology used by execution.
use super::{ExecutionGraph, TransitionReason, build_graph};
use crate::config::SkipCondition;
use crate::error::{CruiseError, Result};
use crate::workflow::CompiledWorkflow;
use std::collections::HashSet;

/// Validate only the reachable execution region. Dynamic conditions are possibilities,
/// not predictions. An error exit (including loop protection) is not a normal exit.
///
/// # Errors
/// Rejects a reachable cycle only when no normal exit is reachable from `entry`.
#[expect(
    clippy::too_many_lines,
    reason = "iterative reachability and SCC analysis share indexed topology"
)]
pub fn validate_execution_graph(
    graph: &ExecutionGraph,
    compiled: &CompiledWorkflow,
    entry: &str,
    skipped_steps: &[String],
    phase: &str,
) -> Result<()> {
    let start = graph
        .nodes
        .get_index_of(entry)
        .ok_or_else(|| CruiseError::StepNotFound(entry.into()))?;
    let n = graph.nodes.len();
    let skipped: HashSet<&str> = skipped_steps.iter().map(String::as_str).collect();
    let mut edges = vec![Vec::new(); n];
    let mut reverse = vec![Vec::new(); n];
    let mut exits = Vec::new();
    for (index, node) in graph.nodes.values().enumerate() {
        let step = compiled
            .steps
            .get(&node.step_name)
            .ok_or_else(|| CruiseError::StepNotFound(node.step_name.clone()))?;
        let always_skip = skipped.contains(node.step_name.as_str())
            || matches!(step.skip, Some(SkipCondition::Static(true)));
        let may_skip = always_skip
            || matches!(step.skip, Some(SkipCondition::Variable(_)))
            || step.when.is_some();
        let group_meta = compiled
            .step_to_invocation
            .get(&node.step_name)
            .and_then(|site| compiled.invocations.get(site));
        let group_zero = group_meta
            .is_some_and(|meta| meta.first_step == node.step_name && meta.max_retries == Some(0));
        let group_exhaustion_is_reachable = group_meta
            .is_some_and(|meta| meta.group_retry_exhaustion_is_reachable(&node.step_name));
        for edge in &node.successors {
            if edge.reason == TransitionReason::GroupRetryExhausted
                && !group_exhaustion_is_reachable
            {
                continue;
            }
            if group_zero && edge.reason != TransitionReason::GroupRetryExhausted {
                continue;
            }
            if !group_zero
                && always_skip
                && !matches!(
                    edge.reason,
                    TransitionReason::SkipFallback | TransitionReason::GroupRetryExhausted
                )
            {
                continue;
            }
            if edge.reason == TransitionReason::SkipFallback && !may_skip {
                continue;
            }
            if edge.reason == TransitionReason::IfNoFileChangesFail {
                continue;
            }
            if let Some(target) = &edge.target {
                let target = graph
                    .nodes
                    .get_index_of(target)
                    .ok_or_else(|| CruiseError::StepNotFound(target.clone()))?;
                edges[index].push(target);
                reverse[target].push(index);
            } else {
                exits.push(index);
            }
        }
    }
    let reachable = reachable_from(&edges, [start]);
    let can_exit = reachable_from(&reverse, exits);
    // Iterative Kosaraju: no recursion or all-pairs reachability matrix.
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for root in 0..n {
        if visited[root] || !reachable[root] {
            continue;
        }
        visited[root] = true;
        let mut stack = vec![(root, 0)];
        while let Some((node, next)) = stack.last_mut() {
            if let Some(&child) = edges[*node].get(*next) {
                *next += 1;
                if !visited[child] {
                    visited[child] = true;
                    stack.push((child, 0));
                }
            } else {
                order.push(*node);
                stack.pop();
            }
        }
    }
    let mut assigned = vec![false; n];
    let mut dangerous = Vec::new();
    for root in order.into_iter().rev() {
        if assigned[root] {
            continue;
        }
        assigned[root] = true;
        let mut stack = vec![root];
        let mut members = Vec::new();
        while let Some(node) = stack.pop() {
            members.push(node);
            for &previous in &reverse[node] {
                if reachable[previous] && !assigned[previous] {
                    assigned[previous] = true;
                    stack.push(previous);
                }
            }
        }
        if !can_exit[root] && (members.len() > 1 || edges[root].contains(&root)) {
            dangerous.extend(members);
        }
    }
    if dangerous.is_empty() {
        return Ok(());
    }
    let names = dangerous
        .iter()
        .map(|&i| {
            let node = &graph.nodes[i];
            let reasons = node
                .successors
                .iter()
                .filter(|edge| edge.reason != TransitionReason::SkipFallback)
                .map(|edge| format!("{:?}", edge.reason))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} ({reasons})", node.step_name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    if !can_exit[start] {
        return Err(CruiseError::InvalidStepConfig(format!(
            "{phase} execution from '{entry}' has no normal exit and reaches a cycle: {names}"
        )));
    }
    crate::status_eprintln!(
        "warning: {phase} execution may enter a cycle with no normal exit: {names}; runtime loop protection remains enabled"
    );
    Ok(())
}

fn reachable_from(edges: &[Vec<usize>], seeds: impl IntoIterator<Item = usize>) -> Vec<bool> {
    let mut seen = vec![false; edges.len()];
    let mut stack = Vec::new();
    for seed in seeds {
        if !seen[seed] {
            seen[seed] = true;
            stack.push(seed);
        }
    }
    while let Some(node) = stack.pop() {
        for &target in &edges[node] {
            if !seen[target] {
                seen[target] = true;
                stack.push(target);
            }
        }
    }
    seen
}

/// Validate main and after-pr before creating workspaces or executing commands.
///
/// # Errors
/// Propagates graph construction and definite nontermination diagnostics.
pub fn validate_workflow(
    compiled: &CompiledWorkflow,
    graph: &ExecutionGraph,
    entry: &str,
    skipped: &[String],
) -> Result<()> {
    validate_execution_graph(graph, compiled, entry, skipped, "main")?;
    if !compiled.after_pr.is_empty() {
        let after = compiled.to_after_pr_compiled();
        let graph = build_graph(&after, graph.max_retries)?;
        validate_execution_graph(&graph, &after, &graph.start, skipped, "after-pr")?;
    }
    Ok(())
}
