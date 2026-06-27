//! Unified data model for items exposed by every engine.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The supported engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Docker,
    Podman,
    Flatpak,
    Snap,
    Ollama,
    NodeModules,
    PythonVenv,
    Tox,
    Mypy,
    GoCache,
    Conda,
    CargoCache,
}

impl Engine {
    pub fn as_str(&self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
            Engine::Flatpak => "flatpak",
            Engine::Snap => "snap",
            Engine::Ollama => "ollama",
            Engine::NodeModules => "node_modules",
            Engine::PythonVenv => "python_venv",
            Engine::Tox => "tox",
            Engine::Mypy => "mypy",
            Engine::GoCache => "go_cache",
            Engine::Conda => "conda",
            Engine::CargoCache => "cargo_cache",
        }
    }

    /// Human-friendly label, e.g. for the TUI/GUI.
    pub fn label(&self) -> &'static str {
        match self {
            Engine::Docker => "Docker",
            Engine::Podman => "Podman",
            Engine::Flatpak => "Flatpak",
            Engine::Snap => "Snap",
            Engine::Ollama => "Ollama",
            Engine::NodeModules => "Node Modules",
            Engine::PythonVenv => "Python venv",
            Engine::Tox => "Tox",
            Engine::Mypy => "Mypy",
            Engine::GoCache => "Go Cache",
            Engine::Conda => "Conda",
            Engine::CargoCache => "Cargo Cache",
        }
    }
}

/// The kind of asset a [`PrunableItem`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Image,
    Container,
    Volume,
    Network,
    BuildCache,
    App,
    Runtime,
    Model,
    SnapRevision,
    NodeModules,
    PythonVenv,
    DependencyCache,
    Other,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Image => "image",
            Category::Container => "container",
            Category::Volume => "volume",
            Category::Network => "network",
            Category::BuildCache => "build_cache",
            Category::App => "app",
            Category::Runtime => "runtime",
            Category::Model => "model",
            Category::SnapRevision => "snap_revision",
            Category::NodeModules => "node_modules",
            Category::PythonVenv => "python_venv",
            Category::DependencyCache => "dependency_cache",
            Category::Other => "other",
        }
    }

    /// Human-friendly plural label, suitable for use as a section
    /// header (e.g. ``"Images"``, ``"Build caches"``).
    pub fn plural_label(&self) -> &'static str {
        match self {
            Category::Image => "Docker Images",
            Category::Container => "Docker Containers",
            Category::Volume => "Docker Volumes",
            Category::Network => "Docker Networks",
            Category::BuildCache => "Build caches",
            Category::App => "Flatpak Apps",
            Category::Runtime => "Flatpak Runtimes",
            Category::Model => "Ollama Models",
            Category::SnapRevision => "Snap revisions",
            Category::NodeModules => "Node Modules",
            Category::PythonVenv => "Python venvs",
            Category::DependencyCache => "Dependency caches",
            Category::Other => "Other",
        }
    }
}

/// The runtime status of an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Stopped,
    Dangling,
    #[default]
    Unused,
    Deleted,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Stopped => "stopped",
            Status::Dangling => "dangling",
            Status::Unused => "unused",
            Status::Deleted => "deleted",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Status::Active)
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self, Status::Deleted)
    }
}

/// A single disk-occupying asset exposed by an engine.
///
/// All fields are public for ergonomics; consumers should treat the
/// value as read-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrunableItem {
    /// Stable native identifier (image hash, flatpak ref, model name, ...).
    pub id: String,

    /// Human-readable display name.
    pub name: String,

    /// Native engine this item belongs to.
    pub engine: Engine,

    /// The scanner name (e.g. ``"docker"``). For most engines this
    /// equals :attr:`Engine`, but engines with multiple sub-scanners
    /// can disambiguate (e.g. ``"docker.image"``).
    pub source: String,

    /// The kind of asset.
    pub category: Category,

    /// Normalized byte size.
    pub size_bytes: u64,

    /// Runtime status. Items with [`Status::Active`] must never be deleted.
    #[serde(default)]
    pub status: Status,

    /// Free-form metadata (e.g. "repository" / "tag" for Docker images).
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl PrunableItem {
    /// Whether this item is safe to delete. Currently a simple
    /// predicate on [`Status`]; kept as a method so future logic
    /// (e.g. dependency checks) lives in one place.
    pub fn is_safe_to_delete(&self) -> bool {
        !self.status.is_active() && !self.status.is_deleted()
    }

    /// Whether this item is *currently* safe to delete, taking the
    /// UI's record of recent deletion failures into account.
    ///
    /// An item with `Status::Unused` (i.e. `is_safe_to_delete() == true`)
    /// is *not* re-deletable if a previous attempt on it has already
    /// failed and the failure is still recorded in `delete_errors`.
    /// This keeps the TUI/GUI from re-queueing items the engine has
    /// already rejected, which would either repeat the failure
    /// (waste of time) or, in the worst case, cause data loss if the
    /// user didn't realise the item was already on a deny-list.
    pub fn is_deletable_for_real(
        &self,
        delete_errors: &std::collections::BTreeMap<(String, String), String>,
    ) -> bool {
        self.is_safe_to_delete()
            && !delete_errors.contains_key(&(self.source.clone(), self.id.clone()))
    }

    /// Return a JSON-serialisable view of this item. Mirrors the
    /// Python `PrunableItem.as_dict()` so CLI ``--json`` output
    /// matches across stacks.
    pub fn as_dict(&self) -> serde_json::Value {
        use serde_json::json;
        json!({
            "id": self.id,
            "name": self.name,
            "engine": self.engine.as_str(),
            "source": self.source,
            "category": self.category.as_str(),
            "size_bytes": self.size_bytes,
            "status": self.status.as_str(),
            "is_safe_to_delete": self.is_safe_to_delete(),
            "extra": self.extra,
        })
    }
}
