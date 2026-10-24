//! Read-only, workflow-scoped OAuth expiry preflight. Unknown credentials pass.
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer};

use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::model_selection::Sdk;

#[derive(Clone, Copy)]
pub(crate) enum AuthScope {
    Planning,
    Execution,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialTarget {
    ClaudeCli,
    JcodeAnthropic,
    JcodeOpenAi,
}

struct OAuthExpiry {
    access_expires_ms: Option<i64>,
    has_refresh: bool,
    refresh_expires_ms: Option<i64>,
}

#[derive(PartialEq, Eq)]
enum Verdict {
    Usable,
    Unknown,
    Expired(&'static str),
}

fn normalize_ms(value: i64) -> i64 {
    if value < 100_000_000_000 {
        value.saturating_mul(1000)
    } else {
        value
    }
}

fn judge(expiry: &OAuthExpiry, now_ms: i64) -> Verdict {
    let Some(access) = expiry.access_expires_ms else {
        return Verdict::Unknown;
    };
    if normalize_ms(access) > now_ms {
        Verdict::Usable
    } else if expiry
        .refresh_expires_ms
        .is_some_and(|expires| normalize_ms(expires) <= now_ms)
    {
        Verdict::Expired("refresh token expired")
    } else if expiry.has_refresh {
        Verdict::Usable
    } else {
        Verdict::Expired("access token expired and no refresh token")
    }
}

// Deliberately not Debug. Discard refresh token contents during deserialization.
#[derive(Default, Deserialize)]
#[serde(default)]
struct AnthropicAccount {
    expires: Option<i64>,
    #[serde(deserialize_with = "refresh_present")]
    refresh: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct OpenAiAccount {
    expires_at: Option<i64>,
    #[serde(deserialize_with = "refresh_present")]
    refresh_token: bool,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ClaudeOAuth {
    expires_at: Option<i64>,
    #[serde(deserialize_with = "refresh_present")]
    refresh_token: bool,
    refresh_token_expires_at: Option<i64>,
}

fn refresh_present<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<bool, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.is_some_and(|token| !token.is_empty()))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CredentialFile {
    #[serde(rename = "claudeAiOauth")]
    claude: Option<ClaudeOAuth>,
    anthropic_accounts: Option<Vec<AnthropicAccount>>,
    openai_accounts: Option<Vec<OpenAiAccount>>,
}

fn inspect_file(path: &Path, target: CredentialTarget, now_ms: i64) -> Verdict {
    let Some(file) = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CredentialFile>(&bytes).ok())
    else {
        return Verdict::Unknown;
    };
    match target {
        CredentialTarget::ClaudeCli => file.claude.map_or(Verdict::Unknown, |oauth| {
            judge(
                &OAuthExpiry {
                    access_expires_ms: oauth.expires_at,
                    has_refresh: oauth.refresh_token,
                    refresh_expires_ms: oauth.refresh_token_expires_at,
                },
                now_ms,
            )
        }),
        CredentialTarget::JcodeAnthropic => judge_accounts(
            file.anthropic_accounts
                .unwrap_or_default()
                .into_iter()
                .map(|account| OAuthExpiry {
                    access_expires_ms: account.expires,
                    has_refresh: account.refresh,
                    refresh_expires_ms: None,
                }),
            now_ms,
        ),
        CredentialTarget::JcodeOpenAi => judge_accounts(
            file.openai_accounts
                .unwrap_or_default()
                .into_iter()
                .map(|account| OAuthExpiry {
                    access_expires_ms: account.expires_at,
                    has_refresh: account.refresh_token,
                    refresh_expires_ms: None,
                }),
            now_ms,
        ),
    }
}

fn judge_accounts(accounts: impl Iterator<Item = OAuthExpiry>, now_ms: i64) -> Verdict {
    let mut expired = Verdict::Unknown;
    for account in accounts {
        match judge(&account, now_ms) {
            verdict @ Verdict::Expired(_) => expired = verdict,
            other => return other,
        }
    }
    expired
}

fn claude_uses_other_auth(config: &WorkflowConfig) -> bool {
    [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ]
    .iter()
    .any(|key| {
        std::env::var_os(key).is_some_and(|value| !value.is_empty())
            || config.env.get(*key).is_some_and(|value| !value.is_empty())
    })
}

fn claude_config_dir(config: &WorkflowConfig, cwd: Option<&Path>) -> Option<std::path::PathBuf> {
    let dir = if let Some(value) = config.env.get("CLAUDE_CONFIG_DIR") {
        // Use the workflow template syntax, including escaped literal braces.
        // Runtime variables are not available to this read-only startup check.
        crate::variable::resolve_template_with_lookup(value, |name| {
            Err(CruiseError::UndefinedVariable(name.to_string()))
        })
        .ok()?
        .into()
    } else {
        std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| home::home_dir().map(|home| home.join(".claude")))?
    };
    if dir.is_absolute() {
        Some(dir)
    } else {
        let base = cwd.map_or_else(std::env::current_dir, |cwd| Ok(cwd.to_path_buf())).ok()?;
        Some(base.join(dir))
    }
}

pub(crate) fn ensure_agents_authenticated(
    config: &WorkflowConfig,
    scope: AuthScope,
    cwd: Option<&Path>,
) -> Result<()> {
    if std::env::var("CRUISE_SKIP_AGENT_AUTH_CHECK").is_ok_and(|value| value == "1")
        || match config.sdk.as_deref() {
            None => !config.command.is_empty(),
            Some(sdk) => Sdk::from_name(sdk).is_none(),
        }
    {
        return Ok(());
    }
    let candidates = match scope {
        AuthScope::Planning => crate::config::planning_model_candidates(config),
        AuthScope::Execution => crate::config::execution_model_candidates(config),
    };
    let mut targets = Vec::new();
    for candidate in candidates {
        let target = match candidate.sdk {
            Sdk::Claude => Some(CredentialTarget::ClaudeCli),
            Sdk::Jcode => {
                let provider = crate::backend::jcode::parse_model_ref(candidate.model.as_deref())
                    .ok()
                    .and_then(|(provider, _, _)| provider);
                match provider.as_deref() {
                    Some("claude" | "claude-oauth" | "claude-subprocess") => {
                        Some(CredentialTarget::JcodeAnthropic)
                    }
                    Some("openai" | "openai-oauth") => Some(CredentialTarget::JcodeOpenAi),
                    _ => None,
                }
            }
            Sdk::OpenCode => None,
        };
        if let Some(target) = target.filter(|target| !targets.contains(target)) {
            targets.push(target);
        }
    }
    let Some(now_ms) = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
    else {
        return Ok(());
    };
    let mut failures = Vec::new();
    for target in targets {
        let (path, label, login) = match target {
            CredentialTarget::ClaudeCli => {
                if claude_uses_other_auth(config) {
                    continue;
                }
                let Some(dir) = claude_config_dir(config, cwd) else {
                    continue;
                };
                (
                    dir.join(".credentials.json"),
                    "claude CLI",
                    "claude auth login",
                )
            }
            CredentialTarget::JcodeAnthropic => (
                crate::backend::jcode::resolve_source_home(cwd).join("auth.json"),
                "jcode provider `claude`",
                "jcode login claude",
            ),
            CredentialTarget::JcodeOpenAi => (
                crate::backend::jcode::resolve_source_home(cwd).join("openai-auth.json"),
                "jcode provider `openai`",
                "jcode login openai",
            ),
        };
        if let Verdict::Expired(reason) = inspect_file(&path, target, now_ms) {
            failures.push(format!(
                "  - {label} ({}): {reason}; run `{login}`",
                path.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CruiseError::AgentAuthExpired(format!(
            "agent authentication expired; log in again before starting:\n{}\nSet CRUISE_SKIP_AGENT_AUTH_CHECK=1 to skip this check.",
            failures.join("\n")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_presence_schema_matrix_preserves_verdicts() {
        let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = temp.path().join("credentials.json");
        for (field, target, prefix, suffix) in [
            (
                "refreshToken", CredentialTarget::ClaudeCli,
                r#"{"claudeAiOauth":{"expiresAt":1"#, "}}",
            ),
            (
                "refresh", CredentialTarget::JcodeAnthropic,
                r#"{"anthropic_accounts":[{"expires":1"#, "}]}",
            ),
            (
                "refresh_token", CredentialTarget::JcodeOpenAi,
                r#"{"openai_accounts":[{"expires_at":1"#, "}]}",
            ),
        ] {
            for (value, blocked, malformed) in [
                (None, true, false),
                (Some("null"), true, false),
                (Some(r#""""#), true, false),
                (Some(r#""secret""#), false, false),
                (Some(r#""escaped\"\u0061""#), false, false),
                (Some("false"), false, true),
                (Some("123"), false, true),
                (Some("[]"), false, true),
                (Some("{}"), false, true),
            ] {
                let refresh = value.map_or_else(String::new, |value| format!(",\"{field}\":{value}"));
                let json = format!("{prefix}{refresh}{suffix}");
                assert_eq!(
                    serde_json::from_str::<CredentialFile>(&json).is_err(), malformed,
                );
                std::fs::write(&path, &json).unwrap_or_else(|e| panic!("{e}"));
                let verdict = inspect_file(&path, target, 1_800_000_000_000);
                assert_eq!(matches!(verdict, Verdict::Expired(_)), blocked);
                if malformed {
                    assert!(verdict == Verdict::Unknown);
                }
                assert_eq!(
                    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}")), json,
                );
            }
        }
    }

    #[test]
    fn backend_eligibility_preserves_executor_selection() {
        for sdk in [
            None, Some("jcode"), Some("claude"), Some("opencode"), Some("unsupported"), Some(""),
        ] {
            for command in [vec![], vec!["cat".to_string()]] {
                let bypass = match sdk {
                    None => !command.is_empty(),
                    Some(sdk) => Sdk::from_name(sdk).is_none(),
                };
                assert_eq!(
                    bypass, !crate::executor::Executor::new(sdk, &command).is_sdk(),
                );
            }
        }
    }

    #[test]
    fn expiry_matrix_at_fixed_time() {
        let now = 1_800_000_000_000;
        let cases = [
            (None, false, None, Verdict::Unknown),
            (Some(now + 1), false, None, Verdict::Usable),
            (Some(now + 1), true, Some(now - 1), Verdict::Usable),
            (Some(now), true, None, Verdict::Usable),
            (Some(now - 1), true, Some(now + 1), Verdict::Usable),
            (Some(now / 1000 + 1), false, None, Verdict::Usable),
            (
                Some(now),
                false,
                None,
                Verdict::Expired("access token expired and no refresh token"),
            ),
            (
                Some(now),
                true,
                Some(now),
                Verdict::Expired("refresh token expired"),
            ),
            (
                Some(now),
                true,
                Some(now / 1000),
                Verdict::Expired("refresh token expired"),
            ),
        ];
        for (access_expires_ms, has_refresh, refresh_expires_ms, expected) in cases {
            assert!(
                judge(
                    &OAuthExpiry {
                        access_expires_ms,
                        has_refresh,
                        refresh_expires_ms
                    },
                    now
                ) == expected
            );
        }
    }

    #[test]
    fn credential_files_are_read_only_and_unknown_accounts_pass() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = temp.path().join("auth.json");
        let now = 1_800_000_000_000;
        for (json, blocked) in [
            (
                r#"{"anthropic_accounts":[{"expires":1},{"expires":2}]}"#,
                true,
            ),
            (r#"{"anthropic_accounts":[{"expires":1},{}]}"#, false),
            (
                r#"{"anthropic_accounts":[{"expires":1},{"expires":"invalid"}]}"#,
                false,
            ),
            (
                r#"{"anthropic_accounts":[{"expires":1,"refresh":"secret"}]}"#,
                false,
            ),
            (r#"{"anthropic_accounts":[]}"#, false),
            ("{broken", false),
        ] {
            std::fs::write(&path, json).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                matches!(
                    inspect_file(&path, CredentialTarget::JcodeAnthropic, now),
                    Verdict::Expired(_)
                ),
                blocked
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}")),
                json
            );
            assert_eq!(
                std::fs::read_dir(temp.path())
                    .unwrap_or_else(|error| panic!("{error}"))
                    .count(),
                1
            );
        }
        assert!(
            inspect_file(
                &temp.path().join("missing"),
                CredentialTarget::ClaudeCli,
                now
            ) == Verdict::Unknown
        );
    }
}
