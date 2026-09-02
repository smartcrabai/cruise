use std::fmt::Write as _;
use std::io::IsTerminal as _;
use std::process::{Command, Stdio};

use crate::cli::{SshArgs, SshTtyMode};
use crate::error::{CruiseError, Result};

/// Run a complete cruise command on the destination through the system
/// OpenSSH client. The remote cruise process owns all session and workspace
/// state, so this launcher deliberately does no local cruise setup.
pub(crate) fn run(args: &SshArgs) -> Result<()> {
    let local_streams_are_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let mut command = build_command(args, local_streams_are_tty);
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            CruiseError::Other(format!(
                "could not run ssh: OpenSSH client is not available on PATH: {error}"
            ))
        })?;

    if status.success() {
        return Ok(());
    }

    Err(CruiseError::Other(match status.code() {
        Some(code) => format!("ssh exited with status {code}"),
        None => "ssh was terminated by a signal".to_string(),
    }))
}

fn build_command(args: &SshArgs, local_streams_are_tty: bool) -> Command {
    let mut command = Command::new("ssh");
    match args.tty {
        SshTtyMode::Auto if local_streams_are_tty => {
            command.arg("-t");
        }
        SshTtyMode::Always => {
            // OpenSSH requires multiple -t options to force allocation when
            // the local process has no TTY, such as when stdin is piped.
            command.arg("-tt");
        }
        SshTtyMode::Never => {
            command.arg("-T");
        }
        SshTtyMode::Auto => {}
    }
    command
        .arg("--")
        .arg(&args.destination)
        .arg(remote_command(args));
    command
}

fn remote_command(args: &SshArgs) -> String {
    let mut command = String::new();
    if let Some(cwd) = &args.cwd {
        let _ = write!(command, "cd {} && ", shell_quote(cwd));
    }
    let _ = write!(command, "exec {}", shell_quote(&args.cruise_bin));
    for argument in &args.args {
        let _ = write!(command, " {}", shell_quote(argument));
    }
    command
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_preserves_shell_data_as_one_word() {
        for value in [
            "",
            "  whitespace  ",
            "single'quote",
            "$(touch marker); `echo unsafe`\n$HOME",
        ] {
            assert_eq!(
                shell_quote(value),
                format!("'{}'", value.replace('\'', "'\\''"))
            );
        }
    }

    #[test]
    fn remote_command_quotes_cwd_binary_and_forwarded_args_independently() {
        let args = SshArgs {
            destination: "devbox".to_string(),
            cwd: Some("/srv/project with spaces".to_string()),
            cruise_bin: "/opt/cruise bin".to_string(),
            tty: SshTtyMode::Auto,
            args: vec!["--plan".to_string(), "task with spaces".to_string()],
        };

        assert_eq!(
            remote_command(&args),
            "cd '/srv/project with spaces' && exec '/opt/cruise bin' '--plan' 'task with spaces'"
        );
    }

    #[test]
    fn tty_mode_maps_to_expected_ssh_arguments() {
        let args = SshArgs {
            destination: "devbox".to_string(),
            cwd: None,
            cruise_bin: "cruise".to_string(),
            tty: SshTtyMode::Always,
            args: vec!["list".to_string()],
        };
        let arguments = |args: &SshArgs, local_tty| {
            build_command(args, local_tty)
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            arguments(&args, false),
            ["-tt", "--", "devbox", "exec 'cruise' 'list'"]
        );
        assert_eq!(
            arguments(&args, true),
            ["-tt", "--", "devbox", "exec 'cruise' 'list'"]
        );

        let mut never = args;
        never.tty = SshTtyMode::Never;
        assert_eq!(
            arguments(&never, true),
            ["-T", "--", "devbox", "exec 'cruise' 'list'"]
        );

        let mut auto = never;
        auto.tty = SshTtyMode::Auto;
        assert_eq!(
            arguments(&auto, false),
            ["--", "devbox", "exec 'cruise' 'list'"]
        );
        assert_eq!(
            arguments(&auto, true),
            ["-t", "--", "devbox", "exec 'cruise' 'list'"]
        );
    }
}
