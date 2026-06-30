//! Scanner for Snap packages.

use super::base::BaseScanner;
use super::Scanner;
use crate::errors::EngineError;
use crate::models::{Category, Engine, PrunableItem, Status};
use crate::size::parse_size;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{BTreeMap, HashSet};

const TIMEOUT_SECS: u64 = 60;

static COL_SEP: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}|\t+").unwrap());

/// System snaps that should never be flagged for deletion.
const PROTECTED: &[&str] = &["snapd", "core", "core20", "core22", "core24", "bare"];

pub struct SnapScanner {
    base: BaseScanner,
}

impl Default for SnapScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapScanner {
    pub const fn new() -> Self {
        Self {
            base: BaseScanner::new("snap", Engine::Snap, "snap"),
        }
    }
}

#[async_trait]
impl Scanner for SnapScanner {
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
        Ok(self.list_snaps().await?)
    }

    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        if PROTECTED.contains(&item.id.as_str()) {
            return Err(EngineError::new(
                format!("refusing to remove protected snap: {}", item.id),
                self.source(),
                vec![],
                None,
                "",
            ));
        }
        self.base
            // Security: Use `--` to prevent the ID from being interpreted as a flag.
            .run(&["snap", "remove", "--", &item.id], TIMEOUT_SECS)
            .await
            .map(|_| ())
    }
}

impl SnapScanner {
    /// Split a single ``snap list`` row into ``(name, version, revision, size_str)``.
    /// Returns ``None`` for the header row or any row with fewer than
    /// three columns.
    fn parse_row(line: &str) -> Option<(String, String, String, String)> {
        let parts: Vec<String> = COL_SEP
            .split(line.trim())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        if parts.len() < 3 {
            return None;
        }
        Some((
            parts[0].clone(),
            parts[1].clone(),
            parts[2].clone(),
            parts.get(3).cloned().unwrap_or_else(|| "0".to_string()),
        ))
    }

    async fn list_snaps(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let running = self.active_service_snaps().await;
        let (out, _) = self
            .base
            .run(&["snap", "list", "--unicode=never"], TIMEOUT_SECS)
            .await?;
        let mut items: Vec<PrunableItem> = Vec::new();
        let mut lines = out.lines();
        // Drop header.
        if lines.next().is_none() {
            return Ok(items);
        }
        for line in lines {
            let Some((name, version, revision, size_str)) = Self::parse_row(line) else {
                continue;
            };
            if name.is_empty() || PROTECTED.contains(&name.as_str()) {
                continue;
            }
            let mut extra = BTreeMap::new();
            extra.insert("version".into(), version);
            extra.insert("revision".into(), revision);
            let is_active = running.contains(&name);
            items.push(PrunableItem {
                id: name.clone(),
                name,
                engine: Engine::Snap,
                source: self.source().to_string(),
                category: Category::SnapRevision,
                size_bytes: parse_size(&size_str),
                status: if is_active {
                    Status::Active
                } else {
                    Status::Unused
                },
                extra,
            });
        }
        Ok(items)
    }

    async fn active_service_snaps(&self) -> HashSet<String> {
        let Ok((out, _)) = self
            .base
            .run(&["snap", "services", "--unicode=never"], TIMEOUT_SECS)
            .await
        else {
            return HashSet::new();
        };
        let mut set: HashSet<String> = HashSet::new();
        let mut lines = out.lines();
        // Drop header.
        if lines.next().is_none() {
            return set;
        }
        for line in lines {
            let parts: Vec<&str> = COL_SEP
                .split(line.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if parts.is_empty() {
                continue;
            }
            // Service names look like ``snap.<snap-name>.<service>`` —
            // take the second component (first is always ``"snap"``).
            let snap_name = parts[0].split('.').nth(1).unwrap_or("").to_string();
            if !snap_name.is_empty() {
                set.insert(snap_name);
            }
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_row_basic() {
        let line = "firefox   1234   4567   250 MB";
        let (name, version, rev, size) = SnapScanner::parse_row(line).unwrap();
        assert_eq!(name, "firefox");
        assert_eq!(version, "1234");
        assert_eq!(rev, "4567");
        assert_eq!(size, "250 MB");
    }

    #[test]
    fn parse_row_three_columns_no_size() {
        let line = "code   1.85   234";
        let (name, version, rev, size) = SnapScanner::parse_row(line).unwrap();
        assert_eq!(name, "code");
        assert_eq!(version, "1.85");
        assert_eq!(rev, "234");
        assert_eq!(size, "0"); // default
    }

    #[test]
    fn parse_row_too_few_columns() {
        assert!(SnapScanner::parse_row("only").is_none());
        assert!(SnapScanner::parse_row("two columns").is_none());
    }
}
