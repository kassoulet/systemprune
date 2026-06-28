//! # systemprune-core
//!
//! Engine-agnostic building blocks for SystemPrune.
//!
//! This crate exposes:
//!
//! * [`PrunableItem`] and the [`Engine`], [`Category`], [`Status`] enums
//!   that every scanner produces.
//! * [`Scanner`] — the trait every engine wrapper implements.
//! * [`Orchestrator`] — coordinates scanning and batched deletion.
//! * [`probe_engines`] / [`which`] — `$PATH` probing helpers.
//! * [`parse_size`] / [`format_size`] — human-readable size helpers.
//! * [`history::History`] — persistent deletion audit log with
//!   rotation, used by the `systemprune history` CLI subcommand
//!   (`more.md` §5.1 / §5.2).
//! * [`scanners::ALL_SCANNERS`] — the canonical list of built-in scanners.

pub mod df;
pub mod errors;
pub mod history;
pub mod log;
pub mod models;
pub mod orchestrator;
pub mod probe;
pub mod scanners;
pub mod size;
pub mod sort;

pub use errors::{EngineError, ParseError, SystemPruneError};
pub use history::{
    command_for as history_command_for, history_path as default_history_path, History,
    HistoryEntry, DEFAULT_HISTORY_LIMIT, DEFAULT_KEEP_FILES, DEFAULT_MAX_BYTES,
    HISTORY_VERSION,
};
pub use log::{system_time_to_rfc3339, ActionLog, LogEntry, LogLevel};
pub use models::{Category, Engine, PrunableItem, Status};
pub use orchestrator::{Dashboard, DashboardRow, DashboardTopItem, DeleteResult, Orchestrator, ScanResult};
pub use probe::{probe_engines, which};
pub use scanners::Scanner;
pub use size::{format_size, parse_size};
pub use sort::{sort_items, sorted_items, SortMode};

/// Library version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
