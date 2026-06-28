//! Concurrent scanning and batched deletion across scanners.

use crate::errors::EngineError;
use crate::history::{History, HistoryEntry, DEFAULT_KEEP_FILES, DEFAULT_MAX_BYTES};
use crate::log::ActionLog;
use crate::models::PrunableItem;
use crate::scanners::Scanner;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
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

/// One engine's aggregated dashboard row.
///
/// Built by [`Dashboard::compute`] from a [`ScanResult`].  Each
/// row summarises a single engine: how many items it found,
/// how much space they occupy, and what the single largest
/// item is.  Used by the CLI's `dashboard` subcommand, the
/// TUI's press-`D` screen, and the GUI's landing page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardRow {
    /// Scanner source name (e.g. ``"docker"``).  Matches
    /// ``PrunableItem::source``; this is the grouping key.
    pub source: String,
    /// Number of items this engine found.
    pub count: usize,
    /// Sum of ``size_bytes`` for every item in the group.
    pub total_bytes: u64,
    /// Largest item in the group by ``size_bytes``.  ``None``
    /// when the engine reported no items.
    pub top: Option<DashboardTopItem>,
}

/// Largest single item in an engine's dashboard row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardTopItem {
    /// Reserved for future clickable GUI rows (`--json`
    /// consumers see it today).  The CLI's text formatter
    /// and the TUI/GUI's `Label`/`ActionRow` views only
    /// render `name` and `size_bytes`; keeping the field
    /// populated lets a follow-up commit light up a
    /// click-to-jump-to-list-view action without a schema
    /// bump.
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
}

/// The full dashboard view.
///
/// Returned by [`Dashboard::compute`] from a [`ScanResult`].
/// Rows are sorted by ``total_bytes`` descending so the
/// biggest disk-space contributors surface first.  Empty
/// ``ScanResult``s produce an empty dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dashboard {
    pub rows: Vec<DashboardRow>,
}

impl Dashboard {
    /// Build a dashboard from a [`ScanResult`].  Groups items by
    /// ``source``, computes the count + total bytes + largest
    /// item per group, then sorts rows by ``total_bytes``
    /// descending.  When two groups have equal totals the
    /// ``source`` name is used as a tie-breaker so the output
    /// is deterministic.
    pub fn compute(scan: &ScanResult) -> Self {
        Self::compute_items(&scan.items)
    }

    /// Build a dashboard directly from a `&[PrunableItem]`.
    /// Equivalent to `compute(&ScanResult { items: items.to_vec(), errors: vec![] })`
    /// but avoids an allocation when the caller already has an
    /// item slice handy (e.g. the TUI/GUI render loop, which
    /// keeps a `Vec<PrunableItem>` in state).
    pub fn compute_items(items: &[crate::models::PrunableItem]) -> Self {
        let mut buckets: BTreeMap<String, Vec<&crate::models::PrunableItem>> = BTreeMap::new();
        for item in items {
            buckets.entry(item.source.clone()).or_default().push(item);
        }
        let mut out: Vec<DashboardRow> = Vec::with_capacity(buckets.len());
        for (source, group) in buckets {
            let total_bytes: u64 = group.iter().map(|i| i.size_bytes).sum();
            let top = group
                .iter()
                .max_by_key(|i| i.size_bytes)
                .map(|i| DashboardTopItem {
                    id: i.id.clone(),
                    name: i.name.clone(),
                    size_bytes: i.size_bytes,
                });
            out.push(DashboardRow {
                source,
                count: group.len(),
                total_bytes,
                top,
            });
        }
        // ``total_bytes`` desc, then ``source`` asc as a
        // deterministic tie-breaker.
        out.sort_by(|a, b| {
            b.total_bytes
                .cmp(&a.total_bytes)
                .then_with(|| a.source.cmp(&b.source))
        });
        Self { rows: out }
    }

    /// Total bytes across every row.  Equivalent to
    /// ``rows.iter().map(|r| r.total_bytes).sum()`` but
    /// exposed as a method so callers do not need to know the
    /// field name.
    pub fn grand_total(&self) -> u64 {
        self.rows.iter().map(|r| r.total_bytes).sum()
    }

    /// Render the dashboard as a fixed-width text table.  The
    /// output uses ``format_size`` so columns line up at
    /// common terminal widths.  Empty dashboards produce an
    /// empty string (callers in the CLI/TUI add their own
    /// "no data" prelude if needed).
    pub fn format_text(&self) -> String {
        use crate::size::format_size;
        let mut out = String::new();
        if self.rows.is_empty() {
            return out;
        }
        let source_w = self
            .rows
            .iter()
            .map(|r| r.source.len())
            .max()
            .unwrap_or(6)
            .clamp(8, 20);
        let size_w = 9; // "999.9 GiB" is 10 chars; we right-align at 9
        // Fixed column width for the unconstrained "Top item"
        // cell so the divider line + per-row lines share the
        // same geometry.  See `truncate(s, TOP_CELL_WIDTH)` below.
        out.push_str(&format!(
            "{:<source_w$}  {:>6}  {:>size_w$}  {}\n",
            "Engine", "Items", "Total", "Top item",
            source_w = source_w,
            size_w = size_w,
        ));
        let divider_width = source_w + 2 + 6 + 2 + size_w + 2 + TOP_CELL_WIDTH;
        out.push_str(&format!("{}\n", "-".repeat(divider_width)));
        for row in &self.rows {
            let top_repr = match &row.top {
                Some(t) => format!(
                    "{} ({})",
                    t.name,
                    format_size(t.size_bytes as i64, true)
                ),
                None => "-".to_string(),
            };
            out.push_str(&format!(
                "{:<source_w$}  {:>6}  {:>size_w$}  {}\n",
                row.source,
                row.count,
                format_size(row.total_bytes as i64, true),
                truncate(&top_repr, TOP_CELL_WIDTH),
                source_w = source_w,
                size_w = size_w,
            ));
        }
        out
    }
}

/// Fixed width of the unconstrained "Top item" column in
/// [`Dashboard::format_text`].  Belt-and-braces shared with the
/// `truncate(...)` call so the divider line and the per-row
/// lines render at the same geometry.
pub(crate) const TOP_CELL_WIDTH: usize = 60;

/// Truncate ``s`` to at most ``max`` characters using an
/// ellipsis when necessary.  Used by [`Dashboard::format_text`]
/// for the ``Top item`` column so very long names do not break
/// the column layout.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
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
    /// Optional path to the persistent deletion log
    /// (`$XDG_DATA_HOME/systemprune/history.json`).  When set,
    /// every successful or engine-failed deletion performed
    /// through `delete_many` is appended to this file with
    /// 10 MB rotation per `more.md` §5.1.
    ///
    /// Items that the orchestrator refuses on its own
    /// (status Active, previously-failed, missing scanner)
    /// are *not* written to the history log: those are
    /// orchestrator-level refusals, not engine interactions.
    history_path: Option<PathBuf>,
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
            history_path: None,
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

    /// Builder-style: attach a persistent history-log path.
    /// When set, every successful or engine-failed deletion
    /// performed via `delete_many` is appended to this file
    /// with the §5.1 schema and 10 MB rotation.
    ///
    /// The path is *not* read at construction time; the
    /// orchestrator just remembers it.  The parent directory
    /// is created on first write.
    pub fn with_history(mut self, path: PathBuf) -> Self {
        self.history_path = Some(path);
        self
    }

    /// Attach a persistent history-log path to an
    /// already-constructed orchestrator.
    pub fn set_history(&mut self, path: PathBuf) {
        self.history_path = Some(path);
    }

    /// Return the configured history-log path, if any.
    pub fn history_path(&self) -> Option<&PathBuf> {
        self.history_path.as_ref()
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
        // Snapshot of history wiring.  Captured before any
        // async work so the receiver loop's buffer-push arm
        // does not have to re-check `self.history_path` on
        // every iteration.  The actual file write happens once
        // after the receiver loop completes (see below).
        let has_history = self.history_path.is_some();
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
        // Buffered history entries.  We build these per-task as
        // results arrive but persist them in a single batch after
        // the receiver loop so concurrent tasks do not race on the
        // load/mutate/save cycle that a per-task `append_to_file`
        // would trigger.
        let mut history_entries: Vec<HistoryEntry> = Vec::new();
        // Single timestamp shared by every entry produced by
        // this `delete_many` call.  This both documents the
        // burst more cleanly (one logical event -> one
        // timestamp) and avoids N syscalls.
        let now = SystemTime::now();

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
            if confirm && delete_errors.is_some_and(|m| m.contains_key(&item_key)) {
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
                    // Buffer the result for history.  We
                    // deliberately do *not* write to the file
                    // here: writing per-task would race with
                    // sibling tasks (each would load the same
                    // file, append their entry, and write
                    // back, potentially losing siblings'
                    // entries).  The single batched write
                    // happens after the receiver loop.
                    //
                    // Only *engine interactions* are
                    // recorded: refused items (Active /
                    // previously-failed / missing scanner)
                    // were filtered out at slot setup so
                    // they never enter `pending`.
                    if has_history {
                        history_entries.push(HistoryEntry::from_result(
                            &result.item,
                            result.success,
                            result
                                .error
                                .as_ref()
                                .and_then(|e| e.returncode),
                            now,
                        ));
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
                    // Cancellation can happen mid-engine-run.
                    // The engine was called but its result
                    // never reached us through the oneshot
                    // channel.  We buffer a "cancelled"
                    // history entry so the audit trail still
                    // records that an attempt happened --
                    // written in the same single batched
                    // pass as the Ok results above so concurrent
                    // siblings cannot race.
                    let cancel_result = DeleteResult {
                        item: original.clone(),
                        success: false,
                        error: Some(EngineError::new(
                            "scanner task was cancelled",
                            original.engine.as_str(),
                            vec![],
                            None,
                            "",
                        )),
                    };
                    if has_history {
                        history_entries.push(HistoryEntry::from_result(
                            &cancel_result.item,
                            cancel_result.success,
                            cancel_result
                                .error
                                .as_ref()
                                .and_then(|e| e.returncode),
                            now,
                        ));
                    }
                    slots[idx] = Some(cancel_result);
                }
            }
        }

        let results: Vec<DeleteResult> = slots
            .into_iter()
            .map(|o| o.expect("all slots filled"))
            .collect();
        // Persist the buffered history entries in a single load/
        // save cycle.  Done after the receiver loop so concurrent
        // engine tasks cannot race on the file.
        if let Some(path) = &self.history_path {
            if let Err(e) = History::append_many(
                path,
                &history_entries,
                DEFAULT_MAX_BYTES,
                DEFAULT_KEEP_FILES,
            ) {
                if let Some(log) = &self.log {
                    log.warn(format!("history write failed: {e}"));
                }
            }
        }
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
