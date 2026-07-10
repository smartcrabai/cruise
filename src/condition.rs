use crate::config::{SkipCondition, WhenCondition};
use crate::error::Result;
use crate::variable::VariableStore;

/// Returns true if the step should be skipped.
///
/// # Errors
///
/// Returns an error if the skip condition references an undefined variable.
pub fn should_skip(skip: Option<&SkipCondition>, vars: &VariableStore) -> Result<bool> {
    match skip {
        None => Ok(false),
        Some(SkipCondition::Static(b)) => Ok(*b),
        Some(SkipCondition::Variable(name)) => {
            let val = vars.get_variable(name)?;
            Ok(val.trim() == "true")
        }
    }
}

/// Returns true if the step should be skipped because the `when.exists` glob has no matches.
///
/// # Errors
///
/// Returns an error if the glob pattern is syntactically invalid or a variable is undefined.
pub fn should_skip_due_to_when(
    when: Option<&WhenCondition>,
    vars: &VariableStore,
    working_dir: Option<&std::path::Path>,
) -> Result<bool> {
    let Some(cond) = when else {
        return Ok(false);
    };
    let Some(pattern_raw) = cond.exists.as_deref() else {
        return Ok(false);
    };
    let pattern = vars.resolve(pattern_raw)?;
    let pattern_abs = if let Some(root) = working_dir {
        let p = std::path::Path::new(&pattern);
        if p.is_absolute() {
            // Absolute pattern — use as-is; joining would silently drop `root`.
            pattern
        } else {
            // Escape glob metacharacters in the directory prefix so that special
            // characters in the working_dir path (e.g. `[`, `]`) are treated as
            // literals and only the pattern part is interpreted as a glob.
            let escaped_root = glob::Pattern::escape(&root.to_string_lossy());
            format!("{escaped_root}/{pattern}")
        }
    } else {
        pattern
    };
    let mut any_match = false;
    let mut had_io_error = false;
    for entry in glob::glob(&pattern_abs).map_err(|e| {
        crate::error::CruiseError::InvalidStepConfig(format!(
            "when.exists glob is invalid after variable substitution: {e}"
        ))
    })? {
        match entry {
            Ok(_) => {
                any_match = true;
                break;
            }
            Err(_) => {
                had_io_error = true;
            }
        }
    }
    if had_io_error && !any_match {
        // Some entries were unreadable (e.g. permission denied). Treat as
        // "files may exist" to avoid silently skipping a step.
        eprintln!(
            "  warning: when.exists glob '{pattern_abs}' encountered I/O errors; treating as matched"
        );
        return Ok(false);
    }
    Ok(!any_match)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variable::VariableStore;

    fn empty_vars() -> VariableStore {
        VariableStore::new(String::new())
    }

    #[test]
    fn test_should_skip_none() -> crate::error::Result<()> {
        assert!(!should_skip(None, &empty_vars())?);
        Ok(())
    }

    #[test]
    fn test_should_skip_false() -> crate::error::Result<()> {
        assert!(!should_skip(
            Some(&SkipCondition::Static(false)),
            &empty_vars()
        )?);
        Ok(())
    }

    #[test]
    fn test_should_skip_true() -> crate::error::Result<()> {
        assert!(should_skip(
            Some(&SkipCondition::Static(true)),
            &empty_vars()
        )?);
        Ok(())
    }

    #[test]
    fn test_should_skip_variable_true() -> crate::error::Result<()> {
        let mut vars = VariableStore::new(String::new());
        vars.set_prev_success(Some(true));
        assert!(should_skip(
            Some(&SkipCondition::Variable("prev.success".to_string())),
            &vars
        )?);
        Ok(())
    }

    #[test]
    fn test_should_skip_variable_false() -> crate::error::Result<()> {
        let mut vars = VariableStore::new(String::new());
        vars.set_prev_success(Some(false));
        assert!(!should_skip(
            Some(&SkipCondition::Variable("prev.success".to_string())),
            &vars
        )?);
        Ok(())
    }

    #[test]
    fn test_should_skip_variable_undefined() {
        // Undefined variable should return an error.
        let vars = VariableStore::new(String::new());
        let result = should_skip(
            Some(&SkipCondition::Variable("prev.success".to_string())),
            &vars,
        );
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod when_tests {
    use super::*;
    use crate::config::WhenCondition;
    use crate::variable::VariableStore;

    fn empty_vars() -> VariableStore {
        VariableStore::new(String::new())
    }

    // when is None → don't skip (no condition to evaluate)
    #[test]
    fn test_when_none_returns_false() {
        let result =
            should_skip_due_to_when(None, &empty_vars(), None).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result);
    }

    // when.exists is None (struct exists but no exists field) → don't skip
    #[test]
    fn test_when_exists_field_is_none_returns_false() {
        let when = WhenCondition { exists: None };
        let result = should_skip_due_to_when(Some(&when), &empty_vars(), None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result);
    }

    // when.exists glob matches an existing file → don't skip
    #[test]
    fn test_when_exists_matching_file_returns_false() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(dir.path().join("main.rs"), "fn main() {}")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let when = WhenCondition {
            exists: Some("*.rs".to_string()),
        };
        let result = should_skip_due_to_when(Some(&when), &empty_vars(), Some(dir.path()))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!result, "file exists, step should not be skipped");
    }

    // when.exists glob matches nothing → skip
    #[test]
    fn test_when_exists_no_match_returns_true() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let when = WhenCondition {
            exists: Some("*.rs".to_string()),
        };
        let result = should_skip_due_to_when(Some(&when), &empty_vars(), Some(dir.path()))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result, "no files, step should be skipped");
    }

    // variable in glob is resolved before matching
    #[test]
    fn test_when_exists_variable_in_glob_resolves() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(dir.path().join("main.go"), "package main")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let vars = VariableStore::new("go".to_string()); // {input} = "go"
        let when = WhenCondition {
            exists: Some("*.{input}".to_string()),
        };
        let result = should_skip_due_to_when(Some(&when), &vars, Some(dir.path()))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            !result,
            "glob with variable substitution should match the file"
        );
    }

    // invalid glob syntax → error
    #[test]
    fn test_when_exists_invalid_glob_returns_error() {
        let when = WhenCondition {
            exists: Some("[invalid".to_string()),
        };
        let result = should_skip_due_to_when(Some(&when), &empty_vars(), None);
        assert!(result.is_err(), "invalid glob should return an error");
    }

    // working_dir=None falls back to current directory (glob is used as-is)
    #[test]
    fn test_when_exists_no_working_dir_uses_glob_as_is() {
        let when = WhenCondition {
            exists: Some("__nonexistent_test_file_xyz__.rs".to_string()),
        };
        let result = should_skip_due_to_when(Some(&when), &empty_vars(), None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(result, "non-matching glob without working_dir should skip");
    }

    // `{{` / `}}` in the exists pattern escape to literal braces before globbing,
    // so a filename containing literal `{`/`}` can be matched.
    #[test]
    fn test_when_exists_escaped_braces_match_literal_filename() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(dir.path().join("{special}.rs"), "fn main() {}")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let when = WhenCondition {
            exists: Some("{{special}}.rs".to_string()),
        };
        let result = should_skip_due_to_when(Some(&when), &empty_vars(), Some(dir.path()))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            !result,
            "escaped literal-brace filename should match, not be skipped"
        );
    }
}
