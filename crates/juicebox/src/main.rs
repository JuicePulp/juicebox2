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

    if let Err(e) = juicebox::Supervisor::new().run(shutdown_rx).await {
        tracing::error!("{e}");
        sig_task.abort();
        std::process::exit(1);
    }

    let _ = sig_task.await;
}
