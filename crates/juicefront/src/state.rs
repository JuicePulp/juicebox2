use std::{collections::HashMap, net::IpAddr, sync::Mutex, time::Instant};

use crate::{
    config::Config,
    ssr::{
        announce::AnnouncementCache,
        ban::BanStatus,
        host::HostCache,
    },
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
    pub rustdoc_exists: bool,
    pub live_reload: bool,
    pub boot_id: String,
    pub shutdown: std::sync::Arc<tokio::sync::Notify>,
}
