Here is a technical specification for **SystemPrune**, outlining its architecture, tech stack, and module boundaries.

This spec provides a clear roadmap for building the orchestrator and the frontend (TUI/GUI).

> **See [`IMPLEMENTATION_STATUS.md`](IMPLEMENTATION_STATUS.md) for the
> per-item parity report** that maps every section of this document
> (and [`more.md`](more.md)) to the current Rust code. The status
> notations used there are:
>
> * ✅ implemented and verified
> * ⚠️ partially implemented (see note)
> * ❌ not implemented
> * ➕ implemented beyond the original spec (bonus work)
>
> All §1–§7 sections below are marked ✅ in the parity report.

## 1. Executive Summary

**SystemPrune** is a unified, user-friendly Linux disk space cleaner focused specifically on modern developer environments and container runtimes. It provides an abstraction layer over disparate CLI tools (Docker, Podman, Flatpak, Snap, Ollama) and a set of filesystem-aware scanners (Node Modules, Python venvs, Tox, Mypy caches, Go build cache, Conda envs, Cargo build cache) to safely analyze disk usage and execute cleanup commands without risking data corruption or requiring manual terminal navigation.

## 2. Core Features

1. **Read-Only Analysis Engine:** Safely parses native CLI outputs (favoring JSON) to build a unified list of deletable assets, their sizes, and their status (e.g., active container vs. dangling image).
2. **Batch Orchestration:** Allows users to queue multiple cleanup actions across different environments and executes them asynchronously.
3. **Safety Guardrails:** Prevents deletion of active containers or currently running Flatpaks/Snaps. Requires explicit user confirmation. Active items and items that previously failed to delete (recorded in the orchestrator's `delete_errors` map) are both refused by both the orchestrator and the UIs.
4. **Multi-Interface Support:** Designed with a decoupled backend to support a Terminal User Interface (TUI) and a Graphical User Interface (GUI).

---

## 3. Architecture & Data Flow

To ensure stability and prevent UI freezing during I/O intensive tasks (like `docker prune`), the application must strictly separate the scanning/deletion logic from the presentation layer.

> **Key Architectural Decision:** The application **never** interacts directly with the filesystem (e.g., `rm -rf /var/lib/docker`). All commands must be routed through the native package managers to preserve database integrity.

---

## 4. Tech Stack Recommendations

### The Rust Stack (Implemented)

Rust creates a single, self-contained binary with strict memory safety and excellent concurrency.

| Component | Technology | Implementation notes |
| --- | --- | --- |
| **Backend Logic** | Rust | `tokio::process::Command` (with `tokio::time::timeout`) wrapped by `BaseScanner::run` in `core/src/scanners/base.rs`. `serde_json` for JSON parsing; `regex` / `once_cell` for text-table parsing when JSON is unavailable. |
| **TUI Frontend** | Ratatui | Ratatui 0.27 + Crossterm 0.28 backend. Reads `core::ScanResult`, keeps its own selection state, calls `core::Orchestrator::delete_many`. |
| **GUI Frontend** | gtk4-rs + libadwaita | Native GTK4 / libadwaita window using the same Rust backend; renders the same grouped-by-category view as the TUI (`ExpanderRow`s inside a `ToolbarView`). |
| **Distribution** | Cargo workspace | Four-crate workspace (`core`, `cli`, `tui`, `gui`) producing three binaries: `systemprune`, `systemprune-tui`, `systemprune-gui`. Single-binary distribution of the CLI is trivial; the GUI needs GTK4 + libadwaita at runtime. |

---

## 5. API Contracts (The Ecosystem Wrappers)

Each module implements a standard interface so the Core Orchestrator can handle them generically.

### The `BaseScanner` Interface

Every module implements a single Rust trait so the Core Orchestrator
can handle them generically. The trait exposes six methods:

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

`is_available` has a default implementation that consults
`which::which(self.binary())`, so scanners that have no CLI
binary to probe (the dev-tool directory-walk scanners in
particular) only need to provide the other five.

### Example Implementation Data Models (JSON Output Targets)

**Docker / Podman Module**

* **Command:** `docker images -a --format '{{json .}}'`
* **Parsing Target:** Map `Repository`, `Tag`, and `Size`. Per-volume sizes come from a separate `docker system df -v` lookup so the GUI can show how much of the total each volume occupies.
* **Action Command:** `docker rmi -f <image_id>` / `docker rm -f <container_id>` / `docker volume rm <name>` / `docker network rm <id>` (the orchestrator picks the right one based on `item.category`).
* **Safety:** Images used by a running container (`Status::Active`); default networks `bridge`/`host`/`none` are never surfaced for deletion.

**Flatpak Module**

* **Command:** `flatpak list --app --columns=application,size,runtime` (requires custom parsing — Flatpak JSON output is limited). Runtimes are listed with `--runtime --columns=application,size,runtime,arch,branch`.
* **Action Command:** `flatpak uninstall --delete-data -y <application_id>` (`--delete-data` also drops the per-user data directory).

**Ollama Module**

* **Command:** `ollama list` (Currently outputs text tables; Primary parser is a regex line matcher; column-splitting fallback handles irregular whitespace).
* **Action Command:** `ollama rm <model_name>`. Models currently loaded (`ollama ps`) are marked `Status::Active`.

**Go build cache (bonus)**

* **Command:** `go env GOCACHE`
* **Action Command:** `go clean -cache` (delegated to `go` so concurrent invocations stay safe).

**Conda envs (bonus)**

* **Command:** `conda env list`
* **Action Command:** `conda env remove -p <path> -y`. The protected `base` env is never surfaced for deletion.

**Cargo build cache (bonus)**

* **Directory:** `$CARGO_HOME` (default `~/.cargo/`).
* **Action:** Per-sub-directory `trash::delete` of `registry/cache/` (pre-downloaded crate tarballs) and `git/` (git-based dependency clones). Sizing is done by walking the directory.

---

## 6. Execution Flow

1. **Initialization:** When SystemPrune launches, it probes the `$PATH` to see which engines exist (e.g., if `snap` is not found, the Snap module is disabled). The `Orchestrator::new` constructor drops any scanner whose `is_available()` returns false before exposing the rest to the callers. Each scanner's `is_available` is consulted independently of the others; nothing short-circuits.
2. **Asynchronous Scanning:** The Orchestrator fires `get_items()` concurrently across all active modules via a `tokio::task::JoinSet`; results are appended to `ScanResult::items` in completion order. **Order preservation is `delete_many`'s job, not `scan_all`'s**: only the deletion path uses pre-allocated result slots plus oneshot channels to guarantee that the returned `Vec<DeleteResult>` lines up with the caller's input order.
3. **Data Aggregation:** The results are normalized into a unified `PrunableItem` schema (`core::models`) and passed to the UI.
4. **User Selection:** The user checks off items to delete in the TUI/GUI or supplies `source:id` pairs on the CLI.
5. **Execution & Feedback:** The Orchestrator iterates through the selected items, triggering `delete_item()` concurrently. Failures (engine non-zero exit, refusal of an active item, missing scanner for the source) are returned per-item as `DeleteResult { item, success, error }` and surfaced to the UI. The orchestrator pushes entries to a shared `ActionLog` (`core::log::ActionLog`) at each scan/delete boundary; the TUI's `l` key and the GUI's hamburger **View Log** entry render that log.

---

## 7. Scanners shipped by `v0.1.0`

The `core::scanners::all_scanners()` registry returns every scanner below on every run; the orchestrator discards the ones whose CLI is not on `$PATH` (or, for the dev-tool walkers whose `is_available` returns `true` always, all of them are kept).

| Source         | Engine        | binary()             | Deletion via                                |
| -------------- | ------------- | -------------------- | ------------------------------------------- |
| `docker`       | `Docker`      | `docker`             | `docker rmi -f / rm -f / volume rm / network rm` |
| `podman`       | `Podman`      | `podman`             | `podman rmi -f / rm -f / volume rm / network rm` |
| `flatpak`      | `Flatpak`     | `flatpak`            | `flatpak uninstall --delete-data -y`        |
| `snap`         | `Snap`        | `snap`               | `snap remove` (system snaps `snapd` / `core*` / `bare` refused) |
| `ollama`       | `Ollama`      | `ollama`             | `ollama rm`                                 |
| `node_modules` | `NodeModules` | `node` (label only)  | `trash::delete(<path>)`                     |
| `python_venv`  | `PythonVenv`  | `python3` (label only) | `trash::delete(<path>)`                    |
| `tox`          | `Tox`         | `tox` (label only)   | `trash::delete(<path>)`                     |
| `mypy`         | `Mypy`        | `mypy` (label only)  | `trash::delete(<path>)`                     |
| `go_cache`     | `GoCache`     | `go`                 | `go clean -cache`                           |
| `conda`        | `Conda`       | `conda`              | `conda env remove -p <path> -y`             |
| `cargo_cache`  | `CargoCache`  | `cargo` (label only)               | `trash::delete(<path>)`         |

> **Footnote — `is_available` overrides.** The dev-tool directory-walk
> scanners (`node_modules`, `python_venv`, `tox`, `mypy`) override
> `Scanner::is_available` to return `true` unconditionally — the
> `binary()` value is a label, not a probe target. `cargo_cache`
> overrides it to return `true` iff at least one of
> `$CARGO_HOME/registry/cache/` or `$CARGO_HOME/git/` exists. Every
> other row above uses the trait's default `is_available`, which
> consults `which::which(self.binary())`.

The first five rows (Docker / Podman / Flatpak / Snap / Ollama)
correspond to the engines named in the §1 Executive Summary. The
next four (`node_modules`, `python_venv`, `tox`, `mypy`) are the
§1.3 dev-tool caches from [`more.md`](more.md). The last three
(`go_cache`, `conda`, `cargo_cache`) are bonus scanners added
between the original spec and `v0.1.0`; see the parity report for
details.
