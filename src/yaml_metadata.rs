use serde::Deserialize;

use crate::config::WorkflowConfig;

/// Extract the `description` field from a YAML string and normalize it to a single line.
///
/// Prefers parsing the full [`WorkflowConfig`] so config selectors (CLI and GUI) surface
/// the same `description` that round-trips through session persistence (`WorkflowConfig`
/// is re-serialized verbatim into `sessions/{id}/config.yaml`, see `src/plan_cmd.rs`).
/// Falls back to a minimal `description`-only parse when the YAML doesn't fully validate
/// as a `WorkflowConfig` (e.g. it's missing `steps`, or is otherwise malformed), so
/// partial or broken config files still surface a description in the selector.
///
/// Returns `None` if the field is absent (in both parses) or the YAML cannot be parsed
/// at all.
#[must_use]
pub fn extract_one_line_description(yaml: &str) -> Option<String> {
    let desc = if let Ok(config) = WorkflowConfig::from_yaml(yaml) {
        config.description
    } else {
        #[derive(Deserialize)]
        struct DescriptionOnly {
            description: Option<String>,
        }
        let parsed: DescriptionOnly = serde_yaml::from_str(yaml).ok()?;
        parsed.description
    }?;
    let normalized = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_one_line_description_returns_description() {
        let yaml = r"
command: [claude, -p]
description: 'team-shared workflow'
steps:
  s1:
    command: echo hi
";
        let desc = extract_one_line_description(yaml);
        assert_eq!(desc, Some("team-shared workflow".to_string()));
    }

    #[test]
    fn test_extract_one_line_description_returns_none_when_absent() {
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";
        let desc = extract_one_line_description(yaml);
        assert_eq!(desc, None);
    }

    #[test]
    fn test_extract_one_line_description_normalizes_multiline() {
        let yaml = "command: [claude, -p]\ndescription: |\n  line one\n  line two\nsteps:\n  s1:\n    command: echo hi\n";
        let desc = extract_one_line_description(yaml);
        assert_eq!(desc, Some("line one line two".to_string()));
    }

    #[test]
    fn test_extract_one_line_description_returns_none_for_invalid_yaml() {
        let yaml = "not: valid: yaml: [unclosed";
        let desc = extract_one_line_description(yaml);
        assert_eq!(desc, None);
    }

    #[test]
    fn test_extract_one_line_description_reads_via_workflow_config_parse() {
        // Given: a YAML that fully validates as WorkflowConfig (has `steps`).
        let yaml = r"
command: [claude, -p]
description: 'team-shared workflow'
steps:
  s1:
    command: echo hi
";
        // The primary path parses the full WorkflowConfig and its `description`
        // field should equal what the one-line extractor reports (module trims
        // and collapses whitespace on top).
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(config.description.as_deref(), Some("team-shared workflow"));

        let desc = extract_one_line_description(yaml);
        assert_eq!(desc, Some("team-shared workflow".to_string()));
    }

    #[test]
    fn test_extract_one_line_description_falls_back_when_workflow_config_parse_fails() {
        // Given: a YAML with a description but missing the required `steps` field,
        // so it does not fully validate as WorkflowConfig.
        let yaml = "description: 'partial config'\n";
        assert!(
            WorkflowConfig::from_yaml(yaml).is_err(),
            "expected this YAML to fail full WorkflowConfig parsing (missing `steps`)"
        );

        // Then: the fallback minimal parse still recovers the description.
        let desc = extract_one_line_description(yaml);
        assert_eq!(desc, Some("partial config".to_string()));
    }
}
