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

---

## 1. Additional Engines

Real disk space wins live in the OS and developer-tool layer, not
just in container runtimes. The next batch of scanners targets the
biggest offenders.

### 1.1 systemd journald

* **Command:** `journalctl --disk-usage` (report), `journalctl
  --vacuum-size=500M` / `--vacuum-time=2weeks` (delete).
* **Source:** `journald`.
* **Category:** `logs`.
* **Item id:** virtual (e.g. `journald:vacuum-size:500M`).
* **Safety:** Never active — always safe to vacuum; the only
  configuration knob is the threshold.

### 1.2 Package manager caches

*Status: future work (not yet implemented.)*

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

### 1.3 Developer-tool caches

*Status: implemented in `core` and shipped by `systemprune-tui` /
`systemprune-gui` / `systemprune`. The scanners walk `$HOME` and
surface each cache as a discrete item.*

| Scanner        | Source         | What it finds                                     | `Category`            |
| -------------- | -------------- | ------------------------------------------------- | --------------------- |
| `NodeModules`  | `node_modules` | `node_modules/` directories (npm, yarn, pnpm)     | `Category::NodeModules` |
| `PythonVenv`   | `python_venv`  | Python virtualenvs (`.venv/`, `venv/`, …)         | `Category::PythonVenv`  |
| `Tox`          | `tox`          | `.tox/` directories created by the `tox` runner   | `Category::DependencyCache` |
| `Mypy`         | `mypy`         | `.mypy_cache/` directories created by `mypy`      | `Category::DependencyCache` |

* **Safety:** None of these are “active” in the sense of running
  processes, so they are always safe to delete. Some invalidate
  build artifacts (a fresh `npm install` / `mypy` re-run is
  required); the TUI/GUI surface this via the standard
  `is_safe_to_delete() == true` semantic rather than a separate
  “caution” flag.
* Per-item `extra` metadata:
  * `path` — absolute path of the cache directory.
  * `project_root` — the parent project the cache belongs to
    (used by the TUI status bar to give context).

### 1.4 Browser / thumbnail caches

*Status: future work (not yet implemented.)*

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

### 2.1 `prune` subcommand

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

### 2.2 Engine-native bulk passes

* `docker system prune -af` (containers, images, networks, build
  cache).
* `podman system prune -af` (same for Podman).
* `flatpak uninstall --unused -y` (unused runtimes).
* `snap set system refresh.retain=2` (limit old snap revisions).

Expose these as first-class subcommands rather than hiding them
behind a generic `prune`. Each has well-defined semantics that
differ across engines, and users expect them to behave like the
native CLI.

### 2.3 `--dry-run` global flag

* Print every item that *would* be deleted with its size and the
  command that would run.
* No subprocess is launched.
* Exit code 0 on success (even though nothing happened).

---

## 3. Filtering & Sorting

### 3.1 CLI flags (apply to `list` and `prune`)

| Flag              | Type      | Description                                  |
| ----------------- | --------- | -------------------------------------------- |
| `--engine`        | csv       | Limit to one or more engines                 |
| `--sort`          | enum      | `size` \| `name` \| `age` \| `source`        |
| `--desc`          | flag      | Reverse the sort order                       |
| `--older-than`    | duration  | Skip items newer than N (e.g. `30d`, `12h`)  |
| `--younger-than`  | duration  | Skip items older than N                      |
| `--name-pattern`  | glob      | Match item name against a glob               |
| `--min-size`      | bytes     | Skip items smaller than N                    |
| `--max-size`      | bytes     | Skip items larger than N                     |
| `--include-active`| flag      | Show `Status::Active` items too              |

Accepts human-friendly duration strings (`30d`, `12h`, `2w`).

### 3.2 TUI/GUI filtering

* Add a search box at the top of the items pane that filters by
  name (case-insensitive substring) in real time.
* Add a sort selector (dropdown in GUI, hotkey in TUI) bound to
  the same set of options as the CLI.

---

## 4. Disk Dashboard

A new first-run screen and CLI subcommand that answers "where is
my disk space going?".

### 4.1 `dashboard` subcommand

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

### 4.2 `df` subcommand

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

### 5.1 Deletion log

Every successful deletion is appended to
`$XDG_DATA_HOME/systemprune/history.json` (default
`~/.local/share/systemprune/history.json`):

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

### 5.2 `history` subcommand

```
$ systemprune history --limit 20
2026-06-27 14:32  docker     image    rust:bookworm          4.1 GB   ok
2026-06-27 14:31  ollama     model    qwen3.5:latest         4.3 GB   ok
2026-06-26 09:15  flatpak    app      org.gimp.GIMP          0.6 GB   failed
```

### 5.3 Undo (best-effort)

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

### 6.1 Location

`$XDG_CONFIG_HOME/systemprune/config.toml` (default
`~/.config/systemprune/config.toml`).

### 6.2 Schema

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

### 6.3 CLI override

Every config key can be overridden on the command line:

```
systemprune list --sort name
systemprune prune --no-confirm
```

### 6.4 `config` subcommand

* `systemprune config show` — print the effective config (defaults
  merged with user file, with the source of each value).
* `systemprune config edit` — open the config file in `$EDITOR`.
* `systemprune config path` — print the resolved config path.
* `systemprune config validate` — check the file for schema errors.

---

## 7. Scheduled Scans

### 7.1 `timer` subcommand

* `systemprune timer install [--interval weekly|daily] [--prune]`
  installs a systemd **user** timer (`~/.config/systemd/user/
  systemprune.timer` + `.service`) that runs `systemprune scan` (or
  `systemprune prune --dry-run` if `--prune` is set) on a schedule.
* `systemprune timer remove` — uninstall the timer.
* `systemprune timer status` — show next run time, last run, and
  journal entries.

### 7.2 Non-systemd hosts

If systemd is not available, fall back to a crontab fragment:

```
# m h dom mon dow command
0 3 * * 0 /usr/local/bin/systemprune scan --json --quiet
```

The user is shown the fragment and prompted to install it manually
(no automatic crontab mutation).

---

## 8. Notifications

* On scan completion: `notify-send "SystemPrune" "Found 14 items,
  18.2 GB total"` (libnotify).
* On `prune` completion: success / partial / failure toast with a
  "Show details" action that opens the GUI / dumps the report.
* Configurable via `notify_command` in the config file — default
  uses `notify-send`, but users can plug in `kdialog`, `zenity`,
  or a webhook.

---

## 9. Scripting API

### 9.1 Stable JSON schema

`list --json` and `delete --json` outputs are part of a versioned
schema:

```json
{
  "schema_version": "1.0",
  "items": [ /* PrunableItem[] */ ],
  "errors": [ /* EngineError[] */ ]
}
```

Breaking changes bump the major version. Additive changes (new
optional fields) are minor.

### 9.2 `list --csv`

CSV export with a fixed header for piping into `awk`, `xargs`, or
spreadsheets:

```
source,category,status,size_bytes,name
docker,image,unused,4398200000,rust:bookworm
ollama,model,unused,4620000000,qwen2.5:7b
```

### 9.3 Exit codes

| Code | Meaning                                          |
| ---- | ------------------------------------------------ |
| 0    | Success                                          |
| 1    | Generic failure (see stderr)                     |
| 2    | Refused to delete an active item without `--yes` |
| 3    | One or more deletions failed; partial success    |
| 4    | Configuration error                              |
| 5    | Unknown engine or scanner error                  |

### 9.4 Machine-readable `--quiet`

`systemprune --quiet <subcommand>` suppresses progress spinners and
human-friendly formatting, suitable for cron logs and CI.

---

## 10. Phased rollout

| Phase | Features                              | Target  | Status      |
| ----- | ------------------------------------- | ------- | ----------- |
| 1     | §2.1 `prune`, §2.3 `--dry-run`, §3.1  | MVP     | shipped     |
| 1.5   | §1.3 dev-tool caches (Node Modules, Python venv, Tox, Mypy) | 0.1.0 | shipped |
| 2     | §1.1 journald, §1.2 package caches    | v0.2    | planned     |
| 3     | §4 dashboard, §5 history              | v0.3    | planned     |
| 4     | §6 config                              | v0.4    | planned     |
| 5     | §7 timer, §8 notifications            | v0.5    | planned     |
| 6     | §1.4 browser caches, §2.2 engine prunes | v0.6   | planned     |
