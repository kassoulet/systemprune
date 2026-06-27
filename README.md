# SystemPrune (Rust)

A unified, user-friendly Linux disk space cleaner for modern developer
environments. SystemPrune wraps the native CLI tools for **Docker**,
**Podman**, **Flatpak**, **Snap**, and **Ollama**, plus a set of
filesystem-aware scanners for **Node Modules**, **Python venvs**,
**Tox**, and **Mypy** caches. All of them are surfaced through a
single interface that analyzes disk usage and safely cleans up
unused assets.

## Features

- **Read-only analysis** — parses native CLI output (preferring JSON) to
  build a unified list of deletable assets with sizes and status.
- **Multi-engine** — Docker, Podman, Flatpak, Snap, Ollama, Node
  Modules, Python venvs, Tox, Mypy.
- **Safety guardrails** — never deletes active containers, running
  Flatpaks/Snaps, or models currently in use.
- **PATH probing** — engines whose CLI is not installed are silently
  disabled at startup.
- **Three frontends**:
  - **CLI** — non-interactive scriptable use.
  - **TUI** — Ratatui-based terminal interface with checkboxes and
    live progress.
  - **GUI** — GTK4 native interface via `gtk4-rs`.

## Workspace layout

This is a Cargo workspace with four members:

| Crate        | Type   | Purpose                                          |
| ------------ | ------ | ------------------------------------------------ |
| `core`       | lib    | Engine-agnostic models, parsers, scanners        |
| `cli`        | bin    | Non-interactive command-line interface (`systemprune`) |
| `tui`        | bin    | Ratatui terminal UI                              |
| `gui`        | bin    | GTK4 desktop UI (gtk4-rs + libadwaita)           |

All scanners and the orchestrator live in `core` and are reused by
both the TUI and the GUI.

## Installation

```bash
git clone <repo>
cd systemprune-rs
cargo build --release
```

The release binaries land in `target/release/`:

```bash
./target/release/systemprune --help
./target/release/systemprune-tui
./target/release/systemprune-gui
```

### Distribution

Cargo workspaces trivially produce a single static binary. You can
package the CLI/TUI/GUI together as:

- **CLI/TUI** — single self-contained ELF/Mach-O binary, install to
  `/usr/local/bin/`.
- **GUI** — depends on the system's GTK4 libraries at runtime; package
  with `cargo deb` or your distro's packaging tooling.

## Usage

### CLI

```bash
# List prunable items across all detected engines
systemprune list

# Limit to a specific engine
systemprune list --engine docker

# Delete by ID
systemprune delete docker:abc123 --yes

# Show detected engines
systemprune engines

# JSON output for scripting
systemprune list --json
```

### TUI

```bash
systemprune-tui
```

The TUI shows a unified table of items **grouped by type** (images,
containers, volumes, networks, apps, runtimes, models, etc.).
Each group is collapsible and has a **Select all** action.

| Key                      | Action                                         |
| ------------------------ | ---------------------------------------------- |
| <kbd>q</kbd> / <kbd>Esc</kbd> | Quit                                       |
| <kbd>r</kbd>              | Rescan                                         |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Move the cursor                              |
| <kbd>space</kbd>         | Toggle the current item (item rows only)       |
| <kbd>enter</kbd>         | Expand / collapse the group at the cursor      |
| <kbd>a</kbd>             | Toggle all (flat) across every safe item       |
| <kbd>A</kbd> (shift)     | Select all in the current group                |
| <kbd>d</kbd>             | Delete selected                                |

### GUI

```bash
systemprune-gui
```

A GTK4 native window built with `gtk4-rs` that shows the same
**grouped-by-type** view: one outer `Gtk.ListBox` of category
groups, each rendered as a `gtk::Expander` with a **Select all**
button in its label widget.

## Architecture

```
core/src/
├── lib.rs              # Public API re-exports
├── models.rs           # PrunableItem, Engine, Category, Status
├── size.rs             # parse_size / format_size
├── probe.rs            # PATH probing
├── orchestrator.rs     # Concurrent scanning & batched deletion
├── errors.rs           # Error types
└── scanners/
    ├── mod.rs          # all_scanners() registry
    ├── base.rs         # BaseScanner + shared subprocess helpers
    ├── fs_scan.rs      # Shared directory-walking helpers (find_dirs_named, …)
    ├── docker.rs
    ├── podman.rs
    ├── flatpak.rs
    ├── snap.rs
    ├── ollama.rs
    ├── node_modules.rs # Walks $HOME for `node_modules/` dirs
    ├── python_venv.rs  # Walks $HOME for Python venvs (.venv, venv, …)
    ├── tox.rs          # Walks $HOME for `.tox/` dirs
    └── mypy.rs         # Walks $HOME for `.mypy_cache/` dirs
```

### The `Scanner` trait

Every scanner implements six methods:

```rust
#[async_trait]
pub trait Scanner: Send + Sync {
    /// Stable source name (e.g. "docker", "node_modules").
    fn source(&self) -> &'static str;
    /// Native engine this scanner wraps.
    fn engine(&self) -> Engine;
    /// CLI binary on $PATH; used by the default `is_available`.
    fn binary(&self) -> &'static str;
    /// Returns `true` if the binary is on $PATH (default impl uses `which`).
    fn is_available(&self) -> bool { /* ... */ }
    /// Enumerate the prunable items this engine exposes.
    async fn get_items(&self) -> Result<Vec<PrunableItem>, EngineError>;
    /// Delete a single item via the engine's native CLI.
    async fn delete_item(&self, item: &PrunableItem) -> Result<(), EngineError>;
}
```

`is_available` has a default implementation that consults `which::which(self.binary())`,
so scanners that do nothing else (the dev-tool directory-walk scanners
in particular) only need to provide the other five.

### Safety

- Active containers, running Flatpaks/Snaps, and in-use Ollama models
  are marked with `Status::Active` and `is_safe_to_delete() == false`.
  The TUI/GUI refuse to toggle their checkboxes; the CLI refuses to
  delete them.
- The application **never** touches the filesystem directly. All
  deletions go through the native engine (`docker rmi`, `flatpak
  uninstall`, etc.) to preserve database integrity.

## Testing

```bash
cargo test --workspace
```

Tests cover the model, size parsing, PATH probe, and orchestrator
logic. Scanner parsers are unit-tested with fixture strings.

## License

MIT
