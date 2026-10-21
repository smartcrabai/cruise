//! Static catalog of built-in workflows.

/// A built-in workflow selectable by name.
#[derive(Debug)]
pub struct BuiltinWorkflow {
    pub name: &'static str,
    pub description: &'static str,
    pub yaml: &'static str,
}

/// Name of the default built-in workflow.
pub const DEFAULT_BUILTIN_NAME: &str = "tdd";

/// The fixed built-in catalog.
pub static BUILTIN_WORKFLOWS: &[BuiltinWorkflow] = &[
    BuiltinWorkflow {
        name: DEFAULT_BUILTIN_NAME,
        description: "Plan, implement test-first, verify, and open a pull request",
        yaml: include_str!("../builtin/tdd.yaml"),
    },
    BuiltinWorkflow {
        name: "refactor",
        description: "Audit and improve code, tests, and structure, then verify",
        yaml: include_str!("../builtin/refactor.yaml"),
    },
    BuiltinWorkflow {
        name: "add-tests",
        description: "Add meaningful tests, linting, and CI for the repository",
        yaml: include_str!("../builtin/add-tests.yaml"),
    },
    BuiltinWorkflow {
        name: "add-formal-specs",
        description: "Add executable Quint, Alloy, and Gherkin specs with CI",
        yaml: include_str!("../builtin/add-formal-specs.yaml"),
    },
    BuiltinWorkflow {
        name: "research",
        description: "Produce a source-backed research report without changing code",
        yaml: include_str!("../builtin/research.yaml"),
    },
    BuiltinWorkflow {
        name: "security",
        description: "Audit security, fix confirmed findings, and verify regressions",
        yaml: include_str!("../builtin/security.yaml"),
    },
];

/// Look up a built-in workflow by its fixed name.
#[must_use]
pub fn find_builtin(name: &str) -> Option<&'static BuiltinWorkflow> {
    BUILTIN_WORKFLOWS.iter().find(|w| w.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BUILTIN_CONFIG_YAML, WorkflowConfig, validate_config};

    #[test]
    fn builtin_catalog_has_unique_names_descriptions_and_valid_yaml() {
        let names: Vec<&str> = BUILTIN_WORKFLOWS.iter().map(|w| w.name).collect();
        assert_eq!(
            names,
            vec![
                "tdd",
                "refactor",
                "add-tests",
                "add-formal-specs",
                "research",
                "security",
            ]
        );
        for w in BUILTIN_WORKFLOWS {
            assert!(
                !w.description.trim().is_empty(),
                "{} lacks description",
                w.name
            );
            let config = WorkflowConfig::from_yaml(w.yaml)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e:?}", w.name));
            validate_config(&config).unwrap_or_else(|e| panic!("{} invalid: {e:?}", w.name));
        }
    }

    #[test]
    fn tdd_builtin_yaml_matches_embedded_config() {
        let tdd = find_builtin(DEFAULT_BUILTIN_NAME).unwrap_or_else(|| panic!("tdd missing"));
        assert_eq!(tdd.yaml, BUILTIN_CONFIG_YAML);
    }

    #[test]
    fn find_builtin_returns_none_for_unknown_name() {
        for removed_name in ["default", "simple", "review"] {
            assert!(
                find_builtin(removed_name).is_none(),
                "removed built-in {removed_name} must not resolve"
            );
        }
        assert!(find_builtin("nope").is_none());
        assert!(find_builtin("../simple").is_none());
    }
}
