use crate::cancellation::CancellationToken;
use command_group::CommandGroup;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub struct Outcome {
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub message: String,
    pub stdout: PathBuf,
    pub stderr: PathBuf,
}
impl Outcome {
    pub fn has_violation(&self) -> bool {
        [&self.stdout, &self.stderr].iter().any(|path| {
            fs::read(path).is_ok_and(|bytes| bytes.windows(11).any(|w| w == b"[violation]"))
        })
    }
    pub fn output(&self) -> String {
        let mut result = String::new();
        for path in [&self.stdout, &self.stderr] {
            if let Ok(bytes) = fs::read(path) {
                let start = bytes.len().saturating_sub(2048);
                result.push_str(&String::from_utf8_lossy(&bytes[start..]));
            }
        }
        if result.len() > 2048 {
            let mut start = result.len() - 2048;
            while !result.is_char_boundary(start) {
                start += 1;
            }
            result.drain(..start);
        }
        result
    }
}
pub fn run_bounded(
    mut command: Command,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
    dir: &Path,
    stage: &str,
) -> Outcome {
    let stdout = dir.join(format!("{stage}.stdout.log"));
    let stderr = dir.join(format!("{stage}.stderr.log"));
    let mut result = Outcome {
        exit: None,
        timed_out: false,
        cancelled: false,
        message: String::new(),
        stdout,
        stderr,
    };
    let start = Instant::now();
    let result_run = (|| -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let out = File::create(&result.stdout)?;
        let err = File::create(&result.stderr)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        let mut child = command.group_spawn()?;
        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                result.cancelled = true;
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            if start.elapsed() >= timeout {
                result.timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            if let Some(status) = child.try_wait()? {
                result.exit = status.code();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    })();
    if let Err(e) = result_run {
        result.message = e.to_string();
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn timeout_kills_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("descendant-finished");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("(sleep 1; touch \"$1\") & wait")
            .arg("sh")
            .arg(&marker);
        let start = Instant::now();
        let outcome = run_bounded(
            command,
            Duration::from_millis(200),
            None,
            dir.path(),
            "timeout",
        );
        assert!(outcome.timed_out, "{}", outcome.message);
        assert!(!outcome.cancelled);
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1200));
        assert!(!marker.exists(), "descendant survived timeout");
    }

    #[test]
    fn cancellation_kills_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("descendant-finished");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("(sleep 1; touch \"$1\") & wait")
            .arg("sh")
            .arg(&marker);
        let token = CancellationToken::new();
        let cancel = token.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            cancel.cancel();
        });
        let outcome = run_bounded(
            command,
            Duration::from_secs(5),
            Some(&token),
            dir.path(),
            "cancel",
        );
        thread.join().unwrap();
        assert!(outcome.cancelled, "{}", outcome.message);
        assert!(!outcome.timed_out);
        std::thread::sleep(Duration::from_millis(1200));
        assert!(!marker.exists(), "descendant survived cancellation");
    }

    #[test]
    fn stdin_is_null_even_when_command_requests_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("if read line; then exit 1; else exit 0; fi")
            .stdin(Stdio::piped());
        let outcome = run_bounded(command, Duration::from_secs(3), None, dir.path(), "stdin");
        assert_eq!(outcome.exit, Some(0), "{}", outcome.message);
        assert!(!outcome.timed_out);
    }
}
