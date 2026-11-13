#[cfg(test)]
mod archive_tests;
mod fetch;
#[cfg(all(test, unix))]
#[path = "tests.rs"]
#[expect(clippy::unwrap_used, reason = "test fixtures must succeed")]
mod lifecycle_tests;
mod record;
pub mod release;
pub mod resolve;
pub mod target;
use crate::error::{CruiseError, Result};
use fs2::FileExt;
pub use resolve::{Resolved, Source, resolve};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[cfg(test)]
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum Provider {
    Jcode,
    Claude,
    #[value(name = "opencode")]
    OpenCode,
}
impl Provider {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jcode => "jcode",
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
        }
    }
    #[must_use]
    pub fn binary_name(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Self::Jcode, true) => "jcode.exe",
            (Self::Claude, true) => "claude.exe",
            (Self::OpenCode, true) => "opencode.exe",
            (Self::Jcode, false) => "jcode",
            (Self::Claude, false) => "claude",
            (Self::OpenCode, false) => "opencode",
        }
    }
    #[must_use]
    pub fn env_override(self) -> &'static str {
        match self {
            Self::Jcode => "CRUISE_JCODE_CLI_PATH",
            Self::Claude => "CRUISE_CLAUDE_CLI_PATH",
            Self::OpenCode => "CRUISE_OPENCODE_CLI_PATH",
        }
    }
}
impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
#[derive(Debug)]
pub enum InstallOutcome {
    Installed { version: String },
    AlreadyInstalled { version: String },
    Reinstalled { version: String, reason: String },
}
fn hosts(p: Provider) -> &'static [&'static str] {
    match p {
        Provider::Jcode => &[
            "github.com",
            "release-assets.githubusercontent.com",
            "objects.githubusercontent.com",
            "github-cloud.s3.amazonaws.com",
        ],
        Provider::Claude => &["downloads.claude.ai"],
        Provider::OpenCode => &["registry.npmjs.org"],
    }
}
/// Install the latest stable provider build.
///
/// # Errors
/// Returns an error if the target is unsupported or resolving, downloading, verifying, or installing fails.
pub fn install(provider: Provider, force: bool) -> Result<InstallOutcome> {
    #[cfg(test)]
    {
        install_in(
            provider,
            force,
            &crate::paths::data_dir()?,
            |key| release::latest(provider, key),
            hosts(provider),
            None,
        )
    }
    #[cfg(not(test))]
    {
        install_in(
            provider,
            force,
            &crate::paths::data_dir()?,
            |key| release::latest(provider, key),
            hosts(provider),
        )
    }
}
pub(crate) fn install_in(
    provider: Provider,
    force: bool,
    data_root: &Path,
    resolve_latest: impl FnOnce(&'static str) -> Result<release::Release>,
    trust_hosts: &[&str],
    #[cfg(test)] source_base: Option<&Url>,
) -> Result<InstallOutcome> {
    let key = target::detect_target_key(provider)?;
    let root = data_root.join("providers");
    let dir = root.join(provider.as_str());
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".install.lock"))?;
    lock.lock_exclusive()?;
    let selected = resolve_latest(key)?;
    if selected.target.key != key
        || !semver::Version::parse(&selected.version).is_ok_and(|version| version.pre.is_empty())
    {
        return Err(CruiseError::Other(
            "latest provider release has an invalid version or target".into(),
        ));
    }
    selected.target.digest.validate()?;
    let record_path = dir.join("install.json");
    for item in fs::read_dir(&dir)? {
        let item = item?;
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".staging-") {
            let _ = fs::remove_dir_all(item.path());
        } else if name.starts_with(".record-") {
            let _ = fs::remove_file(item.path());
        }
    }
    let old = record::read_record(&record_path);
    let reason = match &old {
        Ok(r) if record::record_valid(provider, key, &dir, r) => {
            if record::sha256(&dir.join(&r.relative_path)).is_ok_and(|hash| hash == r.sha256) {
                if r.version == selected.version && !force {
                    return Ok(InstallOutcome::AlreadyInstalled {
                        version: selected.version,
                    });
                }
                if r.version == selected.version {
                    "force"
                } else {
                    "update"
                }
            } else {
                "integrity"
            }
        }
        Ok(_) => "integrity",
        Err(_) if record_path.exists() => "record",
        Err(_) => "missing",
    };
    let staging = dir.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging)?;
    let result = (|| {
        let binary = staging.join(provider.binary_name());
        let version = selected.version;
        #[cfg(test)]
        fetch::fetch_and_verify(&selected.target, &binary, trust_hosts, source_base)?;
        #[cfg(not(test))]
        fetch::fetch_and_verify(&selected.target, &binary, trust_hosts)?;
        fetch::probe(provider, &binary, &version)?;
        let sha256 = record::sha256(&binary)?;
        let stamp = record::stamp(&binary)?;
        let generation = format!("{version}-{}", uuid::Uuid::new_v4());
        let rec = record::InstallRecord {
            schema: 1,
            provider: provider.as_str().into(),
            version: version.clone(),
            target: key.into(),
            relative_path: format!("{generation}/{}", provider.binary_name()),
            sha256,
            len: stamp.len,
            mtime_unix_nanos: stamp.mtime_unix_nanos,
            source_url: selected.target.url,
        };
        publish_install(&dir, &staging, &record_path, &rec)?;
        Ok(if reason == "missing" {
            InstallOutcome::Installed { version }
        } else {
            InstallOutcome::Reinstalled {
                version,
                reason: reason.into(),
            }
        })
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn publish_install(
    dir: &Path,
    staging: &Path,
    record_path: &Path,
    rec: &record::InstallRecord,
) -> Result<()> {
    let final_dir = dir.join(
        Path::new(&rec.relative_path)
            .parent()
            .ok_or_else(|| CruiseError::Other("install path has no parent".into()))?,
    );
    fs::rename(staging, &final_dir)?;
    if let Err(e) = record::write_record_atomic(record_path, rec) {
        let _ = fs::remove_dir_all(&final_dir);
        return Err(e);
    }
    // Readers may still hold an older record or resolved path. Keep old generations.
    Ok(())
}

/// Run provider installation or display provider status.
///
/// # Errors
/// Returns an error if a provider is required or installation or status lookup fails.
pub fn run(provider: Option<Provider>, show_status: bool, force: bool) -> Result<()> {
    if show_status {
        print_status()?;
        return Ok(());
    }
    let provider = provider.ok_or_else(|| CruiseError::Other("provider required".into()))?;
    match install(provider, force)? {
        InstallOutcome::Installed { version } => println!("Installed {provider} {version}"),
        InstallOutcome::AlreadyInstalled { version } => {
            println!("{provider} {version} is already installed");
        }
        InstallOutcome::Reinstalled { version, reason } => {
            println!("Reinstalled {provider} {version} ({reason})");
        }
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedState {
    Ready,
    Damaged,
}

#[derive(Debug)]
pub struct ProviderStatus {
    pub provider: Provider,
    pub version: Option<String>,
    pub managed: Option<ManagedState>,
    pub path: Option<PathBuf>,
    pub override_env: Option<String>,
    pub effective: Result<Source>,
}
/// Return the status of each provider.
///
/// # Errors
/// Returns an error if the data directory cannot be determined.
pub fn status() -> Result<Vec<ProviderStatus>> {
    let root = crate::paths::data_dir()?;
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    Ok(status_in(
        &root,
        |p| std::env::var_os(p.env_override()),
        &dirs,
        |p| target::detect_target_key(p).ok(),
    ))
}
pub(crate) fn status_in(
    data_root: &Path,
    env_override: impl Fn(Provider) -> Option<std::ffi::OsString>,
    dirs: &[PathBuf],
    target_key: impl Fn(Provider) -> Option<&'static str>,
) -> Vec<ProviderStatus> {
    let mut statuses = Vec::new();
    for p in [Provider::Jcode, Provider::Claude, Provider::OpenCode] {
        let dir = data_root.join("providers").join(p.as_str());
        let rec = record::read_record(&dir.join("install.json"));
        let key = target_key(p);
        let (version, managed) = match &rec {
            Ok(r) => (
                Some(r.version.clone()),
                Some(
                    if key.is_some_and(|k| record::record_valid(p, k, &dir, r)) {
                        ManagedState::Ready
                    } else {
                        ManagedState::Damaged
                    },
                ),
            ),
            Err(_) if dir.join("install.json").exists() => (None, Some(ManagedState::Damaged)),
            Err(_) => (None, None),
        };
        let path = dirs
            .iter()
            .map(|d| d.join(p.binary_name()))
            .find(|p| p.is_file());
        let ov = env_override(p).filter(|s| !s.is_empty());
        let effective =
            resolve::resolve_in_impl(p, ov.as_deref(), data_root, dirs, key.unwrap_or(""), false)
                .map(|resolved| resolved.source);
        statuses.push(ProviderStatus {
            provider: p,
            version,
            managed,
            path,
            override_env: ov.map(|value| value.to_string_lossy().into_owned()),
            effective,
        });
    }
    statuses
}
fn print_status() -> Result<()> {
    println!("PROVIDER  VERSION  MANAGED  PATH  OVERRIDE  EFFECTIVE");
    for s in status()? {
        let version = s.version.as_deref().unwrap_or("-");
        let managed = match s.managed {
            Some(ManagedState::Ready) => "ready",
            Some(ManagedState::Damaged) => "damaged",
            None => "missing",
        };
        let effective = match s.effective {
            Ok(Source::Override) => "override",
            Ok(Source::Managed) => "managed",
            Ok(Source::Path) => "path",
            Err(_) if s.override_env.is_some() => "error",
            Err(_) => "missing",
        };
        println!(
            "{}  {}  {}  {}  {}  {}",
            s.provider,
            version,
            managed,
            s.path
                .map_or_else(|| "-".into(), |v| v.display().to_string()),
            s.override_env.unwrap_or_else(|| "-".into()),
            effective
        );
    }
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test fixtures must succeed")]
mod tests {
    use super::*;
    #[test]
    fn status_does_not_create_data_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("absent");
        let statuses = status_in(&root, |_| None, &[], |_| None);
        assert_eq!(statuses.len(), 3);
        assert!(
            statuses
                .iter()
                .all(|s| s.version.is_none() && s.managed.is_none() && s.effective.is_err())
        );
        assert!(!root.exists());
    }
    #[test]
    fn status_reports_typed_sources_and_errors() {
        let temp = tempfile::tempdir().unwrap();
        let path_bin = temp.path().join(Provider::Claude.binary_name());
        fs::write(&path_bin, b"path").unwrap();
        let statuses = status_in(
            temp.path(),
            |p| (p == Provider::Jcode).then(|| temp.path().join("missing").into_os_string()),
            &[temp.path().to_path_buf()],
            |_| Some("test"),
        );
        assert!(statuses[0].override_env.is_some());
        assert!(
            matches!(&statuses[0].effective, Err(CruiseError::Other(message)) if message.contains("CRUISE_JCODE_CLI_PATH"))
        );
        assert_eq!(statuses[1].effective.as_ref().ok(), Some(&Source::Path));
        assert!(statuses[1].managed.is_none());
    }
    #[test]
    fn status_reports_recorded_old_version_as_ready() {
        let temp = tempfile::tempdir().unwrap();
        let provider = Provider::Claude;
        let version = "0.0.1";
        let dir = temp.path().join("providers").join(provider.as_str());
        let version_dir = dir.join(version);
        fs::create_dir_all(&version_dir).unwrap();
        let binary = version_dir.join(provider.binary_name());
        fs::write(&binary, b"binary").unwrap();
        let stamp = record::stamp(&binary).unwrap();
        let rec = record::InstallRecord {
            schema: 1,
            provider: provider.as_str().into(),
            version: version.into(),
            target: "test".into(),
            relative_path: format!("{version}/{}", provider.binary_name()),
            sha256: record::sha256(&binary).unwrap(),
            len: stamp.len,
            mtime_unix_nanos: stamp.mtime_unix_nanos,
            source_url: "https://example.invalid".into(),
        };
        let path = dir.join("install.json");
        record::write_record_atomic(&path, &rec).unwrap();
        let statuses = status_in(temp.path(), |_| None, &[], |_| Some("test"));
        assert_eq!(statuses[1].version.as_deref(), Some(version));
        assert_eq!(statuses[1].managed, Some(ManagedState::Ready));
        assert_eq!(statuses[1].effective.as_ref().ok(), Some(&Source::Managed));
        fs::write(path, b"not json").unwrap();
        let statuses = status_in(temp.path(), |_| None, &[], |_| Some("test"));
        assert_eq!(statuses[1].version, None);
        assert_eq!(statuses[1].managed, Some(ManagedState::Damaged));
    }
    #[test]
    fn record_accepts_legacy_and_generation_paths_but_rejects_unsafe_versions_and_paths() {
        let temp = tempfile::tempdir().unwrap();
        let provider = Provider::Claude;
        let version = "2.1.250";
        let version_dir = temp.path().join(version);
        fs::create_dir(&version_dir).unwrap();
        let binary = version_dir.join(provider.binary_name());
        fs::write(&binary, b"hello").unwrap();
        let stamp = record::stamp(&binary).unwrap();
        let mut rec = record::InstallRecord {
            schema: 1,
            provider: provider.as_str().into(),
            version: version.into(),
            target: "test".into(),
            relative_path: format!("{version}/{}", provider.binary_name()),
            sha256: record::sha256(&binary).unwrap(),
            len: stamp.len,
            mtime_unix_nanos: stamp.mtime_unix_nanos,
            source_url: "https://example.invalid".into(),
        };
        let path = temp.path().join("install.json");
        record::write_record_atomic(&path, &rec).unwrap();
        let read = record::read_record(&path).unwrap();
        assert!(record::record_valid(provider, "test", temp.path(), &read));
        let generation = format!("{version}-00000000-0000-0000-0000-000000000000");
        let generated_binary = temp.path().join(&generation).join(provider.binary_name());
        fs::create_dir_all(generated_binary.parent().unwrap()).unwrap();
        fs::write(&generated_binary, b"hello").unwrap();
        rec.relative_path = format!("{generation}/{}", provider.binary_name());
        let generated_stamp = record::stamp(&generated_binary).unwrap();
        rec.len = generated_stamp.len;
        rec.mtime_unix_nanos = generated_stamp.mtime_unix_nanos;
        assert!(record::record_valid(provider, "test", temp.path(), &rec));
        let prerelease = "2.1.250-rc.1";
        let prerelease_binary = temp.path().join(prerelease).join(provider.binary_name());
        fs::create_dir_all(prerelease_binary.parent().unwrap()).unwrap();
        fs::write(&prerelease_binary, b"hello").unwrap();
        let prerelease_stamp = record::stamp(&prerelease_binary).unwrap();
        rec.version = prerelease.into();
        rec.relative_path = format!("{prerelease}/{}", provider.binary_name());
        rec.len = prerelease_stamp.len;
        rec.mtime_unix_nanos = prerelease_stamp.mtime_unix_nanos;
        assert!(record::record_valid(provider, "test", temp.path(), &rec));
        for (version, relative_path) in [
            ("../outside", "../outside/claude"),
            ("2.1.250", "../outside"),
            ("2.1.250", "/absolute/path"),
            ("2.1.250", "2.1.250-../2.1.250/claude"),
        ] {
            rec.version = version.into();
            rec.relative_path = relative_path.into();
            assert!(
                !record::record_valid(provider, "test", temp.path(), &rec),
                "accepted {version} at {relative_path}"
            );
        }
        fs::write(binary, b"modified").unwrap();
        assert!(!record::record_valid(provider, "test", temp.path(), &read));
    }

    #[test]
    fn failed_record_publication_preserves_previous_binary() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old");
        let staging = temp.path().join(".staging-test");
        fs::create_dir(&old).unwrap();
        fs::write(old.join("claude"), b"old").unwrap();
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("claude"), b"new").unwrap();
        let record_path = temp.path().join("install.json");
        fs::create_dir(&record_path).unwrap();
        let rec = record::InstallRecord {
            schema: 1,
            provider: "claude".into(),
            version: "1.0".into(),
            target: "test".into(),
            relative_path: "new/claude".into(),
            sha256: String::new(),
            len: 3,
            mtime_unix_nanos: 0,
            source_url: String::new(),
        };
        assert!(publish_install(temp.path(), &staging, &record_path, &rec).is_err());
        assert_eq!(fs::read(old.join("claude")).unwrap(), b"old");
        assert!(!temp.path().join("new").exists());
        assert!(record_path.is_dir());
    }
}
