use std::collections::BTreeMap;

/// The shortest advertised retention gets the full hard cap. Longer choices
/// decay exponentially from there.
pub const ANCHOR_TTL_HOURS: f64 = 0.5;

/// Default exponential half-life. With a 10 GiB hard cap this gives roughly
/// 6.25 GiB at 72 hours, 3.5 GiB at a week, and 1.25 GiB at two weeks.
pub const DEFAULT_HALF_LIFE_HOURS: f64 = 112.0;

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

/// Exponential file cap for one retention choice.
///
/// The shortest retention starts at the host's hard maximum; every additional
/// hour cuts the cap by a constant ratio determined by `half_life_hours`.
/// The result is rounded down to a readable step and floored so long retention
/// does not disappear entirely.
#[must_use]
pub fn cap_bytes(ttl_hours: f64, half_life_hours: f64, hard_max_bytes: u64, min_bytes: u64) -> u64 {
    if !ttl_hours.is_finite() || ttl_hours <= 0.0 {
        return hard_max_bytes;
    }
    if !half_life_hours.is_finite() || half_life_hours <= 0.0 {
        return hard_max_bytes;
    }

    let min_bytes = min_bytes.min(hard_max_bytes);
    let raw = if ttl_hours <= ANCHOR_TTL_HOURS {
        hard_max_bytes as f64
    } else {
        hard_max_bytes as f64 * 0.5_f64.powf((ttl_hours - ANCHOR_TTL_HOURS) / half_life_hours)
    };
    let rounded = (raw.floor() as u64 / CAP_STEP_BYTES) * CAP_STEP_BYTES;
    rounded.clamp(min_bytes, hard_max_bytes)
}

/// Builds the wire/API map for every advertised retention choice.
#[must_use]
pub fn build_caps(
    allowed_ttl_hours: &[f64],
    half_life_hours: f64,
    hard_max_bytes: u64,
    min_bytes: u64,
) -> BTreeMap<String, u64> {
    allowed_ttl_hours
        .iter()
        .copied()
        .filter(|hours| hours.is_finite() && *hours > 0.0)
        .map(|hours| {
            (
                ttl_key(hours),
                cap_bytes(hours, half_life_hours, hard_max_bytes, min_bytes),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn default_curve_only_peaks_at_the_shortest_retention() {
        assert_eq!(
            cap_bytes(
                ANCHOR_TTL_HOURS,
                DEFAULT_HALF_LIFE_HOURS,
                10 * GIB,
                256 * 1024 * 1024
            ),
            10 * GIB
        );
        assert_eq!(
            cap_bytes(1.0, DEFAULT_HALF_LIFE_HOURS, 10 * GIB, 256 * 1024 * 1024),
            39 * GIB / 4
        );
        assert_eq!(
            cap_bytes(24.0, DEFAULT_HALF_LIFE_HOURS, 10 * GIB, 256 * 1024 * 1024),
            17 * GIB / 2
        );
    }

    #[test]
    fn default_curve_keeps_three_day_uploads_generous() {
        assert_eq!(
            cap_bytes(72.0, DEFAULT_HALF_LIFE_HOURS, 10 * GIB, 256 * 1024 * 1024),
            25 * GIB / 4
        );
    }

    #[test]
    fn long_retention_is_lowered_and_rounded_down() {
        assert_eq!(
            cap_bytes(168.0, DEFAULT_HALF_LIFE_HOURS, 10 * GIB, 256 * 1024 * 1024),
            7 * GIB / 2
        );
        assert_eq!(
            cap_bytes(336.0, DEFAULT_HALF_LIFE_HOURS, 10 * GIB, 256 * 1024 * 1024),
            5 * GIB / 4
        );
    }

    #[test]
    fn map_uses_retention_keys() {
        let map = build_caps(
            &[0.5, 1.0, 72.0, 336.0],
            DEFAULT_HALF_LIFE_HOURS,
            10 * GIB,
            0,
        );
        assert_eq!(map.get("0.5"), Some(&(10 * GIB)));
        assert_eq!(map.get("1"), Some(&(39 * GIB / 4)));
        assert_eq!(map.get("72"), Some(&(25 * GIB / 4)));
        assert_eq!(map.get("336"), Some(&(5 * GIB / 4)));
    }
}
