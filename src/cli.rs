use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// ForgeMan — Autonomous Software Engineering Agent.
///
/// ForgeMan turns engineering problems into verified solutions:
/// understand → inspect → plan → implement → test → diagnose → improve → verify → report.
#[derive(Parser, Debug)]
#[command(
    name = "forgeman",
    version,
    about = "Autonomous Software Engineering Agent — AI that engineers, not just codes.",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Path to a ForgeMan config file. Defaults to .forgeman/config.toml when present.
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Target repository. Defaults to the current working directory.
    #[arg(long, global = true, value_name = "PATH")]
    pub repo: Option<PathBuf>,

    /// Enable verbose logging.
    #[arg(long, short, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scaffold ForgeMan configuration inside the target repository.
    Init,

    /// Inspect a repository and build its intelligence profile.
    Inspect,

    /// Analyze an engineering task against the repository.
    Analyze {
        /// The engineering task description.
        task: String,
    },

    /// Produce an executable engineering plan for the task.
    Plan {
        /// The engineering task description.
        task: String,
    },

    /// Execute the full engineering loop for a task.
    Run {
        /// The engineering task description.
        task: String,
        /// Override execution.max_iterations from config.
        #[arg(long)]
        max_iterations: Option<u32>,
        /// Override execution.timeout_minutes from config.
        #[arg(long)]
        timeout_minutes: Option<u64>,
    },

    /// Convenience alias for `run`: the whole loop, end to end.
    Solve {
        /// The engineering task description.
        task: String,
        /// Override execution.max_iterations from config.
        #[arg(long)]
        max_iterations: Option<u32>,
        /// Override execution.timeout_minutes from config.
        #[arg(long)]
        timeout_minutes: Option<u64>,
    },

    /// Run validation against the repository.
    Test,

    /// Start (or continue) iterative improvement for a task.
    Improve {
        /// The engineering task description.
        task: String,
    },

    /// Show the engineering report of a previous run.
    Report {
        /// Run id. Defaults to the most recent run.
        #[arg(long)]
        run_id: Option<String>,
    },

    /// Show the git diff a run produced (baseline → final checkpoint).
    Diff {
        /// Run id. Defaults to the most recent run.
        #[arg(long)]
        run_id: Option<String>,
        /// Full diff instead of the stat summary.
        #[arg(long)]
        full: bool,
    },

    /// List runs and their per-iteration git checkpoints.
    History,
}
