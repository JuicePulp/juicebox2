use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime},
};

use tokio::{
    process::{Child, Command},
    sync::watch,
};

const SERVICES: &[&str] = &["juicehost", "juiceback", "juicefront"];

/// Local backend origin assumed when the orchestrator launches juicehost
/// without an explicit `BACKEND_URL`. Matches the default backend port.
const DEFAULT_LOCAL_BACKEND_URL: &str = "http://127.0.0.1:6401";

const MIN_HEALTHY_SECS: u64 = 10;

const MAX_CONSECUTIVE_CRASHES: u32 = 5;

const MAX_BACKOFF_SECS: u64 = 30;

fn restart_backoff_secs(crashes: u32) -> u64 {
    (1u64 << crashes.min(5)).min(MAX_BACKOFF_SECS)
}

fn sibling_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Workspace root when running from `target/debug` or `target/release`.
/// Returns `None` for installed binaries outside a checkout.
fn workspace_dir() -> Option<PathBuf> {
    let dir = sibling_dir();
    let profile = dir.file_name()?.to_str()?;
    if profile != "debug" && profile != "release" {
        return None;
    }
    let target = dir.parent()?;
    if target.file_name()?.to_str()? != "target" {
        return None;
    }
    let root = target.parent()?;
    if !root.join("Cargo.toml").is_file() {
        return None;
    }
    Some(root.to_path_buf())
}

/// Build service binaries before spawning. Always runs (a no-op when
/// fresh) so `cargo run` never supervises stale code after `target/`
/// rebuilds or branch switches. Set `JUICEBOX_NO_BUILD=1` to skip (e.g.
/// production images with prebuilt binaries). `cargo run` releases the
/// target-dir lock once the orchestrator itself is built, so invoking
/// cargo here cannot deadlock.
///
/// Returns whether the services are (now) built: `false` when the build
/// itself failed.
fn ensure_services_built() -> bool {
    if juiceutils::config::optional_secret("JUICEBOX_NO_BUILD")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    let Some(root) = workspace_dir() else {
        tracing::warn!(
            "no workspace checkout found; run `cargo build` first so all services are compiled"
        );
        return false;
    };
    tracing::info!("building services");
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("build");
    // The orchestrator spawns binaries next to itself, so the inner build
    // must target the same profile: `cargo run --release` needs release
    // service binaries, plain `cargo run` needs debug ones.
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    cmd.args(SERVICES.iter().flat_map(|name| ["-p", name]))
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match cmd.status() {
        Ok(status) if status.success() => {
            tracing::info!("service build finished");
            true
        }
        Ok(status) => {
            tracing::error!("service build failed with {status}; run `cargo build` manually");
            false
        }
        Err(e) => {
            tracing::error!(
                "could not run `cargo build`: {e}; run `cargo build` manually so all services are compiled"
            );
            false
        }
    }
}

/// Whether the source watcher (live rebuild + restart) is active.
///
/// Explicit `JUICEBOX_WATCH=1` forces it on, `=0`/`false` forces it off.
/// Otherwise it follows the build: disabled together with
/// `JUICEBOX_NO_BUILD=1` (rebuilding is the watcher's whole job), enabled
/// in every other case where a workspace checkout exists to watch.
fn watch_enabled() -> bool {
    if let Some(v) = juiceutils::config::optional_secret("JUICEBOX_WATCH") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    !juiceutils::config::optional_secret("JUICEBOX_NO_BUILD")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// File kinds that participate in live rebuilds: sources, templates,
/// static assets, icons, locale dicts, and crate/root manifests.
fn watchable_file(path: &Path) -> bool {
    let visible = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| !name.starts_with('.'));
    if !visible {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext,
                "rs" | "html"
                    | "js"
                    | "mjs"
                    | "cjs"
                    | "css"
                    | "json"
                    | "toml"
                    | "svg"
                    | "ttf"
                    | "woff"
                    | "woff2"
                    | "png"
                    | "webp"
                    | "ico"
                    | "txt"
            )
        })
}

/// Directories and files whose changes rebuild + restart the services.
/// Deliberately narrow: `target/`, `ui/` (currently unserved legacy mirror
/// with its own `node_modules/` + `dist/`), data dirs, and databases stay
/// out. `.env` stays out too: secrets are baked into this process's
/// environment at startup, so a respawn would silently keep stale values —
/// changing secrets still needs a full `cargo run` restart.
fn watch_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "juiceback.toml",
        "juicehost.toml",
        "juicefront.toml",
    ] {
        let path = root.join(name);
        if path.is_file() {
            roots.push(path);
        }
    }
    let Ok(crates) = std::fs::read_dir(root.join("crates")) else {
        return roots;
    };
    for entry in crates.flatten() {
        let krate = entry.path();
        for sub in ["src", "templates", "static", "i18n", "icons"] {
            let dir = krate.join(sub);
            if dir.is_dir() {
                roots.push(dir);
            }
        }
        let manifest = krate.join("Cargo.toml");
        if manifest.is_file() {
            roots.push(manifest);
        }
    }
    roots
}

fn scan_dir(dir: &Path, out: &mut HashMap<PathBuf, SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            scan_dir(&path, out);
            continue;
        }
        if !kind.is_file() || !watchable_file(&path) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&path)
            && let Ok(mtime) = meta.modified()
        {
            out.insert(path, mtime);
        }
    }
}

/// Dependency-free polling watcher: snapshots mtimes under the watch roots
/// about once a second and reports when the snapshot differs (covers add,
/// modify, and delete). Cheap enough to run inline in the supervise loop.
struct FsWatcher {
    roots: Vec<PathBuf>,
    last: HashMap<PathBuf, SystemTime>,
    last_scan: Instant,
}

impl FsWatcher {
    fn new(root: &Path) -> Option<Self> {
        let roots = watch_roots(root);
        if roots.is_empty() {
            return None;
        }
        let mut watcher = Self {
            roots,
            last: HashMap::new(),
            last_scan: Instant::now(),
        };
        watcher.resnapshot();
        tracing::info!(
            "watching {} locations for live rebuilds",
            watcher.roots.len()
        );
        Some(watcher)
    }

    fn snapshot(&self) -> HashMap<PathBuf, SystemTime> {
        let mut next = HashMap::new();
        for root in &self.roots {
            if root.is_dir() {
                scan_dir(root, &mut next);
            } else if watchable_file(root)
                && let Ok(meta) = std::fs::metadata(root)
                && let Ok(mtime) = meta.modified()
            {
                next.insert(root.clone(), mtime);
            }
        }
        next
    }

    fn resnapshot(&mut self) {
        self.last = self.snapshot();
        self.last_scan = Instant::now();
    }

    /// Returns `true` once per detected change set.
    fn poll(&mut self) -> bool {
        if self.last_scan.elapsed() < Duration::from_millis(500) {
            return false;
        }
        let next = self.snapshot();
        self.last_scan = Instant::now();
        if next == self.last {
            return false;
        }
        self.last = next;
        true
    }
}

/// Load `./.env` (repo root in dev) for vars not already set. Explicit
/// environment always wins; missing file is fine.
fn load_dotenv() {
    juiceutils::config::load_dotenv();
}

/// Backend origin injected into the juicehost child when the operator did
/// not set `BACKEND_URL` (unset or blank). `Some` means "apply the local
/// default", `None` means "respect the explicit value".
fn host_backend_default() -> Option<&'static str> {
    if std::env::var("BACKEND_URL").is_ok_and(|v| !v.trim().is_empty()) {
        None
    } else {
        Some(DEFAULT_LOCAL_BACKEND_URL)
    }
}

fn spawn(name: &str) -> std::io::Result<Child> {
    let mut cmd = Command::new(sibling_dir().join(name));
    // The orchestrator always runs a local backend next to the host: point
    // the host at it unless the operator set BACKEND_URL explicitly.
    // Without this the host runs backendless and password gates never apply.
    if name == "juicehost"
        && let Some(default) = host_backend_default()
    {
        cmd.env("BACKEND_URL", default);
        tracing::info!("juicehost: BACKEND_URL unset, defaulting to {default}");
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "service binary '{name}' not found next to the juicebox binary; \
                         run `cargo build` first so all services are compiled"
                    ),
                )
            } else {
                e
            }
        })?;
    Ok(child)
}

fn restart_enabled(name: &str) -> bool {
    std::env::var(format!("JUICERESTART_{}", name.to_uppercase())).map_or(true, |v| v != "0")
}

async fn graceful_stop(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        if let Some(pid) = child.id() {
            libc::kill(pid.cast_signed(), libc::SIGTERM);
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

fn check_service_config(name: &str) -> Result<(), String> {
    match name {
        "juiceback" => check_juiceback_config(),
        "juicehost" => check_juicehost_config(),
        _ => Ok(()),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
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

    ensure_services_built();

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

    // Live editing: watch workspace sources and rebuild + restart the
    // services on change. Respawns mint fresh boot IDs, so browsers reload
    // on their own via the live channel. Inert without a checkout
    // (production images) or when explicitly disabled.
    let mut watcher = if watch_enabled() {
        workspace_dir().and_then(|root| FsWatcher::new(&root))
    } else {
        None
    };
    if watcher.is_some() {
        tracing::info!(
            "watching workspace sources for live rebuilds (JUICEBOX_WATCH=0 to disable)"
        );
    }

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let sig_task = tokio::spawn(async move {
        juiceutils::shutdown_signal("juicebox").await;
        let _ = shutdown_tx.send(true);
    });

    loop {
        // Live rebuild: a detected source change rebuilds, and on success
        // gracefully restarts every service. The build runs synchronously
        // (children keep serving meanwhile); a failed build keeps the old
        // binaries running. Snapshots refresh after handling so one change
        // set restarts exactly once.
        if watcher.as_mut().is_some_and(FsWatcher::poll) {
            tracing::info!("sources changed, rebuilding services for live reload");
            if *shutdown_rx.borrow() {
                // Shutting down: leave the restart to the exit path.
            } else if ensure_services_built() {
                for (name, child) in &mut children {
                    tracing::info!("stopping {name} for live reload");
                    graceful_stop(child).await;
                }
                children.clear();
                spawned_at.clear();
                crashes.clear();
                pending.clear();
                for name in SERVICES {
                    match spawn(name) {
                        Ok(child) => {
                            tracing::info!("spawned {name}");
                            children.insert((*name).to_string(), child);
                            spawned_at.insert((*name).to_string(), Instant::now());
                        }
                        Err(e) => {
                            tracing::error!("failed to respawn {name}: {e}");
                        }
                    }
                }
                if children.is_empty() {
                    tracing::error!("live reload respawn failed for all services, exiting");
                    std::process::exit(1);
                }
            } else {
                tracing::error!(
                    "live reload build failed, keeping previous service binaries running"
                );
            }
            if let Some(watcher) = watcher.as_mut() {
                watcher.resnapshot();
            }
        }

        for name in SERVICES {
            let Some(child) = children.get_mut(*name) else {
                continue;
            };
            let status = tokio::select! {
                _ = shutdown_rx.changed() => None,
                status = child.wait() => status.ok(),
                // Tick so the loop keeps cycling while children are
                // healthy: without this the watch check below only runs
                // when a child exits, and live rebuilds never fire.
                // Short tick keeps save-to-restart latency low.
                () = tokio::time::sleep(Duration::from_millis(250)) => None,
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
        "BACKEND_URL",
    ];

    #[test]
    fn watchable_file_picks_sources_not_dotfiles() {
        assert!(watchable_file(Path::new("src/main.rs")));
        assert!(watchable_file(Path::new("templates/index.html")));
        assert!(watchable_file(Path::new("static/js/app.js")));
        assert!(watchable_file(Path::new("static/styles/app.css")));
        assert!(watchable_file(Path::new("i18n/en.json")));
        assert!(watchable_file(Path::new("Cargo.toml")));
        assert!(watchable_file(Path::new("icons/logo.svg")));
        assert!(!watchable_file(Path::new("target/debug/app")));
        assert!(!watchable_file(Path::new(".env")));
        assert!(!watchable_file(Path::new("notes.md")));
        assert!(!watchable_file(Path::new("file")));
        assert!(!watchable_file(Path::new(".hidden.rs")));
    }

    #[test]
    fn watcher_detects_add_modify_delete() {
        fn aged() -> Instant {
            Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or_else(Instant::now)
        }
        let root = std::env::temp_dir().join(format!(
            "juicebox-watch-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let src = root.join("crates").join("demo").join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let mut watcher = FsWatcher::new(&root).expect("watch roots");
        // Fresh snapshot: nothing to report until something changes.
        watcher.last_scan = Instant::now();
        assert!(!watcher.poll());

        std::fs::write(&file, "fn main() { /* edited */ }").unwrap();
        watcher.last_scan = aged();
        assert!(watcher.poll());
        // Same state again: quiet.
        watcher.last_scan = aged();
        assert!(!watcher.poll());

        std::fs::remove_file(&file).unwrap();
        watcher.last_scan = aged();
        assert!(watcher.poll());

        std::fs::remove_dir_all(&root).ok();
    }

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
    fn host_backend_default_applies_local_origin() {
        let (_lock, _guard) = EnvGuard::lock(VARS);

        unsafe {
            std::env::remove_var("BACKEND_URL");
        }
        assert_eq!(host_backend_default(), Some("http://127.0.0.1:6401"));

        unsafe {
            std::env::set_var("BACKEND_URL", "   ");
        }
        assert_eq!(host_backend_default(), Some("http://127.0.0.1:6401"));

        unsafe {
            std::env::set_var("BACKEND_URL", "https://back.example");
        }
        assert_eq!(host_backend_default(), None);
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
