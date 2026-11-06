use cruise::app_config::AppConfig;
use std::fs;

#[test]
fn prevent_sleep_defaults_to_false_when_missing() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.yaml");
    fs::write(&path, "run_all_parallelism: 1\n")?;
    let config = AppConfig::load_from(&path)?;
    let value = serde_yaml::to_value(config)?;
    assert_eq!(value["prevent_sleep"], serde_yaml::Value::Bool(false));
    Ok(())
}

#[test]
fn prevent_sleep_yaml_round_trip_uses_snake_case() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.yaml");
    fs::write(&path, "prevent_sleep: true\n")?;
    let config = AppConfig::load_from(&path)?;
    config.save_to(&path)?;
    let contents = fs::read_to_string(&path)?;
    assert!(contents.contains("prevent_sleep: true"), "{contents}");
    assert!(!contents.contains("preventSleep"), "{contents}");
    let reloaded = AppConfig::load_from(&path)?;
    assert_eq!(
        serde_yaml::to_value(reloaded)?["prevent_sleep"],
        serde_yaml::Value::Bool(true)
    );
    Ok(())
}

#[test]
fn prevent_sleep_json_camel_case_alias_is_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.json");
    fs::write(&path, r#"{"preventSleep": true}"#)?;
    let config = AppConfig::load_from(&path)?;
    assert_eq!(serde_json::to_value(config)?["prevent_sleep"], true);
    Ok(())
}

#[test]
fn prevent_sleep_rejects_non_boolean_values() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.yaml");
    fs::write(&path, "prevent_sleep: \"yes\"\n")?;
    let error = AppConfig::load_from(&path)
        .err()
        .ok_or("invalid config unexpectedly loaded")?
        .to_string();
    assert!(error.contains("invalid config"), "{error}");
    Ok(())
}
