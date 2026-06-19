/// A config file entry discovered in the user config directory.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigEntry {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
}

/// List workflow config files in `$XDG_CONFIG_HOME/cruise/` (defaulting to `~/.config/cruise/`).
///
/// Returns entries sorted by file name. Files that cannot be read or parsed
/// still appear in the list with `description: None`.
#[must_use]
pub fn list_user_configs() -> Vec<ConfigEntry> {
    let Ok(config_dir) = crate::paths::config_dir() else {
        return vec![];
    };
    list_configs_in(&config_dir)
}

/// List config files in the given directory.
#[must_use]
pub fn list_configs_in(dir: &std::path::Path) -> Vec<ConfigEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut configs: Vec<ConfigEntry> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file() && matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
        })
        .map(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let description = std::fs::read_to_string(&p)
                .ok()
                .and_then(|yaml| extract_file_description(&yaml));
            ConfigEntry {
                name,
                path: p.to_string_lossy().into_owned(),
                description,
            }
        })
        .collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    configs
}

/// Extract a description from a YAML config file.
///
/// Tries leading `#` comment first, then falls back to the `description:` YAML field.
fn extract_file_description(yaml: &str) -> Option<String> {
    // Try leading comment (e.g. `# My workflow description`).
    // Skip editor-directive lines such as `# yaml-language-server: $schema=…`.
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            let desc = comment.trim();
            let is_editor_directive = desc.starts_with("yaml-language-server:")
                || desc.starts_with("$schema")
                || desc.starts_with("@schema");
            if !desc.is_empty() && !is_editor_directive {
                return Some(desc.to_string());
            }
            // Directive line: keep scanning for a real description comment.
            continue;
        }
        // First non-comment, non-blank line reached — no human description found.
        break;
    }
    // Fall back to YAML `description:` field
    crate::yaml_metadata::extract_one_line_description(yaml)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // --- list_configs_in ---

    #[test]
    fn test_list_configs_in_empty_directory_returns_empty() {
        // Given: an empty directory
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_configs_in(tmp.path());

        // Then
        assert!(entries.is_empty(), "empty dir should yield no entries");
    }

    #[test]
    fn test_list_configs_in_missing_directory_returns_empty() {
        // Given: a path that does not exist
        let path = std::path::PathBuf::from("/nonexistent/path/that/cannot/exist");

        // When
        let entries = list_configs_in(&path);

        // Then: graceful empty result, no panic
        assert!(
            entries.is_empty(),
            "missing dir should yield no entries (not panic)"
        );
    }

    #[test]
    fn test_list_configs_in_returns_yaml_files_sorted_by_name() {
        // Given: a directory with yaml files in non-alphabetical creation order
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            tmp.path().join("zebra.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo z",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            tmp.path().join("alpha.yml"),
            "command: [local]\nsteps:\n  s:\n    command: echo a",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            tmp.path().join("beta.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo b",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_configs_in(tmp.path());

        // Then: sorted alphabetically by name
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.yml", "beta.yaml", "zebra.yaml"]);
    }

    #[test]
    fn test_list_configs_in_ignores_non_yaml_files() {
        // Given: a directory containing yaml and non-yaml files
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            tmp.path().join("valid.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo ok",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(tmp.path().join("README.md"), "# docs").unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(tmp.path().join("script.sh"), "#!/bin/bash").unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(tmp.path().join("config.toml"), "[foo]").unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_configs_in(tmp.path());

        // Then: only valid.yaml appears
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "valid.yaml");
    }

    #[test]
    fn test_list_configs_in_extracts_description_from_yaml_metadata() {
        // Given: a yaml file with a one-line description comment
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            tmp.path().join("with-desc.yaml"),
            "# My workflow description\ncommand: [local]\nsteps:\n  s:\n    command: echo ok",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_configs_in(tmp.path());

        // Then: description is extracted
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].description.is_some(),
            "description should be extracted from yaml comment"
        );
    }

    #[test]
    fn test_list_configs_in_description_is_none_for_file_without_comment() {
        // Given: a yaml file without a leading comment
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            tmp.path().join("no-desc.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo ok",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_configs_in(tmp.path());

        // Then
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].description.is_none(),
            "description should be None when no comment is present"
        );
    }

    #[test]
    fn test_list_configs_in_path_field_is_absolute() {
        // Given: a directory with a yaml file
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            tmp.path().join("cfg.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo ok",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_configs_in(tmp.path());

        // Then: path is an absolute path string
        assert_eq!(entries.len(), 1);
        assert!(
            std::path::Path::new(&entries[0].path).is_absolute(),
            "path should be absolute: {}",
            entries[0].path
        );
    }

    // --- list_user_configs ---

    #[test]
    fn test_list_user_configs_reads_from_xdg_config_home() {
        // Given: XDG_CONFIG_HOME set to a temp dir with a yaml file
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());

        let cruise_config_dir = tmp.path().join(".config").join("cruise");
        fs::create_dir_all(&cruise_config_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            cruise_config_dir.join("my-workflow.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo ok",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_user_configs();

        // Then: the file is found
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "my-workflow.yaml");
    }

    #[test]
    fn test_list_user_configs_returns_empty_when_config_dir_missing() {
        // Given: a fake HOME with no .config/cruise directory
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());

        // When
        let entries = list_user_configs();

        // Then: no panic, empty result
        assert!(
            entries.is_empty(),
            "missing config dir should return empty list"
        );
    }
}
