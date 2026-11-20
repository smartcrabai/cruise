//! Opt-in GitHub review bot post-processing loop (after-pr only).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cancellation::CancellationToken;
use crate::config::ReviewerScope;
use crate::engine::{ExecutionContext, PromptStepOutput, run_prompt_step};
use crate::error::{CruiseError, Result};
use crate::step::GitHubReviewStep;
use crate::variable::VariableStore;

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const HOST_REVIEW_COMMIT_MESSAGE: &str = "fix: address pull request review feedback";
const STEP_PLACEHOLDER: &str = "github-review";

const REVIEWS_QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!,$after:String){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviews(first:100,after:$after){pageInfo{hasNextPage endCursor}nodes{author{login}commit{oid}}}}}}";
const THREADS_QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!,$after:String){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor}nodes{id isResolved viewerCanReply viewerCanResolve path line comments(first:100){pageInfo{hasNextPage endCursor}nodes{author{login}body}}}}}}}";
const COMMENTS_QUERY: &str = "query($id:ID!,$after:String){node(id:$id){... on PullRequestReviewThread{comments(first:100,after:$after){pageInfo{hasNextPage endCursor}nodes{author{login}body}}}}}";
const REPLY_MUTATION: &str = "mutation($threadId:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$threadId,body:$body}){comment{id}}}";
const RESOLVE_MUTATION: &str = "mutation($threadId:ID!){resolveReviewThread(input:{threadId:$threadId}){thread{id isResolved}}}";

/// Pull request addressed through the `gh` auth context of `working_dir`.
#[derive(Debug, Clone)]
pub struct PullRequestRef {
    pub number: u64,
    pub working_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComment {
    pub author: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThread {
    pub id: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub is_resolved: bool,
    pub viewer_can_reply: bool,
    pub viewer_can_resolve: bool,
    /// All comments; the first one is the thread starter.
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewAction {
    pub thread_id: String,
    pub reply: Option<String>,
    pub resolve: bool,
}

fn failed(detail: impl Into<String>) -> CruiseError {
    CruiseError::GitHubReviewFailed {
        step: STEP_PLACEHOLDER.to_string(),
        detail: detail.into(),
    }
}

/// Attribute a failure to the step. Everything except cancellation becomes
/// `GitHubReviewFailed` so an incomplete loop can never end as a warning.
fn with_step(error: CruiseError, step: &str) -> CruiseError {
    match error {
        CruiseError::Interrupted => CruiseError::Interrupted,
        CruiseError::GitHubReviewFailed { detail, .. } => CruiseError::GitHubReviewFailed {
            step: step.to_string(),
            detail,
        },
        other => CruiseError::GitHubReviewFailed {
            step: step.to_string(),
            detail: other.to_string(),
        },
    }
}

fn check_cancel(cancel: Option<&CancellationToken>) -> Result<()> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err(CruiseError::Interrupted);
    }
    Ok(())
}

async fn run_process(
    program: &str,
    args: &[String],
    pr: &PullRequestRef,
    cancel: Option<&CancellationToken>,
) -> Result<std::process::Output> {
    check_cancel(cancel)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match crate::step::command::run_process_output_cancelled(
        program,
        &refs,
        Some(pr.working_dir.as_path()),
        cancel,
    )
    .await
    {
        Ok(output) => Ok(output),
        Err(CruiseError::Interrupted) => Err(CruiseError::Interrupted),
        Err(e) => Err(failed(format!("failed to run {program}: {e}"))),
    }
}

/// Run `gh api graphql` with the query and variables passed as arguments only.
async fn graphql(
    pr: &PullRequestRef,
    query: &str,
    with_pr: bool,
    fields: &[(&str, &str)],
    cancel: Option<&CancellationToken>,
) -> Result<Value> {
    let mut args: Vec<String> = vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={query}"),
    ];
    if with_pr {
        args.extend([
            "-F".into(),
            "owner={owner}".into(),
            "-F".into(),
            "repo={repo}".into(),
            "-F".into(),
            format!("number={}", pr.number),
        ]);
    }
    for (key, value) in fields {
        args.push("-f".into());
        args.push(format!("{key}={value}"));
    }
    let output = run_process("gh", &args, pr, cancel).await?;
    if !output.status.success() {
        return Err(failed(format!(
            "gh api graphql failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| failed(format!("invalid GitHub response: {e}")))?;
    if value
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        return Err(failed(format!(
            "GitHub returned errors: {}",
            value["errors"]
        )));
    }
    Ok(value)
}

fn pull_request_node<'a>(value: &'a Value, field: &str) -> Result<&'a Value> {
    value
        .pointer(&format!("/data/repository/pullRequest/{field}"))
        .filter(|node| node.is_object())
        .ok_or_else(|| failed(format!("GitHub response is missing pullRequest.{field}")))
}

fn next_cursor(connection: &Value) -> Result<Option<String>> {
    if connection
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Ok(None);
    }
    connection
        .pointer("/pageInfo/endCursor")
        .and_then(Value::as_str)
        .map(|cursor| Some(cursor.to_string()))
        .ok_or_else(|| failed("GitHub pagination has no endCursor"))
}

fn nodes(connection: &Value) -> Result<&Vec<Value>> {
    connection
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| failed("GitHub connection has no nodes"))
}

fn login(node: &Value) -> String {
    node.pointer("/author/login")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_listed(login: &str, bots: &[String]) -> bool {
    bots.iter().any(|bot| bot.eq_ignore_ascii_case(login))
}

/// # Errors
/// `GitHubReviewFailed` on API/auth failure, `Interrupted` on cancel.
pub async fn head_oid(pr: &PullRequestRef, cancel: Option<&CancellationToken>) -> Result<String> {
    let args: Vec<String> = [
        "pr",
        "view",
        &pr.number.to_string(),
        "--json",
        "headRefOid",
        "--jq",
        ".headRefOid",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    let output = run_process("gh", &args, pr, cancel).await?;
    if !output.status.success() {
        return Err(failed(format!(
            "gh pr view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    crate::worktree_pr::gh_output_line(&output.stdout)
        .ok_or_else(|| failed("gh pr view returned an empty head commit"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PullRequestHead {
    target: crate::pr_review_fix::PrTarget,
    oid: String,
}

async fn pull_request_head(
    pr: &PullRequestRef,
    cancel: Option<&CancellationToken>,
) -> Result<PullRequestHead> {
    let args = vec![
        "pr".into(),
        "view".into(),
        pr.number.to_string(),
        "--json".into(),
        "state,isCrossRepository,number,title,url,headRefName,baseRefName,headRefOid".into(),
    ];
    let output = run_process("gh", &args, pr, cancel).await?;
    if !output.status.success() {
        return Err(failed(format!(
            "gh pr view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| failed(format!("invalid GitHub pull request head: {error}")))?;
    let target = crate::pr_review_fix::parse_pr_view(&output.stdout, pr.number)
        .map_err(|error| failed(error.to_string()))?;
    let oid = value
        .get("headRefOid")
        .and_then(Value::as_str)
        .filter(|oid| !oid.trim().is_empty())
        .ok_or_else(|| failed("gh pr view returned an empty head commit"))?
        .to_string();
    Ok(PullRequestHead { target, oid })
}

fn same_pr_identity(
    left: &crate::pr_review_fix::PrTarget,
    right: &crate::pr_review_fix::PrTarget,
) -> bool {
    left.number == right.number
        && left.url == right.url
        && left.head_branch == right.head_branch
        && left.base_branch == right.base_branch
}

async fn ensure_current_pull_request(
    pr: &PullRequestRef,
    expected_target: &crate::pr_review_fix::PrTarget,
    expected_head: &str,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    let current = pull_request_head(pr, cancel).await?;
    if current.oid != expected_head || !same_pr_identity(&current.target, expected_target) {
        return Err(failed(
            "the pull request identity or head changed during the review",
        ));
    }
    Ok(())
}

async fn has_review_of_head(
    pr: &PullRequestRef,
    bots: &[String],
    head_oid: &str,
    cancel: Option<&CancellationToken>,
) -> Result<bool> {
    let mut after: Option<String> = None;
    loop {
        let fields: Vec<(&str, &str)> = after.iter().map(|a| ("after", a.as_str())).collect();
        let value = graphql(pr, REVIEWS_QUERY, true, &fields, cancel).await?;
        let reviews = pull_request_node(&value, "reviews")?;
        if nodes(reviews)?.iter().any(|review| {
            is_listed(&login(review), bots)
                && review.pointer("/commit/oid").and_then(Value::as_str) == Some(head_oid)
        }) {
            return Ok(true);
        }
        after = next_cursor(reviews)?;
        if after.is_none() {
            return Ok(false);
        }
    }
}

async fn sleep_cancellable(duration: Duration, cancel: Option<&CancellationToken>) -> Result<()> {
    tokio::select! {
        () = tokio::time::sleep(duration) => Ok(()),
        () = async {
            match cancel {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        } => Err(CruiseError::Interrupted),
    }
}

/// # Errors
/// `GitHubReviewFailed` on timeout/API failure, `Interrupted` on cancel.
pub async fn wait_for_review(
    pr: &PullRequestRef,
    bots: &[String],
    head_oid: &str,
    deadline: Instant,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    loop {
        if has_review_of_head(pr, bots, head_oid, cancel).await? {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(failed(format!(
                "timed out waiting for a review of {head_oid} by one of: {}",
                bots.join(", ")
            )));
        }
        sleep_cancellable(POLL_INTERVAL.min(deadline - now), cancel).await?;
    }
}

fn parse_comments(connection: &Value) -> Result<Vec<ReviewComment>> {
    Ok(nodes(connection)?
        .iter()
        .map(|comment| ReviewComment {
            author: login(comment),
            body: comment
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect())
}

async fn remaining_comments(
    pr: &PullRequestRef,
    thread_id: &str,
    mut after: Option<String>,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<ReviewComment>> {
    let mut comments = Vec::new();
    while let Some(cursor) = after {
        let value = graphql(
            pr,
            COMMENTS_QUERY,
            false,
            &[("id", thread_id), ("after", &cursor)],
            cancel,
        )
        .await?;
        let connection = value
            .pointer("/data/node/comments")
            .filter(|node| node.is_object())
            .ok_or_else(|| failed("GitHub response is missing thread comments"))?;
        comments.extend(parse_comments(connection)?);
        after = next_cursor(connection)?;
    }
    Ok(comments)
}

/// Fetch every review thread of the pull request, following all pagination.
async fn fetch_threads(
    pr: &PullRequestRef,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<ReviewThread>> {
    let mut threads = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let fields: Vec<(&str, &str)> = after.iter().map(|a| ("after", a.as_str())).collect();
        let value = graphql(pr, THREADS_QUERY, true, &fields, cancel).await?;
        let connection = pull_request_node(&value, "reviewThreads")?;
        for node in nodes(connection)? {
            let id = node
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| failed("review thread has no id"))?
                .to_string();
            let comment_connection = node
                .get("comments")
                .ok_or_else(|| failed("review thread has no comments"))?;
            let mut comments = parse_comments(comment_connection)?;
            if let Some(cursor) = next_cursor(comment_connection)? {
                comments.extend(remaining_comments(pr, &id, Some(cursor), cancel).await?);
            }
            let flag = |name: &str| node.get(name).and_then(Value::as_bool).unwrap_or(false);
            threads.push(ReviewThread {
                id,
                path: node.get("path").and_then(Value::as_str).map(str::to_string),
                line: node.get("line").and_then(Value::as_u64),
                is_resolved: flag("isResolved"),
                viewer_can_reply: flag("viewerCanReply"),
                viewer_can_resolve: flag("viewerCanResolve"),
                comments,
            });
        }
        after = next_cursor(connection)?;
        if after.is_none() {
            return Ok(threads);
        }
    }
}

/// # Errors
/// `GitHubReviewFailed` on API failure, `Interrupted` on cancel.
pub async fn unresolved_bot_threads(
    pr: &PullRequestRef,
    bots: &[String],
    cancel: Option<&CancellationToken>,
) -> Result<Vec<ReviewThread>> {
    Ok(fetch_threads(pr, cancel)
        .await?
        .into_iter()
        .filter(|thread| {
            !thread.is_resolved
                && thread
                    .comments
                    .first()
                    .is_some_and(|first| is_listed(&first.author, bots))
        })
        .collect())
}

async fn unresolved_review_threads(
    pr: &PullRequestRef,
    scope: ReviewerScope,
    bots: &[String],
    permissions: &mut std::collections::HashMap<String, bool>,
    cancel: Option<&CancellationToken>,
) -> Result<(Vec<ReviewThread>, Vec<String>)> {
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    if scope == ReviewerScope::Bots {
        return Ok((unresolved_bot_threads(pr, bots, cancel).await?, excluded));
    }
    for thread in fetch_threads(pr, cancel)
        .await?
        .into_iter()
        .filter(|t| !t.is_resolved)
    {
        let author = thread
            .comments
            .first()
            .map(|c| c.author.as_str())
            .unwrap_or_default();
        let allowed = if author.is_empty() {
            false
        } else if let Some(allowed) = permissions.get(&author.to_ascii_lowercase()) {
            *allowed
        } else {
            // Never interpolate arbitrary API data into an endpoint path.
            if !author
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'[' || c == b']')
            {
                return Err(failed("invalid review author login"));
            }
            let args = vec![
                "api".into(),
                format!("repos/{{owner}}/{{repo}}/collaborators/{author}/permission"),
                "--jq".into(),
                ".permission".into(),
            ];
            let output = run_process("gh", &args, pr, cancel).await?;
            let allowed = if output.status.success() {
                match String::from_utf8_lossy(&output.stdout).trim() {
                    "admin" | "maintain" | "write" => true,
                    "read" | "triage" | "none" => false,
                    _ => return Err(failed("invalid collaborator permission response")),
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("HTTP 404") {
                    false
                } else {
                    return Err(failed(format!(
                        "gh collaborator permission failed: {}",
                        stderr.trim()
                    )));
                }
            };
            permissions.insert(author.to_ascii_lowercase(), allowed);
            allowed
        };
        if allowed {
            included.push(thread);
        } else if scope == ReviewerScope::All {
            excluded.push(thread.id);
        }
    }
    Ok((included, excluded))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResponse {
    actions: Vec<RawAction>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    thread_id: String,
    #[serde(default)]
    reply: Option<String>,
    resolve: bool,
}

/// # Errors
/// `GitHubReviewFailed` when the response is malformed or names unknown/duplicate threads.
pub fn parse_review_actions(response: &str, threads: &[ReviewThread]) -> Result<Vec<ReviewAction>> {
    let parsed: RawResponse = serde_json::from_str(response.trim())
        .map_err(|e| failed(format!("invalid review action response: {e}")))?;
    let mut seen = std::collections::HashSet::new();
    let mut actions = Vec::new();
    for raw in parsed.actions {
        if !threads.iter().any(|thread| thread.id == raw.thread_id) {
            return Err(failed(format!("unknown thread id '{}'", raw.thread_id)));
        }
        if !seen.insert(raw.thread_id.clone()) {
            return Err(failed(format!("duplicate thread id '{}'", raw.thread_id)));
        }
        if raw.reply.as_deref().is_some_and(|r| r.trim().is_empty()) {
            return Err(failed(format!(
                "empty reply for thread '{}'",
                raw.thread_id
            )));
        }
        if raw.reply.is_none() && !raw.resolve {
            return Err(failed(format!(
                "action for thread '{}' has neither reply nor resolve",
                raw.thread_id
            )));
        }
        actions.push(ReviewAction {
            thread_id: raw.thread_id,
            reply: raw.reply,
            resolve: raw.resolve,
        });
    }
    Ok(actions)
}

fn reply_marker(thread_id: &str, reply: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(thread_id.as_bytes());
    hasher.update([0]);
    hasher.update(reply.as_bytes());
    let digest = hasher
        .finalize()
        .iter()
        .take(12)
        .fold(String::new(), |mut acc, byte| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{byte:02x}");
            acc
        });
    format!("<!-- cruise-github-review:{digest} -->")
}

fn validate_action_permissions(threads: &[ReviewThread], actions: &[ReviewAction]) -> Result<()> {
    for action in actions {
        let thread = threads
            .iter()
            .find(|thread| thread.id == action.thread_id)
            .ok_or_else(|| failed(format!("unknown thread id '{}'", action.thread_id)))?;
        if action.reply.as_deref().is_some_and(|r| r.trim().is_empty()) {
            return Err(failed(format!("empty reply for thread '{}'", thread.id)));
        }
        if action.reply.is_some() && !thread.viewer_can_reply {
            return Err(failed(format!(
                "no permission to reply to thread '{}'",
                thread.id
            )));
        }
        if action.resolve && !thread.viewer_can_resolve {
            return Err(failed(format!(
                "no permission to resolve thread '{}'",
                thread.id
            )));
        }
    }
    Ok(())
}

fn validate_live_action_permissions(
    actions: &[ReviewAction],
    current: &[ReviewThread],
) -> Result<()> {
    for action in actions {
        let thread = current
            .iter()
            .find(|thread| thread.id == action.thread_id)
            .ok_or_else(|| failed(format!("thread '{}' no longer exists", action.thread_id)))?;
        if action.reply.is_some() && !thread.viewer_can_reply {
            return Err(failed(format!(
                "no permission to reply to thread '{}'",
                action.thread_id
            )));
        }
        if action.resolve && !thread.viewer_can_resolve {
            return Err(failed(format!(
                "no permission to resolve thread '{}'",
                action.thread_id
            )));
        }
    }
    Ok(())
}

async fn validate_live_actions(
    pr: &PullRequestRef,
    threads: &[ReviewThread],
    actions: &[ReviewAction],
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    validate_action_permissions(threads, actions)?;
    if actions.is_empty() {
        return Ok(());
    }
    let current = fetch_threads(pr, cancel).await?;
    validate_live_action_permissions(actions, &current)
}

async fn apply_actions_internal(
    pr: &PullRequestRef,
    expected_pr: Option<(&crate::pr_review_fix::PrTarget, &str)>,
    threads: &[ReviewThread],
    actions: &[ReviewAction],
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    validate_action_permissions(threads, actions)?;
    if actions.is_empty() {
        return Ok(());
    }
    if let Some((target, head)) = expected_pr {
        ensure_current_pull_request(pr, target, head, cancel).await?;
    }
    let current = fetch_threads(pr, cancel).await?;
    validate_live_action_permissions(actions, &current)?;
    if let Some((target, head)) = expected_pr {
        ensure_current_pull_request(pr, target, head, cancel).await?;
    }
    for action in actions {
        let state = current
            .iter()
            .find(|thread| thread.id == action.thread_id)
            .ok_or_else(|| failed(format!("thread '{}' no longer exists", action.thread_id)))?;
        if let Some(reply) = &action.reply {
            let marker = reply_marker(&action.thread_id, reply);
            if !state.comments.iter().any(|c| c.body.contains(&marker)) {
                let body = format!("{reply}\n\n{marker}");
                graphql(
                    pr,
                    REPLY_MUTATION,
                    false,
                    &[("threadId", &action.thread_id), ("body", &body)],
                    cancel,
                )
                .await?;
            }
        }
        if action.resolve && !state.is_resolved {
            graphql(
                pr,
                RESOLVE_MUTATION,
                false,
                &[("threadId", &action.thread_id)],
                cancel,
            )
            .await?;
        }
    }
    Ok(())
}

/// # Errors
/// `GitHubReviewFailed` on permission/API failure, `Interrupted` on cancel.
pub async fn apply_actions(
    pr: &PullRequestRef,
    threads: &[ReviewThread],
    actions: &[ReviewAction],
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    apply_actions_internal(pr, None, threads, actions, cancel).await
}

fn thread_json(thread: &ReviewThread) -> Value {
    json!({
        "id": thread.id,
        "path": thread.path,
        "line": thread.line,
        "viewer_can_reply": thread.viewer_can_reply,
        "viewer_can_resolve": thread.viewer_can_resolve,
        "comments": thread.comments.iter().map(|c| json!({"author": c.author, "body": c.body})).collect::<Vec<_>>(),
    })
}

fn build_prompt(base: &str, head_oid: &str, threads: &[ReviewThread]) -> String {
    let threads_json =
        serde_json::to_string_pretty(&threads.iter().map(thread_json).collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".to_string());
    let context = format!(
        "\n\n## GitHub review context\n\nPR head commit: {head_oid}\n\nUnresolved review threads selected for this step (JSON):\n{threads_json}\n\nAssess the threads and make only necessary edits. Do not commit or push; Cruise will validate and publish changes when explicitly allowed. Respond with ONLY a JSON object of the form {{\"actions\":[{{\"thread_id\":\"<id>\",\"reply\":\"<optional text>\",\"resolve\":true}}]}}. Use only thread ids listed above, at most once each. Each action needs a non-empty reply or resolve:true. An empty actions list is allowed.\n"
    );
    format!("{base}{}", context.replace('{', "{{").replace('}', "}}"))
}

async fn read_host_git_output<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Vec<u8> {
    const MAX_CAPTURED_OUTPUT: usize = 4 * 1024 * 1024;
    const MARKER: &[u8] = b"\n[... output omitted ...]\n";
    const HEAD: usize = (MAX_CAPTURED_OUTPUT - MARKER.len()) / 2;
    const TAIL: usize = MAX_CAPTURED_OUTPUT - MARKER.len() - HEAD;
    use tokio::io::AsyncReadExt as _;

    let mut head = Vec::new();
    let mut tail = std::collections::VecDeque::new();
    let mut total = 0usize;
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                total = total.saturating_add(read);
                let count = read.min(HEAD.saturating_sub(head.len()));
                head.extend_from_slice(&buffer[..count]);
                for &byte in &buffer[count..read] {
                    if tail.len() == TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(byte);
                }
            }
        }
    }
    if total > HEAD + TAIL {
        head.extend_from_slice(MARKER);
    }
    head.extend(tail);
    head
}

async fn wait_for_cancellation(cancel: Option<&CancellationToken>) {
    if let Some(cancel) = cancel {
        cancel.cancelled().await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn run_host_git(
    pr: &PullRequestRef,
    args: &[String],
    cancel: Option<&CancellationToken>,
) -> Result<std::process::Output> {
    check_cancel(cancel)?;
    let mut command = tokio::process::Command::from(crate::worktree_pr::host_git_command());
    command
        .args(args)
        .current_dir(&pr.working_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| failed(format!("failed to run git: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .map(|mut pipe| tokio::spawn(async move { read_host_git_output(&mut pipe).await }));
    let stderr = child
        .stderr
        .take()
        .map(|mut pipe| tokio::spawn(async move { read_host_git_output(&mut pipe).await }));
    let status = tokio::select! {
        result = child.wait() => result.map_err(|error| failed(format!("git process failed: {error}")))?,
        () = wait_for_cancellation(cancel) => {
            let _ = child.kill().await;
            if let Some(task) = &stdout {
                task.abort();
            }
            if let Some(task) = &stderr {
                task.abort();
            }
            return Err(CruiseError::Interrupted);
        }
    };
    let stdout = match stdout {
        Some(task) => task
            .await
            .map_err(|error| failed(format!("failed to collect git stdout: {error}")))?,
        None => Vec::new(),
    };
    let stderr = match stderr {
        Some(task) => task
            .await
            .map_err(|error| failed(format!("failed to collect git stderr: {error}")))?,
        None => Vec::new(),
    };
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

async fn git_output_args(
    pr: &PullRequestRef,
    args: Vec<String>,
    cancel: Option<&CancellationToken>,
) -> Result<std::process::Output> {
    run_host_git(pr, &args, cancel).await
}

async fn git_output(
    pr: &PullRequestRef,
    args: &[&str],
    cancel: Option<&CancellationToken>,
) -> Result<std::process::Output> {
    git_output_args(pr, args.iter().map(ToString::to_string).collect(), cancel).await
}

async fn git_head(
    pr: &PullRequestRef,
    cancel: Option<&CancellationToken>,
) -> Result<Option<String>> {
    let output = git_output(pr, &["rev-parse", "HEAD"], cancel).await?;
    Ok(output
        .status
        .success()
        .then(|| crate::worktree_pr::gh_output_line(&output.stdout))
        .flatten())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OriginSnapshot {
    fetch_url: String,
    push_url: String,
}

struct HostCommitBaseline {
    head_oid: String,
    branch_ref: String,
    tracking_ref: String,
    target: crate::pr_review_fix::PrTarget,
    origin: OriginSnapshot,
    committer_name: String,
    committer_email: String,
}

fn git_output_lines(output: &std::process::Output, context: &str) -> Result<Vec<String>> {
    if !output.status.success() {
        return Err(failed(format!(
            "{context}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let output = std::str::from_utf8(&output.stdout)
        .map_err(|_| failed(format!("{context} is not valid UTF-8")))?;
    Ok(output.lines().map(ToString::to_string).collect())
}

fn validate_origin_endpoint(
    target: &crate::pr_review_fix::PrTarget,
    remote: &str,
    direction: &str,
) -> Result<()> {
    let url =
        url::Url::parse(&target.url).map_err(|_| failed("invalid resolved pull request URL"))?;
    let segments: Vec<_> = url.path_segments().into_iter().flatten().collect();
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || segments.len() != 4
        || segments[2] != "pull"
        || segments[3].parse::<u64>().ok() != Some(target.number)
    {
        return Err(failed("invalid resolved pull request URL"));
    }
    if remote.contains("://")
        && url::Url::parse(remote).map_or(true, |remote| remote.port().is_some())
    {
        return Err(failed(
            "cannot verify origin repository against resolved pull request",
        ));
    }
    let identity = crate::forge::resolve_remote(remote, Some(crate::forge::ForgeKind::GitHub))
        .map_err(|_| failed("cannot verify origin repository against resolved pull request"))?;
    let expected_repo = format!("{}/{}", segments[0], segments[1]);
    if !identity
        .host
        .eq_ignore_ascii_case(url.host_str().unwrap_or_default())
        || !identity.repository.eq_ignore_ascii_case(&expected_repo)
    {
        return Err(failed(format!(
            "origin {direction} repository does not match resolved pull request repository"
        )));
    }
    Ok(())
}

async fn capture_origin(
    pr: &PullRequestRef,
    target: &crate::pr_review_fix::PrTarget,
    cancel: Option<&CancellationToken>,
) -> Result<OriginSnapshot> {
    let fetch_output = git_output(pr, &["remote", "get-url", "--all", "origin"], cancel).await?;
    if fetch_output
        .stdout
        .iter()
        .any(|byte| byte.is_ascii_control() && *byte != b'\n')
    {
        return Err(failed("origin fetch URL contains a control character"));
    }
    let fetch_urls = git_output_lines(&fetch_output, "failed to inspect origin fetch URL")?;
    if fetch_urls.len() != 1 || fetch_urls[0].is_empty() {
        return Err(failed("origin must have exactly one fetch URL"));
    }
    let push_url = crate::worktree_pr::host_push_endpoint(&pr.working_dir)
        .map_err(|error| failed(format!("failed to validate origin push URL: {error}")))?;
    validate_origin_endpoint(target, &fetch_urls[0], "fetch")?;
    validate_origin_endpoint(target, &push_url, "push")?;
    Ok(OriginSnapshot {
        fetch_url: fetch_urls[0].clone(),
        push_url,
    })
}

async fn ensure_no_in_progress_git_operation(
    pr: &PullRequestRef,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "REBASE_HEAD",
        "BISECT_START",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        let output = git_output_args(
            pr,
            vec!["rev-parse".into(), "--git-path".into(), marker.to_string()],
            cancel,
        )
        .await?;
        if !output.status.success() {
            return Err(failed("failed to inspect Git operation state"));
        }
        let Some(path) = crate::worktree_pr::gh_output_line(&output.stdout) else {
            return Err(failed("Git returned an empty operation-state path"));
        };
        let path = std::path::PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            pr.working_dir.join(path)
        };
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(failed("the worktree has an in-progress Git operation"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(failed(format!(
                    "failed to inspect Git operation state: {error}"
                )));
            }
        }
    }
    Ok(())
}

fn parse_git_ident(ident: &str) -> Result<(String, String)> {
    let close = ident
        .rfind('>')
        .ok_or_else(|| failed("Git committer identity is malformed"))?;
    let open = ident[..close]
        .rfind('<')
        .ok_or_else(|| failed("Git committer identity is malformed"))?;
    let name = ident[..open].trim_end();
    let email = &ident[open + 1..close];
    if name.is_empty() || email.is_empty() {
        return Err(failed("Git committer identity is incomplete"));
    }
    Ok((name.to_string(), email.to_string()))
}

async fn git_committer_identity(
    pr: &PullRequestRef,
    cancel: Option<&CancellationToken>,
) -> Result<(String, String)> {
    let output = git_output(pr, &["var", "GIT_COMMITTER_IDENT"], cancel).await?;
    if !output.status.success() {
        return Err(failed(format!(
            "Git committer identity is unavailable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let ident = crate::worktree_pr::gh_output_line(&output.stdout)
        .ok_or_else(|| failed("Git committer identity is empty"))?;
    parse_git_ident(&ident)
}

async fn ensure_checkout_binding(
    pr: &PullRequestRef,
    branch_ref: &str,
    tracking_ref: &str,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    let branch = git_output(pr, &["symbolic-ref", "--quiet", "HEAD"], cancel).await?;
    if !branch.status.success() {
        return Err(failed("HEAD must be attached to the pull request head ref"));
    }
    if crate::worktree_pr::gh_output_line(&branch.stdout).as_deref() != Some(branch_ref) {
        return Err(failed(
            "the checked-out branch does not match the pull request head ref",
        ));
    }
    let upstream = git_output(
        pr,
        &["rev-parse", "--symbolic-full-name", "@{upstream}"],
        cancel,
    )
    .await?;
    if !upstream.status.success()
        || crate::worktree_pr::gh_output_line(&upstream.stdout).as_deref() != Some(tracking_ref)
    {
        return Err(failed(
            "the checked-out branch does not track the pull request head ref on origin",
        ));
    }
    Ok(())
}

fn ensure_review_git_safety(pr: &PullRequestRef) -> Result<()> {
    crate::worktree_pr::ensure_host_git_safety(&pr.working_dir)
        .map_err(|error| failed(format!("unsafe Git configuration: {error}")))
}

/// Bind the edit turn to a clean checkout at the live PR head and its literal origin.
async fn prepare_host_commit(
    pr: &PullRequestRef,
    expected: &PullRequestHead,
    cancel: Option<&CancellationToken>,
) -> Result<HostCommitBaseline> {
    ensure_review_git_safety(pr)?;
    ensure_no_in_progress_git_operation(pr, cancel).await?;
    let status = git_output(
        pr,
        &["status", "--porcelain", "--untracked-files=all"],
        cancel,
    )
    .await?;
    if !status.status.success() {
        return Err(failed("git status failed before the review prompt"));
    }
    if !status.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(failed(
            "the worktree must be clean before a host-managed review edit",
        ));
    }
    let current = pull_request_head(pr, cancel).await?;
    if current.oid != expected.oid || !same_pr_identity(&current.target, &expected.target) {
        return Err(failed(
            "the pull request head changed while collecting review threads",
        ));
    }
    let origin = capture_origin(pr, &current.target, cancel).await?;
    let branch_ref = format!("refs/heads/{}", current.target.head_branch);
    let tracking_ref = format!("refs/remotes/origin/{}", current.target.head_branch);
    let valid_ref = git_output_args(
        pr,
        vec!["check-ref-format".into(), branch_ref.clone()],
        cancel,
    )
    .await?;
    if !valid_ref.status.success() {
        return Err(failed("GitHub returned an invalid pull request head ref"));
    }
    ensure_checkout_binding(pr, &branch_ref, &tracking_ref, cancel).await?;
    if git_head(pr, cancel).await?.as_deref() != Some(current.oid.as_str()) {
        return Err(failed(
            "the checked-out HEAD does not match the pull request's current head",
        ));
    }
    let tracking = git_output_args(
        pr,
        vec!["rev-parse".into(), "--verify".into(), tracking_ref.clone()],
        cancel,
    )
    .await?;
    if !tracking.status.success()
        || crate::worktree_pr::gh_output_line(&tracking.stdout).as_deref()
            != Some(current.oid.as_str())
    {
        return Err(failed(
            "the origin tracking ref does not match the pull request's current head",
        ));
    }
    let (committer_name, committer_email) = git_committer_identity(pr, cancel).await?;
    Ok(HostCommitBaseline {
        head_oid: current.oid,
        branch_ref,
        tracking_ref,
        target: current.target,
        origin,
        committer_name,
        committer_email,
    })
}

fn is_direct_child(output: &std::process::Output, commit_oid: &str, parent_oid: &str) -> bool {
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut fields = stdout.split_ascii_whitespace();
    fields.next() == Some(commit_oid)
        && fields.next() == Some(parent_oid)
        && fields.next().is_none()
}
/// Stage edits and create a commit-tree object without updating a ref.
async fn create_host_review_commit(
    pr: &PullRequestRef,
    baseline: &HostCommitBaseline,
    cancel: Option<&CancellationToken>,
) -> Result<String> {
    let add = git_output(pr, &["add", "--all", "--", "."], cancel).await?;
    if !add.status.success() {
        return Err(failed(format!(
            "git add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }
    let tree = git_output(pr, &["write-tree"], cancel).await?;
    if !tree.status.success() {
        return Err(failed(format!(
            "git write-tree failed: {}",
            String::from_utf8_lossy(&tree.stderr).trim()
        )));
    }
    let tree = crate::worktree_pr::gh_output_line(&tree.stdout)
        .ok_or_else(|| failed("git write-tree returned an empty tree"))?;
    let commit = git_output_args(
        pr,
        vec![
            "-c".into(),
            format!("user.name={}", baseline.committer_name),
            "-c".into(),
            format!("user.email={}", baseline.committer_email),
            "commit-tree".into(),
            tree,
            "-p".into(),
            baseline.head_oid.clone(),
            "-m".into(),
            HOST_REVIEW_COMMIT_MESSAGE.into(),
        ],
        cancel,
    )
    .await?;
    if !commit.status.success() {
        return Err(failed(format!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        )));
    }
    crate::worktree_pr::gh_output_line(&commit.stdout)
        .filter(|oid| {
            (oid.len() == 40 || oid.len() == 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| failed("git commit-tree returned an invalid object id"))
}

/// Create and attach one commit-tree object by compare-and-swap.
async fn commit_prompt_changes(
    pr: &PullRequestRef,
    baseline: &HostCommitBaseline,
    cancel: Option<&CancellationToken>,
) -> Result<Option<String>> {
    if git_head(pr, cancel).await?.as_deref() != Some(baseline.head_oid.as_str()) {
        return Err(failed(
            "HEAD changed during the review prompt; host commit refused",
        ));
    }
    ensure_checkout_binding(pr, &baseline.branch_ref, &baseline.tracking_ref, cancel).await?;
    ensure_no_in_progress_git_operation(pr, cancel).await?;
    ensure_review_git_safety(pr)?;
    let origin = capture_origin(pr, &baseline.target, cancel).await?;
    if origin != baseline.origin {
        return Err(failed("origin changed during the review prompt"));
    }
    let status = git_output(
        pr,
        &["status", "--porcelain", "--untracked-files=all"],
        cancel,
    )
    .await?;
    if !status.status.success() {
        return Err(failed("git status failed after the review prompt"));
    }
    if status.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let commit_oid = create_host_review_commit(pr, baseline, cancel).await?;
    let update = git_output_args(
        pr,
        vec![
            "update-ref".into(),
            "--no-deref".into(),
            "-m".into(),
            "Cruise: publish validated PR review edits".into(),
            baseline.branch_ref.clone(),
            commit_oid.clone(),
            baseline.head_oid.clone(),
        ],
        cancel,
    )
    .await?;
    if !update.status.success() {
        return Err(failed(
            "the pull request branch moved before the host commit could be attached",
        ));
    }
    if git_head(pr, cancel).await?.as_deref() != Some(commit_oid.as_str()) {
        return Err(failed(
            "HEAD changed while attaching the host review commit",
        ));
    }
    let status = git_output(
        pr,
        &["status", "--porcelain", "--untracked-files=all"],
        cancel,
    )
    .await?;
    if !status.status.success() || !status.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(failed("the host commit left uncommitted changes"));
    }
    Ok(Some(commit_oid))
}

async fn validate_host_commit_for_publish(
    pr: &PullRequestRef,
    baseline: &HostCommitBaseline,
    commit_oid: &str,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    if git_head(pr, cancel).await?.as_deref() != Some(commit_oid) {
        return Err(failed(
            "HEAD changed before the host commit could be published",
        ));
    }
    ensure_checkout_binding(pr, &baseline.branch_ref, &baseline.tracking_ref, cancel).await?;
    ensure_current_pull_request(pr, &baseline.target, &baseline.head_oid, cancel).await?;
    ensure_no_in_progress_git_operation(pr, cancel).await?;
    let origin = capture_origin(pr, &baseline.target, cancel).await?;
    if origin != baseline.origin {
        return Err(failed(
            "origin changed before the host commit could be published",
        ));
    }
    let parents = git_output_args(
        pr,
        vec![
            "rev-list".into(),
            "--parents".into(),
            "-n".into(),
            "1".into(),
            commit_oid.to_string(),
        ],
        cancel,
    )
    .await?;
    if !is_direct_child(&parents, commit_oid, &baseline.head_oid) {
        return Err(failed(
            "refusing to publish a rewrite of the validated pull request head",
        ));
    }
    ensure_review_git_safety(pr)?;
    let status = git_output(
        pr,
        &["status", "--porcelain", "--untracked-files=all"],
        cancel,
    )
    .await?;
    if !status.status.success() || !status.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(failed("the host commit left uncommitted changes"));
    }
    Ok(())
}

/// Push only the captured commit object to the validated PR ref.
async fn publish_host_commit(
    pr: &PullRequestRef,
    baseline: &HostCommitBaseline,
    commit_oid: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    let Some(commit_oid) = commit_oid else {
        return Ok(());
    };
    validate_host_commit_for_publish(pr, baseline, commit_oid, cancel).await?;
    let lease = format!(
        "--force-with-lease={}:{}",
        baseline.branch_ref, baseline.head_oid
    );
    let refspec = format!("{commit_oid}:{}", baseline.branch_ref);
    let push = git_output_args(
        pr,
        vec![
            "-c".into(),
            "push.pushOption=".into(),
            "push".into(),
            "--no-follow-tags".into(),
            "--recurse-submodules=no".into(),
            "--no-verify".into(),
            lease,
            "--".into(),
            baseline.origin.push_url.clone(),
            refspec,
        ],
        cancel,
    )
    .await?;
    if !push.status.success() {
        return Err(failed(format!(
            "git push to the pull-request head failed: {}",
            String::from_utf8_lossy(&push.stderr).trim()
        )));
    }
    let update_tracking = git_output_args(
        pr,
        vec![
            "update-ref".into(),
            "--no-deref".into(),
            baseline.tracking_ref.clone(),
            commit_oid.to_string(),
            baseline.head_oid.clone(),
        ],
        cancel,
    )
    .await?;
    if !update_tracking.status.success() {
        let tracking = git_output_args(
            pr,
            vec![
                "rev-parse".into(),
                "--verify".into(),
                baseline.tracking_ref.clone(),
            ],
            cancel,
        )
        .await?;
        if !tracking.status.success()
            || crate::worktree_pr::gh_output_line(&tracking.stdout).as_deref() != Some(commit_oid)
        {
            return Err(failed(
                "published the PR head but could not update its local origin tracking ref",
            ));
        }
    }
    Ok(())
}

async fn ensure_prompt_clean(
    pr: &PullRequestRef,
    head_before: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    let Some(head_before) = head_before else {
        return Ok(());
    };
    if git_head(pr, cancel).await?.as_deref() != Some(head_before) {
        return Err(failed(
            "the prompt moved HEAD while host commits were not allowed",
        ));
    }
    ensure_review_git_safety(pr)?;
    let status = git_output(
        pr,
        &["status", "--porcelain", "--untracked-files=all"],
        cancel,
    )
    .await?;
    if !status.status.success() {
        return Err(failed("git status failed after the review prompt"));
    }
    if !status.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(failed(
            "the prompt left uncommitted changes; host commit requires allow_commit: true",
        ));
    }
    Ok(())
}

fn write_report(
    vars: &VariableStore,
    file: Option<&str>,
    iterations: &[Value],
    excluded: &std::collections::BTreeSet<String>,
    error: Option<&str>,
) -> Result<()> {
    let Some(file) = file else {
        return Ok(());
    };
    let report = json!({"iterations": iterations, "excluded_thread_ids": excluded, "error": error});
    let text = serde_json::to_string_pretty(&report).map_err(|e| failed(e.to_string()))?;
    vars.write_artifact(file, &text)
}

fn ids(threads: &[ReviewThread]) -> Vec<&str> {
    threads.iter().map(|t| t.id.as_str()).collect()
}

fn same_thread_ids(before: &[ReviewThread], after: &[ReviewThread]) -> bool {
    let before: std::collections::HashSet<_> = ids(before).into_iter().collect();
    let after: std::collections::HashSet<_> = ids(after).into_iter().collect();
    before == after
}

/// Run the `github-review` step: wait for bot review, let the prompt address
/// unresolved threads, apply its reply/resolve decisions, repeat up to the limit.
///
/// # Errors
/// `GitHubReviewFailed` for API/timeout/response/limit failures, `Interrupted` on cancel.
pub(crate) async fn run_step(
    ctx: &ExecutionContext<'_>,
    step: &GitHubReviewStep,
    vars: &mut VariableStore,
    env: &std::collections::HashMap<String, String>,
    timeout: Option<Duration>,
    step_name: &str,
    allow_commit: bool,
) -> Result<()> {
    let timeout = timeout.ok_or_else(|| failed("timeout is required"))?;
    let deadline = Instant::now() + timeout;
    let number = vars
        .get_variable("pr.number")?
        .trim()
        .parse::<u64>()
        .map_err(|e| failed(format!("invalid pr.number: {e}")))?;
    let pr = PullRequestRef {
        number,
        working_dir: ctx
            .working_dir
            .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf),
    };
    let mut report = Vec::new();
    let mut excluded = std::collections::BTreeSet::new();
    let result = run_loop(
        ctx,
        step,
        vars,
        env,
        &pr,
        deadline,
        step_name,
        allow_commit,
        &mut report,
        &mut excluded,
    )
    .await;
    match result {
        Ok(()) => {
            write_report(vars, step.report_file.as_deref(), &report, &excluded, None)?;
            Ok(())
        }
        Err(error) => {
            let error = with_step(error, step_name);
            let _ = write_report(
                vars,
                step.report_file.as_deref(),
                &report,
                &excluded,
                Some(&error.to_string()),
            );
            Err(error)
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "loop state is passed explicitly")]
#[expect(
    clippy::too_many_lines,
    reason = "the review loop keeps GitHub and host-commit lifecycle ordering together"
)]
async fn run_loop(
    ctx: &ExecutionContext<'_>,
    step: &GitHubReviewStep,
    vars: &mut VariableStore,
    env: &std::collections::HashMap<String, String>,
    pr: &PullRequestRef,
    deadline: Instant,
    step_name: &str,
    allow_commit: bool,
    report: &mut Vec<Value>,
    excluded: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    let cancel = ctx.cancel_token;
    let bots = &step.review.bots;
    let scope = step.review.reviewers;
    let mut permissions = std::collections::HashMap::new();
    for iteration in 1..=step.review.max_iterations {
        let head = head_oid(pr, cancel).await?;
        if scope == ReviewerScope::Bots {
            wait_for_review(pr, bots, &head, deadline, cancel).await?;
        }
        let (threads, ignored) =
            unresolved_review_threads(pr, scope, bots, &mut permissions, cancel).await?;
        excluded.extend(ignored);
        if threads.is_empty() {
            report.push(json!({"iteration": iteration, "head_oid": head, "bots": bots, "thread_ids": Vec::<&str>::new(), "actions": [], "remaining_thread_ids": Vec::<&str>::new()}));
            if let Some(entry) = report.last_mut().filter(|_| scope == ReviewerScope::All) {
                entry["reviewers"] = json!("all");
                entry["excluded_thread_ids"] = json!(excluded);
            }
            write_report(vars, step.report_file.as_deref(), report, excluded, None)?;
            return Ok(());
        }
        report.push(json!({"iteration": iteration, "head_oid": head, "bots": bots, "thread_ids": ids(&threads)}));
        if let Some(entry) = report.last_mut().filter(|_| scope == ReviewerScope::All) {
            entry["reviewers"] = json!("all");
            entry["excluded_thread_ids"] = json!(excluded);
        }

        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| failed("timed out before running the prompt"))?;
        let pr_head = pull_request_head(pr, cancel).await?;
        if pr_head.oid != head {
            return Err(failed(
                "the pull request head changed while collecting review threads",
            ));
        }
        let (baseline, head_before) = if allow_commit {
            (Some(prepare_host_commit(pr, &pr_head, cancel).await?), None)
        } else {
            let head_before = git_head(pr, cancel).await?;
            if head_before.is_some() {
                ensure_review_git_safety(pr)?;
                let status = git_output(
                    pr,
                    &["status", "--porcelain", "--untracked-files=all"],
                    cancel,
                )
                .await?;
                if !status.status.success() {
                    return Err(failed("git status failed before the review prompt"));
                }
                if !status.stdout.iter().all(u8::is_ascii_whitespace) {
                    return Err(failed(
                        "the worktree must be clean before the review prompt",
                    ));
                }
            }
            (None, head_before)
        };
        let mut prompt = step.prompt.clone();
        prompt.prompt = build_prompt(&prompt.prompt, &head, &threads);
        let lease = ctx.instructions.and_then(|inbox| {
            inbox.lease(
                step_name,
                crate::instruction_inbox::DeliveryScope::GitHubReview,
            )
        });
        crate::engine::log_instruction_lease(ctx, lease.as_ref(), step_name, "github-review");
        let prompt_result = Box::pin(run_prompt_step(
            vars,
            ctx.compiled,
            &prompt,
            ctx.rate_limit_retries,
            env,
            cancel,
            ctx.working_dir,
            PromptStepOutput::Console(ctx.on_step_log),
            Some(remaining),
            step_name,
            ctx.usage_session_id,
            false,
            // `github-review` allow_commit is consent for the host loop only.
            false,
            lease
                .as_ref()
                .map(crate::instruction_inbox::Lease::block_text),
            ctx.ask_handler,
        ))
        .await;
        crate::engine::finish_instruction_lease(ctx, lease, step_name, &prompt_result);
        match prompt_result {
            Ok(_) => {}
            Err(CruiseError::StepTimeout { .. }) => {
                return Err(failed("timed out while running the prompt"));
            }
            Err(e) => return Err(e),
        }
        let output = vars.prev_output().unwrap_or_default().to_string();
        let actions = parse_review_actions(&output, &threads)?;
        validate_action_permissions(&threads, &actions)?;
        ensure_current_pull_request(pr, &pr_head.target, &pr_head.oid, cancel).await?;
        validate_live_actions(pr, &threads, &actions, cancel).await?;
        let published_head = if let Some(baseline) = baseline.as_ref() {
            let committed = commit_prompt_changes(pr, baseline, cancel).await?;
            publish_host_commit(pr, baseline, committed.as_deref(), cancel).await?;
            committed.unwrap_or_else(|| pr_head.oid.clone())
        } else {
            ensure_prompt_clean(pr, head_before.as_deref(), cancel).await?;
            pr_head.oid.clone()
        };
        ensure_current_pull_request(pr, &pr_head.target, &published_head, cancel).await?;
        apply_actions_internal(
            pr,
            Some((&pr_head.target, &published_head)),
            &threads,
            &actions,
            cancel,
        )
        .await?;
        let (left, ignored) =
            unresolved_review_threads(pr, scope, bots, &mut permissions, cancel).await?;
        excluded.extend(ignored);
        if let Some(entry) = report.last_mut() {
            entry["actions"] = json!(actions.iter().map(|a| json!({"thread_id": a.thread_id, "reply": a.reply.is_some(), "resolve": a.resolve})).collect::<Vec<_>>());
            entry["remaining_thread_ids"] = json!(ids(&left));
            if scope == ReviewerScope::All {
                entry["excluded_thread_ids"] = json!(excluded);
            }
        }
        write_report(vars, step.report_file.as_deref(), report, excluded, None)?;
        if left.is_empty() {
            return Ok(());
        }
        if scope == ReviewerScope::All && same_thread_ids(&threads, &left) {
            return Err(failed(format!(
                "{} review threads remain and iteration {iteration} resolved none (no progress)",
                left.len()
            )));
        }
    }
    Err(failed(format!(
        "unresolved review threads remain after {} iterations",
        step.review.max_iterations
    )))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::error::CruiseError;
    use serde_json::{Value, json};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    const OID: &str = "abc123";
    const BOT: &str = "coderabbitai[bot]";

    struct Fixture {
        tmp: TempDir,
        _path: crate::test_support::EnvGuard,
        _dir: crate::test_support::EnvGuard,
        _git_config: [crate::test_support::EnvGuard; 2],
        _lock: crate::test_support::ProcessLock,
    }

    fn fixture_git(cwd: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap_or_else(|error| panic!("run fixture git {args:?}: {error}"))
    }

    fn fixture_git_ok(cwd: &Path, args: &[&str]) -> String {
        let output = fixture_git(cwd, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    impl Fixture {
        fn new() -> Self {
            let lock = crate::test_support::lock_process();
            let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let bin = tmp.path().join("bin");
            std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e:?}"));
            let repo = tmp.path().join("repo");
            std::fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
            let origin = tmp.path().join("origin.git");
            fixture_git_ok(
                tmp.path(),
                &["init", "--bare", "-q", origin.to_str().unwrap_or("")],
            );
            std::fs::write(repo.join("tracked.txt"), "fixture\n")
                .unwrap_or_else(|e| panic!("write fixture file: {e}"));
            fixture_git_ok(&repo, &["init", "-q", "-b", "feature"]);
            fixture_git_ok(&repo, &["add", "--all"]);
            fixture_git_ok(&repo, &["config", "user.name", "Review fixture"]);
            fixture_git_ok(&repo, &["config", "user.email", "review@example.invalid"]);
            fixture_git_ok(
                &repo,
                &[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    "fixture",
                ],
            );
            let origin_url = origin.to_string_lossy().into_owned();
            fixture_git_ok(&repo, &["remote", "add", "origin", origin_url.as_str()]);
            fixture_git_ok(&repo, &["push", "-q", "-u", "origin", "feature"]);
            fixture_git_ok(
                &repo,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/acme/repo.git",
                ],
            );
            let script = r#"#!/bin/sh
D="$GH_FIXTURE_DIR"
all=$(printf '%s ' "$@" | tr '\n' ' ')
echo "$all" >> "$D/calls.log"
if [ -f "$D/fail" ]; then echo "boom: not authenticated" >&2; exit 1; fi
case "$all" in
  *collaborators/*/permission*)
    case "$all" in
      *collaborators/writer/permission*) echo write ;;
      *collaborators/reader/permission*) echo read ;;
      *collaborators/missing/permission*) echo 'gh: Not Found (HTTP 404)' >&2; exit 1 ;;
      *) echo 'gh: Forbidden (HTTP 403)' >&2; exit 1 ;;
    esac ;;
  *addPullRequestReviewThreadReply*)
    echo "$all" >> "$D/replies.log"
    echo '{"data":{"addPullRequestReviewThreadReply":{"comment":{"id":"c1"}}}}' ;;
  *resolveReviewThread*)
    echo "$all" >> "$D/resolves.log"
    echo '{"data":{"resolveReviewThread":{"thread":{"id":"t","isResolved":true}}}}' ;;
  *reviewThreads*)
    case "$all" in
      *page2cursor*) cat "$D/threads2.json" ;;
      *) cat "$D/threads.json" ;;
    esac ;;
  *reviews*) cat "$D/reviews.json" ;;
  *headRefName*)
    oid=$(git --git-dir "$D/origin.git" rev-parse refs/heads/feature) || exit
    printf '{"state":"OPEN","number":42,"title":"fixture","url":"https://github.com/acme/repo/pull/42","headRefName":"feature","baseRefName":"main","headRefOid":"%s","isCrossRepository":false}' "$oid" ;;
  *)
    case "$all" in
      *--jq*) cat "$D/head.txt" ;;
      *) printf '{"headRefOid":"%s","data":{"repository":{"pullRequest":{"headRefOid":"%s"}}}}' "$(cat "$D/head.txt")" "$(cat "$D/head.txt")" ;;
    esac ;;
esac
"#;
            crate::test_support::write_executable_script(&bin.join("gh"), script);
            let path = crate::test_support::prepend_to_path(&bin);
            let dir = crate::test_support::EnvGuard::set("GH_FIXTURE_DIR", tmp.path());
            let f = Self {
                tmp,
                _path: path,
                _dir: dir,
                _git_config: [
                    crate::test_support::EnvGuard::set("GIT_CONFIG_NOSYSTEM", "1"),
                    crate::test_support::EnvGuard::set("GIT_CONFIG_GLOBAL", "/dev/null"),
                ],
                _lock: lock,
            };
            f.write("head.txt", OID);
            f.reviews(&[]);
            f.threads(&[], None);
            f
        }
        fn write(&self, name: &str, content: &str) {
            std::fs::write(self.tmp.path().join(name), content).unwrap_or_else(|e| panic!("{e:?}"));
        }
        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.tmp.path().join(name)).unwrap_or_default()
        }
        fn reviews(&self, reviews: &[(&str, &str)]) {
            let nodes: Vec<Value> = reviews
                .iter()
                .map(|(login, oid)| json!({"author":{"login":login},"commit":{"oid":oid}}))
                .collect();
            self.write(
                "reviews.json",
                &json!({"data":{"repository":{"pullRequest":{"headRefOid":self.read("head.txt"),
                "reviews":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":nodes}}}}})
                .to_string(),
            );
        }
        fn threads(&self, nodes: &[Value], next: Option<&str>) {
            self.write("threads.json", &threads_json(nodes, next));
        }
        fn path(&self) -> &Path {
            self.tmp.path()
        }
        fn repo_path(&self) -> std::path::PathBuf {
            self.tmp.path().join("repo")
        }
        fn sync_reviews_to_head(&self) {
            let head = fixture_git_ok(&self.repo_path(), &["rev-parse", "HEAD"]);
            self.write("head.txt", &head);
            let mut reviews: Value = serde_json::from_str(&self.read("reviews.json"))
                .unwrap_or_else(|error| panic!("parse fixture reviews: {error}"));
            let pull_request = &mut reviews["data"]["repository"]["pullRequest"];
            pull_request["headRefOid"] = json!(head.clone());
            for review in pull_request["reviews"]["nodes"]
                .as_array_mut()
                .unwrap_or_else(|| panic!("fixture review nodes"))
            {
                review["commit"]["oid"] = json!(head.clone());
            }
            self.write("reviews.json", &reviews.to_string());
        }
    }

    fn threads_json(nodes: &[Value], next: Option<&str>) -> String {
        json!({"data":{"repository":{"pullRequest":{"reviewThreads":{
            "pageInfo":{"hasNextPage":next.is_some(),"endCursor":next},
            "nodes":nodes}}}}})
        .to_string()
    }

    fn thread_node(id: &str, resolved: bool, authors: &[&str]) -> Value {
        let comments: Vec<Value> = authors
            .iter()
            .enumerate()
            .map(|(i, a)| json!({"author":{"login":a},"body":format!("body-{id}-{i}")}))
            .collect();
        json!({"id":id,"isResolved":resolved,"viewerCanReply":true,"viewerCanResolve":true,
            "path":"src/lib.rs","line":7,
            "comments":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":comments}})
    }

    fn pr(f: &Fixture) -> PullRequestRef {
        PullRequestRef {
            number: 42,
            working_dir: f.repo_path(),
        }
    }

    fn bots() -> Vec<String> {
        vec![BOT.to_string()]
    }

    fn deadline_ms(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    fn thread(id: &str, reply: bool, resolve: bool) -> ReviewThread {
        ReviewThread {
            id: id.to_string(),
            path: None,
            line: None,
            is_resolved: false,
            viewer_can_reply: reply,
            viewer_can_resolve: resolve,
            comments: vec![ReviewComment {
                author: BOT.to_string(),
                body: "fix".to_string(),
            }],
        }
    }

    fn act(id: &str, reply: Option<&str>, resolve: bool) -> ReviewAction {
        ReviewAction {
            thread_id: id.to_string(),
            reply: reply.map(str::to_string),
            resolve,
        }
    }

    fn count(log: &str) -> usize {
        log.lines().filter(|l| !l.trim().is_empty()).count()
    }

    #[test]
    fn progress_compares_id_sets_not_order_or_count() {
        let before = [thread("a", true, true), thread("b", true, true)];
        assert!(same_thread_ids(
            &before,
            &[thread("b", true, true), thread("a", true, true)]
        ));
        assert!(!same_thread_ids(
            &before,
            &[thread("a", true, true), thread("c", true, true)]
        ));
        assert!(!same_thread_ids(&before, &[thread("a", true, true)]));
    }

    #[tokio::test]
    async fn all_reviewers_filters_and_caches_permissions() {
        let f = Fixture::new();
        f.threads(
            &[
                thread_node("a", false, &["writer"]),
                thread_node("b", false, &["writer"]),
                thread_node("c", false, &["reader"]),
                thread_node("d", false, &["missing"]),
            ],
            None,
        );
        let mut cache = std::collections::HashMap::new();
        for _ in 0..2 {
            let (included, excluded) =
                unresolved_review_threads(&pr(&f), ReviewerScope::All, &[], &mut cache, None)
                    .await
                    .unwrap_or_else(|error| panic!("fetch review threads: {error}"));
            assert_eq!(ids(&included), vec!["a", "b"]);
            assert_eq!(excluded, vec!["c", "d"]);
        }
        assert_eq!(
            f.read("calls.log")
                .lines()
                .filter(|line| line.contains("collaborators/"))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn all_reviewers_permission_error_fails_closed() {
        let f = Fixture::new();
        f.threads(&[thread_node("a", false, &["forbidden"])], None);
        let result = unresolved_review_threads(
            &pr(&f),
            ReviewerScope::All,
            &[],
            &mut std::collections::HashMap::new(),
            None,
        )
        .await;
        assert!(matches!(
            result,
            Err(CruiseError::GitHubReviewFailed { .. })
        ));
    }

    #[tokio::test]
    async fn head_oid_returns_current_head() {
        let f = Fixture::new();
        let oid = head_oid(&pr(&f), None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(oid, OID);
    }

    #[tokio::test]
    async fn head_oid_api_failure_is_github_review_failed() {
        let f = Fixture::new();
        f.write("fail", "1");
        let result = head_oid(&pr(&f), None).await;
        assert!(matches!(
            result,
            Err(CruiseError::GitHubReviewFailed { .. })
        ));
    }

    #[tokio::test]
    async fn wait_for_review_requires_exact_current_head_oid() {
        let f = Fixture::new();
        f.reviews(&[(BOT, "oldoid")]);
        let result = wait_for_review(&pr(&f), &bots(), OID, deadline_ms(400), None).await;
        assert!(matches!(
            result,
            Err(CruiseError::GitHubReviewFailed { .. })
        ));
    }

    #[tokio::test]
    async fn wait_for_review_ignores_unlisted_author() {
        let f = Fixture::new();
        f.reviews(&[("some-human", OID)]);
        let result = wait_for_review(&pr(&f), &bots(), OID, deadline_ms(400), None).await;
        assert!(matches!(
            result,
            Err(CruiseError::GitHubReviewFailed { .. })
        ));
    }

    #[tokio::test]
    async fn wait_for_review_accepts_any_allowlisted_bot_case_insensitively() {
        let f = Fixture::new();
        f.reviews(&[("Other-Bot[bot]", OID)]);
        let allow = vec![BOT.to_string(), "other-bot[bot]".to_string()];
        let result = wait_for_review(&pr(&f), &allow, OID, deadline_ms(5000), None).await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn wait_for_review_honors_cancel() {
        let f = Fixture::new();
        let token = CancellationToken::new();
        token.cancel();
        let result = wait_for_review(&pr(&f), &bots(), OID, deadline_ms(5000), Some(&token)).await;
        assert!(matches!(result, Err(CruiseError::Interrupted)));
    }

    #[tokio::test]
    async fn unresolved_bot_threads_filters_author_and_resolution() {
        let f = Fixture::new();
        f.threads(
            &[
                thread_node("keep", false, &[BOT, "human"]),
                thread_node("resolved", true, &[BOT]),
                thread_node("human-started", false, &["human", BOT]),
                thread_node("other-bot", false, &["x[bot]"]),
            ],
            None,
        );
        let threads = unresolved_bot_threads(&pr(&f), &bots(), None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        let ids: Vec<&str> = threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["keep"]);
        assert_eq!(threads[0].comments.len(), 2, "all comments are preserved");
        assert_eq!(threads[0].comments[1].author, "human");
    }
    #[tokio::test]
    async fn bot_scope_filters_threads_without_checking_collaborator_permissions() {
        let f = Fixture::new();
        f.threads(
            &[
                thread_node("keep", false, &[BOT, "human"]),
                thread_node("resolved", true, &[BOT]),
                thread_node("human-started", false, &["human", BOT]),
                thread_node("other-bot", false, &["x[bot]"]),
            ],
            None,
        );
        let (threads, excluded) = unresolved_review_threads(
            &pr(&f),
            ReviewerScope::Bots,
            &bots(),
            &mut std::collections::HashMap::new(),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("fetch bot review threads: {error}"));
        assert_eq!(ids(&threads), ["keep"]);
        assert_eq!(excluded, Vec::<String>::new());
        assert!(
            f.read("calls.log")
                .lines()
                .all(|line| !line.contains("collaborators/"))
        );
    }

    #[tokio::test]
    async fn unresolved_bot_threads_follows_pagination_to_the_end() {
        let f = Fixture::new();
        f.threads(&[thread_node("p1", false, &[BOT])], Some("page2cursor"));
        f.write(
            "threads2.json",
            &threads_json(&[thread_node("p2", false, &[BOT])], None),
        );
        let threads = unresolved_bot_threads(&pr(&f), &bots(), None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        let ids: Vec<&str> = threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["p1", "p2"]);
    }

    #[test]
    fn parse_review_actions_accepts_valid_actions_and_empty_list() {
        let threads = [thread("t1", true, true)];
        let actions = parse_review_actions(
            r#"{"actions":[{"thread_id":"t1","reply":"done","resolve":true}]}"#,
            &threads,
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(actions, vec![act("t1", Some("done"), true)]);
        let empty =
            parse_review_actions(r#"{"actions":[]}"#, &threads).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(empty, Vec::new());
    }

    #[test]
    fn parse_review_actions_rejects_invalid_responses() {
        let threads = [thread("t1", true, true)];
        for bad in [
            "not json",
            r#"{"actions":[{"thread_id":"unknown","resolve":true}]}"#,
            r#"{"actions":[{"thread_id":"t1","resolve":true},{"thread_id":"t1","resolve":true}]}"#,
            r#"{"actions":[{"thread_id":"t1","reply":"x"}]}"#,
            r#"{"actions":[{"thread_id":"t1","reply":"","resolve":false}]}"#,
            r#"{"actions":[{"thread_id":"t1","resolve":true}],"extra":1}"#,
            r"{}",
        ] {
            let result = parse_review_actions(bad, &threads);
            assert!(
                matches!(result, Err(CruiseError::GitHubReviewFailed { .. })),
                "should reject {bad}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn apply_actions_rejects_missing_permissions_before_any_mutation() {
        let f = Fixture::new();
        let threads = [thread("t1", false, true), thread("t2", true, false)];
        for action in [act("t1", Some("hi"), false), act("t2", None, true)] {
            let result = apply_actions(&pr(&f), &threads, &[action], None).await;
            assert!(matches!(
                result,
                Err(CruiseError::GitHubReviewFailed { .. })
            ));
        }
        assert_eq!(
            count(&f.read("replies.log")) + count(&f.read("resolves.log")),
            0
        );
    }

    #[tokio::test]
    async fn apply_actions_rejects_foreign_thread_id_without_mutation() {
        let f = Fixture::new();
        let result = apply_actions(
            &pr(&f),
            &[thread("t1", true, true)],
            &[act("foreign", Some("hi"), true)],
            None,
        )
        .await;
        assert!(matches!(
            result,
            Err(CruiseError::GitHubReviewFailed { .. })
        ));
        assert_eq!(
            count(&f.read("replies.log")) + count(&f.read("resolves.log")),
            0
        );
    }

    #[tokio::test]
    async fn apply_actions_replies_and_resolves_only_selected_threads() {
        let f = Fixture::new();
        f.threads(
            &[
                thread_node("t1", false, &[BOT]),
                thread_node("t2", false, &[BOT]),
            ],
            None,
        );
        let threads = [thread("t1", true, true), thread("t2", true, true)];
        apply_actions(
            &pr(&f),
            &threads,
            &[act("t1", Some("fixed it"), true)],
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        let replies = f.read("replies.log");
        let resolves = f.read("resolves.log");
        assert_eq!(count(&replies), 1);
        assert!(replies.contains("t1") && replies.contains("fixed it"));
        assert!(!replies.contains("t2"));
        assert_eq!(count(&resolves), 1);
        assert!(resolves.contains("t1") && !resolves.contains("t2"));
    }

    #[tokio::test]
    async fn apply_actions_does_not_repost_reply_already_present() {
        let f = Fixture::new();
        f.threads(&[thread_node("t1", false, &[BOT])], None);
        let threads = [thread("t1", true, true)];
        let actions = [act("t1", Some("fixed it"), false)];
        apply_actions(&pr(&f), &threads, &actions, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        let posted = f.read("replies.log");
        assert_eq!(count(&posted), 1);

        // Simulate resume: the posted reply (with its marker) is now on the thread.
        let mut node = thread_node("t1", false, &[BOT]);
        node["comments"]["nodes"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("nodes"))
            .push(json!({"author":{"login":"me"},"body":posted}));
        f.threads(&[node], None);
        apply_actions(&pr(&f), &threads, &actions, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            count(&f.read("replies.log")),
            1,
            "reply must not be duplicated"
        );
    }

    #[tokio::test]
    async fn apply_actions_resolve_is_noop_for_already_resolved_thread() {
        let f = Fixture::new();
        f.threads(&[thread_node("t1", true, &[BOT])], None);
        let mut t = thread("t1", true, true);
        t.is_resolved = true;
        apply_actions(&pr(&f), &[t], &[act("t1", None, true)], None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(count(&f.read("resolves.log")), 0);
    }

    // -- after-pr integration ---------------------------------------------------

    use crate::file_tracker::FileTracker;
    use crate::option_handler::NoOpOptionHandler;
    use crate::variable::VariableStore;

    async fn run_after_pr_sdk(f: &Fixture, yaml: &str) -> Result<()> {
        // Exercise compilation and the after-pr conversion without forcing the
        // command backend used by the other integration tests.
        let config =
            crate::config::WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        crate::config::validate_config(&config).unwrap_or_else(|e| panic!("{e:?}"));
        let compiled = crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new("input".to_string());
        vars.set_named_value("pr.number", "42".to_string());
        vars.set_artifacts_root(f.path().join("artifacts"));
        f.sync_reviews_to_head();
        let repo = f.repo_path();
        let mut tracker = FileTracker::with_root(repo.clone());
        crate::worktree_pr::run_after_pr_steps_for_test(
            &compiled,
            &mut vars,
            &mut tracker,
            &repo,
            &NoOpOptionHandler,
        )
        .await
    }

    fn install_after_pr_claude(f: &Fixture) {
        crate::test_support::write_executable_script(
            &f.path().join("bin/claude"),
            r#"#!/bin/sh
read -r user_frame
printf '%s\n' "$user_frame" >> "$GH_FIXTURE_DIR/claude-input"
printf '%s\n' "${CLAUDE_CODE_SUBAGENT_MODEL-}" "${CLAUDE_CODE_SUBAGENT_MODEL_FORCE-}" >> "$GH_FIXTURE_DIR/claude-subagent"
cat <<'JSON'
{"type":"system","subtype":"init","session_id":"fake-after-pr-session"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"{\"actions\":[]}"}]},"session_id":"fake-after-pr-session"}
{"type":"result","subtype":"success","is_error":false,"session_id":"fake-after-pr-session","result":"{\"actions\":[]}"}
JSON
"#,
        );
    }

    #[tokio::test]
    async fn ordinary_after_pr_prompt_inherits_workflow_subagent_model() {
        let f = Fixture::new();
        install_after_pr_claude(&f);
        let result = run_after_pr_sdk(
            &f,
            "sdk: claude\nmodel: sonnet\nsubagent_model: haiku\nenv:\n  CLAUDE_CODE_SUBAGENT_MODEL: stale-workflow-env\n  CLAUDE_CODE_SUBAGENT_MODEL_FORCE: '0'\nsteps:\n  main:\n    prompt: unused main\nafter-pr:\n  answer:\n    prompt: AFTER_PR_GLOBAL_SUBAGENT\n    timeout: 30\n",
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(f.read("claude-input").contains("AFTER_PR_GLOBAL_SUBAGENT"));
        assert_eq!(f.read("claude-subagent"), "haiku\n1\n");
    }

    #[tokio::test]
    async fn github_review_prompt_overrides_workflow_subagent_model() {
        let f = Fixture::new();
        install_after_pr_claude(&f);
        f.reviews(&[(BOT, OID)]);
        f.threads(&[thread_node("t1", false, &[BOT])], None);
        let yaml = format!(
            "sdk: claude\nmodel: sonnet\nsubagent_model: haiku\nsteps:\n  main:\n    prompt: unused main\nafter-pr:\n  review:\n    github-review:\n      bots: [\"{BOT}\"]\n      max-iterations: 1\n    prompt: GITHUB_REVIEW_SUBAGENT_OVERRIDE\n    subagent_model: opus\n    env:\n      CLAUDE_CODE_SUBAGENT_MODEL: stale-step-env\n      CLAUDE_CODE_SUBAGENT_MODEL_FORCE: '0'\n    timeout: 30\n"
        );
        let result = run_after_pr_sdk(&f, &yaml).await;
        assert!(
            f.read("claude-input")
                .contains("GITHUB_REVIEW_SUBAGENT_OVERRIDE"),
            "review must invoke Claude before reaching the iteration limit: {result:?}"
        );
        assert_eq!(f.read("claude-subagent"), "opus\n1\n");
        assert!(
            matches!(&result, Err(CruiseError::GitHubReviewFailed { .. })),
            "{result:?}"
        );
    }

    async fn run_after_pr(f: &Fixture, yaml: &str, command: &str) -> (Result<()>, VariableStore) {
        let mut config =
            crate::config::WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        config.command = vec!["sh".into(), "-c".into(), command.into()];
        crate::config::validate_config(&config).unwrap_or_else(|e| panic!("{e:?}"));
        let compiled = crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new("input".to_string());
        vars.set_named_value("pr.number", "42".to_string());
        vars.set_artifacts_root(f.path().join("artifacts"));
        f.sync_reviews_to_head();
        let repo = f.repo_path();
        let mut tracker = FileTracker::with_root(repo.clone());
        let result = crate::worktree_pr::run_after_pr_steps_for_test(
            &compiled,
            &mut vars,
            &mut tracker,
            &repo,
            &NoOpOptionHandler,
        )
        .await;
        (result, vars)
    }

    fn review_yaml(extra: &str) -> String {
        format!(
            "steps:\n  main:\n    prompt: hi\nafter-pr:\n  review:\n    github-review:\n      bots: [\"{BOT}\"]\n      max-iterations: 2\n    prompt: fix {{pr.number}}\n    timeout: 30\n{extra}"
        )
    }

    #[tokio::test]
    async fn github_review_skips_prompt_when_no_bot_threads() {
        let f = Fixture::new();
        f.reviews(&[(BOT, OID)]);
        let marker = f.path().join("prompt-ran");
        let cmd = format!("touch {}; echo '{{\"actions\":[]}}'", marker.display());
        let (result, _) = run_after_pr(&f, &review_yaml(""), &cmd).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn github_review_fails_with_unresolved_threads_at_iteration_limit() {
        let f = Fixture::new();
        f.reviews(&[(BOT, OID)]);
        f.threads(&[thread_node("t1", false, &[BOT])], None);
        let (result, _) = run_after_pr(
            &f,
            &review_yaml("    output_file: report.json\n"),
            "echo '{\"actions\":[]}'",
        )
        .await;
        assert!(
            matches!(result, Err(CruiseError::GitHubReviewFailed { .. })),
            "{result:?}"
        );
        let report = crate::artifacts::read(&f.path().join("artifacts"), "report.json")
            .unwrap_or_else(|e| panic!("report must be kept on failure: {e:?}"));
        assert!(report.contains("t1"));
    }

    #[tokio::test]
    async fn github_review_invalid_prompt_response_fails_without_mutation() {
        let f = Fixture::new();
        f.reviews(&[(BOT, OID)]);
        f.threads(&[thread_node("t1", false, &[BOT])], None);
        let (result, _) = run_after_pr(&f, &review_yaml(""), "echo 'not json'").await;
        assert!(
            matches!(result, Err(CruiseError::GitHubReviewFailed { .. })),
            "{result:?}"
        );
        assert_eq!(
            count(&f.read("replies.log")) + count(&f.read("resolves.log")),
            0
        );
    }

    #[tokio::test]
    async fn github_review_api_failure_fails_run_but_regular_after_pr_failure_warns() {
        let f = Fixture::new();
        f.write("fail", "1");
        let (result, _) = run_after_pr(&f, &review_yaml(""), "echo '{\"actions\":[]}'").await;
        assert!(
            matches!(result, Err(CruiseError::GitHubReviewFailed { .. })),
            "{result:?}"
        );

        let regular =
            "steps:\n  main:\n    prompt: hi\nafter-pr:\n  boom:\n    command: \"exit 1\"\n";
        let (result, _) = run_after_pr(&f, regular, "true").await;
        assert!(
            result.is_ok(),
            "regular after-pr failures stay warnings: {result:?}"
        );
    }

    #[tokio::test]
    async fn github_review_report_keeps_earlier_iterations_when_later_iteration_fails() {
        let f = Fixture::new();
        f.reviews(&[(BOT, OID)]);
        f.threads(&[thread_node("t1", false, &[BOT])], None);
        let marker = f.path().join("first-done");
        let cmd = format!(
            "if [ -e {m} ]; then echo 'not json'; else touch {m}; echo '{{\"actions\":[]}}'; fi",
            m = marker.display()
        );
        let (result, _) =
            run_after_pr(&f, &review_yaml("    output_file: report.json\n"), &cmd).await;
        assert!(
            matches!(result, Err(CruiseError::GitHubReviewFailed { .. })),
            "{result:?}"
        );
        let report = crate::artifacts::read(&f.path().join("artifacts"), "report.json")
            .unwrap_or_else(|e| panic!("report must be kept on failure: {e:?}"));
        assert!(
            report.contains("\"iteration\": 1") && report.contains("t1"),
            "{report}"
        );
    }
}
