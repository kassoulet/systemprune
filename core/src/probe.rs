//! PATH probing for native engine binaries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, os::unix::fs::PermissionsExt};

/// Engines and the CLI binary we look up on ``$PATH`` for each.
pub fn engine_binaries() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("docker", &["docker"]),
        ("podman", &["podman"]),
        ("flatpak", &["flatpak"]),
        ("snap", &["snap"]),
        ("ollama", &["ollama"]),
    ]
}

/// Return the absolute path of *binary* on ``$PATH`` or ``None``.
pub fn which(binary: &str) -> Option<PathBuf> {
    if binary.is_empty() {
        return None;
    }
    // Consult ``which`` crate first to honour ``PATHEXT`` etc. on
    // non-Unix systems. The function still consults ``$PATH`` directly
    // when needed.
    if let Some(p) = which::which(binary).ok() {
        return Some(p);
    }
    manual_search(binary, &current_path())
}

/// Probe every engine and return ``engine_name -> absolute_binary_path``.
///
/// *path* is the search path; pass ``None`` to use the current ``$PATH``.
pub fn probe_engines(path: Option<&str>) -> BTreeMap<String, PathBuf> {
    let table = engine_binaries();
    let path = path.map(String::from).unwrap_or_else(current_path);
    let mut found = BTreeMap::new();
    for (name, candidates) in table {
        for cand in *candidates {
            if let Some(p) = search_one(&path, cand) {
                found.insert((*name).to_string(), p);
                break;
            }
        }
    }
    found
}

fn current_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

fn search_one(path: &str, binary: &str) -> Option<PathBuf> {
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let p = Path::new(dir).join(binary);
        if is_executable(&p) {
            return Some(p);
        }
    }
    None
}

fn manual_search(binary: &str, path: &str) -> Option<PathBuf> {
    search_one(path, binary)
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::OpenOptionsExt;

    fn make_fake_binary(dir: &std::path::Path, name: &str) {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .mode(0o755)
            .open(dir.join(name))
            .unwrap();
    }

    #[test]
    fn which_finds_existing_binary() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_fake_binary(tmp.path(), "fakebin");
        let path = tmp.path().to_string_lossy().to_string();
        let found = search_one(&path, "fakebin");
        assert!(found.is_some());
    }

    #[test]
    fn which_returns_none_for_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        assert!(search_one(&path, "definitely-not-installed-xyz").is_none());
    }

    #[test]
    fn probe_engines_uses_custom_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_fake_binary(tmp.path(), "docker");
        let path = tmp.path().to_string_lossy().to_string();
        let found = probe_engines(Some(&path));
        assert!(found.contains_key("docker"));
    }

    #[test]
    fn empty_binary_returns_none() {
        assert!(which("").is_none());
    }
}
