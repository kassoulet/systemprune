# SystemPrune — Extended Features (`more.md`)

This document extends the original [`specs.md`](specs.md) with features
that were intentionally left out of the minimum viable product. It
serves as a roadmap for the next iteration of SystemPrune.

The original spec covers Docker, Podman, Flatpak, Snap, and Ollama with
a CLI / TUI / GUI triad. Everything below is **additive** — the
existing API and data model must remain backward compatible.

The Rust implementation is the canonical one. Future ports to
other languages or stacks must implement the same set of extended
features in lockstep.

> **See [`IMPLEMENTATION_STATUS.md`](IMPLEMENTATION_STATUS.md)
> for the per-item parity report.** The status markers used below
> match it:
> * ✅ / ⚠️ — implemented (partial)
> * ❌ — not implemented (only the sections marked ❌ here)
> * ➕ — bonus work added between the original spec and
>   `v0.1.0`
>
> All ✅ / ⚠️ / ➕ items below are validated against the current
> `v0.1.0` source. All ❌ items are still in the pipeline.

---

## 1. Additional Engines

Real disk space wins live in the OS and developer-tool layer, not
just in container runtimes. The next batch of scanners targets the
biggest offenders.

### 1.1 systemd journald (❌)

* **Command:** `journalctl --disk-usage` (report), `journalctl
  --vacuum-size=500M` / `--vacuum-time=2weeks` (delete).
* **Source:** `journald`.
* **Category:** `logs`.
* **Item id:** virtual (e.g. `journald:vacuum-size:500M`).
* **Safety:** Never active — always safe to vacuum; the only
  configuration knob is the threshold.

### 1.2 Package manager caches (❌)

| Engine     | Probe command        | Delete command            | Source      |
| ---------- | -------------------- | ------------------------- | ----------- |
| `apt`      | `apt-get --version`  | `apt-get clean`           | `apt`       |
| `dnf`      | `dnf --version`      | `dnf clean all`           | `dnf`       |
| `pacman`   | `pacman --version`   | `pacman -Sc --noconfirm`  | `pacman`    |
| `zypper`   | `zypper --version`   | `zypper clean -a`         | `zypper`    |

* Size reported via `du -sb /var/cache/{apt,dnf,pacman,zypp}`.
* Will be reported as a single `Category::DependencyCache` item per
  manager; deleting the cache item runs the appropriate `* clean`
  command.

> **Note.** The `cargo_cache` engine in §1.x below is **not** the
> same as the §1.2 package-manager caches — Cargo is a language
> toolchain's *build* cache, while §1.2 targets OS-level
> package-manager states.

### 1.3 Developer-tool caches (✅)

| Scanner        | Source         | What it finds                                     | `Category`            |
| -------------- | -------------- | ------------------------------------------------- | --------------------- |
| `NodeModules`  | `node_modules` | `node_modules/` directories (npm, yarn, pnpm)     | `Category::NodeModules` |
| `PythonVenv`   | `python_venv`  | Python virtualenvs (detected via `pyvenv.cfg`)    | `Category::PythonVenv`  |
| `Tox`          | `tox`          | `.tox/` directories created by the `tox` runner   | `Category::DependencyCache` |
| `Mypy`         | `mypy`         | `.mypy_cache/` directories created by `mypy`      | `Category::DependencyCache` |

* **Safety:** None of these are "active" in the sense of running
  processes, so they are always safe to delete. Some invalidate
  build artifacts (a fresh `npm install` / `mypy` re-run is
  required); the TUI/GUI surface this via the standard
  `is_safe_to_delete() == true` semantic rather than a separate
  "caution" flag.
* Per-item `extra` metadata:
  * `path` — absolute path of the cache directory.
  * `project_root` — the parent project the cache belongs to
    (used by the TUI status bar to give context).

### 1.x Developer-tool caches (bonus) — `v0.1.0` (➕)

Three more dev-tool / package-tool cache scanners were added
between the original spec and `v0.1.0`. They slot into the same
"filesystem-aware" idea as §1.3 but are listed separately because
each has its own quirks:

| Scanner         | Source         | What it finds                                                       | `Category`            | Action                       |
| --------------- | -------------- | ------------------------------------------------------------------- | --------------------- | ---------------------------- |
| `GoCache`       | `go_cache`     | The Go build cache (one item, from `go env GOCACHE`)                | `Category::BuildCache` | `go clean -cache`            |
| `Conda`         | `conda`        | Conda environments from `conda env list` (base env excluded)        | `Category::PythonVenv` | `conda env remove -p <path> -y` |
| `CargoCache`    | `cargo_cache`  | Two items in `$CARGO_HOME`: `registry/cache/` and `git/`            | `Category::BuildCache` | `trash::delete(<path>)` (per sub-dir) |

* **Safety:** All three are `Status::Unused` whenever reported
  (none can be "running"). The `conda` scanner explicitly skips
  the protected `base` env. The `cargo_cache` scanner is
  considered available iff at least one of `registry/cache/` or
  `git/` exists; a stray `$CARGO_HOME` with no sub-dirs does not
  surface anything.
* **Per-item `extra` metadata:** `path` for all three. `Conda`
  additionally stores `env_name`.

### 1.4 Browser / thumbnail caches (❌)

* `~/.cache/google-chrome/Default/Cache`
* `~/.cache/thumbnails/`
* `~/.cache/mozilla/firefox/*/cache2/`
* Source: `thumbnails`, `chromium`, `firefox`.
* Category: `Category::DependencyCache`.
* Safety: always safe (rebuilt on demand).

---

## 2. Bulk Operations

The original spec lists individual items one-by-one. Real cleanup
flows want "delete everything safe older than 30 days" or "free at
least 5 GB".

### 2.1 `prune` subcommand (❌)

```
systemprune prune [--engine docker,flatpak] [--older-than 30d]
                  [--min-size 100MB] [--dry-run] [--yes]
```

* Selects all items matching the filters that are safe to delete.
* Runs deletions in parallel with progress reporting.
* `--dry-run` prints the would-be deletion list and total size
  without touching anything.
* Returns exit code 0 only if every deletion succeeded; partial
  failures exit non-zero with a per-item summary on stderr.

> **Status.** This is the first item in §10's Phase 1, originally
> marked "shipped". In the actual `v0.1.0` binary it is **not**
> shipped — the CLI only exposes `version`, `engines`, `list`, and
> `delete`. The closest current workflow is "scan → select all in
> the TUI/GUI → press `d`". See
> [`IMPLEMENTATION_STATUS.md`](IMPLEMENTATION_STATUS.md#22-2-bulk-operations).

### 2.2 Engine-native bulk passes (❌)

* `docker system prune -af` (containers, images, networks, build
  cache).
* `podman system prune -af` (same for Podman).
* `flatpak uninstall --unused -y` (unused runtimes).
* `snap set system refresh.retain=2` (limit old snap revisions).

Expose these as first-class subcommands rather than hiding them
behind a generic `prune`. Each has well-defined semantics that
differ across engines, and users expect them to behave like the
native CLI.

### 2.3 `--dry-run` global flag (❌)

* Print every item that *would* be deleted with its size and the
  command that would run.
* No subprocess is launched.
* Exit code 0 on success (even though nothing happened).

> **Status.** Originally listed as "shipped" in §10 Phase 1.
> Not actually shipped in `v0.1.0`. The CLI's `--yes` only
> suppresses the confirmation prompt; it does not enable a
> dry-run path.

---

## 3. Filtering & Sorting

### 3.1 CLI flags (apply to `list` and `prune`) (mixed)

| Flag              | Type      | Status in `v0.1.0`   |
| ----------------- | --------- | --------------------- |
| `--engine`        | csv       | ✅ shipped on `list` |
| `--sort`          | enum      | ❌ not shipped         |
| `--desc`          | flag      | ❌ not shipped         |
| `--older-than`    | duration  | ❌ not shipped         |
| `--younger-than`  | duration  | ❌ not shipped         |
| `--name-pattern`  | glob      | ❌ not shipped         |
| `--min-size`      | bytes     | ❌ not shipped         |
| `--max-size`      | bytes     | ❌ not shipped         |
| `--include-active`| flag      | ⚠️ shipped as `--active` (name differs, semantics match) |

Accepts human-friendly duration strings (`30d`, `12h`, `2w`).

> The original §10 Phase 1 status of "shipped" was overstated
> against this list — only `--engine` and `--active` are wired up.
> The rest require §2.1 `prune` (which is also not shipped) so
> they cannot be retrofitted piecemeal without a redesign.

### 3.2 TUI/GUI filtering (mixed)

* **Sort selector** — ✅ shipped. The TUI cycles sort modes with
  the `s` key; the GUI uses a header-bar dropdown. Both use
  `core::sort::SortMode` (Default / Name ascending / Size
  descending / Size ascending). Cross-category order is preserved
  unchanged.
* **Search box** — ❌ not shipped. There is no top-of-items
  substring name filter in either UI.

---

## 4. Disk Dashboard

A new first-run screen and CLI subcommand that answers "where is
my disk space going?".

### 4.1 `dashboard` subcommand (✅ shipped)

```
$ systemprune dashboard
Engine      Items      Total size   Top item
docker        14        18.2 GB     rust:bookworm (4.1 GB)
ollama         6         9.7 GB     qwen2.5:7b  (4.3 GB)
flatpak        3         1.1 GB     org.gimp.GIMP (0.6 GB)
snap           2         0.4 GB     snapd (0.2 GB)
journald      -          2.0 GB     (vacuum to 500M → recover ~1.5 GB)
```

* TUI: a dedicated screen reached by pressing `d`.
* GUI: the landing page on first launch, with a "Show items" button
  that switches to the existing list view.
* Items are grouped by engine; each group shows the count, total
  size, and the single largest item.


#### Implementation notes (deviations)

* **TUI keybinding.**  Spec asked for press `d`.  Lowercase `d` is already taken by the existing "delete selected" binding, so the TUI uses `D` (shift+d) instead.  Reflected in the TUI module-level doc comment and in the on-toggle status text.  All other bindings unchanged.
* **GUI "Show items" button placement.**  Spec wording puts the button on the dashboard page itself.  The current implementation puts the toggle in the *header bar* (next to the existing sort dropdown) so the affordance is visible regardless of which view is currently active.  A literal-spec implementation can layer a second `Button` row at the top of the `dashboard_box` pane in `gui/src/window.rs::rebuild_dashboard_widgets`.
* **Grouping key.**  Items are grouped by `PrunableItem::source`, not by `Engine`.  In `v0.1.0` these are 1-to-1 for every shipped scanner, but the data side keeps the two distinct so a future scanner that exposes multiple sub-scanners (e.g. `docker.image` / `docker.container`) can surface them as separate rows.
* **`format_text` column widths.**  `Engine` auto-sizes to the longest source (clamped 8-20 chars); `Items` is right-aligned at 6 chars; `Total` is right-aligned at 9 chars; `Top item` is truncated to 60 chars (`TOP_CELL_WIDTH` in `core/src/orchestrator.rs`) with an ellipsis on overflow.  The header divider width is computed from the same constants so the table layout stays consistent.
* **Top item selection.**  Computed by `max_by_key(size_bytes)`; ties resolve to the first item the scanner reported.
* **`--json` envelope.**  `systemprune dashboard --json` emits a JSON array of `DashboardRow` (no wrapper object).  This differs from `list --json` shape (which uses an object with a `schema_version` field); pipe consumers should pick the right subcommand.
* **TUI dashboard sizing.**  Each line of the dashboard body is truncated to the frame width (minus the border padding) and ends with `\u{2026}` (ellipsis) on overflow.  This survives narrow terminals with column alignment intact; the only trade-off is that the top-item name is shortened to fit when the frame is narrower than `Engine + Items + Total + TopItem` widths (`TOP_CELL_WIDTH` is 60 in `core/src/orchestrator.rs`).  Earlier shipped variants (no `.wrap(...)` overflow shown raw, or `.wrap(Wrap { trim: false })` splitting rows mid-line) were considered regressions and replaced with this truncation in `tui/src/app.rs::draw_dashboard`.
### 4.2 `df` subcommand (❌)

Quick "df -h" replacement that also breaks down usage per engine:

```
$ systemprune df
Filesystem      Size  Used  Avail  Use%  Mounted on
/dev/sda1       500G  412G   88G   82%  /
  docker                   18.2 GB
  ollama                    9.7 GB
  flatpak                   1.1 GB
  snap                      0.4 GB
  journald                  2.0 GB
  unaccounted            382.6 GB
```

---

## 5. History & Audit

### 5.1 Deletion log (✅ shipped)

Every successful deletion is appended to
`$XDG_DATA_HOME/systemprune/history.json` (default
`~/.local/share/systemprune/history.json`).  The implementation
lives in `core::history` and is wired through the orchestrator's
`with_history(path)` builder (the CLI passes
`core::history::default_history_path()`).

```json
{
  "version": 1,
  "entries": [
    {
      "timestamp": "2026-06-27T14:32:11Z",
      "source": "docker",
      "category": "image",
      "id": "sha256:abc…",
      "name": "rust:bookworm",
      "size_bytes": 4398200000,
      "command": "docker rmi -f sha256:abc…",
      "exit_code": 0
    }
  ]
}
```

The log is rotated at 10 MB (keep last 5 files).

* **Schema version.**  The on-disk file carries an explicit
  `version` field (`History::VERSION = 1`).  A future bump
  would also require planners to handle v0 / v1 seams in the
  reader; today only v1 exists.
* **Atomic write.**  Each append is written to a sibling
  `history.json.tmp` and atomically renamed over the primary
  so a crash mid-write leaves the previous good file intact.
* **Rotation schedule.**  When a primary would exceed
  `DEFAULT_MAX_BYTES` (10 MB) after an append, `rotate()`
  shifts `history.json` → `.1` → `.2` → …, dropping the
  `.K` slot (`K = DEFAULT_KEEP_FILES`) so disk usage stays
  bounded at `5 × 10 MB = 50 MB` worst-case.
* **What gets recorded.**  Only deletions that actually
  reached the engine.  Refused items (Active / previously-
  failed / missing scanner) are orchestrator-level refusals,
  not engine interactions, so they never appear in the
  history.  A `--yes`-less refusal *does not* leave an
  audit-trail entry.
* **Per-item `command` field.**  Reconstructed from
  `(source, category)` via `core::history::command_for` so
  the audit log shows the exact command that ran (e.g.
  `docker rmi -f sha256:abc…`, `go clean -cache`,
  `trash /home/u/proj/node_modules`).  See
  `command_for` in `core/src/history.rs` for the full
  matrix.

### 5.2 `history` subcommand (✅ shipped)

The CLI subcommand from the spec is implemented verbatim:

```
$ systemprune history --limit 20
TIMESTAMP              SOURCE     CATEGORY   STATUS       SIZE  NAME
--------------------------------------------------------------------------------
2026-06-27T14:32:11Z   docker     image      ok       4.1 GiB  rust:bookworm
2026-06-27T14:31:42Z   ollama     model      ok       4.3 GiB  qwen3.5:latest
2026-06-26T09:15:33Z   flatpak    app        failed   0.6 GiB  org.gimp.GIMP
```

* `--limit N` (alias `-n`) limits the table to the most
  recent N entries; default 20.
* `--json` emits a pretty-printed JSON array instead of the
  human-readable table (also honours `--limit`).
* `--path` short-circuits and prints the resolved history
  file path (handy to verify `XDG_DATA_HOME` resolution).
* `--file <path>` overrides the resolved path entirely
  (useful for tests / debugging).

The reader validates `history.json` strictly: a missing
file prints `(history is empty: no deletions recorded)` and
exits 0; a corrupted file prints the parse error and exits
1 (rather than silently overwriting the user's audit
trail).

### 5.3 Undo (best-effort) (❌)

* `systemprune undo <id>` looks up the entry in the history log and
  attempts to reverse the action:
  * Re-pull a deleted Docker image if the original pull command is
    known.
  * Re-install a deleted Flatpak / Snap.
  * Restore a deleted Ollama model from a configured backup
    directory (only if the user has set up a model mirror).
* If undo is not possible, the subcommand exits non-zero with a
  clear message. No silent failures.

---

## 6. Configuration

### 6.1 Location (❌)

`$XDG_CONFIG_HOME/systemprune/config.toml` (default
`~/.config/systemprune/config.toml`).

### 6.2 Schema (❌)

```toml
version = 1

# Per-engine enable/disable (overrides PATH probe)
[engines]
docker         = true
podman         = true
flatpak        = true
snap           = true
ollama         = true
node_modules   = true
python_venv    = true
tox            = true
mypy           = true
apt            = true   # future
dnf            = false  # future

# Default sort order for list / dashboard
sort = "size"     # one of: size, name, age, source
desc = true

# Safety
confirm_bulk = true    # require --yes to skip the prompt
protected_docker_images = ["my-critical-app:prod"]
protected_ollama_models = []
protected_flatpak_apps  = []

# Notifications
notify_on_complete = true
notify_command     = "notify-send"

# History
history_max_bytes  = 10485760   # 10 MB
history_keep_files = 5

# Undo support
ollama_backup_dir  = "/var/backups/ollama"   # empty = disabled
```

### 6.3 CLI override (❌)

Every config key can be overridden on the command line:

```
systemprune list --sort name
systemprune prune --no-confirm
```

### 6.4 `config` subcommand (❌)

* `systemprune config show` — print the effective config (defaults
  merged with user file, with the source of each value).
* `systemprune config edit` — open the config file in `$EDITOR`.
* `systemprune config path` — print the resolved config path.
* `systemprune config validate` — check the file for schema errors.

> **Status.** Nothing in §6 is shipped in `v0.1.0`. The TUI
> binary accepts `--config <path>` but does not use it.
> Engine enable/disable is currently driven solely by PATH
> probing (`which::which(self.binary())` plus, for
> `cargo_cache`, the existence of `$CARGO_HOME/registry/cache/`
> and/or `$CARGO_HOME/git/`).

---

## 7. Scheduled Scans

### 7.1 `timer` subcommand (❌)

* `systemprune timer install [--interval weekly|daily] [--prune]`
  installs a systemd **user** timer (`~/.config/systemd/user/
  systemprune.timer` + `.service`) that runs `systemprune scan` (or
  `systemprune prune --dry-run` if `--prune` is set) on a schedule.
* `systemprune timer remove` — uninstall the timer.
* `systemprune timer status` — show next run time, last run, and
  journal entries.

### 7.2 Non-systemd hosts (❌)

If systemd is not available, fall back to a crontab fragment:

```
# m h dom mon dow command
0 3 * * 0 /usr/local/bin/systemprune scan --json --quiet
```

The user is shown the fragment and prompted to install it manually
(no automatic crontab mutation).

---

## 8. Notifications (❌)

* On scan completion: `notify-send "SystemPrune" "Found 14 items,
  18.2 GB total"` (libnotify).
* On `prune` completion: success / partial / failure toast with a
  "Show details" action that opens the GUI / dumps the report.
* Configurable via `notify_command` in the config file — default
  uses `notify-send`, but users can plug in `kdialog`, `zenity`,
  or a webhook.

---

## 9. Scripting API

### 9.1 Stable JSON schema (✅)

`list --json` and `delete --json` outputs are part of a versioned
schema. As of `v0.1.0` only `list --json` is wired up; it
produces a pretty-printed JSON array of `PrunableItem::as_dict()`:

```json
{
  "schema_version": "1.0",
  "items": [ /* PrunableItem[] */ ],
  "errors": [ /* EngineError[] */ ]
}
```

Each `PrunableItem` is emitted as:

```json
{
  "id": "sha256:abc…",
  "name": "rust:bookworm",
  "engine": "docker",
  "source": "docker",
  "category": "image",
  "size_bytes": 4398200000,
  "status": "unused",
  "is_safe_to_delete": true,
  "extra": { "repository": "rust", "tag": "bookworm" }
}
```

Breaking changes bump the major version. Additive changes (new
optional fields) are minor.

### 9.2 `list --csv` (❌)

CSV export with a fixed header for piping into `awk`, `xargs`, or
spreadsheets:

```
source,category,status,size_bytes,name
docker,image,unused,4398200000,rust:bookworm
ollama,model,unused,4620000000,qwen2.5:7b
```

### 9.3 Exit codes (mixed)

| Code | Meaning                                          | Status in `v0.1.0` |
| ---- | ------------------------------------------------ | --- |
| 0    | Success                                          | ✅ |
| 1    | Generic failure (see stderr)                     | ✅ |
| 2    | Refused to delete an active item without `--yes` | ✅ |
| 3    | One or more deletions failed; partial success    | ⚠️ (currently reported as 1) |
| 4    | Configuration error                              | ❌ |
| 5    | Unknown engine or scanner error                  | ❌ |

> **Note.** Code 2 is the one the CLI explicitly distinguishes
> today (matching the spec wording exactly — "Refusing to delete
> active items without --yes"). Codes 3/4/5 collapse into the
> generic failure path (1) and need a small `ExitCode::from(...)`
> refactor in `cli/src/main.rs` to match the spec perfectly.

### 9.4 Machine-readable `--quiet` (❌)

`systemprune --quiet <subcommand>` suppresses progress spinners and
human-friendly formatting, suitable for cron logs and CI.

> The CLI is already pretty quiet (no progress spinners, no
> pretty colours), but the explicit `--quiet` knob is not
> separately wired.

---

## 10. Phased rollout

The phase table from the original draft was inaccurate for
**Phase 1**: §2.1 `prune` and §2.3 `--dry-run` were marked
"shipped" but were not actually shipped. Below is the corrected
table for `v0.1.0`. The bonus scanners in §1.x above are folded
into Phase 1.5 because they slot into the same dev-tool /
package-tool niche.

| Phase | Features                                                                | Target  | Status (actual) |
| ----- | ----------------------------------------------------------------------- | ------- | --- |
| 1     | §2.1 `prune`, §2.3 `--dry-run`, §3.1 (sort/desc/older-than/younger-than/name-pattern/min-size/max-size) | MVP | ⚠️ partial: only `--engine` and `--active` shipped as CLI filter flags; the rest are gated on `prune` and not shipped. |
| 1.5   | §1.3 dev-tool caches (Node Modules, Python venv, Tox, Mypy)             | 0.1.0   | ✅ shipped (extended with §1.x Go/Conda/Cargo as bonus) |
| 2     | §1.1 journald, §1.2 package caches                                       | v0.2    | ❌ not shipped |
| 3     | §4 dashboard, §5 history                                                 | v0.3    | ❌ not shipped |
| 4     | §6 config                                                                 | v0.4    | ❌ not shipped |
| 5     | §7 timer, §8 notifications                                                | v0.5    | ❌ not shipped |
| 6     | §1.4 browser caches, §2.2 engine prunes                                   | v0.6    | ❌ not shipped |
