use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct FileConfig {
    #[serde(default)]
    pub server: ServerFile,
    #[serde(default)]
    pub urls: UrlsFile,
    #[serde(default)]
    pub cors: CorsFile,
    #[serde(default)]
    pub report: ReportFile,
    #[serde(default)]
    pub cloudflare: CloudflareFile,
    #[serde(default)]
    pub cobalt: CobaltFile,
    #[serde(default)]
    pub dte: DteFile,
    #[serde(default)]
    pub features: FeaturesFile,
    #[serde(default)]
    pub regions: RegionsFile,
    #[serde(default)]
    pub r2: R2File,
    #[serde(default)]
    pub promo: PromoFile,
    #[serde(default)]
    pub sentry: juiceutils::config::SentrySettings,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerFile {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Defaults to `port` plus `QUIC_OFFSET` when unset.
    #[serde(default)]
    pub quic_port: Option<u16>,
    #[serde(default = "default_database_path")]
    pub database_path: String,
    #[serde(default = "default_rate_limit_per_minute")]
    pub rate_limit_per_minute: u32,
    #[serde(default = "default_db_pool_size")]
    pub db_pool_size: u32,
    #[serde(default = "default_max_concurrent_uploads")]
    pub max_concurrent_uploads: u32,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_cleanup_interval_minutes")]
    pub cleanup_interval_minutes: u64,

    #[serde(default)]
    pub quic_cert_path: Option<PathBuf>,

    #[serde(default)]
    pub trusted_proxy_cidrs: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UrlsFile {
    #[serde(default = "default_public_base_url")]
    pub public_base_url: String,
    #[serde(default = "default_juicehost_url")]
    pub juicehost_url: String,
    #[serde(default)]
    pub public_juicehost_url: Option<String>,
    #[serde(default)]
    pub default_storage_host: Option<String>,
    #[serde(default)]
    pub juiceback_origin: Option<String>,
    #[serde(default)]
    pub router_base_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CorsFile {
    #[serde(default = "default_cors_origins")]
    pub origins: Vec<String>,
}

impl Default for CorsFile {
    fn default() -> Self {
        Self {
            origins: default_cors_origins(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReportFile {
    #[serde(default = "default_report_retention_days")]
    pub retention_days: u64,
    #[serde(default = "default_report_retention_days")]
    pub feedback_retention_days: u64,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub smtp_host: Option<String>,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: Option<u16>,
    #[serde(default)]
    pub smtp_username: Option<String>,
    #[serde(default)]
    pub email_recipient: Option<String>,
    #[serde(default)]
    pub email_sender: Option<String>,
}

impl Default for ReportFile {
    fn default() -> Self {
        Self {
            retention_days: default_report_retention_days(),
            feedback_retention_days: default_report_retention_days(),
            webhook_url: None,
            smtp_host: None,
            smtp_port: default_smtp_port(),
            smtp_username: None,
            email_recipient: None,
            email_sender: None,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct CloudflareFile {
    #[serde(default)]
    pub zone_id: Option<String>,
}

/// Each entry is a separate instance, not a fallback chain.
#[derive(Debug, Deserialize, Serialize)]
pub struct CobaltFile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cobalt_api_url")]
    pub api_url: String,

    #[serde(default)]
    pub api_urls: Vec<String>,
    #[serde(default)]
    pub session_api_url: Option<String>,

    /// Session-authenticated instances; require `COBALT_SESSION_API_KEY`.
    #[serde(default)]
    pub session_api_urls: Vec<String>,
    #[serde(default = "default_fetch_empty_retry_delay")]
    pub fetch_empty_retry_delay_secs: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DteFile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_dte_assumed_bps")]
    pub assumed_bps: f64,
    #[serde(default = "default_dte_safety_mult")]
    pub safety_mult: f64,
    #[serde(default = "default_dte_base_overhead")]
    pub base_overhead_secs: i64,
    #[serde(default = "default_dte_min_ttl")]
    pub min_ttl_secs: i64,
    #[serde(default = "default_dte_max_ttl")]
    pub max_ttl_secs: i64,
    #[serde(default = "default_dte_mint_limit")]
    pub mint_limit: u32,
    #[serde(default = "default_dte_mint_window")]
    pub mint_window_secs: u64,
    #[serde(default = "default_dte_mint_burst")]
    pub mint_burst: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FeaturesFile {
    #[serde(default = "default_secure_cookies")]
    pub secure_cookies: bool,
    #[serde(default)]
    pub direct_upload_enabled: bool,

    #[serde(default)]
    pub allow_private_fetch: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RegionsFile {
    #[serde(default)]
    pub public_juicehosts: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct R2File {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub public_base_url: Option<String>,
    #[serde(default = "default_r2_max_bytes")]
    pub max_bytes: u64,
    #[serde(default = "default_r2_max_hot_files")]
    pub max_hot_files: u64,
    #[serde(default = "default_r2_max_promote_bytes")]
    pub max_promote_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PromoFile {
    #[serde(default = "default_promo_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_promo_cool_after_secs")]
    pub cool_after_secs: i64,
    #[serde(default = "default_promo_min_views")]
    pub min_views: u64,
    #[serde(default = "default_promo_min_views_per_min")]
    pub min_views_per_min: f64,
    #[serde(default = "default_promo_min_ttl_remaining_secs")]
    pub min_ttl_remaining_secs: i64,
    #[serde(default = "default_promo_dry_run")]
    pub dry_run: bool,
    #[serde(default = "default_embed_enabled")]
    pub embed_enabled: bool,
    #[serde(default = "default_embed_max_bytes")]
    pub embed_max_bytes: u64,
    #[serde(default = "default_embed_cooldown_secs")]
    pub embed_cooldown_secs: i64,
    #[serde(default = "default_embed_min_ttl_remaining_secs")]
    pub embed_min_ttl_remaining_secs: i64,
}

fn default_host() -> String {
    String::from("127.0.0.1")
}

const fn default_port() -> u16 {
    juiceutils::config::ports::BACKEND
}

fn default_database_path() -> String {
    String::from("./delta.db")
}

const fn default_rate_limit_per_minute() -> u32 {
    10
}

const fn default_db_pool_size() -> u32 {
    8
}

const fn default_max_concurrent_uploads() -> u32 {
    16
}

fn default_log_level() -> String {
    String::from("info")
}

const fn default_cleanup_interval_minutes() -> u64 {
    30
}

fn default_public_base_url() -> String {
    format!("http://localhost:{}", juiceutils::config::ports::HOST)
}

fn default_juicehost_url() -> String {
    format!("http://127.0.0.1:{}", juiceutils::config::ports::HOST)
}

fn default_cors_origins() -> Vec<String> {
    let frontend = juiceutils::config::ports::FRONTEND;
    let host = juiceutils::config::ports::HOST;
    vec![
        format!("http://localhost:{frontend}"),
        format!("http://localhost:{host}"),
        format!("http://127.0.0.1:{host}"),
    ]
}

const fn default_report_retention_days() -> u64 {
    90
}

const fn default_smtp_port() -> Option<u16> {
    Some(465)
}

fn default_cobalt_api_url() -> String {
    String::from("http://localhost:7272")
}

const fn default_fetch_empty_retry_delay() -> u64 {
    crate::constants::FETCH_EMPTY_RETRY_DELAY_SECS
}

fn default_dte_assumed_bps() -> f64 {
    crate::constants::DTE_ASSUMED_BPS
}

fn default_dte_safety_mult() -> f64 {
    crate::constants::DTE_SAFETY_MULT
}

fn default_dte_base_overhead() -> i64 {
    crate::constants::DTE_BASE_OVERHEAD_SECS
}

fn default_dte_min_ttl() -> i64 {
    crate::constants::DTE_MIN_TTL_SECS
}

fn default_dte_max_ttl() -> i64 {
    crate::constants::DTE_MAX_TTL_SECS
}

const fn default_dte_mint_limit() -> u32 {
    crate::constants::DTE_MINT_LIMIT
}

const fn default_dte_mint_window() -> u64 {
    crate::constants::DTE_MINT_WINDOW_SECS
}

const fn default_dte_mint_burst() -> u32 {
    crate::constants::DTE_MINT_BURST
}

const fn default_secure_cookies() -> bool {
    true
}

const fn default_r2_max_bytes() -> u64 {
    10 * 1024 * 1024 * 1024
}

const fn default_r2_max_hot_files() -> u64 {
    500
}

const fn default_r2_max_promote_bytes() -> u64 {
    256 * 1024 * 1024
}

const fn default_promo_interval_secs() -> u64 {
    300
}

const fn default_promo_cool_after_secs() -> i64 {
    30 * 60
}

const fn default_promo_min_views() -> u64 {
    50
}

const fn default_promo_min_views_per_min() -> f64 {
    10.0
}

const fn default_promo_min_ttl_remaining_secs() -> i64 {
    3600
}

const fn default_promo_dry_run() -> bool {
    true
}

const fn default_embed_enabled() -> bool {
    true
}

const fn default_embed_max_bytes() -> u64 {
    25 * 1024 * 1024
}

const fn default_embed_cooldown_secs() -> i64 {
    6 * 3600
}

const fn default_embed_min_ttl_remaining_secs() -> i64 {
    3600
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            server: ServerFile::default(),
            urls: UrlsFile::default(),
            cors: CorsFile::default(),
            report: ReportFile::default(),
            cloudflare: CloudflareFile::default(),
            cobalt: CobaltFile::default(),
            dte: DteFile::default(),
            features: FeaturesFile::default(),
            regions: RegionsFile::default(),
            r2: R2File::default(),
            promo: PromoFile::default(),
            sentry: juiceutils::config::SentrySettings::default(),
        }
    }
}

impl Default for ServerFile {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            quic_port: None,
            database_path: default_database_path(),
            rate_limit_per_minute: default_rate_limit_per_minute(),
            db_pool_size: default_db_pool_size(),
            max_concurrent_uploads: default_max_concurrent_uploads(),
            log_level: default_log_level(),
            cleanup_interval_minutes: default_cleanup_interval_minutes(),
            quic_cert_path: None,
            trusted_proxy_cidrs: Vec::new(),
        }
    }
}

impl Default for UrlsFile {
    fn default() -> Self {
        Self {
            public_base_url: default_public_base_url(),
            juicehost_url: default_juicehost_url(),
            public_juicehost_url: None,
            default_storage_host: None,
            juiceback_origin: None,
            router_base_url: None,
        }
    }
}

impl Default for CobaltFile {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: default_cobalt_api_url(),
            api_urls: Vec::new(),
            session_api_url: None,
            session_api_urls: Vec::new(),
            fetch_empty_retry_delay_secs: default_fetch_empty_retry_delay(),
        }
    }
}

impl Default for DteFile {
    fn default() -> Self {
        Self {
            enabled: false,
            assumed_bps: default_dte_assumed_bps(),
            safety_mult: default_dte_safety_mult(),
            base_overhead_secs: default_dte_base_overhead(),
            min_ttl_secs: default_dte_min_ttl(),
            max_ttl_secs: default_dte_max_ttl(),
            mint_limit: default_dte_mint_limit(),
            mint_window_secs: default_dte_mint_window(),
            mint_burst: default_dte_mint_burst(),
        }
    }
}

impl Default for FeaturesFile {
    fn default() -> Self {
        Self {
            secure_cookies: default_secure_cookies(),
            direct_upload_enabled: false,
            allow_private_fetch: false,
        }
    }
}

impl Default for RegionsFile {
    fn default() -> Self {
        Self {
            public_juicehosts: None,
        }
    }
}

impl Default for R2File {
    fn default() -> Self {
        Self {
            enabled: false,
            bucket: None,
            endpoint: None,
            public_base_url: None,
            max_bytes: default_r2_max_bytes(),
            max_hot_files: default_r2_max_hot_files(),
            max_promote_bytes: default_r2_max_promote_bytes(),
        }
    }
}

impl Default for PromoFile {
    fn default() -> Self {
        Self {
            interval_secs: default_promo_interval_secs(),
            cool_after_secs: default_promo_cool_after_secs(),
            min_views: default_promo_min_views(),
            min_views_per_min: default_promo_min_views_per_min(),
            min_ttl_remaining_secs: default_promo_min_ttl_remaining_secs(),
            dry_run: default_promo_dry_run(),
            embed_enabled: default_embed_enabled(),
            embed_max_bytes: default_embed_max_bytes(),
            embed_cooldown_secs: default_embed_cooldown_secs(),
            embed_min_ttl_remaining_secs: default_embed_min_ttl_remaining_secs(),
        }
    }
}

/// First existing candidate wins; a malformed file is logged and replaced with
/// defaults.
pub(crate) fn load_file_config() -> FileConfig {
    juiceutils::config::load_service_config(
        "juiceback",
        "JUICEBACK_CONFIG",
        &["juiceback.toml", "config.toml"],
    )
}
