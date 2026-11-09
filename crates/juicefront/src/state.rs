use std::{collections::HashMap, net::IpAddr, sync::Mutex, time::Instant};

use crate::{
    config::Config,
    page_cache::PageCache,
    ssr::{announce::AnnouncementCache, ban::BanStatus, host::HostCache},
};

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
    pub host_cache: HostCache,
    pub announcement_cache: AnnouncementCache,
    pub github_cache: Mutex<(Option<Instant>, String)>,
    pub health_cache: Mutex<Option<(Instant, bool)>>,
    pub ban_cache: Mutex<HashMap<IpAddr, (Instant, BanStatus)>>,
    /// Whether the bundled rustdoc tree exists; gates serving `/rustdoc`.
    pub rustdoc_exists: bool,
    pub live_reload: bool,
    /// Regenerated each boot; live-reload clients reload when it changes.
    pub boot_id: String,
    pub page_cache: PageCache,
    /// Fires once on shutdown; long-lived streams select on it to end early.
    pub shutdown: std::sync::Arc<tokio::sync::Notify>,
}
