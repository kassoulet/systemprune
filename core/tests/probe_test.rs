//! Tests for the PATH probe.

use std::os::unix::fs::OpenOptionsExt;
use systemprune_core::probe::{probe_engines, which};
use tempfile::TempDir;

fn make_fake_binary(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let bin = dir.join(name);
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o755)
        .open(&bin)
        .unwrap();
    bin
}

#[test]
fn which_finds_existing_binary() {
    let tmp = TempDir::new().unwrap();
    make_fake_binary(tmp.path(), "fakebin");
    let path = tmp.path().to_string_lossy().to_string();
    std::env::set_var("PATH", &path);
    assert!(which("fakebin").is_some());
}

#[test]
fn which_returns_none_for_missing() {
    std::env::set_var("PATH", "/tmp");
    assert!(which("definitely-not-installed-xyz-987654").is_none());
}

#[test]
fn empty_binary_returns_none() {
    assert!(which("").is_none());
}

#[test]
fn probe_engines_uses_custom_path() {
    let tmp = TempDir::new().unwrap();
    make_fake_binary(tmp.path(), "docker");
    let path = tmp.path().to_string_lossy().to_string();
    let found = probe_engines(Some(&path));
    assert!(found.contains_key("docker"));
}
