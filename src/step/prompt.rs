use std::collections::HashMap;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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

/// Grouped callbacks for streaming LLM stdout/stderr output line by line.
pub struct StreamCallbacks<'a> {
    pub on_stdout: Option<&'a (dyn Fn(&str) + Send + Sync)>,
    pub on_stderr: Option<&'a (dyn Fn(&str) + Send + Sync)>,
}

/// Invoke the LLM command with optional rate-limit retry.
///
/// # Errors
///
/// Returns an error if the LLM process fails to spawn or returns a fatal error.
#[expect(clippy::too_many_arguments)]
pub async fn run_prompt<S: std::hash::BuildHasher, F: Fn(&str)>(
    command: &[String],
    model: Option<&str>,
    prompt: &str,
    max_retries: usize,
    env: &HashMap<String, String, S>,
    on_retry: Option<&F>,
    cancel_token: Option<&CancellationToken>,
    cwd: Option<&std::path::Path>,
    stream_callbacks: Option<&StreamCallbacks<'_>>,
) -> Result<PromptResult> {
    let mut attempts = 0;

    loop {
        let result = execute_prompt(
            command,
            model,
            prompt,
            env,
            cancel_token,
            cwd,
            stream_callbacks,
        )
        .await;

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
#[expect(clippy::too_many_lines)]
async fn execute_prompt<S: std::hash::BuildHasher>(
    command: &[String],
    model: Option<&str>,
    prompt: &str,
    env: &HashMap<String, String, S>,
    cancel_token: Option<&CancellationToken>,
    cwd: Option<&std::path::Path>,
    stream_callbacks: Option<&StreamCallbacks<'_>>,
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

    let mut cmd = Command::new(&command[0]);
    cmd.args(&cmd_args)
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| {
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

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let drain_stdout = async {
        let mut buf = String::new();
        if let Some(pipe) = stdout_pipe {
            let mut reader = BufReader::new(pipe);
            let mut line = String::new();
            // Extract callback once before loop to avoid per-line overhead
            let on_stdout = stream_callbacks.and_then(|s| s.on_stdout);
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let has_newline = line.ends_with('\n');
                        let trimmed = line.trim_end_matches('\n');
                        if let Some(cb) = on_stdout {
                            cb(trimmed);
                        }
                        buf.push_str(trimmed);
                        if has_newline {
                            buf.push('\n');
                        }
                    }
                }
            }
        }
        buf
    };

    let drain_stderr = async {
        let mut buf = String::new();
        if let Some(pipe) = stderr_pipe {
            let mut reader = BufReader::new(pipe);
            let mut line = String::new();
            // Extract callback once before loop to avoid per-line overhead
            let on_stderr = stream_callbacks.and_then(|s| s.on_stderr);
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let has_newline = line.ends_with('\n');
                        let trimmed = line.trim_end_matches('\n');
                        if let Some(cb) = on_stderr {
                            cb(trimmed);
                        }
                        buf.push_str(trimmed);
                        if has_newline {
                            buf.push('\n');
                        }
                    }
                }
            }
        }
        buf
    };

    let status;
    let stdout_buf;
    let stderr_buf;

    tokio::select! {
        (s, (o, e)) = async {
            tokio::join!(
                child.wait(),
                async { tokio::join!(drain_stdout, drain_stderr) },
            )
        } => {
            status = s;
            stdout_buf = o;
            stderr_buf = e;
        }
        () = maybe_cancelled(cancel_token) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(CruiseError::Interrupted);
        }
    }

    let status = status.map_err(|e| CruiseError::CommandError(e.to_string()))?;

    if !status.success() {
        let stderr_for_err = stderr_buf.clone();
        let error_msg = if stderr_for_err.is_empty() {
            format!("command failed (exit code: {:?})", status.code())
        } else {
            stderr_for_err
        };
        return Err(CruiseError::CommandError(error_msg));
    }

    Ok((stdout_buf, stderr_buf))
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
#[expect(clippy::expect_used)]
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
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.output.trim_end(), "test prompt");
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
            None,
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
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.output.trim_end(), "prompt text");
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
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.output.trim_end(), "hello model");
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
            None,
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
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: stderr field is empty, output contains stdin content
        assert_eq!(result.output.trim_end(), "only stdout");
        assert_eq!(result.stderr, "");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_prompt_invokes_on_stdout_per_line() {
        let _guard = crate::test_support::lock_process();
        // Given: a command that writes two lines to stdout
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo line1; echo line2".to_string(),
        ];
        let lines: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let on_stdout = |line: &str| {
            lines.lock().expect("lock poisoned").push(line.to_string());
        };
        // When: run_prompt is called with on_stdout callback
        let stream_callbacks = StreamCallbacks {
            on_stdout: Some(&on_stdout as &(dyn Fn(&str) + Send + Sync)),
            on_stderr: None,
        };
        let result = run_prompt(
            &command,
            None,
            "",
            0,
            &HashMap::new(),
            None::<&fn(&str)>,
            None,
            None,
            Some(&stream_callbacks),
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: callback received lines in order and output contains both lines
        let collected = lines.lock().expect("lock poisoned");
        assert_eq!(*collected, vec!["line1".to_string(), "line2".to_string()]);
        assert_eq!(result.output.trim_end(), "line1\nline2");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_prompt_invokes_on_stderr_per_line() {
        let _guard = crate::test_support::lock_process();
        // Given: a command that writes two lines to stderr
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo err1 >&2; echo err2 >&2".to_string(),
        ];
        let lines: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let on_stderr = |line: &str| {
            lines.lock().expect("lock poisoned").push(line.to_string());
        };
        // When: run_prompt is called with on_stderr callback
        let stream_callbacks = StreamCallbacks {
            on_stdout: None,
            on_stderr: Some(&on_stderr as &(dyn Fn(&str) + Send + Sync)),
        };
        let result = run_prompt(
            &command,
            None,
            "",
            0,
            &HashMap::new(),
            None::<&fn(&str)>,
            None,
            None,
            Some(&stream_callbacks),
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: callback received stderr lines in order
        let collected = lines.lock().expect("lock poisoned");
        assert_eq!(*collected, vec!["err1".to_string(), "err2".to_string()]);
        assert_eq!(result.stderr.trim_end(), "err1\nerr2");
    }
}
