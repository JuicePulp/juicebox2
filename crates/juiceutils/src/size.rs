//! Human-readable byte sizes shared by the frontend and host.

/// Rounding rules for [`format_size`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeStyle {
    /// Round to an integer at 10 or above (and for raw bytes); one decimal
    /// below 10, capped at TB.
    Compact,
    /// Always show one decimal for scaled units; raw bytes stay exact.
    Fixed1,
}

/// Formats a byte count with binary units.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn format_size(bytes: u64, style: SizeStyle) -> String {
    if bytes == 0 {
        return "0 B".to_owned();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    // Keep each style's original bucket calculation: the compact style's float
    // logarithm and the fixed style's integer thresholds can differ at powers.
    let index = match style {
        SizeStyle::Compact => (bytes as f64).log(1024_f64) as usize,
        SizeStyle::Fixed1 => bytes.ilog(1024) as usize,
    }
    .min(units.len() - 1);
    let value = bytes as f64 / 1024_f64.powi(index as i32);
    match style {
        SizeStyle::Compact if value >= 10.0 || index == 0 => {
            format!("{} {}", value.round() as u64, units[index])
        }
        SizeStyle::Fixed1 if index == 0 => format!("{bytes} B"),
        _ => format!("{value:.1} {}", units[index]),
    }
}
