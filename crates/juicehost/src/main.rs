use std::{net::SocketAddr, sync::Arc, time::Duration};

use juicehost::{
    config::Config,
    server::{build_router, print_startup_banner, start_server},
    state::AppState,
    storage::{LocalBackend, S3Backend, StorageBackend},
};
#[cfg(feature = "quic")]
use tokio::sync::Notify;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    juiceutils::install_panic_handler("juicehost");
    juiceutils::config::load_dotenv();

    let config = match Config::try_load() {
        Ok(config) => config,
        Err(error) => juiceutils::exit_on_config_error("juicehost", error),
    };
    // the guard must live for the whole process; dropping it early stops reporting
    let _sentry_guard = juiceutils::bootstrap::init_sentry(
        "SENTRY_DSN_JUICEHOST",
        &config.sentry.environment,
        sentry::release_name!(),
    );

    juiceutils::bootstrap::init_tracing("info", true, true);

    tracing::info!("juicehost v{} starting", env!("CARGO_PKG_VERSION"));

    if config.api_key.is_empty() && !config.allow_no_auth {
        tracing::error!(
            "JUICEHOST_API_KEY is not set - refusing to start with unauthenticated internal endpoints. Configure a key or set JUICEHOST_ALLOW_NO_AUTH=true to override."
        );
        std::process::exit(1);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.worker_threads)
        .enable_all()
        .build()
        .expect("tokio runtime creation failed");

    runtime.block_on(async {
        let mut local_backend: Option<Arc<LocalBackend>> = None;
        let mut s3_backend: Option<Arc<S3Backend>> = None;
        let storage: Arc<dyn StorageBackend> = if config.is_s3_mode() {
            let bucket = config
                .s3_bucket
                .as_deref()
                .expect("is_s3_mode guarantees s3_bucket is set");
            let region = config.s3_region.as_deref().unwrap_or("us-east-1");
            let endpoint = config.s3_endpoint.as_deref();
            let access_key = config.s3_access_key.as_deref().unwrap_or("");
            let secret_key = config.s3_secret_key.as_deref().unwrap_or("");

            tracing::debug!("initialising S3 storage: bucket={bucket}, region={region}");

            let backend = Arc::new(
                S3Backend::new(
                    bucket,
                    region,
                    endpoint,
                    access_key,
                    secret_key,
                    config.s3_allow_http,
                )
                .expect("Failed to create S3 storage backend"),
            );
            s3_backend = Some(Arc::clone(&backend));
            backend
        } else {
            if !config.files_dir.exists() {
                std::fs::create_dir_all(&config.files_dir)
                    .expect("failed to create files directory");
                tracing::info!("created files directory {}", config.files_dir.display());
            }

            let backend = Arc::new(
                LocalBackend::new(config.files_dir.clone(), config.min_free_space_bytes)
                    .expect("invalid files directory"),
            );
            backend
                .init_cache()
                .await
                .expect("invalid files directory entry");
            local_backend = Some(Arc::clone(&backend));
            backend
        };

        if config.audio_embed.enabled() {
            match std::fs::create_dir_all(config.audio_embed.cache_dir()) {
                Ok(()) => tracing::info!(
                    "audio embeds: cache={:?}, cap={} MB, clip={}s",
                    config.audio_embed.cache_dir(),
                    config.audio_embed.cache_max_bytes() / (1024 * 1024),
                    config.audio_embed.max_seconds()
                ),
                Err(e) => tracing::warn!(
                    "audio embeds: could not create cache dir {:?}: {e}",
                    config.audio_embed.cache_dir()
                ),
            }
        }

        let state = Arc::new(AppState::new(&config, storage));
        juicehost::handlers::stats::start_flusher(Arc::clone(&state));
        juicehost::handlers::audio_embed::spawn_pruner(Arc::clone(&state));

        if let Some(local) = local_backend {
            let live = state.live_uploads.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60));
                loop {
                    ticker.tick().await;
                    match local.cleanup_stale(&live).await {
                        Ok(stats) if stats.files > 0 => tracing::info!(
                            "stale upload cleanup removed {} files ({:.2} MiB)",
                            stats.files,
                            stats.bytes as f64 / (1024.0 * 1024.0)
                        ),
                        Ok(_) => {}
                        Err(error) => tracing::warn!("stale upload cleanup failed: {error}"),
                    }
                }
            });
        }

        if let Some(backend) = s3_backend {
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60));
                loop {
                    ticker.tick().await;
                    // Reclaim ID reservations leaked by writers that crashed
                    // mid-upload; the sweep only frees IDs with no published object.
                    match backend.sweep_stale_reservations().await {
                        Ok(removed) if removed > 0 => tracing::info!(
                            "stale S3 reservation sweep removed {removed} IDs"
                        ),
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!("stale S3 reservation sweep failed: {error}")
                        }
                    }
                }
            });
        }

        // without a pepper or a local file the ban list stays disabled, so skip the fetch
        if !config.ip_pepper.is_empty() || config.ban_list_file.is_some() {
            let ban_state = Arc::clone(&state);
            tokio::spawn(async move {
                juicehost::ban::load_ban_file(&ban_state).await;
                juicehost::ban::fetch_snapshot_once(&ban_state).await;
            });
        }

        let app = build_router(Arc::clone(&state));

        let addr = SocketAddr::from((
            config
                .public_host
                .parse::<std::net::IpAddr>()
                .expect("Invalid PUBLIC_HOST"),
            config.public_port,
        ));

        #[cfg(feature = "quic")]
        {
            let shutdown = Arc::new(Notify::new());
            let quic_shutdown = Arc::clone(&shutdown);

            let quic_addr = SocketAddr::from((
                config
                    .quic_host
                    .parse::<std::net::IpAddr>()
                    .expect("Invalid QUIC_HOST"),
                config.quic_port,
            ));

            let quic_router = build_router(Arc::clone(&state));
            let quic_cert = config.quic_cert_path.clone();

            tokio::select! {
                () = async {
                    let quic_limits = juiceutils::QuicServerLimits {
                        max_connections: config.quic_max_connections,
                        max_requests: config.quic_max_requests,
                        handshake_timeout: Duration::from_secs(config.quic_handshake_seconds),
                        idle_timeout: Duration::from_secs(config.quic_idle_seconds),
                        request_timeout: Duration::from_secs(config.quic_request_total_seconds),
                    };
                    let quic_handle = tokio::spawn(juiceutils::start_quic_server_with_limits(quic_router, quic_addr, quic_shutdown, "juicehost", quic_cert, quic_limits));
                    tokio::spawn(async move {
                        match quic_handle.await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => tracing::error!("QUIC server failed: {error}"),
                            Err(error) => tracing::error!("QUIC server task panicked: {error}"),
                        }
                    });
                    print_startup_banner(&config);
                    start_server(app, addr, config.tcp_max_concurrent_requests).await;
                } => {}
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("juicehost shutting down...");
                    shutdown.notify_one();
                }
            }
        }

        #[cfg(not(feature = "quic"))]
        {
            print_startup_banner(&config);
            start_server(app, addr, config.tcp_max_concurrent_requests).await;
        }
    });

    // bound the flush so a stuck Sentry transport cannot delay exit
    if let Some(client) = sentry::Hub::current().client() {
        client.close(Some(Duration::from_secs(2)));
    }
}
