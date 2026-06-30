//! Integration tests for the item sorting helpers.
//!
//! These exercise the public surface (`SortMode::{label, short_label,
//! next, all}`, `sort_items`, `sorted_items`) against fixture
//! `PrunableItem` values built by hand so the assertions are
//! stable across `mod tests` / `tests/` reorganisations.
//!
//! Replaces `core/src/sort.rs::tests`.  The local `item` helper
//! is re-declared here because integration tests live in their
//! own crate and cannot share private scope.

use std::collections::BTreeMap;
use systemprune_core::models::{Category, Engine, PrunableItem, Status};
use systemprune_core::sort::{sort_items, sorted_items, SortMode};

fn item(name: &str, size: u64) -> PrunableItem {
    PrunableItem {
        id: name.to_string(),
        name: name.to_string(),
        engine: Engine::Docker,
        source: "docker".to_string(),
        category: Category::Image,
        size_bytes: size,
        status: Status::Unused,
        extra: BTreeMap::new(),
    }
}

#[test]
fn default_mode_is_noop() {
    let original = vec![item("c", 30), item("a", 10), item("b", 20)];
    let mut v = original.clone();
    sort_items(&mut v, SortMode::Default);
    assert_eq!(v, original, "Default must not reorder items");
}

#[test]
fn name_asc_sorts_alphabetically() {
    let mut v = vec![item("charlie", 30), item("alpha", 10), item("bravo", 20)];
    sort_items(&mut v, SortMode::NameAsc);
    let names: Vec<&str> = v.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
}

#[test]
fn size_desc_sorts_largest_first() {
    let mut v = vec![item("small", 10), item("huge", 1000), item("medium", 100)];
    sort_items(&mut v, SortMode::SizeDesc);
    let sizes: Vec<u64> = v.iter().map(|i| i.size_bytes).collect();
    assert_eq!(sizes, vec![1000, 100, 10]);
}

#[test]
fn size_asc_sorts_smallest_first() {
    let mut v = vec![item("huge", 1000), item("small", 10), item("medium", 100)];
    sort_items(&mut v, SortMode::SizeAsc);
    let sizes: Vec<u64> = v.iter().map(|i| i.size_bytes).collect();
    assert_eq!(sizes, vec![10, 100, 1000]);
}

#[test]
fn sort_is_stable_for_equal_keys() {
    // Three items with the same name.  Stable sort keeps
    // their original order.  We use different `id` fields
    // so equality is purely on `name`.
    let mut v = vec![
        PrunableItem {
            id: "1".into(),
            name: "same".into(),
            engine: Engine::Docker,
            source: "docker".into(),
            category: Category::Image,
            size_bytes: 10,
            status: Status::Unused,
            extra: BTreeMap::new(),
        },
        PrunableItem {
            id: "2".into(),
            name: "same".into(),
            engine: Engine::Docker,
            source: "docker".into(),
            category: Category::Image,
            size_bytes: 20,
            status: Status::Unused,
            extra: BTreeMap::new(),
        },
        PrunableItem {
            id: "3".into(),
            name: "same".into(),
            engine: Engine::Docker,
            source: "docker".into(),
            category: Category::Image,
            size_bytes: 30,
            status: Status::Unused,
            extra: BTreeMap::new(),
        },
    ];
    sort_items(&mut v, SortMode::NameAsc);
    let ids: Vec<&str> = v.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["1", "2", "3"],
        "stable sort must preserve input order on ties"
    );
}

#[test]
fn next_cycles_through_all_modes() {
    assert_eq!(SortMode::Default.next(), SortMode::NameAsc);
    assert_eq!(SortMode::NameAsc.next(), SortMode::SizeDesc);
    assert_eq!(SortMode::SizeDesc.next(), SortMode::SizeAsc);
    assert_eq!(SortMode::SizeAsc.next(), SortMode::Default);
}

#[test]
fn all_returns_modes_in_cycle_order() {
    assert_eq!(
        SortMode::all(),
        [
            SortMode::Default,
            SortMode::NameAsc,
            SortMode::SizeDesc,
            SortMode::SizeAsc,
        ]
    );
}

#[test]
fn label_and_short_label_are_nonempty_for_all_modes() {
    for mode in SortMode::all() {
        assert!(!mode.label().is_empty());
        assert!(!mode.short_label().is_empty());
    }
}

#[test]
fn sorted_items_returns_a_new_vec_without_mutating_input() {
    let original = vec![item("c", 30), item("a", 10), item("b", 20)];
    let sorted = sorted_items(&original, SortMode::NameAsc);
    assert_eq!(
        original.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        vec!["c", "a", "b"],
        "input must be untouched"
    );
    assert_eq!(
        sorted.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
}
