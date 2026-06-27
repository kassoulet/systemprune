//! Scanner for the Go build cache.
//!
//! Uses `go env GOCACHE` to find the cache directory and
//! `go clean -cache` to clean it.  Unlike [`super::node_modules`]
//! which walks the home directory for many folders, there is
//! only one Go build cache per user, so this scanner returns
//! at most one [`PrunableItem`].

use super::base::BaseScanner;
use super::fs_scan::dir_size;
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::PathBuf;

const SOURCE: &str = "go_cache";
// 30s matches `CLEAN_TIMEOUT_SECS`.  `go env` itself is fast,
// but the first invocation in a CI environment that needs to
// download the Go toolchain can take longer than 10s.
const GOCACHE_TIMEOUT_SECS: u64 = 30;
const CLEAN_TIMEOUT_SECS: u64 = 30;

pub struct GoCacheScanner {
    base: BaseScanner,
}

impl Default for GoCacheScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl GoCacheScanner {
    pub const fn new() -> Self {
        Self {
            base: BaseScanner::new(SOURCE, Engine::GoCache, "go"),
        }
    }
}

#[async_trait]
impl Scanner for GoCacheScanner {
    fn source(&self) -> &'static str {
        SOURCE
    }
    fn engine(&self) -> Engine {
        Engine::GoCache
    }
    fn binary(&self) -> &'static str {
        "go"
    }

    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        // `go env GOCACHE` prints the absolute path of the build
        // cache to stdout.  Trim the trailing newline and skip the
        // item entirely if `go` returns an empty string (which
        // shouldn't happen in practice but is a safe fallback).
        let (stdout, _stderr) = self
            .base
            .run(&["go", "env", "GOCACHE"], GOCACHE_TIMEOUT_SECS)
            .await?;
        let cache_path = stdout.trim();
        if cache_path.is_empty() {
            return Ok(Vec::new());
        }
        let path = PathBuf::from(cache_path);
        // If the cache directory does not exist yet (fresh `go`
        // install, or the user never built anything), there is
        // nothing to report.
        if !path.is_dir() {
            return Ok(Vec::new());
        }
        let size = dir_size(&path);
        let mut extra = BTreeMap::new();
        extra.insert("path".into(), path.display().to_string());
        Ok(vec![PrunableItem {
            id: path.display().to_string(),
            name: "Go build cache".to_string(),
            engine: Engine::GoCache,
            source: SOURCE.to_string(),
            category: Category::BuildCache,
            size_bytes: size,
            status: Status::Unused,
            extra,
        }])
    }

    async fn delete_item(&self, _item: &PrunableItem) -> Result<(), EngineError> {
        // `go clean -cache` removes the entire build cache.  This
        // is safer than `trash::delete`ing the directory directly
        // because `go` is the only process that knows the cache's
        // internal layout, and concurrent `go` invocations may
        // hold open file handles inside it.
        //
        // `_item` is intentionally ignored: there is only one Go
        // build cache per user (the one reported by `go env
        // GOCACHE`), so `go clean -cache` always targets the
        // correct directory regardless of which `PrunableItem`
        // was selected for deletion.  If a future caller ever
        // passes a per-item path (e.g. for a different user's
        // cache), this implementation would need to switch to
        // `trash::delete(&_item.id)` to honour the selection.
        self.base
            .run(&["go", "clean", "-cache"], CLEAN_TIMEOUT_SECS)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_stable() {
        // Pin the source string so the orchestrator's `by_source`
        // map keeps working across refactors.
        assert_eq!(GoCacheScanner::new().source(), "go_cache");
    }

    #[test]
    fn engine_is_go_cache() {
        assert_eq!(GoCacheScanner::new().engine(), Engine::GoCache);
    }

    #[test]
    fn binary_is_go() {
        assert_eq!(GoCacheScanner::new().binary(), "go");
    }

    #[test]
    fn is_available_tracks_go_on_path() {
        // The default `is_available` implementation uses
        // `which(self.binary())`, so the scanner is available
        // iff `go` is on `$PATH`.  We don't assert a specific
        // value because CI environments may or may not have Go.
        let available = GoCacheScanner::new().is_available();
        let go_on_path = crate::probe::which("go").is_some();
        assert_eq!(
            available, go_on_path,
            "is_available should match `which go`"
        );
    }
}
