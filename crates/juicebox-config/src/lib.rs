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

/// Read a required secret from the environment.
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

/// Read an optional secret from the environment.
#[must_use]
pub fn optional_secret(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_uses_defaults() {
        let cfg: SentrySettings =
            load_toml_or_default(Path::new("/nonexistent-juicebox-config.toml"));
        assert_eq!(cfg.environment, "production");
        assert_eq!(cfg.dsn, None);
    }
}
