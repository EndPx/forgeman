mod agents;
mod cli;
mod config;
mod core;
mod repository;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use cli::{Cli, Command};
use config::Config;
use core::events::{ConsoleSink, JsonlSink, MultiSink};
use core::model::{Run, RunStatus, Task, new_task_id};
use core::orchestrator::{Orchestrator, StageRegistry};
use core::store::RunStore;
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(err) = dispatch(cli).await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

async fn dispatch(cli: Cli) -> Result<()> {
    let Cli {
        config: config_path,
        repo,
        verbose: _,
        command,
    } = cli;
    let repo_root = match repo {
        Some(path) => path,
        None => std::env::current_dir().expect("current directory is readable"),
    };

    match command {
        Command::Init => {
            let path = Config::scaffold(&repo_root)?;
            println!("✓ ForgeMan config scaffolded at {}", path.display());
            println!("  Review execution/budget limits, then run: forgeman run \"<task>\"");
        }
        Command::Inspect => cmd_inspect(&repo_root)?,
        Command::Analyze { .. } => pending("analyze", 4, "task analyzer"),
        Command::Plan { .. } => pending("plan", 4, "planner"),
        Command::Run {
            task,
            max_iterations,
            timeout_minutes,
        } => {
            cmd_run(
                config_path.as_deref(),
                &repo_root,
                task,
                max_iterations,
                timeout_minutes,
            )
            .await
        }
        Command::Solve {
            task,
            max_iterations,
            timeout_minutes,
        } => {
            cmd_run(
                config_path.as_deref(),
                &repo_root,
                task,
                max_iterations,
                timeout_minutes,
            )
            .await
        }
        Command::Test => pending("test", 6, "test runner"),
        Command::Improve { .. } => pending("improve", 8, "iteration engine"),
        Command::Report { run_id } => cmd_report(&repo_root, run_id)?,
    }
    Ok(())
}

async fn cmd_run(
    config_path: Option<&Path>,
    repo_root: &Path,
    task_description: String,
    max_iterations: Option<u32>,
    timeout_minutes: Option<u64>,
) {
    let mut config = Config::load(config_path, repo_root).expect("configuration is valid");
    if let Some(max) = max_iterations {
        config.execution.max_iterations = max;
    }
    if let Some(minutes) = timeout_minutes {
        config.execution.timeout_minutes = minutes;
    }

    let task = Task {
        id: new_task_id(),
        description: task_description.clone(),
        repo_root: repo_root.to_path_buf(),
        created_at: Utc::now(),
    };

    println!("╔══════════════════════════════════════════╗");
    println!("║               FORGEMAN                   ║");
    println!("║     Autonomous Software Engineer         ║");
    println!("╚══════════════════════════════════════════╝");
    println!();
    println!("Task:");
    println!("{task_description}");
    println!();

    let store = RunStore::new(repo_root);
    let sinks = MultiSink::new(vec![
        Box::new(ConsoleSink),
        Box::new(JsonlSink::new(store.root.clone())),
    ]);

    // Phases 2–8 register the real engineering stages here as they land.
    let registry = build_registry(&config);
    let orchestrator = Orchestrator::new(registry);

    let run = orchestrator.execute_run(task, config, &store, &sinks).await;

    print_run_summary(&run, &store);
    let code = match run.status {
        RunStatus::Verified => 0,
        RunStatus::Aborted { .. } => 2,
        _ => 1,
    };
    std::process::exit(code);
}

fn build_registry(_config: &Config) -> StageRegistry {
    let mut registry = StageRegistry::new();
    // Phase 2: repository explorer.
    registry.register(Arc::new(agents::inspect::InspectStage));
    // Phases 3–8: analyzer, planner, coder, test runner, diagnoser,
    // improver, verifier — registered as they land.
    registry
}

fn cmd_inspect(repo_root: &Path) -> Result<()> {
    let profile = repository::inspector::inspect(repo_root)
        .map_err(|err| anyhow::anyhow!("inspection failed: {err:#}"))?;

    println!("REPOSITORY PROFILE");
    println!("  Root          {}", profile.root.display());
    println!(
        "  Language      {} ({} file(s))",
        profile.primary_language, profile.file_count
    );
    for share in profile.languages.iter().take(4) {
        println!("    - {} ({})", share.language, share.files);
    }
    println!(
        "  Framework     {}",
        profile.framework.as_deref().unwrap_or("none detected")
    );
    println!(
        "  Packages      {}",
        profile
            .package_manager
            .as_deref()
            .unwrap_or("none detected")
    );
    if !profile.entrypoints.is_empty() {
        println!("  Entrypoints   {}", profile.entrypoints.join(", "));
    }
    if !profile.test_frameworks.is_empty() {
        println!("  Tests         {}", profile.test_frameworks.join(", "));
    }
    if !profile.databases.is_empty() {
        println!("  Databases     {}", profile.databases.join(", "));
    }
    if !profile.external_services.is_empty() {
        println!("  Services      {}", profile.external_services.join(", "));
    }
    if !profile.config_files.is_empty() {
        println!("  Config        {}", profile.config_files.join(", "));
    }

    let deps = profile.dependencies.len();
    if deps > 0 {
        println!("  Dependencies  {deps}");
    }

    println!("  Risk areas    {}", profile.risky_areas.len());
    for area in profile.risky_areas.iter().take(8) {
        println!("    - [{}] {}", area.category, area.path);
    }

    // Persist so later stages/commands can reuse the profile without re-walking.
    let target = repo_root.join(config::FORGEMAN_DIR).join("profile.json");
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, serde_json::to_string_pretty(&profile)?)?;
    println!();
    println!("  Profile saved to {}", target.display());
    Ok(())
}

fn print_run_summary(run: &Run, store: &RunStore) {
    println!();
    println!("──────────────────────────────────────────");
    println!();
    println!("RUN          {}", run.id);
    println!("STATUS       {}", run.status);
    println!("ITERATIONS   {}", run.iterations.len());
    let preamble: Vec<&str> = run
        .preamble_results
        .iter()
        .map(|r| r.stage.as_str())
        .collect();
    if !preamble.is_empty() {
        println!("STAGES DONE  {}", preamble.join(", "));
    }
    let failures: usize = run.iterations.iter().map(|i| i.failures.len()).sum();
    if failures > 0 {
        println!("FAILURES     {failures}");
    }
    println!("DURATION     {}s", run.duration_secs());
    println!("EVIDENCE     {}", store.events_path(&run.id).display());
    println!();
}

fn cmd_report(repo_root: &Path, run_id: Option<String>) -> Result<()> {
    let store = RunStore::new(repo_root);
    let id = match run_id {
        Some(id) => id,
        None => store
            .latest_run_id()
            .context("no runs found — execute a task first: forgeman run \"<task>\"")?
            .context("no runs found — execute a task first: forgeman run \"<task>\"")?,
    };
    let run = store.load_run(&id)?;

    println!("FORGEMAN ENGINEERING REPORT");
    println!();
    println!("Run          {}", run.id);
    println!("Task         {}", run.task.description);
    println!("Repository   {}", run.task.repo_root.display());
    println!(
        "Started      {}",
        run.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("Duration     {}s", run.duration_secs());
    println!("Status       {}", run.status);
    println!();
    println!("Iterations   {}", run.iterations.len());
    for iteration in &run.iterations {
        let tests = match &iteration.tests {
            Some(t) => format!("{}/{} passed", t.passed, t.total),
            None => "no test data".to_string(),
        };
        println!("  #{}  tests: {}", iteration.index, tests);
        for failure in &iteration.failures {
            println!("      ✗ [{}] {}", failure.stage, failure.message);
        }
    }
    println!();
    println!("Evidence     {}", store.events_path(&id).display());
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
