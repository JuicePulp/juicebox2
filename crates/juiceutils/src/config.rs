use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{name} is not set")]
    Missing { name: String },
    #[error("{name} is empty")]
    Empty { name: String },
    #[error("{name} must be a boolean (true/false/1/0)")]
    InvalidBoolean { name: String },
    #[error(
        "TICKET_JWT_SECRET is not set. Set TICKET_JWT_SECRET (or JWT_SECRET) to a real secret."
    )]
    MissingTicketSecret,
    #[error("{origin} is set to a placeholder value. Generate a real secret before starting.")]
    PlaceholderSecret { origin: String },
    #[error("JWT_SECRET is not set. Set a real secret.")]
    MissingJwtSecret,
    #[error("JWT_SECRET is set to a placeholder value. Generate a real secret before starting.")]
    PlaceholderJwtSecret,
    #[error("{name} must be a hexadecimal string")]
    InvalidHex { name: String },
    #[error("{name} must encode exactly 32 bytes")]
    InvalidKeyLength { name: String },
}

pub mod ports {
    pub const FRONTEND: u16 = 6400;
    pub const BACKEND: u16 = 6401;
    pub const HOST: u16 = 6402;
    pub const QUIC_OFFSET: u16 = 1;
}

pub mod defaults {
    pub const MAX_FILE_SIZE_MB: u64 = 500;
    pub const MAX_FILE_SIZE_BYTES: u64 = MAX_FILE_SIZE_MB * 1024 * 1024;
    pub const DEFAULT_TTL_HOURS: f64 = 24.0;
    pub const ALLOWED_TTL_HOURS: &[f64] = &[0.5, 1.0, 6.0, 12.0, 24.0, 72.0, 168.0];
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SentrySettings {
    #[serde(default = "default_sentry_env")]
    pub environment: String,
}

impl Default for SentrySettings {
    fn default() -> Self {
        Self {
            environment: default_sentry_env(),
        }
    }
}

fn default_sentry_env() -> String {
    String::from("production")
}

impl SentrySettings {
    #[must_use]
    pub fn from_env_or(file: &Self) -> Self {
        let environment = std::env::var("SENTRY_ENVIRONMENT")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| file.environment.clone());
        Self { environment }
    }
}

pub fn load_toml_or_default<T>(path: &Path) -> T
where
    T: Default + for<'de> Deserialize<'de>,
{
    let Ok(text) = std::fs::read_to_string(path) else {
        tracing::warn!("config file {} unreadable, using defaults", path.display());
        return T::default();
    };
    match toml::from_str::<T>(&text) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                "config file {} invalid ({err}), using defaults",
                path.display()
            );
            T::default()
        }
    }
}

pub fn load_service_config<T>(service_name: &str, env_var: &str, names: &[&str]) -> T
where
    T: Default + for<'de> Deserialize<'de>,
{
    for path in candidate_paths(env_var, names) {
        if path.exists() {
            return load_toml_or_default(&path);
        }
    }
    tracing::warn!("no {service_name} config file found, using defaults");
    T::default()
}

fn candidate_paths(env_var: &str, names: &[&str]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(dir) = std::env::var(env_var) {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
    }
    for name in names {
        paths.push(PathBuf::from(name));
        paths.push(PathBuf::from("/etc/juicebox").join(name.trim_start_matches('.')));
    }
    paths
}

pub fn load_dotenv() {
    let Ok(text) = std::fs::read_to_string(".env") else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || std::env::var(key).is_ok() {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');

        unsafe {
            std::env::set_var(key, value);
        }
    }
}

pub fn required_secret(name: &str) -> Result<String, ConfigError> {
    std::env::var(name)
        .map_err(|_| ConfigError::Missing {
            name: name.to_owned(),
        })
        .and_then(|v| {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Err(ConfigError::Empty {
                    name: name.to_owned(),
                })
            } else {
                Ok(trimmed)
            }
        })
}

#[must_use]
pub fn optional_secret(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

#[must_use]
pub fn is_placeholder_secret(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.starts_with("change_me") || trimmed.starts_with("change_this")
}

pub fn env_bool(name: &str) -> Result<Option<bool>, ConfigError> {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(Some(true)),
            "0" | "false" | "no" | "off" => Ok(Some(false)),
            _ => Err(ConfigError::InvalidBoolean {
                name: name.to_owned(),
            }),
        },
        _ => Ok(None),
    }
}

pub fn ticket_secret(fallback: &str) -> Result<String, ConfigError> {
    let (secret, source) = optional_secret("TICKET_JWT_SECRET")
        .map(|secret| (secret, "TICKET_JWT_SECRET"))
        .or_else(|| optional_secret("JWT_SECRET").map(|secret| (secret, "JWT_SECRET")))
        .unwrap_or_else(|| (fallback.trim().to_owned(), "the service API key"));
    if secret.is_empty() {
        return Err(ConfigError::MissingTicketSecret);
    }
    if is_placeholder_secret(&secret) {
        return Err(ConfigError::PlaceholderSecret {
            origin: source.to_owned(),
        });
    }
    Ok(secret)
}

pub fn validate_jwt_secret(secret: &str) -> Result<(), ConfigError> {
    if secret.trim().is_empty() {
        return Err(ConfigError::MissingJwtSecret);
    }
    if is_placeholder_secret(secret) {
        return Err(ConfigError::PlaceholderJwtSecret);
    }
    Ok(())
}

pub fn validate_ip_encryption_key(key: &str) -> Result<(), ConfigError> {
    validate_hex_key("IP_ENCRYPTION_KEY", key)
}

pub fn validate_hex_key(name: &str, key: &str) -> Result<(), ConfigError> {
    let bytes = hex::decode(key.trim()).map_err(|_| ConfigError::InvalidHex {
        name: name.to_owned(),
    })?;
    if bytes.len() != 32 {
        return Err(ConfigError::InvalidKeyLength {
            name: name.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_uses_defaults() {
        let cfg: SentrySettings = load_toml_or_default(Path::new("/nonexistent-config.toml"));
        assert_eq!(cfg.environment, "production");
    }

    #[test]
    fn placeholder_secrets_detected() {
        assert!(is_placeholder_secret(""));
        assert!(is_placeholder_secret("   "));
        assert!(is_placeholder_secret("change_this_to_a_random_value"));
        assert!(is_placeholder_secret("  change_this_in_production\n"));
        assert!(is_placeholder_secret("change_me"));
        assert!(is_placeholder_secret(
            "change_me_generate_with_openssl_rand_hex_32"
        ));
        assert!(is_placeholder_secret(
            "change_me_optional_defaults_to_JWT_SECRET"
        ));
        assert!(!is_placeholder_secret("a-real-secret-value"));
        assert!(!is_placeholder_secret(
            "9f8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8"
        ));
    }

    #[test]
    fn jwt_secret_validation_rejects_empty_and_placeholders() {
        assert!(validate_jwt_secret("").is_err());
        assert!(validate_jwt_secret("   ").is_err());
        assert!(validate_jwt_secret("change_me_to_something").is_err());
        assert!(validate_jwt_secret("a-real-secret-value").is_ok());
    }

    #[test]
    fn ip_encryption_key_validation_requires_32_hex_bytes() {
        assert!(validate_ip_encryption_key(&"ab".repeat(32)).is_ok());
        assert!(validate_ip_encryption_key(&format!("  {}  ", "ab".repeat(32))).is_ok());
        assert!(validate_ip_encryption_key("").is_err());
        assert!(validate_ip_encryption_key("not-hex").is_err());
        assert!(validate_ip_encryption_key(&"ab".repeat(31)).is_err());
        assert!(validate_ip_encryption_key(&"zz".repeat(32)).is_err());
        let err = validate_ip_encryption_key("nope").unwrap_err();
        assert!(
            !err.to_string().contains("nope"),
            "key material must not leak: {err}"
        );
    }

    #[test]
    fn quic_default_follows_backend_port() {
        assert_eq!(ports::BACKEND + ports::QUIC_OFFSET, ports::HOST);
    }
}
