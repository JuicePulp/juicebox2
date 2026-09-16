//! orchestrator

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::watch;

const SERVICES: &[&str] = &["juicehost", "juiceback", "juicefront"];

fn sibling_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn spawn(name: &str) -> std::io::Result<Child> {
    let child = Command::new(sibling_dir().join(name))
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok(child)
}

/// Restart on crash unless JUICERESTART_<NAME>=0 set
fn restart_enabled(name: &str) -> bool {
    std::env::var(format!("JUICERESTART_{}", name.to_uppercase())).map_or(true, |v| v != "0")
}

/// You have 10 seconds before i kill you
async fn graceful_stop(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        if let Some(pid) = child.id() {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
}

#[cfg(unix)]
async fn wait_for_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("SIGINT handler");
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("received SIGTERM, shutting down"),
        _ = sigint.recv() => tracing::info!("received SIGINT, shutting down"),
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("received SIGINT, shutting down");
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let mut children: HashMap<String, Child> = HashMap::new();
    for name in SERVICES {
        match spawn(name) {
            Ok(child) => {
                tracing::info!("spawned {name}");
                children.insert((*name).to_string(), child);
            }
            Err(e) => {
                tracing::error!("failed to spawn {name}: {e}");
            }
        }
    }

    if children.is_empty() {
        tracing::error!("no services could be spawned, exiting");
        std::process::exit(1);
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let sig_task = tokio::spawn(async move {
        wait_for_signal().await;
        let _ = shutdown_tx.send(true);
    });

    let mut shutdown_rx = shutdown_rx;
    loop {
        for name in SERVICES {
            let Some(child) = children.get_mut(*name) else {
                continue;
            };
            let status = tokio::select! {
                _ = shutdown_rx.changed() => None,
                status = child.wait() => status.ok(), // my child is ok :)
            };
            let Some(status) = status else {
                continue;
            };
            children.remove(*name);
            tracing::warn!("{name} exited with {status:?}");
            if restart_enabled(name) {
                tracing::info!("restarting {name}");
                match spawn(name) {
                    Ok(new_child) => {
                        let _ = children.insert((*name).to_string(), new_child);
                    }
                    Err(e) => tracing::error!("failed to restart {name}: {e}"),
                }
            }
        }

        if shutdown_rx.borrow().clone() || children.is_empty() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = sig_task.await;

    for (name, child) in children.iter_mut() {
        tracing::info!("stopping {name}");
        graceful_stop(child).await;
    }

    tracing::info!("juicebox orchestrator exiting");
}
