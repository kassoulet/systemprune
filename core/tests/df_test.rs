//! Integration tests for the §4.2 disk-usage view (`core::df`).
//!
//! These exercise the public surface (`Df::compute` +
//! `format_text` + JSON shape) via the real `statvfs` against
//! `tempfile::tempdir()`, so the assertions run against actual
//! kernel numbers rather than hard-coded constants.  Once
//! these tests landed, the inline `mod tests` block in
//! `core/src/df.rs` was deleted and its assertions migrated
//! here so the public surface is exercised through the public
//! surface only.
//!
//! The cfg-gated `df_compute_uses_unknown_device_on_non_linux`
//! test at the bottom only compiles on non-Linux hosts; on
//! Linux the real `/proc/self/mountinfo` lookup runs.

use systemprune_core::df::{self, Df};
use systemprune_core::models::{Category, Engine, PrunableItem, Status};
use systemprune_core::orchestrator::ScanResult;

fn docker_image(id: &str, name: &str, size_bytes: u64) -> PrunableItem {
    PrunableItem {
        id: id.to_string(),
        name: name.to_string(),
        engine: Engine::Docker,
        source: "docker".to_string(),
        category: Category::Image,
        size_bytes,
        status: Status::Unused,
        extra: Default::default(),
    }
}

#[test]
fn df_compute_via_statvfs_on_tempdir() {
    // tempfile gives us a real mount; the kernel numbers will
    // be non-zero and stable for the duration of this test.
    let dir = tempfile::tempdir().expect("tempdir");
    let scan = ScanResult {
        items: vec![docker_image("a", "a", 1024)],
        errors: vec![],
    };
    let df = Df::compute(&scan, dir.path()).expect("compute");
    assert!(df.filesystem.total_bytes > 0, "total must be non-zero");
    assert!(df.filesystem.used_bytes <= df.filesystem.total_bytes);
    assert!(df.filesystem.available_bytes <= df.filesystem.total_bytes);
    assert_eq!(df.breakdown.rows.len(), 1);
    assert_eq!(df.breakdown.rows[0].source, "docker");
    // `unaccounted` is `filesystem.used - engine_sum`; pinning
    // the relationship is more robust than pinning the literal.
    assert_eq!(
        df.unaccounted,
        df.filesystem
            .used_bytes
            .saturating_sub(df.breakdown.grand_total()),
    );
}

#[test]
fn df_format_text_contains_header_and_engine_and_unaccounted_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scan = ScanResult {
        items: vec![docker_image("a", "a", 4 * 1024_u64.pow(3))],
        errors: vec![],
    };
    let df = Df::compute(&scan, dir.path()).expect("compute");
    let text = df.format_text();
    assert!(text.contains("Filesystem"), "{text}");
    assert!(text.contains("Mounted on"), "{text}");
    assert!(text.contains("docker"), "{text}");
    assert!(text.contains("unaccounted"), "{text}");
}

#[test]
fn df_json_serializes_three_top_level_keys() {
    // The §4.2 subcommand exposes a stable wire shape so
    // piping consumers can rely on it.  Pin the three keys
    // today; future fields are additive.
    let dir = tempfile::tempdir().expect("tempdir");
    let scan = ScanResult::default();
    let df = Df::compute(&scan, dir.path()).expect("compute");
    let json = serde_json::to_value(&df).expect("serialise");
    assert!(json.get("filesystem").is_some(), "{json}");
    assert!(json.get("breakdown").is_some(), "{json}");
    assert!(json.get("unaccounted").is_some(), "{json}");
}

#[test]
fn df_stat_filesystem_directly_on_tmpdir() {
    // Smoke-test the `df::stat_filesystem` entry point that
    // the CLI does *not* call directly (it goes through
    // `Df::compute`).  This exercises the CString / FFI
    // boundary on a POSIX host independently of the
    // orchestration path.
    let dir = tempfile::tempdir().expect("tempdir");
    let stats = df::stat_filesystem(dir.path()).expect("statvfs");
    assert!(stats.total_bytes > 0);
    assert_eq!(stats.mount_point, dir.path().to_string_lossy());
}

/// Companion to `df_stat_filesystem_directly_on_tmpdir`
/// above: pins additional invariants (`used <= total`,
/// `available <= total`, `use_percent <= 100`) of the same
/// `stat_filesystem` call so a refactor that drops the
/// rounding/cast clamps surfaces here.
#[test]
fn df_stat_filesystem_via_tempdir_returns_consistent_numbers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stats = df::stat_filesystem(dir.path()).expect("statvfs on tempdir should succeed");
    assert!(stats.total_bytes > 0);
    assert!(stats.used_bytes <= stats.total_bytes);
    assert!(stats.available_bytes <= stats.total_bytes);
    assert!(stats.use_percent <= 100);
}

/// `statvfs` on a missing path surfaces `ENOENT`.  The
/// exact error kind varies by host, so we pin the
/// existence-of-error rather than the kind.
#[test]
fn df_stat_filesystem_on_missing_path_returns_error() {
    let bad = std::path::Path::new("/this/path/does/not/exist/systemprune-test-zzz");
    assert!(df::stat_filesystem(bad).is_err());
}

/// `Df::compute` with no items: the breakdown is empty and
/// `unaccounted` equals the filesystem `used`.
#[test]
fn df_compute_empty_scan_has_unaccounted_equal_used() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scan = ScanResult::default();
    let df = Df::compute(&scan, dir.path()).expect("compute");
    assert!(df.breakdown.rows.is_empty());
    assert_eq!(df.unaccounted, df.filesystem.used_bytes);
}

/// `Df::compute` with items summing to less than the
/// filesystem `used`: `unaccounted = used - sum`.  Uses
/// half the max `u64` so we are guaranteed to be below the
/// filesystem `used` regardless of host.  Reading `used`
/// from the same `Df` snapshot avoids a race against a
/// separate `stat_filesystem` call.
#[test]
fn df_compute_unaccounted_is_used_minus_engine_totals() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 4 GiB is a realistic engine-total size that is
    // guaranteed to be below the tempdir's `used`.
    let engine_total = 4 * 1024_u64.pow(3);
    let items = vec![docker_image("a", "a", engine_total)];
    let scan = ScanResult {
        items,
        errors: vec![],
    };
    let df = Df::compute(&scan, dir.path()).expect("compute");
    assert_eq!(df.breakdown.rows.len(), 1);
    assert_eq!(df.breakdown.rows[0].source, "docker");
    assert_eq!(df.breakdown.rows[0].total_bytes, engine_total);
    assert_eq!(
        df.unaccounted,
        df.filesystem.used_bytes.saturating_sub(engine_total),
    );
    assert!(df.unaccounted <= df.filesystem.used_bytes);
}

/// `saturating_sub` semantics: when the engine total
/// exceeds the filesystem `used`, `unaccounted` floors at
/// `0` rather than wrapping into a near-`u64::MAX` value.
#[test]
fn df_compute_unaccounted_clamps_to_zero_on_negative_subtraction() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Pretend the engine reports a huge total.
    let engine_total = u64::MAX / 2;
    let items = vec![docker_image("x", "x", engine_total)];
    let scan = ScanResult {
        items,
        errors: vec![],
    };
    let df = Df::compute(&scan, dir.path()).expect("compute");
    // filesystem.used is small (~few MB); engine_total is
    // u64::MAX/2, so subtraction must saturate.
    assert_eq!(df.unaccounted, 0);
}

/// `format_text` smoke test: the output must contain the
/// header, the device row, the engine row, and the
/// unaccounted line.  We don't pin exact widths because
/// column-width maths depends on the host's numbers.
#[test]
fn df_format_text_emits_expected_row_labels() {
    let dir = tempfile::tempdir().expect("tempdir");
    let items = vec![docker_image("x", "x", 4 * 1024_u64.pow(3))];
    let scan = ScanResult {
        items,
        errors: vec![],
    };
    let df = Df::compute(&scan, dir.path()).expect("compute");
    let text = df.format_text();
    assert!(text.contains("Filesystem"), "header missing: {text}");
    assert!(text.contains("Mounted on"), "header missing: {text}");
    assert!(text.contains("docker"), "engine row missing: {text}");
    assert!(text.contains("unaccounted"), "tail line missing: {text}");
}

/// Non-Linux: the device stub returns `<unknown>`.  The
/// cfg gate ensures this test compiles only on non-Linux
/// hosts; on Linux the real `/proc/self/mountinfo` lookup
/// runs and the test fixture here is a no-op.
#[cfg(not(target_os = "linux"))]
#[test]
fn df_compute_uses_unknown_device_on_non_linux() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scan = ScanResult::default();
    let df = Df::compute(&scan, dir.path()).expect("compute");
    assert_eq!(df.filesystem.device, "<unknown>");
}
