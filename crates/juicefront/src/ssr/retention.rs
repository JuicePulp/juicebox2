use crate::i18n;

pub const DEFAULT_ALLOWED_TTL_HOURS: &[f64] = &[0.5, 1.0, 6.0, 12.0, 24.0, 72.0, 168.0];
pub const DEFAULT_TTL_HOURS: f64 = 24.0;

/// Match an exact configured TTL to its locale key. Exact `==` is
/// correct here: the compared values are discrete configured constants
/// (all exactly representable), not measurements.
#[allow(clippy::float_cmp)]
fn known_key(hours: f64) -> Option<&'static str> {
    if hours == 0.5 {
        Some("retention.30m")
    } else if hours == 1.0 {
        Some("retention.1h")
    } else if hours == 6.0 {
        Some("retention.6h")
    } else if hours == 12.0 {
        Some("retention.12h")
    } else if hours == 72.0 {
        Some("retention.3d")
    } else if hours == 168.0 {
        Some("retention.7d")
    } else {
        None
    }
}

pub fn ttl_tier(hours: f64) -> u8 {
    let hours = if hours.is_finite() {
        hours
    } else {
        DEFAULT_TTL_HOURS
    };
    if hours < 1.0 {
        return 0;
    }
    // Exact divisibility check on discrete hour values, not measurements.
    #[allow(clippy::float_cmp)]
    if (hours / 24.0).fract() == 0.0 {
        return 2;
    }
    1
}

pub fn ttl_label(locale: &str, hours: f64) -> String {
    if let Some(key) = known_key(hours) {
        return i18n::t(locale, key);
    }
    let hours = if hours.is_finite() {
        hours
    } else {
        DEFAULT_TTL_HOURS
    };
    if hours < 1.0 {
        // Display rounding of small positive hour values; saturating `as`
        // casts cannot misbehave here.
        #[allow(clippy::cast_possible_truncation)]
        let mins = (hours * 60.0).round() as i64;
        let key = if mins == 1 {
            "retention.minute"
        } else {
            "retention.minutes"
        };
        return i18n::tv(locale, key, "count", &mins.to_string());
    }
    let days = hours / 24.0;
    // Exact divisibility on discrete values; the guarded `as` casts below
    // only ever see small non-negative magnitudes.
    #[allow(clippy::float_cmp, clippy::cast_possible_truncation)]
    if days.fract() == 0.0 {
        let days = days as i64;
        let key = if days == 1 {
            "retention.day"
        } else {
            "retention.days"
        };
        return i18n::tv(locale, key, "count", &days.to_string());
    }
    #[allow(clippy::float_cmp, clippy::cast_possible_truncation)]
    if hours.fract() == 0.0 {
        let whole = hours as i64;
        let key = if whole == 1 {
            "retention.hour"
        } else {
            "retention.hours"
        };
        return i18n::tv(locale, key, "count", &whole.to_string());
    }
    let rounded = (hours * 100.0).round() / 100.0;
    i18n::tv(locale, "retention.hours", "count", &rounded.to_string())
}
