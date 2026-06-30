//! TUI smoke tests (integration).
//!
//! Render one frame of the SystemPrune TUI against
//! `ratatui::backend::TestBackend` (an in-memory cell buffer)
//! so the entire UI render path
//! (header → sidebar → table → status) runs under `cargo test`
//! without a real terminal / tmux / Xvfb dependency.
//!
//! Why integration rather than in-module?  Once the TUI crate was
//! split into `[lib] + [[bin]]` the test target needs to live in
//! `src/lib.rs::tests` (same crate, no visibility expansion) OR
//! in `tests/smoke.rs` (visibility surface for `App` is `pub`).
//! Integration tests are the right home when the surface you want
//! to pin is also the public surface — refactors that break the
//! integration tests catch exactly the public-API regressions a
//! downstream consumer would hit.
//!
//! Run via:
//!   * `cargo test -p systemprune-tui --test smoke`
//!   * `cargo test -p systemprune-tui -- smoke_tui --nocapture`
//!     (filter crosses test targets)
//!   * `cargo test --workspace --all-targets --no-fail-fast`
//!     (CI: `tui:` job in `.github/workflows/ci.yml`)

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use systemprune_core::models::{Category, Engine, PrunableItem, Status};
use systemprune_tui::app::App;

/// Build a stub [`PrunableItem`] for smoke-test purposes.  Real
/// scanners return items shaped like this — the TUI smoke contract
/// is just "the table can render it".
fn make_item(id: &str, source: &str, status: Status, category: Category) -> PrunableItem {
    // The `Engine` enum is purely metadata; the TUI doesn't read
    // it on the render path it does check `item.source`, so we map
    // each known source to a representative engine and fall back
    // to Docker for anything else.
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

/// Blank-slate frame a user sees on a fresh install where
/// docker / podman / flatpak / snap / ollama are not yet
/// installed.  Verifies the brand header AND the "No engines
/// detected" sidebar copy both render into the captured buffer.
#[test]
fn smoke_tui_renders_with_no_engines_detected() {
    // `App::empty()` keeps the orchestrator free of scanners
    // so `draw_sidebar` falls through to the "No engines
    // detected" branch.  `App::new()` would attach
    // `all_scanners()`; on a host with even one engine CLI
    // installed the sidebar would list that engine instead
    // and the assertion below would fail.
    let mut app = App::empty();
    let buf = app.render_to_buffer(120, 24);
    let text: String = buf.content.iter().map(|c| c.symbol()).collect();
    assert!(
        text.contains("SystemPrune"),
        "expected 'SystemPrune' header to render"
    );
    assert!(
        text.contains("No engines detected"),
        "expected sidebar to show 'No engines detected'"
    );
}

/// When an item is present, the category header AND the item's
/// id must appear in the rendered buffer.  Guards against the
/// regression where a refactor drops items from the display
/// list (silently hiding them from the user).  `rebuild_display_rows`
/// is called explicitly because it normally runs at the end of
/// the async `do_scan`, which is out of scope for these
/// synchronous integration tests.
#[test]
fn smoke_tui_renders_an_item_with_its_category() {
    let mut app = App::new();
    app.items.push(make_item(
        "img-1",
        "docker",
        Status::Unused,
        Category::Image,
    ));
    app.rebuild_display_rows();
    let buf = app.render_to_buffer(120, 24);
    let text: String = buf.content.iter().map(|c| c.symbol()).collect();
    // Pin the exact `plural_label` so a future refactor that
    // moves the category name to the sidebar (or renames it)
    // is caught here.  `text.contains("Docker")` would be too
    // permissive.
    assert!(
        text.contains("Docker Images"),
        "expected category group header ('Docker Images') to render in the table"
    );
    assert!(
        text.contains("img-1"),
        "expected item id 'img-1' to render in the table"
    );
}

/// Quit-key plumbing: pressing `q` must set `App::quit`.  This
/// is the one binding every TUI user exercises on first run,
/// so a regression here is especially costly.  We build the
/// struct literal explicitly (rather than `KeyEvent::new(...)`)
/// so the test does not silently rely on crossterm's `kind`
/// default; `handle_key` returns early for any non-`Press`
/// event and the assertion below would hide that as
/// `App::quit == false`.
#[test]
fn smoke_tui_quit_key_sets_quit_flag() {
    let key = KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    let mut app = App::new();
    app.handle_key(key);
    assert!(app.quit, "pressing 'q' must set App::quit = true");
}
