// Tauri GUI entry point — wired up in Phase 1 implementation.
fn main() {
    fix_path_for_gui();
    cruise_gui::run();
}

/// On macOS, GUI apps launched from Finder do not inherit the shell's PATH.
/// Spawn a login shell to get the user's full PATH and apply it to the process.
#[cfg(target_os = "macos")]
fn fix_path_for_gui() {
    use std::path::PathBuf;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    if let Ok(output) = std::process::Command::new(&shell)
        .args(["-l", "-c", "echo $PATH"])
        .output()
    {
        if let Ok(path) = String::from_utf8(output.stdout) {
            let path = path.trim();
            if !path.is_empty() {
                // SAFETY: called at startup before Tauri spawns any threads.
                unsafe { std::env::set_var("PATH", path) };
                return;
            }
        }
    }
    // Fallback: shell PATH resolution failed silently. Append common user-tool
    // directories so binaries like `claude`, `seher`, etc. can be found.
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&existing).collect();
    for p in &["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"] {
        let pb = PathBuf::from(p);
        if !paths.contains(&pb) {
            paths.push(pb);
        }
    }
    if let Some(home) = home::home_dir() {
        // Insert in reverse order so .cargo/bin ends up at index 0 (highest priority).
        for suffix in &[".local/bin", ".cargo/bin"] {
            let p = home.join(suffix);
            if !paths.contains(&p) {
                paths.insert(0, p);
            }
        }
    }
    if let Ok(joined) = std::env::join_paths(&paths) {
        // SAFETY: called at startup before Tauri spawns any threads.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

#[cfg(not(target_os = "macos"))]
fn fix_path_for_gui() {}
