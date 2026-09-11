//! Small formatting helpers shared by the renderers.

/// Render a duration in seconds as a human-friendly string.
#[must_use]
pub fn duration(seconds: f64) -> String {
    if seconds < 1.0 {
        return format!("{:.0} ms", seconds * 1000.0);
    }
    if seconds < 60.0 {
        return format!("{seconds:.1} s");
    }
    // Truncating division is exactly what is wanted when splitting a duration
    // into hours, minutes and seconds.
    #[allow(clippy::integer_division, clippy::cast_possible_truncation)]
    let (hours, minutes, secs) = {
        let total = seconds.round() as i64;
        (total / 3600, (total % 3600) / 60, total % 60)
    };
    if hours > 0 {
        format!("{hours}h {minutes:02}m {secs:02}s")
    } else {
        format!("{minutes}m {secs:02}s")
    }
}

/// Render a whole number of seconds as a human-friendly string.
#[must_use]
pub fn seconds(value: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    duration(value as f64)
}

/// Render a byte count.
#[must_use]
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    #[allow(clippy::cast_precision_loss)]
    let mut size = value as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 {
        format!("{value} {name}")
    } else {
        format!("{size:.1} {name}")
    }
}

/// Render an optional value, or a dash.
#[must_use]
pub fn optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

/// Escape the pipe characters that would break a Markdown table.
#[must_use]
pub fn table_cell(value: &str) -> String {
    let single_line = value.replace(['\n', '\r'], " ");
    single_line.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_readable() {
        assert_eq!(duration(0.123), "123 ms");
        assert_eq!(duration(12.5), "12.5 s");
        assert_eq!(duration(90.0), "1m 30s");
        assert_eq!(duration(3661.0), "1h 01m 01s");
    }

    #[test]
    fn byte_counts_are_readable() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2.0 KiB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn table_cells_cannot_break_a_table() {
        assert_eq!(table_cell("a|b"), "a\\|b");
        assert_eq!(table_cell("a\nb"), "a b");
    }

    #[test]
    fn missing_values_render_as_a_dash() {
        assert_eq!(optional(None::<u32>), "—");
        assert_eq!(optional(Some(3)), "3");
    }
}
