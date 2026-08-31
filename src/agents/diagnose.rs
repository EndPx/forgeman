//! Failure Analyzer (spec §16/Phase 7): the most important differentiator.
//! When tests fail, this agent produces an evidence-based diagnosis —
//! classification, root cause hypothesis, confidence, and recommended fix —
//! independently of the code that was written.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::model::{FailureRecord, StageName};
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};
use crate::providers::AgentProvider;

use super::llm::{complete, extract_json};

pub const DIAGNOSE_SYSTEM: &str = "\
You are ForgeMan's failure analyzer. You receive a failing test summary, the \
raw failure output, the implementation changes just made, and the plan they \
came from. Diagnose WHY the failure happened. You are independent of the \
coder: judge only from evidence. Respond with ONLY a valid JSON object, no \
prose, no markdown fences, matching this exact schema:\n\
{\n\
  \"classification\": \"test-failure | compile-error | regression | timeout | environment\",\n\
  \"evidence\": \"the concrete output lines or facts that show the problem\",\n\
  \"root_cause\": \"one-sentence root cause hypothesis\",\n\
  \"confidence\": 0.0,\n\
  \"recommended_action\": \"the next concrete change to attempt\",\n\
  \"suspected_files\": [\"paths most likely involved\"]\n\
}";

/// Cap on raw failure output fed to the LLM (tokens are finite; failure
/// summaries live at the tail of most reporter output).
const MAX_FAILURE_OUTPUT_CHARS: usize = 12_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureAnalysis {
    pub classification: String,
    pub evidence: String,
    pub root_cause: String,
    pub confidence: f32,
    pub recommended_action: String,
    #[serde(default)]
    pub suspected_files: Vec<String>,
}

/// Diagnose a failure from its output. Returns analysis plus raw response
/// for cost accounting.
pub async fn diagnose_failure(
    provider: &dyn AgentProvider,
    task_description: &str,
    test_summary: &str,
    failure_output: &str,
    changes_summary: &str,
    feedback: &str,
) -> Result<(FailureAnalysis, crate::providers::Response), String> {
    let mut trimmed_output = failure_output.to_string();
    if trimmed_output.len() > MAX_FAILURE_OUTPUT_CHARS {
        trimmed_output = format!(
            "…[truncated head]…\n{}",
            &trimmed_output[trimmed_output.len() - MAX_FAILURE_OUTPUT_CHARS..]
        );
    }

    let feedback_block = if feedback.is_empty() {
        String::new()
    } else {
        format!("\n\nPREVIOUS ATTEMPT FAILED:\n{feedback}\nAdjust your output accordingly.")
    };
    let user = format!(
        "TASK:\n{task_description}\n\nTEST SUMMARY:\n{test_summary}\n\n\
         FAILURE OUTPUT (tail):\n{trimmed_output}\n\n\
         CHANGES JUST APPLIED:\n{changes_summary}{feedback_block}\n\n\
         Produce the failure analysis JSON now."
    );
    let response = complete(provider, DIAGNOSE_SYSTEM, user).await?;
    let value = extract_json(&response.text)
        .ok_or_else(|| "LLM response contained no valid JSON".to_string())?;
    let analysis: FailureAnalysis = serde_json::from_value(value)
        .map_err(|err| format!("failure-analysis schema mismatch: {err}"))?;
    Ok((analysis, response))
}

/// DiagnoseStage: reads `tests.result` + `tests.output`, publishes
/// `failure.analysis` and records a rich FailureRecord on the iteration.
pub struct DiagnoseStage {
    pub provider: Arc<dyn AgentProvider>,
}

impl Stage for DiagnoseStage {
    fn name(&self) -> StageName {
        StageName::Diagnose
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let tests: crate::core::model::TestSummary =
                ctx.artifact_as("tests.result").ok_or_else(|| {
                    StageError::blocked(
                        StageName::Diagnose,
                        "tests.result artifact missing — test must run first",
                    )
                })?;
            let output: String = ctx.artifact_as("tests.output").unwrap_or_default();
            let changes: Option<super::coder::ImplementationChanges> =
                ctx.artifact_as("implementation.changes");
            let changes_summary = changes
                .map(|changes| {
                    format!(
                        "wrote: {}; deleted: {}",
                        changes.written_files.join(", "),
                        changes.deleted_files.join(", ")
                    )
                })
                .unwrap_or_else(|| "(no changes recorded this iteration)".to_string());

            let summary = format!(
                "{}/{} passed via {}",
                tests.passed, tests.total, tests.command
            );

            let feedback = ctx
                .stage_feedback
                .get(&StageName::Diagnose)
                .cloned()
                .unwrap_or_default();
            let (analysis, response) = diagnose_failure(
                self.provider.as_ref(),
                &ctx.task.description,
                &summary,
                &output,
                &changes_summary,
                &feedback,
            )
            .await
            .map_err(|err| StageError::failed(StageName::Diagnose, err))?;
            ctx.run.add_usage(
                response.input_tokens,
                response.output_tokens,
                response.cost_usd,
            );

            // Attach the diagnosis to the current iteration's failure record
            // so the improver (and the report) can trace it.
            ctx.record_failure(FailureRecord {
                stage: StageName::Diagnose,
                message: format!("{} — {}", analysis.classification, analysis.evidence),
                root_cause: Some(analysis.root_cause.clone()),
                confidence: Some(analysis.confidence),
                recommended_action: Some(analysis.recommended_action.clone()),
            });

            let detail = format!(
                "{} (confidence {:.0}%): {}",
                analysis.classification,
                analysis.confidence * 100.0,
                analysis.root_cause
            );
            let value = serde_json::to_value(&analysis)
                .map_err(|err| StageError::failed(StageName::Diagnose, err.to_string()))?;

            Ok(StageOutput::default()
                .with_artifact("failure.analysis", value)
                .with_detail(detail))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::model::{Run, Task, TestSummary, new_task_id};
    use crate::core::stage::RunContext;
    use crate::providers::test_util::spawn_one_shot;
    use std::path::PathBuf;

    fn make_ctx() -> RunContext {
        let task = Task {
            id: new_task_id(),
            description: "Fix the auth bug".into(),
            repo_root: PathBuf::from("."),
            created_at: chrono::Utc::now(),
        };
        let run = Run::starting(task.clone(), Config::default());
        let mut ctx = RunContext::new(Config::default(), task, run);
        let tests = TestSummary {
            total: 10,
            passed: 7,
            failed: 3,
            command: "cargo-test".into(),
            duration_ms: 100,
        };
        ctx.put_artifact("tests.result", serde_json::to_value(&tests).unwrap());
        ctx.put_artifact(
            "tests.output",
            serde_json::json!("thread panicked at 'expected 401, got 200'"),
        );
        ctx
    }

    fn mock_provider(content: String) -> Arc<dyn AgentProvider> {
        let body = serde_json::json!({
            "model": "claude-sonnet-4",
            "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 400, "completion_tokens": 90 }
        })
        .to_string();
        let (url, _rx) = spawn_one_shot(200, body.leak());
        Arc::new(crate::providers::zai::ZaiProvider::new(
            Some("test".into()),
            "glm-4.7-flash",
            url,
        ))
    }

    #[tokio::test]
    async fn diagnose_publishes_analysis_and_failure_record() {
        let analysis = serde_json::json!({
            "classification": "regression",
            "evidence": "expected 401, got 200",
            "root_cause": "Expiration claim parsed but not validated",
            "confidence": 0.94,
            "recommended_action": "Validate exp against current UNIX timestamp",
            "suspected_files": ["src/auth.rs"]
        });
        let provider = mock_provider(format!("```json\n{analysis}\n```"));
        let mut ctx = make_ctx();
        // Diagnose always runs inside an open iteration in the real loop.
        ctx.run
            .iterations
            .push(crate::core::model::Iteration::new(0));
        let stage = DiagnoseStage { provider };

        let output = stage.execute(&mut ctx).await.unwrap();
        for (key, value) in output.artifacts {
            ctx.put_artifact(key, value);
        }

        let parsed: FailureAnalysis = ctx.artifact_as("failure.analysis").unwrap();
        assert_eq!(parsed.classification, "regression");
        assert!((parsed.confidence - 0.94).abs() < 0.001);
        let failure = ctx
            .run
            .iterations
            .last()
            .and_then(|iteration| iteration.failures.last())
            .expect("failure record attached");
        assert_eq!(
            failure.root_cause.as_deref(),
            Some("Expiration claim parsed but not validated")
        );
    }
}
