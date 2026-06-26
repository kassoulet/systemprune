//! Scanner for Podman (mirrors Docker's CLI surface).

use super::base::BaseScanner;
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use crate::size::parse_size;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::BTreeMap;

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
        let (out, _) = self
            .base
            .run(&["podman", "images", "-a", "--format", "json"], TIMEOUT_SECS)
            .await?;
        let data = parse_json_maybe_array(&out)?;
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
            let in_use = active.contains(&img.id);
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
        let data = parse_json_maybe_array(&out)?;
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
        let data = parse_json_maybe_array(&out)?;
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
        let data = parse_json_maybe_array(&out)?;
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
        let Ok((out, _)) = self
            .base
            .run(&["podman", "ps", "--format", "{{.Image}} {{.ID}}"], TIMEOUT_SECS)
            .await
        else {
            return std::collections::HashSet::new();
        };
        let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for line in out.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                set.insert(parts[1].to_string());
            }
        }
        set
    }
}

/// Podman emits either a JSON array or line-delimited JSON. Handle both.
fn parse_json_maybe_array(out: &str) -> Result<Vec<serde_json::Value>, EngineError> {
    let text = out.trim();
    if text.is_empty() {
        return Ok(vec![]);
    }
    if text.starts_with('[') {
        serde_json::from_str::<Vec<serde_json::Value>>(text).map_err(|e| {
            EngineError::new(
                format!("podman JSON array: {}", e),
                "podman",
                vec![],
                None,
                e.to_string(),
            )
        })
    } else {
        let mut out: Vec<serde_json::Value> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line).map_err(|e| {
                EngineError::new(
                    format!("podman JSON line: {}", e),
                    "podman",
                    vec![],
                    None,
                    e.to_string(),
                )
            })?;
            if v.is_object() {
                out.push(v);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_podman_json_array() {
        let out = r#"[{"Id":"a1","Names":["img"],"Size":"10 MB"}]"#;
        let parsed = parse_json_maybe_array(out).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parses_podman_json_lines() {
        let out = "{\"Id\":\"a1\"}\n{\"Id\":\"a2\"}\n";
        let parsed = parse_json_maybe_array(out).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parses_podman_empty() {
        assert!(parse_json_maybe_array("").unwrap().is_empty());
    }

    #[test]
    fn parses_podman_whitespace_only() {
        assert!(parse_json_maybe_array("   \n  \n").unwrap().is_empty());
    }

    #[test]
    fn parses_podman_skips_non_object_lines() {
        let out = "{\"Id\":\"a1\"}\n42\n\"str\"\n{\"Id\":\"a2\"}\n";
        let parsed = parse_json_maybe_array(out).unwrap();
        // Only the two object lines are kept; the integer and string
        // values are dropped.
        assert_eq!(parsed.len(), 2);
    }
}
