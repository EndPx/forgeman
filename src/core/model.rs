use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

use crate::config::Config;

/// Pipeline stages, in execution order. The orchestrator drives them
/// through the ForgeMan core loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageName {
    Inspect,
    Analyze,
    Plan,
    Implement,
    Test,
    Diagnose,
    Improve,
    Verify,
    Report,
}

impl StageName {
    pub const ALL: [StageName; 9] = [
        StageName::Inspect,
        StageName::Analyze,
        StageName::Plan,
        StageName::Implement,
        StageName::Test,
        StageName::Diagnose,
        StageName::Improve,
        StageName::Verify,
        StageName::Report,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            StageName::Inspect => "inspect",
            StageName::Analyze => "analyze",
            StageName::Plan => "plan",
            StageName::Implement => "implement",
            StageName::Test => "test",
            StageName::Diagnose => "diagnose",
            StageName::Improve => "improve",
            StageName::Verify => "verify",
            StageName::Report => "report",
        }
    }
}

impl fmt::Display for StageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunStatus::Running => f.write_str("running"),
            RunStatus::Verified => f.write_str("verified"),
            RunStatus::Failed { reason } => write!(f, "failed — {reason}"),
            RunStatus::Exhausted { iterations } => {
                write!(f, "exhausted after {iterations} iteration(s)")
            }
            RunStatus::TimedOut => f.write_str("timed out"),
            RunStatus::BudgetExceeded => f.write_str("budget exceeded"),
            RunStatus::Aborted { reason } => write!(f, "aborted — {reason}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub description: String,
    pub repo_root: PathBuf,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    /// All critical tests passed and no critical regression — evidence-backed.
    Verified,
    /// A stage failed repeatedly and ForgeMan escalated instead of retrying forever.
    Failed { reason: String },
    /// Stop condition: iteration budget exhausted without verification.
    Exhausted { iterations: u32 },
    TimedOut,
    BudgetExceeded,
    /// Run could not proceed (e.g. required stages missing at this build phase).
    Aborted { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub task: Task,
    /// Configuration snapshot so every run is reproducible.
    pub config: Config,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    /// Stage results executed before the iteration loop (inspect/analyze/plan).
    pub preamble_results: Vec<StageResult>,
    pub iterations: Vec<Iteration>,
    /// Accumulated LLM spend, updated by provider-backed stages (Phase 3+).
    pub total_cost_usd: f64,
}

impl Run {
    pub fn starting(task: Task, config: Config) -> Self {
        Self {
            id: new_run_id(),
            task,
            config,
            started_at: Utc::now(),
            finished_at: None,
            status: RunStatus::Running,
            preamble_results: Vec::new(),
            iterations: Vec::new(),
            total_cost_usd: 0.0,
        }
    }

    pub fn duration_secs(&self) -> u64 {
        let end = self.finished_at.unwrap_or_else(Utc::now);
        (end - self.started_at).num_seconds().max(0) as u64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Iteration {
    pub index: u32,
    pub started_at: DateTime<Utc>,
    pub stage_results: Vec<StageResult>,
    pub tests: Option<TestSummary>,
    pub failures: Vec<FailureRecord>,
    /// Git checkpoint commit created after this iteration (Phase 9).
    pub git_commit: Option<String>,
}

impl Iteration {
    pub fn new(index: u32) -> Self {
        Self {
            index,
            started_at: Utc::now(),
            stage_results: Vec::new(),
            tests: None,
            failures: Vec::new(),
            git_commit: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: StageName,
    pub status: StageStatus,
    pub attempts: u32,
    pub duration_ms: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Success,
    Failed,
    /// Non-retryable failure (missing prerequisite) — escalated immediately.
    Escalated,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub duration_ms: u64,
}

impl TestSummary {
    pub fn all_passed(&self) -> bool {
        self.failed == 0 && self.total > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionEvaluation {
    pub critical_failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureRecord {
    pub stage: StageName,
    pub message: String,
    pub root_cause: Option<String>,
    pub confidence: Option<f32>,
    pub recommended_action: Option<String>,
}

/// `run_YYYYMMDD_HHMMSS_xxxxxx` — lexicographic order is chronological.
pub fn new_run_id() -> String {
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..6];
    format!("run_{}_{}", Utc::now().format("%Y%m%d_%H%M%S"), suffix)
}

pub fn new_task_id() -> String {
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..6];
    format!("task_{}", suffix)
}
