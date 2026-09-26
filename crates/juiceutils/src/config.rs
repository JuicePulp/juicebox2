//! Shared TOML config loading plus Sentry settings used by every service.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Sentry settings shared by all Juicebox services.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SentrySettings {
    /// Sentry DSN. Falls back to `SENTRY_DSN` when unset.
    #[serde(default)]
    pub dsn: Option<String>,
    /// Environment reported to Sentry.
    #[serde(default = "default_sentry_env")]
    pub environment: String,
}

impl Default for SentrySettings {
    fn default() -> Self {
        Self {
            dsn: None,
            environment: default_sentry_env(),
        }
    }
}

fn default_sentry_env() -> String {
    String::from("production")
}

impl SentrySettings {
    /// Load Sentry settings from the environment with TOML values as defaults.
    #[must_use]
    pub fn from_env_or(file: &Self) -> Self {
        let dsn = std::env::var("SENTRY_DSN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| file.dsn.clone());
        let environment = std::env::var("SENTRY_ENVIRONMENT")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| file.environment.clone());
        Self { dsn, environment }
    }
}

/// Load a TOML config file into `T`, warning and using defaults when missing.
pub fn load_toml_or_default<T>(path: &Path) -> T
where
    T: Default + for<'de> Deserialize<'de>,
{
    let Ok(text) = std::fs::read_to_string(path) else {
        tracing::warn!("config file {} not found, using defaults", path.display());
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

/// Load `./.env` for vars not already set.
///
/// Explicit environment always wins; a missing file is fine. Every binary
/// calls this first so standalone runs behave like `juicebox` supervision.
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

/// Read a required secret from the environment, trimmed.
///
/// # Errors
///
/// Returns an error when the variable is unset or blank.
pub fn required_secret(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("{name} is not set"))
        .and_then(|v| {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Err(anyhow::anyhow!("{name} is empty"))
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

/// Whether a secret value is empty or one of the documented placeholder
/// values shipped in `.env.example`.
///
/// Matches by prefix (`change_me`, `change_this`) so every documented
/// placeholder shape is rejected, not just two exact strings. Services
/// refuse to start with these.
#[must_use]
pub fn is_placeholder_secret(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.starts_with("change_me") || trimmed.starts_with("change_this")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_uses_defaults() {
        let cfg: SentrySettings = load_toml_or_default(Path::new("/nonexistent-config.toml"));
        assert_eq!(cfg.environment, "production");
        assert_eq!(cfg.dsn, None);
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
}
