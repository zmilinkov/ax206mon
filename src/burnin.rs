//! Screen-protection logic: night backlight schedule, header variation and
//! the periodic idle refresh. Pure functions so they are easy to test.

use serde::Deserialize;

/// How the clock header moves/changes to avoid lighting the same pixels
/// all day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variation {
    /// Header always looks the same.
    Off,
    /// Each page gets its own header position/colour.
    Page,
    /// The look changes every `header_variation_minutes`.
    Time,
    /// Both: page and time together.
    #[default]
    Both,
}

/// Number of distinct header looks cycled through.
pub const VARIANTS: usize = 4;

/// Which header look to use (0..VARIANTS).
pub fn header_variant(v: Variation, page: usize, epoch_minutes: u64, interval_minutes: f64) -> usize {
    let time = (epoch_minutes as f64 / interval_minutes.max(1.0)) as usize;
    let n = match v {
        Variation::Off => 0,
        Variation::Page => page,
        Variation::Time => time,
        Variation::Both => page + time,
    };
    n % VARIANTS
}

/// "HH:MM" -> minutes since midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Is `now` inside [start, end)? Handles windows that cross midnight.
pub fn in_window(start: u32, end: u32, now: u32) -> bool {
    if start < end {
        (start..end).contains(&now)
    } else {
        now >= start || now < end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hhmm() {
        assert_eq!(parse_hhmm("07:30"), Some(450));
        assert_eq!(parse_hhmm("7:05"), Some(425));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("noon"), None);
    }

    #[test]
    fn window_crossing_midnight() {
        let (s, e) = (parse_hhmm("23:00").unwrap(), parse_hhmm("07:00").unwrap());
        assert!(in_window(s, e, parse_hhmm("23:30").unwrap()));
        assert!(in_window(s, e, parse_hhmm("03:00").unwrap()));
        assert!(!in_window(s, e, parse_hhmm("07:00").unwrap()));
        assert!(!in_window(s, e, parse_hhmm("12:00").unwrap()));
        // Same-day window.
        assert!(in_window(60, 120, 90));
        assert!(!in_window(60, 120, 120));
    }

    #[test]
    fn variant_selection() {
        assert_eq!(header_variant(Variation::Off, 3, 1000, 30.0), 0);
        assert_eq!(header_variant(Variation::Page, 2, 1000, 30.0), 2);
        // Time-based: advances once per interval.
        let a = header_variant(Variation::Time, 0, 60, 30.0);
        let b = header_variant(Variation::Time, 0, 89, 30.0);
        let c = header_variant(Variation::Time, 0, 90, 30.0);
        assert_eq!(a, b);
        assert_ne!(b, c);
        assert!(header_variant(Variation::Both, 3, 12345, 30.0) < VARIANTS);
    }
}
