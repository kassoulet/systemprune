//! Scanner for `.mypy_cache` directories.
//!
//! The mypy type checker stores cache files in `.mypy_cache` folders
//! that can accumulate over time.

use super::fs_scan::{delete_dir, find_dirs_named, home, HOME_EXCLUDE};
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use async_trait::async_trait;
use std::collections::BTreeMap;

const SOURCE: &str = "mypy";

pub struct MypyScanner;

impl Default for MypyScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl MypyScanner {
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Scanner for MypyScanner {
    fn source(&self) -> &'static str {
        SOURCE
    }
    fn engine(&self) -> Engine {
        Engine::Mypy
    }
    fn binary(&self) -> &'static str {
        "mypy"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let root = home();
        let found = find_dirs_named(&root, ".mypy_cache", HOME_EXCLUDE);
        let items = found
            .into_iter()
            .map(|(path, size)| {
                let parent = path.parent().unwrap_or(&path);
                let parent_name = parent
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let mut extra = BTreeMap::new();
                extra.insert("path".into(), path.display().to_string());
                PrunableItem {
                    id: path.display().to_string(),
                    name: parent_name,
                    engine: Engine::Mypy,
                    source: SOURCE.to_string(),
                    category: Category::DependencyCache,
                    size_bytes: size,
                    status: Status::Unused,
                    extra,
                }
            })
            .collect();
        Ok(items)
    }

    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        delete_dir(&item.id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_mypy_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join(".mypy_cache");
        fs::create_dir_all(&cache).unwrap();

        let root = tmp.path().to_path_buf();
        let found = find_dirs_named(&root, ".mypy_cache", &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, cache);
    }
}
