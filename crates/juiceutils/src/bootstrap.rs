//! Shared startup helpers for the Juicebox binaries.

/// Installs the global tracing subscriber.
///
/// `default_filter` is used when the environment supplies no valid filter.
/// `span_close` emits `FmtSpan::CLOSE` events, and `with_sentry` attaches the
/// Sentry tracing layer.
// The legacy layer is what the binaries used before this helper existed.
#[allow(
    deprecated,
    reason = "the legacy Sentry layer preserves existing behaviour"
)]
pub fn init_tracing(default_filter: &str, span_close: bool, with_sentry: bool) {
    use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, util::SubscriberInitExt};

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.to_owned().into());
    let fmt_layer = tracing_subscriber::fmt::layer();
    let fmt_layer = if span_close {
        fmt_layer.with_span_events(FmtSpan::CLOSE)
    } else {
        fmt_layer
    };
    let sentry_layer = with_sentry.then(sentry_tracing::layer);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(sentry_layer)
        .init();
}

/// Starts Sentry from the service-specific DSN variable, falling back to
/// `SENTRY_DSN`. The returned guard must stay alive for the process lifetime.
///
/// `release` is supplied by the caller so `release_name!()` keeps expanding
/// with the binary's own package metadata.
#[must_use]
pub fn init_sentry(
    dsn_env: &str,
    environment: &str,
    release: Option<std::borrow::Cow<'static, str>>,
) -> Option<sentry::ClientInitGuard> {
    crate::config::optional_secret(dsn_env)
        .or_else(|| crate::config::optional_secret("SENTRY_DSN"))
        .map(|dsn| {
            let traces_sample_rate = std::env::var("SENTRY_TRACES_SAMPLE_RATE")
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
                .map_or(0.05, |rate| rate.clamp(0.0, 1.0));
            sentry::init((
                dsn.as_str(),
                sentry::ClientOptions::default()
                    .maybe_release(release)
                    .environment(environment.to_owned())
                    .traces_sample_rate(traces_sample_rate)
                    .send_default_pii(false),
            ))
        })
}
