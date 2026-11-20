//! Host-owned validation and completion for the after-PR base-sync lifecycle.
//!
//! The workflow may edit conflicted files, but this module alone stages, commits,
//! and pushes those edits. Every operation is bound to a session artifact and a
//! validated PR branch/ref/head captured before `sync_base` runs.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CruiseError, Result};
use crate::forge::{ForgeContext, ForgeKind};
use crate::worktree_pr::SyncBaseFailure;

const SYNC_RESULT_ARTIFACT: &str = "after-pr-sync.json";
const RESOLVER_OUTPUT_ARTIFACT: &str = "after-pr/conflict-resolution.md";
const MERGE_COMMIT_MESSAGE: &str = "Merge PR base after conflict resolution";

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum SyncResult {
    Cleared,
    Pending {
        context: SyncContext,
    },
    MergeConflict {
        context: SyncContext,
        merge_head: String,
        conflict_paths: Vec<String>,
        index_digest: String,
        untracked_paths_digest: String,
    },
    Ready {
        context: SyncContext,
        head: String,
        base_head: String,
        finalized_merge_head: Option<String>,
        pending_index_digest: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SyncContext {
    repo_root: String,
    local_branch: String,
    local_base_head: String,
    target: PullRequestTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct PullRequestTarget {
    number: u64,
    url: String,
    forge: ForgeKind,
    host: String,
    repository: String,
    head_branch: String,
    head_ref: String,
    expected_remote_head: String,
    base_branch: String,
    expected_base_head: String,
}

/// Clear stale state/output and capture the exact checkout and PR branch before sync.
pub(crate) fn clear_sync_result(
    plan: &Path,
    cwd: &Path,
    pr_number: &str,
    pr_url: Option<&str>,
) -> Result<String> {
    clear_session_artifact(plan, SYNC_RESULT_ARTIFACT)?;
    clear_session_artifact(plan, RESOLVER_OUTPUT_ARTIFACT)?;
    write_sync_result(plan, &SyncResult::Cleared)?;

    let root = repository_root(cwd)?;
    crate::worktree_pr::ensure_host_git_safety(&root)?;
    ensure_no_merge(&root)?;
    ensure_clean_worktree(&root, "refusing to sync with a dirty worktree")?;
    let target = resolve_target(&root, pr_number, pr_url)?;
    let local_branch = current_branch(&root)?;
    if local_branch != target.head_branch {
        return Err(other(
            "current branch does not match the pull request head branch",
        ));
    }
    let local_base_head = head(&root)?;
    if !is_ancestor(&root, &target.expected_remote_head, &local_base_head)? {
        return Err(other(
            "local HEAD does not descend from the validated PR head",
        ));
    }
    let expected_base_head = target.expected_base_head.clone();

    write_sync_result(
        plan,
        &SyncResult::Pending {
            context: SyncContext {
                repo_root: root.to_string_lossy().into_owned(),
                local_branch,
                local_base_head,
                target,
            },
        },
    )?;
    Ok(expected_base_head)
}

/// Record only a verified successful sync or a real merge conflict from this sync.
pub(crate) fn record_sync_result(
    plan: &Path,
    cwd: &Path,
    pr_number: &str,
    pr_url: Option<&str>,
    result: std::result::Result<(), SyncBaseFailure>,
) -> Result<()> {
    let previous = read_sync_result(plan)?;
    write_sync_result(plan, &SyncResult::Cleared)?;
    let SyncResult::Pending { context } = previous else {
        return Err(other("no pending host-owned base sync is recorded"));
    };
    if result
        .as_ref()
        .is_err_and(|failure| *failure != SyncBaseFailure::MergeConflict)
    {
        return Ok(());
    }

    let root = Path::new(&context.repo_root);
    crate::worktree_pr::ensure_host_git_safety(cwd)?;
    verify_call_identity(&context, cwd, pr_number, pr_url)?;
    let fresh_target = resolve_target(
        root,
        &context.target.number.to_string(),
        Some(&context.target.url),
    )?;
    if fresh_target != context.target {
        return Err(other("pull request identity or base changed during sync"));
    }
    verify_origin(&context.target, root)?;
    if remote_head(&context.target, root)? != context.target.expected_remote_head {
        return Err(other("validated PR head moved during base sync"));
    }
    if current_branch(root)? != context.local_branch {
        return Err(other("current branch changed during base sync"));
    }
    match result {
        Err(SyncBaseFailure::MergeConflict) => {
            if head(root)? != context.local_base_head {
                return Err(other("HEAD changed during the conflicting base sync"));
            }
            let Some(merge_head) = merge_head(root)? else {
                return Err(other("classified merge conflict has no active MERGE_HEAD"));
            };
            if merge_head.as_str() != context.target.expected_base_head {
                return Err(other(
                    "MERGE_HEAD does not match the authenticated PR base OID",
                ));
            }
            let entries = unmerged_entries(root)?;
            let paths = unmerged_paths(&entries)?;
            if paths.is_empty() {
                return Err(other("classified merge conflict has no unmerged paths"));
            }
            let index_digest = digest(&git(root, &["ls-files", "--stage", "-z"])?.stdout);
            let untracked_paths_digest =
                digest(&git(root, &["ls-files", "--others", "-z"])?.stdout);
            write_sync_result(
                plan,
                &SyncResult::MergeConflict {
                    context,
                    merge_head,
                    conflict_paths: paths.iter().map(|path| encode_hex(path)).collect(),
                    index_digest,
                    untracked_paths_digest,
                },
            )
        }
        Ok(()) => {
            if merge_head(root)?.is_some() || !unmerged_entries(root)?.is_empty() {
                return Err(other("successful base sync left a merge or unmerged paths"));
            }
            ensure_clean_worktree(
                root,
                "successful base sync left unexpected worktree changes",
            )?;
            let sync_head = head(root)?;
            if !is_ancestor(root, &context.local_base_head, &sync_head)? {
                return Err(other("successful base sync rewrote the local branch"));
            }
            let base_head = context.target.expected_base_head.clone();
            if !is_ancestor(root, &base_head, &sync_head)? {
                return Err(other(
                    "successful base sync did not include the authenticated PR base",
                ));
            }
            write_sync_result(
                plan,
                &SyncResult::Ready {
                    context,
                    head: sync_head,
                    base_head,
                    finalized_merge_head: None,
                    pending_index_digest: None,
                },
            )
        }
        Err(_) => Ok(()),
    }
}

/// Host-validate and commit a resolver output, or verify a previously completed sync.
pub(crate) fn finish_conflict_commit(plan: &Path, cwd: &Path) -> Result<()> {
    match read_sync_result(plan)? {
        SyncResult::Cleared => Err(other("no successful base-sync result is recorded")),
        SyncResult::Pending { .. } => Err(other("base sync has not recorded a verified result")),
        SyncResult::Ready {
            context,
            head: recorded_head,
            base_head,
            finalized_merge_head,
            pending_index_digest,
        } => {
            if let Some(expected_index_digest) = pending_index_digest.as_deref() {
                complete_pending_index_install(
                    plan,
                    cwd,
                    &context,
                    &recorded_head,
                    &base_head,
                    finalized_merge_head.as_deref(),
                    expected_index_digest,
                )?;
            }
            verify_ready_state(
                &context,
                cwd,
                &recorded_head,
                &base_head,
                finalized_merge_head.as_deref(),
            )
        }
        SyncResult::MergeConflict {
            context,
            merge_head: recorded_merge_head,
            conflict_paths,
            index_digest,
            untracked_paths_digest,
        } => finish_merge(
            plan,
            cwd,
            context,
            &recorded_merge_head,
            &conflict_paths,
            &index_digest,
            &untracked_paths_digest,
        ),
    }
}

/// Push only the recorded PR branch, with a current expected-head lease and ancestry proof.
pub(crate) fn push_after_sync(plan: &Path, cwd: &Path) -> Result<()> {
    let SyncResult::Ready {
        context,
        head: recorded_head,
        base_head,
        finalized_merge_head,
        pending_index_digest,
    } = read_sync_result(plan)?
    else {
        return Err(other(
            "no verified successful base sync is available to push",
        ));
    };
    if pending_index_digest.is_some() {
        return Err(other(
            "host conflict commit index installation is not complete",
        ));
    }
    verify_ready_state(
        &context,
        cwd,
        &recorded_head,
        &base_head,
        finalized_merge_head.as_deref(),
    )?;
    let root = Path::new(&context.repo_root);
    verify_origin(&context.target, root)?;
    if !is_ancestor(root, &context.target.expected_remote_head, &recorded_head)? {
        return Err(other(
            "push would not be a fast-forward from the validated PR head",
        ));
    }
    let remote = remote_head(&context.target, root)?;
    if remote == recorded_head {
        return Ok(());
    }
    if remote != context.target.expected_remote_head {
        return Err(other(
            "PR branch moved since the validated base sync; refusing to push",
        ));
    }
    let fresh_target = resolve_target(
        root,
        &context.target.number.to_string(),
        Some(&context.target.url),
    )?;
    if fresh_target != context.target {
        return Err(other("pull request identity or base changed before push"));
    }
    let endpoint = crate::worktree_pr::host_push_endpoint(root)?;
    verify_push_endpoint(&context.target, &endpoint)?;

    let lease = format!(
        "--force-with-lease={}:{}",
        context.target.head_ref, context.target.expected_remote_head
    );
    let refspec = format!("{}:{}", recorded_head, context.target.head_ref);
    git(
        root,
        &[
            "push",
            "--porcelain",
            "--no-follow-tags",
            "--no-recurse-submodules",
            &lease,
            &endpoint,
            &refspec,
        ],
    )?;
    if remote_head(&context.target, root)? != recorded_head {
        return Err(other(
            "push completed but the PR branch does not match the validated local HEAD",
        ));
    }
    Ok(())
}

fn finish_merge(
    plan: &Path,
    cwd: &Path,
    context: SyncContext,
    recorded_merge_head: &str,
    recorded_paths: &[String],
    index_digest: &str,
    untracked_paths_digest: &str,
) -> Result<()> {
    let root = Path::new(&context.repo_root);
    crate::worktree_pr::ensure_host_git_safety(cwd)?;
    verify_context_checkout(&context, cwd)?;
    if head(root)? != context.local_base_head {
        return Err(other(
            "HEAD changed after the recorded conflicting base sync",
        ));
    }
    if merge_head(root)?.as_deref() != Some(recorded_merge_head) {
        return Err(other(
            "active MERGE_HEAD does not match the recorded base sync",
        ));
    }

    let entries = unmerged_entries(root)?;
    let actual_paths = unmerged_paths(&entries)?;
    let expected_paths: BTreeSet<Vec<u8>> = recorded_paths
        .iter()
        .map(|path| decode_hex(path))
        .collect::<Result<_>>()?;
    if actual_paths.iter().ne(expected_paths.iter()) {
        return Err(other(
            "unmerged paths do not match the recorded conflict paths",
        ));
    }
    if actual_paths.is_empty() {
        return Err(other("the recorded merge has no unmerged conflict paths"));
    }
    let index = git(root, &["ls-files", "--stage", "-z"])?.stdout;
    if digest(&index) != index_digest {
        return Err(other(
            "the resolver changed the Git index; edit conflicted files only",
        ));
    }
    let output = crate::artifacts::read(&artifact_root(plan)?, RESOLVER_OUTPUT_ARTIFACT).map_err(
        |error| {
            other(format!(
                "no fresh host-written conflict-resolution output is recorded: {error}"
            ))
        },
    )?;
    if output.trim().is_empty() {
        return Err(other("host-written conflict-resolution output is empty"));
    }
    ensure_resolution_scope(root, &expected_paths, untracked_paths_digest)?;

    let (temporary_index, candidate_tree) = stage_resolution(
        cwd,
        &context,
        recorded_merge_head,
        &actual_paths,
        &expected_paths,
        index_digest,
        untracked_paths_digest,
    )?;
    git_with_index(
        root,
        &["commit", "-m", MERGE_COMMIT_MESSAGE],
        &temporary_index.path,
    )?;

    let completed_head = head(root)?;
    verify_merge_commit(
        root,
        &completed_head,
        &context.local_base_head,
        recorded_merge_head,
    )?;
    let completed_tree =
        rev_parse(root, "HEAD^{tree}")?.ok_or_else(|| other("host merge commit has no tree"))?;
    if completed_tree != candidate_tree {
        return Err(other(
            "host merge commit tree differs from the validated candidate index",
        ));
    }
    finish_ready_index_install(
        plan,
        cwd,
        context,
        completed_head,
        recorded_merge_head,
        index_digest,
        &temporary_index,
    )
}

fn finish_ready_index_install(
    plan: &Path,
    cwd: &Path,
    context: SyncContext,
    completed_head: String,
    recorded_merge_head: &str,
    index_digest: &str,
    temporary_index: &TemporaryIndex,
) -> Result<()> {
    let base_head = recorded_merge_head.to_string();
    let mut ready = SyncResult::Ready {
        context,
        head: completed_head,
        base_head,
        finalized_merge_head: Some(recorded_merge_head.to_string()),
        pending_index_digest: Some(index_digest.to_owned()),
    };
    write_sync_result(plan, &ready)?;
    if let SyncResult::Ready {
        context,
        pending_index_digest,
        ..
    } = &mut ready
    {
        temporary_index.install(Path::new(&context.repo_root), index_digest)?;
        *pending_index_digest = None;
    }
    write_sync_result(plan, &ready)?;
    let SyncResult::Ready {
        context,
        head,
        base_head,
        finalized_merge_head,
        ..
    } = ready
    else {
        return Err(other("completed host merge has no ready state"));
    };
    verify_ready_state(
        &context,
        cwd,
        &head,
        &base_head,
        finalized_merge_head.as_deref(),
    )
}

fn stage_resolution(
    cwd: &Path,
    context: &SyncContext,
    recorded_merge_head: &str,
    actual_paths: &[Vec<u8>],
    expected_paths: &BTreeSet<Vec<u8>>,
    index_digest: &str,
    untracked_paths_digest: &str,
) -> Result<(TemporaryIndex, String)> {
    let root = Path::new(&context.repo_root);
    crate::worktree_pr::ensure_host_git_safety(cwd)?;
    let pathspecs = literal_pathspecs(actual_paths);
    let temporary_index = TemporaryIndex::new(root)?;
    git_with_input_at_index(
        root,
        &[
            "add",
            "--all",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        &pathspecs,
        &temporary_index.path,
    )?;
    if !git_with_index(
        root,
        &["ls-files", "--unmerged", "-z"],
        &temporary_index.path,
    )?
    .stdout
    .is_empty()
    {
        return Err(other(
            "Git still reports unresolved merge paths after staging",
        ));
    }
    git_with_index(
        root,
        &["diff", "--no-ext-diff", "--cached", "--check"],
        &temporary_index.path,
    )?;
    ensure_resolution_scope(root, expected_paths, untracked_paths_digest)?;
    let candidate_tree = parse_oid(
        &git_with_index(root, &["write-tree"], &temporary_index.path)?.stdout,
        "resolution tree",
    )?;
    if digest(&git(root, &["ls-files", "--stage", "-z"])?.stdout) != index_digest {
        return Err(other(
            "Git index changed while validating conflict resolutions",
        ));
    }
    if head(root)? != context.local_base_head
        || merge_head(root)?.as_deref() != Some(recorded_merge_head)
    {
        return Err(other(
            "merge identity changed while validating conflict resolutions",
        ));
    }
    Ok((temporary_index, candidate_tree))
}

fn complete_pending_index_install(
    plan: &Path,
    cwd: &Path,
    context: &SyncContext,
    recorded_head: &str,
    base_head: &str,
    finalized_merge_head: Option<&str>,
    expected_index_digest: &str,
) -> Result<()> {
    let recorded_merge_head = finalized_merge_head
        .ok_or_else(|| other("finalized host merge is missing its recorded second parent"))?;
    crate::worktree_pr::ensure_host_git_safety(cwd)?;
    verify_context_checkout(context, cwd)?;
    let root = Path::new(&context.repo_root);
    if head(root)? != recorded_head || merge_head(root)?.is_some() {
        return Err(other(
            "finalized merge identity changed before index recovery",
        ));
    }
    verify_merge_commit(
        root,
        recorded_head,
        &context.local_base_head,
        recorded_merge_head,
    )?;

    let live_index = digest(&git(root, &["ls-files", "--stage", "-z"])?.stdout);
    let temporary_index = TemporaryIndex::new(root)?;
    git_with_index(root, &["read-tree", recorded_head], &temporary_index.path)?;
    if !git_with_index(
        root,
        &["ls-files", "--unmerged", "-z"],
        &temporary_index.path,
    )?
    .stdout
    .is_empty()
    {
        return Err(other("finalized commit tree contains unresolved paths"));
    }
    let candidate_tree = parse_oid(
        &git_with_index(root, &["write-tree"], &temporary_index.path)?.stdout,
        "recovered index tree",
    )?;
    let committed_tree = rev_parse(root, "HEAD^{tree}")?
        .ok_or_else(|| other("finalized host merge commit has no tree"))?;
    if candidate_tree != committed_tree {
        return Err(other(
            "recovered index does not match the finalized merge commit",
        ));
    }
    let candidate_index = digest(
        &git_with_index(root, &["ls-files", "--stage", "-z"], &temporary_index.path)?.stdout,
    );
    if live_index == expected_index_digest {
        temporary_index.install(root, expected_index_digest)?;
    } else if live_index != candidate_index {
        return Err(other(
            "Git index changed after host commit; refusing to replace unrelated staged work",
        ));
    }
    write_sync_result(
        plan,
        &SyncResult::Ready {
            context: context.clone(),
            head: recorded_head.to_string(),
            base_head: base_head.to_string(),
            finalized_merge_head: Some(recorded_merge_head.to_string()),
            pending_index_digest: None,
        },
    )
}

fn verify_ready_state(
    context: &SyncContext,
    cwd: &Path,
    recorded_head: &str,
    base_head: &str,
    finalized_merge_head: Option<&str>,
) -> Result<()> {
    crate::worktree_pr::ensure_host_git_safety(cwd)?;
    verify_context_checkout(context, cwd)?;
    let root = Path::new(&context.repo_root);
    if head(root)? != recorded_head {
        return Err(other("HEAD changed after the verified base sync"));
    }
    if merge_head(root)?.is_some() || !unmerged_entries(root)?.is_empty() {
        return Err(other("a merge or unresolved paths remain after base sync"));
    }
    ensure_clean_worktree(
        root,
        "unexpected staged or worktree changes exist after base sync",
    )?;
    if !is_ancestor(root, &context.local_base_head, recorded_head)?
        || !is_ancestor(root, base_head, recorded_head)?
        || !is_ancestor(root, &context.target.expected_remote_head, recorded_head)?
    {
        return Err(other(
            "verified local HEAD is not a fast-forward of the recorded base and PR heads",
        ));
    }
    if let Some(merge_head) = finalized_merge_head {
        verify_merge_commit(root, recorded_head, &context.local_base_head, merge_head)?;
    }
    Ok(())
}

fn verify_merge_commit(
    root: &Path,
    commit: &str,
    first_parent: &str,
    second_parent: &str,
) -> Result<()> {
    let output = git(root, &["rev-list", "--parents", "-n", "1", commit])?;
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|error| other(format!("Git returned invalid commit parents: {error}")))?;
    let actual: Vec<&str> = value.split_whitespace().collect();
    if actual.len() != 3
        || actual[0] != commit
        || actual[1] != first_parent
        || actual[2] != second_parent
    {
        return Err(other(
            "host commit is not the expected two-parent merge commit",
        ));
    }
    let subject = git(root, &["show", "-s", "--format=%s", commit])?;
    if subject
        .stdout
        .strip_suffix(b"\n")
        .unwrap_or(&subject.stdout)
        != MERGE_COMMIT_MESSAGE.as_bytes()
    {
        return Err(other(
            "host merge commit does not have the expected fixed message",
        ));
    }
    Ok(())
}

fn ensure_resolution_scope(
    root: &Path,
    conflict_paths: &BTreeSet<Vec<u8>>,
    untracked_paths_digest: &str,
) -> Result<()> {
    for path in nul_paths(&git(root, &["diff", "--no-ext-diff", "--name-only", "-z"])?.stdout)? {
        if !conflict_paths.contains(&path) {
            return Err(other(
                "resolver changed a non-conflict path; only conflicted files may be edited",
            ));
        }
    }
    let current_untracked = git(root, &["ls-files", "--others", "-z"])?.stdout;
    if digest(&current_untracked) != untracked_paths_digest {
        return Err(other(
            "resolver added or removed untracked or ignored paths outside the conflict",
        ));
    }
    Ok(())
}

fn verify_call_identity(
    context: &SyncContext,
    cwd: &Path,
    pr_number: &str,
    pr_url: Option<&str>,
) -> Result<()> {
    verify_context_checkout(context, cwd)?;
    let requested = pr_number
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| other("PR number must be positive"))?;
    if requested != context.target.number {
        return Err(other(
            "base-sync PR number does not match the recorded target",
        ));
    }
    if let Some(url) = pr_url {
        let (forge, host, repository, number) = parse_pr_url(url)?;
        if number != requested
            || forge != context.target.forge
            || !host.eq_ignore_ascii_case(&context.target.host)
            || !repository.eq_ignore_ascii_case(&context.target.repository)
        {
            return Err(other("base-sync PR URL does not match the recorded target"));
        }
    }
    Ok(())
}

fn verify_context_checkout(context: &SyncContext, cwd: &Path) -> Result<()> {
    let root = repository_root(cwd)?;
    if root.to_string_lossy() != context.repo_root {
        return Err(other(
            "repository does not match the host-owned base-sync record",
        ));
    }
    if current_branch(&root)? != context.local_branch
        || context.local_branch != context.target.head_branch
    {
        return Err(other(
            "current branch does not match the recorded PR branch",
        ));
    }
    Ok(())
}

fn resolve_target(root: &Path, pr_number: &str, pr_url: Option<&str>) -> Result<PullRequestTarget> {
    let number = pr_number
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| other("PR number must be positive"))?;
    let origin = origin_context(root)?;
    verify_origin_context(root, &origin)?;
    let (forge, host, repository) = if let Some(url) = pr_url {
        let (forge, host, repository, url_number) = parse_pr_url(url)?;
        if url_number != number {
            return Err(other("PR URL number does not match the requested PR"));
        }
        ensure_same_repository(&origin, forge, &host, &repository)?;
        (forge, host, repository)
    } else {
        (origin.kind, origin.host.clone(), origin.repository.clone())
    };

    let (url, head_branch, base_branch, api_head, api_number) = match forge {
        ForgeKind::GitHub => query_github_pr(root, number)?,
        ForgeKind::GitLab => query_gitlab_mr(root, number)?,
    };
    if api_number != number {
        return Err(other("forge returned a different pull request number"));
    }
    let (url_forge, url_host, url_repository, url_number) = parse_pr_url(&url)?;
    if url_number != number {
        return Err(other("forge returned a mismatched pull request URL"));
    }
    ensure_same_repository(&origin, url_forge, &url_host, &url_repository)?;
    if url_forge != forge
        || !host.eq_ignore_ascii_case(&url_host)
        || !repository.eq_ignore_ascii_case(&url_repository)
    {
        return Err(other("forge PR identity does not match origin"));
    }
    validate_branch(root, &head_branch)?;
    validate_branch(root, &base_branch)?;
    let head_ref = format!("refs/heads/{head_branch}");
    let expected_remote_head = ls_remote(root, &head_ref)?;
    if api_head
        .as_deref()
        .is_some_and(|head| head != expected_remote_head)
    {
        return Err(other("PR API head does not match the origin branch head"));
    }
    let base_ref = format!("refs/heads/{base_branch}");
    let expected_base_head = ls_remote(root, &base_ref)?;
    Ok(PullRequestTarget {
        number,
        url,
        forge,
        host,
        repository,
        head_branch,
        head_ref,
        expected_remote_head,
        base_branch,
        expected_base_head,
    })
}

fn query_github_pr(
    root: &Path,
    number: u64,
) -> Result<(String, String, String, Option<String>, u64)> {
    let output = external(
        "gh",
        &[
            "pr".into(),
            "view".into(),
            number.to_string(),
            "--json".into(),
            "number,url,state,headRefName,headRefOid,baseRefName,isCrossRepository".into(),
        ],
        root,
    )?;
    let parsed = crate::pr_review_fix::parse_pr_view(&output.stdout, number)?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| other(format!("invalid gh pr view output: {error}")))?;
    let head = json
        .get("headRefOid")
        .and_then(serde_json::Value::as_str)
        .map(|oid| parse_oid(oid.as_bytes(), "PR head"))
        .transpose()?;
    Ok((
        parsed.url,
        parsed.head_branch,
        parsed.base_branch,
        head,
        parsed.number,
    ))
}

fn query_gitlab_mr(
    root: &Path,
    number: u64,
) -> Result<(String, String, String, Option<String>, u64)> {
    let output = external(
        "glab",
        &[
            "mr".into(),
            "view".into(),
            number.to_string(),
            "--output".into(),
            "json".into(),
        ],
        root,
    )?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| other(format!("invalid glab mr view JSON: {error}")))?;
    let text = |key: &str| {
        json.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    };
    let iid = json
        .get("iid")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| json.get("number").and_then(serde_json::Value::as_u64))
        .unwrap_or(0);
    if iid != number || !text("state").eq_ignore_ascii_case("opened") {
        return Err(other(
            "only the matching open GitLab merge request is supported",
        ));
    }
    let same_project = if let (Some(source), Some(target)) =
        (json.get("source_project_id"), json.get("target_project_id"))
    {
        source == target
    } else {
        let source = json
            .pointer("/source_project/path_with_namespace")
            .or_else(|| json.get("source_project_path"))
            .and_then(serde_json::Value::as_str);
        let target = json
            .pointer("/target_project/path_with_namespace")
            .or_else(|| json.get("target_project_path"))
            .and_then(serde_json::Value::as_str);
        matches!((source, target), (Some(source), Some(target)) if source.eq_ignore_ascii_case(target))
    };
    if !same_project {
        return Err(other(
            "fork or mismatched-project merge requests are not supported",
        ));
    }
    let head_branch = text("source_branch").to_string();
    let base_branch = text("target_branch").to_string();
    let url = text("web_url").to_string();
    if head_branch.is_empty() || base_branch.is_empty() || url.is_empty() {
        return Err(other("glab mr view omitted required branch or URL fields"));
    }
    let api_head = json
        .get("sha")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            json.pointer("/diff_refs/head_sha")
                .and_then(serde_json::Value::as_str)
        })
        .map(|oid| parse_oid(oid.as_bytes(), "MR head"))
        .transpose()?;
    Ok((url, head_branch, base_branch, api_head, iid))
}

fn parse_pr_url(url: &str) -> Result<(ForgeKind, String, String, u64)> {
    let parsed = url::Url::parse(url).map_err(|_| other("invalid pull request URL"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(other(
            "pull request URL must be a credential-free HTTPS URL without a port",
        ));
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let mut segments: Vec<&str> = parsed
        .path_segments()
        .ok_or_else(|| other("invalid pull request URL path"))?
        .collect();
    if segments.last() == Some(&"") {
        segments.pop();
    }
    let (forge, repository, number) = if segments.len() == 4 && segments[2] == "pull" {
        let repository = format!("{}/{}", segments[0], segments[1]);
        (ForgeKind::GitHub, repository, segments[3])
    } else if segments.len() >= 5
        && segments[segments.len() - 3] == "-"
        && segments[segments.len() - 2] == "merge_requests"
    {
        let repository = segments[..segments.len() - 3].join("/");
        (ForgeKind::GitLab, repository, segments[segments.len() - 1])
    } else {
        return Err(other("unsupported pull request URL"));
    };
    let number = number
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| other("invalid pull request number in URL"))?;
    crate::forge::resolve_remote(&format!("https://{host}/{repository}.git"), Some(forge))?;
    Ok((forge, host, repository, number))
}

fn origin_context(root: &Path) -> Result<ForgeContext> {
    crate::forge::repository_context(None, root)
}

fn verify_origin_context(root: &Path, expected: &ForgeContext) -> Result<()> {
    for push in [false, true] {
        let args = if push {
            vec!["remote", "get-url", "--all", "--push", "origin"]
        } else {
            vec!["remote", "get-url", "--all", "origin"]
        };
        let output = git(root, &args)?;
        let urls = std::str::from_utf8(&output.stdout)
            .map_err(|_| other("origin URL is not valid UTF-8"))?;
        let mut found = false;
        for url in urls.lines().map(str::trim).filter(|url| !url.is_empty()) {
            found = true;
            if url.contains("://") {
                let parsed = url::Url::parse(url).map_err(|_| other("cannot verify origin URL"))?;
                if parsed.port().is_some() {
                    return Err(other("cannot verify an origin URL with a nonstandard port"));
                }
            }
            let identity = crate::forge::resolve_remote(url, Some(expected.kind))
                .map_err(|_| other("cannot verify origin repository against pull request"))?;
            if identity.kind != expected.kind
                || !identity.host.eq_ignore_ascii_case(&expected.host)
                || !identity
                    .repository
                    .eq_ignore_ascii_case(&expected.repository)
            {
                return Err(other(format!(
                    "origin {} repository does not match pull request",
                    if push { "push" } else { "fetch" }
                )));
            }
        }
        if !found {
            return Err(other("origin has no configured fetch or push URL"));
        }
    }
    Ok(())
}

fn verify_push_endpoint(target: &PullRequestTarget, endpoint: &str) -> Result<()> {
    let identity = crate::forge::resolve_remote(endpoint, Some(target.forge))
        .map_err(|_| other("cannot verify effective origin push endpoint"))?;
    if identity.kind != target.forge
        || !identity.host.eq_ignore_ascii_case(&target.host)
        || !identity.repository.eq_ignore_ascii_case(&target.repository)
    {
        return Err(other(
            "effective origin push endpoint does not match the validated PR repository",
        ));
    }
    Ok(())
}

fn ensure_same_repository(
    origin: &ForgeContext,
    forge: ForgeKind,
    host: &str,
    repository: &str,
) -> Result<()> {
    if origin.kind != forge
        || !origin.host.eq_ignore_ascii_case(host)
        || !origin.repository.eq_ignore_ascii_case(repository)
    {
        return Err(other("pull request repository does not match origin"));
    }
    Ok(())
}

fn validate_branch(root: &Path, branch: &str) -> Result<()> {
    if branch.is_empty() {
        return Err(other("pull request contains an empty branch name"));
    }
    git(root, &["check-ref-format", "--branch", branch])?;
    Ok(())
}

fn remote_head(target: &PullRequestTarget, root: &Path) -> Result<String> {
    ls_remote(root, &target.head_ref)
}

fn ls_remote(root: &Path, reference: &str) -> Result<String> {
    let output = git(root, &["ls-remote", "--exit-code", "origin", reference])?;
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| other("git ls-remote returned invalid UTF-8"))?;
    let mut lines = text.lines();
    let line = lines
        .next()
        .ok_or_else(|| other("origin did not advertise the validated ref"))?;
    if lines.next().is_some() {
        return Err(other(
            "origin returned multiple matches for the validated ref",
        ));
    }
    let mut fields = line.split_whitespace();
    let oid = fields
        .next()
        .ok_or_else(|| other("origin returned an invalid ref"))?;
    let returned_ref = fields
        .next()
        .ok_or_else(|| other("origin returned an invalid ref"))?;
    if fields.next().is_some() || returned_ref != reference {
        return Err(other("origin returned a mismatched ref"));
    }
    parse_oid(oid.as_bytes(), "remote ref")
}

fn verify_origin(target: &PullRequestTarget, root: &Path) -> Result<()> {
    let expected = ForgeContext {
        kind: target.forge,
        host: target.host.clone(),
        repository: target.repository.clone(),
    };
    verify_origin_context(root, &expected)
}

fn ensure_no_merge(root: &Path) -> Result<()> {
    if merge_head(root)?.is_some() {
        return Err(other(
            "refusing to sync while another merge is already in progress",
        ));
    }
    if !unmerged_entries(root)?.is_empty() {
        return Err(other("refusing to sync with pre-existing unmerged paths"));
    }
    Ok(())
}

fn ensure_clean_worktree(root: &Path, message: &str) -> Result<()> {
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?
    .stdout;
    if status.is_empty() {
        Ok(())
    } else {
        Err(other(message))
    }
}

fn repository_root(repo_dir: &Path) -> Result<PathBuf> {
    let output = git(repo_dir, &["rev-parse", "--show-toplevel"])?;
    let root = std::str::from_utf8(&output.stdout)
        .map_err(|error| other(format!("repository root is not valid UTF-8: {error}")))?
        .trim_end_matches(['\r', '\n']);
    if root.is_empty() {
        return Err(CruiseError::NotGitRepository);
    }
    fs::canonicalize(root).map_err(Into::into)
}

fn current_branch(root: &Path) -> Result<String> {
    let output = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    std::str::from_utf8(&output.stdout)
        .map(|value| value.trim_end_matches(['\r', '\n']).to_string())
        .map_err(|error| other(format!("Git returned an invalid branch name: {error}")))
}

fn head(root: &Path) -> Result<String> {
    let output = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    parse_oid(&output.stdout, "HEAD")
}

fn merge_head(root: &Path) -> Result<Option<String>> {
    let args = ["rev-parse", "--verify", "--quiet", "MERGE_HEAD"];
    let output = raw_git(root, &args)?;
    if output.status.success() {
        parse_oid(&output.stdout, "MERGE_HEAD").map(Some)
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(git_error(&args, &output))
    }
}

fn rev_parse(root: &Path, reference: &str) -> Result<Option<String>> {
    let args = ["rev-parse", "--verify", "--quiet", reference];
    let output = raw_git(root, &args)?;
    if output.status.success() {
        parse_oid(&output.stdout, reference).map(Some)
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(git_error(&args, &output))
    }
}

fn parse_oid(bytes: &[u8], name: &str) -> Result<String> {
    let value = std::str::from_utf8(bytes)
        .map_err(|error| other(format!("Git returned an invalid {name}: {error}")))?
        .trim();
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(other(format!("Git returned an invalid {name}")));
    }
    Ok(value.to_ascii_lowercase())
}

fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    let output = raw_git(root, &args)?;
    if output.status.success() {
        Ok(true)
    } else if output.status.code() == Some(1) {
        Ok(false)
    } else {
        Err(git_error(&args, &output))
    }
}

fn unmerged_entries(root: &Path) -> Result<Vec<u8>> {
    Ok(git(root, &["ls-files", "--unmerged", "-z"])?.stdout)
}

fn unmerged_paths(entries: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut paths = BTreeSet::new();
    for entry in entries
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            return Err(other(
                "Git returned a malformed NUL-delimited unmerged entry",
            ));
        };
        let path = &entry[tab + 1..];
        if path.is_empty() {
            return Err(other("Git returned an empty unmerged path"));
        }
        paths.insert(path.to_vec());
    }
    Ok(paths.into_iter().collect())
}

fn nul_paths(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return Err(other("Git returned malformed NUL-delimited paths"));
    }
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

fn literal_pathspecs(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut pathspecs = Vec::new();
    for path in paths {
        pathspecs.extend_from_slice(b":(literal)");
        pathspecs.extend_from_slice(path);
        pathspecs.push(0);
    }
    pathspecs
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(other("invalid encoded conflict path in trusted sync state"));
    }
    pairs
        .iter()
        .map(|digits| {
            let high = hex_digit(digits[0])?;
            let low = hex_digit(digits[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(other("invalid encoded conflict path in trusted sync state")),
    }
}

fn digest(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}

fn write_sync_result(plan: &Path, result: &SyncResult) -> Result<()> {
    let serialized = serde_json::to_string(result).map_err(|error| {
        other(format!(
            "failed to serialize host-owned sync state: {error}"
        ))
    })?;
    crate::artifacts::write(&artifact_root(plan)?, SYNC_RESULT_ARTIFACT, &serialized)
}

fn read_sync_result(plan: &Path) -> Result<SyncResult> {
    let serialized = crate::artifacts::read(&artifact_root(plan)?, SYNC_RESULT_ARTIFACT)
        .map_err(|error| other(format!("failed to read host-owned sync state: {error}")))?;
    serde_json::from_str(&serialized)
        .map_err(|error| other(format!("invalid host-owned sync state: {error}")))
}

fn clear_session_artifact(plan: &Path, name: &str) -> Result<()> {
    let root = artifact_root(plan)?;
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(other(
                "session artifact directory is not a regular directory",
            ));
        }
        Ok(_) => {}
    }
    let relative = crate::artifacts::validate_name(name)?;
    let mut path = root;
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(other("session artifact to clear contains a symlink"));
            }
            Ok(metadata) if index + 1 == components.len() && !metadata.is_file() => {
                return Err(other("session artifact to clear is not a regular file"));
            }
            Ok(metadata) if index + 1 < components.len() && !metadata.is_dir() => {
                return Err(other("session artifact parent is not a regular directory"));
            }
            Ok(_) => {}
        }
    }
    fs::remove_file(path)?;
    Ok(())
}

fn artifact_root(plan: &Path) -> Result<PathBuf> {
    if plan.file_name() != Some(OsStr::new("plan.md")) {
        return Err(other("CRUISE_SYNC_PLAN must name the session plan.md file"));
    }
    let metadata = fs::symlink_metadata(plan)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(other("session plan path must be a regular file"));
    }
    let session_dir = plan
        .parent()
        .ok_or_else(|| other("session plan path has no parent directory"))?;
    Ok(fs::canonicalize(session_dir)?.join(crate::artifacts::ARTIFACTS_DIR))
}

struct TemporaryIndex {
    path: PathBuf,
    git_index: PathBuf,
}

impl TemporaryIndex {
    fn new(root: &Path) -> Result<Self> {
        let output = git(root, &["rev-parse", "--git-path", "index"])?;
        let path = std::str::from_utf8(&output.stdout)
            .map_err(|error| other(format!("Git returned an invalid index path: {error}")))?
            .trim_end_matches(['\r', '\n']);
        let git_index = PathBuf::from(path);
        let git_index = if git_index.is_absolute() {
            git_index
        } else {
            root.join(git_index)
        };
        let metadata = fs::symlink_metadata(&git_index)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(other("Git index is not a regular file"));
        }
        let temporary = std::env::temp_dir().join(format!(
            "cruise-conflict-index-{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        fs::copy(&git_index, &temporary)?;
        Ok(Self {
            path: temporary,
            git_index,
        })
    }

    fn install(&self, root: &Path, expected_digest: &str) -> Result<()> {
        let mut lock_name = self.git_index.as_os_str().to_os_string();
        lock_name.push(".lock");
        let lock_path = PathBuf::from(lock_name);
        let mut lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)?;
        let mut guard = IndexLock {
            path: lock_path,
            armed: true,
        };
        let current = git(root, &["ls-files", "--stage", "-z"])?.stdout;
        if digest(&current) != expected_digest {
            return Err(other(
                "Git index changed while validating conflict resolutions",
            ));
        }
        let mut candidate = fs::File::open(&self.path)?;
        std::io::copy(&mut candidate, &mut lock)?;
        lock.sync_all()?;
        fs::set_permissions(&guard.path, fs::metadata(&self.git_index)?.permissions())?;
        drop(lock);
        fs::rename(&guard.path, &self.git_index)?;
        guard.armed = false;
        Ok(())
    }
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct IndexLock {
    path: PathBuf,
    armed: bool,
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Result<Output> {
    let output = raw_git(root, args)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(git_error(args, &output))
    }
}

fn raw_git(root: &Path, args: &[&str]) -> Result<Output> {
    git_command(root)
        .args(args)
        .output()
        .map_err(|error| other(format!("failed to run git: {error}")))
}

fn git_with_index(root: &Path, args: &[&str], index: &Path) -> Result<Output> {
    let output = git_command(root)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .output()
        .map_err(|error| other(format!("failed to run git: {error}")))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(git_error(args, &output))
    }
}

fn git_with_input_at_index(
    root: &Path,
    args: &[&str],
    input: &[u8],
    index: &Path,
) -> Result<Output> {
    let mut child = git_command(root)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| other(format!("failed to run git: {error}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| other("failed to open git stdin"))?
        .write_all(input)?;
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(git_error(args, &output))
    }
}

fn git_command(root: &Path) -> Command {
    let mut command = crate::worktree_pr::host_git_command();
    command.current_dir(root);
    command
}

fn external(program: &str, args: &[String], cwd: &Path) -> Result<Output> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| other(format!("failed to run {program}: {error}")))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(other(format!(
            "{program} did not return valid pull request metadata"
        )))
    }
}

fn git_error(args: &[&str], output: &Output) -> CruiseError {
    let detail = output.status.code().map_or_else(
        || "terminated without an exit status".to_string(),
        |code| format!("exited with status {code}"),
    );
    let command = if args.contains(&"push") {
        "push".to_string()
    } else {
        args.join(" ")
    };
    other(format!("git {command} {detail}"))
}

fn other(message: impl Into<String>) -> CruiseError {
    CruiseError::Other(message.into())
}
#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use tempfile::TempDir;

    use super::*;

    #[cfg(windows)]
    const DISABLED_GLOBAL_CONFIG: &str = "NUL";
    #[cfg(not(windows))]
    const DISABLED_GLOBAL_CONFIG: &str = "/dev/null";

    struct HostSyncGitEnvironment {
        _guards: Vec<crate::test_support::EnvGuard>,
        _lock: crate::test_support::ProcessLock,
    }

    fn isolated_host_git_environment() -> HostSyncGitEnvironment {
        let lock = crate::test_support::lock_process();
        let guards = vec![
            crate::test_support::EnvGuard::set("GIT_CONFIG_NOSYSTEM", "1"),
            crate::test_support::EnvGuard::set("GIT_CONFIG_GLOBAL", DISABLED_GLOBAL_CONFIG),
        ];
        HostSyncGitEnvironment {
            _guards: guards,
            _lock: lock,
        }
    }

    struct ConflictFixture {
        _git_environment: HostSyncGitEnvironment,
        _temp: TempDir,
        repo: PathBuf,
        plan: PathBuf,
        context: SyncContext,
        merge_head: String,
    }

    fn conflict_fixture() -> ConflictFixture {
        let git_environment = isolated_host_git_environment();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("create fixture: {error}"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("create repo: {error}"));
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.name", "Host Sync Test"]);
        git_ok(
            &repo,
            &["config", "user.email", "host-sync@example.invalid"],
        );
        fs::write(repo.join("conflict.txt"), "base\n")
            .unwrap_or_else(|error| panic!("write conflict file: {error}"));
        fs::write(repo.join("unrelated.txt"), "original\n")
            .unwrap_or_else(|error| panic!("write unrelated file: {error}"));
        git_ok(&repo, &["add", "--all"]);
        git_ok(&repo, &["commit", "-m", "base"]);
        git_ok(&repo, &["branch", "-M", "main"]);
        fs::write(repo.join(".git/info/exclude"), "existing-ignored.txt\n")
            .unwrap_or_else(|error| panic!("write ignore rule: {error}"));
        fs::write(
            repo.join("existing-ignored.txt"),
            "preexisting ignored file\n",
        )
        .unwrap_or_else(|error| panic!("write ignored file: {error}"));
        git_ok(&repo, &["checkout", "-b", "feature"]);
        fs::write(repo.join("conflict.txt"), "feature\n")
            .unwrap_or_else(|error| panic!("write feature file: {error}"));
        git_ok(&repo, &["commit", "-am", "feature change"]);
        let local_base_head =
            head(&repo).unwrap_or_else(|error| panic!("read feature HEAD: {error}"));
        git_ok(&repo, &["checkout", "main"]);
        fs::write(repo.join("conflict.txt"), "base update\n")
            .unwrap_or_else(|error| panic!("write base conflict: {error}"));
        fs::write(repo.join("unrelated.txt"), "base update\n")
            .unwrap_or_else(|error| panic!("write base unrelated: {error}"));
        git_ok(&repo, &["commit", "-am", "base update"]);
        let merge_head = head(&repo).unwrap_or_else(|error| panic!("read base HEAD: {error}"));
        git_ok(&repo, &["checkout", "feature"]);
        let merge = git_output(&repo, &["merge", "main", "--no-edit"]);
        assert!(!merge.status.success(), "fixture merge should conflict");

        let plan = temp.path().join("session/plan.md");
        fs::create_dir_all(plan.parent().unwrap_or_else(|| panic!("plan parent")))
            .unwrap_or_else(|error| panic!("create session: {error}"));
        fs::write(&plan, "approved plan\n").unwrap_or_else(|error| panic!("write plan: {error}"));
        let root =
            repository_root(&repo).unwrap_or_else(|error| panic!("repository root: {error}"));
        let context = SyncContext {
            repo_root: root.to_string_lossy().into_owned(),
            local_branch: "feature".into(),
            local_base_head: local_base_head.clone(),
            target: PullRequestTarget {
                number: 7,
                url: "https://github.com/owner/repo/pull/7".into(),
                forge: ForgeKind::GitHub,
                host: "github.com".into(),
                repository: "owner/repo".into(),
                head_branch: "feature".into(),
                head_ref: "refs/heads/feature".into(),
                expected_remote_head: local_base_head,
                base_branch: "main".into(),
                expected_base_head: merge_head.clone(),
            },
        };
        let paths = unmerged_paths(
            &unmerged_entries(&repo).unwrap_or_else(|error| panic!("read unmerged paths: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse unmerged paths: {error}"));
        write_sync_result(
            &plan,
            &SyncResult::MergeConflict {
                context: context.clone(),
                merge_head: merge_head.clone(),
                conflict_paths: paths.iter().map(|path| encode_hex(path)).collect(),
                index_digest: digest(&git_ok(&repo, &["ls-files", "--stage", "-z"]).stdout),
                untracked_paths_digest: digest(
                    &git_ok(&repo, &["ls-files", "--others", "-z"]).stdout,
                ),
            },
        )
        .unwrap_or_else(|error| panic!("record fixture conflict: {error}"));
        ConflictFixture {
            _git_environment: git_environment,
            _temp: temp,
            repo,
            plan,
            context,
            merge_head,
        }
    }

    fn git_output(repo: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap_or_else(|error| panic!("run git {args:?}: {error}"))
    }

    fn git_ok(repo: &Path, args: &[&str]) -> Output {
        let output = git_output(repo, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn resolver_output(plan: &Path) {
        crate::artifacts::write(
            &artifact_root(plan).unwrap_or_else(|error| panic!("artifact root: {error}")),
            RESOLVER_OUTPUT_ARTIFACT,
            "Resolved only the recorded conflict paths.\n",
        )
        .unwrap_or_else(|error| panic!("write resolver artifact: {error}"));
    }

    fn resolve_conflict(repo: &Path) {
        fs::write(repo.join("conflict.txt"), "resolved\n")
            .unwrap_or_else(|error| panic!("write resolution: {error}"));
    }

    #[test]
    fn commits_only_recorded_conflicts_and_disables_repository_hooks() {
        let fixture = conflict_fixture();
        resolve_conflict(&fixture.repo);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let hooks = fixture.repo.join(".git/hooks");
            let sentinel = fixture.repo.join("hook-ran");
            let hook = hooks.join("pre-commit");
            fs::write(
                &hook,
                format!("#!/bin/sh\ntouch '{}'\nexit 1\n", sentinel.display()),
            )
            .unwrap_or_else(|error| panic!("write hook: {error}"));
            let mut permissions = fs::metadata(&hook)
                .unwrap_or_else(|error| panic!("hook metadata: {error}"))
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&hook, permissions)
                .unwrap_or_else(|error| panic!("chmod hook: {error}"));
            resolver_output(&fixture.plan);

            finish_conflict_commit(&fixture.plan, &fixture.repo)
                .unwrap_or_else(|error| panic!("finish conflict commit: {error}"));

            assert!(
                !sentinel.exists(),
                "repository pre-commit hook must not run"
            );
        }
        #[cfg(not(unix))]
        {
            resolver_output(&fixture.plan);
            finish_conflict_commit(&fixture.plan, &fixture.repo)
                .unwrap_or_else(|error| panic!("finish conflict commit: {error}"));
        }

        assert_eq!(
            String::from_utf8_lossy(&git_ok(&fixture.repo, &["show", "HEAD:conflict.txt"]).stdout),
            "resolved\n"
        );
        assert_eq!(
            String::from_utf8_lossy(&git_ok(&fixture.repo, &["show", "HEAD:unrelated.txt"]).stdout),
            "base update\n",
            "cleanly merged unrelated changes must be preserved"
        );
        verify_merge_commit(
            &fixture.repo,
            &head(&fixture.repo).unwrap_or_else(|error| panic!("read completed HEAD: {error}")),
            &fixture.context.local_base_head,
            &fixture.merge_head,
        )
        .unwrap_or_else(|error| panic!("verify host merge commit: {error}"));
        assert!(
            merge_head(&fixture.repo)
                .unwrap_or_else(|error| panic!("read MERGE_HEAD: {error}"))
                .is_none()
        );
        assert!(
            git_ok(&fixture.repo, &["status", "--porcelain=v1", "-z"])
                .stdout
                .is_empty(),
            "successful host merge should leave a clean worktree"
        );

        let SyncResult::Ready {
            finalized_merge_head,
            ..
        } = read_sync_result(&fixture.plan)
            .unwrap_or_else(|error| panic!("read final state: {error}"))
        else {
            panic!("conflict completion must persist a ready state");
        };
        assert_eq!(
            finalized_merge_head.as_deref(),
            Some(fixture.merge_head.as_str())
        );
        finish_conflict_commit(&fixture.plan, &fixture.repo).unwrap_or_else(|error| {
            panic!("finalized conflict should resume idempotently: {error}")
        });
    }

    #[test]
    fn missing_resolver_output_leaves_conflict_resumable() {
        let fixture = conflict_fixture();
        resolve_conflict(&fixture.repo);
        let before = digest(&git_ok(&fixture.repo, &["ls-files", "--stage", "-z"]).stdout);

        let error = finish_conflict_commit(&fixture.plan, &fixture.repo)
            .err()
            .unwrap_or_else(|| panic!("missing output artifact must block commit"));
        assert!(error.to_string().contains("fresh host-written"));
        assert_eq!(
            digest(&git_ok(&fixture.repo, &["ls-files", "--stage", "-z"]).stdout),
            before
        );
        assert_eq!(
            merge_head(&fixture.repo)
                .unwrap_or_else(|error| panic!("read MERGE_HEAD: {error}"))
                .as_deref(),
            Some(fixture.merge_head.as_str())
        );

        resolver_output(&fixture.plan);
        finish_conflict_commit(&fixture.plan, &fixture.repo)
            .unwrap_or_else(|error| panic!("valid retry should complete merge: {error}"));
    }

    #[test]
    fn refuses_unrelated_resolver_edits_without_losing_the_merge() {
        let fixture = conflict_fixture();
        resolver_output(&fixture.plan);
        resolve_conflict(&fixture.repo);
        fs::write(
            fixture.repo.join("unrelated.txt"),
            "resolver changed unrelated path\n",
        )
        .unwrap_or_else(|error| panic!("write unrelated edit: {error}"));
        let before = head(&fixture.repo).unwrap_or_else(|error| panic!("read HEAD: {error}"));

        let error = finish_conflict_commit(&fixture.plan, &fixture.repo)
            .err()
            .unwrap_or_else(|| panic!("non-conflict resolver edits must be refused"));
        assert!(error.to_string().contains("non-conflict path"));
        assert_eq!(
            head(&fixture.repo).unwrap_or_else(|error| panic!("read HEAD: {error}")),
            before
        );
        assert!(
            merge_head(&fixture.repo)
                .unwrap_or_else(|error| panic!("read MERGE_HEAD: {error}"))
                .is_some()
        );

        git_ok(&fixture.repo, &["checkout", "--", "unrelated.txt"]);
        finish_conflict_commit(&fixture.plan, &fixture.repo)
            .unwrap_or_else(|error| panic!("valid retry should complete merge: {error}"));
    }

    #[test]
    fn refuses_resolver_staging_and_preserves_original_index() {
        let fixture = conflict_fixture();
        resolver_output(&fixture.plan);
        resolve_conflict(&fixture.repo);
        git_ok(&fixture.repo, &["add", "conflict.txt"]);

        let error = finish_conflict_commit(&fixture.plan, &fixture.repo)
            .err()
            .unwrap_or_else(|| panic!("prompt changes to the index must be refused"));
        assert!(error.to_string().contains("unmerged paths do not match"));
        assert!(
            merge_head(&fixture.repo)
                .unwrap_or_else(|error| panic!("read MERGE_HEAD: {error}"))
                .is_some()
        );
    }

    #[test]
    fn new_ignored_paths_and_failed_commit_preserve_a_resumable_conflict() {
        let fixture = conflict_fixture();
        resolver_output(&fixture.plan);
        resolve_conflict(&fixture.repo);
        let original_index = digest(&git_ok(&fixture.repo, &["ls-files", "--stage", "-z"]).stdout);

        fs::write(
            fixture.repo.join(".git/info/exclude"),
            "existing-ignored.txt\nignored-after-prompt.txt\n",
        )
        .unwrap_or_else(|error| panic!("write ignore rules: {error}"));
        fs::write(fixture.repo.join("ignored-after-prompt.txt"), "unrelated\n")
            .unwrap_or_else(|error| panic!("create ignored file: {error}"));
        let error = finish_conflict_commit(&fixture.plan, &fixture.repo)
            .err()
            .unwrap_or_else(|| panic!("new ignored paths must block conflict commit"));
        assert!(error.to_string().contains("untracked or ignored"));
        fs::remove_file(fixture.repo.join("ignored-after-prompt.txt"))
            .unwrap_or_else(|error| panic!("remove ignored file: {error}"));
        assert_eq!(
            digest(&git_ok(&fixture.repo, &["ls-files", "--stage", "-z"]).stdout),
            original_index
        );

        git_ok(&fixture.repo, &["config", "user.name", ""]);
        git_ok(&fixture.repo, &["config", "user.email", ""]);
        assert!(
            finish_conflict_commit(&fixture.plan, &fixture.repo).is_err(),
            "missing commit identity must fail"
        );
        assert_eq!(
            digest(&git_ok(&fixture.repo, &["ls-files", "--stage", "-z"]).stdout),
            original_index,
            "commit failure must not replace the real unmerged index"
        );
        assert_ne!(
            unmerged_entries(&fixture.repo)
                .unwrap_or_else(|error| panic!("read unresolved entries: {error}"))
                .len(),
            0
        );
        assert!(
            merge_head(&fixture.repo)
                .unwrap_or_else(|error| panic!("read MERGE_HEAD: {error}"))
                .is_some()
        );

        git_ok(&fixture.repo, &["config", "user.name", "Host Sync Test"]);
        git_ok(
            &fixture.repo,
            &["config", "user.email", "host-sync@example.invalid"],
        );
        finish_conflict_commit(&fixture.plan, &fixture.repo)
            .unwrap_or_else(|error| panic!("valid retry should finish merge: {error}"));
    }

    #[test]
    fn successful_fast_forward_sync_is_an_unchanged_head_only_noop() {
        let _git_environment = isolated_host_git_environment();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("create fixture: {error}"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("create repo: {error}"));
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.name", "Host Sync Test"]);
        git_ok(
            &repo,
            &["config", "user.email", "host-sync@example.invalid"],
        );
        fs::write(repo.join("tracked.txt"), "base\n")
            .unwrap_or_else(|error| panic!("write tracked file: {error}"));
        git_ok(&repo, &["add", "--all"]);
        git_ok(&repo, &["commit", "-m", "base"]);
        git_ok(&repo, &["branch", "-M", "feature"]);
        let local_base_head =
            head(&repo).unwrap_or_else(|error| panic!("read pre-sync HEAD: {error}"));
        fs::write(repo.join("tracked.txt"), "base sync\n")
            .unwrap_or_else(|error| panic!("write base-sync change: {error}"));
        git_ok(&repo, &["add", "tracked.txt"]);
        git_ok(&repo, &["commit", "-m", "fast-forward base"]);
        let head = head(&repo).unwrap_or_else(|error| panic!("read sync HEAD: {error}"));
        let plan = temp.path().join("session/plan.md");
        fs::create_dir_all(plan.parent().unwrap_or_else(|| panic!("plan parent")))
            .unwrap_or_else(|error| panic!("create session: {error}"));
        fs::write(&plan, "approved plan\n").unwrap_or_else(|error| panic!("write plan: {error}"));
        let context = SyncContext {
            repo_root: repository_root(&repo)
                .unwrap_or_else(|error| panic!("repository root: {error}"))
                .to_string_lossy()
                .into_owned(),
            local_branch: "feature".into(),
            local_base_head: local_base_head.clone(),
            target: PullRequestTarget {
                number: 11,
                url: "https://github.com/owner/repo/pull/11".into(),
                forge: ForgeKind::GitHub,
                host: "github.com".into(),
                repository: "owner/repo".into(),
                head_branch: "feature".into(),
                head_ref: "refs/heads/feature".into(),
                expected_remote_head: local_base_head,
                base_branch: "main".into(),
                expected_base_head: head.clone(),
            },
        };
        write_sync_result(
            &plan,
            &SyncResult::Ready {
                context,
                head: head.clone(),
                base_head: head.clone(),
                finalized_merge_head: None,
                pending_index_digest: None,
            },
        )
        .unwrap_or_else(|error| panic!("record successful sync: {error}"));

        finish_conflict_commit(&plan, &repo)
            .unwrap_or_else(|error| panic!("successful sync should be a no-op: {error}"));
        assert_eq!(
            super::head(&repo).unwrap_or_else(|error| panic!("read HEAD: {error}")),
            head
        );
    }

    #[test]
    fn parses_only_supported_forge_pull_request_refs() {
        assert_eq!(
            parse_pr_url("https://github.com/owner/repo/pull/12")
                .unwrap_or_else(|error| panic!("parse GitHub URL: {error}")),
            (
                ForgeKind::GitHub,
                "github.com".into(),
                "owner/repo".into(),
                12
            )
        );
        assert_eq!(
            parse_pr_url("https://gitlab.example/group/subgroup/repo/-/merge_requests/13")
                .unwrap_or_else(|error| panic!("parse GitLab URL: {error}")),
            (
                ForgeKind::GitLab,
                "gitlab.example".into(),
                "group/subgroup/repo".into(),
                13
            )
        );
        assert!(parse_pr_url("https://github.com/owner/repo/pull/12?tab=files").is_err());
        assert!(parse_pr_url("https://github.com/owner/fork/pull/12").is_ok());
    }
    #[cfg(unix)]
    struct FakeForgeFixture {
        repo: PathBuf,
        bare: PathBuf,
        plan: PathBuf,
        fake_bin: PathBuf,
        fake_home: PathBuf,
        pr_head: String,
        base_head: String,
    }

    #[cfg(unix)]
    fn fake_forge_fixture(root: &Path) -> FakeForgeFixture {
        let repo = root.join("repo");
        let bare = root.join("origin.git");
        let plan = root.join("session/plan.md");
        let fake_bin = root.join("fake-bin");
        let fake_home = root.join("fake-home");
        fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("create repo: {error}"));
        fs::create_dir_all(plan.parent().unwrap_or_else(|| panic!("plan parent")))
            .unwrap_or_else(|error| panic!("create session: {error}"));
        fs::create_dir_all(&fake_bin).unwrap_or_else(|error| panic!("create fake bin: {error}"));
        fs::create_dir_all(&fake_home).unwrap_or_else(|error| panic!("create fake home: {error}"));
        fs::write(&plan, "approved plan\n").unwrap_or_else(|error| panic!("write plan: {error}"));

        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.name", "Fake Forge Test"]);
        git_ok(
            &repo,
            &["config", "user.email", "fake-forge@example.invalid"],
        );
        fs::write(repo.join("conflict.txt"), "base\n")
            .unwrap_or_else(|error| panic!("write base conflict: {error}"));
        git_ok(&repo, &["add", "--all"]);
        git_ok(&repo, &["commit", "-m", "base"]);
        git_ok(&repo, &["branch", "-M", "main"]);
        git_ok(&repo, &["checkout", "-b", "feature"]);
        fs::write(repo.join("conflict.txt"), "feature\n")
            .unwrap_or_else(|error| panic!("write feature: {error}"));
        git_ok(&repo, &["commit", "-am", "feature"]);
        let pr_head = head(&repo).unwrap_or_else(|error| panic!("read PR head: {error}"));
        git_ok(&repo, &["checkout", "main"]);
        fs::write(repo.join("conflict.txt"), "base update\n")
            .unwrap_or_else(|error| panic!("write base update: {error}"));
        git_ok(&repo, &["commit", "-am", "base update"]);
        let base_head = head(&repo).unwrap_or_else(|error| panic!("read base head: {error}"));
        git_ok(&repo, &["checkout", "feature"]);
        fs::create_dir_all(&bare).unwrap_or_else(|error| panic!("create bare remote: {error}"));
        git_ok(&bare, &["init", "--bare", "-q"]);
        git_ok(
            &repo,
            &[
                "push",
                bare.to_str().unwrap_or_else(|| panic!("bare path")),
                "main",
                "feature",
            ],
        );
        git_ok(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );

        FakeForgeFixture {
            repo,
            bare,
            plan,
            fake_bin,
            fake_home,
            pr_head,
            base_head,
        }
    }

    #[cfg(unix)]
    fn install_fake_forge_tools(fixture: &FakeForgeFixture) {
        use std::os::unix::fs::PermissionsExt as _;

        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let real_git = std::env::split_paths(&original_path)
            .map(|directory| directory.join("git"))
            .find(|path| path.is_file())
            .unwrap_or_else(|| panic!("find real git executable"));
        let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let fake_git = format!(
            r#"#!/bin/bash
set -eu
REAL_GIT={}
BARE={}
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
global=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --no-optional-locks) global+=("$1"); shift ;;
        -c) global+=("$1" "$2"); shift 2 ;;
        *) break ;;
    esac
done
# Bash 3.2 treats an empty argument array as unset under nounset.
exec_git() {{
    exec "$REAL_GIT" ${{global[@]+"${{global[@]}}"}} "$@"
}}
case "$1" in
    remote)
        test "$2" = get-url
        printf '%s\n' 'https://github.com/owner/repo.git' ;;
    ls-remote)
        test "$2" = --exit-code
        case "$3" in origin|"$BARE") ;; *) exit 91 ;; esac
        shift 3
        exec_git ls-remote --exit-code "$BARE" "$@" ;;
    fetch)
        test "$5" = origin
        exec_git "$1" "$2" "$3" "$4" "$BARE" "$6" ;;
    push)
        test "$6" = 'https://github.com/owner/repo.git'
        exec_git "$1" "$2" "$3" "$4" "$5" "$BARE" "$7" ;;
    *)
        exec_git "$@" ;;
esac
"#,
            quote(&real_git),
            quote(&fixture.bare),
        );
        let git_wrapper = fixture.fake_bin.join("git");
        fs::write(&git_wrapper, fake_git).unwrap_or_else(|error| panic!("write fake git: {error}"));
        let gh_json = serde_json::json!({
            "number": 7,
            "url": "https://github.com/owner/repo/pull/7",
            "state": "OPEN",
            "headRefName": "feature",
            "headRefOid": fixture.pr_head,
            "baseRefName": "main",
            "isCrossRepository": false
        })
        .to_string();
        let fake_gh = fixture.fake_bin.join("gh");
        fs::write(&fake_gh, format!("#!/bin/sh\nprintf '%s\\n' '{gh_json}'\n"))
            .unwrap_or_else(|error| panic!("write fake gh: {error}"));
        for executable in [&git_wrapper, &fake_gh] {
            let mut permissions = fs::metadata(executable)
                .unwrap_or_else(|error| panic!("read fake executable metadata: {error}"))
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(executable, permissions)
                .unwrap_or_else(|error| panic!("make fake executable: {error}"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn fake_forge_sync_and_push_use_only_a_disposable_bare_remote() {
        let _git_environment = isolated_host_git_environment();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("create fixture: {error}"));
        let fixture = fake_forge_fixture(temp.path());
        install_fake_forge_tools(&fixture);
        let FakeForgeFixture {
            repo,
            bare,
            plan,
            fake_bin,
            fake_home,
            base_head,
            ..
        } = fixture;

        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![fake_bin];
        paths.extend(std::env::split_paths(&original_path));
        let path =
            std::env::join_paths(paths).unwrap_or_else(|error| panic!("build fake PATH: {error}"));
        let output = Command::new(
            std::env::current_exe().unwrap_or_else(|error| panic!("current test exe: {error}")),
        )
        .args([
            "--exact",
            "host_sync::tests::fake_forge_child",
            "--nocapture",
        ])
        .env("PATH", path)
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", &fake_home)
        .env_remove("CRUISE_FORGE")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env("CRUISE_HOST_SYNC_CHILD", "1")
        .env("CRUISE_HOST_SYNC_REPO", &repo)
        .env("CRUISE_HOST_SYNC_PLAN", &plan)
        .env("CRUISE_HOST_SYNC_BASE_HEAD", base_head)
        .env("CRUISE_HOST_SYNC_BARE", &bare)
        .output()
        .unwrap_or_else(|error| panic!("run isolated fake-forge test: {error}"));
        assert!(
            output.status.success(),
            "isolated fake-forge test failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn fake_forge_child() {
        if std::env::var_os("CRUISE_HOST_SYNC_CHILD").is_none() {
            return;
        }
        let repo = PathBuf::from(
            std::env::var_os("CRUISE_HOST_SYNC_REPO")
                .unwrap_or_else(|| panic!("missing fake-forge repo")),
        );
        let plan = PathBuf::from(
            std::env::var_os("CRUISE_HOST_SYNC_PLAN")
                .unwrap_or_else(|| panic!("missing fake-forge plan")),
        );
        let base_head = std::env::var("CRUISE_HOST_SYNC_BASE_HEAD")
            .unwrap_or_else(|error| panic!("missing fake-forge base head: {error}"));
        let bare = std::env::var_os("CRUISE_HOST_SYNC_BARE")
            .unwrap_or_else(|| panic!("missing fake-forge bare remote"));

        let selected_base = clear_sync_result(
            &plan,
            &repo,
            "7",
            Some("https://github.com/owner/repo/pull/7"),
        )
        .unwrap_or_else(|error| panic!("clear host sync state: {error}"));
        assert_eq!(selected_base, base_head);
        let sync_result = crate::worktree_pr::sync_validated_base(&repo, &selected_base)
            .map_err(|(failure, _error)| failure);
        record_sync_result(
            &plan,
            &repo,
            "7",
            Some("https://github.com/owner/repo/pull/7"),
            sync_result,
        )
        .unwrap_or_else(|error| panic!("record verified conflict: {error}"));
        resolver_output(&plan);
        resolve_conflict(&repo);
        finish_conflict_commit(&plan, &repo)
            .unwrap_or_else(|error| panic!("finish verified conflict: {error}"));
        push_after_sync(&plan, &repo).unwrap_or_else(|error| panic!("guarded local push: {error}"));

        let remote = git_output(
            &repo,
            &[
                "ls-remote",
                "--exit-code",
                bare.to_str().unwrap_or_else(|| panic!("bare path")),
                "refs/heads/feature",
            ],
        );
        assert!(
            remote.status.success(),
            "read disposable bare remote: {}",
            String::from_utf8_lossy(&remote.stderr)
        );
        let remote_oid = std::str::from_utf8(&remote.stdout)
            .unwrap_or_else(|error| panic!("decode bare remote result: {error}"))
            .split_whitespace()
            .next()
            .unwrap_or_else(|| panic!("bare remote omitted feature"));
        assert_eq!(
            remote_oid,
            head(&repo).unwrap_or_else(|error| panic!("read pushed HEAD: {error}"))
        );
    }
}
