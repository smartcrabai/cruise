//! Read-only target discovery for workflow doctor.
use std::path::{Path, PathBuf};

use crate::resolver::{CandidateKind, ConfigSource};

#[derive(Debug)]
pub(crate) struct ResolvedTarget {
    pub target: String,
    pub kind: &'static str,
    pub package: bool,
    pub package_name: Option<String>,
    pub path: Option<PathBuf>,
    pub yaml: String,
    pub source: ConfigSource,
}

#[derive(Debug)]
pub(crate) struct UnresolvedTarget {
    pub target: String,
    pub code: &'static str,
    pub message: String,
}

pub(crate) type TargetResolution = Result<ResolvedTarget, UnresolvedTarget>;

fn error(target: &str, code: &'static str, message: impl Into<String>) -> TargetResolution {
    Err(UnresolvedTarget {
        target: target.to_owned(),
        code,
        message: message.into(),
    })
}

fn builtin(target: &str, name: &str) -> TargetResolution {
    let Some(workflow) = crate::builtin_workflows::find_builtin(name) else {
        return error(
            target,
            "target_not_found",
            format!("unknown built-in workflow: {name}"),
        );
    };
    let source = if workflow.name == crate::builtin_workflows::DEFAULT_BUILTIN_NAME {
        ConfigSource::Builtin
    } else {
        ConfigSource::NamedBuiltin(workflow.name)
    };
    Ok(ResolvedTarget {
        target: target.to_owned(),
        kind: "builtin",
        package: false,
        package_name: None,
        path: None,
        yaml: workflow.yaml.to_owned(),
        source,
    })
}

fn file(
    target: &str,
    path: PathBuf,
    kind: &'static str,
    cwd: &Path,
    packages: &[crate::workflow_packages::InstalledPackage],
) -> TargetResolution {
    if let Err(e) = crate::resolver::reject_defaults_file(&path, cwd) {
        return error(target, "invalid_target", e.to_string());
    }
    let yaml = match std::fs::read_to_string(&path) {
        Ok(yaml) => yaml,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return error(
                target,
                "target_not_found",
                format!("config file not found: {}", path.display()),
            );
        }
        Err(e) => {
            return error(
                target,
                "invalid_target",
                format!("cannot read {}: {e}", path.display()),
            );
        }
    };
    let canonical = std::fs::canonicalize(&path).ok();
    let package_name = packages
        .iter()
        .find(|p| {
            canonical
                .as_ref()
                .is_some_and(|c| std::fs::canonicalize(&p.config_path).ok().as_ref() == Some(c))
        })
        .map(|p| p.name.clone())
        .or_else(|| {
            // list_installed skips unreadable or malformed manifests. Keep those
            // packages visible so verification reports the manifest failure.
            let dir = crate::paths::workflows_dir().ok()?;
            let name = path.file_name()?.to_str()?.strip_suffix(".yaml")?;
            let installed_path = dir.join(format!("{name}.yaml"));
            if canonical.as_ref()? != &std::fs::canonicalize(installed_path).ok()?
                || !dir.join(format!("{name}.cruise-package.json")).exists()
            {
                return None;
            }
            Some(name.to_owned())
        });
    let source = match kind {
        "project" => ConfigSource::Local(path.clone()),
        "user" => ConfigSource::UserDir(path.clone()),
        _ => ConfigSource::Explicit(path.clone()),
    };
    Ok(ResolvedTarget {
        target: target.to_owned(),
        kind,
        package: package_name.is_some(),
        package_name,
        path: Some(path),
        yaml,
        source,
    })
}

/// Resolve explicit selectors or discover local/all workflows in deterministic order.
pub(crate) fn resolve_targets(targets: &[String], all: bool, cwd: &Path) -> Vec<TargetResolution> {
    let packages = crate::workflow_packages::list_installed().unwrap_or_default();
    let user_dir = crate::paths::workflows_dir().ok();
    if !targets.is_empty() {
        return targets
            .iter()
            .map(|target| {
                if target == "__builtin__" {
                    return builtin(target, crate::builtin_workflows::DEFAULT_BUILTIN_NAME);
                }
                if let Some(name) = target.strip_prefix("builtin:") {
                    return builtin(target, name);
                }
                let path = Path::new(target);
                let absolute = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                };
                if target.contains('/')
                    || target.contains('\\')
                    || path.extension().is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml")
                    })
                    || absolute.exists()
                {
                    return file(target, absolute, "file", cwd, &packages);
                }
                let user = user_dir.as_ref().and_then(|dir| {
                    ["yaml", "yml"]
                        .iter()
                        .map(|ext| dir.join(format!("{target}.{ext}")))
                        .find(|p| p.is_file())
                });
                let named_builtin = crate::builtin_workflows::find_builtin(target);
                match (user, named_builtin) {
                    (Some(_), Some(_)) => error(
                        target,
                        "ambiguous_target",
                        format!("'{target}' matches both a user workflow and a built-in workflow"),
                    ),
                    (Some(path), None) => file(target, path, "user", cwd, &packages),
                    (None, Some(_)) => builtin(target, target),
                    (None, None) => error(
                        target,
                        "target_not_found",
                        format!("workflow not found: {target}"),
                    ),
                }
            })
            .collect();
    }
    discover_targets(all, cwd, user_dir, &packages)
}

fn discover_targets(
    all: bool,
    cwd: &Path,
    user_dir: Option<PathBuf>,
    packages: &[crate::workflow_packages::InstalledPackage],
) -> Vec<TargetResolution> {
    let mut result = Vec::new();
    for candidate in crate::resolver::collect_local_candidates(cwd) {
        if let CandidateKind::Local(path) = candidate.source {
            result.push(file(&candidate.label, path, "project", cwd, packages));
        }
    }
    let local_dir = cwd.join(".cruise");
    if let Err(e) = std::fs::read_dir(&local_dir)
        .and_then(std::iter::Iterator::collect::<std::io::Result<Vec<_>>>)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        result.push(error(
            &local_dir.to_string_lossy(),
            "invalid_target",
            format!("cannot read {}: {e}", local_dir.display()),
        ));
    }
    if all {
        if let Some(dir) = user_dir {
            let entries = std::fs::read_dir(&dir)
                .and_then(std::iter::Iterator::collect::<std::io::Result<Vec<_>>>);
            match entries {
                Ok(entries) => {
                    let mut paths: Vec<_> = entries
                        .into_iter()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.is_file()
                                && matches!(
                                    path.extension().and_then(|ext| ext.to_str()),
                                    Some("yaml" | "yml")
                                )
                        })
                        .collect();
                    paths.sort_by_key(|path| path.file_name().unwrap_or_default().to_os_string());
                    for path in paths {
                        let label = path.to_string_lossy().into_owned();
                        result.push(file(&label, path, "user", cwd, packages));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => result.push(error(
                    &dir.to_string_lossy(),
                    "invalid_target",
                    format!("cannot read {}: {e}", dir.display()),
                )),
            }
        }
        for workflow in crate::builtin_workflows::BUILTIN_WORKFLOWS {
            result.push(builtin(
                &format!("builtin:{}", workflow.name),
                workflow.name,
            ));
        }
    }
    if result.is_empty() {
        result.push(error("", "no_targets", "no local workflow targets found"));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_discovery_excludes_defaults_and_orders_candidates()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".cruise"))?;
        for path in [
            "cruise.yaml",
            ".cruise/z.yml",
            ".cruise/a.yaml",
            ".cruise/default.yaml",
        ] {
            std::fs::write(dir.path().join(path), "steps: {}\n")?;
        }
        let targets = resolve_targets(&[], false, dir.path());
        let names: Vec<_> = targets
            .iter()
            .map(|t| t.as_ref().map(|target| target.target.as_str()))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("unexpected unresolved target: {e:?}"))?;
        assert_eq!(
            names,
            ["./cruise.yaml", "./.cruise/a.yaml", "./.cruise/z.yml"]
        );
        Ok(())
    }

    #[test]
    fn empty_local_and_missing_and_builtin_selectors() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        assert_eq!(
            resolve_targets(&[], false, dir.path())[0]
                .as_ref()
                .err()
                .map(|e| e.code),
            Some("no_targets")
        );
        let names = vec![
            "builtin:tdd".into(),
            "builtin:not-present".into(),
            "missing.yaml".into(),
            "__builtin__".into(),
        ];
        let targets = resolve_targets(&names, false, dir.path());
        assert_eq!(targets[0].as_ref().ok().map(|t| t.kind), Some("builtin"));
        assert_eq!(
            targets[1].as_ref().err().map(|e| e.code),
            Some("target_not_found")
        );
        assert_eq!(
            targets[2].as_ref().err().map(|e| e.code),
            Some("target_not_found")
        );
        assert_eq!(targets[3].as_ref().ok().map(|t| t.kind), Some("builtin"));
        Ok(())
    }
}
