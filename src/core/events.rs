use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

use crate::core::model::{RunStatus, StageName, StageStatus};

/// Structured event types (spec §38). Serialized with dotted `event` names
/// so the JSONL log stays greppable and stable for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum EventKind {
    #[serde(rename = "task.created")]
    TaskCreated { description: String },
    #[serde(rename = "run.started")]
    RunStarted,
    #[serde(rename = "stage.started")]
    StageStarted { stage: StageName },
    #[serde(rename = "stage.completed")]
    StageCompleted {
        stage: StageName,
        status: StageStatus,
        attempts: u32,
        duration_ms: u64,
    },
    #[serde(rename = "iteration.started")]
    IterationStarted { index: u32 },
    #[serde(rename = "iteration.completed")]
    IterationCompleted { index: u32, tests_passed: bool },
    #[serde(rename = "failure.detected")]
    FailureDetected {
        stage: StageName,
        message: String,
        attempt: u32,
    },
    #[serde(rename = "decision.created")]
    DecisionCreated { summary: String },
    #[serde(rename = "run.completed")]
    RunCompleted { status: RunStatus },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    pub iteration: Option<u32>,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    pub fn now(run_id: &str, iteration: Option<u32>, kind: EventKind) -> Self {
        Self {
            timestamp: Utc::now(),
            run_id: run_id.to_string(),
            iteration,
            kind,
        }
    }
}

/// Anything that can observe the engineering loop as it happens.
pub trait EventSink: Send + Sync {
    fn record(&self, event: &Event);
}

/// Human-readable progress lines on stderr (stdout stays for results).
pub struct ConsoleSink;

impl EventSink for ConsoleSink {
    fn record(&self, event: &Event) {
        let ts = event.timestamp.format("%H:%M:%S");
        let iter = match event.iteration {
            Some(i) => format!(" [it{i}]"),
            None => String::new(),
        };
        match &event.kind {
            EventKind::TaskCreated { description } => {
                eprintln!("[{ts}] task{iter}: {description}")
            }
            EventKind::RunStarted => eprintln!("[{ts}] ▶ run {} started", event.run_id),
            EventKind::StageStarted { stage } => {
                eprintln!("[{ts}]   ▸ {stage}{iter} …")
            }
            EventKind::StageCompleted {
                stage,
                status,
                attempts,
                duration_ms,
            } => {
                let mark = match status {
                    StageStatus::Success => "✓",
                    _ => "✗",
                };
                let retry = if *attempts > 1 {
                    format!(", attempt {attempts}")
                } else {
                    String::new()
                };
                eprintln!("[{ts}]   {mark} {stage} done ({duration_ms}ms{retry}){iter}")
            }
            EventKind::IterationStarted { index } => {
                eprintln!("[{ts}] ── iteration {index}{iter} ──")
            }
            EventKind::IterationCompleted {
                index,
                tests_passed,
            } => {
                let verdict = if *tests_passed { "pass" } else { "fail" };
                eprintln!("[{ts}] ── iteration {index} completed: tests {verdict}{iter} ──")
            }
            EventKind::FailureDetected {
                stage,
                message,
                attempt,
            } => eprintln!(
                "[{ts}]   ✗ {stage} attempt {attempt}: {message}{iter}"
            ),
            EventKind::DecisionCreated { summary } => {
                eprintln!("[{ts}]   ⚑ decision{iter}: {summary}")
            }
            EventKind::RunCompleted { status } => {
                eprintln!("[{ts}] ■ run {} completed — {status}", event.run_id)
            }
        }
    }
}

/// Machine-readable event log: one JSON object per line under
/// `<runs_root>/<run_id>/events.jsonl`.
pub struct JsonlSink {
    runs_root: PathBuf,
}

impl JsonlSink {
    pub fn new(runs_root: PathBuf) -> Self {
        Self { runs_root }
    }

    pub fn events_path(&self, run_id: &str) -> PathBuf {
        self.runs_root.join(run_id).join("events.jsonl")
    }
}

impl EventSink for JsonlSink {
    fn record(&self, event: &Event) {
        let path = self.events_path(&event.run_id);
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("warning: cannot create event dir {}: {err}", parent.display());
                return;
            }
        }
        match serde_json::to_string(event) {
            Ok(line) => {
                let result = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| writeln!(f, "{line}"));
                if let Err(err) = result {
                    eprintln!("warning: failed to append event to {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("warning: failed to serialize event: {err}"),
        }
    }
}

/// Fan-out to multiple sinks.
pub struct MultiSink {
    sinks: Vec<Box<dyn EventSink>>,
}

impl MultiSink {
    pub fn new(sinks: Vec<Box<dyn EventSink>>) -> Self {
        Self { sinks }
    }
}

impl EventSink for MultiSink {
    fn record(&self, event: &Event) {
        for sink in &self.sinks {
            sink.record(event);
        }
    }
}
