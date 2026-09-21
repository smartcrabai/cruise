pub(crate) fn build_mermaid_source(dag: &crate::webui::dto::DagDto) -> String {
    let mut lines = vec!["graph TD".to_string()];
    let node_ids: Vec<(&str, String)> = dag
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            (
                step.name.as_str(),
                format!("s{index}_{}", sanitize_node_id(&step.name)),
            )
        })
        .collect();
    for step in &dag.steps {
        if let Some((_, node_id)) = node_ids.iter().rev().find(|(name, _)| *name == step.name) {
            lines.push(format!(
                "  {node_id}[\"{}\"]",
                escape_mermaid_label(&step.name)
            ));
        }
    }
    for edge in &dag.edges {
        let Some((_, from)) = node_ids.iter().rev().find(|(name, _)| *name == edge.from) else {
            continue;
        };
        let Some(to_name) = edge.to.as_deref() else {
            lines.push(if edge.reason == "ifNoFileChangesFail" {
                format!("  {from} --> error_terminal[/ERROR/]")
            } else {
                format!("  {from} --> end_terminal[/END/]")
            });
            continue;
        };
        let Some((_, to)) = node_ids.iter().rev().find(|(name, _)| *name == to_name) else {
            continue;
        };
        let reason_label = edge_label(&edge.reason, edge.selector.as_deref());
        let count_label = (edge.traversals > 0).then(|| {
            format!(
                "{} traversals, {} budgeted",
                edge.traversals, edge.budgeted_traversals
            )
        });
        let label = match (reason_label, count_label) {
            (Some(reason), Some(count)) => Some(format!("{reason} · {count}")),
            (Some(reason), None) => Some(reason),
            (None, Some(count)) => Some(count),
            (None, None) => None,
        };
        if let Some(label) = label {
            lines.push(format!(
                "  {from} -->|\"{}\"| {to}",
                escape_mermaid_label(&label)
            ));
        } else {
            lines.push(format!("  {from} --> {to}"));
        }
    }
    if let Some((_, start)) = node_ids
        .iter()
        .rev()
        .find(|(name, _)| *name == dag.start_step)
    {
        lines.push(format!(
            "  style {start} fill:#10b981,color:#fff,stroke:#059669,stroke-width:2px"
        ));
    }
    if let Some(current_step) = dag.current_step.as_deref()
        && let Some((_, current)) = node_ids
            .iter()
            .rev()
            .find(|(name, _)| *name == current_step)
    {
        lines.push(format!(
            "  style {current} fill:#3b82f6,color:#fff,stroke:#2563eb,stroke-width:2px"
        ));
    }
    lines.join("\n")
}

fn sanitize_node_id(name: &str) -> String {
    // JavaScript's regex has no Unicode flag, so astral characters are two
    // replacement units rather than one replacement character.
    name.encode_utf16()
        .map(|unit| {
            let byte = u8::try_from(unit).unwrap_or(b'-');
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect()
}

const MERMAID_LABEL_ESCAPES: &[(char, &str)] = &[
    ('\\', "#92;"),
    ('\n', "#10;"),
    ('\r', "#10;"),
    ('#', "#35;"),
    (';', "#59;"),
    ('`', "#96;"),
    ('[', "#91;"),
    (']', "#93;"),
    ('(', "#40;"),
    (')', "#41;"),
    ('{', "#123;"),
    ('}', "#125;"),
    ('&', "#amp;"),
    ('"', "#quot;"),
    ('<', "#lt;"),
    ('>', "#gt;"),
    ('|', "#124;"),
];

fn escape_mermaid_label(label: &str) -> String {
    let mut escaped = String::with_capacity(label.len());
    for ch in label.chars() {
        if let Some((_, replacement)) = MERMAID_LABEL_ESCAPES
            .iter()
            .find(|(escaped_char, _)| *escaped_char == ch)
        {
            escaped.push_str(replacement);
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

fn edge_label(reason: &str, selector: Option<&str>) -> Option<String> {
    if let Some(selector) = selector.filter(|selector| !selector.is_empty()) {
        let normalized = reason.to_lowercase();
        if normalized == "sequential" || normalized == "next" {
            None
        } else {
            Some(format!("{reason}: {selector}"))
        }
    } else {
        let normalized = reason.to_lowercase();
        if normalized == "sequential" || normalized == "next" {
            None
        } else {
            Some(reason.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webui::dto::{DagDto, DagEdgeDto, DagStepDto};

    /// `(from, to, reason, selector, traversals, budgeted_traversals)`
    type EdgeSpec<'a> = (
        &'a str,
        Option<&'a str>,
        &'a str,
        Option<&'a str>,
        usize,
        usize,
    );

    fn dag(
        start_step: &str,
        current_step: Option<&str>,
        steps: &[&str],
        edges: &[EdgeSpec<'_>],
    ) -> DagDto {
        DagDto {
            start_step: start_step.to_string(),
            current_step: current_step.map(str::to_owned),
            steps: steps
                .iter()
                .map(|name| DagStepDto {
                    name: (*name).to_string(),
                    kind: "command".to_string(),
                    is_terminal: false,
                })
                .collect(),
            edges: edges
                .iter()
                .map(
                    |(from, to, reason, selector, traversals, budgeted_traversals)| DagEdgeDto {
                        from: (*from).to_string(),
                        to: to.map(str::to_owned),
                        reason: (*reason).to_string(),
                        selector: selector.map(str::to_owned),
                        traversals: *traversals,
                        budgeted_traversals: *budgeted_traversals,
                    },
                )
                .collect(),
        }
    }

    #[test]
    fn preserves_cycle_edges_and_terminal_edges() {
        let source = build_mermaid_source(&dag(
            "test",
            Some("review"),
            &["test", "review", "finish"],
            &[
                ("test", Some("review"), "sequential", None, 0, 0),
                (
                    "review",
                    Some("test"),
                    "ifFileChanged",
                    Some("src/**"),
                    0,
                    0,
                ),
                ("review", Some("finish"), "sequential", None, 0, 0),
                ("finish", None, "sequential", None, 0, 0),
            ],
        ));
        assert!(source.contains("s1_review -->|\"ifFileChanged: src/**\"| s0_test"));
        assert!(source.contains("s2_finish --> end_terminal[/END/]"));
    }

    #[test]
    fn renders_edge_labels_and_traversal_counts() {
        let source = build_mermaid_source(&dag(
            "build",
            None,
            &["build", "test"],
            &[("build", Some("test"), "IfFileChanged", Some("src/**"), 3, 2)],
        ));
        assert!(source.contains(
            "s0_build -->|\"IfFileChanged: src/** · 3 traversals, 2 budgeted\"| s1_test"
        ));
    }

    #[test]
    fn sanitizes_node_ids_without_changing_labels() {
        let source = build_mermaid_source(&dag(
            "build",
            None,
            &["build", "test/step"],
            &[("build", Some("test/step"), "next", None, 0, 0)],
        ));
        assert!(source.contains("s1_test_step[\"test/step\"]"));
    }

    #[test]
    fn escapes_mermaid_label_characters() {
        let source = build_mermaid_source(&dag("a\\b", None, &["a\\b\n#;`[](){}&\"<>|"], &[]));
        assert!(
            source.contains(
                "[\"a#92;b#10;#35;#59;#96;#91;#93;#40;#41;#123;#125;#amp;#quot;#lt;#gt;#124;\"]"
            ),
            "{source}"
        );
        assert!(source.contains("s0_a_b_______________["), "{source}");
    }
}
