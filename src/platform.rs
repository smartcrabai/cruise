/// Re-claims the controlling terminal's foreground process group when it was
/// left pointing at a dead process group.
///
/// Cruise runs children on the user's terminal (a `command:` backend, an agent
/// backend's own tool calls). A child that puts itself in the terminal's
/// foreground process group and then exits leaves that group with no surviving
/// members, which makes this process count as "background": the next
/// terminal-mode change from reedline / inquire raises SIGTTOU and the user's
/// shell reports `suspended (tty output)`.
///
/// The repair is driven by that observed state, not by knowing which child did
/// it: this only acts when stdin is a tty *and* the current foreground group
/// has no surviving processes. A live owner (e.g. the user's shell after
/// Ctrl+Z) is never preempted, preserving normal job-control semantics.
pub(crate) fn reclaim_terminal_foreground() {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;

        // The SIGTTOU disposition below is process-global; serialise so two
        // prompts (e.g. an `ask_user` on a backend worker thread and a CLI
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

/// Opens a URL in the user's default browser.
///
/// # Errors
///
/// Returns an error when the platform opener cannot be spawned.
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(windows)]
    let command = "explorer.exe";
    #[cfg(not(any(target_os = "macos", windows)))]
    let command = "xdg-open";
    std::process::Command::new(command)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Restricts `path` to the current owner by installing a protected DACL.
///
/// # Errors
///
/// Returns an error when the security descriptor cannot be built or applied.
#[cfg(windows)]
pub(crate) fn set_owner_only_acl(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::{LocalFree, TRUE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
        SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    let sddl: Vec<u16> = "D:PAI(A;OICI;FA;;;OW)\0".encode_utf16().collect();
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: all pointers refer to live, NUL-terminated buffers or locals; the
    // descriptor allocated by the OS is freed with LocalFree before returning.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let result = if GetSecurityDescriptorDacl(
            descriptor,
            &mut present,
            &mut dacl,
            &mut defaulted,
        ) == 0
        {
            Err(std::io::Error::last_os_error())
        } else if present != TRUE {
            Err(std::io::Error::other("owner-only descriptor has no DACL"))
        } else {
            let status = SetNamedSecurityInfoW(
                wide_path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            );
            if status == 0 {
                Ok(())
            } else {
                Err(std::io::Error::from_raw_os_error(status as i32))
            }
        };
        LocalFree(descriptor);
        result
    }
}

/// Test helper: asserts `path` has a protected DACL holding only allow ACEs
/// for owner rights, SYSTEM, or Administrators.
#[cfg(all(test, windows))]
pub(crate) fn assert_owner_only_dacl(path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };

    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: all pointers refer to live locals or NUL-terminated buffers, and
    // OS-allocated memory is freed with LocalFree before returning.
    unsafe {
        let status = GetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        );
        assert_eq!(
            status,
            0,
            "GetNamedSecurityInfoW failed for {}",
            path.display()
        );
        let mut control = 0u16;
        let mut revision = 0u32;
        assert_ne!(
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
            0
        );
        assert!(control & SE_DACL_PROTECTED != 0, "DACL must be protected");
        assert!(!dacl.is_null(), "DACL must be present");
        let mut info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        assert_ne!(
            GetAclInformation(
                dacl,
                std::ptr::addr_of_mut!(info).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            ),
            0
        );
        assert!(info.AceCount > 0, "DACL must contain at least one ACE");
        for index in 0..info.AceCount {
            let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
            assert_ne!(GetAce(dacl, index, &mut ace), 0);
            let header = ace.cast::<ACE_HEADER>();
            assert_eq!(
                (*header).AceType,
                0,
                "only access-allowed ACEs are expected"
            );
            let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
            let sid = std::ptr::addr_of_mut!((*allowed).SidStart).cast();
            let mut raw: *mut u16 = std::ptr::null_mut();
            assert_ne!(ConvertSidToStringSidW(sid, &mut raw), 0);
            let mut len = 0;
            while *raw.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(raw, len));
            LocalFree(raw.cast());
            assert!(
                matches!(text.as_str(), "S-1-3-4" | "S-1-5-18" | "S-1-5-32-544"),
                "unexpected ACE principal {text}"
            );
        }
        LocalFree(descriptor);
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn windows_owner_only_acl_is_protected_for_file_and_dir() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("secret.txt");
        std::fs::write(&file, b"x").unwrap_or_else(|e| panic!("{e}"));
        super::set_owner_only_acl(&file).unwrap_or_else(|e| panic!("{e}"));
        super::set_owner_only_acl(dir.path()).unwrap_or_else(|e| panic!("{e}"));
        super::assert_owner_only_dacl(&file);
        super::assert_owner_only_dacl(dir.path());
        assert!(std::fs::read(&file).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn owner_only_acl_errors_for_missing_path() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        assert!(super::set_owner_only_acl(&dir.path().join("missing")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_command_uses_cmd_exe_c() {
        assert_eq!(super::shell_command(), ("cmd.exe", "/C"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_custom_command_captures_cmd_output() {
        let (shell, flag) = super::shell_command();
        let output = std::process::Command::new(shell)
            .args([flag, "echo a b& echo %CRUISE_PLATFORM_TEST%"])
            .env("CRUISE_PLATFORM_TEST", "expanded value")
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("a b"), "{text}");
        assert!(text.contains("expanded value"), "{text}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_open_url_spawns_native_opener_without_shell() {
        // `explorer.exe` is a native executable, so `&` and spaces in the URL
        // are passed as one argument and never interpreted by a shell.
        assert!(super::open_url("about:blank?a=1&b=two words").is_ok());
        let missing = std::process::Command::new("cruise-missing-opener.exe")
            .arg("x")
            .spawn();
        assert!(missing.is_err());
    }

    #[test]
    fn reclaim_terminal_foreground_never_panics() {
        // Without a tty (test runner) this is a no-op; with a tty the
        // foreground group is alive, so nothing is preempted either way.
        super::reclaim_terminal_foreground();
    }
}
