//! Ratatui app: layout, state, key handling, scan, and delete.
//!
//! Items are grouped by `Category` and each group is collapsible.
//!
//! Key bindings (when cursor is on a row):
//! - `q` / `Esc` — quit
//! - `r` — rescan
//! - `up` / `down` — move the cursor
//! - `enter` — expand/collapse the group at the cursor (group rows only)
//! - `space` — toggle the current item (item rows only)
//! - `a` — select/deselect all (flat) across all safe items
//! - `A` (shift) — select/deselect all safe items in the current group
//! - `d` — delete selected

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseButton};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use std::collections::{BTreeMap, HashSet};
use std::io::Stdout;
use std::time::Duration;
use systemprune_core::log::ActionLog;
use systemprune_core::models::{Category, PrunableItem, Status};
use systemprune_core::orchestrator::Orchestrator;
use systemprune_core::scanners::all_scanners;
use systemprune_core::size::format_size;
use systemprune_core::sort::SortMode;

type TerminalType = ratatui::Terminal<ratatui::backend::CrosstermBackend<Stdout>>;

/// Per-item display description produced by `App::describe_item_row`.
/// Extracted so unit tests can verify the contract of the
/// `delete_errors` map without rendering a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemRowRender {
    pub mark: &'static str,
    pub name: String,
    pub color: ItemRowColor,
    pub italic: bool,
}

/// Foreground colour hint for a rendered item row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemRowColor {
    Default,
    Red,
}

/// One row in the flat display list: either a group header or a
/// reference to an item in `App::items`.
#[derive(Debug, Clone)]
enum DisplayRow {
    Group {
        category: Category,
        count: usize,
        total_size: u64,
        sel_count: usize,
        sel_size: u64,
        safe_count: usize,
        collapsed: bool,
    },
    Item(usize),
}

struct App {
    orchestrator: Orchestrator,
    items: Vec<PrunableItem>,
    selected: HashSet<(String, String)>,
    table_state: TableState,
    status: String,
    busy: bool,
    /// Categories that are collapsed (default: all expanded).
    collapsed: HashSet<Category>,
    /// Flat list of rows (group headers + visible items) shown in the table.
    display_rows: Vec<DisplayRow>,
    /// Cached sidebar area for hit testing.
    sidebar_area: Rect,
    /// Set to true when the user presses q/Esc to quit.
    quit: bool,
    /// Error messages for failed deletions, keyed by (source, id).
    delete_errors: BTreeMap<(String, String), String>,
    /// Active sort mode for items within each category group.
    /// Cycled by the `s` key binding.
    sort_mode: SortMode,
    /// Shared action log.  The orchestrator pushes entries at
    /// scan/delete boundaries; the TUI reads them when the
    /// user toggles the log view with `l`.
    log: ActionLog,
    /// When true, the main view is replaced by a scrollable
    /// log view.  Toggled by the `l` key binding.
    show_log: bool,
}

impl App {
    fn new() -> Self {
        Self {
            orchestrator: Orchestrator::new(all_scanners()),
            items: Vec::new(),
            selected: HashSet::new(),
            table_state: TableState::default(),
            status: "Scanning\u{2026}".to_string(),
            busy: true,
            collapsed: HashSet::new(),
            display_rows: Vec::new(),
            sidebar_area: Rect::default(),
            quit: false,
            delete_errors: BTreeMap::new(),
            sort_mode: SortMode::Default,
            log: ActionLog::default(),
            show_log: false,
        }
    }

    /// Per-item display description. Extracted from `draw_table`
    /// so unit tests can pin the contract without rendering a
    /// frame.
    pub(crate) fn describe_item_row(&self, item: &PrunableItem) -> ItemRowRender {
        let key = (item.source.clone(), item.id.clone());
        let has_error = self.delete_errors.contains_key(&key);
        let mark = if item.status.is_deleted() {
            "\u{2717}"
        } else if has_error {
            "\u{2716}"
        } else if !item.is_safe_to_delete() {
            "\u{1f512}"
        } else if self.selected.contains(&key) {
            "x"
        } else {
            " "
        };
        let name = if item.status.is_deleted() {
            format!("{} (deleted)", item.name)
        } else if has_error {
            format!("{} (failed)", item.name)
        } else {
            item.name.clone()
        };
        let (color, italic) = if item.status.is_deleted() {
            (ItemRowColor::Default, true)
        } else if has_error {
            (ItemRowColor::Red, false)
        } else {
            (ItemRowColor::Default, false)
        };
        ItemRowRender {
            mark,
            name,
            color,
            italic,
        }
    }

    /// Rebuild the flat list of display rows from `items`, applying
    /// current selection, collapse, and sort state.
    ///
    /// **Sort scope.**  Items are sorted **within** each category
    /// group only; the cross-category order still follows the
    /// first-seen order of the scan.  `SortMode::Default` is a
    /// no-op so the default UX is identical to the pre-sort
    /// behaviour.
    fn rebuild_display_rows(&mut self) {
        self.display_rows.clear();
        // Group by category, preserving first-seen order, using a
        // parallel BTreeMap for the buckets.
        let mut order: Vec<Category> = Vec::new();
        let mut buckets: BTreeMap<Category, Vec<usize>> = BTreeMap::new();
        for (idx, item) in self.items.iter().enumerate() {
            if !buckets.contains_key(&item.category) {
                order.push(item.category);
            }
            buckets.entry(item.category).or_default().push(idx);
        }
        for cat in order {
            // Sort the per-category indices by the active mode.
            // We sort indices (not items) so the rest of the
            // method can keep indexing into `self.items` without
            // remapping.  `Default` is a no-op.
            // Borrow-checker note: we can't hold a mutable
            // borrow of `buckets[cat]` while the `sort_by`
            // closure reads `self.items`, because both are
            // fields of `self`.  We therefore `remove` the
            // bucket, sort it in place (which only reads
            // `self.items` and `self.sort_mode`), and insert
            // it back at the end of the loop after all the
            // read-only passes that use `indices`.
            let mut indices = buckets.remove(&cat).unwrap_or_default();
            let items = &self.items;
            match self.sort_mode {
                SortMode::Default => {}
                SortMode::NameAsc => {
                    indices.sort_by(|&a, &b| items[a].name.cmp(&items[b].name));
                }
                SortMode::SizeDesc => {
                    indices.sort_by(|&a, &b| {
                        items[b].size_bytes.cmp(&items[a].size_bytes)
                    });
                }
                SortMode::SizeAsc => {
                    indices.sort_by(|&a, &b| {
                        items[a].size_bytes.cmp(&items[b].size_bytes)
                    });
                }
            }
            let count = indices.len();
            let total_size: u64 = indices.iter().map(|&i| self.items[i].size_bytes).sum();
            let safe_count = indices
                .iter()
                .filter(|&&i| self.items[i].is_safe_to_delete())
                .count();
            let sel_count = indices
                .iter()
                .filter(|&&i| {
                    let it = &self.items[i];
                    it.is_safe_to_delete()
                        && self.selected.contains(&(it.source.clone(), it.id.clone()))
                })
                .count();
            let sel_size: u64 = indices
                .iter()
                .filter(|&&i| {
                    let it = &self.items[i];
                    it.is_safe_to_delete()
                        && self.selected.contains(&(it.source.clone(), it.id.clone()))
                })
                .map(|&i| self.items[i].size_bytes)
                .sum();
            let collapsed = self.collapsed.contains(&cat);
            self.display_rows.push(DisplayRow::Group {
                category: cat,
                count,
                total_size,
                sel_count,
                sel_size,
                safe_count,
                collapsed,
            });
            if !collapsed {
                for &i in indices.iter() {
                    self.display_rows.push(DisplayRow::Item(i));
                }
            }
            // Put the sorted bucket back.  Done last so all the
            // read-only passes above can use `indices` without
            // re-borrowing `buckets`.
            buckets.insert(cat, indices);
        }
    }

    fn render(&mut self, terminal: &mut TerminalType) -> Result<()> {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(0),
                    Constraint::Length(3),
                ])
                .split(f.size());
            draw_header(f, chunks[0]);
            if self.show_log {
                draw_log_view(f, self, chunks[1]);
            } else {
                let body_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(24), Constraint::Min(0)])
                    .split(chunks[1]);
                self.sidebar_area = body_chunks[0];
                draw_sidebar(f, self, body_chunks[0]);
                draw_table(f, self, body_chunks[1]);
            }
            draw_status(f, &self.status, chunks[2]);
        })?;
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // The capital `A` (i.e. shift+a) binding toggles
        // select-all-in-group; lowercase `a` toggles select-all-flat.
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('l') => {
                // Toggle the log view.  In the log view, `q`/`Esc`
                // still quit (matching the rest of the TUI), and
                // `l` again returns to the items view.
                self.show_log = !self.show_log;
                self.status = if self.show_log {
                    format!("Showing log ({} entries). l=items, q=quit.", self.log.len())
                } else {
                    "Back to items.".to_string()
                };
            }
            KeyCode::Char('r') => {
                if !self.busy {
                    self.busy = true;
                    self.status = "Scanning\u{2026}".to_string();
                }
            }
            KeyCode::Char('s') => {
                // Cycle through sort modes.  No shift variant
                // for now; a future menu-driven picker could
                // bind `S` to one.
                self.sort_mode = self.sort_mode.next();
                self.rebuild_display_rows();
                self.status = format!(
                    "Sort: {} (press s again to change)",
                    self.sort_mode.label()
                );
            }
            KeyCode::Char('a') if shift => self.toggle_select_all_at_cursor(),
            KeyCode::Char('a') => self.toggle_all_flat(),
            KeyCode::Char(' ') => self.toggle_cursor_item(),
            KeyCode::Enter => self.toggle_cursor_group(),
            KeyCode::Char('d') => {
                if self.busy || self.selected.is_empty() {
                    return;
                }
                self.busy = true;
                let total: u64 = self
                    .items
                    .iter()
                    .filter(|i| self.selected.contains(&(i.source.clone(), i.id.clone())))
                    .map(|i| i.size_bytes)
                    .sum();
                self.status = format!(
                    "Deleting {} item(s) ({})\u{2026}",
                    self.selected.len(),
                    format_size(total as i64, true)
                );
            }
            KeyCode::Down => {
                if self.display_rows.is_empty() {
                    return;
                }
                let max = self.display_rows.len() - 1;
                let i = self.table_state.selected().unwrap_or(0);
                self.table_state.select(Some((i + 1).min(max)));
                self.update_cursor_status();
            }
            KeyCode::Up => {
                let i = self.table_state.selected().unwrap_or(0);
                self.table_state.select(Some(i.saturating_sub(1)));
                self.update_cursor_status();
            }
            _ => {}
        }
    }

    fn cursor_row(&self) -> Option<&DisplayRow> {
        let i = self.table_state.selected()?;
        self.display_rows.get(i)
    }

    fn update_cursor_status(&mut self) {
        self.status = self.cursor_info();
    }

    /// Pure description of the current cursor row. Extracted from
    /// `update_cursor_status` so unit tests can pin the contract
    /// without rendering a frame.
    pub(crate) fn cursor_info(&self) -> String {
        match self.cursor_row() {
            Some(DisplayRow::Item(idx)) => {
                let item = &self.items[*idx];
                let key = (item.source.clone(), item.id.clone());
                if let Some(err) = self.delete_errors.get(&key) {
                    format!("Error: {}", err)
                } else if let Some(path) = item.extra.get("path") {
                    path.clone()
                } else if let Some(root) = item.extra.get("project_root") {
                    format!("{} ({})", item.name, root)
                } else {
                    String::new()
                }
            }
            Some(DisplayRow::Group { category, count, .. }) => {
                format!("{} \u{2014} {} item(s)", category.plural_label(), count)
            }
            None => String::new(),
        }
    }

    fn toggle_all_flat(&mut self) {
        let all_keys: HashSet<(String, String)> = self
            .items
            .iter()
            .filter(|i| i.is_deletable_for_real(&self.delete_errors))
            .map(|i| (i.source.clone(), i.id.clone()))
            .collect();
        if self.selected == all_keys {
            self.selected.clear();
        } else {
            self.selected = all_keys;
        }
        self.status = self.selected_status();
        self.rebuild_display_rows();
    }

    fn toggle_select_all_at_cursor(&mut self) {
        if let Some(DisplayRow::Group { category, .. }) = self.cursor_row().cloned() {
            self.toggle_select_all_in_group(category);
        } else {
            // On an item row, capital `A` is a no-op (use lowercase
            // `a` for flat, or press Enter on a group header).
            self.status = "Press A on a group row to select all in that group.".to_string();
        }
    }

    fn toggle_select_all_in_group(&mut self, cat: Category) {
        let safe_keys: HashSet<(String, String)> = self
            .items
            .iter()
            .filter(|i| i.category == cat && i.is_deletable_for_real(&self.delete_errors))
            .map(|i| (i.source.clone(), i.id.clone()))
            .collect();
        if safe_keys.is_empty() {
            self.status = format!("No deletable items in {}.", cat.plural_label());
            return;
        }
        if safe_keys.is_subset(&self.selected) {
            for k in &safe_keys {
                self.selected.remove(k);
            }
            self.status = format!("Deselected all in {}.", cat.plural_label());
        } else {
            for k in safe_keys {
                self.selected.insert(k);
            }
            self.status = self.selected_status();
        }
        self.rebuild_display_rows();
    }

    fn toggle_cursor_item(&mut self) {
        let idx = match self.cursor_row() {
            Some(DisplayRow::Item(i)) => *i,
            _ => return,
        };
        let item = &self.items[idx];
        if !item.is_deletable_for_real(&self.delete_errors) {
            self.status = if !item.is_safe_to_delete() {
                format!("Cannot toggle: {} is active.", item.name)
            } else {
                format!(
                    "Cannot toggle: {} previously failed to delete.",
                    item.name
                )
            };
            return;
        }
        let key = (item.source.clone(), item.id.clone());
        if !self.selected.remove(&key) {
            self.selected.insert(key);
        }
        self.status = self.selected_status();
        self.rebuild_display_rows();
    }

    fn selected_status(&self) -> String {
        let count = self.selected.len();
        if count == 0 {
            return "No items selected.".to_string();
        }
        let total: u64 = self
            .items
            .iter()
            .filter(|i| self.selected.contains(&(i.source.clone(), i.id.clone())))
            .map(|i| i.size_bytes)
            .sum();
        format!("Selected {} item(s) ({}) total.", count, format_size(total as i64, true))
    }

    fn toggle_cursor_group(&mut self) {
        let cat = match self.cursor_row() {
            Some(DisplayRow::Group { category, .. }) => *category,
            _ => return,
        };
        if !self.collapsed.remove(&cat) {
            self.collapsed.insert(cat);
        }
        self.rebuild_display_rows();
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if mouse.kind != crossterm::event::MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let x = mouse.column;
        let y = mouse.row;
        // Check if click is inside the sidebar content area.
        let sa = self.sidebar_area;
        // Content starts at sa.y + 1 (top border) and ends at sa.y + sa.height - 1 (bottom border).
        if x >= sa.x && x < sa.x + sa.width && y > sa.y && y < sa.y + sa.height - 1 {
            let line_idx = (y - sa.y - 1) as usize;
            let engines: Vec<String> = self
                .orchestrator
                .available_engines()
                .into_iter()
                .collect();
            if let Some(engine) = engines.get(line_idx) {
                self.scroll_to_engine(engine);
            }
        }
    }

    fn scroll_to_engine(&mut self, engine: &str) {
        // Find the first display row that contains an item from this engine.
        for (i, dr) in self.display_rows.iter().enumerate() {
            if let DisplayRow::Item(idx) = dr {
                if self.items[*idx].source == engine {
                    self.table_state.select(Some(i));
                    return;
                }
            }
        }
    }

    async fn do_scan(&mut self) {
        let result = self.orchestrator.scan_all().await;
        let item_count = result.items.len();
        let err_count = result.errors.len();
        self.items = result.items;
        self.rebuild_display_rows();
        if err_count > 0 {
            self.status = format!(
                "Found {} item(s) with {} error(s).",
                item_count, err_count
            );
        } else {
            self.status = format!("Found {} item(s).", item_count);
        }
        // Preserve scroll position if possible, otherwise select first item.
        if self.display_rows.is_empty() {
            self.table_state.select(None);
        } else if self.table_state.selected().map_or(true, |i| i >= self.display_rows.len()) {
            self.table_state.select(Some(0));
        }
        self.busy = false;
    }

    async fn do_delete(&mut self) {
        let to_delete: Vec<PrunableItem> = self
            .items
            .iter()
            .filter(|i| {
                self.selected.contains(&(i.source.clone(), i.id.clone()))
                    && i.is_deletable_for_real(&self.delete_errors)
            })
            .cloned()
            .collect();
        if to_delete.is_empty() {
            self.busy = false;
            self.status = "No items to delete.".to_string();
            return;
        }
        let results = self
            .orchestrator
            .delete_many(&to_delete, true, Some(&self.delete_errors))
            .await;
        let ok = results.iter().filter(|r| r.success).count();
        let fail = results.len() - ok;
        for r in &results {
            if r.success {
                self.selected.remove(&(r.item.source.clone(), r.item.id.clone()));
            }
        }
        // Mark successfully deleted items instead of removing them.
        for r in &results {
            if r.success {
                if let Some(item) = self.items.iter_mut().find(|i| i.source == r.item.source && i.id == r.item.id) {
                    item.status = Status::Deleted;
                }
                self.delete_errors.remove(&(r.item.source.clone(), r.item.id.clone()));
            } else if let Some(err) = &r.error {
                let key = (r.item.source.clone(), r.item.id.clone());
                self.delete_errors.insert(key, err.to_string());
            }
        }
        let freed: u64 = results.iter().filter(|r| r.success).map(|r| r.item.size_bytes).sum();
        self.status = format!("Deleted {}, failed {}. Freed {}.", ok, fail, format_size(freed as i64, true));
        self.busy = false;
        self.rebuild_display_rows();
    }
}

pub fn run(terminal: &mut TerminalType) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let mut app = App::new();

    // Initial scan.
    runtime.block_on(async {
        app.do_scan().await;
    });
    app.render(terminal)?;

    loop {
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => {}
            }
        }
        if app.quit {
            break;
        }
        if app.busy {
            if app.status.starts_with("Scanning") {
                runtime.block_on(async {
                    app.do_scan().await;
                });
            } else if app.status.starts_with("Deleting") {
                runtime.block_on(async {
                    app.do_delete().await;
                });
            }
        }
        app.render(terminal)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

fn draw_header(f: &mut ratatui::Frame, area: Rect) {
    let header = Paragraph::new(vec![Line::from(vec![
        Span::styled(
            " SystemPrune ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  s=sort  l=log  a=select all  d=delete  r=rescan  q=quit"),
    ])])
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, area);
}

fn draw_sidebar(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::RIGHT).title("Engines");
    let mut lines: Vec<Line> = Vec::new();
    if app.orchestrator.available_engines().is_empty() {
        lines.push(Line::from(Span::styled(
            "No engines detected.",
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from("Install docker, podman,"));
        lines.push(Line::from("flatpak, snap, or ollama."));
    } else {
        for src in app.orchestrator.available_engines() {
            let count = app.items.iter().filter(|i| i.source == src).count();
            lines.push(Line::from(format!("\u{25cf} {} ({})", src, count)));
        }
    }
    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}

fn draw_table(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let header = Row::new(vec![
        Cell::from("\u{2713}"),
        Cell::from("Category"),
        Cell::from("Status"),
        Cell::from("Size"),
        Cell::from("Name"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .display_rows
        .iter()
        .map(|dr| match dr {
            DisplayRow::Group {
                category,
                count,
                total_size,
                sel_count,
                sel_size,
                safe_count,
                collapsed,
            } => {
                let arrow = if *collapsed { "\u{25b8}" } else { "\u{25be}" };
                let hint = if *sel_count > 0 {
                    format!(
                        "[{}/{} sel, {}]  A=all  Enter=toggle",
                        sel_count,
                        safe_count,
                        format_size(*sel_size as i64, true)
                    )
                } else if *safe_count > 0 {
                    format!("[{} safe]  A=all  Enter=toggle", safe_count)
                } else {
                    format!("[{} safe]", safe_count)
                };
                Row::new(vec![
                    Cell::from(arrow),
                    Cell::from(format!("\u{2588} {}", category.plural_label())),
                    Cell::from(format!("{} items", count)),
                    Cell::from(format_size(*total_size as i64, true)),
                    Cell::from(hint),
                ])
                .style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            }
            DisplayRow::Item(idx) => {
                let item = &app.items[*idx];
                let render = app.describe_item_row(item);
                let style = match (render.color, render.italic) {
                    (ItemRowColor::Default, false) => Style::default(),
                    (ItemRowColor::Default, true) => Style::default().add_modifier(Modifier::ITALIC),
                    (ItemRowColor::Red, _) => Style::default().fg(Color::Red),
                };
                Row::new(vec![
                    Cell::from(render.mark),
                    Cell::from(item.category.plural_label().to_string()),
                    Cell::from(item.status.as_str().to_string()),
                    Cell::from(format_size(item.size_bytes as i64, true)),
                    Cell::from(render.name),
                ]).style(style)
            }
        })
        .collect();

    let widths = [
        Constraint::Length(3),
        Constraint::Length(16),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Min(20),
    ];
    let sel_total: u64 = app
        .items
        .iter()
        .filter(|i| app.selected.contains(&(i.source.clone(), i.id.clone())))
        .map(|i| i.size_bytes)
        .sum();
    let title = if app.selected.is_empty() {
        format!("Items  [sort: {}]", app.sort_mode.short_label())
    } else {
        format!(
            "Items  [{}/{} selected, {} | sort: {}]",
            app.selected.len(),
            app.items.len(),
            format_size(sel_total as i64, true),
            app.sort_mode.short_label()
        )
    };
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_status(f: &mut ratatui::Frame, status: &str, area: Rect) {
    let widget = Paragraph::new(Line::from(Span::raw(status)))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(widget, area);
}

/// Render the action log as a scrollable text view.  Shows
/// the formatted log lines (oldest at the top, newest at the
/// bottom) inside a bordered block.
fn draw_log_view(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let text = app.log.format_lines();
    let title = format!("Action log ({} entries)", app.log.len());
    let widget = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(widget, area);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Unit tests for the deletion-error tracking contract.
    //!
    //! The render path in `draw_table` is a thin wrapper around
    //! `App::describe_item_row`; these tests pin the contract of
    //! that helper (and `cursor_info`) so future refactors cannot
    //! silently break the surface of failed deletions.

    use super::*;
    use systemprune_core::models::Engine;

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

    /// Build an `App` with no real scanners. The render-decision
    /// helpers do not touch the orchestrator, so this is sufficient.
    fn empty_app() -> App {
        App {
            orchestrator: Orchestrator::new(vec![]),
            items: Vec::new(),
            selected: HashSet::new(),
            table_state: TableState::default(),
            status: String::new(),
            busy: false,
            collapsed: HashSet::new(),
            display_rows: Vec::new(),
            sidebar_area: Rect::default(),
            quit: false,
            delete_errors: BTreeMap::new(),
            sort_mode: SortMode::Default,
            log: ActionLog::default(),
            show_log: false,
        }
    }

    #[test]
    fn describe_item_row_unselected_safe_uses_blank_mark() {
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        app.items.push(item.clone());
        let render = app.describe_item_row(&item);
        assert_eq!(render.mark, " ");
        assert_eq!(render.name, "a");
        assert_eq!(render.color, ItemRowColor::Default);
        assert!(!render.italic);
    }

    #[test]
    fn describe_item_row_selected_safe_uses_x_mark() {
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        app.items.push(item.clone());
        app.selected
            .insert((item.source.clone(), item.id.clone()));
        let render = app.describe_item_row(&item);
        assert_eq!(render.mark, "x");
        assert_eq!(render.name, "a");
    }

    #[test]
    fn describe_item_row_active_uses_lock_mark() {
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Active, Category::Image);
        app.items.push(item.clone());
        let render = app.describe_item_row(&item);
        assert_eq!(render.mark, "\u{1f512}");
    }

    #[test]
    fn describe_item_row_deleted_uses_x_mark_and_italic_and_suffix() {
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Deleted, Category::Image);
        app.items.push(item.clone());
        let render = app.describe_item_row(&item);
        assert_eq!(render.mark, "\u{2717}");
        assert_eq!(render.name, "a (deleted)");
        assert_eq!(render.color, ItemRowColor::Default);
        assert!(render.italic);
    }

    #[test]
    fn describe_item_row_with_delete_error_uses_failed_mark_and_red() {
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        app.items.push(item.clone());
        app.delete_errors.insert(
            (item.source.clone(), item.id.clone()),
            "boom".to_string(),
        );
        let render = app.describe_item_row(&item);
        assert_eq!(render.mark, "\u{2716}");
        assert_eq!(render.name, "a (failed)");
        assert_eq!(render.color, ItemRowColor::Red);
        assert!(!render.italic);
    }

    #[test]
    fn describe_item_row_error_takes_precedence_over_selection() {
        // Selection is a non-destructive intent; a failed delete
        // should keep surfacing the error to the user.
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        app.items.push(item.clone());
        app.selected
            .insert((item.source.clone(), item.id.clone()));
        app.delete_errors.insert(
            (item.source.clone(), item.id.clone()),
            "boom".to_string(),
        );
        let render = app.describe_item_row(&item);
        assert_eq!(render.mark, "\u{2716}");
        assert_eq!(render.name, "a (failed)");
        assert_eq!(render.color, ItemRowColor::Red);
    }

    #[test]
    fn describe_item_row_deleted_status_wins_over_error() {
        // A re-scan after a failed delete may or may not keep the
        // previous error; if the item somehow has both, the
        // Status::Deleted display (italic, ✗) is unambiguous and
        // wins, matching the pre-error-tracking behaviour.
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Deleted, Category::Image);
        app.items.push(item.clone());
        app.delete_errors.insert(
            (item.source.clone(), item.id.clone()),
            "boom".to_string(),
        );
        let render = app.describe_item_row(&item);
        assert_eq!(render.mark, "\u{2717}");
        assert_eq!(render.name, "a (deleted)");
        assert!(render.italic);
    }

    // --- cursor_info ---

    #[test]
    fn cursor_info_on_item_with_delete_error_formats_error_prefix() {
        let mut app = empty_app();
        let item = make_item("a", "docker", Status::Unused, Category::Image);
        app.items.push(item);
        app.display_rows.push(DisplayRow::Item(0));
        app.table_state.select(Some(0));
        app.delete_errors
            .insert(("docker".to_string(), "a".to_string()), "boom".to_string());
        assert_eq!(app.cursor_info(), "Error: boom");
    }

    #[test]
    fn cursor_info_on_item_with_path_uses_path() {
        let mut app = empty_app();
        let mut item = make_item("a", "docker", Status::Unused, Category::Image);
        item.extra
            .insert("path".to_string(), "/some/path".to_string());
        app.items.push(item);
        app.display_rows.push(DisplayRow::Item(0));
        app.table_state.select(Some(0));
        assert_eq!(app.cursor_info(), "/some/path");
    }

    #[test]
    fn cursor_info_on_item_with_project_root_uses_name_root() {
        let mut app = empty_app();
        let mut item = make_item("a", "docker", Status::Unused, Category::Image);
        item.extra
            .insert("project_root".to_string(), "/proj".to_string());
        app.items.push(item);
        app.display_rows.push(DisplayRow::Item(0));
        app.table_state.select(Some(0));
        assert_eq!(app.cursor_info(), "a (/proj)");
    }

    #[test]
    fn cursor_info_on_group_uses_plural_label_and_count() {
        let mut app = empty_app();
        app.items
            .push(make_item("a", "docker", Status::Unused, Category::Image));
        app.display_rows.push(DisplayRow::Group {
            category: Category::Image,
            count: 1,
            total_size: 1024,
            sel_count: 0,
            sel_size: 0,
            safe_count: 1,
            collapsed: false,
        });
        app.table_state.select(Some(0));
        assert_eq!(app.cursor_info(), "Docker Images \u{2014} 1 item(s)");
    }

    #[test]
    fn cursor_info_with_no_selection_is_empty() {
        let app = empty_app();
        assert_eq!(app.cursor_info(), "");
    }

    #[test]
    fn cursor_info_error_takes_precedence_over_path() {
        let mut app = empty_app();
        let mut item = make_item("a", "docker", Status::Unused, Category::Image);
        item.extra
            .insert("path".to_string(), "/some/path".to_string());
        app.items.push(item);
        app.display_rows.push(DisplayRow::Item(0));
        app.table_state.select(Some(0));
        app.delete_errors
            .insert(("docker".to_string(), "a".to_string()), "boom".to_string());
        assert_eq!(app.cursor_info(), "Error: boom");
    }
}
