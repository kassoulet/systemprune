//! Scanner for Flatpak apps and runtimes.

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

static COL_SEP: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());

pub struct FlatpakScanner {
    base: BaseScanner,
}

impl Default for FlatpakScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl FlatpakScanner {
    pub const fn new() -> Self {
        Self {
            base: BaseScanner::new("flatpak", Engine::Flatpak, "flatpak"),
        }
    }
}

#[async_trait]
impl Scanner for FlatpakScanner {
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
        items.extend(self.list_apps().await?);
        items.extend(self.list_runtimes().await?);
        Ok(items)
    }

    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        self.base
            .run(
                &["flatpak", "uninstall", "--delete-data", "-y", &item.id],
                TIMEOUT_SECS,
            )
            .await
            .map(|_| ())
    }
}

impl FlatpakScanner {
    /// Split a single ``flatpak list`` row into ``(id, size_str, extra)``.
    /// Returns ``None`` for empty lines or the header row.
    fn parse_row(line: &str) -> Option<(String, String, Vec<String>)> {
        let line = line.trim();
        if line.is_empty() || line.to_lowercase().starts_with("application ") {
            return None;
        }
        let parts: Vec<String> = COL_SEP
            .split(line)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        if parts.is_empty() || parts[0].is_empty() {
            return None;
        }
        let id = parts[0].clone();
        let size = parts.get(1).cloned().unwrap_or_else(|| "0".to_string());
        let extras = parts.into_iter().skip(2).collect();
        Some((id, size, extras))
    }

    async fn list_apps(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let running = self.active_app_ids().await;
        let (out, _) = self
            .base
            .run(
                &["flatpak", "list", "--app", "--columns=application,size,runtime"],
                TIMEOUT_SECS,
            )
            .await?;
        let mut items: Vec<PrunableItem> = Vec::new();
        for line in out.lines() {
            let Some((app_id, size_str, extras)) = Self::parse_row(line) else {
                continue;
            };
            let runtime = extras.into_iter().next().unwrap_or_default();
            let mut extra = BTreeMap::new();
            extra.insert("runtime".into(), runtime);
            let is_active = running.contains(&app_id);
            items.push(PrunableItem {
                id: app_id.clone(),
                name: app_id,
                engine: Engine::Flatpak,
                source: self.source().to_string(),
                category: Category::App,
                size_bytes: parse_size(&size_str),
                status: if is_active { Status::Active } else { Status::Unused },
                extra,
            });
        }
        Ok(items)
    }

    async fn list_runtimes(&self) -> Result<Vec<PrunableItem>, EngineError> {
        let (out, _) = self
            .base
            .run(
                &[
                    "flatpak",
                    "list",
                    "--runtime",
                    "--columns=application,size,runtime,arch,branch",
                ],
                TIMEOUT_SECS,
            )
            .await?;
        let mut items: Vec<PrunableItem> = Vec::new();
        for line in out.lines() {
            let Some((reference, size_str, extras)) = Self::parse_row(line) else {
                continue;
            };
            if reference.is_empty() {
                continue;
            }
            // Column order on the wire is
            //   application, size, runtime, arch, branch
            // `parse_row` already stripped ``reference`` (parts[0])
            // and ``size_str`` (parts[1]); what remains is the
            // ordered ``[runtime, arch, branch, ...]`` tail.
            let mut iter = extras.into_iter();
            let mut extra = BTreeMap::new();
            extra.insert("runtime".into(), iter.next().unwrap_or_default());
            extra.insert("arch".into(), iter.next().unwrap_or_default());
            extra.insert("branch".into(), iter.next().unwrap_or_default());
            items.push(PrunableItem {
                id: reference.clone(),
                name: reference,
                engine: Engine::Flatpak,
                source: self.source().to_string(),
                category: Category::Runtime,
                size_bytes: parse_size(&size_str),
                status: Status::Unused,
                extra,
            });
        }
        Ok(items)
    }

    async fn active_app_ids(&self) -> HashSet<String> {
        let Ok((out, _)) = self
            .base
            .run(&["flatpak", "ps", "--columns=application"], TIMEOUT_SECS)
            .await
        else {
            return HashSet::new();
        };
        let mut set: HashSet<String> = HashSet::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() || line.to_lowercase().starts_with("application") {
                continue;
            }
            set.insert(line.to_string());
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_row_apps() {
        let line = "org.gimp.GIMP          142.1 MB   org.gnome.Platform/x86_64/45";
        let (id, size, extras) = FlatpakScanner::parse_row(line).unwrap();
        assert_eq!(id, "org.gimp.GIMP");
        assert_eq!(size, "142.1 MB");
        assert_eq!(extras, vec!["org.gnome.Platform/x86_64/45".to_string()]);
    }

    #[test]
    fn parse_row_runtimes() {
        let line = "org.gnome.Platform      1.2 GB   master  x86_64   45";
        let (id, size, extras) = FlatpakScanner::parse_row(line).unwrap();
        assert_eq!(id, "org.gnome.Platform");
        assert_eq!(size, "1.2 GB");
        assert_eq!(
            extras,
            vec!["master".to_string(), "x86_64".to_string(), "45".to_string()]
        );
    }

    #[test]
    fn parse_row_skips_header_and_blank() {
        assert!(FlatpakScanner::parse_row("").is_none());
        assert!(FlatpakScanner::parse_row("   ").is_none());
        assert!(FlatpakScanner::parse_row("Application ID   Size   Runtime").is_none());
    }
}
