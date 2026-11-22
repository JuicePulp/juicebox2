mod config;
mod routes;
mod state;

use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    juiceutils::install_panic_handler("juicerouter");
    juiceutils::config::load_dotenv();

    juiceutils::bootstrap::init_tracing("info", false, false);

    let path = std::env::var("JUICEROUTER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("juicerouter.toml"));
    tracing::info!("juicerouter v{} starting", env!("CARGO_PKG_VERSION"));
    tracing::debug!("config: {}", path.display());
    let config = config::Config::load(&path)
        .unwrap_or_else(|error| juiceutils::exit_on_config_error("juicerouter", error))
        .with_env();
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
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move { health_loop(health_state, shutdown_rx).await });

    let app = routes::build(state);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|error| panic!("cannot bind {listen}: {error}"));
    tracing::info!("juicerouter listening on {listen}");

    let server_shutdown = shutdown_tx;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = server_shutdown.send(true);
            tracing::info!("juicerouter shutting down");
        })
        .await
        .unwrap_or_else(|error| panic!("server error: {error}"));
}

// The 5s floor stops a tiny configured interval from hammering nodes.
async fn health_loop(
    state: Arc<state::AppState>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let interval = Duration::from_secs(state.config.router.probe_interval_secs.max(5));
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
            _ = ticker.tick() => {
                for node in &state.registry.nodes {
                    state.probe_node(node).await;
                }
            }
        }
    }
}
