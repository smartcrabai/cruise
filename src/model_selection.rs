//! Jcode model selection and scheduled availability.
use crate::config::{ModelEntry, ModelSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sdk {
    Jcode,
}

impl Sdk {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        "jcode"
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        (name == "jcode").then_some(Self::Jcode)
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

/// Resolve a single entry against the workflow's jcode SDK.
#[must_use]
pub fn resolve_entry(entry: &ModelEntry, sdk: Sdk) -> ModelCandidate {
    ModelCandidate {
        sdk,
        model: Some(entry.model().to_owned()),
    }
}
/// User-declared unavailability keyed by jcode SDK and effort-independent model identity.
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
                let format = crate::retry::ModelReferenceFormat::for_sdk(base);
                let model = crate::retry::ModelReferenceFormat::split(format, &object.model)
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
                availability.entries.push((base, model, windows));
            }
        }
    }
    Ok(availability)
}

impl ModelAvailability {
    pub(crate) fn disabled_by(&self, sdk: Sdk, model: &str, unix_secs: i64) -> Option<&str> {
        let format = crate::retry::ModelReferenceFormat::for_sdk(sdk);
        let base = crate::retry::ModelReferenceFormat::split(format, model).0;
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

/// Resolve strings against the workflow's jcode SDK, never the preceding entry.
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

/// A matching step override retains the workflow's model chain.
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
                        crate::retry::ModelReferenceFormat::split(format, primary).0
                            == crate::retry::ModelReferenceFormat::split(format, model).0
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
        let primary = "test-window-step/model:low";
        let overridden = "test-window-step/model:high";
        let workflow: ModelSpec = serde_json::from_value(serde_json::json!([
            {"model": primary, "disabled_during": ["00:00-24:00"]},
            "test-window-step/spare"
        ]))
        .unwrap_or_else(|error| panic!("disabled_during fixture must deserialize: {error}"));
        let selected = step_model_spec(Some(&workflow), Some(overridden), Sdk::Jcode)
            .unwrap_or_else(|| panic!("matching base identity retains the workflow spec"));
        let mut expected =
            serde_json::to_value(&workflow).unwrap_or_else(|error| panic!("{error}"));
        expected[0]["model"] = serde_json::json!(overridden);
        assert_eq!(
            serde_json::to_value(&selected).unwrap_or_else(|error| panic!("{error}")),
            expected
        );
        let candidates = resolve_candidates(Some(&selected), Sdk::Jcode);
        assert_eq!(candidates[0].sdk, Sdk::Jcode);
        assert_eq!(candidates[0].model.as_deref(), Some(overridden));
        assert_eq!(candidates[1].sdk, Sdk::Jcode);
    }

    #[test]
    fn disabled_window_is_applied_to_the_single_sdk() {
        let workflow: ModelSpec = serde_json::from_value(serde_json::json!([
            {"model": "test-window-identity/model:low", "disabled_during": ["00:00-24:00"]},
            "test-window-identity/spare"
        ]))
        .unwrap_or_else(|error| panic!("disabled_during fixture must deserialize: {error}"));
        let availability =
            availability(Some(&workflow), Sdk::Jcode).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            availability.disabled_by(Sdk::Jcode, "test-window-identity/model:high", 0),
            Some("00:00-24:00")
        );
    }

    #[test]
    fn resolve_entry_uses_the_workflow_sdk_for_every_entry() {
        let string = ModelEntry::String("worker".to_owned());
        let scheduled = ModelEntry::SdkModel(crate::config::SdkModel {
            sdk: None,
            model: "worker".to_owned(),
            disabled_during: vec!["00:00-24:00".to_owned()],
        });
        for entry in [&string, &scheduled] {
            assert_eq!(
                resolve_entry(entry, Sdk::Jcode),
                ModelCandidate {
                    sdk: Sdk::Jcode,
                    model: Some("worker".to_owned())
                }
            );
        }
    }
}
