//! Process supervision library for the Juicebox orchestrator.
//!
//! This crate exposes the machinery that used to live entirely in the
//! `juicebox` binary: building and watching the workspace sources, checking
//! service configuration, spawning and gracefully stopping child processes,
//! and the supervision loop itself. The binary is now a thin wrapper that
//! installs a panic handler, sets up logging and shutdown signalling, and
//! hands control to [`Supervisor::run`].
//!
//! # Example
//!
//! ```no_run
//! use tokio::sync::watch;
//!
//! # async fn example() -> Result<(), juicebox::SupervisorError> {
//! let (_shutdown_tx, shutdown_rx) = watch::channel(false);
//! juicebox::Supervisor::new().run(shutdown_rx).await?;
//! # Ok(())
//! # }
//! ```

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime},
};

use thiserror::Error;
use tokio::{
    process::{Child, Command},
    sync::watch,
};

/// Services managed by the default supervisor.
pub const SERVICES: &[&str] = &["juicehost", "juiceback", "juicefront"];

/// Minimum uptime after which a service exit is considered a clean run rather
/// than a crash.
pub const MIN_HEALTHY_SECS: u64 = 10;

/// Number of consecutive fast crashes after which a service is not restarted.
pub const MAX_CONSECUTIVE_CRASHES: u32 = 5;

/// Upper bound of the exponential restart backoff, in seconds.
pub const MAX_BACKOFF_SECS: u64 = 30;

/// Exponential backoff curve used for restarting crashed services.
pub fn restart_backoff_secs(crashes: u32) -> u64 {
    (1u64 << crashes.min(5)).min(MAX_BACKOFF_SECS)
}

/// Directory containing the currently executing binary.
pub fn sibling_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Locate the workspace checkout when running from a Cargo `target` profile
/// directory.
pub fn workspace_dir() -> Option<PathBuf> {
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

/// Result of attempting to pre-build the managed services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildOutcome {
    Built,
    Skipped,
    Failed,
}

/// `JUICEBOX_NO_BUILD=1` skips the build and expects binaries to exist.
pub async fn ensure_services_built() -> BuildOutcome {
    if juiceutils::config::optional_secret("JUICEBOX_NO_BUILD")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        return BuildOutcome::Skipped;
    }
    let Some(root) = workspace_dir() else {
        tracing::warn!(
            "no workspace checkout found; run `cargo build` first so all services are compiled"
        );
        return BuildOutcome::Skipped;
    };
    tracing::info!("building services (this can take a minute)");
    let mut cmd = Command::new("cargo");
    cmd.arg("build");

    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    cmd.args(SERVICES.iter().flat_map(|name| ["-p", name]))
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match cmd.status().await {
        Ok(status) if status.success() => {
            tracing::info!("services built");
            BuildOutcome::Built
        }
        Ok(status) => {
            tracing::error!("service build failed with {status}; run `cargo build` manually");
            BuildOutcome::Failed
        }
        Err(e) => {
            tracing::error!(
                "could not run `cargo build`: {e}; run `cargo build` manually so all services are compiled"
            );
            BuildOutcome::Failed
        }
    }
}

/// `JUICEBOX_WATCH=0` disables watching; `JUICEBOX_NO_BUILD=1` implies it.
pub fn watch_enabled() -> bool {
    if let Some(v) = juiceutils::config::optional_secret("JUICEBOX_WATCH") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    !juiceutils::config::optional_secret("JUICEBOX_NO_BUILD")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

// Skip dotfiles so editor swap files and `.env` edits do not trigger rebuilds.
pub fn watchable_file(path: &Path) -> bool {
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

fn snapshot_roots(roots: &[PathBuf]) -> HashMap<PathBuf, SystemTime> {
    let mut next = HashMap::new();
    for root in roots {
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

/// Error returned when a filesystem watcher cannot be constructed.
#[derive(Debug, Error)]
pub enum FsWatcherError {
    #[error("no watch roots found under the workspace")]
    NoWatchRoots,
}

/// Polling filesystem watcher; scans run off the async runtime.
pub struct FsWatcher {
    roots: Vec<PathBuf>,
    last: HashMap<PathBuf, SystemTime>,
    last_scan: Instant,
}

impl FsWatcher {
    /// Build a watcher rooted at `root`, taking an initial snapshot.
    pub fn new(root: &Path) -> Result<Self, FsWatcherError> {
        let roots = watch_roots(root);
        if roots.is_empty() {
            return Err(FsWatcherError::NoWatchRoots);
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
        Ok(watcher)
    }

    fn snapshot(&self) -> HashMap<PathBuf, SystemTime> {
        snapshot_roots(&self.roots)
    }

    /// Reset the baseline so the next poll only reports newer changes.
    pub fn resnapshot(&mut self) {
        self.last = self.snapshot();
        self.last_scan = Instant::now();
    }

    /// Returns `true` when watched sources changed since the last snapshot.
    pub async fn poll(&mut self) -> bool {
        if self.last_scan.elapsed() < Duration::from_millis(500) {
            return false;
        }
        let roots = self.roots.clone();
        let next = match tokio::task::spawn_blocking(move || snapshot_roots(&roots)).await {
            Ok(next) => next,
            Err(e) => {
                tracing::warn!("source scan failed: {e}");
                return false;
            }
        };
        self.last_scan = Instant::now();
        if next == self.last {
            return false;
        }
        self.last = next;
        true
    }
}

/// Load `.env` files via `juiceutils`.
pub fn load_dotenv() {
    juiceutils::config::load_dotenv();
}

fn default_local_backend_url() -> String {
    format!("http://127.0.0.1:{}", juiceutils::config::ports::BACKEND)
}

fn host_backend_default() -> Option<String> {
    if std::env::var("BACKEND_URL").is_ok_and(|v| !v.trim().is_empty()) {
        None
    } else {
        Some(default_local_backend_url())
    }
}

/// Spawn a service binary located next to the current executable.
pub fn spawn(name: &str) -> Result<Child, StartupError> {
    let binary = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let mut cmd = Command::new(sibling_dir().join(binary));

    if name == "juicehost"
        && let Some(default) = host_backend_default()
    {
        cmd.env("BACKEND_URL", &default);
        tracing::info!("juicehost: BACKEND_URL unset, defaulting to {default}");
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            let e = if e.kind() == std::io::ErrorKind::NotFound {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "service binary '{name}' not found next to the juicebox binary; \
                         run `cargo build` first so all services are compiled"
                    ),
                )
            } else {
                e
            };
            StartupError::Spawn(e.to_string())
        })?;
    Ok(child)
}

// A crash loop usually means a config error, so retrying is pointless.
fn restart_enabled(name: &str) -> bool {
    std::env::var(format!("JUICERESTART_{}", name.to_uppercase())).map_or(true, |v| v != "0")
}

/// SIGTERM first so services can drain, then force-kill after ten seconds.
pub async fn graceful_stop(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let rc = unsafe { libc::kill(pid.cast_signed(), libc::SIGTERM) };
            if rc != 0 {
                tracing::warn!("failed to send SIGTERM to {pid}");
            }
        }
    }
    #[cfg(not(unix))]
    if let Err(e) = child.start_kill() {
        tracing::warn!("failed to ask child to stop: {e}");
    }
    match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(Ok(_)) => return,
        Ok(Err(e)) => {
            tracing::warn!("failed to wait for child: {e}");
            return;
        }
        Err(_) => {
            tracing::warn!("child ignored SIGTERM for 10s, killing it");
        }
    }
    if let Err(e) = child.start_kill() {
        tracing::warn!("failed to kill child: {e}");
        return;
    }
    if let Err(e) = child.wait().await {
        tracing::warn!("failed to reap killed child: {e}");
    }
}

/// Errors surfaced before or while starting the managed services.
#[derive(Debug, Error)]
pub enum StartupError {
    #[error("juiceback: {0}")]
    Juiceback(String),
    #[error("juicehost: {0}")]
    Juicehost(String),
    #[error("{0}")]
    Spawn(String),
}

fn check_juiceback_config() -> Result<(), StartupError> {
    let jwt = juiceutils::config::optional_secret("JWT_SECRET").ok_or_else(|| {
        StartupError::Juiceback("JWT_SECRET is not set. Set a real secret.".to_string())
    })?;
    juiceutils::config::validate_jwt_secret(&jwt)
        .map_err(|e| StartupError::Juiceback(e.to_string()))?;
    let key = juiceutils::config::optional_secret("IP_ENCRYPTION_KEY").ok_or_else(|| {
        StartupError::Juiceback(
            "IP_ENCRYPTION_KEY is not set (must be 64 hex chars / 32 bytes).".to_string(),
        )
    })?;
    juiceutils::config::validate_ip_encryption_key(&key)
        .map_err(|e| StartupError::Juiceback(e.to_string()))?;
    let storage_key =
        juiceutils::config::optional_secret("STORAGE_ENCRYPTION_KEY").ok_or_else(|| {
            StartupError::Juiceback(
                "STORAGE_ENCRYPTION_KEY is not set (must be 64 hex chars / 32 bytes).".to_string(),
            )
        })?;
    juiceutils::config::validate_hex_key("STORAGE_ENCRYPTION_KEY", &storage_key)
        .map_err(|e| StartupError::Juiceback(e.to_string()))?;
    Ok(())
}

fn check_juicehost_config() -> Result<(), StartupError> {
    if juiceutils::config::optional_secret("JUICEHOST_API_KEY").is_none()
        && !juiceutils::config::env_bool("JUICEHOST_ALLOW_NO_AUTH")
            .map_err(|e| StartupError::Juicehost(e.to_string()))?
            .unwrap_or(false)
    {
        return Err(StartupError::Juicehost(
            "JUICEHOST_API_KEY is not set. Set a key or JUICEHOST_ALLOW_NO_AUTH=true.".to_string(),
        ));
    }
    Ok(())
}

/// Check the configuration required by a single named service.
pub fn check_service_config(name: &str) -> Result<(), StartupError> {
    match name {
        "juiceback" => check_juiceback_config(),
        "juicehost" => check_juicehost_config(),
        _ => Ok(()),
    }
}

// Check secrets up front so the user sees one error, not three.
fn check_services_config<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), StartupError> {
    let mut first_error = None;
    for name in names {
        if let Err(e) = check_service_config(name) {
            tracing::error!("{e}");
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Check the configuration of every default [`SERVICES`] member.
pub fn check_config() -> Result<(), StartupError> {
    check_services_config(SERVICES.iter().copied())
}

/// Fatal errors returned by [`Supervisor::run`].
///
/// Each message matches the error that the thin binary used to log before
/// exiting, so runtime logging is unchanged.
#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("no services could be spawned, exiting")]
    NoServicesSpawned,
    #[error("all services failed, exiting")]
    AllServicesFailed,
    #[error("live reload respawn failed for all services, exiting")]
    LiveReloadRespawnFailed,
    #[error("cannot start: fix the configuration errors above")]
    Config(#[source] StartupError),
    #[error("service build failed, not starting services")]
    Build,
}

/// Configurable process supervisor for the Juicebox services.
pub struct Supervisor {
    services: Vec<String>,
    build: bool,
    watch: bool,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            services: SERVICES.iter().map(|name| (*name).to_string()).collect(),
            build: true,
            watch: watch_enabled(),
        }
    }
}

impl Supervisor {
    /// Create a supervisor using the default services, build and watch
    /// settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the services to supervise. Build and watch settings keep their
    /// defaults.
    pub fn with_services(services: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            services: services.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Enable or disable the pre-start `cargo build` step.
    pub fn with_build(mut self, build: bool) -> Self {
        self.build = build;
        self
    }

    /// Enable or disable live-reload source watching.
    pub fn with_watch(mut self, watch: bool) -> Self {
        self.watch = watch;
        self
    }

    /// Names of the services this supervisor manages.
    pub fn services(&self) -> &[String] {
        &self.services
    }

    /// Run the supervision loop until a shutdown is signalled or a fatal
    /// error occurs.
    pub async fn run(
        self,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), SupervisorError> {
        if let Err(e) = check_services_config(self.services.iter().map(String::as_str)) {
            return Err(SupervisorError::Config(e));
        }

        if self.build && ensure_services_built().await == BuildOutcome::Failed {
            return Err(SupervisorError::Build);
        }

        let mut children: HashMap<String, Child> = HashMap::new();
        let mut spawned_at: HashMap<String, Instant> = HashMap::new();
        let mut crashes: HashMap<String, u32> = HashMap::new();
        let mut pending: HashMap<String, Instant> = HashMap::new();

        for name in &self.services {
            match spawn(name) {
                Ok(child) => {
                    tracing::info!("started {name}");
                    children.insert(name.clone(), child);
                    spawned_at.insert(name.clone(), Instant::now());
                }
                Err(e) => {
                    tracing::error!("failed to spawn {name}: {e}");
                }
            }
        }

        if children.is_empty() {
            return Err(SupervisorError::NoServicesSpawned);
        }

        let mut watcher = if self.watch {
            workspace_dir().and_then(|root| FsWatcher::new(&root).ok())
        } else {
            None
        };
        if watcher.is_some() {
            tracing::info!(
                "watching workspace sources for live rebuilds (JUICEBOX_WATCH=0 to disable)"
            );
        }

        loop {
            let sources_changed = match watcher.as_mut() {
                Some(watcher) => watcher.poll().await,
                None => false,
            };
            if sources_changed {
                tracing::info!("sources changed, rebuilding services for live reload");
                if *shutdown_rx.borrow() {
                } else if ensure_services_built().await != BuildOutcome::Failed {
                    for (name, child) in &mut children {
                        tracing::info!("stopping {name} for live reload");
                        graceful_stop(child).await;
                    }
                    children.clear();
                    spawned_at.clear();
                    crashes.clear();
                    pending.clear();
                    for name in &self.services {
                        match spawn(name) {
                            Ok(child) => {
                                tracing::info!("started {name}");
                                children.insert(name.clone(), child);
                                spawned_at.insert(name.clone(), Instant::now());
                            }
                            Err(e) => {
                                tracing::error!("failed to respawn {name}: {e}");
                            }
                        }
                    }
                    if children.is_empty() {
                        return Err(SupervisorError::LiveReloadRespawnFailed);
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

            for name in &self.services {
                let Some(child) = children.get_mut(name.as_str()) else {
                    continue;
                };
                let status = tokio::select! {
                    _ = shutdown_rx.changed() => None,
                    status = child.wait() => status.ok(),

                    () = tokio::time::sleep(Duration::from_millis(250)) => None,
                };
                let Some(status) = status else {
                    continue;
                };
                children.remove(name.as_str());
                let runtime_secs = spawned_at
                    .remove(name.as_str())
                    .map_or(0, |t| t.elapsed().as_secs());
                if status.success() {
                    tracing::info!("{name} stopped after {runtime_secs}s");
                } else {
                    tracing::warn!("{name} exited with {status:?} after {runtime_secs}s");
                }
                if !restart_enabled(name) {
                    continue;
                }
                let count = if runtime_secs >= MIN_HEALTHY_SECS {
                    0
                } else {
                    crashes.get(name.as_str()).copied().unwrap_or(0) + 1
                };
                crashes.insert(name.clone(), count);
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
                    pending.insert(name.clone(), Instant::now() + Duration::from_secs(delay));
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
                        tracing::info!("started {name}");
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
                return Err(SupervisorError::AllServicesFailed);
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        for (name, child) in &mut children {
            tracing::info!("stopping {name}");
            graceful_stop(child).await;
        }

        tracing::info!("juicebox orchestrator exiting");
        Ok(())
    }
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
        "STORAGE_ENCRYPTION_KEY",
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

    #[tokio::test]
    async fn watcher_detects_add_modify_delete() {
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

        watcher.last_scan = Instant::now();
        assert!(!watcher.poll().await);

        std::fs::write(&file, "fn main() { /* edited */ }").unwrap();
        watcher.last_scan = aged();
        assert!(watcher.poll().await);

        watcher.last_scan = aged();
        assert!(!watcher.poll().await);

        std::fs::remove_file(&file).unwrap();
        watcher.last_scan = aged();
        assert!(watcher.poll().await);

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
    fn juiceback_requires_secrets() {
        let (_lock, _guard) = EnvGuard::lock(VARS);

        unsafe {
            std::env::remove_var("JWT_SECRET");
            std::env::remove_var("IP_ENCRYPTION_KEY");
            std::env::remove_var("STORAGE_ENCRYPTION_KEY");
            std::env::remove_var("COBALT_ENABLED");
        }
        assert!(check_juiceback_config().is_err());

        let hex_key = "0000000000000000000000000000000000000000000000000000000000000001";
        unsafe {
            std::env::set_var("JWT_SECRET", "test-secret");
            std::env::set_var("IP_ENCRYPTION_KEY", hex_key);
        }
        assert!(check_juiceback_config().is_err());

        unsafe {
            std::env::set_var("STORAGE_ENCRYPTION_KEY", hex_key);
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
            std::env::set_var(
                "STORAGE_ENCRYPTION_KEY",
                "0000000000000000000000000000000000000000000000000000000000000001",
            );
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
        assert_eq!(
            host_backend_default(),
            Some(format!(
                "http://127.0.0.1:{}",
                juiceutils::config::ports::BACKEND
            ))
        );

        unsafe {
            std::env::set_var("BACKEND_URL", "   ");
        }
        assert_eq!(
            host_backend_default(),
            Some(format!(
                "http://127.0.0.1:{}",
                juiceutils::config::ports::BACKEND
            ))
        );

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
