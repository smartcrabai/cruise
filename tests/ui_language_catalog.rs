use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

const TAGS: [&str; 7] = ["en", "zh-CN", "es", "fr", "pt-BR", "ja", "ko"];

fn catalog(tag: &str) -> BTreeMap<String, String> {
    fn flatten(prefix: &str, value: &toml::Value, out: &mut BTreeMap<String, String>) {
        match value {
            toml::Value::Table(table) => {
                for (key, value) in table {
                    let name = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    flatten(&name, value, out);
                }
            }
            toml::Value::String(value) => {
                out.insert(prefix.to_owned(), value.clone());
            }
            _ => panic!("catalog entry {prefix} must be a string"),
        }
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("i18n")
        .join(format!("{tag}.toml"));
    let contents = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let parsed: toml::Value = toml::from_str(&contents)
        .unwrap_or_else(|error| panic!("invalid TOML catalog {}: {error}", path.display()));
    let mut entries = BTreeMap::new();
    flatten("", &parsed, &mut entries);
    assert!(!entries.is_empty(), "empty catalog: {tag}");
    entries
}

fn placeholders(value: &str) -> BTreeSet<&str> {
    value
        .match_indices('{')
        .filter_map(|(start, _)| {
            let rest = &value[start + 1..];
            let end = rest.find('}')?;
            let name = &rest[..end];
            (!name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
                .then_some(name)
        })
        .collect()
}

#[test]
fn translations_have_exactly_the_english_keys() {
    let en = catalog("en");
    let expected: BTreeSet<_> = en.keys().collect();
    for tag in TAGS.into_iter().skip(1) {
        let translated = catalog(tag);
        let actual: BTreeSet<_> = translated.keys().collect();
        assert_eq!(actual, expected, "catalog key mismatch for {tag}");
    }
}

#[test]
fn translations_preserve_placeholders() {
    let en = catalog("en");
    for tag in TAGS.into_iter().skip(1) {
        let translated = catalog(tag);
        for (key, english) in &en {
            let value = translated
                .get(key)
                .unwrap_or_else(|| panic!("{tag} missing {key}"));
            assert_eq!(placeholders(value), placeholders(english), "{tag}: {key}");
        }
    }
}

#[test]
fn catalog_values_are_nonempty_and_trimmed() {
    for tag in TAGS {
        for (key, value) in catalog(tag) {
            assert!(!value.is_empty() && value == value.trim(), "{tag}: {key}");
        }
    }
}
