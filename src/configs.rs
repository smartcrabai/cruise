/// A workflow config file entry discovered in the user workflows directory
/// (`<config_dir>/workflows/`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConfigEntry {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
}

/// List workflow config files in `$XDG_CONFIG_HOME/cruise/workflows/`
/// (defaulting to `~/.config/cruise/workflows/`).
///
/// Returns entries sorted by file name. Files that cannot be read or parsed
/// still appear in the list with `description: None`.
#[must_use]
pub fn list_user_configs() -> Vec<ConfigEntry> {
    let Ok(workflows_dir) = crate::paths::workflows_dir() else {
        return vec![];
    };
    list_configs_in(&workflows_dir)
}

/// List config files in the given directory.
#[must_use]
pub fn list_configs_in(dir: &std::path::Path) -> Vec<ConfigEntry> {
    let mut configs: Vec<ConfigEntry> = yaml_paths_in(dir)
        .into_iter()
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

fn yaml_paths_in(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("yaml" | "yml")
                )
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|path| path.file_name().unwrap_or_default().to_os_string());
    files
}

/// Return top-level `*.yaml` and `*.yml` files left in the legacy user config directory.
///
/// The files are returned in filename order. Files in subdirectories are not included.
#[must_use]
pub fn legacy_user_config_files(config_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    yaml_paths_in(config_dir)
}

fn escape_warning_text(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

/// Build the migration warning for `config_dir`, or [`None`] when no legacy
/// workflow YAML files exist.
#[must_use]
fn legacy_warning_message(config_dir: &std::path::Path) -> Option<String> {
    let legacy = legacy_user_config_files(config_dir);
    if legacy.is_empty() {
        return None;
    }

    let names = legacy
        .iter()
        .map(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            escape_warning_text(&name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let workflows_dir = config_dir.join(crate::paths::WORKFLOWS_DIR_NAME);
    let yaml_pattern = workflows_dir.join("*.yaml");
    let alternate_pattern = workflows_dir.join("*.yml");
    Some(format!(
        "warning: found workflow config(s) directly in {}: {}. cruise now reads {} / {} — move them there.",
        escape_warning_text(&config_dir.to_string_lossy()),
        names,
        escape_warning_text(&yaml_pattern.to_string_lossy()),
        escape_warning_text(&alternate_pattern.to_string_lossy())
    ))
}

fn warn_legacy_user_configs_once<F>(once: &std::sync::Once, config_dir: &std::path::Path, emit: F)
where
    F: FnOnce(String),
{
    if once.is_completed() {
        return;
    }
    let Some(message) = legacy_warning_message(config_dir) else {
        return;
    };
    once.call_once(|| emit(message));
}

/// Print a one-shot warning when legacy top-level user workflow configs exist.
///
/// Workflow YAML files are now read from `<config_dir>/workflows/`; files left
/// directly under `<config_dir>` should be moved there.
pub fn warn_legacy_user_configs() {
    static WARN_ONCE: std::sync::Once = std::sync::Once::new();

    let Ok(config_dir) = crate::paths::config_dir() else {
        return;
    };
    warn_legacy_user_configs_once(&WARN_ONCE, &config_dir, |message| eprintln!("{message}"));
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

        let cruise_workflows_dir = tmp.path().join(".config").join("cruise").join("workflows");
        fs::create_dir_all(&cruise_workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            cruise_workflows_dir.join("my-workflow.yaml"),
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
    fn test_list_user_configs_ignores_legacy_top_level_yaml() {
        // Given: a yaml file sits directly in `.config/cruise/` (legacy location),
        // with no `workflows/` subdir present
        let _lock = crate::test_support::lock_process();
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home = crate::test_support::set_fake_home(tmp.path());

        let cruise_config_dir = tmp.path().join(".config").join("cruise");
        fs::create_dir_all(&cruise_config_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            cruise_config_dir.join("legacy.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: echo legacy",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let entries = list_user_configs();
        let legacy = legacy_user_config_files(&cruise_config_dir);

        // Then: the legacy file is reported for migration but not listed as a workflow candidate
        assert_eq!(legacy, vec![cruise_config_dir.join("legacy.yaml")]);
        assert!(
            entries.is_empty(),
            "legacy top-level yaml must not be listed, got: {entries:?}"
        );
    }

    #[test]
    fn test_legacy_warning_message_names_legacy_files_and_workflows_dir() {
        // Given: a legacy top-level workflow file
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(tmp.path().join("old.yaml"), "command: [local]")
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let message = legacy_warning_message(tmp.path())
            .unwrap_or_else(|| panic!("expected a migration warning"));

        // Then
        assert!(message.contains("old.yaml"));
        assert!(message.contains("workflows/*.yaml"));
        assert!(message.contains("workflows/*.yml"));
    }

    #[test]
    fn test_legacy_warning_escapes_control_characters_in_paths() {
        // Given: a legacy workflow whose filename contains a line break
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let filename = "old\nworkflow.yaml";
        fs::write(tmp.path().join(filename), "command: [local]")
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let message = legacy_warning_message(tmp.path())
            .unwrap_or_else(|| panic!("expected a migration warning"));

        // Then: the warning is safe to print as one terminal line
        assert!(message.contains("old\\nworkflow.yaml"));
        assert!(!message.contains(filename));
    }

    #[test]
    fn test_legacy_warning_is_emitted_once() {
        // Given: a legacy top-level workflow file and a fresh warning guard
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(tmp.path().join("old.yaml"), "command: [local]")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let once = std::sync::Once::new();
        let mut messages = Vec::new();

        // When: warning dispatch is attempted twice
        warn_legacy_user_configs_once(&once, tmp.path(), |message| messages.push(message));
        warn_legacy_user_configs_once(&once, tmp.path(), |message| messages.push(message));

        // Then: only one warning is emitted
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_legacy_warning_message_is_silent_without_top_level_yaml() {
        // Given: only a workflow file in the new subdirectory
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let workflows = tmp.path().join(crate::paths::WORKFLOWS_DIR_NAME);
        fs::create_dir_all(&workflows).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(workflows.join("current.yaml"), "command: [local]")
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: no migration warning is produced
        assert!(legacy_warning_message(tmp.path()).is_none());
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
