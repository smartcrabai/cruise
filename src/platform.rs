/// Re-claims the controlling terminal's foreground process group when it was
/// left pointing at a dead process group.
///
/// Child processes spawned during SDK execution can steal the terminal: the
/// codexbar usage probe runs `/bin/zsh -l -i -c 'printf ... "$PATH"'`, and an
/// interactive zsh initialises job control by making itself the terminal's
/// foreground process group. When it exits, the foreground group is dead and
/// this process counts as "background" — the next terminal-mode change from
/// reedline / inquire raises SIGTTOU and the user's shell reports
/// `suspended (tty output)`.
///
/// Only acts when stdin is a tty and the current foreground group has no
/// surviving processes. A live owner (e.g. the user's shell after Ctrl+Z) is
/// never preempted, preserving normal job-control semantics.
pub(crate) fn reclaim_terminal_foreground() {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;

        // The SIGTTOU disposition below is process-global; serialise so two
        // prompts (e.g. an `ask_user` on the pi worker thread and a CLI
        // select) can never interleave the save/restore.
        static RECLAIM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = RECLAIM_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let fd = std::io::stdin().as_raw_fd();
        // SAFETY: raw syscalls on the stdin fd; the SIGTTOU disposition is
        // restored immediately after tcsetpgrp.
        unsafe {
            if libc::isatty(fd) != 1 {
                return;
            }
            let own = libc::getpgrp();
            let fg = libc::tcgetpgrp(fd);
            if fg < 0 || fg == own {
                return;
            }
            // killpg(pg, 0) probes for liveness: ESRCH means every process in
            // the group is gone. Any other outcome (success, EPERM) means the
            // group is alive and legitimately owns the terminal.
            if libc::killpg(fg, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return;
            }
            // tcsetpgrp from a non-foreground group itself raises SIGTTOU;
            // ignore it for the duration of the call.
            let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            if previous == libc::SIG_ERR {
                return;
            }
            let _ = libc::tcsetpgrp(fd, own);
            libc::signal(libc::SIGTTOU, previous);
        }
    }
}

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

#[cfg(test)]
mod tests {
    #[test]
    fn reclaim_terminal_foreground_never_panics() {
        // Without a tty (test runner) this is a no-op; with a tty the
        // foreground group is alive, so nothing is preempted either way.
        super::reclaim_terminal_foreground();
    }
}
