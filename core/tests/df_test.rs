//! Integration tests for the §4.2 disk-usage view (`core::df`).
//!
//! These exercise the public surface (`Df::compute` +
//! `format_text` + JSON shape) via the real `statvfs` against
//! `tempfile::tempdir()`, so the assertions run against actual
//! kernel numbers rather than hard-coded constants.  Inline
//! unit tests for the same surface live in `core/src/df.rs`.

use std::path::Path;
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
