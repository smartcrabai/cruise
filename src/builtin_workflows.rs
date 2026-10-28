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

/// Look up an embedded YAML resource by its normalized, resource-root-relative key.
///
/// Shared resources are implementation dependencies, not selectable workflows.
/// Callers must normalize relative references before looking them up. Unknown keys
/// deliberately do not fall back to the host filesystem.
#[must_use]
pub(crate) fn find_builtin_resource(key: &str) -> Option<&'static str> {
    if key == "_shared/after-pr.yaml" {
        return Some(include_str!("../builtin/_shared/after-pr.yaml"));
    }
    let name = key.strip_suffix(".yaml")?;
    find_builtin(name).map(|workflow| workflow.yaml)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{ConfigSource, resolve_workflow_config};

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
            let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
            let config = resolve_workflow_config(
                w.yaml,
                &ConfigSource::NamedBuiltin(w.name),
                dir.path(),
                None,
            )
            .unwrap_or_else(|e| panic!("{} failed to load: {e:?}", w.name));
            crate::config::validate_config(&config)
                .unwrap_or_else(|e| panic!("{} invalid: {e:?}", w.name));
        }
    }

    #[test]
    fn default_and_named_tdd_have_the_same_executable_steps() {
        let tdd = find_builtin(DEFAULT_BUILTIN_NAME).unwrap_or_else(|| panic!("tdd missing"));
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let load = |yaml, source| {
            resolve_workflow_config(yaml, &source, dir.path(), None)
                .unwrap_or_else(|e| panic!("{e:?}"))
        };
        let default = crate::workflow::compile(load(
            crate::config::BUILTIN_CONFIG_YAML,
            ConfigSource::Builtin,
        ))
        .unwrap_or_else(|e| panic!("{e:?}"));
        let named = crate::workflow::compile(load(
            tdd.yaml,
            ConfigSource::NamedBuiltin(DEFAULT_BUILTIN_NAME),
        ))
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            serde_yaml::to_value(&default.steps).unwrap_or_else(|e| panic!("{e:?}")),
            serde_yaml::to_value(&named.steps).unwrap_or_else(|e| panic!("{e:?}"))
        );
        assert_eq!(
            serde_yaml::to_value(&default.after_pr).unwrap_or_else(|e| panic!("{e:?}")),
            serde_yaml::to_value(&named.after_pr).unwrap_or_else(|e| panic!("{e:?}"))
        );
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
