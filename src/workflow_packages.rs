//! GitHub workflow package installer: pins a GitHub workflow (with its nested
//! `workflow_call` and `prompt_file` dependencies) into the user workflow directory.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{StepConfig, WorkflowConfig};
use crate::workflow_call::GitHubWorkflowRef;

const DEFAULT_WORKFLOW_PATH: &str = "cruise.yaml";
const MANIFEST_SUFFIX: &str = ".cruise-package.json";

#[derive(Debug, Error)]
pub enum WorkflowPackageError {
    #[error("invalid package spec")]
    InvalidSpec,
    #[error("GitHub error: {0}")]
    GitHub(String),
    #[error("invalid workflow: {0}")]
    InvalidWorkflow(String),
    #[error("name collision: {0}")]
    NameCollision(String),
    #[error("not installed: {0}")]
    NotInstalled(String),
    #[error("modified: {0}")]
    Modified(String),
    #[error("confirmation required")]
    ConfirmationRequired,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Manifest(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub owner: String,
    pub repo: String,
    pub path: String,
    pub requested_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct Manifest {
    name: String,
    owner: String,
    repo: String,
    workflow_path: String,
    requested_ref: String,
    resolved_commit: String,
    workflow_sha256: String,
}

#[derive(Debug)]
pub struct PreparedPackage {
    manifest: Manifest,
    yaml: String,
    preview: String,
    replace_existing: bool,
}

impl PreparedPackage {
    /// Human-readable summary of the source and every command the workflow can run.
    pub fn preview(&self) -> &str {
        &self.preview
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub name: String,
    pub owner: String,
    pub repo: String,
    pub workflow_path: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub config_path: PathBuf,
}

/// Default install name for a spec: the repo name for the root workflow, else the path stem.
pub fn default_name(spec: &PackageSpec) -> String {
    if spec.path == DEFAULT_WORKFLOW_PATH {
        return spec.repo.clone();
    }
    Path::new(&spec.path)
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

pub fn parse_package_spec(input: &str) -> Result<PackageSpec, WorkflowPackageError> {
    let (body, requested_ref) = match input.split_once('@') {
        Some((body, r)) if !r.is_empty() && !r.split('/').any(str::is_empty) => (body, Some(r)),
        Some(_) => return Err(WorkflowPackageError::InvalidSpec),
        None => (input, None),
    };
    let parts: Vec<&str> = body.split('/').collect();
    let name_ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if parts.len() < 2 || !name_ok(parts[0]) || !name_ok(parts[1]) {
        return Err(WorkflowPackageError::InvalidSpec);
    }
    let path = if parts.len() == 2 {
        DEFAULT_WORKFLOW_PATH.to_string()
    } else {
        let path_parts = &parts[2..];
        if path_parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
            return Err(WorkflowPackageError::InvalidSpec);
        }
        let path = path_parts.join("/");
        let ext = Path::new(&path).extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            return Err(WorkflowPackageError::InvalidSpec);
        }
        path
    };
    Ok(PackageSpec {
        owner: parts[0].to_string(),
        repo: parts[1].to_string(),
        path,
        requested_ref: requested_ref.map(ToString::to_string),
    })
}

fn gh_api(endpoint: &str) -> Result<String, WorkflowPackageError> {
    let output = Command::new("gh")
        .args(["api", endpoint, "--jq"])
        .arg(if endpoint.contains("/commits/") {
            ".sha"
        } else {
            ".default_branch"
        })
        .output()
        .map_err(|e| WorkflowPackageError::GitHub(format!("failed to run gh: {e}")))?;
    if !output.status.success() {
        return Err(WorkflowPackageError::GitHub(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(WorkflowPackageError::GitHub(format!(
            "empty response for {endpoint}"
        )));
    }
    Ok(value)
}

fn validate_name(name: &str) -> Result<(), WorkflowPackageError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(WorkflowPackageError::InvalidSpec);
    }
    Ok(())
}

fn manifest_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{MANIFEST_SUFFIX}"))
}

fn yaml_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.yaml"))
}

fn sha256_hex(data: &[u8]) -> String {
    crate::file_tracker::sha256_digest(data)
        .iter()
        .fold(String::new(), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

fn step_commands(label: &str, step: &StepConfig, out: &mut Vec<String>) {
    if let Some(command) = &step.command {
        let text = match command {
            crate::config::StringOrVec::Single(s) => s.clone(),
            crate::config::StringOrVec::Multiple(v) => v.join(" && "),
        };
        out.push(format!("step {label}: {text}"));
    }
    if let Some(children) = &step.parallel {
        for (child_name, child) in children {
            step_commands(&format!("{label}/{child_name}"), child, out);
        }
    }
}

fn preview_text(manifest: &Manifest, config: &WorkflowConfig) -> String {
    let mut commands = Vec::new();
    if !config.command.is_empty() {
        commands.push(format!("executor: {}", config.command.join(" ")));
    }
    let all_steps = config
        .steps
        .iter()
        .chain(config.after_pr.iter())
        .chain(config.groups.values().flat_map(|g| g.steps.iter()));
    for (name, step) in all_steps {
        step_commands(name, step, &mut commands);
    }
    for (name, server) in &config.mcp_servers {
        if let Some(command) = &server.command {
            commands.push(format!(
                "mcp {name}: {} {}",
                command,
                server.args.join(" ")
            ));
        }
    }
    let mut text = format!(
        "Workflow package '{}'\n  source: {}/{}/{}\n  requested ref: {}\n  commit: {}\n",
        manifest.name,
        manifest.owner,
        manifest.repo,
        manifest.workflow_path,
        manifest.requested_ref,
        manifest.resolved_commit
    );
    text.push_str("Commands this workflow can run:\n");
    if commands.is_empty() {
        text.push_str("  (none)\n");
    }
    for command in commands {
        let _ = writeln!(text, "  {command}");
    }
    text.push_str(
        "WARNING: this workflow is untrusted and may run arbitrary commands with side effects when you run it.\n",
    );
    text
}

fn build_package(
    name: &str,
    owner: &str,
    repo: &str,
    workflow_path: &str,
    requested_ref: &str,
    replace_existing: bool,
) -> Result<PreparedPackage, WorkflowPackageError> {
    let resolved_commit = gh_api(&format!("repos/{owner}/{repo}/commits/{requested_ref}"))?;
    let reference = GitHubWorkflowRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        git_ref: resolved_commit.clone(),
        path: workflow_path.to_string(),
    };
    let config = crate::workflow_call::resolve_github_workflow_for_install(&reference)
        .map_err(|e| WorkflowPackageError::InvalidWorkflow(e.to_string()))?;
    crate::config::validate_config(&config)
        .map_err(|e| WorkflowPackageError::InvalidWorkflow(e.to_string()))?;
    let self_contained = config
        .steps
        .values()
        .chain(config.after_pr.values())
        .chain(config.groups.values().flat_map(|g| g.steps.values()))
        .all(|s| s.workflow_call.is_none() && s.prompt_file.is_none());
    if !self_contained {
        return Err(WorkflowPackageError::InvalidWorkflow(
            "workflow still has unresolved workflow_call or prompt_file".to_string(),
        ));
    }
    let yaml = serde_yaml::to_string(&config)
        .map_err(|e| WorkflowPackageError::InvalidWorkflow(e.to_string()))?;
    let manifest = Manifest {
        name: name.to_string(),
        owner: owner.to_string(),
        repo: repo.to_string(),
        workflow_path: workflow_path.to_string(),
        requested_ref: requested_ref.to_string(),
        resolved_commit,
        workflow_sha256: sha256_hex(yaml.as_bytes()),
    };
    let preview = preview_text(&manifest, &config);
    Ok(PreparedPackage {
        manifest,
        yaml,
        preview,
        replace_existing,
    })
}

fn check_no_collision(dir: &Path, name: &str) -> Result<(), WorkflowPackageError> {
    let taken = yaml_path(dir, name).exists()
        || dir.join(format!("{name}.yml")).exists()
        || manifest_path(dir, name).exists();
    if taken {
        return Err(WorkflowPackageError::NameCollision(name.to_string()));
    }
    Ok(())
}

fn workflows_dir() -> Result<PathBuf, WorkflowPackageError> {
    crate::paths::workflows_dir().map_err(|e| WorkflowPackageError::GitHub(e.to_string()))
}

pub fn prepare_install(
    spec: &PackageSpec,
    name: &str,
) -> Result<PreparedPackage, WorkflowPackageError> {
    validate_name(name)?;
    check_no_collision(&workflows_dir()?, name)?;
    let requested_ref = match &spec.requested_ref {
        Some(r) => r.clone(),
        None => gh_api(&format!("repos/{}/{}", spec.owner, spec.repo))?,
    };
    build_package(
        name,
        &spec.owner,
        &spec.repo,
        &spec.path,
        &requested_ref,
        false,
    )
}

fn write_atomic(path: &Path, content: &str) -> Result<(), WorkflowPackageError> {
    let file_name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

pub fn commit_install(
    prepared: PreparedPackage,
) -> Result<InstalledPackage, WorkflowPackageError> {
    let dir = workflows_dir()?;
    let name = &prepared.manifest.name;
    if !prepared.replace_existing {
        check_no_collision(&dir, name)?;
    }
    std::fs::create_dir_all(&dir)?;
    let manifest_json = serde_json::to_string_pretty(&prepared.manifest)?;
    let config_path = yaml_path(&dir, name);
    write_atomic(&config_path, &prepared.yaml)?;
    write_atomic(&manifest_path(&dir, name), &manifest_json)?;
    let m = prepared.manifest;
    Ok(InstalledPackage {
        name: m.name,
        owner: m.owner,
        repo: m.repo,
        workflow_path: m.workflow_path,
        requested_ref: m.requested_ref,
        resolved_commit: m.resolved_commit,
        config_path,
    })
}

/// Load a managed package's manifest and verify its YAML is unmodified.
fn load_unmodified(dir: &Path, name: &str) -> Result<Manifest, WorkflowPackageError> {
    validate_name(name)?;
    let manifest_text = std::fs::read_to_string(manifest_path(dir, name))
        .map_err(|_| WorkflowPackageError::NotInstalled(name.to_string()))?;
    let manifest: Manifest = serde_json::from_str(&manifest_text)?;
    let yaml = std::fs::read(yaml_path(dir, name))
        .map_err(|_| WorkflowPackageError::NotInstalled(name.to_string()))?;
    if sha256_hex(&yaml) != manifest.workflow_sha256 {
        return Err(WorkflowPackageError::Modified(name.to_string()));
    }
    Ok(manifest)
}

pub fn prepare_update(name: &str) -> Result<PreparedPackage, WorkflowPackageError> {
    let m = load_unmodified(&workflows_dir()?, name)?;
    build_package(
        &m.name,
        &m.owner,
        &m.repo,
        &m.workflow_path,
        &m.requested_ref,
        true,
    )
}

pub fn remove(name: &str) -> Result<(), WorkflowPackageError> {
    let dir = workflows_dir()?;
    load_unmodified(&dir, name)?;
    std::fs::remove_file(yaml_path(&dir, name))?;
    std::fs::remove_file(manifest_path(&dir, name))?;
    Ok(())
}

pub fn list_installed() -> Result<Vec<InstalledPackage>, WorkflowPackageError> {
    let dir = workflows_dir()?;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut packages = Vec::new();
    for entry in entries {
        let file_name = entry?.file_name().to_string_lossy().into_owned();
        let Some(name) = file_name.strip_suffix(MANIFEST_SUFFIX) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(manifest_path(&dir, name)) else {
            continue;
        };
        let Ok(m) = serde_json::from_str::<Manifest>(&text) else {
            continue;
        };
        let config_path = yaml_path(&dir, name);
        if m.name != name || !config_path.exists() {
            continue;
        }
        packages.push(InstalledPackage {
            name: m.name,
            owner: m.owner,
            repo: m.repo,
            workflow_path: m.workflow_path,
            requested_ref: m.requested_ref,
            resolved_commit: m.resolved_commit,
            config_path,
        });
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(owner: &str, repo: &str, path: &str, r: Option<&str>) -> PackageSpec {
        PackageSpec {
            owner: owner.into(),
            repo: repo.into(),
            path: path.into(),
            requested_ref: r.map(Into::into),
        }
    }

    #[test]
    fn parse_package_spec_defaults_path_to_cruise_yaml_and_ref_to_none() {
        let parsed = parse_package_spec("org/repo").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed, spec("org", "repo", "cruise.yaml", None));
    }

    #[test]
    fn parse_package_spec_accepts_ref_without_path() {
        let parsed = parse_package_spec("org/repo@main").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed, spec("org", "repo", "cruise.yaml", Some("main")));
    }

    #[test]
    fn parse_package_spec_accepts_path_and_slash_ref() {
        let parsed = parse_package_spec("org/repo/workflows/review.yaml@feature/one")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            parsed,
            spec("org", "repo", "workflows/review.yaml", Some("feature/one"))
        );
    }

    #[test]
    fn parse_package_spec_rejects_empty_components_and_traversal() {
        for bad in [
            "",
            "org",
            "/repo",
            "org/",
            "org//x.yaml",
            "org/repo@",
            "org/repo/../x.yaml",
            "org/repo/a/../../x.yaml",
            "https://github.com/org/repo",
        ] {
            assert!(
                matches!(
                    parse_package_spec(bad),
                    Err(WorkflowPackageError::InvalidSpec)
                ),
                "{bad:?} should be InvalidSpec"
            );
        }
    }
}
