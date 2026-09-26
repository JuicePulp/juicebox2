use std::{collections::HashMap, net::IpAddr, sync::Arc, time::Duration};

use askama::Template;
use axum::{
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};

use crate::{
    cx::Cx,
    i18n, proxy,
    proxy::redirect_to,
    ssr::{
        announce, ban, format, health, host, hosts, legal, metadata, retention, types::ServerFile,
    },
    state::AppState,
};

pub struct Shell {
    pub cx: Cx,
    pub modals_html: String,
    pub ads_html: String,
    pub i18n_json: String,
    pub icons_json: String,
    pub extra_scripts: Vec<String>,
}

pub struct Hreflang {
    pub code: &'static str,
    pub href: String,
}

pub struct CardAction {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub aria: String,
    pub href: String,
}

fn hreflang_links(stripped: &str) -> Vec<Hreflang> {
    i18n::LOCALES
        .iter()
        .map(|(code, _)| {
            let href = if *code == "en" {
                stripped.to_owned()
            } else if stripped == "/" {
                format!("/{code}")
            } else {
                format!("/{code}{stripped}")
            };
            Hreflang {
                code: match *code {
                    "fr" => "fr",
                    "ru" => "ru",
                    "es" => "es",
                    _ => "en",
                },
                href,
            }
        })
        .collect()
}

fn absolute_url(headers: &HeaderMap, path_query: &str) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost:6400");
    format!("{scheme}://{host}{path_query}")
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get("cookie")?.to_str().ok()?;
    for part in header.split(';') {
        let (key, value) = part.split_once('=')?;
        if key.trim() == name {
            return Some(value.trim().to_owned());
        }
    }
    None
}

fn query_map(uri: &axum::http::Uri) -> HashMap<String, String> {
    uri.query()
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (host::percent_decode(key), host::percent_decode(value)))
        .collect()
}

struct Shared {
    offline: bool,
    ban: ban::BanStatus,
    eff: host::EffectiveHostConfig,
}

async fn shared(state: &AppState, headers: &HeaderMap, peer: IpAddr) -> Shared {
    let cookie_host =
        host::read_cookie_host(headers.get("cookie").and_then(|value| value.to_str().ok()));
    let (offline, ban, eff) = tokio::join!(
        async { !health::check_backend_health(&state.http, &state.config.juiceback_url).await },
        ban::check_user_ban(
            &state.http,
            &state.config.juiceback_url,
            headers,
            peer,
            &state.config.trusted_proxies,
        ),
        host::resolve_effective_config(state, cookie_host.as_deref()),
    );
    let data = Shared { offline, ban, eff };
    tracing::debug!(
        host = %data.eff.host,
        custom = data.eff.custom,
        offline = data.offline,
        "resolved page request context",
    );
    data
}

#[derive(Template)]
#[template(path = "modals.html")]
struct ModalsTpl {
    cx: Cx,
    flags: ServerFlags,
    upload_mode: String,
    danger_level: String,
    max_bytes: u64,
    allowed_ttl: String,
    default_ttl: String,
    public_base_url: String,
    share_url: String,
    encoded_url: String,
    encoded_text: String,
    share_check_empty: String,
    selector_href: String,
    blocked_text: String,
    shortcuts: Vec<Shortcut>,
    lang_links: Vec<LangLink>,
    locale_codes_json: String,
}

struct ServerFlags {
    quick_link: bool,
    quic: bool,
    custom_id: bool,
    ultrafast: bool,
    cobalt: bool,
}

struct Shortcut {
    action: String,
    keys: Vec<String>,
}

struct LangLink {
    code: &'static str,
    label: String,
    href: String,
    active: bool,
}

fn modals_html(
    locale: &'static str,
    url_href: &str,
    url_path: &str,
    eff: &host::EffectiveHostConfig,
) -> String {
    let cx = Cx::new(locale);
    let config = &eff.config;
    let danger = config.danger_level.as_deref().unwrap_or("high");
    let blocked_key = format!("host.blocked_{danger}");
    let blocked_text = {
        let text = cx.t(&blocked_key);
        if text == blocked_key {
            cx.t("host.blocked_high")
        } else {
            text
        }
    };
    let shortcut = |action_key: &str, keys: &[&str]| Shortcut {
        action: cx.t(action_key),
        keys: keys.iter().map(|key| (*key).to_owned()).collect(),
    };
    let base_path = i18n::split_locale(url_path).1;
    let lang_links = i18n::LOCALES
        .iter()
        .map(|(code, label)| {
            let href = if *code == "en" {
                base_path.clone()
            } else if base_path == "/" {
                format!("/{code}")
            } else {
                format!("/{code}{base_path}")
            };
            LangLink {
                code: match *code {
                    "fr" => "fr",
                    "ru" => "ru",
                    "es" => "es",
                    _ => "en",
                },
                label: (*label).to_owned(),
                href,
                active: *code == locale,
            }
        })
        .collect();
    ModalsTpl {
        cx: cx.clone(),
        flags: ServerFlags {
            quick_link: config.quick_link.unwrap_or(true),
            quic: config.quic.unwrap_or(true),
            custom_id: config.custom_id.unwrap_or(true),
            ultrafast: config.ultrafast.unwrap_or(false),
            cobalt: config.cobalt.unwrap_or(false),
        },
        upload_mode: config
            .upload_mode
            .clone()
            .unwrap_or_else(|| "standard".to_owned()),
        danger_level: danger.to_owned(),
        max_bytes: config.max_file_size_bytes.unwrap_or(524_288_000),
        allowed_ttl: config
            .allowed_ttl_hours
            .clone()
            .unwrap_or_else(|| retention::DEFAULT_ALLOWED_TTL_HOURS.to_vec())
            .iter()
            .map(|hours| {
                if hours.fract() == 0.0 {
                    format!("{}", *hours as i64)
                } else {
                    format!("{hours}")
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        default_ttl: config
            .default_ttl_hours
            .unwrap_or(retention::DEFAULT_TTL_HOURS)
            .to_string(),
        public_base_url: config.public_base_url.clone().unwrap_or_default(),
        share_url: url_href.to_owned(),
        encoded_url: i18n::pct_encode(url_href),
        encoded_text: i18n::pct_encode(&cx.tv("share.check_out", "url", url_href)),
        share_check_empty: i18n::pct_encode(&cx.tv("share.check_out", "url", "")),
        selector_href: format!("{}#host-selector-modal", cx.prefix()),
        blocked_text,
        shortcuts: vec![
            shortcut("shortcuts.file_picker", &["Ctrl", "U"]),
            shortcut("shortcuts.paste_upload", &["Ctrl", "V"]),
            shortcut("shortcuts.copy_file_url", &["Ctrl", "C"]),
            shortcut("shortcuts.close_menu", &["Esc"]),
            shortcut("shortcuts.show_dialog", &["?"]),
            shortcut("shortcuts.go_home", &["G", "H"]),
            shortcut("shortcuts.go_files", &["G", "F"]),
            shortcut("shortcuts.go_report", &["G", "R"]),
            shortcut("shortcuts.go_docs", &["G", "D"]),
            shortcut("shortcuts.open_share", &["G", "S"]),
            shortcut("shortcuts.open_language", &["G", "L"]),
            shortcut("shortcuts.open_settings", &["G", "T"]),
        ],
        lang_links,
        locale_codes_json: "[\"en\",\"fr\",\"ru\",\"es\"]".to_owned(),
    }
    .render()
    .unwrap_or_default()
}

struct AdZone {
    key: String,
    format: String,
}

struct AdSlot {
    id: String,
    cls: String,
    w: u32,
    h: u32,
    key: String,
    format: String,
}

struct AdGroup {
    aside: bool,
    cls: String,
    slots: Vec<AdSlot>,
}

#[derive(Template)]
#[template(path = "ads.html")]
struct AdsTpl {
    cx: Cx,
    eligible: bool,
    groups: Vec<AdGroup>,
}

fn ad_zones() -> std::collections::HashMap<String, AdZone> {
    let mut zones = std::collections::HashMap::new();
    let pairs: &[(&str, &[&str])] = &[
        (
            "PUBLIC_ADSTERRA_SKYSCRAPER_160X600",
            &["box-zone-160-600-l", "box-zone-160-600-r"],
        ),
        (
            "PUBLIC_ADSTERRA_SKYSCRAPER_160X300",
            &["box-zone-160-300-l", "box-zone-160-300-r"],
        ),
        ("PUBLIC_ADSTERRA_LEADERBOARD_468X60", &["box-zone-468-60"]),
        ("PUBLIC_ADSTERRA_MOBILE_BANNER_320X50", &["box-zone-320-50"]),
    ];
    for (env, holders) in pairs {
        if let Some(key) = juiceutils::config::optional_secret(env)
            && !key.is_empty()
        {
            for holder in holders.iter() {
                zones.insert(
                    (*holder).to_owned(),
                    AdZone {
                        key: key.clone(),
                        format: "iframe".to_owned(),
                    },
                );
            }
        }
    }
    zones
}

fn ads_html(locale: &'static str, headers: &HeaderMap) -> String {
    let eligible = cookie_value(headers, "ADTESTING").is_some_and(|value| value == "TRUE");
    let zones = ad_zones();
    if zones.is_empty() {
        return String::new();
    }
    let slot = |id: &str, cls: &str, w: u32, h: u32| AdSlot {
        id: id.to_owned(),
        cls: cls.to_owned(),
        w,
        h,
        key: zones
            .get(id)
            .map(|zone| zone.key.clone())
            .unwrap_or_default(),
        format: zones
            .get(id)
            .map(|zone| zone.format.clone())
            .unwrap_or_default(),
    };
    let rail = "box-slot box-slot--rail";
    AdsTpl {
        cx: Cx::new(locale),
        eligible,
        groups: vec![
            AdGroup {
                aside: true,
                cls: "box-rail box-rail--left".to_owned(),
                slots: vec![
                    slot("box-zone-160-600-l", rail, 160, 600),
                    slot("box-zone-160-300-l", rail, 160, 300),
                ],
            },
            AdGroup {
                aside: true,
                cls: "box-rail box-rail--right".to_owned(),
                slots: vec![
                    slot("box-zone-160-300-r", rail, 160, 300),
                    slot("box-zone-160-600-r", rail, 160, 600),
                ],
            },
            AdGroup {
                aside: false,
                cls: "box-below".to_owned(),
                slots: vec![
                    slot("box-zone-468-60", "box-slot box-slot--banner", 468, 60),
                    slot("box-zone-320-50", "box-slot box-slot--mobile", 320, 50),
                ],
            },
        ],
    }
    .render()
    .unwrap_or_default()
}

fn shell(locale: &'static str, modals: String, ads: String, extra_scripts: Vec<String>) -> Shell {
    Shell {
        cx: Cx::new(locale),
        modals_html: modals,
        ads_html: ads,
        i18n_json: i18n::embedded_json(locale).replace("</", "<\\/"),
        icons_json: crate::icons::bundle_json(crate::icons::RUNTIME_ICONS).replace("</", "<\\/"),
        extra_scripts,
    }
}

fn no_extra() -> Vec<String> {
    Vec::new()
}

struct HeadCtx {
    cx: Cx,
    lang: &'static str,
    title: String,
    description: String,
    canonical: String,
    og_image: String,
    body_class: &'static str,
    commit_label: String,
    structured_data: String,
    current_path: String,
    hreflang: Vec<Hreflang>,
    live_reload: bool,
}

#[derive(Template)]
#[template(path = "shell_head.html")]
struct HeadTpl {
    head: HeadCtx,
}

fn head_ctx(
    locale: &'static str,
    title: String,
    description: String,
    body_class: &'static str,
    stripped_path: &str,
    live_reload: bool,
) -> HeadCtx {
    HeadCtx {
        cx: Cx::new(locale),
        lang: locale,
        title,
        description,
        canonical: metadata::REPO_URL.to_owned(),
        og_image: "/static/assets/og-image.png".to_owned(),
        body_class,
        commit_label: metadata::commit_label(),
        structured_data: metadata::structured_data_json(),
        current_path: stripped_path.to_owned(),
        hreflang: hreflang_links(stripped_path),
        live_reload,
    }
}

fn flags_html(offline: bool, banned: bool) -> String {
    format!(
        "<div id=\"page-flags\" hidden{}{}></div>",
        if offline {
            " data-backend-offline=\"\""
        } else {
            ""
        },
        if banned { " data-banned=\"\"" } else { "" },
    )
}

fn stream_page(
    status: StatusCode,
    stream: impl futures::Stream<Item = Result<bytes::Bytes, std::convert::Infallible>> + Send + 'static,
) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "private, max-age=10")
        .header("vary", "Cookie")
        .body(Body::from_stream(stream))
        .unwrap_or_default()
}

const CSS_BUNDLE: &str = concat!(
    include_str!("../static/styles/upload.css"),
    "\n",
    include_str!("../static/styles/components/RetentionToolbar.css"),
    "\n",
    include_str!("../static/styles/components/RetentionToolbarShared.css"),
    "\n",
    include_str!("../static/styles/components/Page.css"),
    "\n",
    include_str!("../static/styles/components/UploadHeader.css"),
    "\n",
    include_str!("../static/styles/components/UploadFooter.css"),
    "\n",
    include_str!("../static/styles/components/Select.css"),
    "\n",
    include_str!("../static/styles/components/UploadedFilesList.css"),
    "\n",
    include_str!("../static/styles/components/EditFileModal.css"),
    "\n",
    include_str!("../static/styles/components/FilesCard.css"),
    "\n",
    include_str!("../static/styles/components/ContentCard.css"),
    "\n",
    include_str!("../static/styles/components/LanguageModal.css"),
    "\n",
    include_str!("../static/styles/components/CopyBar.css"),
    "\n",
    include_str!("../static/styles/components/NoJs.css"),
    "\n",
    include_str!("../static/styles/components/AnnouncementBanner.css"),
    "\n",
    include_str!("../static/styles/components/UploadTray.css"),
    "\n",
    include_str!("../static/styles/components/ReportCard.css"),
    "\n",
    include_str!("../static/styles/components/ReportForm.css"),
    "\n",
    include_str!("../static/styles/components/DownloadPage.css"),
    "\n",
    include_str!("../static/styles/components/AdSlots.css"),
    "\n",
    include_str!("../static/styles/components/HostSelector.css"),
    "\n",
    include_str!("../static/styles/components/Modals.css"),
    "\n",
    include_str!("../static/styles/components/NetDebug.css"),
    "\n",
    include_str!("../static/styles/components/DocsPage.css"),
);

pub async fn css_bundle() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/css; charset=utf-8")
        .header("cache-control", "no-cache")
        .body(Body::from(CSS_BUNDLE))
        .unwrap_or_default()
}

fn locale_from_path(path: Option<Path<String>>) -> Option<&'static str> {
    match path {
        Some(Path(code)) => match code.as_str() {
            "fr" => Some("fr"),
            "ru" => Some("ru"),
            "es" => Some("es"),
            _ => None,
        },
        None => Some("en"),
    }
}

#[derive(Template)]
#[template(path = "banner.html")]
struct BannerTpl {
    cx: Cx,
    banner: announce::Banner,
    external: bool,
}

#[derive(Template)]
#[template(path = "hostsel.html")]
struct HostselTpl {
    cx: Cx,
    official: Vec<HostRow>,
    unofficial: Vec<HostRow>,
}

struct HostRow {
    url: String,
    name: String,
    region: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "upload_card.html")]
struct UploadCardTpl {
    cx: Cx,
    cobalt: bool,
    cobalt_services: String,
    services: Vec<String>,
    retention: Vec<RetentionOpt>,
    drop_max_html: String,
    uploaded: Option<UploadedFile>,
    repo_url: String,
    commit_short: String,
}

struct RetentionOpt {
    hours: f64,
    label: String,
    checked: bool,
}

struct UploadedFile {
    id: String,
    filename: String,
    icon: String,
    size_str: String,
    url: String,
    delete_token: String,
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .ok()
}

fn json_str(value: &serde_json::Value, key: &str, max_len: usize) -> String {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(|text| text.chars().take(max_len).collect())
        .unwrap_or_default()
}

fn json_num(value: &serde_json::Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .filter(|num| num.is_finite())
        .map_or(0, |num| num as i64)
}

fn parse_uploaded(param: &str) -> Option<UploadedFile> {
    let bytes = base64url_decode(param)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let mime = json_str(&value, "mime_type", 200);
    let size = json_num(&value, "size_bytes") as u64;
    Some(UploadedFile {
        id: json_str(&value, "id", 50),
        filename: json_str(&value, "filename", 500),
        icon: format::icon_for_mime(&mime).to_owned(),
        size_str: format::format_size(size),
        url: json_str(&value, "url", 2048),
        delete_token: json_str(&value, "delete_token", 200),
    })
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTpl {
    shell: Shell,
    flags_html: String,
    banner_html: String,
    upload_html: String,
    hostsel_html: String,
}

pub async fn index(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    req: axum::extract::Request<Body>,
) -> Response<Body> {
    if req.method() != axum::http::Method::GET {
        return proxy::proxy_handler(State(state), ConnectInfo(peer), req).await;
    }
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let query = query_map(&uri);
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let uri_path = uri.path().to_owned();
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            cx.t("site.title"),
            cx.t("site.description_short"),
            "upload-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (data, announcement) = tokio::join!(
            shared(&state, &headers, peer.ip()),
            announce::fetch_announcement(&state),
        );
        let mut allowed = data
        .eff
        .config
        .allowed_ttl_hours
        .clone()
        .unwrap_or_else(|| retention::DEFAULT_ALLOWED_TTL_HOURS.to_vec());
    if allowed.is_empty() {
        allowed = retention::DEFAULT_ALLOWED_TTL_HOURS.to_vec();
    }
    let default_ttl = data
        .eff
        .config
        .default_ttl_hours
        .unwrap_or(retention::DEFAULT_TTL_HOURS);
    let default_index = allowed
        .iter()
        .position(|hours| *hours == default_ttl)
        .unwrap_or(0);
    let max_bytes = data.eff.config.max_file_size_bytes.unwrap_or(524_288_000);
    let cobalt = data.eff.config.cobalt.unwrap_or(false);
    let services = if cobalt {
        host::fetch_cobalt_services(&state).await
    } else {
        Vec::new()
    };
    let drop_max_html = cx.tv(
        "upload.drop_max",
        "size",
        &format!("<span data-drop-max-size>{}</span>", cx.fmt_max(max_bytes)),
    );
    let uploaded = query
        .get("uploaded")
        .and_then(|param| parse_uploaded(param));

    let upload_html = UploadCardTpl {
        cx: cx.clone(),
        cobalt,
        cobalt_services: services.join(","),
        services,
        retention: allowed
            .into_iter()
            .enumerate()
            .map(|(index, hours)| RetentionOpt {
                hours,
                label: cx.ttl(hours),
                checked: index == default_index,
            })
            .collect(),
        drop_max_html,
        uploaded,
        repo_url: metadata::REPO_URL.to_owned(),
        commit_short: metadata::commit_short_hash(),
    }
    .render()
    .unwrap_or_default();

    let banner = announce::build_banner(locale, data.ban.banned, announcement);
    let external = !banner.link_url.is_empty() && !banner.link_url.starts_with('/');
    let banner_html = BannerTpl {
        cx: cx.clone(),
        banner,
        external,
    }
    .render()
    .unwrap_or_default();

    let saved = host::read_cookie_host(headers.get("cookie").and_then(|value| value.to_str().ok()))
        .unwrap_or_default();
    let official: Vec<HostRow> = hosts::official_nodes()
        .into_iter()
        .map(|node| HostRow {
            selected: node.url == saved,
            url: node.url,
            name: node.name,
            region: node.region,
        })
        .collect();
    let unofficial: Vec<HostRow> = hosts::unofficial_nodes()
        .into_iter()
        .map(|node| HostRow {
            selected: node.url == saved,
            url: node.url,
            name: node.name,
            region: node.region,
        })
        .collect();
    let hostsel_html = HostselTpl {
        cx: cx.clone(),
        official,
        unofficial,
    }
    .render()
    .unwrap_or_default();

    let shell = shell(
        locale,
        modals_html(locale, &absolute_url(&headers, &path_query), &uri_path, &data.eff),
        ads_html(locale, &headers),
        vec!["/static/js/upload-card.js".to_owned()],
    );
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
        IndexTpl {
            shell,
            flags_html: flags_html(data.offline, data.ban.banned),
            banner_html,
            upload_html,
            hostsel_html,
        }
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream)
}

#[derive(Template)]
#[template(path = "files_card.html")]
struct FilesTpl {
    cx: Cx,
    card_id: String,
    header_id: String,
    actions_id: String,
    title: String,
    subtitle: String,
    footer_commit: String,
    footer_message: String,
    footer_repo: String,
    empty_message: String,
    actions: Vec<CardAction>,
    has_actions: bool,
    files: Vec<FileRow>,
}

struct FileRow {
    id: String,
    filename: String,
    icon: String,
    meta: String,
    size_bytes: u64,
    uploaded_at: i64,
    expires_at: i64,
    url: String,
    delete_token: String,
    storage_host: String,
    show_host: bool,
    host_short: String,
    can_rename: bool,
    expired: bool,
    ttl_label: String,
    ttl_pct: u64,
    short_id: String,
    index: usize,
}

fn extract_short_id(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let segment = path.rsplit('/').next().unwrap_or("");
    match segment.rsplit_once('.') {
        Some((base, _)) if !base.is_empty() => base.to_owned(),
        _ => segment.to_owned(),
    }
}

fn card_action(locale: &str, id: &str, label_key: &str, icon: &str, href: String) -> CardAction {
    CardAction {
        id: id.to_owned(),
        label: i18n::t(locale, label_key),
        icon: icon.to_owned(),
        aria: i18n::t(locale, label_key),
        href,
    }
}

async fn server_files(state: &AppState, headers: &HeaderMap) -> Vec<ServerFile> {
    let Some(encoded) = cookie_value(headers, "jb_files") else {
        return Vec::new();
    };
    let Some(bytes) = base64url_decode(&encoded) else {
        return Vec::new();
    };
    let pairs: Vec<serde_json::Value> = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            entry.get("id").and_then(|id| id.as_str()).is_some()
                && entry
                    .get("token")
                    .and_then(|token| token.as_str())
                    .is_some()
        })
        .map(|entry| {
            serde_json::json!({
                "id": entry.get("id"),
                "token": entry.get("token"),
            })
        })
        .collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let files = match tokio::time::timeout(Duration::from_secs(5), async {
        let response = state
            .http
            .post(format!("{}/api/owned-files", state.config.juiceback_url))
            .json(&serde_json::json!({ "pairs": pairs }))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<serde_json::Value>().await.ok()
    })
    .await
    {
        Ok(Some(data)) => data,
        _ => return Vec::new(),
    };
    let now = chrono_now();
    files
        .get("files")
        .and_then(|data| data.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|file| serde_json::from_value::<ServerFile>(file).ok())
        .filter(|file| file.expires_at > now)
        .map(|mut file| {
            file.delete_token = String::new();
            file
        })
        .collect()
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[derive(Template)]
#[template(path = "files.html")]
struct FilesPageTpl {
    shell: Shell,
    flags_html: String,
    files_html: String,
}

pub async fn files_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let uri_path = uri.path().to_owned();
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("files.title")),
            cx.t("files.subtitle"),
            "files-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let data = shared(&state, &headers, peer.ip()).await;

    let server_files = if data.offline {
        Vec::new()
    } else {
        server_files(&state, &headers).await
    };
    let default_host = data.eff.config.public_base_url.clone().unwrap_or_default();
    let custom_id = data.eff.config.custom_id.unwrap_or(true);
    let timestamp = now_ms();
    let rows: Vec<FileRow> = server_files
        .into_iter()
        .enumerate()
        .map(|(index, file)| {
            let host = file.storage_host.clone().unwrap_or_default();

            FileRow {
                icon: cx.mime_icon(&file.mime_type),
                meta: format!("{} - {}", cx.fmt_size(file.size_bytes), file.mime_type),
                show_host: !host.is_empty()
                    && !format::is_default_host(
                        &host,
                        Some(&default_host)
                            .filter(|h| !h.is_empty())
                            .map(String::as_str),
                    ),
                host_short: host
                    .trim_start_matches("http://")
                    .trim_start_matches("https://")
                    .to_owned(),
                can_rename: custom_id && !file.url.is_empty(),
                expired: file.expires_at * 1000 <= timestamp,
                ttl_label: format::remaining_label(locale, file.expires_at, timestamp),
                ttl_pct: format::pct_remaining(file.expires_at, file.uploaded_at, timestamp),
                short_id: extract_short_id(&file.url),
                id: file.id,
                filename: file.filename,
                size_bytes: file.size_bytes,
                uploaded_at: file.uploaded_at,
                expires_at: file.expires_at,
                url: file.url,
                delete_token: file.delete_token,
                storage_host: host,
                index,
            }
        })
        .collect();

    let actions = vec![
        CardAction {
            id: "back".to_owned(),
            label: cx.t("nav.back"),
            icon: "back".to_owned(),
            aria: cx.t("nav.back_aria"),
            href: cx.lp("/"),
        },
        card_action(locale, "report", "nav.report", "report", cx.lp("/report")),
        card_action(locale, "more", "nav.more", "more", String::new()),
    ];
    let files_html = FilesTpl {
        cx: cx.clone(),
        card_id: "files-card".to_owned(),
        header_id: "files-card-header".to_owned(),
        actions_id: "files-card-actions".to_owned(),
        title: cx.t("files.fallback_title"),
        subtitle: cx.t("files.fallback_subtitle"),
        footer_commit: metadata::commit_label(),
        footer_message: "juicebox2-epsilon".to_owned(),
        footer_repo: metadata::REPO_URL.to_owned(),
        empty_message: cx.t("files.fallback_empty"),
        has_actions: !actions.is_empty(),
        actions,
        files: rows,
    }
    .render()
    .unwrap_or_default();

    let shell = shell(
        locale,
        modals_html(locale, &absolute_url(&headers, &path_query), &uri_path, &data.eff),
        ads_html(locale, &headers),
        vec!["/static/js/files.js".to_owned()],
    );
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
        FilesPageTpl {
            shell,
            flags_html: flags_html(data.offline, data.ban.banned),
            files_html,
        }
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream)
}

fn content_actions(locale: &str, cx: &Cx) -> Vec<CardAction> {
    vec![
        CardAction {
            id: "back".to_owned(),
            label: cx.t("nav.back"),
            icon: "back".to_owned(),
            aria: cx.t("nav.back_aria"),
            href: cx.lp("/"),
        },
        card_action(locale, "files", "nav.files", "files", cx.lp("/files")),
        card_action(locale, "more", "nav.more", "more", String::new()),
    ]
}

async fn content_shell(
    state: &AppState,
    headers: &HeaderMap,
    peer: IpAddr,
    locale: &'static str,
    path_query: &str,
    uri_path: &str,
) -> (Shell, Shared) {
    let data = shared(state, headers, peer).await;
    let shell = shell(
        locale,
        modals_html(
            locale,
            &absolute_url(headers, path_query),
            uri_path,
            &data.eff,
        ),
        ads_html(locale, headers),
        no_extra(),
    );
    (shell, data)
}

#[derive(Template)]
#[template(path = "download_body.html")]
struct DownloadBodyTpl {
    cx: Cx,
    windows_url: String,
    macos_url: String,
    linux_url: String,
    linux_formats: Vec<LinuxFormat>,
}

#[derive(Template)]
#[template(path = "content.html")]
struct ContentTpl {
    shell: Shell,
    flags_html: String,
    card_id: String,
    header_id: String,
    actions_id: String,
    title: String,
    subtitle: String,
    footer_commit: String,
    footer_message: String,
    footer_repo: String,
    actions: Vec<CardAction>,
    body_html: String,
}

struct LinuxFormat {
    label: String,
    icon: String,
    href: String,
}

async fn latest_tag(state: &AppState) -> String {
    const FALLBACK: &str = "v1.1.4";
    if let Ok(cache) = state.github_cache.lock()
        && let (Some(at), tag) = (&cache.0, &cache.1)
        && at.elapsed() < Duration::from_secs(600)
        && !tag.is_empty()
    {
        return tag.clone();
    }
    let tag = async {
        let response = tokio::time::timeout(Duration::from_secs(5), async {
            let response = state
                .http
                .get("https://api.github.com/repos/juiceboxdev/juicebox-plus/releases/latest")
                .header("User-Agent", "juicebox-ui")
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            response.json::<serde_json::Value>().await.ok()
        })
        .await
        .ok()??;
        response
            .get("tag_name")?
            .as_str()
            .map(str::to_owned)
            .filter(|tag| !tag.is_empty())
    }
    .await
    .unwrap_or_else(|| FALLBACK.to_owned());
    if let Ok(mut cache) = state.github_cache.lock() {
        *cache = (Some(std::time::Instant::now()), tag.clone());
    }
    tag
}

pub async fn download_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("download.title")),
            cx.t("download.description"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (shell, data) = content_shell(&state, &headers, peer.ip(), locale, &path_query, &uri_path).await;
    let tag = latest_tag(&state).await;
    let version = tag.strip_prefix('v').unwrap_or(&tag);
    let releases = "https://github.com/juiceboxdev/juicebox-plus/releases";
    let body_html = DownloadBodyTpl {
        cx: cx.clone(),
        windows_url: format!("{releases}/download/{tag}/juicebox-plus-setup-{version}.exe"),
        macos_url: format!("{releases}/download/{tag}/juicebox-plus-macos"),
        linux_url: format!("{releases}/download/{tag}/juicebox-plus-linux"),
        linux_formats: ["AppImage", "Debian", "Fedora"]
            .into_iter()
            .map(|label| LinuxFormat {
                label: label.to_owned(),
                icon: label.to_lowercase(),
                href: releases.to_owned(),
            })
            .collect(),
    }
    .render()
    .unwrap_or_default();
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
        ContentTpl {
            shell,
            flags_html: flags_html(data.offline, data.ban.banned),
            card_id: "content-card".to_owned(),
            header_id: "content-card-header".to_owned(),
            actions_id: "content-card-actions".to_owned(),
            title: "Download Juicebox+".to_owned(),
            subtitle: "The desktop app for UltraFast uploads".to_owned(),
            footer_commit: metadata::commit_label(),
            footer_message: "juicebox2-epsilon".to_owned(),
            footer_repo: metadata::REPO_URL.to_owned(),
            actions: content_actions(locale, &cx),
            body_html,
        }
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream)
}

#[derive(Template)]
#[template(path = "banned.html")]
struct BannedTpl {
    shell: Shell,
    flags_html: String,
    reason: String,
    contact: String,
}

pub async fn banned_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let ban = ban::check_user_ban(
        &state.http,
        &state.config.juiceback_url,
        &headers,
        peer.ip(),
        &state.config.trusted_proxies,
    )
    .await;
    if !ban.banned {
        return redirect_to(&cx.lp("/"));
    }
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("banned.title")),
            cx.t("banned.description"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (shell, data) = content_shell(&state, &headers, peer.ip(), locale, &path_query, &uri_path).await;
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
        BannedTpl {
            shell,
            flags_html: flags_html(data.offline, true),
            reason: ban.reason.unwrap_or_default(),
            contact: juiceutils::config::optional_secret("PUBLIC_CONTACT_EMAIL")
                .unwrap_or_default(),
        }
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream)
}

#[derive(Template)]
#[template(path = "report.html")]
struct ReportTpl {
    shell: Shell,
    flags_html: String,
    title: String,
    subtitle: String,
    submitted: bool,
    footer_commit: String,
    footer_message: String,
    footer_repo: String,
    actions: Vec<CardAction>,
}

#[derive(Template)]
#[template(path = "feedback.html")]
struct FeedbackTpl {
    shell: Shell,
    flags_html: String,
    title: String,
    subtitle: String,
    submitted: bool,
    footer_commit: String,
    footer_message: String,
    footer_repo: String,
    actions: Vec<CardAction>,
}

pub async fn report_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("report.title")),
            cx.t("report.subtitle"),
            "report-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
    let data = shared(&state, &headers, peer.ip()).await;
    let shell = shell(
        locale,
        modals_html(locale, &absolute_url(&headers, &path_query), &uri_path, &data.eff),
        ads_html(locale, &headers),
        no_extra(),
    );
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
        ReportTpl {
            shell,
            flags_html: flags_html(data.offline, data.ban.banned),
            title: cx.t("report.title"),
        subtitle: cx.t("report.subtitle"),
        submitted: query.get("submitted").is_some_and(|value| value == "1"),
        footer_commit: metadata::commit_short_hash(),
        footer_message: "juicebox2-epsilon".to_owned(),
        footer_repo: metadata::REPO_URL.to_owned(),
        actions: content_actions(locale, &cx),
        }
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream)
}

pub async fn feedback_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("feedback.title")),
            cx.t("feedback.subtitle"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
    let data = shared(&state, &headers, peer.ip()).await;
    let shell = shell(
        locale,
        modals_html(locale, &absolute_url(&headers, &path_query), &uri_path, &data.eff),
        ads_html(locale, &headers),
        no_extra(),
    );
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
        FeedbackTpl {
            shell,
            flags_html: flags_html(data.offline, data.ban.banned),
            title: cx.t("feedback.title"),
        subtitle: cx.t("feedback.subtitle"),
        submitted: query.get("submitted").is_some_and(|value| value == "1"),
        footer_commit: metadata::commit_short_hash(),
        footer_message: "juicebox2-epsilon".to_owned(),
        footer_repo: metadata::REPO_URL.to_owned(),
        actions: content_actions(locale, &cx),
        }
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream)
}

#[derive(Template)]
#[template(path = "faq_body.html")]
struct FaqBodyTpl {
    items: Vec<FaqItem>,
}

struct FaqItem {
    q: String,
    a: String,
    open: bool,
}

pub async fn faq_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("faq.title")),
            cx.t("faq.subtitle"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (shell, data) = content_shell(&state, &headers, peer.ip(), locale, &path_query, &uri_path).await;
    let items: Vec<FaqItem> = (1..=10)
        .map(|num| FaqItem {
            q: cx.t(&format!("faq.q{num}")),
            a: cx.t(&format!("faq.a{num}")),
            open: num == 1,
        })
        .collect();
    let body_html = FaqBodyTpl { items }
        .render()
        .unwrap_or_default();
    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
        ContentTpl {
            shell,
            flags_html: flags_html(data.offline, data.ban.banned),
            card_id: "content-card".to_owned(),
            header_id: "content-card-header".to_owned(),
            actions_id: "content-card-actions".to_owned(),
            title: cx.t("faq.title"),
            subtitle: cx.t("faq.subtitle"),
            footer_commit: metadata::commit_label(),
            footer_message: "juicebox2-epsilon".to_owned(),
            footer_repo: metadata::REPO_URL.to_owned(),
            actions: content_actions(locale, &cx),
            body_html,
        }
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream)
}

#[derive(Template)]
#[template(path = "docs_body.html")]
struct DocsBodyTpl {
    rustdoc_exists: bool,
}

pub async fn docs_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("docs.title")),
            cx.t("docs.description"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let rustdoc_exists = state.rustdoc_exists;
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (shell, data) = content_shell(&state, &headers, peer.ip(), locale, &path_query, &uri_path).await;
        let body_html = DocsBodyTpl { rustdoc_exists }
        .render()
        .unwrap_or_default();
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
            ContentTpl {
                shell,
                flags_html: flags_html(data.offline, data.ban.banned),
                card_id: "content-card".to_owned(),
                header_id: "content-card-header".to_owned(),
                actions_id: "content-card-actions".to_owned(),
                title: cx.t("docs.title"),
                subtitle: cx.t("docs.description"),
                footer_commit: metadata::commit_label(),
                footer_message: "juicebox2-epsilon".to_owned(),
                footer_repo: metadata::REPO_URL.to_owned(),
                actions: content_actions(locale, &cx),
                body_html,
            }
            .render()
            .unwrap_or_default(),
        ));
    };
    stream_page(StatusCode::OK, stream)
}

async fn legal_page(
    state: Arc<AppState>,
    headers: HeaderMap,
    peer: IpAddr,
    locale: &'static str,
    uri_path: String,
    path_query: String,
    title_key: &'static str,
    subtitle_key: &'static str,
    legal_html: String,
    head: String,
) -> Response<Body> {
    let cx = Cx::new(locale);
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (shell, data) = content_shell(&state, &headers, peer, locale, &path_query, &uri_path).await;
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
            ContentTpl {
                shell,
                flags_html: flags_html(data.offline, data.ban.banned),
                card_id: "content-card".to_owned(),
                header_id: "content-card-header".to_owned(),
                actions_id: "content-card-actions".to_owned(),
                title: cx.t(title_key),
                subtitle: cx.t(subtitle_key),
                footer_commit: metadata::commit_label(),
                footer_message: "juicebox2-epsilon".to_owned(),
                footer_repo: metadata::REPO_URL.to_owned(),
                actions: content_actions(locale, &cx),
                body_html: legal_html,
            }
            .render()
            .unwrap_or_default(),
        ));
    };
    stream_page(StatusCode::OK, stream)
}

pub async fn terms_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("terms.title")),
            cx.t("terms.subtitle"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    legal_page(
        state,
        headers,
        peer.ip(),
        locale,
        uri_path,
        path_query,
        "terms.title",
        "terms.subtitle",
        legal::TERMS_HTML.clone(),
        head,
    )
    .await
}

pub async fn privacy_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let Some(locale) = locale_from_path(locale_path) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let cx = Cx::new(locale);
    let stripped = i18n::split_locale(uri.path()).1;
    let path_query = uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str)
        .to_owned();
    let uri_path = uri.path().to_owned();
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("privacy.title")),
            cx.t("privacy.subtitle"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    legal_page(
        state,
        headers,
        peer.ip(),
        locale,
        uri_path,
        path_query,
        "privacy.title",
        "privacy.subtitle",
        legal::PRIVACY_HTML.clone(),
        head,
    )
    .await
}

#[derive(Template)]
#[template(path = "notfound.html")]
struct NotFoundTpl {
    shell: Shell,
    flags_html: String,
}

pub async fn not_found_page(
    state: Arc<AppState>,
    headers: HeaderMap,
    peer: IpAddr,
    uri_path: String,
) -> Response<Body> {
    let (locale, stripped) = i18n::split_locale(&uri_path);
    let locale = if locale == "en" || i18n::is_valid_locale(locale) {
        locale
    } else {
        "en"
    };
    let cx = Cx::new(locale);
    let head = HeadTpl {
        head: head_ctx(
            locale,
            format!("{} - Juicebox", cx.t("not_found.title")),
            cx.t("not_found.description"),
            "content-page",
            &stripped,
            state.live_reload,
        ),
    }
    .render()
    .unwrap_or_default();
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let data = shared(&state, &headers, peer).await;
        let shell = shell(
            locale,
            modals_html(locale, &absolute_url(&headers, &uri_path), &uri_path, &data.eff),
            ads_html(locale, &headers),
            no_extra(),
        );
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
            NotFoundTpl {
                shell,
                flags_html: flags_html(data.offline, data.ban.banned),
            }
            .render()
            .unwrap_or_default(),
        ));
    };
    stream_page(StatusCode::NOT_FOUND, stream)
}

#[derive(Template)]
#[template(path = "admin_index.html")]
struct AdminIndexTpl {
    title: String,
    scripts: Vec<String>,
}

#[derive(Template)]
#[template(path = "admin_login.html")]
struct AdminLoginTpl {
    title: String,
    scripts: Vec<String>,
}

#[derive(Template)]
#[template(path = "admin_files.html")]
struct AdminFilesTpl {
    title: String,
    scripts: Vec<String>,
}

#[derive(Template)]
#[template(path = "admin_bans.html")]
struct AdminBansTpl {
    title: String,
    scripts: Vec<String>,
}

#[derive(Template)]
#[template(path = "admin_reports.html")]
struct AdminReportsTpl {
    title: String,
    scripts: Vec<String>,
}

#[derive(Template)]
#[template(path = "admin_feedback.html")]
struct AdminFeedbackTpl {
    title: String,
    scripts: Vec<String>,
}

#[derive(Template)]
#[template(path = "admin_announcement.html")]
struct AdminAnnouncementTpl {
    title: String,
    scripts: Vec<String>,
}

fn admin_html(status: StatusCode, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(body))
        .unwrap_or_default()
}

pub async fn admin_index() -> Response<Body> {
    admin_html(
        StatusCode::OK,
        AdminIndexTpl {
            title: "Admin Panel - Juicebox".to_owned(),
            scripts: vec!["/static/js/admin-index.js".to_owned()],
        }
        .render()
        .unwrap_or_default(),
    )
}

pub async fn admin_login() -> Response<Body> {
    admin_html(
        StatusCode::OK,
        AdminLoginTpl {
            title: "Admin Login - Juicebox".to_owned(),
            scripts: vec!["/static/js/admin-login.js".to_owned()],
        }
        .render()
        .unwrap_or_default(),
    )
}

pub async fn admin_files() -> Response<Body> {
    admin_html(
        StatusCode::OK,
        AdminFilesTpl {
            title: "Uploaded Files - Admin - Juicebox".to_owned(),
            scripts: vec![
                "/static/js/format.js".to_owned(),
                "/static/js/admin-files.js".to_owned(),
            ],
        }
        .render()
        .unwrap_or_default(),
    )
}

pub async fn admin_bans() -> Response<Body> {
    admin_html(
        StatusCode::OK,
        AdminBansTpl {
            title: "Banned IPs - Admin - Juicebox".to_owned(),
            scripts: vec!["/static/js/admin-bans.js".to_owned()],
        }
        .render()
        .unwrap_or_default(),
    )
}

pub async fn admin_reports() -> Response<Body> {
    admin_html(
        StatusCode::OK,
        AdminReportsTpl {
            title: "Reports - Admin - Juicebox".to_owned(),
            scripts: vec!["/static/js/admin-reports.js".to_owned()],
        }
        .render()
        .unwrap_or_default(),
    )
}

pub async fn admin_feedback() -> Response<Body> {
    admin_html(
        StatusCode::OK,
        AdminFeedbackTpl {
            title: "Feedback - Admin - Juicebox".to_owned(),
            scripts: vec!["/static/js/admin-feedback.js".to_owned()],
        }
        .render()
        .unwrap_or_default(),
    )
}

pub async fn admin_announcement() -> Response<Body> {
    admin_html(
        StatusCode::OK,
        AdminAnnouncementTpl {
            title: "Announcement - Admin - Juicebox".to_owned(),
            scripts: vec!["/static/js/admin-announcement.js".to_owned()],
        }
        .render()
        .unwrap_or_default(),
    )
}
