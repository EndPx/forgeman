// Framework surface consumed by stage implementations landing in Phases 2–8.
#![allow(dead_code)]

use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use crate::config::Config;
use crate::core::model::{
    FailureRecord, Iteration, RegressionEvaluation, Run, StageName, StageResult, Task, TestSummary,
};

pub type StageFuture<'a> =
    Pin<Box<dyn Future<Output = Result<StageOutput, StageError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("{message}")]
    Failed { stage: StageName, message: String },
    /// Non-retryable: a prerequisite is missing (e.g. upstream artifact absent).
    #[error("{message}")]
    Blocked { stage: StageName, message: String },
}

impl StageError {
    pub fn failed(stage: StageName, message: impl Into<String>) -> Self {
        Self::Failed { stage, message: message.into() }
    }

    pub fn blocked(stage: StageName, message: impl Into<String>) -> Self {
        Self::Blocked { stage, message: message.into() }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, StageError::Failed { .. })
    }
}

#[derive(Debug, Clone, Default)]
pub struct StageOutput {
    pub detail: Option<String>,
    /// Artifacts merge into the run context, e.g. `repository.profile`,
    /// `task.analysis`, `plan`, `tests.result`, `failure.analysis`.
    pub artifacts: Vec<(String, serde_json::Value)>,
}

impl StageOutput {
    pub fn with_artifact(mut self, key: &str, value: serde_json::Value) -> Self {
        self.artifacts.push((key.to_string(), value));
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// One step of the ForgeMan loop. Stages are pure orchestration units:
/// they read upstream artifacts from the context and publish their own
/// outputs for downstream stages.
pub trait Stage: Send + Sync {
    fn name(&self) -> StageName;

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a>;
}

/// Mutable state handed to every stage.
pub struct RunContext {
    pub config: Config,
    pub task: Task,
    pub run: Run,
    pub artifacts: BTreeMap<String, serde_json::Value>,
}

impl RunContext {
    pub fn new(config: Config, task: Task, run: Run) -> Self {
        Self { config, task, run, artifacts: BTreeMap::new() }
    }

    pub fn put_artifact(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.artifacts.insert(key.into(), value);
    }

    pub fn artifact(&self, key: &str) -> Option<&serde_json::Value> {
        self.artifacts.get(key)
    }

    pub fn artifact_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.artifacts
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Latest test summary published by the Test stage.
    pub fn tests(&self) -> Option<TestSummary> {
        self.artifact_as("tests.result")
    }

    /// Critical regressions reported by evaluation stages (Phase 6+).
    /// No evaluation artifact means no *known* regression.
    pub fn critical_regressions(&self) -> u32 {
        self.artifact_as::<RegressionEvaluation>("evaluation.regression")
            .map(|r| r.critical_failures)
            .unwrap_or(0)
    }

    pub fn record_failure(&mut self, failure: FailureRecord) {
        if let Some(iteration) = self.current_iteration_mut() {
            iteration.failures.push(failure);
        }
    }

    pub fn current_iteration_mut(&mut self) -> Option<&mut Iteration> {
        self.run.iterations.last_mut()
    }

    pub fn current_iteration_index(&self) -> Option<u32> {
        self.run.iterations.last().map(|i| i.index)
    }

    /// Iteration stages record into the open iteration; preamble stages
    /// (inspect/analyze/plan) record at run level.
    pub fn push_stage_result(&mut self, result: StageResult) {
        match self.current_iteration_mut() {
            Some(iteration) => iteration.stage_results.push(result),
            None => self.run.preamble_results.push(result),
        }
    }
}
