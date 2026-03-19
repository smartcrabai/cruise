/// Returns the shell executable and flag for running a command string on the current platform.
#[must_use]
pub(crate) fn shell_command() -> (&'static str, &'static str) {
    #[cfg(unix)]
    {
        ("sh", "-c")
    }
    #[cfg(windows)]
    {
        ("cmd.exe", "/C")
    }
}
