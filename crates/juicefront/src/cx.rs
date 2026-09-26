use crate::{
    i18n, icons,
    ssr::{format, more_menu, retention},
};
#[derive(Debug, Clone)]
pub struct Cx {
    pub locale: &'static str,
}

#[expect(
    clippy::unused_self,
    reason = "Askama templates invoke these as cx methods; free functions are not callable from templates"
)]
impl Cx {
    pub const fn new(locale: &'static str) -> Self {
        Self { locale }
    }

    pub fn t(&self, key: &str) -> String {
        i18n::t(self.locale, key)
    }

    pub fn tv(&self, key: &str, name: &str, value: &str) -> String {
        i18n::tv(self.locale, key, name, value)
    }

    pub fn icon(&self, name: &str, size: u32) -> String {
        icons::icon_html(name, size, "")
    }

    pub fn icon_c(&self, name: &str, size: u32, class: &str) -> String {
        icons::icon_html(name, size, class)
    }

    pub fn fmt_size(&self, bytes: u64) -> String {
        format::format_size(bytes)
    }

    pub fn fmt_max(&self, bytes: u64) -> String {
        format::format_max_size(bytes)
    }

    pub fn ttl(&self, hours: f64) -> String {
        retention::ttl_label(self.locale, hours)
    }

    pub fn lp(&self, path: &str) -> String {
        i18n::lp(self.locale, path)
    }

    pub fn prefix(&self) -> &str {
        i18n::locale_prefix(self.locale)
    }

    pub fn mime_icon(&self, mime: &str) -> String {
        format::icon_for_mime(mime).to_owned()
    }

    pub fn menu(&self) -> Vec<more_menu::MenuEntry> {
        more_menu::more_menu_items(self.locale)
    }
}
