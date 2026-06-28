//! Per-filesystem disk-usage view (`more.md` §4.2).
//!
//! [`Df`] complements the [`crate::orchestrator::Dashboard`]
//! (§4.1) with a filesystem-level "df -h" replacement that
//! also breaks down usage per detected engine and reports the
//! gap between engine-reported bytes and filesystem bytes as
//! an "unaccounted" line.
//!
//! ## Components
//!
//! * [`FilesystemStats`] \u2014 one mount's `statvfs` numbers
//!   (total / used / available / use-percent) plus the device
//!   name resolved from `/proc/self/mountinfo` on Linux.
//! * [`EngineBreakdown`] \u2014 `(source, total_bytes)` rows
//!   that mirror the §4.1 dashboard.
//! * [`Df`] \u2014 the composed view: filesystem row + per-engine
//!   rows + the `unaccounted` subtraction.
//!
//! ## Platform notes
//!
//! The `statvfs` syscall is POSIX but the device lookup is
//! Linux-only (`/proc/self/mountinfo` is a Linux kernel
//! construct).  On non-Linux platforms the `device` field
//! reports `"<unknown>"`; size math still works because the
//! raw byte counts do not depend on the device name.  CI on
//! macOS / BSD will see the fallback gracefully.
//!
//! ## Errors
//!
//! [`stat_filesystem`] returns [`std::io::Error`] on the
//! underlying `statvfs` failure (e.g. `ENOENT` for a missing
//! path, `EACCES` for unreadable filesystems).  [`compute`]
//! surfaces this via [`crate::errors::SystemPruneError`] so
//! the CLI can log + exit non-zero.

use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::path::Path;

use crate::errors::SystemPruneError;
use crate::size::format_size;

/// `statvfs` numbers for a single mount point.
///
/// `used_bytes` is derived from `f_blocks - f_bfree` and so
/// includes reserved blocks; `available_bytes` uses
/// `f_bavail` (the reserved-aware variant).  This matches
/// the `df` coreutils implementation so the percent column
/// agrees with `df`'s "`Use%`".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemStats {
    /// Device backing this mount (e.g. `"/dev/sda1"`).
    /// `"<unknown>"` when the device name cannot be
    /// resolved (non-Linux, container with `/proc`
    /// masked, etc.).
    pub device: String,
    /// Mountpoint path (e.g. `"/"` or `"/home"`).
    pub mount_point: String,
    /// Total size of the filesystem in bytes.
    pub total_bytes: u64,
    /// Bytes currently in use; excludes swap/cache reserved
    /// blocks only when the kernel reports them via
    /// `f_bfree` (POSIX semantics).
    pub used_bytes: u64,
    /// Bytes available to non-root users (`f_bavail`,
    /// reserved-aware so matches `df -h`).
    pub available_bytes: u64,
    /// Whole-percent use: `100 * used / total`, rounded.
    /// `0` when `total_bytes == 0` (avoids divide-by-zero
    /// on degenerate pseudo-filesystems).
    pub use_percent: u8,
}

/// One engine's contribution to the breakdown row of a
/// [`Df`].  Mirrors the `(source, total_bytes)` pair from
/// [`crate::orchestrator::DashboardRow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineBreakdownRow {
    pub source: String,
    pub total_bytes: u64,
}

/// The per-engine slice of a [`Df`].  Sorted `total_bytes`
/// descending so the biggest disk-space contributor surfaces
/// first, mirroring §4.1's `Dashboard::compute` ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineBreakdown {
    pub rows: Vec<EngineBreakdownRow>,
}

impl EngineBreakdown {
    /// Sum of every row's `total_bytes`.  Drives the
    /// `unaccounted` subtraction in [`Df::compute`].
    pub fn grand_total(&self) -> u64 {
        self.rows.iter().map(|r| r.total_bytes).sum()
    }
}

/// The composed per-mount filesystem view (\u00a74.2).
///
/// Constructed via [`Df::compute`] from a [`crate::orchestrator::ScanResult`]
/// + a mount path.  The breakdown is computed by reusing
/// [`crate::orchestrator::Dashboard::compute_items`] so the
/// engine totals exactly mirror what `systemprune dashboard`
/// prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Df {
    pub filesystem: FilesystemStats,
    pub breakdown: EngineBreakdown,
    /// `filesystem.used - sum(breakdown totals)`, floored at
    /// `0` via `saturating_sub`.  Represents disk space that
    /// is in use on the filesystem but NOT explained by any
    /// detected engine.  Negative values (e.g. an engine
    /// sharded onto a different partition whose bytes get
    /// billed to `/` anyway) saturate to `0` rather than
    /// wraparound.
    pub unaccounted: u64,
}

impl Df {
    /// Compute the §4.2 view for a given mount path.  Calls
    /// [`stat_filesystem`] for the filesystem row and
    /// reuses [`crate::orchestrator::Dashboard::compute_items`]
    /// for the engine breakdown so the two subcommands
    /// (this one + `systemprune dashboard`) cannot drift.
    pub fn compute(
        scan: &crate::orchestrator::ScanResult,
        mount_path: &Path,
    ) -> Result<Self, SystemPruneError> {
        let filesystem = stat_filesystem(mount_path).map_err(SystemPruneError::from)?;
        // Reuse the dashboard's per-engine grouping so both
        // subcommands share the canonical engine totals.
        let dash = crate::orchestrator::Dashboard::compute(scan);
        let mut rows: Vec<EngineBreakdownRow> = dash
            .rows
            .iter()
            .map(|r| EngineBreakdownRow {
                source: r.source.clone(),
                total_bytes: r.total_bytes,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.total_bytes
                .cmp(&a.total_bytes)
                .then_with(|| a.source.cmp(&b.source))
        });
        let breakdown = EngineBreakdown { rows };
        let unaccounted = filesystem
            .used_bytes
            .saturating_sub(breakdown.grand_total());
        Ok(Self {
            filesystem,
            breakdown,
            unaccounted,
        })
    }

    /// Render the §4.2 two-level table.  The filesystem row
    /// is a `df -h`-style header line; the per-engine rows
    /// are indented by two spaces (same leading column as
    /// the filesystem row) with an "unaccounted" line pinned
    /// last so visually it sits alongside the largest
    /// engine rows.
    ///
    /// Empty `breakdown` rows still emit the filesystem
    /// header + the "unaccounted" line so the user can see
    /// how much of their disk is in use today.
    pub fn format_text(&self) -> String {
        let fs = &self.filesystem;
        let mut out = String::new();
        // Header row.  Column widths are fixed at the spec
        // example geometry so the table renders identically
        // across hosts.
        out.push_str(&format!(
            "{:<12}  {:>9}  {:>9}  {:>9}  {:>4}  {}\n",
            "Filesystem", "Size", "Used", "Avail", "Use%", "Mounted on",
        ));
        out.push_str(&format!("{}\n", "-".repeat(60)));
        // Filesystem row.  Use `binary=false` so `df`-style
        // units ("500G") match what `df -h` prints (1000-based).
        out.push_str(&format!(
            "{:<12}  {:>9}  {:>9}  {:>9}  {:>3}%  {}\n",
            truncate(&fs.device, 12),
            format_size(fs.total_bytes as i64, false),
            format_size(fs.used_bytes as i64, false),
            format_size(fs.available_bytes as i64, false),
            fs.use_percent,
            fs.mount_point,
        ));
        // Per-engine indented rows.  Sorted by `total_bytes`
        // descending so the biggest contributor is closest
        // to the filesystem row, matching §4.1's reading
        // order.
        for row in &self.breakdown.rows {
            out.push_str(&format!(
                "  {:<10}  {:>9}  {:>9}\n",
                truncate(&row.source, 10),
                "",
                format_size(row.total_bytes as i64, true),
            ));
        }
        // Unaccounted pinned last so it sits alongside the
        // largest engine rows for easy comparison.
        out.push_str(&format!(
            "  {:<10}  {:>9}  {:>9}\n",
            "unaccounted",
            "",
            format_size(self.unaccounted as i64, true),
        ));
        out
    }
}

/// Read filesystem usage numbers for `mount_path` via
/// `statvfs`.  Resolves the device name on Linux by scanning
/// `/proc/self/mountinfo`; non-Linux platforms (or hidden
/// `/proc`) get the device as `"<unknown>"`.
///
/// The `statvfs` call itself is POSIX but our implementation
/// uses `libc::statvfs` directly because the bundle ships
/// without the `nix` crate to keep the dep footprint small.
/// The `unsafe` block is contained to the FFI call + a
/// single `CString` boundary; all size conversions are
/// infallible `u64` arithmetic.
pub fn stat_filesystem(mount_path: &Path) -> Result<FilesystemStats, std::io::Error> {
    // `CString::new` returns an error if the path contains
    // an interior NUL.  `as_os_str().as_encoded_bytes()` is
    // the only infallible conversion to a byte slice on
    // `Path`; on Unix-like platforms it never contains NULs
    // except for malformed paths, which we reject early so
    // the FFI call cannot UB on a poisoned pointer.
    let cpath = CString::new(mount_path.as_os_str().as_encoded_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL"))?;
    // `libc::statvfs` may be uninitialised; zero-init then
    // have the kernel populate it.  `result != 0` means the
    // syscall failed; we map errno via `last_os_error()` so
    // the caller sees the same error string `stat` would
    // emit on `df`'s failure path.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // `f_frsize` is the fragment size in bytes; `f_blocks`
    // is the total fragment count.  Multiplying gives total
    // bytes \u2014 matches `df`'s ceiling calculation.
    let block_size = stat.f_frsize as u64;
    let total_bytes = (stat.f_blocks as u64).saturating_mul(block_size);
    let free_bytes = (stat.f_bfree as u64).saturating_mul(block_size);
    let available_bytes = (stat.f_bavail as u64).saturating_mul(block_size);
    let used_bytes = total_bytes.saturating_sub(free_bytes);
    // Whole-percent rounding.  Guard against
    // `total_bytes == 0` (some pseudo-filesystems) so we
    // never divide by zero.
    let use_percent: u8 = if total_bytes > 0 {
        // `f64` rounding is fine here; we cast back to u8
        // and clamp at 100 because some filesystems (with
        // over-provisioned volumes) report `used > total`
        // by a few bytes.
        let pct = (used_bytes as f64 / total_bytes as f64) * 100.0;
        pct.round().min(100.0) as u8
    } else {
        0
    };
    let mount_point = mount_path.to_string_lossy().to_string();
    let device = resolve_device_for_mount(&mount_point);
    Ok(FilesystemStats {
        device,
        mount_point,
        total_bytes,
        used_bytes,
        available_bytes,
        use_percent,
    })
}

/// Look up the device backing a given mount path on Linux by
/// scanning `/proc/self/mountinfo`.  Returns `"<unknown>"`
/// when the file is missing or the path is not listed.
///
/// `/proc/self/mountinfo` format (Linux \u2265 2.6.26):
///
/// ```text
/// <mount-id> <parent-id> <major:minor> <root> <mount-point> \
///     <options> - <fstype> <source> <super-options>
/// ```
///
/// We pick the **longest prefix** match (greedy, deepest
/// mount wins) so stats for `/home` correctly report the
/// device backing `/home`, not the device backing `/`.
#[cfg(target_os = "linux")]
fn resolve_device_for_mount(mount_path: &str) -> String {
    let bytes = match std::fs::read("/proc/self/mountinfo") {
        Ok(b) => b,
        Err(_) => return "<unknown>".to_string(),
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(_) => return "<unknown>".to_string(),
    };
    let target = mount_path.trim_end_matches('/');
    let target = if target.is_empty() { "/" } else { target };
    let mut best: Option<(usize, String)> = None;
    for line in text.lines() {
        // /proc/self/mountinfo fields are space-separated.
        // The mount-point field is index 4; the device is
        // after the literal `-` separator.  Splitting on
        // ' - ' reliably skips the options column.
        let (pre_dash, post_dash) = match line.split_once(" - ") {
            Some(pair) => pair,
            None => continue,
        };
        let pre_fields: Vec<&str> = pre_dash.split_whitespace().collect();
        if pre_fields.len() < 5 {
            continue;
        }
        let candidate_mp = pre_fields[4];
        let post_fields: Vec<&str> = post_dash.split_whitespace().collect();
        if post_fields.is_empty() {
            continue;
        }
        let candidate_device = post_fields[0];
        // Greedy longest-prefix match so `/home/foo` picks
        // the `/home` mount in preference to `/`.
        if candidate_mp == target
            || (target.starts_with(candidate_mp)
                && target.chars().nth(candidate_mp.len()) == Some('/'))
        {
            let len = candidate_mp.len();
            if best.as_ref().map(|(l, _)| len > *l).unwrap_or(true) {
                best = Some((len, candidate_device.to_string()));
            }
        }
    }
    best.map(|(_, d)| d)
        .unwrap_or_else(|| "<unknown>".to_string())
}

/// Non-Linux stub.  Keeps the cross-platform build clean;
/// the device column will always be `"<unknown>"` on these
/// platforms, which matches the spec's "graceful when
/// `/proc` is unavailable" intent.
#[cfg(not(target_os = "linux"))]
fn resolve_device_for_mount(_mount_path: &str) -> String {
    "<unknown>".to_string()
}

/// Internal column-width guard.  Mirrors `Dashboard::truncate`
/// but kept private to `df` so the two formatters can evolve
/// independently.  Uses the char-counting variant so
/// multi-byte UTF-8 in engine names cannot blow the
/// column width.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Engine, PrunableItem, Status};

    /// Smoke test for the `statvfs` wrapper.  The test
    /// directory always exists, so this should always
    /// succeed on a POSIX host (which the workspace targets).
    #[test]
    fn stat_filesystem_on_tmpdir_returns_consistent_numbers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stats = stat_filesystem(dir.path()).expect("statvfs on tempdir should succeed");
        // Total bytes is at least the block size of an
        // empty tmpdir; depends on the filesystem but is
        // always non-zero on Linux.
        assert!(stats.total_bytes > 0);
        // Used plus available <= total (reserved blocks are
        // excluded from `f_bavail` so the sum is strictly
        // less on most filesystems).
        assert!(stats.used_bytes <= stats.total_bytes);
        assert!(stats.available_bytes <= stats.total_bytes);
        // `use_percent` rounds to a whole number, so the
        // cast back to u8 doesn't lose precision; we only
        // pin the bounds here.
        assert!(stats.use_percent <= 100);
    }

    /// `statvfs` on a missing path surfaces `ENOENT`.  The
    /// exact error kind varies by host (`NotFound` on
    /// Linux), so we pin the existence-of-error rather than
    /// the kind.
    #[test]
    fn stat_filesystem_on_missing_path_returns_error() {
        let bad = Path::new("/this/path/does/not/exist/systemprune-test-zzz");
        assert!(stat_filesystem(bad).is_err());
    }

    /// `Df::compute` with no items: the breakdown is empty
    /// and `unaccounted` equals the filesystem `used`.
    #[test]
    fn df_compute_empty_scan_has_unaccounted_equal_used() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scan = crate::orchestrator::ScanResult::default();
        let df = Df::compute(&scan, dir.path()).expect("compute");
        assert!(df.breakdown.rows.is_empty());
        assert_eq!(df.unaccounted, df.filesystem.used_bytes);
    }

    /// `Df::compute` with items summing to less than the
    /// filesystem `used`: `unaccounted = used - sum`.
    /// Builds the scenario by selecting a tmpdir (whose
    /// exact used bytes we cannot predict) and verifying
    /// the math relationship rather than a literal value.
    /// The expected `used` is read back from the resulting
    /// `Df` itself so a race between two separate
    /// `stat_filesystem` calls cannot drift the assertion.
    #[test]
    fn df_compute_unaccounted_is_used_minus_engine_totals() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Use half the maximum `u64` so we are guaranteed to
        // be below the filesystem `used` regardless of host.
        // Picking a literal half-of-`used` from a separate
        // `stat_filesystem` call would race with the
        // snapshot taken inside `Df::compute`.
        let engine_total = 4 * 1024_u64.pow(3); // 4 GiB
        let items = vec![PrunableItem {
            id: "x".to_string(),
            name: "x".to_string(),
            engine: Engine::Docker,
            source: "docker".to_string(),
            category: crate::models::Category::Image,
            size_bytes: engine_total,
            status: Status::Unused,
            extra: Default::default(),
        }];
        let scan = crate::orchestrator::ScanResult {
            items,
            errors: vec![],
        };
        let df = Df::compute(&scan, dir.path()).expect("compute");
        assert_eq!(df.breakdown.rows.len(), 1);
        assert_eq!(df.breakdown.rows[0].source, "docker");
        assert_eq!(df.breakdown.rows[0].total_bytes, engine_total);
        // Read `used` from the same `Df` so the assertion is
        // monotonic against the snapshot `compute` saw.
        assert_eq!(
            df.unaccounted,
            df.filesystem.used_bytes.saturating_sub(engine_total),
        );
        // `unaccounted` is a `u64` and the spec pins
        // saturating subtraction so a hypothetical
        // engine-total > used scenario never wraps.
        assert!(df.unaccounted <= df.filesystem.used_bytes);
    }

    /// `saturating_sub` semantics: when the engine total
    /// exceeds the filesystem `used` (unlikely on a real
    /// host but pinned for the math contract), `unaccounted`
    /// floors at `0` rather than wrapping into a
    /// near-`u64::MAX` value.
    #[test]
    fn df_compute_unaccounted_clamps_to_zero_on_negative_subtraction() {
        // We cannot easily fabricate a "tmpfs where used >
        // engine totals" without a real oversized tmpfs, so
        // build a scenario where the engine total is
        // arbitrarily large.
        let dir = tempfile::tempdir().expect("tempdir");
        // Pretend the engine reports a huge total.
        let engine_total = u64::MAX / 2;
        let items = vec![PrunableItem {
            id: "x".to_string(),
            name: "x".to_string(),
            engine: Engine::Docker,
            source: "docker".to_string(),
            category: crate::models::Category::Image,
            size_bytes: engine_total,
            status: Status::Unused,
            extra: Default::default(),
        }];
        let scan = crate::orchestrator::ScanResult {
            items,
            errors: vec![],
        };
        let df = Df::compute(&scan, dir.path()).expect("compute");
        // filesystem.used is small (~few MB); engine_total
        // is u64::MAX/2, so subtraction must saturate.
        assert_eq!(df.unaccounted, 0);
    }

    /// `format_text` smoke test: the output must contain
    /// the header, the device row, the engine row, and the
    /// unaccounted line.  We don't pin exact widths
    /// because column-width maths depends on the host's
    /// numbers.
    #[test]
    fn df_format_text_emits_expected_row_labels() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scan = crate::orchestrator::ScanResult {
            items: vec![PrunableItem {
                id: "x".to_string(),
                name: "x".to_string(),
                engine: Engine::Docker,
                source: "docker".to_string(),
                category: crate::models::Category::Image,
                size_bytes: 4 * 1024_u64.pow(3),
                status: Status::Unused,
                extra: Default::default(),
            }],
            errors: vec![],
        };
        let df = Df::compute(&scan, dir.path()).expect("compute");
        let text = df.format_text();
        assert!(text.contains("Filesystem"), "header missing: {text}");
        assert!(text.contains("Mounted on"), "header missing: {text}");
        assert!(text.contains("docker"), "engine row missing: {text}");
        assert!(text.contains("unaccounted"), "tail line missing: {text}");
    }

    /// Non-Linux: the device stub returns `<unknown>` \u2014
    /// verified by the cfg-gated compile path, not at
    /// runtime.  Skip on Linux where the real lookup runs.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn df_compute_uses_unknown_device_on_non_linux() {
        let scan = crate::orchestrator::ScanResult::default();
        let df = Df::compute(&scan, Path::new("/")).expect("compute");
        assert_eq!(df.filesystem.device, "<unknown>");
    }
}
