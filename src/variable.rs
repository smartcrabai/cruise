use std::collections::HashMap;
use std::path::Path;

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

    /// Named variables defined via the `output` field.
    named: HashMap<String, NamedVariable>,
}

/// A named variable value.
#[derive(Debug, Clone)]
pub enum NamedVariable {
    /// An inline string value.
    Value(String),
    /// A file path whose contents are read on demand.
    FilePath(std::path::PathBuf),
}

impl VariableStore {
    pub fn new(input: String) -> Self {
        Self {
            input,
            ..Default::default()
        }
    }

    /// Register a named variable with an inline string value.
    pub fn set_named_value(&mut self, name: &str, value: String) {
        self.named
            .insert(name.to_string(), NamedVariable::Value(value));
    }

    /// Register a named variable backed by a file path.
    pub fn set_named_file(&mut self, name: &str, path: std::path::PathBuf) {
        self.named
            .insert(name.to_string(), NamedVariable::FilePath(path));
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

    /// Resolve all `{variable_name}` placeholders in `template`.
    /// Returns an error for any undefined variable.
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

                if !closed {
                    // No closing brace — emit literally.
                    result.push('{');
                    result.push_str(&var_name);
                } else {
                    let value = self.get_variable(&var_name)?;
                    result.push_str(&value);
                }
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }

    /// Look up a variable by name and return its value.
    fn get_variable(&self, name: &str) -> Result<String> {
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
                Some(NamedVariable::Value(v)) => Ok(v.clone()),
                Some(NamedVariable::FilePath(path)) => self.read_file_variable(other, path),
                None => Err(CruiseError::UndefinedVariable(other.to_string())),
            },
        }
    }

    /// Read the contents of a file-backed variable.
    fn read_file_variable(&self, name: &str, path: &Path) -> Result<String> {
        std::fs::read_to_string(path)
            .map_err(|e| CruiseError::VariableFileReadError(name.to_string(), e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_resolve_input() {
        let store = VariableStore::new("hello world".to_string());
        assert_eq!(
            store.resolve("Input: {input}").unwrap(),
            "Input: hello world"
        );
    }

    #[test]
    fn test_resolve_prev_output() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_output(Some("LLM response".to_string()));
        assert_eq!(
            store.resolve("Prev: {prev.output}").unwrap(),
            "Prev: LLM response"
        );
    }

    #[test]
    fn test_resolve_prev_input() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_input(Some("user text".to_string()));
        assert_eq!(
            store.resolve("User said: {prev.input}").unwrap(),
            "User said: user text"
        );
    }

    #[test]
    fn test_resolve_prev_stderr() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_stderr(Some("error message".to_string()));
        assert_eq!(
            store.resolve("Error: {prev.stderr}").unwrap(),
            "Error: error message"
        );
    }

    #[test]
    fn test_resolve_prev_success() {
        let mut store = VariableStore::new("input".to_string());
        store.set_prev_success(Some(true));
        assert_eq!(
            store.resolve("Success: {prev.success}").unwrap(),
            "Success: true"
        );
    }

    #[test]
    fn test_resolve_named_value() {
        let mut store = VariableStore::new("input".to_string());
        store.set_named_value("plan", "Step 1: do something".to_string());
        assert_eq!(
            store.resolve("Plan: {plan}").unwrap(),
            "Plan: Step 1: do something"
        );
    }

    #[test]
    fn test_resolve_named_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "File content").unwrap();
        let path = file.path().to_path_buf();

        let mut store = VariableStore::new("input".to_string());
        store.set_named_file("plan", path);
        let result = store.resolve("Plan: {plan}").unwrap();
        assert!(result.contains("File content"));
    }

    #[test]
    fn test_resolve_undefined_variable() {
        let store = VariableStore::new("input".to_string());
        let err = store.resolve("Value: {undefined}").unwrap_err();
        matches!(err, crate::error::CruiseError::UndefinedVariable(name) if name == "undefined");
    }

    #[test]
    fn test_resolve_undefined_prev_output() {
        let store = VariableStore::new("input".to_string());
        let err = store.resolve("{prev.output}").unwrap_err();
        matches!(err, crate::error::CruiseError::UndefinedVariable(name) if name == "prev.output");
    }

    #[test]
    fn test_resolve_multiple_variables() {
        let mut store = VariableStore::new("hello".to_string());
        store.set_prev_output(Some("world".to_string()));
        assert_eq!(
            store.resolve("{input} {prev.output}").unwrap(),
            "hello world"
        );
    }

    #[test]
    fn test_resolve_no_variables() {
        let store = VariableStore::new("input".to_string());
        assert_eq!(
            store.resolve("No variables here").unwrap(),
            "No variables here"
        );
    }

    #[test]
    fn test_resolve_unclosed_brace() {
        let store = VariableStore::new("input".to_string());
        // No closing brace — emit literally.
        assert_eq!(store.resolve("Hello {unclosed").unwrap(), "Hello {unclosed");
    }
}
