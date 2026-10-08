//! Shared HTML page parts for juicehost file pages.
//!
//! Single source of truth for the head metas, base styles, and brand header
//! used by the preview page and the unlock shell: edit the partial files and
//! every page follows. Pages embed these via `__HEAD_META__`, `__BASE_CSS__`,
//! `__BRAND_CSS__`, `__FONT_FACE__`, and `__BRAND__` tokens (partials carry
//! no tokens of their own except `__FONT__`/`__LOGO__`, filled at render).

/// Crawler/theme metas shared by all file pages.
pub const HEAD_META: &str = include_str!("../templates/partials/head_meta.html");

/// Reset, page body, top bar, and filename - identical on every file page.
pub const BASE_CSS: &str = include_str!("../templates/shared/base.css");

/// Pixel brand lockup (logo + Title font), shared by every file page.
pub const BRAND_CSS: &str = include_str!("../templates/shared/brand.css");

/// Brand font face with a `__FONT__` slot for the inlined woff2 data URI.
pub const FONT_FACE_CSS: &str = include_str!("../templates/partials/font_face.css");

/// Brand anchor with a `__LOGO__` slot for the inlined logo data URI.
pub const BRAND_HTML: &str = include_str!("../templates/partials/brand.html");

/// Brand font face for an inlined woff2 data URI.
#[must_use]
pub fn font_face_css(font_data_uri: &str) -> String {
    FONT_FACE_CSS.replace("__FONT__", font_data_uri)
}

/// Brand anchor for the top bar (inlined logo + pixel wordmark).
#[must_use]
pub fn brand_html(logo_data_uri: &str) -> String {
    BRAND_HTML.replace("__LOGO__", logo_data_uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_carry_no_stray_tokens() {
        for part in [HEAD_META, BASE_CSS, BRAND_CSS] {
            assert!(!part.contains("__"), "component leaks a token: {part}");
        }
        assert!(!font_face_css("X").contains("__"));
        assert!(!brand_html("X").contains("__"));
    }

    #[test]
    fn brand_has_logo_and_wordmark() {
        let html = brand_html("DATA_URI");
        assert!(html.contains("DATA_URI"));
        assert!(html.contains("Juicebox<sup>2</sup>"));
        assert!(html.contains("brand-logo"));
    }

    #[test]
    fn base_covers_topbar_and_filename() {
        assert!(BASE_CSS.contains(".topbar{"));
        assert!(BASE_CSS.contains(".fname{"));
        assert!(BRAND_CSS.contains(".brand-logo"));
        assert!(HEAD_META.contains("noindex"));
    }
}
