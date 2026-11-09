use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use askama::Template;
use axum::{
    body::Body,
    extract::{ConnectInfo, Json, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::StreamExt;
use serde::Deserialize;

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
    pub asset_version: &'static str,
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

fn hreflang_links_absolute(headers: &HeaderMap, stripped: &str) -> Vec<Hreflang> {
    i18n::LOCALES
        .iter()
        .copied()
        .map(|(code, _)| {
            let href = if code == i18n::DEFAULT_LOCALE {
                stripped.to_owned()
            } else if stripped == "/" {
                format!("/{code}")
            } else {
                format!("/{code}{stripped}")
            };
            Hreflang {
                code,
                href: absolute_url(headers, &href),
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
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
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

const HEALTH_CACHE_TTL: Duration = Duration::from_secs(1);
const BAN_CACHE_TTL: Duration = Duration::from_secs(10);
const BAN_CACHE_MAX_ENTRIES: usize = 4096;

async fn cached_backend_offline(state: &AppState) -> bool {
    if let Ok(cache) = state.health_cache.lock()
        && let Some((at, healthy)) = *cache
        && at.elapsed() < HEALTH_CACHE_TTL
    {
        return !healthy;
    }
    let healthy = health::check_backend_health(&state.http, &state.config.juiceback_url).await;
    if let Ok(mut cache) = state.health_cache.lock() {
        *cache = Some((Instant::now(), healthy));
    }
    !healthy
}

pub(crate) async fn cached_ban(
    state: &AppState,
    headers: &HeaderMap,
    peer: IpAddr,
) -> ban::BanStatus {
    let ip = juiceutils::proxy::client_ip(headers, peer, &state.config.trusted_proxies);
    if let Ok(cache) = state.ban_cache.lock()
        && let Some((at, status)) = cache.get(&ip)
        && at.elapsed() < BAN_CACHE_TTL
    {
        return status.clone();
    }
    let status = ban::check_user_ban(
        &state.http,
        &state.config.juiceback_url,
        headers,
        peer,
        &state.config.trusted_proxies,
    )
    .await;
    if let Ok(mut cache) = state.ban_cache.lock() {
        if cache.len() >= BAN_CACHE_MAX_ENTRIES {
            cache.retain(|_, (at, _)| at.elapsed() < BAN_CACHE_TTL);
            if cache.len() >= BAN_CACHE_MAX_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(ip, (Instant::now(), status.clone()));
    }
    status
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
        cached_backend_offline(state),
        cached_ban(state, headers, peer),
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
    asset_version: &'static str,
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
        .copied()
        .map(|(code, label)| {
            let href = if code == "en" {
                base_path.clone()
            } else if base_path == "/" {
                format!("/{code}")
            } else {
                format!("/{code}{base_path}")
            };
            LangLink {
                code,
                label: label.to_owned(),
                href,
                active: code == locale,
            }
        })
        .collect();
    ModalsTpl {
        cx: cx.clone(),
        flags: ServerFlags {
            quick_link: config.quick_link.unwrap_or(true),
            quic: config.quic.unwrap_or(true),
            custom_id: config.custom_id.unwrap_or(true),
            cobalt: config.cobalt.unwrap_or(false),
        },
        upload_mode: config
            .upload_mode
            .clone()
            .unwrap_or_else(|| "standard".to_owned()),
        danger_level: danger.to_owned(),
        max_bytes: config
            .max_file_size_bytes
            .unwrap_or(juiceutils::config::defaults::MAX_FILE_SIZE_BYTES),
        allowed_ttl: serde_json::to_string(
            &config
                .allowed_ttl_hours
                .clone()
                .unwrap_or_else(|| retention::DEFAULT_ALLOWED_TTL_HOURS.to_vec()),
        )
        .unwrap_or_else(|_| "[]".to_string()),
        default_ttl: config
            .default_ttl_hours
            .unwrap_or(retention::DEFAULT_TTL_HOURS)
            .to_string(),
        asset_version: env!("JUICEFRONT_ASSET_VERSION"),
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
        locale_codes_json: serde_json::to_string(
            &i18n::LOCALES
                .iter()
                .map(|(code, _)| *code)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_owned()),
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
            for holder in *holders {
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
        asset_version: env!("JUICEFRONT_ASSET_VERSION"),
    }
}

struct HeadCtx {
    cx: Cx,
    lang: &'static str,
    title: String,
    description: String,
    canonical: String,
    og_image: String,
    og_image_alt: String,
    og_locale: &'static str,
    body_class: &'static str,
    commit_label: String,
    structured_data: String,
    current_path: String,
    hreflang: Vec<Hreflang>,
    live_reload: bool,
    asset_version: &'static str,
}

#[derive(Template)]
#[template(path = "shell_head.html")]
struct HeadTpl {
    head: HeadCtx,
}

fn og_locale_for(locale: &'static str) -> &'static str {
    match locale {
        "fr" => "fr_FR",
        "ru" => "ru_RU",
        "es" => "es_ES",
        "ja" => "ja_JP",
        _ => "en_US",
    }
}

fn head_ctx(
    locale: &'static str,
    title: String,
    description: String,
    body_class: &'static str,
    stripped_path: &str,
    full_path: &str,
    headers: &HeaderMap,
    live_reload: bool,
) -> HeadCtx {
    let canonical = absolute_url(headers, full_path);
    HeadCtx {
        cx: Cx::new(locale),
        lang: locale,
        og_image_alt: title.clone(),
        og_locale: og_locale_for(locale),
        title,
        description,
        canonical,
        og_image: absolute_url(headers, "/static/assets/og-image.png?v=2"),
        body_class,
        commit_label: metadata::commit_label(),
        structured_data: metadata::structured_data_json(),
        current_path: absolute_url(headers, stripped_path),
        hreflang: hreflang_links_absolute(headers, stripped_path),
        live_reload,
        asset_version: env!("JUICEFRONT_ASSET_VERSION"),
    }
}

struct PageCtx {
    locale: &'static str,
    cx: Cx,
    stripped: String,
    uri_path: String,
    path_query: String,
}

impl PageCtx {
    fn resolve(locale_path: Option<Path<String>>, uri: &axum::http::Uri) -> Option<Self> {
        let locale = locale_from_path(locale_path)?;
        Some(Self {
            locale,
            cx: Cx::new(locale),
            stripped: i18n::split_locale(uri.path()).1,
            uri_path: uri.path().to_owned(),
            path_query: uri
                .path_and_query()
                .map_or("/", axum::http::uri::PathAndQuery::as_str)
                .to_owned(),
        })
    }

    fn from_uri_path(uri_path: String) -> Self {
        let (locale, stripped) = i18n::split_locale(&uri_path);
        let locale = if locale == i18n::DEFAULT_LOCALE || i18n::is_valid_locale(locale) {
            locale
        } else {
            i18n::DEFAULT_LOCALE
        };
        Self {
            locale,
            cx: Cx::new(locale),
            stripped,
            path_query: uri_path.clone(),
            uri_path,
        }
    }

    fn head(
        &self,
        state: &AppState,
        headers: &HeaderMap,
        title: String,
        description: String,
        body_class: &'static str,
    ) -> String {
        HeadTpl {
            head: head_ctx(
                self.locale,
                title,
                description,
                body_class,
                &self.stripped,
                &self.uri_path,
                headers,
                state.live_reload,
            ),
        }
        .render()
        .unwrap_or_default()
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

async fn stream_page(
    status: StatusCode,
    stream: impl futures::Stream<Item = Result<bytes::Bytes, std::convert::Infallible>> + Send + 'static,
) -> Response<Body> {
    let mut stream = Box::pin(stream);
    let mut body = Vec::with_capacity(128 * 1024);
    while let Some(Ok(chunk)) = stream.next().await {
        body.extend_from_slice(&chunk);
    }
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "private, max-age=10")
        .header("vary", "Cookie")
        .body(Body::from(body))
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
    include_str!("../static/styles/components/QrModal.css"),
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
    include_str!("../static/styles/components/SettingsTabs.css"),
    "\n",
    include_str!("../static/styles/components/FullTab.css"),
    "\n",
    include_str!("../static/styles/components/NetDebug.css"),
    "\n",
    include_str!("../static/styles/components/DocsPage.css"),
);

struct CssAssets {
    identity: Bytes,
    gzip: Bytes,
    br: Bytes,
    zstd: Bytes,
    etag: String,
}

static CSS_ASSETS: std::sync::OnceLock<CssAssets> = std::sync::OnceLock::new();

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn css_assets() -> &'static CssAssets {
    CSS_ASSETS.get_or_init(|| {
        use std::io::Write as _;
        let source = CSS_BUNDLE.as_bytes();
        let gzip = (|| -> std::io::Result<Vec<u8>> {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
            encoder.write_all(source)?;
            encoder.finish()
        })()
        .unwrap_or_else(|_| source.to_vec());
        let br = (|| -> std::io::Result<Vec<u8>> {
            let mut writer = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
            writer.write_all(source)?;
            Ok(writer.into_inner())
        })()
        .unwrap_or_else(|_| source.to_vec());
        let zstd = (|| -> std::io::Result<Vec<u8>> {
            let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3)?;
            encoder.write_all(source)?;
            encoder.finish()
        })()
        .unwrap_or_else(|_| source.to_vec());
        CssAssets {
            identity: Bytes::from_static(source),
            gzip: gzip.into(),
            br: br.into(),
            zstd: zstd.into(),
            etag: format!("\"{:016x}\"", fnv1a(source)),
        }
    })
}

pub async fn css_bundle(headers: HeaderMap, uri: axum::http::Uri) -> Response<Body> {
    let assets = css_assets();
    let accept = headers
        .get("accept-encoding")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (body, encoding) = if accept.contains("zstd") {
        (&assets.zstd, Some("zstd"))
    } else if accept.contains("br") {
        (&assets.br, Some("br"))
    } else if accept.contains("gzip") {
        (&assets.gzip, Some("gzip"))
    } else {
        (&assets.identity, None)
    };
    let versioned = uri
        .query()
        .is_some_and(|query| query.split('&').any(|part| part.starts_with("v=")));
    let cache_control = if versioned {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/css; charset=utf-8")
        .header("cache-control", cache_control)
        .header("etag", assets.etag.as_str())
        .header("vary", "Accept-Encoding");
    if let Some(encoding) = encoding {
        builder = builder.header("content-encoding", encoding);
    }
    builder.body(Body::from(body.clone())).unwrap_or_default()
}

fn plain_text_response(body: &'static str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; charset=utf-8")
        .header("cache-control", "no-cache")
        .body(Body::from(body))
        .unwrap_or_default()
}

pub async fn robots_txt() -> Response<Body> {
    plain_text_response(include_str!("../static/robots.txt"))
}

pub async fn agents_txt() -> Response<Body> {
    plain_text_response(include_str!("../static/agents.txt"))
}

fn locale_from_path(path: Option<Path<String>>) -> Option<&'static str> {
    match path {
        Some(Path(code)) => {
            if code == i18n::DEFAULT_LOCALE || !i18n::is_valid_locale(&code) {
                return None;
            }
            i18n::LOCALES
                .iter()
                .find(|(locale, _)| *locale == code)
                .map(|(locale, _)| *locale)
        }
        None => Some(i18n::DEFAULT_LOCALE),
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
    kind: String,
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
    divider_before: bool,
}

struct UploadedFile {
    id: String,
    filename: String,
    icon: String,
    size_str: String,
    url: String,
    raw_url: String,
    download_url: String,
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

#[allow(clippy::cast_possible_truncation)]
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

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
    let size = json_num(&value, "size_bytes") as u64;
    Some(UploadedFile {
        id: json_str(&value, "id", 50),
        filename: json_str(&value, "filename", 500),
        icon: format::icon_for_mime(&mime).to_owned(),
        size_str: format::format_size(size),
        url: json_str(&value, "url", 2048),
        raw_url: json_str(&value, "raw_url", 2048),
        download_url: json_str(&value, "download_url", 2048),
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

pub async fn upload_target_proxy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(router_url) = state
        .config
        .router_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return (StatusCode::SERVICE_UNAVAILABLE, "router not configured").into_response();
    };
    let url = format!("{}/upload-target", router_url.trim_end_matches('/'));
    let mut request = state.http.get(url).timeout(Duration::from_millis(1_500));
    for name in ["cf-ipcountry", "cf-ray", "cf-connecting-ip"] {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    let response = match request.send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::warn!("router upload-target returned {}", response.status());
            return (StatusCode::SERVICE_UNAVAILABLE, "router unavailable").into_response();
        }
        Err(error) => {
            tracing::warn!("router upload-target failed: {error}");
            return (StatusCode::SERVICE_UNAVAILABLE, "router unavailable").into_response();
        }
    };
    let Ok(body) = response.bytes().await else {
        return (StatusCode::BAD_GATEWAY, "router response error").into_response();
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let head = page.head(
        &state,
        &headers,
        page.cx.t("site.title"),
        page.cx.t("site.description_short"),
        "upload-page",
    );
    let PageCtx {
        locale,
        cx,
        uri_path,
        path_query,
        ..
    } = page;
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

        .position(|hours| {
            #[allow(clippy::float_cmp)]
            {
                *hours == default_ttl
            }
        })
        .unwrap_or(0);
    let max_bytes = data
        .eff
        .config
        .max_file_size_bytes
        .unwrap_or(juiceutils::config::defaults::MAX_FILE_SIZE_BYTES);
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
        retention: {
            let mut prev_tier: Option<u8> = None;
            allowed
                .into_iter()
                .enumerate()
                .map(|(index, hours)| {
                    let tier = retention::ttl_tier(hours);
                    let divider_before =
                        prev_tier.is_some_and(|prev| prev != tier);
                    prev_tier = Some(tier);
                    RetentionOpt {
                        hours,
                        label: cx.ttl(hours),
                        checked: index == default_index,
                        divider_before,
                    }
                })
                .collect()
        },
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
            kind: node.kind,
        })
        .collect();
    let unofficial: Vec<HostRow> = hosts::unofficial_nodes()
        .into_iter()
        .map(|node| HostRow {
            selected: node.url == saved,
            url: node.url,
            name: node.name,
            region: node.region,
            kind: node.kind,
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
    stream_page(StatusCode::OK, stream).await
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
    footer_repo: String,
    empty_message: String,
    notice: String,
    actions: Vec<CardAction>,
    has_actions: bool,
    files: Vec<FileRow>,
}

#[derive(Clone)]
struct FileRow {
    id: String,
    filename: String,
    icon: String,
    meta: String,
    size_bytes: u64,
    uploaded_at: i64,
    expires_at: i64,
    url: String,
    raw_url: String,
    download_url: String,
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
    protected: bool,
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

#[derive(Deserialize)]
pub struct RowsPair {
    pub id: String,
    pub token: String,
}

#[derive(Deserialize)]
pub struct RowsRequest {
    #[serde(default)]
    pub pairs: Vec<RowsPair>,
    pub locale: Option<String>,
}

#[derive(Template)]
#[template(path = "file_rows.html")]
struct FileRowsTpl {
    cx: Cx,
    files: Vec<FileRow>,
}

#[derive(Template)]
#[template(path = "file_modals.html")]
struct FileModalsTpl {
    cx: Cx,
    files: Vec<FileRow>,
}

#[allow(clippy::too_many_arguments)]
fn file_rows(
    cx: &Cx,
    locale: &'static str,
    files: Vec<ServerFile>,
    default_host: &String,
    custom_id: bool,
    timestamp: i64,
    start_index: usize,
) -> Vec<FileRow> {
    files
        .into_iter()
        .enumerate()
        .map(|(offset, file)| {
            let index = start_index + offset;
            let host = file.storage_host.clone().unwrap_or_default();

            FileRow {
                icon: cx.mime_icon(&file.mime_type),
                meta: format!("{} - {}", cx.fmt_size(file.size_bytes), file.mime_type),
                show_host: !host.is_empty()
                    && !format::is_default_host(
                        &host,
                        Some(default_host)
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
                raw_url: file.raw_url,
                download_url: file.download_url,
                delete_token: file.delete_token,
                storage_host: host,
                index,
                protected: file.protected,
            }
        })
        .collect()
}

pub async fn files_rows_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<RowsRequest>,
) -> Response<Body> {
    let locale: &'static str = body.locale.as_deref().map_or(i18n::DEFAULT_LOCALE, |code| {
        i18n::LOCALES
            .iter()
            .find(|(locale, _)| *locale == code)
            .map_or(i18n::DEFAULT_LOCALE, |(locale, _)| *locale)
    });
    let cx = Cx::new(locale);
    let data = shared(&state, &headers, peer.ip()).await;
    let pairs: Vec<serde_json::Value> = body
        .pairs
        .into_iter()
        .take(500)
        .filter(|pair| !pair.id.trim().is_empty() && !pair.token.trim().is_empty())
        .map(|pair| serde_json::json!({ "id": pair.id, "token": pair.token }))
        .collect();
    let Ok(files) = owned_files_for_pairs(&state, pairs).await else {
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "application/json; charset=utf-8")
            .body(Body::from(r#"{"error":"upstream unavailable"}"#))
            .unwrap_or_default();
    };
    let default_host = data.eff.config.public_base_url.clone().unwrap_or_default();
    let custom_id = data.eff.config.custom_id.unwrap_or(true);
    let rows = file_rows(&cx, locale, files, &default_host, custom_id, now_ms(), 0);
    let cards = FileRowsTpl {
        cx: cx.clone(),
        files: rows.clone(),
    }
    .render()
    .unwrap_or_default();
    let modals = FileModalsTpl { cx, files: rows }
        .render()
        .unwrap_or_default();
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "private, no-store")
        .body(Body::from(
            serde_json::json!({ "cards": cards, "modals": modals }).to_string(),
        ))
        .unwrap_or_default()
}

async fn server_files(state: &AppState, headers: &HeaderMap) -> Result<Vec<ServerFile>, ()> {
    let Some(encoded) = cookie_value(headers, "jb_files") else {
        return Ok(Vec::new());
    };
    let Some(bytes) = base64url_decode(&encoded) else {
        return Ok(Vec::new());
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
    owned_files_for_pairs(state, pairs).await
}

async fn owned_files_for_pairs(
    state: &AppState,
    pairs: Vec<serde_json::Value>,
) -> Result<Vec<ServerFile>, ()> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let Ok(Some(files)) = tokio::time::timeout(Duration::from_secs(5), async {
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
    else {
        tracing::warn!("owned-files lookup failed; showing the files page without server entries");
        return Err(());
    };
    let now = chrono_now();
    Ok(files
        .get("files")
        .and_then(|data| data.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|file| serde_json::from_value::<ServerFile>(file).ok())
        .filter(|file| file.expires_at > now)
        .collect())
}

#[allow(clippy::cast_possible_wrap)]
fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
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
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t("files.title")),
        page.cx.t("files.subtitle"),
        "files-page",
    );
    let PageCtx {
        locale,
        cx,
        uri_path,
        path_query,
        ..
    } = page;
    let query = query_map(&uri);
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let data = shared(&state, &headers, peer.ip()).await;
        let mut notice = query
            .get("deleted")
            .filter(|value| !value.is_empty())
            .map(|_| cx.t("sr.file_deleted"))
            .or_else(|| {
                query
                    .get("renamed")
                    .filter(|value| value.as_str() == "1")
                    .map(|_| cx.t("files.rename_success"))
            })
            .unwrap_or_default();

    let server_files = if data.offline {
        Vec::new()
    } else if let Ok(files) = server_files(&state, &headers).await {
        files
    } else {
        notice = cx.t("files.offline");
        Vec::new()
    };
    let default_host = data.eff.config.public_base_url.clone().unwrap_or_default();
    let custom_id = data.eff.config.custom_id.unwrap_or(true);
    let timestamp = now_ms();
    let mut server_files = server_files;
    server_files.sort_by(|a, b| b.uploaded_at.cmp(&a.uploaded_at));
    let rows: Vec<FileRow> = file_rows(
        &cx,
        locale,
        server_files,
        &default_host,
        custom_id,
        timestamp,
        0,
    );

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
        footer_commit: metadata::commit_short_hash(),
        footer_repo: metadata::REPO_URL.to_owned(),
        empty_message: cx.t("files.fallback_empty"),
        notice,
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
    stream_page(StatusCode::OK, stream).await
}

fn content_actions(cx: &Cx) -> Vec<CardAction> {
    vec![
        CardAction {
            id: "back".to_owned(),
            label: cx.t("nav.back"),
            icon: "back".to_owned(),
            aria: cx.t("nav.back_aria"),
            href: cx.lp("/"),
        },
        card_action(cx.locale, "files", "nav.files", "files", cx.lp("/files")),
        card_action(cx.locale, "more", "nav.more", "more", String::new()),
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
        Vec::new(),
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
    footer_repo: String,
    actions: Vec<CardAction>,
    body_html: String,
}

impl ContentTpl {
    fn new(
        shell: Shell,
        data: &Shared,
        cx: &Cx,
        title: String,
        subtitle: String,
        body_html: String,
    ) -> Self {
        Self {
            shell,
            flags_html: flags_html(data.offline, data.ban.banned),
            card_id: "content-card".to_owned(),
            header_id: "content-card-header".to_owned(),
            actions_id: "content-card-actions".to_owned(),
            title,
            subtitle,
            footer_commit: metadata::commit_short_hash(),
            footer_repo: metadata::REPO_URL.to_owned(),
            actions: content_actions(cx),
            body_html,
        }
    }
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
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t("download.title")),
        page.cx.t("download.description"),
        "content-page",
    );
    let PageCtx {
        locale,
        cx,
        uri_path,
        path_query,
        ..
    } = page;
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let ((shell, data), tag) = tokio::join!(
        content_shell(&state, &headers, peer.ip(), locale, &path_query, &uri_path),
        latest_tag(&state),
    );
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
        ContentTpl::new(
            shell,
            &data,
            &cx,
            "Download Juicebox+".to_owned(),
            "The desktop app for paired-device uploads".to_owned(),
            body_html,
        )
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream).await
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
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let ban = cached_ban(&state, &headers, peer.ip()).await;
    if !ban.banned {
        return redirect_to(&page.cx.lp("/"));
    }
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t("banned.title")),
        page.cx.t("banned.description"),
        "content-page",
    );
    let PageCtx {
        locale,
        uri_path,
        path_query,
        ..
    } = page;
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
    stream_page(StatusCode::OK, stream).await
}

#[derive(Template)]
#[template(path = "form_card.html")]
struct FormCardTpl {
    shell: Shell,
    flags_html: String,
    card_id: &'static str,
    title: String,
    subtitle: String,
    submitted: bool,
    footer_commit: String,
    footer_repo: String,
    actions: Vec<CardAction>,
    success_key: &'static str,
    actions_key: &'static str,
    report: bool,
}

#[derive(Clone, Copy)]
struct FormKind {
    card_id: &'static str,
    title_key: &'static str,
    subtitle_key: &'static str,
    body_class: &'static str,
    success_key: &'static str,
    actions_key: &'static str,
    report: bool,
}

const REPORT_FORM: FormKind = FormKind {
    card_id: "report-card",
    title_key: "report.title",
    subtitle_key: "report.subtitle",
    body_class: "report-page",
    success_key: "report.success",
    actions_key: "actions.report_actions",
    report: true,
};

const FEEDBACK_FORM: FormKind = FormKind {
    card_id: "feedback-card",
    title_key: "feedback.title",
    subtitle_key: "feedback.subtitle",
    body_class: "content-page",
    success_key: "feedback.success",
    actions_key: "actions.feedback_actions",
    report: false,
};

async fn form_page(
    state: Arc<AppState>,
    peer: std::net::SocketAddr,
    locale_path: Option<Path<String>>,
    query: HashMap<String, String>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    kind: FormKind,
) -> Response<Body> {
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t(kind.title_key)),
        page.cx.t(kind.subtitle_key),
        kind.body_class,
    );
    let PageCtx {
        locale,
        cx,
        uri_path,
        path_query,
        ..
    } = page;
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let data = shared(&state, &headers, peer.ip()).await;
        let shell = shell(
            locale,
            modals_html(locale, &absolute_url(&headers, &path_query), &uri_path, &data.eff),
            ads_html(locale, &headers),
            Vec::new(),
        );
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
            FormCardTpl {
                shell,
                flags_html: flags_html(data.offline, data.ban.banned),
                card_id: kind.card_id,
                title: cx.t(kind.title_key),
                subtitle: cx.t(kind.subtitle_key),
                submitted: query.get("submitted").is_some_and(|value| value == "1"),
                footer_commit: metadata::commit_short_hash(),
                footer_repo: metadata::REPO_URL.to_owned(),
                actions: content_actions(&cx),
                success_key: kind.success_key,
                actions_key: kind.actions_key,
                report: kind.report,
            }
            .render()
            .unwrap_or_default(),
        ));
    };
    stream_page(StatusCode::OK, stream).await
}

pub async fn report_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    form_page(state, peer, locale_path, query, headers, uri, REPORT_FORM).await
}

pub async fn feedback_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    form_page(state, peer, locale_path, query, headers, uri, FEEDBACK_FORM).await
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
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t("faq.title")),
        page.cx.t("faq.subtitle"),
        "content-page",
    );
    let PageCtx {
        locale,
        cx,
        uri_path,
        path_query,
        ..
    } = page;
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
        ContentTpl::new(
            shell,
            &data,
            &cx,
            cx.t("faq.title"),
            cx.t("faq.subtitle"),
            body_html,
        )
        .render()
        .unwrap_or_default(),
    ));
    };
    stream_page(StatusCode::OK, stream).await
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
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t("docs.title")),
        page.cx.t("docs.description"),
        "content-page",
    );
    let PageCtx {
        locale,
        cx,
        uri_path,
        path_query,
        ..
    } = page;
    let rustdoc_exists = state.rustdoc_exists;
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (shell, data) = content_shell(&state, &headers, peer.ip(), locale, &path_query, &uri_path).await;
        let body_html = DocsBodyTpl { rustdoc_exists }
        .render()
        .unwrap_or_default();
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
            ContentTpl::new(
                shell,
                &data,
                &cx,
                cx.t("docs.title"),
                cx.t("docs.description"),
                body_html,
            )
            .render()
            .unwrap_or_default(),
        ));
    };
    stream_page(StatusCode::OK, stream).await
}

#[derive(Clone, Copy)]
struct LegalDoc {
    title_key: &'static str,
    subtitle_key: &'static str,
    body: &'static str,
}

async fn legal_page(
    state: Arc<AppState>,
    peer: std::net::SocketAddr,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    doc: LegalDoc,
) -> Response<Body> {
    let Some(page) = PageCtx::resolve(locale_path, &uri) else {
        return not_found_page(state, headers, peer.ip(), uri.path().to_owned()).await;
    };
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t(doc.title_key)),
        page.cx.t(doc.subtitle_key),
        "content-page",
    );
    let PageCtx {
        locale,
        cx,
        uri_path,
        path_query,
        ..
    } = page;
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let (shell, data) = content_shell(&state, &headers, peer.ip(), locale, &path_query, &uri_path).await;
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
            ContentTpl::new(
                shell,
                &data,
                &cx,
                cx.t(doc.title_key),
                cx.t(doc.subtitle_key),
                doc.body.to_owned(),
            )
            .render()
            .unwrap_or_default(),
        ));
    };
    stream_page(StatusCode::OK, stream).await
}

pub async fn terms_page(
    State(state): axum::extract::State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    locale_path: Option<Path<String>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    legal_page(
        state,
        peer,
        locale_path,
        headers,
        uri,
        LegalDoc {
            title_key: "terms.title",
            subtitle_key: "terms.subtitle",
            body: legal::TERMS_HTML.as_str(),
        },
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
    legal_page(
        state,
        peer,
        locale_path,
        headers,
        uri,
        LegalDoc {
            title_key: "privacy.title",
            subtitle_key: "privacy.subtitle",
            body: legal::PRIVACY_HTML.as_str(),
        },
    )
    .await
}

#[derive(Template)]
#[template(path = "notfound.html")]
struct NotFoundTpl {
    shell: Shell,
    flags_html: String,
}

#[allow(clippy::unused_async)]
pub async fn not_found_page(
    state: Arc<AppState>,
    headers: HeaderMap,
    peer: IpAddr,
    uri_path: String,
) -> Response<Body> {
    let page = PageCtx::from_uri_path(uri_path);
    let head = page.head(
        &state,
        &headers,
        format!("{} - Juicebox", page.cx.t("not_found.title")),
        page.cx.t("not_found.description"),
        "content-page",
    );
    let PageCtx {
        locale,
        uri_path,
        path_query,
        ..
    } = page;
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(head));
        let data = shared(&state, &headers, peer).await;
        let shell = shell(
            locale,
            modals_html(locale, &absolute_url(&headers, &path_query), &uri_path, &data.eff),
            ads_html(locale, &headers),
            Vec::new(),
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
    stream_page(StatusCode::NOT_FOUND, stream).await
}

#[derive(Template)]
#[template(path = "admin_page.html")]
struct AdminTpl {
    page: &'static str,
    title: String,
    scripts: Vec<String>,
}

#[derive(Clone, Copy)]
struct AdminPage {
    page: &'static str,
    title: &'static str,
    scripts: &'static [&'static str],
}

const ADMIN_INDEX: AdminPage = AdminPage {
    page: "index",
    title: "Admin Panel - Juicebox",
    scripts: &["/static/js/admin-index.js"],
};

const ADMIN_LOGIN: AdminPage = AdminPage {
    page: "login",
    title: "Admin Login - Juicebox",
    scripts: &["/static/js/admin-login.js"],
};

const ADMIN_FILES: AdminPage = AdminPage {
    page: "files",
    title: "Uploaded Files - Admin - Juicebox",
    scripts: &["/static/js/admin-files.js"],
};

const ADMIN_BANS: AdminPage = AdminPage {
    page: "bans",
    title: "Banned IPs - Admin - Juicebox",
    scripts: &["/static/js/admin-bans.js"],
};

const ADMIN_HOSTERS: AdminPage = AdminPage {
    page: "hosters",
    title: "Hosters - Admin - Juicebox",
    scripts: &["/static/js/admin-hosters.js"],
};

const ADMIN_REPORTS: AdminPage = AdminPage {
    page: "reports",
    title: "Reports - Admin - Juicebox",
    scripts: &["/static/js/admin-reports.js"],
};

const ADMIN_FEEDBACK: AdminPage = AdminPage {
    page: "feedback",
    title: "Feedback - Admin - Juicebox",
    scripts: &["/static/js/admin-feedback.js"],
};

const ADMIN_ANNOUNCEMENT: AdminPage = AdminPage {
    page: "announcement",
    title: "Announcement - Admin - Juicebox",
    scripts: &["/static/js/admin-announcement.js"],
};

fn admin_html(status: StatusCode, page: AdminPage) -> Response<Body> {
    let body = AdminTpl {
        page: page.page,
        title: page.title.to_owned(),
        scripts: page
            .scripts
            .iter()
            .map(|script| (*script).to_owned())
            .collect(),
    }
    .render()
    .unwrap_or_default();
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(body))
        .unwrap_or_default()
}

pub async fn admin_index() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_INDEX)
}

pub async fn admin_login() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_LOGIN)
}

pub async fn admin_files() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_FILES)
}

pub async fn admin_bans() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_BANS)
}

pub async fn admin_hosters() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_HOSTERS)
}

pub async fn admin_reports() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_REPORTS)
}

pub async fn admin_feedback() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_FEEDBACK)
}

pub async fn admin_announcement() -> Response<Body> {
    admin_html(StatusCode::OK, ADMIN_ANNOUNCEMENT)
}
