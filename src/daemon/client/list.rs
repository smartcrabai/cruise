use serde::Deserialize;
use serde_json::Value;

use crate::application::{LocalMergeMode, MergePrOutcome, SessionAction, SessionSettingsRequest};
use crate::daemon::client::DaemonClient;
use crate::error::{CruiseError, Result};
use crate::request_context::RequestContext;
use crate::session::SessionState;

pub(crate) use crate::daemon::api::sessions::SessionListItem;

#[derive(Debug, Clone)]
pub(crate) struct SessionDetails {
    pub state: SessionState,
    pub capabilities: Vec<SessionAction>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiffOutput {
    pub branch: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrMergeStatus {
    pub state: String,
    pub mergeable: String,
    pub review_decision: String,
    pub checks: Vec<PrCheckStatus>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PrCheckStatus {
    pub name: String,
    pub status: String,
}

#[derive(Deserialize)]
struct MergePrResponse {
    outcome: String,
}

#[derive(Deserialize)]
struct LocalMergeResponse {
    outcome: String,
    error: Option<String>,
}

impl DaemonClient {
    pub(crate) async fn sessions(&self) -> Result<Vec<SessionListItem>> {
        self.get_json("sessions").await
    }

    pub(crate) async fn session_details(&self, id: &str) -> Result<SessionDetails> {
        let mut value: Value = self.get_json(&format!("sessions/{id}")).await?;
        let capabilities = serde_json::from_value(
            value
                .as_object_mut()
                .and_then(|object| object.remove("capabilities"))
                .unwrap_or(Value::Null),
        )
        .map_err(|error| {
            CruiseError::Other(format!("invalid daemon session capabilities: {error}"))
        })?;
        let state = serde_json::from_value(value).map_err(|error| {
            CruiseError::Other(format!("invalid daemon session response: {error}"))
        })?;
        Ok(SessionDetails {
            state,
            capabilities,
        })
    }

    pub(crate) async fn session_plan(&self, id: &str) -> Result<String> {
        self.get_json(&format!("sessions/{id}/plan")).await
    }

    pub(crate) async fn clean(
        &self,
        _context: &RequestContext,
    ) -> Result<crate::session::CleanupReport> {
        self.post_json("clean", &serde_json::json!({})).await
    }

    pub(crate) async fn diff(&self, id: &str, stat: bool, color: bool) -> Result<DiffOutput> {
        let mut path = format!("sessions/{id}/diff");
        if stat || color {
            path.push('?');
            if stat {
                path.push_str("stat=1");
            }
            if color {
                if stat {
                    path.push('&');
                }
                path.push_str("color=1");
            }
        }
        self.get_json(&path).await
    }

    pub(crate) async fn reset_session(
        &self,
        id: &str,
        _context: &RequestContext,
    ) -> Result<SessionState> {
        self.post_json(&format!("sessions/{id}/reset"), &serde_json::json!({}))
            .await
    }

    pub(crate) async fn delete_session(&self, id: &str, _context: &RequestContext) -> Result<()> {
        let _: Value = self
            .post_json(&format!("sessions/{id}/delete"), &serde_json::json!({}))
            .await?;
        Ok(())
    }

    pub(crate) async fn discard_session(&self, id: &str, context: &RequestContext) -> Result<()> {
        let _: Value = self
            .post_json(
                &format!("sessions/{id}/discard"),
                &serde_json::json!({ "context": context }),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn update_session_settings(
        &self,
        id: &str,
        settings: &SessionSettingsRequest,
        _context: &RequestContext,
    ) -> Result<SessionState> {
        self.post_json(
            &format!("sessions/{id}/settings"),
            &serde_json::json!({ "settings": settings }),
        )
        .await
    }

    pub(crate) async fn inspect_merge(&self, id: &str) -> Result<PrMergeStatus> {
        self.get_json(&format!("sessions/{id}/merge-status")).await
    }

    pub(crate) async fn merge_pr(
        &self,
        id: &str,
        method: crate::pr_merge::PrMergeMethod,
        _context: &RequestContext,
    ) -> Result<MergePrOutcome> {
        let method = match method {
            crate::pr_merge::PrMergeMethod::Merge => "merge",
            crate::pr_merge::PrMergeMethod::Squash => "squash",
            crate::pr_merge::PrMergeMethod::Rebase => "rebase",
        };
        let response: MergePrResponse = self
            .post_json(
                &format!("sessions/{id}/merge-pr"),
                &serde_json::json!({ "method": method }),
            )
            .await?;
        match response.outcome.as_str() {
            "cleaned" => Ok(MergePrOutcome::Cleaned),
            "pending" => Ok(MergePrOutcome::Pending),
            other => Err(CruiseError::Other(format!(
                "invalid daemon merge response: {other}"
            ))),
        }
    }

    pub(crate) async fn local_merge(
        &self,
        id: &str,
        mode: LocalMergeMode,
        _context: &RequestContext,
    ) -> Result<crate::local_merge::LocalMergeOutcome> {
        let mode = match mode {
            LocalMergeMode::Stage => "stage",
            LocalMergeMode::MergeAndCleanup => "mergeAndCleanup",
        };
        let response: LocalMergeResponse = self
            .post_json(
                &format!("sessions/{id}/local-merge"),
                &serde_json::json!({ "mode": mode }),
            )
            .await?;
        match response.outcome.as_str() {
            "staged" => Ok(crate::local_merge::LocalMergeOutcome::Staged),
            "merged" => Ok(crate::local_merge::LocalMergeOutcome::Merged),
            "mergedCleanupFailed" => {
                Ok(crate::local_merge::LocalMergeOutcome::MergedCleanupFailed {
                    error: response.error.ok_or_else(|| {
                        CruiseError::Other(
                            "invalid daemon local-merge response: missing cleanup error".to_owned(),
                        )
                    })?,
                })
            }
            other => Err(CruiseError::Other(format!(
                "invalid daemon local-merge response: {other}"
            ))),
        }
    }

    pub(crate) async fn start_create_pr(&self, id: &str, context: &RequestContext) -> Result<()> {
        let _: Value = self
            .post_json(
                &format!("sessions/{id}/create-pr"),
                &serde_json::json!({ "context": context }),
            )
            .await?;
        Ok(())
    }
}
#[cfg(all(test, unix))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use axum::http::{Method, StatusCode};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;
    use crate::application::{
        CruiseApplication, CurrentStepUpdateDto, OperationKind, SessionSettingsRequest,
    };
    use crate::daemon::server::WebState;
    use crate::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
    use crate::session_config::SessionConfigRef;
    use crate::webui::assets::Assets;

    struct Server {
        dir: TempDir,
        manager: SessionManager,
        application: CruiseApplication,
        client: DaemonClient,
        shutdown: Option<tokio::sync::oneshot::Sender<()>>,
        task: Option<tokio::task::JoinHandle<crate::error::Result<()>>>,
    }

    impl Server {
        async fn start(dir: TempDir) -> Self {
            let manager = SessionManager::new(dir.path().join("data"));
            let application = CruiseApplication::new(manager.clone());
            let webui = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"));
            let assets = Assets {
                templates_dir: webui.join("templates"),
                static_dir: webui.join("static"),
                dev_mode: true,
            };
            let mut state = WebState::new(application.clone(), &assets)
                .unwrap_or_else(|error| panic!("web state: {error}"));
            let token = "list-api-test-token".to_owned();
            state.api_token = Some(token.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|error| panic!("listener: {error}"));
            let origin = format!(
                "http://{}/",
                listener
                    .local_addr()
                    .unwrap_or_else(|error| panic!("listener address: {error}"))
            );
            let (shutdown, receiver) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                crate::daemon::server::serve(listener, state, async move {
                    let _ = receiver.await;
                })
                .await
            });
            Self {
                dir,
                manager,
                application,
                client: DaemonClient {
                    http: reqwest::Client::builder()
                        .timeout(Duration::from_secs(15))
                        .build()
                        .unwrap_or_else(|error| panic!("HTTP client: {error}")),
                    origin,
                    token,
                },
                shutdown: Some(shutdown),
                task: Some(task),
            }
        }

        async fn stop(mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            let task = self
                .task
                .take()
                .unwrap_or_else(|| panic!("server task already stopped"));
            tokio::time::timeout(Duration::from_secs(15), task)
                .await
                .unwrap_or_else(|error| panic!("server shutdown timed out: {error}"))
                .unwrap_or_else(|error| panic!("server task: {error}"))
                .unwrap_or_else(|error| panic!("serve: {error}"));
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
        }
    }

    fn seed_session(
        manager: &SessionManager,
        id: &str,
        base_dir: &Path,
        config_path: &Path,
        phase: SessionPhase,
    ) -> SessionState {
        let mut state = SessionState::new(
            id.to_owned(),
            base_dir.to_path_buf(),
            SessionConfigRef::InlineSnapshot,
            format!("test session {id}"),
        );
        state.phase = phase;
        manager
            .create(&state)
            .unwrap_or_else(|error| panic!("create {id}: {error}"));
        let config =
            std::fs::read(config_path).unwrap_or_else(|error| panic!("read test config: {error}"));
        std::fs::write(
            crate::session_config::snapshot_path(&manager.sessions_dir(), id),
            config,
        )
        .unwrap_or_else(|error| panic!("write {id} config snapshot: {error}"));
        std::fs::write(
            state.plan_path(&manager.sessions_dir()),
            "# Plan\n\nKeep this change.\n",
        )
        .unwrap_or_else(|error| panic!("write {id} plan: {error}"));
        manager
            .load(id)
            .unwrap_or_else(|error| panic!("load {id}: {error}"))
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn isolate_environment(home: &Path) -> Vec<crate::test_support::EnvGuard> {
        let mut guards = vec![
            crate::test_support::EnvGuard::set("HOME", home),
            crate::test_support::EnvGuard::set("XDG_CONFIG_HOME", home.join("xdg-config")),
            crate::test_support::EnvGuard::set("XDG_DATA_HOME", home.join("xdg-data")),
            crate::test_support::EnvGuard::set("XDG_STATE_HOME", home.join("xdg-state")),
            crate::test_support::EnvGuard::set("GIT_CONFIG_COUNT", "0"),
            crate::test_support::EnvGuard::remove("CRUISE_CONFIG"),
            crate::test_support::EnvGuard::remove("CRUISE_DAEMON_PORT"),
            crate::test_support::EnvGuard::remove("HERDR_ENV"),
            crate::test_support::EnvGuard::set("CRUISE_DISABLE_NOTIFICATIONS", "1"),
        ];
        guards.extend(crate::test_support::clear_config_override_envs());
        guards
    }

    fn init_worktree(root: &Path, name: &str) -> (PathBuf, PathBuf, String) {
        let base = root.join(format!("{name}-base"));
        std::fs::create_dir_all(&base).unwrap_or_else(|error| panic!("create repo: {error}"));
        let config = base.join("cruise.yml");
        std::fs::write(
            &config,
            "command: [echo]\ncreate_pr: true\nsteps:\n  build:\n    prompt: make a change\n",
        )
        .unwrap_or_else(|error| panic!("write config: {error}"));
        crate::test_support::init_git_repo(&base);
        let branch = format!("{name}-branch");
        let worktree = root.join(format!("{name}-worktree"));
        let worktree_arg = worktree.to_string_lossy().into_owned();
        let branch_arg = branch.clone();
        git(
            &base,
            &["worktree", "add", "-b", &branch_arg, &worktree_arg],
        );
        (base, worktree, branch)
    }

    fn add_worktree_metadata(manager: &SessionManager, id: &str, worktree: &Path, branch: &str) {
        let mut state = manager
            .load(id)
            .unwrap_or_else(|error| panic!("load {id}: {error}"));
        state.workspace_mode = WorkspaceMode::Worktree;
        state.worktree_path = Some(worktree.to_path_buf());
        state.worktree_branch = Some(branch.to_owned());
        state.worktree_base_branch = Some("main".to_owned());
        state.worktree_base_ref = Some("refs/heads/main".to_owned());
        manager
            .save(&state)
            .unwrap_or_else(|error| panic!("save {id}: {error}"));
    }

    async fn raw(
        client: &DaemonClient,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut request = client.http.request(
            method,
            client
                .api_url(path)
                .unwrap_or_else(|error| panic!("API URL: {error}")),
        );
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .unwrap_or_else(|error| panic!("API request {path}: {error}"))
    }

    fn install_gh_stub(root: &Path) -> PathBuf {
        let dir = root.join("gh-stub");
        std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("create gh dir: {error}"));
        crate::test_support::write_executable_script(
            &dir.join("gh"),
            "#!/bin/sh\ncase \"$1:$2\" in\n  --version:*) echo 'gh version test'; exit 0 ;;\n  pr:view) cat \"$GH_FIXTURE_DIR/view.out\" ;;\n  pr:merge) printf '%s\\n' \"$*\" >> \"$GH_FIXTURE_DIR/merge.log\"; if [ -f \"$GH_FIXTURE_DIR/view_after.out\" ]; then cp \"$GH_FIXTURE_DIR/view_after.out\" \"$GH_FIXTURE_DIR/view.out\"; fi; exit 0 ;;\n  *) echo \"unexpected gh args: $*\" >&2; exit 2 ;;\nesac\n",
        );
        dir
    }

    fn gh_view(state: &str) -> String {
        json!({
            "state": state,
            "mergeable": "MERGEABLE",
            "reviewDecision": "APPROVED",
            "statusCheckRollup": [
                {"__typename": "StatusContext", "context": "ci/unit", "state": "SUCCESS"},
                {"name": "tests", "status": "COMPLETED", "conclusion": "SUCCESS"}
            ]
        })
        .to_string()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one end-to-end scenario covers the list client routes and persisted outcomes"
    )]
    #[tokio::test]
    async fn list_client_wrappers_round_trip_session_actions_and_responses() {
        let _process_lock = crate::test_support::lock_process();
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let _env = isolate_environment(dir.path());
        let server = Server::start(dir).await;
        let root = server.dir.path();
        let config = root.join("config.yml");
        std::fs::write(
            &config,
            "command: [echo]\ncreate_pr: true\nsteps:\n  build:\n    prompt: make a change\n",
        )
        .unwrap_or_else(|error| panic!("write config: {error}"));
        let mut reset_state = seed_session(
            &server.manager,
            "reset-session",
            root,
            &config,
            SessionPhase::Failed("execution failed".to_owned()),
        );
        reset_state.plan_error = Some("bad plan".to_owned());
        reset_state.current_step = Some("build".to_owned());
        server
            .manager
            .save(&reset_state)
            .unwrap_or_else(|error| panic!("save failed session: {error}"));
        seed_session(
            &server.manager,
            "settings-session",
            root,
            &config,
            SessionPhase::Failed("execution failed".to_owned()),
        );
        seed_session(
            &server.manager,
            "delete-session",
            root,
            &config,
            SessionPhase::Planned,
        );
        seed_session(
            &server.manager,
            "discard-session",
            root,
            &config,
            SessionPhase::AwaitingApproval,
        );
        let initial_sessions = server
            .client
            .sessions()
            .await
            .unwrap_or_else(|error| panic!("list sessions: {error}"));
        assert!(
            initial_sessions
                .iter()
                .any(|session| session.id == "settings-session")
        );
        let details = server
            .client
            .session_details("settings-session")
            .await
            .unwrap_or_else(|error| panic!("read session details: {error}"));
        assert_eq!(details.state.id, "settings-session");
        assert!(
            details
                .capabilities
                .contains(&crate::application::SessionAction::EditCurrentStep)
        );
        let session_json = raw(
            &server.client,
            Method::GET,
            "sessions/settings-session",
            Some(&server.client.token),
            None,
        )
        .await
        .json::<Value>()
        .await
        .unwrap_or_else(|error| panic!("read session JSON: {error}"));
        assert!(!session_json["activeOperation"].as_bool().unwrap_or(true));
        assert_eq!(
            server
                .client
                .session_plan("settings-session")
                .await
                .unwrap_or_else(|error| panic!("read plan: {error}")),
            "# Plan\n\nKeep this change.\n"
        );

        let failed = server
            .client
            .reset_session("reset-session", &RequestContext::default())
            .await
            .unwrap_or_else(|error| panic!("reset session: {error}"));
        assert!(matches!(failed.phase, SessionPhase::Planned));
        let persisted = server
            .manager
            .load("reset-session")
            .unwrap_or_else(|error| panic!("load reset session: {error}"));
        assert!(matches!(persisted.phase, SessionPhase::Planned));
        assert!(persisted.plan_error.is_none());
        assert!(persisted.current_step.is_none());

        let updated = server
            .client
            .update_session_settings(
                "settings-session",
                &SessionSettingsRequest {
                    config_path: None,
                    skipped_steps: vec!["build".to_owned()],
                    current_step_update: CurrentStepUpdateDto::Clear,
                    flags: None,
                },
                &RequestContext::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("update settings: {error}"));
        assert!(updated.current_step.is_none());
        let persisted = server
            .manager
            .load("settings-session")
            .unwrap_or_else(|error| panic!("load settings session: {error}"));
        assert_eq!(persisted.skipped_steps, vec!["build".to_owned()]);
        assert!(persisted.current_step.is_none());

        let clean = server
            .client
            .clean(&RequestContext::default())
            .await
            .unwrap_or_else(|error| panic!("clean sessions: {error}"));
        assert_eq!(
            serde_json::to_value(clean).unwrap_or_else(|error| panic!("serialize report: {error}")),
            json!({"deleted": 0, "skipped": 0, "noPrDeleted": 0})
        );

        let (diff_base, diff_worktree, diff_branch) = init_worktree(root, "diff");
        seed_session(
            &server.manager,
            "diff-session",
            &diff_base,
            &diff_base.join("cruise.yml"),
            SessionPhase::Planned,
        );
        add_worktree_metadata(
            &server.manager,
            "diff-session",
            &diff_worktree,
            &diff_branch,
        );
        std::fs::write(diff_worktree.join("README.md"), "changed line\n")
            .unwrap_or_else(|error| panic!("modify diff worktree: {error}"));
        let patch = server
            .client
            .diff("diff-session", false, false)
            .await
            .unwrap_or_else(|error| panic!("get patch: {error}"));
        assert!(patch.text.contains("changed line"));
        assert!(!patch.truncated);
        let stat = server
            .client
            .diff("diff-session", true, false)
            .await
            .unwrap_or_else(|error| panic!("get diff stat: {error}"));
        assert!(stat.text.contains("README.md"));
        assert!(!stat.truncated);
        let colored_stat = server
            .client
            .diff("diff-session", true, true)
            .await
            .unwrap_or_else(|error| panic!("get colored diff stat: {error}"));
        assert!(colored_stat.text.contains("README.md"));
        std::fs::write(
            diff_worktree.join("large-untracked.txt"),
            "x\n".repeat(600_000),
        )
        .unwrap_or_else(|error| panic!("write large diff: {error}"));
        let truncated = server
            .client
            .diff("diff-session", false, false)
            .await
            .unwrap_or_else(|error| panic!("get truncated diff: {error}"));
        assert!(truncated.truncated);
        assert!(truncated.text.len() <= crate::branch_diff::MAX_PATCH_BYTES);

        let gh_dir = install_gh_stub(root);
        let gh_state_dir = root.join("gh-state");
        std::fs::create_dir_all(&gh_state_dir)
            .unwrap_or_else(|error| panic!("create gh state dir: {error}"));
        let _gh_state = crate::test_support::EnvGuard::set("GH_FIXTURE_DIR", &gh_state_dir);
        let _path = crate::test_support::prepend_to_path(&gh_dir);
        std::fs::write(gh_state_dir.join("view.out"), gh_view("OPEN"))
            .unwrap_or_else(|error| panic!("write open PR state: {error}"));
        let mut open_pr = seed_session(
            &server.manager,
            "open-pr",
            root,
            &config,
            SessionPhase::Completed,
        );
        open_pr.pr_url = Some("https://github.com/owner/repo/pull/12".to_owned());
        server
            .manager
            .save(&open_pr)
            .unwrap_or_else(|error| panic!("save PR session: {error}"));
        let preview = server
            .client
            .inspect_merge("open-pr")
            .await
            .unwrap_or_else(|error| panic!("inspect PR: {error}"));
        assert_eq!(preview.state, "OPEN");
        assert_eq!(preview.mergeable, "MERGEABLE");
        assert_eq!(preview.review_decision, "APPROVED");
        assert_eq!(preview.checks[0].name, "ci/unit");
        assert_eq!(preview.checks[1].status, "SUCCESS");
        std::fs::write(gh_state_dir.join("view_after.out"), gh_view("OPEN"))
            .unwrap_or_else(|error| panic!("write pending PR state: {error}"));
        assert_eq!(
            server
                .client
                .merge_pr(
                    "open-pr",
                    crate::pr_merge::PrMergeMethod::Squash,
                    &RequestContext::default(),
                )
                .await
                .unwrap_or_else(|error| panic!("merge pending PR: {error}")),
            MergePrOutcome::Pending
        );
        assert!(
            server.manager.load("open-pr").is_ok(),
            "pending merge must retain the session"
        );
        let mut merged_pr = seed_session(
            &server.manager,
            "merged-pr",
            root,
            &config,
            SessionPhase::Completed,
        );
        merged_pr.pr_url = Some("https://github.com/owner/repo/pull/13".to_owned());
        server
            .manager
            .save(&merged_pr)
            .unwrap_or_else(|error| panic!("save merged PR session: {error}"));
        std::fs::write(gh_state_dir.join("view.out"), gh_view("OPEN"))
            .unwrap_or_else(|error| panic!("restore open PR state: {error}"));
        std::fs::write(gh_state_dir.join("view_after.out"), gh_view("MERGED"))
            .unwrap_or_else(|error| panic!("write merged PR state: {error}"));
        assert_eq!(
            server
                .client
                .merge_pr(
                    "merged-pr",
                    crate::pr_merge::PrMergeMethod::Merge,
                    &RequestContext::default(),
                )
                .await
                .unwrap_or_else(|error| panic!("merge completed PR: {error}")),
            MergePrOutcome::Cleaned
        );
        assert!(server.manager.load("merged-pr").is_err());

        let (stage_base, stage_worktree, stage_branch) = init_worktree(root, "stage");
        seed_session(
            &server.manager,
            "stage-session",
            &stage_base,
            &stage_base.join("cruise.yml"),
            SessionPhase::Completed,
        );
        add_worktree_metadata(
            &server.manager,
            "stage-session",
            &stage_worktree,
            &stage_branch,
        );
        std::fs::write(stage_worktree.join("README.md"), "stage this content\n")
            .unwrap_or_else(|error| panic!("modify stage worktree: {error}"));
        assert!(matches!(
            server
                .client
                .local_merge(
                    "stage-session",
                    LocalMergeMode::Stage,
                    &RequestContext::default(),
                )
                .await
                .unwrap_or_else(|error| panic!("stage local merge: {error}")),
            crate::local_merge::LocalMergeOutcome::Staged
        ));
        let staged = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&stage_base)
            .status()
            .unwrap_or_else(|error| panic!("check staged merge: {error}"));
        assert!(!staged.success(), "local stage must leave a staged squash");
        assert!(server.manager.load("stage-session").is_ok());

        let (merge_base, merge_worktree, merge_branch) = init_worktree(root, "local-merge");
        seed_session(
            &server.manager,
            "local-merge-session",
            &merge_base,
            &merge_base.join("cruise.yml"),
            SessionPhase::Completed,
        );
        add_worktree_metadata(
            &server.manager,
            "local-merge-session",
            &merge_worktree,
            &merge_branch,
        );
        std::fs::write(merge_worktree.join("README.md"), "merge this content\n")
            .unwrap_or_else(|error| panic!("modify merge worktree: {error}"));
        assert!(matches!(
            server
                .client
                .local_merge(
                    "local-merge-session",
                    LocalMergeMode::MergeAndCleanup,
                    &RequestContext::default(),
                )
                .await
                .unwrap_or_else(|error| panic!("merge locally: {error}")),
            crate::local_merge::LocalMergeOutcome::Merged
        ));
        assert!(server.manager.load("local-merge-session").is_err());
        assert_eq!(
            std::fs::read_to_string(merge_base.join("README.md"))
                .unwrap_or_else(|error| panic!("read merged base: {error}")),
            "merge this content\n"
        );

        let (create_base, create_worktree, create_branch) = init_worktree(root, "create-pr");
        seed_session(
            &server.manager,
            "create-pr-session",
            &create_base,
            &create_base.join("cruise.yml"),
            SessionPhase::PrFailed("publication interrupted".to_owned()),
        );
        add_worktree_metadata(
            &server.manager,
            "create-pr-session",
            &create_worktree,
            &create_branch,
        );
        server
            .client
            .start_create_pr("create-pr-session", &RequestContext::default())
            .await
            .unwrap_or_else(|error| panic!("start create PR: {error}"));
        let created = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = server
                    .manager
                    .load("create-pr-session")
                    .unwrap_or_else(|error| panic!("load create PR session: {error}"));
                if !matches!(state.phase, SessionPhase::PrFailed(_)) {
                    break state;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|error| panic!("create PR did not finish: {error}"));
        assert!(matches!(created.phase, SessionPhase::Completed));

        server
            .client
            .discard_session("discard-session", &RequestContext::default())
            .await
            .unwrap_or_else(|error| panic!("discard session: {error}"));
        assert!(server.manager.load("discard-session").is_err());
        server
            .client
            .delete_session("delete-session", &RequestContext::default())
            .await
            .unwrap_or_else(|error| panic!("delete session: {error}"));
        assert!(server.manager.load("delete-session").is_err());
        server.stop().await;
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one end-to-end scenario checks the list API error contracts"
    )]
    #[tokio::test]
    async fn list_api_returns_auth_not_found_busy_and_invalid_statuses() {
        let _process_lock = crate::test_support::lock_process();
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let _env = isolate_environment(dir.path());
        let server = Server::start(dir).await;
        let root = server.dir.path();
        let config = root.join("config.yml");
        std::fs::write(
            &config,
            "command: [echo]\nsteps:\n  build:\n    prompt: test\n",
        )
        .unwrap_or_else(|error| panic!("write config: {error}"));
        seed_session(
            &server.manager,
            "busy-session",
            root,
            &config,
            SessionPhase::Failed("execution failed".to_owned()),
        );

        let missing_token = raw(&server.client, Method::GET, "sessions", None, None).await;
        assert_eq!(missing_token.status(), StatusCode::UNAUTHORIZED);
        let invalid_token = raw(
            &server.client,
            Method::GET,
            "sessions",
            Some("wrong-token"),
            None,
        )
        .await;
        assert_eq!(invalid_token.status(), StatusCode::UNAUTHORIZED);

        let unknown = raw(
            &server.client,
            Method::GET,
            "sessions/missing-session",
            Some(&server.client.token),
            None,
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert!(
            server
                .client
                .reset_session("missing-session", &RequestContext::default())
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .delete_session("missing-session", &RequestContext::default())
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .discard_session("missing-session", &RequestContext::default())
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .update_session_settings(
                    "missing-session",
                    &SessionSettingsRequest {
                        config_path: None,
                        skipped_steps: Vec::new(),
                        current_step_update: CurrentStepUpdateDto::Unchanged,
                        flags: None,
                    },
                    &RequestContext::default(),
                )
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .inspect_merge("missing-session")
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .merge_pr(
                    "missing-session",
                    crate::pr_merge::PrMergeMethod::Merge,
                    &RequestContext::default(),
                )
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .local_merge(
                    "missing-session",
                    LocalMergeMode::Stage,
                    &RequestContext::default(),
                )
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .start_create_pr("missing-session", &RequestContext::default())
                .await
                .is_err()
        );
        assert!(
            server
                .client
                .diff("missing-session", false, false)
                .await
                .is_err()
        );

        let invalid_mode = raw(
            &server.client,
            Method::POST,
            "sessions/busy-session/local-merge",
            Some(&server.client.token),
            Some(json!({"mode": "fast-forward"})),
        )
        .await;
        assert_eq!(invalid_mode.status(), StatusCode::BAD_REQUEST);
        let invalid_merge_method = raw(
            &server.client,
            Method::POST,
            "sessions/busy-session/merge-pr",
            Some(&server.client.token),
            Some(json!({"method": "octopus"})),
        )
        .await;
        assert_eq!(invalid_merge_method.status(), StatusCode::BAD_REQUEST);

        let claim = server
            .application
            .runtime()
            .try_begin("busy-session", OperationKind::Run)
            .unwrap_or_else(|error| panic!("claim session: {error}"));
        let session_read = raw(
            &server.client,
            Method::GET,
            "sessions/busy-session",
            Some(&server.client.token),
            None,
        )
        .await
        .json::<Value>()
        .await
        .unwrap_or_else(|error| panic!("read busy session: {error}"));
        assert!(session_read["activeOperation"].as_bool().unwrap_or(false));
        assert!(
            session_read["capabilities"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
        let create_pr_busy = raw(
            &server.client,
            Method::POST,
            "sessions/busy-session/create-pr",
            Some(&server.client.token),
            Some(json!({"context": null})),
        )
        .await;
        assert_eq!(create_pr_busy.status(), StatusCode::CONFLICT);
        assert!(
            server
                .client
                .start_create_pr("busy-session", &RequestContext::default())
                .await
                .is_err()
        );
        let busy = raw(
            &server.client,
            Method::POST,
            "sessions/busy-session/reset",
            Some(&server.client.token),
            Some(json!({})),
        )
        .await;
        assert_eq!(busy.status(), StatusCode::CONFLICT);
        let body = busy
            .json::<Value>()
            .await
            .unwrap_or_else(|error| panic!("parse busy response: {error}"));
        assert_eq!(body["error"], "session busy-session is already busy");
        drop(claim);
        server.stop().await;
    }
    #[tokio::test]
    async fn create_pr_setup_failure_returns_error_to_client_without_waiting() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let server = Server::start(dir).await;
        let base = server.dir.path().join("missing-worktree-base");
        std::fs::create_dir_all(&base).unwrap_or_else(|error| panic!("create base: {error}"));
        let config_path = base.join("cruise.yml");
        std::fs::write(
            &config_path,
            "command: [cat]\ncreate_pr: true\nsteps:\n  build:\n    prompt: test\n",
        )
        .unwrap_or_else(|error| panic!("write config: {error}"));
        seed_session(
            &server.manager,
            "missing-worktree",
            &base,
            &config_path,
            SessionPhase::PrFailed("publication failed".to_owned()),
        );

        let response = tokio::time::timeout(
            Duration::from_secs(1),
            raw(
                &server.client,
                Method::POST,
                "sessions/missing-worktree/create-pr",
                Some(&server.client.token),
                Some(json!({ "context": null })),
            ),
        )
        .await
        .unwrap_or_else(|error| panic!("create-PR setup response timed out: {error}"));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            server
                .client
                .start_create_pr("missing-worktree", &RequestContext::default()),
        )
        .await
        .unwrap_or_else(|error| panic!("create-PR client request timed out: {error}"));
        assert!(result.is_err());
        assert_eq!(
            server
                .application
                .runtime()
                .active_operation("missing-worktree"),
            None
        );
        server.stop().await;
    }
}
