use std::fs;

use cruise::app_config::AppConfig;
use serde_json::{Value, json};
use tempfile::TempDir;

fn config(value: Value) -> AppConfig {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("deserialize app config: {error}"))
}

#[test]
fn missing_language_defaults_to_unset_and_is_not_serialized() {
    for config in [AppConfig::default(), config(json!({}))] {
        let serialized = serde_json::to_value(&config)
            .unwrap_or_else(|error| panic!("serialize app config: {error}"));
        assert!(serialized.get("uiLanguage").is_none());
        assert!(serialized.get("ui_language").is_none());
        config
            .validate()
            .unwrap_or_else(|error| panic!("default app config is invalid: {error}"));
    }
}

#[test]
fn language_alias_deserializes_and_serializes_as_camel_case() {
    for key in ["uiLanguage", "ui_language"] {
        let input = json!({key: "zh-CN", "runAllParallelism": 3});
        let config = config(input);
        config
            .validate()
            .unwrap_or_else(|error| panic!("valid language config rejected: {error}"));
        let output = serde_json::to_value(&config)
            .unwrap_or_else(|error| panic!("serialize app config: {error}"));
        assert_eq!(output["uiLanguage"], "zh-CN");
        assert!(output.get("ui_language").is_none());
        assert_eq!(output["runAllParallelism"], 3);
    }
}

#[test]
fn supported_language_tags_and_auto_pass_validation() {
    for language in [
        "en", "zh-CN", "es", "fr", "pt-BR", "ja", "ko", "auto", "ZH_cn", "PT_br",
    ] {
        let config = config(json!({"uiLanguage": language}));
        assert!(config.validate().is_ok(), "rejected {language}");
        assert_eq!(
            serde_json::to_value(config)
                .unwrap_or_else(|error| panic!("serialize language config: {error}"))["uiLanguage"],
            language
        );
    }
}

#[test]
fn unsupported_languages_fail_validation() {
    for language in ["xx", "pt", "zh-Hans", "zh-TW", "en-US", "", "en.UTF-8"] {
        let config = config(json!({"uiLanguage": language}));
        assert!(config.validate().is_err(), "accepted {language:?}");
    }
}

#[test]
fn json_and_yaml_persist_and_reload_language_without_losing_other_settings() {
    let temp =
        TempDir::new().unwrap_or_else(|error| panic!("create config test directory: {error}"));
    for extension in ["json", "yaml"] {
        for language in ["en", "zh-CN", "es", "fr", "pt-BR", "ja", "ko", "auto"] {
            let path = temp.path().join(format!("{language}.{extension}"));
            let original = config(json!({"uiLanguage": language, "runAllParallelism": 4}));
            original
                .save_to(&path)
                .unwrap_or_else(|error| panic!("save {extension} config: {error}"));
            let loaded = AppConfig::load_from(&path)
                .unwrap_or_else(|error| panic!("load {extension} config: {error}"));
            assert_eq!(
                serde_json::to_value(&loaded)
                    .unwrap_or_else(|error| panic!("serialize loaded config: {error}")),
                serde_json::to_value(&original)
                    .unwrap_or_else(|error| panic!("serialize original config: {error}"))
            );
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {extension} config: {error}"));
            assert!(
                text.contains("uiLanguage"),
                "missing canonical key in {text}"
            );
            assert!(!text.contains("ui_language"), "alias leaked into {text}");
        }
    }
}

#[test]
fn invalid_language_cannot_be_loaded_or_overwrite_existing_config() {
    let temp =
        TempDir::new().unwrap_or_else(|error| panic!("create config test directory: {error}"));
    for (extension, invalid) in [
        ("json", "{\"uiLanguage\":\"xx\"}"),
        ("yaml", "uiLanguage: xx\n"),
    ] {
        let path = temp.path().join(format!("config.{extension}"));
        fs::write(&path, invalid)
            .unwrap_or_else(|error| panic!("write invalid {extension} config: {error}"));
        assert!(
            AppConfig::load_from(&path).is_err(),
            "accepted invalid {extension}"
        );

        let original = b"existing config must survive";
        fs::write(&path, original)
            .unwrap_or_else(|error| panic!("restore {extension} config: {error}"));
        let bad = config(json!({"uiLanguage": "xx"}));
        assert!(bad.save_to(&path).is_err(), "saved invalid {extension}");
        assert_eq!(
            fs::read(&path)
                .unwrap_or_else(|error| panic!("read preserved {extension} config: {error}")),
            original
        );
    }
}
