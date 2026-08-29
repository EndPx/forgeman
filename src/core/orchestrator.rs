use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;

use crate::config::Config;
use crate::core::events::{Event, EventKind, EventSink};
use crate::core::model::{
    FailureRecord, Iteration, Run, RunStatus, StageName, StageResult, StageStatus, Task, TestSummary,
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
        Self { stages: HashMap::new() }
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
const LOOP: [StageName; 2] = [StageName::Implement, StageName::Test];

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
            EventKind::TaskCreated { description: ctx.task.description.clone() },
        ));
        sinks.record(&Event::now(&ctx.run.id, None, EventKind::RunStarted));

        // Fail fast when required stages are not registered yet.
        let missing: Vec<StageName> = PREAMBLE
            .into_iter()
            .chain(LOOP)
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
                self.finish(&mut ctx, RunStatus::Failed { reason }, store, sinks).await;
                return ctx.run;
            }
        }

        let deadline = Instant::now() + ctx.config.timeout();
        let mut index: u32 = 0;
        let mut final_status = loop {
            if Instant::now() >= deadline {
                break RunStatus::TimedOut;
            }

            sinks.record(&Event::now(
                &ctx.run.id,
                Some(index),
                EventKind::IterationStarted { index },
            ));
            ctx.run.iterations.push(Iteration::new(index));

            let mut stage_failure: Option<String> = None;
            for name in LOOP {
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

            if !passed {
                if let Err(reason) = self.run_stage(StageName::Diagnose, &mut ctx, sinks).await {
                    break RunStatus::Failed { reason };
                }
            }

            sinks.record(&Event::now(
                &ctx.run.id,
                Some(index),
                EventKind::IterationCompleted { index, tests_passed: passed },
            ));

            // Stop condition: all tests pass and no critical regression.
            if passed && ctx.critical_regressions() == 0 {
                if self.registry.contains(StageName::Verify) {
                    if let Err(reason) = self.run_stage(StageName::Verify, &mut ctx, sinks).await {
                        break RunStatus::Failed { reason };
                    }
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

            if let Err(reason) = self.run_stage(StageName::Improve, &mut ctx, sinks).await {
                break RunStatus::Failed { reason };
            }
        };

        if self.registry.contains(StageName::Report) {
            if let Err(reason) = self.run_stage(StageName::Report, &mut ctx, sinks).await {
                final_status = RunStatus::Failed { reason };
            }
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
                    let message = err.to_string();
                    let status = if err.is_retryable() {
                        StageStatus::Failed
                    } else {
                        StageStatus::Escalated
                    };
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
