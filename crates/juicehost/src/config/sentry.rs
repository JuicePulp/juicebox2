use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct SentryConfig {
    #[serde(default = "SentryConfig::default_environment")]
    environment: String,

    #[serde(default = "SentryConfig::default_trace_sample_rate")]
    trace_sample_rate: f32,

    #[serde(default)]
    dsn: DsnConfig,
}

impl SentryConfig {
    #[must_use]
    pub fn environment(&self) -> &str {
        &self.environment
    }

    #[must_use]
    pub const fn trace_sample_rate(&self) -> f32 {
        self.trace_sample_rate
    }

    #[must_use]
    pub fn dsn(&self) -> Option<&String> {
        self.dsn.dsn()
    }

    #[must_use]
    fn default_environment() -> String {
        "production".to_owned()
    }

    #[must_use]
    const fn default_trace_sample_rate() -> f32 {
        0.05
    }
}

impl Default for SentryConfig {
    #[inline]
    fn default() -> Self {
        Self {
            environment: Self::default_environment(),
            trace_sample_rate: Self::default_trace_sample_rate(),
            dsn: DsnConfig::default(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct DsnConfig {
    backup: Option<String>,
    juicehost: Option<String>,
}

impl DsnConfig {
    #[must_use]
    pub fn dsn(&self) -> Option<&String> {
        self.juicehost.as_ref().or(self.backup.as_ref())
    }
}
