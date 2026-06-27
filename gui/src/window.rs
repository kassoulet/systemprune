//! Adwaita desktop GUI for SystemPrune.
//!
//! Layout:
//!   * Header bar with Rescan / Delete Selected buttons
//!   * Horizontal split: left = engine list, right = items grouped
//!     by `Category`. Each group is an `adw::ExpanderRow` inside an
//!     `adw::PreferencesGroup`.
//!   * Status bar at the bottom

use adw::prelude::*;
use adw::{ActionRow, ExpanderRow, HeaderBar, ToolbarView};
use gtk::{
    Box as GtkBox, Button, CheckButton, Label, ListBox, Orientation,
    ScrolledWindow, Separator,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use systemprune_core::models::{Category, PrunableItem};
use systemprune_core::orchestrator::Orchestrator;
use systemprune_core::scanners::all_scanners;
use systemprune_core::size::format_size;

/// Build and present the main application window.
pub fn build_window(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("SystemPrune")
        .default_width(960)
        .default_height(600)
        .build();

    let orchestrator = Orchestrator::new(all_scanners());
    let state = Rc::new(RefCell::new(State::new(orchestrator)));

    // --- Header bar ---
    let header = HeaderBar::new();
    let rescan_button = Button::from_icon_name("view-refresh-symbolic");
    rescan_button.set_tooltip_text(Some("Rescan"));
    let delete_button = Button::with_label("Delete Selected");
    delete_button.set_tooltip_text(Some("Delete selected items"));
    header.pack_start(&rescan_button);
    header.pack_end(&delete_button);

    // --- ToolbarView wraps header + content ---
    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);

    // --- Outer vertical box: body + status ---
    let outer = GtkBox::new(Orientation::Vertical, 0);
    toolbar_view.set_content(Some(&outer));
    window.set_content(Some(&toolbar_view));

    // --- Main horizontal split: engines + items ---
    let main_box = GtkBox::new(Orientation::Horizontal, 0);
    main_box.set_vexpand(true);
    main_box.set_hexpand(true);
    outer.append(&main_box);

    // --- Engines sidebar (left) ---
    let engines_scroll = ScrolledWindow::new();
    engines_scroll.set_size_request(200, -1);
    engines_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    engines_scroll.set_vscrollbar_policy(gtk::PolicyType::Automatic);
    let engines_list = ListBox::new();
    engines_list.set_selection_mode(gtk::SelectionMode::Single);
    engines_list.set_css_classes(&["navigation-sidebar"]);
    engines_scroll.set_child(Some(&engines_list));
    main_box.append(&engines_scroll);

    // --- Separator between sidebar and content ---
    let vsep = Separator::new(Orientation::Vertical);
    main_box.append(&vsep);

    // --- Groups list (right): one expander row per category ---
    let items_scroll = ScrolledWindow::new();
    items_scroll.set_hexpand(true);
    items_scroll.set_vexpand(true);
    let groups_box = GtkBox::new(Orientation::Vertical, 0);
    items_scroll.set_child(Some(&groups_box));
    main_box.append(&items_scroll);

    // --- Status bar ---
    outer.append(&Separator::new(Orientation::Horizontal));
    let status = Label::new(Some("Ready."));
    status.set_xalign(0.0);
    status.set_margin_start(8);
    status.set_margin_end(8);
    status.set_margin_top(4);
    status.set_margin_bottom(4);
    outer.append(&status);

    // --- Wire up button handlers ---
    {
        let state = state.clone();
        let status_label = status.clone();
        let engines_list = engines_list.clone();
        let groups_box = groups_box.clone();
        rescan_button.connect_clicked(move |_| {
            do_scan(&state, &status_label, &engines_list, &groups_box);
        });
    }
    {
        let state = state.clone();
        let status_label = status.clone();
        let engines_list = engines_list.clone();
        let groups_box = groups_box.clone();
        delete_button.connect_clicked(move |_| {
            do_delete(&state, &status_label, &engines_list, &groups_box);
        });
    }

    // First scan.
    {
        let state = state.clone();
        let status_label = status.clone();
        let engines_list = engines_list.clone();
        let groups_box = groups_box.clone();
        window.connect_show(move |_| {
            do_scan(&state, &status_label, &engines_list, &groups_box);
        });
    }

    window.present();
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct State {
    orchestrator: Orchestrator,
    items: Vec<PrunableItem>,
    selected: HashSet<(String, String)>,
    busy: bool,
    /// True while populating widgets during a rebuild.
    rebuilding: bool,
    /// Reusable Tokio runtime for scan/delete operations.
    runtime: tokio::runtime::Runtime,
    /// Cached per-category expander rows.
    group_expander_rows: BTreeMap<Category, ExpanderRow>,
    /// Per-item checkbox, keyed by `(source, id)`.
    item_checkboxes: BTreeMap<(String, String), CheckButton>,
}

impl State {
    fn new(orchestrator: Orchestrator) -> Self {
        Self {
            orchestrator,
            items: Vec::new(),
            selected: HashSet::new(),
            busy: false,
            rebuilding: false,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime"),
            group_expander_rows: BTreeMap::new(),
            item_checkboxes: BTreeMap::new(),
        }
    }

    /// Items grouped by category, preserving first-seen order.
    fn grouped(&self) -> Vec<(Category, Vec<PrunableItem>)> {
        let mut order: Vec<Category> = Vec::new();
        let mut buckets: BTreeMap<Category, Vec<PrunableItem>> = BTreeMap::new();
        for item in &self.items {
            if !buckets.contains_key(&item.category) {
                order.push(item.category);
            }
            buckets.entry(item.category).or_default().push(item.clone());
        }
        order.into_iter().map(|c| (c, buckets[&c].clone())).collect()
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn do_scan(
    state: &Rc<RefCell<State>>,
    status: &Label,
    engines_list: &ListBox,
    groups_box: &GtkBox,
) {
    if state.borrow().busy {
        return;
    }
    state.borrow_mut().busy = true;
    status.set_text("Scanning\u{2026}");

    let result = {
        let orch = state.borrow().orchestrator.clone();
        state.borrow().runtime.block_on(orch.scan_all())
    };
    let count = result.items.len();
    {
        let mut s = state.borrow_mut();
        s.items = result.items;
        s.busy = false;
    }
    rebuild_groups(state, groups_box);
    refresh_engines(state, engines_list);
    status.set_text(&format!("Found {} item(s).", count));
}

fn do_delete(
    state: &Rc<RefCell<State>>,
    status: &Label,
    engines_list: &ListBox,
    groups_box: &GtkBox,
) {
    if state.borrow().busy {
        return;
    }
    let to_delete: Vec<PrunableItem> = state
        .borrow()
        .items
        .iter()
        .filter(|i| {
            i.is_safe_to_delete()
                && state
                    .borrow()
                    .selected
                    .contains(&(i.source.clone(), i.id.clone()))
        })
        .cloned()
        .collect();
    if to_delete.is_empty() {
        status.set_text("Nothing selected.");
        return;
    }
    state.borrow_mut().busy = true;
    let total_size: i64 = to_delete.iter().map(|i| i.size_bytes as i64).sum();
    status.set_text(&format!(
        "Deleting {} item(s) ({})\u{2026}",
        to_delete.len(),
        format_size(total_size, true)
    ));

    let results = {
        let orch = state.borrow().orchestrator.clone();
        state
            .borrow()
            .runtime
            .block_on(orch.delete_many(&to_delete, true))
    };
    let ok = results.iter().filter(|r| r.success).count();
    let fail = results.len() - ok;
    {
        let mut s = state.borrow_mut();
        for r in &results {
            if r.success {
                s.selected.remove(&(r.item.source.clone(), r.item.id.clone()));
            }
        }
        s.items.retain(|i| {
            !results
                .iter()
                .any(|r| r.success && r.item.source == i.source && r.item.id == i.id)
        });
        s.busy = false;
    }
    rebuild_groups(state, groups_box);
    refresh_engines(state, engines_list);
    let freed: i64 = results.iter().filter(|r| r.success).map(|r| r.item.size_bytes as i64).sum();
    status.set_text(&format!(
        "Deleted {}, failed {}. Freed {}.",
        ok,
        fail,
        format_size(freed, true)
    ));
}

// ---------------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------------

fn refresh_engines(state: &Rc<RefCell<State>>, engines_list: &ListBox) {
    while let Some(row) = engines_list.row_at_index(0) {
        engines_list.remove(&row);
    }
    let s = state.borrow();
    for src in s.orchestrator.available_engines() {
        let count = s.items.iter().filter(|i| i.source == src).count();
        let row = ActionRow::builder()
            .title(escape_markup(&src))
            .subtitle(format!("{} item(s)", count))
            .activatable(false)
            .build();
        engines_list.append(&row);
    }
}

fn rebuild_groups(state: &Rc<RefCell<State>>, groups_box: &GtkBox) {
    state.borrow_mut().rebuilding = true;
    // Clear existing children.
    while let Some(child) = groups_box.first_child() {
        groups_box.remove(&child);
    }
    {
        let mut s = state.borrow_mut();
        s.group_expander_rows.clear();
        s.item_checkboxes.clear();
    }
    let snapshot: Vec<(Category, Vec<PrunableItem>)> = state.borrow().grouped();
    for (cat, items) in snapshot {
        append_group(state, groups_box, cat, &items);
    }
    state.borrow_mut().rebuilding = false;
}

fn append_group(
    state: &Rc<RefCell<State>>,
    groups_box: &GtkBox,
    cat: Category,
    items: &[PrunableItem],
) {
    let total_size: i64 = items.iter().map(|i| i.size_bytes as i64).sum();
    let sel_size: i64 = items
        .iter()
        .filter(|i| {
            i.is_safe_to_delete()
                && state
                    .borrow()
                    .selected
                    .contains(&(i.source.clone(), i.id.clone()))
        })
        .map(|i| i.size_bytes as i64)
        .sum();
    // --- ExpanderRow for the group ---
    let expander_row = ExpanderRow::new();
    expander_row.set_title(cat.plural_label());
    let subtitle = if sel_size > 0 {
        format!(
            "{} item{}, {} to delete",
            items.len(),
            if items.len() == 1 { "" } else { "s" },
            format_size(sel_size, true)
        )
    } else {
        format!(
            "{} item{}, {}",
            items.len(),
            if items.len() == 1 { "" } else { "s" },
            format_size(total_size, true)
        )
    };
    expander_row.set_subtitle(&escape_markup(&subtitle));
    expander_row.set_expanded(true);

    // --- Add items directly as rows of the ExpanderRow ---
    for item in items {
        let row = make_item_row(state, item);
        expander_row.add_row(&row);
    }

    groups_box.append(&expander_row);

    // --- Cache widgets ---
    {
        let mut s = state.borrow_mut();
        s.group_expander_rows.insert(cat, expander_row.clone());
    }
}

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

fn make_item_row(
    state: &Rc<RefCell<State>>,
    item: &PrunableItem,
) -> ActionRow {
    let key = (item.source.clone(), item.id.clone());
    let initially_selected = {
        let s = state.borrow();
        s.selected.contains(&key) && item.is_safe_to_delete()
    };

    // --- Checkbox for selection ---
    let checkbox = CheckButton::new();
    checkbox.set_active(initially_selected);
    checkbox.set_sensitive(item.is_safe_to_delete());
    {
        let state = state.clone();
        let item_source = item.source.clone();
        let item_id = item.id.clone();
        checkbox.connect_toggled(move |cb| {
            on_item_toggled(&state, cb.is_active(), &item_source, &item_id);
        });
    }

    // --- Size label as suffix ---
    let size_label = Label::new(Some(&format_size(item.size_bytes as i64, true)));
    size_label.set_xalign(1.0);
    size_label.set_margin_end(4);

    // --- Status as suffix ---
    let status_label = Label::new(Some(item.status.as_str()));
    status_label.set_xalign(0.0);
    status_label.set_width_chars(8);
    status_label.set_margin_end(8);

    // --- ActionRow ---
    let row = ActionRow::builder()
        .title(escape_markup(&item.name))
        .subtitle(escape_markup(&item.source))
        .activatable(false)
        .build();
    row.add_prefix(&checkbox);
    row.add_suffix(&status_label);
    row.add_suffix(&size_label);

    // --- Add tooltip with full path if available ---
    if let Some(path) = item.extra.get("path") {
        row.set_tooltip_text(Some(path));
    } else if let Some(root) = item.extra.get("project_root") {
        row.set_tooltip_text(Some(root));
    }

    state.borrow_mut().item_checkboxes.insert(key, checkbox);

    row
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

fn on_item_toggled(
    state: &Rc<RefCell<State>>,
    active: bool,
    source: &str,
    id: &str,
) {
    if state.borrow().rebuilding {
        return;
    }
    let key = (source.to_string(), id.to_string());
    // Find the category of this item before mutating state.
    let category = state
        .borrow()
        .items
        .iter()
        .find(|i| i.source == source && i.id == id)
        .map(|i| i.category);
    {
        let mut s = state.borrow_mut();
        let present_and_safe = s
            .items
            .iter()
            .any(|i| i.source == source && i.id == id && i.is_safe_to_delete());
        if !present_and_safe {
            return;
        }
        if active {
            s.selected.insert(key);
        } else {
            s.selected.remove(&key);
        }
    }
    // Update the ExpanderRow subtitle for this item's category.
    if let Some(cat) = category {
        update_group_subtitle(state, cat);
    }
}

/// Recompute and set the subtitle for a category's ExpanderRow.
fn update_group_subtitle(state: &Rc<RefCell<State>>, cat: Category) {
    let (subtitle, expander) = {
        let s = state.borrow();
        let items: Vec<&PrunableItem> = s.items.iter().filter(|i| i.category == cat).collect();
        let total_size: i64 = items.iter().map(|i| i.size_bytes as i64).sum();
        let sel_size: i64 = items
            .iter()
            .filter(|i| {
                i.is_safe_to_delete()
                    && s.selected.contains(&(i.source.clone(), i.id.clone()))
            })
            .map(|i| i.size_bytes as i64)
            .sum();
        let text = if sel_size > 0 {
            format!(
                "{} item{}, {} to delete",
                items.len(),
                if items.len() == 1 { "" } else { "s" },
                format_size(sel_size, true)
            )
        } else {
            format!(
                "{} item{}, {}",
                items.len(),
                if items.len() == 1 { "" } else { "s" },
                format_size(total_size, true)
            )
        };
        let expander = s.group_expander_rows.get(&cat).cloned();
        (text, expander)
    };
    if let Some(e) = expander {
        e.set_subtitle(&escape_markup(&subtitle));
    }
}
