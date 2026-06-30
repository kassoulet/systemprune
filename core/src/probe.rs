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
    if let Ok(p) = which::which(binary) {
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
