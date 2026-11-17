use std::collections::BTreeMap;

/// Default byte-hour budget: 10 GiB for a day, then a gradual taper through
/// longer retention choices (6.5 GiB at 72 hours, 2.75 GiB at a week).
pub const DEFAULT_BUDGET_GIB_HOURS: f64 = 480.0;

/// Floor so a very long retention still leaves a usable upload limit.
pub const DEFAULT_MIN_FILE_SIZE_BYTES: u64 = 256 * 1024 * 1024;

/// Caps are rounded down to this step to keep the UI readable.
pub const CAP_STEP_BYTES: u64 = 256 * 1024 * 1024;

const GIB_BYTES: f64 = 1024.0 * 1024.0 * 1024.0;

/// Stable map key for a TTL. `1.0` becomes `"1"`, matching the JSON emitted by
/// the host and the values in the retention picker.
#[must_use]
pub fn ttl_key(hours: f64) -> String {
    hours.to_string()
}

/// Time-weighted file cap for one retention choice.
///
/// The cap is capped by the host's hard maximum, rounded down to a readable
/// step, and floored so long retention does not disappear entirely.
#[must_use]
pub fn cap_bytes(
    ttl_hours: f64,
    budget_gib_hours: f64,
    hard_max_bytes: u64,
    min_bytes: u64,
) -> u64 {
    if !ttl_hours.is_finite() || ttl_hours <= 0.0 {
        return hard_max_bytes;
    }
    if !budget_gib_hours.is_finite() || budget_gib_hours <= 0.0 {
        return hard_max_bytes;
    }

    let min_bytes = min_bytes.min(hard_max_bytes);
    let raw = (budget_gib_hours * GIB_BYTES / ttl_hours).min(hard_max_bytes as f64);
    let rounded = (raw.floor() as u64 / CAP_STEP_BYTES) * CAP_STEP_BYTES;
    rounded.clamp(min_bytes, hard_max_bytes)
}

/// Builds the wire/API map for every advertised retention choice.
#[must_use]
pub fn build_caps(
    allowed_ttl_hours: &[f64],
    budget_gib_hours: f64,
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
                cap_bytes(hours, budget_gib_hours, hard_max_bytes, min_bytes),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn default_budget_keeps_three_day_uploads_generous() {
        assert_eq!(
            cap_bytes(72.0, DEFAULT_BUDGET_GIB_HOURS, 10 * GIB, 256 * 1024 * 1024),
            13 * GIB / 2
        );
    }

    #[test]
    fn short_retention_hits_the_hard_cap() {
        assert_eq!(
            cap_bytes(0.5, DEFAULT_BUDGET_GIB_HOURS, 10 * GIB, 256 * 1024 * 1024),
            10 * GIB
        );
        assert_eq!(
            cap_bytes(24.0, DEFAULT_BUDGET_GIB_HOURS, 10 * GIB, 256 * 1024 * 1024),
            10 * GIB
        );
    }

    #[test]
    fn long_retention_is_lowered_and_rounded_down() {
        assert_eq!(
            cap_bytes(168.0, DEFAULT_BUDGET_GIB_HOURS, 10 * GIB, 256 * 1024 * 1024),
            11 * GIB / 4
        );
        assert_eq!(
            cap_bytes(336.0, DEFAULT_BUDGET_GIB_HOURS, 10 * GIB, 256 * 1024 * 1024),
            5 * GIB / 4
        );
    }

    #[test]
    fn map_uses_retention_keys() {
        let map = build_caps(
            &[0.5, 1.0, 72.0, 336.0],
            DEFAULT_BUDGET_GIB_HOURS,
            10 * GIB,
            0,
        );
        assert_eq!(map.get("0.5"), Some(&(10 * GIB)));
        assert_eq!(map.get("1"), Some(&(10 * GIB)));
        assert_eq!(map.get("72"), Some(&(13 * GIB / 2)));
        assert_eq!(map.get("336"), Some(&(5 * GIB / 4)));
    }
}
