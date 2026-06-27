//! Adwaita desktop GUI for SystemPrune.
//!
//! Layout:
//!   * Header bar with Rescan / Delete Selected buttons
//!   * Horizontal split: left = engine list, right = items grouped
//!     by `Category`. Each group is an `adw::ExpanderRow` inside an
//!     `adw::PreferencesGroup`.
//!   * Status bar at the bottom

use adw::prelude::*;
use adw::{ActionRow, ExpanderRow, HeaderBar, PreferencesGroup};
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
    window.set_titlebar(Some(&header));

    // --- Outer vertical box: body + status ---
    let outer = GtkBox::new(Orientation::Vertical, 0);
    window.set_child(Some(&outer));

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
    /// Cached group summary labels so per-item toggle handlers can
    /// update the "[sel/total]" display without a full rebuild.
    group_summary_labels: BTreeMap<Category, Label>,
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
            group_summary_labels: BTreeMap::new(),
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
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let orch = state.borrow().orchestrator.clone();
        rt.block_on(orch.scan_all())
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
    status.set_text(&format!("Deleting {} item(s)\u{2026}", to_delete.len()));

    let results = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let orch = state.borrow().orchestrator.clone();
        rt.block_on(orch.delete_many(&to_delete, true))
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
    status.set_text(&format!("Deleted {}, failed {}.", ok, fail));
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
            .title(&src)
            .subtitle(&format!("{} item(s)", count))
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
        s.group_summary_labels.clear();
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
    // --- PreferencesGroup for this category ---
    let group = PreferencesGroup::new();
    group.set_title(cat.plural_label());
    group.set_description(Some(&format!(
        "{} item{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    )));

    // --- Summary label and select-all button in the header suffix ---
    let summary = Label::new(Some("[0/0]"));
    summary.set_xalign(1.0);
    let select_all_btn = Button::with_label("Select all");
    select_all_btn.set_tooltip_text(Some("Toggle all safe-to-delete items in this group"));
    {
        let state = state.clone();
        select_all_btn.connect_clicked(move |_| {
            on_select_all_clicked(&state, cat);
        });
    }

    // --- ExpanderRow for the group ---
    let expander_row = ExpanderRow::new();
    expander_row.set_title(cat.plural_label());
    expander_row.set_subtitle(&format!(
        "{} item{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    ));

    // --- Add items directly as rows of the ExpanderRow ---
    for item in items {
        let row = make_item_row(state, item, cat);
        expander_row.add_row(&row);
    }
    expander_row.set_expanded(true);

    group.add(&expander_row);
    groups_box.append(&group);

    // --- Cache widgets ---
    {
        let mut s = state.borrow_mut();
        s.group_summary_labels.insert(cat, summary.clone());
        s.group_expander_rows.insert(cat, expander_row.clone());
    }
    update_group_summary(state, cat);
}

fn make_item_row(
    state: &Rc<RefCell<State>>,
    item: &PrunableItem,
    cat: Category,
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
            on_item_toggled(&state, cb.is_active(), &item_source, &item_id, cat);
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
        .title(&item.name)
        .subtitle(&item.source)
        .activatable(false)
        .build();
    row.add_prefix(&checkbox);
    row.add_suffix(&status_label);
    row.add_suffix(&size_label);

    state.borrow_mut().item_checkboxes.insert(key, checkbox);

    row
}

fn update_group_summary(state: &Rc<RefCell<State>>, cat: Category) {
    let s = state.borrow();
    let label = match s.group_summary_labels.get(&cat) {
        Some(l) => l,
        None => return,
    };
    let in_group: Vec<&PrunableItem> =
        s.items.iter().filter(|i| i.category == cat).collect();
    let safe = in_group.iter().filter(|i| i.is_safe_to_delete()).count();
    let sel = in_group
        .iter()
        .filter(|i| {
            i.is_safe_to_delete()
                && s.selected.contains(&(i.source.clone(), i.id.clone()))
        })
        .count();
    if safe == 0 {
        label.set_text("[0 safe]");
    } else {
        label.set_text(&format!("[{} / {}]", sel, safe));
    }
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

fn on_item_toggled(
    state: &Rc<RefCell<State>>,
    active: bool,
    source: &str,
    id: &str,
    cat: Category,
) {
    if state.borrow().rebuilding {
        return;
    }
    let key = (source.to_string(), id.to_string());
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
    drop(s);
    update_group_summary(state, cat);
}

fn on_select_all_clicked(state: &Rc<RefCell<State>>, cat: Category) {
    let safe_keys: Vec<(String, String)> = {
        let s = state.borrow();
        s.items
            .iter()
            .filter(|i| i.category == cat && i.is_safe_to_delete())
            .map(|i| (i.source.clone(), i.id.clone()))
            .collect()
    };
    if safe_keys.is_empty() {
        return;
    }
    let all_selected = {
        let s = state.borrow();
        safe_keys.iter().all(|k| s.selected.contains(k))
    };
    {
        let mut s = state.borrow_mut();
        if all_selected {
            for k in &safe_keys {
                s.selected.remove(k);
            }
        } else {
            for k in &safe_keys {
                s.selected.insert(k.clone());
            }
        }
    }
    {
        let s = state.borrow();
        for k in &safe_keys {
            if let Some(cb) = s.item_checkboxes.get(k) {
                let should_be_active = s.selected.contains(k);
                if cb.is_active() != should_be_active {
                    cb.set_active(should_be_active);
                }
            }
        }
    }
    update_group_summary(state, cat);
}
