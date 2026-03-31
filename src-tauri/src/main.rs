// Tauri GUI entry point - wired up in Phase 1 implementation.
fn main() {
    fix_path_for_gui();
    cruise_gui::run();
}

/// On macOS, GUI apps launched from Finder do not inherit the shell's PATH.
/// Spawn a login shell to get the user's full PATH and apply it to the process.
#[cfg(target_os = "macos")]
fn fix_path_for_gui() {
    use std::path::PathBuf;

    const MISE_SHIMS: &str = ".local/share/mise/shims";

    fn prepend_if_missing(paths: &mut Vec<PathBuf>, p: PathBuf) {
        if !paths.contains(&p) {
            paths.insert(0, p);
        }
    }

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    // Try interactive login shell first to capture mise/nvm/etc. activated in .zshrc.
    // Fall back to login-only shell if interactive fails (e.g. .zshrc has prompts).
    let path_from_shell = [
        &["-i", "-l", "-c", "echo $PATH"][..],
        &["-l", "-c", "echo $PATH"][..],
    ]
    .iter()
    .find_map(|args| {
        std::process::Command::new(&shell)
            .args(*args)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });
    if let Some(path) = path_from_shell {
        // Always ensure mise shims are present so that tools managed by
        // `mise activate` (non-shim mode) are still findable.
        let mut paths: Vec<PathBuf> = std::env::split_paths(&path).collect();
        if let Some(home) = home::home_dir() {
            prepend_if_missing(&mut paths, home.join(MISE_SHIMS));
        }
        if let Ok(joined) = std::env::join_paths(&paths) {
            // SAFETY: called at startup before Tauri spawns any threads.
            unsafe { std::env::set_var("PATH", joined) };
        }
        return;
    }
    // Fallback: shell PATH resolution failed. Append common user-tool directories.
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&existing).collect();
    for p in &["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"] {
        let pb = PathBuf::from(p);
        if !paths.contains(&pb) {
            paths.push(pb);
        }
    }
    if let Some(home) = home::home_dir() {
        // Insert in reverse order so MISE_SHIMS ends up at index 0 (highest priority).
        for suffix in &[".local/bin", ".cargo/bin", MISE_SHIMS] {
            prepend_if_missing(&mut paths, home.join(suffix));
        }
    }
    if let Ok(joined) = std::env::join_paths(&paths) {
        // SAFETY: called at startup before Tauri spawns any threads.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

#[cfg(not(target_os = "macos"))]
fn fix_path_for_gui() {}
