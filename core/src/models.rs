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
}

impl Engine {
    pub fn as_str(&self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
            Engine::Flatpak => "flatpak",
            Engine::Snap => "snap",
            Engine::Ollama => "ollama",
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
            Category::Other => "other",
        }
    }

    /// Human-friendly plural label, suitable for use as a section
    /// header (e.g. ``"Images"``, ``"Build caches"``).
    pub fn plural_label(&self) -> &'static str {
        match self {
            Category::Image => "Images",
            Category::Container => "Containers",
            Category::Volume => "Volumes",
            Category::Network => "Networks",
            Category::BuildCache => "Build caches",
            Category::App => "Apps",
            Category::Runtime => "Runtimes",
            Category::Model => "Models",
            Category::SnapRevision => "Snap revisions",
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
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Stopped => "stopped",
            Status::Dangling => "dangling",
            Status::Unused => "unused",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Status::Active)
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
        !self.status.is_active()
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
