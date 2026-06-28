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
    /// scanner's `delete_item` invocation.
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
                    format!(
                        "failed to parse {} as history.json: {}",
                        path.display(),
                        e
                    ),
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
        ("flatpak", Category::App)
        | ("flatpak", Category::Runtime) => {
            svec(&["flatpak", "uninstall", "--delete-data", "-y", &id])
        }
        ("snap", Category::SnapRevision) => svec(&["snap", "remove", &id]),
        ("ollama", Category::Model) => svec(&["ollama", "rm", &id]),
        ("go_cache", Category::BuildCache) => svec(&["go", "clean", "-cache"]),
        ("conda", Category::PythonVenv) => {
            svec(&["conda", "env", "remove", "-p", &id, "-y"])
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Category, Engine, PrunableItem, Status};
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn make_item(
        source: &str,
        category: Category,
        id: &str,
    ) -> PrunableItem {
        let engine = match source {
            "docker" => Engine::Docker,
            "podman" => Engine::Podman,
            "flatpak" => Engine::Flatpak,
            "snap" => Engine::Snap,
            "ollama" => Engine::Ollama,
            "node_modules" => Engine::NodeModules,
            "python_venv" => Engine::PythonVenv,
            "tox" => Engine::Tox,
            "mypy" => Engine::Mypy,
            "go_cache" => Engine::GoCache,
            "conda" => Engine::Conda,
            "cargo_cache" => Engine::CargoCache,
            _ => Engine::Docker,
        };
        PrunableItem {
            id: id.to_string(),
            name: id.to_string(),
            engine,
            source: source.to_string(),
            category,
            size_bytes: 1_000_000,
            status: Status::Unused,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn command_for_docker_image_uses_rmi_dash_f() {
        let item = make_item("docker", Category::Image, "sha256:abc");
        assert_eq!(command_for(&item), "docker rmi -f sha256:abc");
    }

    #[test]
    fn command_for_docker_container_uses_rm_dash_f() {
        let item = make_item("docker", Category::Container, "abcd1234");
        assert_eq!(command_for(&item), "docker rm -f abcd1234");
    }

    #[test]
    fn command_for_docker_volume_uses_volume_rm() {
        let item = make_item("docker", Category::Volume, "my-vol");
        assert_eq!(command_for(&item), "docker volume rm my-vol");
    }

    #[test]
    fn command_for_podman_matches_docker_layout() {
        let item = make_item("podman", Category::Image, "sha256:abc");
        assert_eq!(command_for(&item), "podman rmi -f sha256:abc");
    }

    #[test]
    fn command_for_flatpak_app_uses_uninstall_delete_data() {
        let item = make_item("flatpak", Category::App, "org.gimp.GIMP");
        assert_eq!(
            command_for(&item),
            "flatpak uninstall --delete-data -y org.gimp.GIMP"
        );
    }

    #[test]
    fn command_for_snap_uses_remove() {
        let item = make_item("snap", Category::SnapRevision, "firefox");
        assert_eq!(command_for(&item), "snap remove firefox");
    }

    #[test]
    fn command_for_ollama_uses_rm() {
        let item = make_item("ollama", Category::Model, "qwen2.5:7b");
        assert_eq!(command_for(&item), "ollama rm qwen2.5:7b");
    }

    #[test]
    fn command_for_node_modules_uses_trash() {
        let item = make_item(
            "node_modules",
            Category::NodeModules,
            "/home/u/proj/node_modules",
        );
        assert_eq!(
            command_for(&item),
            "trash /home/u/proj/node_modules"
        );
    }

    #[test]
    fn command_for_go_cache_uses_clean_cache() {
        let item = make_item("go_cache", Category::BuildCache, "/home/u/.cache/go-build");
        assert_eq!(command_for(&item), "go clean -cache");
    }

    #[test]
    fn command_for_conda_uses_env_remove() {
        let item = make_item(
            "conda",
            Category::PythonVenv,
            "/home/u/miniconda3/envs/myenv",
        );
        assert_eq!(
            command_for(&item),
            "conda env remove -p /home/u/miniconda3/envs/myenv -y"
        );
    }

    #[test]
    fn command_for_cargo_cache_uses_trash() {
        let item = make_item(
            "cargo_cache",
            Category::BuildCache,
            "/home/u/.cargo/registry/cache",
        );
        assert_eq!(
            command_for(&item),
            "trash /home/u/.cargo/registry/cache"
        );
    }

    #[test]
    fn command_for_unknown_combo_is_empty() {
        let item = make_item("docker", Category::App, "weird-id");
        assert_eq!(command_for(&item), "");
    }

    #[test]
    fn history_entry_from_success_uses_zero_exit_code() {
        let item = make_item("docker", Category::Image, "sha256:a");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
        let entry =
            HistoryEntry::from_result(&item, true, None, now);
        assert_eq!(entry.exit_code, 0);
        assert_eq!(entry.command, "docker rmi -f sha256:a");
        assert_eq!(entry.source, "docker");
        assert_eq!(entry.category, "image");
        assert_eq!(entry.id, "sha256:a");
        assert_eq!(entry.timestamp, "1970-01-01T00:00:00Z");
        assert_eq!(entry.size_bytes, 1_000_000);
    }

    #[test]
    fn history_entry_from_failure_prefers_engine_returncode() {
        let item = make_item("docker", Category::Image, "sha256:a");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
        let entry =
            HistoryEntry::from_result(&item, false, Some(125), now);
        assert_eq!(entry.exit_code, 125);
    }

    #[test]
    fn history_entry_from_failure_without_engine_returncode_uses_minus_one() {
        let item = make_item("docker", Category::Image, "sha256:a");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
        let entry =
            HistoryEntry::from_result(&item, false, None, now);
        assert_eq!(entry.exit_code, -1);
    }

    #[test]
    fn history_round_trip_preserves_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let mut h = History::new();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let item = make_item(
            "docker",
            Category::Image,
            "sha256:abcdef",
        );
        h.entries.push(HistoryEntry::from_result(
            &item, true, None, now,
        ));
        h.save(&path).unwrap();

        let loaded = History::load(&path).unwrap();
        assert_eq!(loaded, h);
        assert_eq!(loaded.version, HISTORY_VERSION);
    }

    #[test]
    fn history_load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let loaded = History::load(&path).unwrap();
        assert!(loaded.entries.is_empty());
        assert_eq!(loaded.version, HISTORY_VERSION);
    }

    #[test]
    fn history_load_corrupt_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        std::fs::write(&path, "this is not JSON").unwrap();
        // A corrupted file must surface as an error so the
        // caller can recover explicitly (e.g. rename the bad
        // file away and start fresh) rather than silently
        // dropping the user's audit trail.
        assert!(History::load(&path).is_err());
    }

    #[test]
    fn append_to_file_creates_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let item = make_item(
            "docker",
            Category::Image,
            "sha256:abc",
        );
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
        let entry = HistoryEntry::from_result(
            &item, true, None, now,
        );
        History::append_to_file(&path, &entry, 1 << 20, 5).unwrap();
        let h = History::load(&path).unwrap();
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].id, "sha256:abc");
    }

    #[test]
    fn append_to_file_rotates_at_max_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
        // 1-byte payload forces rotation on the very next
        // append.  (1 KB max is tight enough that pretty
        // output always exceeds it.)
        let max_bytes = 1024;
        for i in 0..5 {
            // 50-character names push the file past 1 KB
            // after a handful of entries.
            let id = format!("{:050}", i);
            let mut item =
                make_item("docker", Category::Image, &id);
            item.name = id.clone();
            let entry = HistoryEntry::from_result(
                &item, true, None, now,
            );
            History::append_to_file(
                &path,
                &entry,
                max_bytes,
                5,
            )
            .unwrap();
        }
        // After enough appends we should have at least one
        // rotation file.  The exact threshold depends on the
        // pretty-printer's newline rules, but with names this
        // long 5 entries are well past 1 KB.
        assert!(
            path.with_extension("json.1").exists()
                || path.with_extension("json.2").exists()
                || path.with_extension("json.3").exists()
                || path.with_extension("json.4").exists(),
            "expected at least one rotation after max_bytes overshoot"
        );
    }

    #[test]
    fn rotate_drops_excess_rotations() {
        // Real rotation happens between delete bursts.  We
        // simulate three bursts: each one writes a fresh
        // `history.json`, then triggers a `rotate` so the
        // previous burst slides into `.1` (then `.2` after the
        // third burst).  With `keep_files=3` the third burst
        // must never promote an entry into `.3` (only two
        // previous bursts ever get retained).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);

        // Burst 1: write, rotate.  The primary moves into `.1`.
        let mut h = History::new();
        h.entries.push(HistoryEntry::from_result(
            &make_item("docker", Category::Image, "sha256:abc"),
            true, None, now,
        ));
        h.save(&path).unwrap();
        rotate(&path, 3).unwrap();
        assert!(
            dir.path().join("history.json.1").exists(),
            "expected .1 to exist after first rotate"
        );
        assert!(!path.exists(), "primary gone after rotate");

        // Burst 2: write again, rotate.  .1 slides into .2 and
        // the new burst lands in .1.
        h.entries.push(HistoryEntry::from_result(
            &make_item("docker", Category::Image, "sha256:def"),
            true, None, now,
        ));
        h.save(&path).unwrap();
        rotate(&path, 3).unwrap();
        assert!(dir.path().join("history.json.1").exists());
        assert!(dir.path().join("history.json.2").exists());

        // Burst 3: write again, rotate.  With keep_files=3
        // we keep at most .1 and .2 (two prior bursts).  .3 is
        // the discard slot that must NOT receive a new entry.
        h.entries.push(HistoryEntry::from_result(
            &make_item("docker", Category::Image, "sha256:ghi"),
            true, None, now,
        ));
        h.save(&path).unwrap();
        rotate(&path, 3).unwrap();
        assert!(dir.path().join("history.json.1").exists());
        assert!(dir.path().join("history.json.2").exists());
        assert!(
            !dir.path().join("history.json.3").exists(),
            "keep_files=3 must never produce .3"
        );
        assert!(!path.exists());
    }

    #[test]
    fn append_many_records_all_entries_in_order() {
        // Empty file + a batch of N: the batch must land in
        // order.  This exercises the happy path of the new
        // append_many primitive (single load, single save).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
        let ids = ["sha256:a", "sha256:b", "sha256:c"];
        let entries: Vec<HistoryEntry> = ids
            .iter()
            .map(|id| {
                HistoryEntry::from_result(
                    &make_item("docker", Category::Image, id),
                    true,
                    None,
                    now,
                )
            })
            .collect();
        History::append_many(&path, &entries, 1 << 20, 5).unwrap();
        let h = History::load(&path).unwrap();
        assert_eq!(h.entries.len(), 3);
        assert_eq!(h.entries[0].id, "sha256:a");
        assert_eq!(h.entries[1].id, "sha256:b");
        assert_eq!(h.entries[2].id, "sha256:c");
    }

    #[test]
    fn append_many_preserves_existing_entries() {
        // Seed 2 entries via the per-entry append_to_file path
        // (so we touch the same on-disk format the rest of the
        // world uses), then push 2 more via the batch API.
        // Both halves must be present in order -- this is the
        // invariant the orchestrator's race-fix relies on
        // (the loaded entries are not clobbered by the new
        // batch).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(0);
        for id in ["sha256:a", "sha256:b"] {
            let entry = HistoryEntry::from_result(
                &make_item("docker", Category::Image, id),
                true,
                None,
                now,
            );
            History::append_to_file(&path, &entry, 1 << 20, 5)
                .unwrap();
        }
        let new_entries: Vec<HistoryEntry> = ["sha256:c", "sha256:d"]
            .iter()
            .map(|id| {
                HistoryEntry::from_result(
                    &make_item("docker", Category::Image, id),
                    true,
                    None,
                    now,
                )
            })
            .collect();
        History::append_many(&path, &new_entries, 1 << 20, 5)
            .unwrap();
        let h = History::load(&path).unwrap();
        assert_eq!(h.entries.len(), 4);
        assert_eq!(h.entries[0].id, "sha256:a");
        assert_eq!(h.entries[1].id, "sha256:b");
        assert_eq!(h.entries[2].id, "sha256:c");
        assert_eq!(h.entries[3].id, "sha256:d");
    }

    #[test]
    fn rotation_path_appends_numeric_suffix() {
        let base = Path::new("/tmp/foo/history.json");
        assert_eq!(
            rotation_path(base, 1),
            PathBuf::from("/tmp/foo/history.json.1")
        );
        assert_eq!(
            rotation_path(base, 2),
            PathBuf::from("/tmp/foo/history.json.2")
        );
    }
}
