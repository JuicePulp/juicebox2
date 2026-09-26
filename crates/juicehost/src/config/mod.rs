use std::path::{Path, PathBuf};

use juiceutils::file_validation::ProtectionLevel;
use serde::{Deserialize, Serialize};

mod ban;
mod directory;
mod error;
mod feature;
mod limits;
mod public;
mod quic;
mod s3;
mod secret;
mod security;
mod thread;

use ban::BanSettings;
use directory::DirectorySettings;
pub use error::ConfigError;
use feature::FeatureSettings;
use limits::LimitsSettings;
use public::PublicSettings;
use quic::QuicSettings;
use s3::S3Settings;
use secret::SecretSettings;
use security::SecuritySettings;
use thread::ThreadSettings;

#[derive(Debug, Clone)]
pub struct Config {
    pub public_host: String,

    pub public_port: u16,

    pub quic_host: String,

    pub quic_port: u16,

    pub quic_cert_path: Option<std::path::PathBuf>,

    pub quic_max_connections: usize,

    pub quic_max_requests: usize,

    pub quic_handshake_seconds: u64,

    pub quic_idle_seconds: u64,

    pub quic_request_total_seconds: u64,

    pub worker_threads: usize,

    pub files_dir: PathBuf,

    pub backend_url: Option<String>,

    pub frontend_url: Option<String>,

    pub api_key: String,

    pub allow_no_auth: bool,

    pub allowed_origins: Vec<String>,

    pub danger_level: ProtectionLevel,

    pub trusted_proxy_cidrs: Vec<juiceutils::proxy::IpCidr>,

    pub s3_bucket: Option<String>,

    pub s3_region: Option<String>,

    pub s3_endpoint: Option<String>,

    pub s3_allow_http: bool,

    pub s3_access_key: Option<String>,

    pub s3_secret_key: Option<String>,

    pub min_free_space_bytes: u64,

    pub max_file_size_bytes: u64,

    pub max_range_response_bytes: u64,

    pub max_concurrent_uploads: usize,

    pub max_concurrent_downloads: usize,

    pub max_concat_parts: usize,

    pub tcp_body_inactivity_seconds: u64,

    pub tcp_request_total_seconds: u64,

    pub tcp_max_concurrent_requests: usize,

    pub quick_link: bool,

    pub custom_id: bool,

    pub file_cache_enabled: bool,

    pub file_cache_max_age_secs: u64,

    pub default_ttl_hours: f64,

    pub allowed_ttl_hours: Vec<f64>,

    pub ticket_jwt_secret: String,

    pub ip_pepper: String,

    pub ban_list_file: Option<PathBuf>,

    pub ban_sync_url: Option<String>,

    pub ban_sync_interval: u64,

    pub sentry: juiceutils::config::SentrySettings,
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct FileConfig {
    #[serde(default)]
    pub public: PublicFile,

    #[serde(default)]
    pub quic: QuicFile,

    #[serde(default)]
    pub threads: ThreadFile,

    #[serde(default)]
    pub dirs: DirectoryFile,

    #[serde(default)]
    pub security: SecurityFile,

    #[serde(default)]
    pub s3: S3File,

    #[serde(default)]
    pub limits: LimitsFile,

    #[serde(default)]
    pub features: FeaturesFile,

    #[serde(default)]
    pub ban: BanFile,

    #[serde(default)]
    pub sentry: juiceutils::config::SentrySettings,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PublicFile {
    #[serde(default = "default_public_host")]
    pub host: String,

    #[serde(default = "default_public_port")]
    pub port: u16,
}

impl Default for PublicFile {
    fn default() -> Self {
        Self {
            host: default_public_host(),
            port: default_public_port(),
        }
    }
}
fn default_public_host() -> String {
    PublicSettings::DEFAULT_IP.to_owned()
}

const fn default_public_port() -> u16 {
    PublicSettings::DEFAULT_PORT
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuicFile {
    pub host: Option<String>,

    pub port: Option<u16>,

    #[serde(default = "default_quic_cert")]
    pub cert_path: PathBuf,

    #[serde(default = "default_quic_max_connections")]
    pub max_connections: usize,

    #[serde(default = "default_quic_max_requests")]
    pub max_requests: usize,

    #[serde(default = "default_quic_handshake", alias = "handshake_secs")]
    pub handshake_seconds: u64,

    #[serde(default = "default_quic_idle", alias = "idle_secs")]
    pub idle_seconds: u64,

    #[serde(default = "default_quic_request_total", alias = "request_total_secs")]
    pub request_total_seconds: u64,
}

impl Default for QuicFile {
    fn default() -> Self {
        Self {
            host: None,
            port: None,
            cert_path: default_quic_cert(),
            max_connections: default_quic_max_connections(),
            max_requests: default_quic_max_requests(),
            handshake_seconds: default_quic_handshake(),
            idle_seconds: default_quic_idle(),
            request_total_seconds: default_quic_request_total(),
        }
    }
}

fn default_quic_cert() -> PathBuf {
    PathBuf::from("./quic-cert.der")
}

const fn default_quic_max_connections() -> usize {
    256
}

const fn default_quic_max_requests() -> usize {
    256
}

const fn default_quic_handshake() -> u64 {
    10
}

const fn default_quic_idle() -> u64 {
    30
}

const fn default_quic_request_total() -> u64 {
    600
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadFile {
    #[serde(default = "default_worker_threads")]
    pub worker_threads: usize,
}

impl Default for ThreadFile {
    fn default() -> Self {
        Self {
            worker_threads: default_worker_threads(),
        }
    }
}
const fn default_worker_threads() -> usize {
    3
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DirectoryFile {
    #[serde(default = "default_files_dir")]
    pub files_dir: PathBuf,

    #[serde(default)]
    pub backend_url: Option<String>,

    #[serde(default)]
    pub frontend_url: Option<String>,
}

impl Default for DirectoryFile {
    fn default() -> Self {
        Self {
            files_dir: default_files_dir(),
            backend_url: None,
            frontend_url: None,
        }
    }
}

fn default_files_dir() -> PathBuf {
    PathBuf::from("./files")
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SecurityFile {
    #[serde(default = "default_danger_level")]
    pub danger_level: String,

    #[serde(default)]
    pub allowed_origins: Option<Vec<String>>,

    #[serde(default)]
    pub trusted_proxy_cidrs: Option<String>,

    #[serde(default)]
    pub allow_no_auth: bool,
}

impl Default for SecurityFile {
    fn default() -> Self {
        Self {
            danger_level: default_danger_level(),
            allowed_origins: None,
            trusted_proxy_cidrs: None,
            allow_no_auth: false,
        }
    }
}

fn default_danger_level() -> String {
    String::from("high")
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct S3File {
    #[serde(default)]
    pub bucket: Option<String>,

    #[serde(default)]
    pub region: Option<String>,

    #[serde(default)]
    pub endpoint: Option<String>,

    #[serde(default)]
    pub allow_http: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LimitsFile {
    #[serde(default = "default_min_free_space_gb")]
    pub min_free_space_gb: u64,

    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u64,

    #[serde(default = "default_max_range_response_mb")]
    pub max_range_response_mb: u64,

    #[serde(default = "default_max_concurrent_uploads")]
    pub max_concurrent_uploads: usize,

    #[serde(default = "default_max_concurrent_downloads")]
    pub max_concurrent_downloads: usize,

    #[serde(default = "default_max_concat_parts")]
    pub max_concat_parts: usize,

    #[serde(
        default = "default_tcp_body_inactivity",
        alias = "tcp_body_inactivity_secs"
    )]
    pub tcp_body_inactivity_seconds: u64,

    #[serde(
        default = "default_tcp_request_total",
        alias = "tcp_request_total_secs"
    )]
    pub tcp_request_total_seconds: u64,

    #[serde(default = "default_tcp_max_concurrent")]
    pub tcp_max_concurrent_requests: usize,
}

impl Default for LimitsFile {
    fn default() -> Self {
        Self {
            min_free_space_gb: default_min_free_space_gb(),
            max_file_size_mb: default_max_file_size_mb(),
            max_range_response_mb: default_max_range_response_mb(),
            max_concurrent_uploads: default_max_concurrent_uploads(),
            max_concurrent_downloads: default_max_concurrent_downloads(),
            max_concat_parts: default_max_concat_parts(),
            tcp_body_inactivity_seconds: default_tcp_body_inactivity(),
            tcp_request_total_seconds: default_tcp_request_total(),
            tcp_max_concurrent_requests: default_tcp_max_concurrent(),
        }
    }
}

const fn default_min_free_space_gb() -> u64 {
    5
}

const fn default_max_file_size_mb() -> u64 {
    500
}

const fn default_max_range_response_mb() -> u64 {
    16
}

const fn default_max_concurrent_uploads() -> usize {
    16
}

const fn default_max_concurrent_downloads() -> usize {
    64
}

const fn default_max_concat_parts() -> usize {
    128
}

const fn default_tcp_body_inactivity() -> u64 {
    30
}

const fn default_tcp_request_total() -> u64 {
    600
}

const fn default_tcp_max_concurrent() -> usize {
    512
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FeaturesFile {
    #[serde(default = "default_true")]
    pub quick_link: bool,

    #[serde(default = "default_true")]
    pub custom_id: bool,

    #[serde(default)]
    pub file_cache_enabled: bool,

    #[serde(default = "default_file_cache_max_age")]
    pub file_cache_max_age_secs: u64,

    #[serde(default = "default_default_ttl")]
    pub default_ttl_hours: f64,

    #[serde(default = "default_allowed_ttls")]
    pub allowed_ttl_hours: Vec<f64>,
}

impl Default for FeaturesFile {
    fn default() -> Self {
        Self {
            quick_link: true,
            custom_id: true,
            file_cache_enabled: false,
            file_cache_max_age_secs: default_file_cache_max_age(),
            default_ttl_hours: default_default_ttl(),
            allowed_ttl_hours: default_allowed_ttls(),
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_file_cache_max_age() -> u64 {
    3600
}

const fn default_default_ttl() -> f64 {
    24.0
}

fn default_allowed_ttls() -> Vec<f64> {
    vec![0.5, 1.0, 6.0, 12.0, 24.0, 72.0, 168.0]
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BanFile {
    #[serde(default)]
    pub list_file: Option<PathBuf>,

    #[serde(default)]
    pub sync_url: Option<String>,

    #[serde(default = "default_ban_sync_interval")]
    pub sync_interval_secs: u64,
}

impl Default for BanFile {
    fn default() -> Self {
        Self {
            list_file: None,
            sync_url: None,
            sync_interval_secs: default_ban_sync_interval(),
        }
    }
}

const fn default_ban_sync_interval() -> u64 {
    BanSettings::DEFAULT_SYNC_INTERVAL
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(dir) = std::env::var("JUICEHOST_CONFIG") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
    }
    for name in [".juicehost.toml", "juicehost.toml", "config.toml"] {
        paths.push(PathBuf::from(name));
        paths.push(PathBuf::from("/etc/juicebox").join(name.trim_start_matches('.')));
    }
    paths
}

fn load_file_config() -> FileConfig {
    for path in candidate_paths() {
        if path.exists() {
            return load_one_file(&path);
        }
    }
    tracing::warn!("no juicehost config file found, using defaults");
    FileConfig::default()
}

fn load_one_file(path: &Path) -> FileConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        tracing::warn!(
            "config file {path} unreadable, using defaults",
            path = path.display()
        );
        return FileConfig::default();
    };
    match toml::from_str::<FileConfig>(&text) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                "config file {path} invalid ({err}), using defaults",
                path = path.display()
            );
            FileConfig::default()
        }
    }
}

impl Config {
    pub fn try_load() -> Result<Self, ConfigError> {
        Self::try_load_from(&load_file_config())
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        Self::try_load_from(&FileConfig::default())
    }

    fn try_load_from(file: &FileConfig) -> Result<Self, ConfigError> {
        let public = PublicSettings::load(&file.public)?;
        let quic = QuicSettings::load(&file.quic, &public)?;
        let threads = ThreadSettings::load(&file.threads)?;
        let directories = DirectorySettings::load(&file.dirs);
        let security = SecuritySettings::load(&file.security, &directories)?;
        let s3 = S3Settings::load(&file.s3)?;
        let limits = LimitsSettings::load(&file.limits)?;
        let features = FeatureSettings::load(&file.features)?;
        let secrets = SecretSettings::load(&security)?;
        let ban = BanSettings::load(&file.ban);
        let sentry = juiceutils::config::SentrySettings::from_env_or(&file.sentry);

        Ok(Self {
            public_host: public.host().to_owned(),
            public_port: public.port(),
            quic_host: quic.host().to_owned(),
            quic_port: quic.port(),
            quic_cert_path: Some(quic.cert_path().to_owned()),
            quic_max_connections: quic.max_connections(),
            quic_max_requests: quic.max_requests(),
            quic_handshake_seconds: quic.handshake_seconds(),
            quic_idle_seconds: quic.idle_seconds(),
            quic_request_total_seconds: quic.request_total_seconds(),
            worker_threads: threads.worker_threads(),
            files_dir: directories.files_dir().to_owned(),
            backend_url: directories.backend_url().map(str::to_owned),
            frontend_url: directories.frontend_url().map(str::to_owned),
            api_key: security.api_key().to_owned(),
            allow_no_auth: security.allow_no_auth(),
            allowed_origins: security.allowed_origins().to_owned(),
            danger_level: security.danger_level(),
            trusted_proxy_cidrs: security.trusted_proxy_cidrs().to_owned(),
            s3_bucket: s3.bucket().map(str::to_owned),
            s3_region: s3.region().map(str::to_owned),
            s3_endpoint: s3.endpoint().map(str::to_owned),
            s3_allow_http: s3.allow_http(),
            s3_access_key: s3.access_key().map(str::to_owned),
            s3_secret_key: s3.secret_key().map(str::to_owned),
            min_free_space_bytes: limits.min_free_space_bytes(),
            max_file_size_bytes: limits.max_file_size_bytes(),
            max_range_response_bytes: limits.max_range_response_bytes(),
            max_concurrent_uploads: limits.max_concurrent_uploads(),
            max_concurrent_downloads: limits.max_concurrent_downloads(),
            max_concat_parts: limits.max_concat_parts(),
            tcp_body_inactivity_seconds: limits.tcp_body_inactivity_seconds(),
            tcp_request_total_seconds: limits.tcp_request_total_seconds(),
            tcp_max_concurrent_requests: limits.tcp_max_concurrent_requests(),
            quick_link: features.quick_link(),
            custom_id: features.custom_id(),
            file_cache_enabled: features.file_cache_enabled(),
            file_cache_max_age_secs: features.file_cache_max_age_secs(),
            default_ttl_hours: features.default_ttl_hours(),
            allowed_ttl_hours: features.allowed_ttl_hours().to_owned(),
            ticket_jwt_secret: secrets.ticket_jwt_secret().to_owned(),
            ip_pepper: secrets.ip_pepper().to_owned(),
            ban_list_file: ban.list_file().cloned(),
            ban_sync_url: ban.sync_url().map(str::to_owned),
            ban_sync_interval: ban.sync_interval(),
            sentry,
        })
    }

    #[must_use]
    pub const fn is_s3_mode(&self) -> bool {
        self.s3_bucket.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_juicehost_env() {
        for name in [
            "JUICEHOST_API_KEY",
            "JUICEHOST_CONFIG",
            "S3_ACCESS_KEY",
            "S3_SECRET_KEY",
            "JWT_SECRET",
            "TICKET_JWT_SECRET",
            "IP_PEPPER",
        ] {
            unsafe {
                std::env::remove_var(name);
            }
        }
    }

    #[test]
    fn missing_api_key_is_optional() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_juicehost_env();
        unsafe {
            std::env::set_var("TICKET_JWT_SECRET", "test-ticket-secret");
        }
        let result = Config::from_env();
        assert!(result.is_ok(), "from_env failed: {:?}", result.err());
        assert!(result.unwrap().api_key.is_empty());
        unsafe {
            std::env::remove_var("TICKET_JWT_SECRET");
        }
    }

    #[test]
    fn missing_ticket_secret_fails_closed() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_juicehost_env();
        let result = Config::from_env();
        assert!(
            matches!(
                result,
                Err(ConfigError::MissingSecret {
                    name: "TICKET_JWT_SECRET"
                })
            ),
            "expected MissingSecret, got: {:?}",
            result
                .map(|cfg| cfg.ticket_jwt_secret.is_empty())
                .map_err(|e| e.to_string())
        );
    }

    #[test]
    fn whitespace_api_key_becomes_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_juicehost_env();
        unsafe {
            std::env::set_var("JUICEHOST_API_KEY", "   ");
            std::env::set_var("TICKET_JWT_SECRET", "test-ticket-secret");
        }
        let result = Config::from_env();
        assert!(result.is_ok());
        assert!(result.unwrap().api_key.is_empty());
        unsafe {
            std::env::remove_var("TICKET_JWT_SECRET");
        }
    }

    #[test]
    fn toml_values_flow_into_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("TICKET_JWT_SECRET", "test-ticket-secret");
        }
        let file: FileConfig = toml::from_str(
            "[public]\nhost = \"10.0.0.9\"\nport = 6410\n[features]\nquick_link = false\ncustom_id = false\ndefault_ttl_hours = 6.0\nallowed_ttl_hours = [6.0, 24.0]\n",
        )
        .unwrap();
        let cfg = Config::try_load_from(&file).unwrap();
        unsafe {
            std::env::remove_var("TICKET_JWT_SECRET");
        }
        assert_eq!(cfg.public_host, "10.0.0.9");
        assert_eq!(cfg.public_port, 6410);
        assert!(!cfg.quick_link);
        assert!(!cfg.custom_id);
        assert_eq!(cfg.default_ttl_hours, 6.0);
    }
}
