//! Tests for the unified data model.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use systemprune_core::models::{Category, Engine, PrunableItem, Status};

#[test]
fn is_safe_to_delete_false_for_active() {
    let item = PrunableItem {
        id: "abc".into(),
        name: "x".into(),
        engine: Engine::Docker,
        source: "docker".into(),
        category: Category::Image,
        size_bytes: 1024,
        status: Status::Active,
        extra: BTreeMap::new(),
    };
    assert!(!item.is_safe_to_delete());
}

#[test]
fn is_safe_to_delete_true_for_non_active() {
    for status in [Status::Stopped, Status::Dangling, Status::Unused] {
        let item = PrunableItem {
            id: "abc".into(),
            name: "x".into(),
            engine: Engine::Docker,
            source: "docker".into(),
            category: Category::Image,
            size_bytes: 0,
            status,
            extra: BTreeMap::new(),
        };
        assert!(item.is_safe_to_delete(), "status {:?} should be safe", status);
    }
}

#[test]
fn engine_label_and_value() {
    assert_eq!(Engine::Docker.label(), "Docker");
    assert_eq!(Engine::Docker.as_str(), "docker");
    assert_eq!(Engine::Ollama.as_str(), "ollama");
}

#[test]
fn category_label_handles_underscores() {
    assert_eq!(Category::BuildCache.as_str(), "build_cache");
    assert_eq!(Category::SnapRevision.as_str(), "snap_revision");
}

#[test]
fn category_plural_label_is_human_friendly() {
    assert_eq!(Category::Image.plural_label(), "Docker Images");
    assert_eq!(Category::Container.plural_label(), "Docker Containers");
    assert_eq!(Category::Volume.plural_label(), "Docker Volumes");
    assert_eq!(Category::Network.plural_label(), "Docker Networks");
    assert_eq!(Category::BuildCache.plural_label(), "Build caches");
    assert_eq!(Category::SnapRevision.plural_label(), "Snap revisions");
    assert_eq!(Category::Model.plural_label(), "Ollama Models");
    assert_eq!(Category::App.plural_label(), "Flatpak Apps");
    assert_eq!(Category::Runtime.plural_label(), "Flatpak Runtimes");
    assert_eq!(Category::Other.plural_label(), "Other");
}

#[test]
fn category_ord_works_for_btreemap_key() {
    // Exercises the PartialOrd/Ord derives that we added so the UIs
    // can put `Category` into a `BTreeMap` (Rust GUI does this for
    // caching group summary labels).
    let mut map: BTreeMap<Category, u32> = BTreeMap::new();
    map.insert(Category::Network, 1);
    map.insert(Category::Image, 2);
    map.insert(Category::Container, 3);
    let keys: Vec<Category> = map.keys().copied().collect();
    // `BTreeMap` iterates in key order, so the categories are
    // sorted by their derived `Ord`.
    assert_eq!(
        keys,
        vec![Category::Image, Category::Container, Category::Network]
    );
}

#[test]
fn category_ord_works_for_btreeset() {
    let mut set: BTreeSet<Category> = BTreeSet::new();
    set.insert(Category::Other);
    set.insert(Category::Image);
    set.insert(Category::Model);
    let ordered: Vec<Category> = set.into_iter().collect();
    assert_eq!(
        ordered,
        vec![Category::Image, Category::Model, Category::Other]
    );
}

#[test]
fn category_works_in_hashset() {
    let mut set: HashSet<Category> = HashSet::new();
    set.insert(Category::Image);
    set.insert(Category::Image);
    assert_eq!(set.len(), 1);
    assert!(set.contains(&Category::Image));
}

#[test]
fn status_serde_round_trip() {
    let item = PrunableItem {
        id: "abc".into(),
        name: "x".into(),
        engine: Engine::Docker,
        source: "docker".into(),
        category: Category::Image,
        size_bytes: 1024,
        status: Status::Dangling,
        extra: BTreeMap::new(),
    };
    let json = serde_json::to_string(&item).unwrap();
    let parsed: PrunableItem = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.status, Status::Dangling);
    assert_eq!(parsed.size_bytes, 1024);
}
