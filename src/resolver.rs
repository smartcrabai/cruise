use std::path::PathBuf;

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};

/// Indicates where the resolved config came from.
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Explicitly specified via `-c`.
    Explicit(PathBuf),
    /// Specified via `CRUISE_CONFIG` environment variable.
    EnvVar(PathBuf),
    /// Found `cruise.yaml` / `cruise.yml` in the current directory.
    Local(PathBuf),
    /// Selected from `~/.cruise/`.
    UserDir(PathBuf),
    /// No file found; using built-in default.
    Builtin,
}

impl ConfigSource {
    pub fn display_string(&self) -> String {
        match self {
            Self::Builtin => "config: (builtin default)".to_string(),
            Self::Explicit(p) | Self::EnvVar(p) | Self::Local(p) | Self::UserDir(p) => {
                format!("config: {}", p.display())
            }
        }
    }
}

/// Resolve a workflow config, returning (yaml_content, source).
///
/// Resolution order:
/// 1. `explicit` (`-c` flag) — error if file does not exist.
/// 2. `CRUISE_CONFIG` env var — error if file does not exist.
/// 3. `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml`.
/// 4. `~/.cruise/*.yaml` / `*.yml` — auto-select if exactly one, else prompt.
/// 6. Built-in default.
pub fn resolve_config(explicit: Option<&str>) -> Result<(String, ConfigSource)> {
    // 1. Explicit path (-c flag).
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

    // 2. CRUISE_CONFIG environment variable.
    if let Ok(env_path) = std::env::var("CRUISE_CONFIG") {
        let buf = PathBuf::from(&env_path);
        let yaml = std::fs::read_to_string(&buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CruiseError::ConfigNotFound(env_path)
            } else {
                CruiseError::Other(format!("failed to read '{}': {}", buf.display(), e))
            }
        })?;
        return Ok((yaml, ConfigSource::EnvVar(buf)));
    }

    // 3-4. Local config files: visible first, then hidden.
    for name in &["cruise.yaml", "cruise.yml", ".cruise.yaml", ".cruise.yml"] {
        if let Some((yaml, path)) = try_read_local(name)? {
            return Ok((yaml, ConfigSource::Local(path)));
        }
    }

    // 5. ~/.cruise/*.yaml / *.yml
    if let Ok(home) = std::env::var("HOME") {
        let cruise_dir = PathBuf::from(home).join(".cruise");
        let files = collect_yaml_files(&cruise_dir);
        if !files.is_empty() {
            let path = if files.len() == 1 {
                files.into_iter().next().unwrap()
            } else {
                prompt_select_config(&files)?
            };
            let yaml = std::fs::read_to_string(&path).map_err(|e| {
                CruiseError::Other(format!("failed to read '{}': {}", path.display(), e))
            })?;
            return Ok((yaml, ConfigSource::UserDir(path)));
        }
    }

    // 6. Built-in default.
    let yaml = serde_yaml::to_string(&WorkflowConfig::default_builtin())
        .map_err(|e| CruiseError::Other(format!("failed to serialize built-in config: {}", e)))?;
    Ok((yaml, ConfigSource::Builtin))
}

/// Try to read a local file by name. Returns `Ok(None)` if not found, `Ok(Some(...))` on
/// success, or `Err(...)` on other I/O errors.
fn try_read_local(name: &str) -> Result<Option<(String, PathBuf)>> {
    let path = PathBuf::from(name);
    match std::fs::read_to_string(&path) {
        Ok(yaml) => Ok(Some((yaml, path))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CruiseError::Other(format!(
            "failed to read '{}': {}",
            name, e
        ))),
    }
}

/// Collect `*.yaml` and `*.yml` files in `dir`, sorted by file name.
/// Subdirectories named `sessions` or `worktrees` are excluded.
fn collect_yaml_files(dir: &PathBuf) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            // Skip sessions/ and worktrees/ subdirectories.
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "sessions" || name == "worktrees" {
                    return false;
                }
            }
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

/// Prompt the user to select one of the given config files using inquire.
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

    let selected = match inquire::Select::new("Select a workflow config", names.clone()).prompt() {
        Ok(name) => name,
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => {
            return Err(CruiseError::Other("config selection cancelled".to_string()));
        }
        Err(e) => return Err(CruiseError::Other(e.to_string())),
    };

    let selection = names.iter().position(|n| n == &selected).unwrap();
    Ok(files[selection].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// Mutex to serialize tests that mutate process-global state (cwd, env vars).
    static GLOBAL_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that serializes access to global state and restores the working directory on drop.
    struct DirGuard {
        prev: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl DirGuard {
        fn new() -> Self {
            let lock = GLOBAL_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            Self {
                prev: std::env::current_dir().unwrap(),
                _lock: lock,
            }
        }
    }
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev);
        }
    }

    /// RAII guard that restores a single environment variable on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: caller holds GLOBAL_ENV_LOCK, so no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
        fn remove(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: caller holds GLOBAL_ENV_LOCK, so no concurrent env access.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: caller holds GLOBAL_ENV_LOCK, so no concurrent env access.
            unsafe {
                if let Some(ref v) = self.prev {
                    std::env::set_var(self.key, v);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

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
        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap();

        // Point HOME to a dir without .cruise to avoid ~/.cruise interference.
        let fake_home = tempfile::tempdir().unwrap();
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap();
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

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap();
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap();
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- local cruise.yml ----

    #[test]
    fn test_resolve_local_yml() {
        let tmp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp_dir.path().join("cruise.yml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap();

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap();
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap();
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- local .cruise.yaml (hidden) ----

    #[test]
    fn test_resolve_hidden_cruise_yaml() {
        let tmp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp_dir.path().join(".cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap();

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap();
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap();
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- local .cruise.yml (hidden) ----

    #[test]
    fn test_resolve_hidden_cruise_yml() {
        let tmp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp_dir.path().join(".cruise.yml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap();

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap();
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap();
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- CRUISE_CONFIG env var ----

    #[test]
    fn test_resolve_env_var_ok() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "command: [echo]\nsteps:\n  s:\n    command: echo").unwrap();
        let path = tmp.path().to_str().unwrap();

        let _dir_guard = DirGuard::new();
        let _env_guard = EnvGuard::set("CRUISE_CONFIG", std::ffi::OsStr::new(path));

        let (yaml, source) = resolve_config(None).unwrap();
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::EnvVar(_)));
    }

    #[test]
    fn test_resolve_env_var_missing_file() {
        let _dir_guard = DirGuard::new();
        let _env_guard = EnvGuard::set(
            "CRUISE_CONFIG",
            std::ffi::OsStr::new("/nonexistent/env/cruise.yaml"),
        );

        let result = resolve_config(None);
        assert!(result.is_err());
    }

    // ---- CRUISE_CONFIG takes priority over local file ----

    #[test]
    fn test_env_var_takes_priority_over_local() {
        let tmp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap();

        let mut env_tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            env_tmp,
            "command: [envvar]\nsteps:\n  s:\n    command: envvar"
        )
        .unwrap();
        let env_path = env_tmp.path().to_str().unwrap();

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap();
        let _env_guard = EnvGuard::set("CRUISE_CONFIG", std::ffi::OsStr::new(env_path));

        let (yaml, source) = resolve_config(None).unwrap();
        assert!(yaml.contains("envvar"));
        assert!(matches!(source, ConfigSource::EnvVar(_)));
    }

    // ---- cruise.yaml takes priority over .cruise.yaml ----

    #[test]
    fn test_local_takes_priority_over_hidden() {
        let tmp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [visible]\nsteps:\n  s:\n    command: visible",
        )
        .unwrap();
        std::fs::write(
            tmp_dir.path().join(".cruise.yaml"),
            "command: [hidden]\nsteps:\n  s:\n    command: hidden",
        )
        .unwrap();

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap();
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, _source) = resolve_config(None).unwrap();
        assert!(yaml.contains("visible"));
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

    // ---- builtin roundtrip ----

    #[test]
    fn test_builtin_yaml_roundtrip() {
        use crate::config::WorkflowConfig;
        let original = WorkflowConfig::default_builtin();
        let yaml = serde_yaml::to_string(&original).unwrap();
        let parsed = WorkflowConfig::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.steps.len(), original.steps.len());
        assert_eq!(parsed.model, original.model);
        assert_eq!(parsed.plan_model, original.plan_model);
        assert_eq!(parsed.command, original.command);
        for key in original.steps.keys() {
            assert!(parsed.steps.contains_key(key), "missing step: {}", key);
        }
    }
}
