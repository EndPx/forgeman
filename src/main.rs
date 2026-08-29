mod cli;
mod config;
mod core;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};
use config::Config;

fn main() {
    let cli = Cli::parse();
    if let Err(err) = dispatch(cli) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn dispatch(cli: Cli) -> Result<()> {
    let repo_root = cli
        .repo
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory is readable"));

    match cli.command {
        Command::Init => {
            let path = Config::scaffold(&repo_root)?;
            println!("✓ ForgeMan config scaffolded at {}", path.display());
            println!("  Review execution/budget limits, then run: forgeman run \"<task>\"");
        }
        Command::Inspect => pending("inspect", 2, "repository inspector"),
        Command::Analyze { .. } => pending("analyze", 4, "task analyzer"),
        Command::Plan { .. } => pending("plan", 4, "planner"),
        Command::Run { .. } | Command::Solve { .. } => pending("run/solve", 1, "core orchestrator"),
        Command::Test => pending("test", 6, "test runner"),
        Command::Improve { .. } => pending("improve", 8, "iteration engine"),
        Command::Report { .. } => pending("report", 9, "reporting"),
    }
    Ok(())
}

/// Stages that are specified but not built yet must fail gracefully and
/// informatively, never silently pretend to work.
fn pending(command: &str, phase: u8, component: &str) {
    eprintln!(
        "forgeman {command}: not implemented yet.\n  \
         This capability ({component}) lands in Phase {phase} of the build roadmap."
    );
    std::process::exit(2);
}
