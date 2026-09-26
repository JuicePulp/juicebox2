use std::{collections::HashMap, path::PathBuf};

pub mod file;
pub use file::FileConfig;
use file::load_file_config;

#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub quic_port: u16,
    pub database_path: String,
    pub rate_limit_per_minute: u32,
    pub db_pool_size: u32,

    pub max_concurrent_uploads: u32,
    pub public_base_url: String,
    pub log_level: String,
    pub cleanup_interval_minutes: u64,
    pub juicehost_api_key: String,
    pub juicehost_url: String,

    pub public_juicehost_url: String,
    pub juiceback_origin: String,
    pub jwt_secret: String,

    pub ip_encryption_key: String,

    pub ip_pepper: String,
    pub cors_origins: Vec<String>,

    pub report_webhook_url: Option<String>,

    pub smtp_host: Option<String>,

    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,

    pub report_email_recipient: Option<String>,

    pub report_email_sender: Option<String>,

    pub quic_cert_path: Option<PathBuf>,

    pub trusted_proxy_cidrs: Vec<juiceutils::proxy::IpCidr>,
    pub report_retention_days: u64,
    pub feedback_retention_days: u64,

    pub cf_api_token: Option<String>,

    pub cf_zone_id: Option<String>,

    pub ticket_jwt_secret: String,

    pub secure_cookies: bool,

    pub sentry_environment: String,

    pub direct_upload_enabled: bool,

    pub cobalt_enabled: bool,

    pub cobalt_api_url: String,

    pub cobalt_api_key: String,

    pub cobalt_session_api_url: Option<String>,

    pub cobalt_session_api_key: Option<String>,

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
            .field("sentry_environment", &self.sentry_environment)
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

impl Config {
    pub fn try_load() -> Result<Self, String> {
        Self::try_load_from(&load_file_config())
    }

    fn try_load_from(file: &FileConfig) -> Result<Self, String> {
        let host = file.server.host.clone();
        let port = file.server.port;
        let quic_port = file.server.quic_port.unwrap_or(port + 1);

        let database_path = file.server.database_path.clone();
        let rate_limit_per_minute = file.server.rate_limit_per_minute;
        let db_pool_size = file.server.db_pool_size;
        let max_concurrent_uploads = file.server.max_concurrent_uploads;
        let public_base_url = file.urls.public_base_url.trim_end_matches('/').to_string();
        let log_level = file.server.log_level.clone();
        let cleanup_interval_minutes = file.server.cleanup_interval_minutes;

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

        let encryption_key_bytes = hex::decode(&ip_encryption_key)
            .map_err(|_| "IP_ENCRYPTION_KEY must be a hexadecimal string".to_string())?;
        if encryption_key_bytes.len() != 32 {
            return Err("IP_ENCRYPTION_KEY must encode exactly 32 bytes".to_string());
        }

        let juicehost_url = file.urls.juicehost_url.trim_end_matches('/').to_string();
        let public_juicehost_url = file
            .urls
            .public_juicehost_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| public_base_url.clone())
            .trim_end_matches('/')
            .to_string();
        let juiceback_origin = file
            .urls
            .juiceback_origin
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("http://{host}:{port}"))
            .trim_end_matches('/')
            .to_string();

        let cors_origins: Vec<String> = file
            .cors
            .origins
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let report_webhook_url = file.report.webhook_url.clone();
        let smtp_host = file.report.smtp_host.clone();
        let smtp_port = file.report.smtp_port;
        let smtp_username = file.report.smtp_username.clone();
        let report_email_recipient = file.report.email_recipient.clone();
        let report_email_sender = file.report.email_sender.clone();

        let quic_cert_path = file.server.quic_cert_path.clone();

        let trusted_proxy_cidrs = juiceutils::proxy::parse_trusted_proxy_cidrs(
            &file.server.trusted_proxy_cidrs.join(","),
        )
        .map_err(|e| format!("invalid server.trusted_proxy_cidrs: {e}"))?;

        let report_retention_days = file.report.retention_days;
        let feedback_retention_days = file.report.feedback_retention_days;
        let cf_zone_id = file.cloudflare.zone_id.clone();

        let secure_cookies = file.features.secure_cookies;
        let direct_upload_enabled = file.features.direct_upload_enabled;

        let allow_private_fetch = file.features.allow_private_fetch;

        let cobalt_enabled = file.cobalt.enabled;
        let cobalt_api_url = file.cobalt.api_url.trim_end_matches('/').to_string();
        let cobalt_session_api_url = file
            .cobalt
            .session_api_url
            .clone()
            .map(clean_url)
            .filter(|v| !v.is_empty());
        let fetch_empty_retry_delay_secs = file.cobalt.fetch_empty_retry_delay_secs;

        let dte_enabled = file.dte.enabled;
        let dte_assumed_bps = file.dte.assumed_bps;
        let dte_safety_mult = file.dte.safety_mult;
        let dte_base_overhead_secs = file.dte.base_overhead_secs;
        let dte_min_ttl_secs = file.dte.min_ttl_secs;
        let dte_max_ttl_secs = file.dte.max_ttl_secs;
        let dte_mint_limit = file.dte.mint_limit;
        let dte_mint_window_secs = file.dte.mint_window_secs;
        let dte_mint_burst = file.dte.mint_burst;

        let region_public_juicehosts = file.regions.public_juicehosts.clone().unwrap_or_default();

        if cobalt_session_api_url.is_some() != cobalt_session_api_key.is_some() {
            return Err(
                "cobalt session_api_url and COBALT_SESSION_API_KEY must be set together \
                 (or both left unset)."
                    .to_string(),
            );
        }

        if juiceutils::config::is_placeholder_secret(&jwt_secret) {
            return Err(
                "JWT_SECRET is set to a placeholder value. Generate a real secret before starting."
                    .to_string(),
            );
        }

        if cobalt_enabled && cobalt_api_key.is_empty() {
            return Err(
                "cobalt is enabled but COBALT_API_KEY is not set. Cobalt instances with \
                 auth enabled require an API key; set COBALT_API_KEY or disable cobalt in [cobalt]."
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
            sentry_environment: file.sentry.environment.clone(),
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
