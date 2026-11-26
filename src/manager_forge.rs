//! Exact, read-only forge observation and explicit publication primitives for Goals.
//!
//! Callers must durably save publication intent before invoking `push` or `create`.
//! An uncertain create must be reconciled with `lookup`, never blindly retried.

use crate::forge::{ForgeContext, ForgeKind};
use crate::manager::GoalStatus;
use command_group::CommandGroup;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PullRequest {
    pub number: u64,
    pub url: String,
    pub repository: String,
    pub source: String,
    pub target: String,
    pub head_sha: String,
    pub state: PullRequestState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PullRequestState {
    Open,
    Closed,
    Merged,
}

fn read_output(mut reader: impl Read) -> Result<Vec<u8>, String> {
    const LIMIT: usize = 16 * 1024 * 1024;
    let mut output = Vec::new();
    let mut buf = [0u8; 8192];
    let mut overflow = false;
    loop {
        let count = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if output.len() + count <= LIMIT {
            output.extend_from_slice(&buf[..count]);
        } else {
            overflow = true;
        }
    }
    if overflow {
        Err("forge output exceeds 16 MiB".into())
    } else {
        Ok(output)
    }
}

fn run(
    dir: &Path,
    program: &str,
    args: &[String],
    cancel: &crate::cancellation::CancellationToken,
) -> Result<String, String> {
    run_inner(dir, program, args, cancel, None)
}

/// Run an external effect only while the Goal is durably in `status`.
pub(crate) fn run_effect(
    dir: &Path,
    program: &str,
    args: &[String],
    cancel: &crate::cancellation::CancellationToken,
    goal_id: &str,
    status: &GoalStatus,
) -> Result<String, String> {
    run_inner(dir, program, args, cancel, Some((goal_id, status)))
}

fn run_inner(
    dir: &Path,
    program: &str,
    args: &[String],
    cancel: &crate::cancellation::CancellationToken,
    gate: Option<(&str, &GoalStatus)>,
) -> Result<String, String> {
    if cancel.is_cancelled() {
        return Err(format!("{program} cancelled before start"));
    }
    let gate = gate
        .map(|(id, status)| {
            crate::manager_publication::effect_guard(dir, id, cancel, status)
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GLAB_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .group_spawn()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    drop(gate);
    let mut stdout = child.inner().stdout.take().ok_or("missing stdout pipe")?;
    let mut stderr = child.inner().stderr.take().ok_or("missing stderr pipe")?;
    let out_reader = std::thread::spawn(move || read_output(&mut stdout));
    let err_reader = std::thread::spawn(move || read_output(&mut stderr));
    let mut cancelled = false;
    let status = loop {
        if cancel.is_cancelled() {
            cancelled = true;
            let _ = child.kill();
            break child
                .wait()
                .map_err(|e| format!("cannot wait for {program}: {e}"))?;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {program}: {e}"));
            }
        }
    };
    while !out_reader.is_finished() || !err_reader.is_finished() {
        if cancel.is_cancelled() {
            let _ = child.kill();
            return Err(format!(
                "{program} was cancelled while draining output; outcome may be uncertain"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let stdout = out_reader.join().map_err(|_| "stdout reader failed")??;
    let stderr = err_reader.join().map_err(|_| "stderr reader failed")??;
    if cancelled || cancel.is_cancelled() {
        return Err(format!("{program} was cancelled; outcome may be uncertain"));
    }
    if !status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    String::from_utf8(stdout).map_err(|e| format!("invalid {program} output: {e}"))
}

fn api(
    dir: &Path,
    ctx: &ForgeContext,
    endpoint: &str,
    paginate: bool,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<serde_json::Value, String> {
    let mut args = vec!["api".into(), "--hostname".into(), ctx.host.clone()];
    if paginate {
        args.push("--paginate".into());
    }
    args.push(endpoint.into());
    let text = run(
        dir,
        match ctx.kind {
            ForgeKind::GitHub => "gh",
            ForgeKind::GitLab => "glab",
        },
        &args,
        cancel,
    )?;
    // Pagination emits consecutive JSON arrays, rather than one JSON document.
    let values: Result<Vec<serde_json::Value>, _> = serde_json::Deserializer::from_str(&text)
        .into_iter()
        .collect();
    let values = values.map_err(|e| format!("invalid forge JSON: {e}"))?;
    if paginate {
        if values.is_empty() {
            return Err("empty forge list response".into());
        }
        let mut all = Vec::new();
        for value in values {
            all.extend(
                value
                    .as_array()
                    .ok_or("forge list is not an array")?
                    .iter()
                    .cloned(),
            );
        }
        Ok(serde_json::Value::Array(all))
    } else {
        if values.len() != 1 {
            return Err("ambiguous forge response".into());
        }
        values
            .into_iter()
            .next()
            .ok_or_else(|| "ambiguous forge response".into())
    }
}

fn field<'a>(v: &'a serde_json::Value, pointer: &str) -> Result<&'a str, String> {
    v.pointer(pointer)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("forge response missing {pointer}"))
}

fn parse(ctx: &ForgeContext, v: &serde_json::Value) -> Result<PullRequest, String> {
    let (number, url, repository, source, target, head, state) = match ctx.kind {
        ForgeKind::GitHub => {
            let state = field(v, "/state")?;
            let merged = !v
                .pointer("/merged_at")
                .ok_or("missing GitHub merge evidence")?
                .is_null();
            let state = match (state, merged) {
                ("closed", true) => PullRequestState::Merged,
                ("closed", false) => PullRequestState::Closed,
                ("open", false) => PullRequestState::Open,
                _ => return Err("ambiguous GitHub PR state".into()),
            };
            (
                v.pointer("/number"),
                field(v, "/html_url")?,
                field(v, "/base/repo/full_name")?,
                field(v, "/head/ref")?,
                field(v, "/base/ref")?,
                field(v, "/head/sha")?,
                state,
            )
        }
        ForgeKind::GitLab => {
            let state = match field(v, "/state")? {
                "opened" => PullRequestState::Open,
                "closed" => PullRequestState::Closed,
                "merged" => PullRequestState::Merged,
                _ => return Err("ambiguous GitLab MR state".into()),
            };
            // Project identity is scoped by the API endpoint. Reject cross-project source MRs.
            let source_id = v
                .pointer("/source_project_id")
                .and_then(serde_json::Value::as_u64)
                .ok_or("missing source project ID")?;
            let target_id = v
                .pointer("/target_project_id")
                .and_then(serde_json::Value::as_u64)
                .ok_or("missing target project ID")?;
            if source_id != target_id {
                return Err("merge request source belongs to another repository".into());
            }
            (
                v.pointer("/iid"),
                field(v, "/web_url")?,
                ctx.repository.as_str(),
                field(v, "/source_branch")?,
                field(v, "/target_branch")?,
                field(v, "/sha")?,
                state,
            )
        }
    };
    let number = number
        .and_then(serde_json::Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or("missing PR number")?;
    Ok(PullRequest {
        number,
        url: url.into(),
        repository: repository.into(),
        source: source.into(),
        target: target.into(),
        head_sha: head.into(),
        state,
    })
}

fn encoded(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn endpoint(ctx: &ForgeContext) -> String {
    match ctx.kind {
        ForgeKind::GitHub => format!("repos/{}/pulls", ctx.repository),
        ForgeKind::GitLab => format!("projects/{}/merge_requests", encoded(&ctx.repository)),
    }
}

/// Lookup all states, rejecting duplicate or cross-repository matches. Errors never mean absence.
/// When no target is given, a PR from this source to any base is evidence, not absence.
pub(crate) fn lookup(
    dir: &Path,
    ctx: &ForgeContext,
    source: &str,
    target: Option<&str>,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<Option<PullRequest>, String> {
    let query = format!("{}?state=all&per_page=100", endpoint(ctx));
    let values = api(dir, ctx, &query, true, cancel)?;
    let mut found = None;
    for value in values.as_array().ok_or("forge list is not an array")? {
        let (s, t) = match ctx.kind {
            ForgeKind::GitHub => (field(value, "/head/ref")?, field(value, "/base/ref")?),
            ForgeKind::GitLab => (
                field(value, "/source_branch")?,
                field(value, "/target_branch")?,
            ),
        };
        if s != source || target.is_some_and(|target| t != target) {
            continue;
        }
        let pr = parse(ctx, value)?;
        if pr.repository != ctx.repository {
            return Err("forge repository identity mismatch".into());
        }
        if ctx.kind == ForgeKind::GitHub && field(value, "/head/repo/full_name")? != ctx.repository
        {
            return Err("PR source belongs to another repository".into());
        }
        if found.replace(pr).is_some() {
            return Err("multiple matching PRs require human resolution".into());
        }
    }
    Ok(found)
}

pub(crate) fn lookup_title_retry(
    dir: &Path,
    ctx: &ForgeContext,
    source: &str,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<Option<PullRequest>, String> {
    // A public PR list alone does not prove authentication on this host.
    let user = api(dir, ctx, "user", false, cancel)?;
    if !user["id"].as_u64().is_some_and(|id| id > 0) {
        return Err("cannot establish authenticated GitHub identity".into());
    }
    lookup(dir, ctx, source, None, cancel)
}

/// Observe a saved PR identity. Any drift is an error, not completion.
pub(crate) fn observe(
    dir: &Path,
    ctx: &ForgeContext,
    expected: &PullRequest,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<PullRequestState, String> {
    let value = api(
        dir,
        ctx,
        &format!("{}/{}", endpoint(ctx), expected.number),
        false,
        cancel,
    )?;
    let actual = parse(ctx, &value)?;
    if actual.number != expected.number
        || actual.url != expected.url
        || actual.repository != expected.repository
        || actual.repository != ctx.repository
        || actual.source != expected.source
        || actual.target != expected.target
        || actual.head_sha != expected.head_sha
    {
        return Err("PR identity or verified head has drifted".into());
    }
    if ctx.kind == ForgeKind::GitHub && field(&value, "/head/repo/full_name")? != ctx.repository {
        return Err("PR source repository has drifted".into());
    }
    Ok(actual.state)
}

/// Push only the verified object to a dedicated branch, without force.
pub(crate) fn remote_head(
    dir: &Path,
    branch: &str,
    cancel: &crate::cancellation::CancellationToken,
) -> Result<String, String> {
    run(
        dir,
        "git",
        &[
            "ls-remote".into(),
            "--heads".into(),
            "origin".into(),
            format!("refs/heads/{branch}"),
        ],
        cancel,
    )
    .map(|out| out.trim().to_owned())
}

pub(crate) fn push(
    dir: &Path,
    sha: &str,
    branch: &str,
    cancel: &crate::cancellation::CancellationToken,
    goal_id: &str,
) -> Result<(), String> {
    if !(sha.len() == 40 || sha.len() == 64)
        || !sha.bytes().all(|b| b.is_ascii_hexdigit())
        || branch.is_empty()
        || branch.starts_with('-')
        || branch.contains(':')
    {
        return Err("invalid verified SHA or publication branch".into());
    }
    run_effect(
        dir,
        "git",
        &[
            "push".into(),
            "origin".into(),
            format!("{sha}:refs/heads/{branch}"),
        ],
        cancel,
        goal_id,
        &GoalStatus::Verifying,
    )?;
    Ok(())
}

pub(crate) fn github_title_prefix(title: &str) -> Option<&str> {
    const LIMIT: usize = 256;
    if title.len() <= LIMIT {
        return None;
    }
    let mut chars = title.char_indices();
    let (end, _) = chars.nth(LIMIT - 1)?;
    chars.next()?;
    Some(&title[..end])
}

#[derive(Clone, Copy)]
pub(crate) struct CreateRequest<'a> {
    pub source: &'a str,
    pub target: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

/// Create once after an authoritative absence lookup. The caller must reconcile any error.
pub(crate) fn create(
    dir: &Path,
    ctx: &ForgeContext,
    request: CreateRequest<'_>,
    cancel: &crate::cancellation::CancellationToken,
    goal_id: &str,
) -> Result<(), String> {
    let CreateRequest {
        source,
        target,
        title,
        body,
    } = request;
    let repo = format!("{}/{}", ctx.host, ctx.repository);
    let (program, args): (&str, Vec<String>) = match ctx.kind {
        ForgeKind::GitHub => {
            let (title, body) = match github_title_prefix(title) {
                Some(prefix) => (
                    format!("{prefix}…"),
                    format!("{body}\n\n## Goal objective\n\n{title}"),
                ),
                None => (title.into(), body.into()),
            };
            (
                "gh",
                vec![
                    "pr".into(),
                    "create".into(),
                    "--repo".into(),
                    repo,
                    "--head".into(),
                    source.into(),
                    "--base".into(),
                    target.into(),
                    "--title".into(),
                    title,
                    "--body".into(),
                    body,
                ],
            )
        }
        ForgeKind::GitLab => (
            "glab",
            vec![
                "mr".into(),
                "create".into(),
                "--repo".into(),
                repo,
                "--source-branch".into(),
                source.into(),
                "--target-branch".into(),
                target.into(),
                "--title".into(),
                title.into(),
                "--description".into(),
                body.into(),
                "--yes".into(),
            ],
        ),
    };
    run_effect(dir, program, &args, cancel, goal_id, &GoalStatus::Verifying)?;
    Ok(())
}
