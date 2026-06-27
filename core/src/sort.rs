//! Item sorting utilities shared by the GUI and TUI.
//!
//! Items are grouped by `Category` first (so the group
//! structure is preserved), then sorted **within** each
//! group by the active [`SortMode`].  The cross-category
//! order is unchanged -- it still follows the first-seen
//! order of the underlying scan, so the user sees the same
//! categories in the same order regardless of sort mode.
//!
//! `SortMode::Default` is a no-op (items keep their original
//! first-seen order) so the default UX is identical to the
//! pre-sort behaviour.

use crate::models::PrunableItem;

/// How items should be ordered within a category group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortMode {
    /// Original first-seen order from the scan.  This is the
    /// default and is a no-op for the sort helpers below.
    Default,
    /// Alphabetical by `name`, ascending (A -> Z).
    NameAsc,
    /// By `size_bytes`, largest first.  Most useful for
    /// cleanup: the biggest wins surface at the top.
    SizeDesc,
    /// By `size_bytes`, smallest first.
    SizeAsc,
}

impl SortMode {
    /// Human-friendly label for use in dropdowns, menus, and
    /// status bars.  Kept short so it fits in a TUI status line.
    pub fn label(&self) -> &'static str {
        match self {
            SortMode::Default => "Default order",
            SortMode::NameAsc => "Name (A-Z)",
            SortMode::SizeDesc => "Size (largest first)",
            SortMode::SizeAsc => "Size (smallest first)",
        }
    }

    /// Short label for compact UI surfaces (e.g. the TUI
    /// status bar).  Falls back to the full label for modes
    /// that don't have a recognised short form.
    pub fn short_label(&self) -> &'static str {
        match self {
            SortMode::Default => "default",
            SortMode::NameAsc => "name",
            SortMode::SizeDesc => "size desc",
            SortMode::SizeAsc => "size asc",
        }
    }

    /// Cycle to the next sort mode.  Used by the TUI's `s`
    /// key binding so the user can step through modes without
    /// a menu.
    pub fn next(self) -> Self {
        match self {
            SortMode::Default => SortMode::NameAsc,
            SortMode::NameAsc => SortMode::SizeDesc,
            SortMode::SizeDesc => SortMode::SizeAsc,
            SortMode::SizeAsc => SortMode::Default,
        }
    }

    /// All sort modes in the cycle order used by [`next`].
    /// Useful for building a `gtk::DropDown` model or a TUI
    /// menu without duplicating the cycle order.
    pub fn all() -> [SortMode; 4] {
        [
            SortMode::Default,
            SortMode::NameAsc,
            SortMode::SizeDesc,
            SortMode::SizeAsc,
        ]
    }
}

/// Sort `items` **in place** according to `mode`.
///
/// `SortMode::Default` is a no-op so the default UX is
/// identical to the pre-sort behaviour.  All other modes use
/// a stable sort, so items with equal sort keys keep their
/// relative order (e.g. two items with the same name stay in
/// the order they were scanned).
pub fn sort_items(items: &mut [PrunableItem], mode: SortMode) {
    match mode {
        SortMode::Default => {}
        SortMode::NameAsc => {
            items.sort_by(|a, b| a.name.cmp(&b.name));
        }
        SortMode::SizeDesc => {
            items.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
        }
        SortMode::SizeAsc => {
            items.sort_by(|a, b| a.size_bytes.cmp(&b.size_bytes));
        }
    }
}

/// Convenience wrapper: sort a cloned `Vec` and return it,
/// leaving the caller's slice untouched.  Useful when the
/// caller cannot mutate the source (e.g. when iterating over
/// `&self.items` in a const context).
pub fn sorted_items(items: &[PrunableItem], mode: SortMode) -> Vec<PrunableItem> {
    let mut v = items.to_vec();
    sort_items(&mut v, mode);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Category, Engine, Status};
    use std::collections::BTreeMap;

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
        assert_eq!(ids, vec!["1", "2", "3"], "stable sort must preserve input order on ties");
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
}
