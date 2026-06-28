//! Command-line interface for SystemPrune.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use systemprune_core::df::Df;
use systemprune_core::history::{
    default_history_path, History, HistoryEntry, DEFAULT_HISTORY_LIMIT,
};
use systemprune_core::log::ActionLog;
use systemprune_core::models::PrunableItem;
use systemprune_core::orchestrator::{Dashboard, DashboardRow, Orchestrator};
use systemprune_core::scanners::all_scanners;
use systemprune_core::size::format_size;

#[derive(Debug, Parser)]
#[command(
    name = "systemprune",
    about = "Unified disk space cleaner for Docker, Podman, Flatpak, Snap, and Ollama.",
    version
)]
struct Cli {
    /// Emit machine-readable JSON (with the `list` subcommand).
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List prunable items.
    List {
        /// Restrict to a single engine (docker, podman, flatpak, snap, ollama).
        #[arg(long)]
        engine: Option<String>,

        /// Also include active (non-deletable) items.
        #[arg(long)]
        active: bool,
    },
    /// Delete one or more items identified as ``source:id``.
    Delete {
        /// One or more item IDs in ``source:id`` form.
        ids: Vec<String>,
        /// Skip the safety prompt for non-active items.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List detected engines and their binary paths.
    Engines,
    /// Show the disk-usage dashboard: a per-engine breakdown of
    /// item counts, total bytes, and the single largest item
    /// (more.md §4.1).
    Dashboard {
        /// Restrict to a single engine (docker, ollama, ...).
        #[arg(long)]
        engine: Option<String>,

        /// Emit a JSON array of per-engine dashboard rows
        /// instead of the human-readable table.
        #[arg(long)]
        json: bool,
    },
    /// Show the disk-usage breakdown per filesystem: a `df -h`
    /// header row for the chosen mount plus per-engine
    /// indented rows summing to the filesystem `used` (with an
    /// `unaccounted` line for the gap). See `more.md` §4.2.
    Df {
        /// Filesystem mount point to query. Defaults to `/`
        /// so the bare subcommand shows the root partition.
        #[arg(long, default_value = "/")]
        mount: PathBuf,

        /// Emit a JSON object instead of the human-readable
        /// table (keys: `filesystem`, `breakdown`, `unaccounted`).
        #[arg(long)]
        json: bool,
    },
    /// Show the persistent deletion audit log
    /// (`$XDG_DATA_HOME/systemprune/history.json`).
    History {
        /// Show at most N of the most recent entries.
        /// Defaults to 20 per the §5.2 spec.
        #[arg(long, short = 'n', default_value_t = DEFAULT_HISTORY_LIMIT)]
        limit: usize,

        /// Print the resolved history file path and exit.
        #[arg(long)]
        path: bool,

        /// Override the path to the history file.  Useful for
        /// debugging and for testing with a tmpfs directory.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Print the SystemPrune version.
    Version,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let log = ActionLog::default();
    let history_path = default_history_path();

    let orchestrator = {
        let mut o = Orchestrator::new(all_scanners()).with_log(log.clone());
        if let Some(p) = &history_path {
            o = o.with_history(p.clone());
        }
        o
    };

    let rc = match cli.command {
        Command::Version => {
            println!("systemprune {}", systemprune_core::VERSION);
            ExitCode::SUCCESS
        }
        Command::Engines => {
            for s in orchestrator.active_scanners() {
                println!("{:<10} {}", s.source(), s.binary());
            }
            ExitCode::SUCCESS
        }
        Command::List { engine, active } => {
            let result = orchestrator.scan_all().await;
            let mut items = result.items;
            if let Some(eng) = &engine {
                items.retain(|i| i.source == *eng);
            }
            if !active {
                items.retain(|i| i.is_safe_to_delete());
            }
            if cli.json {
                let payload: Vec<_> = items.iter().map(|i| i.as_dict()).collect();
                match serde_json::to_string_pretty(&payload) {
                    Ok(s) => {
                        println!("{}", s);
                        return ExitCode::SUCCESS;
                    }
                    Err(e) => {
                        eprintln!("json error: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            }
            if items.is_empty() {
                println!("(no prunable items found)");
                return ExitCode::SUCCESS;
            }
            let max_name = items.iter().map(|i| i.name.len()).max().unwrap_or(4).clamp(16, 60);
            println!(
                "{:<10} {:<14} {:<10} {:>10}  NAME",
                "SOURCE", "CATEGORY", "STATUS", "SIZE"
            );
            println!("{}", "-".repeat(64));
            for item in &items {
                let name: String = if item.name.len() > max_name {
                    item.name.chars().take(max_name).collect()
                } else {
                    item.name.clone()
                };
                println!(
                    "{:<10} {:<14} {:<10} {:>10}  {}",
                    item.source,
                    item.category.as_str(),
                    item.status.as_str(),
                    format_size(item.size_bytes as i64, true),
                    name,
                );
            }
            if !result.errors.is_empty() {
                eprintln!();
                eprintln!("Errors:");
                for err in &result.errors {
                    eprintln!("  - {}", err);
                }
            }
            ExitCode::SUCCESS
        }
        Command::Delete { ids, yes } => {
            let result = orchestrator.scan_all().await;
            let by_lookup: std::collections::HashMap<String, PrunableItem> = result
                .items
                .iter()
                .map(|i| (format!("{}:{}", i.source, i.id), i.clone()))
                .collect();

            let mut targets: Vec<PrunableItem> = Vec::new();
            let mut missing: Vec<String> = Vec::new();
            for key in &ids {
                match by_lookup.get(key) {
                    Some(i) => targets.push(i.clone()),
                    None => missing.push(key.clone()),
                }
            }
            if !missing.is_empty() {
                eprintln!("Unknown IDs (no matching item found):");
                for m in &missing {
                    eprintln!("  - {}", m);
                }
            }
            if targets.is_empty() {
                return ExitCode::from(1);
            }
            if !yes {
                let unsafe_items: Vec<&PrunableItem> = targets
                    .iter()
                    .filter(|t| !t.is_safe_to_delete())
                    .collect();
                if !unsafe_items.is_empty() {
                    eprintln!("Refusing to delete active items without --yes:");
                    for u in unsafe_items {
                        eprintln!("  - {}:{} ({})", u.source, u.id, u.status.as_str());
                    }
                    return ExitCode::from(2);
                }
                println!("The following items will be deleted:");
                for t in &targets {
                    println!(
                        "  - {}:{}  {}  ({})",
                        t.source,
                        t.id,
                        t.name,
                        format_size(t.size_bytes as i64, true)
                    );
                }
                let mut reply = String::new();
                if std::io::stdin().read_line(&mut reply).is_err() {
                    eprintln!("Aborted.");
                    return ExitCode::from(1);
                }
                if !matches!(reply.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return ExitCode::from(1);
                }
            }
            // The CLI is a one-shot tool with no concept of
            // persistent failure tracking, so we pass `None` for
            // `delete_errors` and rely on the per-item
            // `is_safe_to_delete()` check.  When
            // `default_history_path()` returns `Some(p)` the
            // orchestrator also writes each engine-confirmed
            // result to `$XDG_DATA_HOME/systemprune/history.json`
            // with 10 MB rotation.
            let results = orchestrator.delete_many(&targets, true, None).await;
            let mut rc = 0;
            for r in &results {
                if r.success {
                    println!("  ok {}:{}", r.item.source, r.item.id);
                } else {
                    eprintln!("  FAILED {}:{} - {}", r.item.source, r.item.id, error_to_string(&r.error));
                    rc = 1;
                }
            }
            ExitCode::from(rc)
        }
        Command::History { limit, path, file } => run_history(limit, path, file, cli.json),
        Command::Dashboard { engine, json } => run_dashboard(&orchestrator, engine, json).await,
        Command::Df { mount, json } => run_df(&orchestrator, mount, json).await,
    };

    // After the command finishes, print the action log to
    // stderr so the user can see what the app did (scan
    // start, per-scanner results, delete attempts, errors).
    // Only print when the log is non-empty to avoid noise
    // on trivial invocations like `systemprune version`.
    if !log.is_empty() {
        eprintln!();
        eprintln!("--- action log ---");
        eprint!("{}", log.format_lines());
        eprintln!();
    }

    rc
}

/// Read back the persistent history log and print it.
/// Resolution order for the path:
/// 1. `--file <path>` if supplied (handy for tests / debugging).
/// 2. `default_history_path()` (XDG-derived).
///
/// `--path` short-circuits and prints the resolved path so
/// users can verify which file would be used.
fn run_history(limit: usize, show_path: bool, file: Option<PathBuf>, as_json: bool) -> ExitCode {
    let path = match file {
        Some(p) => p,
        None => match default_history_path() {
            Some(p) => p,
            None => {
                eprintln!(
                    "could not resolve a data directory for the history log \
                     (both $XDG_DATA_HOME and a home directory are unset)"
                );
                return ExitCode::from(4);
            }
        },
    };
    if show_path {
        println!("{}", path.display());
        return ExitCode::SUCCESS;
    }
    let history = match History::load(&path) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("failed to read history file {}: {}", path.display(), e);
            return ExitCode::from(1);
        }
    };
    if history.entries.is_empty() {
        println!("(history is empty: no deletions recorded)");
        return ExitCode::SUCCESS;
    }
    let total = history.entries.len();
    // 0 means "no limit"; cap at total entries.  Otherwise
    // sane-clamp at total so the prefix math below stays in
    // range.  `saturating_sub` keeps the index non-negative
    // even if a future caller passes a wildly large limit.
    let visible_n = if limit == 0 || total <= limit {
        total
    } else {
        limit
    };
    // Single slice expression covers both `visible_n == total`
    // and `visible_n < total` cases.  `&history.entries[..]`
    // and `&history.entries[i..]` are both `&[HistoryEntry]`,
    // so the JSON and table paths can share the same borrow
    // without needing `Cow`.
    let slice: &[HistoryEntry] = &history.entries[total - visible_n..];

    if as_json {
        // Honour `--limit` even when streaming JSON so the
        // output stays small for piped consumers.
        match serde_json::to_string_pretty(slice) {
            Ok(s) => {
                println!("{}", s);
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("json error: {}", e);
                return ExitCode::from(1);
            }
        }
    }
    print_history_table(slice, total, visible_n);
    ExitCode::SUCCESS
}

/// Render the §4.2 per-filesystem disk-usage view.  Runs a
/// fresh `scan_all` to populate the engine breakdown and
/// then computes a [`df::Df`] for `mount_path`.  Prints the
/// two-level table or a JSON object depending on `as_json`.
///
/// Failure modes:
/// * `scan_all` per-engine failures are surfaced on stderr
///   like the dashboard subcommand.
/// * `[df::Df::compute]` can fail when `statvfs` on
///   `mount_path` returns an error (e.g. `ENOENT`,
///   `EACCES`).  In that case we print the syscall error to
///   stderr and exit with code 1.
async fn run_df(
    orchestrator: &Orchestrator,
    mount_path: PathBuf,
    as_json: bool,
) -> ExitCode {
    let result = orchestrator.scan_all().await;
    let error_count = result.errors.len();
    let computed = match Df::compute(&result, &mount_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "df: failed to stat filesystem at {}: {e}",
                mount_path.display()
            );
            return ExitCode::from(1);
        }
    };
    if as_json {
        match serde_json::to_string_pretty(&computed) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("json error: {e}");
                return ExitCode::from(1);
            }
        }
    } else {
        print!("{}", computed.format_text());
    }
    if error_count > 0 {
        eprintln!();
        eprintln!("Scan warnings ({error_count}):");
        for err in &result.errors {
            eprintln!("  - {err}");
        }
    }
    ExitCode::SUCCESS
}

/// Render the disk-usage dashboard (§4.1).  Runs a fresh
/// `scan_all`, groups items by engine via
/// [`Dashboard::compute`], filters by `--engine` if
/// supplied, and prints either a fixed-width text table or a
/// JSON array depending on `as_json`.
///
/// Failure modes:
/// * `scan_all` reports no errors at this level; per-engine
///   failures are surfaced only on stderr at the end.
/// * `--engine <name>` that matches no detected engine
///   produces an empty result set and an empty table with a
///   "no data for engine X" prelude, exit 0.
/// * `--json` errors fall through to `eprintln` and exit 1.
async fn run_dashboard(
    orchestrator: &Orchestrator,
    engine_filter: Option<String>,
    as_json: bool,
) -> ExitCode {
    let result = orchestrator.scan_all().await;
    let error_count = result.errors.len();
    let dash = Dashboard::compute(&result);
    let filtered: Vec<DashboardRow> = match &engine_filter {
        Some(name) => dash
            .rows
            .iter()
            .filter(|r| r.source == *name)
            .cloned()
            .collect(),
        None => dash.rows.iter().cloned().collect(),
    };
    if as_json {
        match serde_json::to_string_pretty(&filtered) {
            Ok(s) => println!("{}", s),
            Err(e) => {
                eprintln!("json error: {}", e);
                return ExitCode::from(1);
            }
        }
    } else {
        if let Some(name) = &engine_filter {
            println!("Dashboard for engine: {}", name);
        }
        if filtered.is_empty() {
            if let Some(name) = &engine_filter {
                println!("(no items found for engine \"{}\")", name);
            } else {
                println!("(no prunable items found)");
            }
        } else {
            // Use the structured Dashboard's text helper so
            // the all-engines and filtered paths share one
            // formatter.  Move `filtered` into the wrapper
            // rather than cloning it; this is the only
            // allocation made for the text path.
            let scoped = Dashboard { rows: filtered };
            print!("{}", scoped.format_text());
            let total: i64 = scoped.grand_total() as i64;
            println!(
                "Total across {} engine(s): {}",
                scoped.rows.len(),
                format_size(total, true)
            );
        }
    }
    if error_count > 0 {
        eprintln!();
        eprintln!("Scan warnings ({}):", error_count);
        for err in &result.errors {
            eprintln!("  - {}", err);
        }
    }
    ExitCode::SUCCESS
}

/// Print a deterministic, human-readable list of the most
/// recent N entries.  Ordering is newest-last so the visual
/// order matches the order things happened in real life.
fn print_history_table(slice: &[HistoryEntry], total: usize, visible: usize) {
    println!(
        "{:<20}  {:<10} {:<10} {:<10} {:>10}  {}",
        "TIMESTAMP", "SOURCE", "CATEGORY", "STATUS", "SIZE", "NAME"
    );
    println!("{}", "-".repeat(80));
    for e in slice {
        let status = if e.exit_code == 0 { "ok" } else { "failed" };
        println!(
            "{:<20}  {:<10} {:<10} {:<10} {:>10}  {}",
            e.timestamp,
            truncate(&e.source, 10),
            truncate(&e.category, 10),
            status,
            format_size(e.size_bytes as i64, true),
            e.name,
        );
    }
    if total > visible {
        println!(
            "(showing last {} of {} entries; pass --limit N to see more)",
            visible, total
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

fn error_to_string(e: &Option<systemprune_core::errors::EngineError>) -> String {
    match e {
        Some(err) => err.to_string(),
        None => "<no error>".to_string(),
    }
}
