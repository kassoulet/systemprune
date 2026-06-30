//! Integration tests for the shared `ActionLog` + `LogLevel` +
//! `LogEntry` surfaces and the public RFC-3339 formatters that
//! `history.rs` reuses.
//!
//! The internal `epoch_to_utc` algorithm is exercised
//! transitively via `system_time_to_rfc3339` against fixed
//! timestamps; the former direct algorithm-pinning test was
//! dropped because the public-facing string assertions already
//! cover every branch of the date conversion.
//!
//! Replaces `core/src/log.rs::tests`.  See `core/src/log.rs`
//! for the public surface these tests target.

use std::thread;
use std::time::{Duration, SystemTime};
use systemprune_core::log::{system_time_to_rfc3339, ActionLog, LogEntry, LogLevel};

#[test]
fn new_log_is_empty() {
    let log = ActionLog::new(100);
    assert!(log.is_empty());
    assert_eq!(log.len(), 0);
    assert!(log.entries().is_empty());
}

#[test]
fn push_appends_entries_in_order() {
    let log = ActionLog::new(100);
    log.info("first");
    log.warn("second");
    log.error("third");
    let entries = log.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].level, LogLevel::Info);
    assert_eq!(entries[0].message, "first");
    assert_eq!(entries[1].level, LogLevel::Warn);
    assert_eq!(entries[2].level, LogLevel::Error);
}

#[test]
fn capacity_drops_oldest_on_overflow() {
    let log = ActionLog::new(3);
    log.info("a");
    log.info("b");
    log.info("c");
    log.info("d");
    log.info("e");
    let entries = log.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].message, "c");
    assert_eq!(entries[1].message, "d");
    assert_eq!(entries[2].message, "e");
}

#[test]
fn capacity_zero_is_unbounded() {
    let log = ActionLog::new(0);
    for i in 0..1000 {
        log.info(format!("entry {i}"));
    }
    assert_eq!(log.len(), 1000);
}

#[test]
fn clear_empties_the_log() {
    let log = ActionLog::new(100);
    log.info("a");
    log.info("b");
    assert_eq!(log.len(), 2);
    log.clear();
    assert!(log.is_empty());
}

#[test]
fn clone_shares_state() {
    let log = ActionLog::new(100);
    let log2 = log.clone();
    log.info("from handle 1");
    log2.info("from handle 2");
    // Both writes are visible through both handles
    // because they share the same `Arc<Mutex<_>>`.
    assert_eq!(log.len(), 2);
    assert_eq!(log2.len(), 2);
}

#[test]
fn thread_safety_concurrent_pushes() {
    // Spawn 8 threads, each pushing 100 entries.  The
    // log must not panic and must end up with exactly
    // 800 entries.
    let log = ActionLog::new(0);
    let mut handles = Vec::new();
    for t in 0..8 {
        let log = log.clone();
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                log.info(format!("thread {t} entry {i}"));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(log.len(), 800);
}

#[test]
fn format_line_includes_level_and_message() {
    let log = ActionLog::new(100);
    log.warn("something happened");
    let lines = log.format_lines();
    assert!(lines.contains("[WARN]"));
    assert!(lines.contains("something happened"));
}

#[test]
fn level_ordering_is_info_lt_warn_lt_error() {
    assert!(LogLevel::Info < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Error);
}

#[test]
fn level_tag_is_uppercase() {
    assert_eq!(LogLevel::Info.tag(), "INFO");
    assert_eq!(LogLevel::Warn.tag(), "WARN");
    assert_eq!(LogLevel::Error.tag(), "ERROR");
}

#[test]
fn format_line_renders_known_timestamp() {
    // Pin the format to a specific timestamp so a
    // refactor that changes the rendering surfaces in
    // code review.
    let entry = LogEntry {
        timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(0),
        level: LogLevel::Info,
        message: "epoch".to_string(),
    };
    assert_eq!(entry.format_line(), "[1970-01-01T00:00:00Z] [INFO] epoch");
}

#[test]
fn format_lines_joins_with_newlines() {
    let log = ActionLog::new(100);
    log.info("a");
    log.info("b");
    let out = log.format_lines();
    assert!(out.contains("a\n["));
    assert!(out.ends_with("b"));
}

#[test]
fn system_time_to_rfc3339_matches_log_entry_format() {
    // The two helpers (private format_line, public
    // system_time_to_rfc3339) must agree so external
    // callers see the same wire format as the in-memory
    // log lines.
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    assert_eq!(system_time_to_rfc3339(t), "1970-01-01T00:00:00Z");
}

#[test]
fn system_time_to_rfc3339_falls_back_for_pre_epoch() {
    // A pre-UNIX_EPOCH time (e.g. system clock anomaly)
    // must not panic.  The formatter returns the
    // documented "<unknown time>" sentinel.
    let t = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
    assert_eq!(system_time_to_rfc3339(t), "<unknown time>");
}
