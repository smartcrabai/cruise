use super::{
    Body, Harness, Request, StatusCode, create_session, form_body, get, harness, raw_response, send,
};
use crate::manager::{self, Check, Criterion, Goal, GoalStatus, Proposal, Task};
use crate::test_support::{EnvGuard, init_git_repo, run_git_ok};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use tower::ServiceExt as _;

fn isolate_records(harness: &Harness) -> EnvGuard {
    EnvGuard::set("XDG_DATA_HOME", harness.dir.path().join("xdg-data"))
}

fn repository(harness: &Harness) -> PathBuf {
    let repo = harness.dir.path().join("review repo");
    std::fs::create_dir(&repo).unwrap_or_else(|e| panic!("create repo: {e}"));
    init_git_repo(&repo);
    run_git_ok(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    run_git_ok(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo
}

fn snapshot(repo: &Path) -> manager::Snapshot {
    manager::snapshot(repo).unwrap_or_else(|e| panic!("snapshot: {e}"))
}

fn repo_key(path: &Path) -> String {
    URL_SAFE_NO_PAD.encode(path.to_string_lossy().as_bytes())
}

fn key(repo: &Path) -> String {
    repo_key(&snapshot(repo).repository)
}

fn proposal(repo: &Path) -> Proposal {
    let head = std::process::Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap_or_else(|e| panic!("git rev-parse: {e}"));
    assert!(head.status.success());
    manager::save_proposal(
        repo,
        Proposal {
            id: String::new(),
            revision: 0,
            objective: "Deliver the reviewable feature".into(),
            target_branch: "main".into(),
            target_sha: String::from_utf8_lossy(&head.stdout).trim().into(),
            exclusions: vec!["Do not publish unrelated changes".into()],
            criteria: vec![Criterion {
                id: "criterion".into(),
                description: "Feature is usable".into(),
            }],
            checks: vec![Check {
                id: "check".into(),
                criterion_ids: vec!["criterion".into()],
                argv: vec!["printf".into(), "check-argument".into()],
            }],
            tasks: vec![
                Task {
                    id: "implement-feature".into(),
                    instructions: "Implement".into(),
                    dependencies: vec![],
                },
                Task {
                    id: "verify-feature".into(),
                    instructions: "Verify".into(),
                    dependencies: vec!["implement-feature".into()],
                },
            ],
            workflow: String::new(),
            contract: None,
        },
    )
    .unwrap_or_else(|e| panic!("save proposal: {e}"))
}

fn goal(repo: &Path) -> Goal {
    let pending = proposal(repo);
    manager::approve(repo, &pending.id, pending.revision)
        .unwrap_or_else(|e| panic!("seed Goal: {e}"))
}

fn blocked_verification(repo: &Path) -> Goal {
    use serde_json::json;

    let active = goal(repo);
    manager::update_goal(repo, &active.id, |goal| {
        let tip = &goal.proposal.target_sha;
        let check = &goal.proposal.checks[0];
        goal.status = GoalStatus::Blocked;
        goal.reason = Some("The approved check failed".into());
        goal.execution.attempts =
            serde_json::from_value(json!(goal.proposal.tasks.iter().map(|task| json!({
            "task_id": task.id, "attempt": 1, "session_id": format!("child-{}", task.id),
            "base_sha": tip, "launch_started": true, "result_sha": tip, "integrated": true,
        })).collect::<Vec<_>>()))?;
        goal.execution.publication = Some(serde_json::from_value(json!({
            "tip": tip, "worktree": repo.join("verification"), "branch": "cruise/goal/fixture",
            "phase": "Ready", "forge_kind": "GitHub", "forge_host": "github.com",
            "forge_repository": "example/project", "checks": [{
                "check_id": check.id, "argv": check.argv, "verified_tip": tip, "exit_code": 1,
                "cancelled": false, "stdout_log": "out", "stderr_log": "err",
            }],
        }))?);
        Ok(goal.clone())
    })
    .unwrap_or_else(|e| panic!("seed failed verification: {e}"))
}

async fn submit(
    harness: &Harness,
    uri: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String, String) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("hx-request", "true")
        .body(Body::from(form_body(fields)))
        .unwrap_or_else(|e| panic!("POST {uri}: {e}"));
    send(harness, request).await
}

fn fragment(content_type: &str, html: &str, text: &str) {
    assert!(
        content_type.starts_with("text/html"),
        "{content_type}: {html}"
    );
    assert!(html.contains("<hx-partial"), "{html}");
    assert!(
        html.to_lowercase().contains(&text.to_lowercase()),
        "missing {text:?}: {html}"
    );
}

#[tokio::test]
async fn manager_list_empty_has_open_form_and_navigation() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let (status, _, html) = get(&harness, "/manager", &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let text = html.to_lowercase();
    assert!(
        text.contains("no manager") || text.contains("no repositor"),
        "{html}"
    );
    assert!(
        html.contains("Open repository") && html.contains("/webui/manager/open"),
        "{html}"
    );
    let (status, _, home) = get(&harness, "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("href=\"/manager\""), "{home}");
}

#[tokio::test]
async fn manager_list_links_to_canonical_repository_record() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    proposal(&repo);
    let (status, _, html) = get(&harness, "/manager", &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(snapshot(&repo).repository.to_string_lossy().as_ref()),
        "{html}"
    );
    assert!(
        html.contains(&format!("href=\"/manager/{}\"", key(&repo))),
        "{html}"
    );
}

#[tokio::test]
async fn manager_detail_rejects_malformed_and_unlisted_repository_keys() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    for key in ["not+base64".into(), key(&repo)] {
        let (status, _, _) = get(&harness, &format!("/manager/{key}"), &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{key}");
    }
    proposal(&repo);
    let detail = format!("/manager/{}", key(&repo));
    let request = Request::builder()
        .uri(detail.as_str())
        .header("host", "localhost:8484")
        .header("cookie", "cruise_webui=test-session")
        .header("hx-request", "true")
        .body(Body::empty())
        .unwrap_or_else(|e| panic!("build detail request: {e}"));
    let response = raw_response(&harness, request).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the same key must work after registration"
    );
    assert_eq!(
        response.headers()["hx-push-url"]
            .to_str()
            .unwrap_or_else(|e| panic!("read push URL: {e}")),
        detail
    );
}

#[tokio::test]
async fn manager_open_non_git_path_returns_inline_validation_error() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let (status, content_type, html) = submit(
        &harness,
        "/webui/manager/open",
        &[("path", &harness.dir.path().to_string_lossy())],
    )
    .await;
    assert!(
        status.is_success() || status.is_client_error(),
        "{status}: {html}"
    );
    fragment(
        &content_type,
        &html,
        "manager requires a local Git repository",
    );
}

#[tokio::test]
async fn manager_open_rejects_unlisted_checkout_and_redirects_listed_checkout() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let destination = format!("/manager/{}", key(&repo));
    let request = Request::builder()
        .method("POST")
        .uri("/webui/manager/open")
        .header("host", "localhost:8484")
        .header("cookie", "cruise_webui=test-session")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("hx-request", "true")
        .body(Body::from(form_body(&[("path", &repo.to_string_lossy())])))
        .unwrap_or_else(|e| panic!("build open request: {e}"));
    let response = raw_response(&harness, request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!response.headers().contains_key("location"));
    assert!(!response.headers().contains_key("hx-redirect"));
    let ct = response.headers()["content-type"]
        .to_str()
        .unwrap_or_else(|e| panic!("read content type: {e}"))
        .to_owned();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|e| panic!("read response body: {e}"));
    let html = std::str::from_utf8(&body).unwrap_or_else(|e| panic!("decode response: {e}"));
    fragment(&ct, html, "No Manager record exists for this repository.");
    assert!(html.contains("manager-open-error"), "{html}");
    assert!(
        manager::snapshots()
            .unwrap_or_else(|e| panic!("list Manager snapshots: {e}"))
            .is_empty()
    );
    assert_eq!(
        get(&harness, &destination, &[]).await.0,
        StatusCode::NOT_FOUND
    );

    proposal(&repo);
    let response = super::post_form(
        &harness,
        "/webui/manager/open",
        form_body(&[("path", &repo.to_string_lossy())]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"]
        .to_str()
        .unwrap_or_else(|e| panic!("read redirect location: {e}"));
    assert_eq!(location, destination);
    assert_eq!(get(&harness, location, &[]).await.0, StatusCode::OK);
}

#[tokio::test]
async fn manager_open_rejects_persisted_bare_repository() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let bare = harness.dir.path().join("bare.git");
    std::fs::create_dir(&bare).unwrap_or_else(|e| panic!("create bare fixture directory: {e}"));
    run_git_ok(&bare, &["init", "--bare"]);
    let saved = snapshot(&bare);
    let directory = crate::paths::data_dir()
        .unwrap_or_else(|e| panic!("resolve data directory: {e}"))
        .join("manager");
    std::fs::create_dir_all(&directory).unwrap_or_else(|e| panic!("create Manager store: {e}"));
    let digest =
        <sha2::Sha256 as sha2::Digest>::digest(saved.repository.to_string_lossy().as_bytes());
    let mut name = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(name, "{byte:02x}");
    }
    let record = directory.join(format!("{name}.json"));
    let bytes = serde_json::to_vec(&saved).unwrap_or_else(|e| panic!("serialize snapshot: {e}"));
    std::fs::write(&record, &bytes).unwrap_or_else(|e| panic!("write bare record: {e}"));
    for htmx in [false, true] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/webui/manager/open")
            .header("host", "localhost:8484")
            .header("cookie", "cruise_webui=test-session")
            .header("content-type", "application/x-www-form-urlencoded");
        if htmx {
            request = request.header("hx-request", "true");
        }
        let response = raw_response(
            &harness,
            request
                .body(Body::from(form_body(&[("path", &bare.to_string_lossy())])))
                .unwrap_or_else(|e| panic!("build bare open request: {e}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response.headers().contains_key("hx-redirect"));
        assert!(!response.headers().contains_key("location"));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|e| panic!("read response body: {e}"));
        assert!(
            std::str::from_utf8(&body)
                .unwrap_or_else(|e| panic!("decode response: {e}"))
                .contains("manager-open-error")
        );
        assert_eq!(
            std::fs::read(&record).unwrap_or_else(|e| panic!("read bare record: {e}")),
            bytes
        );
    }
}

#[tokio::test]
async fn manager_detail_displays_reviewed_proposal_and_revision() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let pending = proposal(&repo);
    let key = key(&repo);
    let (status, _, html) = get(&harness, &format!("/manager/{key}"), &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let mut visible = vec![
        pending.objective.as_str(),
        pending.target_sha.as_str(),
        pending.criteria[0].description.as_str(),
        pending.exclusions[0].as_str(),
    ];
    visible.extend(pending.tasks.iter().map(|task| task.id.as_str()));
    visible.extend(
        pending
            .checks
            .iter()
            .flat_map(|check| check.argv.iter().map(String::as_str)),
    );
    for value in visible {
        assert!(html.contains(value), "missing {value:?}: {html}");
    }
    for (name, value) in [
        ("proposal_id", pending.id.clone()),
        ("revision", pending.revision.to_string()),
    ] {
        let hidden_field = html.split('<').find(|tag| {
            tag.starts_with("input ")
                && tag.contains("type=\"hidden\"")
                && tag.contains(&format!("name=\"{name}\""))
        });
        assert!(
            hidden_field.is_some_and(|tag| tag
                .split('>')
                .next()
                .is_some_and(|attrs| attrs.contains(&format!("value=\"{value}\"")))),
            "missing hidden {name}={value}: {html}"
        );
    }
    let task_list = |html: &str, heading: &str| {
        let list = html
            .split_once(heading)
            .unwrap_or_else(|| panic!("missing task heading"))
            .1
            .split_once("</ul>")
            .unwrap_or_else(|| panic!("missing task list terminator"))
            .0;
        let first = list
            .find(
                "<strong>implement-feature</strong><p class=\"whitespace-pre-wrap\">Implement</p>",
            )
            .unwrap_or_else(|| panic!("missing implementation task"));
        let second = list
            .find("<strong>verify-feature</strong><p class=\"whitespace-pre-wrap\">Verify</p>")
            .unwrap_or_else(|| panic!("missing verification task"));
        let dependency = list
            .find("<p>Dependencies: <code class=\"mr-2\">implement-feature</code>")
            .unwrap_or_else(|| panic!("missing task dependency"));
        assert!(first < second && second < dependency, "{list}");
    };
    task_list(&html, "<h3 class=\"font-semibold\">Tasks</h3>");
    manager::approve(&repo, &pending.id, pending.revision)
        .unwrap_or_else(|e| panic!("approve proposal: {e}"));
    let (status, _, html) = get(&harness, &format!("/manager/{key}"), &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    task_list(&html, "<h4 class=\"font-semibold\">Tasks</h4>");
}

#[tokio::test]
async fn manager_approve_rejects_stale_revision_without_creating_goal() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let pending = proposal(&repo);
    let uri = format!("/webui/manager/{}/approve", key(&repo));
    let (status, ct, html) = submit(
        &harness,
        &uri,
        &[
            ("proposal_id", &pending.id),
            ("revision", &(pending.revision + 1).to_string()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{html}");
    fragment(&ct, &html, "stale or unknown manager proposal");
    assert!(snapshot(&repo).goals.is_empty());
}

#[tokio::test]
async fn manager_approve_current_revision_creates_active_goal_with_cookie_only() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let pending = proposal(&repo);
    let uri = format!("/webui/manager/{}/approve", key(&repo));
    let (status, ct, html) = submit(
        &harness,
        &uri,
        &[
            ("proposal_id", &pending.id),
            ("revision", &pending.revision.to_string()),
        ],
    )
    .await;
    assert!(status.is_success(), "{status}: {html}");
    fragment(&ct, &html, "Active");
    let saved = snapshot(&repo);
    assert_eq!(saved.goals.len(), 1);
    assert_eq!(saved.goals[0].status, GoalStatus::Active);
    assert_eq!(saved.goals[0].proposal.id, pending.id);
    assert_eq!(saved.goals[0].proposal.revision, pending.revision);
    assert!(
        !html.contains("test-secret"),
        "bearer token leaked into WebUI"
    );
}

#[tokio::test]
async fn manager_pause_and_resume_restore_prior_goal_state() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let goal = goal(&repo);
    let key = key(&repo);
    let base = format!("/webui/manager/{key}");
    let controls = |html: &str, pause, resume| {
        assert_eq!(
            html.contains(&format!("hx-post=\"{base}/pause\"")),
            pause,
            "{html}"
        );
        assert_eq!(
            html.contains(&format!("hx-post=\"{base}/resume\"")),
            resume,
            "{html}"
        );
    };
    for (action, label, expected) in [
        ("pause", "Paused", GoalStatus::Paused),
        ("resume", "Active", GoalStatus::Active),
    ] {
        let (status, ct, html) = submit(
            &harness,
            &format!("{base}/{action}"),
            &[("goal_id", &goal.id)],
        )
        .await;
        assert!(status.is_success(), "{status}: {html}");
        fragment(&ct, &html, label);
        assert_eq!(snapshot(&repo).goals[0].status, expected);
        controls(
            &html,
            expected == GoalStatus::Active,
            expected == GoalStatus::Paused,
        );
    }
    manager::update_goal(&repo, &goal.id, |goal| {
        goal.status = GoalStatus::Completed;
        Ok(())
    })
    .unwrap_or_else(|e| panic!("complete fixture goal: {e}"));
    let (status, _, html) = get(&harness, &format!("/manager/{key}"), &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    controls(&html, false, false);
}

#[tokio::test]
async fn manager_pause_unknown_goal_returns_inline_error() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    proposal(&repo);
    let (status, ct, html) = submit(
        &harness,
        &format!("/webui/manager/{}/pause", key(&repo)),
        &[("goal_id", "unknown-goal")],
    )
    .await;
    assert!(status.is_client_error(), "{status}: {html}");
    fragment(&ct, &html, "unknown Manager goal");
}

async fn check_corrective_approval(
    harness: &Harness,
    repo: &Path,
    blocked: &Goal,
    preview: &manager::CorrectivePreview,
    base: &str,
    task_id: &str,
) {
    for (revision, message) in [
        (None, "Missing form field: revision"),
        (Some("oops"), "Invalid revision"),
    ] {
        let mut fields = vec![
            ("goal_id", blocked.id.as_str()),
            ("preview_id", preview.id.as_str()),
        ];
        if let Some(revision) = revision {
            fields.push(("revision", revision));
        }
        let (status, ct, html) =
            submit(harness, &format!("{base}/approve-corrective"), &fields).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{html}");
        fragment(&ct, &html, message);
        let saved = snapshot(repo);
        assert!(saved.goals[0].execution.corrective_approvals.is_empty());
    }
    for (revision, current) in [(preview.revision + 1, false), (preview.revision, true)] {
        let (status, ct, html) = submit(
            harness,
            &format!("{base}/approve-corrective"),
            &[
                ("goal_id", &blocked.id),
                ("preview_id", &preview.id),
                ("revision", &revision.to_string()),
            ],
        )
        .await;
        let stored = &snapshot(repo).goals[0];
        if current {
            assert!(status.is_success(), "{status}: {html}");
            fragment(&ct, &html, task_id);
            assert_eq!(stored.status, GoalStatus::Active);
            assert!(stored.proposal.tasks.iter().any(|task| task.id == task_id));
        } else {
            assert_eq!(status, StatusCode::CONFLICT, "{html}");
            fragment(&ct, &html, "stale or unknown corrective preview");
            assert_eq!(stored.status, GoalStatus::Blocked);
            assert_eq!(stored.proposal.tasks.len(), blocked.proposal.tasks.len());
        }
    }
}

#[tokio::test]
async fn manager_corrective_review_requires_the_integrated_tip_and_current_preview() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let blocked = blocked_verification(&repo);
    let key = key(&repo);
    let base = format!("/webui/manager/{key}");
    let task_id = "correct-failed-check";
    let tip = blocked.proposal.target_sha.as_str();
    let (status, _, html) = get(&harness, &format!("/manager/{key}"), &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&format!("hx-post=\"{base}/preview-corrective\"")),
        "{html}"
    );
    for (expected_tip, current) in [("stale-tip", false), (tip, true)] {
        let (status, ct, html) = submit(
            &harness,
            &format!("{base}/preview-corrective"),
            &[
                ("goal_id", &blocked.id),
                ("expected_tip", expected_tip),
                ("task_id", task_id),
                ("instructions", "Correct the failed verification"),
            ],
        )
        .await;
        let stored = &snapshot(&repo).goals[0];
        assert_eq!(stored.status, GoalStatus::Blocked);
        assert_eq!(stored.proposal.tasks.len(), blocked.proposal.tasks.len());
        if current {
            assert!(status.is_success(), "{status}: {html}");
            fragment(&ct, &html, task_id);
            let preview = stored
                .execution
                .corrective_preview
                .as_ref()
                .unwrap_or_else(|| panic!("missing corrective preview"));
            assert!(
                html.contains(&format!("name=\"revision\" value=\"{}\"", preview.revision)),
                "{html}"
            );
        } else {
            assert_eq!(status, StatusCode::CONFLICT, "{html}");
            fragment(&ct, &html, "tip changed");
            assert!(stored.execution.corrective_preview.is_none());
        }
    }
    let preview = snapshot(&repo).goals[0]
        .execution
        .corrective_preview
        .clone()
        .unwrap_or_else(|| panic!("missing reviewed corrective preview"));
    for (id, instructions) in [
        (blocked.proposal.tasks[0].id.as_str(), "Duplicate task"),
        (task_id, "  "),
    ] {
        let (status, ct, html) = submit(
            &harness,
            &format!("{base}/preview-corrective"),
            &[
                ("goal_id", &blocked.id),
                ("expected_tip", tip),
                ("task_id", id),
                ("instructions", instructions),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{html}");
        fragment(&ct, &html, "invalid corrective task or dependencies");
        assert!(html.contains("manager-action-error"), "{html}");
        assert_eq!(
            serde_json::to_value(&snapshot(&repo).goals[0].execution.corrective_preview)
                .unwrap_or_else(|e| panic!("serialize saved preview: {e}")),
            serde_json::to_value(&preview)
                .unwrap_or_else(|e| panic!("serialize expected preview: {e}")),
        );
    }
    check_corrective_approval(&harness, &repo, &blocked, &preview, &base, task_id).await;
}

fn save_retry_preview(repo: &Path, blocked: &Goal) -> manager::CorrectivePreview {
    manager::preview_corrective(
        repo,
        &blocked.id,
        &blocked.proposal.target_sha,
        Task {
            id: "repair-verification".into(),
            instructions: "Repair the failed verification".into(),
            dependencies: vec![blocked.proposal.tasks[0].id.clone()],
        },
    )
    .unwrap_or_else(|e| panic!("save corrective preview: {e}"))
}

fn retry_blocked_verification(repo: &Path, blocked: &Goal) {
    crate::manager_publication::retry_verification(repo, &blocked.id)
        .unwrap_or_else(|e| panic!("retry verification: {e}"));
}

fn restore_failed_verification(repo: &Path, blocked: &Goal) {
    // Represent the next failed verification at the same tip, without replacing
    // any history written by the retry under test.
    manager::update_goal(repo, &blocked.id, |goal| {
        goal.status = GoalStatus::Blocked;
        goal.reason.clone_from(&blocked.reason);
        goal.execution
            .publication
            .clone_from(&blocked.execution.publication);
        Ok(())
    })
    .unwrap_or_else(|e| panic!("seed second failed verification: {e}"));
}

fn assert_stale_retry_preview(repo: &Path, preview: &manager::CorrectivePreview) {
    let before = serde_json::json!(snapshot(repo).goals[0]);
    let result = manager::approve_corrective(repo, &preview.goal_id, &preview.id, preview.revision);
    assert!(
        matches!(&result, Err(crate::error::CruiseError::Busy(message))
            if message.contains("stale or unknown corrective preview")),
        "expected stale preview conflict, got {result:?}"
    );
    assert_eq!(serde_json::json!(snapshot(repo).goals[0]), before);
}

#[test]
fn manager_retry_rejected_leaves_corrective_preview_and_goal_unchanged() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let blocked = blocked_verification(&repo);
    save_retry_preview(&repo, &blocked);
    manager::update_goal(&repo, &blocked.id, |goal| {
        goal.status = GoalStatus::Paused;
        Ok(())
    })
    .unwrap_or_else(|e| panic!("pause seeded goal: {e}"));
    let before = serde_json::json!(snapshot(&repo).goals[0]);
    assert!(crate::manager_publication::retry_verification(&repo, &blocked.id).is_err());
    assert_eq!(serde_json::json!(snapshot(&repo).goals[0]), before);
}

#[test]
fn manager_retry_corrective_preview_cannot_be_approved_even_after_same_tip_fails() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let blocked = blocked_verification(&repo);
    let preview = save_retry_preview(&repo, &blocked);
    retry_blocked_verification(&repo, &blocked);
    restore_failed_verification(&repo, &blocked);
    assert_stale_retry_preview(&repo, &preview);
}

#[test]
fn manager_retry_corrective_preview_advances_revision_and_allows_fresh_approval() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let blocked = blocked_verification(&repo);
    let discarded = save_retry_preview(&repo, &blocked);
    retry_blocked_verification(&repo, &blocked);
    restore_failed_verification(&repo, &blocked);
    let fresh = save_retry_preview(&repo, &blocked);
    assert_ne!(fresh.id, discarded.id);
    assert_eq!(fresh.revision, discarded.revision + 1);
    assert_stale_retry_preview(&repo, &discarded);
    let approved = manager::approve_corrective(&repo, &blocked.id, &fresh.id, fresh.revision)
        .unwrap_or_else(|e| panic!("approve fresh corrective preview: {e}"));
    let manager::Correction::Task(task) = &fresh.correction else {
        panic!("expected a corrective task preview");
    };
    let mut tasks = blocked.proposal.tasks;
    tasks.push(task.clone());
    assert_eq!(
        serde_json::json!(approved.proposal.tasks),
        serde_json::json!(tasks)
    );
    assert_eq!(
        serde_json::json!(approved.execution.corrective_approvals),
        serde_json::json!([fresh])
    );
    assert_stale_retry_preview(&repo, &discarded);
}

#[tokio::test]
async fn manager_retry_distinguishes_task_from_verification_scope() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let blocked = blocked_verification(&repo);
    let preview = save_retry_preview(&repo, &blocked);
    let key = key(&repo);
    let uri = format!("/webui/manager/{key}/retry");
    let page = format!("/manager/{key}");
    let dialog = format!("manager-approve-corrective-{}", preview.id);
    let heading = "Corrective task preview";
    let (status, _, html) = get(&harness, &page, &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(&dialog) && html.contains(heading), "{html}");
    let (status, ct, html) = submit(
        &harness,
        &uri,
        &[
            ("goal_id", &blocked.id),
            ("task_id", &blocked.proposal.tasks[0].id),
        ],
    )
    .await;
    assert!(!status.is_success(), "{status}: {html}");
    fragment(&ct, &html, "already integrated");
    assert_eq!(snapshot(&repo).goals[0].status, GoalStatus::Blocked);
    let (status, ct, html) = submit(&harness, &uri, &[("goal_id", &blocked.id)]).await;
    assert!(status.is_success(), "{status}: {html}");
    fragment(&ct, &html, "Active");
    let persisted = &snapshot(&repo).goals[0];
    assert_eq!(persisted.status, GoalStatus::Active);
    assert!(persisted.execution.publication.is_none());
    assert_eq!(
        serde_json::json!(persisted.execution.publication_history),
        serde_json::json!([blocked.execution.publication])
    );
    let manager::Correction::Task(task) = &preview.correction else {
        panic!("expected a corrective task preview");
    };
    let (status, _, html) = get(&harness, &page, &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        !html.contains(&dialog) && !html.contains(heading) && !html.contains(&task.instructions),
        "discarded corrective preview is still visible: {html}"
    );
    assert_stale_retry_preview(&repo, &preview);
}

#[tokio::test]
async fn manager_resume_task_preserves_the_failed_attempt_identity() {
    check_child_action("resume-task", false).await;
}

#[tokio::test]
async fn manager_task_retry_appends_a_fresh_attempt() {
    check_child_action("retry", false).await;
}

#[tokio::test]
async fn manager_conflicting_completed_child_can_retry_while_resume_is_starting() {
    check_child_action("retry", true).await;
}

fn seed_failed_child(
    harness: &Harness,
    repo: &Path,
    active: &Goal,
    resume_starting: bool,
) -> crate::session::SessionState {
    let task = &active.proposal.tasks[0];
    let tip = &active.proposal.target_sha;
    let goal_ref = format!("refs/heads/cruise/goal/{}", active.id);
    run_git_ok(repo, &["update-ref", &goal_ref, tip]);
    let mut child = create_session(harness);
    let worktree = harness.manager.worktrees_dir().join(&child.id);
    std::fs::create_dir_all(harness.manager.worktrees_dir())
        .unwrap_or_else(|e| panic!("create worktree directory: {e}"));
    run_git_ok(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            worktree
                .to_str()
                .unwrap_or_else(|| panic!("non-UTF-8 worktree path")),
            tip,
        ],
    );
    child.worktree_path = Some(worktree.clone());
    child.phase = crate::session::SessionPhase::Failed("fixture failed".into());
    child.managed_child = Some(crate::session::ManagedChildOwner {
        goal_id: active.id.clone(),
        task_id: task.id.clone(),
        attempt: 1,
        revision: active.proposal.revision,
        base_sha: tip.clone(),
    });
    harness
        .manager
        .save(&child)
        .unwrap_or_else(|e| panic!("save child: {e}"));
    manager::update_goal(repo, &active.id, |goal| {
        goal.execution.anchor = Some(repo.canonicalize()?);
        goal.execution.integrated_tip = Some(tip.clone());
        goal.execution.attempts = serde_json::from_value(serde_json::json!([{
            "task_id": task.id, "attempt": 1, "session_id": child.id,
            "base_sha": tip, "launch_started": true, "resume_starting": resume_starting,
        }]))?;
        Ok(())
    })
    .unwrap_or_else(|e| panic!("seed attempt: {e}"));
    child
}

fn seed_child_conflict(
    harness: &Harness,
    repo: &Path,
    active: &Goal,
    child: &mut crate::session::SessionState,
) {
    let tip = &active.proposal.target_sha;
    let goal_ref = format!("refs/heads/cruise/goal/{}", active.id);
    let worktree = harness.manager.worktrees_dir().join(&child.id);
    let commit = |path: &Path, contents: &str| {
        std::fs::write(path.join("README.md"), contents)
            .unwrap_or_else(|e| panic!("write fixture README: {e}"));
        run_git_ok(path, &["add", "README.md"]);
        run_git_ok(path, &["commit", "-m", contents]);
        let head = std::process::Command::new("git")
            .current_dir(path)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap_or_else(|e| panic!("read Git HEAD: {e}"));
        assert!(head.status.success());
        String::from_utf8(head.stdout)
            .unwrap_or_else(|e| panic!("decode Git HEAD: {e}"))
            .trim()
            .to_owned()
    };
    let result = commit(&worktree, "child change");
    let goal_tip = commit(repo, "conflicting goal change");
    run_git_ok(repo, &["update-ref", &goal_ref, &goal_tip, tip]);
    let conflict =
        crate::manager_git::prepare_integration(repo, &goal_ref, &goal_tip, tip, &result)
            .err()
            .unwrap_or_else(|| panic!("fixture integration should conflict"));
    child.phase = crate::session::SessionPhase::Completed;
    harness
        .manager
        .save(child)
        .unwrap_or_else(|e| panic!("save completed child: {e}"));
    manager::update_goal(repo, &active.id, |goal| {
        goal.status = GoalStatus::Blocked;
        goal.reason = Some(format!(
            "Child {} integration requires attention: {conflict}",
            child.id
        ));
        goal.execution.integrated_tip = Some(goal_tip);
        goal.execution.attempts[0].result_sha = Some(result);
        Ok(())
    })
    .unwrap_or_else(|e| panic!("seed blocked goal: {e}"));
}

async fn check_child_action(action: &str, resume_starting: bool) {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let active = goal(&repo);
    let mut child = seed_failed_child(&harness, &repo, &active, resume_starting);
    let task = &active.proposal.tasks[0];
    let key = key(&repo);
    let detail = format!("/manager/{key}");
    if resume_starting {
        let (status, _, html) = get(&harness, &detail, &[]).await;
        assert_eq!(status, StatusCode::OK, "{html}");
        for action in ["resume-task", "retry"] {
            assert!(
                !html.contains(&format!("hx-post=\"/webui/manager/{key}/{action}\"")),
                "{html}"
            );
        }

        seed_child_conflict(&harness, &repo, &active, &mut child);
    }
    let previous = serde_json::to_value(&snapshot(&repo).goals[0].execution.attempts[0])
        .unwrap_or_else(|e| panic!("serialize original attempt: {e}"));
    let uri = format!("/webui/manager/{key}/{action}");
    let (status, _, html) = get(&harness, &detail, &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    if resume_starting {
        assert!(
            !html.contains(&format!("hx-post=\"/webui/manager/{key}/resume-task\"")),
            "{html}"
        );
    }
    let form = html
        .split(&format!("hx-post=\"{uri}\""))
        .skip(1)
        .filter_map(|rest| rest.split("</form>").next())
        .find(|form| form.contains("name=\"task_id\""))
        .unwrap_or_else(|| panic!("missing task action form"));
    let value = |name| {
        form.split_once(&format!("name=\"{name}\" value=\""))
            .unwrap_or_else(|| panic!("missing hidden form field"))
            .1
            .split('"')
            .next()
            .unwrap_or_else(|| panic!("missing form value"))
    };
    assert_eq!(value("goal_id"), active.id);
    assert_eq!(value("task_id"), task.id);
    let _resume_queue = (action == "resume-task")
        .then(crate::manager_coordinator::accept_resume_requests_without_dispatch);
    let (status, ct, html) = submit(
        &harness,
        &uri,
        &[("goal_id", value("goal_id")), ("task_id", value("task_id"))],
    )
    .await;
    assert!(status.is_success(), "{status}: {html}");
    fragment(&ct, &html, "Active");
    let persisted = snapshot(&repo);
    assert_eq!(persisted.goals[0].status, GoalStatus::Active);
    let attempts = &persisted.goals[0].execution.attempts;
    assert_eq!(attempts[0].attempt, 1);
    assert_eq!(attempts[0].session_id, child.id);
    assert_eq!(attempts[0].resume_pending, action == "resume-task");
    if action == "resume-task" {
        assert_eq!(attempts.len(), 1);
    } else {
        assert_eq!(
            serde_json::to_value(&attempts[0])
                .unwrap_or_else(|e| panic!("serialize persisted attempt: {e}")),
            previous
        );
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[1].attempt, 2);
        assert_ne!(attempts[1].session_id, child.id);
    }
}

#[tokio::test]
async fn manager_answer_unknown_prompt_returns_conflict_fragment() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let goal = goal(&repo);
    let (status, ct, html) = submit(
        &harness,
        &format!("/webui/manager/{}/answer", key(&repo)),
        &[
            ("goal_id", &goal.id),
            ("prompt_id", "unknown-prompt"),
            ("answer", "Yes"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{html}");
    fragment(&ct, &html, "stale or unknown");
}

async fn answer_ask_in_browser(
    harness: &Harness,
    key: &str,
    prompt_id: &str,
    answer: &str,
) -> Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("bind browser fixture: {e}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("fixture address: {e}"));
    let app = crate::daemon::server::application_router(harness.state.clone(), true);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let browser = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        tokio::process::Command::new("node")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args([
                "tests/webui/manager-ask.mjs",
                &format!("http://{address}/manager/{key}"),
                prompt_id,
                answer,
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await;
    server.abort();
    browser
}

fn seed_ask_child(
    harness: &Harness,
    repo: &Path,
    goal: &Goal,
) -> (
    crate::session::SessionState,
    crate::session::ManagedChildOwner,
) {
    let mut child = create_session(harness);
    let owner = crate::session::ManagedChildOwner {
        goal_id: goal.id.clone(),
        task_id: goal.proposal.tasks[0].id.clone(),
        attempt: 1,
        revision: goal.proposal.revision,
        base_sha: goal.proposal.target_sha.clone(),
    };
    child.managed_child = Some(owner.clone());
    harness
        .manager
        .save(&child)
        .unwrap_or_else(|e| panic!("save owned child: {e}"));
    manager::update_goal(repo, &goal.id, |goal| {
        goal.execution
            .attempts
            .push(serde_json::from_value(serde_json::json!({
                "task_id": owner.task_id, "attempt": owner.attempt, "session_id": child.id,
                "base_sha": owner.base_sha, "launch_started": true, "integrated": false,
            }))?);
        Ok(())
    })
    .unwrap_or_else(|e| panic!("seed owned attempt: {e}"));
    (child, owner)
}

#[tokio::test]
async fn manager_answer_keeps_live_ask_pending_until_valid_reply() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let goal = goal(&repo);
    let (child, owner) = seed_ask_child(&harness, &repo, &goal);
    let runtime = harness.state.application.runtime();
    let ask = crate::application::test_managed_prompt::start(&runtime, &child.id, &owner)
        .unwrap_or_else(|e| panic!("start managed Ask: {e}"));
    let prompt_id = ask
        .request_id
        .await
        .unwrap_or_else(|e| panic!("register managed Ask: {e}"));
    let key = key(&repo);
    let answer = "Use the isolated fixture";
    if std::env::var_os("WEBUI_MANAGER_BROWSER_ASK").is_some() {
        let browser = answer_ask_in_browser(&harness, &key, &prompt_id, answer).await;
        let success = browser
            .as_ref()
            .is_ok_and(|result| result.as_ref().is_ok_and(|output| output.status.success()));
        if !success {
            ask.cancel.cancel();
        }
        let delivered = tokio::time::timeout(std::time::Duration::from_secs(2), ask.result).await;
        ask.cancel.cancel();
        let output = browser
            .unwrap_or_else(|e| panic!("Chromium timed out: {e}"))
            .unwrap_or_else(|e| panic!("launch Node/Playwright: {e}"));
        assert!(success, "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(
            delivered
                .ok()
                .and_then(Result::ok)
                .and_then(Result::ok)
                .as_deref(),
            Some(answer)
        );
        return;
    }
    let uri = format!("/webui/manager/{key}/answer");
    let blank = submit(
        &harness,
        &uri,
        &[
            ("goal_id", &goal.id),
            ("prompt_id", &prompt_id),
            ("answer", " "),
        ],
    )
    .await;
    let still_pending = runtime
        .pending_prompts(&child.id)
        .iter()
        .any(|prompt| prompt.request_id == prompt_id);
    let valid = submit(
        &harness,
        &uri,
        &[
            ("goal_id", &goal.id),
            ("prompt_id", &prompt_id),
            ("answer", answer),
        ],
    )
    .await;
    let delivered = if valid.0.is_success() {
        tokio::time::timeout(std::time::Duration::from_secs(2), ask.result)
            .await
            .ok()
    } else {
        ask.cancel.cancel();
        Some(ask.result.await)
    };
    ask.cancel.cancel();
    assert_eq!(blank.0, StatusCode::BAD_REQUEST, "{}", blank.2);
    fragment(&blank.1, &blank.2, "must not be empty");
    assert!(still_pending, "blank answer consumed the live Ask");
    assert!(valid.0.is_success(), "{}: {}", valid.0, valid.2);
    assert_eq!(
        delivered
            .and_then(Result::ok)
            .and_then(Result::ok)
            .as_deref(),
        Some(answer)
    );
    assert!(runtime.pending_prompts(&child.id).is_empty());
}

#[tokio::test]
async fn manager_answer_option_rejects_stale_prompt_inline() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let goal = goal(&repo);
    let uri = format!("/webui/manager/{}/answer-option", key(&repo));
    let (status, ct, html) = submit(
        &harness,
        &uri,
        &[
            ("goal_id", &goal.id),
            ("prompt_id", "expired-option"),
            ("nextStep", "chosen-step"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{html}");
    fragment(&ct, &html, "stale or unknown");
}

#[tokio::test]
async fn manager_resume_task_does_not_resume_unknown_task() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let goal = goal(&repo);
    let uri = format!("/webui/manager/{}/resume-task", key(&repo));
    let (status, ct, html) = submit(
        &harness,
        &uri,
        &[("goal_id", &goal.id), ("task_id", "unknown-task")],
    )
    .await;
    assert!(!status.is_success(), "{status}: {html}");
    fragment(&ct, &html, "unknown Manager task");
    assert!(snapshot(&repo).goals[0].execution.attempts.is_empty());
}

#[tokio::test]
async fn manager_conversation_actions_while_turn_in_flight_are_busy_without_appending() {
    // Other daemon tests can permanently cancel the process-wide Manager token.
    // In a fresh test process, exercise the real provider admission path unchanged.
    const CHILD: &str = "CRUISE_TEST_MANAGER_BUSY_CHILD";
    const TEST: &str = "webui::tests::manager::manager_conversation_actions_while_turn_in_flight_are_busy_without_appending";
    if std::env::var_os(CHILD).is_none() {
        let exe = std::env::current_exe().unwrap_or_else(|e| panic!("locate test binary: {e}"));
        let output = std::process::Command::new(exe)
            .args(["--exact", TEST])
            .env(CHILD, "1")
            .output()
            .unwrap_or_else(|e| panic!("run isolated Manager test: {e}"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "isolated Manager test: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let cwd = repo.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (finish, done) = std::sync::mpsc::channel::<()>();
    // Enter a separate current-thread runtime without driving its worker. This
    // leaves a real accepted turn in flight deterministically, with no model call.
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("conversation runtime: {e}"));
        let accepted = {
            let _enter = runtime.enter();
            crate::manager_provider::message(&cwd, "First turn".into(), Vec::new())
        };
        let _ = started.send(accepted);
        let _ = done.recv();
    });
    let first = ready
        .await
        .unwrap_or_else(|e| panic!("start conversation: {e}"))
        .unwrap_or_else(|e| panic!("accept first turn: {e}"));
    let base = format!("/webui/manager/{}", key(&repo));
    let message = submit(
        &harness,
        &format!("{base}/message"),
        &[("text", "Concurrent turn")],
    )
    .await;
    let propose = submit(&harness, &format!("{base}/propose"), &[]).await;
    drop(finish);
    assert!(
        worker.join().is_ok(),
        "conversation worker did not shut down"
    );
    for (action, (status, ct, html)) in [("message", message), ("propose", propose)] {
        assert_eq!(status, StatusCode::CONFLICT, "{action}: {html}");
        fragment(&ct, &html, "already running");
    }
    let saved = snapshot(&repo);
    assert_eq!(saved.transcript.len(), 1);
    assert_eq!(saved.transcript[0].id, first.id);
}

#[tokio::test]
async fn manager_startup_failure_renders_inline_and_preserves_pending_proposal() {
    let mut harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let pending = proposal(&repo);
    let key = key(&repo);
    let before = serde_json::to_value(snapshot(&repo))
        .unwrap_or_else(|e| panic!("serialize initial snapshot: {e}"));
    let corrupt = harness
        .dir
        .path()
        .join("xdg-data/cruise/manager/corrupt.json");
    std::fs::write(&corrupt, "{invalid json")
        .unwrap_or_else(|e| panic!("write corrupt record: {e}"));
    assert!(manager::snapshots().is_err());
    harness.state.manager_startup_error = Some("test recovery failure".into());
    let (status, ct, html) = submit(
        &harness,
        &format!("/webui/manager/{key}/approve"),
        &[
            ("proposal_id", &pending.id),
            ("revision", &pending.revision.to_string()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{html}");
    fragment(
        &ct,
        &html,
        "Manager disabled after startup recovery failure: test recovery failure",
    );
    assert!(html.contains("manager-action-error"), "{html}");
    assert_eq!(
        serde_json::to_value(snapshot(&repo))
            .unwrap_or_else(|e| panic!("serialize final snapshot: {e}")),
        before
    );
}

#[tokio::test]
async fn manager_routes_require_web_session_and_same_origin_mutations() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let pending = proposal(&repo);
    let key = key(&repo);
    for uri in [
        "/manager".to_owned(),
        format!("/manager/{key}"),
        format!("/webui/manager/{key}/sync"),
    ] {
        let request = Request::builder()
            .uri(&uri)
            .header("host", "localhost:8484")
            .header("authorization", "Bearer test-secret")
            .body(Body::empty())
            .unwrap_or_else(|e| panic!("{uri}: {e}"));
        assert_eq!(
            raw_response(&harness, request).await.status(),
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    let uri = format!("/webui/manager/{key}/approve");
    let app = crate::daemon::server::application_router(harness.state.clone(), true);
    for (headers, expected) in [
        (
            [
                ("authorization", "Bearer test-secret"),
                ("origin", "http://localhost:8484"),
            ],
            StatusCode::UNAUTHORIZED,
        ),
        (
            [
                ("cookie", "cruise_webui=test-session"),
                ("origin", "http://evil.example:8484"),
            ],
            StatusCode::FORBIDDEN,
        ),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri(&uri)
            .header("host", "localhost:8484")
            .header(headers[0].0, headers[0].1)
            .header(headers[1].0, headers[1].1)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(form_body(&[
                ("proposal_id", &pending.id),
                ("revision", &pending.revision.to_string()),
            ])))
            .unwrap_or_else(|e| panic!("approve: {e}"));
        let response = app
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {});
        assert_eq!(response.status(), expected);
        assert!(snapshot(&repo).goals.is_empty());
    }
}

#[tokio::test]
async fn manager_sync_returns_new_conversation_without_reload() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    proposal(&repo);
    let key = key(&repo);
    let uri = format!("/webui/manager/{key}/sync");
    let (status, _, html) = get(&harness, &format!("/manager/{key}"), &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(&uri) && html.contains("every 3s"), "{html}");
    let text = "A transcript update from outside the browser";
    manager::message(&repo, text.into()).unwrap_or_else(|e| panic!("seed turn: {e}"));
    let (status, ct, html) = get(&harness, &uri, &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    fragment(&ct, &html, text);
}

#[tokio::test]
async fn manager_child_links_to_its_goal_repository_and_remains_read_only() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let goal = goal(&repo);
    let mut child = create_session(&harness);
    child.phase = crate::session::SessionPhase::Suspended;
    child.managed_child = Some(crate::session::ManagedChildOwner {
        goal_id: goal.id,
        task_id: goal.proposal.tasks[0].id.clone(),
        attempt: 1,
        revision: goal.proposal.revision,
        base_sha: goal.proposal.target_sha,
    });
    harness
        .manager
        .save(&child)
        .unwrap_or_else(|e| panic!("save child: {e}"));
    let directory = crate::paths::data_dir()
        .unwrap_or_else(|e| panic!("resolve data directory: {e}"))
        .join("manager");
    std::fs::write(directory.join("corrupt.json"), "{invalid json")
        .unwrap_or_else(|e| panic!("write corrupt record: {e}"));
    assert!(manager::snapshots().is_err());
    let key = key(&repo);
    for uri in [
        "/manager".into(),
        format!("/manager/{key}"),
        format!("/webui/manager/{key}/sync"),
    ] {
        let (status, _, html) = get(&harness, &uri, &[]).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {html}");
        assert!(
            html.contains(&key) || html.contains("Deliver the reviewable feature"),
            "{uri}: {html}"
        );
    }
    let unlisted = repo_key(&repo.join("unlisted"));
    assert_eq!(
        get(&harness, &format!("/manager/{unlisted}"), &[]).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, _, html) = get(&harness, &format!("/sessions/{}", child.id), &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("read-only"), "{html}");
    for action in ["run", "editor/tell", "reset"] {
        assert!(
            !html.contains(&format!("/webui/sessions/{}/{action}\"", child.id)),
            "{html}"
        );
    }
    assert!(
        html.contains("Open in Manager") && html.contains(&format!("href=\"/manager/{key}\"")),
        "{html}"
    );
}

#[tokio::test]
async fn manager_page_reviews_and_approves_a_contract_amendment() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let blocked = blocked_verification(&repo);
    let criterion = &blocked.proposal.criteria[0];
    let edit = manager::ContractEdit {
        target: manager::ContractTarget::Criterion(criterion.id.clone()),
        old_sha256: String::new(),
        replace: criterion.description.clone(),
        with: "Narrowed criterion".into(),
    };
    let preview = manager::amendment(&blocked, vec![edit])
        .and_then(|correction| {
            let tip = &blocked.proposal.target_sha;
            manager::preview_correction(&repo, &blocked.id, tip, 0, correction)
        })
        .unwrap_or_else(|e| panic!("save amendment preview: {e}"));
    let key = key(&repo);

    let (status, _, html) = get(&harness, &format!("/manager/{key}"), &[]).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let dialog = format!("manager-approve-corrective-{}", preview.id);
    assert!(
        html.contains("+ Narrowed criterion") && html.contains(&dialog),
        "{html}"
    );

    let (status, _, html) = submit(
        &harness,
        &format!("/webui/manager/{key}/approve-corrective"),
        &[
            ("goal_id", &blocked.id),
            ("preview_id", &preview.id),
            ("revision", &preview.revision.to_string()),
        ],
    )
    .await;
    assert!(status.is_success(), "{status}: {html}");
    let stored = &snapshot(&repo).goals[0];
    assert_eq!(stored.contract_revision(), 1);
    assert_eq!(
        serde_json::json!(stored.proposal),
        serde_json::json!(blocked.proposal)
    );
    assert_eq!(stored.status, GoalStatus::Blocked);
}

#[tokio::test]
async fn manager_page_displays_pinned_retarget_and_hides_conflicting_approval() {
    let harness = harness();
    let _data = isolate_records(&harness);
    let repo = repository(&harness);
    let blocked = blocked_verification(&repo);
    let key = key(&repo);
    for conflict in [None, Some("CONFLICT in src/lib.rs")] {
        manager::update_goal(&repo, &blocked.id, |goal| {
            goal.proposal.objective = "Long objective ".repeat(20);
            let rejection = r#"PR create outcome uncertain (Err("gh failed: GraphQL: Title is too long (maximum is 256 characters) (createPullRequest)"), lookup Ok(None)). Human resolution required"#;
            goal.reason = Some(rejection.into());
            let mut publication = serde_json::to_value(&goal.execution.publication)?;
            publication["phase"] = serde_json::json!("CreateIntent");
            goal.execution.publication = serde_json::from_value(publication)?;
            goal.execution.corrective_preview = Some(manager::CorrectivePreview {
                id: "base-advance".into(),
                revision: 7,
                goal_id: goal.id.clone(),
                integrated_tip: "verified-tip".into(),
                contract_revision: 2,
                correction: manager::Correction::Retarget(manager::RetargetPreview {
                    repository: "github.com/example/project".into(),
                    target_ref: "refs/heads/main".into(),
                    old_base: "previous-base".into(),
                    new_base: "pinned-base".into(),
                    proposal_revision: 1,
                    publication_rejection: rejection.into(),
                    publication_digest: "verification-digest".into(),
                    contract_digest: "contract-digest".into(),
                    github_user_id: 1,
                    commits: "a1b2c3 Advance target".into(),
                    files: "M src/lib.rs".into(),
                    merge_tree: conflict.is_none().then(|| "reviewed-tree".into()),
                    conflict: conflict.map(str::to_owned),
                }),
            });
            Ok(())
        })
        .unwrap_or_else(|e| panic!("seed retarget preview: {e}"));
        let (status, _, html) = get(&harness, &format!("/manager/{key}"), &[]).await;
        assert_eq!(status, StatusCode::OK, "{html}");
        for expected in [
            "github.com/example/project",
            "refs/heads/main",
            "verified-tip",
            "previous-base",
            "pinned-base",
            "Contract revision: 2",
            "verification-digest",
            "a1b2c3 Advance target",
            "M src/lib.rs",
            conflict.unwrap_or("Merge conflicts: none"),
        ] {
            assert!(html.contains(expected), "missing {expected:?}: {html}");
        }
        assert_eq!(
            html.contains("manager-approve-corrective-base-advance"),
            conflict.is_none()
        );
    }
}
