//! Command-line interface for SystemPrune.

use clap::{Parser, Subcommand};
use std::process::ExitCode;
use systemprune_core::models::PrunableItem;
use systemprune_core::orchestrator::Orchestrator;
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
    /// Print the SystemPrune version.
    Version,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let orchestrator = Orchestrator::new(all_scanners());

    match cli.command {
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
                "{:<10} {:<14} {:<10} {:>10}  {}",
                "SOURCE", "CATEGORY", "STATUS", "SIZE", "NAME"
            );
            println!("{}", "-".repeat(64));
            for item in &items {
                let name = if item.name.len() > max_name {
                    &item.name[..max_name]
                } else {
                    &item.name
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
            let results = orchestrator.delete_many(&targets, true).await;
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
    }
}

fn error_to_string(e: &Option<systemprune_core::errors::EngineError>) -> String {
    match e {
        Some(err) => err.to_string(),
        None => "<no error>".to_string(),
    }
}
