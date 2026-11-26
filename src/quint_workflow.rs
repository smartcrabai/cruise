mod evidence;
mod tools;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::artifacts;
use crate::cancellation::CancellationToken;
use crate::config::{PermissionMode, WorkflowConfig};
use crate::error::{CruiseError, Result};
use crate::executor::{Executor, PromptRun};
use crate::session::SessionState;
use crate::workflow::CompiledWorkflow;

pub(crate) use evidence::publication_evidence;
pub(crate) use tools::Tools;

pub(crate) const FLAG: &str = "formal_methods";
pub(crate) const APPROVED_VAR: &str = "quint.approved";
pub(crate) const MAX_PNG: usize = 1024 * 1024;
const MODEL_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum Run {
    Off,
    On { stage: Stage },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Stage {
    Draft {
        feedback: Option<String>,
    },
    Candidate {
        model_sha256: String,
    },
    Approved {
        preview: Preview,
    },
    Running {
        preview: Preview,
        finalizing: bool,
    },
    MainFailed {
        preview: Preview,
    },
    Verified {
        preview: Preview,
        receipt: evidence::Receipt,
    },
    Rejected {
        preview: Preview,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Preview {
    pub(crate) model_sha256: String,
    pub(crate) image: Image,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Image {
    name: String,
    pub(crate) sha256: String,
}

impl Image {
    pub(crate) fn read(&self, artifacts_root: &Path) -> Result<Vec<u8>> {
        let bytes = artifacts::read_bytes(artifacts_root, &self.name, MAX_PNG)?;
        ensure_png(&bytes)?;
        if sha256(&bytes) != self.sha256 {
            return Err(error("Quint review image changed; reopen the review"));
        }
        Ok(bytes)
    }
}

pub(crate) fn error(message: impl Into<String>) -> CruiseError {
    CruiseError::Other(message.into())
}

fn sha256(bytes: &[u8]) -> String {
    crate::http_download::hex_digest(&Sha256::digest(bytes))
}

pub(crate) fn ensure_png(bytes: &[u8]) -> Result<()> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() > MAX_PNG {
        return Err(error("Quint renderer did not produce a bounded PNG"));
    }
    Ok(())
}

impl Run {
    pub(crate) fn enabled(&self) -> bool {
        matches!(self, Self::On { .. })
    }

    pub(crate) fn finalizing(&self) -> bool {
        matches!(
            self,
            Self::On {
                stage: Stage::Running {
                    finalizing: true,
                    ..
                } | Stage::Verified { .. }
            }
        )
    }

    pub(crate) fn stage(&self) -> Option<&Stage> {
        match self {
            Self::Off => None,
            Self::On { stage } => Some(stage),
        }
    }
}

pub(crate) fn configure(config: &WorkflowConfig, state: &mut SessionState) -> Result<()> {
    let configured = config.flags.get(FLAG).is_some_and(|flag| flag.quint_review);
    let enabled = configured && state.flags.get(FLAG) == Some(&true);
    match state.quint_run.as_ref() {
        Some(Run::On {
            stage: Stage::Rejected { .. },
        }) => {
            return Err(error(
                "Quint model was rejected; reset the session to start a new execution",
            ));
        }
        Some(run) if run.enabled() != enabled => return Err(flag_changed()),
        None if configured => {
            if enabled && state.current_step.is_some() {
                return Err(flag_changed());
            }
            state.quint_run = Some(if enabled {
                Run::On {
                    stage: Stage::Draft { feedback: None },
                }
            } else {
                Run::Off
            });
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn flag_changed() -> CruiseError {
    error("Cannot change formal_methods after execution has started; reset the session first")
}

pub(crate) fn record_workflow_report(
    vars: &crate::variable::VariableStore,
    step: &str,
    output: &str,
) -> Result<()> {
    let name = match step {
        "write-test-first" => "test-first.md",
        "implement-after-tests" => "implementation.md",
        _ => return Ok(()),
    };
    let Some(execution) = vars
        .named_file(APPROVED_VAR)
        .and_then(Path::parent)
        .and_then(Path::file_name)
    else {
        return Ok(());
    };
    let report = crate::telemetry::stderr::sanitize_with_limit(output, 16 * 1024).body;
    vars.write_artifact(
        &format!("quint/{}/{name}", execution.to_string_lossy()),
        &report,
    )
}

pub(crate) struct Files {
    root: PathBuf,
    prefix: String,
}

impl Files {
    pub(crate) fn new(root: &Path, execution: Option<&str>) -> Result<Self> {
        let execution = execution
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .ok_or_else(|| error("Quint review requires an execution ID"))?;
        Ok(Self {
            root: root.to_path_buf(),
            prefix: format!("quint/{execution}"),
        })
    }

    fn name(&self, name: &str) -> String {
        format!("{}/{name}", self.prefix)
    }

    fn write(&self, name: &str, bytes: &[u8], limit: usize) -> Result<()> {
        artifacts::write_bytes(&self.root, &self.name(name), bytes, limit)
    }

    fn read_model(&self, name: &str) -> Result<Vec<u8>> {
        artifacts::read_bytes(&self.root, &self.name(name), MODEL_LIMIT)
    }

    pub(crate) fn rendered_candidate(&self, preview: &Preview) -> Result<bool> {
        Ok(
            sha256(&self.read_model("candidate.qnt")?) == preview.model_sha256
                && preview.image.read(&self.root).is_ok(),
        )
    }

    pub(crate) fn store_candidate(&self, source: &str) -> Result<String> {
        self.write("candidate.qnt", source.as_bytes(), MODEL_LIMIT)?;
        Ok(sha256(source.as_bytes()))
    }

    pub(crate) async fn preview(
        &self,
        tools: &Tools,
        token: &CancellationToken,
    ) -> Result<Preview> {
        let source = self.read_model("candidate.qnt")?;
        let model_sha256 = sha256(&source);
        let result = tools::check(tools, &self.root, source, true, token).await?;
        let png = result
            .png
            .ok_or_else(|| error("Missing Quint review PNG"))?;
        let sha256 = sha256(&png);
        let name = self.name(&format!("{sha256}.png"));
        artifacts::write_bytes(&self.root, &name, &png, MAX_PNG)?;
        self.write(
            "preview.log",
            result.log.as_bytes(),
            artifacts::MAX_ARTIFACT_SIZE,
        )?;
        Ok(Preview {
            model_sha256,
            image: Image { name, sha256 },
        })
    }

    pub(crate) fn approve(&self, preview: &Preview) -> Result<PathBuf> {
        let source = self.read_model("candidate.qnt")?;
        if sha256(&source) != preview.model_sha256 {
            return Err(error("Quint candidate changed after review"));
        }
        preview.image.read(&self.root)?;
        let name = format!("{}.qnt", preview.model_sha256);
        self.write(&name, &source, MODEL_LIMIT)?;
        Ok(self.root.join(self.name(&name)))
    }

    pub(crate) fn approved(&self, preview: &Preview) -> Result<PathBuf> {
        let name = format!("{}.qnt", preview.model_sha256);
        if sha256(&self.read_model(&name)?) != preview.model_sha256 {
            return Err(error(
                "Approved Quint model changed; reset the execution for a new review",
            ));
        }
        Ok(self.root.join(self.name(&name)))
    }

    pub(crate) fn prior_candidate(&self) -> String {
        self.read_model("candidate.qnt")
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    }
}

pub(crate) struct Agent<'a> {
    pub(crate) workflow: &'a CompiledWorkflow,
    pub(crate) workspace: &'a Path,
    pub(crate) token: &'a CancellationToken,
    pub(crate) session: &'a str,
    pub(crate) retries: usize,
}

impl Agent<'_> {
    async fn run(
        &self,
        prompt: &str,
        step: &str,
        read_only: bool,
        inputs: Vec<PathBuf>,
    ) -> Result<String> {
        let workflow = self.workflow;
        let executor = Executor::new(workflow.sdk.as_deref(), &workflow.command);
        let permission = if read_only {
            PermissionMode::ReadOnly
        } else {
            workflow.permission
        };
        let tool_policy = crate::tool_policy::ToolPolicy::for_prompt_step(
            &workflow.disallowed_tools,
            &[],
            false,
            permission,
        );
        let mcp_servers = tool_policy.filter_mcp_servers(&workflow.mcp_servers);
        let guard = crate::commit_guard::CommitGuard::prepare(Some(self.workspace), &workflow.env)?;
        let result = {
            let env = guard.as_ref().map_or(&workflow.env, |guard| guard.env());
            let token = self.token.child_token();
            let run = executor.run_with_jcode_access(
                PromptRun {
                    usage: Some(crate::usage::UsageScope {
                        session_id: Some(self.session),
                        step: Some(step),
                        purpose: crate::usage::UsagePurpose::WorkflowStep,
                    }),
                    prompt,
                    model: workflow.model.as_ref(),
                    subagent_model: workflow.subagent_model.as_ref(),
                    max_retries: self.retries,
                    env,
                    mcp_servers: &mcp_servers,
                    on_notice: None,
                    cancel_token: Some(&token),
                    working_dir: Some(self.workspace),
                    stream: None,
                    tools: Vec::new(),
                    on_session_id: None,
                    resume: None,
                    computer_use: !read_only && workflow.computer_use,
                    permission,
                    tool_policy,
                },
                crate::backend::jcode::SandboxAccess {
                    read_only,
                    inputs,
                    ..Default::default()
                },
            );
            tokio::pin!(run);
            if let Ok(result) = tokio::time::timeout(Duration::from_mins(20), &mut run).await {
                result
            } else {
                token.cancel();
                let _ = run.await;
                Err(error(format!("{step} timed out")))
            }
        };
        let response = match guard {
            Some(guard) => guard.finish(result),
            None => result,
        }?;
        Ok(response.result.output)
    }

    pub(crate) async fn draft(
        &self,
        vars: &crate::variable::VariableStore,
        files: &Files,
        feedback: Option<&str>,
    ) -> Result<String> {
        let plan = vars
            .named_file(crate::session::PLAN_VAR)
            .filter(|path| path.is_file());
        let mut prompt = vars.resolve(include_str!("../prompts/quint-draft.md"))?;
        if let Some(feedback) = feedback {
            use std::fmt::Write as _;
            let _ = write!(
                prompt,
                "\n\nPrior candidate:\n{}\n\nRevision request or validation feedback:\n{feedback}",
                files.prior_candidate()
            );
        }
        let output = self
            .run(
                &prompt,
                "quint-draft",
                true,
                plan.map(Path::to_path_buf).into_iter().collect(),
            )
            .await?;
        let extracted = crate::formal_verify::extract_blocks(&output);
        let [block] = extracted.blocks.as_slice() else {
            return Err(error(
                "Quint draft must return exactly one closed quint code block",
            ));
        };
        if !extracted.over_limit.is_empty()
            || block.language != crate::formal_verify::Language::Quint
            || block.source.len() > MODEL_LIMIT
        {
            return Err(error("Quint draft must return one bounded Quint model"));
        }
        files.store_candidate(&block.source)
    }

    pub(crate) async fn finish(
        &self,
        files: &Files,
        preview: &Preview,
    ) -> Result<evidence::Receipt> {
        evidence::finish(self, files, preview).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formal_mode_cannot_change_until_reset() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let config = serde_yaml::from_str(
            "flags:\n  formal_methods:\n    quint_review: true\nsteps:\n  implement:\n    command: echo ok\n",
        )?;
        let mut state = SessionState::new(
            "test".into(),
            ".".into(),
            crate::session_config::SessionConfigRef::InlineSnapshot,
            "task".into(),
        );
        configure(&config, &mut state)?;
        assert!(matches!(state.quint_run, Some(Run::Off)));
        state.flags.insert(FLAG.into(), true);
        assert!(configure(&config, &mut state).is_err());
        state.reset_to_planned();
        configure(&config, &mut state)?;
        assert!(matches!(
            state.quint_run,
            Some(Run::On {
                stage: Stage::Draft { .. }
            })
        ));
        state.flags.insert(FLAG.into(), false);
        assert!(configure(&config, &mut state).is_err());
        Ok(())
    }
}
