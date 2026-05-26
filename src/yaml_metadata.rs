use serde::Deserialize;

/// Extract the `description` field from a YAML string and normalize it to a single line.
///
/// Returns `None` if the field is absent or the YAML cannot be parsed.
#[must_use]
pub fn extract_one_line_description(yaml: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct DescriptionOnly {
        description: Option<String>,
    }
    let parsed: DescriptionOnly = serde_yaml::from_str(yaml).ok()?;
    let desc = parsed.description?;
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
}
