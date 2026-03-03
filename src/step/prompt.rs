use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{CruiseError, Result};
use crate::step::command::{calculate_backoff, is_rate_limited};

/// Result of executing a prompt step.
#[derive(Debug, Clone)]
pub struct PromptResult {
    pub output: String,
}

/// Invoke the LLM command with optional rate-limit retry.
pub async fn run_prompt(
    command: &[String],
    model: Option<&str>,
    instruction: Option<&str>,
    prompt: &str,
    max_retries: usize,
) -> Result<PromptResult> {
    let mut attempts = 0;

    loop {
        let result = execute_prompt(command, model, instruction, prompt).await;

        match result {
            Ok(output) => return Ok(PromptResult { output }),
            Err(e) => {
                let err_msg = e.to_string();
                if is_rate_limited(&err_msg) && attempts < max_retries {
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
                return Err(e);
            }
        }
    }
}

/// Spawn the LLM process, write the prompt to stdin, and capture stdout.
async fn execute_prompt(
    command: &[String],
    model: Option<&str>,
    instruction: Option<&str>,
    prompt: &str,
) -> Result<String> {
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

    if let Some(inst) = instruction {
        cmd_args.push("--system-prompt".to_string());
        cmd_args.push(inst.to_string());
    }

    let mut child = Command::new(&command[0])
        .args(&cmd_args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CruiseError::ProcessSpawnError(e.to_string()))?;

    // Write the prompt via stdin to avoid ARG_MAX limits.
    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| CruiseError::IoError(e))?;
        // Close stdin to send EOF.
        drop(stdin);
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| CruiseError::CommandError(e.to_string()))?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        let error_msg = if stderr.is_empty() {
            format!("command failed (exit code: {:?})", output.status.code())
        } else {
            stderr.clone()
        };
        return Err(CruiseError::CommandError(error_msg));
    }

    // Also detect rate limits reported via stderr.
    if is_rate_limited(&stderr) {
        return Err(CruiseError::CommandError(stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Build the full argument list for the LLM command (test helper).
#[cfg(test)]
pub(crate) fn build_command_args(
    command: &[String],
    model: Option<&str>,
    instruction: Option<&str>,
) -> Vec<String> {
    let mut args = command.to_vec();

    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m.to_string());
    }

    if let Some(inst) = instruction {
        args.push("--system-prompt".to_string());
        args.push(inst.to_string());
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_args_minimal() {
        let command = vec!["claude".to_string(), "-p".to_string()];
        let args = build_command_args(&command, None, None);
        assert_eq!(args, vec!["claude", "-p"]);
    }

    #[test]
    fn test_build_command_args_with_model() {
        let command = vec!["claude".to_string(), "-p".to_string()];
        let args = build_command_args(&command, Some("claude-opus-4-5"), None);
        assert_eq!(args, vec!["claude", "-p", "--model", "claude-opus-4-5"]);
    }

    #[test]
    fn test_build_command_args_with_instruction() {
        let command = vec!["claude".to_string(), "-p".to_string()];
        let args = build_command_args(&command, None, Some("You are helpful"));
        assert_eq!(
            args,
            vec!["claude", "-p", "--system-prompt", "You are helpful"]
        );
    }

    #[test]
    fn test_build_command_args_with_all() {
        let command = vec!["claude".to_string(), "-p".to_string()];
        let args = build_command_args(&command, Some("my-model"), Some("Be helpful"));
        assert_eq!(
            args,
            vec![
                "claude",
                "-p",
                "--model",
                "my-model",
                "--system-prompt",
                "Be helpful"
            ]
        );
    }

    #[tokio::test]
    async fn test_run_prompt_with_echo() {
        // Use `cat` to echo back stdin as a stand-in for a real LLM.
        let command = vec!["cat".to_string()];
        let result = run_prompt(&command, None, None, "test prompt", 0)
            .await
            .unwrap();
        assert_eq!(result.output, "test prompt");
    }

    #[tokio::test]
    async fn test_run_prompt_empty_command() {
        let result = run_prompt(&[], None, None, "prompt", 0).await;
        assert!(result.is_err());
    }
}
