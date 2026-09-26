use std::path::PathBuf;

use crate::config::BanFile;

#[derive(Debug)]
pub struct BanSettings {
    list_file: Option<PathBuf>,
    sync_url: Option<String>,
    sync_interval: u64,
}

impl BanSettings {
    pub const DEFAULT_SYNC_INTERVAL: u64 = 30;

    #[must_use]
    pub const fn list_file(&self) -> Option<&PathBuf> {
        self.list_file.as_ref()
    }

    #[must_use]
    pub fn sync_url(&self) -> Option<&str> {
        self.sync_url.as_deref()
    }

    #[must_use]
    pub const fn sync_interval(&self) -> u64 {
        self.sync_interval
    }

    #[must_use]
    pub fn load(file: &BanFile) -> Self {
        let list_file = file.list_file.clone();
        let sync_url = file.sync_url.clone();
        let sync_interval = file.sync_interval_secs;
        Self {
            list_file,
            sync_url,
            sync_interval,
        }
    }
}
