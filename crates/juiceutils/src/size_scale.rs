use std::collections::BTreeMap;

/// Floor so a very long retention still leaves a usable upload limit.
pub const DEFAULT_MIN_FILE_SIZE_BYTES: u64 = 256 * 1024 * 1024;

/// Caps are rounded down to this step to keep the UI readable.
pub const CAP_STEP_BYTES: u64 = 256 * 1024 * 1024;

/// Stable map key for a TTL. `1.0` becomes `"1"`, matching the JSON emitted by
/// the host and the values in the retention picker.
#[must_use]
pub fn ttl_key(hours: f64) -> String {
    hours.to_string()
}

/// Cubic file cap for one retention choice.
///
/// The shortest retention starts at the host's hard maximum; the cap then
/// follows `min + (hard - min) * (1 - u)^3`, where `u` is the normalized
/// position between the shortest and longest advertised retention.
/// The result is rounded down to a readable step and floored so long retention
/// does not disappear entirely.
#[must_use]
pub fn cap_bytes(
    ttl_hours: f64,
    min_ttl_hours: f64,
    max_ttl_hours: f64,
    hard_max_bytes: u64,
    min_bytes: u64,
) -> u64 {
    if !ttl_hours.is_finite() || ttl_hours <= 0.0 {
        return hard_max_bytes;
    }

    let min_bytes = min_bytes.min(hard_max_bytes);
    let span = max_ttl_hours - min_ttl_hours;
    let raw = if !span.is_finite() || span <= 0.0 || ttl_hours <= min_ttl_hours {
        hard_max_bytes as f64
    } else {
        let u = ((ttl_hours - min_ttl_hours) / span).clamp(0.0, 1.0);
        min_bytes as f64 + (hard_max_bytes - min_bytes) as f64 * (1.0 - u).powi(3)
    };
    let rounded = (raw.floor() as u64 / CAP_STEP_BYTES) * CAP_STEP_BYTES;
    rounded.clamp(min_bytes, hard_max_bytes)
}

/// Builds the wire/API map for every advertised retention choice.
#[must_use]
pub fn build_caps(
    allowed_ttl_hours: &[f64],
    hard_max_bytes: u64,
    min_bytes: u64,
) -> BTreeMap<String, u64> {
    let min_ttl_hours = allowed_ttl_hours
        .iter()
        .copied()
        .filter(|hours| hours.is_finite() && *hours > 0.0)
        .fold(f64::INFINITY, f64::min);
    let max_ttl_hours = allowed_ttl_hours
        .iter()
        .copied()
        .filter(|hours| hours.is_finite() && *hours > 0.0)
        .fold(f64::NEG_INFINITY, f64::max);
    allowed_ttl_hours
        .iter()
        .copied()
        .filter(|hours| hours.is_finite() && *hours > 0.0)
        .map(|hours| {
            (
                ttl_key(hours),
                cap_bytes(
                    hours,
                    min_ttl_hours,
                    max_ttl_hours,
                    hard_max_bytes,
                    min_bytes,
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    const MIN: u64 = 5 * GIB / 4;

    #[test]
    fn cubic_curve_only_peaks_at_the_shortest_retention() {
        assert_eq!(cap_bytes(1.0, 1.0, 720.0, 10 * GIB, MIN), 10 * GIB);
        assert_eq!(cap_bytes(6.0, 1.0, 720.0, 10 * GIB, MIN), 39 * GIB / 4);
        assert_eq!(cap_bytes(24.0, 1.0, 720.0, 10 * GIB, MIN), 9 * GIB);
    }

    #[test]
    fn cubic_curve_keeps_three_day_uploads_generous() {
        assert_eq!(cap_bytes(72.0, 1.0, 720.0, 10 * GIB, MIN), 15 * GIB / 2);
    }

    #[test]
    fn long_retention_is_lowered_and_rounded_down() {
        assert_eq!(cap_bytes(168.0, 1.0, 720.0, 10 * GIB, MIN), 5 * GIB);
        assert_eq!(cap_bytes(336.0, 1.0, 720.0, 10 * GIB, MIN), 5 * GIB / 2);
        assert_eq!(cap_bytes(720.0, 1.0, 720.0, 10 * GIB, MIN), 5 * GIB / 4);
    }

    #[test]
    fn map_uses_retention_keys() {
        let map = build_caps(&[1.0, 72.0, 336.0, 720.0], 10 * GIB, MIN);
        assert_eq!(map.get("1"), Some(&(10 * GIB)));
        assert_eq!(map.get("72"), Some(&(15 * GIB / 2)));
        assert_eq!(map.get("336"), Some(&(5 * GIB / 2)));
        assert_eq!(map.get("720"), Some(&(5 * GIB / 4)));
    }
}
