use crate::config::SkipCondition;
use crate::error::Result;
use crate::variable::VariableStore;

/// Returns true if the step should be skipped.
pub fn should_skip(skip: &Option<SkipCondition>, vars: &VariableStore) -> Result<bool> {
    match skip {
        None => Ok(false),
        Some(SkipCondition::Static(b)) => Ok(*b),
        Some(SkipCondition::Variable(name)) => {
            let val = vars.get_variable(name)?;
            Ok(val.trim() == "true")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variable::VariableStore;

    fn empty_vars() -> VariableStore {
        VariableStore::new(String::new())
    }

    #[test]
    fn test_should_skip_none() {
        assert!(!should_skip(&None, &empty_vars()).unwrap());
    }

    #[test]
    fn test_should_skip_false() {
        assert!(!should_skip(&Some(SkipCondition::Static(false)), &empty_vars()).unwrap());
    }

    #[test]
    fn test_should_skip_true() {
        assert!(should_skip(&Some(SkipCondition::Static(true)), &empty_vars()).unwrap());
    }

    #[test]
    fn test_should_skip_variable_true() {
        let mut vars = VariableStore::new(String::new());
        vars.set_prev_success(Some(true));
        assert!(
            should_skip(
                &Some(SkipCondition::Variable("prev.success".to_string())),
                &vars
            )
            .unwrap()
        );
    }

    #[test]
    fn test_should_skip_variable_false() {
        let mut vars = VariableStore::new(String::new());
        vars.set_prev_success(Some(false));
        assert!(
            !should_skip(
                &Some(SkipCondition::Variable("prev.success".to_string())),
                &vars
            )
            .unwrap()
        );
    }

    #[test]
    fn test_should_skip_variable_undefined() {
        // Undefined variable should return an error.
        let vars = VariableStore::new(String::new());
        let result = should_skip(
            &Some(SkipCondition::Variable("prev.success".to_string())),
            &vars,
        );
        assert!(result.is_err());
    }
}
