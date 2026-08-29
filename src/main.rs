mod agents;
mod cli;
mod config;
mod core;
mod env;
mod git;
mod providers;
mod repository;
mod sandbox;
mod tools;

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
    // Provider credentials live in .env (gitignored). CWD first, then repo.
    env::load_dotenv(std::path::Path::new(".env"));
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
    env::load_dotenv(&repo_root.join(".env"));

    match command {
        Command::Init => {
            let path = Config::scaffold(&repo_root)?;
            println!("✓ ForgeMan config scaffolded at {}", path.display());
            println!("  Review execution/budget limits, then run: forgeman run \"<task>\"");
        }
        Command::Inspect => cmd_inspect(&repo_root)?,
        Command::Analyze { task } => cmd_analyze(config_path.as_deref(), &repo_root, &task).await?,
        Command::Plan { task } => cmd_plan(config_path.as_deref(), &repo_root, &task).await?,
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
            .await?
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
            .await?
        }
        Command::Test => cmd_test(&repo_root)?,
        // The improvement engine runs inside the engineering loop; the CLI
        // command re-enters the same loop on the repository.
        Command::Improve { task } => {
            cmd_run(config_path.as_deref(), &repo_root, task, None, None).await?
        }
        Command::Report { run_id } => cmd_report(&repo_root, run_id)?,
        Command::Diff { run_id, full } => cmd_diff(&repo_root, run_id, full)?,
        Command::History => cmd_history(&repo_root)?,
    }
    Ok(())
}

async fn cmd_run(
    config_path: Option<&Path>,
    repo_root: &Path,
    task_description: String,
    max_iterations: Option<u32>,
    timeout_minutes: Option<u64>,
) -> Result<()> {
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
    let registry = build_registry(&config)?;
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

fn build_registry(config: &Config) -> Result<StageRegistry> {
    let provider: Arc<dyn providers::AgentProvider> = Arc::from(providers::build(&config.agent)?);
    let mut registry = StageRegistry::new();
    // Phase 2: repository explorer.
    registry.register(Arc::new(agents::inspect::InspectStage));
    // Phase 4: analyzer + planner (LLM-backed).
    registry.register(Arc::new(agents::analyze::AnalyzeStage {
        provider: provider.clone(),
    }));
    registry.register(Arc::new(agents::plan::PlanStage {
        provider: provider.clone(),
    }));
    // Phase 5: coder — applies edits through the audited tool layer.
    registry.register(Arc::new(agents::coder::CoderStage {
        provider: provider.clone(),
    }));
    // Phase 6: test runner — ecosystem detection + result parsing.
    registry.register(Arc::new(agents::test_runner::TestStage));
    // Phase 7: failure analyzer — independent root-cause diagnosis.
    registry.register(Arc::new(agents::diagnose::DiagnoseStage {
        provider: provider.clone(),
    }));
    // Phase 8: iterative improvement + verification gate.
    registry.register(Arc::new(agents::improve::ImproveStage {
        provider: provider.clone(),
    }));
    registry.register(Arc::new(agents::verify::VerifyStage));
    Ok(registry)
}

async fn cmd_analyze(config_path: Option<&Path>, repo_root: &Path, task: &str) -> Result<()> {
    let config = Config::load(config_path, repo_root)?;
    let provider = providers::build(&config.agent)?;
    let profile = repository::inspector::inspect(repo_root)?;

    println!(
        "Analyzing with {} ({}) …",
        config.agent.provider, config.agent.model
    );
    let (analysis, response) =
        agents::analyze::analyze_task(provider.as_ref(), &profile.summary(), task)
            .await
            .map_err(|err| anyhow::anyhow!("analysis failed: {err}"))?;

    println!();
    println!("TASK ANALYSIS");
    println!("  Goal       {}", analysis.goal);
    print_list("  Components ", &analysis.affected_components);
    print_list("  Constraints", &analysis.constraints);
    print_list("  Risks      ", &analysis.risks);
    print_list("  Edge cases ", &analysis.edge_cases);
    print_list("  Ambiguities", &analysis.ambiguities);
    println!(
        "  [{} tok in / {} tok out, ${:.4}]",
        response.input_tokens, response.output_tokens, response.cost_usd
    );

    persist_json(
        repo_root,
        "task-analysis.json",
        &serde_json::to_value(&analysis)?,
    )?;
    Ok(())
}

async fn cmd_plan(config_path: Option<&Path>, repo_root: &Path, task: &str) -> Result<()> {
    let config = Config::load(config_path, repo_root)?;
    let provider = providers::build(&config.agent)?;
    let profile = repository::inspector::inspect(repo_root)?;

    println!(
        "Planning with {} ({}) …",
        config.agent.provider, config.agent.model
    );
    let (analysis, _) = agents::analyze::analyze_task(provider.as_ref(), &profile.summary(), task)
        .await
        .map_err(|err| anyhow::anyhow!("analysis failed: {err}"))?;
    let (plan, response) =
        agents::plan::build_plan(provider.as_ref(), &profile.summary(), &analysis)
            .await
            .map_err(|err| anyhow::anyhow!("planning failed: {err}"))?;

    println!();
    println!("IMPLEMENTATION PLAN");
    println!("  Strategy   {}", plan.summary);
    for (index, step) in plan.steps.iter().enumerate() {
        println!("  [{}] {}", index + 1, step.description);
        if !step.affected_files.is_empty() {
            println!("       files: {}", step.affected_files.join(", "));
        }
    }
    println!("  Validation:");
    for criterion in &plan.validation_criteria {
        println!("    - {criterion}");
    }
    if !plan.rollback.is_empty() {
        println!("  Rollback   {}", plan.rollback);
    }
    println!(
        "  [{} tok in / {} tok out, ${:.4}]",
        response.input_tokens, response.output_tokens, response.cost_usd
    );

    persist_json(repo_root, "plan.json", &serde_json::to_value(&plan)?)?;
    Ok(())
}

fn print_list(label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    println!("{label} {}", items.join("; "));
}

fn persist_json(repo_root: &Path, filename: &str, value: &serde_json::Value) -> Result<()> {
    let target = repo_root.join(config::FORGEMAN_DIR).join(filename);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, serde_json::to_string_pretty(value)?)?;
    println!();
    println!("  Saved to {}", target.display());
    Ok(())
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
    let id = resolve_run_id(&store, run_id)?;
    let run = store.load_run(&id)?;

    let report = build_report(&run, &store);
    println!("{report}");

    // Persist the human-readable report as evidence (spec §30).
    let target = repo_root
        .join(config::FORGEMAN_DIR)
        .join("reports")
        .join(format!("{id}.md"));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, &report)?;
    println!("Report saved to {}", target.display());
    Ok(())
}

fn resolve_run_id(store: &RunStore, run_id: Option<String>) -> Result<String> {
    match run_id {
        Some(id) => Ok(id),
        None => store
            .latest_run_id()?
            .context("no runs found — execute a task first: forgeman run \"<task>\""),
    }
}

/// Assemble the human-readable engineering report (spec §30) from run
/// evidence: baseline vs final tests, checkpoints, failures, tool usage.
fn build_report(run: &Run, store: &RunStore) -> String {
    let mut out = String::new();
    out.push_str("FORGEMAN ENGINEERING REPORT\n\n");
    out.push_str(&format!("Run          {}\n", run.id));
    out.push_str(&format!("Task         {}\n", run.task.description));
    out.push_str(&format!("Repository   {}\n", run.task.repo_root.display()));
    out.push_str(&format!(
        "Started      {}\n",
        run.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!("Duration     {}s\n", run.duration_secs()));
    out.push_str(&format!("Status       {}\n\n", run.status));

    out.push_str(&format!("Iterations   {}\n", run.iterations.len()));
    if let Some(base) = &run.baseline_commit {
        out.push_str(&format!("Baseline     commit {base}\n"));
    }
    let first_tests = run.iterations.iter().find_map(|i| i.tests.as_ref());
    let last_tests = run.iterations.iter().rev().find_map(|i| i.tests.as_ref());
    if let (Some(first), Some(last)) = (first_tests, last_tests) {
        out.push_str(&format!(
            "Tests        {} → {} / {}\n",
            first.passed, last.passed, last.total
        ));
        if last.total > 0 {
            let gain = (last.passed as f32 - first.passed as f32) / last.total as f32 * 100.0;
            out.push_str(&format!("Improvement  {gain:+.1}% of total suite\n"));
        }
    }
    out.push_str(&format!(
        "Tools        {} invocation(s)\n",
        run.tool_executions.len()
    ));
    out.push_str(&format!("Cost         ${:.4}\n\n", run.total_cost_usd));

    for iteration in &run.iterations {
        let tests = match &iteration.tests {
            Some(t) => format!("{}/{} passed", t.passed, t.total),
            None => "no test data".to_string(),
        };
        out.push_str(&format!("  #{}  tests: {tests}", iteration.index));
        if let Some(commit) = &iteration.git_commit {
            out.push_str(&format!("  commit {commit}"));
        }
        out.push('\n');
        for failure in &iteration.failures {
            out.push_str(&format!(
                "      ✗ [{}] {}\n",
                failure.stage, failure.message
            ));
            if let Some(cause) = &failure.root_cause {
                out.push_str(&format!("        root cause: {cause}\n"));
            }
            if let Some(action) = &failure.recommended_action {
                out.push_str(&format!("        next action: {action}\n"));
            }
        }
    }

    out.push_str(&format!(
        "\nEvidence     {}\n",
        store.events_path(&run.id).display()
    ));
    out
}

/// `forgeman test` — run the repository's validation suite directly.
fn cmd_test(repo_root: &Path) -> Result<()> {
    let commands = agents::test_runner::detect_commands(repo_root);
    if commands.is_empty() {
        anyhow::bail!("no test commands detected (supported ecosystems: cargo, npm, pytest)");
    }

    let timeout = std::time::Duration::from_secs(20 * 60);
    let mut failed = false;
    for command in &commands {
        println!("▶ {} …", command.name);
        let prepared = sandbox::prepare(
            repo_root,
            command,
            &Config::load(None, repo_root)?.sandbox,
            sandbox::detect_docker(),
        )
        .map_err(|err| anyhow::anyhow!("{err}"))?;
        let (ok, output, _code, duration_ms) =
            agents::test_runner::run_prepared(prepared, &command.name, timeout)
                .map_err(|err| anyhow::anyhow!("{err}"))?;
        let summary = agents::test_runner::parse_summary(&command.name, &output, duration_ms);
        let mark = if ok { "✓" } else { "✗" };
        println!(
            "  {mark} {}: {}/{} passed ({}ms)",
            command.name, summary.passed, summary.total, summary.duration_ms
        );
        if !ok {
            failed = true;
            // Show the tail of the failing output for immediate feedback.
            let tail: Vec<&str> = output.lines().rev().take(15).collect();
            for line in tail.iter().rev() {
                println!("    | {line}");
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// `forgeman diff` — what did a run change? (spec §22 minimum MVP)
fn cmd_diff(repo_root: &Path, run_id: Option<String>, full: bool) -> Result<()> {
    let store = RunStore::new(repo_root);
    let id = resolve_run_id(&store, run_id)?;
    let run = store.load_run(&id)?;

    let Some(base) = run.baseline_commit.as_deref() else {
        anyhow::bail!("run {id} has no baseline commit (repository not under git?)");
    };
    let head = git::current_commit(repo_root)
        .map_err(anyhow::Error::msg)?
        .unwrap_or_else(|| base.to_string());
    if base == head {
        println!("No changes between baseline {base} and HEAD {head}.");
        return Ok(());
    }

    if full {
        println!(
            "{}",
            git::diff_between(repo_root, base, &head).map_err(anyhow::Error::msg)?
        );
    } else {
        let commits = git::commits_between(repo_root, base, &head).map_err(anyhow::Error::msg)?;
        println!("Checkpoints ({base} → {head}):");
        for commit in &commits {
            println!("  {commit}");
        }
    }
    Ok(())
}

/// `forgeman history` — every run and its checkpoints.
fn cmd_history(repo_root: &Path) -> Result<()> {
    let store = RunStore::new(repo_root);
    let ids = store.list_run_ids()?;
    if ids.is_empty() {
        println!("No runs yet — start with: forgeman run \"<task>\"");
        return Ok(());
    }
    for id in ids.iter().rev() {
        let run = store.load_run(id)?;
        let commits: Vec<&str> = run
            .iterations
            .iter()
            .filter_map(|i| i.git_commit.as_deref())
            .collect();
        println!(
            "{}  {}  status: {}",
            run.id,
            run.started_at.format("%Y-%m-%d %H:%M"),
            run.status
        );
        println!(
            "  task: {} | iterations: {}{}",
            truncate(&run.task.description, 60),
            run.iterations.len(),
            if commits.is_empty() {
                String::new()
            } else {
                format!(" | commits: {}", commits.join(", "))
            }
        );
    }
    Ok(())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{cut}…")
    }
}
