use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::Path;

use super::{WorkflowPackageError, sha256_hex, validate_name};
use serde::{Deserialize, Serialize};

pub(crate) const MANIFEST_FILE: &str = "cruise-package.yaml";
pub(crate) const BUNDLE_LOCK_FILE: &str = ".cruise-package-lock.json";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackageManifest {
    #[serde(default)]
    pub description: Option<String>,
    pub workflows: Vec<ManifestWorkflow>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestWorkflow {
    pub name: String,
    pub path: String,
}

pub(crate) fn valid_path(path: &str) -> bool {
    !path.contains('\\')
        && !path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        && matches!(
            Path::new(path).extension().and_then(|s| s.to_str()),
            Some("yaml" | "yml")
        )
        && !matches!(
            Path::new(path).file_name().and_then(|s| s.to_str()),
            Some("cruise-package.yaml" | "cruise-package.yml")
        )
}
pub(crate) fn parse_manifest(text: &str) -> Result<PackageManifest, WorkflowPackageError> {
    let manifest: PackageManifest = serde_yaml::from_str(text)
        .map_err(|e| WorkflowPackageError::InvalidManifest(e.to_string()))?;
    if manifest.workflows.is_empty() {
        return Err(WorkflowPackageError::InvalidManifest(
            "workflows must not be empty".into(),
        ));
    }
    let mut names = HashSet::new();
    let mut paths = HashSet::new();
    for wf in &manifest.workflows {
        if validate_name(&wf.name).is_err()
            || !names.insert(wf.name.to_ascii_lowercase())
            || !valid_path(&wf.path)
            || !paths.insert(&wf.path)
        {
            return Err(WorkflowPackageError::InvalidManifest(format!(
                "invalid or duplicate workflow: {} ({})",
                wf.name, wf.path
            )));
        }
    }
    Ok(manifest)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundleLock {
    pub name: String,
    pub owner: String,
    pub repo: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub workflows: BTreeMap<String, LockedWorkflow>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LockedWorkflow {
    pub source_path: String,
    pub sha256: String,
}

pub(crate) fn read_lock(dir: &Path, name: &str) -> Result<BundleLock, WorkflowPackageError> {
    read_lock_at(&dir.join(name), name)
}
fn read_lock_at(base: &Path, name: &str) -> Result<BundleLock, WorkflowPackageError> {
    let path = base.join(BUNDLE_LOCK_FILE);
    if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file()) {
        return Err(WorkflowPackageError::NotInstalled(name.into()));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|_| WorkflowPackageError::NotInstalled(name.into()))?;
    let lock: BundleLock =
        serde_json::from_str(&text).map_err(|_| WorkflowPackageError::Modified(name.into()))?;
    let mut names = HashSet::new();
    if lock.name != name
        || lock.workflows.is_empty()
        || lock.workflows.iter().any(|(n, w)| {
            validate_name(n).is_err()
                || !names.insert(n.to_ascii_lowercase())
                || !valid_path(&w.source_path)
        })
    {
        return Err(WorkflowPackageError::Modified(name.into()));
    }
    Ok(lock)
}
pub(crate) fn load_unmodified(dir: &Path, name: &str) -> Result<BundleLock, WorkflowPackageError> {
    load_unmodified_at(&dir.join(name), name)
}
pub(crate) fn load_unmodified_at(
    base: &Path,
    name: &str,
) -> Result<BundleLock, WorkflowPackageError> {
    if !std::fs::symlink_metadata(base).is_ok_and(|m| m.file_type().is_dir()) {
        return Err(WorkflowPackageError::Modified(name.into()));
    }
    let lock = read_lock_at(base, name)?;
    let entries =
        std::fs::read_dir(base).map_err(|_| WorkflowPackageError::Modified(name.into()))?;
    for entry in entries {
        let entry = entry.map_err(|_| WorkflowPackageError::Modified(name.into()))?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !entry
            .file_type()
            .map_err(|_| WorkflowPackageError::Modified(name.into()))?
            .is_file()
            || (file_name != BUNDLE_LOCK_FILE
                && !file_name
                    .strip_suffix(".yaml")
                    .is_some_and(|n| lock.workflows.contains_key(n)))
        {
            return Err(WorkflowPackageError::Modified(name.into()));
        }
    }
    for (n, workflow) in &lock.workflows {
        let path = base.join(format!("{n}.yaml"));
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file())
            || std::fs::read(path).map_or(true, |b| sha256_hex(&b) != workflow.sha256)
        {
            return Err(WorkflowPackageError::Modified(name.into()));
        }
    }
    Ok(lock)
}

pub(crate) fn interrupted_previous(
    dir: &Path,
    name: &str,
) -> Result<Option<std::path::PathBuf>, WorkflowPackageError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(None);
    };
    let mut previous = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!(".{name}.previous-"))
        {
            previous.push(entry.path());
        }
    }
    if previous.len() > 1 {
        return Err(WorkflowPackageError::Interrupted(name.into()));
    }
    Ok(previous.pop())
}

pub(crate) fn recover_bundle(dir: &Path, name: &str) -> Result<(), WorkflowPackageError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut previous = Vec::new();
    let mut cleanup = Vec::new();
    for entry in entries {
        let entry = entry?;
        let filename = entry.file_name().to_string_lossy().into_owned();
        if filename.starts_with(&format!(".{name}.previous-")) {
            previous.push(entry.path());
        } else if filename.starts_with(&format!(".{name}.staging-"))
            || filename.starts_with(&format!(".{name}.removing-"))
        {
            cleanup.push(entry.path());
        }
    }
    let target = dir.join(name);
    if !target.exists() && previous.len() > 1 {
        return Err(WorkflowPackageError::Interrupted(name.into()));
    }
    for path in cleanup {
        remove_entry(&path)?;
    }
    if !target.exists() && previous.len() == 1 {
        std::fs::rename(&previous[0], &target)?;
    } else {
        for path in previous {
            remove_entry(&path)?;
        }
    }
    Ok(())
}
fn remove_entry(path: &Path) -> Result<(), std::io::Error> {
    if std::fs::symlink_metadata(path)?.file_type().is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), WorkflowPackageError> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
pub(crate) fn commit_bundle(
    dir: &Path,
    lock: &BundleLock,
    files: &BTreeMap<String, String>,
    replace: bool,
) -> Result<(), WorkflowPackageError> {
    let name = &lock.name;
    let staging = dir.join(format!(".{name}.staging-{}", uuid::Uuid::new_v4()));
    let previous = dir.join(format!(".{name}.previous-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging)?;
    let result = (|| {
        for (n, yaml) in files {
            write_new(&staging.join(format!("{n}.yaml")), yaml.as_bytes())?;
        }
        write_new(
            &staging.join(BUNDLE_LOCK_FILE),
            serde_json::to_string_pretty(lock)?.as_bytes(),
        )?;
        let target = dir.join(name);
        if replace {
            load_unmodified(dir, name)?;
            std::fs::rename(&target, &previous)?;
        }
        if let Err(e) = std::fs::rename(&staging, &target) {
            if replace {
                let _ = std::fs::rename(&previous, &target);
            }
            return Err(e.into());
        }
        if replace {
            let _ = std::fs::remove_dir_all(&previous);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}
pub(crate) fn remove_bundle(dir: &Path, name: &str) -> Result<(), WorkflowPackageError> {
    load_unmodified(dir, name)?;
    let removing = dir.join(format!(".{name}.removing-{}", uuid::Uuid::new_v4()));
    std::fs::rename(dir.join(name), &removing)?;
    std::fs::remove_dir_all(removing)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rejects_duplicate_names_paths_and_unknown_fields() {
        for text in [
            "workflows:\n  - { name: Review, path: a.yaml }\n  - { name: review, path: b.yaml }",
            "workflows:\n  - { name: one, path: a.yaml }\n  - { name: two, path: a.yaml }",
            "path: workflows\nworkflows:\n  - { name: one, path: a.yaml }",
            "workflows:\n  - { name: one, path: ../a.yaml }",
        ] {
            assert!(parse_manifest(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn ambiguous_recovery_preserves_all_entries() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for name in [".pkg.previous-a", ".pkg.previous-b", ".pkg.staging-a"] {
            std::fs::create_dir(dir.path().join(name))?;
        }
        assert!(matches!(
            recover_bundle(dir.path(), "pkg"),
            Err(WorkflowPackageError::Interrupted(_))
        ));
        for name in [".pkg.previous-a", ".pkg.previous-b", ".pkg.staging-a"] {
            assert!(dir.path().join(name).is_dir());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lock_is_not_discoverable() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("pkg"))?;
        std::fs::write(dir.path().join("lock"), "{}")?;
        symlink(
            dir.path().join("lock"),
            dir.path().join("pkg").join(BUNDLE_LOCK_FILE),
        )?;
        assert!(matches!(
            read_lock(dir.path(), "pkg"),
            Err(WorkflowPackageError::NotInstalled(_))
        ));
        Ok(())
    }
}
