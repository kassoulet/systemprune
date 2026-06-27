//! Per-engine scanner implementations.

pub mod base;
pub mod docker;
pub mod flatpak;
pub mod fs_scan;
pub mod mypy;
pub mod node_modules;
pub mod ollama;
pub mod podman;
pub mod python_venv;
pub mod snap;
pub mod tox;

use crate::errors::EngineError;
use crate::models::{Engine, PrunableItem};
use async_trait::async_trait;
use std::sync::Arc;

pub use base::BaseScanner;

/// The interface every engine wrapper implements.
#[async_trait]
pub trait Scanner: Send + Sync {
    /// Stable source name used by the orchestrator (e.g. ``"docker"``).
    fn source(&self) -> &'static str;

    /// Native engine this scanner wraps.
    fn engine(&self) -> Engine;

    /// Name of the CLI binary on ``$PATH`` (used by
    /// [`BaseScanner::is_available`]).
    fn binary(&self) -> &'static str;

    /// Return ``true`` if this scanner's CLI is on ``$PATH``.
    fn is_available(&self) -> bool {
        crate::probe::which(self.binary()).is_some()
    }

    /// Return the list of prunable items reported by this engine.
    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError>;

    /// Delete the given item via the engine's native CLI. Implementations
    /// must raise [`EngineError`] on failure.
    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError>;
}

/// The canonical set of built-in scanners. The orchestrator uses this
/// by default.
pub fn all_scanners() -> Vec<Arc<dyn Scanner>> {
    vec![
        Arc::new(docker::DockerScanner::new()),
        Arc::new(podman::PodmanScanner::new()),
        Arc::new(flatpak::FlatpakScanner::new()),
        Arc::new(snap::SnapScanner::new()),
        Arc::new(ollama::OllamaScanner::new()),
        Arc::new(node_modules::NodeModulesScanner::new()),
        Arc::new(python_venv::PythonVenvScanner::new()),
        Arc::new(tox::ToxScanner::new()),
        Arc::new(mypy::MypyScanner::new()),
    ]
}
