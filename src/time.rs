//! Clock helpers that need no Win32: unix time, ISO 8601 parsing, countdowns.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.frac][Z|±HH:MM]` into unix seconds.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |a: usize, z: usize| -> Option<i64> {
        let part = s.get(a..z)?;
        if !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let oh = num(i + 1, i + 3)?;
            let om = if b.get(i + 3) == Some(&b':') { num(i + 4, i + 6)? } else { num(i + 3, i + 5)? };
            let o = oh * 3600 + om * 60;
            if sign == b'-' { -o } else { o }
        }
        _ => return None,
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Compact duration such as `4d 21h`, `1h 05m`, `12m` or `<1m`.
pub fn countdown(secs: i64) -> String {
    if secs < 60 {
        return "<1m".into();
    }
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_timestamps() {
        // 2026-10-08T22:00:00Z
        assert_eq!(parse_iso8601("2026-10-08T22:00:00.089159+00:00"), Some(1_791_496_800));
        assert_eq!(parse_iso8601("2026-10-08T22:00:00Z"), Some(1_791_496_800));
        assert_eq!(parse_iso8601("2026-10-09T00:00:00+02:00"), Some(1_791_496_800));
        assert_eq!(parse_iso8601("2026-10-08T17:00:00-0500"), Some(1_791_496_800));
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_iso8601(""), None);
        assert_eq!(parse_iso8601("2026-13-08T22:00:00Z"), None);
        assert_eq!(parse_iso8601("2026-10-08 22:00"), None);
        assert_eq!(parse_iso8601("2026-10-08T22:00:00+xx:00"), None);
    }

    #[test]
    fn countdowns() {
        assert_eq!(countdown(30), "<1m");
        assert_eq!(countdown(12 * 60 + 5), "12m");
        assert_eq!(countdown(3600 + 5 * 60), "1h 05m");
        assert_eq!(countdown(4 * 86_400 + 21 * 3600 + 59), "4d 21h");
    }
}
