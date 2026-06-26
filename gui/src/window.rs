//! GTK4 desktop GUI for SystemPrune.
//!
//! Layout:
//!   * Header bar with Rescan / Delete Selected buttons
//!   * Horizontal split: left = engine list, right = items grouped
//!     by `Category`. Each group is a `gtk::Expander` with a
//!     "Select all" button in the label widget.
//!   * Status bar at the bottom
//!
//! Uses the simpler `gtk::ListBox` + `gtk::CheckButton` + `gtk::Label`
//! widget set rather than `TreeView` / `TreeModel` / `CellRenderer`
//! (most of which is deprecated since GTK 4.10).

use gtk::prelude::*;
use gtk::{
    Box as GtkBox, Button, CheckButton, Expander, Frame, HeaderBar, Label, ListBox,
    ListBoxRow, Orientation, ScrolledWindow, Separator, Window,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use systemprune_core::models::{Category, PrunableItem};
use systemprune_core::orchestrator::Orchestrator;
use systemprune_core::scanners::all_scanners;
use systemprune_core::size::format_size;

/// Build and present the main application window.
pub fn build_window(app: &gtk::Application) {
    let window = Window::builder()
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
    let engines_frame = Frame::new(Some("Engines"));
    engines_frame.set_size_request(200, -1);
    let engines_scroll = ScrolledWindow::new();
    engines_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    engines_scroll.set_vscrollbar_policy(gtk::PolicyType::Automatic);
    let engines_list = ListBox::new();
    engines_list.set_selection_mode(gtk::SelectionMode::Single);
    engines_scroll.set_child(Some(&engines_list));
    engines_frame.set_child(Some(&engines_scroll));
    main_box.append(&engines_frame);

    // --- Groups list (right): one expander per category ---
    let items_scroll = ScrolledWindow::new();
    items_scroll.set_hexpand(true);
    items_scroll.set_vexpand(true);
    let groups_list = ListBox::new();
    groups_list.set_selection_mode(gtk::SelectionMode::None);
    items_scroll.set_child(Some(&groups_list));
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
        let groups_list = groups_list.clone();
        rescan_button.connect_clicked(move |_| {
            do_scan(
                &state,
                &status_label,
                &engines_list,
                &groups_list,
            );
        });
    }
    {
        let state = state.clone();
        let status_label = status.clone();
        let engines_list = engines_list.clone();
        let groups_list = groups_list.clone();
        delete_button.connect_clicked(move |_| {
            do_delete(
                &state,
                &status_label,
                &engines_list,
                &groups_list,
            );
        });
    }

    // First scan.
    {
        let state = state.clone();
        let status_label = status.clone();
        let engines_list = engines_list.clone();
        let groups_list = groups_list.clone();
        window.connect_show(move |_| {
            do_scan(
                &state,
                &status_label,
                &engines_list,
                &groups_list,
            );
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
    /// True while populating widgets during a rebuild. The
    /// per-item toggle handler ignores events while this is set
    /// so the programmatic ``set_active`` calls don't fire a
    /// storm of redundant handler invocations.
    rebuilding: bool,
    /// Cached group summary labels so per-item toggle handlers can
    /// update the "[sel/total]" display without a full rebuild.
    group_summary_labels: BTreeMap<Category, Label>,
    /// Cached per-category expander so we can find them by category.
    group_expanders: BTreeMap<Category, Expander>,
    /// Cached per-category inner ListBox of item rows, keyed by
    /// `(source, id)` -> CheckButton.
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
            group_expanders: BTreeMap::new(),
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
    groups_list: &ListBox,
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
    rebuild_groups(state, groups_list);
    refresh_engines(state, engines_list);
    status.set_text(&format!("Found {} item(s).", count));
}

fn do_delete(
    state: &Rc<RefCell<State>>,
    status: &Label,
    engines_list: &ListBox,
    groups_list: &ListBox,
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
        // Keep everything except successfully-deleted items; failed
        // items remain in the view so the user can see what went
        // wrong.
        s.items.retain(|i| {
            !results
                .iter()
                .any(|r| r.success && r.item.source == i.source && r.item.id == i.id)
        });
        s.busy = false;
    }
    rebuild_groups(state, groups_list);
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
        let label = Label::new(Some(&format!("{} ({})", src, count)));
        label.set_xalign(0.0);
        label.set_margin_start(8);
        label.set_margin_end(8);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        let row = ListBoxRow::new();
        row.set_child(Some(&label));
        engines_list.append(&row);
    }
}

fn rebuild_groups(state: &Rc<RefCell<State>>, groups_list: &ListBox) {
    // Mark the state as "rebuilding" so per-item toggle handlers
    // ignore the programmatic ``set_active`` calls made during
    // population.
    state.borrow_mut().rebuilding = true;
    // Clear existing rows.
    while let Some(row) = groups_list.row_at_index(0) {
        groups_list.remove(&row);
    }
    // Reset caches.
    {
        let mut s = state.borrow_mut();
        s.group_summary_labels.clear();
        s.group_expanders.clear();
        s.item_checkboxes.clear();
    }
    // Snapshot the grouped items under a short-lived borrow so the
    // builder closures don't hold the RefCell lock.
    let snapshot: Vec<(Category, Vec<PrunableItem>)> = state.borrow().grouped();
    for (cat, items) in snapshot {
        append_group(state, groups_list, cat, &items);
    }
    state.borrow_mut().rebuilding = false;
}

fn append_group(
    state: &Rc<RefCell<State>>,
    groups_list: &ListBox,
    cat: Category,
    items: &[PrunableItem],
) {
    let expander = Expander::new(None);

    // --- Header label widget: title, [sel/total], Select all button ---
    let header_box = GtkBox::new(Orientation::Horizontal, 8);
    header_box.set_margin_start(4);
    header_box.set_margin_end(4);

    let title = Label::new(None);
    title.set_markup(&format!(
        "<b>{}</b>  ({} item{})",
        cat.plural_label(),
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    ));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    header_box.append(&title);

    let summary = Label::new(Some("[0/0]"));
    summary.set_xalign(1.0);
    header_box.append(&summary);

    let select_all_btn = Button::with_label("Select all");
    select_all_btn.set_tooltip_text(Some("Toggle all safe-to-delete items in this group"));
    {
        let state = state.clone();
        select_all_btn.connect_clicked(move |_| {
            on_select_all_clicked(&state, cat);
        });
    }
    header_box.append(&select_all_btn);

    expander.set_label_widget(Some(&header_box));

    // --- Inner list of items ---
    let inner_list = ListBox::new();
    inner_list.set_selection_mode(gtk::SelectionMode::None);
    for item in items {
        let row = make_item_row(state, item, cat);
        inner_list.append(&row);
    }
    expander.set_child(Some(&inner_list));
    expander.set_expanded(true);

    // --- Wrap the expander in a ListBoxRow and append ---
    let list_row = ListBoxRow::new();
    list_row.set_child(Some(&expander));
    groups_list.append(&list_row);

    // --- Cache widgets and refresh the summary label ---
    {
        let mut s = state.borrow_mut();
        s.group_summary_labels.insert(cat, summary.clone());
        s.group_expanders.insert(cat, expander.clone());
    }
    update_group_summary(state, cat);
}

fn make_item_row(
    state: &Rc<RefCell<State>>,
    item: &PrunableItem,
    cat: Category,
) -> ListBoxRow {
    let row = ListBoxRow::new();

    // Horizontal box: [checkbox] [source] [status] [size] [name]
    let hbox = GtkBox::new(Orientation::Horizontal, 12);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(2);
    hbox.set_margin_bottom(2);

    // Checkbox.
    let key = (item.source.clone(), item.id.clone());
    let initially_selected = {
        let s = state.borrow();
        s.selected.contains(&key) && item.is_safe_to_delete()
    };
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
    hbox.append(&checkbox);

    // Source.
    let source_label = Label::new(Some(&item.source));
    source_label.set_width_chars(10);
    source_label.set_xalign(0.0);
    hbox.append(&source_label);

    // Status.
    let status_label = Label::new(Some(item.status.as_str()));
    status_label.set_width_chars(10);
    status_label.set_xalign(0.0);
    hbox.append(&status_label);

    // Size.
    let size_label = Label::new(Some(&format_size(item.size_bytes as i64, true)));
    size_label.set_width_chars(10);
    size_label.set_xalign(1.0);
    hbox.append(&size_label);

    // Name.
    let name_label = Label::new(Some(&item.name));
    name_label.set_xalign(0.0);
    name_label.set_hexpand(true);
    name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    hbox.append(&name_label);

    row.set_child(Some(&hbox));

    // Cache the checkbox for later "select all" updates.
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
    // Suppress events fired by the programmatic ``set_active`` calls
    // we make while populating widgets.
    if state.borrow().rebuilding {
        return;
    }
    let key = (source.to_string(), id.to_string());
    let mut s = state.borrow_mut();
    // Verify the item is still present and safe to delete; if it has
    // disappeared or become unsafe, silently no-op (the checkbox
    // should have been disabled by the rebuild anyway).
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
    // Collect safe keys in the group under a short-lived borrow.
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
    // Update each row's checkbox state to reflect the new selection.
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
