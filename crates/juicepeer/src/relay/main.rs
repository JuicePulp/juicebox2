//! Entry point of the `juicepeer-relay` binary.

use std::sync::Arc;

use juicepeer::relay::{RelayState, config::Config, routes};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    juiceutils::install_panic_handler("juicepeer-relay");
    juiceutils::config::load_dotenv();

    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        println!("{}", Config::usage());
        return;
    }

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::try_load()
        .unwrap_or_else(|error| juiceutils::exit_on_config_error("juicepeer-relay", error));
    let listen = config.listen_addr();
    tracing::info!(
        "juicepeer-relay v{} listening on {listen} (base {}, backend {})",
        env!("CARGO_PKG_VERSION"),
        config.relay_base_url,
        config.juiceback_url
    );
    tracing::info!(
        "limits: sessions={} frames={}B requests={} timeout={}s heartbeat={}s",
        config.max_sessions,
        config.max_frame_bytes,
        config.max_concurrent_requests,
        config.request_timeout_secs,
        config.heartbeat_interval_secs
    );

    let state = match RelayState::new(config) {
        Ok(state) => Arc::new(state),
        Err(error) => juiceutils::exit_on_config_error("juicepeer-relay", error),
    };
    let app = routes::build(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .unwrap_or_else(|error| panic!("cannot bind {listen}: {error}"));

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|error| panic!("server error: {error}"));
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("juicepeer-relay shutting down");
}
