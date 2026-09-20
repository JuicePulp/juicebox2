//! orchestrator

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

use tokio::{
    process::{Child, Command},
    sync::watch,
};

const SERVICES: &[&str] = &["juicehost", "juiceback", "juicefront"];

/// A child that lived less than this long counts as a crash, not a clean run.
const MIN_HEALTHY_SECS: u64 = 10;
/// Stop restarting a service after this many consecutive crashes.
const MAX_CONSECUTIVE_CRASHES: u32 = 5;
/// Upper bound for the restart backoff.
const MAX_BACKOFF_SECS: u64 = 30;

/// Seconds to wait before respawning after `crashes` consecutive crashes.
fn restart_backoff_secs(crashes: u32) -> u64 {
    (1u64 << crashes.min(5)).min(MAX_BACKOFF_SECS)
}

fn sibling_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Load `./.env` (repo root in dev) for vars not already set. Explicit
/// environment always wins; missing file is fine.
fn load_dotenv() {
    juiceutils::config::load_dotenv();
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

/// SIGTERM the child, then wait up to 10s for it to exit.
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

fn check_juiceback_config() -> Result<(), String> {
    let jwt = juiceutils::config::optional_secret("JWT_SECRET")
        .filter(|v| !juiceutils::config::is_placeholder_secret(v));
    if jwt.is_none() {
        return Err(
            "juiceback: JWT_SECRET is not set (or is a placeholder). Set a real secret."
                .to_string(),
        );
    }
    match juiceutils::config::optional_secret("IP_ENCRYPTION_KEY") {
        Some(key) if key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) => {}
        _ => {
            return Err(
                "juiceback: IP_ENCRYPTION_KEY is not set (must be 64 hex chars / 32 bytes)."
                    .to_string(),
            );
        }
    }
    if juiceutils::config::optional_secret("COBALT_ENABLED")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        && juiceutils::config::optional_secret("COBALT_API_KEY").is_none()
    {
        return Err(
            "juiceback: COBALT_ENABLED is set but COBALT_API_KEY is not. Set it or disable COBALT_ENABLED."
                .to_string(),
        );
    }
    Ok(())
}

fn check_juicehost_config() -> Result<(), String> {
    if juiceutils::config::optional_secret("JUICEHOST_API_KEY").is_none()
        && !juiceutils::config::optional_secret("JUICEHOST_ALLOW_NO_AUTH")
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        return Err(
            "juicehost: JUICEHOST_API_KEY is not set. Set a key or JUICEHOST_ALLOW_NO_AUTH=true."
                .to_string(),
        );
    }
    Ok(())
}

const fn check_juicefront_config() -> Result<(), String> {
    Ok(())
}

/// Minimal startup-blocking config check, before spawning anything.
/// Catches missing secrets that would otherwise crash-loop the services.
fn check_service_config(name: &str) -> Result<(), String> {
    match name {
        "juiceback" => check_juiceback_config(),
        "juicehost" => check_juicehost_config(),
        "juicefront" => check_juicefront_config(),
        _ => Ok(()),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    load_dotenv();

    let mut failed_config = false;
    for name in SERVICES {
        if let Err(e) = check_service_config(name) {
            tracing::error!("{e}");
            failed_config = true;
        }
    }
    if failed_config {
        tracing::error!("missing required configuration, not starting services");
        std::process::exit(1);
    }

    let mut children: HashMap<String, Child> = HashMap::new();
    let mut spawned_at: HashMap<String, Instant> = HashMap::new();
    let mut crashes: HashMap<String, u32> = HashMap::new();
    let mut pending: HashMap<String, Instant> = HashMap::new();

    for name in SERVICES {
        match spawn(name) {
            Ok(child) => {
                tracing::info!("spawned {name}");
                children.insert((*name).to_string(), child);
                spawned_at.insert((*name).to_string(), Instant::now());
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

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let sig_task = tokio::spawn(async move {
        juiceutils::shutdown_signal("juicebox").await;
        let _ = shutdown_tx.send(true);
    });

    loop {
        for name in SERVICES {
            let Some(child) = children.get_mut(*name) else {
                continue;
            };
            let status = tokio::select! {
                _ = shutdown_rx.changed() => None,
                status = child.wait() => status.ok(),
            };
            let Some(status) = status else {
                continue;
            };
            children.remove(*name);
            let runtime_secs = spawned_at
                .remove(*name)
                .map_or(0, |t| t.elapsed().as_secs());
            tracing::warn!("{name} exited with {status:?} after {runtime_secs}s");
            if !restart_enabled(name) {
                continue;
            }
            let count = if runtime_secs >= MIN_HEALTHY_SECS {
                0
            } else {
                crashes.get(*name).copied().unwrap_or(0) + 1
            };
            crashes.insert((*name).to_string(), count);
            if count >= MAX_CONSECUTIVE_CRASHES {
                tracing::error!(
                    "{name} crashed {count} times in a row within {MIN_HEALTHY_SECS}s of startup, \
                     not restarting (likely a config error outside the pre-start check). \
                     Fix it and restart juicebox."
                );
                continue;
            }
            let delay = restart_backoff_secs(count);
            tracing::info!("restarting {name} in {delay}s (crash #{count})");
            if !*shutdown_rx.borrow() {
                pending.insert(
                    (*name).to_string(),
                    Instant::now() + Duration::from_secs(delay),
                );
            }
        }

        let now = Instant::now();
        let due: Vec<String> = pending
            .iter()
            .filter(|(_, at)| now >= **at)
            .map(|(name, _)| name.clone())
            .collect();
        for name in due {
            pending.remove(&name);
            if !restart_enabled(&name) {
                continue;
            }
            match spawn(&name) {
                Ok(new_child) => {
                    tracing::info!("spawned {name}");
                    children.insert(name.clone(), new_child);
                    spawned_at.insert(name, Instant::now());
                }
                Err(e) => {
                    tracing::error!("failed to restart {name}: {e}");
                    let count = crashes.get(&name).copied().unwrap_or(0) + 1;
                    crashes.insert(name.clone(), count);
                    if count >= MAX_CONSECUTIVE_CRASHES {
                        tracing::error!(
                            "{name} failed to spawn {count} times in a row, not retrying"
                        );
                    } else {
                        let delay = restart_backoff_secs(count);
                        pending.insert(name, Instant::now() + Duration::from_secs(delay));
                    }
                }
            }
        }

        if *shutdown_rx.borrow() {
            break;
        }
        if children.is_empty() && pending.is_empty() {
            tracing::error!("all services failed, exiting");
            std::process::exit(1);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = sig_task.await;

    for (name, child) in &mut children {
        tracing::info!("stopping {name}");
        graceful_stop(child).await;
    }

    tracing::info!("juicebox orchestrator exiting");
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn lock(vars: &[&str]) -> (std::sync::MutexGuard<'static, ()>, Self) {
            let guard = ENV_LOCK.lock().unwrap();
            let saved = vars
                .iter()
                .map(|v| (v.to_string(), std::env::var(v).ok()))
                .collect();
            (guard, Self { saved })
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in std::mem::take(&mut self.saved) {
                match value {
                    Some(v) => unsafe { std::env::set_var(&name, v) },
                    None => unsafe { std::env::remove_var(&name) },
                }
            }
        }
    }

    const VARS: &[&str] = &[
        "JWT_SECRET",
        "IP_ENCRYPTION_KEY",
        "COBALT_ENABLED",
        "COBALT_API_KEY",
        "JUICEHOST_API_KEY",
        "JUICEHOST_ALLOW_NO_AUTH",
    ];

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(restart_backoff_secs(0), 1);
        assert_eq!(restart_backoff_secs(1), 2);
        assert_eq!(restart_backoff_secs(2), 4);
        assert_eq!(restart_backoff_secs(4), 16);
        assert_eq!(restart_backoff_secs(5), 30);
        assert_eq!(restart_backoff_secs(100), 30);
    }

    #[test]
    fn placeholder_secrets_rejected() {
        use juiceutils::config::is_placeholder_secret;

        assert!(is_placeholder_secret(""));
        assert!(is_placeholder_secret("   "));
        assert!(is_placeholder_secret("change_this_to_a_random_value"));
        assert!(is_placeholder_secret("change_this_in_production"));
        assert!(!is_placeholder_secret("a-real-secret-value"));
    }

    #[test]
    fn juiceback_requires_secrets() {
        let (_lock, _guard) = EnvGuard::lock(VARS);
        unsafe {
            std::env::remove_var("JWT_SECRET");
            std::env::remove_var("IP_ENCRYPTION_KEY");
            std::env::remove_var("COBALT_ENABLED");
        }
        assert!(check_juiceback_config().is_err());

        unsafe {
            std::env::set_var("JWT_SECRET", "test-secret");
            std::env::set_var(
                "IP_ENCRYPTION_KEY",
                "0000000000000000000000000000000000000000000000000000000000000001",
            );
        }
        assert!(check_juiceback_config().is_ok());

        unsafe {
            std::env::set_var("JWT_SECRET", "change_this_to_a_random_value");
        }
        assert!(check_juiceback_config().is_err());
    }

    #[test]
    fn juiceback_rejects_bad_encryption_key() {
        let (_lock, _guard) = EnvGuard::lock(VARS);
        unsafe {
            std::env::set_var("JWT_SECRET", "test-secret");
            std::env::set_var("IP_ENCRYPTION_KEY", "not-hex");
            std::env::remove_var("COBALT_ENABLED");
        }
        assert!(check_juiceback_config().is_err());
    }

    #[test]
    fn juicehost_requires_api_key_or_override() {
        let (_lock, _guard) = EnvGuard::lock(VARS);
        unsafe {
            std::env::remove_var("JUICEHOST_API_KEY");
            std::env::remove_var("JUICEHOST_ALLOW_NO_AUTH");
        }
        assert!(check_juicehost_config().is_err());

        unsafe {
            std::env::set_var("JUICEHOST_ALLOW_NO_AUTH", "true");
        }
        assert!(check_juicehost_config().is_ok());

        unsafe {
            std::env::remove_var("JUICEHOST_ALLOW_NO_AUTH");
            std::env::set_var("JUICEHOST_API_KEY", "key");
        }
        assert!(check_juicehost_config().is_ok());
    }
}
