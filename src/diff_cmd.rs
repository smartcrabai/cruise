use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

use crate::cli::DiffArgs;
use crate::daemon::client::DaemonClient;
use crate::error::Result;

pub(crate) fn write_session_diff<W: Write>(writer: &mut W, diff: &str) -> io::Result<()> {
    writer.write_all(diff.as_bytes())
}

pub async fn print_session_diff(client: &DaemonClient, session_id: &str, stat: bool) -> Result<()> {
    let use_pager = !stat && io::stdout().is_terminal();
    let diff = client.diff(session_id, stat, use_pager).await?;
    if diff.text.is_empty() {
        eprintln!("No changes on {}", diff.branch);
    }
    if diff.truncated {
        eprintln!("warning: diff truncated");
    }
    if use_pager && let Ok(mut child) = Command::new("less").arg("-R").stdin(Stdio::piped()).spawn()
    {
        let result = child
            .stdin
            .take()
            .map(|mut stdin| write_session_diff(&mut stdin, &diff.text));
        let _ = child.wait();
        if let Some(result) = result {
            match result {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
    }
    write_session_diff(&mut io::stdout().lock(), &diff.text)?;
    Ok(())
}

pub async fn run(args: &DiffArgs) -> Result<()> {
    let client = DaemonClient::connect().await?;
    print_session_diff(&client, &args.session, args.stat).await
}
