use std::collections::BTreeMap;
use std::sync::OnceLock;

pub mod resolve;
pub use resolve::{LocaleSettings, LocaleSource, ResolvedLocale};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Locale {
    #[default]
    En,
    ZhCn,
    Es,
    Fr,
    PtBr,
    Ja,
    Ko,
}
impl Locale {
    pub const ALL: [Self; 7] = [
        Self::En,
        Self::ZhCn,
        Self::Es,
        Self::Fr,
        Self::PtBr,
        Self::Ja,
        Self::Ko,
    ];
    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
            Self::Es => "es",
            Self::Fr => "fr",
            Self::PtBr => "pt-BR",
            Self::Ja => "ja",
            Self::Ko => "ko",
        }
    }
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::ZhCn => "简体中文",
            Self::Es => "Español",
            Self::Fr => "Français",
            Self::PtBr => "Português (Brasil)",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
        }
    }
    pub fn from_tag(value: &str) -> Option<Self> {
        let normalized = value.trim().replace('_', "-");
        Self::ALL
            .into_iter()
            .find(|l| l.tag().eq_ignore_ascii_case(&normalized))
    }
}
static PROCESS_LOCALE: OnceLock<Locale> = OnceLock::new();
pub fn init_process_locale(locale: Locale) {
    let _ = PROCESS_LOCALE.set(locale);
}
pub fn process_locale() -> Locale {
    *PROCESS_LOCALE.get().unwrap_or(&Locale::En)
}
fn raw(locale: Locale) -> &'static str {
    match locale {
        Locale::En => include_str!("../../i18n/en.toml"),
        Locale::ZhCn => include_str!("../../i18n/zh-CN.toml"),
        Locale::Es => include_str!("../../i18n/es.toml"),
        Locale::Fr => include_str!("../../i18n/fr.toml"),
        Locale::PtBr => include_str!("../../i18n/pt-BR.toml"),
        Locale::Ja => include_str!("../../i18n/ja.toml"),
        Locale::Ko => include_str!("../../i18n/ko.toml"),
    }
}
fn flatten(prefix: &str, value: &toml::Value, out: &mut BTreeMap<String, String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let name = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&name, value, out);
            }
        }
        toml::Value::String(s) => {
            out.insert(prefix.to_owned(), s.clone());
        }
        _ => {}
    }
}
fn catalog(locale: Locale) -> &'static BTreeMap<String, String> {
    static CATALOGS: [OnceLock<BTreeMap<String, String>>; 7] = [const { OnceLock::new() }; 7];
    CATALOGS[locale as usize].get_or_init(|| {
        let value: toml::Value =
            toml::from_str(raw(locale)).unwrap_or_else(|_| toml::Value::Table(Default::default()));
        let mut out = BTreeMap::new();
        flatten("", &value, &mut out);
        out
    })
}
pub fn t(locale: Locale, key: &str) -> String {
    render_key(catalog(locale), catalog(Locale::En), key)
}

fn render_key(
    primary: &BTreeMap<String, String>,
    fallback: &BTreeMap<String, String>,
    key: &str,
) -> String {
    primary
        .get(key)
        .or_else(|| fallback.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_owned())
}
pub fn tf(locale: Locale, key: &str, vars: &[(&str, &str)]) -> String {
    interpolate(&t(locale, key), vars)
}

fn interpolate(template: &str, vars: &[(&str, &str)]) -> String {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('}') else {
            break;
        };
        let name = &rest[1..end];
        if let Some((_, value)) = vars.iter().find(|(key, _)| *key == name) {
            result.push_str(value);
        } else {
            result.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}
pub fn webui_props(locale: Locale) -> &'static serde_json::Value {
    static PROPS: [OnceLock<serde_json::Value>; 7] = [const { OnceLock::new() }; 7];
    PROPS[locale as usize].get_or_init(|| {
        let value: toml::Value =
            toml::from_str(raw(locale)).unwrap_or_else(|_| toml::Value::Table(Default::default()));
        serde_json::to_value(value.get("webui")).unwrap_or(serde_json::Value::Null)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_is_single_pass_and_preserves_unknown_placeholders() {
        assert_eq!(
            interpolate("{a} {b} {missing} {", &[("a", "{b}"), ("b", "value")]),
            "{b} value {missing} {"
        );
    }

    #[test]
    fn missing_translation_returns_key_and_embedded_webui_props_are_nested() {
        assert_eq!(t(Locale::Ja, "missing.key"), "missing.key");
        assert_eq!(webui_props(Locale::Ja)["sidebar"]["sessions"], "セッション");
    }

    #[test]
    fn catalog_fallback_prefers_primary_then_english_then_key() {
        let primary = BTreeMap::from([("translated".to_string(), "訳".to_string())]);
        let english = BTreeMap::from([
            ("translated".to_string(), "translation".to_string()),
            ("fallback".to_string(), "English".to_string()),
        ]);
        assert_eq!(render_key(&primary, &english, "translated"), "訳");
        assert_eq!(render_key(&primary, &english, "fallback"), "English");
        assert_eq!(render_key(&primary, &english, "missing"), "missing");
    }
}
