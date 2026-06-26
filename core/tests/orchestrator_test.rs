//! Tests for the Orchestrator with stub scanners.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use systemprune_core::errors::EngineError;
use systemprune_core::models::{Category, Engine, PrunableItem, Status};
use systemprune_core::orchestrator::{Orchestrator, ScanResult};
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
    let results = orch.delete_many(&[active, safe], true).await;
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
    let results = orch.delete_many(&[safe, bad], false).await;
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
    let results = orch.delete_many(&[item], false).await;
    assert!(!results[0].success);
    assert!(results[0].error.is_some());
}
