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
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::{
            HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
        };

        // The runtime receives duplicated stdio handles. Don't pass the
        // launcher's original captured pipes as extra inheritable handles.
        for handle in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                // SAFETY: these are borrowed standard handles, and changing the
                // inheritance flag does not close or invalidate them.
                if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
    }
    let executable = std::env::current_exe()?;
    let runtime = install_identity::resolve_runtime(&executable)?;
    let status = Command::new(runtime)
        .args(std::env::args_os().skip(1))
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}
