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
pub async fn run_commands(cmds: &[String], max_retries: usize) -> Result<CommandResult> {
    let mut last_result = CommandResult {
        success: true,
        stderr: String::new(),
    };

    for cmd in cmds {
        last_result = run_command(cmd, max_retries).await?;
        if !last_result.success {
            return Ok(last_result);
        }
    }

    Ok(last_result)
}

/// Execute a shell command with optional rate-limit retry.
pub async fn run_command(cmd: &str, max_retries: usize) -> Result<CommandResult> {
    let mut attempts = 0;

    loop {
        let result = execute_command(cmd).await?;

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
async fn execute_command(cmd: &str) -> Result<CommandResult> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CruiseError::ProcessSpawnError(e.to_string()))?
        .wait_with_output()
        .await
        .map_err(|e| CruiseError::CommandError(e.to_string()))?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
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
    let secs = (base_secs * 2u64.pow((attempt as u32).saturating_sub(1))).min(max_secs);
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
        let result = run_command("echo hello", 0).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_failing_command() {
        let result = run_command("exit 1", 0).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_run_commands_sequential() {
        let cmds = vec!["echo a".to_string(), "echo b".to_string()];
        let result = run_commands(&cmds, 0).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_commands_stops_on_failure() {
        // Second command would succeed but shouldn't run because first fails.
        let cmds = vec!["exit 1".to_string(), "echo ok".to_string()];
        let result = run_commands(&cmds, 0).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_run_commands_empty() {
        let result = run_commands(&[], 0).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_command_captures_stderr() {
        let result = run_command("echo 'error msg' >&2; exit 1", 0)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.stderr.contains("error msg"));
    }
}
