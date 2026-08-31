use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::{CruiseError, Result};

pub(crate) type StepLogCallback<'a> = dyn Fn(&str, &str) + Send + Sync + 'a;

/// Result of executing a command step.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub stderr: String,
}

/// Execute commands sequentially, stopping on failure or cancellation.
pub(crate) async fn run_commands<S: std::hash::BuildHasher>(
    cmds: &[String],
    max_retries: usize,
    env: &HashMap<String, String, S>,
    cwd: Option<&std::path::Path>,
    timeout: Option<Duration>,
    on_step_log: Option<&StepLogCallback<'_>>,
    cancel_token: Option<&crate::cancellation::CancellationToken>,
) -> Result<CommandResult> {
    let mut last_result = CommandResult {
        success: true,
        stderr: String::new(),
    };
    for cmd in cmds {
        last_result = run_command(
            cmd,
            max_retries,
            env,
            cwd,
            timeout,
            on_step_log,
            cancel_token,
        )
        .await?;
        if !last_result.success {
            return Ok(last_result);
        }
    }
    Ok(last_result)
}

/// Execute one shell command with optional logging, retry, and cancellation.
async fn run_command<S: std::hash::BuildHasher>(
    cmd: &str,
    max_retries: usize,
    env: &HashMap<String, String, S>,
    cwd: Option<&std::path::Path>,
    timeout: Option<Duration>,
    on_step_log: Option<&StepLogCallback<'_>>,
    cancel_token: Option<&crate::cancellation::CancellationToken>,
) -> Result<CommandResult> {
    let mut attempts = 0;
    loop {
        let result =
            execute_command_cancel(cmd, env, cwd, timeout, on_step_log, cancel_token).await?;
        if result.success {
            return Ok(result);
        }
        if is_rate_limited(&result.stderr) && attempts < max_retries {
            attempts += 1;
            let delay = calculate_backoff(attempts);
            crate::status_eprintln!(
                "Rate limit detected. Retrying in {:.1}s... ({}/{})",
                delay.as_secs_f64(),
                attempts,
                max_retries
            );
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = cancel_wait(cancel_token) => return Err(CruiseError::Interrupted),
            }
            continue;
        }
        return Ok(result);
    }
}

async fn cancel_wait(token: Option<&crate::cancellation::CancellationToken>) {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

fn spawn_command<S: std::hash::BuildHasher>(
    cmd: &str,
    env: &HashMap<String, String, S>,
    cwd: Option<&std::path::Path>,
    quiet: bool,
) -> Result<tokio::process::Child> {
    let (shell, flag) = crate::platform::shell_command();
    let mut builder = Command::new(shell);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        builder.as_std_mut().process_group(0);
    }
    builder
        .arg(flag)
        .arg(cmd)
        .envs(env)
        .stdin(if quiet {
            std::process::Stdio::null()
        } else {
            std::process::Stdio::inherit()
        })
        .stdout(if quiet {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::inherit()
        })
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = cwd {
        builder.current_dir(dir);
    }
    builder
        .spawn()
        .map_err(|e| CruiseError::ProcessSpawnError(e.to_string()))
}

async fn execute_command_cancel<S: std::hash::BuildHasher>(
    cmd: &str,
    env: &HashMap<String, String, S>,
    cwd: Option<&std::path::Path>,
    timeout: Option<Duration>,
    on_step_log: Option<&StepLogCallback<'_>>,
    cancel_token: Option<&crate::cancellation::CancellationToken>,
) -> Result<CommandResult> {
    let quiet = crate::console_mode::is_quiet();
    let mut child = spawn_command(cmd, env, cwd, quiet)?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let output_deadline = timeout.map(|duration| Instant::now() + duration);

    let stdout_task = quiet.then(|| {
        tokio::spawn(async move {
            match stdout_pipe {
                Some(mut pipe) => read_output_bounded(&mut pipe).await,
                None => String::new(),
            }
        })
    });
    let stderr_task = tokio::spawn(async move {
        match stderr_pipe {
            Some(mut pipe) => read_output_bounded(&mut pipe).await,
            None => String::new(),
        }
    });

    let wait_result = async {
        if let Some(duration) = timeout {
            tokio::time::timeout(duration, child.wait()).await
        } else {
            Ok(child.wait().await)
        }
    };
    let status_result = tokio::select! {
        result = wait_result => result,
        () = cancel_wait(cancel_token) => {
            terminate_process_group(child.id());
            let _ = child.kill().await;
            let _ = child.wait().await;
            abort_output_task(stdout_task).await;
            abort_output_task(Some(stderr_task)).await;
            return Err(CruiseError::Interrupted);
        }
    };

    if status_result.is_err() {
        terminate_process_group(child.id());
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    let stdout = match stdout_task {
        Some(task) => collect_output(task, output_deadline).await,
        None => String::new(),
    };
    let stderr = collect_output(stderr_task, output_deadline).await;

    if quiet && let Some(log) = on_step_log {
        for line in stdout.lines() {
            log("stdout", line);
        }
        for line in stderr.lines() {
            log("stderr", line);
        }
    }

    match status_result {
        Ok(Ok(status)) => {
            if !quiet && !stderr.is_empty() {
                eprint!("{stderr}");
            }
            Ok(CommandResult {
                success: status.success(),
                stderr,
            })
        }
        Ok(Err(e)) => Err(CruiseError::CommandError(e.to_string())),
        Err(_elapsed) => {
            let secs = timeout.map_or(0, |duration| duration.as_secs());
            crate::status_eprintln!("  step timed out after {secs}s");
            Err(CruiseError::StepTimeout {
                step: cmd.to_string(),
                after_secs: secs,
            })
        }
    }
}

const MAX_CAPTURED_OUTPUT: usize = 4 * 1024 * 1024;

async fn read_output_bounded<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> String {
    let mut bytes = Vec::with_capacity(8192);
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) if bytes.len() < MAX_CAPTURED_OUTPUT => {
                let remaining = MAX_CAPTURED_OUTPUT - bytes.len();
                bytes.extend_from_slice(&buffer[..read.min(remaining)]);
            }
            Ok(_) => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn abort_output_task(task: Option<tokio::task::JoinHandle<String>>) {
    let Some(task) = task else {
        return;
    };
    task.abort();
    let _ = task.await;
}

fn terminate_process_group(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    #[cfg(unix)]
    {
        // Commands are started in their own process group, so descendants
        // holding inherited pipes are terminated with the shell.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
async fn collect_output(
    mut task: tokio::task::JoinHandle<String>,
    deadline: Option<Instant>,
) -> String {
    let deadline = deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(5));
    let remaining = deadline.saturating_duration_since(Instant::now());
    tokio::select! {
        result = &mut task => result.unwrap_or_default(),
        () = tokio::time::sleep(remaining) => {
            task.abort();
            let _ = task.await;
            String::new()
        }
    }
}

/// Run one external process with null stdin and cancellation-aware process
/// group cleanup. This is shared by git/gh helpers so they cannot strand a
/// network child while an operation is being canceled.
pub(crate) async fn run_process_output_cancelled(
    program: &str,
    args: &[&str],
    cwd: Option<&std::path::Path>,
    cancel_token: Option<&crate::cancellation::CancellationToken>,
) -> Result<std::process::Output> {
    let mut builder = Command::new(program);
    builder
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(cwd) = cwd {
        builder.current_dir(cwd);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        builder.as_std_mut().process_group(0);
    }
    let mut child = builder
        .spawn()
        .map_err(|e| CruiseError::ProcessSpawnError(e.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .map(|mut pipe| tokio::spawn(async move { read_output_bounded(&mut pipe).await }));
    let stderr = child
        .stderr
        .take()
        .map(|mut pipe| tokio::spawn(async move { read_output_bounded(&mut pipe).await }));
    let status = tokio::select! {
        result = child.wait() => result,
        () = cancel_wait(cancel_token) => {
            terminate_process_group(child.id());
            let _ = child.kill().await;
            let _ = child.wait().await;
            abort_output_task(stdout).await;
            abort_output_task(stderr).await;
            return Err(CruiseError::Interrupted);
        }
    }
    .map_err(|e| CruiseError::CommandError(e.to_string()))?;
    let deadline = Some(Instant::now() + Duration::from_secs(5));
    let stdout = match stdout {
        Some(task) => collect_output(task, deadline).await,
        None => String::new(),
    };
    let stderr = match stderr {
        Some(task) => collect_output(task, deadline).await,
        None => String::new(),
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
    })
}

/// Return true if `stderr` indicates a rate-limit error.
#[must_use]
pub fn is_rate_limited(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("too many requests")
        || lower.contains("ratelimit")
}

/// Exponential backoff: 2s base, 60s cap.
#[must_use]
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

    struct QuietModeGuard;

    impl Drop for QuietModeGuard {
        fn drop(&mut self) {
            crate::console_mode::set_quiet(false);
        }
    }

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
        // capped at 1 minute
        assert_eq!(calculate_backoff(10), Duration::from_mins(1));
    }

    #[tokio::test]
    async fn test_run_successful_command() {
        let result = run_command("echo hello", 0, &HashMap::new(), None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_failing_command() {
        let result = run_command("exit 1", 0, &HashMap::new(), None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_run_commands_sequential() {
        let cmds = vec!["echo a".to_string(), "echo b".to_string()];
        let result = run_commands(&cmds, 0, &HashMap::new(), None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_run_commands_stops_on_failure() {
        // Second command would succeed but shouldn't run because first fails.
        let cmds = vec!["exit 1".to_string(), "echo ok".to_string()];
        let result = run_commands(&cmds, 0, &HashMap::new(), None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_run_commands_empty() {
        let result = run_commands(&[], 0, &HashMap::new(), None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_quiet_commands_capture_both_streams_for_step_logging() {
        let _lock = crate::test_support::lock_process();
        crate::console_mode::set_quiet(true);
        let _quiet = QuietModeGuard;
        let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let logs_for_callback = std::sync::Arc::clone(&logs);
        let on_step_log = move |stream: &str, line: &str| {
            logs_for_callback
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .push((stream.to_string(), line.to_string()));
        };

        let result = run_commands(
            &["printf stdout; printf stderr >&2".to_string()],
            0,
            &HashMap::new(),
            None,
            None,
            Some(&on_step_log),
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));

        assert!(result.success);
        assert_eq!(
            *logs.lock().unwrap_or_else(|e| panic!("{e}")),
            vec![
                ("stdout".to_string(), "stdout".to_string()),
                ("stderr".to_string(), "stderr".to_string()),
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_quiet_command_does_not_wait_for_descendant_holding_stdout() {
        let _lock = crate::test_support::lock_process();
        crate::console_mode::set_quiet(true);
        let _quiet = QuietModeGuard;
        let started = Instant::now();

        let result = run_command(
            "sleep 2 2>/dev/null & echo done",
            0,
            &HashMap::new(),
            None,
            Some(Duration::from_secs(1)),
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));

        assert!(result.success);
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_command_captures_stderr() {
        let result = run_command(
            "echo 'error msg' >&2; exit 1",
            0,
            &HashMap::new(),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
        assert!(result.stderr.contains("error msg"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_command_with_env() {
        let mut env = HashMap::new();
        env.insert("CRUISE_TEST_VAR".to_string(), "hello_env".to_string());
        // The command echoes the env var; success means env was passed correctly.
        let result = run_command(
            "test \"$CRUISE_TEST_VAR\" = hello_env",
            0,
            &env,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_commands_partial_failure_stderr() {
        // Second command fails with a message written to stderr.
        let cmds = vec![
            "echo step1".to_string(),
            "echo 'err_msg' >&2; exit 1".to_string(),
        ];
        let result = run_commands(&cmds, 0, &HashMap::new(), None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result.success);
        assert!(result.stderr.contains("err_msg"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_command_multiple_env_vars() {
        let mut env = HashMap::new();
        env.insert("VAR_A".to_string(), "alpha".to_string());
        env.insert("VAR_B".to_string(), "beta".to_string());
        let result = run_command(
            r#"test "$VAR_A" = alpha && test "$VAR_B" = beta"#,
            0,
            &env,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_command_env_in_echo() {
        let mut env = HashMap::new();
        env.insert("GREETING".to_string(), "hello".to_string());
        // stdout is inherited (not captured), but success means the command ran.
        let result = run_command("echo $GREETING", 0, &env, None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result.success);
    }
}
