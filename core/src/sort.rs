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
