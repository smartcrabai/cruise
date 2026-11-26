use std::sync::Arc;

use super::{
    ApplicationEvent, ApplicationRuntime, CancellationToken, CruiseError, OperationClaim,
    OptionChoice, OptionResult, PendingKind, PromptResponse, Result, RunInputs, RunSetup,
    RuntimeOptionHandler, SessionManager, SessionState, option_choice_payload,
};
use crate::quint_workflow::{APPROVED_VAR, Agent, Files, Image, Run, Stage, Tools, error};

pub(super) const APPROVE: &str = "quint-approve";
const REJECT: &str = "quint-reject";
const REVISE: &str = "quint-revise";

fn set_stage(
    manager: &SessionManager,
    claim: &OperationClaim,
    session: &mut SessionState,
    stage: Stage,
) -> Result<()> {
    session.quint_run = Some(Run::On { stage });
    if !claim
        .runtime
        .persist_if_active(claim, || manager.save(session))?
    {
        return Err(CruiseError::Interrupted);
    }
    Ok(())
}

fn choices() -> [OptionChoice; 3] {
    [
        OptionChoice::Selector {
            label: "Approve Quint model".into(),
            next: Some(APPROVE.into()),
        },
        OptionChoice::Selector {
            label: "Reject and stop".into(),
            next: Some(REJECT.into()),
        },
        OptionChoice::TextInput {
            label: "Request changes".into(),
            next: Some(REVISE.into()),
        },
    ]
}

fn files(manager: &SessionManager, state: &SessionState) -> Result<Files> {
    Files::new(
        &state.artifacts_path(&manager.sessions_dir()),
        state.execution_id.as_deref(),
    )
}

#[expect(clippy::too_many_lines, reason = "one durable approval state machine")]
pub(super) async fn begin_review(
    manager: &SessionManager,
    claim: &OperationClaim,
    setup: &mut RunSetup,
    inputs: &mut RunInputs,
    option: &RuntimeOptionHandler,
    on_start: &(dyn Fn(&str) -> Result<()> + Sync),
    on_log: &crate::step::command::StepLogCallback<'_>,
) -> Result<()> {
    let session = &mut setup.state;
    let Some(Run::On { .. }) = &session.quint_run else {
        return Ok(());
    };
    let files = files(manager, session)?;
    let mut tools = None;
    loop {
        let stage = session
            .quint_run
            .as_ref()
            .and_then(Run::stage)
            .ok_or_else(|| error("Missing Quint review state"))?
            .clone();
        let (preview, model) = match stage {
            Stage::MainFailed { .. } if setup.dag.state.completed => {
                return Err(error(
                    "A main workflow step failed; select the failed step to retry or reset the session",
                ));
            }
            Stage::MainFailed { preview } => {
                let model = files.approved(&preview)?;
                (preview, model)
            }
            Stage::Verified { preview, .. } if !setup.dag.state.completed => {
                let model = files.approved(&preview)?;
                (preview, model)
            }
            Stage::Running { preview, .. } | Stage::Verified { preview, .. } => {
                inputs
                    .vars
                    .set_named_file(APPROVED_VAR, files.approved(&preview)?);
                return Ok(());
            }
            Stage::Rejected { .. } => return Err(error(REJECTED)),
            Stage::Draft { feedback } => {
                if tools.is_none() {
                    tools = Some(Tools::load()?);
                }
                on_start("quint-draft")?;
                let agent = Agent {
                    workflow: &setup.compiled,
                    workspace: setup.workspace.path(),
                    token: &claim.token,
                    session: claim.session_id(),
                    retries: setup.rate_limit_retries,
                };
                let model_sha256 = crate::retry::with_active_policy(
                    setup.retry_policy.clone(),
                    agent.draft(&inputs.vars, &files, feedback.as_deref()),
                )
                .await?;
                set_stage(manager, claim, session, Stage::Candidate { model_sha256 })?;
                continue;
            }
            Stage::Candidate { model_sha256 } => {
                if session.current_step.is_some()
                    || setup.dag.state.completed
                    || setup.dag.state.current.is_some()
                {
                    return Err(error(
                        "A run cannot begin at an implementation step without Quint approval; reset the session",
                    ));
                }
                let tools = match &tools {
                    Some(tools) => tools,
                    None => tools.insert(Tools::load()?),
                };
                on_start("quint-preview")?;
                let next = match review_candidate(
                    &files,
                    tools,
                    &claim.token,
                    option,
                    &model_sha256,
                    on_log,
                )
                .await
                {
                    Err(CruiseError::CommandError(message)) => {
                        set_stage(
                            manager,
                            claim,
                            session,
                            Stage::Draft {
                                feedback: Some(message.clone()),
                            },
                        )?;
                        return Err(error(message));
                    }
                    result => result?,
                };
                set_stage(manager, claim, session, next)?;
                continue;
            }
            Stage::Approved { preview } => {
                if !files.rendered_candidate(&preview)? {
                    set_stage(manager, claim, session, changed_candidate())?;
                    continue;
                }
                let Ok(model) = files.approved(&preview) else {
                    set_stage(manager, claim, session, changed_candidate())?;
                    continue;
                };
                (preview, model)
            }
        };
        set_stage(
            manager,
            claim,
            session,
            Stage::Running {
                preview,
                finalizing: false,
            },
        )?;
        inputs.vars.set_named_file(APPROVED_VAR, model);
        return Ok(());
    }
}

const REJECTED: &str = "Quint model rejected; implementation and pull-request creation stopped";

fn changed_candidate() -> Stage {
    Stage::Draft {
        feedback: Some("The candidate changed. Recreate the complete model for review.".into()),
    }
}

async fn review_candidate(
    files: &Files,
    tools: &Tools,
    token: &CancellationToken,
    option: &RuntimeOptionHandler,
    expected: &str,
    log: &crate::step::command::StepLogCallback<'_>,
) -> Result<Stage> {
    let preview = files.preview(tools, token).await?;
    if preview.model_sha256 != expected {
        return Ok(changed_candidate());
    }
    log(
        "info",
        &format!(
            "Quint PNG ready: model SHA-256 {}, PNG SHA-256 {}",
            preview.model_sha256, preview.image.sha256
        ),
    );
    let response = option
        .select_with_image(
            &choices(),
            Some("Review the Quint state transitions before implementation."),
            Some(token),
            Some(preview.image.clone()),
        )
        .await?;
    if !files.rendered_candidate(&preview)? {
        return Ok(changed_candidate());
    }
    match response.next_step.as_deref() {
        Some(APPROVE) => {
            files.approve(&preview)?;
            log(
                "info",
                &format!("Approved Quint model {}", preview.model_sha256),
            );
            Ok(Stage::Approved { preview })
        }
        Some(REVISE) => {
            log(
                "info",
                "Quint model revision requested; implementation remains blocked",
            );
            Ok(Stage::Draft {
                feedback: response.text_input,
            })
        }
        Some(REJECT) => Ok(Stage::Rejected { preview }),
        _ => Err(error("Invalid Quint review decision")),
    }
}

pub(super) async fn finish_review(
    manager: &SessionManager,
    claim: &OperationClaim,
    setup: &mut RunSetup,
    failed: usize,
    on_start: &(dyn Fn(&str) -> Result<()> + Sync),
    on_log: &crate::step::command::StepLogCallback<'_>,
) -> Result<()> {
    let Some(Run::On { stage }) = &setup.state.quint_run else {
        return Ok(());
    };
    if failed > 0 {
        if !setup.state.exec
            && let Stage::Running { preview, .. } = stage
        {
            let stage = Stage::MainFailed {
                preview: preview.clone(),
            };
            set_stage(manager, claim, &mut setup.state, stage)?;
        }
        return Err(error(format!(
            "Quint final verification blocked by {failed} failed main workflow step(s)"
        )));
    }
    let Stage::Running { preview, .. } = stage else {
        return if matches!(stage, Stage::Verified { .. }) {
            Ok(())
        } else {
            Err(error("Quint approval missing before final verification"))
        };
    };
    let preview = preview.clone();
    let files = files(manager, &setup.state)?;
    set_stage(
        manager,
        claim,
        &mut setup.state,
        Stage::Running {
            preview: preview.clone(),
            finalizing: true,
        },
    )?;
    on_start("quint-final")?;
    let agent = Agent {
        workflow: &setup.compiled,
        workspace: setup.workspace.path(),
        token: &claim.token,
        session: claim.session_id(),
        retries: setup.rate_limit_retries,
    };
    let receipt = crate::retry::with_active_policy(
        setup.retry_policy.clone(),
        agent.finish(&files, &preview),
    )
    .await?;
    set_stage(
        manager,
        claim,
        &mut setup.state,
        Stage::Verified { preview, receipt },
    )?;
    on_log(
        "info",
        "Approved Quint model passed final verification; receipts saved",
    );
    Ok(())
}

impl RuntimeOptionHandler {
    pub(super) async fn select_with_image(
        &self,
        choices: &[OptionChoice],
        plan: Option<&str>,
        token: Option<&CancellationToken>,
        image: Option<Image>,
    ) -> Result<OptionResult> {
        if choices.is_empty() {
            return Ok(OptionResult {
                next_step: None,
                text_input: None,
            });
        }
        let effective = token.cloned().unwrap_or_else(|| self.token.clone());
        let prompt = plan.unwrap_or("Select an option");
        let has_image = image.is_some();
        let (id, receiver) = self.runtime.register_prompt_with_image(
            &self.session_id,
            self.claim_identity,
            PendingKind::Option,
            has_image.then_some(prompt),
            Some(choices),
            image,
        )?;
        if let Err(error) = self.sink.send(ApplicationEvent::OptionRequired {
            session_id: self.session_id.clone(),
            request_id: id.clone(),
            prompt: prompt.into(),
            choices: choices.iter().map(option_choice_payload).collect(),
            has_image,
        }) {
            self.runtime
                .unregister_prompt(&id, &self.session_id, self.claim_identity);
            return Err(error);
        }
        let runtime = Arc::clone(&self.runtime);
        let session_id = self.session_id.clone();
        let claim_identity = self.claim_identity;
        let response = tokio::task::spawn_blocking(move || {
            runtime.wait_prompt(
                &session_id,
                &id,
                claim_identity,
                &receiver,
                Some(&effective),
            )
        })
        .await
        .map_err(|error| CruiseError::Other(format!("option prompt worker failed: {error}")))??;
        match response {
            PromptResponse::Option(result) => Ok(result),
            PromptResponse::Ask(_) => Err(error("received ask response for option prompt")),
        }
    }
}

impl ApplicationRuntime {
    pub(super) fn prompt_image(&self, session_id: &str, request_id: &str) -> Result<Vec<u8>> {
        let missing = || {
            CruiseError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No active image for this prompt",
            ))
        };
        let image = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(request_id)
            .filter(|request| request.session_id == session_id)
            .and_then(|request| request.image.clone())
            .ok_or_else(missing)?;
        let state = self.manager.load(session_id)?;
        let bytes = image.read(&state.artifacts_path(&self.manager.sessions_dir()))?;
        let claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let request = pending
            .get_mut(request_id)
            .filter(|request| request.session_id == session_id)
            .filter(|request| {
                claims.get(session_id).is_some_and(|claim| {
                    claim.identity == request.claim_identity && !claim.token.is_cancelled()
                })
            })
            .ok_or_else(missing)?;
        request.image_served = true;
        Ok(bytes)
    }
}
