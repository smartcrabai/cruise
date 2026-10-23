//! Shared, lossless SDK-qualified model selection.
use crate::config::{ModelEntry, ModelSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sdk {
    Jcode,
    Claude,
    OpenCode,
}

impl Sdk {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jcode => "jcode",
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
        }
    }
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "jcode" => Some(Self::Jcode),
            "claude" => Some(Self::Claude),
            "opencode" => Some(Self::OpenCode),
            _ => None,
        }
    }
}
impl std::fmt::Display for Sdk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelCandidate {
    pub sdk: Sdk,
    pub model: Option<String>,
}
pub type ModelCandidates = Vec<ModelCandidate>;

/// An empty planning list is accepted configuration, not a model override.
pub(crate) fn planning_spec<'a>(
    plan: Option<&'a ModelSpec>,
    global: Option<&'a ModelSpec>,
) -> Option<&'a ModelSpec> {
    plan.filter(|spec| spec.primary().is_some()).or(global)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkSessionId {
    pub sdk: Sdk,
    pub id: String,
}

/// Collect owners only after explicit retry chains have replaced implicit tails.
pub(crate) fn applicable_sdks(
    config: &crate::config::WorkflowConfig,
    candidates: &[ModelCandidate],
    base_sdk: Sdk,
) -> std::collections::HashSet<Sdk> {
    crate::retry::effective_candidates(config.retry.as_ref(), candidates, base_sdk)
        .into_iter().map(|candidate| candidate.sdk).collect()
}

/// Resolve strings against the original workflow SDK, never the preceding entry.
#[must_use]
pub fn resolve_candidates(spec: Option<&ModelSpec>, sdk: Sdk) -> ModelCandidates {
    match spec {
        Some(ModelSpec::Single(model)) => vec![ModelCandidate {
            sdk,
            model: Some(model.clone()),
        }],
        Some(ModelSpec::Multiple(entries)) if !entries.is_empty() => entries
            .iter()
            .map(|entry| match entry {
                ModelEntry::String(model) => ModelCandidate {
                    sdk,
                    model: Some(model.clone()),
                },
                ModelEntry::SdkModel(entry) => ModelCandidate {
                    sdk: Sdk::from_name(&entry.sdk).unwrap_or(sdk),
                    model: Some(entry.model.clone()),
                },
            })
            .collect(),
        _ => vec![ModelCandidate { sdk, model: None }],
    }
}

/// A matching step override retains the workflow chain only within the same SDK.
#[must_use]
pub fn select_step_model(
    workflow: Option<&ModelSpec>,
    step: Option<&str>,
    sdk: Sdk,
) -> ModelCandidates {
    match step {
        None => resolve_candidates(workflow, sdk),
        Some(_) => resolve_candidates(step_model_spec(workflow, step, sdk).as_ref(), sdk),
    }
}

pub(crate) fn step_model_spec(
    workflow: Option<&ModelSpec>,
    step: Option<&str>,
    sdk: Sdk,
) -> Option<ModelSpec> {
    let candidates = resolve_candidates(workflow, sdk);
    match step {
        None => workflow.cloned(),
        Some(model)
            if candidates.first().is_some_and(|candidate| {
                candidate.sdk == sdk && candidate.model.as_deref().is_some_and(|primary| {
                    let format = crate::retry::ModelReferenceFormat::for_sdk(sdk);
                    format.split(primary).0 == format.split(model).0
                })
            }) =>
        {
            let mut selected = workflow.cloned()?;
            match &mut selected {
                ModelSpec::Single(primary) => *primary = model.to_owned(),
                ModelSpec::Multiple(entries) => match entries.first_mut()? {
                    ModelEntry::String(primary) => *primary = model.to_owned(),
                    ModelEntry::SdkModel(primary) => primary.model = model.to_owned(),
                },
            }
            Some(selected)
        }
        Some(model) => Some(ModelSpec::Single(model.to_owned())),
    }
}
