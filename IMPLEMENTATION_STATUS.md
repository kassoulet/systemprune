# SystemPrune — Implementation Status Report

This report compares [`specs.md`](specs.md) and [`more.md`](more.md)
against the current Rust implementation (workspace HEAD, `v0.1.0`).
Each spec item is marked:

* **✅** — implemented and verified against source code
* **⚠️** — partially implemented (see note)
* **❌** — not implemented
* **➕** — implemented but not in the original spec (bonus work)

The corresponding rows in [`specs.md`](specs.md) and
[`more.md`](more.md) have been updated where the actual
implementation diverges from the original draft. In particular,
[`more.md` §10 Phased rollout](more.md#10-phased-rollout) has
been corrected to match this report.

---

## 1. `specs.md` (Technical specification)

The original spec covers the orchestrator + frontend triad. Every
numbered claim is implemented.

| § | Status | Implementation notes |
| --- | :--: | --- |
| §1 Executive summary | ✅ | Top-level pitch matches reality. |
| §2.1 Read-only analysis engine | ✅ | `Scanner::get_items` parses native CLI output (favours JSON; falls back to regex / column splitting). |
| §2.2 Batch orchestration | ✅ | `Orchestrator::delete_many` runs each item concurrently; returned `Vec<DeleteResult>` preserves caller order via pre-allocated slots plus oneshot channels. |
| §2.3 Safety guardrails | ✅ | `Status::Active` and `Status::Deleted` short-circuit `is_safe_to_delete()`. Orchestrator also refuses items present in the caller's `delete_errors` map (defence in depth). |
| §2.4 Multi-interface support | ✅ | `cli` (`systemprune`), `tui` (`systemprune-tui`, Ratatui), `gui` (`systemprune-gui`, libadwaita) all consume `systemprune-core`. |
| §3 Architecture / data flow | ✅ | Async separation via `tokio::process::Command` wrapped in `BaseScanner::run` with a `tokio::time::timeout`. The application **never** touches the filesystem directly — every deletion goes through the engine's native CLI (`docker rmi`, `flatpak uninstall --delete-data`, `ollama rm`, `go clean -cache`, `conda env remove -p`, etc.). |
| §4 Tech stack | ✅ | Backend = Rust, TUI = Ratatui, GUI = gtk4-rs + libadwaita. Distribution is a Cargo workspace producing three binaries from `cli/`, `tui/`, `gui/`. |
| §5 API contract (`Scanner` trait) | ✅ | `pub trait Scanner` in `core/src/scanners/mod.rs` matches the spec listing byte-for-byte (six methods, default `is_available` via `which::which`). |
| §6 Execution flow | ✅ | `Orchestrator::new` drops scanners whose CLI is not on `$PATH`; `scan_all` issues `get_items` concurrently and aggregates into `ScanResult`; the UI picks items and `delete_many` runs the selection in parallel. |

**Result.** `specs.md` §1–§6 is fully implemented.

---

## 2. `more.md` (Extended features)

### 2.1 §1 Additional engines

| § | Status | Implementation notes |
| --- | :--: | --- |
| §1.1 systemd journald | ❌ | Not in `all_scanners()`; no `journalctl --disk-usage` / `--vacuum-size<size>` / `--vacuum-time<time>` plumbing. |
| §1.2 Package-manager caches (`apt` / `dnf` / `pacman` / `zypper`) | ❌ | No scanners registered. These target OS-level package-manager states; not to be confused with the bonus Cargo cache scanner (§1.x below). |
| §1.3 Dev-tool caches — *in spec* | ✅ | `NodeModulesScanner`, `PythonVenvScanner` (via `pyvenv.cfg` marker), `ToxScanner` (`.tox/`), `MypyScanner` (`.mypy_cache/`). All walk `$HOME` and surface every match as a discrete `PrunableItem`. Deletion uses `trash::delete` so it is recoverable. |
| §1.3 (bonus) Go build cache | ➕ | `GoCacheScanner` resolves the cache via `go env GOCACHE` and reports one `Category::BuildCache` item. Deletion runs `go clean -cache`. |
| §1.3 (bonus) Conda environments | ➕ | `CondaScanner` enumerates envs via `conda env list`, **skips the protected `base` env**, sizes each via `dir_size`, and deletes via `conda env remove -p <path> -y`. |
| §1.x (bonus) Cargo build cache | ➕ | `CargoCacheScanner` resolves `$CARGO_HOME` (default `~/.cargo/`), surfaces two `Category::BuildCache` items: `registry/cache/` (pre-downloaded crate tarballs) and `git/` (git-based dependency clones). Deletion uses `trash::delete` (Cargo has no built-in cache eviction). |
| §1.4 Browser / thumbnail caches | ❌ | Per spec, future work. No scanner for `~/.cache/google-chrome/`, `~/.cache/thumbnails/`, or `~/.cache/mozilla/firefox/*/cache2/`. |

### 2.2 §2 Bulk operations

| § | Status | Implementation notes |
| --- | :--: | --- |
| §2.1 `prune` subcommand | ❌ | The CLI exposes only `version`, `engines`, `list`, and `delete`. There is no `prune` subcommand, no `--older-than` filter, no `--min-size` filter, no batch "auto-prune". The nearest workflow is "scan → select all in TUI/GUI → press `d`". |
| §2.2 Engine-native bulk passes | ❌ | No invocation of `docker system prune -af`, `podman system prune -af`, `flatpak uninstall --unused -y`, or `snap set system refresh.retain=2`. |
| §2.3 Global `--dry-run` flag | ❌ | No `--dry-run` flag exists; deletions always invoke the engine. The CLI's `--yes` is the closest equivalent (suppresses just the confirmation prompt). |

### 2.3 §3 Filtering & sorting

| § | Status | Implementation notes |
| --- | :--: | --- |
| §3.1 `--engine` | ✅ | Wired on `list` (`--engine docker`, etc.). Items are post-filtered against the field before rendering. |
| §3.1 `--include-active` | ⚠️ | Implemented under the slightly different flag name `--active`. Semantics match the spec exactly (items with `Status::Active` are kept). |
| §3.1 `--sort` / `--desc` / `--older-than` / `--younger-than` / `--name-pattern` / `--min-size` / `--max-size` | ❌ | None of these CLI flags exist. |
| §3.2 TUI/GUI sort selector | ✅ | `core::sort::SortMode` (Default / Name ascending / Size descending / Size ascending). TUI: cycle with `s` key. GUI: dropdown in the header bar. Both preserve the cross-category first-seen order. |
| §3.2 TUI/GUI search box | ❌ | No substring name filter; only sort. |

### 2.4 §4 Disk dashboard

| § | Status | Implementation notes |
| --- | :--: | --- |
| §4.1 `dashboard` subcommand + landing page | ✅ shipped (with documented deviations) | CLI `systemprune dashboard [--engine NAME] [--json]` runs a fresh `scan_all`, groups items by source via `Dashboard::compute_items(&ScanResult::by_engine)`, sorts rows by `total_bytes` desc with `source` asc tie-break, prints either a fixed-width text table or a JSON array.  TUI dashboard reached by `D` (shift+d); the spec asked for lowercase `d` but that key is reserved for the existing "delete selected" binding, so a deviation is recorded here and in `more.md`.  GUI dashboard is the spec-mandated landing page (initial visibility sets `items_scroll.set_visible(false)` / `dashboard_scroll.set_visible(true)`); a header-bar "Show items" / "Show dashboard" toggle button flips views (`on_view_toggle_clicked`).  A future contributor who wants the literal spec wording of "Show items" *on the dashboard page itself* can add a `Button` row at the top of `dashboard_box` \u2014 the header button satisfies the spec functionally.  `do_scan` rebuilds both per-category groups (`rebuild_groups`) and the dashboard pane (`rebuild_dashboard_widgets`) inside a single `with_rebuilding` wrap following the existing `panic.txt` defensive pattern. |
| §4.2 `df` subcommand | ❌ | Not implemented. |

### 2.5 §5 History & audit

| § | Status | Implementation notes |
| --- | :--: | --- |
| §5.1 Persisted `history.json` | ✅ | `core::history::History` (re-exported as `systemprune_core::history::History`) flushes the §5.1 schema to `$XDG_DATA_HOME/systemprune/history.json` (default `~/.local/share/systemprune/history.json`) with 10 MB rotation (`DEFAULT_MAX_BYTES`) keeping `history.json` + `.1` … `.4` (`DEFAULT_KEEP_FILES = 5`). Atomicity: every append is written to a sibling `*.tmp` and renamed over the primary. Corrupted files surface as errors rather than silent overwrites. The `ActionLog` still exists for the in-memory action trace consumed by the TUI/GUI; the persistent deletion log is a separate concern handled by `Orchestrator::with_history`. |
| §5.2 `history` subcommand | ✅ | Implemented alongside `with_history`. `systemprune history [--limit N] [--json] [--path] [--file <path>]` reads back through `History::load`. Includes `--limit` defaults from `DEFAULT_HISTORY_LIMIT = 20` matching the §5.2 spec example. `--path` prints the resolved file path; `--file` overrides it for tests/debugging. |
| §5.3 `undo` subcommand | ❌ | Not implemented. |

### 2.6 §6 Configuration

| § | Status | Implementation notes |
| --- | :--: | --- |
| §6.1–6.4 `config.toml` + `config show/edit/validate` | ❌ | The TUI binary accepts `--config <path>` but the flag is currently unused. No TOML loader; no env override. |

### 2.7 §7 Scheduled scans

| § | Status | Implementation notes |
| --- | :--: | --- |
| §7.1 `timer install/remove/status` | ❌ | Not in CLI. No `~/.config/systemd/user/systemprune.{timer,service}` generator. |
| §7.2 Crontab fragment fallback | ❌ | Not in CLI. |

### 2.8 §8 Notifications

| § | Status | Implementation notes |
| --- | :--: | --- |
| §8 libnotify / `notify_command` | ❌ | No `notify-send` / `kdialog` / `zenity` integration; no `notify_command` knob (also absent from the non-existent config file). |

### 2.9 §9 Scripting API

| § | Status | Implementation notes |
| --- | :--: | --- |
| §9.1 `list --json` (stable schema) | ✅ | Emits a JSON array of `PrunableItem::as_dict()`. The schema covers `id` / `name` / `engine` / `source` / `category` / `size_bytes` / `status` / `is_safe_to_delete` / `extra` (a `string -> string` map of engine-specific metadata such as `repository` / `tag` / `path`). |
| §9.2 `list --csv` | ❌ | No `--csv` flag. |
| §9.3 Exit codes 0 / 1 / 2 | ⚠️ | CLI returns 0 on success, 1 on missing IDs / transaction-failure, and **2 specifically when refusing to delete `Status::Active` items without `--yes`** (this last one matches the spec exactly). Codes 3 / 4 / 5 are not distinguished. |
| §9.4 `--quiet` | ❌ | The CLI is already minimal (no progress spinners), but the explicit `--quiet` knob is not wired. |

### 2.10 §10 Phased rollout

Re-mapping the §10 phase groupings to actual status. The
original table in `more.md` marked §2.1 and §2.3 as "shipped";
the updated `more.md` corrects this.

| Phase | Feature § | Original status | Actual status |
| --- | --- | --- | :--: |
| 1 | §2.1 `prune` subcommand | shipped | ❌ not shipped |
| 1 | §2.3 `--dry-run` | shipped | ❌ not shipped |
| 1 | §3.1 CLI filter/sort flags | shipped | ⚠️ only `--engine` + `--active` shipped; sort/time/glob/byte filters not. |
| 1.5 | §1.3 dev-tool caches (Node Modules, Python venv, Tox, Mypy) | shipped | ✅ shipped + extended with Go, Conda, Cargo. |
| 2 | §1.1 journald | planned | ❌ still not implemented |
| 2 | §1.2 package-manager caches | planned | ❌ still not implemented (separate from the new Cargo cache scanner). |
| 3 | §4 dashboard | planned | ✅ shipped (CLI dashboard subcommand + TUI `D` screen + GUI landing page).  §4.2 `df` subcommand is the only remaining sub-section of Phase 3. |
| 3 | §5 history, §5.2 history subcommand | planned (early) | ✅ shipped ahead of dashboard: `core::history::History` flushes §5.1 schema to `$XDG_DATA_HOME/systemprune/history.json` with 10 MB rotation; CLI exposes `systemprune history [--limit N] [--json] [--path] [--file <path>]`. `§5.3 undo` still ❌. |
| 4 | §6 config | planned | ❌ still not implemented |
| 5 | §7 timer | planned | ❌ still not implemented |
| 5 | §8 notifications | planned | ❌ still not implemented |
| 6 | §1.4 browser caches | planned | ❌ still not implemented |
| 6 | §2.2 engine-native bulk passes | planned | ❌ still not implemented |

**Net of `v0.1.0`:** the shipped surface is essentially the
original core spec (`specs.md §1–§6`), an in-memory action log
that the TUI/GUI can display, sort-within-group, post-delete
failure tracking in the GUI/TUI (re-queue prevention), and three
bonus dev-tool / package-tool cache scanners (Go build cache via
`go clean -cache`, Conda envs via `conda env remove`, Cargo build
cache via `trash::delete`) that extend the §1.3 dev-tool idea.

---

## 3. Suggested follow-up work, in priority order

1. **§2.1 `prune` subcommand** — biggest user-visible gap. The
   orchestrator already supports batch deletion; the work is
   wiring up the CLI surface and the §3.1 filter flags.
2. *(removed — shipped in `v0.1.0`.)*
3. **§4.1 `dashboard` subcommand** — `ScanResult::by_engine()`
   already groups items, so the data side is there.
4. **§2.3 `--dry-run`** — small implementation; the orchestrator
   would short-circuit `delete_many` and print the target list
   instead of invoking the engine.
5. **§1.1 journald** — adds a non-engine scanner that closely
   mirrors `GoCacheScanner` in shape.

---

## 4. What this report is not

* **Not a changelog.** Per-release notes live in `CHANGELOG.md`.
* **Not a design document.** The original intent lives in
  `specs.md` and `more.md`; those documents are the source of
  truth. This report only annotates them.
* **Not a roadmap.** Decisions about what to build next belong
  in the project's issue tracker.
