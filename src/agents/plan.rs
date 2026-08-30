//! Planner agent (spec §11): converts the task analysis into an executable,
//! verifiable implementation plan.

use std::sync::Arc;

use crate::core::model::StageName;
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};
use crate::providers::AgentProvider;

pub use crate::core::model::ImplementationPlan;

use super::analyze::TaskAnalysis;
use super::llm::{complete, extract_json};

pub const PLAN_SYSTEM: &str = "\
You are ForgeMan's engineering planner. Given a repository profile and a task \
analysis, produce a minimal executable implementation plan. Steps must be \
concrete enough to implement and each plan must include validation criteria. \
Respond with ONLY a valid JSON object, no prose, no markdown fences, matching \
this exact schema:\n\
{\n\
  \"summary\": \"one-sentence strategy statement\",\n\
  \"steps\": [\n\
    { \"description\": \"what to do\", \"affected_files\": [\"paths\"] }\n\
  ],\n\
  \"validation_criteria\": [\"how to prove the change works\"],\n\
  \"rollback\": \"how to undo the change safely\"\n\
}";

/// Build the plan from an analysis. Returns plan plus raw response for cost.
pub async fn build_plan(
    provider: &dyn AgentProvider,
    profile_summary: &str,
    analysis: &TaskAnalysis,
    feedback: &str,
) -> Result<(ImplementationPlan, crate::providers::Response), String> {
    let analysis_json = serde_json::to_string_pretty(analysis)
        .map_err(|err| format!("cannot serialize analysis: {err}"))?;
    let feedback_block = if feedback.is_empty() {
        String::new()
    } else {
        format!("\n\nPREVIOUS ATTEMPT FAILED:\n{feedback}\nAdjust your output accordingly.")
    };
    let user = format!(
        "REPOSITORY PROFILE:\n{profile_summary}\n\nTASK ANALYSIS:\n{analysis_json}{feedback_block}\n\n\
         Produce the implementation plan JSON now."
    );
    let response = complete(provider, PLAN_SYSTEM, user).await?;
    let value = extract_json(&response.text)
        .ok_or_else(|| "LLM response contained no valid JSON".to_string())?;
    let plan: ImplementationPlan =
        serde_json::from_value(value).map_err(|err| format!("plan schema mismatch: {err}"))?;
    if plan.steps.is_empty() {
        return Err("plan contains no steps".to_string());
    }
    Ok((plan, response))
}

/// Stage wrapper: reads `task.analysis`, publishes `plan`.
pub struct PlanStage {
    pub provider: Arc<dyn AgentProvider>,
}

impl Stage for PlanStage {
    fn name(&self) -> StageName {
        StageName::Plan
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let profile = super::inspect::profile_of(ctx).ok_or_else(|| {
                StageError::blocked(
                    StageName::Plan,
                    "repository.profile artifact missing — inspect must run first",
                )
            })?;
            let analysis: super::analyze::TaskAnalysis =
                ctx.artifact_as("task.analysis").ok_or_else(|| {
                    StageError::blocked(
                        StageName::Plan,
                        "task.analysis artifact missing — analyze must run first",
                    )
                })?;

            let feedback = ctx
                .stage_feedback
                .get(&StageName::Plan)
                .cloned()
                .unwrap_or_default();
            let (plan, response) = build_plan(
                self.provider.as_ref(),
                &profile.summary(),
                &analysis,
                &feedback,
            )
            .await
            .map_err(|err| StageError::failed(StageName::Plan, err))?;
            ctx.run.total_cost_usd += response.cost_usd;

            let detail = format!(
                "{} step(s): {} [{} tok]",
                plan.steps.len(),
                plan.summary,
                response.output_tokens
            );
            let value = serde_json::to_value(&plan)
                .map_err(|err| StageError::failed(StageName::Plan, err.to_string()))?;

            Ok(StageOutput::default()
                .with_artifact("plan", value)
                .with_detail(detail))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::analyze::TaskAnalysis;
    use crate::config::Config;
    use crate::core::model::{Run, Task, new_task_id};
    use crate::core::stage::RunContext;
    use crate::providers::test_util::spawn_one_shot;
    use std::path::PathBuf;

    fn plan_json() -> serde_json::Value {
        serde_json::json!({
            "summary": "Validate exp claim in the JWT service",
            "steps": [
                { "description": "Add expiration validation", "affected_files": ["src/auth/jwt.rs"] },
                { "description": "Add regression test", "affected_files": ["tests/auth.rs"] }
            ],
            "validation_criteria": ["all tests pass", "expired token returns 401"],
            "rollback": "git revert the commit"
        })
    }

    fn mock_provider(content: String) -> Arc<dyn AgentProvider> {
        let body = serde_json::json!({
            "model": "claude-sonnet-4",
            "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 200, "completion_tokens": 80 }
        })
        .to_string();
        let (url, _rx) = spawn_one_shot(200, body.leak());
        Arc::new(crate::providers::zai::ZaiProvider::new(
            Some("test".into()),
            "glm-4.7-flash",
            url,
        ))
    }

    fn make_ctx_with_analysis() -> RunContext {
        let task = Task {
            id: new_task_id(),
            description: "Fix the auth bug".into(),
            repo_root: PathBuf::from("."),
            created_at: chrono::Utc::now(),
        };
        let run = Run::starting(task.clone(), Config::default());
        let mut ctx = RunContext::new(Config::default(), task, run);
        let profile = crate::repository::profile::RepositoryProfile {
            root: PathBuf::from("."),
            primary_language: "Rust".into(),
            languages: vec![],
            framework: None,
            package_manager: Some("cargo".into()),
            entrypoints: vec![],
            test_frameworks: vec![],
            dependencies: vec![],
            config_files: vec![],
            databases: vec![],
            external_services: vec![],
            risky_areas: vec![],
            file_count: 5,
            tree: vec![],
        };
        ctx.put_artifact(
            "repository.profile",
            serde_json::to_value(&profile).unwrap(),
        );
        let analysis = TaskAnalysis {
            goal: "Reject expired JWT".into(),
            affected_components: vec![],
            constraints: vec![],
            assumptions: vec![],
            risks: vec![],
            edge_cases: vec![],
            ambiguities: vec![],
        };
        ctx.put_artifact("task.analysis", serde_json::to_value(&analysis).unwrap());
        ctx
    }

    #[tokio::test]
    async fn plan_stage_publishes_artifact() {
        let fenced = format!("```json\n{}\n```", plan_json());
        let provider = mock_provider(fenced);
        let mut ctx = make_ctx_with_analysis();
        let stage = PlanStage { provider };

        let output = stage.execute(&mut ctx).await.unwrap();
        for (key, value) in output.artifacts {
            ctx.put_artifact(key, value);
        }

        let plan: ImplementationPlan = ctx.artifact_as("plan").unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert!(plan.validation_criteria.len() >= 1);
        assert!(ctx.run.total_cost_usd > 0.0);
    }

    #[tokio::test]
    async fn plan_stage_rejects_empty_plan() {
        let empty = serde_json::json!({
            "summary": "s",
            "steps": [],
            "validation_criteria": [],
            "rollback": "revert"
        });
        let provider = mock_provider(format!("```json\n{empty}\n```"));
        let mut ctx = make_ctx_with_analysis();
        let stage = PlanStage { provider };

        let err = stage.execute(&mut ctx).await.unwrap_err();
        assert!(err.to_string().contains("no steps"));
    }

    #[tokio::test]
    async fn plan_stage_blocked_without_analysis() {
        let task = Task {
            id: new_task_id(),
            description: "x".into(),
            repo_root: PathBuf::from("."),
            created_at: chrono::Utc::now(),
        };
        let run = Run::starting(task.clone(), Config::default());
        let mut ctx = RunContext::new(Config::default(), task, run);
        // Profile present (inspect ran), but analysis missing — plan must
        // block on the missing analysis, not the profile.
        let profile = crate::repository::profile::RepositoryProfile {
            root: PathBuf::from("."),
            primary_language: "Rust".into(),
            languages: vec![],
            framework: None,
            package_manager: Some("cargo".into()),
            entrypoints: vec![],
            test_frameworks: vec![],
            dependencies: vec![],
            config_files: vec![],
            databases: vec![],
            external_services: vec![],
            risky_areas: vec![],
            file_count: 5,
            tree: vec![],
        };
        ctx.put_artifact(
            "repository.profile",
            serde_json::to_value(&profile).unwrap(),
        );
        let provider = mock_provider("{}".to_string());
        let stage = PlanStage { provider };

        let err = stage.execute(&mut ctx).await.unwrap_err();
        assert!(err.to_string().contains("task.analysis"));
    }
}
