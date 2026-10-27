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

/// Resolve a single entry against the original workflow SDK.
#[must_use]
pub fn resolve_entry(entry: &ModelEntry, sdk: Sdk) -> ModelCandidate {
    ModelCandidate {
        sdk: match entry {
            ModelEntry::String(_) => sdk,
            ModelEntry::SdkModel(entry) => {
                entry.sdk.as_deref().and_then(Sdk::from_name).unwrap_or(sdk)
            }
        },
        model: Some(entry.model().to_owned()),
    }
}

/// User-declared unavailability keyed by SDK and effort-independent model identity.
#[derive(Default, Clone)]
pub(crate) struct ModelAvailability {
    entries: Vec<(Sdk, String, ScheduledWindows)>,
}

type ScheduledWindows = Vec<(String, crate::model_schedule::TimeWindow)>;

pub(crate) fn availability(
    spec: Option<&ModelSpec>,
    base: Sdk,
) -> crate::error::Result<ModelAvailability> {
    let mut availability = ModelAvailability::default();
    if let Some(spec) = spec {
        spec.validate("model")?;
        if let ModelSpec::Multiple(entries) = spec {
            for entry in entries {
                let ModelEntry::SdkModel(object) = entry else {
                    continue;
                };
                if object.disabled_during.is_empty() {
                    continue;
                }
                let sdk = object.sdk.as_deref().and_then(Sdk::from_name).unwrap_or(base);
                let model = crate::retry::ModelReferenceFormat::for_sdk(sdk)
                    .split(&object.model)
                    .0
                    .to_owned();
                let windows = object
                    .disabled_during
                    .iter()
                    .map(|raw| {
                        crate::model_schedule::parse_time_window(raw)
                            .map(|window| (raw.clone(), window))
                    })
                    .collect::<crate::error::Result<Vec<_>>>()?;
                availability.entries.push((sdk, model, windows));
            }
        }
    }
    Ok(availability)
}

impl ModelAvailability {
    pub(crate) fn disabled_by(&self, sdk: Sdk, model: &str, unix_secs: i64) -> Option<&str> {
        let format = crate::retry::ModelReferenceFormat::for_sdk(sdk);
        let base = format.split(model).0;
        self.entries
            .iter()
            .filter(|(owner, candidate, _)| *owner == sdk && candidate == base)
            .flat_map(|(_, _, windows)| windows)
            .find(|(_, window)| window.contains(unix_secs))
            .map(|(raw, _)| raw.as_str())
    }
}

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
        .into_iter()
        .map(|candidate| candidate.sdk)
        .collect()
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
            .map(|entry| resolve_entry(entry, sdk))
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
                candidate.sdk == sdk
                    && candidate.model.as_deref().is_some_and(|primary| {
                        let format = crate::retry::ModelReferenceFormat::for_sdk(sdk);
                        format.split(primary).0 == format.split(model).0
                    })
            }) =>
        {
            let mut selected = workflow.cloned()?;
            match &mut selected {
                ModelSpec::Single(primary) => model.clone_into(primary),
                ModelSpec::Multiple(entries) => match entries.first_mut()? {
                    ModelEntry::String(primary) => model.clone_into(primary),
                    ModelEntry::SdkModel(primary) => model.clone_into(&mut primary.model),
                },
            }
            Some(selected)
        }
        Some(model) => Some(ModelSpec::Single(model.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_window_step_effort_override_preserves_object_and_fallbacks() {
        for (sdk, primary, overridden) in [
            (
                Sdk::Jcode,
                "test-window-step/model:low",
                "test-window-step/model:high",
            ),
            (
                Sdk::OpenCode,
                "provider/test-window-step:free:low",
                "provider/test-window-step:free:high",
            ),
            (
                Sdk::OpenCode,
                "provider/test-window-step:free#fast",
                "provider/test-window-step:free#careful",
            ),
        ] {
            let workflow: ModelSpec = serde_json::from_value(serde_json::json!([
                {"model": primary, "disabled_during": ["00:00-24:00"]},
                {"sdk": "claude", "model": "test-window-step/spare"}
            ]))
            .unwrap_or_else(|error| panic!("disabled_during fixture must deserialize: {error}"));
            let selected = step_model_spec(Some(&workflow), Some(overridden), sdk)
                .unwrap_or_else(|| panic!("matching base identity retains the workflow spec"));
            let mut expected =
                serde_json::to_value(&workflow).unwrap_or_else(|error| panic!("{error}"));
            expected[0]["model"] = serde_json::json!(overridden);
            assert_eq!(
                serde_json::to_value(&selected).unwrap_or_else(|error| panic!("{error}")),
                expected
            );
            let candidates = resolve_candidates(Some(&selected), sdk);
            assert_eq!(candidates[0].sdk, sdk);
            assert_eq!(candidates[0].model.as_deref(), Some(overridden));
            assert_eq!(candidates[1].sdk, Sdk::Claude);
        }
    }

    #[test]
    fn disabled_window_same_base_cross_sdk_override_does_not_inherit() {
        let workflow: ModelSpec = serde_json::from_value(serde_json::json!([
            {"sdk": "claude", "model": "test-window-cross-sdk/model:low", "disabled_during": ["00:00-24:00"]},
            "test-window-cross-sdk/spare"
        ]))
        .unwrap_or_else(|error| panic!("disabled_during fixture must deserialize: {error}"));
        let overridden = "test-window-cross-sdk/model:high";
        let selected = step_model_spec(Some(&workflow), Some(overridden), Sdk::Jcode)
            .unwrap_or_else(|| panic!("step override must select a standalone model"));
        assert_eq!(
            serde_json::to_value(&selected).unwrap_or_else(|error| panic!("{error}")),
            serde_json::json!(overridden)
        );
        assert_eq!(
            resolve_candidates(Some(&selected), Sdk::Jcode),
            [ModelCandidate { sdk: Sdk::Jcode, model: Some(overridden.to_owned()) }]
        );
        let availability = availability(Some(&selected), Sdk::Jcode)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(availability.disabled_by(Sdk::Jcode, overridden, 0).is_none());
    }

    #[test]
    fn resolve_entry_uses_workflow_sdk_for_strings_and_explicit_sdk_for_objects() {
        let string = ModelEntry::String("worker".to_owned());
        let object: ModelEntry = serde_yaml::from_str("{sdk: claude, model: haiku}")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            resolve_entry(&string, Sdk::Jcode),
            ModelCandidate {
                sdk: Sdk::Jcode,
                model: Some("worker".to_owned())
            }
        );
        assert_eq!(
            resolve_entry(&string, Sdk::Claude),
            ModelCandidate {
                sdk: Sdk::Claude,
                model: Some("worker".to_owned())
            }
        );
        assert_eq!(
            resolve_entry(&object, Sdk::Jcode),
            ModelCandidate {
                sdk: Sdk::Claude,
                model: Some("haiku".to_owned())
            }
        );
        let unknown = ModelEntry::SdkModel(crate::config::SdkModel {
            sdk: Some("unknown".to_owned()),
            model: " worker:free ".to_owned(),
            disabled_during: Vec::new(),
        });
        let string = ModelEntry::String(unknown.model().to_owned());
        for sdk in [Sdk::Jcode, Sdk::Claude, Sdk::OpenCode] {
            let candidate = resolve_entry(&unknown, sdk);
            assert_eq!(candidate, resolve_entry(&string, sdk));
            assert_eq!(candidate.sdk, sdk);
            assert_eq!(candidate.model.as_deref(), Some(" worker:free "));
        }
    }
}
