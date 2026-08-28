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
    /// Selected from `~/.config/cruise/workflows/`.
    UserDir(PathBuf),
    /// Using the built-in default, either as the fallback or by explicit selection.
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

    /// Returns whether a persisted source string represents the built-in default.
    #[must_use]
    pub fn is_builtin_source(source: &str) -> bool {
        source == crate::new_session_history::BUILTIN_CONFIG_KEY
            || source == "config: (builtin default)"
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
/// Resolve a workflow config from its source, including `workflow_call` and
/// `prompt_file` references, against its source directory.
///
/// File-backed sources are reloaded from their source path; the supplied `yaml` is
/// parsed for the built-in source, where `base_dir` supplies the relative-path base.
///
/// # Errors
///
/// Returns an error when the config cannot be parsed or references cannot be resolved.
pub fn resolve_workflow_config(
    yaml: &str,
    source: &ConfigSource,
    base_dir: &std::path::Path,
) -> Result<crate::config::WorkflowConfig> {
    match source.path() {
        Some(path) => crate::workflow_call::resolve_workflow_calls_from_path(path),
        None => crate::workflow_call::resolve_workflow_calls(
            crate::config::WorkflowConfig::from_yaml(yaml)
                .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?,
            base_dir,
        ),
    }
}

/// Parse and validate a config, resolving `workflow_call` and `prompt_file` references.
///
/// # Errors
///
/// Returns an error when the config cannot be parsed, references cannot be resolved,
/// environment overrides are invalid, or validation fails.
pub fn load_config_from_source(
    yaml: &str,
    source: &ConfigSource,
) -> Result<crate::config::WorkflowConfig> {
    let config = match source.path() {
        Some(_) => resolve_workflow_config(yaml, source, std::path::Path::new("."))?,
        None => resolve_workflow_config(yaml, source, &std::env::current_dir()?)?,
    };
    crate::config::validate_config(&config)?;
    Ok(config)
}

/// Resolve a workflow config, returning (`yaml_content`, source).
///
/// Resolution order:
/// 1. `explicit` (`-c` flag) -- error if file does not exist. The special value
///    `__builtin__` ([`crate::new_session_history::BUILTIN_CONFIG_KEY`]) selects the
///    built-in default config without touching the filesystem.
/// 2. `CRUISE_CONFIG` env var -- error if file does not exist.
/// 3. `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml`.
/// 4. `./.cruise/*.yaml` / `*.yml` (sorted by filename).
/// 5. `~/.config/cruise/workflows/*.yaml` / `*.yml` -- interactive selector includes a trailing
///    "Built-in default" entry so the built-in default can be chosen explicitly.
///
/// # Errors
///
/// Falls back to the embedded built-in workflow when no config file is found.
/// [`CruiseError::ConfigNotFound`] is returned only when an explicitly requested
/// file or `CRUISE_CONFIG` path does not exist.
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
/// 1. `explicit` -- error if file does not exist; `__builtin__` selects the built-in default.
/// 2. `CRUISE_CONFIG` env var -- error if file does not exist.
/// 3. `cruise.yaml` / `cruise.yml` / `.cruise.yaml` / `.cruise.yml` under `cwd`.
/// 4. `.cruise/*.yaml` / `*.yml` under `cwd` (sorted by filename).
/// 5. `~/.config/cruise/workflows/*.yaml` / `*.yml`.
///
/// # Errors
///
/// Falls back to the embedded built-in workflow when no config file is found.
/// [`CruiseError::ConfigNotFound`] is returned only when an explicitly requested
/// file or `CRUISE_CONFIG` path does not exist.
pub fn resolve_config_in_dir(
    explicit: Option<&str>,
    cwd: &std::path::Path,
) -> Result<(String, ConfigSource)> {
    resolve_config_in_dir_with_interactive(explicit, cwd, false)
}

/// A candidate config source for the interactive selector.
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

/// Appends candidates from a YAML directory to `candidates`, resolving each
/// path to absolute and constructing the `CandidateKind` via `kind`.
/// The display label is shortened via `shorten_display_path` (`./` under
/// `cwd`, `~/` under `home`); the stored `PathBuf` stays absolute.
fn push_yaml_dir_candidates(
    candidates: &mut Vec<ConfigCandidate>,
    dir: &std::path::Path,
    cwd: &std::path::Path,
    home: Option<&std::path::Path>,
    kind: impl Fn(PathBuf) -> CandidateKind,
) {
    for file in crate::configs::legacy_user_config_files(dir) {
        let file = to_absolute(file);
        let label = shorten_display_path(&file, cwd, home);
        candidates.push(ConfigCandidate {
            label,
            source: kind(file),
        });
    }
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
    let home = home::home_dir();

    // 1. CRUISE_CONFIG env var — error if set but file missing (same policy as -c).
    if let Some(env_path) = env_val {
        let buf = PathBuf::from(&env_path);
        match std::fs::metadata(&buf) {
            Ok(_) => {
                let abs = to_absolute(buf);
                let label = format!(
                    "CRUISE_CONFIG → {}",
                    shorten_display_path(&abs, cwd, home.as_deref())
                );
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
            let label = shorten_display_path(&abs, cwd, home.as_deref());
            candidates.push(ConfigCandidate {
                label,
                source: CandidateKind::Local(abs),
            });
        }
    }

    // 2.5. Local .cruise/ directory (./.cruise/*.yaml / *.yml), ASCII-sorted.
    // Note: only top-level YAML files in .cruise/ are scanned (non-recursive).
    // Any YAML state file written directly to .cruise/ in the future would be
    // misidentified as a config candidate; avoid placing YAML data files there.
    let local_dir = cwd.join(".cruise");
    if local_dir.is_dir() {
        push_yaml_dir_candidates(
            &mut candidates,
            &local_dir,
            cwd,
            home.as_deref(),
            CandidateKind::Local,
        );
    }

    // 3. User workflow config files (~/.config/cruise/workflows/*.yaml / *.yml), ASCII-sorted.
    // Legacy top-level yaml files left directly in config_dir are no longer read;
    // warn once per process so the user knows to migrate them.
    crate::configs::warn_legacy_user_configs();
    if let Ok(workflows_dir) = crate::paths::workflows_dir() {
        push_yaml_dir_candidates(
            &mut candidates,
            &workflows_dir,
            cwd,
            home.as_deref(),
            CandidateKind::UserDir,
        );
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
        // Built-in sentinel (`__builtin__`): select the built-in default without any
        // filesystem access. Resolved here once so every caller (CLI `-c`, GUI
        // create_session / repo mode / session edit) gets the same behaviour.
        if path == crate::new_session_history::BUILTIN_CONFIG_KEY {
            return Ok((
                crate::config::BUILTIN_CONFIG_YAML.to_string(),
                ConfigSource::Builtin,
            ));
        }
        let buf = PathBuf::from(path);
        let yaml = read_config_file(&buf)?;
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
        || candidates.len() == 1
    {
        // Non-interactive, CRUISE_CONFIG is set, or only the built-in entry remains:
        // take the highest-priority candidate without prompting.
        candidates.into_iter().next().ok_or_else(|| {
            CruiseError::Other("internal error: candidate list was empty".to_string())
        })?
    } else {
        // Interactive: offer all candidates — "Built-in default" is always last, so
        // cursor position 0 still lands on the highest-priority file and Enter-spam
        // keeps selecting it, while the built-in default remains explicitly selectable
        // even when config files exist.
        prompt_select_among_candidates(candidates)?
    };

    // 4. Read the chosen candidate and return.
    materialize_candidate(chosen)
}

/// Convert a `ConfigCandidate` to a `(yaml, ConfigSource)` pair.
fn materialize_candidate(candidate: ConfigCandidate) -> Result<(String, ConfigSource)> {
    let (path, source): (PathBuf, fn(PathBuf) -> ConfigSource) = match candidate.source {
        CandidateKind::EnvVar(path) => (path, ConfigSource::EnvVar),
        CandidateKind::Local(path) => (path, ConfigSource::Local),
        CandidateKind::UserDir(path) => (path, ConfigSource::UserDir),
        CandidateKind::Builtin => {
            return Ok((
                crate::config::BUILTIN_CONFIG_YAML.to_string(),
                ConfigSource::Builtin,
            ));
        }
    };
    read_config_file(&path).map(|yaml| (yaml, source(path)))
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

/// Shorten `path` for interactive display:
/// `./rel` under `cwd`, `~/rel` under `home`, absolute otherwise.
fn shorten_display_path(
    path: &std::path::Path,
    cwd: &std::path::Path,
    home: Option<&std::path::Path>,
) -> String {
    // cwd first: it is usually nested under home, and "./" is the shorter form.
    if let Ok(rel) = path.strip_prefix(cwd)
        && !rel.as_os_str().is_empty()
    {
        return format!("./{}", rel.display());
    }
    // Guard `home.parent()`: when home is the filesystem root, every absolute path would match.
    if let Some(home) = home
        && home.parent().is_some()
        && let Ok(rel) = path.strip_prefix(home)
        && !rel.as_os_str().is_empty()
    {
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
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

    use crate::test_support::{EnvGuard, lock_process};

    fn user_workflows_dir(home: &std::path::Path) -> PathBuf {
        home.join(".config").join("cruise").join("workflows")
    }

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

    #[test]
    fn test_load_config_from_source_resolves_prompt_file_through_explicit_entry_point() {
        let _lock = lock_process();
        // Given: an explicit config file and a prompt file beside it.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let prompt_path = dir.path().join("prompts").join("implement.md");
        std::fs::create_dir_all(
            prompt_path
                .parent()
                .unwrap_or_else(|| panic!("missing parent")),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(&prompt_path, "Implement from the entry point.\n")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let config_path = dir.path().join("cruise.yaml");
        std::fs::write(
            &config_path,
            "command: [echo]\nsteps:\n  implement:\n    prompt_file: prompts/implement.md\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let source = ConfigSource::Explicit(config_path);

        // When: the normal validated config-loading entry point is used.
        let config =
            load_config_from_source("ignored yaml", &source).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: prompt_file is resolved and inlined before validation returns.
        let step = &config.steps["implement"];
        assert_eq!(
            step.prompt.as_deref(),
            Some("Implement from the entry point.\n")
        );
        assert_eq!(step.prompt_file, None);
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

    // ---- resolve_config_in_dir ----

    #[test]
    fn test_resolve_in_dir_local_config_beats_user_dir() {
        // Given: a repo directory has cruise.yaml, and ~/.config/cruise/workflows/default.yaml also exists
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            repo_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let workflows_dir = user_workflows_dir(fake_home.path());
        std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            workflows_dir.join("default.yaml"),
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
        // Given: repo dir has no local config; home has exactly one ~/.config/cruise/workflows/*.yaml
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let workflows_dir = user_workflows_dir(fake_home.path());
        std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            workflows_dir.join("myconf.yaml"),
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

        // Then: falls back to a user workflow config, not builtin
        assert!(
            yaml.contains("userdir"),
            "expected userdir config, got: {yaml}"
        );
        assert!(
            matches!(source, ConfigSource::UserDir(_)),
            "expected UserDir, got: {source:?}"
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
    fn test_resolve_in_dir_ignores_legacy_and_prefers_workflows() {
        // Given: no local config and a legacy top-level user workflow
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = fake_home.path().join(".config").join("cruise");
        std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("legacy.yaml"),
            "command: [legacy]\nsteps:\n  s:\n    command: legacy",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: only the legacy file exists
        let (yaml, source) = resolve_config_in_dir_with_interactive(None, repo_dir.path(), false)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: it is ignored in favor of the built-in default
        assert!(matches!(source, ConfigSource::Builtin));
        assert_eq!(yaml, crate::config::BUILTIN_CONFIG_YAML);

        // And when a workflow is added, it becomes the user-dir candidate
        let workflows_dir = cruise_dir.join("workflows");
        std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            workflows_dir.join("migrated.yaml"),
            "command: [migrated]\nsteps:\n  s:\n    command: migrated",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let (yaml, source) = resolve_config_in_dir_with_interactive(None, repo_dir.path(), false)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("migrated"));
        assert!(
            matches!(source, ConfigSource::UserDir(path) if path == workflows_dir.join("migrated.yaml"))
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
        let workflows_dir = user_workflows_dir(fake_home.path());
        std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(workflows_dir.join("b.yaml"), "").unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(workflows_dir.join("a.yaml"), "").unwrap_or_else(|e| panic!("{e:?}"));
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
        let workflows_dir = user_workflows_dir(fake_home.path());
        std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            workflows_dir.join("b.yaml"),
            "command: [beta]\nsteps:\n  s:\n    command: beta",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            workflows_dir.join("a.yaml"),
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
    fn test_nothing_returns_builtin_without_prompt() {
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        for interactive in [false, true] {
            let (yaml, source) =
                resolve_config_in_dir_with_interactive(None, repo_dir.path(), interactive)
                    .unwrap_or_else(|e| panic!("{e:?}"));
            assert!(matches!(source, ConfigSource::Builtin));
            assert_eq!(yaml, crate::config::BUILTIN_CONFIG_YAML);
        }
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

    // ---- explicit __builtin__ sentinel (Built-in default selection) ----

    #[test]
    fn test_explicit_builtin_sentinel_wins_over_files_and_env() {
        let env_file = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            env_file.path(),
            "command: [envvar]\nsteps:\n  s:\n    command: envvar",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let env_path = env_file
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"))
            .to_string();
        let repo_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            repo_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _env_guard = EnvGuard::set("CRUISE_CONFIG", std::ffi::OsStr::new(&env_path));

        for interactive in [false, true] {
            let (yaml, source) = resolve_config_in_dir_with_interactive(
                Some(crate::new_session_history::BUILTIN_CONFIG_KEY),
                repo_dir.path(),
                interactive,
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(yaml, crate::config::BUILTIN_CONFIG_YAML);
            assert!(matches!(source, ConfigSource::Builtin));
        }
    }

    // ---- builtin roundtrip ----

    // ---- .cruise/ directory as local config source ----

    #[test]
    fn test_resolve_local_cruise_dir_yaml() {
        // Given: cwd has only .cruise/foo.yaml; no top-level cruise.yaml etc.
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = tmp_dir.path().join(".cruise");
        std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("foo.yaml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved non-interactively against that cwd
        let (yaml, source) = resolve_config_in_dir_with_interactive(None, tmp_dir.path(), false)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: .cruise/foo.yaml is selected as a Local source
        assert!(
            yaml.contains("echo"),
            "expected foo.yaml content, got: {yaml}"
        );
        assert!(
            matches!(source, ConfigSource::Local(_)),
            "expected Local, got: {source:?}"
        );
        if let ConfigSource::Local(p) = source {
            assert_eq!(
                p,
                cruise_dir.join("foo.yaml"),
                "resolved path must point to .cruise/foo.yaml"
            );
        }
    }

    #[test]
    fn test_collect_candidates_local_dir_after_single_files_before_userdir() {
        // Given: cruise.yaml (top-level), .cruise/team.yaml, and ~/.config/cruise/workflows/global.yaml
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [top]\nsteps:\n  s:\n    command: top",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = tmp_dir.path().join(".cruise");
        std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("team.yaml"),
            "command: [team]\nsteps:\n  s:\n    command: team",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let workflows_dir = user_workflows_dir(fake_home.path());
        std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            workflows_dir.join("global.yaml"),
            "command: [global]\nsteps:\n  s:\n    command: global",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: candidates are collected
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: Local(cruise.yaml) < Local(.cruise/team.yaml) < UserDir(global.yaml) < Builtin
        let local: Vec<&ConfigCandidate> = candidates
            .iter()
            .filter(|c| matches!(c.source, CandidateKind::Local(_)))
            .collect();
        let user_dir: Vec<&ConfigCandidate> = candidates
            .iter()
            .filter(|c| matches!(c.source, CandidateKind::UserDir(_)))
            .collect();

        assert!(!local.is_empty(), "expected at least one Local candidate");
        assert!(
            !user_dir.is_empty(),
            "expected at least one UserDir candidate"
        );

        // top-level cruise.yaml must appear before .cruise/team.yaml
        let top_idx = candidates
            .iter()
            .position(|c| match &c.source {
                CandidateKind::Local(p) => p.file_name().unwrap_or_default() == "cruise.yaml",
                _ => false,
            })
            .unwrap_or_else(|| panic!("cruise.yaml candidate not found"));
        let team_idx = candidates
            .iter()
            .position(|c| match &c.source {
                CandidateKind::Local(p) => p.file_name().unwrap_or_default() == "team.yaml",
                _ => false,
            })
            .unwrap_or_else(|| panic!(".cruise/team.yaml candidate not found"));
        let user_idx = candidates
            .iter()
            .position(|c| matches!(c.source, CandidateKind::UserDir(_)))
            .unwrap_or_else(|| panic!("UserDir candidate not found"));

        assert!(
            top_idx < team_idx,
            "cruise.yaml (idx {top_idx}) must precede .cruise/team.yaml (idx {team_idx})"
        );
        assert!(
            team_idx < user_idx,
            ".cruise/team.yaml (idx {team_idx}) must precede user-dir (idx {user_idx})"
        );
    }

    #[test]
    fn test_collect_candidates_local_dir_multiple_files_ascii_sorted() {
        // Given: .cruise/ has b.yaml and a.yml (insertion order is arbitrary)
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = tmp_dir.path().join(".cruise");
        std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("b.yaml"),
            "command: [beta]\nsteps:\n  s:\n    command: beta",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("a.yml"),
            "command: [alpha]\nsteps:\n  s:\n    command: alpha",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: candidates are collected
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: .cruise/ candidates are ASCII-sorted (a.yml before b.yaml)
        let dir_locals: Vec<&ConfigCandidate> = candidates
            .iter()
            .filter(|c| match &c.source {
                CandidateKind::Local(p) => p
                    .parent()
                    .and_then(|d| d.file_name())
                    .is_some_and(|n| n == ".cruise"),
                _ => false,
            })
            .collect();

        assert_eq!(
            dir_locals.len(),
            2,
            "expected 2 .cruise/ candidates, got {dir_locals:?}"
        );
        let first_name = match &dir_locals[0].source {
            CandidateKind::Local(p) => p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            _ => panic!("expected Local"),
        };
        assert_eq!(
            first_name, "a.yml",
            ".cruise/ candidates must be ASCII-sorted (a.yml before b.yaml)"
        );
    }

    #[test]
    fn test_interactive_false_local_dir_picks_ascii_first() {
        // Given: .cruise/ has b.yaml and a.yaml; no top-level cruise.yaml; interactive=false
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = tmp_dir.path().join(".cruise");
        std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("b.yaml"),
            "command: [beta]\nsteps:\n  s:\n    command: beta",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("a.yaml"),
            "command: [alpha]\nsteps:\n  s:\n    command: alpha",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let _env_guard = EnvGuard::remove("CRUISE_CONFIG");

        // When: resolved non-interactively
        let (yaml, source) = resolve_config_in_dir_with_interactive(None, tmp_dir.path(), false)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: ASCII-first file (a.yaml) is selected without prompting
        assert!(
            yaml.contains("alpha"),
            "expected a.yaml (alpha) content, got: {yaml}"
        );
        if let ConfigSource::Local(ref p) = source {
            assert_eq!(
                p.file_name().unwrap_or_default().to_str().unwrap_or(""),
                "a.yaml",
                "must pick ASCII-first .cruise/ file when non-interactive"
            );
        } else {
            panic!("expected Local, got: {source:?}");
        }
    }

    // ---- shorten_display_path ----

    #[test]
    fn test_shorten_display_path_direct_child_of_cwd() {
        // Given: a file directly under cwd
        let cwd = std::path::Path::new("/p");
        let home = std::path::Path::new("/home/u");

        // When: shortened
        let label = shorten_display_path(&cwd.join("cruise.yaml"), cwd, Some(home));

        // Then: ./ prefix
        assert_eq!(label, "./cruise.yaml");
    }

    #[test]
    fn test_shorten_display_path_nested_under_cwd() {
        // Given: a file nested under cwd
        let cwd = std::path::Path::new("/p");
        let home = std::path::Path::new("/home/u");

        // When: shortened
        let label = shorten_display_path(&cwd.join(".cruise").join("t.yaml"), cwd, Some(home));

        // Then: ./ prefix with nested relative path
        let expected = format!(
            "./{}",
            std::path::Path::new(".cruise").join("t.yaml").display()
        );
        assert_eq!(label, expected);
    }

    #[test]
    fn test_shorten_display_path_under_home() {
        // Given: a file under home but not under cwd
        let cwd = std::path::Path::new("/p");
        let home = std::path::Path::new("/home/u");

        // When: shortened
        let label = shorten_display_path(
            &home.join(".config").join("cruise").join("a.yaml"),
            cwd,
            Some(home),
        );

        // Then: ~/ prefix
        let expected = format!(
            "~/{}",
            std::path::Path::new(".config")
                .join("cruise")
                .join("a.yaml")
                .display()
        );
        assert_eq!(label, expected);
    }

    #[test]
    fn test_shorten_display_path_cwd_takes_priority_over_home() {
        // Given: cwd is inside home and path is under both
        let home = std::path::Path::new("/home/u");
        let cwd = home.join("proj");
        let path = cwd.join("cruise.yaml");

        // When: shortened
        let label = shorten_display_path(&path, &cwd, Some(home));

        // Then: shorter ./ form wins over ~/
        assert_eq!(label, "./cruise.yaml");
    }

    #[test]
    fn test_shorten_display_path_outside_cwd_and_home_stays_absolute() {
        // Given: a file under neither cwd nor home
        let cwd = std::path::Path::new("/p");
        let home = std::path::Path::new("/home/u");
        let path = std::path::Path::new("/opt/shared/x.yaml");

        // When: shortened
        let label = shorten_display_path(path, cwd, Some(home));

        // Then: absolute path unchanged
        assert_eq!(label, "/opt/shared/x.yaml");
    }

    #[test]
    fn test_shorten_display_path_root_home_is_not_shortened() {
        // Given: home is the filesystem root (would match every absolute path)
        let cwd = std::path::Path::new("/p");
        let home = std::path::Path::new("/");
        let path = std::path::Path::new("/opt/shared/x.yaml");

        // When: shortened
        let label = shorten_display_path(path, cwd, Some(home));

        // Then: guard prevents ~//...; absolute path unchanged
        assert_eq!(label, "/opt/shared/x.yaml");
    }

    #[test]
    fn test_shorten_display_path_none_home_only_shortens_cwd() {
        // Given: home is unavailable (None)
        let cwd = std::path::Path::new("/p");

        // When: a path under cwd and a path elsewhere are shortened
        let under_cwd = shorten_display_path(&cwd.join("cruise.yaml"), cwd, None);
        let elsewhere = shorten_display_path(std::path::Path::new("/home/u/a.yaml"), cwd, None);

        // Then: cwd shortening still applies; other paths stay absolute
        assert_eq!(under_cwd, "./cruise.yaml");
        assert_eq!(elsewhere, "/home/u/a.yaml");
    }

    #[test]
    fn test_shorten_display_path_equal_to_cwd_or_home_not_shortened() {
        // Given: path equals cwd itself (empty relative remainder)
        let cwd = std::path::Path::new("/p");
        let home = std::path::Path::new("/home/u");

        // When: shortened
        let same_as_cwd = shorten_display_path(cwd, cwd, Some(home));
        let same_as_home = shorten_display_path(home, cwd, Some(home));

        // Then: guard prevents bare "./" or "~/"; absolute path unchanged
        assert_eq!(same_as_cwd, "/p");
        assert_eq!(same_as_home, "/home/u");
    }

    // ---- collect_candidates label shortening ----

    #[test]
    fn test_collect_candidates_local_label_is_dot_slash() {
        // Given: cwd has cruise.yaml; no other config
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            tmp_dir.path().join("cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    command: echo",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: local candidate label is exactly "./cruise.yaml"
        let local = candidates
            .iter()
            .find(|c| matches!(c.source, CandidateKind::Local(_)))
            .unwrap_or_else(|| panic!("expected Local candidate"));
        assert_eq!(local.label, "./cruise.yaml");
    }

    #[test]
    fn test_collect_candidates_local_dir_label_is_dot_slash_nested() {
        // Given: cwd has .cruise/team.yaml; no top-level cruise.yaml
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = tmp_dir.path().join(".cruise");
        std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(
            cruise_dir.join("team.yaml"),
            "command: [team]\nsteps:\n  s:\n    command: team",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: .cruise/ candidate label is "./.cruise/team.yaml"
        let local = candidates
            .iter()
            .find(|c| matches!(c.source, CandidateKind::Local(_)))
            .unwrap_or_else(|| panic!("expected Local candidate"));
        let expected = format!(
            "./{}",
            std::path::Path::new(".cruise").join("team.yaml").display()
        );
        assert_eq!(local.label, expected);
    }

    #[test]
    fn test_collect_candidates_user_dir_label_starts_with_tilde() {
        // Given: no local config; `.config/cruise/workflows/a.yaml` exists
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let workflows_dir = user_workflows_dir(fake_home.path());
        std::fs::create_dir_all(&workflows_dir).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(workflows_dir.join("a.yaml"), "").unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected
        let candidates =
            collect_candidates(tmp_dir.path(), None).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: user-dir candidate label starts with "~/" and keeps the filename
        let user_dir = candidates
            .iter()
            .find(|c| matches!(c.source, CandidateKind::UserDir(_)))
            .unwrap_or_else(|| panic!("expected UserDir candidate"));
        assert!(
            user_dir.label.starts_with("~/"),
            "user-dir label must start with '~/', got: {}",
            user_dir.label
        );
        assert!(
            user_dir.label.ends_with("a.yaml"),
            "user-dir label must keep the filename, got: {}",
            user_dir.label
        );
    }

    #[test]
    fn test_collect_candidates_env_label_is_shortened() {
        // Given: CRUISE_CONFIG points at a file inside cwd
        let tmp_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let env_file = tmp_dir.path().join("env.yaml");
        std::fs::write(&env_file, "command: [echo]").unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let _guard = DirGuard::new();
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());

        // When: candidates collected
        let env_val = env_file
            .to_str()
            .unwrap_or_else(|| panic!("non-UTF8"))
            .to_string();
        let candidates =
            collect_candidates(tmp_dir.path(), Some(env_val)).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: env candidate label uses the shortened "./rel" form, not the absolute path
        let env_candidate = candidates
            .iter()
            .find(|c| matches!(c.source, CandidateKind::EnvVar(_)))
            .unwrap_or_else(|| panic!("expected EnvVar candidate"));
        assert_eq!(env_candidate.label, "CRUISE_CONFIG → ./env.yaml");
    }
}
