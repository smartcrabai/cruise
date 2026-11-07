use indexmap::IndexMap;
use serde::Serialize;

use crate::cli::ExecArgs;
use crate::config::DEFAULT_MAX_RETRIES;
use crate::error::{CruiseError, Result};

pub(crate) const DEFAULT_REVIEW_ROUNDS: usize = 2;
const IMPLEMENT_PROMPT: &str = include_str!("../prompts/instant-implement.md");
const REVIEW_PROMPT: &str = include_str!("../prompts/instant-review.md");

#[derive(Serialize)]
struct Workflow<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    sdk: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_retries: Option<usize>,
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    groups: IndexMap<&'static str, Group>,
    steps: IndexMap<String, Step<'a>>,
}

#[derive(Serialize)]
struct Group {
    #[serde(rename = "if")]
    condition: Condition,
    max_retries: usize,
    steps: IndexMap<&'static str, Step<'static>>,
}

#[derive(Serialize)]
struct Condition {
    #[serde(rename = "file-changed")]
    file_changed: &'static str,
}

#[derive(Serialize)]
struct Step<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<&'a str>,
}

pub(crate) fn build_yaml(args: &ExecArgs) -> Result<String> {
    if args.input.as_deref().unwrap_or_default().trim().is_empty() {
        return Err(CruiseError::Other(
            "instant execution requires a non-empty task".into(),
        ));
    }
    if args.review_rounds == Some(0) {
        return Err(CruiseError::Other(
            "review rounds must be greater than zero".into(),
        ));
    }
    if args.verify.iter().any(|command| command.trim().is_empty()) {
        return Err(CruiseError::Other(
            "verify command must not be empty".into(),
        ));
    }
    if args
        .verify
        .iter()
        .any(|command| command.contains(['{', '}']))
    {
        return Err(CruiseError::Other(
            "verify command must not contain braces or workflow variables".into(),
        ));
    }
    let rounds = args.review_rounds.unwrap_or(DEFAULT_REVIEW_ROUNDS);
    let mut steps = IndexMap::new();
    steps.insert(
        "implement".to_string(),
        Step {
            prompt: Some(IMPLEMENT_PROMPT),
            command: None,
            group: None,
        },
    );
    let mut groups = IndexMap::new();
    if args.review {
        let mut review_steps = IndexMap::new();
        review_steps.insert(
            "review",
            Step {
                prompt: Some(REVIEW_PROMPT),
                command: None,
                group: None,
            },
        );
        groups.insert(
            "review-loop",
            Group {
                condition: Condition {
                    file_changed: "review-pass/review",
                },
                max_retries: rounds,
                steps: review_steps,
            },
        );
        steps.insert(
            "review-pass".to_string(),
            Step {
                prompt: None,
                command: None,
                group: Some("review-loop"),
            },
        );
    }
    for (index, command) in args.verify.iter().enumerate() {
        steps.insert(
            format!("verify-{}", index + 1),
            Step {
                prompt: None,
                command: Some(command),
                group: None,
            },
        );
    }
    let workflow = Workflow {
        sdk: args.sdk.as_deref(),
        model: args.model.as_deref(),
        max_retries: (args.review && rounds > DEFAULT_MAX_RETRIES).then_some(rounds),
        groups,
        steps,
    };
    serde_yaml::to_string(&workflow).map_err(|error| {
        CruiseError::Other(format!("failed to serialize instant workflow: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{load_config_from_source, ConfigSource};

    #[test]
    fn generated_inline_workflow_contract() {
        let mut args = ExecArgs { input: Some("task".into()), instant: true, sdk: None, model: None,
            review: false, review_rounds: None, verify: vec![], config: None,
            max_retries: None, rate_limit_retries: 0, dry_run: false, flags: vec![] };
        let yaml = build_yaml(&args).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        for key in ["sdk", "model", "max_retries", "groups"] { assert!(value.get(key).is_none(), "{yaml}"); }
        let config = load_config_from_source(&yaml, &ConfigSource::Inline).unwrap();
        assert_eq!(config.steps.keys().map(String::as_str).collect::<Vec<_>>(), ["implement"]);
        assert_eq!(config.steps["implement"].prompt.as_deref(), Some(IMPLEMENT_PROMPT));
        for prompt in [IMPLEMENT_PROMPT, REVIEW_PROMPT] {
            assert_eq!(prompt.matches("{input}").count(), 1);
            assert_eq!(prompt.matches('{').count(), 1);
            assert_eq!(prompt.matches('}').count(), 1);
        }
        args.review = true;
        args.review_rounds = Some(5);
        args.verify = vec!["echo one".into(), "echo two".into()];
        let yaml = build_yaml(&args).unwrap();
        let config = load_config_from_source(&yaml, &ConfigSource::Inline).unwrap();
        assert_eq!(config.steps.keys().map(String::as_str).collect::<Vec<_>>(), ["implement", "review-pass", "verify-1", "verify-2"]);
        assert_eq!(config.max_retries, Some(5));
        assert_eq!(config.groups["review-loop"].max_retries, Some(5));
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(value["steps"]["verify-1"]["command"].as_str(), Some("echo one"));
        assert_eq!(value["steps"]["verify-2"]["command"].as_str(), Some("echo two"));
    }
}
