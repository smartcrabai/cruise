use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

use crate::branch_diff::{BranchDiff, session_branch_diff};
use crate::cli::DiffArgs;
use crate::error::Result;
use crate::session::{SessionManager, SessionState};

pub fn write_session_diff<W: Write>(w: &mut W, diff: &BranchDiff, stat: bool) -> io::Result<()> {
    w.write_all(if stat { &diff.stat } else { &diff.patch }.as_bytes())
}

pub fn print_session_diff(state: &SessionState, stat: bool) -> Result<()> {
    let use_pager = !stat && io::stdout().is_terminal();
    let diff = session_branch_diff(state, use_pager)?;
    if diff.patch.is_empty() && diff.stat.is_empty() {
        eprintln!("No changes on {}", diff.branch);
    }
    if diff.truncated {
        eprintln!("warning: diff truncated");
    }
    if use_pager {
        if let Ok(mut child) = Command::new("less").arg("-R").stdin(Stdio::piped()).spawn() {
            let result = child
                .stdin
                .take()
                .map(|mut stdin| write_session_diff(&mut stdin, &diff, false));
            let _ = child.wait();
            if let Some(result) = result {
                match result {
                    Ok(()) => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    write_session_diff(&mut io::stdout().lock(), &diff, stat)?;
    Ok(())
}

pub fn run(args: DiffArgs) -> Result<()> {
    let manager = SessionManager::new(crate::paths::data_dir()?);
    let state = manager.load(&args.session)?;
    print_session_diff(&state, args.stat)
}
