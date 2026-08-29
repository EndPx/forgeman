//! Verify stage (Phase 8): the final gate before VERIFIED. It re-asserts
//! the stop condition from evidence — all tests passing and zero critical
//! regressions — and publishes the verification record.

use serde::{Deserialize, Serialize};

use crate::core::model::StageName;
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub status: String,
    pub tests_passed: u32,
    pub tests_total: u32,
    pub critical_regressions: u32,
}

pub struct VerifyStage;

impl Stage for VerifyStage {
    fn name(&self) -> StageName {
        StageName::Verify
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let tests: crate::core::model::TestSummary =
                ctx.artifact_as("tests.result").ok_or_else(|| {
                    StageError::blocked(
                        StageName::Verify,
                        "tests.result artifact missing — test must run first",
                    )
                })?;

            // Verification is evidence-only: no LLM judgement here.
            if !tests.all_passed() {
                return Err(StageError::failed(
                    StageName::Verify,
                    format!(
                        "verification failed: {}/{} tests passing",
                        tests.passed, tests.total
                    ),
                ));
            }
            let regressions = ctx.critical_regressions();
            if regressions > 0 {
                return Err(StageError::failed(
                    StageName::Verify,
                    format!("verification failed: {regressions} critical regression(s)"),
                ));
            }

            let verification = Verification {
                status: "verified".to_string(),
                tests_passed: tests.passed,
                tests_total: tests.total,
                critical_regressions: 0,
            };
            let value = serde_json::to_value(&verification)
                .map_err(|err| StageError::failed(StageName::Verify, err.to_string()))?;
            Ok(StageOutput::default()
                .with_artifact("verification", value)
                .with_detail(format!(
                    "VERIFIED — {}/{} tests, 0 critical regressions",
                    tests.passed, tests.total
                )))
        })
    }
}
