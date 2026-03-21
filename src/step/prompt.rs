use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::cancellation::CancellationToken;
use crate::error::{CruiseError, Result};
use crate::step::command::{calculate_backoff, is_rate_limited};

/// Result of executing a prompt step.
#[derive(Debug, Clone)]
pub struct PromptResult {
    pub output: String,
    pub stderr: String,
}

/// Invoke the LLM command with optional rate-limit retry.
///
/// # Errors
///
/// Returns an error if the LLM process fails to spawn or returns a fatal error.
pub async fn run_prompt<S: std::hash::BuildHasher, F: Fn(&str)>(
    command: &[String],
    model: Option<&str>,
    prompt: &str,
    max_retries: usize,
    env: &HashMap<String, String, S>,
    on_retry: Option<&F>,
    cancel_token: Option<&CancellationToken>,
) -> Result<PromptResult> {
    let mut attempts = 0;

    loop {
        let result = execute_prompt(command, model, prompt, env, cancel_token).await;

        match result {
            Ok((output, stderr)) => return Ok(PromptResult { output, stderr }),
            Err(e) => {
                let err_msg = e.to_string();
                if is_rate_limited(&err_msg) && attempts < max_retries {
                    attempts += 1;
                    let delay = calculate_backoff(attempts);
                    let msg = format!(
                        "Rate limit detected. Retrying in {:.1}s... ({}/{})",
                        delay.as_secs_f64(),
                        attempts,
                        max_retries
                    );
                    if let Some(cb) = on_retry {
                        cb(&msg);
                    } else {
                        eprintln!("{msg}");
                    }
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
}

/// Resolves when the token is cancelled, or waits forever if no token is provided.
async fn maybe_cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(t) => t.cancelled().await,
        None => std::future::pending().await,
    }
}

/// Spawn the LLM process, write the prompt to stdin, and capture stdout and stderr.
async fn execute_prompt<S: std::hash::BuildHasher>(
    command: &[String],
    model: Option<&str>,
    prompt: &str,
    env: &HashMap<String, String, S>,
    cancel_token: Option<&CancellationToken>,
) -> Result<(String, String)> {
    if command.is_empty() {
        return Err(CruiseError::InvalidStepConfig(
            "command list is empty".to_string(),
        ));
    }

    let mut cmd_args: Vec<String> = command[1..].to_vec();

    if let Some(m) = model {
        cmd_args.push("--model".to_string());
        cmd_args.push(m.to_string());
    }

    let mut child = Command::new(&command[0])
        .args(&cmd_args)
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            CruiseError::ProcessSpawnError(format!("failed to spawn '{}': {e}", command[0]))
        })?;

    // Write the prompt via stdin to avoid ARG_MAX limits.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(CruiseError::IoError)?;
        // Close stdin to send EOF.
        drop(stdin);
    }

    // Collect stdout/stderr concurrently with waiting to avoid pipe-buffer deadlocks.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(ref mut s) = stdout_pipe {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(ref mut s) = stderr_pipe {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });

    let status = tokio::select! {
        result = child.wait() => {
            result.map_err(|e| CruiseError::CommandError(e.to_string()))?
        }
        () = maybe_cancelled(cancel_token) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(CruiseError::Interrupted);
        }
    };

    let stdout_bytes = stdout_task.await.unwrap_or_default();
    let stderr_bytes = stderr_task.await.unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();

    if !status.success() {
        let error_msg = if stderr.is_empty() {
            format!("command failed (exit code: {:?})", status.code())
        } else {
            stderr
        };
        return Err(CruiseError::CommandError(error_msg));
    }

    // Also detect rate limits reported via stderr.
    if is_rate_limited(&stderr) {
        return Err(CruiseError::CommandError(stderr));
    }

    Ok((String::from_utf8_lossy(&stdout_bytes).to_string(), stderr))
}

/// Build the full argument list for the LLM command (test helper).
#[cfg(test)]
pub(crate) fn build_command_args(command: &[String], model: Option<&str>) -> Vec<String> {
    let mut args = command.to_vec();

    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m.to_string());
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_args_minimal() {
        let command = vec!["claude".to_string(), "-p".to_string()];
        let args = build_command_args(&command, None);
        assert_eq!(args, vec!["claude", "-p"]);
    }

    #[test]
    fn test_build_command_args_with_model() {
        let command = vec!["claude".to_string(), "-p".to_string()];
        let args = build_command_args(&command, Some("claude-opus-4-5"));
        assert_eq!(args, vec!["claude", "-p", "--model", "claude-opus-4-5"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_prompt_with_echo() {
        let _guard = crate::test_support::lock_process();
        // Use `cat` to echo back stdin as a stand-in for a real LLM.
        let command = vec!["cat".to_string()];
        let result = run_prompt(
            &command,
            None,
            "test prompt",
            0,
            &HashMap::new(),
            None::<&fn(&str)>,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.output, "test prompt");
    }

    #[tokio::test]
    async fn test_run_prompt_empty_command() {
        let result = run_prompt(
            &[],
            None,
            "prompt",
            0,
            &HashMap::new(),
            None::<&fn(&str)>,
            None,
        )
        .await;
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_prompt_with_env() {
        let _guard = crate::test_support::lock_process();
        // cat echoes stdin regardless of env; verify env does not break execution.
        let command = vec!["cat".to_string()];
        let mut env = HashMap::new();
        env.insert("SOME_VAR".to_string(), "some_value".to_string());
        let result = run_prompt(
            &command,
            None,
            "prompt text",
            0,
            &env,
            None::<&fn(&str)>,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.output, "prompt text");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_prompt_with_model_arg() {
        let _guard = crate::test_support::lock_process();
        // "sh -c cat" ignores extra positional args (--model test-model become $0/$1 in sh).
        let command = vec!["sh".to_string(), "-c".to_string(), "cat".to_string()];
        let result = run_prompt(
            &command,
            Some("test-model"),
            "hello model",
            0,
            &HashMap::new(),
            None::<&fn(&str)>,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.output, "hello model");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_prompt_captures_stderr() {
        let _guard = crate::test_support::lock_process();
        // Given: a command that writes to both stdout and stderr
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo out_text; echo err_text >&2".to_string(),
        ];
        // When: run_prompt is called with an empty prompt (stdin ignored by the script)
        let result = run_prompt(
            &command,
            None,
            "",
            0,
            &HashMap::new(),
            None::<&fn(&str)>,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: stdout is in output and stderr is captured in stderr field
        assert_eq!(result.output.trim(), "out_text");
        assert_eq!(result.stderr.trim(), "err_text");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_prompt_stderr_empty_when_no_stderr() {
        let _guard = crate::test_support::lock_process();
        // Given: a command that writes only to stdout (cat echoes stdin)
        let command = vec!["cat".to_string()];
        // When: run_prompt is called
        let result = run_prompt(
            &command,
            None,
            "only stdout",
            0,
            &HashMap::new(),
            None::<&fn(&str)>,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: stderr field is empty, output contains stdin content
        assert_eq!(result.output, "only stdout");
        assert_eq!(result.stderr, "");
    }
}
