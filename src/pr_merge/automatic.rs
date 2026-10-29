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
const GITHUB_READINESS_TIMEOUT: Duration = Duration::from_mins(10);
const GITHUB_READINESS_POLL_INTERVAL: Duration = Duration::from_secs(2);
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
    // Prevent read-only Git status checks from refreshing the index during a concurrent commit.
    if program == "git" && args.first().is_some_and(|arg| arg == "status") {
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }

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
    check_state: Option<String>,
    check_commit_oid: Option<String>,
    context_total: usize,
    contexts_registered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitHubCheckStatus {
    Green,
    Pending,
    Failed,
}

impl GitHubCheckStatus {
    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Failed, _) | (_, Self::Failed) => Self::Failed,
            (Self::Pending, _) | (_, Self::Pending) => Self::Pending,
            _ => Self::Green,
        }
    }
}

struct GitHubCiPage {
    check_state: Option<String>,
    check_commit_oid: Option<String>,
    context_total: usize,
    check_status: GitHubCheckStatus,
    check_names: Vec<String>,
    contexts_registered: bool,
    has_next_page: bool,
    end_cursor: Option<String>,
}

struct GitHubSnapshotPage {
    snapshot: GitHubSnapshot,
    check_names: Vec<String>,
    check_status: GitHubCheckStatus,
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitHubReadiness {
    Ready,
    AlreadyMerged,
    Pending,
}

#[derive(Debug, Clone, Copy)]
enum GitHubMergeRefusalReason {
    MergeNotPerformed,
    HeadChanged,
    BaseChanged,
    SquashDisabled,
    MergeInProgress,
    Unclassified,
}

impl GitHubMergeRefusalReason {
    fn from_message(message: &str) -> Self {
        if message.chars().count() > 512 {
            return Self::Unclassified;
        }
        let normalized = message.lines().map(str::trim).collect::<Vec<_>>().join(" ");
        let normalized = normalized.trim();
        match normalized {
            "Pull Request is not mergeable" => Self::MergeNotPerformed,
            "Head branch was modified. Review and try the merge again." => Self::HeadChanged,
            "Base branch was modified. Review and try the merge again." => Self::BaseChanged,
            "Squash merges are not allowed on this repository." => Self::SquashDisabled,
            "Merge already in progress" => Self::MergeInProgress,
            _ => Self::Unclassified,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::MergeNotPerformed => "merge_not_performed",
            Self::HeadChanged => "head_changed",
            Self::BaseChanged => "base_changed",
            Self::SquashDisabled => "squash_disabled",
            Self::MergeInProgress => "merge_in_progress",
            Self::Unclassified => "unclassified",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::MergeNotPerformed => "Pull request is not mergeable.",
            Self::HeadChanged => "Head branch changed before the merge could be completed.",
            Self::BaseChanged => "Base branch changed before the merge could be completed.",
            Self::SquashDisabled => "Squash merges are disabled for this repository.",
            Self::MergeInProgress => "A merge is already in progress.",
            Self::Unclassified => "The refusal reason could not be classified.",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct GitHubMergeDiagnostic {
    http_status: Option<u16>,
    exit_code: Option<i32>,
    reason: GitHubMergeRefusalReason,
}

impl GitHubMergeDiagnostic {
    fn from_output(output: &Output) -> Self {
        let mut diagnostic = Self {
            http_status: None,
            exit_code: output.status.code(),
            reason: GitHubMergeRefusalReason::Unclassified,
        };
        let bytes = &output.stdout;
        // Status can still be useful when the remainder is oversized or invalid UTF-8.
        if let Some(line) = bytes[..bytes.len().min(64 * 1024)]
            .split(|byte| *byte == b'\n')
            .next()
            && let Ok(line) = std::str::from_utf8(line)
        {
            let mut parts = line.split_whitespace();
            if matches!(parts.next(), Some("HTTP/1.1" | "HTTP/2" | "HTTP/2.0"))
                && let Some(status) = parts.next()
                && status.len() == 3
                && status.bytes().all(|byte| byte.is_ascii_digit())
            {
                diagnostic.http_status = status.parse().ok();
            }
        }
        if bytes.len() > 64 * 1024 {
            return diagnostic;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            return diagnostic;
        };
        let body = if diagnostic.http_status.is_some() {
            text.split_once("\r\n\r\n")
                .or_else(|| text.split_once("\n\n"))
                .map(|(_, body)| body)
        } else {
            Some(text)
        };
        if let Some(body) = body
            && let Ok(value) = serde_json::from_str::<Value>(body)
            && let Some(message) = value.get("message").and_then(Value::as_str)
        {
            diagnostic.reason = GitHubMergeRefusalReason::from_message(message);
        }
        diagnostic
    }

    fn message(self) -> String {
        let mut fields = format!("reason={}", self.reason.code());
        if let Some(status) = self.http_status {
            fields.push_str(&format!(", HTTP={status}"));
        }
        if let Some(code) = self.exit_code {
            fields.push_str(&format!(", exit={code}"));
        }
        format!(
            "GitHub rejected the squash merge request ({fields}): {}",
            self.reason.description()
        )
    }
}

struct GitHubReadinessObservation {
    telemetry_context: Option<opentelemetry::Context>,
    mergeable: Option<String>,
    merge_state: Option<String>,
    check_state: Option<String>,
    check_commit_oid: Option<String>,
    ci_head_matches: bool,
    reason: Option<&'static str>,
    merge_diagnostic: Option<GitHubMergeDiagnostic>,
}

impl GitHubReadinessObservation {
    fn new() -> Self {
        Self {
            telemetry_context: crate::telemetry::capture_github_context(),
            mergeable: None,
            merge_state: None,
            check_state: None,
            check_commit_oid: None,
            ci_head_matches: false,
            reason: None,
            merge_diagnostic: None,
        }
    }

    fn timeout_message(&self) -> String {
        format!(
            "GitHub readiness timed out (mergeable={}, mergeStateStatus={}, CI state={}, CI head={}, waiting for={})",
            self.mergeable.as_deref().unwrap_or("unobserved"),
            self.merge_state.as_deref().unwrap_or("unobserved"),
            self.check_state.as_deref().unwrap_or("unobserved"),
            self.check_commit_oid.as_deref().unwrap_or("unobserved"),
            self.reason.unwrap_or("readiness"),
        )
    }
}

impl Drop for GitHubReadinessObservation {
    fn drop(&mut self) {
        let Some(context) = &self.telemetry_context else {
            return;
        };
        crate::telemetry::record_github_readiness(
            context,
            self.mergeable.as_deref().zip(self.merge_state.as_deref()),
            self.reason.unwrap_or("none"),
            self.check_state.as_deref().unwrap_or("unobserved"),
            self.ci_head_matches,
        );
        if let Some(diagnostic) = self.merge_diagnostic {
            crate::telemetry::record_github_merge_refusal(
                context,
                diagnostic.reason.code(),
                diagnostic.http_status,
                diagnostic.exit_code,
            );
        }
    }
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
    observation: &mut GitHubReadinessObservation,
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
    if observation.telemetry_context.is_some()
        && (observation.mergeable.as_deref() != Some(metadata.mergeable.as_str())
            || observation.merge_state.as_deref() != Some(metadata.merge_state.as_str()))
    {
        observation.mergeable = Some(metadata.mergeable.clone());
        observation.merge_state = Some(metadata.merge_state.clone());
    }
    observation.reason = None;
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
        if !squash_allowed {
            return Err(AutoSquashError::Rejected(
                "GitHub repository policy does not allow squash merging".to_string(),
            ));
        }
    }
    Ok(metadata)
}

fn parse_github_rollup_state(state: &str) -> Result<GitHubCheckStatus, AutoSquashError> {
    match state {
        "SUCCESS" => Ok(GitHubCheckStatus::Green),
        "PENDING" | "EXPECTED" => Ok(GitHubCheckStatus::Pending),
        "ERROR" | "FAILURE" => Ok(GitHubCheckStatus::Failed),
        _ => Err(AutoSquashError::Rejected(
            "GitHub returned an unknown CI rollup state".to_string(),
        )),
    }
}

fn parse_github_ci_commit_oid(rollup: &Value) -> Result<Option<String>, AutoSquashError> {
    match rollup.get("commit") {
        Some(Value::Null) => Ok(None),
        Some(commit) if commit.is_object() => match commit.get("oid") {
            Some(Value::Null) => Ok(None),
            Some(oid) => {
                let oid = oid.as_str().filter(|oid| !oid.is_empty()).ok_or_else(|| {
                    AutoSquashError::Rejected("GitHub CI head is malformed".into())
                })?;
                if !is_object_id(oid) {
                    return Err(AutoSquashError::Rejected(
                        "GitHub CI head is malformed".into(),
                    ));
                }
                Ok(Some(oid.to_string()))
            }
            None => Err(AutoSquashError::Rejected(
                "GitHub CI head is unavailable".into(),
            )),
        },
        _ => Err(AutoSquashError::Rejected(
            "GitHub CI head is unavailable".into(),
        )),
    }
}

fn unregistered_github_ci_page(
    check_state: Option<String>,
    check_commit_oid: Option<String>,
    check_status: GitHubCheckStatus,
) -> GitHubCiPage {
    GitHubCiPage {
        check_state,
        check_commit_oid,
        context_total: 0,
        check_status,
        check_names: Vec::new(),
        contexts_registered: false,
        has_next_page: false,
        end_cursor: None,
    }
}

fn parse_github_ci_check_nodes(
    nodes: &[Value],
) -> Result<(Vec<String>, GitHubCheckStatus), AutoSquashError> {
    let mut check_names = Vec::with_capacity(nodes.len());
    let mut check_status = GitHubCheckStatus::Green;
    for node in nodes {
        let status = match required_str(node, "__typename")? {
            "CheckRun" => {
                let status = required_str(node, "status")?;
                let conclusion = node.get("conclusion").ok_or_else(|| {
                    AutoSquashError::Rejected("GitHub CI conclusion is unavailable".into())
                })?;
                let conclusion = if conclusion.is_null() {
                    None
                } else {
                    let conclusion = conclusion.as_str().ok_or_else(|| {
                        AutoSquashError::Rejected("GitHub CI conclusion is malformed".into())
                    })?;
                    if !matches!(
                        conclusion,
                        "ACTION_REQUIRED"
                            | "CANCELLED"
                            | "FAILURE"
                            | "NEUTRAL"
                            | "SKIPPED"
                            | "STALE"
                            | "STARTUP_FAILURE"
                            | "SUCCESS"
                            | "TIMED_OUT"
                    ) {
                        return Err(AutoSquashError::Rejected(
                            "GitHub returned an unknown CI conclusion".to_string(),
                        ));
                    }
                    Some(conclusion)
                };
                let name = required_str(node, "name")?;
                let status = match status {
                    "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" | "PENDING" => {
                        GitHubCheckStatus::Pending
                    }
                    "COMPLETED" => match conclusion {
                        Some("SUCCESS" | "NEUTRAL" | "SKIPPED") => GitHubCheckStatus::Green,
                        Some(_) => GitHubCheckStatus::Failed,
                        None => {
                            return Err(AutoSquashError::Rejected(
                                "GitHub CI conclusion is unavailable".into(),
                            ));
                        }
                    },
                    _ => {
                        return Err(AutoSquashError::Rejected(
                            "GitHub returned an unknown CI check status".to_string(),
                        ));
                    }
                };
                check_names.push(name.to_string());
                status
            }
            "StatusContext" => {
                let state = required_str(node, "state")?;
                let name = required_str(node, "context")?;
                let status = match state {
                    "SUCCESS" => GitHubCheckStatus::Green,
                    "EXPECTED" | "PENDING" => GitHubCheckStatus::Pending,
                    "ERROR" | "FAILURE" => GitHubCheckStatus::Failed,
                    _ => {
                        return Err(AutoSquashError::Rejected(
                            "GitHub returned an unknown CI status".to_string(),
                        ));
                    }
                };
                check_names.push(name.to_string());
                status
            }
            _ => {
                return Err(AutoSquashError::Rejected(
                    "GitHub returned an unknown CI check type".to_string(),
                ));
            }
        };
        check_status = check_status.combine(status);
    }
    Ok((check_names, check_status))
}

fn parse_github_ci_context_page(
    contexts: &Value,
    check_state: String,
    check_commit_oid: Option<String>,
    rollup_status: GitHubCheckStatus,
) -> Result<GitHubCiPage, AutoSquashError> {
    let contexts = contexts
        .as_object()
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI checks are malformed".to_string()))?;
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
    if context_total > 0 && check_commit_oid.is_none() {
        return Err(AutoSquashError::Rejected(
            "GitHub CI head is unavailable".into(),
        ));
    }
    let (check_names, node_status) = parse_github_ci_check_nodes(nodes)?;
    Ok(GitHubCiPage {
        check_state: Some(check_state),
        check_commit_oid,
        context_total,
        check_status: node_status.combine(rollup_status),
        check_names,
        contexts_registered: true,
        has_next_page,
        end_cursor,
    })
}

fn parse_github_ci_page(pr: &Value) -> Result<GitHubCiPage, AutoSquashError> {
    let rollup = pr
        .get("statusCheckRollup")
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI status is unavailable".into()))?;
    if rollup.is_null() {
        return Ok(unregistered_github_ci_page(
            None,
            None,
            GitHubCheckStatus::Green,
        ));
    }
    if !rollup.is_object() {
        return Err(AutoSquashError::Rejected(
            "GitHub CI status is malformed".to_string(),
        ));
    }
    let check_state = required_str(rollup, "state")?.to_string();
    let rollup_status = parse_github_rollup_state(&check_state)?;
    let check_commit_oid = parse_github_ci_commit_oid(rollup)?;
    let contexts = rollup
        .get("contexts")
        .ok_or_else(|| AutoSquashError::Rejected("GitHub CI checks are unavailable".into()))?;
    if contexts.is_null() {
        return Ok(unregistered_github_ci_page(
            Some(check_state),
            check_commit_oid,
            rollup_status,
        ));
    }
    parse_github_ci_context_page(contexts, check_state, check_commit_oid, rollup_status)
}

fn parse_github_snapshot(
    value: &Value,
    observation: &mut GitHubReadinessObservation,
) -> Result<GitHubSnapshotPage, AutoSquashError> {
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
    let metadata = parse_github_metadata(pr, repository, observation)?;
    let ci = parse_github_ci_page(pr)?;
    Ok(GitHubSnapshotPage {
        snapshot: GitHubSnapshot {
            metadata,
            check_state: ci.check_state,
            check_commit_oid: ci.check_commit_oid,
            context_total: ci.context_total,
            contexts_registered: ci.contexts_registered,
        },
        check_names: ci.check_names,
        check_status: ci.check_status,
        has_next_page: ci.has_next_page,
        end_cursor: ci.end_cursor,
    })
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
    expected_head: &str,
    observation: &mut GitHubReadinessObservation,
) -> Result<(GitHubSnapshot, Vec<String>, GitHubCheckStatus), AutoSquashError> {
    let mut cursor = None;
    let mut snapshot: Option<GitHubSnapshot> = None;
    let mut names = Vec::new();
    let mut node_count = 0usize;
    let mut check_status = GitHubCheckStatus::Green;
    loop {
        let value =
            github_graphql(reference, GITHUB_STATUS_QUERY, cursor.as_deref(), request).await?;
        let GitHubSnapshotPage {
            snapshot: current,
            check_names: page_names,
            check_status: page_status,
            has_next_page: has_next,
            end_cursor,
        } = parse_github_snapshot(&value, observation)?;
        if snapshot.is_none() {
            if observation.telemetry_context.is_none() {
                observation.mergeable = Some(current.metadata.mergeable.clone());
                observation.merge_state = Some(current.metadata.merge_state.clone());
            }
            observation.check_state = Some(
                current
                    .check_state
                    .clone()
                    .unwrap_or_else(|| "unregistered".to_string()),
            );
            observation.check_commit_oid = Some(
                current
                    .check_commit_oid
                    .clone()
                    .unwrap_or_else(|| "unregistered".to_string()),
            );
            observation.ci_head_matches =
                current.check_commit_oid.as_deref() == Some(expected_head);
        }
        if current.metadata.head_oid != expected_head {
            return Err(AutoSquashError::Rejected(
                "GitHub pull request head changed while status was being read".to_string(),
            ));
        }
        if let Some(first) = &snapshot {
            if first != &current {
                return Err(AutoSquashError::Rejected(
                    "GitHub pull request changed while status was being read".to_string(),
                ));
            }
        } else {
            snapshot = Some(current);
        }
        node_count = node_count.saturating_add(page_names.len());
        names.extend(page_names);
        check_status = check_status.combine(page_status);
        if !has_next {
            break;
        }
        cursor = end_cursor;
    }
    let snapshot = snapshot.ok_or_else(|| {
        AutoSquashError::Rejected("GitHub pull request data is unavailable".into())
    })?;
    if snapshot.contexts_registered && node_count != snapshot.context_total {
        return Err(AutoSquashError::Rejected(
            "GitHub CI checks are missing or incomplete".to_string(),
        ));
    }
    Ok((snapshot, names, check_status))
}

/// Active GitHub branch rules, or the one known plan restriction that prevents listing them.
enum GitHubBranchRules {
    Available(Value),
    UnavailableOnPlan,
}

#[derive(serde::Deserialize)]
struct GitHubBranchRulesApiError<'a> {
    #[serde(borrow)]
    message: &'a str,
    #[serde(borrow)]
    status: &'a str,
}

fn github_branch_rules_unavailable_on_plan(stdout: &[u8]) -> bool {
    // gh --paginate --slurp wraps the failed page body; also accept a bare body.
    let error =
        if let Ok([error]) = serde_json::from_slice::<[GitHubBranchRulesApiError<'_>; 1]>(stdout) {
            error
        } else {
            let Ok(error) = serde_json::from_slice::<GitHubBranchRulesApiError<'_>>(stdout) else {
                return false;
            };
            error
        };
    error.status == "403"
        && error.message
            == "Upgrade to GitHub Pro or make this repository public to enable this feature."
}

async fn github_branch_rules(
    reference: &ProviderRef,
    branch: &str,
    request: &AutoSquashRequest<'_>,
) -> Result<GitHubBranchRules, AutoSquashError> {
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
        if github_branch_rules_unavailable_on_plan(&output.stdout) {
            return Ok(GitHubBranchRules::UnavailableOnPlan);
        }
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
    Ok(GitHubBranchRules::Available(rules))
}

fn validate_github_required_status_checks(
    rule: &Value,
    checked: Option<&HashSet<&str>>,
) -> Result<(), AutoSquashError> {
    let required = rule
        .pointer("/parameters/required_status_checks")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AutoSquashError::Rejected("GitHub required status checks are unreadable".into())
        })?;
    for check in required {
        let context = required_str(check, "context")?;
        if checked.is_some_and(|checked| !checked.contains(context)) {
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

fn validate_github_rules(
    rules: &Value,
    check_names: Option<&[String]>,
) -> Result<bool, AutoSquashError> {
    let checked: Option<HashSet<&str>> =
        check_names.map(|names| names.iter().map(String::as_str).collect());
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
                    validate_github_required_status_checks(rule, checked.as_ref())?;
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
        "--include".to_string(),
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
    observation: &mut GitHubReadinessObservation,
) -> Result<GitHubReadiness, AutoSquashError> {
    let (state, remote_head) = github_identity(reference, request).await?;
    if remote_head != expected_head {
        observation.reason = None;
        return Err(AutoSquashError::Rejected(
            "local HEAD does not match the GitHub pull request head".to_string(),
        ));
    }
    if state == "MERGED" {
        observation.reason = None;
        return Ok(GitHubReadiness::AlreadyMerged);
    }
    if state != "OPEN" {
        observation.reason = None;
        return Err(AutoSquashError::Rejected(
            "GitHub pull request is not open".to_string(),
        ));
    }
    let (snapshot, check_names, check_status) =
        inspect_github(reference, request, expected_head, observation).await?;
    if snapshot.metadata.state == "MERGED" {
        return Ok(GitHubReadiness::AlreadyMerged);
    }
    if snapshot.metadata.state != "OPEN" {
        return Err(AutoSquashError::Rejected(
            "GitHub pull request is not open".to_string(),
        ));
    }
    let current_head_rollup =
        snapshot.contexts_registered && snapshot.check_commit_oid.as_deref() == Some(expected_head);
    let current_head_ci = current_head_rollup && snapshot.context_total > 0;
    if current_head_rollup && check_status == GitHubCheckStatus::Failed {
        return Err(AutoSquashError::Rejected(
            "GitHub CI failed for the current pull request head".to_string(),
        ));
    }
    let mergeability_pending = match (
        snapshot.metadata.mergeable.as_str(),
        snapshot.metadata.merge_state.as_str(),
    ) {
        ("MERGEABLE", "CLEAN") => false,
        ("MERGEABLE", "UNSTABLE")
            if !current_head_ci || check_status == GitHubCheckStatus::Pending =>
        {
            true
        }
        ("UNKNOWN", "CLEAN" | "UNKNOWN") => true,
        _ => {
            return Err(AutoSquashError::Rejected(format!(
                "GitHub pull request is not confirmed mergeable (mergeable={}, mergeStateStatus={})",
                snapshot.metadata.mergeable, snapshot.metadata.merge_state
            )));
        }
    };
    let (readiness, reason) = if !current_head_ci {
        (
            GitHubReadiness::Pending,
            Some(if snapshot.context_total > 0 {
                "current-head CI registration"
            } else {
                "CI registration"
            }),
        )
    } else if check_status == GitHubCheckStatus::Pending {
        (GitHubReadiness::Pending, Some("current-head CI checks"))
    } else if mergeability_pending {
        (GitHubReadiness::Pending, Some("GitHub mergeability"))
    } else {
        (GitHubReadiness::Ready, None)
    };
    observation.reason = reason;

    let branch_rules =
        github_branch_rules(reference, &snapshot.metadata.base_branch, request).await?;
    let reviews_required = match branch_rules {
        GitHubBranchRules::Available(rules) => {
            validate_github_rules(&rules, current_head_ci.then_some(check_names.as_slice()))?
        }
        // The synchronous merge endpoint remains authoritative for rules this plan cannot list.
        GitHubBranchRules::UnavailableOnPlan => false,
    };
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
    Ok(readiness)
}

async fn perform_github(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
    expected_head: &str,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    perform_github_with_timing(
        reference,
        request,
        expected_head,
        GITHUB_READINESS_TIMEOUT,
        GITHUB_READINESS_POLL_INTERVAL,
    )
    .await
}

async fn perform_github_with_timing(
    reference: &ProviderRef,
    request: &AutoSquashRequest<'_>,
    expected_head: &str,
    readiness_timeout: Duration,
    poll_interval: Duration,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    let deadline = tokio::time::Instant::now() + readiness_timeout;
    let mut observation = GitHubReadinessObservation::new();
    loop {
        check_cancel(request.cancel_token)?;
        let attempt = tokio::select! {
            biased;
            () = wait_for_cancel(request.cancel_token) => return Err(AutoSquashError::Cancelled),
            attempt = tokio::time::timeout_at(deadline, async {
                if local_head(request).await? != expected_head {
                    return Err(AutoSquashError::Rejected(
                        "local HEAD changed before the merge request".to_string(),
                    ));
                }
                github_merge_readiness(reference, request, expected_head, &mut observation).await
            }) => attempt,
        };
        let readiness = match attempt {
            Ok(readiness) => readiness?,
            Err(_) => return Err(AutoSquashError::Rejected(observation.timeout_message())),
        };
        match readiness {
            GitHubReadiness::Ready => break,
            GitHubReadiness::AlreadyMerged => return Ok(AutoSquashOutcome::AlreadyMerged),
            GitHubReadiness::Pending => {
                let pending_head = tokio::select! {
                    biased;
                    () = wait_for_cancel(request.cancel_token) => return Err(AutoSquashError::Cancelled),
                    result = tokio::time::timeout_at(deadline, local_head(request)) => match result {
                        Ok(result) => result?,
                        Err(_) => return Err(AutoSquashError::Rejected(observation.timeout_message())),
                    },
                };
                if pending_head != expected_head {
                    return Err(AutoSquashError::Rejected(
                        "local HEAD changed while GitHub readiness was pending".to_string(),
                    ));
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(AutoSquashError::Rejected(observation.timeout_message()));
                }
                let wait = poll_interval.min(remaining);
                tokio::select! {
                    biased;
                    () = wait_for_cancel(request.cancel_token) => return Err(AutoSquashError::Cancelled),
                    () = tokio::time::sleep(wait) => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(AutoSquashError::Rejected(observation.timeout_message()));
                }
            }
        }
    }
    check_cancel(request.cancel_token)?;
    let final_head = tokio::select! {
        biased;
        () = wait_for_cancel(request.cancel_token) => return Err(AutoSquashError::Cancelled),
        result = tokio::time::timeout_at(deadline, local_head(request)) => match result {
            Ok(result) => result?,
            Err(_) => return Err(AutoSquashError::Rejected(observation.timeout_message())),
        },
    };
    if final_head != expected_head {
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
    let (merge_succeeded, merge_uncertain, diagnostic) = match merge_result {
        Ok(output) => {
            let succeeded = successful(&output);
            let diagnostic = (!succeeded).then(|| GitHubMergeDiagnostic::from_output(&output));
            (succeeded, false, diagnostic)
        }
        Err(ProcessError::CancelledBeforeStart) => return Err(AutoSquashError::Cancelled),
        Err(ProcessError::Cancelled) => {
            return Err(AutoSquashError::Unconfirmed(
                "cancellation occurred after the merge request was sent".to_string(),
            ));
        }
        Err(ProcessError::Failed | ProcessError::TimedOut) => (false, true, None),
    };
    let result = confirm_github(
        reference,
        request,
        expected_head,
        merge_succeeded,
        merge_uncertain,
    )
    .await;
    match result {
        Err(AutoSquashError::CommandFailed(_)) if diagnostic.is_some() => {
            let diagnostic = diagnostic.expect("diagnostic was checked");
            observation.merge_diagnostic = Some(diagnostic);
            Err(AutoSquashError::CommandFailed(diagnostic.message()))
        }
        result => result,
    }
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
