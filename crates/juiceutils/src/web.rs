

pub const HEAD_META: &str = include_str!("../templates/partials/head_meta.html");

pub const BASE_CSS: &str = include_str!("../templates/shared/base.css");

pub const BRAND_CSS: &str = include_str!("../templates/shared/brand.css");

pub const PREVIEW_CSS: &str = include_str!("../templates/shared/preview_chrome.css");

pub const FONT_FACE_CSS: &str = include_str!("../templates/partials/font_face.css");

pub const BRAND_HTML: &str = include_str!("../templates/partials/brand.html");

#[must_use]
pub fn font_face_css(font_data_uri: &str) -> String {
    FONT_FACE_CSS.replace("__FONT__", font_data_uri)
}

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
        assert!(html.contains("Juicebox²"));
        assert!(html.contains("brand-logo"));
    }

    #[test]
    fn base_covers_topbar_and_filename() {
        assert!(BASE_CSS.contains(".topbar{"));
        assert!(BASE_CSS.contains(".fname{"));
        assert!(BRAND_CSS.contains(".brand-logo"));
        assert!(HEAD_META.contains("noindex"));
        assert!(PREVIEW_CSS.contains(".audio-hero"));
        assert!(PREVIEW_CSS.contains(".dl-hero"));
        assert!(PREVIEW_CSS.contains(".text-view"));
    }
}
