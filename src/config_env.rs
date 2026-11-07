//! Explicit allowlist and value grammar for configuration environment overrides.
use crate::config::{LanguagesConfig, RetryConfig, WorkflowConfig};
use crate::error::{CruiseError, Result};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvKind {
    Text,
    Bool,
    Count,
    Model,
    Json,
}

pub struct WorkflowEnvKey {
    pub var: &'static str,
    pub path: &'static str,
    pub kind: EnvKind,
    pub set: fn(&mut WorkflowConfig, Value) -> Result<()>,
}
pub struct AppEnvKey {
    pub var: &'static str,
    pub path: &'static str,
    pub kind: EnvKind,
}

/// Parse an override, treating whitespace-only values as absent.
///
/// # Errors
/// Returns an error when the value does not match the expected grammar.
pub fn parse_override(var: &'static str, kind: EnvKind, raw: &str) -> Result<Option<Value>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let invalid = |expected: &str| {
        CruiseError::Other(format!("invalid value for {var} (expected {expected})"))
    };
    let value = match kind {
        EnvKind::Text => Value::String(raw.to_owned()),
        EnvKind::Bool => match raw {
            "true" | "1" => Value::Bool(true),
            "false" | "0" => Value::Bool(false),
            _ => {
                return Err(CruiseError::Other(format!(
                    "invalid value for {var}: '{raw}' (expected true/false/1/0)"
                )));
            }
        },
        EnvKind::Count => {
            if !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid("a non-negative integer"));
            }
            Value::from(
                raw.parse::<u64>()
                    .map_err(|_| invalid("a non-negative integer"))?,
            )
        }
        EnvKind::Model if !raw.starts_with('[') && !raw.starts_with('{') => {
            Value::String(raw.to_owned())
        }
        EnvKind::Model | EnvKind::Json => {
            serde_json::from_str(raw).map_err(|_| invalid("valid JSON"))?
        }
    };
    Ok(Some(value))
}

fn decode<T: serde::de::DeserializeOwned>(var: &str, value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| {
        CruiseError::Other(format!(
            "invalid value for {var} (expected {})",
            std::any::type_name::<T>()
        ))
    })
}
fn retry(config: &mut WorkflowConfig) -> &mut RetryConfig {
    config.retry.get_or_insert_with(RetryConfig::default)
}
macro_rules! key {
    ($var:literal, $path:literal, $kind:ident, $field:ident) => {
        WorkflowEnvKey {
            var: $var,
            path: $path,
            kind: EnvKind::$kind,
            set: |c, v| {
                c.$field = decode($var, v)?;
                Ok(())
            },
        }
    };
    ($var:literal, $path:literal, $kind:ident, retry.$field:ident) => {
        WorkflowEnvKey {
            var: $var,
            path: $path,
            kind: EnvKind::$kind,
            set: |c, v| {
                let value = decode($var, v)?;
                retry(c).$field = value;
                Ok(())
            },
        }
    };
    ($var:literal, $path:literal, $kind:ident, languages.$field:ident) => {
        WorkflowEnvKey {
            var: $var,
            path: $path,
            kind: EnvKind::$kind,
            set: |c, v| {
                let value = decode($var, v)?;
                c.languages
                    .get_or_insert_with(LanguagesConfig::default)
                    .$field = Some(value);
                Ok(())
            },
        }
    };
}

pub static WORKFLOW_ENV_KEYS: &[WorkflowEnvKey] = &[
    key!("CRUISE_COMMAND", "command", Json, command),
    key!("CRUISE_SDK", "sdk", Text, sdk),
    key!("CRUISE_MODEL", "model", Model, model),
    key!("CRUISE_PLAN_MODEL", "plan_model", Model, plan_model),
    key!(
        "CRUISE_SUBAGENT_MODEL",
        "subagent_model",
        Model,
        subagent_model
    ),
    key!("CRUISE_MAX_RETRIES", "max_retries", Count, max_retries),
    key!(
        "CRUISE_INTERACTIVE_PLANNING",
        "interactive_planning",
        Bool,
        interactive_planning
    ),
    key!(
        "CRUISE_CLEANUP_AFTER_PR",
        "cleanup_after_pr",
        Bool,
        cleanup_after_pr
    ),
    key!("CRUISE_FORCE_EXEC", "force_exec", Bool, force_exec),
    key!("CRUISE_COMPUTER_USE", "computer_use", Bool, computer_use),
    key!("CRUISE_PERMISSION", "permission", Text, permission),
    key!(
        "CRUISE_DISALLOWED_TOOLS",
        "disallowed_tools",
        Json,
        disallowed_tools
    ),
    key!("CRUISE_RETRY", "retry", Json, retry),
    key!(
        "CRUISE_RETRY_BASE_DELAY_MS",
        "retry.base_delay_ms",
        Count,
        retry.base_delay_ms
    ),
    key!(
        "CRUISE_RETRY_MAX_DELAY_MS",
        "retry.max_delay_ms",
        Count,
        retry.max_delay_ms
    ),
    key!(
        "CRUISE_RETRY_MODEL_FALLBACK",
        "retry.model_fallback",
        Bool,
        retry.model_fallback
    ),
    key!(
        "CRUISE_RETRY_FALLBACK_CHAINS",
        "retry.fallback_chains",
        Json,
        retry.fallback_chains
    ),
    key!("CRUISE_LANGUAGES", "languages", Json, languages),
    key!("CRUISE_LANGUAGE_PR", "languages.pr", Text, languages.pr),
    key!(
        "CRUISE_LANGUAGE_PLAN",
        "languages.plan",
        Text,
        languages.plan
    ),
];
pub static APP_ENV_KEYS: &[AppEnvKey] = &[AppEnvKey {
    var: "CRUISE_RUN_ALL_PARALLELISM",
    path: "run_all_parallelism",
    kind: EnvKind::Count,
}];
pub static EXCLUDED_KEYS: &[(&str, &str)] = &[
    ("env", "named map and secrets"),
    ("mcp_servers", "named map and secrets"),
    ("pre_steps", "named workflow steps"),
    ("steps", "workflow structure"),
    ("groups", "workflow structure"),
    ("after-pr", "workflow structure"),
    ("inputs", "workflow structure"),
    ("description", "workflow identity"),
    ("flags", "workflow structure"),
    ("create_pr", "workflow contract"),
    ("pr_language", "deprecated"),
    ("plan_language", "deprecated"),
];
pub static PROCESS_ENV_NAMES: &[&str] = &[
    "CRUISE_CONFIG",
    "CRUISE_FORGE",
    "CRUISE_RUNTIME",
    "CRUISE_SKIP_AGENT_AUTH_CHECK",
    "CRUISE_DISABLE_NOTIFICATIONS",
    "CRUISE_DISABLE_HERDR",
    "CRUISE_OBSERVABILITY_ENABLED",
    "CRUISE_COMMIT_COAUTHOR_NAME",
    "CRUISE_COMMIT_COAUTHOR_EMAIL",
];

pub fn all_override_names() -> impl Iterator<Item = &'static str> {
    WORKFLOW_ENV_KEYS
        .iter()
        .map(|key| key.var)
        .chain(APP_ENV_KEYS.iter().map(|key| key.var))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn malformed_command_error_does_not_expose_value() {
        let secret = "recognizable-secret-123";
        let error = parse_override("CRUISE_COMMAND", EnvKind::Json, &format!("[\"{secret}\""))
            .err()
            .unwrap_or_else(|| panic!("expected error"))
            .to_string();
        assert!(error.contains("CRUISE_COMMAND"));
        assert!(error.contains("valid JSON"));
        assert!(!error.contains(secret));
    }

    #[test]
    fn wrong_type_command_error_does_not_expose_value() {
        let secret = "recognizable-secret-456";
        let value = parse_override("CRUISE_COMMAND", EnvKind::Json, &format!("\"{secret}\""))
            .unwrap_or_else(|error| panic!("{error:?}"))
            .unwrap_or_else(|| panic!("expected value"));
        let key = WORKFLOW_ENV_KEYS
            .iter()
            .find(|key| key.var == "CRUISE_COMMAND")
            .unwrap_or_else(|| panic!("expected command key"));
        let mut config =
            WorkflowConfig::from_yaml("command: [echo]\nsteps:\n  s1:\n    prompt: hi\n")
                .unwrap_or_else(|error| panic!("{error:?}"));
        let error = (key.set)(&mut config, value)
            .err()
            .unwrap_or_else(|| panic!("expected error"))
            .to_string();
        assert!(error.contains("CRUISE_COMMAND"));
        assert!(error.contains("Vec"));
        assert!(!error.contains(secret));
    }

    #[test]
    fn grammar_and_errors() {
        for (raw, expected) in [
            ("true", true),
            (" 1 ", true),
            ("false", false),
            ("0", false),
        ] {
            assert_eq!(
                parse_override("CRUISE_TEST", EnvKind::Bool, raw)
                    .unwrap_or_else(|error| panic!("{error:?}")),
                Some(Value::Bool(expected))
            );
        }
        for raw in ["True", "yes", "2"] {
            assert!(
                parse_override("CRUISE_TEST", EnvKind::Bool, raw)
                    .err()
                    .unwrap_or_else(|| panic!("expected error"))
                    .to_string()
                    .contains("CRUISE_TEST")
            );
        }
        for raw in ["-1", "+1", "1.5", "abc", "18446744073709551616"] {
            assert!(parse_override("CRUISE_TEST", EnvKind::Count, raw).is_err());
        }
        assert_eq!(
            parse_override("CRUISE_TEST", EnvKind::Count, "0")
                .unwrap_or_else(|error| panic!("{error:?}")),
            Some(Value::from(0))
        );
        assert_eq!(
            parse_override("CRUISE_TEST", EnvKind::Text, "  Japanese ")
                .unwrap_or_else(|error| panic!("{error:?}")),
            Some(Value::from("Japanese"))
        );
        assert_eq!(
            parse_override("CRUISE_TEST", EnvKind::Text, "  ")
                .unwrap_or_else(|error| panic!("{error:?}")),
            None
        );
        assert!(
            parse_override("CRUISE_TEST", EnvKind::Model, "[\"a\",\"b\"]")
                .unwrap_or_else(|error| panic!("{error:?}"))
                .unwrap_or_else(|| panic!("expected value"))
                .is_array()
        );
        assert!(
            parse_override(
                "CRUISE_TEST",
                EnvKind::Model,
                "{\"sdk\":\"claude\",\"model\":\"x\"}"
            )
            .unwrap_or_else(|error| panic!("{error:?}"))
            .unwrap_or_else(|| panic!("expected value"))
            .is_object()
        );
        assert!(parse_override("CRUISE_TEST", EnvKind::Model, "[\"a\"").is_err());
        assert_eq!(
            parse_override("CRUISE_TEST", EnvKind::Json, "null")
                .unwrap_or_else(|error| panic!("{error:?}")),
            Some(Value::Null)
        );
        assert!(parse_override("CRUISE_TEST", EnvKind::Json, "{").is_err());
    }

    #[test]
    fn names_are_unique_and_do_not_include_process_switches() {
        let mut names = HashSet::new();
        for name in all_override_names() {
            assert!(name.starts_with("CRUISE_"));
            assert!(names.insert(name), "duplicate {name}");
            assert!(!PROCESS_ENV_NAMES.contains(&name));
        }
        assert!(!names.contains("CRUISE_CREATE_PR"));
    }
}
