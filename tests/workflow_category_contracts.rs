use cruise::workflow_categories::{
    PickerCandidate, PickerNode, WorkflowCategoryNode, build_picker_tree,
    validate_workflow_categories,
};
use indexmap::IndexMap;

type Categories = IndexMap<String, WorkflowCategoryNode>;

fn categories(yaml: &str) -> Categories {
    serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{e}"))
}

fn candidate(key: &str, builtin: bool, local: bool, package: bool) -> PickerCandidate {
    PickerCandidate {
        value: format!("{key}-{builtin}-{local}-{package}"),
        label: key.into(),
        key: key.into(),
        is_builtin: builtin,
        is_local: local,
        is_installed_package: package,
        description: None,
    }
}

// Compare only the observable shape and candidate indices, not implementation details.
fn shape(nodes: &[PickerNode]) -> Vec<String> {
    fn visit(nodes: &[PickerNode], path: &str, out: &mut Vec<String>) {
        for node in nodes {
            match node {
                PickerNode::Category { name, nodes } => {
                    let path = if path.is_empty() {
                        name.clone()
                    } else {
                        format!("{path}/{name}")
                    };
                    out.push(format!("[{path}]"));
                    visit(nodes, &path, out);
                }
                PickerNode::Workflow {
                    candidate,
                    description,
                } => {
                    out.push(format!("{path}:{candidate}:{description:?}"));
                }
            }
        }
    }
    let mut out = Vec::new();
    visit(nodes, "", &mut out);
    out
}

#[test]
fn parses_nested_categories_with_plain_and_described_entries() {
    let parsed = categories(
        "Development:\n  workflows: [tdd, {refactor: R}]\n  Backend:\n    workflows: [api]\nQuality: {}\n",
    );
    assert_eq!(
        parsed.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Development", "Quality"]
    );
    let dev = &parsed["Development"];
    assert_eq!(
        dev.workflows
            .iter()
            .map(|e| (e.name.as_str(), e.description.as_deref()))
            .collect::<Vec<_>>(),
        [("tdd", None), ("refactor", Some("R"))]
    );
    assert_eq!(
        dev.children.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Backend"]
    );
    assert_eq!(dev.children["Backend"].workflows[0].name, "api");
}

#[test]
fn rejects_malformed_categories_and_entries() {
    let error = serde_yaml::from_str::<Categories>("Development: [tdd]")
        .err()
        .unwrap_or_else(|| panic!("invalid category should fail"))
        .to_string();
    assert!(error.contains("Development"), "{error}");
    for yaml in ["Dev: {workflows: [{a: x, b: y}]}", "Dev: {workflows: [3]}"] {
        assert!(
            serde_yaml::from_str::<Categories>(yaml).is_err(),
            "accepted {yaml}"
        );
    }
}

#[test]
fn validation_rejects_blank_names_descriptions_and_conflicts() {
    for yaml in [
        "' ': {}",
        "' Dev': {}",
        "Dev: {workflows: [' ']}",
        "Dev: {workflows: [{tdd: ''}]}",
        "Dev: {workflows: [{tdd: A}]}\nOther: {workflows: [{tdd: B}]}",
    ] {
        let result = serde_yaml::from_str::<Categories>(yaml)
            .map_err(|e| e.to_string())
            .and_then(|c| validate_workflow_categories(&c).map_err(|e| e.to_string()));
        assert!(result.is_err(), "accepted {yaml}");
    }
    assert!(
        validate_workflow_categories(&categories(
            "Dev: {workflows: [{tdd: A}]}\nOther: {workflows: [{tdd: A}]}"
        ))
        .is_ok()
    );
}

#[test]
fn round_trips_yaml_and_json_preserving_order() {
    let original =
        categories("Zed: {workflows: [tdd, {refactor: R}]}\nAlpha: {Child: {workflows: [api]}}\n");
    let yaml = serde_yaml::to_string(&original).unwrap_or_else(|e| panic!("{e}"));
    let json = serde_json::to_string(&original).unwrap_or_else(|e| panic!("{e}"));
    for decoded in [
        serde_yaml::from_str::<Categories>(&yaml).unwrap_or_else(|e| panic!("{e}")),
        serde_json::from_str::<Categories>(&json).unwrap_or_else(|e| panic!("{e}")),
    ] {
        assert_eq!(decoded, original);
        assert_eq!(
            decoded.keys().map(String::as_str).collect::<Vec<_>>(),
            ["Zed", "Alpha"]
        );
    }
}

#[test]
fn default_categories_and_user_replacement() {
    let candidates = [
        candidate("tdd", true, false, false),
        candidate("refactor", true, false, false),
        candidate("add-tests", true, false, false),
        candidate("add-formal-specs", true, false, false),
        candidate("research", true, false, false),
        candidate("security", true, false, false),
    ];
    let tree = build_picker_tree(&candidates, &Categories::new(), true, "Others");
    let names: Vec<_> = tree
        .iter()
        .filter_map(|n| match n {
            PickerNode::Category { name, .. } => Some(name.as_str()),
            PickerNode::Workflow { .. } => None,
        })
        .collect();
    assert_eq!(names, ["Development", "Quality", "Research", "Security"]);
    let tree = build_picker_tree(
        &candidates[..2],
        &categories("Dev: {workflows: [refactor]}"),
        true,
        "Others",
    );
    assert_eq!(
        shape(&tree),
        ["[Dev]", "Dev:1:None", "[Others]", "Others:0:None"]
    );
}

#[test]
fn matching_order_multiple_sources_and_duplicate_membership() {
    let candidates = [
        candidate("review", false, true, false),
        candidate("review", false, false, false),
        candidate("review", true, false, false),
    ];
    let cats = categories(
        "All: {workflows: [{review: custom}]}\nBuiltins: {workflows: ['builtin:review']}\nAgain: {workflows: [review]}",
    );
    assert_eq!(
        shape(&build_picker_tree(&candidates, &cats, false, "Others")),
        [
            "[All]",
            "All:0:Some(\"custom\")",
            "All:1:Some(\"custom\")",
            "All:2:Some(\"custom\")",
            "[Builtins]",
            "Builtins:2:None",
            "[Again]",
            "Again:0:None",
            "Again:1:None",
            "Again:2:None"
        ]
    );
}

#[test]
fn root_order_pruning_packages_and_others() {
    let candidates = [
        candidate("local1", false, true, false),
        candidate("package", false, false, true),
        candidate("user", false, false, false),
        candidate("local2", false, true, false),
        candidate("api", false, false, false),
        candidate("builtin", true, false, false),
    ];
    let cats = categories(
        "Empty: {workflows: [missing]}\nDev:\n  Dead: {workflows: [missing]}\n  Backend: {workflows: [api]}\n",
    );
    assert_eq!(
        shape(&build_picker_tree(&candidates, &cats, true, "Misc")),
        [
            ":0:None",
            ":3:None",
            "[Dev]",
            "[Dev/Backend]",
            "Dev/Backend:4:None",
            "[Packages]",
            "Packages:1:None",
            "[Misc]",
            "Misc:2:None",
            "Misc:5:None"
        ]
    );
    assert_eq!(
        shape(&build_picker_tree(&candidates, &cats, false, "Misc")),
        [
            ":0:None",
            ":3:None",
            "[Dev]",
            "[Dev/Backend]",
            "Dev/Backend:4:None",
            "[Packages]",
            "Packages:1:None"
        ]
    );
}
