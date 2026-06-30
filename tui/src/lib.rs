//! Library code for the SystemPrune TUI.
//!
//! Most of the app lives in the [`app`] module.  The binary
//! (`systemprune-tui`) is a thin wrapper that handles terminal
//! setup and the event loop; integration tests under
//! `tui/tests/` reach into this crate via the public surface
//! declared in [`app`].
//!
//! Visibility contract:
//!   * [`app::App`] is `pub` so tests can construct and drive
//!     the app.
//!   * The fields and methods integration tests need
//!     (`items`, `quit`, `new`, `handle_key`,
//!     `rebuild_display_rows`, `render_to_buffer`) are `pub`.
//!   * Other fields and helper methods stay private — they are
//!     for internal use by the `App` impl itself.
//!   * `App::render_to_buffer` is the bridge integration tests
//!     use to drive the full render path against
//!     `ratatui::backend::TestBackend` without a real terminal.

pub mod app;
