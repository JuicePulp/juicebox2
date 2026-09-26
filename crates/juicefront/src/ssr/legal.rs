pub const TERMS_TXT: &str = include_str!("../../static/public/terms-of-service.txt");
pub const PRIVACY_TXT: &str = include_str!("../../static/public/privacy-policy.txt");

pub static TERMS_HTML: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| render_txt_to_html(TERMS_TXT));
pub static PRIVACY_HTML: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| render_txt_to_html(PRIVACY_TXT));

pub fn render_txt_to_html(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_list = false;
    let mut para: Vec<String> = Vec::new();

    let close_para = |out: &mut Vec<String>, para: &mut Vec<String>| {
        if !para.is_empty() {
            out.push(format!("<p>{}</p>", inline(&para.join("<br>"))));
            para.clear();
        }
    };
    let close_list = |out: &mut Vec<String>, in_list: &mut bool| {
        if *in_list {
            out.push("</ul>".to_owned());
            *in_list = false;
        }
    };

    for line in raw.replace("\r\n", "\n").split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            close_list(&mut out, &mut in_list);
            close_para(&mut out, &mut para);
            continue;
        }
        if let Some(heading) = parse_heading(trimmed) {
            close_list(&mut out, &mut in_list);
            close_para(&mut out, &mut para);
            out.push(heading);
            continue;
        }
        if trimmed.chars().all(|ch| ch == '-') && trimmed.len() >= 3 {
            close_list(&mut out, &mut in_list);
            close_para(&mut out, &mut para);
            out.push("<hr>".to_owned());
            continue;
        }
        if let Some(banner) = parse_banner(trimmed) {
            close_list(&mut out, &mut in_list);
            close_para(&mut out, &mut para);
            out.push(banner);
            continue;
        }
        if let Some(item) = parse_item(trimmed) {
            close_para(&mut out, &mut para);
            if !in_list {
                out.push("<ul>".to_owned());
                in_list = true;
            }
            out.push(format!("<li>{}</li>", inline(item)));
            continue;
        }
        close_list(&mut out, &mut in_list);
        para.push(trimmed.to_owned());
    }
    close_list(&mut out, &mut in_list);
    close_para(&mut out, &mut para);
    out.join("\n")
}

fn parse_heading(trimmed: &str) -> Option<String> {
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = trimmed[hashes..].strip_prefix(' ')?;
    let level = (hashes + 1).min(3);
    Some(format!("<h{level}>{}</h{level}>", inline(rest)))
}

fn parse_banner(trimmed: &str) -> Option<String> {
    let inner = trimmed.strip_prefix("---")?.strip_suffix("---")?.trim();
    if inner.is_empty() || inner.contains("---") {
        return None;
    }
    Some(format!("<h2>{}</h2>", inline(inner)))
}

fn parse_item(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('*'))
        .and_then(|rest| rest.strip_prefix(' '))
        .map(str::trim)
}

fn inline(text: &str) -> String {
    let escaped = crate::i18n::escape_html(text);
    apply_markup(&escaped, "**", "strong", "em")
}

fn apply_markup(text: &str, token: &str, strong: &str, em: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(token) {
        let after = &rest[start + token.len()..];
        if let Some(end) = after.find(token) {
            out.push_str(&rest[..start]);
            out.push_str(&format!("<{strong}>{}</{strong}>", &after[..end]));
            rest = &after[end + token.len()..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    if token == "**" {
        return apply_markup(&out, "*", em, em);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headings_lists_and_markup() {
        let html = render_txt_to_html("# Title\n\n- one\n- **two**\n\nplain *em* text");
        assert!(html.contains("<h2>Title</h2>"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li><strong>two</strong></li>"));
        assert!(html.contains("<p>plain <em>em</em> text</p>"));
    }

    #[test]
    fn legal_texts_render_nonempty() {
        assert!(!render_txt_to_html(TERMS_TXT).is_empty());
        assert!(!render_txt_to_html(PRIVACY_TXT).is_empty());
    }
}
