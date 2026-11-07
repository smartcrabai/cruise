use super::UsageCounts;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum GroupDim {
    Session,
    Step,
    Purpose,
    Backend,
    Model,
}
impl GroupDim {
    pub fn name(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Step => "step",
            Self::Purpose => "purpose",
            Self::Backend => "backend",
            Self::Model => "model",
        }
    }
}
#[derive(Deserialize)]
struct Record {
    session_id: Option<String>,
    step: Option<String>,
    purpose: String,
    backend: String,
    model: Option<String>,
    usage: InputUsage,
}
#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum InputUsage {
    Reported {
        #[serde(flatten)]
        counts: UsageCounts,
        by_model: Option<Vec<ModelUsage>>,
    },
    Unavailable {
        reason: String,
    },
}
#[derive(Deserialize)]
struct ModelUsage {
    model: Option<String>,
    #[serde(flatten)]
    counts: UsageCounts,
}
#[derive(Default, Serialize)]
struct Row {
    calls: u64,
    unavailable_calls: u64,
    reported_calls: u64,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cost_usd: Option<f64>,
}
impl Row {
    fn add(&mut self, counts: Option<&UsageCounts>) {
        self.calls += 1;
        if let Some(c) = counts {
            self.reported_calls += 1;
            macro_rules! add { ($($field:ident),*) => { $(if let Some(v) = c.$field { self.$field = Some(self.$field.unwrap_or(0).saturating_add(v)); })* }; }
            add!(
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
                reasoning_tokens
            );
            if let (Some(i), Some(o)) = (c.input_tokens, c.output_tokens) {
                self.total_tokens = Some(
                    self.total_tokens
                        .unwrap_or(0)
                        .saturating_add(i.saturating_add(o)),
                );
            }
            if let Some(v) = c.cost_usd {
                self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + v);
            }
        } else {
            self.unavailable_calls += 1;
        }
    }
}
#[derive(Serialize)]
pub struct Summary {
    schema: u8,
    group_by: Vec<&'static str>,
    rows: Vec<Map<String, Value>>,
    malformed_lines: u64,
    unsupported_version_lines: u64,
}
pub fn summarize(
    path: &Path,
    dims: &[GroupDim],
    session: Option<&str>,
) -> crate::error::Result<Summary> {
    let mut result = Summary {
        schema: 1,
        group_by: dims.iter().map(|d| d.name()).collect(),
        rows: vec![],
        malformed_lines: 0,
        unsupported_version_lines: 0,
    };
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e.into()),
    };
    let mut groups: BTreeMap<Vec<Option<String>>, Row> = BTreeMap::new();
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let value: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(_) => {
                result.malformed_lines += 1;
                continue;
            }
        };
        match value.get("v").and_then(Value::as_u64) {
            Some(1) => {}
            Some(_) => {
                result.unsupported_version_lines += 1;
                continue;
            }
            None => {
                result.malformed_lines += 1;
                continue;
            }
        }
        let record: Record = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(_) => {
                result.malformed_lines += 1;
                continue;
            }
        };
        if session.is_some_and(|s| record.session_id.as_deref() != Some(s)) {
            continue;
        }
        let key = |model: Option<&String>| {
            dims.iter()
                .map(|d| match d {
                    GroupDim::Session => record.session_id.clone(),
                    GroupDim::Step => record.step.clone(),
                    GroupDim::Purpose => Some(record.purpose.clone()),
                    GroupDim::Backend => Some(record.backend.clone()),
                    GroupDim::Model => model.cloned(),
                })
                .collect::<Vec<_>>()
        };
        match &record.usage {
            InputUsage::Unavailable { reason } => {
                let _ = reason;
                groups
                    .entry(key(record.model.as_ref()))
                    .or_default()
                    .add(None);
            }
            InputUsage::Reported { counts, by_model } => {
                if let Some(models) = by_model.as_ref().filter(|v| !v.is_empty())
                    && dims.contains(&GroupDim::Model)
                {
                    for model in models {
                        let default_model = "(default)".to_string();
                        groups
                            .entry(key(Some(model.model.as_ref().unwrap_or(&default_model))))
                            .or_default()
                            .add(Some(&model.counts));
                    }
                } else {
                    groups
                        .entry(key(record.model.as_ref()))
                        .or_default()
                        .add(Some(counts));
                }
            }
        }
    }
    for (key, row) in groups {
        let mut object = match serde_json::to_value(row)? {
            Value::Object(object) => object,
            _ => Default::default(),
        };
        for (dim, value) in dims.iter().zip(key) {
            object.insert(dim.name().into(), json!(value));
        }
        result.rows.push(object);
    }
    Ok(result)
}
const COLUMNS: &[&str] = &[
    "calls",
    "unavailable_calls",
    "reported_calls",
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
    "reasoning_tokens",
    "total_tokens",
    "cost_usd",
];
fn cell(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}
fn escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
impl Summary {
    pub fn json(&self) -> crate::error::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
    pub fn csv(&self) -> String {
        let cols: Vec<&str> = self
            .group_by
            .iter()
            .copied()
            .chain(COLUMNS.iter().copied())
            .collect();
        let mut lines = vec![cols.join(",")];
        for row in &self.rows {
            lines.push(
                cols.iter()
                    .map(|c| escape(&cell(row.get(*c))))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        lines.join("\n")
    }
    pub fn text(&self) -> String {
        let cols: Vec<&str> = self
            .group_by
            .iter()
            .copied()
            .chain([
                "calls",
                "unavailable_calls",
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
                "reasoning_tokens",
                "total_tokens",
                "cost_usd",
            ])
            .collect();
        let mut headers = self.group_by.clone();
        headers.extend([
            "calls", "unavailable", "input", "output", "cache_read", "cache_creation",
            "reasoning", "total", "cost_usd",
        ]);
        let mut lines = vec![headers.join("  ")];
        for row in &self.rows {
            lines.push(
                cols.iter()
                    .map(|c| {
                        let value = row.get(*c);
                        if *c == "cost_usd" {
                            if let Some(n) = value.and_then(Value::as_f64) {
                                return format!("{n:.4}");
                            }
                        }
                        let s = cell(value);
                        if s.is_empty() { "-".to_string() } else { s }
                    })
                    .collect::<Vec<_>>()
                    .join("  "),
            );
        }
        if self.rows.iter().any(|r| {
            r.get("unavailable_calls")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
        }) {
            lines.push("Reported values are a lower bound when calls are unavailable.".into());
        }
        lines.join("\n")
    }
}
