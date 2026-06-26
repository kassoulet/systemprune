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
//! * [`scanners::ALL_SCANNERS`] — the canonical list of built-in scanners.

pub mod errors;
pub mod models;
pub mod orchestrator;
pub mod probe;
pub mod scanners;
pub mod size;

pub use errors::{EngineError, ParseError, SystemPruneError};
pub use models::{Category, Engine, PrunableItem, Status};
pub use orchestrator::{DeleteResult, Orchestrator, ScanResult};
pub use probe::{probe_engines, which};
pub use scanners::Scanner;
pub use size::{format_size, parse_size};

/// Library version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
