//! Scanner for Python virtual environments.
//!
//! Detects venvs by looking for the `pyvenv.cfg` marker file that
//! CPython creates in every virtual environment.

use super::fs_scan::{delete_dir, find_dirs_with_marker, home};
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use async_trait::async_trait;
use std::collections::BTreeMap;

const SOURCE: &str = "python_venv";
const MARKER: &str = "pyvenv.cfg";

pub struct PythonVenvScanner;

impl Default for PythonVenvScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonVenvScanner {
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Scanner for PythonVenvScanner {
    fn source(&self) -> &'static str {
        SOURCE
    }
    fn engine(&self) -> Engine {
        Engine::PythonVenv
    }
    fn binary(&self) -> &'static str {
        "python3"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let root = home();
        let found = find_dirs_with_marker(&root, MARKER);
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
                    engine: Engine::PythonVenv,
                    source: SOURCE.to_string(),
                    category: Category::PythonVenv,
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
    fn detects_venv_by_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let venv = tmp.path().join(".venv");
        fs::create_dir_all(venv.join("bin")).unwrap();
        fs::write(venv.join(MARKER), "").unwrap();

        let root = tmp.path().to_path_buf();
        let found = find_dirs_with_marker(&root, MARKER);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, venv);
    }
}
