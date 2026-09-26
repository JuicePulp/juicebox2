use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{i18n, state::AppState};

const CACHE_TTL: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Announcement {
    pub message: Option<String>,
    pub link_url: Option<String>,
    pub mode: Option<String>,
}

pub type AnnouncementCache = Mutex<HashMap<String, (Option<Announcement>, Instant)>>;

pub async fn fetch_announcement(state: &AppState) -> Option<Announcement> {
    let key = state.config.juiceback_url.clone();
    if let Ok(cache) = state.announcement_cache.lock()
        && let Some((cached, at)) = cache.get(&key)
        && at.elapsed() < CACHE_TTL
    {
        return cached.clone();
    }
    let fetched = async {
        let response = tokio::time::timeout(
            FETCH_TIMEOUT,
            state
                .http
                .get(format!("{}/api/announcement", state.config.juiceback_url))
                .send(),
        )
        .await
        .ok()?
        .ok()?;
        if !response.status().is_success() {
            return None;
        }
        tokio::time::timeout(FETCH_TIMEOUT, response.json::<Announcement>())
            .await
            .ok()?
            .ok()
    }
    .await;
    if let Ok(mut cache) = state.announcement_cache.lock() {
        cache.insert(key, (fetched.clone(), Instant::now()));
    }
    fetched
}

#[derive(Debug, Clone)]
pub struct Banner {
    pub show: bool,
    pub mode: String,
    pub message: String,
    pub link_url: String,
    pub learn_more: String,
}

pub async fn banner_for(state: &AppState, locale: &str, banned: bool) -> Banner {
    let announcement = fetch_announcement(state).await;
    build_banner(locale, banned, announcement)
}

pub fn build_banner(locale: &str, banned: bool, announcement: Option<Announcement>) -> Banner {
    if banned {
        return Banner {
            show: true,
            mode: "danger".to_owned(),
            message: i18n::t(locale, "banner.banned_notice"),
            link_url: i18n::lp(locale, "/banned"),
            learn_more: i18n::t(locale, "banner.learn_more"),
        };
    }
    match announcement {
        Some(announcement)
            if announcement
                .message
                .as_deref()
                .is_some_and(|message| !message.is_empty()) =>
        {
            Banner {
                show: true,
                mode: announcement.mode.unwrap_or_else(|| "warning".to_owned()),
                message: announcement.message.unwrap_or_default(),
                link_url: announcement.link_url.unwrap_or_default(),
                learn_more: i18n::t(locale, "banner.learn_more"),
            }
        }
        _ => Banner {
            show: false,
            mode: String::new(),
            message: String::new(),
            link_url: String::new(),
            learn_more: String::new(),
        },
    }
}
