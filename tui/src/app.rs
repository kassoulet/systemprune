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
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use std::collections::{BTreeMap, HashSet};
use std::io::Stdout;
use std::time::Duration;
use systemprune_core::models::{Category, PrunableItem, Status};
use systemprune_core::orchestrator::Orchestrator;
use systemprune_core::scanners::all_scanners;
use systemprune_core::size::format_size;

type TerminalType = ratatui::Terminal<ratatui::backend::CrosstermBackend<Stdout>>;

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
        }
    }

    /// Rebuild the flat list of display rows from `items`, applying
    /// current selection and collapse state.
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
            let indices = &buckets[&cat];
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
                for &i in indices {
                    self.display_rows.push(DisplayRow::Item(i));
                }
            }
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
            let body_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(24), Constraint::Min(0)])
                .split(chunks[1]);
            self.sidebar_area = body_chunks[0];
            draw_sidebar(f, self, body_chunks[0]);
            draw_table(f, self, body_chunks[1]);
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
            KeyCode::Char('r') => {
                if !self.busy {
                    self.busy = true;
                    self.status = "Scanning\u{2026}".to_string();
                }
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
        match self.cursor_row() {
            Some(DisplayRow::Item(idx)) => {
                let item = &self.items[*idx];
                if let Some(path) = item.extra.get("path") {
                    self.status = path.clone();
                } else if let Some(root) = item.extra.get("project_root") {
                    self.status = format!("{} ({})", item.name, root);
                }
            }
            Some(DisplayRow::Group { category, count, .. }) => {
                self.status = format!("{} \u{2014} {} item(s)", category.plural_label(), count);
            }
            None => {}
        }
    }

    fn toggle_all_flat(&mut self) {
        let all_keys: HashSet<(String, String)> = self
            .items
            .iter()
            .filter(|i| i.is_safe_to_delete())
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
            .filter(|i| i.category == cat && i.is_safe_to_delete())
            .map(|i| (i.source.clone(), i.id.clone()))
            .collect();
        if safe_keys.is_empty() {
            self.status = format!("No safe items in {}.", cat.plural_label());
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
        if !item.is_safe_to_delete() {
            self.status = format!("Cannot toggle: {} is active.", item.name);
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
        // Reset collapse state on every fresh scan.
        self.collapsed = HashSet::new();
        self.rebuild_display_rows();
        if err_count > 0 {
            self.status = format!(
                "Found {} item(s) with {} error(s).",
                item_count, err_count
            );
        } else {
            self.status = format!("Found {} item(s).", item_count);
        }
        if !self.display_rows.is_empty() {
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
                    && i.is_safe_to_delete()
            })
            .cloned()
            .collect();
        if to_delete.is_empty() {
            self.busy = false;
            self.status = "No items to delete.".to_string();
            return;
        }
        let results = self.orchestrator.delete_many(&to_delete, true).await;
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
        Span::raw("Unified Linux disk cleaner"),
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
                let key = (item.source.clone(), item.id.clone());
                let mark = if item.status.is_deleted() {
                    "\u{2717}"
                } else if !item.is_safe_to_delete() {
                    "\u{1f512}"
                } else if app.selected.contains(&key) {
                    "x"
                } else {
                    " "
                };
                let name = if item.status.is_deleted() {
                    format!("{} (deleted)", item.name)
                } else {
                    item.name.clone()
                };
                let style = if item.status.is_deleted() {
                    Style::default().add_modifier(Modifier::ITALIC)
                } else {
                    Style::default()
                };
                Row::new(vec![
                    Cell::from(mark),
                    Cell::from(item.category.plural_label().to_string()),
                    Cell::from(item.status.as_str().to_string()),
                    Cell::from(format_size(item.size_bytes as i64, true)),
                    Cell::from(name),
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
        "Items".to_string()
    } else {
        format!(
            "Items  [{}/{} selected, {}]",
            app.selected.len(),
            app.items.len(),
            format_size(sel_total as i64, true)
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
