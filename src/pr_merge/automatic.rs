//! Fail-closed synchronous squash merging for active after-pr workflows.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::cancellation::CancellationToken;

const PROCESS_TIMEOUT: Duration = Duration::from_secs(120);
const GITHUB_STATUS_QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!,$after:String){repository(owner:$owner,name:$repo){squashMergeAllowed pullRequest(number:$number){state isDraft headRefOid baseRefName mergeable mergeStateStatus reviewDecision mergeQueueEntry{id} autoMergeRequest{enabledAt} statusCheckRollup{state commit{oid} contexts(first:100,after:$after){totalCount pageInfo{hasNextPage endCursor} nodes{__typename ... on CheckRun{name status conclusion} ... on StatusContext{context state}}}}}}}";
const GITHUB_IDENTITY_QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$number){state headRefOid}}}";

/// Request to synchronously squash the open pull request associated with the current worktree.
pub(crate) struct AutoSquashRequest<'a> {
    pub pr_url: &'a str,
    pub working_dir: &'a Path,
    pub env: &'a HashMap<String, String>,
    pub cancel_token: Option<&'a CancellationToken>,
}

/// Result of a completed automatic squash operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoSquashOutcome {
    Merged,
    AlreadyMerged,
}

/// A safe, actionable failure from an automatic squash operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutoSquashError {
    Cancelled,
    Rejected(String),
    CommandFailed(String),
    Unconfirmed(String),
}

impl fmt::Display for AutoSquashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("automatic squash merge was cancelled"),
            Self::Rejected(reason) => {
                write!(formatter, "automatic squash merge rejected: {reason}")
            }
            Self::CommandFailed(reason) => {
                write!(formatter, "automatic squash merge command failed: {reason}")
            }
            Self::Unconfirmed(reason) => {
                write!(formatter, "automatic squash merge is unconfirmed: {reason}")
            }
        }
    }
}

impl Error for AutoSquashError {}

#[derive(Debug, Clone, Copy)]
enum ProcessError {
    CancelledBeforeStart,
    Cancelled,
    Failed,
    TimedOut,
}

async fn wait_for_cancel(token: Option<&CancellationToken>) {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

fn check_cancel(token: Option<&CancellationToken>) -> Result<(), AutoSquashError> {
    if token.is_some_and(CancellationToken::is_cancelled) {
        Err(AutoSquashError::Cancelled)
    } else {
        Ok(())
    }
}

async fn run_process(
    program: &str,
    args: &[String],
    working_dir: &Path,
    env: &HashMap<String, String>,
    cancel_token: Option<&CancellationToken>,
) -> Result<Output, ProcessError> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(ProcessError::CancelledBeforeStart);
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(working_dir)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let result = tokio::select! {
        biased;
        result = tokio::time::timeout(PROCESS_TIMEOUT, command.output()) => result,
        () = wait_for_cancel(cancel_token) => return Err(ProcessError::Cancelled),
    };
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(_)) => Err(ProcessError::Failed),
        Err(_) => Err(ProcessError::TimedOut),
    }
}

fn run_error(error: ProcessError, command: &'static str) -> AutoSquashError {
    match error {
        ProcessError::CancelledBeforeStart | ProcessError::Cancelled => AutoSquashError::Cancelled,
        ProcessError::Failed | ProcessError::TimedOut => {
            AutoSquashError::CommandFailed(format!("unable to run {command}"))
        }
    }
}

fn successful(output: &Output) -> bool {
    output.status.success()
}

async fn git_output(
    args: &[&str],
    request: &AutoSquashRequest<'_>,
) -> Result<Vec<u8>, AutoSquashError> {
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    let output = run_process(
        "git",
        &owned,
        request.working_dir,
        request.env,
        request.cancel_token,
    )
    .await
    .map_err(|error| run_error(error, "git"))?;
    if !successful(&output) {
        return Err(AutoSquashError::CommandFailed(
            "unable to inspect the local Git worktree".to_string(),
        ));
    }
    Ok(output.stdout)
}

async fn local_head(request: &AutoSquashRequest<'_>) -> Result<String, AutoSquashError> {
    let status = git_output(
        &["status", "--porcelain=v1", "--untracked-files=all"],
        request,
    )
    .await?;
    if !status.is_empty() {
        return Err(AutoSquashError::Rejected(
            "the local worktree is not clean".to_string(),
        ));
    }
    let head = git_output(&["rev-parse", "--verify", "HEAD"], request).await?;
    let head = std::str::from_utf8(&head)
        .map_err(|_| AutoSquashError::Rejected("local Git HEAD is unreadable".to_string()))?
        .trim();
    if !is_object_id(head) {
        return Err(AutoSquashError::Rejected(
            "local Git HEAD is unavailable".to_string(),
        ));
    }
    Ok(head.to_string())
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
enum ProviderRef {
    GitHub {
        hostname: String,
        owner: String,
        repo: String,
        number: u64,
    },
    GitLab {
        hostname: String,
        project: String,
        iid: u64,
    },
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn parse_provider_ref(pr_url: &str) -> Result<ProviderRef, AutoSquashError> {
    let rejected = || {
        AutoSquashError::Rejected(
            "pull request URL must be a valid HTTPS GitHub or GitLab merge request URL".into(),
        )
    };
    let Some(rest) = pr_url.strip_prefix("https://") else {
        return Err(rejected());
    };
    if rest.bytes().any(|byte| matches!(byte, b'?' | b'#' | b'@')) {
        return Err(rejected());
    }
    let Some((authority, path)) = rest.split_once('/') else {
        return Err(rejected());
    };
    if authority.is_empty()
        || !authority
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
    {
        return Err(rejected());
    }
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    if let Some(marker) = segments.iter().position(|segment| *segment == "-") {
        if segments.get(marker + 1) == Some(&"merge_requests")
            && marker >= 2
            && marker + 3 == segments.len()
            && segments[..marker]
                .iter()
                .all(|segment| valid_component(segment))
        {
            let iid = segments[marker + 2]
                .parse::<u64>()
                .map_err(|_| rejected())?;
            if iid == 0 {
                return Err(rejected());
            }
            return Ok(ProviderRef::GitLab {
                hostname: authority.to_ascii_lowercase(),
                project: segments[..marker].join("/"),
                iid,
            });
        }
        return Err(rejected());
    }
    if segments.len() == 4
        && valid_component(segments[0])
        && valid_component(segments[1])
        && segments[2] == "pull"
    {
        let number = segments[3].parse::<u64>().map_err(|_| rejected())?;
        let repo = segments[1].trim_end_matches(".git");
        if number == 0 || repo.is_empty() {
            return Err(rejected());
        }
        return Ok(ProviderRef::GitHub {
            hostname: authority.to_ascii_lowercase(),
            owner: segments[0].to_string(),
            repo: repo.to_string(),
            number,
        });
    }
    Err(rejected())
}

fn url_encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubMetadata {
    state: String,
    head_oid: String,
    base_branch: String,
    mergeable: String,
    merge_state: String,
    review_decision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubSnapshot {
    metadata: GitHubMetadata,
    check_state: String,
    check_commit_oid: String,
    context_total: usize,
}

struct GitHubCiPage {
    check_state: String,
    check_commit_oid: String,
    context_total: usize,
    check_names: Vec<String>,
    has_next_page: bool,
    end_cursor: Option<String>,
}

fn required_str<'a>(value: &'a Value, key: &'static str) -> Result<&'a str, AutoSquashError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AutoSquashError::Rejected(format!("provider response is missing {key}")))
}

fn required_bool(value: &Value, key: &'static str) -> Result<bool, AutoSquashError> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| AutoSquashError::Rejected(format!("provider response is missing {key}")))
}

fn parse_github_metadata(
    pr: &Value,
    repository: &Value,
) -> Result<GitHubMetadata, AutoSquashError> {
    let state = required_str(pr, "state")?.to_string();
    let is_draft = required_bool(pr, "isDraft")?;
    let squash_allowed = required_bool(repository, "squashMergeAllowed")?;
    let queue_entry = pr.get("mergeQueueEntry").ok_or_else(|| {
        AutoSquashError::Rejected("GitHub merge queue status is unavailable".to_string())
    })?;
    let auto_merge = pr.get("autoMergeRequest").ok_or_else(|| {
        AutoSquashError::Rejected("GitHub auto-merge status is unavailable".to_string())
    })?;
    let head_oid = required_str(pr, "headRefOid")?.to_string();
    if !is_object_id(&head_oid) {
        return Err(AutoSquashError::Rejected(
            "GitHub pull request head is unavailable".to_string(),
        ));
    }
    let review = pr.get("reviewDecision").ok_or_else(|| {
        AutoSquashError::Rejected("GitHub review status is unavailable".to_string())
    })?;
    let review_decision = if review.is_null() {
        None
    } else {
        Some(
            review
                .as_str()
                .filter(|decision| !decision.is_empty())
                .ok_or_else(|| {
                    AutoSquashError::Rejected("GitHub review status is malformed".into())
                })?
                .to_string(),
        )
    };
    let metadata = GitHubMetadata {
        state,
        head_oid,
        base_branch: required_str(pr, "baseRefName")?.to_string(),
        mergeable: required_str(pr, "mergeable")?.to_string(),
        merge_state: required_str(pr, "mergeStateStatus")?.to_string(),
        review_decision,
    };
    if metadata.state == "OPEN" {
        if is_draft {
            return Err(AutoSquashError::Rejected(
                "GitHub pull request is a draft".to_string(),
            ));
        }
        if !queue_entry.is_null() {
            return Err(AutoSquashError::Rejected(
                "GitHub pull request is in a merge queue".to_string(),
            ));
        }
        if !auto_merge.is_null() {
            return Err(AutoSquashError::Rejected(
                "GitHub pull request already has auto-merge enabled".to_string(),
            ));
        }
        if metadata.mergeable != "MERGEABLE" || metadata.merge_state != "CLEAN" {
            return Err(AutoSquashError::Rejected(
                "GitHub pull request is not confirmed mergeable".to_string(),
            ));
        }
        if !squash_allowed {
            return Err(AutoSquashError::Rejected(
                "GitHub repository policy does not allow squash merging".to_string(),
            ));
        }
    }
    Ok(metadata)
}

fn parse_github_ci_page(rollup: &Value) -> Result<GitHubCiPage, AutoSquashError> {
    let contexts = rollup
        .get("contexts")
        .filter(|node| node.is_object())
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI checks are unavailable".into()))?;
    let nodes = contexts
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI checks are incomplete".into()))?;
    let context_total = contexts
        .get("totalCount")
        .and_then(Value::as_u64)
        .and_then(|count| usize::try_from(count).ok())
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI check count is unavailable".into()))?;
    let page_info = contexts
        .get("pageInfo")
        .filter(|node| node.is_object())
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI pagination is unavailable".into()))?;
    let has_next_page = page_info
        .get("hasNextPage")
        .and_then(Value::as_bool)
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI pagination is incomplete".into()))?;
    let end_cursor = page_info
        .get("endCursor")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if has_next_page && end_cursor.is_none() {
        return Err(AutoSquashError::Rejected(
            "GitHub CI pagination is incomplete".to_string(),
        ));
    }
    let mut check_names = Vec::with_capacity(nodes.len());
    for node in nodes {
        let name = match required_str(node, "__typename")? {
            "CheckRun" => required_str(node, "name")?,
            "StatusContext" => required_str(node, "context")?,
            _ => {
                return Err(AutoSquashError::Rejected(
                    "GitHub returned an unknown CI check type".to_string(),
                ));
            }
        };
        check_names.push(name.to_string());
    }
    Ok(GitHubCiPage {
        check_state: required_str(rollup, "state")?.to_string(),
        check_commit_oid: rollup
            .pointer("/commit/oid")
            .and_then(Value::as_str)
            .filter(|oid| !oid.is_empty())
            .ok_or_else(|| AutoSquashError::Rejected("GitHub CI head is unavailable".into()))?
            .to_string(),
        context_total,
        check_names,
        has_next_page,
        end_cursor,
    })
}

fn parse_github_snapshot(
    value: &Value,
) -> Result<(GitHubSnapshot, Vec<String>, bool, Option<String>), AutoSquashError> {
    if value
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        return Err(AutoSquashError::CommandFailed(
            "GitHub could not provide complete pull request status".to_string(),
        ));
    }
    let pr = value
        .pointer("/data/repository/pullRequest")
        .filter(|node| node.is_object())
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub pull request data is unavailable".into())
        })?;
    let repository = value
        .pointer("/data/repository")
        .filter(|node| node.is_object())
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub repository policy is unavailable".into())
        })?;
    let metadata = parse_github_metadata(pr, repository)?;
    let rollup = pr
        .get("statusCheckRollup")
        .filter(|node| node.is_object())
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI status is unavailable".into()))?;
    let ci = parse_github_ci_page(rollup)?;
    let snapshot = GitHubSnapshot {
        metadata,
        check_state: ci.check_state,
        check_commit_oid: ci.check_commit_oid,
        context_total: ci.context_total,
    };
    Ok((snapshot, ci.check_names, ci.has_next_page, ci.end_cursor))
}

fn github_check_nodes_green(nodes: &[Value]) -> Result<(), AutoSquashError> {
    if nodes.is_empty() {
        return Ok(());
    }
    for node in nodes {
        match required_str(node, "__typename")? {
            "CheckRun" => {
                let status = required_str(node, "status")?;
                let conclusion = required_str(node, "conclusion")?;
                if status != "COMPLETED" || !matches!(conclusion, "SUCCESS" | "NEUTRAL" | "SKIPPED")
                {
                    return Err(AutoSquashError::Rejected(
                        "GitHub has a pending or unsuccessful CI check".to_string(),
                    ));
                }
            }
            "StatusContext" => {
                if required_str(node, "state")? != "SUCCESS" {
                    return Err(AutoSquashError::Rejected(
                        "GitHub has a pending or unsuccessful CI status".to_string(),
                    ));
                }
            }
            _ => {
                return Err(AutoSquashError::Rejected(
                    "GitHub returned an unknown CI check type".to_string(),
                ));
            }
        }
    }
    Ok(())
}

async fn github_graphql(
    reference: &ProviderRef,
    query: &str,
    after: Option<&str>,
    request: &AutoSquashRequest<'_>,
) -> Result<Value, AutoSquashError> {
    let ProviderRef::GitHub {
        hostname,
        owner,
        repo,
        number,
    } = reference
    else {
        return Err(AutoSquashError::Rejected(
            "provider URL changed unexpectedly".into(),
        ));
    };
    let mut args = vec![
        "api".to_string(),
        "--hostname".to_string(),
        hostname.clone(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={query}"),
        "-f".to_string(),
        format!("owner={owner}"),
        "-f".to_string(),
        format!("repo={repo}"),
        "-F".to_string(),
        format!("number={number}"),
    ];
    if let Some(after) = after {
        args.extend(["-f".to_string(), format!("after={after}")]);
    }
    let output = run_process(
        "gh",
        &args,
        request.working_dir,
        request.env,
        request.cancel_token,
    )
    .await
    .map_err(|error| run_error(error, "gh"))?;
    if !successful(&output) {
        return Err(AutoSquashError::CommandFailed(
            "unable to inspect GitHub pull request".to_string(),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| {
        AutoSquashError::Rejected("GitHub returned malformed pull request status".to_string())
    })
}

async fn github_identity(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
) -> Result<(String, String), AutoSquashError> {
    let value = github_graphql(reference, GITHUB_IDENTITY_QUERY, None, request).await?;
    if value
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        return Err(AutoSquashError::CommandFailed(
            "unable to inspect GitHub pull request state".to_string(),
        ));
    }
    let pr = value
        .pointer("/data/repository/pullRequest")
        .filter(|node| node.is_object())
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub pull request data is unavailable".into())
        })?;
    let state = required_str(pr, "state")?.to_string();
    let head_oid = required_str(pr, "headRefOid")?.to_string();
    if !is_object_id(&head_oid) {
        return Err(AutoSquashError::Rejected(
            "GitHub pull request head is unavailable".to_string(),
        ));
    }
    Ok((state, head_oid))
}

async fn inspect_github(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
) -> Result<(GitHubSnapshot, Vec<String>), AutoSquashError> {
    let mut cursor = None;
    let mut snapshot: Option<GitHubSnapshot> = None;
    let mut names = Vec::new();
    let mut node_count = 0usize;
    loop {
        let value =
            github_graphql(reference, GITHUB_STATUS_QUERY, cursor.as_deref(), request).await?;
        let parsed = parse_github_snapshot(&value)?;
        let (current, page_names, has_next, end_cursor) = parsed;
        if let Some(first) = &snapshot {
            if first != &current {
                return Err(AutoSquashError::Rejected(
                    "GitHub pull request changed while status was being read".to_string(),
                ));
            }
        } else {
            snapshot = Some(current);
        }
        let pr = value
            .pointer("/data/repository/pullRequest")
            .ok_or_else(|| {
                AutoSquashError::Rejected("GitHub pull request data is unavailable".into())
            })?;
        let nodes = pr
            .pointer("/statusCheckRollup/contexts/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| AutoSquashError::Rejected("GitHub CI checks are incomplete".into()))?;
        github_check_nodes_green(nodes)?;
        node_count = node_count.saturating_add(nodes.len());
        names.extend(page_names);
        if !has_next {
            break;
        }
        cursor = end_cursor;
    }
    let snapshot = snapshot.ok_or_else(|| {
        AutoSquashError::Rejected("GitHub pull request data is unavailable".into())
    })?;
    if snapshot.context_total == 0 || node_count != snapshot.context_total {
        return Err(AutoSquashError::Rejected(
            "GitHub CI checks are missing or incomplete".to_string(),
        ));
    }
    if snapshot.check_commit_oid != snapshot.metadata.head_oid {
        return Err(AutoSquashError::Rejected(
            "GitHub CI does not apply to the current pull request head".to_string(),
        ));
    }
    if snapshot.check_state != "SUCCESS" {
        return Err(AutoSquashError::Rejected(
            "GitHub CI has not passed for the current pull request head".to_string(),
        ));
    }
    Ok((snapshot, names))
}

async fn github_branch_rules(
    reference: &ProviderRef,
    branch: &str,
    request: &AutoSquashRequest<'_>,
) -> Result<Value, AutoSquashError> {
    let ProviderRef::GitHub {
        hostname,
        owner,
        repo,
        ..
    } = reference
    else {
        return Err(AutoSquashError::Rejected(
            "provider URL changed unexpectedly".into(),
        ));
    };
    let endpoint = format!(
        "repos/{owner}/{repo}/rules/branches/{}?per_page=100",
        url_encode_path_segment(branch)
    );
    let args = vec![
        "api".to_string(),
        "--hostname".to_string(),
        hostname.clone(),
        "--paginate".to_string(),
        "--slurp".to_string(),
        endpoint,
    ];
    let output = run_process(
        "gh",
        &args,
        request.working_dir,
        request.env,
        request.cancel_token,
    )
    .await
    .map_err(|error| run_error(error, "gh"))?;
    if !successful(&output) {
        return Err(AutoSquashError::CommandFailed(
            "unable to inspect active GitHub branch rules".to_string(),
        ));
    }
    let rules: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        AutoSquashError::Rejected("GitHub returned malformed branch rules".to_string())
    })?;
    if !rules.is_array() {
        return Err(AutoSquashError::Rejected(
            "GitHub active branch rules are unavailable".to_string(),
        ));
    }
    Ok(rules)
}
fn validate_github_required_status_checks(
    rule: &Value,
    checked: &HashSet<&str>,
) -> Result<(), AutoSquashError> {
    let required = rule
        .pointer("/parameters/required_status_checks")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub required status checks are unreadable".into())
        })?;
    for check in required {
        let context = required_str(check, "context")?;
        if !checked.contains(context) {
            return Err(AutoSquashError::Rejected(
                "a required GitHub CI check is missing from the current head".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_github_pull_request_rule(rule: &Value) -> Result<bool, AutoSquashError> {
    let parameters = rule
        .get("parameters")
        .filter(|parameters| parameters.is_object())
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub pull request policy is unreadable".to_string())
        })?;
    let allowed_methods = match parameters.get("allowed_merge_methods") {
        None => None,
        Some(value) => {
            let methods = value.as_array().ok_or_else(|| {
                AutoSquashError::Rejected("GitHub allowed merge methods are unreadable".into())
            })?;
            if methods
                .iter()
                .any(|method| !matches!(method.as_str(), Some("merge" | "squash" | "rebase")))
            {
                return Err(AutoSquashError::Rejected(
                    "GitHub allowed merge methods are malformed".into(),
                ));
            }
            Some(methods)
        }
    };
    if allowed_methods.is_some_and(|methods| {
        !methods
            .iter()
            .any(|method| method.as_str() == Some("squash"))
    }) {
        return Err(AutoSquashError::Rejected(
            "GitHub policy does not allow squash merging".to_string(),
        ));
    }
    let approvals_required = parameters
        .get("required_approving_review_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub required review count is unreadable".into())
        })?;
    let code_owner_reviews = parameters
        .get("require_code_owner_review")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub code-owner review policy is unreadable".into())
        })?;
    let last_push_approval = parameters
        .get("require_last_push_approval")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub last-push review policy is unreadable".into())
        })?;
    let required_reviewers: &[Value] = match parameters.get("required_reviewers") {
        None => &[],
        Some(value) => value.as_array().ok_or_else(|| {
            AutoSquashError::Rejected("GitHub required reviewers are unreadable".into())
        })?,
    };
    if required_reviewers
        .iter()
        .any(|reviewer| !reviewer.is_object())
    {
        return Err(AutoSquashError::Rejected(
            "GitHub required reviewer policy is malformed".to_string(),
        ));
    }
    Ok(approvals_required > 0
        || code_owner_reviews
        || last_push_approval
        || !required_reviewers.is_empty())
}

fn validate_github_rules(rules: &Value, check_names: &[String]) -> Result<bool, AutoSquashError> {
    let checked: HashSet<&str> = check_names.iter().map(String::as_str).collect();
    let mut reviews_required = false;
    for page in rules.as_array().into_iter().flatten() {
        let entries = page
            .as_array()
            .map_or_else(|| std::slice::from_ref(page), |items| items.as_slice());
        for rule in entries {
            match required_str(rule, "type")? {
                "merge_queue" => {
                    return Err(AutoSquashError::Rejected(
                        "the target branch requires a merge queue; synchronous squash merging is unavailable".to_string(),
                    ));
                }
                "required_status_checks" => {
                    validate_github_required_status_checks(rule, &checked)?;
                }
                "pull_request" => {
                    reviews_required |= validate_github_pull_request_rule(rule)?;
                }
                _ => {}
            }
        }
    }
    Ok(reviews_required)
}

fn github_merge_args(
    reference: &ProviderRef,
    head_oid: &str,
) -> Result<Vec<String>, AutoSquashError> {
    let ProviderRef::GitHub {
        hostname,
        owner,
        repo,
        number,
    } = reference
    else {
        return Err(AutoSquashError::Rejected(
            "provider URL changed unexpectedly".into(),
        ));
    };
    Ok(vec![
        "api".to_string(),
        "--hostname".to_string(),
        hostname.clone(),
        "--method".to_string(),
        "PUT".to_string(),
        format!("repos/{owner}/{repo}/pulls/{number}/merge"),
        "-f".to_string(),
        format!("sha={head_oid}"),
        "-f".to_string(),
        "merge_method=squash".to_string(),
    ])
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitLabSnapshot {
    state: String,
    is_draft: bool,
    head_oid: String,
    merge_status: String,
    merge_when_pipeline_succeeds: bool,
    pipeline_sha: String,
    pipeline_status: String,
}

fn gitlab_api_args(
    reference: &ProviderRef,
    endpoint_suffix: &str,
) -> Result<Vec<String>, AutoSquashError> {
    let ProviderRef::GitLab {
        hostname,
        project,
        iid,
    } = reference
    else {
        return Err(AutoSquashError::Rejected(
            "provider URL changed unexpectedly".into(),
        ));
    };
    let project = project
        .split('/')
        .map(url_encode_path_segment)
        .collect::<Vec<_>>()
        .join("%2F");
    let endpoint = format!("projects/{project}/merge_requests/{iid}{endpoint_suffix}");
    Ok(vec![
        "api".to_string(),
        "--hostname".to_string(),
        hostname.clone(),
        endpoint,
    ])
}

async fn gitlab_api(
    reference: &ProviderRef,
    endpoint_suffix: &str,
    request: &AutoSquashRequest<'_>,
) -> Result<Value, AutoSquashError> {
    let args = gitlab_api_args(reference, endpoint_suffix)?;
    let output = run_process(
        "glab",
        &args,
        request.working_dir,
        request.env,
        request.cancel_token,
    )
    .await
    .map_err(|error| run_error(error, "glab"))?;
    if !successful(&output) {
        return Err(AutoSquashError::CommandFailed(
            "unable to inspect GitLab merge request".to_string(),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| {
        AutoSquashError::Rejected("GitLab returned malformed merge request status".to_string())
    })
}

async fn gitlab_identity(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
) -> Result<(String, String), AutoSquashError> {
    let value = gitlab_api(reference, "", request).await?;
    let state = required_str(&value, "state")?.to_string();
    let head_oid = required_str(&value, "sha")?.to_string();
    if !is_object_id(&head_oid) {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request head is unavailable".to_string(),
        ));
    }
    Ok((state, head_oid))
}

async fn gitlab_project(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
) -> Result<Value, AutoSquashError> {
    let ProviderRef::GitLab {
        hostname, project, ..
    } = reference
    else {
        return Err(AutoSquashError::Rejected(
            "provider URL changed unexpectedly".into(),
        ));
    };
    let encoded = project
        .split('/')
        .map(url_encode_path_segment)
        .collect::<Vec<_>>()
        .join("%2F");
    let args = vec![
        "api".to_string(),
        "--hostname".to_string(),
        hostname.clone(),
        format!("projects/{encoded}"),
    ];
    let output = run_process(
        "glab",
        &args,
        request.working_dir,
        request.env,
        request.cancel_token,
    )
    .await
    .map_err(|error| run_error(error, "glab"))?;
    if !successful(&output) {
        return Err(AutoSquashError::CommandFailed(
            "unable to inspect GitLab project squash policy".to_string(),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| {
        AutoSquashError::Rejected("GitLab returned malformed project settings".to_string())
    })
}

fn parse_gitlab_snapshot(value: &Value) -> Result<GitLabSnapshot, AutoSquashError> {
    let head_oid = required_str(value, "sha")?.to_string();
    if !is_object_id(&head_oid) {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request head is unavailable".to_string(),
        ));
    }
    let pipeline = value
        .get("head_pipeline")
        .filter(|pipeline| pipeline.is_object())
        .ok_or_else(|| AutoSquashError::Rejected("GitLab CI pipeline is unavailable".into()))?;
    let pipeline_sha = required_str(pipeline, "sha")?.to_string();
    let pipeline_status = required_str(pipeline, "status")?.to_string();
    Ok(GitLabSnapshot {
        state: required_str(value, "state")?.to_string(),
        is_draft: required_bool(value, "draft")?,
        head_oid,
        merge_status: required_str(value, "detailed_merge_status")?.to_string(),
        merge_when_pipeline_succeeds: required_bool(value, "merge_when_pipeline_succeeds")?,
        pipeline_sha,
        pipeline_status,
    })
}

fn validate_gitlab_squash_policy(project: &Value) -> Result<(), AutoSquashError> {
    match required_str(project, "squash_option")? {
        "never" => Err(AutoSquashError::Rejected(
            "GitLab project policy does not allow squash merging".to_string(),
        )),
        "always" | "default_on" | "default_off" => Ok(()),
        _ => Err(AutoSquashError::Rejected(
            "GitLab project squash policy is unknown".to_string(),
        )),
    }
}

fn validate_gitlab_open(
    snapshot: &GitLabSnapshot,
    local_head: &str,
) -> Result<(), AutoSquashError> {
    if snapshot.state != "opened" {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request is not open".to_string(),
        ));
    }
    if snapshot.is_draft {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request is a draft".to_string(),
        ));
    }
    if snapshot.head_oid != local_head {
        return Err(AutoSquashError::Rejected(
            "local HEAD does not match the GitLab merge request head".to_string(),
        ));
    }
    if snapshot.merge_status != "mergeable" {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request is not confirmed mergeable".to_string(),
        ));
    }
    if snapshot.merge_when_pipeline_succeeds {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request already has auto-merge enabled".to_string(),
        ));
    }
    if snapshot.pipeline_sha != snapshot.head_oid || snapshot.pipeline_status != "success" {
        return Err(AutoSquashError::Rejected(
            "GitLab CI has not passed for the current merge request head".to_string(),
        ));
    }
    Ok(())
}

fn gitlab_merge_args(
    reference: &ProviderRef,
    head_oid: &str,
) -> Result<Vec<String>, AutoSquashError> {
    let mut args = gitlab_api_args(reference, "/merge")?;
    args.extend([
        "--method".to_string(),
        "PUT".to_string(),
        "--field".to_string(),
        format!("sha={head_oid}"),
        "--field".to_string(),
        "squash=true".to_string(),
        "--field".to_string(),
        "should_remove_source_branch=false".to_string(),
        "--field".to_string(),
        "auto_merge=false".to_string(),
    ]);
    Ok(args)
}

async fn github_merge_readiness(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
    expected_head: &str,
) -> Result<Option<AutoSquashOutcome>, AutoSquashError> {
    let (state, remote_head) = github_identity(reference, request).await?;
    if remote_head != expected_head {
        return Err(AutoSquashError::Rejected(
            "local HEAD does not match the GitHub pull request head".to_string(),
        ));
    }
    if state == "MERGED" {
        return Ok(Some(AutoSquashOutcome::AlreadyMerged));
    }
    if state != "OPEN" {
        return Err(AutoSquashError::Rejected(
            "GitHub pull request is not open".to_string(),
        ));
    }
    let (snapshot, check_names) = inspect_github(reference, request).await?;
    if snapshot.metadata.head_oid != expected_head {
        return Err(AutoSquashError::Rejected(
            "GitHub pull request head changed while status was being read".to_string(),
        ));
    }
    if snapshot.metadata.state == "MERGED" {
        return Ok(Some(AutoSquashOutcome::AlreadyMerged));
    }
    if snapshot.metadata.state != "OPEN" {
        return Err(AutoSquashError::Rejected(
            "GitHub pull request is not open".to_string(),
        ));
    }
    let rules = github_branch_rules(reference, &snapshot.metadata.base_branch, request).await?;
    let reviews_required = validate_github_rules(&rules, &check_names)?;
    match snapshot.metadata.review_decision.as_deref() {
        Some("APPROVED") => {}
        Some(_) => {
            return Err(AutoSquashError::Rejected(
                "GitHub review requirements are not confirmed fulfilled".to_string(),
            ));
        }
        None if reviews_required => {
            return Err(AutoSquashError::Rejected(
                "GitHub required reviews are not confirmed fulfilled".to_string(),
            ));
        }
        None => {}
    }
    Ok(None)
}

async fn perform_github(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
    expected_head: &str,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    if let Some(outcome) = github_merge_readiness(reference, request, expected_head).await? {
        return Ok(outcome);
    }
    check_cancel(request.cancel_token)?;
    if local_head(request).await? != expected_head {
        return Err(AutoSquashError::Rejected(
            "local HEAD changed before the merge request".to_string(),
        ));
    }
    check_cancel(request.cancel_token)?;

    let args = github_merge_args(reference, expected_head)?;
    let merge_result = run_process(
        "gh",
        &args,
        request.working_dir,
        request.env,
        request.cancel_token,
    )
    .await;
    let (merge_succeeded, merge_uncertain) = match merge_result {
        Ok(output) => (successful(&output), false),
        Err(ProcessError::CancelledBeforeStart) => return Err(AutoSquashError::Cancelled),
        Err(ProcessError::Cancelled) => {
            return Err(AutoSquashError::Unconfirmed(
                "cancellation occurred after the merge request was sent".to_string(),
            ));
        }
        Err(ProcessError::Failed | ProcessError::TimedOut) => (false, true),
    };
    confirm_github(
        reference,
        request,
        expected_head,
        merge_succeeded,
        merge_uncertain,
    )
    .await
}

async fn confirm_github(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
    expected_head: &str,
    merge_command_succeeded: bool,
    merge_command_uncertain: bool,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    let (state, head_oid) = github_identity(reference, request).await.map_err(|_| {
        AutoSquashError::Unconfirmed("GitHub did not provide a confirmation status".to_string())
    })?;
    if head_oid != expected_head {
        return Err(AutoSquashError::Unconfirmed(
            "GitHub pull request head changed before merge confirmation".to_string(),
        ));
    }
    match state.as_str() {
        "MERGED" => Ok(AutoSquashOutcome::Merged),
        "CLOSED" => Err(AutoSquashError::Rejected(
            "GitHub pull request closed without a confirmed merge".to_string(),
        )),
        "OPEN" if merge_command_uncertain => Err(AutoSquashError::Unconfirmed(
            "GitHub could not confirm the merge request outcome".to_string(),
        )),
        "OPEN" if merge_command_succeeded => Err(AutoSquashError::Unconfirmed(
            "GitHub has not confirmed that the pull request merged".to_string(),
        )),
        "OPEN" => Err(AutoSquashError::CommandFailed(
            "GitHub rejected the squash merge request".to_string(),
        )),
        _ => Err(AutoSquashError::Unconfirmed(
            "GitHub returned an unknown pull request state after the merge request".to_string(),
        )),
    }
}
async fn perform_gitlab(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
    expected_head: &str,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    let (state, remote_head) = gitlab_identity(reference, request).await?;
    if remote_head != expected_head {
        return Err(AutoSquashError::Rejected(
            "local HEAD does not match the GitLab merge request head".to_string(),
        ));
    }
    if state == "merged" {
        return Ok(AutoSquashOutcome::AlreadyMerged);
    }
    if state != "opened" {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request is not open".to_string(),
        ));
    }
    let initial = gitlab_api(reference, "", request).await?;
    let snapshot = parse_gitlab_snapshot(&initial)?;
    if snapshot.head_oid != expected_head {
        return Err(AutoSquashError::Rejected(
            "GitLab merge request head changed while status was being read".to_string(),
        ));
    }
    if snapshot.state == "merged" {
        return Ok(AutoSquashOutcome::AlreadyMerged);
    }
    validate_gitlab_open(&snapshot, expected_head)?;
    let project = gitlab_project(reference, request).await?;
    validate_gitlab_squash_policy(&project)?;
    check_cancel(request.cancel_token)?;
    if local_head(request).await? != expected_head {
        return Err(AutoSquashError::Rejected(
            "local HEAD changed before the merge request".to_string(),
        ));
    }
    check_cancel(request.cancel_token)?;

    let args = gitlab_merge_args(reference, expected_head)?;
    let merge_result = run_process(
        "glab",
        &args,
        request.working_dir,
        request.env,
        request.cancel_token,
    )
    .await;
    let (merge_succeeded, merge_uncertain) = match merge_result {
        Ok(output) => (successful(&output), false),
        Err(ProcessError::CancelledBeforeStart) => return Err(AutoSquashError::Cancelled),
        Err(ProcessError::Cancelled) => {
            return Err(AutoSquashError::Unconfirmed(
                "cancellation occurred after the merge request was sent".to_string(),
            ));
        }
        Err(ProcessError::Failed | ProcessError::TimedOut) => (false, true),
    };
    confirm_gitlab(
        reference,
        request,
        expected_head,
        merge_succeeded,
        merge_uncertain,
    )
    .await
}
async fn confirm_gitlab(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
    expected_head: &str,
    merge_command_succeeded: bool,
    merge_command_uncertain: bool,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    let (state, head_oid) = gitlab_identity(reference, request).await.map_err(|_| {
        AutoSquashError::Unconfirmed("GitLab did not provide a confirmation status".to_string())
    })?;
    if head_oid != expected_head {
        return Err(AutoSquashError::Unconfirmed(
            "GitLab merge request head changed before merge confirmation".to_string(),
        ));
    }
    match state.as_str() {
        "merged" => Ok(AutoSquashOutcome::Merged),
        "closed" => Err(AutoSquashError::Rejected(
            "GitLab merge request closed without a confirmed merge".to_string(),
        )),
        "opened" if merge_command_uncertain => Err(AutoSquashError::Unconfirmed(
            "GitLab could not confirm the merge request outcome".to_string(),
        )),
        "opened" if merge_command_succeeded => Err(AutoSquashError::Unconfirmed(
            "GitLab has not confirmed that the merge request merged".to_string(),
        )),
        "opened" => Err(AutoSquashError::CommandFailed(
            "GitLab rejected the squash merge request".to_string(),
        )),
        _ => Err(AutoSquashError::Unconfirmed(
            "GitLab returned an unknown merge request state after the merge request".to_string(),
        )),
    }
}
#[cfg(all(test, unix))]
mod tests;

/// Synchronously squash-merges an open GitHub pull request or GitLab merge request.
///
/// The operation does not alter local files or perform session cleanup. Provider errors are
/// deliberately sanitized: raw command output and environment values are never returned.
pub(crate) async fn squash_merge(
    request: AutoSquashRequest<'_>,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    check_cancel(request.cancel_token)?;
    let expected_head = local_head(&request).await?;
    let reference = parse_provider_ref(request.pr_url)?;
    match &reference {
        ProviderRef::GitHub { .. } => perform_github(&reference, &request, &expected_head).await,
        ProviderRef::GitLab { .. } => perform_gitlab(&reference, &request, &expected_head).await,
    }
}
