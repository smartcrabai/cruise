use std::path::PathBuf;

use crate::error::{CruiseError, Result};

/// Embedded default workflow configuration.
const DEFAULT_CONFIG: &str = include_str!("default_config.yaml");

/// Indicates where the resolved config came from.
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Explicitly specified via `-c`.
    Explicit(PathBuf),
    /// Found `cruise.yaml` in the current directory.
    Local(PathBuf),
    /// Selected from `~/.cruise/`.
    UserDir(PathBuf),
    /// No file found; using built-in default.
    Builtin,
}

/// Resolve a workflow config, returning (yaml_content, source).
///
/// Resolution order:
/// 1. `explicit` (`-c` flag) — error if file does not exist.
/// 2. `./cruise.yaml` in the current directory.
/// 3. `~/.cruise/*.yaml` / `*.yml` — auto-select if exactly one, else prompt.
/// 4. Built-in default.
pub fn resolve_config(explicit: Option<&str>) -> Result<(String, ConfigSource)> {
    // 1. Explicit path.
    if let Some(path) = explicit {
        let buf = PathBuf::from(path);
        let yaml = std::fs::read_to_string(&buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CruiseError::ConfigNotFound(path.to_string())
            } else {
                CruiseError::Other(format!("failed to read '{}': {}", path, e))
            }
        })?;
        return Ok((yaml, ConfigSource::Explicit(buf)));
    }

    // 2. Local cruise.yaml.
    let local = PathBuf::from("cruise.yaml");
    if local.exists() {
        let yaml = std::fs::read_to_string(&local)
            .map_err(|_| CruiseError::ConfigNotFound("cruise.yaml".to_string()))?;
        return Ok((yaml, ConfigSource::Local(local)));
    }

    // 3. ~/.cruise/*.yaml / *.yml
    if let Ok(home) = std::env::var("HOME") {
        let cruise_dir = PathBuf::from(home).join(".cruise");
        if cruise_dir.is_dir() {
            let files = collect_yaml_files(&cruise_dir);
            match files.len() {
                0 => {}
                1 => {
                    let path = files.into_iter().next().unwrap();
                    let yaml = std::fs::read_to_string(&path)
                        .map_err(|e| CruiseError::Other(e.to_string()))?;
                    return Ok((yaml, ConfigSource::UserDir(path)));
                }
                _ => {
                    let path = prompt_select_config(&files)?;
                    let yaml = std::fs::read_to_string(&path)
                        .map_err(|e| CruiseError::Other(e.to_string()))?;
                    return Ok((yaml, ConfigSource::UserDir(path)));
                }
            }
        }
    }

    // 4. Built-in default.
    Ok((DEFAULT_CONFIG.to_string(), ConfigSource::Builtin))
}

/// Collect `*.yaml` and `*.yml` files in `dir`, sorted by file name.
fn collect_yaml_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("yaml") | Some("yml")
                )
        })
        .collect();
    files.sort_by_key(|p| p.file_name().unwrap_or_default().to_os_string());
    files
}

/// Prompt the user to select one of the given config files using dialoguer.
fn prompt_select_config(files: &[PathBuf]) -> Result<PathBuf> {
    let names: Vec<String> = files
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let selection = dialoguer::Select::new()
        .with_prompt("Select a workflow config")
        .items(&names)
        .default(0)
        .interact_opt()
        .map_err(|e| CruiseError::Other(e.to_string()))?
        .ok_or_else(|| CruiseError::Other("config selection cancelled".to_string()))?;

    Ok(files[selection].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ---- explicit path ----

    #[test]
    fn test_resolve_explicit_ok() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "command: [echo]\nsteps:\n  s:\n    command: echo").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let (yaml, source) = resolve_config(Some(&path)).unwrap();
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Explicit(_)));
    }

    #[test]
    fn test_resolve_explicit_missing() {
        let result = resolve_config(Some("/nonexistent/path/cruise.yaml"));
        assert!(result.is_err());
    }

    // ---- builtin fallback ----

    #[test]
    fn test_resolve_builtin_fallback() {
        // Run in a temp dir that has no cruise.yaml and no HOME/.cruise.
        let tmp_dir = tempfile::tempdir().unwrap();
        let prev_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp_dir.path()).unwrap();

        // Point HOME to a dir without .cruise to avoid ~/.cruise interference.
        let fake_home = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("HOME").ok();
        // SAFETY: single-threaded test, no concurrent env access.
        unsafe { std::env::set_var("HOME", fake_home.path()) };

        let result = resolve_config(None);

        // Restore env.
        std::env::set_current_dir(prev_dir).unwrap();
        // SAFETY: single-threaded test, no concurrent env access.
        unsafe {
            if let Some(h) = prev_home {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
        }

        let (yaml, source) = result.unwrap();
        assert!(yaml.contains("steps"));
        assert!(matches!(source, ConfigSource::Builtin));
    }

    // ---- local cruise.yaml ----

    #[test]
    fn test_resolve_local() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let config_path = tmp_dir.path().join("cruise.yaml");
        std::fs::write(
            &config_path,
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap();

        let prev_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp_dir.path()).unwrap();

        let result = resolve_config(None);

        std::env::set_current_dir(prev_dir).unwrap();

        let (yaml, source) = result.unwrap();
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- collect_yaml_files ----

    #[test]
    fn test_collect_yaml_files_sorted() {
        let tmp_dir = tempfile::tempdir().unwrap();
        std::fs::write(tmp_dir.path().join("b.yaml"), "").unwrap();
        std::fs::write(tmp_dir.path().join("a.yml"), "").unwrap();
        std::fs::write(tmp_dir.path().join("c.yaml"), "").unwrap();
        std::fs::write(tmp_dir.path().join("d.txt"), "").unwrap();

        let files = collect_yaml_files(&tmp_dir.path().to_path_buf());
        let names: Vec<&str> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["a.yml", "b.yaml", "c.yaml"]);
    }

    #[test]
    fn test_collect_yaml_files_empty_dir() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let files = collect_yaml_files(&tmp_dir.path().to_path_buf());
        assert!(files.is_empty());
    }

    // ---- builtin YAML parses ----

    #[test]
    fn test_builtin_yaml_parses() {
        use crate::config::WorkflowConfig;
        let config = WorkflowConfig::from_yaml(DEFAULT_CONFIG).unwrap();
        assert!(!config.steps.is_empty());
    }
}
