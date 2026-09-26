use serde::Serialize;

use crate::i18n;

#[derive(Debug, Clone, Serialize)]
pub struct MenuItem {
    pub href: String,
    pub icon: String,
    pub label: String,
    pub js_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MenuEntry {
    pub item: Option<MenuItem>,
}

pub fn more_menu_items(locale: &str) -> Vec<MenuEntry> {
    let entry = |href: String, icon: &str, key: &str, js_only: bool| MenuEntry {
        item: Some(MenuItem {
            href,
            icon: icon.to_owned(),
            label: i18n::t(locale, key),
            js_only,
        }),
    };
    vec![
        entry(
            "#shortcuts-modal".to_owned(),
            "command",
            "nav.shortcuts",
            true,
        ),
        entry(i18n::lp(locale, "/docs"), "notebook", "nav.docs", false),
        entry(i18n::lp(locale, "/faq"), "circle-help", "nav.faq", false),
        entry("#share-modal".to_owned(), "share", "nav.share", false),
        entry("#pair-modal".to_owned(), "smartphone", "nav.pair_app", true),
        entry("#language-modal".to_owned(), "globe", "nav.language", false),
        MenuEntry { item: None },
        entry(
            i18n::lp(locale, "/feedback"),
            "message-square",
            "nav.feedback",
            false,
        ),
        entry(i18n::lp(locale, "/terms"), "shield", "nav.terms", false),
        entry(i18n::lp(locale, "/privacy"), "lock", "nav.privacy", false),
    ]
}
