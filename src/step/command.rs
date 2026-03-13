use std::collections::HashMap;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{CruiseError, Result};

/// Result of executing a command step.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub stderr: String,
}

/// Execute a list of shell commands sequentially.
/// Stops immediately on the first failure and returns that result.
pub async fn run_commands(
    cmds: &[String],
    max_retries: usize,
    env: &HashMap<String, String>,
) -> Result<CommandResult> {
    let mut last_result = CommandResult {
        success: true,
        stderr: String::new(),
    };

    for cmd in cmds {
        last_result = run_command(cmd, max_retries, env).await?;
        if !last_result.success {
            return Ok(last_result);
        }
    }

    Ok(last_result)
}

/// Execute a shell command with optional rate-limit retry.
pub async fn run_command(
    cmd: &str,
    max_retries: usize,
    env: &HashMap<String, String>,
) -> Result<CommandResult> {
    let mut attempts = 0;

    loop {
        let result = execute_command(cmd, env).await?;

        if result.success {
            return Ok(result);
        }

        if is_rate_limited(&result.stderr) && attempts < max_retries {
            attempts += 1;
            let delay = calculate_backoff(attempts);
            eprintln!(
                "Rate limit detected. Retrying in {:.1}s... ({}/{})",
                delay.as_secs_f64(),
                attempts,
                max_retries
            );
            tokio::time::sleep(delay).await;
            continue;
        }

        return Ok(result);
    }
}

/// Run `sh -c cmd`, streaming stdout and capturing stderr.
async fn execute_command(cmd: &str, env: &HashMap<String, String>) -> Result<CommandResult> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .envs(env)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CruiseError::ProcessSpawnError(e.to_string()))?
        .wait_with_output()
        .await
        .map_err(|e| CruiseError::CommandError(e.to_string()))?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    let success = output.status.success();

    Ok(CommandResult { success, stderr })
}

/// Return true if `stderr` indicates a rate-limit error.
pub fn is_rate_limited(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("too many requests")
        || lower.contains("ratelimit")
}

/// Exponential backoff: 2s base, 60s cap.
pub fn calculate_backoff(attempt: usize) -> Duration {
    let base_secs = 2u64;
    let max_secs = 60u64;
    let exp = u32::try_from(attempt).unwrap_or(u32::MAX).saturating_sub(1);
    let secs = (base_secs * 2u64.pow(exp)).min(max_secs);
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_rate_limited_rate_limit() {
        assert!(is_rate_limited("Error: rate limit exceeded"));
    }

    #[test]
    fn test_is_rate_limited_429() {
        assert!(is_rate_limited("HTTP 429 Too Many Requests"));
    }

    #[test]
    fn test_is_rate_limited_too_many_requests() {
        assert!(is_rate_limited("too many requests"));
    }

    #[test]
    fn test_is_rate_limited_ratelimit() {
        assert!(is_rate_limited("RateLimit exceeded"));
    }

    #[test]
    fn test_is_not_rate_limited() {
        assert!(!is_rate_limited("Normal error message"));
        assert!(!is_rate_limited(""));
        assert!(!is_rate_limited("compilation error"));
    }

    #[test]
    fn test_calculate_backoff() {
        assert_eq!(calculate_backoff(1), Duration::from_secs(2));
        assert_eq!(calculate_backoff(2), Duration::from_secs(4));
        assert_eq!(calculate_backoff(3), Duration::from_secs(8));
        assert_eq!(calculate_backoff(4), Duration::from_secs(16));
        assert_eq!(calculate_backoff(5), Duration::from_secs(32));
        // capped at 60 seconds
        assert_eq!(calculate_backoff(10), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_run_successful_command() {
        let result = run_command("echo hello", 0, &HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_failing_command() {
        let result = run_command("exit 1", 0, &HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_run_commands_sequential() {
        let cmds = vec!["echo a".to_string(), "echo b".to_string()];
        let result = run_commands(&cmds, 0, &HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_commands_stops_on_failure() {
        // Second command would succeed but shouldn't run because first fails.
        let cmds = vec!["exit 1".to_string(), "echo ok".to_string()];
        let result = run_commands(&cmds, 0, &HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_run_commands_empty() {
        let result = run_commands(&[], 0, &HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_command_captures_stderr() {
        let result = run_command("echo 'error msg' >&2; exit 1", 0, &HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
        assert!(result.stderr.contains("error msg"));
    }

    #[tokio::test]
    async fn test_run_command_with_env() {
        let mut env = HashMap::new();
        env.insert("CRUISE_TEST_VAR".to_string(), "hello_env".to_string());
        // The command echoes the env var; success means env was passed correctly.
        let result = run_command("test \"$CRUISE_TEST_VAR\" = hello_env", 0, &env)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_commands_partial_failure_stderr() {
        // Second command fails with a message written to stderr.
        let cmds = vec![
            "echo step1".to_string(),
            "echo 'err_msg' >&2; exit 1".to_string(),
        ];
        let result = run_commands(&cmds, 0, &HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
        assert!(result.stderr.contains("err_msg"));
    }

    #[tokio::test]
    async fn test_run_command_multiple_env_vars() {
        let mut env = HashMap::new();
        env.insert("VAR_A".to_string(), "alpha".to_string());
        env.insert("VAR_B".to_string(), "beta".to_string());
        let result = run_command(r#"test "$VAR_A" = alpha && test "$VAR_B" = beta"#, 0, &env)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_command_env_in_echo() {
        let mut env = HashMap::new();
        env.insert("GREETING".to_string(), "hello".to_string());
        // stdout is inherited (not captured), but success means the command ran.
        let result = run_command("echo $GREETING", 0, &env)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }
}
