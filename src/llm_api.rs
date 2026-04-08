use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::LlmApiConfigYaml;
use crate::error::{CruiseError, Result};

pub const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o";
const MAX_TOOL_ROUNDS: usize = 5;

const ALLOWED_GIT_SUBCOMMANDS: &[&str] = &["diff", "log", "show", "rev-parse", "merge-base"];
const MAX_API_RETRIES: usize = 3;

/// Resolved LLM API configuration ready for use.
#[derive(Debug, Clone)]
pub struct LlmApiConfig {
    pub api_key: String,
    pub endpoint: String,
    pub model: String,
}

// -- Internal API types ----------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct ApiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ApiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: ApiFunctionCall,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct ApiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(serde::Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ApiMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolDef]>,
}

#[derive(serde::Serialize, Clone)]
struct ToolDef {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionDef,
}

#[derive(serde::Serialize, Clone)]
struct FunctionDef {
    name: &'static str,
    description: &'static str,
    parameters: serde_json::Value,
}

#[derive(serde::Deserialize, Debug)]
struct ChatResponse {
    choices: Vec<ApiChoice>,
}

#[derive(serde::Deserialize, Debug)]
struct ApiChoice {
    message: ApiResponseMessage,
}

#[derive(serde::Deserialize, Debug)]
struct ApiResponseMessage {
    #[expect(dead_code)]
    role: String,
    content: Option<String>,
    tool_calls: Option<Vec<ApiToolCall>>,
}

// -- Public functions ------------------------------------------------------

/// Resolve LLM API config from environment variables and/or config file values.
///
/// Priority: env vars > config file > defaults.
/// Returns `None` if no API key is available (API mode disabled).
#[must_use]
pub fn resolve_llm_api_config(config_llm: Option<&LlmApiConfigYaml>) -> Option<LlmApiConfig> {
    let api_key = std::env::var("CRUISE_LLM_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .or_else(|| {
            config_llm
                .and_then(|c| c.api_key.clone())
                .filter(|k| !k.is_empty())
        })?;

    let endpoint = std::env::var("CRUISE_LLM_ENDPOINT")
        .ok()
        .filter(|e| !e.is_empty())
        .or_else(|| config_llm.and_then(|c| c.endpoint.clone()))
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

    let model = std::env::var("CRUISE_LLM_MODEL")
        .ok()
        .filter(|m| !m.is_empty())
        .or_else(|| config_llm.and_then(|c| c.model.clone()))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    Some(LlmApiConfig {
        api_key,
        endpoint,
        model,
    })
}

/// Validate that `file_path` is within `working_dir` (path traversal protection).
///
/// Returns the resolved absolute path on success, or an error if the path
/// escapes the working directory.
fn validate_read_file_path(working_dir: &Path, file_path: &str) -> Result<PathBuf> {
    let raw = Path::new(file_path);
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        working_dir.join(raw)
    };

    let canonical_wd = working_dir.canonicalize().map_err(|e| {
        CruiseError::Other(format!(
            "cannot canonicalize working directory {}: {e}",
            working_dir.display()
        ))
    })?;

    // Canonicalize the candidate; fall back to lexical normalization for non-existent paths.
    let canonical_candidate = candidate
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(&candidate));

    if !canonical_candidate.starts_with(&canonical_wd) {
        return Err(CruiseError::Other(format!(
            "path traversal rejected: '{file_path}' escapes the working directory"
        )));
    }
    // Return the joined (non-canonicalized) path so the result starts_with
    // the original working_dir even on systems where it contains symlinks
    // (e.g. /var -> /private/var on macOS).
    Ok(candidate)
}

/// Validate that `subcommand` is in the read-only git subcommand allow-list.
///
/// Allowed: `diff`, `log`, `show`, `rev-parse`, `merge-base`.
fn validate_git_subcommand(subcommand: &str) -> Result<()> {
    if ALLOWED_GIT_SUBCOMMANDS.contains(&subcommand) {
        Ok(())
    } else {
        Err(CruiseError::Other(format!(
            "git subcommand '{}' is not allowed; permitted: {}",
            subcommand,
            ALLOWED_GIT_SUBCOMMANDS.join(", ")
        )))
    }
}

/// Reject extra git arguments that could redirect output to a file (e.g. `--output`, `-o`).
fn validate_git_args(args_str: &str) -> Result<()> {
    for token in args_str.split_whitespace() {
        if token == "--output"
            || token.starts_with("--output=")
            || token == "-o"
            || token.starts_with("-o=")
        {
            return Err(CruiseError::Other(format!(
                "git argument '{token}' is not allowed; output-redirecting flags are forbidden"
            )));
        }
    }
    Ok(())
}

/// Generate PR metadata (title, body) via LLM API with tool use.
///
/// Falls back gracefully; callers should handle `Err` by using the CLI path.
///
/// # Errors
///
/// Returns an error if the HTTP request fails or the API returns an unexpected response.
pub async fn generate_pr_metadata(
    config: &LlmApiConfig,
    plan_path: &Path,
    pr_language: &str,
    working_dir: &Path,
) -> Result<(String, String)> {
    let client = build_client(config)?;
    let tools = pr_tools();

    let plan_content = std::fs::read_to_string(plan_path)
        .map_err(|e| CruiseError::Other(format!("failed to read plan file: {e}")))?;

    let system_content = format!(
        "You are a helpful assistant that generates Pull Request metadata. \
         Use the available tools to inspect git diffs. \
         After gathering enough information, output ONLY the following block -- \
         no preamble, explanation, or commentary:\n\n\
         ---\ntitle: \"Write a concise PR title here\"\n---\n\
         Write the PR description here.\n\n\
         Write the title and description in {pr_language}."
    );

    let user_content = format!(
        "Generate PR metadata for the changes in this repository. \
         Use git_diff to inspect the changes (e.g. args: HEAD~1..HEAD).\n\n\
         Plan:\n{plan_content}"
    );

    let mut messages = vec![
        ApiMessage {
            role: "system".to_string(),
            content: Some(system_content),
            tool_calls: None,
            tool_call_id: None,
        },
        ApiMessage {
            role: "user".to_string(),
            content: Some(user_content),
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    for _ in 0..MAX_TOOL_ROUNDS {
        let req = ChatRequest {
            model: &config.model,
            messages: &messages,
            tools: Some(&tools),
        };
        let response = call_api_with_retry(&client, config, &req).await?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| CruiseError::Other("empty response from LLM API".to_string()))?;

        let assistant_msg = ApiMessage {
            role: "assistant".to_string(),
            content: choice.message.content.clone(),
            tool_calls: choice.message.tool_calls.clone(),
            tool_call_id: None,
        };
        messages.push(assistant_msg);

        match choice.message.tool_calls {
            None => {
                let text = choice
                    .message
                    .content
                    .ok_or_else(|| CruiseError::Other("LLM API returned no content".to_string()))?;
                return Ok(parse_api_pr_output(&text));
            }
            Some(tool_calls) => {
                for tc in &tool_calls {
                    let result = execute_tool_call(tc, working_dir).unwrap_or_else(|e| {
                        format!("Error executing tool {}: {e}", tc.function.name)
                    });
                    messages.push(ApiMessage {
                        role: "tool".to_string(),
                        content: Some(result),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                    });
                }
            }
        }
    }

    // Max tool rounds exceeded -- request final response without tools
    let req = ChatRequest {
        model: &config.model,
        messages: &messages,
        tools: None,
    };
    let response = call_api_with_retry(&client, config, &req).await?;
    let text = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| CruiseError::Other("empty response from LLM API".to_string()))?
        .message
        .content
        .ok_or_else(|| CruiseError::Other("LLM API returned no content".to_string()))?;
    Ok(parse_api_pr_output(&text))
}

/// Generate a session title via LLM API.
///
/// Falls back gracefully; callers should handle `Err` by using `derive_session_title`.
///
/// # Errors
///
/// Returns an error if the HTTP request fails or the API returns an unexpected response.
pub async fn generate_session_title(
    config: &LlmApiConfig,
    input: &str,
    plan_content: &str,
) -> Result<String> {
    let client = build_client(config)?;

    let system_content = "You are a helpful assistant that generates concise session titles. \
         Given a task description and a plan, output ONLY a short title (maximum 80 characters). \
         No preamble, explanation, quotes, or commentary -- just the title text.";

    let user_content = format!("Task: {input}\n\nPlan:\n{plan_content}");

    let req = ChatRequest {
        model: &config.model,
        messages: &[
            ApiMessage {
                role: "system".to_string(),
                content: Some(system_content.to_string()),
                tool_calls: None,
                tool_call_id: None,
            },
            ApiMessage {
                role: "user".to_string(),
                content: Some(user_content),
                tool_calls: None,
                tool_call_id: None,
            },
        ],
        tools: None,
    };

    let response = call_api_with_retry(&client, config, &req).await?;
    let title = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| CruiseError::Other("empty response from LLM API".to_string()))?
        .message
        .content
        .ok_or_else(|| CruiseError::Other("LLM API returned no content".to_string()))?;

    let trimmed: String = title.trim().chars().take(80).collect();
    Ok(trimmed)
}

// -- Private helpers -------------------------------------------------------

fn build_client(config: &LlmApiConfig) -> Result<reqwest::Client> {
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

    let mut headers = HeaderMap::new();
    let auth_value = format!("Bearer {}", config.api_key);
    let mut auth_header = HeaderValue::from_str(&auth_value)
        .map_err(|e| CruiseError::Other(format!("invalid API key (non-ASCII characters): {e}")))?;
    auth_header.set_sensitive(true);
    headers.insert(AUTHORIZATION, auth_header);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CruiseError::Other(format!("failed to build HTTP client: {e}")))
}

async fn call_api_with_retry(
    client: &reqwest::Client,
    config: &LlmApiConfig,
    request: &ChatRequest<'_>,
) -> Result<ChatResponse> {
    let url = format!("{}/chat/completions", config.endpoint.trim_end_matches('/'));

    for attempt in 1..=MAX_API_RETRIES {
        let resp = client
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| CruiseError::Other(format!("HTTP request failed: {e}")))?;

        let status = resp.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_API_RETRIES {
            let backoff = crate::step::command::calculate_backoff(attempt);
            eprintln!(
                "warning: LLM API rate limited, retrying in {}s...",
                backoff.as_secs()
            );
            tokio::time::sleep(backoff).await;
            continue;
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CruiseError::Other(format!(
                "LLM API error {status}: {body}"
            )));
        }

        return resp
            .json::<ChatResponse>()
            .await
            .map_err(|e| CruiseError::Other(format!("failed to parse API response: {e}")));
    }

    unreachable!("loop always returns before exhausting iterations")
}

fn execute_tool_call(tool_call: &ApiToolCall, working_dir: &Path) -> Result<String> {
    let args: serde_json::Value =
        serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
            CruiseError::Other(format!(
                "invalid tool arguments for '{}': {e}",
                tool_call.function.name
            ))
        })?;

    match tool_call.function.name.as_str() {
        "read_file" => {
            let path = args["path"].as_str().ok_or_else(|| {
                CruiseError::Other("read_file: missing required 'path' argument".to_string())
            })?;
            let validated = validate_read_file_path(working_dir, path)?;
            std::fs::read_to_string(&validated)
                .map_err(|e| CruiseError::Other(format!("failed to read file '{path}': {e}")))
        }
        "git_diff" => {
            let args_str = args["args"].as_str().unwrap_or_default();
            run_git_command(working_dir, "diff", args_str)
        }
        "git_log" => {
            let args_str = args["args"].as_str().unwrap_or_default();
            run_git_command(working_dir, "log", args_str)
        }
        other => Err(CruiseError::Other(format!("unknown tool '{other}'"))),
    }
}

fn run_git_command(working_dir: &Path, subcommand: &str, args_str: &str) -> Result<String> {
    validate_git_subcommand(subcommand)?;
    validate_git_args(args_str)?;
    let extra_args: Vec<&str> = args_str.split_whitespace().collect();
    let output = std::process::Command::new("git")
        .current_dir(working_dir)
        .arg(subcommand)
        .args(&extra_args)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to spawn git {subcommand}: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.is_empty() {
        Ok(String::from_utf8_lossy(&output.stderr).into_owned())
    } else {
        Ok(stdout.into_owned())
    }
}

fn pr_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            kind: "function",
            function: FunctionDef {
                name: "read_file",
                description: "Read the contents of a file at a relative path within the working directory",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file"
                        }
                    },
                    "required": ["path"]
                }),
            },
        },
        ToolDef {
            kind: "function",
            function: FunctionDef {
                name: "git_diff",
                description: "Run git diff to inspect changes in the repository",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "string",
                            "description": "Additional arguments for git diff (e.g. 'HEAD~1..HEAD')"
                        }
                    }
                }),
            },
        },
        ToolDef {
            kind: "function",
            function: FunctionDef {
                name: "git_log",
                description: "Run git log to inspect the commit history",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "string",
                            "description": "Additional arguments for git log (e.g. '--oneline -5')"
                        }
                    }
                }),
            },
        },
    ]
}

/// Lexically normalize a path (resolve `.` and `..` without filesystem access).
fn normalize_path(path: &Path) -> PathBuf {
    let mut components: Vec<std::path::Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if matches!(components.last(), Some(std::path::Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            std::path::Component::CurDir => {}
            _ => components.push(component),
        }
    }
    components.iter().collect()
}

/// Parse LLM output for PR metadata (frontmatter format).
/// Returns `(title, body)`.
fn parse_api_pr_output(text: &str) -> (String, String) {
    let text = text.trim();

    if let Some((title, body)) = crate::metadata::try_parse_frontmatter(text) {
        return (title, body.trim().to_string());
    }

    if let Some(pos) = text.find("\n---\n")
        && let Some((title, body)) = crate::metadata::try_parse_frontmatter(&text[pos + 1..])
    {
        return (title, body.trim().to_string());
    }

    (String::new(), text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmApiConfigYaml;
    use crate::test_support::{EnvGuard, lock_process};

    fn clean_env() -> (EnvGuard, EnvGuard, EnvGuard) {
        (
            EnvGuard::remove("CRUISE_LLM_API_KEY"),
            EnvGuard::remove("CRUISE_LLM_ENDPOINT"),
            EnvGuard::remove("CRUISE_LLM_MODEL"),
        )
    }

    // -- resolve_llm_api_config -------------------------------------------------

    #[test]
    fn test_resolve_llm_api_config_returns_none_when_no_api_key() {
        // Given: no API key in env, no config with api_key
        let _lock = lock_process();
        let _env = clean_env();

        // When: resolve_llm_api_config is called without any config
        let result = resolve_llm_api_config(None);

        // Then: returns None (API mode disabled)
        assert!(
            result.is_none(),
            "expected None when no API key is available"
        );
    }

    #[test]
    fn test_resolve_llm_api_config_uses_env_api_key() {
        // Given: CRUISE_LLM_API_KEY is set in env
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-env-key");

        // When: resolve_llm_api_config is called
        let result = resolve_llm_api_config(None);

        // Then: returns Some with the env key
        let config = result.unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));
        assert_eq!(config.api_key, "sk-env-key");
    }

    #[test]
    fn test_resolve_llm_api_config_uses_config_api_key_as_fallback() {
        // Given: no env API key, config file has api_key
        let _lock = lock_process();
        let _env = clean_env();
        let config_llm = LlmApiConfigYaml {
            api_key: Some("sk-config-key".to_string()),
            endpoint: None,
            model: None,
        };

        // When: resolve_llm_api_config is called with config
        let result = resolve_llm_api_config(Some(&config_llm));

        // Then: returns Some with the config key
        let config = result.unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));
        assert_eq!(config.api_key, "sk-config-key");
    }

    #[test]
    fn test_resolve_llm_api_config_env_key_overrides_config_key() {
        // Given: both env API key and config api_key are present
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-env-key");
        let config_llm = LlmApiConfigYaml {
            api_key: Some("sk-config-key".to_string()),
            endpoint: None,
            model: None,
        };

        // When: resolve_llm_api_config is called
        let result = resolve_llm_api_config(Some(&config_llm));

        // Then: env key takes precedence over config key
        let config = result.unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));
        assert_eq!(config.api_key, "sk-env-key");
    }

    #[test]
    fn test_resolve_llm_api_config_default_endpoint() {
        // Given: only API key set, no endpoint in env or config
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-test");

        // When: resolve_llm_api_config is called
        let config =
            resolve_llm_api_config(None).unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));

        // Then: endpoint defaults to the OpenAI base URL
        assert_eq!(config.endpoint, DEFAULT_ENDPOINT);
    }

    #[test]
    fn test_resolve_llm_api_config_default_model() {
        // Given: only API key set, no model in env or config
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-test");

        // When: resolve_llm_api_config is called
        let config =
            resolve_llm_api_config(None).unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));

        // Then: model defaults to gpt-4o
        assert_eq!(config.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_resolve_llm_api_config_env_endpoint_overrides_default() {
        // Given: CRUISE_LLM_ENDPOINT is set to a custom URL
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-test");
        let _endpoint = EnvGuard::set("CRUISE_LLM_ENDPOINT", "http://localhost:11434/v1");

        // When: resolve_llm_api_config is called
        let config =
            resolve_llm_api_config(None).unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));

        // Then: custom endpoint is used
        assert_eq!(config.endpoint, "http://localhost:11434/v1");
    }

    #[test]
    fn test_resolve_llm_api_config_env_model_overrides_default() {
        // Given: CRUISE_LLM_MODEL is set to a custom model name
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-test");
        let _model = EnvGuard::set("CRUISE_LLM_MODEL", "llama3");

        // When: resolve_llm_api_config is called
        let config =
            resolve_llm_api_config(None).unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));

        // Then: custom model is used
        assert_eq!(config.model, "llama3");
    }

    #[test]
    fn test_resolve_llm_api_config_env_endpoint_overrides_config_endpoint() {
        // Given: both env endpoint and config endpoint are present
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-test");
        let _endpoint = EnvGuard::set("CRUISE_LLM_ENDPOINT", "http://env-host/v1");
        let config_llm = LlmApiConfigYaml {
            api_key: None,
            endpoint: Some("http://config-host/v1".to_string()),
            model: None,
        };

        // When: resolve_llm_api_config is called
        let config = resolve_llm_api_config(Some(&config_llm))
            .unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));

        // Then: env endpoint takes precedence over config endpoint
        assert_eq!(config.endpoint, "http://env-host/v1");
    }

    #[test]
    fn test_resolve_llm_api_config_config_endpoint_used_when_no_env_endpoint() {
        // Given: no env endpoint, but config has endpoint
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-test");
        let config_llm = LlmApiConfigYaml {
            api_key: None,
            endpoint: Some("http://config-host/v1".to_string()),
            model: None,
        };

        // When: resolve_llm_api_config is called
        let config = resolve_llm_api_config(Some(&config_llm))
            .unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));

        // Then: config endpoint is used
        assert_eq!(config.endpoint, "http://config-host/v1");
    }

    #[test]
    fn test_resolve_llm_api_config_config_model_used_when_no_env_model() {
        // Given: no env model, but config has model
        let _lock = lock_process();
        let _env = clean_env();
        let _key = EnvGuard::set("CRUISE_LLM_API_KEY", "sk-test");
        let config_llm = LlmApiConfigYaml {
            api_key: None,
            endpoint: None,
            model: Some("claude-opus-4-5".to_string()),
        };

        // When: resolve_llm_api_config is called
        let config = resolve_llm_api_config(Some(&config_llm))
            .unwrap_or_else(|| panic!("expected Some(LlmApiConfig)"));

        // Then: config model is used
        assert_eq!(config.model, "claude-opus-4-5");
    }

    // -- validate_read_file_path ----------------------------------------------

    #[test]
    fn test_validate_read_file_path_allows_file_within_working_dir() {
        // Given: a file that exists within the working directory
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let file = tmp.path().join("plan.md");
        std::fs::write(&file, "# Plan").unwrap_or_else(|e| panic!("{e:?}"));

        // When: validate_read_file_path is called with a relative path
        let result = validate_read_file_path(tmp.path(), "plan.md");

        // Then: returns Ok with the resolved path
        assert!(
            result.is_ok(),
            "expected Ok for in-bounds path, got: {result:?}"
        );
    }

    #[test]
    fn test_validate_read_file_path_resolved_path_is_within_working_dir() {
        // Given: a file within the working directory
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let file = tmp.path().join("plan.md");
        std::fs::write(&file, "# Plan").unwrap_or_else(|e| panic!("{e:?}"));

        // When: validate_read_file_path succeeds
        let resolved =
            validate_read_file_path(tmp.path(), "plan.md").unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the returned path starts with the working directory
        assert!(
            resolved.starts_with(tmp.path()),
            "resolved path {resolved:?} should be within working dir {:?}",
            tmp.path()
        );
    }

    #[test]
    fn test_validate_read_file_path_rejects_path_traversal_with_dotdot() {
        // Given: a path that attempts to escape the working directory via ..
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));

        // When: validate_read_file_path is called with a traversal path
        let result = validate_read_file_path(tmp.path(), "../../etc/passwd");

        // Then: returns Err (path traversal rejected)
        assert!(
            result.is_err(),
            "expected Err for path traversal '../../etc/passwd', got Ok"
        );
    }

    #[test]
    fn test_validate_read_file_path_rejects_absolute_path_outside_working_dir() {
        // Given: an absolute path that is outside the working directory
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));

        // When: validate_read_file_path is called with an outside absolute path
        let result = validate_read_file_path(tmp.path(), "/etc/passwd");

        // Then: returns Err
        assert!(
            result.is_err(),
            "expected Err for absolute path outside working dir, got Ok"
        );
    }

    #[test]
    fn test_validate_read_file_path_rejects_path_pointing_to_parent_dir() {
        // Given: a path that resolves to the parent of the working directory
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let parent = tmp
            .path()
            .parent()
            .unwrap_or_else(|| panic!("no parent"))
            .to_str()
            .unwrap_or_else(|| panic!("non-utf8 path"))
            .to_string();

        // When: validate_read_file_path is called with the parent path
        let result = validate_read_file_path(tmp.path(), &parent);

        // Then: returns Err (parent dir is outside working dir)
        assert!(
            result.is_err(),
            "expected Err for parent directory path, got Ok"
        );
    }

    // -- validate_git_subcommand ------------------------------------------------

    #[test]
    fn test_validate_git_subcommand_allows_diff() {
        // Given: "diff" -- a read-only subcommand
        // When / Then: returns Ok
        assert!(
            validate_git_subcommand("diff").is_ok(),
            "'diff' must be allowed"
        );
    }

    #[test]
    fn test_validate_git_subcommand_allows_log() {
        assert!(
            validate_git_subcommand("log").is_ok(),
            "'log' must be allowed"
        );
    }

    #[test]
    fn test_validate_git_subcommand_allows_show() {
        assert!(
            validate_git_subcommand("show").is_ok(),
            "'show' must be allowed"
        );
    }

    #[test]
    fn test_validate_git_subcommand_allows_rev_parse() {
        assert!(
            validate_git_subcommand("rev-parse").is_ok(),
            "'rev-parse' must be allowed"
        );
    }

    #[test]
    fn test_validate_git_subcommand_allows_merge_base() {
        assert!(
            validate_git_subcommand("merge-base").is_ok(),
            "'merge-base' must be allowed"
        );
    }

    #[test]
    fn test_validate_git_subcommand_rejects_push() {
        // Given: "push" -- a destructive subcommand
        // When / Then: returns Err
        assert!(
            validate_git_subcommand("push").is_err(),
            "'push' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_subcommand_rejects_reset() {
        assert!(
            validate_git_subcommand("reset").is_err(),
            "'reset' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_subcommand_rejects_commit() {
        assert!(
            validate_git_subcommand("commit").is_err(),
            "'commit' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_subcommand_rejects_checkout() {
        assert!(
            validate_git_subcommand("checkout").is_err(),
            "'checkout' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_subcommand_rejects_fetch() {
        assert!(
            validate_git_subcommand("fetch").is_err(),
            "'fetch' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_subcommand_rejects_empty_string() {
        // Given: empty string (no subcommand)
        // When / Then: returns Err
        assert!(
            validate_git_subcommand("").is_err(),
            "empty subcommand must be rejected"
        );
    }

    // -- validate_git_args -----------------------------------------------------

    #[test]
    fn test_validate_git_args_allows_empty() {
        assert!(validate_git_args("").is_ok(), "empty args must be allowed");
    }

    #[test]
    fn test_validate_git_args_allows_safe_flags() {
        assert!(
            validate_git_args("--stat --name-only HEAD~1").is_ok(),
            "safe flags must be allowed"
        );
    }

    #[test]
    fn test_validate_git_args_rejects_output_flag() {
        // Given: --output (without value)
        assert!(
            validate_git_args("--output").is_err(),
            "'--output' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_args_rejects_output_eq_flag() {
        // Given: --output=/path
        assert!(
            validate_git_args("--output=/tmp/evil").is_err(),
            "'--output=...' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_args_rejects_short_o_flag() {
        assert!(validate_git_args("-o").is_err(), "'-o' must be rejected");
    }

    #[test]
    fn test_validate_git_args_rejects_short_o_eq_flag() {
        assert!(
            validate_git_args("-o=/tmp/evil").is_err(),
            "'-o=...' must be rejected"
        );
    }

    #[test]
    fn test_validate_git_args_rejects_output_among_other_flags() {
        // Given: --output mixed with other flags
        assert!(
            validate_git_args("--stat --output=/tmp/out --name-only").is_err(),
            "'--output=...' embedded in other flags must be rejected"
        );
    }
}
