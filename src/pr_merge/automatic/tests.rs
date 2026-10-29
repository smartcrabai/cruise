use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::{AutoSquashError, AutoSquashOutcome, AutoSquashRequest, squash_merge};

struct Fixture {
    temp: TempDir,
    repo: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    effects: PathBuf,
    merged: PathBuf,
    gh_identity: PathBuf,
    gh_identity_after: PathBuf,
    gh_status: PathBuf,
    gh_status_after: PathBuf,
    gh_status_sequence: PathBuf,
    gh_status_calls: PathBuf,
    gh_rules: PathBuf,
    gl_mr: PathBuf,
    gl_mr_after: PathBuf,
    gl_mr_seen: PathBuf,
    gl_project: PathBuf,
    env: HashMap<String, String>,
    head: String,
}

impl Fixture {
    fn new() -> Self {
        // Setup spawns real git and snapshots PATH for later child commands.
        // Serialize that work with tests which temporarily change process PATH.
        let _process_lock = crate::test_support::lock_process();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let repo = temp.path().join("repo");
        let head = initialize_test_repository(&repo);
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("{error}"));
        let log = temp.path().join("provider.log");
        let effects = temp.path().join("effects.log");
        let merged = temp.path().join("merged");
        let gh_identity = temp.path().join("gh_identity.json");
        let gh_identity_after = temp.path().join("gh_identity_after.json");
        let gh_status = temp.path().join("gh_status.json");
        let gh_status_after = temp.path().join("gh_status_after.json");
        let gh_status_sequence = temp.path().join("gh_status_sequence.jsonl");
        let gh_status_calls = temp.path().join("gh_status_calls");
        let gh_rules = temp.path().join("gh_rules.json");
        let gl_mr = temp.path().join("gl_mr.json");
        let gl_mr_after = temp.path().join("gl_mr_after.json");
        let gl_mr_seen = temp.path().join("gl_mr_seen");
        let gl_project = temp.path().join("gl_project.json");
        let path = std::env::var("PATH").unwrap_or_default();
        let mut env = HashMap::from([
            ("PATH".to_string(), format!("{}:{path}", bin.display())),
            ("FAKE_LOG".to_string(), log.display().to_string()),
            ("FAKE_EFFECTS".to_string(), effects.display().to_string()),
            ("FAKE_MERGED".to_string(), merged.display().to_string()),
            ("FAKE_EXPECTED_HEAD".to_string(), head.clone()),
            (
                "FAKE_GH_IDENTITY".to_string(),
                gh_identity.display().to_string(),
            ),
            (
                "FAKE_GH_IDENTITY_AFTER".to_string(),
                gh_identity_after.display().to_string(),
            ),
            (
                "FAKE_GH_STATUS".to_string(),
                gh_status.display().to_string(),
            ),
            (
                "FAKE_GH_STATUS_AFTER".to_string(),
                gh_status_after.display().to_string(),
            ),
            (
                "FAKE_GH_STATUS_SEQUENCE".to_string(),
                gh_status_sequence.display().to_string(),
            ),
            (
                "FAKE_GH_STATUS_CALLS".to_string(),
                gh_status_calls.display().to_string(),
            ),
            ("FAKE_GH_RULES".to_string(), gh_rules.display().to_string()),
            ("FAKE_GL_MR".to_string(), gl_mr.display().to_string()),
            (
                "FAKE_GL_MR_AFTER".to_string(),
                gl_mr_after.display().to_string(),
            ),
            (
                "FAKE_GL_MR_SEEN".to_string(),
                gl_mr_seen.display().to_string(),
            ),
            (
                "FAKE_GL_PROJECT".to_string(),
                gl_project.display().to_string(),
            ),
        ]);
        if let Ok(inherited_path) = std::env::var("PATH") {
            env.insert(
                "PATH".to_string(),
                format!("{}:{inherited_path}", bin.display()),
            );
        }
        let fixture = Self {
            temp,
            repo,
            bin,
            log,
            effects,
            merged,
            gh_identity,
            gh_identity_after,
            gh_status,
            gh_status_after,
            gh_status_sequence,
            gh_status_calls,
            gh_rules,
            gl_mr,
            gl_mr_after,
            gl_mr_seen,
            gl_project,
            env,
            head,
        };
        fixture.install_gh();
        fixture.install_glab();
        fixture
    }

    fn install_script(&self, name: &str, body: &str) {
        let path = self.bin.join(name);
        fs::write(&path, body).unwrap_or_else(|error| panic!("{error}"));
        let mut permissions = fs::metadata(&path)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap_or_else(|error| panic!("{error}"));
    }

    fn install_gh(&self) {
        self.install_script(
            "gh",
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_LOG"
if [ "$1" = "api" ]; then
  args="$*"
  shift
  if [ "$1" = "--hostname" ]; then shift 2; fi
  if [ "$1" = "--include" ]; then shift; fi
  if [ "$1" = "--method" ]; then
    [ "$2" = "PUT" ] || exit 64
    shift 2
    if [ "$1" = "--include" ]; then shift; fi
    [ "$1" = "${FAKE_GH_MERGE_PATH:-repos/acme/service/pulls/7/merge}" ] || exit 64
    shift
    if [ "$1" = "--include" ]; then shift; fi
    [ "$1" = "-f" ] && [ "$2" = "sha=$FAKE_EXPECTED_HEAD" ] || exit 64
    shift 2
    if [ "$1" = "--include" ]; then shift; fi
    [ "$1" = "-f" ] && [ "$2" = "merge_method=squash" ] || exit 64
    if [ -n "${FAKE_GH_RESPONSE:-}" ]; then
      cat "$FAKE_GH_RESPONSE"
      [ "${FAKE_GH_CONFIRM_AFTER:-0}" = "1" ] && touch "$FAKE_MERGED"
      exit "${FAKE_GH_EXIT:-1}"
    fi
    if [ "${FAKE_GH_MERGE_STATUS:-200}" = "405" ]; then
      printf '%s\n' 'HTTP 405: merge unavailable' >&2
      exit 1
    fi
    printf '%s\n' "$args" >> "$FAKE_EFFECTS"
    touch "$FAKE_MERGED"
    exit 0
  fi
  case "$*" in
    *graphql*)
      # Model gh api -F coercion for GraphQL String! variables.
      case "$*" in
        *"-F owner=true"*|*"-F owner=false"*|*"-F owner=null"*|\
        *"-F repo=true"*|*"-F repo=false"*|*"-F repo=null"*|\
        *"-F owner="[0-9]*|*"-F repo="[0-9]*|\
        *"-F owner=-"[0-9]*|*"-F repo=-"[0-9]*)
          exit 1
          ;;
      esac
      case "$*" in
        *mergeQueueEntry*)
          if [ -f "$FAKE_MERGED" ]; then
            cat "$FAKE_GH_STATUS_AFTER"
          elif [ -f "$FAKE_GH_STATUS_SEQUENCE" ]; then
            calls=$(cat "$FAKE_GH_STATUS_CALLS" 2>/dev/null || printf '0')
            calls=$((calls + 1))
            printf '%s\n' "$calls" > "$FAKE_GH_STATUS_CALLS"
            remaining=$calls
            response=
            while IFS= read -r item || [ -n "$item" ]; do
              response=$item
              remaining=$((remaining - 1))
              [ "$remaining" -le 0 ] && break
            done < "$FAKE_GH_STATUS_SEQUENCE"
            printf '%s\n' "$response"
          else
            cat "$FAKE_GH_STATUS"
          fi
          ;;
        *)
          if [ -f "$FAKE_MERGED" ]; then cat "$FAKE_GH_IDENTITY_AFTER"; else cat "$FAKE_GH_IDENTITY"; fi
          ;;
      esac
      ;;
    *rules/branches/*)
      if [ "${FAKE_GH_RULES_DIRECT_ERROR:-}" = "1" ]; then
        cat "$FAKE_GH_RULES"
      else
        printf '['
        cat "$FAKE_GH_RULES"
        printf ']\n'
      fi
      if [ "${FAKE_GH_RULES_ERROR:-}" = "1" ]; then exit 1; fi
      ;;
    *) exit 64 ;;
  esac
else
  exit 64
fi
"#,
        );
    }
    fn install_glab(&self) {
        self.install_script(
            "glab",
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_LOG"
case "$*" in
  *"/merge --method PUT"*)
    if [ "${FAKE_GL_MERGE_REJECT:-}" = "1" ]; then
      printf '%s\n' '{"message":"Merge request is no longer mergeable"}'
      exit 1
    fi
    printf '%s\n' "$*" >> "$FAKE_EFFECTS"
    touch "$FAKE_MERGED"
    printf '{}\n'
    ;;
  *"projects/group%2Fsubgroup%2Fproject") cat "$FAKE_GL_PROJECT" ;;
  *"/merge_requests/42")
    if [ -f "$FAKE_MERGED" ]; then
      cat "$FAKE_GL_MR_AFTER"
    elif [ "${FAKE_GL_MR_SECOND_SNAPSHOT:-}" = "1" ] && [ -f "$FAKE_GL_MR_SEEN" ]; then
      cat "$FAKE_GL_MR_AFTER"
    else
      cat "$FAKE_GL_MR"
      touch "$FAKE_GL_MR_SEEN"
    fi
    ;;
esac
"#,
        );
    }

    fn request<'a>(
        &'a self,
        url: &'a str,
        cancel_token: Option<&'a crate::cancellation::CancellationToken>,
    ) -> AutoSquashRequest<'a> {
        AutoSquashRequest {
            pr_url: url,
            working_dir: &self.repo,
            env: &self.env,
            cancel_token,
        }
    }

    fn configure_github(&self, identity: &Value, status: &Value, rules: &Value) {
        write_json(&self.gh_identity, identity);
        write_json(&self.gh_status, status);
        write_json(&self.gh_rules, rules);
        write_json(
            &self.gh_identity_after,
            &github_identity("MERGED", &self.head),
        );
        write_json(&self.gh_status_after, status);
        for path in [&self.gh_status_sequence, &self.gh_status_calls] {
            if path.exists() {
                fs::remove_file(path).unwrap_or_else(|error| panic!("{error}"));
            }
        }
    }

    fn configure_github_sequence(&self, identity: &Value, statuses: &[Value], rules: &Value) {
        let initial = statuses
            .first()
            .unwrap_or_else(|| panic!("GitHub status sequence must not be empty"));
        self.configure_github(identity, initial, rules);
        let sequence = statuses
            .iter()
            .map(|status| serde_json::to_string(status).unwrap_or_else(|error| panic!("{error}")))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&self.gh_status_sequence, format!("{sequence}\n"))
            .unwrap_or_else(|error| panic!("{error}"));
    }

    fn github_status_call_count(&self) -> usize {
        fs::read_to_string(&self.gh_status_calls)
            .ok()
            .and_then(|calls| calls.trim().parse().ok())
            .unwrap_or_default()
    }

    fn configure_gitlab(&self, mr: &Value, project: &Value) {
        write_json(&self.gl_mr, mr);
        write_json(&self.gl_project, project);
        if self.gl_mr_seen.exists() {
            fs::remove_file(&self.gl_mr_seen).unwrap_or_else(|error| panic!("{error}"));
        }
        let mut merged = (*mr).clone();
        merged["state"] = json!("merged");
        write_json(&self.gl_mr_after, &merged);
    }

    fn effect_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.effects)
            .unwrap_or_default()
            .lines()
            .map(ToString::to_string)
            .collect()
    }

    fn provider_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn initialize_test_repository(repo: &Path) -> String {
    fs::create_dir_all(repo).unwrap_or_else(|error| panic!("{error}"));
    git(repo, &["init", "--quiet"]);
    git(repo, &["config", "user.email", "test@example.invalid"]);
    git(repo, &["config", "user.name", "Test User"]);
    fs::write(repo.join("file.txt"), "test\n").unwrap_or_else(|error| panic!("{error}"));
    git(repo, &["add", "file.txt"]);
    git(repo, &["commit", "--quiet", "-m", "initial"]);
    String::from_utf8(git(repo, &["rev-parse", "HEAD"]).stdout)
        .unwrap_or_else(|error| panic!("{error}"))
        .trim()
        .to_string()
}

fn git(directory: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        serde_json::to_vec(value).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

fn github_identity(state: &str, head: &str) -> Value {
    json!({ "data": { "repository": { "pullRequest": { "state": state, "headRefOid": head } } } })
}

fn github_status(head: &str) -> Value {
    json!({
        "data": { "repository": {
            "squashMergeAllowed": true,
            "pullRequest": {
            "state": "OPEN",
            "isDraft": false,
            "headRefOid": head,
            "baseRefName": "main",
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "reviewDecision": "APPROVED",
            "mergeQueueEntry": null,
            "autoMergeRequest": null,
            "statusCheckRollup": {
                "state": "SUCCESS",
                "commit": { "oid": head },
                "contexts": {
                    "totalCount": 1,
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": [{
                        "__typename": "CheckRun",
                        "name": "ci/build",
                        "status": "COMPLETED",
                        "conclusion": "SUCCESS"
                    }]
                }
            }
        } } }
    })
}

async fn run_github_with_timing(
    fixture: &Fixture,
    cancel_token: Option<&crate::cancellation::CancellationToken>,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<AutoSquashOutcome, AutoSquashError> {
    let url = "https://github.com/acme/service/pull/7";
    let request = fixture.request(url, cancel_token);
    let reference = super::parse_provider_ref(url).unwrap_or_else(|error| panic!("{error}"));
    super::perform_github_with_timing(&reference, &request, &fixture.head, timeout, poll_interval)
        .await
}

async fn wait_for_github_status_calls(fixture: &Fixture, expected: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.github_status_call_count() < expected {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("status query counter did not reach {expected}: {error}"));
}

fn github_open_identity(head: &str) -> Value {
    github_identity("OPEN", head)
}

fn gitlab_mr(state: &str, head: &str) -> Value {
    json!({
        "state": state,
        "draft": false,
        "sha": head,
        "detailed_merge_status": "mergeable",
        "merge_when_pipeline_succeeds": false,
        "head_pipeline": { "sha": head, "status": "success" }
    })
}

#[tokio::test]
async fn github_merge_uses_squash_and_atomic_head_guard_once() {
    let fixture = Fixture::new();
    fixture.configure_github(
        &github_open_identity(&fixture.head),
        &github_status(&fixture.head),
        &json!([]),
    );

    let outcome =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

    assert_eq!(outcome, Ok(AutoSquashOutcome::Merged));
    let effects = fixture.effect_lines();
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effects[0],
        format!(
            "api --hostname github.com --include --method PUT repos/acme/service/pulls/7/merge -f sha={} -f merge_method=squash",
            fixture.head
        )
    );
}

#[tokio::test]
async fn github_allows_the_exact_plan_limited_branch_rules_error() {
    let mut fixture = Fixture::new();
    let mut status = github_status(&fixture.head);
    status["data"]["repository"]["pullRequest"]["reviewDecision"] = json!(null);
    for direct_body in [false, true] {
        if fixture.merged.exists() {
            fs::remove_file(&fixture.merged).unwrap_or_else(|error| panic!("{error}"));
        }
        if fixture.effects.exists() {
            fs::remove_file(&fixture.effects).unwrap_or_else(|error| panic!("{error}"));
        }
        fixture.configure_github(&github_open_identity(&fixture.head), &status, &json!([]));
        fixture
            .env
            .insert("FAKE_GH_RULES_ERROR".to_string(), "1".to_string());
        if direct_body {
            fixture
                .env
                .insert("FAKE_GH_RULES_DIRECT_ERROR".to_string(), "1".to_string());
        } else {
            fixture.env.remove("FAKE_GH_RULES_DIRECT_ERROR");
        }
        write_json(
            &fixture.gh_rules,
            &json!({
                "message": "Upgrade to GitHub Pro or make this repository public to enable this feature.",
                "status": "403"
            }),
        );

        let result =
            squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

        assert_eq!(
            result,
            Ok(AutoSquashOutcome::Merged),
            "direct body: {direct_body}"
        );
        let effects = fixture.effect_lines();
        assert_eq!(effects.len(), 1);
        assert_eq!(
            effects[0],
            format!(
                "api --hostname github.com --include --method PUT repos/acme/service/pulls/7/merge -f sha={} -f merge_method=squash",
                fixture.head
            )
        );
    }
}

#[tokio::test]
async fn github_rejects_other_or_malformed_branch_rules_errors_without_merge() {
    let mut fixture = Fixture::new();
    fixture
        .env
        .insert("FAKE_GH_RULES_ERROR".to_string(), "1".to_string());
    let cases = [
        (
            "generic 403",
            r#"{"message":"Resource not accessible by integration","status":"403"}"#,
        ),
        (
            "wrong status",
            r#"{"message":"Upgrade to GitHub Pro or make this repository public to enable this feature.","status":"404"}"#,
        ),
        (
            "missing status",
            r#"{"message":"Upgrade to GitHub Pro or make this repository public to enable this feature."}"#,
        ),
        (
            "malformed 403",
            r#"{"message":"Upgrade to GitHub Pro or make this repository public to enable this feature.","status":"403""#,
        ),
    ];
    for (reason, response) in cases {
        fixture.configure_github(
            &github_open_identity(&fixture.head),
            &github_status(&fixture.head),
            &json!([]),
        );
        fs::write(&fixture.gh_rules, response)
            .unwrap_or_else(|error| panic!("writing {reason} fixture: {error}"));

        let result =
            squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

        assert!(
            matches!(result, Err(AutoSquashError::CommandFailed(_))),
            "{reason}: {result:?}"
        );
        assert_eq!(
            fixture.effect_lines(),
            Vec::<String>::new(),
            "{reason} must reject before merge"
        );
    }
}

#[tokio::test]
async fn review_regression_github_numeric_and_null_like_owner_repo_are_passed_as_strings() {
    for (owner, repo) in [("123", "true"), ("false", "null")] {
        let mut fixture = Fixture::new();
        let merge_path = format!("repos/{owner}/{repo}/pulls/7/merge");
        fixture
            .env
            .insert("FAKE_GH_MERGE_PATH".to_string(), merge_path);
        fixture.configure_github(
            &github_open_identity(&fixture.head),
            &github_status(&fixture.head),
            &json!([]),
        );

        let url = format!("https://github.com/{owner}/{repo}/pull/7");
        let result = squash_merge(fixture.request(&url, None)).await;

        assert_eq!(result, Ok(AutoSquashOutcome::Merged), "{owner}/{repo}");
        assert_eq!(fixture.effect_lines().len(), 1);
    }
}

#[tokio::test]
async fn review_regression_github_absent_optional_rule_arrays_are_permissive() {
    let fixture = Fixture::new();
    let mut status = github_status(&fixture.head);
    status["data"]["repository"]["pullRequest"]["reviewDecision"] = json!(null);
    fixture.configure_github(
        &github_open_identity(&fixture.head),
        &status,
        &json!([{
            "type": "pull_request",
            "parameters": {
                "required_approving_review_count": 0,
                "require_code_owner_review": false,
                "require_last_push_approval": false
            }
        }]),
    );

    let result =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

    assert_eq!(result, Ok(AutoSquashOutcome::Merged));
    assert_eq!(fixture.effect_lines().len(), 1);
}

#[tokio::test]
async fn review_regression_github_malformed_optional_rule_arrays_are_rejected() {
    let fixture = Fixture::new();
    for (field, invalid) in [
        ("required_reviewers", json!(null)),
        ("required_reviewers", json!("reviewer")),
        ("required_reviewers", json!([null])),
        ("allowed_merge_methods", json!(null)),
        ("allowed_merge_methods", json!("squash")),
        ("allowed_merge_methods", json!([])),
        ("allowed_merge_methods", json!(["squash", null])),
    ] {
        if fixture.merged.exists() {
            fs::remove_file(&fixture.merged).unwrap_or_else(|error| panic!("{error}"));
        }
        if fixture.effects.exists() {
            fs::remove_file(&fixture.effects).unwrap_or_else(|error| panic!("{error}"));
        }
        let mut parameters = json!({
            "required_approving_review_count": 0,
            "require_code_owner_review": false,
            "require_last_push_approval": false,
            "allowed_merge_methods": ["squash"],
            "required_reviewers": []
        });
        parameters[field] = invalid;
        fixture.configure_github(
            &github_open_identity(&fixture.head),
            &github_status(&fixture.head),
            &json!([{ "type": "pull_request", "parameters": parameters }]),
        );

        let result =
            squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

        assert!(
            matches!(result, Err(AutoSquashError::Rejected(_))),
            "{field}: {result:?}"
        );
        assert_eq!(
            fixture.effect_lines(),
            Vec::<String>::new(),
            "{field} must reject before merge"
        );
    }
}

#[tokio::test]
async fn github_allows_null_review_decision_when_no_review_rule_requires_approval() {
    let fixture = Fixture::new();
    let mut status = github_status(&fixture.head);
    status["data"]["repository"]["pullRequest"]["reviewDecision"] = json!(null);
    fixture.configure_github(&github_open_identity(&fixture.head), &status, &json!([]));

    let result =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

    assert_eq!(result, Ok(AutoSquashOutcome::Merged));
    assert_eq!(fixture.effect_lines().len(), 1);
}

async fn assert_github_rejected(
    fixture: &Fixture,
    reason: &str,
    identity: &Value,
    status: &Value,
    rules: &Value,
) {
    if fixture.merged.exists() {
        fs::remove_file(&fixture.merged).unwrap_or_else(|error| panic!("{error}"));
    }
    if fixture.effects.exists() {
        fs::remove_file(&fixture.effects).unwrap_or_else(|error| panic!("{error}"));
    }
    fixture.configure_github(identity, status, rules);
    let result =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;
    assert!(
        matches!(result, Err(AutoSquashError::Rejected(_))),
        "{reason}: {result:?}"
    );
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "{reason} must not issue a merge"
    );
}
#[tokio::test]
async fn github_blocks_queue_and_squash_policy_without_merge() {
    let fixture = Fixture::new();
    let mut queued = github_status(&fixture.head);
    queued["data"]["repository"]["pullRequest"]["mergeQueueEntry"] = json!({ "id": "queue-entry" });
    let mut auto_merging = github_status(&fixture.head);
    auto_merging["data"]["repository"]["pullRequest"]["autoMergeRequest"] =
        json!({ "enabledAt": "now" });
    let mut repository_disallows_squash = github_status(&fixture.head);
    repository_disallows_squash["data"]["repository"]["squashMergeAllowed"] = json!(false);
    let cases = [
        (
            "queue entry",
            github_open_identity(&fixture.head),
            queued,
            json!([]),
        ),
        (
            "existing auto-merge",
            github_open_identity(&fixture.head),
            auto_merging,
            json!([]),
        ),
        (
            "queue policy",
            github_open_identity(&fixture.head),
            github_status(&fixture.head),
            json!([{ "type": "merge_queue" }]),
        ),
        (
            "repository disallows squash",
            github_open_identity(&fixture.head),
            repository_disallows_squash,
            json!([]),
        ),
        (
            "branch rules disallow squash",
            github_open_identity(&fixture.head),
            github_status(&fixture.head),
            json!([{
                "type": "pull_request",
                "parameters": { "allowed_merge_methods": ["merge"] }
            }]),
        ),
    ];
    for (reason, identity, status, rules) in cases {
        assert_github_rejected(&fixture, reason, &identity, &status, &rules).await;
    }
}

#[tokio::test]
async fn github_blocks_failed_ci_and_review_requirements_without_merge() {
    let fixture = Fixture::new();
    let mut failed_ci = github_status(&fixture.head);
    failed_ci["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"][0]["conclusion"] =
        json!("FAILURE");
    let mut needs_review = github_status(&fixture.head);
    needs_review["data"]["repository"]["pullRequest"]["reviewDecision"] = json!("REVIEW_REQUIRED");
    let mut approval_missing = github_status(&fixture.head);
    approval_missing["data"]["repository"]["pullRequest"]["reviewDecision"] = json!(null);
    let cases = [
        (
            "failed CI",
            github_open_identity(&fixture.head),
            failed_ci,
            json!([]),
        ),
        (
            "review",
            github_open_identity(&fixture.head),
            needs_review,
            json!([]),
        ),
        (
            "missing required approval",
            github_open_identity(&fixture.head),
            approval_missing,
            json!([{
                "type": "pull_request",
                "parameters": {
                    "allowed_merge_methods": ["squash"],
                    "required_approving_review_count": 1,
                    "require_code_owner_review": false,
                    "require_last_push_approval": false,
                    "required_reviewers": []
                }
            }]),
        ),
    ];
    for (reason, identity, status, rules) in cases {
        assert_github_rejected(&fixture, reason, &identity, &status, &rules).await;
    }
}

#[tokio::test]
async fn github_blocks_drafts_conflicts_and_head_races() {
    let fixture = Fixture::new();
    let other_head = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut draft = github_status(&fixture.head);
    draft["data"]["repository"]["pullRequest"]["isDraft"] = json!(true);
    let mut conflict = github_status(&fixture.head);
    conflict["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
    let cases = [
        (
            "draft",
            github_open_identity(&fixture.head),
            draft,
            json!([]),
        ),
        (
            "conflict",
            github_open_identity(&fixture.head),
            conflict,
            json!([]),
        ),
        (
            "stale head",
            github_open_identity(other_head),
            github_status(&fixture.head),
            json!([]),
        ),
    ];
    for (reason, identity, status, rules) in cases {
        assert_github_rejected(&fixture, reason, &identity, &status, &rules).await;
    }
}

#[tokio::test]
async fn github_mergeability_rejection_reports_observed_values_without_merge() {
    for (mergeable, merge_state_status) in [("CONFLICTING", "DIRTY"), ("MERGEABLE", "UNSTABLE")] {
        let fixture = Fixture::new();
        let mut status = github_status(&fixture.head);
        status["data"]["repository"]["pullRequest"]["mergeable"] = json!(mergeable);
        status["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!(merge_state_status);
        fixture.configure_github(&github_open_identity(&fixture.head), &status, &json!([]));

        let result =
            squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;
        let reason = match result {
            Err(AutoSquashError::Rejected(reason)) => reason,
            other => panic!("{mergeable}/{merge_state_status}: {other:?}"),
        };
        assert!(
            reason.contains(&format!("mergeable={mergeable}")),
            "{reason}"
        );
        assert!(
            reason.contains(&format!("mergeStateStatus={merge_state_status}")),
            "{reason}"
        );
        assert_eq!(
            fixture.effect_lines(),
            Vec::<String>::new(),
            "{mergeable}/{merge_state_status} must not issue a merge"
        );
    }
}

#[tokio::test]
async fn github_pending_ci_with_unstable_mergeability_waits_for_current_head_success() {
    let fixture = Fixture::new();
    let mut pending = github_status(&fixture.head);
    pending["data"]["repository"]["pullRequest"]["mergeable"] = json!("MERGEABLE");
    pending["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("UNSTABLE");
    pending["data"]["repository"]["pullRequest"]["statusCheckRollup"]["state"] = json!("PENDING");
    pending["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"][0]["status"] =
        json!("IN_PROGRESS");
    pending["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"][0]["conclusion"] =
        Value::Null;
    fixture.configure_github_sequence(
        &github_open_identity(&fixture.head),
        &[pending, github_status(&fixture.head)],
        &json!([]),
    );

    let result = run_github_with_timing(
        &fixture,
        None,
        Duration::from_secs(2),
        Duration::from_millis(5),
    )
    .await;

    assert_eq!(result, Ok(AutoSquashOutcome::Merged));
    assert_eq!(fixture.github_status_call_count(), 2);
    assert_eq!(fixture.effect_lines().len(), 1);
}

#[tokio::test]
async fn github_current_head_ci_failure_wins_over_unstable_mergeability() {
    let fixture = Fixture::new();
    let mut failed = github_status(&fixture.head);
    failed["data"]["repository"]["pullRequest"]["mergeable"] = json!("MERGEABLE");
    failed["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("UNSTABLE");
    failed["data"]["repository"]["pullRequest"]["statusCheckRollup"]["state"] = json!("PENDING");
    failed["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["totalCount"] =
        json!(2);
    failed["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("CI fixture nodes must be an array"))
        .push(json!({
            "__typename": "CheckRun",
            "name": "ci/pending",
            "status": "IN_PROGRESS",
            "conclusion": null
        }));
    failed["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"][0]["conclusion"] =
        json!("FAILURE");
    fixture.configure_github(&github_open_identity(&fixture.head), &failed, &json!([]));

    let result = run_github_with_timing(
        &fixture,
        None,
        Duration::from_millis(500),
        Duration::from_millis(5),
    )
    .await;

    let reason = match result {
        Err(AutoSquashError::Rejected(reason)) => reason,
        other => panic!("expected immediate current-head CI rejection, got {other:?}"),
    };
    assert!(
        reason.contains("CI failed for the current pull request head"),
        "{reason}"
    );
    assert_eq!(fixture.provider_log().matches("mergeQueueEntry").count(), 1);
    assert_eq!(fixture.effect_lines(), Vec::<String>::new());
}

#[tokio::test]
async fn github_stale_unstable_ci_waits_for_current_head_success() {
    let fixture = Fixture::new();
    let mut stale = github_status(&fixture.head);
    stale["data"]["repository"]["pullRequest"]["statusCheckRollup"]["commit"]["oid"] =
        json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    stale["data"]["repository"]["pullRequest"]["mergeable"] = json!("MERGEABLE");
    stale["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("UNSTABLE");
    let mut pending = github_status(&fixture.head);
    pending["data"]["repository"]["pullRequest"]["statusCheckRollup"]["state"] = json!("PENDING");
    pending["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"][0]["status"] =
        json!("IN_PROGRESS");
    pending["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"][0]["conclusion"] =
        Value::Null;
    fixture.configure_github_sequence(
        &github_open_identity(&fixture.head),
        &[stale, pending, github_status(&fixture.head)],
        &json!([]),
    );

    let result = run_github_with_timing(
        &fixture,
        None,
        Duration::from_secs(2),
        Duration::from_millis(5),
    )
    .await;

    assert_eq!(result, Ok(AutoSquashOutcome::Merged));
    assert_eq!(fixture.github_status_call_count(), 3);
    assert_eq!(fixture.effect_lines().len(), 1);
}

#[tokio::test]
async fn github_null_and_empty_unstable_ci_registration_waits_for_checks() {
    let fixture = Fixture::new();
    let mut null_rollup = github_status(&fixture.head);
    null_rollup["data"]["repository"]["pullRequest"]["mergeable"] = json!("MERGEABLE");
    null_rollup["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("UNSTABLE");
    null_rollup["data"]["repository"]["pullRequest"]["statusCheckRollup"] = Value::Null;
    let mut empty_rollup = github_status(&fixture.head);
    empty_rollup["data"]["repository"]["pullRequest"]["mergeable"] = json!("MERGEABLE");
    empty_rollup["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("UNSTABLE");
    empty_rollup["data"]["repository"]["pullRequest"]["statusCheckRollup"]["state"] =
        json!("EXPECTED");
    empty_rollup["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["totalCount"] =
        json!(0);
    empty_rollup["data"]["repository"]["pullRequest"]["statusCheckRollup"]["contexts"]["nodes"] =
        json!([]);
    fixture.configure_github_sequence(
        &github_open_identity(&fixture.head),
        &[null_rollup, empty_rollup, github_status(&fixture.head)],
        &json!([]),
    );

    let result = run_github_with_timing(
        &fixture,
        None,
        Duration::from_secs(2),
        Duration::from_millis(5),
    )
    .await;

    assert_eq!(result, Ok(AutoSquashOutcome::Merged));
    assert_eq!(fixture.github_status_call_count(), 3);
    assert_eq!(fixture.effect_lines().len(), 1);
}

#[tokio::test]
async fn github_repeated_unknown_times_out_with_latest_observed_values() {
    let fixture = Fixture::new();
    let mut unknown = github_status(&fixture.head);
    unknown["data"]["repository"]["pullRequest"]["mergeable"] = json!("UNKNOWN");
    fixture.configure_github(&github_open_identity(&fixture.head), &unknown, &json!([]));

    let result = run_github_with_timing(
        &fixture,
        None,
        Duration::from_millis(500),
        Duration::from_millis(5),
    )
    .await;

    let reason = match result {
        Err(AutoSquashError::Rejected(reason)) => reason,
        other => panic!("expected bounded readiness timeout, got {other:?}"),
    };
    assert!(reason.contains("mergeable=UNKNOWN"), "{reason}");
    assert!(reason.contains("mergeStateStatus=CLEAN"), "{reason}");
    assert!(reason.contains("CI state=SUCCESS"), "{reason}");
    assert!(
        reason.contains(&format!("CI head={}", fixture.head)),
        "{reason}"
    );
    assert_eq!(fixture.effect_lines(), Vec::<String>::new());
}

#[tokio::test]
async fn github_malformed_ci_state_is_not_retried() {
    let fixture = Fixture::new();
    let mut malformed = github_status(&fixture.head);
    malformed["data"]["repository"]["pullRequest"]["statusCheckRollup"]["state"] =
        json!("UNKNOWN_STATE");
    fixture.configure_github_sequence(
        &github_open_identity(&fixture.head),
        &[malformed, github_status(&fixture.head)],
        &json!([]),
    );

    let result = run_github_with_timing(
        &fixture,
        None,
        Duration::from_secs(2),
        Duration::from_millis(5),
    )
    .await;

    assert!(matches!(result, Err(AutoSquashError::Rejected(_))));
    assert_eq!(fixture.github_status_call_count(), 1);
    assert_eq!(fixture.effect_lines(), Vec::<String>::new());
}

#[tokio::test]
async fn github_cancellation_during_readiness_wait_never_merges() {
    let fixture = Fixture::new();
    let mut unknown = github_status(&fixture.head);
    unknown["data"]["repository"]["pullRequest"]["mergeable"] = json!("UNKNOWN");
    fixture.configure_github_sequence(&github_open_identity(&fixture.head), &[unknown], &json!([]));
    let cancellation = crate::cancellation::CancellationToken::new();
    let running = run_github_with_timing(
        &fixture,
        Some(&cancellation),
        Duration::from_secs(5),
        Duration::from_millis(5),
    );
    tokio::pin!(running);

    tokio::select! {
        result = &mut running => panic!("readiness finished before retrying: {result:?}"),
        () = wait_for_github_status_calls(&fixture, 2) => {}
    }
    cancellation.cancel();
    let result = running.await;

    assert_eq!(result, Err(AutoSquashError::Cancelled));
    assert_eq!(fixture.effect_lines(), Vec::<String>::new());
}

#[tokio::test]
async fn github_local_head_change_during_readiness_never_merges() {
    let fixture = Fixture::new();
    let alternate_head = {
        let _process_lock = crate::test_support::lock_process();
        git(
            &fixture.repo,
            &["commit", "--allow-empty", "--quiet", "-m", "alternate HEAD"],
        );
        let alternate_head = String::from_utf8(git(&fixture.repo, &["rev-parse", "HEAD"]).stdout)
            .unwrap_or_else(|error| panic!("{error}"))
            .trim()
            .to_string();
        git(&fixture.repo, &["reset", "--hard", fixture.head.as_str()]);
        alternate_head
    };
    let mut unknown = github_status(&fixture.head);
    unknown["data"]["repository"]["pullRequest"]["mergeable"] = json!("UNKNOWN");
    fixture.configure_github_sequence(&github_open_identity(&fixture.head), &[unknown], &json!([]));
    let running = run_github_with_timing(
        &fixture,
        None,
        Duration::from_secs(5),
        Duration::from_millis(5),
    );
    tokio::pin!(running);

    tokio::select! {
        result = &mut running => panic!("readiness finished before local HEAD changed: {result:?}"),
        () = wait_for_github_status_calls(&fixture, 2) => {}
    }
    {
        let _process_lock = crate::test_support::lock_process();
        git(
            &fixture.repo,
            &["update-ref", "HEAD", alternate_head.as_str()],
        );
    }
    let result = running.await;

    let reason = match result {
        Err(AutoSquashError::Rejected(reason)) => reason,
        other => panic!("expected local HEAD rejection, got {other:?}"),
    };
    assert!(reason.contains("local HEAD changed"), "{reason}");
    assert_eq!(fixture.effect_lines(), Vec::<String>::new());
}

#[tokio::test]
async fn github_local_head_check_does_not_refresh_git_index() {
    let fixture = Fixture::new();
    let tracked_file =
        fs::File::open(fixture.repo.join("file.txt")).unwrap_or_else(|error| panic!("{error}"));
    tracked_file
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(2)))
        .unwrap_or_else(|error| panic!("{error}"));
    let index_path = fixture.repo.join(".git/index");
    let index_before = fs::read(&index_path).unwrap_or_else(|error| panic!("{error}"));
    let url = "https://github.com/acme/service/pull/7";
    let request = fixture.request(url, None);

    let head = super::local_head(&request).await;

    assert_eq!(head, Ok(fixture.head.clone()));
    assert_eq!(
        fs::read(index_path).unwrap_or_else(|error| panic!("{error}")),
        index_before
    );
}

#[tokio::test]
async fn github_already_merged_same_head_is_idempotent_without_ci_or_write() {
    let fixture = Fixture::new();
    write_json(
        &fixture.gh_identity,
        &github_identity("MERGED", &fixture.head),
    );
    write_json(
        &fixture.gh_status,
        &json!({"not": "needed for already-merged PR"}),
    );
    write_json(&fixture.gh_rules, &json!([]));

    let outcome =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

    assert_eq!(outcome, Ok(AutoSquashOutcome::AlreadyMerged));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "merged PR wrote an effect"
    );
    assert!(!fixture.provider_log().contains("rules/branches"));
}

#[tokio::test]
async fn github_open_after_one_merge_request_is_unconfirmed_without_retry() {
    let fixture = Fixture::new();
    fixture.configure_github(
        &github_open_identity(&fixture.head),
        &github_status(&fixture.head),
        &json!([]),
    );
    write_json(
        &fixture.gh_identity_after,
        &github_identity("OPEN", &fixture.head),
    );

    let result =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

    assert!(matches!(result, Err(AutoSquashError::Unconfirmed(_))));
    assert_eq!(fixture.effect_lines().len(), 1);
}

#[tokio::test]
async fn github_rest_405_rejects_synchronous_merge_without_a_queued_effect() {
    let mut fixture = Fixture::new();
    fixture.configure_github(
        &github_open_identity(&fixture.head),
        &github_status(&fixture.head),
        &json!([]),
    );
    fixture
        .env
        .insert("FAKE_GH_MERGE_STATUS".to_string(), "405".to_string());

    let result =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;

    assert!(matches!(result, Err(AutoSquashError::CommandFailed(_))));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "405 response wrote an effect"
    );
    assert!(
        fixture
            .provider_log()
            .contains("repos/acme/service/pulls/7/merge")
    );
}

async fn github_refusal_fixture(
    response: &[u8],
    after: Option<&str>,
) -> (Fixture, Result<AutoSquashOutcome, AutoSquashError>) {
    let mut fixture = Fixture::new();
    fixture.configure_github(
        &github_open_identity(&fixture.head),
        &github_status(&fixture.head),
        &json!([]),
    );
    let response_path = fixture.temp.path().join("response.bin");
    fs::write(&response_path, response).unwrap_or_else(|error| panic!("{error}"));
    fixture.env.insert(
        "FAKE_GH_RESPONSE".into(),
        response_path.display().to_string(),
    );
    if let Some(state) = after {
        write_json(
            &fixture.gh_identity_after,
            &github_identity(state, &fixture.head),
        );
        fixture
            .env
            .insert("FAKE_GH_CONFIRM_AFTER".into(), "1".into());
    }
    let result =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;
    (fixture, result)
}

#[tokio::test]
async fn github_refusal_canonical_reasons_are_safe_and_not_inferred_from_status() {
    for (status, message, reason) in [
        (405, "Pull Request is not mergeable", "merge_not_performed"),
        (
            409,
            "Head branch was modified. Review and try the merge again.",
            "head_changed",
        ),
        (
            405,
            "Base branch was modified. Review and try the merge again.",
            "base_changed",
        ),
        (
            405,
            "Squash merges are not allowed on this repository.",
            "squash_disabled",
        ),
        (409, "Merge already in progress", "merge_in_progress"),
    ] {
        let response = format!(
            "HTTP/2 {status}\r\nX-Secret: header-secret\r\n\r\n{}",
            json!({"message": message})
        );
        let (fixture, result) = github_refusal_fixture(response.as_bytes(), None).await;
        let Err(AutoSquashError::CommandFailed(error)) = result else {
            panic!("unexpected result: {result:?}")
        };
        assert!(error.contains(&format!("reason={reason}")), "{error}");
        assert!(error.contains(&format!("HTTP={status}")), "{error}");
        assert!(error.contains("exit=1"), "{error}");
        assert!(!error.contains("header-secret"), "{error}");
        assert_eq!(fixture.effect_lines(), Vec::<String>::new());
        assert_eq!(
            fixture
                .provider_log()
                .lines()
                .filter(|line| line.contains("pulls/7/merge"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn github_refusal_http_2_0_include_response_is_classified() {
    let response = b"HTTP/2.0 409 Conflict\r\nContent-Type: application/json\r\n\r\n{\"message\":\"Head branch was modified. Review and try the merge again.\"}";
    let (_, result) = github_refusal_fixture(response, None).await;
    let Err(AutoSquashError::CommandFailed(error)) = result else {
        panic!("unexpected result: {result:?}")
    };
    assert!(error.contains("HTTP=409"), "{error}");
    assert!(error.contains("reason=head_changed"), "{error}");
}

#[tokio::test]
async fn github_refusal_untrusted_and_boundary_responses_are_unclassified() {
    let secret = "Bearer secret-token https://private.example/path";
    let cases = vec![
        format!(
            "HTTP/1.1 405 Nope\nX-Secret: {secret}\n\n{}",
            json!({"message": format!("Pull Request is not mergeable {secret}")})
        )
        .into_bytes(),
        b"HTTP/2 409\r\n\r\nnot-json".to_vec(),
        b"".to_vec(),
        b"HTTP/2 405\n\n\xff".to_vec(),
        format!("HTTP/2 405\n\n{}", json!({"message": "a".repeat(513)})).into_bytes(),
        format!("HTTP/2 405\n\n{}", "a".repeat(65537)).into_bytes(),
    ];
    for response in cases {
        let (fixture, result) = github_refusal_fixture(&response, None).await;
        let Err(AutoSquashError::CommandFailed(error)) = result else {
            panic!("unexpected result: {result:?}")
        };
        assert!(error.contains("reason=unclassified"), "{error}");
        assert!(error.contains("exit=1"), "{error}");
        assert!(!error.contains(secret), "{error}");
        assert_eq!(fixture.effect_lines(), Vec::<String>::new());
    }
}

#[tokio::test]
async fn github_refusal_confirmation_takes_priority_over_provider_response() {
    let response = b"HTTP/2 409\r\n\r\n{\"message\":\"Head branch was modified. Review and try the merge again.\"}";
    let (fixture, result) = github_refusal_fixture(response, Some("MERGED")).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(fixture.effect_lines(), Vec::<String>::new());
    let (_, result) = github_refusal_fixture(response, Some("CLOSED")).await;
    let Err(error) = result else {
        panic!("closed PR must not merge: {result:?}");
    };
    assert!(
        !error.to_string().contains("reason=head_changed"),
        "{error}"
    );
}

#[tokio::test]
async fn github_malformed_provider_output_is_sanitized_and_has_no_effect() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.gh_identity,
        "this is malformed and must not be exposed",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let result =
        squash_merge(fixture.request("https://github.com/acme/service/pull/7", None)).await;
    let error = match result {
        Err(error) => error.to_string(),
        Ok(outcome) => panic!("expected malformed provider response, got {outcome:?}"),
    };
    assert!(!error.contains("malformed and must not be exposed"));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "malformed response wrote an effect"
    );
}

#[tokio::test]
async fn review_regression_gitlab_mergeable_ce_without_approval_state_merges() {
    let fixture = Fixture::new();
    fixture.configure_gitlab(
        &gitlab_mr("opened", &fixture.head),
        &json!({ "squash_option": "default_off" }),
    );

    let result = squash_merge(fixture.request(
        "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
        None,
    ))
    .await;

    assert_eq!(result, Ok(AutoSquashOutcome::Merged));
    assert!(!fixture.provider_log().contains("/approval_state"));
    assert_eq!(fixture.effect_lines().len(), 1);
}

#[tokio::test]
async fn review_regression_gitlab_not_approved_and_approvals_syncing_reject() {
    let fixture = Fixture::new();
    for status in ["not_approved", "approvals_syncing"] {
        if fixture.merged.exists() {
            fs::remove_file(&fixture.merged).unwrap_or_else(|error| panic!("{error}"));
        }
        if fixture.effects.exists() {
            fs::remove_file(&fixture.effects).unwrap_or_else(|error| panic!("{error}"));
        }
        let mut mr = gitlab_mr("opened", &fixture.head);
        mr["detailed_merge_status"] = json!(status);
        fixture.configure_gitlab(&mr, &json!({ "squash_option": "default_off" }));

        let result = squash_merge(fixture.request(
            "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
            None,
        ))
        .await;

        assert!(
            matches!(result, Err(AutoSquashError::Rejected(_))),
            "{status}: {result:?}"
        );
        assert_eq!(
            fixture.effect_lines(),
            Vec::<String>::new(),
            "{status} must reject before merge"
        );
    }
}

#[tokio::test]
async fn review_regression_gitlab_server_rejects_stale_ready_status_without_merge_success() {
    let mut fixture = Fixture::new();
    fixture.configure_gitlab(
        &gitlab_mr("opened", &fixture.head),
        &json!({ "squash_option": "default_off" }),
    );
    fixture
        .env
        .insert("FAKE_GL_MERGE_REJECT".to_string(), "1".to_string());

    let result = squash_merge(fixture.request(
        "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
        None,
    ))
    .await;

    assert!(matches!(result, Err(AutoSquashError::CommandFailed(_))));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "server rejection wrote an effect"
    );
    assert!(!fixture.merged.exists());
    assert!(fixture.provider_log().contains("/merge --method PUT"));
}
#[tokio::test]
async fn gitlab_merge_uses_expected_sha_squash_and_explicit_safe_options_once() {
    let fixture = Fixture::new();
    fixture.configure_gitlab(
        &gitlab_mr("opened", &fixture.head),
        &json!({ "squash_option": "default_off" }),
    );

    let outcome = squash_merge(fixture.request(
        "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
        None,
    ))
    .await;

    assert_eq!(outcome, Ok(AutoSquashOutcome::Merged));
    let effects = fixture.effect_lines();
    assert_eq!(effects.len(), 1);
    let command = &effects[0];
    assert!(command.contains("--method PUT"));
    assert!(command.contains(&format!("sha={}", fixture.head)));
    assert!(command.contains("squash=true"));
    assert!(command.contains("should_remove_source_branch=false"));
    assert!(command.contains("auto_merge=false"));
    assert!(command.contains("group%2Fsubgroup%2Fproject"));
    assert!(command.contains("--hostname gitlab.example.com"));
}

#[tokio::test]
async fn gitlab_blocks_pending_policy_head_states_without_merge() {
    let fixture = Fixture::new();
    let other_head = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let cases = [
        (
            "stale pipeline",
            gitlab_mr("opened", &fixture.head).tap(|mr| {
                mr["head_pipeline"]["sha"] = json!(other_head);
            }),
            json!({ "squash_option": "default_off" }),
        ),
        (
            "draft",
            gitlab_mr("opened", &fixture.head).tap(|mr| mr["draft"] = json!(true)),
            json!({ "squash_option": "default_off" }),
        ),
        (
            "pending pipeline",
            gitlab_mr("opened", &fixture.head)
                .tap(|mr| mr["head_pipeline"]["status"] = json!("running")),
            json!({ "squash_option": "default_off" }),
        ),
        (
            "existing auto-merge",
            gitlab_mr("opened", &fixture.head)
                .tap(|mr| mr["merge_when_pipeline_succeeds"] = json!(true)),
            json!({ "squash_option": "default_off" }),
        ),
        (
            "squash forbidden",
            gitlab_mr("opened", &fixture.head),
            json!({ "squash_option": "never" }),
        ),
        (
            "stale head",
            gitlab_mr("opened", other_head),
            json!({ "squash_option": "default_off" }),
        ),
    ];

    for (reason, mr, project) in cases {
        if fixture.merged.exists() {
            fs::remove_file(&fixture.merged).unwrap_or_else(|error| panic!("{error}"));
        }
        if fixture.effects.exists() {
            fs::remove_file(&fixture.effects).unwrap_or_else(|error| panic!("{error}"));
        }
        fixture.configure_gitlab(&mr, &project);
        let result = squash_merge(fixture.request(
            "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
            None,
        ))
        .await;
        assert!(
            matches!(result, Err(AutoSquashError::Rejected(_))),
            "{reason}: {result:?}"
        );
        assert_eq!(
            fixture.effect_lines(),
            Vec::<String>::new(),
            "{reason} must not issue a merge"
        );
    }
}

#[tokio::test]
async fn gitlab_second_snapshot_requires_expected_head_before_already_merged() {
    let mut fixture = Fixture::new();
    let other_head = "cccccccccccccccccccccccccccccccccccccccc";
    fixture.configure_gitlab(
        &gitlab_mr("opened", &fixture.head),
        &json!({ "squash_option": "default_off" }),
    );
    write_json(&fixture.gl_mr_after, &gitlab_mr("merged", other_head));
    fixture
        .env
        .insert("FAKE_GL_MR_SECOND_SNAPSHOT".to_string(), "1".to_string());

    let changed_head = squash_merge(fixture.request(
        "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
        None,
    ))
    .await;

    assert!(matches!(changed_head, Err(AutoSquashError::Rejected(_))));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "stale head wrote an effect"
    );

    fixture.configure_gitlab(
        &gitlab_mr("opened", &fixture.head),
        &json!({ "squash_option": "default_off" }),
    );
    let same_head = squash_merge(fixture.request(
        "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
        None,
    ))
    .await;

    assert_eq!(same_head, Ok(AutoSquashOutcome::AlreadyMerged));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "already-merged MR wrote an effect"
    );
}

#[tokio::test]
async fn gitlab_already_merged_same_head_is_idempotent_and_local_dirty_state_blocks() {
    let fixture = Fixture::new();
    write_json(
        &fixture.gl_mr,
        &json!({ "state": "merged", "sha": fixture.head }),
    );
    let outcome = squash_merge(fixture.request(
        "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
        None,
    ))
    .await;
    assert_eq!(outcome, Ok(AutoSquashOutcome::AlreadyMerged));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "already-merged MR wrote an effect"
    );

    fs::write(fixture.repo.join("dirty.txt"), "uncommitted\n")
        .unwrap_or_else(|error| panic!("{error}"));
    let result = squash_merge(fixture.request(
        "https://gitlab.example.com/group/subgroup/project/-/merge_requests/42",
        None,
    ))
    .await;
    assert!(matches!(result, Err(AutoSquashError::Rejected(_))));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "dirty repository wrote an effect"
    );
}

#[tokio::test]
async fn cancellation_before_any_command_has_no_provider_effect() {
    let fixture = Fixture::new();
    let cancellation = crate::cancellation::CancellationToken::new();
    cancellation.cancel();

    let result = squash_merge(fixture.request(
        "https://github.com/acme/service/pull/7",
        Some(&cancellation),
    ))
    .await;

    assert_eq!(result, Err(AutoSquashError::Cancelled));
    assert_eq!(
        fixture.effect_lines(),
        Vec::<String>::new(),
        "cancelled request wrote an effect"
    );
    assert!(!fixture.log.exists());
}

trait Tap: Sized {
    fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}

impl<T> Tap for T {}
