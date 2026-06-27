//! Concurrent scanning and batched deletion across scanners.

use crate::errors::{EngineError, SystemPruneError};
use crate::log::ActionLog;
use crate::models::PrunableItem;
use crate::scanners::Scanner;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::task::JoinSet;

/// The aggregated result of a single scan.
#[derive(Debug, Default, Clone)]
pub struct ScanResult {
    pub items: Vec<PrunableItem>,
    pub errors: Vec<EngineError>,
}

impl ScanResult {
    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.size_bytes).sum()
    }

    /// Group items by their `source` field for UI display.
    pub fn by_engine(&self) -> BTreeMap<String, Vec<PrunableItem>> {
        let mut out: BTreeMap<String, Vec<PrunableItem>> = BTreeMap::new();
        for item in &self.items {
            out.entry(item.source.clone()).or_default().push(item.clone());
        }
        out
    }

    /// Group items by their `category` field for the grouped-by-type
    /// UIs. The returned vec preserves first-seen order of
    /// categories; the inner vec preserves first-seen order of
    /// items within each category.
    pub fn by_category(&self) -> Vec<(crate::models::Category, Vec<PrunableItem>)> {
        let mut order: Vec<crate::models::Category> = Vec::new();
        let mut buckets: BTreeMap<crate::models::Category, Vec<PrunableItem>> =
            BTreeMap::new();
        for item in &self.items {
            if !buckets.contains_key(&item.category) {
                order.push(item.category);
            }
            buckets.entry(item.category).or_default().push(item.clone());
        }
        order
            .into_iter()
            .map(|c| (c, buckets.remove(&c).unwrap_or_default()))
            .collect()
    }
}

/// Per-item result from `delete_many`.
#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub item: PrunableItem,
    pub success: bool,
    pub error: Option<EngineError>,
}

/// Coordinates scanning and deletion across a set of [`Scanner`]s.
#[derive(Clone)]
pub struct Orchestrator {
    all: Vec<Arc<dyn Scanner>>,
    active: Vec<Arc<dyn Scanner>>,
    by_source: BTreeMap<String, Arc<dyn Scanner>>,
    /// Optional action log.  When set, scan/delete events
    /// are pushed here so the UIs can show a trace of what
    /// the app is doing.  A `None` log is a no-op.
    log: Option<ActionLog>,
}

impl Orchestrator {
    /// Create a new orchestrator and immediately drop scanners whose
    /// CLI binary is not on ``$PATH``.
    pub fn new(scanners: Vec<Arc<dyn Scanner>>) -> Self {
        let all = scanners;
        let mut active: Vec<Arc<dyn Scanner>> = Vec::new();
        let mut by_source: BTreeMap<String, Arc<dyn Scanner>> = BTreeMap::new();
        for s in &all {
            if s.is_available() {
                by_source.insert(s.source().to_string(), s.clone());
                active.push(s.clone());
            }
        }
        Self {
            all,
            active,
            by_source,
            log: None,
        }
    }

    /// Builder-style: attach an action log.  Returns `self`
    /// for chaining.
    pub fn with_log(mut self, log: ActionLog) -> Self {
        self.log = Some(log);
        self
    }

    /// Attach an action log to an already-constructed
    /// orchestrator.
    pub fn set_log(&mut self, log: ActionLog) {
        self.log = Some(log);
    }

    pub fn all_scanners(&self) -> &[Arc<dyn Scanner>] {
        &self.all
    }

    pub fn active_scanners(&self) -> &[Arc<dyn Scanner>] {
        &self.active
    }

    pub fn available_engines(&self) -> Vec<String> {
        self.active.iter().map(|s| s.source().to_string()).collect()
    }

    /// Run `get_items` on every active scanner in parallel.
    pub async fn scan_all(&self) -> ScanResult {
        if let Some(log) = &self.log {
            log.info(format!(
                "Scanning started ({} active scanner(s))",
                self.active.len()
            ));
        }
        let mut set = JoinSet::new();
        for scanner in &self.active {
            let s = scanner.clone();
            set.spawn(async move {
                let res = s.get_items().await;
                (s.source().to_string(), res)
            });
        }
        let mut items: Vec<PrunableItem> = Vec::new();
        let mut errors: Vec<EngineError> = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((src, Ok(scanner_items))) => {
                    if let Some(log) = &self.log {
                        log.info(format!(
                            "Scanner {} found {} item(s)",
                            src,
                            scanner_items.len()
                        ));
                    }
                    items.extend(scanner_items);
                }
                Ok((src, Err(e))) => {
                    if let Some(log) = &self.log {
                        log.error(format!("Scanner {} failed: {}", src, e.message));
                    }
                    errors.push(e);
                }
                Err(join_err) => {
                    if let Some(log) = &self.log {
                        log.error(format!("Scanner task join error: {}", join_err));
                    }
                }
            }
        }
        if let Some(log) = &self.log {
            log.info(format!(
                "Scanning complete: {} item(s), {} error(s)",
                items.len(),
                errors.len()
            ));
        }
        ScanResult { items, errors }
    }

    /// Delete the given items via their owning scanner.
    ///
    /// If `confirm` is true, items whose `is_safe_to_delete()` is
    /// false are skipped and reported in the result with
    /// `success = false`.
    ///
    /// If `confirm` is true and `delete_errors` is `Some(map)`,
    /// items whose `(source, id)` is present in the map are also
    /// skipped with a synthetic "previously failed" error. This
    /// is defence-in-depth against re-queuing items the engine has
    /// already rejected: the UIs already filter failed items out
    /// of their `to_delete` selection, but a future code path or a
    /// programmatic caller (e.g. a test) might bypass the filter,
    /// and we want the orchestrator to stay correct on its own.
    /// Pass `None` to opt out (e.g. from the CLI which has no
    /// concept of persistent failure tracking).
    ///
    /// The returned vector has the same length and ordering as
    /// *items*; entries for items that had no matching scanner or
    /// whose owning task failed to join are still present (with
    /// `success = false` and a synthetic `EngineError`).
    pub async fn delete_many(
        &self,
        items: &[PrunableItem],
        confirm: bool,
        delete_errors: Option<&BTreeMap<(String, String), String>>,
    ) -> Vec<DeleteResult> {
        if let Some(log) = &self.log {
            let total: u64 = items.iter().map(|i| i.size_bytes).sum();
            log.info(format!(
                "Delete started: {} item(s), {}",
                items.len(),
                crate::size::format_size(total as i64, true)
            ));
        }
        // Pre-allocate slots so the returned vector preserves the
        // caller's order regardless of completion order. Each slot
        // is filled either with the scanner's result or with a
        // synthetic error.
        let mut slots: Vec<Option<DeleteResult>> = (0..items.len()).map(|_| None).collect();
        // Receivers to await after spawning all the delete tasks.
        // Each entry remembers the originating index so we can
        // write the result back to the correct slot.
        let mut pending: Vec<(usize, tokio::sync::oneshot::Receiver<DeleteResult>)> =
            Vec::new();

        for (idx, item) in items.iter().cloned().enumerate() {
            // Extract strings up-front so the borrowed ``item`` can
            // be moved into the spawned task without invalidating
            // any references.
            let engine_name = item.engine.as_str().to_string();
            let item_source = item.source.clone();
            let item_name = item.name.clone();
            let item_id = item.id.clone();
            let item_key = (item_source.clone(), item_id);

            if confirm && !item.is_safe_to_delete() {
                slots[idx] = Some(DeleteResult {
                    item,
                    success: false,
                    error: Some(EngineError::new(
                        format!("Refusing to delete active item: {}", item_name),
                        engine_name,
                        vec![],
                        None,
                        "",
                    )),
                });
                continue;
            }
            if confirm && delete_errors.map_or(false, |m| m.contains_key(&item_key)) {
                slots[idx] = Some(DeleteResult {
                    item,
                    success: false,
                    error: Some(EngineError::new(
                        format!(
                            "Refusing to delete item that previously failed: {}",
                            item_name
                        ),
                        engine_name,
                        vec![],
                        None,
                        "",
                    )),
                });
                continue;
            }
            let scanner = match self.by_source.get(&item_source) {
                Some(s) => s.clone(),
                None => {
                    slots[idx] = Some(DeleteResult {
                        item,
                        success: false,
                        error: Some(EngineError::new(
                            format!("No active scanner for source {}", item_source),
                            engine_name,
                            vec![],
                            None,
                            "",
                        )),
                    });
                    continue;
                }
            };
            let (tx, rx) = tokio::sync::oneshot::channel::<DeleteResult>();
            tokio::spawn(async move {
                let result = match scanner.delete_item(&item).await {
                    Ok(()) => DeleteResult {
                        item,
                        success: true,
                        error: None,
                    },
                    Err(e) => DeleteResult {
                        item,
                        success: false,
                        error: Some(e),
                    },
                };
                let _ = tx.send(result);
            });
            pending.push((idx, rx));
        }

        for (idx, rx) in pending {
            match rx.await {
                Ok(result) => {
                    if let Some(log) = &self.log {
                        if result.success {
                            log.info(format!(
                                "Deleted {}:{} ({} bytes)",
                                result.item.source,
                                result.item.id,
                                result.item.size_bytes
                            ));
                        } else {
                            let msg = result
                                .error
                                .as_ref()
                                .map(|e| e.message.as_str())
                                .unwrap_or("(no error message)");
                            log.error(format!(
                                "Delete failed {}:{} \u{2014} {}",
                                result.item.source, result.item.id, msg
                            ));
                        }
                    }
                    slots[idx] = Some(result);
                }
                Err(_) => {
                    let original = &items[idx];
                    if let Some(log) = &self.log {
                        log.error(format!(
                            "Delete cancelled for {}:{}",
                            original.source, original.id
                        ));
                    }
                    slots[idx] = Some(DeleteResult {
                        item: original.clone(),
                        success: false,
                        error: Some(EngineError::new(
                            "scanner task was cancelled",
                            original.engine.as_str(),
                            vec![],
                            None,
                            "",
                        )),
                    });
                }
            }
        }

        let results: Vec<DeleteResult> = slots
            .into_iter()
            .map(|o| o.expect("all slots filled"))
            .collect();
        if let Some(log) = &self.log {
            let ok = results.iter().filter(|r| r.success).count();
            let fail = results.len() - ok;
            let freed: u64 = results
                .iter()
                .filter(|r| r.success)
                .map(|r| r.item.size_bytes)
                .sum();
            log.info(format!(
                "Delete complete: {} succeeded, {} failed, {} freed",
                ok,
                fail,
                crate::size::format_size(freed as i64, true)
            ));
        }
        results
    }

    /// Return the scanner responsible for `item`, if any.
    pub fn scanner_for(&self, item: &PrunableItem) -> Option<Arc<dyn Scanner>> {
        self.by_source.get(&item.source).cloned()
    }
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("all", &self.all.iter().map(|s| s.source()).collect::<Vec<_>>())
            .field("active", &self.active.iter().map(|s| s.source()).collect::<Vec<_>>())
            .finish()
    }
}

/// Convert a [`SystemPruneError`] to a human-readable string.
pub fn describe_error(err: &SystemPruneError) -> String {
    err.to_string()
}
