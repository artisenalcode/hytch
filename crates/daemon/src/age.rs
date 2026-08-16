//! Human-readable session age, for the end-of-session log marker.

/// Format a duration in whole seconds as e.g. `"1d 2h 3m 4s"`, dropping
/// leading zero units (`"5s"`, `"3m 4s"`, `"2h 3m 4s"`).
pub fn format_age(total_secs: u64) -> String {
    let d = total_secs / 86400;
    let h = (total_secs % 86400) / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;

    if d > 0 {
        format!("{d}d {h}h {m}m {s}s")
    } else if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_age;

    #[test]
    fn seconds_only() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(45), "45s");
    }

    #[test]
    fn minutes_and_seconds() {
        assert_eq!(format_age(65), "1m 5s");
        assert_eq!(format_age(3599), "59m 59s");
    }

    #[test]
    fn hours_minutes_seconds() {
        assert_eq!(format_age(3661), "1h 1m 1s");
        assert_eq!(format_age(86399), "23h 59m 59s");
    }

    #[test]
    fn days_hours_minutes_seconds() {
        assert_eq!(format_age(90065), "1d 1h 1m 5s");
    }
}
