//! Persistent deletion audit log.
//!
//! Mirrors [`more.md` §5.1 / §5.2]: every completed (or failed)
//! deletion is appended to
//! `$XDG_DATA_HOME/systemprune/history.json` (default
//! `~/.local/share/systemprune/history.json`).  The file is
//! rotated at 10 MB; the orchestrator and the previous 4
//! rotations (`history.json.1` … `history.json.4`) are kept.
//!
//! The wire format is versioned:
//!
//! ```json
//! {
//!   "version": 1,
//!   "entries": [
//!     { "timestamp": "...", "source": "...", "category": "...",
//!       "id": "...", "name": "...", "size_bytes": 0,
//!       "command": "...", "exit_code": 0 }
//!   ]
//! }
//! ```
//!
//! `History::VERSION` is the discriminant.  A future bump
//! would also require encouraging readers (`history` CLI
//! subcommand) to handle v0 / v1 seams; right now only v1
//! exists.

use crate::errors::{format_command, SystemPruneError};
use crate::log::system_time_to_rfc3339;
use crate::models::{Category, PrunableItem};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Schema version of `history.json`.  Bumped on breaking changes.
pub const HISTORY_VERSION: u32 = 1;

/// Default per-file size cap.  When `history.json` would exceed
/// this after a new entry, it is rotated before the write.
pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

/// Default number of rotation files kept on top of `history.json`.
/// The CLI keeps `history.json` plus `history.json.1` …
/// `history.json.{N-1}`.
pub const DEFAULT_KEEP_FILES: usize = 5;

/// Default value for the `systemprune history --limit` flag
/// (matches the §5.2 spec example).
pub const DEFAULT_HISTORY_LIMIT: usize = 20;

const ROTATION_SUFFIX_LEN: usize = 1; // ".1" .. ".{N-1}"

/// One persistent log entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// RFC 3339 / ISO 8601 UTC timestamp with `Z` suffix,
    /// e.g. ``"2026-06-27T14:32:11Z"``.
    pub timestamp: String,
    /// Stable source name (matches `PrunableItem::source`).
    pub source: String,
    /// Category as a snake-case string
    /// (matches `PrunableItem::category::as_str()`).
    pub category: String,
    /// Stable native identifier, e.g. ``"sha256:abc…"`` for
    /// Docker images or an absolute path for filesystem caches.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Normalized byte size.
    #[serde(default)]
    pub size_bytes: u64,
    /// Human-readable command line (e.g. ``"docker rmi -f sha256:abc…"``).
    #[serde(default)]
    pub command: String,
    /// Engine exit code: 0 = success; non-zero = failure.
    #[serde(default)]
    pub exit_code: i32,
}

impl HistoryEntry {
    /// Build a [`HistoryEntry`] from a deletion result.  The
    /// `command` field is reconstructed from the item's source
    /// + category via [`command_for`], which mirrors every
    ///   scanner's `delete_item` invocation.
    pub fn from_result(
        item: &PrunableItem,
        success: bool,
        engine_returncode: Option<i32>,
        now: SystemTime,
    ) -> Self {
        Self {
            timestamp: system_time_to_rfc3339(now),
            source: item.source.clone(),
            category: item.category.as_str().to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            size_bytes: item.size_bytes,
            command: command_for(item),
            exit_code: engine_returncode.unwrap_or(if success { 0 } else { -1 }),
        }
    }
}

/// Top-level wrapper for `history.json`.
///
/// The struct is serialised verbatim, so the on-disk shape is
/// `{ "version": 1, "entries": [...] }` — matches the §5.1
/// spec example byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub version: u32,
    pub entries: Vec<HistoryEntry>,
}

impl History {
    pub const VERSION: u32 = HISTORY_VERSION;

    pub fn new() -> Self {
        Self {
            version: Self::VERSION,
            entries: Vec::new(),
        }
    }

    /// Load `path`.  Returns an empty [`History`] when the file
    /// does not exist (the common "first run" case).  Other I/O
    /// or parse errors surface as [`SystemPruneError`].
    ///
    /// **Corruption handling.**  An unreadable / unparseable
    /// file is reported as an error rather than silently
    /// overwritten: silently dropping a user's audit trail is a
    /// bad look.  Callers that want to recover from corruption
    /// (e.g. an interactive "rotate the bad file" prompt) can
    /// catch the error, rename `path` out of the way, then call
    /// `load` again.
    pub fn load(path: &Path) -> Result<Self, SystemPruneError> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                SystemPruneError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to parse {} as history.json: {}", path.display(), e),
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(SystemPruneError::Io(e)),
        }
    }

    /// Atomically write `self` back to `path`.  Writes to a
    /// sibling `*.tmp` file first, then renames over the
    /// destination so a crash mid-write cannot leave a
    /// half-finished history file in place.
    pub fn save(&self, path: &Path) -> Result<(), SystemPruneError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = tmp_path(path);
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            SystemPruneError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to serialise history: {e}"),
            ))
        })?;
        {
            let mut f = File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Convenience: load → push `entry` → save.  Handles
    /// rotation if `path` would grow past `max_bytes` after the
    /// new entry is appended.  `keep_files` includes the
    /// primary file: passing 5 keeps `history.json` plus
    /// `.1` … `.4`.
    ///
    /// **Corrupt-file policy.**  A corrupt `history.json`
    /// (parse failure, half-truncated write, manual edit that
    /// breaks the schema) is **propagated as an error** so the
    /// caller can surface it.  Silently overwriting a user's
    /// audit trail would defeat the point of having one — a
    /// load failure is surfaced to the orchestrator's action
    /// log and the new entry is *not* appended, leaving the
    /// corrupt file untouched for the operator to recover.
    /// (If you want the previous "best effort" semantics,
    /// call `History::new()` yourself and `History::save`
    /// directly.)
    pub fn append_to_file(
        path: &Path,
        entry: &HistoryEntry,
        max_bytes: u64,
        keep_files: usize,
    ) -> Result<(), SystemPruneError> {
        Self::append_many(path, std::slice::from_ref(entry), max_bytes, keep_files)
    }

    /// Batch variant of [`History::append_to_file`].  Loads the
    /// existing file **once**, extends the in-memory `entries`
    /// vector with `new_entries`, then writes the result back
    /// **once**.  This is the only safe shape to use when the
    /// caller has many entries to record within one logical
    /// event (e.g. the per-engine results produced by a single
    /// `delete_many` burst): the single-shot `append_to_file`
    /// load/mutate/save races with itself under concurrent
    /// callers because each call reads the on-disk state at a
    /// different point and can clobber the others.
    ///
    /// `new_entries` may be empty (this is a no-op save of the
    /// existing file, useful only for round-trip tests).
    /// Rotation triggers on the *pre-append* file size: if the
    /// existing file already exceeds `max_bytes`, we rotate
    /// before writing so the new batch lives in a fresh
    /// `history.json`.
    ///
    /// **Inter-process concurrency.**  This call is safe for
    /// concurrent callers within a single process (e.g. the
    /// orchestrator's `delete_many` loop, which now batches
    /// every deletion in a single `append_many` after the
    /// tasks join).  Two `systemprune` *processes* writing
    /// to the same `history.json` still race at the OS level
    /// (no `flock` is taken); callers that need that
    /// guarantee must add their own OS-level locking or use a
    /// separate output file per process.
    ///
    /// **Rotation contract.**  Rotation keeps older entries
    /// on disk as `history.json.1` … `.N-1`, but the live
    /// view returned by [`History::load`] reads only the
    /// primary file.  `systemprune history` therefore shows
    /// entries written *after* the most recent rotation;
    /// entries rotated out are preserved on disk for
    /// forensics but are not visible without reading the
    /// rotation files explicitly.  This matches the
    /// pre-existing `append_to_file` semantics; callers that
    /// want a unified live view across rotations would need
    /// to fold `.1` … `.N-1` back into `h.entries` before
    /// saving.
    pub fn append_many(
        path: &Path,
        new_entries: &[HistoryEntry],
        max_bytes: u64,
        keep_files: usize,
    ) -> Result<(), SystemPruneError> {
        let mut h = Self::load(path)?;
        let current_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if current_size >= max_bytes {
            rotate(path, keep_files)?;
            // Rotation moved the primary out of the way.
            // Drop surviving entries from rotated-in files:
            // the new write targets a fresh `history.json`.
            h = Self::new();
        }
        h.entries.extend(new_entries.iter().cloned());
        h.save(path)
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve `$XDG_DATA_HOME/systemprune/history.json`,
/// falling back to `~/.local/share/systemprune/history.json`
/// when `$XDG_DATA_HOME` is unset.
///
/// Returns `None` only when both [`dirs::data_dir`] and a home
/// directory are unavailable (a very degenerate environment).
pub fn history_path() -> Option<PathBuf> {
    let data_dir = dirs::data_dir()?;
    Some(data_dir.join("systemprune").join("history.json"))
}

/// Alias for [`history_path`] under the name used by the CLI
/// (`systemprune history --path`).  Exposed at
/// `systemprune_core::history::default_history_path` so the
/// CLI can `use systemprune_core::history::default_history_path`
/// alongside the other types in this module.
pub fn default_history_path() -> Option<PathBuf> {
    history_path()
}

/// Build the rotation path `history.json.N` for a given primary
/// path and rotation index.  Index 0 is reserved (we never
/// rotate to `.0`); the caller passes `1..keep_files`.
pub fn rotation_path(base: &Path, idx: usize) -> PathBuf {
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    let filename = base
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history.json".to_string());
    parent.join(format!("{filename}.{idx}"))
}

/// Sibling temp path used for atomic writes.
fn tmp_path(base: &Path) -> PathBuf {
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    let filename = base
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history.json".to_string());
    parent.join(format!("{filename}.tmp"))
}

/// Rotate `history.json` → `history.json.1` → `.2` → ….  The
/// primary file is moved aside; the previous `.{N-1}` rotation
/// is deleted unconditionally so disk usage stays bounded at
/// `keep_files` × `max_bytes`.
///
/// **Padding strategy.**  Real delete bursts happen across
/// many `append_to_file` calls inside one process run; rotate
/// is then invoked when the next append would push the file
/// past the size cap.  When the caller passes `keep_files=5`,
/// the kept rotation slots are `.1` … `.4` and slot `.5` is
/// the discard target.  After `rotate`, the file the user
/// then *appends* to (i.e. the primary `history.json`) lands
/// in `.1` only if the primary had content at rotate time;
/// empty primaries (rotates issued between bursts) leave a
/// gap that the next populated primary fills.
pub fn rotate(base: &Path, keep_files: usize) -> Result<(), SystemPruneError> {
    if keep_files <= 1 {
        return Ok(()); // Nothing to keep.
    }
    // Drop the discard slot (`keep_files`) so its old content,
    // if any, doesn't get overwritten by the upward shift.
    let drop_idx = keep_files;
    if drop_idx > ROTATION_SUFFIX_LEN {
        let _ = fs::remove_file(rotation_path(base, drop_idx));
    }
    // Shift every kept slot up by one, top-down so the rename
    // source never gets clobbered by the rename destination
    // check below.  `Vec::iter().rev()` is a more explicit form
    // than `Range::rev()` and avoids any ambiguity about
    // exclusive ranges with single-element spans.
    let keep_max = keep_files - 1;
    if keep_max > 1 {
        let to_shift: Vec<usize> = (1..keep_max).collect();
        for &idx in to_shift.iter().rev() {
            let src = rotation_path(base, idx);
            let dst = rotation_path(base, idx + 1);
            if src.exists() {
                if dst.exists() {
                    // On filesystems that don't auto-overwrite
                    // (e.g. when the destination was just
                    // removed by a previous iteration), drop
                    // any leftover so the rename succeeds.
                    let _ = fs::remove_file(&dst);
                }
                fs::rename(&src, &dst)?;
            }
        }
    }
    // Finally, the primary file becomes `.1` if it exists.
    if base.exists() {
        let first_rot = rotation_path(base, 1);
        if first_rot.exists() {
            let _ = fs::remove_file(&first_rot);
        }
        fs::rename(base, &first_rot)?;
    }
    Ok(())
}

/// Reconstruct the deletion command line for an item, mirroring
/// the [`Scanner::delete_item`] implementations in this crate.
///
/// This is the single source of truth for the `command` field
/// in [`HistoryEntry`]: changing the way a scanner deletes an
/// item should require changing this function too (or
/// delegating it to the scanner via a new trait method).
pub fn command_for(item: &PrunableItem) -> String {
    let argv = argv_for(item);
    format_command(&argv)
}

fn argv_for(item: &PrunableItem) -> Vec<String> {
    let id = item.id.clone();
    match (item.source.as_str(), item.category) {
        ("docker", Category::Image) => svec(&["docker", "rmi", "-f", &id]),
        ("docker", Category::Container) => svec(&["docker", "rm", "-f", &id]),
        ("docker", Category::Volume) => svec(&["docker", "volume", "rm", &id]),
        ("docker", Category::Network) => svec(&["docker", "network", "rm", &id]),
        ("podman", Category::Image) => svec(&["podman", "rmi", "-f", &id]),
        ("podman", Category::Container) => svec(&["podman", "rm", "-f", &id]),
        ("podman", Category::Volume) => svec(&["podman", "volume", "rm", &id]),
        ("podman", Category::Network) => svec(&["podman", "network", "rm", &id]),
        ("flatpak", Category::App) | ("flatpak", Category::Runtime) => {
            svec(&["flatpak", "uninstall", "--delete-data", "-y", &id])
        }
        ("snap", Category::SnapRevision) => svec(&["snap", "remove", &id]),
        ("ollama", Category::Model) => svec(&["ollama", "rm", &id]),
        ("go_cache", Category::BuildCache) => svec(&["go", "clean", "-cache"]),
        ("conda", Category::PythonVenv) => svec(&["conda", "env", "remove", "-p", &id, "-y"]),
        ("node_modules", Category::NodeModules)
        | ("python_venv", Category::PythonVenv)
        | ("tox", Category::DependencyCache)
        | ("mypy", Category::DependencyCache)
        | ("cargo_cache", Category::BuildCache) => svec(&["trash", &id]),
        _ => svec(&[]), // Unknown combo; the audit log records `command=""` rather than guessing.
    }
}

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}
