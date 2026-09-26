use std::{collections::HashMap, sync::LazyLock};

pub const LOCALES: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "Français"),
    ("ru", "Русский"),
    ("es", "Español"),
];

pub const DEFAULT_LOCALE: &str = "en";

pub const LOCALE_PREFIXES: &[&str] = &["fr", "ru", "es"];

pub fn is_valid_locale(code: &str) -> bool {
    LOCALES.iter().any(|(c, _)| *c == code)
}

fn locale_static(code: &str) -> &'static str {
    match code {
        "fr" => "fr",
        "ru" => "ru",
        "es" => "es",
        _ => "en",
    }
}

pub fn locale_prefix(locale: &str) -> &str {
    if locale == DEFAULT_LOCALE {
        ""
    } else {
        match locale {
            "fr" => "/fr",
            "ru" => "/ru",
            "es" => "/es",
            _ => "",
        }
    }
}

pub fn lp(locale: &str, path: &str) -> String {
    format!("{}{path}", locale_prefix(locale))
}

pub fn split_locale(path: &str) -> (&'static str, String) {
    for prefix in LOCALE_PREFIXES {
        if path == format!("/{prefix}") {
            return (locale_static(prefix), "/".to_owned());
        }
        if let Some(rest) = path.strip_prefix(&format!("/{prefix}/")) {
            return (locale_static(prefix), format!("/{rest}"));
        }
    }
    ("en", path.to_owned())
}

struct Bundles {
    maps: HashMap<String, HashMap<String, String>>,
}

static BUNDLES: LazyLock<Bundles> = LazyLock::new(|| {
    let mut maps = HashMap::new();
    for (code, text) in [
        ("en", include_str!("../i18n/en.json")),
        ("fr", include_str!("../i18n/fr.json")),
        ("ru", include_str!("../i18n/ru.json")),
        ("es", include_str!("../i18n/es.json")),
    ] {
        let parsed: serde_json::Value = serde_json::from_str(text).expect("i18n JSON must parse");
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
        maps.insert(code.to_owned(), dict);
    }
    Bundles { maps }
});

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

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn pct_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_resolve_and_prefix() {
        assert_eq!(split_locale("/fr/files").0, "fr");
        assert_eq!(split_locale("/files").0, "en");
        assert!(is_valid_locale("ru"));
        assert!(!is_valid_locale("xx"));
        assert_eq!(locale_prefix("en"), "");
        assert_eq!(locale_prefix("fr"), "/fr");
        assert_eq!(lp("fr", "/files"), "/fr/files");
        assert_eq!(lp("en", "/files"), "/files");
    }

    #[test]
    fn translations_fall_back_to_english() {
        assert!(!t("en", "site.title").is_empty());
        assert!(!t("fr", "site.title").is_empty());
        assert_eq!(t("xx", "site.title"), t("en", "site.title"));
        assert_eq!(t("en", "no.such.key"), "no.such.key");
    }

    #[test]
    fn all_locales_cover_english_keys() {
        let en_keys: Vec<String> = BUNDLES.maps["en"].keys().cloned().collect();
        assert!(!en_keys.is_empty());
        for (code, _) in LOCALES.iter().skip(1) {
            let map = &BUNDLES.maps[*code];
            let missing: Vec<&String> = en_keys
                .iter()
                .filter(|key| !map.contains_key(*key))
                .collect();
            assert!(missing.is_empty(), "{code} missing keys: {missing:?}");
        }
    }

    #[test]
    fn strip_locale_prefix_routes() {
        assert_eq!(split_locale("/fr/files"), ("fr", "/files".to_owned()));
        assert_eq!(split_locale("/files"), ("en", "/files".to_owned()));
        assert_eq!(split_locale("/ru"), ("ru", "/".to_owned()));
    }
}
