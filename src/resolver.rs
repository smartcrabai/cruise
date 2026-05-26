use std::path::PathBuf;

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
    /// Selected from `~/.config/cruise/`.
    UserDir(PathBuf),
}

impl ConfigSource {
    #[must_use]
    pub fn display_string(&self) -> String {
        let (Self::Explicit(p) | Self::EnvVar(p) | Self::Local(p) | Self::UserDir(p)) = self;
        format!("config: {}", p.display())
    }

    /// Returns the path to the config file.
    #[must_use]
    pub fn path(&self) -> &PathBuf {
        let (Self::Explicit(p) | Self::EnvVar(p) | Self::Local(p) | Self::UserDir(p)) = self;
        p
    }
}

/// Resolve a workflow config, returning (`yaml_content`, source).
///
/// Resolution order:
/// 1. `explicit` (`-c` flag) -- error if file does not exist.
/// 2. `CRUISE_CONFIG` env var -- error if file does not exist.
/// 3. `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml`.
/// 4. `~/.config/cruise/*.yaml` / `*.yml` -- auto-select if exactly one, else prompt.
///
/// # Errors
///
/// Returns [`CruiseError::ConfigNotFound`] if no config file is found. Specify one with
/// `-c` or `CRUISE_CONFIG`, or place a config in `~/.config/cruise/`.
pub fn resolve_config(explicit: Option<&str>) -> Result<(String, ConfigSource)> {
    let cwd = std::env::current_dir()
        .map_err(|e| CruiseError::Other(format!("failed to get current directory: {e}")))?;
    resolve_config_in_dir(explicit, &cwd)
}

/// Like [`resolve_config`] but uses `cwd` for local-file discovery instead of the
/// process working directory.
///
/// This is safe to call from concurrent Tauri request handlers because it does not
/// mutate `std::env::current_dir()`.  Resolution order is identical to [`resolve_config`]:
/// 1. `explicit` -- error if file does not exist.
/// 2. `CRUISE_CONFIG` env var -- error if file does not exist.
/// 3. `cruise.yaml` / `cruise.yml` / `.cruise.yaml` / `.cruise.yml` under `cwd`.
/// 4. `~/.config/cruise/*.yaml` / `*.yml`.
///
/// # Errors
///
/// Returns [`CruiseError::ConfigNotFound`] if no config file is found. Specify one with
/// `-c` or `CRUISE_CONFIG`, or place a config in `~/.config/cruise/`.
pub fn resolve_config_in_dir(
    explicit: Option<&str>,
    cwd: &std::path::Path,
) -> Result<(String, ConfigSource)> {
    // 1. Explicit path (-c flag).
    if let Some(path) = explicit {
        let buf = PathBuf::from(path);
        let yaml = std::fs::read_to_string(&buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CruiseError::ConfigNotFound(path.to_string())
            } else {
                CruiseError::Other(format!("failed to read '{path}': {e}"))
            }
        })?;
        return Ok((yaml, ConfigSource::Explicit(to_absolute(buf))));
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
        return Ok((yaml, ConfigSource::EnvVar(to_absolute(buf))));
    }

    // 3. Local config files relative to `cwd` (not process cwd).
    for name in &["cruise.yaml", "cruise.yml", ".cruise.yaml", ".cruise.yml"] {
        let path = cwd.join(name);
        match std::fs::read_to_string(&path) {
            Ok(yaml) => return Ok((yaml, ConfigSource::Local(path))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CruiseError::Other(format!(
                    "failed to read '{}': {e}",
                    path.display()
                )));
            }
        }
    }

    // 4. $XDG_CONFIG_HOME/cruise/*.yaml / *.yml (defaults to ~/.config/cruise/)
    if let Ok(config_dir) = crate::paths::config_dir() {
        let mut files = collect_yaml_files(&config_dir);
        if !files.is_empty() {
            let path = if files.len() == 1 {
                files.remove(0)
            } else {
                prompt_select_config(&files)?
            };
            let yaml = std::fs::read_to_string(&path).map_err(|e| {
                CruiseError::Other(format!("failed to read '{}': {}", path.display(), e))
            })?;
            return Ok((yaml, ConfigSource::UserDir(path)));
        }
    }

    // No config found — require the user to specify one explicitly.
    Err(CruiseError::ConfigNotFound(
        "no config found: checked cruise.yaml/.yml in the given directory and ~/.config/cruise/. \
         Specify one with -c or CRUISE_CONFIG."
            .to_string(),
    ))
}

/// Convert a path to absolute by joining with the current working directory.
/// If the path is already absolute, it is returned unchanged.
/// Falls back to the original path if `current_dir()` fails.
fn to_absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
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
            p.is_file() && matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
        })
        .collect();
    files.sort_by_key(|p| p.file_name().unwrap_or_default().to_os_string());
    files
}

/// Build `(display_label, path)` pairs for each config file.
///
/// Reads each file, parses `description`, and formats label as
/// `"{filename}  —  {description}"` when present or `"{filename}"` when absent.
/// Files that fail to parse are included with filename-only labels.
fn build_config_select_items(files: &[PathBuf]) -> Vec<(String, PathBuf)> {
    files
        .iter()
        .map(|path| {
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let label = if let Some(desc) = std::fs::read_to_string(path)
                .ok()
                .and_then(|yaml| crate::config::extract_one_line_description(&yaml))
            {
                format!("{file_name}  —  {desc}")
            } else {
                file_name
            };
            (label, path.clone())
        })
        .collect()
}

/// Prompt the user to select one of the given config files using inquire.
fn prompt_select_config(files: &[PathBuf]) -> Result<PathBuf> {
    let items = build_config_select_items(files);
    let labels: Vec<String> = items.iter().map(|(label, _)| label.clone()).collect();

    let selected_idx = match inquire::Select::new("Select a workflow config", labels).raw_prompt() {
        Ok(opt) => opt.index,
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => {
            return Err(CruiseError::Other("config selection cancelled".to_string()));
        }
        Err(e) => return Err(CruiseError::Other(e.to_string())),
    };

    items
        .into_iter()
        .nth(selected_idx)
        .map(|(_, path)| path)
        .ok_or_else(|| CruiseError::Other("selected config index out of range".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// RAII guard that serializes access to global state and restores the working directory on drop.
    struct DirGuard {
        prev: PathBuf,
        _lock: crate::test_support::ProcessLock,
    }
    impl DirGuard {
        fn new() -> Self {
            let lock = crate::test_support::lock_process();
            Self {
                prev: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
                _lock: lock,
            }
        }
    }
    impl Drop for DirGuard {
        fn drop(&mut self) {
            if std::env::set_current_dir(&self.prev).is_err() {
                let _ = std::env::set_current_dir("/");
            }
        }
    }

    use crate::test_support::EnvGuard;

    // ---- explicit path ----

    #[test]
    fn test_resolve_explicit_ok() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        writeln!(tmp, "command: [echo]\nsteps:\n  s:\n    command: echo")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"))
            .to_string();
        let (yaml, source) = resolve_config(Some(&path)).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Explicit(_)));
    }

    #[test]
    fn test_resolve_explicit_missing() {
        let result = resolve_config(Some("/nonexistent/path/cruise.yaml"));
        assert!(result.is_err());
    }

    // ---- local cruise.yaml ----

    #[test]
    fn test_resolve_local() {
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let config_path = tmp_dir.path().join("cruise.yaml");
        std::fs::write(
            &config_path,
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- local cruise.yml ----

    #[test]
    fn test_resolve_local_yml() {
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- local .cruise.yaml (hidden) ----

    #[test]
    fn test_resolve_hidden_cruise_yaml() {
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join(".cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- local .cruise.yml (hidden) ----

    #[test]
    fn test_resolve_hidden_cruise_yml() {
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join(".cruise.yml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("echo"));
        assert!(matches!(source, ConfigSource::Local(_)));
    }

    // ---- CRUISE_CONFIG env var ----

    #[test]
    fn test_resolve_env_var_ok() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        writeln!(tmp, "command: [echo]\nsteps:\n  s:\n    command: echo")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"));

        let _dir_guard = DirGuard::new();
        let _env_guard = EnvGuard::set("CRUISE_CONFIG", std::ffi::OsStr::new(path));

        let (yaml, source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
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
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let mut env_tmp = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        writeln!(
            env_tmp,
            "command: [envvar]\nsteps:\n  s:\n    command: envvar"
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let env_path = env_tmp
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let _env_guard = EnvGuard::set("CRUISE_CONFIG", std::ffi::OsStr::new(env_path));

        let (yaml, source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("envvar"));
        assert!(matches!(source, ConfigSource::EnvVar(_)));
    }

    // ---- cruise.yaml takes priority over .cruise.yaml ----

    #[test]
    fn test_local_takes_priority_over_hidden() {
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [visible]\nsteps:\n  s:\n    command: visible",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join(".cruise.yaml"),
            "command: [hidden]\nsteps:\n  s:\n    command: hidden",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, _source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("visible"));
    }

    // ---- collect_yaml_files ----

    #[test]
    fn test_collect_yaml_files_sorted() {
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(tmp_dir.path().join("b.yaml"), "").unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(tmp_dir.path().join("a.yml"), "").unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(tmp_dir.path().join("c.yaml"), "").unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(tmp_dir.path().join("d.txt"), "").unwrap_or_else(|e| panic!("{e:?}"));

        let files = collect_yaml_files(&tmp_dir.path().to_path_buf());
        let names: Vec<&str> = files
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_else(|| panic!("unexpected None"))
                    .to_str()
                    .unwrap_or_else(|| panic!("unexpected None"))
            })
            .collect();
        assert_eq!(names, vec!["a.yml", "b.yaml", "c.yaml"]);
    }

    #[test]
    fn test_collect_yaml_files_empty_dir() {
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let files = collect_yaml_files(&tmp_dir.path().to_path_buf());
        assert!(files.is_empty());
    }

    // ---- resolve_config_in_dir ----

    #[test]
    fn test_resolve_in_dir_local_config_beats_user_dir() {
        // Given: a repo directory has cruise.yaml, and ~/.cruise/default.yaml also exists
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            repo_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let config_cruise = fake_home.path().join(".config").join("cruise");
        std::fs::create_dir_all(&config_cruise).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            config_cruise.join("default.yaml"),
            "command: [userdir]\nsteps:\n  s:\n    command: userdir",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved against the repo directory
        let (yaml, source) =
            resolve_config_in_dir(None, repo_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the repo-local config wins over the user-dir default
        assert!(yaml.contains("local"), "expected local config, got: {yaml}");
        assert!(
            matches!(source, ConfigSource::Local(_)),
            "expected Local, got: {source:?}"
        );
        if let ConfigSource::Local(p) = source {
            assert_eq!(p, repo_dir.path().join("cruise.yaml"));
        }
    }

    #[test]
    fn test_resolve_in_dir_does_not_use_process_cwd() {
        // Given: process cwd has cruise.yaml, but the given dir does not
        let process_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            process_dir.path().join("cruise.yaml"),
            "command: [process_cwd]\nsteps:\n  s:\n    command: process_cwd",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let other_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(process_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved against a different directory (not the process cwd)
        let result = resolve_config_in_dir(None, other_dir.path());

        // Then: the process cwd's cruise.yaml is NOT picked up; returns error (no builtin fallback)
        assert!(
            result.is_err(),
            "expected Err (process cwd must not be used), got Ok"
        );
    }

    #[test]
    fn test_resolve_in_dir_explicit_path_bypasses_dir() {
        // Given: a repo dir with local cruise.yaml, and a separate explicit config file
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            repo_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let explicit_file = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            explicit_file.path(),
            "command: [explicit]\nsteps:\n  s:\n    command: explicit",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let explicit_path = explicit_file
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"))
            .to_string();

        let _dir_guard = DirGuard::new();
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: an explicit config path is provided alongside a repo dir
        let (yaml, source) = resolve_config_in_dir(Some(&explicit_path), repo_dir.path())
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the explicit config wins over local repo config
        assert!(
            yaml.contains("explicit"),
            "expected explicit config, got: {yaml}"
        );
        assert!(
            matches!(source, ConfigSource::Explicit(_)),
            "expected Explicit, got: {source:?}"
        );
    }

    #[test]
    fn test_resolve_in_dir_env_var_bypasses_dir() {
        // Given: a repo dir with cruise.yaml, and CRUISE_CONFIG pointing elsewhere
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            repo_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let env_file = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            env_file.path(),
            "command: [envvar]\nsteps:\n  s:\n    command: envvar",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let env_path = env_file
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"));

        let _dir_guard = DirGuard::new();
        let _env_guard = EnvGuard::set("CRUISE_CONFIG", std::ffi::OsStr::new(env_path));

        // When: resolved against the repo dir while CRUISE_CONFIG is set
        let (yaml, source) =
            resolve_config_in_dir(None, repo_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: CRUISE_CONFIG wins over local repo config
        assert!(
            yaml.contains("envvar"),
            "expected envvar config, got: {yaml}"
        );
        assert!(
            matches!(source, ConfigSource::EnvVar(_)),
            "expected EnvVar, got: {source:?}"
        );
    }

    #[test]
    fn test_resolve_in_dir_falls_back_to_user_dir() {
        // Given: repo dir has no local config; home has exactly one ~/.cruise/*.yaml
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let config_cruise = fake_home.path().join(".config").join("cruise");
        std::fs::create_dir_all(&config_cruise).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            config_cruise.join("myconf.yaml"),
            "command: [userdir]\nsteps:\n  s:\n    command: userdir",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        // `home` crate 0.5.x uses USERPROFILE on Windows, HOME on Unix.
        let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let _home_guard = EnvGuard::set(home_var, fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved against the empty repo dir
        let (yaml, source) =
            resolve_config_in_dir(None, repo_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: falls back to user-dir config, not builtin
        assert!(
            yaml.contains("userdir"),
            "expected userdir config, got: {yaml}"
        );
        assert!(
            matches!(source, ConfigSource::UserDir(_)),
            "expected UserDir, got: {source:?}"
        );
    }

    // ---- no config available → error (builtin removed) ----

    #[test]
    fn test_no_config_returns_config_not_found_error() {
        // Given: no config file exists anywhere (no explicit, no env var, no local file, no user dir)
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolve_config is called with no explicit path
        let result = resolve_config(None);

        // Then: returns a ConfigNotFound error (no built-in fallback)
        let Err(err) = result else {
            panic!("expected Err when no config is available, got Ok");
        };
        assert!(
            matches!(err, CruiseError::ConfigNotFound(_)),
            "expected ConfigNotFound error"
        );
    }

    #[test]
    fn test_resolve_in_dir_no_config_returns_error() {
        // Given: an empty directory with no cruise.yaml and no user-dir config
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolve_config_in_dir is called with an empty directory
        let result = resolve_config_in_dir(None, repo_dir.path());

        // Then: returns a ConfigNotFound error (no built-in fallback)
        let Err(err) = result else {
            panic!("expected Err when no config found in dir, got Ok");
        };
        assert!(
            matches!(err, CruiseError::ConfigNotFound(_)),
            "expected ConfigNotFound error"
        );
    }

    #[test]
    fn test_no_config_error_contains_usage_hint() {
        // Given: no config anywhere
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolve_config fails
        let Err(err) = resolve_config(None) else {
            panic!("expected resolve_config to return Err when no config is available");
        };
        let err_str = err.to_string();

        // Then: error message guides the user toward -c or CRUISE_CONFIG
        assert!(
            err_str.contains("-c") || err_str.contains("CRUISE_CONFIG"),
            "error message should suggest -c or CRUISE_CONFIG, got: {err_str}"
        );
    }

    #[test]
    fn test_resolve_in_dir_does_not_fall_back_to_builtin_when_empty() {
        // Given: process cwd has cruise.yaml, but resolve_config_in_dir targets a different empty dir
        let process_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            process_dir.path().join("cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let other_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(process_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved against the empty other directory
        let result = resolve_config_in_dir(None, other_dir.path());

        // Then: returns Err (no builtin fallback; process cwd config must not leak)
        assert!(
            result.is_err(),
            "expected Err (no builtin fallback, process cwd should not be used), got Ok"
        );
    }

    // ---- build_config_select_items ----

    #[test]
    fn test_build_config_select_items_with_description() {
        // Given: a YAML file that includes a description
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let path = dir.path().join("team.yaml");
        std::fs::write(
            &path,
            "command: [claude, -p]\ndescription: team-shared\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: building select items
        let items = build_config_select_items(std::slice::from_ref(&path));

        // Then: label is "filename  —  description"
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "team.yaml  —  team-shared");
        assert_eq!(items[0].1, path);
    }

    #[test]
    fn test_build_config_select_items_without_description() {
        // Given: a YAML file without description
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let path = dir.path().join("quick-fix.yml");
        std::fs::write(
            &path,
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: building select items
        let items = build_config_select_items(std::slice::from_ref(&path));

        // Then: label is filename only, no trailing separator
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "quick-fix.yml");
        assert_eq!(items[0].1, path);
    }

    #[test]
    fn test_build_config_select_items_with_broken_yaml() {
        // Given: a file containing malformed YAML
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let path = dir.path().join("broken.yaml");
        std::fs::write(&path, "not: valid: yaml: [unclosed").unwrap_or_else(|e| panic!("{e:?}"));

        // When: building select items
        let items = build_config_select_items(std::slice::from_ref(&path));

        // Then: entry still appears with filename-only label (parse failure -> no description)
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "broken.yaml");
        assert_eq!(items[0].1, path);
    }
}
