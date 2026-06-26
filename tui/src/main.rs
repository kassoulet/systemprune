//! Ratatui-based TUI for SystemPrune.

mod app;

use anyhow::Result;
use clap::Parser;
use std::io::{stdout, Stdout};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "systemprune-tui",
    about = "Terminal UI for SystemPrune",
    version
)]
struct Cli {
    /// Optional config file (currently unused; reserved for future use).
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let _cli = Cli::parse();
    let mut terminal = setup_terminal()?;
    let result = app::run(&mut terminal);
    restore_terminal()?;
    result
}

type Terminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<Stdout>>;

fn setup_terminal() -> Result<Terminal> {
    crossterm::terminal::enable_raw_mode()?;
    let mut out = stdout();
    crossterm::execute!(
        out,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(out);
    let terminal = ratatui::Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal() -> Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    Ok(())
}
