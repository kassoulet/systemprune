Here is a technical specification for **SystemPrune**, outlining its architecture, tech stack, and module boundaries.

This spec provides a clear roadmap for building the orchestrator and the frontend (TUI/GUI).

## 1. Executive Summary

**SystemPrune** is a unified, user-friendly Linux disk space cleaner focused specifically on modern developer environments and container runtimes. It provides an abstraction layer over disparate CLI tools (Docker, Podman, Flatpak, Snap, Ollama) to safely analyze disk usage and execute cleanup commands without risking data corruption or requiring manual terminal navigation.

## 2. Core Features

1. **Read-Only Analysis Engine:** Safely parses native CLI outputs (favoring JSON) to build a unified list of deletable assets, their sizes, and their status (e.g., active container vs. dangling image).
2. **Batch Orchestration:** Allows users to queue multiple cleanup actions across different environments and executes them asynchronously.
3. **Safety Guardrails:** Prevents deletion of active containers or currently running Flatpaks/Snaps. Requires explicit user confirmation.
4. **Multi-Interface Support:** Designed with a decoupled backend to support both a Terminal User Interface (TUI) and a Graphical User Interface (GUI).

---

## 3. Architecture & Data Flow

To ensure stability and prevent UI freezing during I/O intensive tasks (like `docker prune`), the application must strictly separate the scanning/deletion logic from the presentation layer.

> **Key Architectural Decision:** The application **never** interacts directly with the filesystem (e.g., `rm -rf /var/lib/docker`). All commands must be routed through the native package managers to preserve database integrity.

---

## 4. Tech Stack Recommendations

### Option A: The Python Stack (Recommended for Speed of Development)

Python is exceptionally strong at subprocess management and JSON parsing, making it ideal for a CLI orchestrator.

| Component | Technology | Rationale |
| --- | --- | --- |
| **Backend Logic** | Python 3.11+ | Built-in `subprocess` module and `asyncio` for non-blocking CLI calls. |
| **TUI Frontend** | Textual | Modern, async-first Python TUI framework with CSS styling. |
| **GUI Frontend** | PyGObject (GTK4) | Native integration with the GNOME desktop environment. |
| **Distribution** | PyPI / pipx / Flathub | `pipx` for isolated CLI installs, Flathub for the GUI. |

### Option B: The Rust Stack (Recommended for Performance & Distribution)

Rust creates a single, self-contained binary with strict memory safety and excellent concurrency.

| Component | Technology | Rationale |
| --- | --- | --- |
| **Backend Logic** | Rust | `std::process::Command` for execution, `serde_json` for parsing. |
| **TUI Frontend** | Ratatui | Extremely fast, reliable terminal interface library. |
| **GUI Frontend** | Tauri 2 | Allows building the UI with Web Tech (HTML/Tailwind) while using the Rust backend for system commands. |
| **Distribution** | Cargo / Distro Repos | Single binary distribution is trivial. |

---

## 5. API Contracts (The Ecosystem Wrappers)

Each module must implement a standard interface so the Core Orchestrator can handle them generically.

### The `BaseScanner` Interface

Every module must expose two primary methods:

1. `get_items() -> List[PrunableItem]`
2. `delete_item(id: str) -> Result<Success, Error>`

### Example Implementation Data Models (JSON Output Targets)

**Docker / Podman Module**

* **Command:** `docker images --format '{{json .}}'`
* **Parsing Target:** Map `Repository`, `Tag`, and `Size`.
* **Action Command:** `docker rmi <image_id>`

**Flatpak Module**

* **Command:** `flatpak list --app --columns=application,size,runtime` (requires custom parsing, Flatpak JSON output is limited).
* **Action Command:** `flatpak uninstall <application_id> --delete-data`

**Ollama Module**

* **Command:** `ollama list` (Currently outputs text tables; requires regex or strict column splitting).
* **Action Command:** `ollama rm <model_name>`

---

## 6. Execution Flow

1. **Initialization:** When SystemPrune launches, it probes the `$PATH` to see which engines exist (e.g., if `snap` is not found, the Snap module is disabled).
2. **Asynchronous Scanning:** The Orchestrator fires `get_items()` concurrently across all active modules.
3. **Data Aggregation:** The results are normalized into a unified `PrunableItem` schema and passed to the UI.
4. **User Selection:** The user checks off items to delete.
5. **Execution & Feedback:** The Orchestrator iterates through the selected items, triggering `delete_item()`. The UI displays a live progress bar based on successful return codes from the underlying system commands.
