use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::{Provider, pins, record, target};
use crate::error::{CruiseError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Override,
    Managed,
    Path,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub program: PathBuf,
    pub source: Source,
}
impl Resolved {
    #[must_use]
    pub fn explicit_path(&self) -> Option<PathBuf> {
        (self.source != Source::Path).then(|| self.program.clone())
    }
    #[must_use]
    pub fn is_managed(&self) -> bool {
        self.source == Source::Managed
    }
}

/// Resolves a provider using local state without installing or executing it.
///
/// # Errors
/// Returns an error if an override is invalid or no usable provider is found.
pub fn resolve(provider: Provider) -> Result<Resolved> {
    resolve_with_path(provider, std::env::var_os("PATH").as_deref(), None)
}

pub(crate) fn resolve_with_path(provider: Provider, path: Option<&OsStr>, cwd: Option<&Path>) -> Result<Resolved> {
    let value = std::env::var_os(provider.env_override());
    let dirs: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).map(|dir| {
            if dir.is_relative() { cwd.unwrap_or_else(|| Path::new(".")).join(dir) } else { dir }
        }).collect())
        .unwrap_or_default();
    let root = crate::paths::data_dir().ok();
    let target = target::detect_target_key(provider).unwrap_or("");
    resolve_optional_root(
        provider, value.as_deref(), root.as_deref(), &dirs, target, true,
    )
}

pub(crate) fn resolve_in(
    provider: Provider,
    env_override: Option<&OsStr>,
    data_root: &Path,
    path_dirs: &[PathBuf],
    target_key: &str,
) -> Result<Resolved> {
    resolve_in_impl(
        provider,
        env_override,
        data_root,
        path_dirs,
        target_key,
        true,
    )
}

pub(crate) fn resolve_in_impl(
    provider: Provider,
    env_override: Option<&OsStr>,
    data_root: &Path,
    path_dirs: &[PathBuf],
    target_key: &str,
    warn: bool,
) -> Result<Resolved> {
    resolve_optional_root(provider, env_override, Some(data_root), path_dirs, target_key, warn)
}

fn resolve_optional_root(
    provider: Provider,
    env_override: Option<&OsStr>,
    data_root: Option<&Path>,
    path_dirs: &[PathBuf],
    target_key: &str,
    warn: bool,
) -> Result<Resolved> {
    if let Some(value) = env_override.filter(|v| !v.is_empty()) {
        let program = PathBuf::from(value);
        if !program.is_file() {
            return Err(CruiseError::Other(format!(
                "{} points to '{}', which is not an existing regular file",
                provider.env_override(),
                program.display()
            )));
        }
        return Ok(Resolved {
            program: if program.is_absolute() {
                program
            } else {
                std::env::current_dir()?.join(program)
            },
            source: Source::Override,
        });
    }
    if let Some(data_root) = data_root {
        let dir = data_root.join("providers").join(provider.as_str());
        let record_path = dir.join("install.json");
        match record::read_record(&record_path) {
            Ok(rec) if record::record_valid(provider, pins::pin(provider), target_key, &dir, &rec) => {
                return Ok(Resolved {
                    program: dir.join(rec.relative_path),
                    source: Source::Managed,
                });
            }
            Err(CruiseError::IoError(ref error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ if warn => warn_invalid(provider),
            _ => {}
        }
    }
    if let Some(program) = path_dirs
        .iter()
        .map(|dir| dir.join(provider.binary_name()))
        .find(|path| path.is_file())
    {
        return Ok(Resolved {
            program,
            source: Source::Path,
        });
    }
    Err(CruiseError::Other(format!(
        "{} is not installed: no managed install and no {} on PATH. Run `cruise install {}` or set {}.",
        provider.as_str(),
        provider.as_str(),
        provider.as_str(),
        provider.env_override()
    )))
}

fn warn_invalid(provider: Provider) {
    static WARNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut warned = WARNED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if warned.insert(provider.as_str()) {
        eprintln!(
            "cruise: managed {} does not match this cruise's pinned {} (or is damaged); checking PATH for {}. Run `cruise install {}`.",
            provider.as_str(),
            pins::pin(provider).version,
            provider.as_str(),
            provider.as_str()
        );
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "isolated resolution fixtures")]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unavailable_root_keeps_override_and_path_fallback() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join(Provider::Jcode.binary_name());
        fs::write(&binary, b"fixture").unwrap();
        let resolve = |value: Option<&OsStr>, dirs: &[PathBuf]| {
            resolve_optional_root(Provider::Jcode, value, None, dirs, "", false)
        };
        assert_eq!(resolve(Some(binary.as_os_str()), &[]).unwrap().source, Source::Override);
        assert_eq!(resolve(None, &[root.path().to_path_buf()]).unwrap().source, Source::Path);
        assert!(resolve(None, &[]).unwrap_err().to_string().contains("cruise install jcode"));
    }

    #[test]
    fn relative_override_becomes_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let temp = tempfile::Builder::new().prefix(".cruise-override-test-").tempdir_in(&cwd).unwrap();
        let binary = temp.path().join("provider");
        fs::write(&binary, b"fixture").unwrap();
        let relative = binary.strip_prefix(&cwd).unwrap();
        let resolved = resolve_optional_root(Provider::Jcode, Some(relative.as_os_str()), None, &[], "", false).unwrap();
        assert_eq!(resolved.program, binary);
    }

    #[test]
    fn override_beats_valid_managed_and_invalid_records_fall_back() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("providers/jcode");
        let version = pins::pin(Provider::Jcode).version;
        let binary = dir.join(version).join(Provider::Jcode.binary_name());
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"managed").unwrap();
        let stamp = record::stamp(&binary).unwrap();
        let base = serde_json::json!({
            "schema": 1, "provider": "jcode", "version": version,
            "target": "macos-aarch64", "relative_path": format!("{version}/{}", Provider::Jcode.binary_name()),
            "sha256": "ignored-for-resolution", "len": stamp.len,
            "mtime_unix_nanos": stamp.mtime_unix_nanos, "source_url": "https://example.invalid"
        });
        let record_path = dir.join("install.json");
        let path_dir = root.path().join("bin");
        fs::create_dir(&path_dir).unwrap();
        fs::write(path_dir.join(Provider::Jcode.binary_name()), b"path").unwrap();
        let dirs = [path_dir];
        let explicit = root.path().join("override");
        fs::write(&explicit, b"override").unwrap();
        fs::write(&record_path, serde_json::to_vec(&base).unwrap()).unwrap();
        let resolve = |value: Option<&OsStr>| {
            resolve_in_impl(
                Provider::Jcode,
                value,
                root.path(),
                &dirs,
                "macos-aarch64",
                false,
            )
        };
        assert_eq!(
            resolve(Some(explicit.as_os_str())).unwrap().source,
            Source::Override
        );
        assert_eq!(
            resolve(Some(OsStr::new(""))).unwrap().source,
            Source::Managed
        );
        let bad = root.path().join("missing");
        let message = resolve(Some(bad.as_os_str())).unwrap_err().to_string();
        assert!(message.contains(Provider::Jcode.env_override()));
        assert!(message.contains(&bad.display().to_string()));

        for (field, value) in [
            ("schema", serde_json::json!(2)),
            ("provider", serde_json::json!("claude")),
            ("version", serde_json::json!("0.0.1")),
            ("target", serde_json::json!("wrong-target")),
            ("relative_path", serde_json::json!("../escape")),
            ("relative_path", serde_json::json!("/absolute/path")),
            ("len", serde_json::json!(stamp.len + 1)),
            (
                "mtime_unix_nanos",
                serde_json::json!(stamp.mtime_unix_nanos + 1),
            ),
        ] {
            let mut invalid = base.clone();
            invalid[field] = value;
            fs::write(&record_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert_eq!(
                resolve(None).unwrap().source,
                Source::Path,
                "invalid {field}"
            );
        }
        fs::write(&record_path, b"{invalid json").unwrap();
        assert_eq!(resolve(None).unwrap().source, Source::Path);
    }

    #[test]
    fn missing_or_directory_path_is_not_a_binary() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bin");
        fs::create_dir_all(path.join(Provider::Claude.binary_name())).unwrap();
        let err = resolve_in(Provider::Claude, None, root.path(), &[path], "darwin-arm64")
            .unwrap_err()
            .to_string();
        assert!(err.contains("claude is not installed"));
        assert!(err.contains("cruise install claude"));
        assert!(err.contains("CRUISE_CLAUDE_CLI_PATH"));
    }

    #[test]
    fn override_precedes_path_and_invalid_override_is_error() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bin");
        fs::create_dir(&path).unwrap();
        fs::write(path.join(Provider::Jcode.binary_name()), b"path").unwrap();
        let explicit = root.path().join("explicit");
        fs::write(&explicit, b"override").unwrap();
        let dirs = [path];
        let resolved = resolve_in(
            Provider::Jcode,
            Some(explicit.as_os_str()),
            root.path(),
            &dirs,
            "macos-aarch64",
        )
        .unwrap();
        assert_eq!(resolved.source, Source::Override);
        assert_eq!(resolved.explicit_path(), Some(explicit));
        let bad = root.path().join("missing");
        assert!(
            resolve_in(
                Provider::Jcode,
                Some(bad.as_os_str()),
                root.path(),
                &dirs,
                "macos-aarch64"
            )
            .is_err()
        );
        let fallback = resolve_in(
            Provider::Jcode,
            Some(OsStr::new("")),
            root.path(),
            &dirs,
            "macos-aarch64",
        )
        .unwrap();
        assert_eq!(fallback.source, Source::Path);
        assert_eq!(fallback.explicit_path(), None);
    }

    #[test]
    fn valid_record_and_damage_follow_precedence() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("providers/jcode");
        let binary = dir
            .join(pins::pin(Provider::Jcode).version)
            .join(Provider::Jcode.binary_name());
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"managed").unwrap();
        let stamp = record::stamp(&binary).unwrap();
        let rec = record::InstallRecord {
            schema: 1,
            provider: "jcode".into(),
            version: pins::pin(Provider::Jcode).version.into(),
            target: "macos-aarch64".into(),
            relative_path: format!(
                "{}/{}",
                pins::pin(Provider::Jcode).version,
                Provider::Jcode.binary_name()
            ),
            sha256: String::new(),
            len: stamp.len,
            mtime_unix_nanos: stamp.mtime_unix_nanos,
            source_url: String::new(),
        };
        fs::write(dir.join("install.json"), serde_json::to_vec(&rec).unwrap()).unwrap();
        let path = root.path().join("bin");
        fs::create_dir(&path).unwrap();
        fs::write(path.join(Provider::Jcode.binary_name()), b"path").unwrap();
        let dirs = [path];
        let resolved = resolve_in_impl(
            Provider::Jcode,
            None,
            root.path(),
            &dirs,
            "macos-aarch64",
            false,
        )
        .unwrap();
        assert_eq!(resolved.program, binary);
        assert!(resolved.is_managed());
        fs::write(&binary, b"damaged binary").unwrap();
        assert_eq!(
            resolve_in_impl(
                Provider::Jcode,
                None,
                root.path(),
                &dirs,
                "macos-aarch64",
                false
            )
            .unwrap()
            .source,
            Source::Path
        );
    }
}
