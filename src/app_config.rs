use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CruiseError, Result};

/// Application-level configuration persisted as JSON or YAML in the Cruise config directory.
///
/// This is distinct from per-workflow YAML configs (which live in `~/.config/cruise/workflows/`).
///
/// ## Behaviour
/// - Missing files -> returns [`AppConfig::default()`]; new installations use `config.json`.
/// - Invalid JSON or YAML or invalid field values -> returns a clear error (never silently clamped).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    /// Maximum number of sessions the `WebUI` and TUI execute concurrently in `run --all` mode.
    ///
    /// Must be >= 1. Defaults to `1` (preserves backward-compatible sequential behaviour).
    #[serde(alias = "run_all_parallelism")]
    pub run_all_parallelism: usize,

    /// Default model reference or ordered primary-plus-fallback list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::config::ModelSpec>,

    /// Model reference or ordered primary-plus-fallback list for the built-in plan step.
    #[serde(
        rename = "plan_model",
        alias = "planModel",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan_model: Option<crate::config::ModelSpec>,
}

fn is_yaml_config_path(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("yaml")
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            run_all_parallelism: 1,
            model: None,
            plan_model: None,
        }
    }
}

impl AppConfig {
    /// Return the canonical app config path under `$XDG_CONFIG_HOME/cruise`
    /// (defaulting to `$HOME/.config/cruise`).
    ///
    /// Existing `config.json` takes precedence over `config.yaml`; when neither exists,
    /// returns the `config.json` path.
    ///
    /// # Errors
    ///
    /// Returns an error if the config directory cannot be determined or candidate paths cannot
    /// be inspected.
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = crate::paths::config_dir()?;
        let json_path = config_dir.join("config.json");
        let yaml_path = config_dir.join("config.yaml");
        let exists = |path: &Path| {
            path.try_exists().map_err(|error| {
                CruiseError::Other(format!(
                    "failed to check config file {}: {error}",
                    path.display()
                ))
            })
        };

        if exists(&json_path)? {
            Ok(json_path)
        } else if exists(&yaml_path)? {
            Ok(yaml_path)
        } else {
            Ok(json_path)
        }
    }

    /// Load the app config using the canonical [`Self::config_path`].
    ///
    /// # Errors
    ///
    /// Returns an error if the config directory cannot be determined, the selected file cannot
    /// be read or parsed as JSON/YAML, or validation fails.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    /// Load the app config from an explicit path (useful in tests and alternative config locations).
    ///
    /// A `.yaml` extension selects YAML; all other paths retain JSON behavior. An absent file
    /// returns [`AppConfig::default()`], while a present invalid file returns an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed, or if validation fails.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(CruiseError::Other(format!(
                    "failed to read config {}: {e}",
                    path.display()
                )));
            }
        };
        let config: Self = if is_yaml_config_path(path) {
            serde_yaml::from_str(&content).map_err(|e| {
                CruiseError::Other(format!("invalid config YAML in {}: {e}", path.display()))
            })?
        } else {
            serde_json::from_str(&content).map_err(|e| {
                CruiseError::Other(format!("invalid config JSON in {}: {e}", path.display()))
            })?
        };
        config.validate()?;
        Ok(config)
    }

    /// Persist the app config to disk using the canonical [`Self::config_path`].
    ///
    /// # Errors
    ///
    /// Returns an error if the config directory cannot be determined or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        self.save_to(&path)
    }

    /// Persist the app config to an explicit path.
    ///
    /// A `.yaml` extension selects YAML; all other paths retain JSON behavior. Creates parent
    /// directories as needed and writes through a temp file then rename for atomicity.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written or if the config is invalid.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CruiseError::Other(format!(
                    "failed to create config dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        // Write to a uniquely named temp file in the same directory then rename for atomicity.
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let tmp_path = parent.join(format!(".config-{}.tmp", Uuid::new_v4().simple()));
        let content = if is_yaml_config_path(path) {
            serde_yaml::to_string(self)
                .map_err(|e| CruiseError::Other(format!("failed to serialize config YAML: {e}")))?
        } else {
            serde_json::to_string_pretty(self)
                .map_err(|e| CruiseError::Other(format!("failed to serialize config: {e}")))?
        };
        let mut tmp_created = false;
        let write_result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)?;
            tmp_created = true;
            file.write_all(content.as_bytes())?;
            file.flush()?;
            file.sync_all()?;
            fs::rename(&tmp_path, path)
        })();
        if let Err(e) = write_result {
            if tmp_created {
                let _ = fs::remove_file(&tmp_path);
            }
            return Err(CruiseError::Other(format!(
                "failed to write config to {}: {e}",
                tmp_path.display()
            )));
        }
        Ok(())
    }

    /// Validate the config, returning a clear error for any invalid field.
    ///
    /// `run_all_parallelism` must be >= 1. A value of `0` is never silently clamped.
    ///
    /// # Errors
    ///
    /// Returns [`CruiseError::Other`] if any field is invalid.
    pub fn validate(&self) -> Result<()> {
        if let Some(model) = &self.model {
            model.validate("model")?;
        }
        if let Some(model) = &self.plan_model {
            model.validate("plan_model")?;
        }
        if self.run_all_parallelism == 0 {
            return Err(CruiseError::Other(
                "run_all_parallelism must be >= 1 (got 0)".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, lock_process};
    use tempfile::TempDir;

    // -- Helpers --------------------------------------------------------------

    /// Write `content` to `<dir>/.config/cruise/config.json`, creating parent dirs.
    /// Returns the path to the written file.
    fn write_config_file(dir: &Path, content: &str) -> PathBuf {
        let config_dir = dir.join(".config").join("cruise");
        std::fs::create_dir_all(&config_dir).unwrap_or_else(|e| panic!("{e}"));
        let config_path = config_dir.join("config.json");
        std::fs::write(&config_path, content).unwrap_or_else(|e| panic!("{e}"));
        config_path
    }

    // -- AppConfig::default() --------------------------------------------------

    #[test]
    fn test_default_parallelism_is_one() {
        // Given/When: a default AppConfig
        let config = AppConfig::default();
        // Then: run_all_parallelism is 1 -- backward-compatible sequential behaviour
        assert_eq!(config.run_all_parallelism, 1);
    }

    // -- AppConfig::validate() ------------------------------------------------

    #[test]
    fn test_validate_accepts_parallelism_of_one() {
        // Given: minimum valid parallelism
        let config = AppConfig {
            run_all_parallelism: 1,
            ..AppConfig::default()
        };
        // When/Then: no error
        assert!(config.validate().is_ok(), "parallelism=1 should be valid");
    }

    #[test]
    fn test_validate_accepts_large_parallelism() {
        // Given: large valid parallelism
        let config = AppConfig {
            run_all_parallelism: 64,
            ..AppConfig::default()
        };
        // When/Then: no error
        assert!(config.validate().is_ok(), "parallelism=64 should be valid");
    }

    #[test]
    fn test_validate_rejects_zero_parallelism() {
        // Given: parallelism of 0 -- explicitly invalid
        let config = AppConfig {
            run_all_parallelism: 0,
            ..AppConfig::default()
        };
        // When: validate
        let result = config.validate();
        // Then: returns a clear error -- must NOT be silently clamped
        assert!(
            result.is_err(),
            "expected error for run_all_parallelism=0, got Ok"
        );
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(()) => unreachable!("already asserted is_err above"),
        };
        assert!(
            msg.contains("parallelism") || msg.contains('0'),
            "error message should mention the invalid value, got: {msg}"
        );
    }

    // -- AppConfig::config_path() ----------------------------------------------

    #[test]
    fn test_config_path_prefers_json_then_yaml_and_defaults_to_json() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _xdg_config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().as_os_str());
        let config_dir = tmp.path().join("cruise");
        let json_path = config_dir.join("config.json");
        let yaml_path = config_dir.join("config.yaml");

        // A new installation keeps the existing JSON default.
        assert_eq!(
            AppConfig::config_path().unwrap_or_else(|e| panic!("{e}")),
            json_path
        );

        // YAML is selected only when JSON is absent.
        std::fs::create_dir_all(&config_dir).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(&yaml_path, "runAllParallelism: 2\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            AppConfig::config_path().unwrap_or_else(|e| panic!("{e}")),
            yaml_path
        );
        assert_eq!(
            AppConfig::load()
                .unwrap_or_else(|e| panic!("{e}"))
                .run_all_parallelism,
            2
        );

        // JSON precedence is retained even if that file is invalid.
        std::fs::write(&json_path, "invalid JSON").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            AppConfig::config_path().unwrap_or_else(|e| panic!("{e}")),
            json_path
        );
        let Err(error) = AppConfig::load() else {
            panic!("expected invalid JSON config to fail");
        };
        let error = error.to_string();
        assert!(error.contains("invalid config JSON"), "{error}");
    }

    // -- AppConfig::load_from() ------------------------------------------------

    #[test]
    fn test_load_from_returns_defaults_when_file_absent() {
        // Given: a path that does not exist
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let missing = tmp.path().join("nonexistent").join("config.json");
        // When: load_from
        let config =
            AppConfig::load_from(&missing).unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: returns defaults (not an error)
        assert_eq!(
            config,
            AppConfig::default(),
            "absent file should yield defaults"
        );
    }

    #[test]
    fn test_load_from_returns_error_for_invalid_json() {
        // Given: a config file with malformed JSON
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let config_path = write_config_file(tmp.path(), "not valid json at all");
        // When: load_from
        let result = AppConfig::load_from(&config_path);
        // Then: returns an error -- not silently ignored or defaulted
        assert!(result.is_err(), "expected error for invalid JSON, got Ok");
    }

    #[test]
    fn test_load_from_defaults_missing_fields() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let config_path = write_config_file(tmp.path(), "{}");

        let config =
            AppConfig::load_from(&config_path).unwrap_or_else(|e| panic!("expected Ok, got: {e}"));

        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn test_load_from_supports_scalar_array_null_and_absent_models() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cases = [
            (
                r#"{"model":"primary"}"#,
                Some(crate::config::ModelSpec::Single("primary".to_string())),
            ),
            (
                r#"{"model":["primary","fallback"]}"#,
                Some(crate::config::ModelSpec::Multiple(vec![
                    "primary".into(),
                    "fallback".into(),
                ])),
            ),
            (r#"{"model":null,"plan_model":null}"#, None),
            ("{}", None),
        ];

        for (index, (content, expected_model)) in cases.into_iter().enumerate() {
            let path = tmp.path().join(format!("{index}.json"));
            std::fs::write(&path, content).unwrap_or_else(|e| panic!("{e}"));
            let config =
                AppConfig::load_from(&path).unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
            assert_eq!(config.model, expected_model, "input: {content}");
            assert_eq!(config.run_all_parallelism, 1, "input: {content}");
        }
    }

    #[test]
    fn test_load_from_supports_canonical_plan_model_and_camel_case_alias() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        for (index, (key, expected)) in [
            (
                "plan_model",
                crate::config::ModelSpec::Multiple(vec!["primary".into(), "fallback".into()]),
            ),
            (
                "planModel",
                crate::config::ModelSpec::Single("legacy".to_string()),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let path = tmp.path().join(format!("{index}.json"));
            std::fs::write(
                &path,
                format!(
                    r#"{{"{key}":{}}}"#,
                    serde_json::to_string(&expected).unwrap_or_else(|e| panic!("{e}"))
                ),
            )
            .unwrap_or_else(|e| panic!("{e}"));
            let config =
                AppConfig::load_from(&path).unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
            assert_eq!(config.plan_model, Some(expected));
        }
    }

    #[test]
    fn test_load_from_rejects_invalid_model_types() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        for (index, content) in [r#"{"model":42}"#, r#"{"plan_model":["valid",42]}"#]
            .into_iter()
            .enumerate()
        {
            let path = tmp.path().join(format!("{index}.json"));
            std::fs::write(&path, content).unwrap_or_else(|e| panic!("{e}"));
            assert!(
                AppConfig::load_from(&path).is_err(),
                "invalid model type should be rejected: {content}"
            );
        }
    }

    #[test]
    fn test_load_from_returns_error_for_zero_parallelism_in_file() {
        // Given: a config file with run_all_parallelism = 0 -- explicitly invalid
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let config_path = write_config_file(tmp.path(), r#"{"run_all_parallelism": 0}"#);
        // When: load_from
        let result = AppConfig::load_from(&config_path);
        // Then: returns a clear error -- must NOT silently clamp to 1
        assert!(
            result.is_err(),
            "expected error for run_all_parallelism=0, got Ok"
        );
    }

    #[test]
    fn test_load_from_parses_valid_parallelism() {
        // Given: a valid config file with run_all_parallelism = 4
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let config_path = write_config_file(tmp.path(), r#"{"run_all_parallelism": 4}"#);
        // When: load_from
        let config =
            AppConfig::load_from(&config_path).unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        // Then: parallelism matches the file
        assert_eq!(config.run_all_parallelism, 4);
    }

    // -- AppConfig::save_to() + load_from() round-trip ------------------------

    #[test]
    fn test_save_to_then_load_from_round_trips_models_and_parallelism() {
        // Given: a non-default AppConfig
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("config.json");
        let original = AppConfig {
            run_all_parallelism: 8,
            model: Some(crate::config::ModelSpec::Multiple(vec![
                "primary".into(),
                "fallback".into(),
            ])),
            plan_model: Some(crate::config::ModelSpec::Multiple(vec![
                "plan-primary".into(),
                "plan-fallback".into(),
            ])),
        };
        // When: save then load
        original
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save failed: {e}"));
        let loaded = AppConfig::load_from(&path).unwrap_or_else(|e| panic!("load failed: {e}"));
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|e| panic!("read failed: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse saved config failed: {e}"));
        assert!(saved.get("plan_model").is_some());
        assert!(saved.get("planModel").is_none());
        assert_eq!(loaded, original);

        let mut updated = loaded;
        updated.run_all_parallelism = 3;
        updated
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save updated config failed: {e}"));
        let updated = AppConfig::load_from(&path)
            .unwrap_or_else(|e| panic!("reload updated config failed: {e}"));
        assert_eq!(updated.run_all_parallelism, 3);
        assert_eq!(updated.model, original.model);
        assert_eq!(updated.plan_model, original.plan_model);
    }

    #[test]
    fn test_yaml_save_and_load_round_trip_model_shapes_and_parallelism() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cases = [
            (
                "runAllParallelism: 8\nmodel: primary\nplan_model:\n  - plan-primary\n  - plan-fallback\n",
                AppConfig {
                    run_all_parallelism: 8,
                    model: Some(crate::config::ModelSpec::Single("primary".to_string())),
                    plan_model: Some(crate::config::ModelSpec::Multiple(vec![
                        "plan-primary".into(),
                        "plan-fallback".into(),
                    ])),
                },
            ),
            (
                "run_all_parallelism: 4\nmodel:\n  - primary\n  - fallback\nplanModel: plan-primary\n",
                AppConfig {
                    run_all_parallelism: 4,
                    model: Some(crate::config::ModelSpec::Multiple(vec![
                        "primary".into(),
                        "fallback".into(),
                    ])),
                    plan_model: Some(crate::config::ModelSpec::Single("plan-primary".to_string())),
                },
            ),
        ];

        for (index, (content, expected)) in cases.into_iter().enumerate() {
            let path = tmp.path().join(format!("{index}.yaml"));
            std::fs::write(&path, content).unwrap_or_else(|e| panic!("{e}"));
            let loaded = AppConfig::load_from(&path).unwrap_or_else(|e| panic!("load failed: {e}"));
            assert_eq!(loaded, expected);

            loaded
                .save_to(&path)
                .unwrap_or_else(|e| panic!("save failed: {e}"));
            assert_eq!(
                AppConfig::load_from(&path).unwrap_or_else(|e| panic!("reload failed: {e}")),
                expected
            );
        }
    }

    #[test]
    fn test_yaml_load_reports_invalid_yaml_and_path() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "runAllParallelism: [").unwrap_or_else(|e| panic!("{e}"));

        let Err(error) = AppConfig::load_from(&path) else {
            panic!("expected invalid YAML config to fail");
        };
        let error = error.to_string();
        assert!(
            error.contains(&format!("invalid config YAML in {}", path.display())),
            "{error}"
        );
    }

    #[test]
    fn test_save_to_creates_parent_directories() {
        // Given: a deeply nested path whose parents do not yet exist
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("a").join("b").join("c").join("config.json");
        let config = AppConfig {
            run_all_parallelism: 2,
            ..AppConfig::default()
        };
        // When: save_to
        config
            .save_to(&path)
            .unwrap_or_else(|e| panic!("save failed: {e}"));
        // Then: the file now exists
        assert!(path.exists(), "config file should have been created");
    }

    #[test]
    fn test_save_to_overwrites_existing_file() {
        // Given: a file with one value already written
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("config.json");
        AppConfig {
            run_all_parallelism: 3,
            ..AppConfig::default()
        }
        .save_to(&path)
        .unwrap_or_else(|e| panic!("{e}"));
        // When: save a different value
        AppConfig {
            run_all_parallelism: 7,
            ..AppConfig::default()
        }
        .save_to(&path)
        .unwrap_or_else(|e| panic!("{e}"));
        // Then: the file contains the new value
        let loaded = AppConfig::load_from(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(loaded.run_all_parallelism, 7);
    }

    #[test]
    fn test_save_to_does_not_persist_invalid_config() {
        // Given: a config that fails validation (parallelism = 0)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let path = tmp.path().join("config.json");
        let invalid = AppConfig {
            run_all_parallelism: 0,
            ..AppConfig::default()
        };
        let result = invalid.save_to(&path);
        // Then: returns an error; the file should not have been created (or is unusable)
        assert!(
            result.is_err(),
            "save_to should reject invalid config, got Ok"
        );
    }
}
