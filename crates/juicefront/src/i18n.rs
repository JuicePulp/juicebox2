use std::{collections::HashMap, sync::LazyLock};

#[derive(Clone, Copy)]
pub struct Locale {
    pub code: &'static str,
    pub name: &'static str,
    /// Empty for the default locale.
    prefix: &'static str,
    bundle: &'static str,
}

pub const LOCALES: &[Locale] = &[
    Locale {
        code: "en",
        name: "English",
        prefix: "",
        bundle: include_str!("../i18n/en.json"),
    },
    Locale {
        code: "fr",
        name: "Français",
        prefix: "/fr",
        bundle: include_str!("../i18n/fr.json"),
    },
    Locale {
        code: "ru",
        name: "Русский",
        prefix: "/ru",
        bundle: include_str!("../i18n/ru.json"),
    },
    Locale {
        code: "es",
        name: "Español",
        prefix: "/es",
        bundle: include_str!("../i18n/es.json"),
    },
    Locale {
        code: "ja",
        name: "日本語",
        prefix: "/ja",
        bundle: include_str!("../i18n/ja.json"),
    },
];

/// Used when no prefix matches and for missing translations.
pub const DEFAULT_LOCALE: &str = "en";

fn find(code: &str) -> Option<&'static Locale> {
    LOCALES.iter().find(|locale| locale.code == code)
}

pub fn is_valid_locale(code: &str) -> bool {
    find(code).is_some()
}

pub fn locale_prefix(locale: &str) -> &'static str {
    find(locale).map_or("", |locale| locale.prefix)
}

pub fn lp(locale: &str, path: &str) -> String {
    format!("{}{path}", locale_prefix(locale))
}

/// Unknown prefixes pass through unchanged as English.
pub fn split_locale(path: &str) -> (&'static str, String) {
    for locale in LOCALES.iter().filter(|locale| !locale.prefix.is_empty()) {
        if path == locale.prefix {
            return (locale.code, "/".to_owned());
        }
        if let Some(rest) = path
            .strip_prefix(locale.prefix)
            .and_then(|p| p.strip_prefix('/'))
        {
            return (locale.code, format!("/{rest}"));
        }
    }
    (DEFAULT_LOCALE, path.to_owned())
}

struct Bundles {
    maps: HashMap<String, HashMap<String, String>>,
}

static BUNDLES: LazyLock<Bundles> = LazyLock::new(|| {
    let mut maps = HashMap::new();
    for locale in LOCALES {
        let parsed: serde_json::Value =
            serde_json::from_str(locale.bundle).expect("i18n JSON must parse");
        let dict = parsed
            .get("dict")
            .and_then(|dict| dict.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|text| (key.clone(), text.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        maps.insert(locale.code.to_owned(), dict);
    }
    Bundles { maps }
});

/// Falls back to English; a missing key is returned verbatim.
pub fn t(locale: &str, key: &str) -> String {
    lookup(locale, key).unwrap_or_else(|| key.to_owned())
}

pub fn tv(locale: &str, key: &str, name: &str, value: &str) -> String {
    interpolate(&t(locale, key), &[(name, value)])
}

fn lookup(locale: &str, key: &str) -> Option<String> {
    BUNDLES
        .maps
        .get(locale)
        .and_then(|map| map.get(key))
        .or_else(|| BUNDLES.maps.get(DEFAULT_LOCALE)?.get(key))
        .cloned()
}

pub fn interpolate(template: &str, params: &[(&str, &str)]) -> String {
    let mut out = template.to_owned();
    for (name, value) in params {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

pub fn embedded_json(locale: &str) -> String {
    let locale = if is_valid_locale(locale) {
        locale
    } else {
        DEFAULT_LOCALE
    };
    let empty = HashMap::new();
    let dict = BUNDLES.maps.get(locale).unwrap_or(&empty);
    serde_json::json!({ "locale": locale, "dict": dict }).to_string()
}

/// Encodes all but ASCII alphanumerics and `-_.!~*'()` as UTF-8 bytes.
pub fn pct_encode(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_resolve_and_prefix() {
        assert_eq!(split_locale("/fr/files"), ("fr", "/files".to_owned()));
        assert_eq!(split_locale("/files"), ("en", "/files".to_owned()));
        assert_eq!(split_locale("/ru"), ("ru", "/".to_owned()));
        assert!(is_valid_locale("ru"));
        assert!(!is_valid_locale("xx"));
        assert_eq!(locale_prefix("en"), "");
        assert_eq!(locale_prefix("fr"), "/fr");
        assert_eq!(locale_prefix("ja"), "/ja");
        assert_eq!(lp("fr", "/files"), "/fr/files");
        assert_eq!(lp("en", "/files"), "/files");
    }

    #[test]
    fn translations_fall_back_to_english() {
        assert_eq!(t("xx", "site.title"), t("en", "site.title"));
        assert_eq!(t("en", "no.such.key"), "no.such.key");
    }

    #[test]
    fn all_locales_cover_english_keys() {
        let en_keys: Vec<String> = BUNDLES.maps["en"].keys().cloned().collect();
        assert!(!en_keys.is_empty());
        for locale in LOCALES.iter().skip(1) {
            let map = &BUNDLES.maps[locale.code];
            let missing: Vec<&String> = en_keys
                .iter()
                .filter(|key| !map.contains_key(*key))
                .collect();
            assert!(
                missing.is_empty(),
                "{} missing keys: {missing:?}",
                locale.code
            );
        }
    }
}
