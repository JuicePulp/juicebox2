use std::{collections::HashMap, path::PathBuf};

use thiserror::Error;

pub mod file;
pub use file::FileConfig;
use file::load_file_config;

use crate::cobalt::CobaltEndpoint;

/// Messages name the setting but never include secret values.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(transparent)]
    Secret(#[from] juiceutils::config::ConfigError),
    #[error("{name} must be a non-negative integer")]
    InvalidInteger { name: String },
    #[error("{0}")]
    Invalid(String),
    #[error("password must be at least {min_len} characters")]
    PasswordTooShort { min_len: usize },
    #[error("password must be at most {max_len} characters")]
    PasswordTooLong { max_len: usize },
}

/// Secrets come from the environment, not TOML; `Debug` redacts them.
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
    pub default_storage_host: Option<String>,
    pub juiceback_origin: String,
    pub router_base_url: Option<String>,
    pub jwt_secret: String,

    pub ip_encryption_key: String,

    /// Empty means unsalted hashes.
    pub ip_pepper: String,

    pub storage_encryption_key: String,

    pub password_min_len: usize,
    pub password_try_limit: u32,
    pub password_try_window_secs: u64,
    pub encrypted_force_relay: bool,
    pub cors_origins: Vec<String>,

    pub report_webhook_url: Option<String>,

    pub smtp_host: Option<String>,

    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,

    pub report_email_recipient: Option<String>,

    pub report_email_sender: Option<String>,

    pub quic_cert_path: Option<PathBuf>,

    /// Only these CIDRs' forwarded-for header is trusted; other clients use the
    /// socket address.
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

    pub cobalt_endpoints: Vec<CobaltEndpoint>,

    pub cobalt_session_api_url: Option<String>,

    pub cobalt_session_api_key: Option<String>,

    pub bun_fetch_url: Option<String>,

    pub bun_fetch_key: Option<String>,

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

    /// Allows private/loopback fetches. SSRF hazard; local development only.
    pub allow_private_fetch: bool,

    pub r2_enabled: bool,
    pub r2_bucket: Option<String>,
    pub r2_endpoint: Option<String>,
    pub r2_public_base_url: String,
    pub r2_access_key: String,
    pub r2_secret_key: String,
    pub r2_max_bytes: u64,
    pub r2_max_hot_files: u64,
    pub r2_max_promote_bytes: u64,

    pub promo_interval_secs: u64,
    pub promo_cool_after_secs: i64,
    pub promo_min_views: u64,
    pub promo_min_views_per_min: f64,
    pub promo_min_ttl_remaining_secs: i64,
    pub promo_dry_run: bool,
    pub embed_promote_enabled: bool,
    pub embed_promote_max_bytes: u64,
    pub embed_promote_cooldown_secs: i64,
    pub embed_promote_min_ttl_remaining_secs: i64,
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
            .field("storage_encryption_key", &"[REDACTED]")
            .field("password_min_len", &self.password_min_len)
            .field("password_try_limit", &self.password_try_limit)
            .field("password_try_window_secs", &self.password_try_window_secs)
            .field("encrypted_force_relay", &self.encrypted_force_relay)
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
            .field(
                "cobalt_endpoints",
                &self
                    .cobalt_endpoints
                    .iter()
                    .map(|e| (&e.url, e.session))
                    .collect::<Vec<_>>(),
            )
            .field("cobalt_session_api_url", &self.cobalt_session_api_url)
            .field(
                "cobalt_session_api_key",
                &self.cobalt_session_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("bun_fetch_url", &self.bun_fetch_url)
            .field(
                "bun_fetch_key",
                &self.bun_fetch_key.as_ref().map(|_| "[REDACTED]"),
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
            .field("embed_promote_enabled", &self.embed_promote_enabled)
            .field("embed_promote_max_bytes", &self.embed_promote_max_bytes)
            .field(
                "embed_promote_cooldown_secs",
                &self.embed_promote_cooldown_secs,
            )
            .field(
                "embed_promote_min_ttl_remaining_secs",
                &self.embed_promote_min_ttl_remaining_secs,
            )
            .finish()
    }
}

impl Config {
    pub fn try_load() -> Result<Self, ConfigError> {
        Self::try_load_from(&load_file_config())
    }

    fn try_load_from(file: &FileConfig) -> Result<Self, ConfigError> {
        let host = file.server.host.clone();
        let port = file.server.port;
        let quic_port = file
            .server
            .quic_port
            .unwrap_or(port + juiceutils::config::ports::QUIC_OFFSET);

        let database_path = file.server.database_path.clone();
        let rate_limit_per_minute = file.server.rate_limit_per_minute;
        let db_pool_size = file.server.db_pool_size;
        let max_concurrent_uploads = file.server.max_concurrent_uploads;
        let public_base_url = file.urls.public_base_url.trim_end_matches('/').to_string();
        let log_level = file.server.log_level.clone();
        let cleanup_interval_minutes = file.server.cleanup_interval_minutes;

        let juicehost_api_key =
            juiceutils::config::optional_secret("JUICEHOST_API_KEY").unwrap_or_default();
        let jwt_secret = juiceutils::config::required_secret("JWT_SECRET")?;
        let ip_encryption_key = juiceutils::config::required_secret("IP_ENCRYPTION_KEY")?;
        let ip_pepper = juiceutils::config::optional_secret("IP_PEPPER").unwrap_or_default();
        let ticket_jwt_secret = juiceutils::config::ticket_secret(&jwt_secret)?;
        let smtp_password = juiceutils::config::optional_secret("SMTP_PASSWORD");
        let cobalt_api_key =
            juiceutils::config::optional_secret("COBALT_API_KEY").unwrap_or_default();
        let cobalt_session_api_key = juiceutils::config::optional_secret("COBALT_SESSION_API_KEY");
        let bun_fetch_url = std::env::var("BUN_FETCH_URL")
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty());
        let bun_fetch_key = std::env::var("BUN_FETCH_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let cf_api_token = juiceutils::config::optional_secret("CF_API_TOKEN");

        juiceutils::config::validate_ip_encryption_key(&ip_encryption_key)?;

        let storage_encryption_key = juiceutils::config::required_secret("STORAGE_ENCRYPTION_KEY")?;
        juiceutils::config::validate_hex_key("STORAGE_ENCRYPTION_KEY", &storage_encryption_key)?;

        let password_min_len = parse_env_usize("PASSWORD_MIN_LEN", DEFAULT_PASSWORD_MIN_LEN)?;
        let password_try_limit = parse_env_u32("PASSWORD_TRY_LIMIT", DEFAULT_PASSWORD_TRY_LIMIT)?;
        let password_try_window_secs =
            parse_env_u64("PASSWORD_TRY_WINDOW_SECS", DEFAULT_PASSWORD_TRY_WINDOW_SECS)?;
        let encrypted_force_relay = juiceutils::config::env_bool("ENCRYPTED_FORCE_RELAY")?
            .unwrap_or(DEFAULT_ENCRYPTED_FORCE_RELAY);

        let juicehost_url = juiceutils::config::optional_secret("JUICEHOST_URL")
            .unwrap_or_else(|| file.urls.juicehost_url.clone())
            .trim_end_matches('/')
            .to_string();
        let public_juicehost_url = file
            .urls
            .public_juicehost_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| public_base_url.clone())
            .trim_end_matches('/')
            .to_string();
        let default_storage_host = std::env::var("DEFAULT_STORAGE_HOST")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                file.urls
                    .default_storage_host
                    .clone()
                    .map(|value| value.trim().trim_end_matches('/').to_string())
                    .filter(|value| !value.is_empty())
            });
        let juiceback_origin = file
            .urls
            .juiceback_origin
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("http://{host}:{port}"))
            .trim_end_matches('/')
            .to_string();
        let router_base_url = std::env::var("ROUTER_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.urls.router_base_url.clone())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim_end_matches('/').to_string());

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
        .map_err(|e| ConfigError::Invalid(format!("invalid server.trusted_proxy_cidrs: {e}")))?;

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

        let mut cobalt_endpoints = Vec::new();
        push_cobalt_endpoint(
            &mut cobalt_endpoints,
            cobalt_api_url.clone(),
            &cobalt_api_key,
            false,
        );
        for url in &file.cobalt.api_urls {
            push_cobalt_endpoint(&mut cobalt_endpoints, url.clone(), &cobalt_api_key, false);
        }
        if let (Some(url), Some(key)) = (&cobalt_session_api_url, &cobalt_session_api_key) {
            push_cobalt_endpoint(&mut cobalt_endpoints, url.clone(), key, true);
        }
        if cobalt_session_api_key.is_some() {
            for url in &file.cobalt.session_api_urls {
                push_cobalt_endpoint(
                    &mut cobalt_endpoints,
                    url.clone(),
                    cobalt_session_api_key.as_deref().unwrap_or_default(),
                    true,
                );
            }
        }
        if !file.cobalt.session_api_urls.is_empty() && cobalt_session_api_key.is_none() {
            return Err(ConfigError::Invalid(
                "cobalt session_api_urls requires COBALT_SESSION_API_KEY to be set".to_string(),
            ));
        }
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

        let r2_enabled = file.r2.enabled;
        let r2_bucket = std::env::var("R2_BUCKET")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.r2.bucket.clone());
        let r2_endpoint = std::env::var("R2_ENDPOINT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.r2.endpoint.clone());
        let r2_public_base_url = std::env::var("R2_PUBLIC_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.r2.public_base_url.clone())
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let r2_access_key =
            juiceutils::config::optional_secret("R2_ACCESS_KEY").unwrap_or_default();
        let r2_secret_key =
            juiceutils::config::optional_secret("R2_SECRET_KEY").unwrap_or_default();
        let r2_max_bytes = parse_env_u64("R2_MAX_BYTES", file.r2.max_bytes)?;
        let r2_max_hot_files = parse_env_u64("R2_MAX_HOT_FILES", file.r2.max_hot_files)?;
        let r2_max_promote_bytes =
            parse_env_u64("R2_MAX_PROMOTE_BYTES", file.r2.max_promote_bytes)?;

        // One-minute floor so a misconfigured promoter cannot spin.
        let promo_interval_secs = file.promo.interval_secs.max(60);
        let promo_cool_after_secs = file.promo.cool_after_secs.max(60);
        let promo_min_views = file.promo.min_views;
        let promo_min_views_per_min = file.promo.min_views_per_min;
        let promo_min_ttl_remaining_secs = file.promo.min_ttl_remaining_secs.max(0);
        let promo_dry_run =
            juiceutils::config::env_bool("PROMO_DRY_RUN")?.unwrap_or(file.promo.dry_run);
        let embed_promote_enabled = file.promo.embed_enabled;
        let embed_promote_max_bytes = file.promo.embed_max_bytes;
        let embed_promote_cooldown_secs = file.promo.embed_cooldown_secs.max(60);
        let embed_promote_min_ttl_remaining_secs = file.promo.embed_min_ttl_remaining_secs.max(0);

        if cobalt_session_api_url.is_some() != cobalt_session_api_key.is_some()
            && file.cobalt.session_api_urls.is_empty()
        {
            return Err(ConfigError::Invalid(
                "cobalt session_api_url and COBALT_SESSION_API_KEY must be set together \
                 (or both left unset)."
                    .to_string(),
            ));
        }

        if bun_fetch_url.is_some() != bun_fetch_key.is_some() {
            return Err(ConfigError::Invalid(
                "BUN_FETCH_URL and BUN_FETCH_KEY must be set together (or both left unset)."
                    .to_string(),
            ));
        }

        juiceutils::config::validate_jwt_secret(&jwt_secret)?;

        if cobalt_enabled && cobalt_api_key.is_empty() {
            return Err(ConfigError::Invalid(
                "cobalt is enabled but COBALT_API_KEY is not set. Cobalt instances with \
                 auth enabled require an API key; set COBALT_API_KEY or disable cobalt in [cobalt]."
                    .to_string(),
            ));
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
            default_storage_host,
            juiceback_origin,
            router_base_url,
            jwt_secret,
            ip_encryption_key,
            ip_pepper,
            storage_encryption_key,
            password_min_len,
            password_try_limit,
            password_try_window_secs,
            encrypted_force_relay,
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
            sentry_environment: juiceutils::config::SentrySettings::from_env_or(&file.sentry)
                .environment,
            direct_upload_enabled,
            cobalt_enabled,
            cobalt_api_url,
            cobalt_api_key,
            cobalt_endpoints,
            cobalt_session_api_url,
            cobalt_session_api_key,
            bun_fetch_url,
            bun_fetch_key,
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
            r2_enabled,
            r2_bucket,
            r2_endpoint,
            r2_public_base_url,
            r2_access_key,
            r2_secret_key,
            r2_max_bytes,
            r2_max_hot_files,
            r2_max_promote_bytes,
            promo_interval_secs,
            promo_cool_after_secs,
            promo_min_views,
            promo_min_views_per_min,
            promo_min_ttl_remaining_secs,
            promo_dry_run,
            embed_promote_enabled,
            embed_promote_max_bytes,
            embed_promote_cooldown_secs,
            embed_promote_min_ttl_remaining_secs,
        })
    }
}

fn clean_url(s: String) -> String {
    s.trim_end_matches('/').to_string()
}

fn push_cobalt_endpoint(
    endpoints: &mut Vec<CobaltEndpoint>,
    url: impl Into<String>,
    api_key: &str,
    session: bool,
) {
    let url = url.into().trim().trim_end_matches('/').to_string();
    if url.is_empty() || endpoints.iter().any(|existing| existing.url == url) {
        return;
    }
    endpoints.push(CobaltEndpoint::new(url, api_key, session));
}

pub const DEFAULT_PASSWORD_MIN_LEN: usize = 8;

pub const PASSWORD_MAX_LEN: usize = 256;
const DEFAULT_PASSWORD_TRY_LIMIT: u32 = 10;
const DEFAULT_PASSWORD_TRY_WINDOW_SECS: u64 = 600;
const DEFAULT_ENCRYPTED_FORCE_RELAY: bool = true;

fn parse_env_usize(name: &str, default: usize) -> Result<usize, ConfigError> {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => {
            raw.trim().parse().map_err(|_| ConfigError::InvalidInteger {
                name: name.to_owned(),
            })
        }
        _ => Ok(default),
    }
}

fn parse_env_u32(name: &str, default: u32) -> Result<u32, ConfigError> {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => {
            raw.trim().parse().map_err(|_| ConfigError::InvalidInteger {
                name: name.to_owned(),
            })
        }
        _ => Ok(default),
    }
}

fn parse_env_u64(name: &str, default: u64) -> Result<u64, ConfigError> {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => {
            raw.trim().parse().map_err(|_| ConfigError::InvalidInteger {
                name: name.to_owned(),
            })
        }
        _ => Ok(default),
    }
}

pub fn validate_upload_password(password: &str, min_len: usize) -> Result<(), ConfigError> {
    if password.len() < min_len {
        return Err(ConfigError::PasswordTooShort { min_len });
    }
    if password.len() > PASSWORD_MAX_LEN {
        return Err(ConfigError::PasswordTooLong {
            max_len: PASSWORD_MAX_LEN,
        });
    }
    Ok(())
}

impl Config {
    pub fn storage_file_key(
        &self,
    ) -> Result<crate::crypto_file::FileKey, crate::crypto_file::CryptoError> {
        crate::crypto_file::FileKey::from_hex(&self.storage_encryption_key)
    }

    pub fn check_upload_password(&self, password: &str) -> Result<(), ConfigError> {
        validate_upload_password(password, self.password_min_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_example_parses() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../juiceback.toml.example"
        ));
        let file: FileConfig = toml::from_str(text).expect("juiceback.toml.example must parse");
        assert_eq!(file.server.port, juiceutils::config::ports::BACKEND);
    }

    #[test]
    fn upload_password_policy() {
        assert!(validate_upload_password("12345678", 8).is_ok());
        assert!(validate_upload_password("1234567", 8).is_err());
        assert!(validate_upload_password(&"x".repeat(PASSWORD_MAX_LEN), 8).is_ok());
        assert!(validate_upload_password(&"x".repeat(PASSWORD_MAX_LEN + 1), 8).is_err());
        let err = validate_upload_password("short", 8).unwrap_err();
        assert!(
            !err.to_string().contains("short"),
            "password must not leak: {err}"
        );
    }
}
