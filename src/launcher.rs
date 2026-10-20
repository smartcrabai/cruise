#[path = "install_identity.rs"]
mod install_identity;

use std::io;
use std::process::Command;

fn main() {
    if let Err(error) = launch() {
        eprintln!("cruise launcher: {error}");
        std::process::exit(1);
    }
}

#[cfg(unix)]
fn launch() -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    let executable = std::env::current_exe()?;
    let runtime = install_identity::resolve_runtime(&executable)?;
    let error = Command::new(runtime)
        .args(std::env::args_os().skip(1))
        .exec();
    Err(error)
}

#[cfg(any(windows, not(unix)))]
fn launch() -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let runtime = install_identity::resolve_runtime(&executable)?;
    let status = Command::new(runtime)
        .args(std::env::args_os().skip(1))
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

