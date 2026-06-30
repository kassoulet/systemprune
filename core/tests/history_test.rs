//! Integration tests for `systemprune_core::history`.
//!
//! These exercise the public surface \u2014 `History` / `HistoryEntry`,
//! `command_for`, `rotation_path`, `rotate` \u2014 against
//! `tempfile::tempdir()` so the on-disk rotation and atomic-write
//! behaviour is exercised end to end.
//!
//! Replaces `core/src/history.rs::tests`.  Every assertion is
//! reachable from the public surface.  The local `make_item`
//! helper is re-declared here because integration tests live in
//! their own crate and cannot share private scope.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use systemprune_core::history::{
    command_for, rotate, rotation_path, History, HistoryEntry, HISTORY_VERSION,
};
use systemprune_core::models::{Category, Engine, PrunableItem, Status};

fn make_item(source: &str, category: Category, id: &str) -> PrunableItem {
    let engine = match source {
        "docker" => Engine::Docker,
        "podman" => Engine::Podman,
        "flatpak" => Engine::Flatpak,
        "snap" => Engine::Snap,
        "ollama" => Engine::Ollama,
        "node_modules" => Engine::NodeModules,
        "python_venv" => Engine::PythonVenv,
        "tox" => Engine::Tox,
        "mypy" => Engine::Mypy,
        "go_cache" => Engine::GoCache,
        "conda" => Engine::Conda,
        "cargo_cache" => Engine::CargoCache,
        _ => Engine::Docker,
    };
    PrunableItem {
        id: id.to_string(),
        name: id.to_string(),
        engine,
        source: source.to_string(),
        category,
        size_bytes: 1_000_000,
        status: Status::Unused,
        extra: BTreeMap::new(),
    }
}

#[test]
fn command_for_docker_image_uses_rmi_dash_f() {
    let item = make_item("docker", Category::Image, "sha256:abc");
    assert_eq!(command_for(&item), "docker rmi -f sha256:abc");
}

#[test]
fn command_for_docker_container_uses_rm_dash_f() {
    let item = make_item("docker", Category::Container, "abcd1234");
    assert_eq!(command_for(&item), "docker rm -f abcd1234");
}

#[test]
fn command_for_docker_volume_uses_volume_rm() {
    let item = make_item("docker", Category::Volume, "my-vol");
    assert_eq!(command_for(&item), "docker volume rm my-vol");
}

#[test]
fn command_for_podman_matches_docker_layout() {
    let item = make_item("podman", Category::Image, "sha256:abc");
    assert_eq!(command_for(&item), "podman rmi -f sha256:abc");
}

#[test]
fn command_for_flatpak_app_uses_uninstall_delete_data() {
    let item = make_item("flatpak", Category::App, "org.gimp.GIMP");
    assert_eq!(
        command_for(&item),
        "flatpak uninstall --delete-data -y org.gimp.GIMP"
    );
}

#[test]
fn command_for_snap_uses_remove() {
    let item = make_item("snap", Category::SnapRevision, "firefox");
    assert_eq!(command_for(&item), "snap remove firefox");
}

#[test]
fn command_for_ollama_uses_rm() {
    let item = make_item("ollama", Category::Model, "qwen2.5:7b");
    assert_eq!(command_for(&item), "ollama rm qwen2.5:7b");
}

#[test]
fn command_for_node_modules_uses_trash() {
    let item = make_item(
        "node_modules",
        Category::NodeModules,
        "/home/u/proj/node_modules",
    );
    assert_eq!(command_for(&item), "trash /home/u/proj/node_modules");
}

#[test]
fn command_for_go_cache_uses_clean_cache() {
    let item = make_item("go_cache", Category::BuildCache, "/home/u/.cache/go-build");
    assert_eq!(command_for(&item), "go clean -cache");
}

#[test]
fn command_for_conda_uses_env_remove() {
    let item = make_item(
        "conda",
        Category::PythonVenv,
        "/home/u/miniconda3/envs/myenv",
    );
    assert_eq!(
        command_for(&item),
        "conda env remove -p /home/u/miniconda3/envs/myenv -y"
    );
}

#[test]
fn command_for_cargo_cache_uses_trash() {
    let item = make_item(
        "cargo_cache",
        Category::BuildCache,
        "/home/u/.cargo/registry/cache",
    );
    assert_eq!(command_for(&item), "trash /home/u/.cargo/registry/cache");
}

#[test]
fn command_for_unknown_combo_is_empty() {
    let item = make_item("docker", Category::App, "weird-id");
    assert_eq!(command_for(&item), "");
}

#[test]
fn history_entry_from_success_uses_zero_exit_code() {
    let item = make_item("docker", Category::Image, "sha256:a");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    let entry = HistoryEntry::from_result(&item, true, None, now);
    assert_eq!(entry.exit_code, 0);
    assert_eq!(entry.command, "docker rmi -f sha256:a");
    assert_eq!(entry.source, "docker");
    assert_eq!(entry.category, "image");
    assert_eq!(entry.id, "sha256:a");
    assert_eq!(entry.timestamp, "1970-01-01T00:00:00Z");
    assert_eq!(entry.size_bytes, 1_000_000);
}

#[test]
fn history_entry_from_failure_prefers_engine_returncode() {
    let item = make_item("docker", Category::Image, "sha256:a");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    let entry = HistoryEntry::from_result(&item, false, Some(125), now);
    assert_eq!(entry.exit_code, 125);
}

#[test]
fn history_entry_from_failure_without_engine_returncode_uses_minus_one() {
    let item = make_item("docker", Category::Image, "sha256:a");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    let entry = HistoryEntry::from_result(&item, false, None, now);
    assert_eq!(entry.exit_code, -1);
}

#[test]
fn history_round_trip_preserves_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let mut h = History::new();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let item = make_item("docker", Category::Image, "sha256:abcdef");
    h.entries
        .push(HistoryEntry::from_result(&item, true, None, now));
    h.save(&path).unwrap();

    let loaded = History::load(&path).unwrap();
    assert_eq!(loaded, h);
    assert_eq!(loaded.version, HISTORY_VERSION);
}

#[test]
fn history_load_missing_file_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.json");
    let loaded = History::load(&path).unwrap();
    assert!(loaded.entries.is_empty());
    assert_eq!(loaded.version, HISTORY_VERSION);
}

#[test]
fn history_load_corrupt_file_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    std::fs::write(&path, "this is not JSON").unwrap();
    // A corrupted file must surface as an error so the
    // caller can recover explicitly (e.g. rename the bad
    // file away and start fresh) rather than silently
    // dropping the user's audit trail.
    assert!(History::load(&path).is_err());
}

#[test]
fn append_to_file_creates_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let item = make_item("docker", Category::Image, "sha256:abc");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    let entry = HistoryEntry::from_result(&item, true, None, now);
    History::append_to_file(&path, &entry, 1 << 20, 5).unwrap();
    let h = History::load(&path).unwrap();
    assert_eq!(h.entries.len(), 1);
    assert_eq!(h.entries[0].id, "sha256:abc");
}

#[test]
fn append_to_file_rotates_at_max_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    // 1 KB max is tight enough that pretty output always
    // exceeds it after a handful of 50-character id entries.
    let max_bytes = 1024;
    for i in 0..5 {
        let id = format!("{:050}", i);
        let mut item = make_item("docker", Category::Image, &id);
        item.name = id.clone();
        let entry = HistoryEntry::from_result(&item, true, None, now);
        History::append_to_file(&path, &entry, max_bytes, 5).unwrap();
    }
    assert!(
        path.with_extension("json.1").exists()
            || path.with_extension("json.2").exists()
            || path.with_extension("json.3").exists()
            || path.with_extension("json.4").exists(),
        "expected at least one rotation after max_bytes overshoot"
    );
}

#[test]
fn rotate_drops_excess_rotations() {
    // Real rotation happens between delete bursts.  We
    // simulate three bursts: each one writes a fresh
    // `history.json`, then triggers a `rotate` so the
    // previous burst slides into `.1` (then `.2` after the
    // third burst).  With `keep_files=3` the third burst
    // must never promote an entry into `.3` (only two
    // previous bursts ever get retained).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);

    // Burst 1: write, rotate.  The primary moves into `.1`.
    let mut h = History::new();
    h.entries.push(HistoryEntry::from_result(
        &make_item("docker", Category::Image, "sha256:abc"),
        true,
        None,
        now,
    ));
    h.save(&path).unwrap();
    rotate(&path, 3).unwrap();
    assert!(
        dir.path().join("history.json.1").exists(),
        "expected .1 to exist after first rotate"
    );
    assert!(!path.exists(), "primary gone after rotate");

    // Burst 2: write again, rotate.  .1 slides into .2 and
    // the new burst lands in .1.
    h.entries.push(HistoryEntry::from_result(
        &make_item("docker", Category::Image, "sha256:def"),
        true,
        None,
        now,
    ));
    h.save(&path).unwrap();
    rotate(&path, 3).unwrap();
    assert!(dir.path().join("history.json.1").exists());
    assert!(dir.path().join("history.json.2").exists());

    // Burst 3: write again, rotate.  With keep_files=3
    // we keep at most .1 and .2 (two prior bursts).  .3 is
    // the discard slot that must NOT receive a new entry.
    h.entries.push(HistoryEntry::from_result(
        &make_item("docker", Category::Image, "sha256:ghi"),
        true,
        None,
        now,
    ));
    h.save(&path).unwrap();
    rotate(&path, 3).unwrap();
    assert!(dir.path().join("history.json.1").exists());
    assert!(dir.path().join("history.json.2").exists());
    assert!(
        !dir.path().join("history.json.3").exists(),
        "keep_files=3 must never produce .3"
    );
    assert!(!path.exists());
}

#[test]
fn append_many_records_all_entries_in_order() {
    // Empty file + a batch of N: the batch must land in
    // order.  This exercises the happy path of the new
    // append_many primitive (single load, single save).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    let ids = ["sha256:a", "sha256:b", "sha256:c"];
    let entries: Vec<HistoryEntry> = ids
        .iter()
        .map(|id| {
            HistoryEntry::from_result(&make_item("docker", Category::Image, id), true, None, now)
        })
        .collect();
    History::append_many(&path, &entries, 1 << 20, 5).unwrap();
    let h = History::load(&path).unwrap();
    assert_eq!(h.entries.len(), 3);
    assert_eq!(h.entries[0].id, "sha256:a");
    assert_eq!(h.entries[1].id, "sha256:b");
    assert_eq!(h.entries[2].id, "sha256:c");
}

#[test]
fn append_many_preserves_existing_entries() {
    // Seed 2 entries via the per-entry append_to_file path
    // (so we touch the same on-disk format the rest of the
    // world uses), then push 2 more via the batch API.
    // Both halves must be present in order -- this is the
    // invariant the orchestrator's race-fix relies on
    // (the loaded entries are not clobbered by the new
    // batch).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
    for id in ["sha256:a", "sha256:b"] {
        let entry =
            HistoryEntry::from_result(&make_item("docker", Category::Image, id), true, None, now);
        History::append_to_file(&path, &entry, 1 << 20, 5).unwrap();
    }
    let new_entries: Vec<HistoryEntry> = ["sha256:c", "sha256:d"]
        .iter()
        .map(|id| {
            HistoryEntry::from_result(&make_item("docker", Category::Image, id), true, None, now)
        })
        .collect();
    History::append_many(&path, &new_entries, 1 << 20, 5).unwrap();
    let h = History::load(&path).unwrap();
    assert_eq!(h.entries.len(), 4);
    assert_eq!(h.entries[0].id, "sha256:a");
    assert_eq!(h.entries[1].id, "sha256:b");
    assert_eq!(h.entries[2].id, "sha256:c");
    assert_eq!(h.entries[3].id, "sha256:d");
}

#[test]
fn rotation_path_appends_numeric_suffix() {
    let base = Path::new("/tmp/foo/history.json");
    assert_eq!(
        rotation_path(base, 1),
        PathBuf::from("/tmp/foo/history.json.1")
    );
    assert_eq!(
        rotation_path(base, 2),
        PathBuf::from("/tmp/foo/history.json.2")
    );
}
