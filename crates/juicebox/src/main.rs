use tokio::sync::watch;

#[tokio::main]
async fn main() {
    juiceutils::install_panic_handler("juicebox");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    juicebox::load_dotenv();
    tracing::info!("juicebox v{} starting", env!("CARGO_PKG_VERSION"));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let sig_task = tokio::spawn(async move {
        juiceutils::shutdown_signal("juicebox").await;
        let _ = shutdown_tx.send(true);
    });

    // `JUICEBOX_PEER=1` adds the peer relay to the supervised set, linking the
    // local stack to juicepeer so files that stay on this device can be shared.
    let peer = juiceutils::config::optional_secret("JUICEBOX_PEER")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let supervisor = if peer {
        tracing::info!("peer hosting enabled; supervising {}", juicebox::PEER_RELAY_SERVICE);
        juicebox::Supervisor::with_services(juicebox::services_with_peer())
    } else {
        juicebox::Supervisor::new()
    };

    if let Err(e) = supervisor.run(shutdown_rx).await {
        tracing::error!("{e}");
        sig_task.abort();
        std::process::exit(1);
    }

    let _ = sig_task.await;
}
