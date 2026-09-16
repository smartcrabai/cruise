//! Compatibility names for persisted `dag.json` and existing library clients.
//! Execution uses the fixed topology in [`crate::graph`], never DAG expansion.
pub use crate::graph::build_graph as build_dag;
pub use crate::graph::persistence::{
    DAG_FILE_NAME, load_graph as load_dag, save_graph as save_dag,
};
pub use crate::graph::{
    ExecutionGraph as ExecutionDag, GraphNode as DagNode, HashMapSnapshot, NodeId, NodeRuntime,
    NodeSuccessor, TransitionReason,
};

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use crate::workflow::{CompiledWorkflow, compile};
    use std::collections::HashSet;
    use std::path::PathBuf;

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
        assert_eq!(dag.start, "step1");
        assert_eq!(dag.nodes[&dag.start].step_name, "step1");

        let first = &dag.nodes[&dag.start];
        assert_eq!(first.successors.len(), 2);
        assert_eq!(first.successors[0].reason, TransitionReason::Sequential);
        let second_id = first.successors[0].target.as_ref().unwrap();
        assert_eq!(dag.nodes[second_id].step_name, "step2");

        let second = &dag.nodes[second_id];
        assert_eq!(second.successors.len(), 2);
        assert_eq!(second.successors[0].target, None);
        assert_eq!(second.successors[0].reason, TransitionReason::Sequential);
    }

    #[test]
    fn test_dag_topology_is_independent_of_retry_ceiling() {
        // Given: a retrying step whose compiled topology is fixed regardless of
        // how many times the runtime is allowed to traverse its self edge.
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  implement:
    command: echo implement
    if:
      fail:
        retry: true
  finish:
    command: echo finish
",
        );

        // When: the same workflow is compiled with small, ordinary, and very
        // large retry ceilings.
        let small = build_dag(&compiled, 1).unwrap_or_else(|e| panic!("{e:?}"));
        let ordinary = build_dag(&compiled, 3).unwrap_or_else(|e| panic!("{e:?}"));
        let large = build_dag(&compiled, 1_000_000).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: retry policy changes runtime decisions, not the number or
        // identity of compiled graph nodes.
        for dag in [&small, &ordinary, &large] {
            assert_eq!(
                dag.nodes.len(),
                compiled.steps.len(),
                "one graph node is required for each compiled step"
            );
            for step_name in compiled.steps.keys() {
                let matching: Vec<_> = dag
                    .nodes
                    .values()
                    .filter(|node| node.step_name == *step_name)
                    .collect();
                assert_eq!(
                    matching.len(),
                    1,
                    "step {step_name:?} must have one stable graph node"
                );
            }
        }

        let small_edge_count: usize = small.nodes.values().map(|node| node.successors.len()).sum();
        let ordinary_edge_count: usize = ordinary
            .nodes
            .values()
            .map(|node| node.successors.len())
            .sum();
        let large_edge_count: usize = large.nodes.values().map(|node| node.successors.len()).sum();
        assert_eq!(small_edge_count, ordinary_edge_count);
        assert_eq!(ordinary_edge_count, large_edge_count);

        let small_ids: HashSet<_> = small
            .nodes
            .values()
            .map(|node| (node.step_name.as_str(), node.id.as_str()))
            .collect();
        let large_ids: HashSet<_> = large
            .nodes
            .values()
            .map(|node| (node.step_name.as_str(), node.id.as_str()))
            .collect();
        assert_eq!(
            small_ids, large_ids,
            "stable node identity must survive a retry-ceiling change"
        );
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
        assert_eq!(a.successors.len(), 2);
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

        // Plan 3.1 and 3.2: topology never encodes exhaustion as success.
        let b = &dag.nodes["b"];
        assert!(b.successors.iter().any(|edge| matches!(&edge.reason, TransitionReason::IfFileChanged { target } if target == "c") && edge.target.as_deref() == Some("c")));

        assert!(b.successors.iter().any(|s| {
            s.reason == TransitionReason::SkipFallback
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "c")
        }));
    }

    #[test]
    fn test_dag_next_and_skip_share_one_target() {
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

        // Normal and skip reasons have the same target and one shared counter key.
        assert_eq!(a.step_name, "a");
        assert_eq!(a.successors.len(), 2);
        assert_eq!(a.successors[0].reason, TransitionReason::Next);
        assert_eq!(
            dag.nodes[a.successors[0].target.as_ref().unwrap()].step_name,
            "b"
        );
    }

    #[test]
    fn test_dag_last_step_with_next_has_terminal_skip_fallback() {
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

        // Plan 3.3: skipping the final step ends instead of following explicit next.
        assert_eq!(b.step_name, "b");
        assert_eq!(b.successors.len(), 2);
        assert!(
            b.successors
                .iter()
                .any(|edge| edge.reason == TransitionReason::SkipFallback && edge.target.is_none())
        );
        assert_eq!(b.successors[0].reason, TransitionReason::Next);
        assert_eq!(
            dag.nodes[b.successors[0].target.as_ref().unwrap()].step_name,
            "a"
        );
    }

    #[test]
    fn test_dag_if_fail_retry_remains_a_self_edge() {
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

        // Plan 3.1: a retry remains a real self edge at every ceiling.
        assert_eq!(dag.nodes.len(), compiled.steps.len());
        let retry = &dag.nodes["step1"];
        assert!(
            retry
                .successors
                .iter()
                .any(|edge| edge.reason == TransitionReason::IfFailRetry
                    && edge.target.as_deref() == Some("step1"))
        );
        assert!(
            !retry
                .successors
                .iter()
                .any(|edge| edge.reason == TransitionReason::IfFailRetry && edge.target.is_none())
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
    fn test_dag_no_file_changes_failed_is_terminal() {
        // Given: a step with `if.no-file-changes: failed`.
        // When: the execution DAG is built, the no-change branch is a terminal
        // edge (workflow aborts), alongside the normal sequential exit.
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  implement:
    command: echo implement
    if:
      no-file-changes: failed
  done:
    command: echo done
",
        );

        let dag = build_dag(&compiled, 2).unwrap_or_else(|e| panic!("{e:?}"));

        let implement = &dag.nodes[&dag.start];
        assert_eq!(implement.step_name, "implement");
        assert_eq!(implement.successors.len(), 3);
        assert!(
            implement.successors.iter().any(|s| {
                s.reason == TransitionReason::IfNoFileChangesFail && s.target.is_none()
            })
        );
        assert!(implement.successors.iter().any(|s| {
            s.reason == TransitionReason::Sequential
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "done")
        }));
    }

    #[test]
    fn test_dag_no_file_changes_retry_reuses_the_same_node() {
        // Given: a step with `if.no-file-changes: retry`.
        // When: the execution DAG is built, the no-change branch is a self-edge
        // back to the same step; once the self-edge budget (max_retries) is
        // exhausted, the retry transition becomes terminal (loop protection).
        let compiled = compile_yaml(
            r"
command: [echo]
steps:
  work:
    command: echo work
    if:
      no-file-changes: retry
  done:
    command: echo done
",
        );

        let dag = build_dag(&compiled, 1).unwrap_or_else(|e| panic!("{e:?}"));

        let work = &dag.nodes[&dag.start];
        assert_eq!(work.step_name, "work");
        assert_eq!(work.successors.len(), 3);

        // First visit: the self-edge is within budget and loops back to `work`.
        assert!(work.successors.iter().any(|s| {
            s.reason == TransitionReason::IfNoFileChangesRetry
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "work")
        }));
        assert!(work.successors.iter().any(|s| {
            s.reason == TransitionReason::Sequential
                && s.target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "done")
        }));

        // Repeated visits reuse this exact node; exhaustion is a runtime error.
        assert_eq!(dag.nodes.len(), 2);
        let retry = work
            .successors
            .iter()
            .find(|edge| edge.reason == TransitionReason::IfNoFileChangesRetry)
            .unwrap();
        assert_eq!(retry.target.as_ref(), Some(&work.id));
        assert!(
            !work
                .successors
                .iter()
                .any(|edge| edge.reason == TransitionReason::IfNoFileChangesRetry
                    && edge.target.is_none())
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
    fn test_dag_includes_group_retry_edge_without_group_max_retries() {
        // Given: a group whose retry condition is present but whose group-level
        // max_retries is intentionally omitted. The global runtime protection
        // still needs a static edge to evaluate.
        let compiled = compile_yaml(
            r"
command: [echo]
groups:
  review:
    if:
      file-changed: prepare
    steps:
      inspect:
        command: echo inspect
steps:
  prepare:
    command: echo prepare
  review-pass:
    group: review
  finish:
    command: echo finish
",
        );

        // When: the graph is built.
        let dag = build_dag(&compiled, 3).unwrap_or_else(|e| panic!("{e:?}"));
        let last_group_step = dag
            .nodes
            .values()
            .find(|node| node.step_name == "review-pass/inspect")
            .unwrap_or_else(|| panic!("missing expanded group step"));

        // Then: the possible group retry is represented even without a group
        // retry ceiling, so the runtime can apply the global edge budget.
        assert!(
            last_group_step.successors.iter().any(|successor| {
                matches!(
                    successor.reason,
                    TransitionReason::GroupRetry { ref target } if target == "prepare"
                ) && successor
                    .target
                    .as_ref()
                    .is_some_and(|id| dag.nodes[id].step_name == "prepare")
            }),
            "an unbounded group retry must retain its retry edge"
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
