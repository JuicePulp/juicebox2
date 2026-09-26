use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

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
    pub sentry: juiceutils::config::SentrySettings,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerFile {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
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
    pub juiceback_origin: Option<String>,
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

#[derive(Debug, Deserialize, Serialize)]
pub struct CobaltFile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cobalt_api_url")]
    pub api_url: String,
    #[serde(default)]
    pub session_api_url: Option<String>,
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

fn default_host() -> String {
    String::from("127.0.0.1")
}

const fn default_port() -> u16 {
    6401
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
    String::from("http://localhost:6402")
}

fn default_juicehost_url() -> String {
    String::from("http://127.0.0.1:6402")
}

fn default_cors_origins() -> Vec<String> {
    vec![String::from("http://localhost:6400")]
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
            juiceback_origin: None,
        }
    }
}

impl Default for CobaltFile {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: default_cobalt_api_url(),
            session_api_url: None,
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

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(dir) = std::env::var("JUICEBACK_CONFIG") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
    }
    for name in ["juiceback.toml", "config.toml"] {
        paths.push(PathBuf::from(name));
        paths.push(PathBuf::from("/etc/juicebox").join(name));
    }
    paths
}

pub(crate) fn load_file_config() -> FileConfig {
    for path in candidate_paths() {
        if path.exists() {
            return load_one_file(&path);
        }
    }
    tracing::warn!("no juiceback config file found, using defaults");
    FileConfig::default()
}

fn load_one_file(path: &Path) -> FileConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        tracing::warn!("config file {} unreadable, using defaults", path.display());
        return FileConfig::default();
    };
    match toml::from_str::<FileConfig>(&text) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                "config file {} invalid ({err}), using defaults",
                path.display()
            );
            FileConfig::default()
        }
    }
}
