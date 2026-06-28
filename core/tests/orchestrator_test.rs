//! Tests for the Orchestrator with stub scanners.

use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use systemprune_core::errors::EngineError;
use systemprune_core::models::{Category, Engine, PrunableItem, Status};
use systemprune_core::orchestrator::{Dashboard, Orchestrator, ScanResult};
use systemprune_core::scanners::Scanner;

/// Shared counter for the set of item ids the stub was asked to delete.
/// Living outside the stub itself makes the assertions easy from the
/// test body without needing a downcast.
#[derive(Default)]
struct DeleteCounter(Mutex<Vec<String>>);

struct StubScanner {
    source: &'static str,
    engine: Engine,
    items: Vec<PrunableItem>,
    available: bool,
    delete_raises: bool,
    counter: Arc<DeleteCounter>,
}

#[async_trait]
impl Scanner for StubScanner {
    fn source(&self) -> &'static str {
        self.source
    }
    fn engine(&self) -> Engine {
        self.engine
    }
    fn binary(&self) -> &'static str {
        self.source
    }
    fn is_available(&self) -> bool {
        self.available
    }
    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError> {
        Ok(self.items.clone())
    }
    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError> {
        self.counter.0.lock().unwrap().push(item.id.clone());
        if self.delete_raises {
            return Err(EngineError::new(
                "stub failure",
                self.source,
                vec![],
                Some(1),
                "boom",
            ));
        }
        Ok(())
    }
}

fn image(id: &str, status: Status) -> PrunableItem {
    PrunableItem {
        id: id.to_string(),
        name: id.to_string(),
        engine: Engine::Docker,
        source: "docker".to_string(),
        category: Category::Image,
        size_bytes: 1024,
        status,
        extra: Default::default(),
    }
}

fn make_stub(
    source: &'static str,
    engine: Engine,
    available: bool,
    delete_raises: bool,
    counter: Arc<DeleteCounter>,
) -> Arc<dyn Scanner> {
    Arc::new(StubScanner {
        source,
        engine,
        items: vec![],
        available,
        delete_raises,
        counter,
    })
}

#[tokio::test]
async fn filters_unavailable_scanners() {
    let a = make_stub("a", Engine::Docker, true, false, Arc::new(DeleteCounter::default()));
    let b = make_stub("b", Engine::Ollama, false, false, Arc::new(DeleteCounter::default()));
    let orch = Orchestrator::new(vec![a, b]);
    assert_eq!(orch.available_engines(), vec!["a".to_string()]);
    assert_eq!(orch.all_scanners().len(), 2);
}

#[tokio::test]
async fn scan_all_collects_items_and_groups_by_engine() {
    let items = vec![
        image("1", Status::Unused),
        image("2", Status::Unused),
        PrunableItem {
            id: "m1".into(),
            name: "m1".into(),
            engine: Engine::Ollama,
            source: "ollama".into(),
            category: Category::Model,
            size_bytes: 4000,
            status: Status::Unused,
            extra: Default::default(),
        },
    ];
    let docker_items = items[0..2].to_vec();
    let ollama_items = vec![items[2].clone()];
    let docker: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "docker",
        engine: Engine::Docker,
        items: docker_items,
        available: true,
        delete_raises: false,
        counter: Arc::new(DeleteCounter::default()),
    });
    let ollama: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "ollama",
        engine: Engine::Ollama,
        items: ollama_items,
        available: true,
        delete_raises: false,
        counter: Arc::new(DeleteCounter::default()),
    });
    let orch = Orchestrator::new(vec![docker, ollama]);
    let result = orch.scan_all().await;
    assert_eq!(result.items.len(), 3);
    assert_eq!(result.total_bytes(), 1024 + 1024 + 4000);
    let grouped = result.by_engine();
    assert!(grouped.contains_key("docker"));
    assert!(grouped.contains_key("ollama"));
}

#[tokio::test]
async fn scan_result_groups_items_by_category() {
    // Three items of two different categories from two different
    // engines. `by_category` must preserve first-seen order of
    // categories, and keep the items in their original order within
    // each bucket.
    let items = vec![
        image("1", Status::Unused),
        PrunableItem {
            id: "v1".into(),
            name: "v1".into(),
            engine: Engine::Docker,
            source: "docker".into(),
            category: Category::Volume,
            size_bytes: 100,
            status: Status::Unused,
            extra: Default::default(),
        },
        image("2", Status::Unused),
        PrunableItem {
            id: "m1".into(),
            name: "m1".into(),
            engine: Engine::Ollama,
            source: "ollama".into(),
            category: Category::Model,
            size_bytes: 4000,
            status: Status::Unused,
            extra: Default::default(),
        },
    ];
    let result = ScanResult {
        items,
        errors: Vec::new(),
    };
    let grouped = result.by_category();
    let cats: Vec<Category> = grouped.iter().map(|(c, _)| *c).collect();
    assert_eq!(cats, vec![Category::Image, Category::Volume, Category::Model]);
    assert_eq!(grouped[0].1.len(), 2); // two images
    assert_eq!(grouped[1].1.len(), 1); // one volume
    assert_eq!(grouped[2].1.len(), 1); // one model
}

#[tokio::test]
async fn delete_many_skips_active_by_default() {
    let active = image("act", Status::Active);
    let safe = image("ok", Status::Unused);
    let counter = Arc::new(DeleteCounter::default());
    let docker: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "docker",
        engine: Engine::Docker,
        items: vec![],
        available: true,
        delete_raises: false,
        counter: counter.clone(),
    });
    let orch = Orchestrator::new(vec![docker]);
    let results = orch.delete_many(&[active, safe], true, None).await;
    assert_eq!(results.len(), 2);
    let successes: Vec<bool> = results.iter().map(|r| r.success).collect();
    assert_eq!(successes, vec![false, true]);
    let calls = counter.0.lock().unwrap();
    assert_eq!(*calls, vec!["ok".to_string()]);
}

#[tokio::test]
async fn delete_many_reports_per_item_failure() {
    let safe = image("ok", Status::Unused);
    let bad = image("bad", Status::Unused);
    let counter = Arc::new(DeleteCounter::default());
    let docker: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "docker",
        engine: Engine::Docker,
        items: vec![],
        available: true,
        delete_raises: true,
        counter: counter.clone(),
    });
    let orch = Orchestrator::new(vec![docker]);
    let results = orch.delete_many(&[safe, bad], false, None).await;
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(!r.success);
        assert!(r.error.is_some());
    }
    // The scanner was called for both items, but both failed.
    let calls = counter.0.lock().unwrap();
    assert_eq!(*calls, vec!["ok".to_string(), "bad".to_string()]);
}

#[tokio::test]
async fn delete_many_mixed_results_carry_per_item_engine_error() {
    // One scanner that fails every call; one that succeeds.  Each
    // owns an item.  The orchestrator must report per-item
    // success/failure and copy the scanner's `EngineError` into the
    // failing `DeleteResult` so the UI can surface it to the user.
    let ok = image("ok", Status::Unused);
    let bad = PrunableItem {
        id: "bad".into(),
        name: "bad".into(),
        engine: Engine::Ollama,
        source: "ollama".into(),
        category: Category::Model,
        size_bytes: 4096,
        status: Status::Unused,
        extra: Default::default(),
    };
    let docker_counter = Arc::new(DeleteCounter::default());
    let ollama_counter = Arc::new(DeleteCounter::default());
    let docker: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "docker",
        engine: Engine::Docker,
        items: vec![],
        available: true,
        delete_raises: false,
        counter: docker_counter.clone(),
    });
    let ollama: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "ollama",
        engine: Engine::Ollama,
        items: vec![],
        available: true,
        delete_raises: true,
        counter: ollama_counter.clone(),
    });
    let orch = Orchestrator::new(vec![docker, ollama]);

    let results = orch.delete_many(&[ok, bad], false, None).await;
    assert_eq!(results.len(), 2);

    // Order must match the input order.
    assert!(results[0].success);
    assert!(results[0].error.is_none());
    assert_eq!(results[0].item.id, "ok");
    // The successful item is reported in its pre-delete form so the
    // UI can flip `Status::Deleted` on its own copy.
    assert_eq!(results[0].item.status, Status::Unused);

    assert!(!results[1].success);
    let err = results[1]
        .error
        .as_ref()
        .expect("failed delete carries an EngineError");
    assert_eq!(err.engine, "ollama");
    assert_eq!(err.returncode, Some(1));
    assert!(err.message.contains("stub failure"));
    // The original input is preserved; the orchestrator does not
    // mutate it.
    assert_eq!(results[1].item.id, "bad");
    assert_eq!(results[1].item.status, Status::Unused);

    // Both scanners were actually invoked.
    let docker_calls = docker_counter.0.lock().unwrap();
    let ollama_calls = ollama_counter.0.lock().unwrap();
    assert_eq!(*docker_calls, vec!["ok".to_string()]);
    assert_eq!(*ollama_calls, vec!["bad".to_string()]);
}

#[tokio::test]
async fn delete_many_no_scanner_for_source() {
    let item = PrunableItem {
        id: "orphan".into(),
        name: "o".into(),
        engine: Engine::Docker,
        source: "gone".into(),
        category: Category::Image,
        size_bytes: 1,
        status: Status::Unused,
        extra: Default::default(),
    };
    let docker: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "docker",
        engine: Engine::Docker,
        items: vec![],
        available: true,
        delete_raises: false,
        counter: Arc::new(DeleteCounter::default()),
    });
    let orch = Orchestrator::new(vec![docker]);
    let results = orch.delete_many(&[item], false, None).await;
    assert!(!results[0].success);
    assert!(results[0].error.is_some());
}

#[tokio::test]
async fn delete_many_rejects_items_in_delete_errors_map() {
    // Defence-in-depth: a failed item that somehow slips past the
    // UI's `is_deletable_for_real` filter (future code path, test
    // injection, programmatic caller) must still be rejected by the
    // orchestrator.  The scanner should never be invoked.
    let item = image("bad", Status::Unused);
    let counter = Arc::new(DeleteCounter::default());
    let docker: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "docker",
        engine: Engine::Docker,
        items: vec![],
        available: true,
        // The stub would *succeed* if called; the rejection must
        // come from the orchestrator's delete_errors check, not the
        // scanner.
        delete_raises: false,
        counter: counter.clone(),
    });
    let orch = Orchestrator::new(vec![docker]);
    let mut delete_errors = BTreeMap::new();
    delete_errors.insert(
        ("docker".to_string(), "bad".to_string()),
        "permission denied".to_string(),
    );

    let results = orch
        .delete_many(&[item], true, Some(&delete_errors))
        .await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].success);
    let err = results[0]
        .error
        .as_ref()
        .expect("rejected item carries an EngineError");
    assert!(err.message.contains("previously failed"));
    assert_eq!(err.engine, "docker");
    // The original input is preserved.
    assert_eq!(results[0].item.id, "bad");
    assert_eq!(results[0].item.status, Status::Unused);
    // The scanner was *not* called for the rejected item.
    let calls = counter.0.lock().unwrap();
    assert!(calls.is_empty(), "scanner must not be invoked for rejected items");
}

#[tokio::test]
async fn delete_many_delete_errors_only_blocks_matching_keys() {
    // An item with the same source but a different id, or a
    // different source with the same id, must not be blocked.
    let matched = image("bad", Status::Unused);
    let same_source = image("other", Status::Unused);
    let different_source = PrunableItem {
        id: "bad".into(),
        name: "bad".into(),
        engine: Engine::Ollama,
        source: "ollama".into(),
        category: Category::Model,
        size_bytes: 1024,
        status: Status::Unused,
        extra: Default::default(),
    };
    let counter = Arc::new(DeleteCounter::default());
    let docker: Arc<dyn Scanner> = Arc::new(StubScanner {
        source: "docker",
        engine: Engine::Docker,
        items: vec![],
        available: true,
        delete_raises: false,
        counter: counter.clone(),
    });
    let orch = Orchestrator::new(vec![docker]);
    let mut delete_errors = BTreeMap::new();
    delete_errors.insert(
        ("docker".to_string(), "bad".to_string()),
        "boom".to_string(),
    );

    let results = orch
        .delete_many(&[matched, same_source, different_source], true, Some(&delete_errors))
        .await;
    assert_eq!(results.len(), 3);
    // `matched` is blocked: its `(docker, "bad")` key matches an
    // entry in `delete_errors` exactly.
    assert!(!results[0].success);
    assert!(results[0].error.as_ref().unwrap().message.contains("previously failed"));
    // `same_source` is docker with id="other" \u2014 not in
    // `delete_errors`, so the tie-breaker does not fire. The
    // orchestrator dispatches it to the docker scanner, which
    // succeeds (`delete_raises=false`).
    assert!(results[1].success);
    assert!(results[1].error.is_none());
    // `different_source` is ollama with id="bad" \u2014 not in
    // `delete_errors` either, but the orchestrator has no ollama
    // scanner, so it falls through to the "No active scanner" path.
    assert!(!results[2].success);
    assert!(results[2].error.as_ref().unwrap().message.contains("No active scanner"));
    // The scanner was invoked exactly once, for the non-blocked
    // docker item.
    let calls = counter.0.lock().unwrap();
    assert_eq!(*calls, vec!["other".to_string()]);
}

// ---------------------------------------------------------------------------
// Dashboard tests (more.md §4.1).
// ---------------------------------------------------------------------------
//
// These tests pin the contract of `Dashboard::compute` and
// `Dashboard::format_text` so the rendering surface (CLI,
// TUI, GUI) cannot drift from one another.

/// Convenience factory: a docker image with the given id, name,
/// size, and status.  Keeps the `Dashboard` tests focused on
/// grouping/sorting behaviour rather than boilerplate.
fn docker_image(
    id: &str,
    name: &str,
    size_bytes: u64,
    status: Status,
) -> PrunableItem {
    PrunableItem {
        id: id.to_string(),
        name: name.to_string(),
        engine: Engine::Docker,
        source: "docker".to_string(),
        category: Category::Image,
        size_bytes,
        status,
        extra: Default::default(),
    }
}

fn ollama_model(
    id: &str,
    name: &str,
    size_bytes: u64,
    status: Status,
) -> PrunableItem {
    PrunableItem {
        id: id.to_string(),
        name: name.to_string(),
        engine: Engine::Ollama,
        source: "ollama".to_string(),
        category: Category::Model,
        size_bytes,
        status,
        extra: Default::default(),
    }
}

#[test]
fn dashboard_compute_empty_scan_is_empty() {
    let scan = ScanResult::default();
    let dash = Dashboard::compute(&scan);
    assert!(dash.rows.is_empty());
    assert_eq!(dash.grand_total(), 0);
}

#[test]
fn dashboard_compute_single_engine_groups_all_items() {
    let items = vec![
        docker_image("a", "rust:bookworm", 4_000_000_000, Status::Unused),
        docker_image("b", "node:20",    2_000_000_000, Status::Unused),
        docker_image("c", "alpine:3",     100_000_000, Status::Unused),
    ];
    let scan = ScanResult { items, errors: vec![] };
    let dash = Dashboard::compute(&scan);
    assert_eq!(dash.rows.len(), 1);
    let row = &dash.rows[0];
    assert_eq!(row.source, "docker");
    assert_eq!(row.count, 3);
    assert_eq!(row.total_bytes, 6_100_000_000);
    let top = row.top.as_ref().expect("dashboard row with items has a top item");
    assert_eq!(top.name, "rust:bookworm");
    assert_eq!(top.size_bytes, 4_000_000_000);
    assert_eq!(top.id, "a");
}

#[test]
fn dashboard_compute_sorts_rows_by_total_desc_with_deterministic_tiebreak() {
    // Three engines with different totals so the sort is fixed.
    // Then a fourth with the same total as the third to verify
    // the source-name tie-breaker.
    let items = vec![
        docker_image("d1", "d1", 100, Status::Unused),
        ollama_model("o1", "o1", 9_900, Status::Unused),
        // `flatpak` and `snap` have equal totals -- `flatpak`
        // should come first alphabetically.
        PrunableItem {
            id: "f1".into(),
            name: "f1".into(),
            engine: Engine::Flatpak,
            source: "flatpak".into(),
            category: Category::App,
            size_bytes: 500,
            status: Status::Unused,
            extra: Default::default(),
        },
        PrunableItem {
            id: "s1".into(),
            name: "s1".into(),
            engine: Engine::Snap,
            source: "snap".into(),
            category: Category::SnapRevision,
            size_bytes: 500,
            status: Status::Unused,
            extra: Default::default(),
        },
    ];
    let scan = ScanResult { items, errors: vec![] };
    let dash = Dashboard::compute(&scan);
    let sources: Vec<&str> = dash.rows.iter().map(|r| r.source.as_str()).collect();
    // Descending totals: ollama (9.9K), flatpak (500), snap (500),
    // docker (100). flatpak precedes snap on the equal-total
    // tie-break because `f` < `s`.
    assert_eq!(sources, vec!["ollama", "flatpak", "snap", "docker"]);
}

#[test]
fn dashboard_compute_top_item_picks_largest_in_group() {
    let items = vec![
        docker_image("tiny",   "tiny:latest",        100, Status::Unused),
        docker_image("medium", "medium:latest",   50_000, Status::Unused),
        docker_image("big",    "big:latest", 2_000_000_000, Status::Unused),
        docker_image("med2",   "med2:latest",     70_000, Status::Unused),
    ];
    let scan = ScanResult { items, errors: vec![] };
    let dash = Dashboard::compute(&scan);
    let top = dash.rows[0].top.as_ref().expect("top item present");
    // "big" is the unique maximum so the result is unambiguous.
    assert_eq!(top.id, "big");
    assert_eq!(top.name, "big:latest");
    assert_eq!(top.size_bytes, 2_000_000_000);
}

#[test]
fn dashboard_compute_includes_active_items_in_count_and_total() {
    // The dashboard surfaces *every* item, regardless of status,
    // so a user can see how much space an active container is
    // occupying.  Deletion is gated by `PrunableItem::is_safe_to
    // _delete` elsewhere; the dashboard is a read-only snapshot.
    let items = vec![
        docker_image("a", "a", 100, Status::Unused),
        docker_image("b", "b", 200, Status::Active),
    ];
    let scan = ScanResult { items, errors: vec![] };
    let dash = Dashboard::compute(&scan);
    assert_eq!(dash.rows.len(), 1);
    assert_eq!(dash.rows[0].count, 2);
    assert_eq!(dash.rows[0].total_bytes, 300);
}

#[test]
fn dashboard_grand_total_matches_sum_across_rows() {
    let items = vec![
        docker_image("a", "a", 100, Status::Unused),
        ollama_model("o", "o", 200, Status::Unused),
    ];
    let scan = ScanResult { items, errors: vec![] };
    let dash = Dashboard::compute(&scan);
    assert_eq!(dash.grand_total(), 300);
}

#[test]
fn dashboard_format_text_on_empty_input_is_empty_string() {
    let dash = Dashboard::default();
    let text = dash.format_text();
    assert!(text.is_empty(), "empty dashboard -> empty text");
}

#[test]
fn dashboard_format_text_contains_header_and_each_row() {
    // Sizes chosen so `format_size(., binary=true)` produces
    // a clean integer GiB value at the chosen bucket.  The
    // raw `5_000_000_000` byte value would render as "4.7
    // GiB" (binary round-down), so the test uses exactly
    // `4 * 1024^3` for deterministic output.
    let four_gib = 4_u64 * 1024 * 1024 * 1024;
    let items = vec![
        docker_image("big", "big:latest", four_gib, Status::Unused),
        docker_image("sm",  "sm:latest",        100, Status::Unused),
    ];
    let scan = ScanResult { items, errors: vec![] };
    let text = Dashboard::compute(&scan).format_text();
    assert!(text.contains("Engine"), "header column 'Engine': {text}");
    assert!(text.contains("Items"), "header column 'Items': {text}");
    assert!(text.contains("Total"), "header column 'Total': {text}");
    assert!(text.contains("Top item"), "header column 'Top item': {text}");
    assert!(text.contains("docker"), "engine name appears: {text}");
    assert!(text.contains("2"), "count 2 appears: {text}");
    assert!(
        text.contains("big:latest"),
        "top item name appears: {text}"
    );
    assert!(
        text.contains("4.0 GiB"),
        "4.0 GiB appears in table: {text}"
    );
}

#[test]
fn dashboard_format_text_truncates_long_top_item_names() {
    // A very long name that would break the column layout.  The
    // truncation adds an ellipsis so the column stays tidy.
    let big_name = "a".repeat(200);
    let items = vec![docker_image("big", &big_name, 1_000, Status::Unused)];
    let scan = ScanResult { items, errors: vec![] };
    let text = Dashboard::compute(&scan).format_text();
    assert!(text.contains('\u{2026}'), "ellipsis marks truncation: {text}");
    assert!(
        !text.contains(&big_name),
        "full name was not truncated: {text}"
    );
}
