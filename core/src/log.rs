//! Shared action log for tracing what the app is doing.
//!
//! Every UI surface (GUI, TUI, CLI) gets an `ActionLog`
//! handle and pushes entries at key events (scan start/end,
//! delete attempts, errors, user actions).  The log is
//! in-memory, thread-safe, and cheap to clone (`Arc`-based
//! internally).
//!
//! **Not a general-purpose logger.**  This is a focused
//! action log for tracing user-visible app behaviour, not a
//! replacement for `tracing`/`log`/`env_logger`.  The three
//! differences that matter:
//!
//! 1. **Persistent handle.**  The log is stored in
//!    `Rc`/`Arc` so the GUI's `State` and the orchestrator
//!    can each hold a clone and push entries from anywhere.
//! 2. **Snapshotable.**  `entries()` returns a `Vec<LogEntry>`
//!    that the UIs can render directly (dialog body, TUI
//!    panel, etc.).
//! 3. **Bounded.**  `with_capacity(cap)` caps the log at
//!    `cap` entries; oldest entries are dropped on overflow
//!    so a long-running session doesn't grow unbounded.
//!
//! **Timestamps.**  `LogEntry::timestamp` is
//! `std::time::SystemTime` so the format is portable.  The
//! UIs can format it however they like.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Severity of a log entry.  Ordered from least to most
/// severe so `PartialOrd`/`Ord` derive meaningfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Informational: a normal action completed.
    Info,
    /// Warning: an action had a non-fatal problem.
    Warn,
    /// Error: an action failed.
    Error,
}

impl LogLevel {
    /// Short uppercase tag for compact display surfaces
    /// (e.g. ``"[INFO]"``).
    pub fn tag(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// A single log entry.  Cheap to clone (two `String`s and a
/// `SystemTime`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub timestamp: SystemTime,
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    /// Format the entry as a single line for display, e.g.
    /// ``"[2024-01-15T10:30:45Z] [INFO] Scanning started"``.
    ///
    /// **Timestamp format.**  Uses RFC 3339 / ISO 8601
    /// (via `humantime`-style fallback to seconds since
    /// UNIX_EPOCH if the conversion fails).  The exact
    /// format is not part of the public contract -- it is
    /// for human display only and may change.
    pub fn format_line(&self) -> String {
        let ts = system_time_to_rfc3339(self.timestamp);
        format!("[{}] [{}] {}", ts, self.level.tag(), self.message)
    }
}

/// Render a [`SystemTime`] as a UTC RFC 3339 / ISO 8601 string
/// of the form ``"YYYY-MM-DDTHH:MM:SSZ"``.  Times before
/// `UNIX_EPOCH` (which shouldn't happen in practice) are
/// rendered as ``"<unknown time>"`` so the formatter never panics.
///
/// Exposed publicly so other modules (notably
/// [`crate::history`]) can render timestamps in the same
/// shape without duplicating the algorithm.  Uses the
/// civil-from-days algorithm from Howard Hinnant's
/// ``<chrono>/`` proposal -- no `chrono` / `time` deps.
pub fn system_time_to_rfc3339(t: SystemTime) -> String {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs();
            let (year, month, day, hour, min, sec) = epoch_to_utc(secs);
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                year, month, day, hour, min, sec
            )
        }
        Err(_) => "<unknown time>".to_string(),
    }
}

/// Thread-safe, cheaply-cloneable action log.
///
/// Internally an `Arc<Mutex<Vec<LogEntry>>>` so cloning the
/// handle is just an `Arc` bump and entries can be pushed
/// from any thread (the orchestrator's async tasks, the UI
/// thread, etc.).
#[derive(Debug, Clone)]
pub struct ActionLog {
    inner: Arc<Mutex<Vec<LogEntry>>>,
    /// Maximum number of entries to retain.  When the log
    /// is full, the oldest entry is dropped on each push.
    capacity: usize,
}

impl ActionLog {
    /// Create a new log with the given capacity.  A capacity
    /// of 0 means "unbounded" -- every entry is kept.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            capacity,
        }
    }

    /// Push an entry at the given level.
    pub fn push(&self, level: LogLevel, message: impl Into<String>) {
        let entry = LogEntry {
            timestamp: SystemTime::now(),
            level,
            message: message.into(),
        };
        let mut guard = self.inner.lock().expect("ActionLog mutex poisoned");
        if self.capacity > 0 && guard.len() >= self.capacity {
            // Drop the oldest entry.  O(n) shift is fine:
            // we only do this once per push at the cap, and
            // a typical cap is a few hundred entries.
            guard.remove(0);
        }
        guard.push(entry);
    }

    /// Convenience: push an `Info` entry.
    pub fn info(&self, message: impl Into<String>) {
        self.push(LogLevel::Info, message);
    }

    /// Convenience: push a `Warn` entry.
    pub fn warn(&self, message: impl Into<String>) {
        self.push(LogLevel::Warn, message);
    }

    /// Convenience: push an `Error` entry.
    pub fn error(&self, message: impl Into<String>) {
        self.push(LogLevel::Error, message);
    }

    /// Return a snapshot of all current entries, oldest first.
    pub fn entries(&self) -> Vec<LogEntry> {
        self.inner.lock().expect("ActionLog mutex poisoned").clone()
    }

    /// Return the number of entries currently stored.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("ActionLog mutex poisoned").len()
    }

    /// True if the log has no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all entries.  Useful for the GUI's "Clear log"
    /// button and for tests.
    pub fn clear(&self) {
        self.inner.lock().expect("ActionLog mutex poisoned").clear();
    }

    /// Format all entries as a multi-line string, oldest
    /// first.  Each line uses [`LogEntry::format_line`].
    pub fn format_lines(&self) -> String {
        let entries = self.entries();
        let mut out = String::new();
        for (i, e) in entries.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&e.format_line());
        }
        out
    }
}

impl Default for ActionLog {
    fn default() -> Self {
        // 500 entries is enough for a few minutes of
        // typical app activity without unbounded growth.
        Self::new(500)
    }
}

/// Convert seconds-since-UNIX_EPOCH to a UTC
/// ``(year, month, day, hour, min, sec)`` tuple.  Uses the
/// civil-from-days algorithm from Howard Hinnant's
/// ``<chrono>/`` proposal -- no dependencies, no leap
/// second table, good enough for a debug log.
fn epoch_to_utc(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let hour = (secs_of_day / 3600) as u32;
    let min = ((secs_of_day % 3600) / 60) as u32;
    let sec = (secs_of_day % 60) as u32;
    // Shift epoch from 1970-01-01 to 0000-03-01 (the
    // algorithm's base date).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32; // [0, 146_096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d, hour, min, sec)
}
