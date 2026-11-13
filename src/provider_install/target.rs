use super::Provider;
use crate::error::{CruiseError, Result};
fn musl_from_maps(maps: &str) -> Option<bool> {
    if maps
        .lines()
        .any(|line| line.contains("ld-musl-") || line.contains("libc.musl-"))
    {
        Some(true)
    } else if maps
        .lines()
        .any(|line| line.contains("libc.so.6") || line.contains("ld-linux"))
    {
        Some(false)
    } else {
        None
    }
}

/// Detects the current machine's provider artifact target.
///
/// # Errors
/// Returns an error if this OS, architecture, or libc has no supported release.
pub fn detect_target_key(provider: Provider) -> Result<&'static str> {
    let arch = std::env::consts::ARCH;
    let musl = cfg!(target_os = "linux")
        && std::fs::read_to_string("/proc/self/maps")
            .ok()
            .and_then(|maps| musl_from_maps(&maps))
            .unwrap_or(cfg!(target_env = "musl"));
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    let avx2 = std::is_x86_feature_detected!("avx2");
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    let avx2 = false;
    target_key_for(provider, std::env::consts::OS, arch, musl, avx2)
}

/// Maps host characteristics to a provider artifact key.
///
/// # Errors
/// Returns an error if the supplied platform has no supported release.
pub fn target_key_for(
    provider: Provider,
    os: &str,
    arch: &str,
    musl: bool,
    avx2: bool,
) -> Result<&'static str> {
    use Provider::{Claude, Jcode, OpenCode};
    let key = match (provider, os, arch, musl, avx2) {
        (Jcode, "linux", "x86_64", false, _) => Some("linux-x86_64"),
        (Jcode, "linux", "aarch64", false, _) => Some("linux-aarch64"),
        (Jcode, "macos", "x86_64", _, _) => Some("macos-x86_64"),
        (Jcode, "macos", "aarch64", _, _) => Some("macos-aarch64"),
        (Jcode, "windows", "x86_64", _, _) => Some("windows-x86_64"),
        (Jcode, "windows", "aarch64", _, _) => Some("windows-aarch64"),
        (Claude, "macos", "x86_64", _, _) | (OpenCode, "macos", "x86_64", _, true) => {
            Some("darwin-x64")
        }
        (Claude | OpenCode, "macos", "aarch64", _, _) => Some("darwin-arm64"),
        (Claude, "linux", "x86_64", false, _) | (OpenCode, "linux", "x86_64", false, true) => {
            Some("linux-x64")
        }
        (Claude, "linux", "x86_64", true, _) | (OpenCode, "linux", "x86_64", true, true) => {
            Some("linux-x64-musl")
        }
        (Claude | OpenCode, "linux", "aarch64", false, _) => Some("linux-arm64"),
        (Claude | OpenCode, "linux", "aarch64", true, _) => Some("linux-arm64-musl"),
        (Claude, "windows", "x86_64", _, _) => Some("win32-x64"),
        (Claude, "windows", "aarch64", _, _) => Some("win32-arm64"),
        (OpenCode, "macos", "x86_64", _, false) => Some("darwin-x64-baseline"),
        (OpenCode, "linux", "x86_64", false, false) => Some("linux-x64-baseline"),
        (OpenCode, "linux", "x86_64", true, false) => Some("linux-x64-baseline-musl"),
        (OpenCode, "windows", "aarch64", _, _) => Some("windows-arm64"),
        (OpenCode, "windows", "x86_64", _, true) => Some("windows-x64"),
        (OpenCode, "windows", "x86_64", _, false) => Some("windows-x64-baseline"),
        _ => None,
    };
    key.ok_or_else(|| {
        CruiseError::Other(format!(
            "no {} build for {os}-{arch}; set {} or install {} on PATH",
            name(provider),
            provider.env_override(),
            name(provider)
        ))
    })
}
fn name(provider: Provider) -> &'static str {
    match provider {
        Provider::Jcode => "jcode",
        Provider::Claude => "claude",
        Provider::OpenCode => "opencode",
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "isolated target fixtures")]
mod tests {
    use super::*;
    #[test]
    fn libc_detection_uses_loaded_libraries() {
        assert_eq!(musl_from_maps("/lib/ld-musl-x86_64.so.1"), Some(true));
        assert_eq!(
            musl_from_maps("/lib/x86_64-linux-gnu/libc.so.6"),
            Some(false)
        );
        assert_eq!(musl_from_maps(""), None);
    }
    #[test]
    fn expected_target_mappings() {
        use Provider::{Claude, Jcode, OpenCode};
        let cases = [
            (Jcode, "linux", "x86_64", false, true, "linux-x86_64"),
            (Jcode, "linux", "aarch64", false, false, "linux-aarch64"),
            (Jcode, "macos", "x86_64", false, false, "macos-x86_64"),
            (Jcode, "macos", "aarch64", false, false, "macos-aarch64"),
            (Jcode, "windows", "x86_64", false, true, "windows-x86_64"),
            (Jcode, "windows", "aarch64", false, false, "windows-aarch64"),
            (Claude, "linux", "x86_64", false, false, "linux-x64"),
            (Claude, "linux", "aarch64", false, false, "linux-arm64"),
            (Claude, "linux", "x86_64", true, true, "linux-x64-musl"),
            (Claude, "linux", "aarch64", true, false, "linux-arm64-musl"),
            (Claude, "macos", "x86_64", false, true, "darwin-x64"),
            (Claude, "macos", "aarch64", false, false, "darwin-arm64"),
            (Claude, "windows", "x86_64", false, true, "win32-x64"),
            (Claude, "windows", "aarch64", false, false, "win32-arm64"),
            (OpenCode, "linux", "x86_64", false, true, "linux-x64"),
            (
                OpenCode,
                "linux",
                "x86_64",
                false,
                false,
                "linux-x64-baseline",
            ),
            (OpenCode, "linux", "x86_64", true, true, "linux-x64-musl"),
            (
                OpenCode,
                "linux",
                "x86_64",
                true,
                false,
                "linux-x64-baseline-musl",
            ),
            (OpenCode, "linux", "aarch64", false, false, "linux-arm64"),
            (
                OpenCode,
                "linux",
                "aarch64",
                true,
                false,
                "linux-arm64-musl",
            ),
            (OpenCode, "macos", "x86_64", false, true, "darwin-x64"),
            (
                OpenCode,
                "macos",
                "x86_64",
                false,
                false,
                "darwin-x64-baseline",
            ),
            (OpenCode, "macos", "aarch64", false, false, "darwin-arm64"),
            (OpenCode, "windows", "x86_64", false, true, "windows-x64"),
            (
                OpenCode,
                "windows",
                "x86_64",
                false,
                false,
                "windows-x64-baseline",
            ),
            (
                OpenCode,
                "windows",
                "aarch64",
                false,
                false,
                "windows-arm64",
            ),
        ];
        for (provider, os, arch, musl, avx2, expected) in cases {
            assert_eq!(
                target_key_for(provider, os, arch, musl, avx2).unwrap(),
                expected
            );
        }
        let error = target_key_for(Jcode, "linux", "x86_64", true, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no jcode build for linux-x86_64"));
        assert!(error.contains("CRUISE_JCODE_CLI_PATH"));
        assert!(target_key_for(Claude, "freebsd", "x86_64", false, true).is_err());
    }
}
