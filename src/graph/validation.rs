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
pub fn validate_execution_graph(
    graph: &ExecutionGraph,
    compiled: &CompiledWorkflow,
    entry: &str,
    skipped_steps: &[String],
    phase: &str,
) -> Result<()> {
    let analysis = analyze_phase(graph, compiled, entry, skipped_steps, phase)?;
    if let Some(message) = analysis.cycle_without_exit {
        crate::status_eprintln!("warning: {message}");
    }
    Ok(())
}

/// Static analysis of one execution phase.
#[derive(Debug)]
pub struct PhaseAnalysis {
    pub phase: String,
    pub unreachable_steps: Vec<String>,
    pub cycle_without_exit: Option<String>,
}

struct LiveEdges {
    start: usize,
    edges: Vec<Vec<usize>>,
    reverse: Vec<Vec<usize>>,
    exits: Vec<usize>,
    reachable: Vec<bool>,
}

fn live_edges(
    graph: &ExecutionGraph,
    compiled: &CompiledWorkflow,
    entry: &str,
    skipped_steps: &[String],
) -> Result<LiveEdges> {
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
    Ok(LiveEdges {
        start,
        edges,
        reverse,
        exits,
        reachable,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "iterative SCC analysis shares indexed topology"
)]
pub(crate) fn analyze_phase(
    graph: &ExecutionGraph,
    compiled: &CompiledWorkflow,
    entry: &str,
    skipped_steps: &[String],
    phase: &str,
) -> Result<PhaseAnalysis> {
    let LiveEdges {
        start,
        edges,
        reverse,
        exits,
        reachable,
    } = live_edges(graph, compiled, entry, skipped_steps)?;
    let n = graph.nodes.len();
    let unreachable_steps = graph
        .nodes
        .values()
        .enumerate()
        .filter(|(i, _)| !reachable[*i])
        .map(|(_, node)| node.step_name.clone())
        .collect();
    let mut analysis = PhaseAnalysis {
        phase: phase.into(),
        unreachable_steps,
        cycle_without_exit: None,
    };
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
        return Ok(analysis);
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
    analysis.cycle_without_exit = Some(format!(
        "{phase} execution may enter a cycle with no normal exit: {names}; runtime loop protection remains enabled"
    ));
    Ok(analysis)
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

/// Analyze main and after-pr phases without emitting warnings.
pub fn analyze_workflow(
    compiled: &CompiledWorkflow,
    graph: &ExecutionGraph,
    entry: &str,
    skipped: &[String],
) -> Result<Vec<PhaseAnalysis>> {
    let mut phases = vec![analyze_phase(graph, compiled, entry, skipped, "main")?];
    if !compiled.after_pr.is_empty() {
        let after = compiled.to_after_pr_compiled();
        let after_graph = build_graph(&after, graph.max_retries)?;
        phases.push(analyze_phase(
            &after_graph,
            &after,
            &after_graph.start,
            skipped,
            "after-pr",
        )?);
    }
    Ok(phases)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    fn analyze(yaml: &str, skipped: &[String]) -> Result<Vec<PhaseAnalysis>> {
        let config = WorkflowConfig::from_yaml(yaml)
            .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
        let compiled = crate::workflow::compile(config)?;
        let graph = build_graph(&compiled, 3)?;
        analyze_workflow(&compiled, &graph, &graph.start, skipped)
    }

    #[test]
    fn unreachable_respects_skipped_steps() {
        let yaml = "command: [echo]\nsteps:\n  a:\n    command: echo a\n    next: c\n  b:\n    command: echo b\n  c:\n    command: echo c\n";
        let normal = analyze(yaml, &[]).unwrap();
        assert_eq!(normal[0].phase, "main");
        assert_eq!(normal[0].unreachable_steps, ["b"]);
        let skipped = analyze(yaml, &["a".into()]).unwrap();
        assert!(skipped[0].unreachable_steps.is_empty());
    }

    #[test]
    fn reports_definite_nontermination() {
        let yaml = "command: [echo]\nsteps:\n  a:\n    command: echo a\n    next: b\n  b:\n    command: echo b\n    next: a\n";
        let error = analyze(yaml, &[]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has no normal exit and reaches a cycle")
        );
    }

    #[test]
    fn analyzes_after_pr_independently() {
        let yaml = "command: [echo]\nsteps:\n  a:\n    command: echo a\nafter-pr:\n  x:\n    command: echo x\n    next: z\n  y:\n    command: echo y\n  z:\n    command: echo z\n";
        let phases = analyze(yaml, &[]).unwrap();
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[1].phase, "after-pr");
        assert_eq!(phases[1].unreachable_steps, ["y"]);
    }
}
