use std::path::PathBuf;

use crate::config::BanFile;

/// IP ban list and backend sync settings.
#[derive(Debug)]
pub struct BanSettings {
    list_file: Option<PathBuf>,
    sync_url: Option<String>,
    sync_interval: u64,
}

impl BanSettings {
    pub const DEFAULT_SYNC_INTERVAL: u64 = 30;

    pub const fn list_file(&self) -> Option<&PathBuf> {
        self.list_file.as_ref()
    }

    pub const fn sync_url(&self) -> Option<&String> {
        self.sync_url.as_ref()
    }

    pub const fn sync_interval(&self) -> u64 {
        self.sync_interval
    }

    pub fn load(file: &BanFile) -> Self {
        let list_file = std::env::var("BAN_LIST_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| file.list_file.clone());
        let sync_url = std::env::var("BAN_SYNC_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .or_else(|| file.sync_url.clone());
        let sync_interval = std::env::var("BAN_SYNC_INTERVAL")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(file.sync_interval_secs);
        Self {
            list_file,
            sync_url,
            sync_interval,
        }
    }
}
