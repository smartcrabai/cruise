//! Opt-in GitHub review bot post-processing loop (after-pr only).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cancellation::CancellationToken;
use crate::engine::{ExecutionContext, PromptStepOutput, run_prompt_step};
use crate::error::{CruiseError, Result};
use crate::step::GitHubReviewStep;
use crate::variable::VariableStore;

const POLL_INTERVAL: Duration = Duration::from_secs(10);
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

/// # Errors
/// `GitHubReviewFailed` on permission/API failure, `Interrupted` on cancel.
pub async fn apply_actions(
    pr: &PullRequestRef,
    threads: &[ReviewThread],
    actions: &[ReviewAction],
    cancel: Option<&CancellationToken>,
) -> Result<()> {
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
    if actions.is_empty() {
        return Ok(());
    }
    let current = fetch_threads(pr, cancel).await?;
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
        "\n\n## GitHub review context\n\nPR head commit: {head_oid}\n\nUnresolved review threads from allowlisted bots (JSON):\n{threads_json}\n\nAfter making and committing any needed fixes, respond with ONLY a JSON object of the form {{\"actions\":[{{\"thread_id\":\"<id>\",\"reply\":\"<optional text>\",\"resolve\":true}}]}}. Use only thread ids listed above, at most once each. Each action needs a non-empty reply or resolve:true. An empty actions list is allowed.\n"
    );
    format!("{base}{}", context.replace('{', "{{").replace('}', "}}"))
}

async fn git_output(
    pr: &PullRequestRef,
    args: &[&str],
    cancel: Option<&CancellationToken>,
) -> Result<std::process::Output> {
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    run_process("git", &args, pr, cancel).await
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

/// Reject a dirty worktree and push HEAD when the prompt committed.
async fn publish_prompt_commits(
    pr: &PullRequestRef,
    head_before: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    let Some(before) = head_before else {
        return Ok(());
    };
    let status = git_output(pr, &["status", "--porcelain"], cancel).await?;
    if !status.status.success() {
        return Err(failed("git status failed"));
    }
    if !status.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(failed(
            "the prompt left uncommitted changes; commit them or leave the worktree clean",
        ));
    }
    if git_head(pr, cancel).await?.as_deref() != Some(before) {
        let push = git_output(pr, &["push"], cancel).await?;
        if !push.status.success() {
            return Err(failed(format!(
                "git push failed: {}",
                String::from_utf8_lossy(&push.stderr).trim()
            )));
        }
    }
    Ok(())
}

fn write_report(
    vars: &VariableStore,
    file: Option<&str>,
    iterations: &[Value],
    error: Option<&str>,
) -> Result<()> {
    let Some(file) = file else {
        return Ok(());
    };
    let report = json!({"iterations": iterations, "error": error});
    let text = serde_json::to_string_pretty(&report).map_err(|e| failed(e.to_string()))?;
    vars.write_artifact(file, &text)
}

fn ids(threads: &[ReviewThread]) -> Vec<&str> {
    threads.iter().map(|t| t.id.as_str()).collect()
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
    )
    .await;
    match result {
        Ok(()) => {
            write_report(vars, step.report_file.as_deref(), &report, None)?;
            Ok(())
        }
        Err(error) => {
            let error = with_step(error, step_name);
            let _ = write_report(
                vars,
                step.report_file.as_deref(),
                &report,
                Some(&error.to_string()),
            );
            Err(error)
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "loop state is passed explicitly")]
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
) -> Result<()> {
    let cancel = ctx.cancel_token;
    let bots = &step.review.bots;
    for iteration in 1..=step.review.max_iterations {
        let head = head_oid(pr, cancel).await?;
        wait_for_review(pr, bots, &head, deadline, cancel).await?;
        let threads = unresolved_bot_threads(pr, bots, cancel).await?;
        if threads.is_empty() {
            report.push(json!({"iteration": iteration, "head_oid": head, "bots": bots, "thread_ids": Vec::<&str>::new(), "actions": [], "remaining_thread_ids": Vec::<&str>::new()}));
            write_report(vars, step.report_file.as_deref(), report, None)?;
            return Ok(());
        }
        report.push(json!({"iteration": iteration, "head_oid": head, "bots": bots, "thread_ids": ids(&threads)}));

        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| failed("timed out before running the prompt"))?;
        let mut prompt = step.prompt.clone();
        prompt.prompt = build_prompt(&prompt.prompt, &head, &threads);
        let head_before = git_head(pr, cancel).await?;
        match Box::pin(run_prompt_step(
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
            false,
            allow_commit,
        ))
        .await
        {
            Ok(_) => {}
            Err(CruiseError::StepTimeout { .. }) => {
                return Err(failed("timed out while running the prompt"));
            }
            Err(e) => return Err(e),
        }
        let output = vars.prev_output().unwrap_or_default().to_string();
        let actions = parse_review_actions(&output, &threads)?;
        publish_prompt_commits(pr, head_before.as_deref(), cancel).await?;
        apply_actions(pr, &threads, &actions, cancel).await?;
        let left = unresolved_bot_threads(pr, bots, cancel).await?;
        if let Some(entry) = report.last_mut() {
            entry["actions"] = json!(actions.iter().map(|a| json!({"thread_id": a.thread_id, "reply": a.reply.is_some(), "resolve": a.resolve})).collect::<Vec<_>>());
            entry["remaining_thread_ids"] = json!(ids(&left));
        }
        write_report(vars, step.report_file.as_deref(), report, None)?;
        if left.is_empty() {
            return Ok(());
        }
    }
    Err(failed(format!(
        "unresolved bot threads remain after {} iterations",
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
        _lock: crate::test_support::ProcessLock,
        _path: crate::test_support::EnvGuard,
        _dir: crate::test_support::EnvGuard,
    }

    impl Fixture {
        fn new() -> Self {
            let lock = crate::test_support::lock_process();
            let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
            let bin = tmp.path().join("bin");
            std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e:?}"));
            let script = r#"#!/bin/sh
D="$GH_FIXTURE_DIR"
all=$(printf '%s ' "$@" | tr '\n' ' ')
echo "$all" >> "$D/calls.log"
case "$all" in
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
  *)
    if [ -f "$D/fail" ]; then echo "boom: not authenticated" >&2; exit 1; fi
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
                _lock: lock,
                _path: path,
                _dir: dir,
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
            working_dir: f.path().to_path_buf(),
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

    async fn run_after_pr(f: &Fixture, yaml: &str, command: &str) -> (Result<()>, VariableStore) {
        let mut config =
            crate::config::WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        config.command = vec!["sh".into(), "-c".into(), command.into()];
        crate::config::validate_config(&config).unwrap_or_else(|e| panic!("{e:?}"));
        let compiled = crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new("input".to_string());
        vars.set_named_value("pr.number", "42".to_string());
        vars.set_artifacts_root(f.path().join("artifacts"));
        let mut tracker = FileTracker::with_root(f.path().to_path_buf());
        let result = crate::worktree_pr::run_after_pr_steps_for_test(
            &compiled,
            &mut vars,
            &mut tracker,
            f.path(),
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
