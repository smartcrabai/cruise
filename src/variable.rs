use std::collections::HashMap;

use crate::error::{CruiseError, Result};

/// Holds all runtime variables for a workflow execution.
#[derive(Debug, Default, Clone)]
pub struct VariableStore {
    /// Initial input from the CLI argument or stdin.
    input: String,

    /// LLM output from the previous step.
    prev_output: Option<String>,

    /// User text input from the previous option step.
    prev_input: Option<String>,

    /// Stderr captured from the previous command step.
    prev_stderr: Option<String>,

    /// Exit status of the previous command step.
    prev_success: Option<bool>,

    /// Named variables (e.g. plan file path).
    named: HashMap<String, NamedVariable>,
}

/// A named variable value.
#[derive(Debug, Clone)]
pub enum NamedVariable {
    /// A file path - resolves to the path string itself (not the file contents).
    FilePath(std::path::PathBuf),
    /// A plain string value.
    Value(String),
}

impl VariableStore {
    #[must_use]
    pub fn new(input: String) -> Self {
        Self {
            input,
            ..Default::default()
        }
    }

    /// Register a named variable backed by a file path.
    pub fn set_named_file(&mut self, name: &str, path: std::path::PathBuf) {
        self.named
            .insert(name.to_string(), NamedVariable::FilePath(path));
    }

    /// Register a named variable with a plain string value.
    pub fn set_named_value(&mut self, name: &str, value: String) {
        self.named
            .insert(name.to_string(), NamedVariable::Value(value));
    }

    pub fn set_prev_output(&mut self, output: Option<String>) {
        self.prev_output = output;
    }

    pub fn set_prev_input(&mut self, input: Option<String>) {
        self.prev_input = input;
    }

    pub fn set_prev_stderr(&mut self, stderr: Option<String>) {
        self.prev_stderr = stderr;
    }

    pub fn set_prev_success(&mut self, success: Option<bool>) {
        self.prev_success = success;
    }

    pub fn set_input(&mut self, input: String) {
        self.input = input;
    }

    #[must_use]
    pub fn input_is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Resolve all `{variable_name}` placeholders in `template`.
    ///
    /// # Errors
    ///
    /// Returns an error if the template references an undefined variable.
    pub fn resolve(&self, template: &str) -> Result<String> {
        let mut result = String::new();
        let mut chars = template.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '{' {
                // Collect the variable name up to the closing brace.
                let mut var_name = String::new();
                let mut closed = false;

                for inner_ch in chars.by_ref() {
                    if inner_ch == '}' {
                        closed = true;
                        break;
                    }
                    var_name.push(inner_ch);
                }

                if closed {
                    let value = self.get_variable(&var_name)?;
                    result.push_str(&value);
                } else {
                    // No closing brace -- emit literally.
                    result.push('{');
                    result.push_str(&var_name);
                }
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }

    /// Look up a variable by name and return its value.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` does not correspond to a defined variable.
    pub fn get_variable(&self, name: &str) -> Result<String> {
        match name {
            "input" => Ok(self.input.clone()),
            "prev.output" => self
                .prev_output
                .clone()
                .ok_or_else(|| CruiseError::UndefinedVariable("prev.output".to_string())),
            "prev.input" => self
                .prev_input
                .clone()
                .ok_or_else(|| CruiseError::UndefinedVariable("prev.input".to_string())),
            "prev.stderr" => self
                .prev_stderr
                .clone()
                .ok_or_else(|| CruiseError::UndefinedVariable("prev.stderr".to_string())),
            "prev.success" => self
                .prev_success
                .map(|b| b.to_string())
                .ok_or_else(|| CruiseError::UndefinedVariable("prev.success".to_string())),
            other => match self.named.get(other) {
                Some(NamedVariable::FilePath(path)) => Ok(path.to_string_lossy().to_string()),
                Some(NamedVariable::Value(val)) => Ok(val.clone()),
                None => Err(CruiseError::UndefinedVariable(other.to_string())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_resolve_input() {
        let store = VariableStore::new("hello world".to_string());
        assert_eq!(
            store
                .resolve("Input: {input}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "Input: hello world"
        );
    }

    #[test]
    fn test_resolve_prev_output() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_output(Some("LLM response".to_string()));
        assert_eq!(
            store
                .resolve("Prev: {prev.output}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "Prev: LLM response"
        );
    }

    #[test]
    fn test_resolve_prev_input() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_input(Some("user text".to_string()));
        assert_eq!(
            store
                .resolve("User said: {prev.input}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "User said: user text"
        );
    }

    #[test]
    fn test_resolve_prev_stderr() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_stderr(Some("error message".to_string()));
        assert_eq!(
            store
                .resolve("Error: {prev.stderr}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "Error: error message"
        );
    }

    #[test]
    fn test_resolve_prev_success() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_success(Some(true));
        assert_eq!(
            store
                .resolve("Success: {prev.success}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "Success: true"
        );
    }

    #[test]
    fn test_resolve_named_file() {
        let file = NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = file.path().to_path_buf();
        let path_str = path.to_string_lossy().to_string();

        let mut store = VariableStore::new("input".to_string());
        store.set_named_file("plan", path);
        let result = store
            .resolve("Plan: {plan}")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result, format!("Plan: {path_str}"));
    }

    #[test]
    fn test_resolve_undefined_variable() {
        let store = VariableStore::new("input".to_string());
        let err = store
            .resolve("Value: {undefined}")
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        matches!(err, crate::error::CruiseError::UndefinedVariable(name) if name == "undefined");
    }

    #[test]
    fn test_resolve_undefined_prev_output() {
        let store = VariableStore::new("input".to_string());
        let err = store
            .resolve("{prev.output}")
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        matches!(err, crate::error::CruiseError::UndefinedVariable(name) if name == "prev.output");
    }

    #[test]
    fn test_resolve_multiple_variables() {
        let mut store = VariableStore::new("hello".to_string());
        store.set_prev_output(Some("world".to_string()));
        assert_eq!(
            store
                .resolve("{input} {prev.output}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "hello world"
        );
    }

    #[test]
    fn test_resolve_no_variables() {
        let store = VariableStore::new("input".to_string());
        assert_eq!(
            store
                .resolve("No variables here")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "No variables here"
        );
    }

    #[test]
    fn test_resolve_unclosed_brace() {
        let store = VariableStore::new("input".to_string());
        // No closing brace -- emit literally.
        assert_eq!(
            store
                .resolve("Hello {unclosed")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "Hello {unclosed"
        );
    }

    #[test]
    fn test_set_named_value_resolves() {
        // Given: a store with a named string value set via set_named_value
        let mut store = VariableStore::new("input".to_string());
        store.set_named_value("greeting", "Hello".to_string());
        // When: resolved
        let result = store
            .resolve("Say: {greeting}")
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: variable is substituted correctly
        assert_eq!(result, "Say: Hello");
    }

    #[test]
    fn test_resolve_pr_url() {
        // Given: pr.url is set as a named value
        let mut store = VariableStore::new("input".to_string());
        store.set_named_value(
            "pr.url",
            "https://github.com/owner/repo/pull/42".to_string(),
        );
        // When: resolved in a template
        let result = store
            .resolve("PR: {pr.url}")
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: URL is substituted
        assert_eq!(result, "PR: https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn test_resolve_pr_number() {
        // Given: pr.number is set as a named value
        let mut store = VariableStore::new("input".to_string());
        store.set_named_value("pr.number", "42".to_string());
        // When: resolved in a command template
        let result = store
            .resolve("gh pr edit {pr.number} --add-label foo")
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: PR number is substituted
        assert_eq!(result, "gh pr edit 42 --add-label foo");
    }

    #[test]
    fn test_set_named_value_overrides_existing() {
        // Given: the same named value is set twice
        let mut store = VariableStore::new("input".to_string());
        store.set_named_value("pr.number", "10".to_string());
        store.set_named_value("pr.number", "42".to_string());
        // When: resolved
        let result = store
            .resolve("{pr.number}")
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: latest value wins
        assert_eq!(result, "42");
    }

    #[test]
    fn test_resolve_double_brace() {
        // Given: template "{{input}}" -- outer `{` opens a var whose name is "{input"
        let store = VariableStore::new("hello".to_string());
        // The parser sees `{` -> starts collecting; collects `{input` until `}` is hit.
        // Then calls get_variable("{input") which is undefined.
        let err = store
            .resolve("{{input}}")
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(
            matches!(err, crate::error::CruiseError::UndefinedVariable(ref n) if n == "{input"),
            "expected UndefinedVariable(\"{{input\"), got: {err:?}"
        );
    }

    #[test]
    fn test_resolve_empty_var_name() {
        // Given: template "{}" -- empty variable name
        let store = VariableStore::new("hello".to_string());
        // get_variable("") falls through to named lookup, nothing registered -> UndefinedVariable
        let err = store
            .resolve("{}")
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(
            matches!(err, crate::error::CruiseError::UndefinedVariable(ref n) if n.is_empty()),
            "expected UndefinedVariable(\"\"), got: {err:?}"
        );
    }

    #[test]
    fn test_resolve_trailing_open_brace() {
        // Given: template "trailing {" -- no closing brace
        let store = VariableStore::new("hello".to_string());
        // Parser hits `{`, collects until end-of-string, closed=false -> emits literally.
        let result = store
            .resolve("trailing {")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result, "trailing {");
    }

    #[test]
    fn test_resolve_nested_dot_path() {
        // Given: store with "foo.bar.baz" registered as a named value
        let mut store = VariableStore::new("input".to_string());
        store.set_named_value("foo.bar.baz", "deep".to_string());
        // When: resolved
        let result = store
            .resolve("{foo.bar.baz}")
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: resolves correctly
        assert_eq!(result, "deep");

        // And: an unregistered dotted name returns UndefinedVariable
        let store2 = VariableStore::new("input".to_string());
        let err = store2
            .resolve("{foo.bar.baz}")
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(
            matches!(err, crate::error::CruiseError::UndefinedVariable(ref n) if n == "foo.bar.baz"),
            "expected UndefinedVariable(\"foo.bar.baz\"), got: {err:?}"
        );
    }

    #[test]
    fn test_set_named_value_both_pr_vars() {
        // Given: both pr.url and pr.number are set
        let mut store = VariableStore::new("input".to_string());
        store.set_named_value(
            "pr.url",
            "https://github.com/owner/repo/pull/42".to_string(),
        );
        store.set_named_value("pr.number", "42".to_string());
        // When: template uses both placeholders
        let result = store
            .resolve("echo 'PR #{pr.number} created: {pr.url}'")
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Then: both are substituted
        assert_eq!(
            result,
            "echo 'PR #42 created: https://github.com/owner/repo/pull/42'"
        );
    }
}
