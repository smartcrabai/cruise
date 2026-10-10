//! `cruise workflow` subcommands.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::builtin_workflows::{BUILTIN_WORKFLOWS, find_builtin};
use crate::error::{CruiseError, Result};

/// Print the built-in workflow names and descriptions.
pub fn list() {
    for workflow in BUILTIN_WORKFLOWS {
        println!("{}\t{}", workflow.name, workflow.description);
    }
}

/// Eject a built-in workflow to the project (`./.cruise/`) or user workflows directory.
///
/// # Errors
///
/// Returns an error when the destination cannot be determined or ejecting fails.
pub fn eject(name: &str, project: bool) -> Result<()> {
    let dir = if project {
        std::env::current_dir()?.join(".cruise")
    } else {
        crate::paths::workflows_dir()?
    };
    let path = eject_builtin(name, &dir)?;
    println!("Ejected '{name}' to {}", path.display());
    Ok(())
}

/// Copy a built-in workflow to `<destination_dir>/<name>.yaml` without overwriting.
///
/// # Errors
///
/// Fails for unknown names, existing files, or I/O errors.
pub fn eject_builtin(name: &str, destination_dir: &Path) -> Result<PathBuf> {
    let workflow = find_builtin(name)
        .ok_or_else(|| CruiseError::Other(format!("unknown built-in workflow: {name}")))?;
    std::fs::create_dir_all(destination_dir)?;
    let path = destination_dir.join(format!("{}.yaml", workflow.name));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    if let Err(e) = file
        .write_all(workflow.yaml.as_bytes())
        .and_then(|()| file.flush())
    {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CruiseError;

    #[test]
    fn eject_builtin_copies_yaml_to_new_destination() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let dest = tmp.path().join("missing").join("parent");
        let path = eject_builtin("simple", &dest).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(path, dest.join("simple.yaml"));
        let expected = crate::builtin_workflows::find_builtin("simple")
            .unwrap_or_else(|| panic!("simple missing"))
            .yaml;
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), expected);
    }

    #[test]
    fn eject_builtin_does_not_overwrite_existing_file() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let existing = tmp.path().join("review.yaml");
        std::fs::write(&existing, b"mine: true\n").unwrap_or_else(|e| panic!("{e:?}"));
        let err = eject_builtin("review", tmp.path()).err();
        match err {
            Some(CruiseError::IoError(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
            }
            other => panic!("expected AlreadyExists IoError, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&existing).unwrap_or_default(),
            b"mine: true\n"
        );
    }

    #[test]
    fn eject_builtin_rejects_unknown_name_without_writing() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let dest = tmp.path().join("out");
        for name in ["nope", "../escape", "simple/../x"] {
            assert!(matches!(
                eject_builtin(name, &dest).err(),
                Some(CruiseError::Other(_))
            ));
        }
        assert!(!tmp.path().join("escape.yaml").exists());
        let entries = std::fs::read_dir(&dest).map_or(0, Iterator::count);
        assert_eq!(entries, 0);
    }
}
