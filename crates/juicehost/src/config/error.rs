#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("QUIC_PORT must be set when PUBLIC_PORT is 65535")]
    MissingQuicPort,

    #[error("TTL values must be finite, positive, and include DEFAULT_TTL_HOURS")]
    InvalidTtlConfiguration,

    #[error("invalid audio embed configuration")]
    InvalidAudioEmbedConfiguration,

    #[error("{0}")]
    InvalidTrustedProxyCidrs(String),

    #[error("{0}")]
    InvalidEnvironment(String),

    #[error("{0}")]
    InvalidSecret(String),
}
