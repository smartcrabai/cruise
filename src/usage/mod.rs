use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

pub mod summary;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageCounts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl UsageCounts {
    pub(crate) fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.reasoning_tokens.is_none()
            && self.cache_read_input_tokens.is_none()
            && self.cache_creation_input_tokens.is_none()
            && self.cost_usd.is_none()
    }
    fn add_complete(&mut self, rhs: &Self) {
        macro_rules! sum { ($($field:ident),*) => { $(self.$field = self.$field.zip(rhs.$field).and_then(|(a,b)| a.checked_add(b));)* }; }
        sum!(
            input_tokens,
            output_tokens,
            reasoning_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens
        );
        self.cost_usd = self
            .cost_usd
            .zip(rhs.cost_usd)
            .map(|(a, b)| a + b)
            .filter(|v| v.is_finite());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageRound {
    pub model: Option<String>,
    pub counts: UsageCounts,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UsageAccumulator {
    totals: UsageCounts,
    groups: BTreeMap<Option<String>, UsageCounts>,
    rounds: u64,
}
impl UsageAccumulator {
    pub fn add(&mut self, round: UsageRound) {
        if round.counts.is_empty() {
            return;
        }
        if self.rounds == 0 {
            self.totals = round.counts.clone();
        } else {
            self.totals.add_complete(&round.counts);
        }
        self.rounds += 1;
        self.groups
            .entry(round.model)
            .and_modify(|c| c.add_complete(&round.counts))
            .or_insert(round.counts);
    }
    pub fn into_field(self, unreported_reason: &'static str) -> UsageField {
        if self.rounds == 0 {
            return UsageField::Unavailable {
                reason: unreported_reason,
            };
        }
        let by_model = if self.groups.keys().any(Option::is_some) {
            Some(self.groups.into_iter().collect())
        } else {
            None
        };
        UsageField::Reported {
            totals: self.totals,
            by_model,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UsageField {
    Reported {
        totals: UsageCounts,
        by_model: Option<Vec<(Option<String>, UsageCounts)>>,
    },
    Unavailable {
        reason: &'static str,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum UsagePurpose {
    WorkflowStep,
    Planning,
    PlanTitle,
    PrMetadata,
    WorkflowGenerate,
}
impl UsagePurpose {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowStep => "workflow_step",
            Self::Planning => "planning",
            Self::PlanTitle => "plan_title",
            Self::PrMetadata => "pr_metadata",
            Self::WorkflowGenerate => "workflow_generate",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct UsageScope<'a> {
    pub session_id: Option<&'a str>,
    pub step: Option<&'a str>,
    pub purpose: UsagePurpose,
}

#[derive(Serialize)]
struct ModelUsage<'a> {
    model: &'a Option<String>,
    #[serde(flatten)]
    counts: &'a UsageCounts,
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum SerializedUsage<'a> {
    Reported {
        #[serde(flatten)]
        totals: &'a UsageCounts,
        #[serde(skip_serializing_if = "Option::is_none")]
        by_model: Option<Vec<ModelUsage<'a>>>,
    },
    Unavailable {
        reason: &'a str,
    },
}
#[derive(Serialize)]
struct UsageRecord<'a> {
    v: u8,
    ts: String,
    session_id: Option<&'a str>,
    step: Option<&'a str>,
    purpose: &'a str,
    backend: &'a str,
    model: Option<&'a str>,
    attempt: u64,
    outcome: &'a str,
    usage: SerializedUsage<'a>,
}

pub(crate) fn usage_path() -> crate::error::Result<PathBuf> {
    Ok(crate::paths::state_dir()?.join("usage.ndjson"))
}
static SWITCH_WARNING: OnceLock<()> = OnceLock::new();
static WRITE_WARNING: OnceLock<()> = OnceLock::new();

fn recording_enabled() -> bool {
    match std::env::var("CRUISE_USAGE_RECORDING") {
        Err(_) => true,
        Ok(value) => {
            let value = value.trim();
            if value == "0" || value.eq_ignore_ascii_case("false") {
                false
            } else if value == "1" || value.eq_ignore_ascii_case("true") {
                true
            } else {
                SWITCH_WARNING.get_or_init(|| {
                    eprintln!("warning: invalid CRUISE_USAGE_RECORDING value, recording enabled");
                });
                true
            }
        }
    }
}
fn append_record(path: &Path, record: &UsageRecord<'_>) -> crate::error::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| crate::error::CruiseError::Other("invalid usage path".into()))?;
    let mut dirs = std::fs::DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dirs.mode(0o700);
    }
    dirs.create(parent)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}
pub(crate) fn record_attempt(
    scope: UsageScope<'_>,
    backend: &str,
    model: Option<&str>,
    attempt: u64,
    outcome: &str,
    usage: &UsageField,
) {
    if !recording_enabled() {
        return;
    }
    let serialized = match usage {
        UsageField::Unavailable { reason } => SerializedUsage::Unavailable { reason },
        UsageField::Reported { totals, by_model } => SerializedUsage::Reported {
            totals,
            by_model: by_model.as_ref().map(|groups| {
                groups
                    .iter()
                    .map(|(model, counts)| ModelUsage { model, counts })
                    .collect()
            }),
        },
    };
    let record = UsageRecord {
        v: 1,
        ts: crate::session::current_iso8601(),
        session_id: scope.session_id,
        step: scope.step,
        purpose: scope.purpose.as_str(),
        backend,
        model,
        attempt,
        outcome,
        usage: serialized,
    };
    record_attempt_with_warn_guard(&record, usage_path(), &WRITE_WARNING);
}

fn record_attempt_with_warn_guard(
    record: &UsageRecord<'_>,
    path: crate::error::Result<PathBuf>,
    warning: &OnceLock<()>,
) {
    let result = path.and_then(|path| append_record(&path, record));
    if let Err(error) = result {
        warning.get_or_init(|| eprintln!("warning: usage record skipped: {error}"));
    }
}
