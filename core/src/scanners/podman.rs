//! Scanner for Podman (mirrors Docker's CLI surface).

use super::base::BaseScanner;
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use crate::size::parse_size;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::BTreeMap;
use tracing::warn;

const TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Deserialize)]
struct PodmanImage {
    #[serde(alias = "Id", alias = "ID")]
    id: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default, alias = "Repository")]
    repository: String,
    #[serde(default, alias = "Tag")]
    tag: String,
    #[serde(default, alias = "Size")]
    size: String,
}

#[derive(Debug, Deserialize)]
struct PodmanContainer {
    #[serde(alias = "Id", alias = "ID")]
    id: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default, alias = "State")]
    state: String,
    #[serde(default, alias = "Image")]
    image: String,
    #[serde(default, alias = "Size")]
    size: String,
}

#[derive(Debug, Deserialize)]
struct PodmanVolume {
    #[serde(default, alias = "Name")]
    name: String,
    #[serde(default, alias = "Driver")]
    driver: String,
}

#[derive(Debug, Deserialize)]
struct PodmanNetwork {
    #[serde(default, alias = "Id", alias = "ID")]
    id: String,
    #[serde(default, alias = "Name")]
    name: String,
    #[serde(default, alias = "Driver")]
    driver: String,
}

pub struct PodmanScanner {
    base: BaseScanner,
}

impl Default for PodmanScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PodmanScanner {
    pub const fn new() -> Self {
        Self {
            base: BaseScanner::new("podman", Engine::Podman, "podman"),
        }
    }
}

#[async_trait]
impl Scanner for PodmanScanner {
    fn source(&self) -> &'static str {
        self.base.source
    }
    fn engine(&self) -> Engine {
        self.base.engine_kind
    }
    fn binary(&self) -> &'static str {
        self.base.binary
    }

    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let mut items: Vec<PrunableItem> = Vec::new();
        items.extend(self.list_images().await?);
        items.extend(self.list_containers().await?);
        items.extend(self.list_volumes().await?);
        items.extend(self.list_networks().await?);
        Ok(items)
    }

    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        let argv: Vec<&str> = match item.category {
            Category::Image => vec!["podman", "rmi", "-f", &item.id],
            Category::Container => vec!["podman", "rm", "-f", &item.id],
            Category::Volume => vec!["podman", "volume", "rm", &item.id],
            Category::Network => vec!["podman", "network", "rm", &item.id],
            other => {
                return Err(EngineError::new(
                    format!("unsupported podman category: {:?}", other),
                    self.source(),
                    vec![],
                    None,
                    "",
                ));
            }
        };
        self.base.run(&argv, TIMEOUT_SECS).await.map(|_| ())
    }
}

impl PodmanScanner {
    async fn list_images(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let active = self.active_container_image_ids().await;
        // Podman may return short (12-char) or long (sha256:...) IDs.
        // Normalise to first 12 hex chars for consistent comparison.
        let active_short: std::collections::HashSet<String> =
            active.iter().map(|id| short_image_id(id)).collect();
        let (out, _) = self
            .base
            .run(&["podman", "images", "-a", "--format", "json"], TIMEOUT_SECS)
            .await?;
        let data = parse_json_maybe_array(&out);
        let mut items: Vec<PrunableItem> = Vec::new();
        for entry in data {
            let img: PodmanImage = serde_json::from_value(entry).map_err(|e| {
                EngineError::new(
                    format!("podman image JSON: {}", e),
                    self.source(),
                    vec![],
                    None,
                    e.to_string(),
                )
            })?;
            if img.id.is_empty() {
                continue;
            }
            let repo = if img.repository.is_empty() {
                img.names.first().cloned().unwrap_or_else(|| "<none>".into())
            } else {
                img.repository.clone()
            };
            let tag = if img.tag.is_empty() {
                "<none>".to_string()
            } else {
                img.tag.clone()
            };
            let is_dangling = (repo == "<none>" || repo.is_empty()) && tag == "<none>";
            let in_use = active_short.contains(&short_image_id(&img.id));
            let status = if in_use {
                Status::Active
            } else if is_dangling {
                Status::Dangling
            } else {
                Status::Unused
            };
            let name = if is_dangling {
                img.id.chars().take(12).collect()
            } else {
                format!("{}:{}", repo, tag)
            };
            let mut extra = BTreeMap::new();
            extra.insert("repository".into(), repo);
            extra.insert("tag".into(), tag);
            items.push(PrunableItem {
                id: img.id,
                name,
                engine: Engine::Podman,
                source: self.source().to_string(),
                category: Category::Image,
                size_bytes: parse_size(&img.size),
                status,
                extra,
            });
        }
        Ok(items)
    }

    async fn list_containers(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let (out, _) = self
            .base
            .run(&["podman", "ps", "-a", "--format", "json"], TIMEOUT_SECS)
            .await?;
        let data = parse_json_maybe_array(&out);
        let mut items: Vec<PrunableItem> = Vec::new();
        for entry in data {
            let c: PodmanContainer = serde_json::from_value(entry).map_err(|e| {
                EngineError::new(
                    format!("podman container JSON: {}", e),
                    self.source(),
                    vec![],
                    None,
                    e.to_string(),
                )
            })?;
            if c.id.is_empty() {
                continue;
            }
            let state = c.state.to_lowercase();
            if state == "running" {
                continue;
            }
            let display = c
                .names
                .first()
                .cloned()
                .unwrap_or_else(|| c.id.chars().take(12).collect());
            let mut extra = BTreeMap::new();
            extra.insert("image".into(), c.image);
            extra.insert("state".into(), state);
            items.push(PrunableItem {
                id: c.id,
                name: display,
                engine: Engine::Podman,
                source: self.source().to_string(),
                category: Category::Container,
                size_bytes: parse_size(&c.size),
                status: Status::Stopped,
                extra,
            });
        }
        Ok(items)
    }

    async fn list_volumes(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let (out, _) = self
            .base
            .run(&["podman", "volume", "ls", "--format", "json"], TIMEOUT_SECS)
            .await?;
        let data = parse_json_maybe_array(&out);
        let mut items: Vec<PrunableItem> = Vec::new();
        for entry in data {
            let v: PodmanVolume = serde_json::from_value(entry).map_err(|e| {
                EngineError::new(
                    format!("podman volume JSON: {}", e),
                    self.source(),
                    vec![],
                    None,
                    e.to_string(),
                )
            })?;
            if v.name.is_empty() {
                continue;
            }
            let mut extra = BTreeMap::new();
            extra.insert("driver".into(), v.driver);
            items.push(PrunableItem {
                id: v.name.clone(),
                name: v.name,
                engine: Engine::Podman,
                source: self.source().to_string(),
                category: Category::Volume,
                size_bytes: 0,
                status: Status::Unused,
                extra,
            });
        }
        Ok(items)
    }

    async fn list_networks(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let (out, _) = self
            .base
            .run(&["podman", "network", "ls", "--format", "json"], TIMEOUT_SECS)
            .await?;
        let data = parse_json_maybe_array(&out);
        let mut items: Vec<PrunableItem> = Vec::new();
        for entry in data {
            let n: PodmanNetwork = serde_json::from_value(entry).map_err(|e| {
                EngineError::new(
                    format!("podman network JSON: {}", e),
                    self.source(),
                    vec![],
                    None,
                    e.to_string(),
                )
            })?;
            if n.id.is_empty() || n.name.is_empty() {
                continue;
            }
            if matches!(n.name.as_str(), "podman" | "default") {
                continue;
            }
            let mut extra = BTreeMap::new();
            extra.insert("driver".into(), n.driver);
            items.push(PrunableItem {
                id: n.id,
                name: n.name,
                engine: Engine::Podman,
                source: self.source().to_string(),
                category: Category::Network,
                size_bytes: 0,
                status: Status::Unused,
                extra,
            });
        }
        Ok(items)
    }

    async fn active_container_image_ids(&self) -> std::collections::HashSet<String> {
        // `{{.ID}}` is the *container* ID — we explicitly want the
        // *image* ID, which is exposed as `{{.ImageID}}`. Using the
        // wrong field silently marked in-use images as safe to delete,
        // which was a critical safety bug.
        let Ok((out, _)) = self
            .base
            .run(
                &["podman", "ps", "--format", "{{.Image}} {{.ImageID}}"],
                TIMEOUT_SECS,
            )
            .await
        else {
            return std::collections::HashSet::new();
        };
        let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for line in out.lines() {
            // ``--format "{{.Image}} {{.ImageID}}"`` does NOT emit a
            // header row, so we parse every non-empty line. If a
            // future implementation switches back to a table format
            // and emits a literal ``Image  ImageID`` header, the
            // split would still produce 2 entries and we'd silently
            // have a header in the set; callers tolerate this
            // because no real image ID matches the literal header
            // string.
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                set.insert(parts[1].to_string());
            }
        }
        set
    }
}

/// Normalise a Podman image id for set membership comparison.
/// Strips the ``sha256:`` prefix and truncates to 12 characters so
/// that short and long forms of the same id compare equal.
fn short_image_id(id: &str) -> String {
    let stripped = id.strip_prefix("sha256:").unwrap_or(id);
    stripped.chars().take(12).collect()
}

/// Podman emits either a JSON array or line-delimited JSON. Handle both.
///
/// Both branches are lenient: malformed entries are dropped with a
/// warning so a single bad line does not abort the whole scan.
/// This matches the resilience contract in the Python podman
/// scanner (``systemprune.scanners.podman._parse_json_lines``) and
/// in the Docker scanners on both stacks. The function never
/// returns an error because the scanner surfaces malformation only
/// as a ``warn!`` log line; callers should always be able to fall
/// back to an empty list.
fn parse_json_maybe_array(out: &str) -> Vec<serde_json::Value> {
    let text = out.trim();
    if text.is_empty() {
        return vec![];
    }
    if text.starts_with('[') {
        match serde_json::from_str::<Vec<serde_json::Value>>(text) {
            Ok(arr) => arr.into_iter().filter(|v| v.is_object()).collect(),
            Err(e) => {
                warn!(
                    "podman: dropping malformed JSON array ({}); returning empty result",
                    e
                );
                vec![]
            }
        }
    } else {
        let mut out: Vec<serde_json::Value> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line) {
                // Object values are kept; everything else (arrays,
                // numbers, strings, booleans, null) is silently dropped
                // because the scanner only knows how to build a
                // ``PrunableItem`` from a JSON object.
                Ok(v) if v.is_object() => out.push(v),
                Ok(_) => {}
                Err(e) => warn!(
                    "podman: dropping malformed JSON line ({}): {}",
                    e, line
                ),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_podman_json_array() {
        let out = r#"[{"Id":"a1","Names":["img"],"Size":"10 MB"}]"#;
        let parsed = parse_json_maybe_array(out);
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parses_podman_json_lines() {
        let out = "{\"Id\":\"a1\"}\n{\"Id\":\"a2\"}\n";
        let parsed = parse_json_maybe_array(out);
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parses_podman_empty() {
        assert!(parse_json_maybe_array("").is_empty());
    }

    #[test]
    fn parses_podman_whitespace_only() {
        assert!(parse_json_maybe_array("   \n  \n").is_empty());
    }

    #[test]
    fn parses_podman_skips_non_object_lines() {
        let out = "{\"Id\":\"a1\"}\n42\n\"str\"\n{\"Id\":\"a2\"}\n";
        let parsed = parse_json_maybe_array(out);
        // Only the two object lines are kept; the integer and string
        // values are dropped.
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parses_podman_skips_malformed_array() {
        // A malformed array no longer aborts the call; we just get
        // an empty list back (with a warn! log line).
        let parsed = parse_json_maybe_array("[not json");
        assert!(parsed.is_empty());
    }
}
