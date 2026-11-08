use super::Locale;

pub const EXPECTED: &str = "en, zh-CN, es, fr, pt-BR, ja, ko or auto";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleSource {
    Env,
    Config,
    System,
    Default,
}
impl LocaleSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Config => "config",
            Self::System => "system",
            Self::Default => "default",
        }
    }
}
#[derive(Debug, Default, Clone, Copy)]
pub struct LocaleSettings<'a> {
    pub env: Option<&'a str>,
    pub config: Option<&'a str>,
    pub system: Option<&'a str>,
}
#[derive(Debug)]
pub struct ResolvedLocale {
    pub locale: Locale,
    pub source: LocaleSource,
    pub warnings: Vec<String>,
}
pub fn resolve(settings: LocaleSettings<'_>) -> ResolvedLocale {
    let mut warnings = Vec::new();
    for (value, name, source) in [
        (settings.env, "CRUISE_UI_LANGUAGE", LocaleSource::Env),
        (settings.config, "uiLanguage", LocaleSource::Config),
    ] {
        if let Some(value) = value {
            let value = value.trim();
            if value.is_empty() || value.eq_ignore_ascii_case("auto") {
                continue;
            }
            if let Some(locale) = Locale::from_tag(value) {
                return ResolvedLocale {
                    locale,
                    source,
                    warnings,
                };
            }
            warnings.push(format!(
                "warning: ignoring {name}='{value}': expected {EXPECTED}"
            ));
        }
    }
    if let Some(locale) = settings.system.and_then(from_system_locale) {
        return ResolvedLocale {
            locale,
            source: LocaleSource::System,
            warnings,
        };
    }
    ResolvedLocale {
        locale: Locale::En,
        source: LocaleSource::Default,
        warnings,
    }
}
#[must_use]
pub fn posix_locale_var() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
}
#[must_use]
pub fn from_system_locale(value: &str) -> Option<Locale> {
    let normalized = value
        .trim()
        .split(['.', '@', ':'])
        .next()?
        .replace('_', "-")
        .to_ascii_lowercase();
    let parts: Vec<_> = normalized.split('-').collect();
    match parts.first().copied()? {
        "en" => Some(Locale::En),
        "es" => Some(Locale::Es),
        "fr" => Some(Locale::Fr),
        "pt" => Some(Locale::PtBr),
        "ja" => Some(Locale::Ja),
        "ko" => Some(Locale::Ko),
        "zh" => {
            if parts
                .iter()
                .skip(1)
                .any(|p| matches!(*p, "tw" | "hk" | "mo" | "hant"))
            {
                None
            } else if parts.len() == 1
                || parts
                    .iter()
                    .skip(1)
                    .any(|p| matches!(*p, "cn" | "sg" | "hans"))
            {
                Some(Locale::ZhCn)
            } else {
                None
            }
        }
        _ => None,
    }
}
#[must_use]
pub fn parse_cookie_locale(cookie: &str) -> Option<Locale> {
    cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .filter(|(name, _)| name.trim() == "cruise_ui_language")
        .find_map(|(_, value)| Locale::from_tag(value))
}
#[cfg(windows)]
#[must_use]
pub fn system_locale_windows() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    // LOCALE_NAME_MAX_LENGTH from the Windows SDK, including the terminator.
    let mut buf = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    (len > 1).then(|| String::from_utf16_lossy(&buf[..len as usize - 1]))
}
#[cfg(not(windows))]
#[must_use]
pub fn system_locale_windows() -> Option<String> {
    None
}
pub fn system_locale() -> Option<String> {
    select_system_locale(posix_locale_var(), system_locale_windows)
}
fn select_system_locale(
    posix: Option<String>,
    windows: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match posix {
        Some(value) if from_system_locale(&value).is_some() => Some(value),
        Some(value) => windows().or(Some(value)),
        None => windows(),
    }
}
#[must_use]
pub fn resolve_current(config: Option<&str>) -> ResolvedLocale {
    let env = std::env::var("CRUISE_UI_LANGUAGE").ok();
    let system = system_locale();
    resolve(LocaleSettings {
        env: env.as_deref(),
        config,
        system: system.as_deref(),
    })
}
/// Validates that a locale setting is `auto` or a supported locale tag.
///
/// # Errors
/// Returns an error when `value` is not `auto` or a supported locale tag.
pub fn validate_language(value: &str) -> Result<String, String> {
    if value == value.trim()
        && (value.eq_ignore_ascii_case("auto") || Locale::from_tag(value).is_some())
    {
        Ok(value.to_owned())
    } else {
        Err(format!("expected {EXPECTED}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_invalid_values() {
        let result = resolve(LocaleSettings {
            env: Some("bad"),
            config: Some("ZH_cn"),
            system: Some("ja_JP.UTF-8"),
        });
        assert_eq!(result.locale, Locale::ZhCn);
        assert_eq!(result.source, LocaleSource::Config);
        assert_eq!(result.warnings.len(), 1);
        let result = resolve(LocaleSettings {
            env: Some("auto"),
            config: None,
            system: Some("ko_KR.UTF-8"),
        });
        assert_eq!(result.locale, Locale::Ko);
        assert_eq!(result.source, LocaleSource::System);
    }

    #[test]
    fn system_and_cookie_parsing() {
        assert_eq!(
            select_system_locale(Some("C".into()), || Some("ja-JP".into())).as_deref(),
            Some("ja-JP")
        );
        assert_eq!(
            select_system_locale(Some("fr_FR".into()), || Some("ja-JP".into())).as_deref(),
            Some("fr_FR")
        );
        assert_eq!(from_system_locale("pt_PT.UTF-8"), Some(Locale::PtBr));
        assert_eq!(from_system_locale("zh_Hans_CN"), Some(Locale::ZhCn));
        assert_eq!(from_system_locale("zh_Hant_TW"), None);
        assert_eq!(from_system_locale("C.UTF-8"), None);
        assert_eq!(
            parse_cookie_locale("cruise_ui_language=invalid; cruise_ui_language=ja"),
            Some(Locale::Ja)
        );
        assert_eq!(Locale::from_tag("en-US"), None);
    }

    #[test]
    fn explicit_tag_validation_is_stricter_than_system_locales() {
        for tag in [
            "en", "zh-CN", "es", "fr", "pt-BR", "ja", "ko", "auto", "ZH_cn", "PT_br",
        ] {
            assert!(validate_language(tag).is_ok(), "{tag}");
        }
        for tag in ["", "pt", "zh-Hans", "zh-TW", "en-US", "en.UTF-8", " ja "] {
            assert!(validate_language(tag).is_err(), "{tag}");
        }
        for locale in ["zh_TW", "zh_HK", "zh_MO", "zh_Hant", "C", "POSIX", ""] {
            assert_eq!(from_system_locale(locale), None, "{locale}");
        }
        for (input, expected) in [
            ("zh", Locale::ZhCn),
            ("zh_SG", Locale::ZhCn),
            ("en_US.UTF-8", Locale::En),
            ("fr_CA@latin", Locale::Fr),
            ("es_MX", Locale::Es),
            ("ja_JP", Locale::Ja),
        ] {
            assert_eq!(from_system_locale(input), Some(expected), "{input}");
        }
    }

    #[test]
    fn invalid_values_warn_and_fall_through_without_losing_warnings() {
        let result = resolve(LocaleSettings {
            env: Some("bad-env"),
            config: Some("bad-config"),
            system: Some("fr_FR.UTF-8"),
        });
        assert_eq!(
            (result.locale, result.source),
            (Locale::Fr, LocaleSource::System)
        );
        assert_eq!(result.warnings.len(), 2);
        assert!(result.warnings[0].contains("CRUISE_UI_LANGUAGE='bad-env'"));
        assert!(result.warnings[1].contains("uiLanguage='bad-config'"));
        let result = resolve(LocaleSettings {
            env: None,
            config: None,
            system: Some("unsupported"),
        });
        assert_eq!(
            (result.locale, result.source),
            (Locale::En, LocaleSource::Default)
        );
        assert_eq!(result.warnings, Vec::<String>::new());
    }

    #[test]
    fn cookie_parser_uses_first_valid_matching_name() {
        assert_eq!(
            parse_cookie_locale(
                "other=ja; cruise_ui_language=auto; cruise_ui_language=pt_br; cruise_ui_language=ko"
            ),
            Some(Locale::PtBr)
        );
        assert_eq!(
            parse_cookie_locale("cruise_ui_language=zh-TW; other=en"),
            None
        );
    }
}
