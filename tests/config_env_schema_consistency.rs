use cruise::config_env::{APP_ENV_KEYS, EXCLUDED_KEYS, WORKFLOW_ENV_KEYS};
use std::collections::BTreeSet;

#[test]
fn all_registered_overrides_are_documented() {
    for text in [
        include_str!("../README.md"),
        include_str!("../skills/cruise-cli/SKILL.md"),
    ] {
        for name in cruise::config_env::all_override_names() {
            assert!(
                text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|word| word == name),
                "missing documentation for {name}"
            );
        }
    }
}

#[test]
fn schema_properties_are_classified_and_override_paths_exist() {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../cruise-schema.json")).unwrap();
    let properties = schema["properties"].as_object().unwrap();
    let excluded: BTreeSet<_> = EXCLUDED_KEYS.iter().map(|(key, _)| *key).collect();
    let roots: BTreeSet<_> = WORKFLOW_ENV_KEYS
        .iter()
        .map(|key| key.path.split('.').next().unwrap())
        .collect();
    for key in properties.keys().filter(|key| key.as_str() != "$schema") {
        assert!(
            roots.contains(key.as_str()) || excluded.contains(key.as_str()),
            "unclassified schema property: {key}"
        );
    }
    for key in WORKFLOW_ENV_KEYS {
        let mut current = &schema;
        for part in key.path.split('.') {
            if let Some(reference) = current.get("$ref").and_then(|v| v.as_str()) {
                current = schema
                    .pointer(reference.strip_prefix('#').unwrap())
                    .unwrap();
            }
            current = &current["properties"][part];
            assert!(!current.is_null(), "missing schema path: {}", key.path);
        }
    }
    let app = serde_json::to_value(cruise::app_config::AppConfig::default()).unwrap();
    for key in APP_ENV_KEYS {
        // AppConfig uses camelCase in its persisted JSON representation.
        let mut parts = key.path.split('_');
        let mut json_key = parts.next().unwrap().to_owned();
        for part in parts {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                json_key.extend(first.to_uppercase());
                json_key.extend(chars);
            }
        }
        assert!(
            app.get(&json_key).is_some(),
            "missing AppConfig key: {}",
            key.path
        );
    }
}
