#![expect(clippy::unwrap_used, reason = "isolated graph contract fixtures")]

use cruise::config::WorkflowConfig;
use cruise::flags::{FlagValues, enabled_graph};
use cruise::graph::persistence::{load_graph, save_graph};
use cruise::graph::validation::validate_execution_graph;
use cruise::graph::{EdgeCounter, ExecutionGraph, TransitionReason, build_graph};
use cruise::workflow::{CompiledWorkflow, compile};
use serde_json::json;

fn fixture(steps: &str) -> (CompiledWorkflow, ExecutionGraph) {
    let config = WorkflowConfig::from_yaml(&format!("command: [echo]\n{steps}")).unwrap();
    let compiled = compile(config).unwrap();
    let graph = build_graph(&compiled, 3).unwrap();
    (compiled, graph)
}

#[test]
fn quality_gate_declares_one_self_edge_and_keeps_normal_exits() {
    for tail in ["", "  finish: {command: 'true'}\n"] {
        let (compiled, graph) = fixture(&format!(
            "steps:\n  implement:\n    prompt: implement\n    quality_gates:\n      - command: 'true'\n{tail}"
        ));
        let successors = &graph.nodes["implement"].successors;
        let gate_edges: Vec<_> = successors
            .iter()
            .filter(|edge| serde_json::to_value(&edge.reason).unwrap() == json!("QualityGateRetry"))
            .collect();
        assert_eq!(gate_edges.len(), 1);
        assert_eq!(gate_edges[0].target.as_deref(), Some("implement"));
        assert_eq!(gate_edges[0].reason.label(), "quality gate");
        assert!(
            successors
                .iter()
                .any(|edge| edge.reason == TransitionReason::Sequential
                    && edge.target.as_deref()
                        == if tail.is_empty() {
                            None
                        } else {
                            Some("finish")
                        })
        );
        if !tail.is_empty() {
            assert!(graph.nodes["finish"].successors.iter().all(|edge| {
                serde_json::to_value(&edge.reason).unwrap() != json!("QualityGateRetry")
            }));
        }
        validate_execution_graph(&graph, &compiled, "implement", &[], "workflow").unwrap();
    }
}

#[test]
fn ungated_graph_successors_keep_legacy_serialization() {
    let (_, graph) =
        fixture("steps:\n  implement: {prompt: implement}\n  finish: {command: 'true'}\n");
    assert_eq!(
        serde_json::to_value(&graph.nodes["implement"].successors).unwrap(),
        json!([
            {"reason":"Sequential", "target":"finish"},
            {"reason":"SkipFallback", "target":"finish"}
        ])
    );
    assert_eq!(
        serde_json::to_value(&graph.nodes["finish"].successors).unwrap(),
        json!([
            {"reason":"Sequential", "target":null},
            {"reason":"SkipFallback", "target":null}
        ])
    );
}

#[test]
fn disabled_gate_node_is_removed_without_dangling_edges() {
    let (compiled, graph) = fixture(
        "flags:\n  gate: {}\nsteps:\n  first: {command: 'true'}\n  implement:\n    flag: gate\n    prompt: implement\n    quality_gates:\n      - command: 'true'\n  finish: {command: 'true'}\n",
    );
    let projected = enabled_graph(&graph, &compiled, &FlagValues::new());
    assert!(!projected.nodes.contains_key("implement"));
    assert!(
        projected
            .nodes
            .values()
            .flat_map(|node| &node.successors)
            .all(|edge| edge
                .target
                .as_ref()
                .is_none_or(|target| projected.nodes.contains_key(target)))
    );
    assert!(
        projected.nodes["first"]
            .successors
            .iter()
            .any(|edge| edge.reason == TransitionReason::Sequential
                && edge.target.as_deref() == Some("finish"))
    );
    assert_eq!(
        enabled_graph(
            &graph,
            &compiled,
            &FlagValues::from([("gate".into(), true)])
        ),
        graph
    );
}

#[test]
fn checkpoint_roundtrips_gate_feedback_pending_retry_and_budget() {
    let (_, mut graph) = fixture(
        "steps:\n  implement:\n    prompt: implement\n    quality_gates:\n      - command: 'true'\n",
    );
    let mut value = serde_json::to_value(&graph).unwrap();
    value["state"]["current"] = json!("implement");
    value["state"]["pending"] = json!({"target":"implement", "reason":"QualityGateRetry", "budgeted":true, "group_retry":null});
    value["state"]["runtime"]["quality_gate_feedback"] = json!("X");
    graph = serde_json::from_value(value).unwrap();
    graph.state.edge_counts.insert(
        ("implement".into(), "implement".into()),
        EdgeCounter {
            traversals: 2,
            budgeted_traversals: 2,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dag.json");
    save_graph(&graph, &path).unwrap();
    let saved = load_graph(&path).unwrap();
    let (_, mut resumed) = fixture(
        "steps:\n  implement:\n    prompt: implement\n    quality_gates:\n      - command: 'true'\n",
    );
    resumed.restore(&saved).unwrap();
    let state = serde_json::to_value(&resumed.state).unwrap();
    assert_eq!(state["runtime"]["quality_gate_feedback"], "X");
    assert_eq!(state["pending"]["reason"], "QualityGateRetry");
    assert_eq!(state["pending"]["budgeted"], true);
    assert_eq!(
        resumed.state.edge_counts[&("implement".into(), "implement".into())].budgeted_traversals,
        2
    );
}

#[test]
fn legacy_checkpoint_without_gate_feedback_still_loads() {
    let (_, graph) = fixture("steps:\n  implement: {prompt: implement}\n");
    let mut legacy = serde_json::to_value(&graph).unwrap();
    legacy["state"]["runtime"]
        .as_object_mut()
        .unwrap()
        .remove("quality_gate_feedback");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dag.json");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let loaded = load_graph(&path).unwrap();
    assert_eq!(loaded.version, 1);
    assert!(
        serde_json::to_value(&loaded.state.runtime)
            .unwrap()
            .get("quality_gate_feedback")
            .is_none_or(serde_json::Value::is_null)
    );
}
