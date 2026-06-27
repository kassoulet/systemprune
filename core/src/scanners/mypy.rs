//! Scanner for `.mypy_cache` directories.
//!
//! The mypy type checker stores cache files in `.mypy_cache` folders
//! that can accumulate over time.

use super::fs_scan::{delete_dir, find_dirs_named, home, make_item};
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem};
use async_trait::async_trait;

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
        let found = find_dirs_named(&root, ".mypy_cache");
        let items = found
            .into_iter()
            .map(|(path, size)| {
                make_item(path, size, Engine::Mypy, SOURCE, Category::DependencyCache)
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
        let found = find_dirs_named(&root, ".mypy_cache");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, cache);
    }
}
