//! Auth preflight contracts exercised through the shared planning entry point.
//! A cancelled executor is a deterministic sentinel: preflight must reject bad
//! credentials first, while permissive cases reach cancellation without agents.

#[tokio::test]
async fn auth_planning_checks_only_planning_candidates() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        "auth.json",
        &json!({"anthropic_accounts": [{"expires": EXPIRED}]}),
    );
    assert_allowed(
        &fixture
            .plan("sdk: jcode\nmodel: claude/run-only\nplan_model: openai-api/planner")
            .await,
    );
}

#[tokio::test]
async fn auth_claude_default_home_credentials_are_checked() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    let _config_dir = EnvGuard::remove("CLAUDE_CONFIG_DIR");
    let claude_dir = fixture.dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        claude_dir.join(".credentials.json"),
        json!({"claudeAiOauth": {"expiresAt": EXPIRED}}).to_string(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_expired(&fixture.plan("sdk: claude").await);
}

#[tokio::test]
async fn auth_credential_read_error_is_unknown() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    // A directory cannot be read as JSON, even when tests run as root.
    std::fs::create_dir(fixture.dir.path().join(".credentials.json"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_allowed(&fixture.plan("sdk: claude").await);
}
use super::*;
use crate::cancellation::CancellationToken;
use crate::error::CruiseError;
use crate::test_support::{EnvGuard, lock_process, set_fake_home};
use serde_json::{Value, json};
use tempfile::TempDir;

const EXPIRED: i64 = 1_700_000_000_000;
const FUTURE: i64 = 8_000_000_000_000;

#[tokio::test]
async fn auth_workflow_claude_directory_overrides_inherited_credentials() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    let selected = fixture.dir.path().join("selected{literal}");
    std::fs::create_dir_all(&selected).unwrap_or_else(|e| panic!("{e}"));
    for (workflow_expiry, inherited_expiry) in [(FUTURE, EXPIRED), (EXPIRED, FUTURE)] {
        fixture.write(
            ".credentials.json",
            &json!({"claudeAiOauth": {"expiresAt": inherited_expiry}}),
        );
        std::fs::write(
            selected.join(".credentials.json"),
            json!({"claudeAiOauth": {"expiresAt": workflow_expiry}}).to_string(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        for directory in [
            "selected{{literal}}".to_string(),
            selected
                .to_string_lossy()
                .replace('{', "{{")
                .replace('}', "}}"),
        ] {
            let yaml = format!("sdk: claude\nenv:\n  CLAUDE_CONFIG_DIR: '{directory}'");
            let error = fixture.plan(&yaml).await;
            if workflow_expiry == EXPIRED {
                assert_expired(&error);
            } else {
                assert_allowed(&error);
            }
            let config =
                WorkflowConfig::from_yaml(&format!("{yaml}\nsteps:\n  work:\n    prompt: test\n"))
                    .unwrap_or_else(|e| panic!("{e}"));
            let execution = crate::agent_auth::ensure_agents_authenticated(
                &config,
                crate::agent_auth::AuthScope::Execution,
                Some(fixture.dir.path()),
            );
            assert_eq!(execution.is_err(), workflow_expiry == EXPIRED);
        }
    }
    fixture.write(
        ".credentials.json",
        &json!({"claudeAiOauth": {"expiresAt": EXPIRED}}),
    );
    let config = WorkflowConfig::from_yaml(
        "sdk: claude\nenv:\n  CLAUDE_CONFIG_DIR: '{prev.output}'\nsteps:\n  work:\n    prompt: test\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    for scope in [
        crate::agent_auth::AuthScope::Planning,
        crate::agent_auth::AuthScope::Execution,
    ] {
        assert!(
            crate::agent_auth::ensure_agents_authenticated(
                &config,
                scope,
                Some(fixture.dir.path()),
            )
            .is_ok()
        );
    }
}

const AUTH_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CRUISE_SKIP_AGENT_AUTH_CHECK",
];

struct Credentials {
    dir: TempDir,
    _env: Vec<EnvGuard>,
}

impl Credentials {
    // Caller holds the process lock until after this fixture is dropped.
    fn new() -> Self {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let mut env = set_fake_home(dir.path());
        env.extend(AUTH_ENV.iter().map(|key| EnvGuard::remove(key)));
        env.push(EnvGuard::set("JCODE_HOME", dir.path()));
        env.push(EnvGuard::set("CLAUDE_CONFIG_DIR", dir.path()));
        #[cfg(unix)]
        {
            let bin = dir.path().join("bin");
            std::fs::create_dir(&bin).unwrap_or_else(|e| panic!("{e}"));
            env.push(EnvGuard::set(
                "AUTH_CONTRACT_MARKER",
                dir.path().join("cli-invoked"),
            ));
            for cli in ["claude", "jcode", "opencode"] {
                crate::test_support::write_executable_script(
                    &bin.join(cli),
                    "#!/bin/sh\n: > \"$AUTH_CONTRACT_MARKER\"\nexit 97\n",
                );
            }
            env.push(crate::test_support::prepend_to_path(&bin));
        }
        Self { dir, _env: env }
    }

    fn write(&self, name: &str, value: &Value) {
        std::fs::write(self.dir.path().join(name), value.to_string())
            .unwrap_or_else(|e| panic!("{e}"));
    }

    async fn plan(&self, yaml: &str) -> CruiseError {
        let config = WorkflowConfig::from_yaml(&format!(
            "{yaml}\ninteractive_planning: false\nsteps:\n  work:\n    prompt: test\n"
        ))
        .unwrap_or_else(|e| panic!("invalid fixture: {e}"));
        let plan_path = self.dir.path().join("plan.md");
        let token = CancellationToken::new();
        token.cancel();
        let ctx = PlanPromptCtx {
            config: &config,
            ask: std::sync::Arc::new(crate::ask_handler::NoninteractiveAskHandler),
            plan_path: &plan_path,
            interactive: false,
            progress: PlanProgress::Hidden,
            rate_limit_retries: 0,
            working_dir: Some(self.dir.path()),
            grill: false,
            on_session_id: None,
            on_notice: None,
            cancel_token: Some(&token),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_plan_prompt_template(
                &ctx,
                &mut VariableStore::new("task".into()),
                "test",
                "auth contract",
                None,
                &mut None,
                false,
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("preflight must not launch an agent: {e}"));
        match result {
            Err(error) => error,
            Ok(_) => panic!("cancelled executor unexpectedly completed"),
        }
    }
}

fn assert_allowed(error: &CruiseError) {
    assert!(
        matches!(error, CruiseError::Interrupted),
        "expected executor cancellation after permissive preflight, got {error}"
    );
}

fn assert_expired(error: &CruiseError) {
    assert_eq!(
        error.telemetry_type(),
        "agent_auth_expired",
        "preflight must stop before executor cancellation: {error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn auth_preflight_does_not_launch_auth_commands() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        ".credentials.json",
        &json!({"claudeAiOauth": {"expiresAt": EXPIRED, "refreshToken": "refresh-secret"}}),
    );
    assert_allowed(&fixture.plan("sdk: claude").await);
    assert!(
        !fixture.dir.path().join("cli-invoked").exists(),
        "auth preflight must read credentials, not invoke an external CLI"
    );
}

macro_rules! claude_case {
    ($name:ident, $oauth:expr, $blocked:expr) => {
        #[tokio::test]
        async fn $name() {
            let _lock = lock_process();
            let fixture = Credentials::new();
            fixture.write(".credentials.json", &json!({"claudeAiOauth": $oauth}));
            let error = fixture.plan("sdk: claude\nmodel: sonnet").await;
            if $blocked { assert_expired(&error); } else { assert_allowed(&error); }
        }
    };
}

claude_case!(
    auth_valid_access_is_allowed,
    json!({"expiresAt": FUTURE}),
    false
);
claude_case!(
    auth_expired_access_with_refresh_is_allowed,
    json!({"expiresAt": EXPIRED, "refreshToken": "refresh-secret"}),
    false
);
claude_case!(
    auth_expired_access_without_refresh_is_rejected,
    json!({"expiresAt": EXPIRED}),
    true
);
claude_case!(
    auth_expired_refresh_is_rejected,
    json!({"expiresAt": EXPIRED, "refreshToken": "refresh-secret", "refreshTokenExpiresAt": EXPIRED}),
    true
);
claude_case!(
    auth_valid_access_with_expired_refresh_is_allowed,
    json!({"expiresAt": FUTURE, "refreshToken": "refresh-secret", "refreshTokenExpiresAt": EXPIRED}),
    false
);
claude_case!(
    auth_missing_expiry_is_unknown,
    json!({"refreshToken": "refresh-secret"}),
    false
);
claude_case!(
    auth_seconds_expiry_is_normalized,
    json!({"expiresAt": FUTURE / 1000}),
    false
);
claude_case!(
    auth_empty_refresh_is_not_refreshable,
    json!({"expiresAt": EXPIRED, "refreshToken": ""}),
    true
);

#[tokio::test]
async fn auth_missing_file_is_unknown() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    assert_allowed(&fixture.plan("sdk: claude").await);
}

#[tokio::test]
async fn auth_malformed_json_is_unknown() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    std::fs::write(fixture.dir.path().join(".credentials.json"), "{broken")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_allowed(&fixture.plan("sdk: claude").await);
}

#[tokio::test]
async fn auth_claude_process_api_auth_bypasses_oauth() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        ".credentials.json",
        &json!({"claudeAiOauth": {"expiresAt": EXPIRED}}),
    );
    for key in &AUTH_ENV[..6] {
        let _api = EnvGuard::set(key, "fixture-api-secret");
        assert_allowed(&fixture.plan("sdk: claude").await);
    }
}

#[tokio::test]
async fn auth_claude_workflow_api_auth_bypasses_oauth() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        ".credentials.json",
        &json!({"claudeAiOauth": {"expiresAt": EXPIRED}}),
    );
    for key in &AUTH_ENV[..6] {
        assert_allowed(
            &fixture
                .plan(&format!("sdk: claude\nenv:\n  {key}: fixture-api-secret"))
                .await,
        );
    }
}

#[tokio::test]
async fn auth_empty_api_key_does_not_bypass_expired_oauth() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        ".credentials.json",
        &json!({"claudeAiOauth": {"expiresAt": EXPIRED}}),
    );
    let _api = EnvGuard::set("ANTHROPIC_API_KEY", "");
    assert_expired(
        &fixture
            .plan("sdk: claude\nenv:\n  ANTHROPIC_API_KEY: ''")
            .await,
    );
}

macro_rules! jcode_case {
    ($name:ident, $model:expr, $accounts:expr, $blocked:expr) => {
        #[tokio::test]
        async fn $name() {
            let _lock = lock_process();
            let fixture = Credentials::new();
            fixture.write("auth.json", &json!({"anthropic_accounts": $accounts}));
            fixture.write("openai-auth.json", &json!({"openai_accounts": [{"expires_at": EXPIRED}]}));
            let error = fixture.plan(&format!("sdk: jcode\nmodel: {}", $model)).await;
            if $blocked { assert_expired(&error); } else { assert_allowed(&error); }
        }
    };
}

jcode_case!(
    auth_jcode_all_expired_accounts_rejected,
    "claude/x",
    json!([{"expires": EXPIRED}, {"expires": EXPIRED - 1000}]),
    true
);
jcode_case!(
    auth_jcode_one_valid_account_allows_planning,
    "claude/x",
    json!([{"expires": EXPIRED}, {"expires": FUTURE}]),
    false
);
jcode_case!(
    auth_jcode_unknown_account_allows_planning,
    "claude/x",
    json!([{"expires": EXPIRED}, {}]),
    false
);
jcode_case!(
    auth_jcode_refreshable_account_allows_planning,
    "claude/x",
    json!([{"expires": EXPIRED, "refresh": "refresh-secret"}]),
    false
);
jcode_case!(
    auth_jcode_empty_accounts_are_unknown,
    "claude/x",
    json!([]),
    false
);
jcode_case!(
    auth_jcode_bare_model_is_skipped,
    "x",
    json!([{"expires": EXPIRED}]),
    false
);
jcode_case!(
    auth_jcode_api_provider_is_skipped,
    "openai-api/x",
    json!([{"expires": EXPIRED}]),
    false
);
jcode_case!(
    auth_jcode_openrouter_is_skipped,
    "openrouter/x",
    json!([{"expires": EXPIRED}]),
    false
);
jcode_case!(
    auth_jcode_claude_oauth_alias_is_checked,
    "claude-oauth/x",
    json!([{"expires": EXPIRED}]),
    true
);
jcode_case!(
    auth_jcode_claude_subprocess_alias_is_checked,
    "claude-subprocess/x",
    json!([{"expires": EXPIRED}]),
    true
);

#[tokio::test]
async fn auth_jcode_openai_uses_openai_credentials() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        "auth.json",
        &json!({"anthropic_accounts": [{"expires": FUTURE}]}),
    );
    fixture.write(
        "openai-auth.json",
        &json!({"openai_accounts": [{"expires_at": EXPIRED}]}),
    );
    for provider in ["openai", "openai-oauth"] {
        assert_expired(
            &fixture
                .plan(&format!("sdk: jcode\nmodel: {provider}/x"))
                .await,
        );
    }
}

#[tokio::test]
async fn auth_jcode_openai_refresh_token_allows_planning() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        "openai-auth.json",
        &json!({"openai_accounts": [{"expires_at": EXPIRED, "refresh_token": "refresh-secret"}]}),
    );
    assert_allowed(&fixture.plan("sdk: jcode\nmodel: openai-oauth/x").await);
}

#[tokio::test]
async fn auth_command_backend_does_not_inspect_jcode_credentials() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        "auth.json",
        &json!({"anthropic_accounts": [{"expires": EXPIRED}]}),
    );
    assert_allowed(&fixture.plan("command: [echo]\nmodel: claude/x").await);
}

#[tokio::test]
async fn auth_opencode_is_skipped() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        "auth.json",
        &json!({"anthropic_accounts": [{"expires": EXPIRED}]}),
    );
    assert_allowed(&fixture.plan("sdk: opencode\nmodel: claude/x").await);
}

#[tokio::test]
async fn auth_opt_out_one_bypasses_expired_credentials() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        "auth.json",
        &json!({"anthropic_accounts": [{"expires": EXPIRED}]}),
    );
    let _skip = EnvGuard::set("CRUISE_SKIP_AGENT_AUTH_CHECK", "1");
    assert_allowed(&fixture.plan("sdk: jcode\nmodel: claude/x").await);
}

#[tokio::test]
async fn auth_opt_out_other_values_do_not_bypass() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        "auth.json",
        &json!({"anthropic_accounts": [{"expires": EXPIRED}]}),
    );
    let _skip = EnvGuard::set("CRUISE_SKIP_AGENT_AUTH_CHECK", "true");
    assert_expired(&fixture.plan("sdk: jcode\nmodel: claude/x").await);
}

#[tokio::test]
async fn auth_error_aggregates_expired_targets_and_remediation() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    let claude = json!({"claudeAiOauth": {"expiresAt": EXPIRED, "accessToken": "access-secret", "refreshToken": "refresh-secret", "refreshTokenExpiresAt": EXPIRED}});
    let jcode =
        json!({"openai_accounts": [{"expires_at": EXPIRED, "access_token": "openai-secret"}]});
    fixture.write(".credentials.json", &claude);
    fixture.write("openai-auth.json", &jcode);
    let error = fixture.plan("sdk: jcode\nplan_model:\n  - {sdk: claude, model: sonnet}\n  - {sdk: jcode, model: openai/x}").await;
    assert_expired(&error);
    let message = error.to_string();
    for fragment in [
        "claude auth login",
        "jcode login openai",
        "CRUISE_SKIP_AGENT_AUTH_CHECK=1",
    ] {
        assert!(
            message.contains(fragment),
            "missing remediation {fragment}: {message}"
        );
    }
}

#[tokio::test]
async fn auth_error_does_not_expose_credentials() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    fixture.write(
        ".credentials.json",
        &json!({"claudeAiOauth": {
            "expiresAt": EXPIRED, "accessToken": "access-secret",
            "refreshToken": "refresh-secret", "refreshTokenExpiresAt": EXPIRED
        }}),
    );
    let error = fixture.plan("sdk: claude").await;
    assert_expired(&error);
    for secret in ["access-secret", "refresh-secret"] {
        assert!(
            !error.to_string().contains(secret),
            "credential leaked in error"
        );
        assert!(
            !format!("{error:?}").contains(secret),
            "credential leaked in Debug"
        );
    }
}

#[tokio::test]
async fn auth_preflight_leaves_credential_files_unchanged() {
    let _lock = lock_process();
    let fixture = Credentials::new();
    let value = json!({"anthropic_accounts": [{"expires": EXPIRED, "refresh": "refresh-secret"}]});
    fixture.write("auth.json", &value);
    assert_allowed(&fixture.plan("sdk: jcode\nmodel: claude/x").await);
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("auth.json"))
            .unwrap_or_else(|e| panic!("{e}")),
        value.to_string()
    );
}
