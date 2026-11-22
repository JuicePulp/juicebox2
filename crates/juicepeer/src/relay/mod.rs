//! Public WebSocket relay that bridges web downloaders to native peers.
//!
//! The relay never fetches a URL from a peer and never writes file bytes to
//! disk. It verifies a device JWT, asks juiceback which file ids may be served,
//! and tunnels fetch requests over the peer's WebSocket.

pub mod config;
pub mod routes;
pub mod session;

use config::Config;
use session::SessionRegistry;

/// Dependencies shared by the HTTP routes and the peer WebSocket sessions.
pub struct RelayState {
    pub config: Config,
    pub registry: SessionRegistry,
    /// Only ever used for `POST /internal/peer/authorize`.
    pub client: reqwest::Client,
}

impl RelayState {
    /// # Errors
    ///
    /// Returns a description when the HTTP client cannot be built.
    pub fn new(config: Config) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout())
            .build()
            .map_err(|error| format!("cannot build HTTP client: {error}"))?;
        Ok(Self {
            config,
            registry: SessionRegistry::default(),
            client,
        })
    }
}
