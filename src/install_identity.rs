use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const ACTIVE_RUNTIME_FILE: &str = ".cruise-runtime-active";
pub const MANAGED_RUNTIME_PREFIX: &str = "cruise-runtime-managed-";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallBaseline {
    launcher: MetadataStamp,
    runtime: MetadataStamp,
}

/// shortcut: same-size replacements preserving mtime are indistinguishable; use stronger portable identity if available.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MetadataStamp {
    size: u64,
    modified_seconds: u64,
    modified_nanoseconds: u32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntime {
    pub schema_version: u32,
    pub runtime: String,
    pub version: String,
    pub target: String,
    pub sha256: String,
    pub baseline: InstallBaseline,
}

pub fn valid_runtime_name(name: &str) -> bool {
    name.starts_with("cruise-runtime-")
        && name.len() > "cruise-runtime-".len()
        && name
            .bytes()
            .all(|byte| byte != b'/' && byte != b'\\' && byte != 0 && !byte.is_ascii_control())
        && Path::new(name).components().count() == 1
        && Path::new(name).file_name() == Some(OsStr::new(name))
}

pub fn metadata_path(runtime: &Path) -> PathBuf {
    let mut path = runtime.as_os_str().to_os_string();
    path.push(".json");
    PathBuf::from(path)
}

pub fn launcher_name() -> &'static str {
    if cfg!(windows) {
        "cruise.exe"
    } else {
        "cruise"
    }
}

pub fn runtime_name() -> &'static str {
    if cfg!(windows) {
        "cruise-runtime.exe"
    } else {
        "cruise-runtime"
    }
}

pub fn install_baseline(bin: &Path) -> io::Result<InstallBaseline> {
    Ok(InstallBaseline {
        launcher: metadata_stamp(&bin.join(launcher_name()))?,
        runtime: metadata_stamp(&bin.join(runtime_name()))?,
    })
}

fn metadata_stamp(path: &Path) -> io::Result<MetadataStamp> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "install baseline member is not a regular file: {}",
                path.display()
            ),
        ));
    }
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(MetadataStamp {
        size: metadata.len(),
        modified_seconds: modified.as_secs(),
        modified_nanoseconds: modified.subsec_nanos(),
    })
}

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
    resolve_runtime_for_install(executable, inspect(executable))
}

#[cfg(test)]
fn resolve_runtime_with_inputs(
    executable: &Path,
    override_runtime: Option<&OsStr>,
    cargo_home: Option<&Path>,
) -> io::Result<PathBuf> {
    if let Some(runtime) = override_runtime {
        return require_executable(PathBuf::from(runtime));
    }
    resolve_runtime_for_install(executable, inspect_with_cargo_home(executable, cargo_home))
}

fn resolve_runtime_for_install(
    executable: &Path,
    install_state: InstallState,
) -> io::Result<PathBuf> {
    let install_dir = match install_state {
        InstallState::Managed(directory) => {
            let active_path = directory.join(ACTIVE_RUNTIME_FILE);
            match fs::symlink_metadata(&active_path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return warn_and_use_fixed_runtime(
                        &directory,
                        &active_path,
                        "active pointer is a symlink",
                    );
                }
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return warn_and_use_fixed_runtime(
                        &directory,
                        &active_path,
                        "active pointer is not a regular file",
                    );
                }
                Ok(_) => {
                    let name = fs::read_to_string(&active_path)?;
                    let name = name.trim();
                    if !valid_runtime_name(name) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid runtime selection in {}", active_path.display()),
                        ));
                    }
                    if name.starts_with(MANAGED_RUNTIME_PREFIX) {
                        return match managed_runtime_path(&directory, name) {
                            Ok(path) => Ok(path),
                            Err(error) => warn_and_use_fixed_runtime(
                                &directory,
                                &active_path,
                                &format!("managed overlay is invalid ({error})"),
                            ),
                        };
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

    require_executable(install_dir.join(runtime_name()))
}

fn managed_runtime_path(directory: &Path, name: &str) -> io::Result<PathBuf> {
    let runtime = directory.join(name);
    require_managed_executable(&runtime)?;

    let sidecar = metadata_path(&runtime);
    let sidecar_metadata = fs::symlink_metadata(&sidecar)?;
    if !sidecar_metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "managed runtime metadata is not a regular file: {}",
                sidecar.display()
            ),
        ));
    }
    let record: ManagedRuntime = serde_json::from_slice(&fs::read(&sidecar)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if record.schema_version != 1
        || record.runtime != name
        || record.version.trim().is_empty()
        || !name
            .strip_prefix(MANAGED_RUNTIME_PREFIX)
            .is_some_and(|suffix| {
                suffix
                    .strip_prefix(&record.version)
                    .is_some_and(|rest| rest.starts_with('-'))
            })
        || record.target.trim().is_empty()
        || record.sha256.len() != 64
        || !record.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || record.baseline != install_baseline(directory)?
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "managed runtime metadata does not match {}",
                runtime.display()
            ),
        ));
    }
    Ok(runtime)
}

fn require_managed_executable(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("managed runtime is not a regular file: {}", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("managed runtime is not executable: {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn warn_and_use_fixed_runtime(
    directory: &Path,
    active_path: &Path,
    reason: &str,
) -> io::Result<PathBuf> {
    eprintln!(
        "warning: ignoring runtime selection in {}: {reason}; using fixed sibling runtime",
        active_path.display()
    );
    require_executable(directory.join(runtime_name()))
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
    use super::{
        ACTIVE_RUNTIME_FILE, InstallState, ManagedRuntime, inspect_with_cargo_home,
        install_baseline, metadata_path, resolve_runtime_with_inputs, runtime_name,
        valid_runtime_name,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    fn write_executable(path: &Path, contents: &[u8]) {
        fs::write(path, contents)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|error| panic!("failed to chmod {}: {error}", path.display()));
        }
    }

    fn install(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let cargo_home = root.join("cargo-home");
        let bin = cargo_home.join("bin");
        fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("failed to create bin: {error}"));
        let cargo_home = fs::canonicalize(cargo_home)
            .unwrap_or_else(|error| panic!("failed to canonicalize Cargo Home: {error}"));
        let bin = cargo_home.join("bin");
        let launcher = bin.join(super::launcher_name());
        write_executable(&launcher, b"launcher");
        write_executable(&bin.join(runtime_name()), b"fixed-runtime");
        (cargo_home, bin, launcher)
    }

    fn managed_name() -> &'static str {
        if cfg!(windows) {
            "cruise-runtime-managed-1.2.3-test.exe"
        } else {
            "cruise-runtime-managed-1.2.3-test"
        }
    }

    fn install_overlay(bin: &Path, name: &str) {
        write_executable(&bin.join(name), b"overlay-runtime");
        let record = ManagedRuntime {
            schema_version: 1,
            runtime: name.to_owned(),
            version: "1.2.3".to_owned(),
            target: "test-target".to_owned(),
            sha256: "ab".repeat(32),
            baseline: install_baseline(bin)
                .unwrap_or_else(|error| panic!("baseline failed: {error}")),
        };
        fs::write(
            metadata_path(&bin.join(name)),
            serde_json::to_vec(&record)
                .unwrap_or_else(|error| panic!("sidecar serialization failed: {error}")),
        )
        .unwrap_or_else(|error| panic!("sidecar write failed: {error}"));
        fs::write(bin.join(ACTIVE_RUNTIME_FILE), name)
            .unwrap_or_else(|error| panic!("pointer write failed: {error}"));
    }

    #[test]
    fn managed_cargo_home_bin_returns_its_canonical_path() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let (cargo_home, bin, launcher) = install(temp.path());
        let canonical_bin = fs::canonicalize(&bin)
            .unwrap_or_else(|error| panic!("failed to canonicalize bin: {error}"));
        assert!(matches!(
            inspect_with_cargo_home(&launcher, Some(&cargo_home)),
            InstallState::Managed(directory) if directory == canonical_bin
        ));
    }

    #[test]
    fn homebrew_classification_precedes_cargo_home_matching() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cargo_home = temp.path().join("Cellar/cruise/1.2.3");
        let bin = cargo_home.join("bin");
        fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("failed to create bin: {error}"));
        let launcher = bin.join(super::launcher_name());
        write_executable(&launcher, b"launcher");
        assert!(matches!(
            inspect_with_cargo_home(&launcher, Some(&cargo_home)),
            InstallState::Homebrew
        ));

        let external = temp.path().join("external");
        fs::create_dir_all(&external)
            .unwrap_or_else(|error| panic!("failed to create external dir: {error}"));
        let executable = external.join("cruise");
        fs::write(&executable, b"launcher")
            .unwrap_or_else(|error| panic!("failed to write external executable: {error}"));
        assert!(matches!(
            inspect_with_cargo_home(&executable, Some(&cargo_home)),
            InstallState::External
        ));
    }

    #[test]
    fn runtime_names_allow_legacy_and_managed_basenames_only() {
        assert!(valid_runtime_name("cruise-runtime-0.2.16"));
        assert!(valid_runtime_name(managed_name()));
        assert!(!valid_runtime_name("cruise-runtime-"));
        assert!(!valid_runtime_name("../cruise-runtime-0.2.16"));
        assert!(!valid_runtime_name("cruise-runtime-../outside"));
        assert!(!valid_runtime_name("cruise-runtime-foo\\bar"));
        assert!(!valid_runtime_name("runtime-cruise"));
    }

    #[test]
    fn healthy_managed_overlay_is_selected_and_override_still_wins() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let (cargo_home, bin, launcher) = install(temp.path());
        install_overlay(&bin, managed_name());
        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(managed_name())
        );

        let override_runtime = temp.path().join("override-runtime");
        fs::write(&override_runtime, b"override")
            .unwrap_or_else(|error| panic!("failed to write override runtime: {error}"));
        assert_eq!(
            resolve_runtime_with_inputs(
                &launcher,
                Some(override_runtime.as_os_str()),
                Some(&cargo_home),
            )
            .unwrap_or_else(|error| panic!("override resolution failed: {error}")),
            override_runtime
        );
    }

    #[test]
    fn missing_or_corrupt_managed_sidecar_falls_back_to_fixed_runtime() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let (cargo_home, bin, launcher) = install(temp.path());
        let candidate = bin.join(managed_name());
        write_executable(&candidate, b"overlay-runtime");
        fs::write(bin.join(ACTIVE_RUNTIME_FILE), managed_name())
            .unwrap_or_else(|error| panic!("pointer write failed: {error}"));
        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(runtime_name())
        );

        fs::write(metadata_path(&candidate), b"{not-json")
            .unwrap_or_else(|error| panic!("corrupt sidecar write failed: {error}"));
        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(runtime_name())
        );
    }

    #[test]
    fn changed_fixed_pair_metadata_disables_old_overlay() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let (cargo_home, bin, launcher) = install(temp.path());
        install_overlay(&bin, managed_name());
        write_executable(&bin.join(runtime_name()), b"reinstalled-fixed-runtime");

        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(runtime_name())
        );
    }

    #[test]
    fn legacy_runtime_pointer_keeps_selection_and_traversal_is_rejected() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let (cargo_home, bin, launcher) = install(temp.path());
        let legacy = if cfg!(windows) {
            "cruise-runtime-0.2.16.exe"
        } else {
            "cruise-runtime-0.2.16"
        };
        fs::write(bin.join(legacy), b"legacy-runtime")
            .unwrap_or_else(|error| panic!("legacy runtime write failed: {error}"));
        fs::write(bin.join(ACTIVE_RUNTIME_FILE), legacy)
            .unwrap_or_else(|error| panic!("pointer write failed: {error}"));
        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(legacy)
        );

        fs::write(bin.join(ACTIVE_RUNTIME_FILE), "../outside")
            .unwrap_or_else(|error| panic!("pointer write failed: {error}"));
        let Err(error) = resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home)) else {
            panic!("traversal pointer must be rejected");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn new_overlay_rejects_symlinked_runtime_sidecar_and_pointer() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let (cargo_home, bin, launcher) = install(temp.path());
        let name = managed_name();
        install_overlay(&bin, name);
        let runtime = bin.join(name);
        let external_runtime = temp.path().join("external-runtime");
        write_executable(&external_runtime, b"external-runtime");
        fs::remove_file(&runtime).unwrap_or_else(|error| panic!("runtime removal failed: {error}"));
        symlink(&external_runtime, &runtime)
            .unwrap_or_else(|error| panic!("runtime symlink failed: {error}"));
        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(runtime_name())
        );

        fs::remove_file(&runtime).unwrap_or_else(|error| panic!("runtime removal failed: {error}"));
        write_executable(&runtime, b"overlay-runtime");
        let sidecar = metadata_path(&runtime);
        let sidecar_target = temp.path().join("sidecar-target");
        fs::copy(&sidecar, &sidecar_target)
            .unwrap_or_else(|error| panic!("sidecar copy failed: {error}"));
        fs::remove_file(&sidecar).unwrap_or_else(|error| panic!("sidecar removal failed: {error}"));
        symlink(&sidecar_target, &sidecar)
            .unwrap_or_else(|error| panic!("sidecar symlink failed: {error}"));
        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(runtime_name())
        );

        fs::remove_file(&sidecar).unwrap_or_else(|error| panic!("sidecar removal failed: {error}"));
        let pointer = bin.join(ACTIVE_RUNTIME_FILE);
        let pointer_target = temp.path().join("pointer-target");
        fs::copy(&pointer, &pointer_target)
            .unwrap_or_else(|error| panic!("pointer copy failed: {error}"));
        fs::remove_file(&pointer).unwrap_or_else(|error| panic!("pointer removal failed: {error}"));
        symlink(&pointer_target, &pointer)
            .unwrap_or_else(|error| panic!("pointer symlink failed: {error}"));
        assert_eq!(
            resolve_runtime_with_inputs(&launcher, None, Some(&cargo_home))
                .unwrap_or_else(|error| panic!("resolver failed: {error}")),
            bin.join(runtime_name())
        );
    }
}
