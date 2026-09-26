use std::{sync::Mutex, time::Instant};

use crate::{
    config::Config,
    ssr::{announce::AnnouncementCache, host::HostCache},
};

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
    pub host_cache: HostCache,
    pub announcement_cache: AnnouncementCache,
    pub github_cache: Mutex<(Option<Instant>, String)>,
    pub rustdoc_exists: bool,
    pub live_reload: bool,
    pub boot_id: String,
    pub shutdown: std::sync::Arc<tokio::sync::Notify>,
}
