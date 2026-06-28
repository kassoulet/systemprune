//! Scanner for Ollama models.

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

static LINE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*(?P<name>[^\s]+)\s+(?P<id>[0-9a-f]{12,})\s+(?P<size>\S+(?:\s\S+)?)\s+(?P<modified>.+?)\s*$")
        .expect("ollama line regex compiles")
});

static FALLBACK_SEP: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());

pub struct OllamaScanner {
    base: BaseScanner,
}

impl Default for OllamaScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaScanner {
    pub const fn new() -> Self {
        Self {
            base: BaseScanner::new("ollama", Engine::Ollama, "ollama"),
        }
    }
}

#[async_trait]
impl Scanner for OllamaScanner {
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
        let loaded = self.loaded_model_names().await;
        let (out, _) = self
            .base
            .run(&["ollama", "list"], TIMEOUT_SECS)
            .await?;
        let mut items: Vec<PrunableItem> = Vec::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.to_lowercase().starts_with("name ")
                || line.chars().all(|c| c == '-' || c == ' ')
            {
                continue;
            }
            let (name, size_str, model_id) = match Self::parse_line(line) {
                Some(parsed) => parsed,
                None => continue,
            };
            let status = if loaded.contains(&name) {
                Status::Active
            } else {
                Status::Unused
            };
            let mut extra = BTreeMap::new();
            extra.insert("model_id".into(), model_id);
            extra.insert("size".into(), size_str.clone());
            items.push(PrunableItem {
                id: name.clone(),
                name,
                engine: Engine::Ollama,
                source: self.source().to_string(),
                category: Category::Model,
                size_bytes: parse_size(&size_str),
                status,
                extra,
            });
        }
        Ok(items)
    }

    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        self.base
            // Security: Use `--` to prevent the ID from being interpreted as a flag.
            .run(&["ollama", "rm", "--", &item.id], TIMEOUT_SECS)
            .await
            .map(|_| ())
    }
}

impl OllamaScanner {
    fn parse_line(line: &str) -> Option<(String, String, String)> {
        if let Some(caps) = LINE_RE.captures(line) {
            return Some((
                caps.name("name")?.as_str().to_string(),
                caps.name("size")?.as_str().to_string(),
                caps.name("id")?.as_str().to_string(),
            ));
        }
        let parts: Vec<&str> = FALLBACK_SEP.split(line.trim()).filter(|s| !s.is_empty()).collect();
        if parts.len() >= 3 {
            return Some((
                parts[0].to_string(),
                parts[2..].join(" "),
                parts[1].to_string(),
            ));
        }
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string(), String::new()));
        }
        None
    }

    async fn loaded_model_names(&self) -> HashSet<String> {
        let Ok((out, _)) = self.base.run(&["ollama", "ps"], TIMEOUT_SECS).await else {
            return HashSet::new();
        };
        let mut set: HashSet<String> = HashSet::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.to_lowercase().starts_with("name ")
                || line.chars().all(|c| c == '-' || c == ' ')
            {
                continue;
            }
            if let Some(name) = line.split_whitespace().next() {
                set.insert(name.to_string());
            }
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_with_regex() {
        let line = "llama2:7b        1a2b3c4d5e6f    3.8 GB    2 weeks ago";
        let (name, size, id) = OllamaScanner::parse_line(line).unwrap();
        assert_eq!(name, "llama2:7b");
        assert_eq!(id, "1a2b3c4d5e6f");
        assert!(size.contains("3.8"));
        assert!(size.contains("GB"));
    }

    #[test]
    fn parse_line_fallback() {
        let line = "mistral:7b     1234abcd     4.1GB";
        let (name, size, _id) = OllamaScanner::parse_line(line).unwrap();
        assert_eq!(name, "mistral:7b");
        assert!(size.contains("4.1"));
    }

    #[test]
    fn parse_line_unparseable() {
        assert!(OllamaScanner::parse_line("").is_none());
        assert!(OllamaScanner::parse_line("garbage").is_none());
    }

    #[test]
    fn parse_line_two_columns_fallback() {
        // Two-column fallback: name + size, no id.
        let line = "qwen2.5:7b    5.2GB";
        let (name, size, id) = OllamaScanner::parse_line(line).unwrap();
        assert_eq!(name, "qwen2.5:7b");
        assert!(size.contains("5.2"));
        assert_eq!(id, "");
    }

    #[test]
    fn parse_line_irregular_whitespace() {
        // Mixed tabs / spaces should still be parseable.
        let line = "llama3.1:8b\tdeadbeef0000\t4.7 GB\t1 week ago";
        let (name, size, id) = OllamaScanner::parse_line(line).unwrap();
        assert_eq!(name, "llama3.1:8b");
        assert_eq!(id, "deadbeef0000");
        assert!(size.contains("4.7"));
    }
}
