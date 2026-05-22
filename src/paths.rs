use std::path::PathBuf;

use crate::error::Result;

/// Returns `$XDG_CONFIG_HOME/cruise` or `$HOME/.config/cruise`.
///
/// # Errors
///
/// Returns an error if neither `XDG_CONFIG_HOME` nor the home directory can be determined.
pub fn config_dir() -> Result<PathBuf> {
    xdg_or_home("XDG_CONFIG_HOME", &[".config"])
}

/// Returns `$XDG_DATA_HOME/cruise` or `$HOME/.local/share/cruise`.
///
/// # Errors
///
/// Returns an error if neither `XDG_DATA_HOME` nor the home directory can be determined.
pub fn data_dir() -> Result<PathBuf> {
    xdg_or_home("XDG_DATA_HOME", &[".local", "share"])
}

/// Returns `$XDG_STATE_HOME/cruise` or `$HOME/.local/state/cruise`.
///
/// # Errors
///
/// Returns an error if neither `XDG_STATE_HOME` nor the home directory can be determined.
pub fn state_dir() -> Result<PathBuf> {
    xdg_or_home("XDG_STATE_HOME", &[".local", "state"])
}

fn xdg_or_home(xdg_var: &str, home_base: &[&str]) -> Result<PathBuf> {
    use crate::error::CruiseError;
    if let Some(val) = std::env::var_os(xdg_var) {
        return Ok(PathBuf::from(val).join("cruise"));
    }
    let home = home::home_dir()
        .ok_or_else(|| CruiseError::Other("cannot determine home directory".to_string()))?;
    let mut path = home;
    for component in home_base {
        path = path.join(component);
    }
    Ok(path.join("cruise"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    use crate::test_support::{EnvGuard, lock_process, set_fake_home};

    // -- config_dir() -------------------------------------------------------

    #[test]
    fn test_config_dir_uses_xdg_config_home_when_set() {
        // Given: XDG_CONFIG_HOME points to a custom location
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(tmp.path());
        let _xdg = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("xdg-config").as_os_str());
        // When: config_dir() is called
        let dir = config_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: returns $XDG_CONFIG_HOME/cruise, not the HOME fallback
        assert_eq!(dir, tmp.path().join("xdg-config").join("cruise"));
    }

    #[test]
    fn test_config_dir_falls_back_to_home_dot_config_when_xdg_unset() {
        // Given: XDG_CONFIG_HOME is unset, HOME points to a fake path
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(tmp.path());
        // When: config_dir() is called
        let dir = config_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: returns $HOME/.config/cruise
        assert_eq!(dir, tmp.path().join(".config").join("cruise"));
    }

    // -- data_dir() ---------------------------------------------------------

    #[test]
    fn test_data_dir_uses_xdg_data_home_when_set() {
        // Given: XDG_DATA_HOME points to a custom location
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(tmp.path());
        let _xdg = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("xdg-data").as_os_str());
        // When: data_dir() is called
        let dir = data_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: returns $XDG_DATA_HOME/cruise, not the HOME fallback
        assert_eq!(dir, tmp.path().join("xdg-data").join("cruise"));
    }

    #[test]
    fn test_data_dir_falls_back_to_home_dot_local_share_when_xdg_unset() {
        // Given: XDG_DATA_HOME is unset, HOME points to a fake path
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(tmp.path());
        // When: data_dir() is called
        let dir = data_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: returns $HOME/.local/share/cruise
        assert_eq!(dir, tmp.path().join(".local").join("share").join("cruise"));
    }

    // -- state_dir() --------------------------------------------------------

    #[test]
    fn test_state_dir_uses_xdg_state_home_when_set() {
        // Given: XDG_STATE_HOME points to a custom location
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(tmp.path());
        let _xdg = EnvGuard::set("XDG_STATE_HOME", tmp.path().join("xdg-state").as_os_str());
        // When: state_dir() is called
        let dir = state_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: returns $XDG_STATE_HOME/cruise, not the HOME fallback
        assert_eq!(dir, tmp.path().join("xdg-state").join("cruise"));
    }

    #[test]
    fn test_state_dir_falls_back_to_home_dot_local_state_when_xdg_unset() {
        // Given: XDG_STATE_HOME is unset, HOME points to a fake path
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(tmp.path());
        // When: state_dir() is called
        let dir = state_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: returns $HOME/.local/state/cruise
        assert_eq!(dir, tmp.path().join(".local").join("state").join("cruise"));
    }

    // -- XDG_* env var priority over HOME fallback ---------------------------

    #[test]
    fn test_config_dir_xdg_takes_priority_over_home_fallback() {
        // Given: both XDG_CONFIG_HOME and HOME are set to different dirs
        let _lock = lock_process();
        let home_tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let xdg_tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(home_tmp.path());
        let _xdg = EnvGuard::set("XDG_CONFIG_HOME", xdg_tmp.path().as_os_str());
        // When: config_dir() is called
        let dir = config_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: XDG_CONFIG_HOME wins; result is under xdg_tmp, not home_tmp
        assert!(
            dir.starts_with(xdg_tmp.path()),
            "XDG_CONFIG_HOME should take priority, got: {}",
            dir.display()
        );
    }

    #[test]
    fn test_data_dir_xdg_takes_priority_over_home_fallback() {
        // Given: both XDG_DATA_HOME and HOME are set to different dirs
        let _lock = lock_process();
        let home_tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let xdg_tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(home_tmp.path());
        let _xdg = EnvGuard::set("XDG_DATA_HOME", xdg_tmp.path().as_os_str());
        // When: data_dir() is called
        let dir = data_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: XDG_DATA_HOME wins
        assert!(
            dir.starts_with(xdg_tmp.path()),
            "XDG_DATA_HOME should take priority, got: {}",
            dir.display()
        );
    }

    #[test]
    fn test_state_dir_xdg_takes_priority_over_home_fallback() {
        // Given: both XDG_STATE_HOME and HOME are set to different dirs
        let _lock = lock_process();
        let home_tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let xdg_tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = set_fake_home(home_tmp.path());
        let _xdg = EnvGuard::set("XDG_STATE_HOME", xdg_tmp.path().as_os_str());
        // When: state_dir() is called
        let dir = state_dir().unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: XDG_STATE_HOME wins
        assert!(
            dir.starts_with(xdg_tmp.path()),
            "XDG_STATE_HOME should take priority, got: {}",
            dir.display()
        );
    }

    // -- All three dirs end with "cruise" ------------------------------------

    #[test]
    fn test_all_dirs_end_with_cruise_component() {
        // Given: clean fake HOME, no XDG vars
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _guards = set_fake_home(tmp.path());
        // When: all three dirs are resolved
        let config = config_dir().unwrap_or_else(|e| panic!("{e}"));
        let data = data_dir().unwrap_or_else(|e| panic!("{e}"));
        let state = state_dir().unwrap_or_else(|e| panic!("{e}"));
        // Then: every path ends with the "cruise" component
        assert_eq!(
            config
                .file_name()
                .unwrap_or_else(|| panic!("config_dir must have a final component")),
            "cruise",
            "config_dir should end with 'cruise'"
        );
        assert_eq!(
            data.file_name()
                .unwrap_or_else(|| panic!("data_dir must have a final component")),
            "cruise",
            "data_dir should end with 'cruise'"
        );
        assert_eq!(
            state
                .file_name()
                .unwrap_or_else(|| panic!("state_dir must have a final component")),
            "cruise",
            "state_dir should end with 'cruise'"
        );
    }

    // -- set_fake_home isolates all XDG dirs from host env -------------------

    #[test]
    fn test_set_fake_home_isolates_all_three_dirs_from_host_xdg_vars() {
        // Given: set_fake_home is called (which also clears XDG_* vars)
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _guards = set_fake_home(tmp.path());
        // When: all three dirs are resolved
        let config = config_dir().unwrap_or_else(|e| panic!("{e}"));
        let data = data_dir().unwrap_or_else(|e| panic!("{e}"));
        let state = state_dir().unwrap_or_else(|e| panic!("{e}"));
        // Then: every path is rooted under the fake HOME (no host XDG_* leak)
        assert!(
            config.starts_with(tmp.path()),
            "config_dir should be under fake HOME: {}",
            config.display()
        );
        assert!(
            data.starts_with(tmp.path()),
            "data_dir should be under fake HOME: {}",
            data.display()
        );
        assert!(
            state.starts_with(tmp.path()),
            "state_dir should be under fake HOME: {}",
            state.display()
        );
    }
}
