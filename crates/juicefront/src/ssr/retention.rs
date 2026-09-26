use crate::i18n;

pub const DEFAULT_ALLOWED_TTL_HOURS: &[f64] = &[0.5, 1.0, 6.0, 12.0, 24.0, 72.0, 168.0];
pub const DEFAULT_TTL_HOURS: f64 = 24.0;

fn known_key(hours: f64) -> Option<&'static str> {
    if hours == 0.5 {
        Some("retention.30m")
    } else if hours == 1.0 {
        Some("retention.1h")
    } else if hours == 6.0 {
        Some("retention.6h")
    } else if hours == 12.0 {
        Some("retention.12h")
    } else if hours == 24.0 {
        Some("retention.24h")
    } else if hours == 72.0 {
        Some("retention.3d")
    } else if hours == 168.0 {
        Some("retention.7d")
    } else {
        None
    }
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
        let mins = (hours * 60.0).round() as i64;
        let key = if mins == 1 {
            "retention.minute"
        } else {
            "retention.minutes"
        };
        return i18n::tv(locale, key, "count", &mins.to_string());
    }
    let days = hours / 24.0;
    if days.fract() == 0.0 {
        let days = days as i64;
        let key = if days == 1 {
            "retention.day"
        } else {
            "retention.days"
        };
        return i18n::tv(locale, key, "count", &days.to_string());
    }
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
