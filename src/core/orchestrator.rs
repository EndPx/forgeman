// Registry/execution surface consumed by stage implementations landing in Phases 2–8.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;

use crate::config::Config;
use crate::core::events::{Event, EventKind, EventSink};
use crate::core::model::{
    FailureRecord, Iteration, Run, RunStatus, StageName, StageResult, StageStatus, Task,
    TestSummary,
};
use crate::core::stage::{RunContext, Stage};
use crate::core::store::RunStore;

/// Registry of stage implementations. Phases 2–8 register real stages here;
/// the orchestrator itself stays generic over how stages do their work.
pub struct StageRegistry {
    stages: HashMap<StageName, Arc<dyn Stage>>,
}

impl Default for StageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl StageRegistry {
    pub fn new() -> Self {
        Self {
            stages: HashMap::new(),
        }
    }

    pub fn register(&mut self, stage: Arc<dyn Stage>) {
        let name = stage.name();
        self.stages.insert(name, stage);
    }

    pub fn get(&self, name: StageName) -> Option<Arc<dyn Stage>> {
        self.stages.get(&name).cloned()
    }

    pub fn contains(&self, name: StageName) -> bool {
        self.stages.contains_key(&name)
    }
}

const PREAMBLE: [StageName; 3] = [StageName::Inspect, StageName::Analyze, StageName::Plan];
/// Required for the loop: initial implementation, validation, diagnosis and
/// improvement (improve runs in iterations 1+).
const LOOP_REQUIRED: [StageName; 4] = [
    StageName::Implement,
    StageName::Test,
    StageName::Diagnose,
    StageName::Improve,
];
/// Artifacts scoped to a single iteration; cleared when a new one starts.
const ITERATION_ARTIFACTS: [&str; 5] = [
    "tests.result",
    "tests.output",
    "failure.analysis",
    "implementation.changes",
    "improvement.changes",
];

pub struct Orchestrator {
    registry: StageRegistry,
}

impl Orchestrator {
    pub fn new(registry: StageRegistry) -> Self {
        Self { registry }
    }

    /// Execute the full ForgeMan loop:
    /// preamble (inspect → analyze → plan) → iteration loop (implement →
    /// test → diagnose → improve) → report, with stop conditions and
    /// escalation. Always returns a finished, persisted run.
    pub async fn execute_run(
        &self,
        task: Task,
        config: Config,
        store: &RunStore,
        sinks: &dyn EventSink,
    ) -> Run {
        let run = Run::starting(task.clone(), config.clone());
        let mut ctx = RunContext::new(config, task, run);

        sinks.record(&Event::now(
            &ctx.run.id,
            None,
            EventKind::TaskCreated {
                description: ctx.task.description.clone(),
            },
        ));
        sinks.record(&Event::now(&ctx.run.id, None, EventKind::RunStarted));

        // Capture the baseline for diffs and improvement stats (spec §22).
        if crate::git::is_repo(&ctx.task.repo_root)
            && let Ok(Some(hash)) = crate::git::current_commit(&ctx.task.repo_root)
        {
            ctx.run.baseline_commit = Some(hash);
        }

        // Fail fast when required stages are not registered yet.
        let missing: Vec<StageName> = PREAMBLE
            .into_iter()
            .chain(LOOP_REQUIRED)
            .filter(|name| !self.registry.contains(*name))
            .collect();
        if !missing.is_empty() {
            let names = missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            self.finish(
                &mut ctx,
                RunStatus::Aborted {
                    reason: format!(
                        "required stages not registered yet: {names} — they land in later build phases"
                    ),
                },
                store,
                sinks,
            )
            .await;
            return ctx.run;
        }

        for name in PREAMBLE {
            if let Err(reason) = self.run_stage(name, &mut ctx, sinks).await {
                self.finish(&mut ctx, RunStatus::Failed { reason }, store, sinks)
                    .await;
                return ctx.run;
            }
        }

        let deadline = Instant::now() + ctx.config.timeout();
        let mut index: u32 = 0;
        let mut final_status = loop {
            if Instant::now() >= deadline {
                break RunStatus::TimedOut;
            }

            // Iteration-scoped artifacts reset every loop; preamble artifacts
            // (repository.profile, task.analysis, plan) persist.
            for key in ITERATION_ARTIFACTS {
                ctx.artifacts.remove(key);
            }

            sinks.record(&Event::now(
                &ctx.run.id,
                Some(index),
                EventKind::IterationStarted { index },
            ));
            ctx.run.iterations.push(Iteration::new(index));

            // Iteration 0 implements the plan; later iterations improve from
            // the previous failure analysis (spec §47 loop: IMPROVE loops
            // back through TEST → DIAGNOSE).
            let work_stage = if index == 0 {
                StageName::Implement
            } else {
                StageName::Improve
            };

            let mut stage_failure: Option<String> = None;
            for name in [work_stage, StageName::Test] {
                if let Err(reason) = self.run_stage(name, &mut ctx, sinks).await {
                    stage_failure = Some(reason);
                    break;
                }
            }
            if let Some(reason) = stage_failure {
                break RunStatus::Failed { reason };
            }

            let tests = ctx.tests();
            if let (Some(summary), Some(iteration)) = (tests.clone(), ctx.current_iteration_mut()) {
                iteration.tests = Some(summary);
            }
            let passed = tests.as_ref().is_some_and(TestSummary::all_passed);

            if !passed
                && let Err(reason) = self.run_stage(StageName::Diagnose, &mut ctx, sinks).await
            {
                break RunStatus::Failed { reason };
            }

            // Checkpoint the iteration as a git commit (spec §22). Failures
            // to checkpoint never abort the run — they are degraded evidence.
            if crate::git::is_repo(&ctx.task.repo_root) {
                let tests_note = tests
                    .as_ref()
                    .map(|t| format!("{}/{}", t.passed, t.total))
                    .unwrap_or_else(|| "n/a".to_string());
                let message = format!("forgeman: iteration {index} — tests {tests_note}");
                match crate::git::commit_all(&ctx.task.repo_root, &message) {
                    Ok(Some(hash)) => {
                        if let Some(iteration) = ctx.current_iteration_mut() {
                            iteration.git_commit = Some(hash.clone());
                        }
                        ctx.defer_event(EventKind::DecisionCreated {
                            summary: format!("git checkpoint {hash} ({tests_note})"),
                        });
                    }
                    Ok(None) => {}
                    Err(err) => eprintln!("warning: git checkpoint failed: {err}"),
                }
            }

            sinks.record(&Event::now(
                &ctx.run.id,
                Some(index),
                EventKind::IterationCompleted {
                    index,
                    tests_passed: passed,
                },
            ));

            // Stop condition: all tests pass and no critical regression.
            if passed && ctx.critical_regressions() == 0 {
                if self.registry.contains(StageName::Verify)
                    && let Err(reason) = self.run_stage(StageName::Verify, &mut ctx, sinks).await
                {
                    break RunStatus::Failed { reason };
                }
                break RunStatus::Verified;
            }

            index += 1;
            if index >= ctx.config.execution.max_iterations {
                break RunStatus::Exhausted { iterations: index };
            }
            if ctx.run.total_cost_usd >= ctx.config.budget.max_cost_usd {
                break RunStatus::BudgetExceeded;
            }
        };

        if self.registry.contains(StageName::Report)
            && let Err(reason) = self.run_stage(StageName::Report, &mut ctx, sinks).await
        {
            final_status = RunStatus::Failed { reason };
        }

        self.finish(&mut ctx, final_status, store, sinks).await;
        ctx.run
    }

    /// Run one stage with bounded retries (spec: no infinite retry).
    /// `Blocked` errors are non-retryable and escalate immediately.
    async fn run_stage(
        &self,
        name: StageName,
        ctx: &mut RunContext,
        sinks: &dyn EventSink,
    ) -> Result<(), String> {
        let stage = self
            .registry
            .get(name)
            .ok_or_else(|| format!("stage `{name}` is not registered"))?;

        let max_attempts = ctx.config.execution.max_stage_attempts;
        let mut last_error = String::new();
        let mut total_duration_ms: u64 = 0;

        for attempt in 1..=max_attempts {
            let iter_index = ctx.current_iteration_index();
            sinks.record(&Event::now(
                &ctx.run.id,
                iter_index,
                EventKind::StageStarted { stage: name },
            ));
            let started = Instant::now();

            match stage.execute(ctx).await {
                Ok(output) => {
                    let duration_ms = started.elapsed().as_millis() as u64;
                    for (key, value) in output.artifacts {
                        ctx.put_artifact(key, value);
                    }
                    // Flush stage-deferred events (tool executions, …).
                    for kind in ctx.event_buffer.drain(..) {
                        sinks.record(&Event::now(&ctx.run.id, iter_index, kind));
                    }
                    ctx.push_stage_result(StageResult {
                        stage: name,
                        status: StageStatus::Success,
                        attempts: attempt,
                        duration_ms,
                        detail: output.detail,
                    });
                    sinks.record(&Event::now(
                        &ctx.run.id,
                        iter_index,
                        EventKind::StageCompleted {
                            stage: name,
                            status: StageStatus::Success,
                            attempts: attempt,
                            duration_ms,
                        },
                    ));
                    return Ok(());
                }
                Err(err) => {
                    let duration_ms = started.elapsed().as_millis() as u64;
                    total_duration_ms += duration_ms;
                    let message = err.to_string();
                    let status = if err.is_retryable() {
                        StageStatus::Failed
                    } else {
                        StageStatus::Escalated
                    };
                    for kind in ctx.event_buffer.drain(..) {
                        sinks.record(&Event::now(&ctx.run.id, iter_index, kind));
                    }
                    sinks.record(&Event::now(
                        &ctx.run.id,
                        iter_index,
                        EventKind::FailureDetected {
                            stage: name,
                            message: message.clone(),
                            attempt,
                        },
                    ));
                    sinks.record(&Event::now(
                        &ctx.run.id,
                        iter_index,
                        EventKind::StageCompleted {
                            stage: name,
                            status,
                            attempts: attempt,
                            duration_ms,
                        },
                    ));
                    ctx.record_failure(FailureRecord {
                        stage: name,
                        message: message.clone(),
                        root_cause: None,
                        confidence: None,
                        recommended_action: None,
                    });
                    last_error = message;
                    if !err.is_retryable() {
                        break;
                    }
                }
            }
        }

        // Record the escalated outcome so iterations always show what happened,
        // including stages that never succeeded (spec: iteration evidence).
        ctx.push_stage_result(StageResult {
            stage: name,
            status: StageStatus::Escalated,
            attempts: max_attempts,
            duration_ms: total_duration_ms,
            detail: Some(last_error.clone()),
        });

        Err(format!(
            "unable to automatically resolve stage `{name}` after {max_attempts} attempt(s). Last error: {last_error}"
        ))
    }

    async fn finish(
        &self,
        ctx: &mut RunContext,
        status: RunStatus,
        store: &RunStore,
        sinks: &dyn EventSink,
    ) {
        ctx.run.status = status.clone();
        ctx.run.finished_at = Some(Utc::now());
        sinks.record(&Event::now(
            &ctx.run.id,
            None,
            EventKind::RunCompleted { status },
        ));
        if let Err(err) = store.save_run(&ctx.run) {
            eprintln!("warning: failed to persist run record: {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::JsonlSink;
    use crate::core::model::{TestSummary, new_task_id};
    use crate::core::stage::{StageError, StageOutput};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    struct FakeStage<F>
    where
        F: Fn(&mut RunContext) -> Result<StageOutput, StageError> + Send + Sync,
    {
        name: StageName,
        behavior: F,
    }

    impl<F> Stage for FakeStage<F>
    where
        F: Fn(&mut RunContext) -> Result<StageOutput, StageError> + Send + Sync,
    {
        fn name(&self) -> StageName {
            self.name
        }

        fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> crate::core::stage::StageFuture<'a> {
            Box::pin(async move { (self.behavior)(ctx) })
        }
    }

    fn fake(name: StageName) -> Arc<dyn Stage> {
        Arc::new(FakeStage {
            name,
            behavior: |_| Ok(StageOutput::default()),
        })
    }

    fn publish_tests(failed: u32) -> Result<StageOutput, StageError> {
        let summary = TestSummary {
            total: 10,
            passed: 10 - failed,
            failed,
            command: "fake".into(),
            duration_ms: 1,
        };
        Ok(StageOutput::default()
            .with_artifact("tests.result", serde_json::to_value(summary).unwrap()))
    }

    fn improving_stage() -> Arc<dyn Stage> {
        Arc::new(FakeStage {
            name: StageName::Improve,
            behavior: |ctx| {
                let fixes = ctx.artifact("fixes").and_then(|v| v.as_u64()).unwrap_or(0) + 1;
                ctx.put_artifact("fixes", serde_json::json!(fixes));
                Ok(StageOutput::default())
            },
        })
    }

    fn full_registry(test: Arc<dyn Stage>) -> StageRegistry {
        let mut reg = StageRegistry::new();
        reg.register(fake(StageName::Inspect));
        reg.register(fake(StageName::Analyze));
        reg.register(fake(StageName::Plan));
        reg.register(fake(StageName::Implement));
        reg.register(test);
        reg.register(fake(StageName::Diagnose));
        reg.register(improving_stage());
        reg.register(fake(StageName::Verify));
        reg.register(fake(StageName::Report));
        reg
    }

    fn config_with(max_iterations: u32, attempts: u32) -> Config {
        let mut c = Config::default();
        c.execution.max_iterations = max_iterations;
        c.execution.max_stage_attempts = attempts;
        c
    }

    fn make_task() -> Task {
        // A unique non-existent path outside this repository: the orchestrator
        // checkpoints to git when repo_root is a real git repo, and tests must
        // never touch the developer's working tree (learned the hard way).
        let repo_root =
            std::env::temp_dir().join(format!("forgeman-test-{}", uuid::Uuid::new_v4()));
        Task {
            id: new_task_id(),
            description: "Fix the authentication bug".into(),
            repo_root,
            created_at: Utc::now(),
        }
    }

    #[derive(Default, Clone)]
    struct CollectedSink {
        events: Arc<Mutex<Vec<Event>>>,
    }

    impl EventSink for CollectedSink {
        fn record(&self, event: &Event) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[tokio::test]
    async fn run_completes_verified_when_tests_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let orch = Orchestrator::new(full_registry(Arc::new(FakeStage {
            name: StageName::Test,
            behavior: |_| publish_tests(0),
        })));
        let sink = CollectedSink::default();

        let run = orch
            .execute_run(make_task(), config_with(5, 3), &store, &sink)
            .await;

        assert_eq!(run.status, RunStatus::Verified);
        assert_eq!(run.iterations.len(), 1);
        assert_eq!(run.preamble_results.len(), 3);
        assert!(run.iterations[0].tests.as_ref().unwrap().all_passed());
        // The run record is persisted with matching status.
        let reloaded = store.load_run(&run.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Verified);
    }

    #[tokio::test]
    async fn run_improves_until_tests_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let orch = Orchestrator::new(full_registry(Arc::new(FakeStage {
            name: StageName::Test,
            behavior: |ctx| {
                let fixes = ctx.artifact("fixes").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                publish_tests(if fixes >= 2 { 0 } else { 3 })
            },
        })));
        let sink = CollectedSink::default();

        let run = orch
            .execute_run(make_task(), config_with(5, 3), &store, &sink)
            .await;

        assert_eq!(run.status, RunStatus::Verified);
        assert_eq!(run.iterations.len(), 3);
        assert_eq!(run.iterations[0].tests.as_ref().unwrap().failed, 3);
        assert_eq!(run.iterations[2].tests.as_ref().unwrap().failed, 0);
    }

    #[tokio::test]
    async fn run_exhausts_at_max_iterations() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let orch = Orchestrator::new(full_registry(Arc::new(FakeStage {
            name: StageName::Test,
            behavior: |_| publish_tests(3),
        })));

        let run = orch
            .execute_run(
                make_task(),
                config_with(2, 3),
                &store,
                &CollectedSink::default(),
            )
            .await;

        assert_eq!(run.status, RunStatus::Exhausted { iterations: 2 });
        assert_eq!(run.iterations.len(), 2);
    }

    #[tokio::test]
    async fn run_aborts_when_required_stages_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let orch = Orchestrator::new(StageRegistry::new());
        let sink = CollectedSink::default();

        let run = orch
            .execute_run(make_task(), config_with(5, 3), &store, &sink)
            .await;

        match &run.status {
            RunStatus::Aborted { reason } => {
                assert!(
                    reason.contains("inspect"),
                    "reason should name missing stages: {reason}"
                );
            }
            other => panic!("expected Aborted, got {other:?}"),
        }
        // Even aborted runs are persisted as evidence.
        let reloaded = store.load_run(&run.id).unwrap();
        assert_eq!(reloaded.status, run.status);
    }

    #[tokio::test]
    async fn run_fails_after_repeated_stage_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let mut reg = StageRegistry::new();
        reg.register(fake(StageName::Inspect));
        reg.register(fake(StageName::Analyze));
        reg.register(fake(StageName::Plan));
        reg.register(Arc::new(FakeStage {
            name: StageName::Implement,
            behavior: |_| Err(StageError::failed(StageName::Implement, "boom")),
        }));
        reg.register(fake(StageName::Test));
        reg.register(fake(StageName::Diagnose));
        reg.register(fake(StageName::Improve));
        let orch = Orchestrator::new(reg);
        let sink = CollectedSink::default();

        let run = orch
            .execute_run(make_task(), config_with(5, 2), &store, &sink)
            .await;

        match &run.status {
            RunStatus::Failed { reason } => {
                assert!(reason.contains("after 2 attempt(s)"), "got: {reason}");
                assert!(reason.contains("boom"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(run.iterations[0].stage_results[0].attempts, 2);
        let failures = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e.kind, EventKind::FailureDetected { .. }))
            .count();
        assert_eq!(failures, 2, "one failure.detected event per attempt");
    }

    #[tokio::test]
    async fn run_times_out() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let orch = Orchestrator::new(full_registry(Arc::new(FakeStage {
            name: StageName::Test,
            behavior: |_| publish_tests(0),
        })));
        let mut config = config_with(5, 3);
        config.execution.timeout_minutes = 0;

        let run = orch
            .execute_run(make_task(), config, &store, &CollectedSink::default())
            .await;

        assert_eq!(run.status, RunStatus::TimedOut);
        assert!(run.iterations.is_empty());
    }

    #[test]
    fn jsonl_sink_writes_dotted_event_names() {
        let tmp = tempfile::tempdir().unwrap();
        let runs_root = tmp.path().join("runs");
        let sink = JsonlSink::new(runs_root.clone());

        sink.record(&Event::now(
            "run_20260829_000000_aaa111",
            Some(0),
            EventKind::StageStarted {
                stage: StageName::Test,
            },
        ));

        let path = runs_root
            .join("run_20260829_000000_aaa111")
            .join("events.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("\"event\":\"stage.started\""),
            "got: {content}"
        );
        assert!(content.contains("\"stage\":\"test\""), "got: {content}");
    }
}
