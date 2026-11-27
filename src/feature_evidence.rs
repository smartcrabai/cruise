//! Feature-journey evidence for PR/MR bodies with the built-in TDD flag `evidence`
//! (`tests/formal/pr_verification_evidence.qnt`).
//!
//! The repository's verification skill (`.agents/skills/verify-*/`) maps features to
//! the code they cover. `prepare` picks the features this change touches, an agent
//! drives them through the skill, and `validate` accepts the run only when every
//! touched feature was proved with saved files, nothing outside the skill changed,
//! and no text artifact looks like a credential. Failed, blocked, or unresolved
//! map-drift features stop the run before the PR. Publication pushes the files to the
//! `cruise-evidence` branch and adds a section bound to the validated tree; a later
//! head marks it stale until the evidence is refreshed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CruiseError, Result};
use crate::trusted_git::{
    changed_paths, diff_paths, git_output, session_artifacts_root, worktree_tree,
};

const STATE: &str = "feature-evidence/state.json";
const PLAN: &str = "feature-evidence/plan.md";
const REPORT: &str = "feature-evidence/report.json";
const FILES: &str = "feature-evidence/files";
const RUNS: &str = "target/verify-cruise";
const BRANCH: &str = "cruise-evidence";
const HEADING: &str = "### Feature journeys";
const MAX_FILE: usize = 5 * 1024 * 1024;
const MAX_FILE_LEN: u64 = 5 * 1024 * 1024;
const MAX_TOTAL: u64 = 25 * 1024 * 1024;
const MAX_FILES: usize = 60;
const TEXT: [&str; 6] = ["txt", "md", "json", "html", "log", "cast"];
const IMAGES: [&str; 4] = ["png", "gif", "jpg", "webp"];
const SECRETS: [&str; 9] = [
    "nonce=",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "Bearer ",
    "BEGIN PRIVATE KEY",
];

#[derive(Default, Serialize, Deserialize)]
struct State {
    skill: String,
    touched: Vec<Feature>,
    baseline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    validated: Option<Validated>,
    #[serde(default)]
    refresh_pending: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Feature {
    id: String,
    file: String,
}

#[derive(Serialize, Deserialize)]
struct Validated {
    tree: String,
    features: Vec<Proof>,
}

#[derive(Serialize, Deserialize)]
struct Proof {
    id: String,
    summary: String,
    files: Vec<Saved>,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    artifact: String,
    name: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    features: Vec<Reported>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reported {
    id: String,
    status: String,
    summary: String,
    files: Vec<String>,
}

fn load(root: &Path) -> Result<Option<State>> {
    let Ok(text) = crate::artifacts::read(root, STATE) else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|err| evidence_error(&format!("invalid evidence state: {err}")))
}

fn save(root: &Path, state: &State) -> Result<()> {
    let text = serde_json::to_string_pretty(state)
        .map_err(|err| evidence_error(&format!("cannot encode evidence state: {err}")))?;
    crate::artifacts::write(root, STATE, &format!("{text}\n"))
}

/// The single `.agents/skills/verify-*/SKILL.md` directory, relative to `cwd`.
fn find_skill(cwd: &Path) -> Result<String> {
    let pattern = cwd.join(".agents/skills/verify-*/SKILL.md");
    let skills: Vec<PathBuf> = glob::glob(&pattern.to_string_lossy())
        .map_err(|err| evidence_error(&err.to_string()))?
        .filter_map(std::result::Result::ok)
        .collect();
    match skills.as_slice() {
        [] => Err(evidence_error(
            "no verification skill: add `.agents/skills/verify-<app>/` with a feature map, or turn the `evidence` flag off",
        )),
        [skill] => skill
            .parent()
            .and_then(|dir| dir.strip_prefix(cwd).ok())
            .map(slash_path)
            .ok_or_else(|| evidence_error("verification skill path is outside the workspace")),
        _ => Err(evidence_error(
            "several `.agents/skills/verify-*/` skills; keep one per repository",
        )),
    }
}

/// Feature files and the `Covers:` paths each one declares.
fn features(cwd: &Path, skill: &str) -> Result<Vec<(Feature, Vec<String>)>> {
    let dir = cwd.join(skill).join("features");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|err| evidence_error(&format!("cannot read {}: {err}", dir.display())))?
    {
        let path = entry?.path();
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if path.extension().is_none_or(|ext| ext != "md") || stem == "README" {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let covers = text
            .lines()
            .find_map(|line| line.strip_prefix("Covers:"))
            .map(|line| {
                line.split('`')
                    .skip(1)
                    .step_by(2)
                    .map(str::to_owned)
                    .filter(|cover| !cover.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let file = path.strip_prefix(cwd).map(slash_path).unwrap_or_default();
        out.push((Feature { id: stem, file }, covers));
    }
    out.sort_by(|(a, _), (b, _)| a.id.cmp(&b.id));
    Ok(out)
}

/// Repository-relative path with `/` separators, as Git reports paths.
fn slash_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn covers_path(cover: &str, path: &str) -> bool {
    if let Some(dir) = cover.strip_suffix('/') {
        return path.starts_with(&format!("{dir}/"));
    }
    path == cover
        || glob::Pattern::new(cover).is_ok_and(|pattern| {
            pattern.matches_with(
                path,
                glob::MatchOptions {
                    require_literal_separator: true,
                    ..glob::MatchOptions::default()
                },
            )
        })
}

fn session_base(plan: &Path) -> String {
    plan.parent()
        .map(|dir| dir.join("state.json"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<crate::session::SessionState>(&text).ok())
        .and_then(|state| crate::branch_diff::session_base_commit(&state).ok())
        .unwrap_or_else(|| "HEAD".to_owned())
}

/// Pick the features this change touches and write the drive plan.
/// Returns `true` when there is at least one feature to drive.
pub(crate) fn prepare(cwd: &Path, plan: &Path) -> Result<bool> {
    let root = session_artifacts_root(plan)?;
    let skill = find_skill(cwd)?;
    let changed = changed_paths(cwd, &session_base(plan))?;
    let touched: Vec<Feature> = features(cwd, &skill)?
        .into_iter()
        .filter(|(_, covers)| {
            changed
                .iter()
                .any(|path| covers.iter().any(|cover| covers_path(cover, path)))
        })
        .map(|(feature, _)| feature)
        .collect();
    let session = plan
        .parent()
        .and_then(Path::file_name)
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let mut text = format!(
        "Verification skill: `{skill}/SKILL.md`\nEvidence directory for this run: `{RUNS}/{session}/`\n\nFeatures this change touches:\n"
    );
    if touched.is_empty() {
        text.push_str("- none\n");
    }
    for feature in &touched {
        let _ = writeln!(text, "- `{}`: `{}`", feature.id, feature.file);
    }
    crate::artifacts::write(&root, PLAN, &text)?;
    let previous = load(&root)?;
    save(
        &root,
        &State {
            skill,
            touched: touched.clone(),
            baseline: worktree_tree(cwd)?,
            validated: None,
            refresh_pending: previous.is_some_and(|state| state.refresh_pending),
        },
    )?;
    Ok(!touched.is_empty())
}

/// Accept the drive report, copy its files into the session, and bind them to the tree.
pub(crate) fn validate(cwd: &Path, plan: &Path) -> Result<String> {
    let root = session_artifacts_root(plan)?;
    let mut state = load(&root)?
        .ok_or_else(|| evidence_error("evidence-prepare must run before validation"))?;
    let current = worktree_tree(cwd)?;
    if let Some(path) = diff_paths(cwd, &state.baseline, &current)?
        .iter()
        .find(|path| !path.starts_with(&format!("{}/", state.skill)))
    {
        return Err(evidence_error(&format!(
            "the evidence step may edit only `{}/`, but changed `{path}`",
            state.skill
        )));
    }
    let mut proofs = Vec::new();
    if !state.touched.is_empty() {
        let report: Report = serde_json::from_str(crate::artifacts::read(&root, REPORT)?.trim())
            .map_err(|err| evidence_error(&format!("invalid evidence report: {err}")))?;
        let mut total = 0;
        let mut count = 0;
        for feature in &state.touched {
            let reported = report
                .features
                .iter()
                .find(|reported| reported.id == feature.id)
                .ok_or_else(|| {
                    evidence_error(&format!("no result for feature `{}`", feature.id))
                })?;
            let summary = crate::telemetry::stderr::sanitize(&reported.summary).body;
            match reported.status.as_str() {
                "proved" => {}
                "failed" => {
                    return Err(evidence_error(&format!(
                        "feature regression in `{}`: {summary}",
                        feature.id
                    )));
                }
                "blocked" | "map drift" => {
                    return Err(evidence_error(&format!(
                        "evidence blocked for `{}`: {summary}",
                        feature.id
                    )));
                }
                other => {
                    return Err(evidence_error(&format!(
                        "feature `{}` has unknown status `{other}`",
                        feature.id
                    )));
                }
            }
            if reported.files.is_empty() {
                return Err(evidence_error(&format!(
                    "feature `{}` is proved without saved files",
                    feature.id
                )));
            }
            let mut files = Vec::new();
            for (index, relative) in reported.files.iter().enumerate() {
                let saved = save_file(cwd, &root, &feature.id, index, relative)?;
                total += std::fs::metadata(cwd.join(relative))?.len();
                count += 1;
                if total > MAX_TOTAL || count > MAX_FILES {
                    return Err(evidence_error("evidence files exceed the size limit"));
                }
                files.push(saved);
            }
            proofs.push(Proof {
                id: feature.id.clone(),
                summary,
                files,
            });
        }
    }
    let summary = format!("Feature evidence proved {} feature(s).", proofs.len());
    state.validated = Some(Validated {
        tree: current,
        features: proofs,
    });
    save(&root, &state)?;
    Ok(summary)
}

fn save_file(cwd: &Path, root: &Path, id: &str, index: usize, relative: &str) -> Result<Saved> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || relative.split('/').any(|part| part == "..")
        || !relative.starts_with(&format!("{RUNS}/"))
    {
        return Err(evidence_error(&format!(
            "evidence file `{relative}` must be under `{RUNS}/`"
        )));
    }
    let path = cwd.join(rel);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|err| evidence_error(&format!("evidence file `{relative}`: {err}")))?;
    if !meta.file_type().is_file() || meta.len() > MAX_FILE_LEN {
        return Err(evidence_error(&format!(
            "evidence file `{relative}` must be a regular file of at most 5 MiB"
        )));
    }
    let bytes = std::fs::read(&path)?;
    let ext = rel
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !TEXT.contains(&ext.as_str()) && !IMAGES.contains(&ext.as_str()) {
        return Err(evidence_error(&format!(
            "evidence file `{relative}` has an unsupported type"
        )));
    }
    if TEXT.contains(&ext.as_str()) {
        let text = String::from_utf8_lossy(&bytes);
        if let Some(pattern) = SECRETS.iter().find(|pattern| text.contains(**pattern)) {
            return Err(evidence_error(&format!(
                "evidence file `{relative}` contains credential-like text (`{pattern}`)"
            )));
        }
    }
    let name = rel
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let artifact = format!("{FILES}/{id}/{index:02}-{name}");
    crate::artifacts::write_bytes(root, &artifact, &bytes, MAX_FILE)?;
    Ok(Saved {
        artifact,
        name,
        sha256: hex(&Sha256::digest(&bytes)),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The `### Feature journeys` section for the PR/MR body, after publishing its files.
/// `None` when the evidence flag did not run for this session.
pub(crate) fn publication(
    root: &Path,
    workspace: &Path,
    session: &str,
    forge: crate::forge::ForgeKind,
) -> Result<Option<String>> {
    let Some(state) = load(root)? else {
        return Ok(None);
    };
    let validated = state.validated.as_ref().ok_or_else(|| {
        evidence_error("feature evidence was not validated; rerun the evidence step")
    })?;
    if worktree_tree(workspace)? != validated.tree {
        return Err(evidence_error(
            "the workspace changed after the feature evidence was validated; rerun the evidence step",
        ));
    }
    let tree = &validated.tree;
    let mut section = format!("{HEADING} (tree `{}`)\n\n", &tree[..12.min(tree.len())]);
    if validated.features.is_empty() {
        section.push_str("No feature in the verification map is touched by this change.\n");
        return Ok(Some(section));
    }
    let links = match forge {
        crate::forge::ForgeKind::GitHub => Some(publish(root, workspace, session, validated)?),
        crate::forge::ForgeKind::GitLab => None,
    };
    for proof in &validated.features {
        let _ = writeln!(section, "- **{}**: {}", proof.id, proof.summary);
        for saved in &proof.files {
            match &links {
                Some((base_raw, base_blob)) => {
                    let path = saved
                        .artifact
                        .trim_start_matches(FILES)
                        .trim_start_matches('/');
                    if is_image(&saved.name) {
                        let _ = writeln!(section, "\n  ![{}]({base_raw}/{path})\n", saved.name);
                    } else {
                        let _ = writeln!(section, "  - [{}]({base_blob}/{path})", saved.name);
                    }
                }
                None => {
                    let _ = writeln!(
                        section,
                        "  - `{}` (SHA-256 `{}`, kept in the session artifacts)",
                        saved.name,
                        &saved.sha256[..12]
                    );
                }
            }
        }
    }
    Ok(Some(section))
}

fn is_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| IMAGES.contains(&ext.to_string_lossy().to_ascii_lowercase().as_str()))
}

/// Commit the validated files to `cruise-evidence` without touching the checkout and
/// return the raw and blob URL prefixes of the pushed commit.
fn publish(
    root: &Path,
    workspace: &Path,
    session: &str,
    validated: &Validated,
) -> Result<(String, String)> {
    let remote = String::from_utf8_lossy(
        &git_output(workspace, &["config", "--get", "remote.origin.url"])?.stdout,
    )
    .trim()
    .to_owned();
    let slug = remote
        .trim_start_matches("git@github.com:")
        .trim_start_matches("https://github.com/")
        .trim_end_matches(".git")
        .to_owned();
    if slug.split('/').count() != 2 || slug.contains(':') {
        return Err(evidence_error(&format!(
            "cannot publish evidence: origin `{remote}` is not a GitHub repository"
        )));
    }
    let prefix = format!("sessions/{session}/{}", &validated.tree[..12]);
    let index =
        std::env::temp_dir().join(format!("cruise-evidence-{}.index", uuid::Uuid::new_v4()));
    let result = (|| {
        let _ = git_output(
            workspace,
            &[
                "fetch",
                "-q",
                "origin",
                &format!("+refs/heads/{BRANCH}:refs/cruise-evidence/remote"),
            ],
        );
        let parent = git_output(
            workspace,
            &[
                "rev-parse",
                "-q",
                "--verify",
                "refs/cruise-evidence/remote^{commit}",
            ],
        )
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|oid| crate::trusted_git::valid_oid(oid));
        let git = |args: &[&str]| {
            crate::trusted_git::git_output_with_index(workspace, args, Some(&index))
        };
        match &parent {
            Some(parent) => git(&["read-tree", parent])?,
            None => git(&["read-tree", "--empty"])?,
        };
        for proof in &validated.features {
            for saved in &proof.files {
                let bytes = crate::artifacts::read_bytes(root, &saved.artifact, MAX_FILE)?;
                let blob = hash_object(workspace, &bytes)?;
                let path = format!(
                    "{prefix}/{}",
                    saved
                        .artifact
                        .trim_start_matches(FILES)
                        .trim_start_matches('/')
                );
                git(&[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},{path}"),
                ])?;
            }
        }
        let tree = String::from_utf8_lossy(&git(&["write-tree"])?.stdout)
            .trim()
            .to_owned();
        let mut args = vec!["commit-tree", tree.as_str(), "-m", "evidence"];
        if let Some(parent) = &parent {
            args.extend(["-p", parent.as_str()]);
        }
        let commit = String::from_utf8_lossy(&git(&args)?.stdout)
            .trim()
            .to_owned();
        let push = git_output(
            workspace,
            &[
                "push",
                "-q",
                "origin",
                &format!("{commit}:refs/heads/{BRANCH}"),
            ],
        )?;
        if !push.status.success() {
            return Err(evidence_error(&format!(
                "cannot push evidence to `{BRANCH}`: {}",
                String::from_utf8_lossy(&push.stderr).trim()
            )));
        }
        Ok((
            format!("https://github.com/{slug}/raw/{commit}/{prefix}"),
            format!("https://github.com/{slug}/blob/{commit}/{prefix}"),
        ))
    })();
    let _ = std::fs::remove_file(&index);
    result
}

fn hash_object(workspace: &Path, bytes: &[u8]) -> Result<String> {
    use std::io::Write as _;
    let mut child = std::process::Command::new("git")
        .args(["hash-object", "-w", "--stdin"])
        .current_dir(workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| evidence_error("git hash-object has no stdin"))?
        .write_all(bytes)?;
    let output = child.wait_with_output()?;
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !crate::trusted_git::valid_oid(&oid) {
        return Err(evidence_error("git hash-object failed"));
    }
    Ok(oid)
}

/// After-PR: whether the published evidence still matches the workspace. When it does
/// not, mark the PR/MR section stale and prepare a new drive (`true` = refresh owed).
pub(crate) fn refresh_check(cwd: &Path, plan: &Path, pr_url: &str) -> Result<bool> {
    let root = session_artifacts_root(plan)?;
    let Some(mut state) = load(&root)? else {
        return Ok(false);
    };
    let current = worktree_tree(cwd)?;
    if state.validated.as_ref().is_some_and(|v| v.tree == current) {
        return Ok(false);
    }
    let head =
        String::from_utf8_lossy(&git_output(cwd, &["rev-parse", "--short=12", "HEAD"])?.stdout)
            .trim()
            .to_owned();
    edit_section(
        pr_url,
        &format!(
            "{HEADING} (stale: predates `{head}`)\n\nRefreshing the evidence for the new head.\n"
        ),
    )?;
    state.refresh_pending = true;
    save(&root, &state)?;
    prepare(cwd, plan)
}

/// After-PR: validate the refreshed run and replace the PR/MR section.
pub(crate) fn republish(cwd: &Path, plan: &Path, pr_url: &str) -> Result<String> {
    let root = session_artifacts_root(plan)?;
    let Some(state) = load(&root)? else {
        return Ok("No feature evidence for this session.".into());
    };
    if !state.refresh_pending {
        return Ok("Feature evidence is current.".into());
    }
    let summary = validate(cwd, plan)?;
    let session = plan
        .parent()
        .and_then(Path::file_name)
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let forge = if pr_url.contains("/-/merge_requests/") {
        crate::forge::ForgeKind::GitLab
    } else {
        crate::forge::ForgeKind::GitHub
    };
    let section = publication(&root, cwd, &session, forge)?
        .ok_or_else(|| evidence_error("refreshed evidence is missing"))?;
    edit_section(pr_url, &section)?;
    let mut state = load(&root)?.unwrap_or_default();
    state.refresh_pending = false;
    save(&root, &state)?;
    Ok(summary)
}

/// Replace the `### Feature journeys` section of the PR/MR body (or append it).
fn edit_section(pr_url: &str, section: &str) -> Result<()> {
    let gitlab = pr_url.contains("/-/merge_requests/");
    let body = if gitlab {
        let (project, number) = gitlab_target(pr_url)?;
        let out = run_forge(
            "glab",
            &[
                "mr", "view", &number, "--repo", &project, "--output", "json",
            ],
        )?;
        serde_json::from_str::<serde_json::Value>(&out)
            .ok()
            .and_then(|value| value["description"].as_str().map(str::to_owned))
            .unwrap_or_default()
    } else {
        run_forge(
            "gh",
            &["pr", "view", pr_url, "--json", "body", "--jq", ".body"],
        )?
    };
    let updated = replace_section(&body, section);
    let file =
        std::env::temp_dir().join(format!("cruise-evidence-body-{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&file, &updated)?;
    let path = file.to_string_lossy().into_owned();
    let result = if gitlab {
        let (project, number) = gitlab_target(pr_url)?;
        run_forge(
            "glab",
            &[
                "mr",
                "update",
                &number,
                "--repo",
                &project,
                "--description",
                &updated,
            ],
        )
        .map(|_| ())
    } else {
        run_forge("gh", &["pr", "edit", pr_url, "--body-file", &path]).map(|_| ())
    };
    let _ = std::fs::remove_file(&file);
    result
}

fn gitlab_target(url: &str) -> Result<(String, String)> {
    let (project, number) = url
        .split_once("/-/merge_requests/")
        .ok_or_else(|| evidence_error("malformed merge request URL"))?;
    Ok((project.to_owned(), number.trim_end_matches('/').to_owned()))
}

fn run_forge(program: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(evidence_error(&format!(
            "`{program} {}` failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned())
}

fn replace_section(body: &str, section: &str) -> String {
    let Some(start) = body.find(HEADING) else {
        let mut out = body.trim_end().to_owned();
        out.push_str("\n\n");
        out.push_str(section);
        return out;
    };
    let after = &body[start + HEADING.len()..];
    let end = after
        .find("\n### ")
        .into_iter()
        .chain(after.find("\n## "))
        .min()
        .map_or(body.len(), |offset| start + HEADING.len() + offset + 1);
    format!("{}{}{}", &body[..start], section.trim_end(), {
        let rest = &body[end..];
        if rest.is_empty() {
            "\n".to_owned()
        } else {
            format!("\n\n{rest}")
        }
    })
}

/// The touched-feature list for tests and the drive plan.
#[cfg(test)]
fn touched_ids(root: &Path) -> Vec<String> {
    load(root)
        .ok()
        .flatten()
        .map(|state| state.touched.into_iter().map(|f| f.id).collect())
        .unwrap_or_default()
}

fn evidence_error(message: &str) -> CruiseError {
    CruiseError::Other(format!("feature evidence: {message}"))
}

#[cfg(test)]
mod tests;
