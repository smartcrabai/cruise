// Tauri GUI entry point — wired up in Phase 1 implementation.
fn main() {
    fix_path_for_gui();
    cruise_gui::run();
}

/// On macOS, GUI apps launched from Finder do not inherit the shell's PATH.
/// Spawn a login shell to get the user's full PATH and apply it to the process.
#[cfg(target_os = "macos")]
fn fix_path_for_gui() {
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
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn fix_path_for_gui() {}
