# Changelog

All notable changes to **systemprune-rs** are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `ScanResult::from_items()` (behind `#[cfg(test)]`) for easy test setup.
- `ScanResult::by_category()` test in `core/tests/orchestrator_test.rs`.
- `Category` model tests in `core/tests/models_test.rs`:
  `plural_label`, `Ord` (BTreeMap / BTreeSet), `Hash` (HashSet).
- Inline parser tests in `scanners/docker.rs` (container, volume, network),
  `scanners/ollama.rs` (two-column fallback, irregular whitespace),
  `scanners/podman.rs` (whitespace-only, skips non-object lines).
- New `parse_row` helpers in `scanners/flatpak.rs` and `scanners/snap.rs`,
  with 3 inline tests each. The `list_apps` / `list_runtimes` /
  `list_snaps` methods now go through these helpers instead of doing
  inline parsing.
- `CHANGELOG.md` and a "Grouped-by-type view" key-binding table in
  the `TUI` section of `README.md`.

### Changed
- `scanners/flatpak.rs` and `scanners/snap.rs` now share their row
  parsing logic with their tests via the `parse_row` helper.

### Removed
- Dead helper `fn _silence_hashset_unused(...)` from
  `core/tests/orchestrator_test.rs` (and the now-unused `HashSet`
  import).

## [0.1.0] — 2025-XX-XX

Initial release of the Rust workspace.

### Added
- Workspace crates: `core` (library), `systemprune` (CLI), `systemprune-tui`
  (Ratatui terminal UI), `systemprune-gui` (gtk4-rs GTK4 desktop UI).
- Scanners: Docker, Podman, Flatpak, Snap, Ollama.
- `Scanner` trait with `get_items` / `delete_item` / `is_available` /
  `source` / `engine` / `binary`.
- `Orchestrator` with concurrent `scan_all` (tokio `JoinSet`) and
  order-preserving `delete_many` (pre-allocated slots + oneshot
  channels).
- `PrunableItem` model with `is_safe_to_delete` safety guardrail.
- `BaseScanner` helpers: PATH probe via `which::which`, subprocess
  with timeout, JSON / text-table parsers.
- TUI features: filter, sort, multi-select, status bar, key bindings
  (`q` quit, `r` rescan, `space` toggle, `a` toggle-all, `A`
  select-all-in-group, `enter` expand/collapse, `d` delete, arrows
  to move).
- GUI features: `Gtk.ListBox`-based grouped-by-type view with
  per-group collapsible `Gtk.Expander`, **Select all** button, status
  bar, Rescan / Delete Selected header buttons.
- CLI features: `version`, `engines`, `list` (human + `--json`),
  `delete` (with `--yes`).
- `Category::plural_label()` method for human-friendly section
  headers (e.g. `"Images"`, `"Build caches"`).
- `Category` now derives `PartialOrd` and `Ord` so it can be used
  as a `BTreeMap` / `BTreeSet` key.
- `ScanResult::by_category()` for the grouped-by-type UIs.

### Safety
- Active containers, running Flatpaks/Snaps, and in-use Ollama
  models are marked `Status::Active` and `is_safe_to_delete() == false`.
  The TUI/GUI refuse to toggle their checkboxes; the CLI refuses
  to delete them.
- Docker default networks (`bridge`, `host`, `none`) and Snap
  system snaps (`snapd`, `core`, `bare`, …) are protected and
  never surfaced for deletion.
- The application **never** touches the filesystem directly. All
  deletions go through the native engine (`docker rmi`, `flatpak
  uninstall`, etc.) to preserve database integrity.

[Unreleased]: https://example.com/systemprune-rs/compare/v0.1.0...HEAD
[0.1.0]: https://example.com/systemprune-rs/releases/tag/v0.1.0
