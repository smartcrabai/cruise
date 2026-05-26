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
    /// Selected from `~/.config/cruise/`.
    UserDir(PathBuf),
    /// No file found; using built-in default.
    Builtin,
}

impl ConfigSource {
    #[must_use]
    pub fn display_string(&self) -> String {
        match self {
            Self::Builtin => "config: (builtin default)".to_string(),
            Self::Explicit(p) | Self::EnvVar(p) | Self::Local(p) | Self::UserDir(p) => {
                format!("config: {}", p.display())
            }
        }
    }

    /// Returns the path to the config file, or `None` for the built-in default.
    #[must_use]
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            Self::Explicit(p) | Self::EnvVar(p) | Self::Local(p) | Self::UserDir(p) => Some(p),
            Self::Builtin => None,
        }
    }
}

/// Resolve a workflow config, returning (`yaml_content`, source).
///
/// Resolution order:
/// 1. `explicit` (`-c` flag) -- error if file does not exist.
/// 2. `CRUISE_CONFIG` env var -- error if file does not exist.
/// 3. `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml`.
/// 4. `~/.cruise/*.yaml` / `*.yml` -- auto-select if exactly one, else prompt.
/// 5. Built-in default.
///
/// # Errors
///
/// Returns an error if an explicitly specified config file is not found or cannot be read.
pub fn resolve_config(explicit: Option<&str>) -> Result<(String, ConfigSource)> {
    use std::io::IsTerminal;
    let cwd = std::env::current_dir()
        .map_err(|e| CruiseError::Other(format!("failed to get current directory: {e}")))?;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        resolve_config_in_dir_with_interactive(explicit, &cwd, true)
    } else {
        resolve_config_in_dir(explicit, &cwd)
    }
}

/// Like [`resolve_config`] but uses `cwd` for local-file discovery instead of the
/// process working directory.
///
/// This is safe to call from concurrent Tauri request handlers because it does not
/// mutate `std::env::current_dir()`.  Resolution order is identical to [`resolve_config`]:
/// 1. `explicit` -- error if file does not exist.
/// 2. `CRUISE_CONFIG` env var -- error if file does not exist.
/// 3. `cruise.yaml` / `cruise.yml` / `.cruise.yaml` / `.cruise.yml` under `cwd`.
/// 4. `~/.cruise/*.yaml` / `*.yml`.
/// 5. Built-in default.
///
/// # Errors
///
/// Returns an error if an explicitly specified config file is not found or cannot be read.
pub fn resolve_config_in_dir(
    explicit: Option<&str>,
    cwd: &std::path::Path,
) -> Result<(String, ConfigSource)> {
    resolve_config_in_dir_with_interactive(explicit, cwd, false)
}

/// A candidate config file for the interactive selector.
#[derive(Debug)]
struct ConfigCandidate {
    label: String,
    source: CandidateKind,
}

impl std::fmt::Display for ConfigCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label)
    }
}

#[derive(Debug)]
enum CandidateKind {
    EnvVar(PathBuf),
    Local(PathBuf),
    UserDir(PathBuf),
    Builtin,
}

/// Collect all candidate config files in priority order.
///
/// `env_val` is the value of `CRUISE_CONFIG` already read from the environment
/// (or `None` if unset). Passing it as a parameter keeps this function testable
/// without mutating the process environment.
///
/// # Errors
///
/// Returns an error if `env_val` is `Some` but the referenced file does not
/// exist or cannot be read — same behaviour as the `Explicit` path.
fn collect_candidates(
    cwd: &std::path::Path,
    env_val: Option<String>,
) -> Result<Vec<ConfigCandidate>> {
    let mut candidates = Vec::new();

    // 1. CRUISE_CONFIG env var — error if set but file missing (same policy as -c).
    if let Some(env_path) = env_val {
        let buf = PathBuf::from(&env_path);
        match std::fs::metadata(&buf) {
            Ok(_) => {
                let abs = to_absolute(buf);
                let label = format!("CRUISE_CONFIG → {}", abs.display());
                candidates.push(ConfigCandidate {
                    label,
                    source: CandidateKind::EnvVar(abs),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CruiseError::ConfigNotFound(env_path));
            }
            Err(e) => {
                return Err(CruiseError::Other(format!(
                    "failed to access '{}': {e}",
                    buf.display()
                )));
            }
        }
    }

    // 2. Local config files in priority order.
    for name in &["cruise.yaml", "cruise.yml", ".cruise.yaml", ".cruise.yml"] {
        let path = cwd.join(name);
        if path.is_file() {
            let abs = to_absolute(path.clone());
            let label = format!("{name} ({})", abs.display());
            candidates.push(ConfigCandidate {
                label,
                source: CandidateKind::Local(abs),
            });
        }
    }

    // 3. User-dir config files (~/.config/cruise/*.yaml / *.yml), ASCII-sorted.
    if let Ok(config_dir) = crate::paths::config_dir() {
        for file in collect_yaml_files(&config_dir) {
            let filename = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let label = format!("{filename} ({})", file.display());
            candidates.push(ConfigCandidate {
                label,
                source: CandidateKind::UserDir(file),
            });
        }
    }

    // 4. Built-in default — always last.
    candidates.push(ConfigCandidate {
        label: "Built-in default".to_string(),
        source: CandidateKind::Builtin,
    });

    Ok(candidates)
}

/// Core resolution logic parameterised by `interactive`.
///
/// When `interactive` is `false` the first candidate from the priority list is
/// adopted automatically (no prompt is shown). When `true` and there are ≥ 2
/// candidates an `inquire::Select` is presented to the user.
fn resolve_config_in_dir_with_interactive(
    explicit: Option<&str>,
    cwd: &std::path::Path,
    interactive: bool,
) -> Result<(String, ConfigSource)> {
    // 1. Explicit path (-c flag) — highest priority, no prompt regardless of interactive.
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

    // 2. Collect all candidates (env var read here to keep collect_candidates testable).
    let env_val = std::env::var("CRUISE_CONFIG").ok();
    let candidates = collect_candidates(cwd, env_val)?;

    // 3. Pick a candidate.
    let chosen = if !interactive
        || matches!(
            candidates.first().map(|c| &c.source),
            Some(CandidateKind::EnvVar(_))
        )
    {
        // Non-interactive, or CRUISE_CONFIG is set: take the highest-priority candidate.
        candidates.into_iter().next().ok_or_else(|| {
            CruiseError::Other("internal error: candidate list was empty".to_string())
        })?
    } else {
        // Interactive: offer only real files — Builtin is an implicit fallback, not a
        // choice the user should be able to accidentally select (Issue #2).
        let real: Vec<ConfigCandidate> = candidates
            .into_iter()
            .filter(|c| !matches!(c.source, CandidateKind::Builtin))
            .collect();
        if real.is_empty() {
            ConfigCandidate {
                label: "Built-in default".to_string(),
                source: CandidateKind::Builtin,
            }
        } else if real.len() == 1 {
            real.into_iter().next().unwrap()
        } else {
            prompt_select_among_candidates(real)?
        }
    };

    // 4. Read the chosen candidate and return.
    materialize_candidate(chosen)
}

/// Convert a `ConfigCandidate` to a `(yaml, ConfigSource)` pair by reading the file.
fn materialize_candidate(candidate: ConfigCandidate) -> Result<(String, ConfigSource)> {
    match candidate.source {
        CandidateKind::EnvVar(path) => {
            let yaml = read_config_file(&path)?;
            Ok((yaml, ConfigSource::EnvVar(path)))
        }
        CandidateKind::Local(path) => {
            let yaml = read_config_file(&path)?;
            Ok((yaml, ConfigSource::Local(path)))
        }
        CandidateKind::UserDir(path) => {
            let yaml = read_config_file(&path)?;
            Ok((yaml, ConfigSource::UserDir(path)))
        }
        CandidateKind::Builtin => {
            let yaml = serde_yaml::to_string(&WorkflowConfig::default_builtin()).map_err(|e| {
                CruiseError::Other(format!("failed to serialize built-in config: {e}"))
            })?;
            Ok((yaml, ConfigSource::Builtin))
        }
    }
}

fn read_config_file(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CruiseError::ConfigNotFound(path.display().to_string())
        } else {
            CruiseError::Other(format!("failed to read '{}': {e}", path.display()))
        }
    })
}

/// Present an `inquire::Select` of candidates and return the chosen one.
fn prompt_select_among_candidates(candidates: Vec<ConfigCandidate>) -> Result<ConfigCandidate> {
    match inquire::Select::new("Select a workflow config", candidates)
        .with_starting_cursor(0)
        .prompt()
    {
        Ok(candidate) => Ok(candidate),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => Err(CruiseError::Other("config selection cancelled".to_string())),
        Err(e) => Err(CruiseError::Other(e.to_string())),
    }
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

    // ---- builtin fallback ----

    #[test]
    fn test_resolve_builtin_fallback() {
        // Run in a temp dir that has no cruise.yaml and no HOME/.cruise.
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _dir_guard = DirGuard::new();
        std::env::set_current_dir(tmp_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        // Point HOME to an empty dir; also clear XDG_CONFIG_HOME to avoid host config interference.
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _xdg_guard = EnvGuard::remove("XDG_CONFIG_HOME");
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        let (yaml, source) = resolve_config(None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("steps"));
        assert!(matches!(source, ConfigSource::Builtin));
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
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved against a different directory (not the process cwd)
        let (_yaml, source) =
            resolve_config_in_dir(None, other_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the process cwd's cruise.yaml is NOT picked up; falls back to builtin
        assert!(
            matches!(source, ConfigSource::Builtin),
            "expected Builtin (process cwd should be ignored), got: {source:?}"
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

    #[test]
    fn test_resolve_in_dir_falls_back_to_builtin() {
        // Given: repo dir has no local config and home has no ~/.cruise files
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));

        let _dir_guard = DirGuard::new();
        let _home_guard = EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved with nothing available
        let (_yaml, source) =
            resolve_config_in_dir(None, repo_dir.path()).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: falls back to built-in default
        assert!(
            matches!(source, ConfigSource::Builtin),
            "expected Builtin, got: {source:?}"
        );
    }

    // ---- collect_candidates ----

    #[test]
    fn test_collect_candidates_only_builtin_when_nothing_exists() {
        // Given: empty cwd, no env var, no user-dir files
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates are collected with no env var
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: exactly one candidate (Builtin) at the end
        assert_eq!(
            candidates.len(),
            1,
            "expected only Builtin, got {candidates:?}"
        );
        assert!(
            matches!(candidates[0].source, CandidateKind::Builtin),
            "expected Builtin, got {:?}",
            candidates[0].source
        );
    }

    #[test]
    fn test_collect_candidates_builtin_always_last() {
        // Given: local cruise.yaml exists
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected without env var
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: last candidate is always Builtin
        assert!(!candidates.is_empty(), "candidates should not be empty");
        assert!(
            matches!(
                candidates
                    .last()
                    .unwrap_or_else(|| panic!("unexpected empty"))
                    .source,
                CandidateKind::Builtin
            ),
            "last candidate must be Builtin, got: {candidates:?}"
        );
    }

    #[test]
    fn test_collect_candidates_env_is_first() {
        // Given: CRUISE_CONFIG env file exists; local cruise.yaml also exists
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let env_file = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        let env_path = env_file
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"))
            .to_string();
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected with env var path
        let candidates =
            collect_candidates(tmp_dir.path(), Some(env_path)).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: first candidate is EnvVar; a Local candidate follows it
        assert!(
            candidates.len() >= 2,
            "expected at least EnvVar + Local, got {candidates:?}"
        );
        assert!(
            matches!(candidates[0].source, CandidateKind::EnvVar(_)),
            "first candidate must be EnvVar, got: {:?}",
            candidates[0].source
        );
        assert!(
            matches!(candidates[1].source, CandidateKind::Local(_)),
            "second candidate must be Local, got: {:?}",
            candidates[1].source
        );
    }

    #[test]
    fn test_collect_candidates_local_cruise_yaml_before_cruise_yml() {
        // Given: cwd has cruise.yaml and cruise.yml
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [yaml]\nsteps:\n  s:\n    command: yaml",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yml"),
            "command: [yml]\nsteps:\n  s:\n    command: yml",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: cruise.yaml (Local) comes before cruise.yml (Local) in the list
        let local_candidates: Vec<&ConfigCandidate> = candidates
            .iter()
            .filter(|c| matches!(c.source, CandidateKind::Local(_)))
            .collect();
        assert!(
            local_candidates.len() >= 2,
            "expected at least 2 local candidates, got {local_candidates:?}"
        );
        let first_name = match &local_candidates[0].source {
            CandidateKind::Local(p) => p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            _ => panic!("expected Local"),
        };
        assert_eq!(
            first_name, "cruise.yaml",
            "cruise.yaml must precede cruise.yml"
        );
    }

    #[test]
    fn test_collect_candidates_env_missing_file_returns_error() {
        // Given: env_val points to a nonexistent file
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();

        // When: collect_candidates with nonexistent path
        let result =
            collect_candidates(tmp_dir.path(), Some("/nonexistent/cruise.yaml".to_string()));

        // Then: returns an error (same policy as explicit -c flag)
        assert!(
            result.is_err(),
            "expected error for missing env file, got Ok"
        );
    }

    #[test]
    fn test_collect_candidates_user_dir_in_ascii_order() {
        // Given: user-dir has b.yaml and a.yaml; no local config
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let config_cruise = fake_home.path().join(".config").join("cruise");
        std::fs::create_dir_all(&config_cruise).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(config_cruise.join("b.yaml"), "").unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(config_cruise.join("a.yaml"), "").unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: user-dir candidates appear in ASCII filename order (a.yaml before b.yaml)
        let user_dir_candidates: Vec<&ConfigCandidate> = candidates
            .iter()
            .filter(|c| matches!(c.source, CandidateKind::UserDir(_)))
            .collect();
        assert_eq!(
            user_dir_candidates.len(),
            2,
            "expected 2 user-dir candidates, got {user_dir_candidates:?}"
        );
        let first_name = match &user_dir_candidates[0].source {
            CandidateKind::UserDir(p) => p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            _ => panic!("expected UserDir"),
        };
        assert_eq!(
            first_name, "a.yaml",
            "user-dir candidates must be ASCII-sorted"
        );
    }

    #[test]
    fn test_collect_candidates_label_contains_kind_prefix() {
        // Given: env var file exists
        let env_file = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        let env_path = env_file
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"))
            .to_string();
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected
        let candidates =
            collect_candidates(tmp_dir.path(), Some(env_path)).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: env candidate label contains "CRUISE_CONFIG" for disambiguation
        let env_candidate = candidates
            .iter()
            .find(|c| matches!(c.source, CandidateKind::EnvVar(_)))
            .unwrap_or_else(|| panic!("expected EnvVar candidate"));
        assert!(
            env_candidate.label.contains("CRUISE_CONFIG"),
            "env label must include 'CRUISE_CONFIG', got: {}",
            env_candidate.label
        );

        // And: builtin label indicates it is a default/builtin option
        let builtin_candidate = candidates
            .iter()
            .find(|c| matches!(c.source, CandidateKind::Builtin))
            .unwrap_or_else(|| panic!("expected Builtin candidate"));
        let lower = builtin_candidate.label.to_lowercase();
        assert!(
            lower.contains("builtin") || lower.contains("default"),
            "builtin label must indicate it is a default, got: {}",
            builtin_candidate.label
        );
    }

    // ---- resolve_config_in_dir_with_interactive ----

    #[test]
    fn test_interactive_false_cruise_yaml_beats_cruise_yml() {
        // Given: cwd has cruise.yaml and cruise.yml; interactive mode is off
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [yaml]\nsteps:\n  s:\n    command: yaml",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yml"),
            "command: [yml]\nsteps:\n  s:\n    command: yml",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved non-interactively
        let (yaml, source) = resolve_config_in_dir_with_interactive(None, tmp_dir.path(), false)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: cruise.yaml wins over cruise.yml (priority order preserved)
        assert!(
            yaml.contains("yaml") && !yaml.contains("yml\n"),
            "expected cruise.yaml content, got: {yaml}"
        );
        if let ConfigSource::Local(ref p) = source {
            assert_eq!(
                p.file_name().unwrap_or_default().to_str().unwrap_or(""),
                "cruise.yaml",
                "resolved path must be cruise.yaml"
            );
        } else {
            panic!("expected Local, got: {source:?}");
        }
    }

    #[test]
    fn test_interactive_false_user_dir_multiple_files_picks_ascii_first() {
        // Given: no local config; user-dir has b.yaml and a.yaml; interactive mode is off
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let config_cruise = fake_home.path().join(".config").join("cruise");
        std::fs::create_dir_all(&config_cruise).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            config_cruise.join("b.yaml"),
            "command: [beta]\nsteps:\n  s:\n    command: beta",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            config_cruise.join("a.yaml"),
            "command: [alpha]\nsteps:\n  s:\n    command: alpha",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved non-interactively (old code would have prompted and blocked)
        let (yaml, source) = resolve_config_in_dir_with_interactive(None, repo_dir.path(), false)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: ASCII-first file (a.yaml) is selected without prompting
        assert!(
            yaml.contains("alpha"),
            "expected a.yaml (alpha) content, got: {yaml}"
        );
        if let ConfigSource::UserDir(ref p) = source {
            assert_eq!(
                p.file_name().unwrap_or_default().to_str().unwrap_or(""),
                "a.yaml",
                "must pick ASCII-first file when non-interactive"
            );
        } else {
            panic!("expected UserDir, got: {source:?}");
        }
    }

    #[test]
    fn test_interactive_false_nothing_returns_builtin() {
        // Given: empty dir, no env var, no user-dir files; interactive mode is off
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved non-interactively with nothing available
        let (_yaml, source) = resolve_config_in_dir_with_interactive(None, repo_dir.path(), false)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: falls back to built-in default
        assert!(
            matches!(source, ConfigSource::Builtin),
            "expected Builtin, got: {source:?}"
        );
    }

    #[test]
    fn test_interactive_true_explicit_path_bypasses_selector() {
        // Given: interactive=true AND an explicit config path is provided
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
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved with interactive=true but explicit path present
        let (yaml, source) =
            resolve_config_in_dir_with_interactive(Some(&explicit_path), repo_dir.path(), true)
                .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: explicit config is returned; no inquire selector is shown
        assert!(
            yaml.contains("explicit"),
            "expected explicit config content, got: {yaml}"
        );
        assert!(
            matches!(source, ConfigSource::Explicit(_)),
            "expected Explicit, got: {source:?}"
        );
    }

    // ---- builtin roundtrip ----

    #[test]
    fn test_builtin_yaml_roundtrip() {
        use crate::config::WorkflowConfig;
        let original = WorkflowConfig::default_builtin();
        let yaml = serde_yaml::to_string(&original).unwrap_or_else(|e| panic!("{e:?}"));
        let parsed = WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(parsed.steps.len(), original.steps.len());
        assert_eq!(parsed.model, original.model);
        assert_eq!(parsed.plan_model, original.plan_model);
        assert_eq!(parsed.pr_language, original.pr_language);
        assert_eq!(parsed.command, original.command);
        for key in original.steps.keys() {
            assert!(parsed.steps.contains_key(key), "missing step: {key}");
        }
    }
}
