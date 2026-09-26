use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    http::Request,
    middleware,
    routing::{any, get},
};
use mimalloc::MiMalloc;
use sentry::integrations::tower::NewSentryLayer;
use tokio::net::TcpSocket;
use tower_http::{
    compression::{
        CompressionLayer,
        predicate::{DefaultPredicate, NotForContentType, Predicate},
    },
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, util::SubscriberInitExt};

mod admin;
mod config;
mod cx;
mod handlers;
mod i18n;
mod icons;
mod live;
mod proxy;
mod ssr;
mod state;
mod ws;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

async fn shutdown_signal() {
    juiceutils::shutdown_signal("juicefront").await;
}

fn static_base() -> Option<std::path::PathBuf> {
    for candidate in ["crates/juicefront/static", "static"] {
        let path = std::path::PathBuf::from(candidate);
        if path.is_dir() {
            return Some(path);
        }
    }
    None
}

fn main() {
    juiceutils::config::load_dotenv();

    let config = config::Config::load();
    let _sentry_guard = juiceutils::config::optional_secret("SENTRY_DSN_JUICEFRONT")
        .or_else(|| juiceutils::config::optional_secret("SENTRY_DSN"))
        .map(|dsn| {
            let traces_sample_rate = std::env::var("SENTRY_TRACES_SAMPLE_RATE")
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
                .map_or(0.05, |rate| rate.clamp(0.0, 1.0));
            sentry::init((
                dsn.as_str(),
                sentry::ClientOptions::default()
                    .maybe_release(sentry::release_name!())
                    .environment(config.sentry_environment.clone())
                    .traces_sample_rate(traces_sample_rate)
                    .send_default_pii(false),
            ))
        });

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_span_events(FmtSpan::CLOSE))
        .with(sentry_tracing::layer())
        .init();

    tracing::info!("juicefront v{} starting", env!("CARGO_PKG_VERSION"));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");

    runtime.block_on(async {
        let rustdoc_exists = static_base().is_some_and(|base| base.join("public/rustdoc").is_dir());
        let live_reload = live::live_reload_enabled();
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let state = Arc::new(state::AppState {
            http: proxy::build_client(),
            config,
            host_cache: HashMap::new().into(),
            announcement_cache: HashMap::new().into(),
            github_cache: Mutex::new((None, String::new())),
            rustdoc_exists,
            live_reload,
            boot_id: uuid::Uuid::new_v4().to_string(),
            shutdown: Arc::clone(&shutdown),
        });
        tracing::info!(
            "proxy: juiceback={} juicehost={}",
            state.config.juiceback_url,
            state.config.juicehost_url
        );

        crate::ssr::host::prewarm_known_providers(Arc::clone(&state));

        if live_reload {
            tracing::info!("live reload enabled (GET /__live)");
        }

        let compression = CompressionLayer::new()
            .gzip(true)
            .br(true)
            .zstd(true)
            .compress_when(
                DefaultPredicate::new()
                    .and(NotForContentType::new("video/"))
                    .and(NotForContentType::new("audio/"))
                    .and(NotForContentType::new("font/"))
                    .and(NotForContentType::new("application/octet-stream"))
                    .and(NotForContentType::new("application/zip"))
                    .and(NotForContentType::new("application/gzip"))
                    .and(NotForContentType::new("application/offset+octet-stream")),
            );

        let mut pages = Router::new();
        if live_reload {
            pages = pages.route("/__live", get(live::live_handler));
        }
        pages = pages
            .route("/", any(handlers::index))
            .route("/{locale}", any(handlers::index))
            .route("/{locale}/", any(handlers::index))
            .route("/files", get(handlers::files_page))
            .route("/files/", get(handlers::files_page))
            .route("/{locale}/files", get(handlers::files_page))
            .route("/{locale}/files/", get(handlers::files_page))
            .route("/download", get(handlers::download_page))
            .route("/download/", get(handlers::download_page))
            .route("/{locale}/download", get(handlers::download_page))
            .route("/{locale}/download/", get(handlers::download_page))
            .route("/banned", get(handlers::banned_page))
            .route("/banned/", get(handlers::banned_page))
            .route("/{locale}/banned", get(handlers::banned_page))
            .route("/{locale}/banned/", get(handlers::banned_page))
            .route("/report", get(handlers::report_page))
            .route("/report/", get(handlers::report_page))
            .route("/{locale}/report", get(handlers::report_page))
            .route("/{locale}/report/", get(handlers::report_page))
            .route("/feedback", get(handlers::feedback_page))
            .route("/feedback/", get(handlers::feedback_page))
            .route("/{locale}/feedback", get(handlers::feedback_page))
            .route("/{locale}/feedback/", get(handlers::feedback_page))
            .route("/faq", get(handlers::faq_page))
            .route("/faq/", get(handlers::faq_page))
            .route("/{locale}/faq", get(handlers::faq_page))
            .route("/{locale}/faq/", get(handlers::faq_page))
            .route("/docs", get(handlers::docs_page))
            .route("/docs/", get(handlers::docs_page))
            .route("/{locale}/docs", get(handlers::docs_page))
            .route("/{locale}/docs/", get(handlers::docs_page))
            .route("/terms", get(handlers::terms_page))
            .route("/terms/", get(handlers::terms_page))
            .route("/{locale}/terms", get(handlers::terms_page))
            .route("/{locale}/terms/", get(handlers::terms_page))
            .route("/privacy", get(handlers::privacy_page))
            .route("/privacy/", get(handlers::privacy_page))
            .route("/{locale}/privacy", get(handlers::privacy_page))
            .route("/{locale}/privacy/", get(handlers::privacy_page))
            .route("/admin", get(handlers::admin_index))
            .route("/admin/login", get(handlers::admin_login))
            .route("/admin/files", get(handlers::admin_files))
            .route("/admin/bans", get(handlers::admin_bans))
            .route("/admin/reports", get(handlers::admin_reports))
            .route("/admin/feedback", get(handlers::admin_feedback))
            .route("/admin/announcement", get(handlers::admin_announcement))
            .route("/static/bundle.css", get(handlers::css_bundle));

        if let Some(base) = static_base() {
            let cache_policy = if live_reload { "no-store" } else { "no-cache" };
            let revalidate = || {
                tower::ServiceBuilder::new().layer(SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static(cache_policy),
                ))
            };
            pages = pages
                .nest_service(
                    "/static/styles",
                    revalidate().service(ServeDir::new(base.join("styles"))),
                )
                .nest_service("/static/assets", ServeDir::new(base.join("assets")))
                .nest_service(
                    "/static/js",
                    revalidate().service(ServeDir::new(base.join("js"))),
                )
                .route_service(
                    "/static/speculation.json",
                    ServeFile::new(base.join("speculation.json")),
                );
            if rustdoc_exists {
                pages = pages.nest_service("/rustdoc", ServeDir::new(base.join("public/rustdoc")));
            }
        } else {
            tracing::warn!("static asset directory not found, pages will render without styling");
        }

        pages = pages.layer(compression);

        let app = Router::new()
            .merge(pages)
            .route("/api/device/ws", any(ws::device_ws_proxy))
            .fallback(any(proxy::proxy_handler));

        let app = app
            .layer(middleware::from_fn_with_state(
                Arc::clone(&state),
                admin::admin_guard,
            ))
            .layer(middleware::from_fn(juiceutils::add_security_headers))
            .layer(TraceLayer::new_for_http())
            .layer(NewSentryLayer::<Request<Body>>::new_from_top())
            .with_state(state.clone());

        let addr = format!("{}:{}", state.config.ui_host, state.config.ui_port);
        let socket = TcpSocket::new_v4().expect("Failed to create TCP socket");
        socket
            .set_reuseaddr(true)
            .expect("Failed to set SO_REUSEADDR");
        socket
            .bind(addr.parse().expect("Invalid bind address"))
            .expect("Failed to bind to address");
        let listener = socket.listen(1024).expect("Failed to listen");
        tracing::info!("Listening on http://{addr}");
        {
            let shutdown = Arc::clone(&shutdown);
            tokio::spawn(async move {
                shutdown_signal().await;
                shutdown.notify_waiters();
            });
        }
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move { shutdown.notified().await })
        .await
        .expect("Server failed");
    });

    if let Some(client) = sentry::Hub::current().client() {
        client.close(Some(Duration::from_secs(2)));
    }
}
