//! Static catalog of built-in workflows.

/// A built-in workflow selectable by name.
#[derive(Debug)]
pub struct BuiltinWorkflow {
    pub name: &'static str,
    pub description: &'static str,
    pub yaml: &'static str,
}

/// Name of the default built-in workflow.
pub const DEFAULT_BUILTIN_NAME: &str = "default";

/// The fixed built-in catalog.
pub static BUILTIN_WORKFLOWS: &[BuiltinWorkflow] = &[
    BuiltinWorkflow {
        name: DEFAULT_BUILTIN_NAME,
        description: "Plan, implement, verify, and open a pull request",
        yaml: include_str!("../builtin/default.yaml"),
    },
    BuiltinWorkflow {
        name: "simple",
        description: "Implement, test, auto-fix failures, then commit",
        yaml: include_str!("../builtin/simple.yaml"),
    },
    BuiltinWorkflow {
        name: "review",
        description: "Review the changes and report findings without editing files",
        yaml: include_str!("../builtin/review.yaml"),
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
        assert_eq!(names, vec!["default", "simple", "review"]);
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
    fn default_builtin_yaml_matches_legacy_constant() {
        let default = find_builtin("default").unwrap_or_else(|| panic!("default missing"));
        assert_eq!(default.yaml, BUILTIN_CONFIG_YAML);
    }

    #[test]
    fn find_builtin_returns_none_for_unknown_name() {
        assert!(find_builtin("nope").is_none());
        assert!(find_builtin("../simple").is_none());
    }
}
