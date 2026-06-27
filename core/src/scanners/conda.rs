//! Scanner for conda environments.
//!
//! Uses `conda env list` to enumerate all conda environments
//! and `conda env remove -p <path> -y` to clean them.  The
//! `base` env (the conda installation itself) is intentionally
//! skipped because it is not a user-created environment and
//! deleting it would break the conda installation.

use super::base::BaseScanner;
use super::fs_scan::dir_size;
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::PathBuf;

const SOURCE: &str = "conda";
// 30s for `conda env list` matches the Go cache scanner.
// `conda env list` itself is fast, but a cold-start conda
// installation can take a few seconds to import plugins.
const LIST_TIMEOUT_SECS: u64 = 30;
// 60s for `conda env remove` because removing a large env
// (e.g. one with PyTorch or TensorFlow) can take a while.
const REMOVE_TIMEOUT_SECS: u64 = 60;

pub struct CondaScanner {
    base: BaseScanner,
}

impl Default for CondaScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CondaScanner {
    pub const fn new() -> Self {
        Self {
            base: BaseScanner::new(SOURCE, Engine::Conda, "conda"),
        }
    }

    /// Parse the stdout of `conda env list`.  Each non-comment,
    /// non-blank line has the form ``name   /path/to/env`` (two
    /// whitespace-separated fields; the name has no spaces).
    /// Returns ``(name, path)`` pairs in the order conda emitted
    /// them.  The `base` env is included in the output so the
    /// caller can decide whether to skip it (we do, see
    /// `get_items`).
    fn parse_env_list(output: &str) -> Vec<(String, PathBuf)> {
        let mut envs = Vec::new();
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[0].to_string();
                let path = PathBuf::from(parts[1]);
                envs.push((name, path));
            }
        }
        envs
    }
}

#[async_trait]
impl Scanner for CondaScanner {
    fn source(&self) -> &'static str {
        SOURCE
    }
    fn engine(&self) -> Engine {
        Engine::Conda
    }
    fn binary(&self) -> &'static str {
        "conda"
    }

    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let (stdout, _stderr) = self
            .base
            .run(&["conda", "env", "list"], LIST_TIMEOUT_SECS)
            .await?;
        let envs = Self::parse_env_list(&stdout);
        let mut items = Vec::new();
        for (name, path) in envs {
            // The `base` env is the conda installation itself.
            // Deleting it would break the user's conda install
            // (and is not what they asked for anyway).  Skip it
            // entirely so it never appears as a prunable item.
            if name == "base" {
                continue;
            }
            if !path.is_dir() {
                // `conda env list` may list stale entries that
                // no longer exist on disk.  Skip them silently.
                continue;
            }
            let size = dir_size(&path);
            let mut extra = BTreeMap::new();
            extra.insert("path".into(), path.display().to_string());
            extra.insert("env_name".into(), name.clone());
            items.push(PrunableItem {
                id: path.display().to_string(),
                name,
                engine: Engine::Conda,
                source: SOURCE.to_string(),
                category: Category::PythonVenv,
                size_bytes: size,
                status: Status::Unused,
                extra,
            });
        }
        Ok(items)
    }

    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        // `conda env remove -p <path> -y` removes the env at the
        // given path.  This is preferred over `trash::delete`ing
        // the directory because conda knows the env's internal
        // layout (e.g. `conda-meta/history` files) and can
        // perform a clean uninstall, whereas `trash::delete`
        // would leave stale conda metadata behind.
        self.base
            .run(
                &["conda", "env", "remove", "-p", &item.id, "-y"],
                REMOVE_TIMEOUT_SECS,
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn source_is_stable() {
        assert_eq!(CondaScanner::new().source(), "conda");
    }

    #[test]
    fn engine_is_conda() {
        assert_eq!(CondaScanner::new().engine(), Engine::Conda);
    }

    #[test]
    fn binary_is_conda() {
        assert_eq!(CondaScanner::new().binary(), "conda");
    }

    #[test]
    fn is_available_tracks_conda_on_path() {
        let available = CondaScanner::new().is_available();
        let conda_on_path = crate::probe::which("conda").is_some();
        assert_eq!(
            available, conda_on_path,
            "is_available should match `which conda`"
        );
    }

    #[test]
    fn parse_env_list_handles_typical_output() {
        // The first line is a comment, the rest are `name  path`.
        let output = "\
# conda environments:
#
base                  /home/user/miniconda3
myenv                 /home/user/miniconda3/envs/myenv
otherenv              /home/user/miniconda3/envs/otherenv
";
        let envs = CondaScanner::parse_env_list(output);
        assert_eq!(
            envs,
            vec![
                ("base".to_string(), PathBuf::from("/home/user/miniconda3")),
                (
                    "myenv".to_string(),
                    PathBuf::from("/home/user/miniconda3/envs/myenv")
                ),
                (
                    "otherenv".to_string(),
                    PathBuf::from("/home/user/miniconda3/envs/otherenv")
                ),
            ]
        );
    }

    #[test]
    fn parse_env_list_skips_blank_and_comment_lines() {
        let output = "\n# header\n\nbase /a\n\nmyenv /b\n";
        let envs = CondaScanner::parse_env_list(output);
        assert_eq!(
            envs,
            vec![
                ("base".to_string(), PathBuf::from("/a")),
                ("myenv".to_string(), PathBuf::from("/b")),
            ]
        );
    }

    #[test]
    fn parse_env_list_skips_malformed_lines() {
        // A line with only one field (no path) is malformed and
        // is skipped.  Well-formed lines around it still parse.
        let output = "base /a\nmalformed\nmyenv /b\n";
        let envs = CondaScanner::parse_env_list(output);
        assert_eq!(
            envs,
            vec![
                ("base".to_string(), PathBuf::from("/a")),
                ("myenv".to_string(), PathBuf::from("/b")),
            ]
        );
    }
}
