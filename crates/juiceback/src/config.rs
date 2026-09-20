//! Configuration loaded from a TOML file with environment overrides.
//! Secrets always come from the environment, never from TOML.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Holds every setting juiceback needs to run.
#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub quic_port: u16,
    pub database_path: String,
    pub rate_limit_per_minute: u32,
    pub db_pool_size: u32,
    /// Concurrent multipart uploads process-wide. Raised from the legacy
    /// value once streaming removed the per-upload full-file buffering that
    /// the old cap protected against.
    pub max_concurrent_uploads: u32,
    pub public_base_url: String,
    pub log_level: String,
    pub cleanup_interval_minutes: u64,
    pub juicehost_api_key: String,
    pub juicehost_url: String,
    /// Public URL that remote devices can use to reach juicehost. Falls back to
    /// `PUBLIC_BASE_URL` when `PUBLIC_JUICEHOST_URL` is unset.
    pub public_juicehost_url: String,
    pub juiceback_origin: String,
    pub jwt_secret: String,
    /// AES-256-GCM key for encrypting IP addresses before storage
    /// (hex-encoded).
    pub ip_encryption_key: String,
    /// Secret pepper for HMAC ban-lookup digests.
    pub ip_pepper: String,
    pub cors_origins: Vec<String>,
    /// Webhook URL for report notifications (Discord, Slack, etc.).
    pub report_webhook_url: Option<String>,
    /// SMTP host for report email notifications.
    pub smtp_host: Option<String>,
    /// SMTP port (default 465 for TLS).
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    /// Email address to send report notifications to.
    pub report_email_recipient: Option<String>,
    /// Email address reports appear to come from.
    pub report_email_sender: Option<String>,
    /// Path to the QUIC TLS certificate file for cert pinning.
    pub quic_cert_path: Option<PathBuf>,
    /// CIDRs whose peers are allowed to supply forwarding headers.
    pub trusted_proxy_cidrs: Vec<juiceutils::proxy::IpCidr>,
    pub report_retention_days: u64,
    pub feedback_retention_days: u64,
    /// Cloudflare API token for cache purging (optional).
    pub cf_api_token: Option<String>,
    /// Cloudflare zone ID for cache purging (optional).
    pub cf_zone_id: Option<String>,
    /// JWT signing secret for upload tickets shared with juicehost. Falls back
    /// to `jwt_secret` if unset.
    pub ticket_jwt_secret: String,
    /// Whether authentication and ownership cookies require HTTPS.
    pub secure_cookies: bool,
    /// Whether browser uploads may bypass the Node/Juiceback byte relay.
    pub direct_upload_enabled: bool,
    /// Whether the `JuiceBox` x Cobalt.Tools URL-fetch feature is enabled
    /// (default off).
    pub cobalt_enabled: bool,
    /// Base URL of the self-hosted cobalt API instance.
    pub cobalt_api_url: String,
    /// Cobalt Api-Key auth token. Required when `cobalt_enabled` is true.
    pub cobalt_api_key: String,
    /// Optional second cobalt instance with `YouTube` session auth (cookies +
    /// poToken provider). Used as a fallback when the primary instance
    /// refuses a `YouTube` link at client level (private/age/region-locked).
    pub cobalt_session_api_url: Option<String>,
    /// Api-Key auth token for the session instance.
    pub cobalt_session_api_key: Option<String>,
    /// Seconds between rescue-ladder passes for enforcement-gated `YouTube`
    /// links (flapping windows often open within seconds-minutes).
    pub fetch_empty_retry_delay_secs: u64,
    pub dte_enabled: bool,
    pub dte_assumed_bps: f64,
    pub dte_safety_mult: f64,
    pub dte_base_overhead_secs: i64,
    pub dte_min_ttl_secs: i64,
    pub dte_max_ttl_secs: i64,
    pub dte_mint_limit: u32,
    pub dte_mint_window_secs: u64,
    pub dte_mint_burst: u32,
    pub region_public_juicehosts: HashMap<String, String>,
    /// Test/dev escape hatch for the outbound-URL SSRF policy: when true,
    /// loopback and private addresses are accepted as fetch/tunnel/storage
    /// targets. Default false; never enable in production.
    pub allow_private_fetch: bool,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("quic_port", &self.quic_port)
            .field("database_path", &self.database_path)
            .field("rate_limit_per_minute", &self.rate_limit_per_minute)
            .field("db_pool_size", &self.db_pool_size)
            .field("max_concurrent_uploads", &self.max_concurrent_uploads)
            .field("public_base_url", &self.public_base_url)
            .field("log_level", &self.log_level)
            .field("cleanup_interval_minutes", &self.cleanup_interval_minutes)
            .field("juicehost_api_key", &"[REDACTED]")
            .field("juicehost_url", &self.juicehost_url)
            .field("public_juicehost_url", &self.public_juicehost_url)
            .field("juiceback_origin", &self.juiceback_origin)
            .field("jwt_secret", &"[REDACTED]")
            .field("ip_encryption_key", &"[REDACTED]")
            .field("ip_pepper", &"[REDACTED]")
            .field("cors_origins", &self.cors_origins)
            .field(
                "report_webhook_url",
                &self.report_webhook_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_username", &self.smtp_username)
            .field(
                "smtp_password",
                &self.smtp_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("report_email_recipient", &self.report_email_recipient)
            .field("report_email_sender", &self.report_email_sender)
            .field("trusted_proxy_cidrs", &self.trusted_proxy_cidrs)
            .field("report_retention_days", &self.report_retention_days)
            .field("feedback_retention_days", &self.feedback_retention_days)
            .field(
                "cf_api_token",
                &self.cf_api_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "cf_zone_id",
                &self.cf_zone_id.as_ref().map(|_| "[REDACTED]"),
            )
            .field("ticket_jwt_secret", &"[REDACTED]")
            .field("secure_cookies", &self.secure_cookies)
            .field("direct_upload_enabled", &self.direct_upload_enabled)
            .field("cobalt_enabled", &self.cobalt_enabled)
            .field("cobalt_api_url", &self.cobalt_api_url)
            .field("cobalt_api_key", &"[REDACTED]")
            .field("cobalt_session_api_url", &self.cobalt_session_api_url)
            .field(
                "cobalt_session_api_key",
                &self.cobalt_session_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("dte_enabled", &self.dte_enabled)
            .field("dte_assumed_bps", &self.dte_assumed_bps)
            .field("dte_safety_mult", &self.dte_safety_mult)
            .field("dte_base_overhead_secs", &self.dte_base_overhead_secs)
            .field("dte_min_ttl_secs", &self.dte_min_ttl_secs)
            .field("dte_max_ttl_secs", &self.dte_max_ttl_secs)
            .field("dte_mint_limit", &self.dte_mint_limit)
            .field("dte_mint_window_secs", &self.dte_mint_window_secs)
            .field("dte_mint_burst", &self.dte_mint_burst)
            .field("region_public_juicehosts", &self.region_public_juicehosts)
            .field("allow_private_fetch", &self.allow_private_fetch)
            .finish()
    }
}

/// TOML file layout for juiceback. Every section is optional.
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

/// Candidate config file locations, first hit wins.
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

fn load_file_config() -> FileConfig {
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

impl Config {
    /// Load configuration from a TOML file with environment overrides.
    pub fn try_load() -> Result<Self, String> {
        Self::try_load_from(&load_file_config())
    }

    fn try_load_from(file: &FileConfig) -> Result<Self, String> {
        let host = std::env::var("HOST").unwrap_or_else(|_| file.server.host.clone());
        let port = std::env::var("PORT")
            .unwrap_or_else(|_| file.server.port.to_string())
            .parse::<u16>()
            .map_err(|e| format!("Invalid PORT: {e}"))?;
        let quic_port = std::env::var("QUIC_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .or(file.server.quic_port)
            .unwrap_or(port + 1);

        let database_path =
            std::env::var("DATABASE_PATH").unwrap_or_else(|_| file.server.database_path.clone());
        let rate_limit_per_minute = std::env::var("RATE_LIMIT_PER_MINUTE")
            .unwrap_or_else(|_| file.server.rate_limit_per_minute.to_string())
            .parse::<u32>()
            .map_err(|e| format!("Invalid RATE_LIMIT_PER_MINUTE: {e}"))?;
        let db_pool_size = std::env::var("DB_POOL_SIZE")
            .unwrap_or_else(|_| file.server.db_pool_size.to_string())
            .parse::<u32>()
            .map_err(|e| format!("Invalid DB_POOL_SIZE: {e}"))?;
        let max_concurrent_uploads = std::env::var("MAX_CONCURRENT_UPLOADS")
            .unwrap_or_else(|_| file.server.max_concurrent_uploads.to_string())
            .parse::<u32>()
            .map_err(|e| format!("Invalid MAX_CONCURRENT_UPLOADS: {e}"))?;
        let public_base_url = std::env::var("PUBLIC_BASE_URL")
            .unwrap_or_else(|_| file.urls.public_base_url.clone())
            .trim_end_matches('/')
            .to_string();
        let log_level =
            std::env::var("LOG_LEVEL").unwrap_or_else(|_| file.server.log_level.clone());
        let cleanup_interval_minutes = std::env::var("CLEANUP_INTERVAL_MINUTES")
            .unwrap_or_else(|_| file.server.cleanup_interval_minutes.to_string())
            .parse::<u64>()
            .map_err(|e| format!("Invalid CLEANUP_INTERVAL_MINUTES: {e}"))?;

        // Secrets are env-only, never from TOML.
        let juicehost_api_key =
            juiceutils::config::optional_secret("JUICEHOST_API_KEY").unwrap_or_default();
        let jwt_secret =
            juiceutils::config::required_secret("JWT_SECRET").map_err(|e| e.to_string())?;
        let ip_encryption_key =
            juiceutils::config::required_secret("IP_ENCRYPTION_KEY").map_err(|e| e.to_string())?;
        let ip_pepper = juiceutils::config::optional_secret("IP_PEPPER").unwrap_or_default();
        let ticket_jwt_secret = juiceutils::config::optional_secret("TICKET_JWT_SECRET")
            .unwrap_or_else(|| jwt_secret.clone());
        let smtp_password = juiceutils::config::optional_secret("SMTP_PASSWORD");
        let cobalt_api_key =
            juiceutils::config::optional_secret("COBALT_API_KEY").unwrap_or_default();
        let cobalt_session_api_key = juiceutils::config::optional_secret("COBALT_SESSION_API_KEY");
        let cf_api_token = juiceutils::config::optional_secret("CF_API_TOKEN");

        // IP encryption key must decode to exactly 32 bytes.
        let encryption_key_bytes = hex::decode(&ip_encryption_key)
            .map_err(|_| "IP_ENCRYPTION_KEY must be a hexadecimal string".to_string())?;
        if encryption_key_bytes.len() != 32 {
            return Err("IP_ENCRYPTION_KEY must encode exactly 32 bytes".to_string());
        }

        let juicehost_url = std::env::var("JUICEHOST_URL")
            .unwrap_or_else(|_| file.urls.juicehost_url.clone())
            .trim_end_matches('/')
            .to_string();
        let public_juicehost_url = std::env::var("PUBLIC_JUICEHOST_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.urls.public_juicehost_url.clone())
            .unwrap_or_else(|| public_base_url.clone())
            .trim_end_matches('/')
            .to_string();
        let juiceback_origin = std::env::var("JUICEBACK_ORIGIN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.urls.juiceback_origin.clone())
            .unwrap_or_else(|| format!("http://{host}:{port}"))
            .trim_end_matches('/')
            .to_string();

        let cors_origins = std::env::var("CORS_ORIGINS")
            .unwrap_or_else(|_| file.cors.origins.join(","))
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        let report_webhook_url = std::env::var("REPORT_WEBHOOK_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let smtp_host = std::env::var("SMTP_HOST")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.report.smtp_host.clone());
        let smtp_port = std::env::var("SMTP_PORT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.parse::<u16>().unwrap_or(465))
            .or(file.report.smtp_port);
        let smtp_username = std::env::var("SMTP_USERNAME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.report.smtp_username.clone());
        let report_email_recipient = std::env::var("REPORT_EMAIL_RECIPIENT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.report.email_recipient.clone());
        let report_email_sender = std::env::var("REPORT_EMAIL_SENDER")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.report.email_sender.clone());

        let quic_cert_path = std::env::var("QUIC_CERT_PATH")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        let trusted_proxy_cidrs = juiceutils::proxy::parse_trusted_proxy_cidrs(
            &std::env::var("TRUSTED_PROXY_CIDRS").unwrap_or_default(),
        )?;

        let report_retention_days = std::env::var("REPORT_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(file.report.retention_days);
        let feedback_retention_days = std::env::var("FEEDBACK_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(file.report.feedback_retention_days);
        let cf_zone_id = std::env::var("CF_ZONE_ID")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.cloudflare.zone_id.clone());

        let secure_cookies = std::env::var("SECURE_COOKIES")
            .map_or(file.features.secure_cookies, |v| {
                v.trim().eq_ignore_ascii_case("true") || v.trim() == "1"
            });
        let direct_upload_enabled = std::env::var("DIRECT_UPLOAD_ENABLED")
            .map_or(file.features.direct_upload_enabled, |v| {
                v.trim().eq_ignore_ascii_case("true") || v.trim() == "1"
            });
        // Test/dev only: never set in production. Lets the fetch/tunnel
        // SSRF policy accept loopback and private addresses.
        let allow_private_fetch =
            std::env::var("JUICEBACK_ALLOW_PRIVATE_FETCH").map_or(false, |v| {
                matches!(
                    v.trim().to_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            });

        let cobalt_enabled = std::env::var("COBALT_ENABLED").map_or(file.cobalt.enabled, |v| {
            v.trim().eq_ignore_ascii_case("true") || v.trim() == "1"
        });
        let cobalt_api_url = std::env::var("COBALT_API_URL")
            .unwrap_or_else(|_| file.cobalt.api_url.clone())
            .trim_end_matches('/')
            .to_string();
        let cobalt_session_api_url = std::env::var("COBALT_SESSION_API_URL")
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| file.cobalt.session_api_url.clone().map(clean_url));
        let fetch_empty_retry_delay_secs = std::env::var("FETCH_EMPTY_RETRY_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(file.cobalt.fetch_empty_retry_delay_secs);

        let dte_enabled = std::env::var("DTE_ENABLED")
            .or_else(|_| std::env::var("DTE"))
            .map_or(file.dte.enabled, |v| {
                v.trim().eq_ignore_ascii_case("true") || v.trim() == "1"
            });
        let dte_assumed_bps = std::env::var("DTE_ASSUMED_BPS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(file.dte.assumed_bps);
        let dte_safety_mult = std::env::var("DTE_SAFETY_MULT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(file.dte.safety_mult);
        let dte_base_overhead_secs = std::env::var("DTE_BASE_OVERHEAD_SECS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(file.dte.base_overhead_secs);
        let dte_min_ttl_secs = std::env::var("DTE_MIN_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(file.dte.min_ttl_secs);
        let dte_max_ttl_secs = std::env::var("DTE_MAX_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(file.dte.max_ttl_secs);
        let dte_mint_limit = std::env::var("DTE_MINT_LIMIT")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(file.dte.mint_limit);
        let dte_mint_window_secs = std::env::var("DTE_MINT_WINDOW_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(file.dte.mint_window_secs);
        let dte_mint_burst = std::env::var("DTE_MINT_BURST")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(file.dte.mint_burst);

        let region_public_juicehosts = file.regions.public_juicehosts.clone().unwrap_or_default();

        if cobalt_session_api_url.is_some() != cobalt_session_api_key.is_some() {
            return Err(
                "COBALT_SESSION_API_URL and COBALT_SESSION_API_KEY must be set together \
                 (or both left unset)."
                    .to_string(),
            );
        }

        // Refuse to start with placeholder secrets.
        if juiceutils::config::is_placeholder_secret(&jwt_secret) {
            return Err(
                "JWT_SECRET is set to a placeholder value. Generate a real secret before starting."
                    .to_string(),
            );
        }

        if cobalt_enabled && cobalt_api_key.is_empty() {
            return Err(
                "COBALT_ENABLED is true but COBALT_API_KEY is not set. Cobalt instances with \
                 auth enabled require an API key; set COBALT_API_KEY or disable COBALT_ENABLED."
                    .to_string(),
            );
        }

        Ok(Self {
            host,
            port,
            quic_port,
            database_path,
            rate_limit_per_minute,
            db_pool_size,
            max_concurrent_uploads,
            public_base_url,
            log_level,
            cleanup_interval_minutes,
            juicehost_api_key,
            juicehost_url,
            public_juicehost_url,
            juiceback_origin,
            jwt_secret,
            ip_encryption_key,
            ip_pepper,
            cors_origins,
            report_webhook_url,
            smtp_host,
            smtp_port,
            smtp_username,
            smtp_password,
            report_email_recipient,
            report_email_sender,
            quic_cert_path,
            trusted_proxy_cidrs,
            report_retention_days,
            feedback_retention_days,
            cf_api_token,
            cf_zone_id,
            ticket_jwt_secret,
            secure_cookies,
            direct_upload_enabled,
            cobalt_enabled,
            cobalt_api_url,
            cobalt_api_key,
            cobalt_session_api_url,
            cobalt_session_api_key,
            fetch_empty_retry_delay_secs,
            dte_enabled,
            dte_assumed_bps,
            dte_safety_mult,
            dte_base_overhead_secs,
            dte_min_ttl_secs,
            dte_max_ttl_secs,
            dte_mint_limit,
            dte_mint_window_secs,
            dte_mint_burst,
            region_public_juicehosts,
            allow_private_fetch,
        })
    }
}

fn clean_url(s: String) -> String {
    s.trim_end_matches('/').to_string()
}
