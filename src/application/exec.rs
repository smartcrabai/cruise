use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{ApplicationEventSink, CruiseApplication, LogSink, OperationKind, RunRequest};
use crate::error::{CruiseError, Result};
use crate::resolver::ConfigSource;
use crate::session::{SessionPhase, SessionState, WorkspaceMode};
use crate::session_config::SessionConfigRef;

/// Resolved input needed to create and execute a transient exec session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecStartRequest {
    pub config_yaml: String,
    pub config_source: SessionConfigRef,
    pub input: String,
    #[serde(default)]
    pub flags: crate::flags::FlagValues,
    #[serde(default)]
    pub skipped_steps: Vec<String>,
    #[serde(default)]
    pub max_retries: Option<usize>,
    #[serde(default = "super::default_rate_limit_retries")]
    pub rate_limit_retries: usize,
    pub context: crate::request_context::RequestContext,
}

impl ExecStartRequest {
    /// Resolve an exec request in an existing local directory.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid input or directory, or if configuration resolution fails.
    pub fn for_directory(
        base_dir: &std::path::Path,
        config_path: Option<&str>,
        input: &str,
        flags: crate::flags::FlagValues,
        skipped_steps: Vec<String>,
    ) -> Result<Self> {
        let input = input.trim().to_owned();
        if input.is_empty() {
            return Err(CruiseError::Other("exec input cannot be empty".into()));
        }
        if crate::issue_input::parse_binding(&input)?.is_some() {
            return Err(CruiseError::Other(
                "exec does not resolve #N issue input; use Create draft to plan from an issue"
                    .into(),
            ));
        }
        let base = std::path::PathBuf::from(crate::new_session_history::expand_tilde(
            &base_dir.to_string_lossy(),
        ));
        if !base.is_absolute() || !base.is_dir() {
            return Err(CruiseError::Other(
                "exec requires an existing absolute working directory".into(),
            ));
        }
        let config_path = config_path.map(crate::new_session_history::expand_tilde);
        let (config_yaml, source) =
            crate::resolver::resolve_config_in_dir(config_path.as_deref(), &base)?;
        Ok(Self {
            config_yaml,
            config_source: SessionConfigRef::from_source(&source, None)?,
            input,
            flags,
            skipped_steps,
            max_retries: None,
            rate_limit_retries: super::default_rate_limit_retries(),
            context: crate::request_context::RequestContext::for_dir(base),
        })
    }
}

impl CruiseApplication {
    /// Create and start a transient exec session in the requesting client's directory.
    ///
    /// Terminal sessions are disposed by the regular application run path; paused and
    /// suspended sessions remain available for explicit resume.
    ///
    /// # Errors
    ///
    /// Returns an error if the request config or flags are invalid or the session cannot
    /// be persisted.
    pub fn start_exec(
        &self,
        request: ExecStartRequest,
        sink: Arc<dyn ApplicationEventSink>,
        log_sink: Option<Arc<dyn LogSink>>,
    ) -> Result<String> {
        let state = self.create_exec_session(&request)?;
        let session_id = state.id.clone();
        let claim = self.runtime().try_begin(&session_id, OperationKind::Run)?;
        let run_request = RunRequest {
            max_retries: request.max_retries,
            rate_limit_retries: request.rate_limit_retries,
            context: Some(request.context),
            ..RunRequest::default()
        };
        let application = self.clone();
        tokio::spawn(async move {
            let _ = application
                .run_with_claim_and_log_sink(claim, run_request, sink, log_sink)
                .await;
        });
        Ok(session_id)
    }

    fn create_exec_session(&self, request: &ExecStartRequest) -> Result<SessionState> {
        let source = resolver_source(&request.config_source)?;
        let config = crate::resolver::resolve_workflow_config_with_env(
            &request.config_yaml,
            &source,
            &request.context.cwd,
            Some(&request.context.cwd),
            crate::config::ConfigEnv::Snapshot(&request.context.config_env),
        )?;
        crate::config::validate_config(&config)?;
        crate::flags::validate_names(&config, &request.flags)?;

        let session_id = crate::session::SessionManager::new_session_id();
        let mut session = SessionState::new(
            session_id.clone(),
            request.context.cwd.clone(),
            request.config_source.clone(),
            request.input.clone(),
        );
        session.workspace_mode = WorkspaceMode::CurrentBranch;
        session.allow_dirty_working_tree = true;
        session.phase = SessionPhase::Planned;
        session.exec = true;
        session.flags = request.flags.clone();
        let mut nodes = crate::workflow::list_skippable_steps(&config)?;
        nodes.extend(crate::workflow::list_skippable_after_pr_steps(&config)?);
        session.skipped_steps = crate::flags::drop_disabled_skips(
            request.skipped_steps.clone(),
            &nodes,
            &request.flags,
        );
        self.manager.create_with_config(&session, &config)?;

        let session_dir = self.manager.sessions_dir().join(&session_id);
        if let Err(error) = std::fs::write(session.plan_path(&self.manager.sessions_dir()), "") {
            let _ = std::fs::remove_dir_all(session_dir);
            return Err(error.into());
        }
        Ok(session)
    }
}

fn resolver_source(config_source: &SessionConfigRef) -> Result<ConfigSource> {
    match config_source {
        SessionConfigRef::File { path } => Ok(ConfigSource::Explicit(path.clone())),
        SessionConfigRef::BuiltinSnapshot { name } => {
            let name = name
                .as_deref()
                .unwrap_or(crate::builtin_workflows::DEFAULT_BUILTIN_NAME);
            let builtin = crate::builtin_workflows::find_builtin(name)
                .ok_or_else(|| CruiseError::Other(format!("unknown built-in workflow `{name}`")))?;
            if builtin.name == crate::builtin_workflows::DEFAULT_BUILTIN_NAME {
                Ok(ConfigSource::Builtin)
            } else {
                Ok(ConfigSource::NamedBuiltin(builtin.name))
            }
        }
        SessionConfigRef::InlineSnapshot => Ok(ConfigSource::Inline),
        SessionConfigRef::RepoSnapshot { .. } => Err(CruiseError::Other(
            "exec cannot use a repository snapshot config".to_string(),
        )),
    }
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod tests;
