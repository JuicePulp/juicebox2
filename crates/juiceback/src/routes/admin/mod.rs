use std::sync::Arc;

use axum::Router;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};

use crate::{state::AppState, utils::TrustedClientIpKeyExtractor};

pub mod announcement;
pub mod auth;
pub mod bans;
pub mod common;
pub mod feedback;
pub mod files;
pub mod hosters;
pub mod reports;

pub use announcement::{
    AnnouncementRequest, AnnouncementResponse, get_announcement_handler, put_announcement_handler,
};
pub use auth::{
    CheckResponse, LoginRequest, LoginResponse, check_handler, login_handler, logout_handler,
};
pub use bans::{
    AdminBansResponse, BanExportEntry, BanExportResponse, BanImportEntry, BanImportRequest,
    BanImportResponse, BanIpRequest, ban_ip_handler, export_bans_handler, import_bans_handler,
    list_bans_handler, unban_ip_handler,
};
pub use common::{AdminUser, ListParams};
pub use feedback::{
    AdminFeedbackEntry, AdminFeedbackResponse, delete_feedback_handler, list_feedback_handler,
};
pub use files::{AdminFileEntry, AdminFilesResponse, delete_file_handler, list_files_handler};
pub use hosters::{
    AdminHostersResponse, HosterBanRequest, ban_hoster_handler, list_hosters_handler,
    unban_hoster_handler,
};
pub use reports::{
    AdminReportEntry, AdminReportsResponse, delete_report_handler, list_reports_handler,
};

#[must_use]
pub fn admin_routes(trusted_proxy_cidrs: &[juiceutils::proxy::IpCidr]) -> Router<Arc<AppState>> {
    let conf = {
        let mut builder = GovernorConfigBuilder::default();
        builder.period(std::time::Duration::from_secs_f64(
            crate::constants::ADMIN_RATE_LIMIT_WINDOW_SECS as f64
                / crate::constants::ADMIN_RATE_LIMIT_BURST as f64,
        ));
        builder.burst_size(crate::constants::ADMIN_RATE_LIMIT_BURST);
        Arc::new(
            builder
                .key_extractor(TrustedClientIpKeyExtractor::new(
                    trusted_proxy_cidrs.to_vec(),
                ))
                .finish()
                .expect("admin rate-limiter config must build"),
        )
    };
    let login_limiter = Router::new()
        .route("/api/admin/login", axum::routing::post(login_handler))
        .layer(GovernorLayer::new(Arc::clone(&conf)));

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(120));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            conf.limiter().retain_recent();
        }
    });

    Router::new()
        .merge(login_limiter)
        .route(
            "/api/admin/logout",
            axum::routing::get(logout_handler).post(logout_handler),
        )
        .route("/api/admin/check", axum::routing::get(check_handler))
        .route("/api/admin/files", axum::routing::get(list_files_handler))
        .route(
            "/api/admin/file/{id}",
            axum::routing::delete(delete_file_handler),
        )
        .route(
            "/api/admin/reports",
            axum::routing::get(list_reports_handler),
        )
        .route(
            "/api/admin/report/{id}",
            axum::routing::delete(delete_report_handler),
        )
        .route(
            "/api/admin/bans",
            axum::routing::get(list_bans_handler).post(ban_ip_handler),
        )
        .route(
            "/api/admin/bans/export",
            axum::routing::get(export_bans_handler),
        )
        .route(
            "/api/admin/bans/import",
            axum::routing::post(import_bans_handler),
        )
        .route(
            "/api/admin/ban/{ip}",
            axum::routing::delete(unban_ip_handler),
        )
        .route(
            "/api/admin/announcement",
            axum::routing::get(get_announcement_handler).put(put_announcement_handler),
        )
        .route(
            "/api/admin/feedback",
            axum::routing::get(list_feedback_handler),
        )
        .route(
            "/api/admin/feedback/{id}",
            axum::routing::delete(delete_feedback_handler),
        )
        .route(
            "/api/admin/hosters",
            axum::routing::get(list_hosters_handler),
        )
        .route(
            "/api/admin/hosters/{host}/ban",
            axum::routing::post(ban_hoster_handler).delete(unban_hoster_handler),
        )
}
