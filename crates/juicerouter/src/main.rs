mod config;
mod routes;
mod state;

use std::{path::PathBuf, sync::Arc, time::Duration};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    juiceutils::install_panic_handler("juicerouter");
    juiceutils::config::load_dotenv();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let path = std::env::var("JUICEROUTER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("juicerouter.toml"));
    tracing::info!("juicerouter v{} starting", env!("CARGO_PKG_VERSION"));
    tracing::debug!("config: {}", path.display());
    let config = config::Config::load(&path).with_env();
    let listen = format!("{}:{}", config.server.host, config.server.port);
    let probe_interval = config.router.probe_interval_secs.max(5);

    let state = state::AppState::new(config);
    let regions = state
        .registry
        .nodes
        .iter()
        .map(|node| node.region.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    tracing::info!(
        "router: {} node(s) [{regions}], probing every {probe_interval}s",
        state.registry.nodes.len()
    );

    let health_state = Arc::clone(&state);
    tokio::spawn(async move { health_loop(health_state).await });

    let app = routes::build(state);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|error| panic!("cannot bind {listen}: {error}"));
    tracing::info!("juicerouter listening on {listen}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|error| panic!("server error: {error}"));
}

async fn health_loop(state: Arc<state::AppState>) {
    let interval = Duration::from_secs(state.config.router.probe_interval_secs.max(5));
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        for node in &state.registry.nodes {
            state.probe_node(node).await;
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("juicerouter shutting down");
}
