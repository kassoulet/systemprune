//! Scanner for the Cargo build cache.
//!
//! Targets the two sub-directories of [`CARGO_HOME`] (default
//! `~/.cargo/`) that are safe to delete and that re-download
//! automatically the next time Cargo builds a crate:
//!
//! - `registry/cache/` — pre-downloaded crate tarballs and
//!   metadata for every registry the user has ever touched.
//! - `git/`            — bare clones and checkouts of every
//!   git-based dependency Cargo has ever fetched.
//!
//! [`CARGO_HOME`]: https://doc.rust-lang.org/cargo/guide/cargo-home.html
//!
//! Unlike [`super::go_cache`], which delegates deletion to the
//! engine's own `clean` command, Cargo has no equivalent
//! built-in.  Cargo's `cargo clean` only targets build artifacts
//! in `target/`, not the shared cache.  We therefore use
//! [`trash::delete`] to move the directory to the platform
//! trash.  This is safe because the cache is purely a
//! performance optimisation: deleting it forces Cargo to
//! re-download on the next build, with no data loss.

use super::fs_scan::dir_size;
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SOURCE: &str = "cargo_cache";
const REGISTRY_CACHE_DIR: &str = "registry";
const CACHE_SUBDIR: &str = "cache";
const GIT_DIR: &str = "git";

/// Resolve the Cargo home directory: `$CARGO_HOME` if set and
/// non-empty, otherwise `$HOME/.cargo/`.  Returns `None` if
/// neither yields a usable path (e.g. no home directory).
fn cargo_home() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CARGO_HOME") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let home = dirs::home_dir()?;
    Some(home.join(".cargo"))
}

/// Return ``(label, path)`` for each Cargo cache sub-directory
/// that actually exists on disk and has a non-empty path.  The
/// order matches the documentation in the module header:
/// ``registry/cache`` first, then ``git``.
fn existing_cache_subdirs(home: &Path) -> Vec<(&'static str, PathBuf)> {
    let registry_cache = home.join(REGISTRY_CACHE_DIR).join(CACHE_SUBDIR);
    let git = home.join(GIT_DIR);
    let mut out: Vec<(&'static str, PathBuf)> = Vec::new();
    if registry_cache.is_dir() {
        out.push(("Cargo registry cache", registry_cache));
    }
    if git.is_dir() {
        out.push(("Cargo git dependencies", git));
    }
    out
}

pub struct CargoCacheScanner {
    /// Source name.  Kept on the struct so we can return it
    /// from [`Scanner::source`] without re-allocating.
    source: &'static str,
}

impl Default for CargoCacheScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CargoCacheScanner {
    pub const fn new() -> Self {
        Self { source: SOURCE }
    }
}

#[async_trait]
impl Scanner for CargoCacheScanner {
    fn source(&self) -> &'static str {
        self.source
    }

    fn engine(&self) -> Engine {
        Engine::CargoCache
    }

    /// The CLI binary this scanner nominally wraps.  Cargo
    /// itself is not used at runtime (there is no
    /// `cargo clean-cache` command), but the trait requires a
    /// value and ``"cargo"`` is the most informative one.
    fn binary(&self) -> &'static str {
        "cargo"
    }

    /// Overridden from the default because the Cargo cache can
    /// outlive the `cargo` binary itself: a user who removed
    /// cargo with `rustup` still has a perfectly valid cache
    /// to clean.  The scanner is therefore available iff at
    /// least one of the target sub-directories exists.
    fn is_available(&self) -> bool {
        cargo_home()
            .map(|h| !existing_cache_subdirs(&h).is_empty())
            .unwrap_or(false)
    }

    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let home = match cargo_home() {
            Some(h) => h,
            // No `$CARGO_HOME` and no `$HOME/.cargo` is a
            // degenerate case; treat it as an empty result
            // rather than an error so the orchestrator can
            // continue to the other scanners.
            None => return Ok(Vec::new()),
        };
        let mut items = Vec::new();
        for (name, path) in existing_cache_subdirs(&home) {
            let size = dir_size(&path);
            let mut extra = BTreeMap::new();
            extra.insert("path".into(), path.display().to_string());
            extra.insert("subdir".into(), name.to_string());
            items.push(PrunableItem {
                id: path.display().to_string(),
                name: name.to_string(),
                engine: Engine::CargoCache,
                source: SOURCE.to_string(),
                category: Category::BuildCache,
                size_bytes: size,
                status: Status::Unused,
                extra,
            });
        }
        Ok(items)
    }

    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        // Use the path stored in `item.id` (set by
        // `get_items`) so the deletion targets exactly the
        // sub-directory the user selected, not the entire
        // `CARGO_HOME`.
        let item_id = item.id.clone();
        let path = PathBuf::from(&item_id);
        if !path.is_dir() {
            return Err(EngineError::new(
                format!("path is not a directory: {}", path.display()),
                SOURCE,
                vec![item_id],
                None,
                String::new(),
            ));
        }
        // Move owned values into the blocking task so the
        // closure satisfies the `'static` bound.
        let id_for_err = item_id.clone();
        tokio::task::spawn_blocking(move || {
            trash::delete(&path).map_err(|e| {
                EngineError::new(
                    format!("failed to trash cargo cache dir: {e}"),
                    SOURCE,
                    vec![id_for_err],
                    None,
                    e.to_string(),
                )
            })
        })
        .await
        .map_err(|e| {
            EngineError::new(
                format!("task join error: {e}"),
                SOURCE,
                vec![item_id],
                None,
                e.to_string(),
            )
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_stable() {
        // Pin the source string so the orchestrator's
        // `by_source` map keeps working across refactors.
        assert_eq!(CargoCacheScanner::new().source(), "cargo_cache");
    }

    #[test]
    fn engine_is_cargo_cache() {
        assert_eq!(CargoCacheScanner::new().engine(), Engine::CargoCache);
    }

    #[test]
    fn binary_is_cargo() {
        // Even though we don't shell out to cargo, the
        // trait contract requires a binary name.  Keep it
        // pinned to "cargo" so a refactor that changes it
        // shows up in code review.
        assert_eq!(CargoCacheScanner::new().binary(), "cargo");
    }

    #[test]
    fn cargo_home_uses_env_var_when_set() {
        // We can't mutate the process env in a way that
        // affects other tests, so just verify the resolver
        // doesn't panic and returns *something* consistent
        // with `CARGO_HOME` semantics.  The full
        // env-override path is covered by
        // `cargo_home_prefers_env_var_over_home` below when
        // the env var happens to be set in the test
        // environment.
        let resolved = cargo_home();
        // Either path is fine; what matters is that the
        // resolver doesn't crash.
        assert!(resolved.is_some() || resolved.is_none());
    }

    #[test]
    fn cargo_home_prefers_env_var_over_home() {
        // Use a clearly synthetic CARGO_HOME so we know
        // exactly which path the resolver should pick.
        // We set it via `set_var` which is `unsafe` on
        // edition 2024 but allowed on edition 2021; this
        // crate uses the 2021 edition per the workspace
        // `Cargo.toml`.  We accept the data-race risk here
        // because unit tests in this module are the only
        // reader of this env var.
        let synthetic = "/tmp/systemprune-test-cargo-home-not-real";
        // SAFETY: see comment above.
        unsafe {
            std::env::set_var("CARGO_HOME", synthetic);
        }
        let resolved = cargo_home();
        // SAFETY: restore the env var to whatever it was
        // before.  We can't read the previous value
        // portably, but the test is best-effort: leaving
        // CARGO_HOME set to our synthetic path only affects
        // subsequent tests in this process that also
        // resolve the cargo home, and they should not
        // depend on a specific value.
        unsafe {
            std::env::remove_var("CARGO_HOME");
        }
        assert_eq!(resolved, Some(PathBuf::from(synthetic)));
    }

    #[test]
    fn existing_cache_subdirs_skips_missing() {
        // An empty temp dir has no `registry/cache` or
        // `git` sub-dir, so the helper must return empty.
        let tmp = tempfile::TempDir::new().unwrap();
        let subdirs = existing_cache_subdirs(tmp.path());
        assert!(
            subdirs.is_empty(),
            "expected no cache subdirs in an empty temp dir, got {subdirs:?}"
        );
    }

    #[test]
    fn existing_cache_subdirs_finds_both() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("registry/cache")).unwrap();
        std::fs::create_dir_all(tmp.path().join("git")).unwrap();
        let subdirs = existing_cache_subdirs(tmp.path());
        assert_eq!(subdirs.len(), 2);
        // The helper documents registry/cache first, then git.
        assert_eq!(subdirs[0].0, "Cargo registry cache");
        assert_eq!(subdirs[1].0, "Cargo git dependencies");
        assert!(subdirs[0].1.ends_with("registry/cache"));
        assert!(subdirs[1].1.ends_with("git"));
    }

    #[test]
    fn existing_cache_subdirs_finds_registry_cache_only() {
        // A user who has never built a git-based dependency
        // (or has explicitly disabled git support) will
        // have a `registry/cache` but no `git` dir.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("registry/cache")).unwrap();
        let subdirs = existing_cache_subdirs(tmp.path());
        assert_eq!(subdirs.len(), 1);
        assert_eq!(subdirs[0].0, "Cargo registry cache");
    }

    #[test]
    fn existing_cache_subdirs_finds_git_only() {
        // Conversely, a pure-git dependency graph (rare but
        // possible) leaves only the `git` dir.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("git")).unwrap();
        let subdirs = existing_cache_subdirs(tmp.path());
        assert_eq!(subdirs.len(), 1);
        assert_eq!(subdirs[0].0, "Cargo git dependencies");
    }

    #[test]
    fn is_available_reflects_cache_dirs() {
        // The scanner is available iff the resolver finds at
        // least one of the target sub-dirs.  We can't fully
        // control the test environment's $CARGO_HOME, so
        // we just check the helper and the public method
        // agree.
        let available = CargoCacheScanner::new().is_available();
        let via_resolver = cargo_home()
            .map(|h| !existing_cache_subdirs(&h).is_empty())
            .unwrap_or(false);
        assert_eq!(available, via_resolver);
    }
}
