use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const ACTIVE_RUNTIME_FILE: &str = ".cruise-runtime-active";

#[derive(Clone, Debug)]
pub enum InstallState {
    Managed(PathBuf),
    Homebrew,
    External,
}

/// Inspect install ownership without changing files. This is also used by the
/// launch path, so `cruise update --status` remains read-only.
pub fn inspect(executable: &Path) -> InstallState {
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home::home_dir().map(|home| home.join(".cargo")));
    inspect_with_cargo_home(executable, cargo_home.as_deref())
}

fn inspect_with_cargo_home(executable: &Path, cargo_home: Option<&Path>) -> InstallState {
    let resolved = fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    if is_homebrew_path(&resolved) {
        return InstallState::Homebrew;
    }

    let Some(directory) = resolved.parent() else {
        return InstallState::External;
    };
    let Some(cargo_home) = cargo_home else {
        return InstallState::External;
    };
    let Some(directory) = matching_cargo_home_bin(directory, cargo_home) else {
        return InstallState::External;
    };
    InstallState::Managed(directory)
}

pub fn resolve_runtime(executable: &Path) -> io::Result<PathBuf> {
    if let Some(runtime) = env::var_os("CRUISE_RUNTIME") {
        return require_executable(PathBuf::from(runtime));
    }

    let install_dir = match inspect(executable) {
        InstallState::Managed(directory) => {
            let active_path = directory.join(ACTIVE_RUNTIME_FILE);
            match fs::read_to_string(&active_path) {
                Ok(name) => {
                    let name = name.trim();
                    let path = Path::new(name);
                    let file_name = path.file_name();
                    if path.components().count() != 1
                        || file_name != Some(OsStr::new(name))
                        || !name.starts_with("cruise-runtime-")
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid runtime selection in {}", active_path.display()),
                        ));
                    }
                    return require_executable(directory.join(name));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => directory,
                Err(error) => return Err(error),
            }
        }
        InstallState::Homebrew | InstallState::External => executable
            .parent()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot determine Cruise install directory",
                )
            })?
            .to_path_buf(),
    };

    require_executable(install_dir.join(if cfg!(windows) {
        "cruise-runtime.exe"
    } else {
        "cruise-runtime"
    }))
}

fn require_executable(path: PathBuf) -> io::Result<PathBuf> {
    let metadata = fs::metadata(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Cruise runtime not found at {}", path.display()),
            )
        } else {
            error
        }
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Cruise runtime is not a file: {}", path.display()),
        ));
    }
    Ok(path)
}

fn is_homebrew_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => name == OsStr::new("Cellar") || name == OsStr::new("Caskroom"),
        _ => false,
    })
}

fn matching_cargo_home_bin(install_dir: &Path, cargo_home: &Path) -> Option<PathBuf> {
    let expected = cargo_home.join("bin");
    let expected = fs::canonicalize(&expected).unwrap_or(expected);
    let install_dir = fs::canonicalize(install_dir).unwrap_or_else(|_| install_dir.to_path_buf());
    (install_dir == expected).then_some(install_dir)
}

#[cfg(test)]
mod tests {
    use super::{InstallState, inspect_with_cargo_home};
    use std::fs;

    #[test]
    fn managed_cargo_home_bin_returns_its_canonical_path() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cargo_home = temp.path().join("cargo");
        let bin = cargo_home.join("bin");
        fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("failed to create bin: {error}"));
        let launcher = bin.join(if cfg!(windows) {
            "cruise.exe"
        } else {
            "cruise"
        });
        fs::write(&launcher, b"launcher")
            .unwrap_or_else(|error| panic!("failed to create launcher: {error}"));
        let canonical_bin = fs::canonicalize(&bin)
            .unwrap_or_else(|error| panic!("failed to canonicalize bin: {error}"));

        assert!(matches!(
            inspect_with_cargo_home(&launcher, Some(&cargo_home)),
            InstallState::Managed(directory) if directory == canonical_bin
        ));
    }
}
