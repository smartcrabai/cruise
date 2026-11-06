use cruise::app_config::AppConfig;
use std::fs;

#[test]
fn prevent_sleep_defaults_to_false_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "run_all_parallelism: 1\n").unwrap();
    let config = AppConfig::load_from(&path).unwrap();
    let value = serde_yaml::to_value(config).unwrap();
    assert_eq!(value["prevent_sleep"], serde_yaml::Value::Bool(false));
}

#[test]
fn prevent_sleep_yaml_round_trip_uses_snake_case() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "prevent_sleep: true\n").unwrap();
    let config = AppConfig::load_from(&path).unwrap();
    config.save_to(&path).unwrap();
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("prevent_sleep: true"), "{contents}");
    assert!(!contents.contains("preventSleep"), "{contents}");
    let reloaded = AppConfig::load_from(&path).unwrap();
    assert_eq!(
        serde_yaml::to_value(reloaded).unwrap()["prevent_sleep"],
        serde_yaml::Value::Bool(true)
    );
}

#[test]
fn prevent_sleep_json_camel_case_alias_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    fs::write(&path, r#"{"preventSleep": true}"#).unwrap();
    let config = AppConfig::load_from(&path).unwrap();
    assert_eq!(serde_json::to_value(config).unwrap()["prevent_sleep"], true);
}

#[test]
fn prevent_sleep_rejects_non_boolean_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "prevent_sleep: \"yes\"\n").unwrap();
    let error = AppConfig::load_from(&path).unwrap_err().to_string();
    assert!(error.contains("invalid config"), "{error}");
}
