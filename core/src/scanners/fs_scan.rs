//! Shared filesystem scanning utilities for directory-based scanners.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};

/// Compute the total size of a directory in bytes by walking its contents.
pub fn dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Return the home directory, or `/` as a fallback.
pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Walk `root` looking for directories whose *file name* matches `dir_name`.
/// Returns `(path, size_bytes)` for each match, skipping symlinks and
/// directories that cannot be stat'd.
pub fn find_dirs_named(root: &Path, dir_name: &str) -> Vec<(PathBuf, u64)> {
    let mut results = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        if entry.file_name().to_string_lossy() == dir_name {
            let size = dir_size(entry.path());
            results.push((entry.path().to_path_buf(), size));
        }
    }
    results
}

/// Walk `root` looking for directories that contain a marker file.
/// Returns `(path, size_bytes)` for each match.
pub fn find_dirs_with_marker(root: &Path, marker_file: &str) -> Vec<(PathBuf, u64)> {
    let mut results = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        let candidate = entry.path().join(marker_file);
        if candidate.is_file() {
            let size = dir_size(entry.path());
            results.push((entry.path().to_path_buf(), size));
        }
    }
    results
}

/// Build a [`PrunableItem`] for a discovered directory.
pub fn make_item(
    path: PathBuf,
    size_bytes: u64,
    engine: Engine,
    source: &'static str,
    category: Category,
) -> PrunableItem {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mut extra = BTreeMap::new();
    extra.insert("path".into(), path.display().to_string());
    PrunableItem {
        id: path.display().to_string(),
        name,
        engine,
        source: source.to_string(),
        category,
        size_bytes,
        status: Status::Unused,
        extra,
    }
}

/// Delete a directory tree.
pub async fn delete_dir(path: &str) -> Result<(), EngineError> {
    let p = PathBuf::from(path);
    if !p.is_dir() {
        return Err(EngineError::new(
            format!("path is not a directory: {path}"),
            "fs_scan",
            vec![],
            None,
            String::new(),
        ));
    }
    tokio::task::spawn_blocking(move || {
        std::fs::remove_dir_all(&p).map_err(|e| {
            EngineError::new(
                format!("failed to delete {}: {e}", p.display()),
                "fs_scan",
                vec![],
                None,
                e.to_string(),
            )
        })
    })
    .await
    .map_err(|e| {
        EngineError::new(
            format!("task join error: {e}"),
            "fs_scan",
            vec![],
            None,
            e.to_string(),
        )
    })?
}
