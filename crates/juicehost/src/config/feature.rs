use crate::config::{ConfigError, FeaturesFile};

/// Feature toggle and TTL settings.
#[derive(Debug)]
pub struct FeatureSettings {
    quick_link: bool,
    custom_id: bool,
    file_cache_enabled: bool,
    file_cache_max_age_secs: u64,
    default_ttl_hours: f64,
    allowed_ttl_hours: Vec<f64>,
}

impl FeatureSettings {
    pub const fn quick_link(&self) -> bool {
        self.quick_link
    }

    pub const fn custom_id(&self) -> bool {
        self.custom_id
    }

    pub const fn file_cache_enabled(&self) -> bool {
        self.file_cache_enabled
    }

    pub const fn file_cache_max_age_secs(&self) -> u64 {
        self.file_cache_max_age_secs
    }

    pub const fn default_ttl_hours(&self) -> f64 {
        self.default_ttl_hours
    }

    pub fn allowed_ttl_hours(&self) -> &[f64] {
        &self.allowed_ttl_hours
    }

    pub fn load(file: &FeaturesFile) -> Result<Self, ConfigError> {
        let quick_link = file.quick_link;
        let custom_id = file.custom_id;
        let file_cache_enabled = file.file_cache_enabled;
        let file_cache_max_age_secs = file.file_cache_max_age_secs;
        let allowed_ttl_hours: Vec<f64> = file.allowed_ttl_hours.clone();
        let default_ttl_hours = file.default_ttl_hours;

        if allowed_ttl_hours.is_empty()
            || allowed_ttl_hours
                .iter()
                .any(|ttl| !ttl.is_finite() || *ttl <= 0.0)
            || !default_ttl_hours.is_finite()
            || default_ttl_hours <= 0.0
            || !allowed_ttl_hours.contains(&default_ttl_hours)
        {
            return Err(ConfigError::InvalidTtlConfiguration);
        }

        Ok(Self {
            quick_link,
            custom_id,
            file_cache_enabled,
            file_cache_max_age_secs,
            default_ttl_hours,
            allowed_ttl_hours,
        })
    }
}
