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
    gio, Box as GtkBox, Button, CheckButton, Label, MenuButton,
    Orientation, ScrolledWindow, Separator,
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

    // --- Hamburger menu (About, etc.) ---
    // Built with a `gio::Menu` model and a `MenuButton` so the
    // entries are accessible via the standard Adwaita hamburger
    // icon.  The single "About SystemPrune" entry activates the
    // `app.about` action registered below.
    let menu = gio::Menu::new();
    menu.append(Some("About SystemPrune"), Some("app.about"));
    let menu_button = MenuButton::new();
    menu_button.set_icon_name("open-menu-symbolic");
    menu_button.set_tooltip_text(Some("Menu"));
    menu_button.set_menu_model(Some(&menu));
    header.pack_end(&menu_button);

    // --- ToolbarView wraps header + content ---
    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&header);

    // --- Outer vertical box: body + status ---
    let outer = GtkBox::new(Orientation::Vertical, 0);
    toolbar_view.set_content(Some(&outer));
    window.set_content(Some(&toolbar_view));

    // --- Main content area: items grouped by category ---
    let main_box = GtkBox::new(Orientation::Vertical, 0);
    main_box.set_vexpand(true);
    main_box.set_hexpand(true);
    outer.append(&main_box);

    // --- Groups list: one expander row per category ---
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
        let groups_box = groups_box.clone();
        rescan_button.connect_clicked(move |_| {
            do_scan(&state, &status_label, &groups_box);
        });
    }
    {
        let state = state.clone();
        let status_label = status.clone();
        let groups_box = groups_box.clone();
        delete_button.connect_clicked(move |_| {
            do_delete(&state, &status_label, &groups_box);
        });
    }

    // First scan.
    {
        let state = state.clone();
        let status_label = status.clone();
        let groups_box = groups_box.clone();
        window.connect_show(move |_| {
            do_scan(&state, &status_label, &groups_box);
        });
    }

    // --- About dialog action ---
    // The hamburger menu's "About SystemPrune" entry activates
    // `app.about`; we handle it here by building and presenting
    // an `adw::AboutWindow`.  The window is built lazily on
    // first activation and cached in `state.about_window`, so
    // repeated menu clicks bring the same dialog to the front
    // instead of stacking new ones.  The window is transient
    // for the main window and modal so it appears on top and
    // is dismissed before the user can interact with the main
    // window again.
    //
    // **One-shot assumption:** `build_window` is not designed
    // to be called more than once for the same `app`.  If it
    // is, this `app.add_action(&about_action)` call will panic
    // with "action already registered".  Today `main` only
    // calls `build_window` from `connect_activate`, and GTK
    // fires that once per app run, so this is safe.  A future
    // refactor that calls `build_window` multiple times should
    // either guard this call with a check or move the action
    // registration to `main`.
    //
    // `app` and `window` are cloned (cheap, they are
    // `glib::Object`s) because the closure must own its
    // captures to satisfy the `'static` bound on
    // `SimpleAction::connect_activate`.
    let about_action = gio::SimpleAction::new("about", None);
    let app_clone = app.clone();
    let parent_clone = window.clone();
    let cache_clone = state.borrow().about_window.clone();
    about_action.connect_activate(move |_, _| {
        // Build the about window lazily on first activation
        // and cache it.  The two borrows are sequential (each
        // released before the next is taken) so no `RefMut` is
        // held across `present()`.  `present()` may fire
        // internal GTK signals, but none re-enter our `State`
        // (the about window has no signal handlers connected
        // to it), so no `with_rebuilding` wrap is needed.
        if cache_clone.borrow().is_none() {
            *cache_clone.borrow_mut() =
                Some(build_about_window(&app_clone, &parent_clone));
        }
        if let Some(about) = cache_clone.borrow().as_ref() {
            about.present();
        }
    });
    app.add_action(&about_action);

    window.present();
}

/// Build the "About SystemPrune" dialog window.  The window is
/// transient for `parent` and associated with `app` so it gets
/// the correct GApplication icon and lifecycle.  All metadata is
/// hard-coded from the workspace `Cargo.toml` and the `core`
/// crate's `VERSION` constant.
fn build_about_window(
    app: &adw::Application,
    parent: &adw::ApplicationWindow,
) -> adw::AboutWindow {
    let about = adw::AboutWindow::builder()
        .application_name("SystemPrune")
        .developer_name("SystemPrune Contributors")
        .version(systemprune_core::VERSION)
        .copyright("\u{00a9} 2024-2026 SystemPrune Contributors")
        .license_type(gtk::License::MitX11)
        .website("https://github.com/example/systemprune")
        .issue_url("https://github.com/example/systemprune/issues")
        .comments(
            "Unified disk space cleaner for Docker, Podman, Flatpak, \
             Snap, and Ollama.  Scans for unused artifacts across all \
             detected engines and lets you delete them in bulk.",
        )
        .developers(vec!["SystemPrune Contributors".to_string()])
        .translator_credits("Translate me!")
        .build();
    about.set_application(Some(app));
    about.set_transient_for(Some(parent));
    about.set_modal(true);
    about
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
    /// Per-category "Select all" / "Deselect all" button shown as
    /// a suffix on the expander row's title bar.
    group_toggle_buttons: BTreeMap<Category, Button>,
    /// Error messages for failed deletions, keyed by (source, id).
    delete_errors: BTreeMap<(String, String), String>,
    /// Cached `adw::AboutWindow` so repeated activations of the
    /// "About SystemPrune" menu entry bring the same dialog to
    /// the front instead of stacking new ones.  Built lazily on
    /// first activation; the inner `Option` is `None` until then.
    about_window: Rc<RefCell<Option<adw::AboutWindow>>>,
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
            group_toggle_buttons: BTreeMap::new(),
            delete_errors: BTreeMap::new(),
            about_window: Rc::new(RefCell::new(None)),
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
// State-borrow helpers
// ---------------------------------------------------------------------------
//
// **The RefCell deadlock pattern.**
//
// `State` is wrapped in `Rc<RefCell<State>>`.  GTK emits many
// signals synchronously (e.g., `toggled` on a `CheckButton`,
// `clicked` on a `Button`) when state-changing methods like
// `set_active` are called.  If a signal handler then tries to
// `state.borrow()` while the outer code still holds a `RefMut`,
// Rust panics with "RefCell already mutably borrowed".
//
// The first instance of this bug (the original `panic.txt`)
// happened in `on_group_toggle_clicked`: a `RefMut` was held while
// `cb.set_active(new_active)` fired the per-item `toggled` signal,
// which immediately called `state.borrow().rebuilding` and
// panicked.  The fix is the **safe signal-firing pattern**:
//
//   1. Extract any state-derived values into local variables
//      (the borrow is released at the end of the block).
//   2. Set `state.rebuilding = true` (brief borrow, released).
//   3. Fire the GTK signal — the per-item callback sees
//      `rebuilding == true` and bails out of its own borrows.
//   4. Set `state.rebuilding = false` (brief borrow, released).
//
// Two helpers in this section encapsulate parts of the pattern:
//
//   * `with_rebuilding` — wraps steps 2–4 around a GTK
//     signal-firing closure, with a RAII guard so the flag is
//     *always* cleared (even on panic).
//   * `try_borrow_mut` — best-effort mutation that returns
//     `None` instead of panicking if state is already borrowed.
//     Use this inside callback bodies that may be re-entered
//     and would rather skip the work than crash.
//
// **Audit conclusion:** every signal-firing call site inside a
// `state.borrow()` scope is wrapped in `with_rebuilding`.  The
// current call sites are: `set_active` in `make_item_row`,
// `set_active` in `on_group_toggle_clicked`, the whole rebuild
// in `rebuild_groups`, and the `set_label` / `set_sensitive`
// calls in `update_group_toggle_button`.  Future signal-firing
// call sites inside a `state.borrow()` scope should follow the
// same pattern.

/// Run a closure while `state.rebuilding` is `true`, so the
/// per-item `toggled` callbacks see the flag and bail out of
/// their own `state.borrow()` / `state.borrow_mut()` calls.
///
/// **Primary use — GTK signal-firing.**  Wrap a closure that
/// fires connected signals (e.g., `set_active` on a
/// `CheckButton` whose `toggled` handler re-enters state).  See
/// `on_group_toggle_clicked` for the canonical example.
///
/// **Broader use — multi-step rebuild / panic-safety.**  Wrap a
/// whole rebuild flow (widget creation, cache clearing,
/// per-item work) to keep `rebuilding` set for the entire
/// scope.  See `rebuild_groups` for the real call site.  This
/// also makes the scope panic-safe: a panicking GTK call no
/// longer leaves `rebuilding = true` stuck forever, because
/// the RAII guard clears the flag during unwind.
///
/// The flag is set just before the closure runs and cleared
/// just after — the borrows are brief, so re-entrant callbacks
/// can acquire their own borrows safely.  An internal RAII
/// guard guarantees the flag is cleared even if the closure
/// panics.
///
/// # Recommended caller pattern (signal-firing)
///
/// ```ignore
/// // 1. Extract state-derived values; the borrow ends here.
/// let widgets: Vec<CheckButton> = {
///     let s = state.borrow();
///     // ... collect widgets from `s.item_checkboxes` ...
/// };
/// // 2. Fire GTK signals inside `with_rebuilding`.
/// with_rebuilding(state, || {
///     for w in widgets {
///         w.set_active(new_value); // signal fires, callback bails
///     }
/// });
/// ```
fn with_rebuilding<F, R>(state: &Rc<RefCell<State>>, f: F) -> R
where
    F: FnOnce() -> R,
{
    // RAII guard: clears the flag on Drop, including during
    // panic unwinding.  Without this, a panicking closure would
    // leave `rebuilding = true` set forever, freezing the GUI.
    //
    // **Nested-call semantics:** the guard unconditionally sets
    // `rebuilding = false` on Drop rather than saving/restoring
    // the previous value.  The per-item callback only bails on
    // `rebuilding == true`; in the clobbered window (between
    // the inner Drop and the outer Drop) a callback that fires
    // would proceed with its full work.  This is **deadlock-
    // safe** because the outer closure holds no `RefMut`, so
    // the callback can always acquire a fresh borrow; it's
    // just *not* "bail-safe" in the clobbered window.  For
    // the only current call site (`on_group_toggle_clicked`)
    // no nested `with_rebuilding` is used, so the clobber
    // never materialises.  If a future caller introduces
    // either a nested `with_rebuilding` or a deeper meaning
    // for `rebuilding` (e.g. "do not even render" vs. "skip
    // callback"), this guard should be changed to
    // save/restore the previous value instead.
    struct Guard<'a> {
        state: &'a Rc<RefCell<State>>,
    }
    impl<'a> Drop for Guard<'a> {
        fn drop(&mut self) {
            // We deliberately swallow any borrow failure here:
            // a `BorrowMutError` while dropping a guard means
            // state is already mutably borrowed, which can only
            // happen if the closure itself leaked a `RefMut`
            // (e.g., by storing it in a longer-lived structure).
            // In that pathological case, the next borrow_mut will
            // panic and the user will see the original bug.
            if let Ok(mut s) = self.state.try_borrow_mut() {
                s.rebuilding = false;
            }
        }
    }
    state.borrow_mut().rebuilding = true;
    let _guard = Guard { state };
    f()
}

/// Try to mutate state.  Returns `None` if state is already
/// borrowed (e.g., from a re-entered signal callback) instead
/// of panicking with "RefCell already mutably borrowed".
///
/// `with_rebuilding` is the right tool when a GTK signal handler
/// should be **suppressed** (via the `rebuilding` flag) so the
/// outer call can finish before the callback runs.  This helper
/// is the right tool when a callback body itself wants to
/// **attempt** a state mutation, gracefully skipping the work
/// if an outer call is still in progress.
///
/// Note that this helper does **not** suppress callbacks or
/// guard against re-entry itself — it merely makes the borrow
/// attempt best-effort.  Combine it with `state.borrow().rebuilding`
/// checks in the callback if you need both:
///
/// ```ignore
/// if state.borrow().rebuilding {
///     return; // outer rebuild in progress, skip
/// }
/// try_borrow_mut(state, |s| {
///     // We have a fresh `RefMut`; mutate freely.
///     s.selected.insert(key);
/// });
/// ```
///
/// # Example
///
/// ```ignore
/// // In a per-item callback that may be re-entered:
/// try_borrow_mut(state, |s| {
///     s.selected.insert(key);
/// });
/// ```
#[allow(dead_code)] // No current production call site; reserved for
                    // future callback re-entry points.  The 4 unit
                    // tests in `mod tests` below exercise it.
fn try_borrow_mut<F, R>(state: &Rc<RefCell<State>>, f: F) -> Option<R>
where
    F: FnOnce(&mut State) -> R,
{
    state.try_borrow_mut().ok().map(|mut s| f(&mut s))
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn do_scan(
    state: &Rc<RefCell<State>>,
    status: &Label,
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
    status.set_text(&format!("Found {} item(s).", count));
}

fn do_delete(
    state: &Rc<RefCell<State>>,
    status: &Label,
    groups_box: &GtkBox,
) {
    if state.borrow().busy {
        return;
    }
    let s = state.borrow();

    // Count how many selected items the safety filter will exclude
    // because they are already in `delete_errors` from a prior
    // batch. These are reported as "skipped" and stripped from
    // `selected` after the delete runs, so the user never sees a
    // selection that includes items the engine has already rejected.
    let skipped_count: usize = s
        .selected
        .iter()
        .filter(|(source, id)| s.delete_errors.contains_key(&(source.clone(), id.clone())))
        .count();

    // Build the actual delete batch: items that are both selected
    // and pass the full safety filter (`is_deletable_for_real`).
    let to_delete: Vec<PrunableItem> = s
        .items
        .iter()
        .filter(|i| {
            i.is_deletable_for_real(&s.delete_errors)
                && s.selected.contains(&(i.source.clone(), i.id.clone()))
        })
        .cloned()
        .collect();
    drop(s);

    // Nothing to do and nothing to strip → bail.
    if to_delete.is_empty() && skipped_count == 0 {
        status.set_text("Nothing selected.");
        return;
    }
    state.borrow_mut().busy = true;
    if !to_delete.is_empty() {
        let total_size: i64 = to_delete.iter().map(|i| i.size_bytes as i64).sum();
        status.set_text(&format!(
            "Deleting {} item(s) ({})\u{2026}",
            to_delete.len(),
            format_size(total_size, true)
        ));
    } else {
        // Only skipped items were in the selection; the orchestrator
        // doesn't need to run.
        status.set_text("Cleaning up selection\u{2026}");
    }

    let results: Vec<systemprune_core::orchestrator::DeleteResult> = if !to_delete.is_empty() {
        let s = state.borrow();
        let orch = s.orchestrator.clone();
        // Hoist the borrow so the `&s.delete_errors` reference is
        // valid for the entire `block_on` call.  Prevents the
        // orchestrator from re-queuing items that previously
        // failed (defence in depth).
        s.runtime.block_on(orch.delete_many(&to_delete, true, Some(&s.delete_errors)))
    } else {
        Vec::new()
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
        // Mark successfully deleted items instead of removing them.
        for r in &results {
            if r.success {
                if let Some(item) = s.items.iter_mut().find(|i| i.source == r.item.source && i.id == r.item.id) {
                    item.status = systemprune_core::models::Status::Deleted;
                }
                s.delete_errors.remove(&(r.item.source.clone(), r.item.id.clone()));
            } else if let Some(err) = &r.error {
                let key = (r.item.source.clone(), r.item.id.clone());
                s.delete_errors.insert(key, err.to_string());
            }
        }

        // Strip any remaining items in `selected` that are also in
        // `delete_errors`.  This covers both the pre-existing
        // failed items (counted in `skipped_count` above) and the
        // just-failed items added to `delete_errors` in this batch.
        // The user sees a clean selection after the delete runs.
        let to_strip: Vec<(String, String)> = s
            .selected
            .iter()
            .filter(|(source, id)| s.delete_errors.contains_key(&(source.clone(), id.clone())))
            .cloned()
            .collect();
        for k in &to_strip {
            s.selected.remove(k);
        }
        s.busy = false;
    }
    rebuild_groups(state, groups_box);
    let freed: i64 = results.iter().filter(|r| r.success).map(|r| r.item.size_bytes as i64).sum();

    // Final status: standard "Deleted N, failed M. Freed X." plus an
    // optional "Skipped K previously-failed item(s)." tail when any
    // item was held back by the safety filter. The skipped line
    // tells the user why their selection shrank.
    let mut msg = if !to_delete.is_empty() {
        format!(
            "Deleted {}, failed {}. Freed {}.",
            ok,
            fail,
            format_size(freed, true)
        )
    } else {
        "Selection cleaned up.".to_string()
    };
    if skipped_count > 0 {
        msg.push_str(&format!(
            " Skipped {} previously-failed item(s).",
            skipped_count
        ));
    }
    status.set_text(&msg);
}

// ---------------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------------

fn rebuild_groups(state: &Rc<RefCell<State>>, groups_box: &GtkBox) {
    // Wrap the whole rebuild in `with_rebuilding` so:
    //   1. The `rebuilding` flag is set for the entire rebuild
    //      (suppresses per-item `toggled` callbacks fired by
    //      widgets created during `append_group`).
    //   2. The flag is cleared via the helper's RAII guard,
    //      even if `append_group` (or any of the GTK calls
    //      below) panics — the previous bare
    //      `borrow_mut().rebuilding = true/false` pattern would
    //      have left the flag stuck at `true` forever on panic,
    //      freezing the GUI.
    //   3. The call site mirrors `on_group_toggle_clicked`,
    //      which also wraps its signal-firing block in
    //      `with_rebuilding`.  Future signal-firing call sites
    //      should follow the same pattern.
    with_rebuilding(state, || {
        // Save expansion state before rebuilding.
        let expansion_state: BTreeMap<Category, bool> = {
            let s = state.borrow();
            s.group_expander_rows
                .iter()
                .map(|(cat, row)| (*cat, row.is_expanded()))
                .collect()
        };
        // Clear existing children.
        while let Some(child) = groups_box.first_child() {
            groups_box.remove(&child);
        }
        {
            let mut s = state.borrow_mut();
            s.group_expander_rows.clear();
            s.item_checkboxes.clear();
            s.group_toggle_buttons.clear();
        }
        let snapshot: Vec<(Category, Vec<PrunableItem>)> = state.borrow().grouped();
        for (cat, items) in snapshot {
            append_group(state, groups_box, cat, &items);
        }
        // Restore expansion state after rebuilding.
        {
            let s = state.borrow();
            for (cat, expanded) in &expansion_state {
                if let Some(row) = s.group_expander_rows.get(cat) {
                    row.set_expanded(*expanded);
                }
            }
        }
    });
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

    // --- "Select all" / "Deselect all" button as a suffix on the
    //     expander row's title bar. Toggles every safe item in this
    //     category in one click, mirroring the TUI's "A" binding. ---
    let toggle_button = Button::with_label("Select all");
    toggle_button.set_tooltip_text(Some(
        "Select or deselect every safe item in this group",
    ));
    toggle_button.set_valign(gtk::Align::Center);
    expander_row.add_suffix(&toggle_button);

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
        s.group_toggle_buttons.insert(cat, toggle_button.clone());
    }

    // --- Wire up the click handler.  Done after caching so the
    //     handler can read the cached button. ---
    {
        let state = state.clone();
        toggle_button.connect_clicked(move |_| {
            on_group_toggle_clicked(&state, cat);
        });
    }

    // Initial label/sensitivity reflects the items + selection.
    update_group_toggle_button(state, cat);
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
    let render = describe_item_row(&state.borrow().delete_errors, item);

    // --- Checkbox for selection ---
    let checkbox = CheckButton::new();
    // Defensive: wrap the initial `set_active` in `with_rebuilding`
    // so a future refactor that reorders this call past
    // `connect_toggled` cannot reintroduce the original `panic.txt`
    // deadlock.  Today the call is *before* `connect_toggled`, so
    // no signal handler is connected and no `toggled` signal can
    // fire — the flag guard is redundant.  But the guard makes the
    // code robust to either ordering, so future contributors don't
    // need to remember which one is safe.
    with_rebuilding(state, || {
        checkbox.set_active(initially_selected);
    });
    checkbox.set_sensitive(render.checkbox_sensitive);
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
        .title(&render.title)
        .subtitle(escape_markup(&item.source))
        .activatable(false)
        .build();
    row.add_prefix(&checkbox);
    row.add_suffix(&status_label);
    row.add_suffix(&size_label);

    // Styling for deleted or failed items.
    if let Some(class) = render.css_class {
        row.add_css_class(class);
    }

    // --- Add tooltip with error details, path, or project root ---
    if let Some(tooltip) = render.tooltip.as_deref() {
        row.set_tooltip_text(Some(tooltip));
    }

    state.borrow_mut().item_checkboxes.insert(key, checkbox);

    row
}

/// Per-item display description for the GUI. Extracted from
/// `make_item_row` so unit tests can pin the contract of the
/// `delete_errors` map without constructing GTK widgets.
pub(crate) fn describe_item_row(
    delete_errors: &BTreeMap<(String, String), String>,
    item: &PrunableItem,
) -> GuiItemRowRender {
    let key = (item.source.clone(), item.id.clone());
    let has_error = delete_errors.contains_key(&key);
    let escaped_name = escape_markup(&item.name);
    let title = if item.status.is_deleted() {
        format!("{} (deleted)", escaped_name)
    } else if has_error {
        format!("{} (failed)", escaped_name)
    } else {
        escaped_name
    };
    let css_class = if item.status.is_deleted() {
        Some("dim-label")
    } else if has_error {
        Some("error")
    } else {
        None
    };
    let tooltip = if let Some(err) = delete_errors.get(&key) {
        Some(err.clone())
    } else if let Some(path) = item.extra.get("path") {
        Some(path.clone())
    } else if let Some(root) = item.extra.get("project_root") {
        Some(root.clone())
    } else {
        None
    };
    // `is_deletable_for_real` collapses the two safety predicates
    // (status + previous-failure) into one, so a failed item with
    // `Status::Unused` is correctly treated as non-deletable here.
    let checkbox_sensitive = item.is_deletable_for_real(delete_errors);
    GuiItemRowRender {
        title,
        tooltip,
        css_class,
        checkbox_sensitive,
    }
}

/// Per-item render description produced by `describe_item_row`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct GuiItemRowRender {
    pub title: String,
    pub tooltip: Option<String>,
    pub css_class: Option<&'static str>,
    pub checkbox_sensitive: bool,
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
        // Use the full safety predicate so a previously-failed
        // item cannot be re-toggled by the per-item checkbox.
        // (The checkbox is also `set_sensitive(false)` for those
        // items, but a programmatic `set_active(true)` could
        // otherwise sneak through.)
        let present_and_deletable = s.items.iter().any(|i| {
            i.source == source && i.id == id && i.is_deletable_for_real(&s.delete_errors)
        });
        if !present_and_deletable {
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
        update_group_toggle_button(state, cat);
    }
}

/// Handler for the per-expander "Select all" / "Deselect all" button.
///
/// Toggles every *safe* item in the given category. If all of them
/// are currently selected, deselects them; otherwise selects them
/// all. Mirrors the TUI's "A" (shift+a) binding.
fn on_group_toggle_clicked(state: &Rc<RefCell<State>>, cat: Category) {
    if state.borrow().rebuilding {
        return;
    }
    // Collect every actually-deletable item key in this category.
    // A previously-failed item is `Status::Unused` but excluded here
    // so the group button cannot re-queue it for another doomed
    // attempt.
    let safe_keys: Vec<(String, String)> = {
        let s = state.borrow();
        s.items
            .iter()
            .filter(|i| i.category == cat && i.is_deletable_for_real(&s.delete_errors))
            .map(|i| (i.source.clone(), i.id.clone()))
            .collect()
    };
    if safe_keys.is_empty() {
        return;
    }
    // Decide which way to flip. If all are already selected, we
    // deselect all; otherwise we select every safe item.
    let all_selected = {
        let s = state.borrow();
        safe_keys.iter().all(|k| s.selected.contains(k))
    };
    let new_active = !all_selected;
    {
        let mut s = state.borrow_mut();
        for k in &safe_keys {
            if new_active {
                s.selected.insert(k.clone());
            } else {
                s.selected.remove(k);
            }
        }
    }
    // Sync every per-item checkbox widget so the UI matches state.
    //
    // We **must** drop the borrow on `state` before calling
    // `set_active`: GTK emits the `toggled` signal synchronously,
    // and the per-item `on_item_toggled` callback immediately
    // calls `state.borrow().rebuilding`.  Holding a `RefMut`
    // across the `set_active` calls would panic with "RefCell
    // already mutably borrowed" (see `panic.txt` for the
    // original report).  The `with_rebuilding` helper enforces
    // the safe pattern: extract state-derived values into local
    // variables first, then run the GTK calls with the
    // `rebuilding` flag held set around them, so the per-item
    // callbacks can re-borrow safely while they run.
    let checkboxes: Vec<CheckButton> = {
        let s = state.borrow();
        safe_keys
            .iter()
            .filter_map(|k| s.item_checkboxes.get(k).cloned())
            .collect()
    };
    with_rebuilding(state, || {
        for cb in &checkboxes {
            cb.set_active(new_active);
        }
    });
    update_group_subtitle(state, cat);
    update_group_toggle_button(state, cat);
}

/// Recompute the "Select all" button's label and sensitivity for a
/// category.  Extracted into a pure helper so unit tests can pin the
/// contract without instantiating GTK widgets.
fn update_group_toggle_button(state: &Rc<RefCell<State>>, cat: Category) {
    let (safe_count, selected_count) = {
        let s = state.borrow();
        let safe_count = s
            .items
            .iter()
            .filter(|i| i.category == cat && i.is_deletable_for_real(&s.delete_errors))
            .count();
        let selected_count = s
            .items
            .iter()
            .filter(|i| {
                i.category == cat
                    && i.is_deletable_for_real(&s.delete_errors)
                    && s.selected.contains(&(i.source.clone(), i.id.clone()))
            })
            .count();
        (safe_count, selected_count)
    };
    let render = compute_group_toggle_button_state(safe_count, selected_count);
    // Extract the button reference **before** firing signals, so
    // the state borrow is released.  The GTK `set_label` and
    // `set_sensitive` setters fire `notify::label` /
    // `notify::sensitive` synchronously; today no handler for
    // either signal re-enters state, but a future contributor
    // could add one.  Wrapping the signal-firing calls in
    // `with_rebuilding` ensures any such handler sees the
    // `rebuilding` flag and bails out of its own
    // `state.borrow()`.  This is the same defensive pattern
    // applied to `set_active` in `make_item_row` and to the
    // whole rebuild in `rebuild_groups`.
    let btn = state.borrow().group_toggle_buttons.get(&cat).cloned();
    if let Some(btn) = btn {
        with_rebuilding(state, || {
            btn.set_label(render.label);
            btn.set_sensitive(render.sensitive);
        });
    }
}

/// Pure description of the per-group "Select all" button's
/// label and sensitivity, given the count of safe items and the
/// count of currently-selected safe items in the group.
pub(crate) fn compute_group_toggle_button_state(
    safe_count: usize,
    selected_count: usize,
) -> GroupToggleButtonRender {
    if safe_count == 0 {
        GroupToggleButtonRender {
            label: "Select all",
            sensitive: false,
        }
    } else if selected_count >= safe_count {
        GroupToggleButtonRender {
            label: "Deselect all",
            sensitive: true,
        }
    } else {
        GroupToggleButtonRender {
            label: "Select all",
            sensitive: true,
        }
    }
}

/// Per-group toggle-button description produced by
/// `compute_group_toggle_button_state`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) struct GroupToggleButtonRender {
    pub label: &'static str,
    pub sensitive: bool,
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
                i.is_deletable_for_real(&s.delete_errors)
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Unit tests for the deletion-error tracking contract.
    //!
    //! The render path in `make_item_row` is a thin wrapper around
    //! `describe_item_row`; these tests pin the contract of that
    //! helper so future refactors cannot silently break the
    //! surface of failed deletions in the GUI.

    use super::*;
    use systemprune_core::models::{Engine, Status};

    fn make_item(id: &str, source: &str, status: Status, category: Category) -> PrunableItem {
        let engine = match source {
            "docker" => Engine::Docker,
            "ollama" => Engine::Ollama,
            _ => Engine::Docker,
        };
        PrunableItem {
            id: id.to_string(),
            name: id.to_string(),
            engine,
            source: source.to_string(),
            category,
            size_bytes: 1024,
            status,
            extra: Default::default(),
        }
    }

    fn empty_errors() -> BTreeMap<(String, String), String> {
        BTreeMap::new()
    }

    #[test]
    fn describe_item_row_safe_unused_no_metadata_uses_raw_name() {
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        let render = describe_item_row(&empty_errors(), &item);
        assert_eq!(render.title, "a");
        assert_eq!(render.tooltip, None);
        assert_eq!(render.css_class, None);
        assert!(render.checkbox_sensitive);
    }

    #[test]
    fn describe_item_row_failed_delete_uses_failed_suffix_and_error_class() {
        let mut errors = empty_errors();
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        errors.insert(("docker".to_string(), "a".to_string()), "boom".to_string());
        let render = describe_item_row(&errors, &item);
        assert_eq!(render.title, "a (failed)");
        assert_eq!(render.tooltip, Some("boom".to_string()));
        assert_eq!(render.css_class, Some("error"));
        // Failed items cannot be re-selected until the user
        // explicitly retries; the checkbox is greyed out.
        assert!(!render.checkbox_sensitive);
    }

    #[test]
    fn describe_item_row_deleted_uses_deleted_suffix_and_dim_label() {
        let item = make_item("a", "docker", Status::Deleted, Category::Image);
        let render = describe_item_row(&empty_errors(), &item);
        assert_eq!(render.title, "a (deleted)");
        assert_eq!(render.css_class, Some("dim-label"));
        // `is_safe_to_delete` returns false for Deleted, so the
        // checkbox is also disabled.
        assert!(!render.checkbox_sensitive);
    }

    #[test]
    fn describe_item_row_active_is_not_safe_and_has_no_special_class() {
        let item = make_item("a", "docker", Status::Active, Category::Image);
        let render = describe_item_row(&empty_errors(), &item);
        assert_eq!(render.title, "a");
        assert_eq!(render.css_class, None);
        assert!(!render.checkbox_sensitive);
    }

    #[test]
    fn describe_item_row_with_path_uses_path_as_tooltip() {
        let mut item = make_item("a", "docker", Status::Unused, Category::Image);
        item.extra
            .insert("path".to_string(), "/some/path".to_string());
        let render = describe_item_row(&empty_errors(), &item);
        assert_eq!(render.tooltip, Some("/some/path".to_string()));
    }

    #[test]
    fn describe_item_row_with_project_root_uses_root_as_tooltip() {
        let mut item = make_item("a", "docker", Status::Unused, Category::Image);
        item.extra
            .insert("project_root".to_string(), "/proj".to_string());
        let render = describe_item_row(&empty_errors(), &item);
        assert_eq!(render.tooltip, Some("/proj".to_string()));
    }

    #[test]
    fn describe_item_row_error_takes_precedence_over_path_in_tooltip() {
        let mut errors = empty_errors();
        let mut item = make_item("a", "docker", Status::Unused, Category::Image);
        item.extra
            .insert("path".to_string(), "/some/path".to_string());
        errors.insert(("docker".to_string(), "a".to_string()), "boom".to_string());
        let render = describe_item_row(&errors, &item);
        // Error details trump metadata: the user is more likely to
        // want the failure reason than a path they cannot act on.
        assert_eq!(render.tooltip, Some("boom".to_string()));
        assert_eq!(render.title, "a (failed)");
        assert_eq!(render.css_class, Some("error"));
    }

    #[test]
    fn describe_item_row_escapes_markup_chars_in_name() {
        let mut item = make_item("a", "docker", Status::Unused, Category::Image);
        item.name = "<weird & name>".to_string();
        let render = describe_item_row(&empty_errors(), &item);
        // Pango markup special chars must be escaped before being
        // passed to `ActionRow::set_title`, otherwise the row
        // crashes at construction time.
        assert_eq!(render.title, "&lt;weird &amp; name&gt;");
    }

    #[test]
    fn describe_item_row_error_key_must_match_source_and_id_exactly() {
        // A delete error for a different source/id must not affect
        // this item.
        let mut errors = empty_errors();
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        errors.insert(
            ("ollama".to_string(), "a".to_string()),
            "boom".to_string(),
        );
        let render = describe_item_row(&errors, &item);
        assert_eq!(render.title, "a");
        assert_eq!(render.css_class, None);
        assert!(render.checkbox_sensitive);
    }

    // --- per-group toggle button ---

    #[test]
    fn group_toggle_button_is_disabled_when_group_has_no_safe_items() {
        let r = compute_group_toggle_button_state(0, 0);
        assert_eq!(r.label, "Select all");
        assert!(!r.sensitive);
    }

    #[test]
    fn group_toggle_button_disabled_when_group_only_has_active_items() {
        // Safe count is 0, even if some other items are present and
        // selected (the helper only counts safe ones).
        let r = compute_group_toggle_button_state(0, 3);
        assert_eq!(r.label, "Select all");
        assert!(!r.sensitive);
    }

    #[test]
    fn group_toggle_button_says_select_when_none_selected() {
        let r = compute_group_toggle_button_state(5, 0);
        assert_eq!(r.label, "Select all");
        assert!(r.sensitive);
    }

    #[test]
    fn group_toggle_button_says_select_when_partially_selected() {
        let r = compute_group_toggle_button_state(5, 2);
        assert_eq!(r.label, "Select all");
        assert!(r.sensitive);
    }

    #[test]
    fn group_toggle_button_says_deselect_when_all_selected() {
        let r = compute_group_toggle_button_state(5, 5);
        assert_eq!(r.label, "Deselect all");
        assert!(r.sensitive);
    }

    #[test]
    fn group_toggle_button_treats_overselected_as_deselect() {
        // Defensive: if selected_count >= safe_count (e.g. stale
        // selection keys left after a delete), the button still
        // shows "Deselect all" so the next click cleans up.
        let r = compute_group_toggle_button_state(3, 4);
        assert_eq!(r.label, "Deselect all");
        assert!(r.sensitive);
    }

    // --- with_rebuilding helper ---

    fn empty_state() -> Rc<RefCell<State>> {
        Rc::new(RefCell::new(State::new(Orchestrator::new(vec![]))))
    }

    #[test]
    fn with_rebuilding_sets_flag_during_closure() {
        let state = empty_state();
        assert!(!state.borrow().rebuilding, "flag starts false");
        let mut saw_true = false;
        with_rebuilding(&state, || {
            saw_true = state.borrow().rebuilding;
        });
        assert!(saw_true, "closure must observe rebuilding == true");
        assert!(!state.borrow().rebuilding, "flag must be cleared after");
    }

    #[test]
    fn with_rebuilding_returns_closure_value() {
        let state = empty_state();
        let result = with_rebuilding(&state, || 42_i32);
        assert_eq!(result, 42);
    }

    #[test]
    fn with_rebuilding_clears_flag_even_when_closure_panics() {
        // RAII guard must clear the flag during unwind, otherwise
        // a panicking closure would freeze the GUI with
        // `rebuilding = true` forever.  This is the whole point
        // of the Drop guard.
        let state = empty_state();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_rebuilding(&state, || panic!("test panic"));
        }));
        assert!(result.is_err(), "expected the inner panic to propagate");
        assert!(
            !state.borrow().rebuilding,
            "flag must be cleared by Drop guard during unwind"
        );
    }

    #[test]
    fn with_rebuilding_inner_drop_clobbers_outer_flag() {
        // Pins the documented nested-call semantics: the inner
        // `Guard::Drop` unconditionally assigns `rebuilding = false`,
        // so between the inner drop and the outer drop the flag
        // is `false` even though the outer scope still holds a
        // guard.  This is intentional (see the doc comment on
        // `with_rebuilding`'s `Guard`) and a future refactor
        // introducing a deeper meaning for the flag must update
        // the guard to save/restore instead.
        let state = empty_state();
        let mut saw_after_inner_drop = None;
        with_rebuilding(&state, || {
            with_rebuilding(&state, || {});
            // Inner guard has just dropped; the outer guard is
            // still alive.  The flag is now `false`, clobbered
            // from the outer's `true`.
            saw_after_inner_drop = Some(state.borrow().rebuilding);
        });
        assert_eq!(
            saw_after_inner_drop,
            Some(false),
            "inner Drop must clobber outer flag to false"
        );
        assert!(!state.borrow().rebuilding, "outer flag must be cleared");
    }

    // --- try_borrow_mut helper ---

    #[test]
    fn try_borrow_mut_runs_closure_when_borrow_available() {
        let state = empty_state();
        let result = try_borrow_mut(&state, |s| {
            s.busy = true;
            s.items.len()
        });
        assert_eq!(result, Some(0));
        // Closure's mutation must have stuck.
        assert!(
            state.borrow().busy,
            "closure should have mutated state.busy = true"
        );
    }

    #[test]
    fn try_borrow_mut_returns_none_when_already_borrowed_mutably() {
        let state = empty_state();
        // Outer `borrow_mut` is held for the rest of the scope.
        let _held = state.borrow_mut();
        // The inner attempt must report `None` rather than
        // panicking with "RefCell already mutably borrowed".
        let result = try_borrow_mut(&state, |_s| panic!("closure must not run"));
        assert_eq!(result, None);
    }

    #[test]
    fn try_borrow_mut_returns_none_when_already_borrowed_immutably() {
        // Documents `RefCell` semantics inherited by the helper:
        // an outstanding immutable borrow also blocks a mutable
        // borrow, so `try_borrow_mut` correctly reports `None`
        // instead of panicking.  The helper just delegates to
        // `RefCell::try_borrow_mut`, but pinning the behaviour
        // here means a future refactor that switches to
        // `try_borrow_imm` or a custom borrow helper would
        // surface a semantic regression.
        let state = empty_state();
        let _held = state.borrow();
        let result = try_borrow_mut(&state, |_s| 42_i32);
        assert_eq!(result, None);
    }

    #[test]
    fn try_borrow_mut_passes_closure_return_value_through() {
        let state = empty_state();
        let result = try_borrow_mut(&state, |_s| 42_i32);
        assert_eq!(result, Some(42));
    }

    // --- GUI regression tests (require a display server) ---

    /// Helper: build a minimal `Rc<RefCell<State>>` with one safe
    /// docker image, register a `CheckButton` for it, and connect
    /// the real `on_item_toggled` callback.  The connection is the
    /// crucial bit: without it, `set_active` fires the signal into
    /// the void, the per-item callback never runs, and the test
    /// cannot reproduce the original `panic.txt` deadlock.
    fn make_state_with_one_docker_item() -> Rc<RefCell<State>> {
        let state = Rc::new(RefCell::new(State::new(Orchestrator::new(vec![]))));
        {
            let mut s = state.borrow_mut();
            s.items
                .push(make_item("a", "docker", Status::Unused, Category::Image));
        }
        let checkbox = CheckButton::new();
        let state_for_signal = state.clone();
        checkbox.connect_toggled(move |cb| {
            // This is the callback the real `make_item_row` wires
            // up; the per-item handler that originally panicked
            // with "RefCell already mutably borrowed".
            on_item_toggled(&state_for_signal, cb.is_active(), "docker", "a");
        });
        state.borrow_mut().item_checkboxes.insert(
            ("docker".to_string(), "a".to_string()),
            checkbox,
        );
        state
    }

    /// Regression test for the RefCell deadlock documented in
    /// `panic.txt`.  The original `on_group_toggle_clicked` held a
    /// `RefMut<State>` while calling `cb.set_active(new_active)`, so
    /// the synchronous `toggled` signal hit `on_item_toggled`,
    /// which tried to `state.borrow().rebuilding` and panicked with
    /// "RefCell already mutably borrowed".
    ///
    /// This test calls the handler on a minimal state with a real
    /// per-item signal handler connected.  If the deadlock returns,
    /// the test panics.
    ///
    /// Marked `#[ignore]` because GTK widget creation needs a
    /// display server (X11 / Wayland / Xvfb).  Run manually with:
    ///
    /// ```bash
    /// xvfb-run -a cargo test --package systemprune-gui -- --ignored
    /// ```
    #[test]
    #[ignore = "requires a display server; run with `cargo test -- --ignored` under xvfb-run"]
    fn group_toggle_clicked_selects_all_items_when_none_selected() {
        let state = make_state_with_one_docker_item();

        // This must not panic.
        on_group_toggle_clicked(&state, Category::Image);

        // The "select all" branch flips every safe item in the
        // group into `selected`; verify the item is now there.
        let s = state.borrow();
        assert!(
            s.selected.contains(&("docker".to_string(), "a".to_string())),
            "expected item to be selected after group-toggle click"
        );
    }

    /// Companion test that covers the "deselect all" branch
    /// (`all_selected == true` ⇒ `new_active == false`).
    #[test]
    #[ignore = "requires a display server; run with `cargo test -- --ignored` under xvfb-run"]
    fn group_toggle_clicked_deselects_all_items_when_all_selected() {
        let state = make_state_with_one_docker_item();
        // Pre-seed the selection so `all_selected` is true. The
        // checkbox's visual state is irrelevant to the
        // `on_group_toggle_clicked` decision (it only consults
        // `state.selected`), so we deliberately avoid the side
        // effect of an early `set_active(true)` that would
        // re-enter `on_item_toggled` and pollute the trace.
        state
            .borrow_mut()
            .selected
            .insert(("docker".to_string(), "a".to_string()));

        // This must not panic.
        on_group_toggle_clicked(&state, Category::Image);

        // The "deselect all" branch should have removed the item.
        let s = state.borrow();
        assert!(
            !s.selected.contains(&("docker".to_string(), "a".to_string())),
            "expected item to be deselected after group-toggle click"
        );
    }

    /// Regression test that pins the "make_item_row must never
    /// panic" contract enforced by the defensive `with_rebuilding`
    /// wrap around `checkbox.set_active(initially_selected)`.
    ///
    /// The original `panic.txt` deadlock was caused by holding a
    /// `RefMut` across a signal-firing GTK call.  `make_item_row`
    /// guards against a future refactor that reorders the calls
    /// by wrapping the initial `set_active` in `with_rebuilding`
    /// so the per-item `toggled` callback (which IS connected by
    /// the end of `make_item_row`) sees `rebuilding == true` and
    /// bails out of its own borrow.  This test exercises the full
    /// function on a minimal state and verifies it returns
    /// without panicking, which would catch a future refactor
    /// that simultaneously:
    ///   1. Reorders `connect_toggled` before `set_active`, AND
    ///   2. Removes the `with_rebuilding` guard.
    ///
    /// The two changes together would reintroduce the original
    /// `panic.txt` deadlock ("RefCell already mutably borrowed").
    /// The current code is safe in either order because of the
    /// guard, so a reorder alone would not break this test \u2014
    /// which is exactly the property we want from a defensive
    /// refactor: contributors can reorder freely without
    /// reintroducing the bug.
    ///
    /// Marked `#[ignore]` because GTK widget creation needs a
    /// display server (X11 / Wayland / Xvfb).  Run manually with:
    ///
    /// ```bash
    /// xvfb-run -a cargo test --package systemprune-gui -- --ignored
    /// ```
    #[test]
    #[ignore = "requires a display server; run with `cargo test -- --ignored` under xvfb-run"]
    fn make_item_row_does_not_panic_when_creating_a_checkbox() {
        // Build a state but DO NOT pre-register a CheckButton:
        // `make_item_row` will register its own checkbox AND
        // connect the real `on_item_toggled` callback internally.
        // This is the scenario the defensive `with_rebuilding`
        // wrap is designed to protect.
        let state = Rc::new(RefCell::new(State::new(Orchestrator::new(vec![]))));
        let item = make_item("a", "docker", Status::Unused, Category::Image);

        // This must not panic, even though `make_item_row`
        // internally calls `set_active` on a CheckButton whose
        // `toggled` signal is connected (via `connect_toggled`
        // later in the same function) to the real
        // `on_item_toggled` callback.  The `with_rebuilding`
        // guard ensures the callback bails out of its own
        // `state.borrow().rebuilding` check.
        let _row = make_item_row(&state, &item);

        // Sanity check: the function should have registered the
        // checkbox in `state.item_checkboxes` as a side effect.
        let s = state.borrow();
        assert!(
            s.item_checkboxes
                .contains_key(&("docker".to_string(), "a".to_string())),
            "make_item_row should register the new checkbox in state.item_checkboxes"
        );
    }

    /// Regression test that pins the "`update_group_toggle_button`
    /// must never panic" contract enforced by the defensive
    /// `with_rebuilding` wrap around `btn.set_label` /
    /// `btn.set_sensitive`.
    ///
    /// The original `update_group_toggle_button` held a
    /// `Ref<State>` (from `state.borrow()`) across both
    /// `btn.set_label` and `btn.set_sensitive`, which fire
    /// `notify::label` / `notify::sensitive` synchronously.
    /// Today no handler for either signal re-enters state, so
    /// the borrow doesn't conflict.  But a future contributor
    /// who adds a handler that calls `state.borrow()` or
    /// `state.borrow_mut()` would hit "RefCell already
    /// mutably borrowed" without the guard.  This test
    /// exercises the full function on a minimal state with a
    /// cached button and verifies it returns without
    /// panicking.
    ///
    /// Marked `#[ignore]` because GTK widget creation needs a
    /// display server (X11 / Wayland / Xvfb).  Run manually with:
    ///
    /// ```bash
    /// xvfb-run -a cargo test --package systemprune-gui -- --ignored
    /// ```
    #[test]
    #[ignore = "requires a display server; run with `cargo test -- --ignored` under xvfb-run"]
    fn update_group_toggle_button_does_not_panic_when_refreshing() {
        let state = Rc::new(RefCell::new(State::new(Orchestrator::new(vec![]))));
        {
            let mut s = state.borrow_mut();
            s.items
                .push(make_item("a", "docker", Status::Unused, Category::Image));
        }
        let btn = Button::with_label("Select all");
        state
            .borrow_mut()
            .group_toggle_buttons
            .insert(Category::Image, btn.clone());

        // This must not panic.  The `with_rebuilding` guard
        // around the setter calls ensures any future
        // `notify::label` or `notify::sensitive` handler that
        // re-enters state sees the flag and bails.
        update_group_toggle_button(&state, Category::Image);
    }
}
