//! Scanner for `.tox` directories.
//!
//! `.tox` folders are created by the `tox` test automation tool and
//! can grow large over time.

use super::fs_scan::{delete_dir, find_dirs_named, home};
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use async_trait::async_trait;
use std::collections::BTreeMap;

const SOURCE: &str = "tox";

pub struct ToxScanner;

impl Default for ToxScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl ToxScanner {
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Scanner for ToxScanner {
    fn source(&self) -> &'static str {
        SOURCE
    }
    fn engine(&self) -> Engine {
        Engine::Tox
    }
    fn binary(&self) -> &'static str {
        "tox"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let root = home();
        let found = find_dirs_named(&root, ".tox");
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
                    engine: Engine::Tox,
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
    fn detects_tox_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let tox = tmp.path().join(".tox");
        fs::create_dir_all(&tox).unwrap();

        let root = tmp.path().to_path_buf();
        let found = find_dirs_named(&root, ".tox");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, tox);
    }
}
