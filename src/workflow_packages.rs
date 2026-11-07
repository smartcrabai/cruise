//! GitHub workflow package installer: pins a GitHub workflow (with its nested
//! `workflow_call` and `prompt_file` dependencies) into the user workflow directory.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{StepConfig, WorkflowConfig};
use crate::workflow_call::GitHubWorkflowRef;
pub(crate) mod bundle;
use std::collections::BTreeMap;

const DEFAULT_WORKFLOW_PATH: &str = "cruise.yaml";
const MANIFEST_SUFFIX: &str = ".cruise-package.json";

#[derive(Debug, Error)]
pub enum WorkflowPackageError {
    #[error("invalid package spec")]
    InvalidSpec,
    #[error("invalid package manifest: {0}")]
    InvalidManifest(String),
    #[error("interrupted package change needs manual cleanup: {0}")]
    Interrupted(String),
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
    pub explicit_path: bool,
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
    kind: PreparedKind,
    preview: String,
    replace_existing: bool,
}

#[derive(Debug)]
enum PreparedKind {
    Single {
        manifest: Manifest,
        yaml: String,
    },
    Bundle {
        lock: bundle::BundleLock,
        files: BTreeMap<String, String>,
    },
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
    pub workflows: Vec<String>,
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
        if path_parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
        {
            return Err(WorkflowPackageError::InvalidSpec);
        }
        let path = path_parts.join("/");
        let ext = Path::new(&path).extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("yaml" | "yml")) || !bundle::valid_path(&path) {
            return Err(WorkflowPackageError::InvalidSpec);
        }
        path
    };
    Ok(PackageSpec {
        owner: parts[0].to_string(),
        repo: parts[1].to_string(),
        path,
        requested_ref: requested_ref.map(ToString::to_string),
        explicit_path: parts.len() > 2,
    })
}

fn gh_jq(endpoint: &str, jq: &str) -> Result<String, WorkflowPackageError> {
    let output = Command::new("gh")
        .args(["api", endpoint, "--jq"])
        .arg(jq)
        .output()
        .map_err(|e| WorkflowPackageError::GitHub(format!("failed to run gh: {e}")))?;
    if !output.status.success() {
        return Err(WorkflowPackageError::GitHub(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(value)
}

fn resolve_commit(
    owner: &str,
    repo: &str,
    requested_ref: &str,
) -> Result<(String, String), WorkflowPackageError> {
    let value = gh_jq(
        &format!("repos/{owner}/{repo}/commits/{requested_ref}"),
        ".sha + \" \" + .commit.tree.sha",
    )?;
    let mut parts = value.split_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(commit), Some(tree), None)
            if commit.len() == 40
                && commit.bytes().all(|c| c.is_ascii_hexdigit())
                && tree.len() == 40
                && tree.bytes().all(|c| c.is_ascii_hexdigit()) =>
        {
            Ok((commit.into(), tree.into()))
        }
        _ => Err(WorkflowPackageError::GitHub(
            "invalid commit or tree response".into(),
        )),
    }
}
fn manifest_entry_kind(listing: &str) -> Result<bool, WorkflowPackageError> {
    match listing.trim() {
        "" => Ok(false),
        "blob 100644" | "blob 100755" => Ok(true),
        _ => Err(WorkflowPackageError::InvalidManifest(
            "cruise-package.yaml is not a regular file".into(),
        )),
    }
}
fn probe_manifest(owner: &str, repo: &str, tree: &str) -> Result<bool, WorkflowPackageError> {
    manifest_entry_kind(&gh_jq(
        &format!("repos/{owner}/{repo}/git/trees/{tree}"),
        "if .truncated then \"TRUNCATED\" else .tree[] | select(.path == \"cruise-package.yaml\") | \"\\(.type) \\(.mode)\" end",
    )?)
}

fn verify_bundle_workflow_entry(
    owner: &str,
    repo: &str,
    tree: &str,
    path: &str,
) -> Result<(), WorkflowPackageError> {
    let path_json = serde_json::to_string(path)?;
    let jq = format!(
        "if .truncated then \"TRUNCATED\" else .tree[] | select(.path == {path_json}) | \"\\(.type) \\(.mode)\" end"
    );
    let listing = gh_jq(
        &format!("repos/{owner}/{repo}/git/trees/{tree}?recursive=1"),
        &jq,
    )?;
    if matches!(listing.trim(), "blob 100644" | "blob 100755") {
        Ok(())
    } else {
        Err(WorkflowPackageError::InvalidManifest(format!(
            "workflow {path} is missing or is not a regular file"
        )))
    }
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

fn collect_commands(config: &WorkflowConfig) -> Vec<String> {
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
            commands.push(format!("mcp {name}: {} {}", command, server.args.join(" ")));
        }
    }
    commands
}

fn preview_text(manifest: &Manifest, config: &WorkflowConfig) -> String {
    let commands = collect_commands(config);
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

fn fetch_workflow_artifact(
    owner: &str,
    repo: &str,
    resolved_commit: &str,
    workflow_path: &str,
) -> Result<(WorkflowConfig, String), WorkflowPackageError> {
    let reference = GitHubWorkflowRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        git_ref: resolved_commit.to_string(),
        path: workflow_path.to_string(),
    };
    let config = crate::workflow_call::resolve_github_workflow_for_install(&reference)
        .map_err(|e| WorkflowPackageError::InvalidWorkflow(e.to_string()))?;
    crate::config::validate_config(&config)
        .map_err(|e| WorkflowPackageError::InvalidWorkflow(e.to_string()))?;
    let self_contained = config.inputs.is_empty()
        && config
            .steps
            .values()
            .chain(config.after_pr.values())
            .chain(config.groups.values().flat_map(|g| g.steps.values()))
            .all(materialized_step);
    if !self_contained {
        return Err(WorkflowPackageError::InvalidWorkflow(
            "workflow still has unresolved inputs, with, workflow_call or prompt_file".to_string(),
        ));
    }
    let yaml = serde_yaml::to_string(&config)
        .map_err(|e| WorkflowPackageError::InvalidWorkflow(e.to_string()))?;
    Ok((config, yaml))
}

fn build_package(
    name: &str,
    owner: &str,
    repo: &str,
    workflow_path: &str,
    requested_ref: &str,
    resolved_commit: String,
    replace_existing: bool,
) -> Result<PreparedPackage, WorkflowPackageError> {
    let (config, yaml) = fetch_workflow_artifact(owner, repo, &resolved_commit, workflow_path)?;
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
        kind: PreparedKind::Single { manifest, yaml },
        preview,
        replace_existing,
    })
}

fn build_bundle(
    name: &str,
    owner: &str,
    repo: &str,
    requested_ref: &str,
    resolved_commit: String,
    tree: &str,
    replace_existing: bool,
) -> Result<PreparedPackage, WorkflowPackageError> {
    let reference = GitHubWorkflowRef {
        owner: owner.into(),
        repo: repo.into(),
        git_ref: resolved_commit.clone(),
        path: bundle::MANIFEST_FILE.into(),
    };
    let text = crate::workflow_call::fetch_github_file_for_install(&reference)
        .map_err(|e| WorkflowPackageError::InvalidManifest(e.to_string()))?;
    let manifest = bundle::parse_manifest(&text)?;
    let mut lock = bundle::BundleLock {
        name: name.into(),
        owner: owner.into(),
        repo: repo.into(),
        requested_ref: requested_ref.into(),
        resolved_commit,
        workflows: BTreeMap::new(),
    };
    let mut files = BTreeMap::new();
    let mut configs = BTreeMap::new();
    for wf in manifest.workflows {
        verify_bundle_workflow_entry(owner, repo, tree, &wf.path)?;
        let (config, yaml) = fetch_workflow_artifact(owner, repo, &lock.resolved_commit, &wf.path)?;
        lock.workflows.insert(
            wf.name.clone(),
            bundle::LockedWorkflow {
                source_path: wf.path,
                sha256: sha256_hex(yaml.as_bytes()),
            },
        );
        configs.insert(wf.name.clone(), config);
        files.insert(wf.name, yaml);
    }
    let mut preview = format!(
        "Workflow package '{name}'\n  source: {owner}/{repo}/{}\n  requested ref: {requested_ref}\n  commit: {}\n",
        bundle::MANIFEST_FILE,
        lock.resolved_commit
    );
    if let Some(description) = manifest.description {
        let _ = writeln!(preview, "  description: {description}");
    }
    preview.push_str("Workflows:\n");
    for (n, wf) in &lock.workflows {
        let _ = writeln!(preview, "  {n}  {}", wf.source_path);
    }
    preview.push_str("Commands this workflow can run:\n");
    for (n, config) in &configs {
        let commands = collect_commands(config);
        if commands.is_empty() {
            let _ = writeln!(preview, "  {n}: (none)");
        }
        for command in commands {
            let _ = writeln!(preview, "  {n}: {command}");
        }
        let _ = writeln!(preview, "WARNING: workflow {n} is untrusted and may run arbitrary commands with side effects when you run it.");
    }
    Ok(PreparedPackage {
        kind: PreparedKind::Bundle { lock, files },
        preview,
        replace_existing,
    })
}

fn materialized_step(step: &crate::config::StepConfig) -> bool {
    step.workflow_call.is_none()
        && step.prompt_file.is_none()
        && step.with.is_none()
        && step
            .parallel
            .as_ref()
            .is_none_or(|children| children.values().all(materialized_step))
}

fn check_no_collision(dir: &Path, name: &str) -> Result<(), WorkflowPackageError> {
    let taken = std::fs::symlink_metadata(yaml_path(dir, name)).is_ok()
        || std::fs::symlink_metadata(dir.join(format!("{name}.yml"))).is_ok()
        || std::fs::symlink_metadata(manifest_path(dir, name)).is_ok()
        || std::fs::symlink_metadata(dir.join(name)).is_ok();
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
        None => gh_jq(
            &format!("repos/{}/{}", spec.owner, spec.repo),
            ".default_branch",
        )?,
    };
    if requested_ref.is_empty() {
        return Err(WorkflowPackageError::GitHub("empty default branch".into()));
    }
    let (commit, tree) = resolve_commit(&spec.owner, &spec.repo, &requested_ref)?;
    if !spec.explicit_path && probe_manifest(&spec.owner, &spec.repo, &tree)? {
        return build_bundle(
            name,
            &spec.owner,
            &spec.repo,
            &requested_ref,
            commit,
            &tree,
            false,
        );
    }
    build_package(
        name,
        &spec.owner,
        &spec.repo,
        &spec.path,
        &requested_ref,
        commit,
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

pub fn commit_install(prepared: PreparedPackage) -> Result<InstalledPackage, WorkflowPackageError> {
    let dir = workflows_dir()?;
    let _guard = operation_lock()?;
    let name = match &prepared.kind {
        PreparedKind::Single { manifest, .. } => &manifest.name,
        PreparedKind::Bundle { lock, .. } => &lock.name,
    };
    bundle::recover_bundle(&dir, name)?;
    if !prepared.replace_existing {
        check_no_collision(&dir, name)?;
    }
    std::fs::create_dir_all(&dir)?;
    let (manifest, yaml) = match prepared.kind {
        PreparedKind::Single { manifest, yaml } => (manifest, yaml),
        PreparedKind::Bundle { lock, files } => {
            bundle::commit_bundle(&dir, &lock, &files, prepared.replace_existing)?;
            return Ok(installed_bundle(&dir, lock));
        }
    };
    if prepared.replace_existing {
        load_unmodified(&dir, &manifest.name)?;
    }
    let name = &manifest.name;
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let config_path = yaml_path(&dir, name);
    write_atomic(&config_path, &yaml)?;
    write_atomic(&manifest_path(&dir, name), &manifest_json)?;
    let m = manifest;
    Ok(InstalledPackage {
        name: m.name,
        owner: m.owner,
        repo: m.repo,
        workflow_path: m.workflow_path,
        requested_ref: m.requested_ref,
        resolved_commit: m.resolved_commit,
        config_path,
        workflows: Vec::new(),
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
    validate_name(name)?;
    let dir = workflows_dir()?;
    let target = dir.join(name);
    let bundle_path = if manifest_path(&dir, name).is_file() {
        None
    } else if target.is_dir() {
        Some(target)
    } else if !target.exists() {
        bundle::interrupted_previous(&dir, name)?
    } else {
        None
    };
    if let Some(bundle_path) = bundle_path {
        let lock = bundle::load_unmodified_at(&bundle_path, name)?;
        let (commit, tree) = resolve_commit(&lock.owner, &lock.repo, &lock.requested_ref)?;
        if !probe_manifest(&lock.owner, &lock.repo, &tree)? {
            return Err(WorkflowPackageError::InvalidManifest(
                "manifest is missing".into(),
            ));
        }
        return build_bundle(
            name,
            &lock.owner,
            &lock.repo,
            &lock.requested_ref,
            commit,
            &tree,
            true,
        );
    }
    let m = load_unmodified(&dir, name)?;
    let (commit, _) = resolve_commit(&m.owner, &m.repo, &m.requested_ref)?;
    build_package(
        &m.name,
        &m.owner,
        &m.repo,
        &m.workflow_path,
        &m.requested_ref,
        commit,
        true,
    )
}

pub fn remove(name: &str) -> Result<(), WorkflowPackageError> {
    validate_name(name)?;
    let dir = workflows_dir()?;
    let _guard = operation_lock()?;
    if manifest_path(&dir, name).is_file() {
        load_unmodified(&dir, name)?;
        std::fs::remove_file(yaml_path(&dir, name))?;
        std::fs::remove_file(manifest_path(&dir, name))?;
        return Ok(());
    }
    bundle::recover_bundle(&dir, name)?;
    if dir.join(name).is_dir() {
        return bundle::remove_bundle(&dir, name);
    }
    load_unmodified(&dir, name)?;
    std::fs::remove_file(yaml_path(&dir, name))?;
    std::fs::remove_file(manifest_path(&dir, name))?;
    Ok(())
}

fn operation_lock() -> Result<std::fs::File, WorkflowPackageError> {
    use fs2::FileExt;
    let dir = crate::paths::state_dir().map_err(|e| WorkflowPackageError::GitHub(e.to_string()))?;
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("workflow-packages.lock"))?;
    file.try_lock_exclusive().map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            WorkflowPackageError::GitHub("another workflow package operation is running".into())
        } else {
            e.into()
        }
    })?;
    Ok(file)
}
fn installed_bundle(dir: &Path, lock: bundle::BundleLock) -> InstalledPackage {
    InstalledPackage {
        config_path: dir.join(&lock.name),
        workflows: lock.workflows.keys().cloned().collect(),
        name: lock.name,
        owner: lock.owner,
        repo: lock.repo,
        workflow_path: bundle::MANIFEST_FILE.into(),
        requested_ref: lock.requested_ref,
        resolved_commit: lock.resolved_commit,
    }
}

pub fn list_installed() -> Result<Vec<InstalledPackage>, WorkflowPackageError> {
    let dir = workflows_dir()?;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut packages = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !file_name.starts_with('.') && entry.file_type()?.is_dir() {
            if let Ok(lock) = bundle::read_lock(&dir, &file_name) {
                if lock
                    .workflows
                    .keys()
                    .all(|n| dir.join(&file_name).join(format!("{n}.yaml")).is_file())
                {
                    packages.push(installed_bundle(&dir, lock));
                }
            }
            continue;
        }
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
            workflows: Vec::new(),
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
            explicit_path: path != "cruise.yaml",
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
