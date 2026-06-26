//! Tests for the size parser and formatter.

use systemprune_core::{format_size, parse_size};

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
    assert_eq!(format_size(2 * 1024i64.pow(3), true), "2.0 GiB");
}

#[test]
fn format_decimal_units() {
    assert_eq!(format_size(1500, false), "1.5 KB");
}

#[test]
fn format_negative() {
    let out = format_size(-2048, true);
    assert!(out.starts_with('-'));
    assert!(out.contains("KiB"));
}
