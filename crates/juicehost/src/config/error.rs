#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("QUIC_PORT must be set when PUBLIC_PORT is 65535")]
    MissingQuicPort,

    #[error("TTL values must be finite, positive, and include DEFAULT_TTL_HOURS")]
    InvalidTtlConfiguration,

    #[error("{0}")]
    InvalidTrustedProxyCidrs(String),

    #[error("{name} must be set (ticket JWTs cannot be signed with an empty key)")]
    MissingSecret { name: &'static str },
}
