use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};
use crate::flags::FlagValues;
use crate::new_session_history::{HistoryScope, NewSessionHistory};
use crate::session::{SessionPhase, SessionState};

use super::{CruiseApplication, OperationKind, PlanRequest};

/// Options applied when approving a generated plan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveRequest {
    #[serde(default)]
    pub flags: Option<FlagValues>,
    #[serde(default)]
    pub skipped_steps: Option<Vec<String>>,
    #[serde(default)]
    pub generate_title: bool,
    #[serde(default)]
    pub context: Option<crate::request_context::RequestContext>,
}

/// Body accepted by the plan-start endpoint.
pub type PlanStart = PlanRequest;

impl CruiseApplication {
    /// Approve an awaiting plan, applying client-selected flags and skipped steps.
    ///
    /// SDK title generation and its authentication preflight run in the daemon.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is not awaiting approval, its plan is
    /// unusable, configuration or title generation fails, or persistence fails.
    pub async fn approve_with_request(
        &self,
        id: &str,
        request: ApproveRequest,
    ) -> Result<SessionState> {
        let claim = self.runtime().try_begin(id, OperationKind::Mutate)?;
        let mut state = self.read_session(id)?;
        if !matches!(state.phase, SessionPhase::AwaitingApproval) {
            return Err(CruiseError::Other(format!(
                "session is in '{}' phase and cannot be approved",
                state.phase.label()
            )));
        }
        if state.plan_error.is_some() || !self.session_plan_available(&state) {
            return Err(CruiseError::Other(
                "session has no usable plan to approve".to_string(),
            ));
        }

        let config_env = request
            .context
            .as_ref()
            .map_or(crate::config::ConfigEnv::Process, |context| {
                crate::config::ConfigEnv::Snapshot(&context.config_env)
            });
        let config = self.manager.load_config_with_env(&state, config_env)?;
        if let Some(flags) = request.flags {
            crate::flags::validate_names(&config, &flags)?;
            state.flags = flags;
        }
        if let Some(skipped_steps) = request.skipped_steps {
            state.skipped_steps = super::filter_skipped_steps(&config, skipped_steps)?;
        }
        let mut nodes = crate::workflow::list_skippable_steps(&config)?;
        nodes.extend(crate::workflow::list_skippable_after_pr_steps(&config)?);
        state.skipped_steps = crate::flags::drop_disabled_skips(
            std::mem::take(&mut state.skipped_steps),
            &nodes,
            &state.flags,
        );

        if request.generate_title {
            let plan = self.session_plan(id)?;
            let token = claim.token();
            let logger = crate::session::SessionLogger::new(self.manager.run_log_path(id));
            generate_approval_title(
                &mut state,
                &config,
                &plan,
                request.context.as_ref(),
                &logger,
                &token,
            )
            .await?;
        }
        if claim.token().is_cancelled() {
            return Err(CruiseError::Interrupted);
        }
        let create_pr = config.create_pr;
        state.approve();
        if state.pr_target.is_none() {
            crate::repo_clone::cleanup_after_approval_checked(
                &self.manager,
                &mut state,
                create_pr,
            )?;
        }
        self.manager.save(&state)?;
        // Persist skip history only after the daemon has committed approval.
        let resolved_config_key = state
            .config
            .stable_identity(state.repo.as_deref(), &state.id);
        let base_dir = state.base_dir.to_string_lossy();
        let scope = state.repo.as_deref().map_or(
            HistoryScope::Directory(base_dir.as_ref()),
            HistoryScope::Repo,
        );
        let mut history = NewSessionHistory::load_best_effort();
        history.record_skip_selection_for_scope(
            scope,
            &resolved_config_key,
            state.skipped_steps.clone(),
        );
        history.save_best_effort();
        Ok(state)
    }
    /// Apply a fully specified fix-plan request while preserving its retry and context settings.
    ///
    /// # Errors
    ///
    /// Returns an error if planning or session persistence fails.
    pub async fn fix_with_request(
        &self,
        id: &str,
        request: PlanRequest,
        sink: std::sync::Arc<dyn super::ApplicationEventSink>,
    ) -> Result<SessionState> {
        self.plan_with_operation(id, request, sink, OperationKind::Fix)
            .await
    }

    /// Apply a fully specified ask-plan request while preserving its retry and context settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the question is empty, planning fails, or session persistence fails.
    pub async fn ask_with_request(
        &self,
        id: &str,
        request: PlanRequest,
        sink: std::sync::Arc<dyn super::ApplicationEventSink>,
    ) -> Result<SessionState> {
        if request
            .question
            .as_deref()
            .is_some_and(|question| question.trim().is_empty())
        {
            return Err(CruiseError::Other("question must not be empty".to_string()));
        }
        self.plan_with_operation(id, request, sink, OperationKind::Ask)
            .await
    }

    /// Apply a fully specified verification request with the requested retry policy.
    ///
    /// # Errors
    ///
    /// Returns an error if verification or session persistence fails.
    pub async fn verify_with_request(
        &self,
        id: &str,
        request: PlanRequest,
        sink: std::sync::Arc<dyn super::ApplicationEventSink>,
    ) -> Result<SessionState> {
        self.plan_with_operation(id, request, sink, OperationKind::Verify)
            .await
    }
}

async fn generate_approval_title(
    state: &mut SessionState,
    config: &crate::config::WorkflowConfig,
    plan: &str,
    context: Option<&crate::request_context::RequestContext>,
    logger: &crate::session::SessionLogger,
    cancel_token: &crate::cancellation::CancellationToken,
) -> Result<()> {
    if state.pr_target.is_some()
        || !crate::executor::Executor::new(config.sdk.as_deref(), &config.command).is_sdk()
    {
        crate::metadata::refresh_session_title_from_plan(state, plan);
        return Ok(());
    }

    let mut auth_config = config.clone();
    auth_config.env.clear();
    crate::agent_auth::ensure_agents_authenticated(
        &auth_config,
        crate::agent_auth::AuthScope::Planning,
        context.map(|context| context.cwd.as_path()),
    )?;
    let on_notice = |message: &str| {
        crate::status_eprintln!("{}", console::style(message).dim());
        logger.write(&format!("[info] {message}"));
    };
    match generate_title_via_sdk(
        config,
        &state.input,
        plan,
        cancel_token,
        Some(&on_notice),
        Some(&state.id),
    )
    .await
    {
        Ok(title) => state.title = Some(title),
        Err(CruiseError::Interrupted) => return Err(CruiseError::Interrupted),
        Err(error @ CruiseError::MissingJcodeModel(_)) => return Err(error),
        Err(error) => {
            eprintln!("warning: SDK title generation failed: {error}");
            crate::metadata::refresh_session_title_from_plan(state, plan);
        }
    }
    Ok(())
}

async fn generate_title_via_sdk(
    config: &crate::config::WorkflowConfig,
    input: &str,
    plan_content: &str,
    cancel_token: &crate::cancellation::CancellationToken,
    on_notice: Option<&(dyn Fn(&str) + Send + Sync)>,
    usage_session_id: Option<&str>,
) -> Result<String> {
    let title_store = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let tool = crate::sdk_tools::generate_title_tool(std::sync::Arc::clone(&title_store));
    let executor = crate::executor::Executor::new(config.sdk.as_deref(), &config.command);
    let model =
        crate::model_selection::planning_spec(config.plan_model.as_ref(), config.model.as_ref());
    let prompt = format!(
        "Generate a concise session title (max 80 chars) for this task and plan. \
         Call the generate_title tool with your title.\n\n\
         Task: {input}\n\nPlan:\n{plan_content}"
    );
    let env = HashMap::new();
    executor
        .run(crate::executor::PromptRun {
            usage: Some(crate::usage::UsageScope {
                session_id: usage_session_id,
                step: None,
                purpose: crate::usage::UsagePurpose::PlanTitle,
            }),
            prompt: &prompt,
            model,
            subagent_model: None,
            max_retries: 1,
            env: &env,
            mcp_servers: &config.mcp_servers,
            on_notice,
            cancel_token: Some(cancel_token),
            working_dir: None,
            stream: None,
            tools: vec![tool],
            on_session_id: None,
            resume: None,
            computer_use: false,
            permission: crate::config::PermissionMode::Full,
            tool_policy: crate::tool_policy::ToolPolicy::default(),
        })
        .await?;
    title_store
        .lock()
        .map_err(|error| CruiseError::Other(format!("title store lock poisoned: {error}")))?
        .clone()
        .ok_or_else(|| CruiseError::Other("SDK agent did not call generate_title tool".to_string()))
}
