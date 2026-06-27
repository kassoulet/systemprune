//! Scanner for Docker images, containers, volumes, and networks.

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
struct DockerImage {
    #[serde(alias = "ID", alias = "Id")]
    id: String,
    #[serde(default, alias = "Repository")]
    repository: String,
    #[serde(default, alias = "Tag")]
    tag: String,
    #[serde(default, alias = "Size")]
    size: String,
    #[serde(default, alias = "CreatedSince")]
    created_since: String,
    #[serde(default, alias = "CreatedAt")]
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct DockerContainer {
    #[serde(alias = "ID", alias = "Id")]
    id: String,
    #[serde(default, alias = "Names")]
    names: String,
    #[serde(default, alias = "State")]
    state: String,
    #[serde(default, alias = "Image")]
    image: String,
    #[serde(default, alias = "Size")]
    size: String,
}

#[derive(Debug, Deserialize)]
struct DockerVolume {
    #[serde(default, alias = "Name")]
    name: String,
    #[serde(default, alias = "Driver")]
    driver: String,
}

#[derive(Debug, Deserialize)]
struct DockerNetwork {
    #[serde(default, alias = "ID", alias = "Id")]
    id: String,
    #[serde(default, alias = "Name")]
    name: String,
    #[serde(default, alias = "Driver")]
    driver: String,
}

pub struct DockerScanner {
    base: BaseScanner,
}

impl Default for DockerScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerScanner {
    pub const fn new() -> Self {
        Self {
            base: BaseScanner::new("docker", Engine::Docker, "docker"),
        }
    }
}

#[async_trait]
impl Scanner for DockerScanner {
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
            Category::Image => vec!["docker", "rmi", "-f", &item.id],
            Category::Container => vec!["docker", "rm", "-f", &item.id],
            Category::Volume => vec!["docker", "volume", "rm", &item.id],
            Category::Network => vec!["docker", "network", "rm", &item.id],
            other => {
                return Err(EngineError::new(
                    format!("unsupported docker category: {:?}", other),
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

impl DockerScanner {
    async fn list_images(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let active = self.active_container_image_ids().await;
        let (out, _) = self
            .base
            .run(&["docker", "images", "-a", "--format", "{{json .}}"], TIMEOUT_SECS)
            .await?;
        // Docker's `docker ps` may return short (12-char) IDs while
        // `docker images` returns long (`sha256:...`) IDs. Normalise
        // both to the first 12 hex characters for comparison.
        let active_short: std::collections::HashSet<String> = active
            .iter()
            .map(|id| short_image_id(id))
            .collect();
        let mut items: Vec<PrunableItem> = Vec::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let img: DockerImage = match serde_json::from_str(line) {
                Ok(i) => i,
                Err(e) => {
                    // One malformed line must not abort the whole
                    // scan; skip and continue so the user still sees
                    // the well-formed entries.
                    warn!(
                        "docker: skipping malformed image JSON line ({}): {}",
                        e, line
                    );
                    continue;
                }
            };
            if img.id.is_empty() {
                continue;
            }
            let is_dangling =
                (img.repository == "<none>" || img.repository.is_empty()) && img.tag == "<none>";
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
                format!("{}:{}", img.repository, img.tag)
            };
            let mut extra = BTreeMap::new();
            extra.insert("repository".into(), img.repository.clone());
            extra.insert("tag".into(), img.tag.clone());
            extra.insert("created_since".into(), img.created_since);
            extra.insert("created_at".into(), img.created_at);
            items.push(PrunableItem {
                id: img.id,
                name,
                engine: Engine::Docker,
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
            .run(&["docker", "ps", "-a", "--format", "{{json .}}"], TIMEOUT_SECS)
            .await?;
        let mut items: Vec<PrunableItem> = Vec::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let c: DockerContainer = match serde_json::from_str(line) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        "docker: skipping malformed container JSON line ({}): {}",
                        e, line
                    );
                    continue;
                }
            };
            if c.id.is_empty() {
                continue;
            }
            let state = c.state.to_lowercase();
            if state == "running" {
                continue; // only stopped containers are prunable
            }
            let name = if c.names.is_empty() {
                c.id.chars().take(12).collect()
            } else {
                c.names.clone()
            };
            let mut extra = BTreeMap::new();
            extra.insert("image".into(), c.image);
            extra.insert("state".into(), state);
            items.push(PrunableItem {
                id: c.id,
                name,
                engine: Engine::Docker,
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
        // Get volume sizes from `docker system df -v`.
        let volume_sizes = self.volume_size_map().await;
        let (out, _) = self
            .base
            .run(&["docker", "volume", "ls", "--format", "{{json .}}"], TIMEOUT_SECS)
            .await?;
        let mut items: Vec<PrunableItem> = Vec::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: DockerVolume = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        "docker: skipping malformed volume JSON line ({}): {}",
                        e, line
                    );
                    continue;
                }
            };
            if v.name.is_empty() {
                continue;
            }
            let size_bytes = volume_sizes.get(&v.name).copied().unwrap_or(0);
            let mut extra = BTreeMap::new();
            extra.insert("driver".into(), v.driver);
            items.push(PrunableItem {
                id: v.name.clone(),
                name: v.name,
                engine: Engine::Docker,
                source: self.source().to_string(),
                category: Category::Volume,
                size_bytes,
                status: Status::Unused,
                extra,
            });
        }
        Ok(items)
    }

    /// Parse `docker system df -v` to get per-volume sizes.
    async fn volume_size_map(&self) -> BTreeMap<String, u64> {
        let Ok((out, _)) = self
            .base
            .run(&["docker", "system", "df", "-v"], TIMEOUT_SECS)
            .await
        else {
            return BTreeMap::new();
        };
        let mut map = BTreeMap::new();
        let mut in_volumes_section = false;
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Detect the "Local Volumes:" or "VOLUME NAME" header.
            if line.contains("VOLUME NAME") || line.starts_with("Local Volumes") {
                in_volumes_section = true;
                continue;
            }
            // Stop if we hit the next section (Build Cache, etc.).
            if in_volumes_section && (line.starts_with("Build Cache") || line.starts_with("Images") || line.starts_with("Containers")) {
                in_volumes_section = false;
                continue;
            }
            if !in_volumes_section {
                continue;
            }
            // Parse line: "volume_name    links    size"
            // Use 2+ whitespace as separator.
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let name = parts[0].to_string();
                // Size is the last column.
                let size_str = parts.last().unwrap_or(&"0");
                let size = parse_size(size_str);
                if !name.is_empty() && size > 0 {
                    map.insert(name, size);
                }
            }
        }
        map
    }

    async fn list_networks(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let (out, _) = self
            .base
            .run(&["docker", "network", "ls", "--format", "{{json .}}"], TIMEOUT_SECS)
            .await?;
        let mut items: Vec<PrunableItem> = Vec::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let n: DockerNetwork = match serde_json::from_str(line) {
                Ok(n) => n,
                Err(e) => {
                    warn!(
                        "docker: skipping malformed network JSON line ({}): {}",
                        e, line
                    );
                    continue;
                }
            };
            if n.id.is_empty() || n.name.is_empty() {
                continue;
            }
            // Default networks must never be removed.
            if matches!(n.name.as_str(), "bridge" | "host" | "none") {
                continue;
            }
            let mut extra = BTreeMap::new();
            extra.insert("driver".into(), n.driver);
            items.push(PrunableItem {
                id: n.id,
                name: n.name,
                engine: Engine::Docker,
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
            .run(&["docker", "ps", "--format", "{{.Image}} {{.ImageID}}"], TIMEOUT_SECS)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_image_json() {
        let line = r#"{"ID":"sha256:abc123","Repository":"nginx","Tag":"latest","Size":"142MB","CreatedSince":"2 days ago","CreatedAt":"2024-01-01 00:00:00 +0000 UTC"}"#;
        let img: DockerImage = serde_json::from_str(line).unwrap();
        assert_eq!(img.id, "sha256:abc123");
        assert_eq!(img.repository, "nginx");
    }

    #[test]
    fn parses_docker_container_json() {
        let line = r#"{"ID":"abcd1234efgh","Names":"web","State":"exited","Image":"nginx:latest","Size":"0B"}"#;
        let c: DockerContainer = serde_json::from_str(line).unwrap();
        assert_eq!(c.id, "abcd1234efgh");
        assert_eq!(c.state, "exited");
        assert_eq!(c.names, "web");
    }

    #[test]
    fn parses_docker_volume_json() {
        let line = r#"{"Name":"my-vol","Driver":"local"}"#;
        let v: DockerVolume = serde_json::from_str(line).unwrap();
        assert_eq!(v.name, "my-vol");
        assert_eq!(v.driver, "local");
    }

    #[test]
    fn parses_docker_network_json() {
        let line = r#"{"ID":"net1234","Name":"bridge","Driver":"bridge"}"#;
        let n: DockerNetwork = serde_json::from_str(line).unwrap();
        assert_eq!(n.id, "net1234");
        assert_eq!(n.name, "bridge");
    }

    #[test]
    fn short_image_id_normalises_prefixes() {
        assert_eq!(short_image_id("sha256:abc123def456"), "abc123def456");
        assert_eq!(short_image_id("abc123def4567890"), "abc123def456");
        assert_eq!(short_image_id("abc123"), "abc123");
        assert_eq!(short_image_id(""), "");
    }
}

/// Normalise a Docker image id for set membership comparison.
/// Strips the ``sha256:`` prefix and truncates to 12 characters so
/// that short and long forms of the same id compare equal.
fn short_image_id(id: &str) -> String {
    let stripped = id.strip_prefix("sha256:").unwrap_or(id);
    stripped.chars().take(12).collect()
}
