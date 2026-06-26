//! Human-readable size parsing and formatting.
//!
//! All byte conversions use powers of 1024, matching what
//! ``ls -h`` and the various CLIs display.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

/// Multipliers in powers of 1024, keyed by unit suffix.
fn multipliers() -> &'static HashMap<&'static str, u64> {
    static M: Lazy<HashMap<&'static str, u64>> = Lazy::new(|| {
        let mut m = HashMap::new();
        m.insert("B", 1);
        m.insert("KIB", 1024);
        m.insert("KB", 1024);
        m.insert("MIB", 1024 * 1024);
        m.insert("MB", 1024 * 1024);
        m.insert("GIB", 1024u64.pow(3));
        m.insert("GB", 1024u64.pow(3));
        m.insert("TIB", 1024u64.pow(4));
        m.insert("TB", 1024u64.pow(4));
        m.insert("PIB", 1024u64.pow(5));
        m.insert("PB", 1024u64.pow(5));
        m
    });
    &M
}

// Match an optional sign, a number (int or float, possibly with a
// thousands separator), optional whitespace, and a unit suffix.
static PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*(?P<sign>[+-]?)(?P<num>\d{1,3}(?:,\d{3})*|\d+(?:\.\d+)?)\s*(?P<unit>[a-zA-Z]+)?\s*$",
    )
    .expect("size regex compiles")
});

const PLACEHOLDER_VALUES: &[&str] = &["", "n/a", "na", "none", "<none>", "-", "?", "0b"];

/// Parse a human-readable size string into bytes.
///
/// Returns ``0`` for unparseable or placeholder values rather than
/// raising — the caller can decide whether that's an error.
pub fn parse_size(value: &str) -> u64 {
    let s = value.trim();
    if s.is_empty() || PLACEHOLDER_VALUES.iter().any(|p| s.eq_ignore_ascii_case(p)) {
        return 0;
    }

    let captures = match PATTERN.captures(s) {
        Some(c) => c,
        None => return 0,
    };

    let num_str = captures.name("num").map(|m| m.as_str()).unwrap_or("0");
    let unit_match = captures.name("unit").map(|m| m.as_str());
    let sign = captures.name("sign").map(|m| m.as_str()).unwrap_or("");

    let num_str_clean: String = num_str.replace(',', "");
    let num: f64 = match num_str_clean.parse() {
        Ok(n) => n,
        Err(_) => return 0,
    };
    let num = if sign == "-" { -num } else { num };

    let unit_str = unit_match.map(|u| u.to_ascii_uppercase());
    let unit = unit_str.as_deref().unwrap_or("B");

    match multipliers().get(unit) {
        Some(&m) => (num * m as f64).round() as i64,
        None => {
            // If the input had a unit suffix we didn't recognise
            // (e.g. ``"12xyz"``) the value is not a real size —
            // return 0 rather than misinterpreting the leading
            // number as bytes.
            if unit_match.is_some() {
                0
            } else {
                num.round() as i64
            }
        }
    }
    .max(0) as u64
}

/// Format a byte count as a human-readable string.
///
/// *binary* controls whether 1024-based (KiB/MiB) or 1000-based
/// (KB/MB) units are used.
pub fn format_size(num_bytes: i64, binary: bool) -> String {
    if num_bytes < 0 {
        return format!("-{}", format_size(-num_bytes, binary));
    }
    let n = num_bytes as f64;
    let (base, units): (u64, &[&str]) = if binary {
        (1024, &["B", "KiB", "MiB", "GiB", "TiB", "PiB"])
    } else {
        (1000, &["B", "KB", "MB", "GB", "TB", "PB"])
    };
    if n < base as f64 {
        return format!("{} {}", num_bytes, units[0]);
    }
    let mut value = n;
    for unit in &units[1..] {
        value /= base as f64;
        if value < base as f64 {
            return format!("{:.1} {}", value, unit);
        }
    }
    format!("{:.1} {}", value, units.last().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        assert_eq!(parse_size("0B"), 0);
        assert_eq!(parse_size("0"), 0);
        assert_eq!(parse_size(""), 0);
        assert_eq!(parse_size("1B"), 1);
        assert_eq!(parse_size("1 KB"), 1024);
        assert_eq!(parse_size("1KB"), 1024);
        assert_eq!(parse_size("234 MB"), 234 * 1024 * 1024);
        assert_eq!(parse_size("2 GB"), 2 * 1024u64.pow(3));
        assert_eq!(parse_size("12MiB"), 12 * 1024 * 1024);
        assert_eq!(parse_size("3.8 GB"), (3.8 * 1024f64.powf(3.0)) as u64);
        assert_eq!(parse_size("1,234 KB"), 1234 * 1024);
    }

    #[test]
    fn parse_garbage_returns_zero() {
        assert_eq!(parse_size("not a size"), 0);
        assert_eq!(parse_size("12xyz"), 0);
    }

    #[test]
    fn parse_placeholders() {
        assert_eq!(parse_size("<none>"), 0);
        assert_eq!(parse_size("-"), 0);
        assert_eq!(parse_size("N/A"), 0);
    }

    #[test]
    fn format_bytes() {
        assert_eq!(format_size(0, true), "0 B");
        assert_eq!(format_size(512, true), "512 B");
        assert_eq!(format_size(1024, true), "1.0 KiB");
        assert_eq!(format_size(2 * 1024u64.pow(3) as i64, true), "2.0 GiB");
    }

    #[test]
    fn format_decimal_units() {
        assert_eq!(format_size(1500, false), "1.5 KB");
    }
}
