//! Configuration for the public peer relay.
//!
//! Values are resolved in this order: CLI flags, environment variables, TOML
//! file, code defaults. Secrets are never read from the TOML file so they
//! cannot be committed by accident. The TOML parsing itself is delegated to
//! `juiceutils` because this crate does not depend on `toml` directly.

use std::{net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use serde::Deserialize;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 6410;
const DEFAULT_RELAY_BASE_URL: &str = "https://ff.juicey.dev";
const DEFAULT_JUICEBACK_URL: &str = "http://127.0.0.1:6401";
const DEFAULT_MAX_SESSIONS: usize = 1_024;
const DEFAULT_MAX_FRAME_BYTES: usize = 1_048_576;
const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 128;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 60;
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Runtime configuration of the relay.
#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub relay_base_url: String,
    pub juiceback_url: String,
    /// Shared device-JWT signing secret; never logged.
    pub jwt_secret: String,
    /// Key used for `POST /internal/peer/authorize`; never logged.
    pub api_key: String,
    pub max_sessions: usize,
    pub max_frame_bytes: usize,
    pub max_concurrent_requests: usize,
    pub request_timeout_secs: u64,
    pub heartbeat_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
            relay_base_url: DEFAULT_RELAY_BASE_URL.to_owned(),
            juiceback_url: DEFAULT_JUICEBACK_URL.to_owned(),
            jwt_secret: String::new(),
            api_key: String::new(),
            max_sessions: DEFAULT_MAX_SESSIONS,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
            heartbeat_interval_secs: DEFAULT_HEARTBEAT_INTERVAL_SECS,
        }
    }
}

impl Config {
    /// Human-readable flag list used by `--help`.
    #[must_use]
    pub const fn usage() -> &'static str {
        "juicepeer-relay [OPTIONS]\n\
         \x20     --config <path>                 TOML config file\n\
         \x20     --listen <host:port>            listener address\n\
         \x20     --host <ip>                     listener host\n\
         \x20     --port <u16>                    listener port (default 6410)\n\
         \x20     --relay-base-url <url>          public relay base URL\n\
         \x20     --juiceback-url <url>           juiceback base URL\n\
         \x20     --jwt-secret <secret>           device JWT secret (prefer env)\n\
         \x20     --api-key <key>                 internal API key (prefer env)\n\
         \x20     --max-sessions <n>              concurrent peer sessions\n\
         \x20     --max-frame-bytes <n>           max WS binary frame payload\n\
         \x20     --max-concurrent-requests <n>   in-flight browser requests\n\
         \x20     --request-timeout-secs <n>      per-request timeout\n\
         \x20     --heartbeat-interval-secs <n>   ping interval"
    }

    /// Loads config from the process environment and CLI.
    ///
    /// # Errors
    ///
    /// Returns a description when a flag is malformed, a secret is missing or
    /// a URL is not an absolute http(s) URL.
    pub fn try_load() -> Result<Self, String> {
        let cli = parse_cli(std::env::args().skip(1))?;
        Self::from_cli(&cli)
    }

    fn from_cli(cli: &Cli) -> Result<Self, String> {
        let file: FileConfig = cli.config_path.as_ref().map_or_else(
            || {
                juiceutils::config::load_service_config(
                    "juicepeer",
                    "JUICEPEER_CONFIG",
                    &[".juicepeer.toml", "juicepeer.toml"],
                )
            },
            |path| juiceutils::config::load_toml_or_default(path.as_path()),
        );
        let mut config = Self::from_file(&file);
        config.apply_env();
        config.apply_cli(cli)?;
        config.validate()?;
        Ok(config)
    }

    fn from_file(file: &FileConfig) -> Self {
        Self {
            host: file.host.clone(),
            port: file.port,
            relay_base_url: file.relay_base_url.clone(),
            juiceback_url: file.juiceback_url.clone(),
            ..Self::default()
        }
        .with_limits(
            file.max_sessions,
            file.max_frame_bytes,
            file.max_concurrent_requests,
            file.request_timeout_secs,
            file.heartbeat_interval_secs,
        )
    }

    const fn with_limits(
        mut self,
        max_sessions: usize,
        max_frame_bytes: usize,
        max_concurrent_requests: usize,
        request_timeout_secs: u64,
        heartbeat_interval_secs: u64,
    ) -> Self {
        self.max_sessions = max_sessions;
        self.max_frame_bytes = max_frame_bytes;
        self.max_concurrent_requests = max_concurrent_requests;
        self.request_timeout_secs = request_timeout_secs;
        self.heartbeat_interval_secs = heartbeat_interval_secs;
        self
    }

    fn apply_env(&mut self) {
        if let Some(addr) = env_parsed::<SocketAddr>("JUICEPEER_LISTEN") {
            self.host = addr.ip().to_string();
            self.port = addr.port();
        }
        assign_env_string(&mut self.host, "JUICEPEER_HOST");
        assign_env(&mut self.port, "JUICEPEER_PORT");
        assign_env_string(&mut self.relay_base_url, "JUICEPEER_RELAY_BASE_URL");
        assign_env_string(&mut self.juiceback_url, "JUICEPEER_JUICEBACK_URL");
        assign_env_string(&mut self.juiceback_url, "JUICEBACK_URL");
        assign_env_string(&mut self.jwt_secret, "JUICEPEER_JWT_SECRET");
        if self.jwt_secret.is_empty() {
            assign_env_string(&mut self.jwt_secret, "TICKET_JWT_SECRET");
        }
        if self.jwt_secret.is_empty() {
            assign_env_string(&mut self.jwt_secret, "JWT_SECRET");
        }
        assign_env_string(&mut self.api_key, "JUICEPEER_API_KEY");
        if self.api_key.is_empty() {
            assign_env_string(&mut self.api_key, "JUICEHOST_API_KEY");
        }
        assign_env(&mut self.max_sessions, "JUICEPEER_MAX_SESSIONS");
        assign_env(&mut self.max_frame_bytes, "JUICEPEER_MAX_FRAME_BYTES");
        assign_env(
            &mut self.max_concurrent_requests,
            "JUICEPEER_MAX_CONCURRENT_REQUESTS",
        );
        assign_env(
            &mut self.request_timeout_secs,
            "JUICEPEER_REQUEST_TIMEOUT_SECS",
        );
        assign_env(
            &mut self.heartbeat_interval_secs,
            "JUICEPEER_HEARTBEAT_INTERVAL_SECS",
        );
    }

    fn apply_cli(&mut self, cli: &Cli) -> Result<(), String> {
        if let Some(addr) = &cli.listen {
            let addr = addr
                .parse::<SocketAddr>()
                .map_err(|_| format!("invalid --listen address: {addr}"))?;
            self.host = addr.ip().to_string();
            self.port = addr.port();
        }
        assign_str(&mut self.host, cli.host.as_deref());
        assign_option(&mut self.port, cli.port);
        assign_str(&mut self.relay_base_url, cli.relay_base_url.as_deref());
        assign_str(&mut self.juiceback_url, cli.juiceback_url.as_deref());
        assign_str(&mut self.jwt_secret, cli.jwt_secret.as_deref());
        assign_str(&mut self.api_key, cli.api_key.as_deref());
        assign_option(&mut self.max_sessions, cli.max_sessions);
        assign_option(&mut self.max_frame_bytes, cli.max_frame_bytes);
        assign_option(
            &mut self.max_concurrent_requests,
            cli.max_concurrent_requests,
        );
        assign_option(&mut self.request_timeout_secs, cli.request_timeout_secs);
        assign_option(
            &mut self.heartbeat_interval_secs,
            cli.heartbeat_interval_secs,
        );
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        juiceutils::config::validate_jwt_secret(&self.jwt_secret).map_err(|e| e.to_string())?;
        format!("{}:{}", self.host, self.port)
            .parse::<SocketAddr>()
            .map_err(|_| format!("invalid listen address: {}:{}", self.host, self.port))?;
        if self.api_key.trim().is_empty() {
            return Err("JUICEPEER_API_KEY (or JUICEHOST_API_KEY) is not set".to_owned());
        }
        validate_http_url("relay_base_url", &self.relay_base_url)?;
        validate_http_url("juiceback_url", &self.juiceback_url)?;
        if self.max_sessions == 0 || self.max_concurrent_requests == 0 {
            return Err("max_sessions and max_concurrent_requests must be positive".to_owned());
        }
        if self.max_frame_bytes < 1_024 {
            return Err("max_frame_bytes must be at least 1024".to_owned());
        }
        if self.request_timeout_secs == 0 || self.heartbeat_interval_secs == 0 {
            return Err("request and heartbeat timeouts must be positive".to_owned());
        }
        Ok(())
    }

    #[must_use]
    pub fn listen_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)))
    }

    #[must_use]
    pub const fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }

    #[must_use]
    pub const fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.heartbeat_interval_secs)
    }
}

/// TOML-backed, non-secret settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct FileConfig {
    host: String,
    port: u16,
    relay_base_url: String,
    juiceback_url: String,
    max_sessions: usize,
    max_frame_bytes: usize,
    max_concurrent_requests: usize,
    request_timeout_secs: u64,
    heartbeat_interval_secs: u64,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
            relay_base_url: DEFAULT_RELAY_BASE_URL.to_owned(),
            juiceback_url: DEFAULT_JUICEBACK_URL.to_owned(),
            max_sessions: DEFAULT_MAX_SESSIONS,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
            heartbeat_interval_secs: DEFAULT_HEARTBEAT_INTERVAL_SECS,
        }
    }
}

#[derive(Debug, Default)]
struct Cli {
    config_path: Option<PathBuf>,
    listen: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    relay_base_url: Option<String>,
    juiceback_url: Option<String>,
    jwt_secret: Option<String>,
    api_key: Option<String>,
    max_sessions: Option<usize>,
    max_frame_bytes: Option<usize>,
    max_concurrent_requests: Option<usize>,
    request_timeout_secs: Option<u64>,
    heartbeat_interval_secs: Option<u64>,
}

fn parse_cli(mut args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    while let Some(arg) = args.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) => (key, Some(value.to_owned())),
            None => (arg.as_str(), None),
        };
        let value = match inline {
            Some(value) => value,
            None => args
                .next()
                .ok_or_else(|| format!("missing value for {key}"))?,
        };
        apply_flag(&mut cli, key, &value)?;
    }
    Ok(cli)
}

fn apply_flag(cli: &mut Cli, key: &str, value: &str) -> Result<(), String> {
    match key {
        "--config" | "-c" => cli.config_path = Some(PathBuf::from(value)),
        "--listen" => cli.listen = Some(value.to_owned()),
        "--host" => cli.host = Some(value.to_owned()),
        "--port" => cli.port = Some(parse_value(key, value)?),
        "--relay-base-url" => cli.relay_base_url = Some(value.to_owned()),
        "--juiceback-url" => cli.juiceback_url = Some(value.to_owned()),
        "--jwt-secret" => cli.jwt_secret = Some(value.to_owned()),
        "--api-key" => cli.api_key = Some(value.to_owned()),
        "--max-sessions" => cli.max_sessions = Some(parse_value(key, value)?),
        "--max-frame-bytes" => cli.max_frame_bytes = Some(parse_value(key, value)?),
        "--max-concurrent-requests" => cli.max_concurrent_requests = Some(parse_value(key, value)?),
        "--request-timeout-secs" => cli.request_timeout_secs = Some(parse_value(key, value)?),
        "--heartbeat-interval-secs" => cli.heartbeat_interval_secs = Some(parse_value(key, value)?),
        _ => return Err(format!("unknown argument: {key}")),
    }
    Ok(())
}

fn parse_value<T: FromStr>(key: &str, value: &str) -> Result<T, String> {
    value
        .parse::<T>()
        .map_err(|_| format!("invalid value for {key}: {value}"))
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_parsed<T: FromStr>(name: &str) -> Option<T> {
    env_string(name).and_then(|value| value.parse::<T>().ok())
}

fn assign_env<T: FromStr + Copy>(target: &mut T, name: &str) {
    if let Some(value) = env_parsed::<T>(name) {
        *target = value;
    }
}

fn assign_env_string(target: &mut String, name: &str) {
    if let Some(value) = env_string(name) {
        *target = value;
    }
}

fn assign_str(target: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        target.replace_range(.., value);
    }
}

const fn assign_option<T: Copy>(target: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *target = value;
    }
}

fn validate_http_url(label: &str, raw: &str) -> Result<(), String> {
    let parsed = url::Url::parse(raw).map_err(|e| format!("{label} is invalid: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err(format!("{label} must be an absolute http(s) URL"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_contract() {
        let config = Config::default();
        assert_eq!(config.port, 6410);
        assert_eq!(config.max_sessions, 1_024);
        assert_eq!(config.max_frame_bytes, 1_048_576);
        assert_eq!(config.max_concurrent_requests, 128);
        assert_eq!(config.request_timeout(), Duration::from_secs(60));
        assert_eq!(config.heartbeat_interval(), Duration::from_secs(30));
    }

    #[test]
    fn cli_overrides_are_parsed() {
        let cli = parse_cli(
            [
                "--port",
                "7000",
                "--listen=127.0.0.1:7010",
                "--max-sessions",
                "4",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(cli.port, Some(7000));
        assert_eq!(cli.listen.as_deref(), Some("127.0.0.1:7010"));
        assert_eq!(cli.max_sessions, Some(4));
    }

    #[test]
    fn invalid_cli_is_rejected() {
        assert!(parse_cli(["--port", "nope"].into_iter().map(str::to_owned)).is_err());
        assert!(parse_cli(["--bogus", "1"].into_iter().map(str::to_owned)).is_err());
    }

    #[test]
    fn urls_must_be_absolute_http() {
        assert!(validate_http_url("x", "https://ff.example/").is_ok());
        assert!(validate_http_url("x", "ftp://ff.example").is_err());
        assert!(validate_http_url("x", "not-a-url").is_err());
    }

    #[test]
    fn secrets_are_required() {
        let mut config = Config::default();
        assert!(config.validate().is_err());
        config.jwt_secret = "a-real-secret".to_owned();
        assert!(config.validate().is_err());
        config.api_key = "key".to_owned();
        assert!(config.validate().is_ok());
    }
}
