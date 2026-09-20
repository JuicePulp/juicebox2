use utoipa::OpenApi;

use crate::{
    db::{BanRecord, FeedbackRecord, HosterRecord, ReportRecord},
    routes::{
        admin::{
            AdminBansResponse, AdminFeedbackEntry, AdminFeedbackResponse, AdminFileEntry,
            AdminFilesResponse, AdminHostersResponse, AdminReportEntry, AdminReportsResponse,
            AnnouncementRequest, AnnouncementResponse, BanExportEntry, BanExportResponse,
            BanImportEntry, BanImportRequest, BanImportResponse, BanIpRequest, CheckResponse,
            HosterBanRequest, LoginRequest, LoginResponse,
        },
        manage::{
            ClientFilesRequest, ClientFilesResponse, FileInfoResponse, OwnedFilesRequest,
            OwnedFilesResponse, RenewRequest, RenewResponse,
        },
        pairing::{
            GenerateCodeRequest, GenerateCodeResponse, PairingDevice, VerifyCodeRequest,
            VerifyCodeResponse,
        },
        presence::{DeviceListResponse, PresenceDevice},
        register::{RegisterRequest, RegisterResponse},
        upload::{
            ReserveResponse, ReserveUploadRequest, UltrafastCompleteRequest,
            UltrafastReserveRequest, UltrafastReserveResponse, UploadResponse,
        },
    },
};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Juicebox API",
        description = "Public HTTP API for Juicebox... a privacy-first file hosting service.\n\nUpload files via multipart form, TUS resumable uploads, or ultrafast device-to-device uploads. Manage files by renewing IDs, deleting, or looking up owned files. Pair juicebox-plus devices for direct uploads.\n\nEvery endpoint returns JSON unless you send an `Accept: text/html` header, in which case some return HTML.",
        version = "1.0.0",
        license(name = "GPL-3.0-only", identifier = "GPL-3.0-only"),
        contact(name = "Juicebox", url = "https://box.juicey.dev"),
    ),
    servers(
        (url = "https://box.juicey.dev", description = "Production"),
        (url = "/", description = "Relative to the server root"),
    ),
    paths(
        // Uploads
        crate::routes::upload::multipart::upload_handler,
        crate::routes::upload::multipart::reserve_upload_handler,
        crate::routes::upload::ultrafast::ultrafast_reserve_handler,
        crate::routes::upload::ultrafast::ultrafast_complete_handler,
        crate::routes::upload::ultrafast::device_upload_handler,
        crate::routes::tus::create::create_upload_handler,
        crate::routes::tus::query::get_upload_handler,
        crate::routes::tus::patch::patch_upload_handler,
        crate::routes::tus::query::delete_upload_handler,
        crate::routes::tus::query::options_handler,
        // File management
        crate::routes::manage::file_info_handler,
        crate::routes::manage::delete_file_handler,
        crate::routes::manage::delete_file_form_handler,
        crate::routes::manage::renew_file_id_handler,
        crate::routes::manage::owned_files_handler,
        crate::routes::manage::list_client_files_handler,
        crate::routes::manage::put_client_files_handler,
        // Pairing
        crate::routes::pairing::generate_code_handler,
        crate::routes::pairing::verify_code_handler,
        crate::routes::pairing::list_devices_handler,
        crate::routes::pairing::unpair_device_handler,
        // Devices & presence
        crate::routes::presence::device_status_handler,
        crate::routes::presence::presence_sse_handler,
        crate::routes::presence::ping_device_handler,
        crate::routes::presence::list_connected_devices_handler,
        // General
        crate::routes::health_handler,
        crate::routes::config_handler,
        crate::routes::announcement_handler,
        crate::routes::validate_host_handler,
        crate::routes::ban_status_handler,
        crate::routes::noscript::report_submit_handler,
        crate::routes::noscript::feedback_submit_handler,
        crate::routes::register::register_handler,
        // Internal
        crate::routes::upload::internal::file_status_handler,
        crate::routes::ban_snapshot_handler,
        // Cobalt URL fetching
        crate::routes::fetch::handlers::fetch_start_handler,
        crate::routes::fetch::handlers::fetch_status_handler,
        crate::routes::fetch::handlers::fetch_services_handler,
        // Admin
        crate::routes::admin::auth::login_handler,
        crate::routes::admin::auth::logout_handler,
        crate::routes::admin::auth::check_handler,
        crate::routes::admin::files::list_files_handler,
        crate::routes::admin::files::delete_file_handler,
        crate::routes::admin::reports::list_reports_handler,
        crate::routes::admin::reports::delete_report_handler,
        crate::routes::admin::feedback::list_feedback_handler,
        crate::routes::admin::feedback::delete_feedback_handler,
        crate::routes::admin::bans::list_bans_handler,
        crate::routes::admin::bans::ban_ip_handler,
        crate::routes::admin::bans::unban_ip_handler,
        crate::routes::admin::bans::export_bans_handler,
        crate::routes::admin::bans::import_bans_handler,
        crate::routes::admin::announcement::get_announcement_handler,
        crate::routes::admin::announcement::put_announcement_handler,
        crate::routes::admin::hosters::list_hosters_handler,
        crate::routes::admin::hosters::ban_hoster_handler,
        crate::routes::admin::hosters::unban_hoster_handler,
    ),
    components(schemas(
        // Upload types
        UploadResponse,
        ReserveResponse,
        ReserveUploadRequest,
        UltrafastReserveRequest,
        UltrafastReserveResponse,
        UltrafastCompleteRequest,
        // File management types
        FileInfoResponse,
        RenewResponse,
        RenewRequest,
        OwnedFilesRequest,
        OwnedFilesResponse,
        ClientFilesRequest,
        ClientFilesResponse,
        // Pairing types
        GenerateCodeRequest,
        GenerateCodeResponse,
        VerifyCodeRequest,
        VerifyCodeResponse,
        PairingDevice,
        // Presence types
        DeviceListResponse,
        PresenceDevice,
        // Admin types
        AdminFilesResponse,
        AdminFileEntry,
        AdminReportsResponse,
        AdminReportEntry,
        AdminFeedbackResponse,
        AdminFeedbackEntry,
        AdminBansResponse,
        BanRecord,
        AdminHostersResponse,
        HosterRecord,
        AnnouncementResponse,
        AnnouncementRequest,
        // Admin auth types
        LoginRequest,
        LoginResponse,
        CheckResponse,
        // Admin ban management types
        BanIpRequest,
        BanExportEntry,
        BanExportResponse,
        BanImportEntry,
        BanImportRequest,
        BanImportResponse,
        HosterBanRequest,
        // Reporting types
        ReportRecord,
        FeedbackRecord,
        // Register types
        RegisterRequest,
        RegisterResponse,
        crate::routes::fetch::FetchStartRequest,
        crate::routes::fetch::FetchStartResponse,
        crate::routes::fetch::FetchStatusResponse,
        crate::routes::fetch::FetchServicesResponse,
        crate::routes::fetch::FetchFileResponse,
    )),
    tags(
        (name = "Uploads", description = "File upload endpoints"),
        (name = "Files", description = "File management endpoints"),
        (name = "Pairing", description = "Device pairing endpoints...generate codes, verify codes, list and unpair paired devices."),
        (name = "Devices", description = "Device presence and communication...WebSocket connection, SSE events, ping, and status checks."),
        (name = "Cobalt", description = "JuiceBox x Cobalt.Tools URL fetching endpoints. Disabled unless enabled in [cobalt]."),
        (name = "General", description = "General endpoints"),
        (name = "Internal", description = "Internal endpoints for juiceback-juicehost communication. Protected by shared API key."),
        (name = "Admin", description = "Admin panel endpoints. All admin endpoints require a valid JWT token in a cookie named `token`, obtained via POST /api/admin/login."),
    ),
    modifiers(&SecurityAddon),
)]
pub struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "cookie_auth",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
        components.add_security_scheme(
            "api_key",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
        components.add_security_scheme(
            "device_jwt",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
    }
}
