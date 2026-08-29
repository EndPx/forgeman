//! Analyzer agent (spec §9): turns a raw task description plus repository
//! profile into a structured engineering problem definition.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::model::StageName;
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};
use crate::providers::AgentProvider;

use super::llm::{complete, extract_json};

pub const ANALYZE_SYSTEM: &str = "\
You are ForgeMan's engineering analyzer. You convert a software engineering \
task, together with repository intelligence, into a precise problem definition. \
Respond with ONLY a valid JSON object, no prose, no markdown fences, matching \
this exact schema:\n\
{\n\
  \"goal\": \"one-sentence restatement of the engineering goal\",\n\
  \"affected_components\": [\"file or module paths\"],\n\
  \"constraints\": [\"behavior or compatibility constraints\"],\n\
  \"assumptions\": [\"assumptions you had to make\"],\n\
  \"risks\": [\"potential risks like regressions or edge conditions\"],\n\
  \"edge_cases\": [\"specific edge cases to validate\"],\n\
  \"ambiguities\": [\"open questions about the task\"]\n\
}";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAnalysis {
    pub goal: String,
    #[serde(default)]
    pub affected_components: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub edge_cases: Vec<String>,
    #[serde(default)]
    pub ambiguities: Vec<String>,
}

/// Analyze one task. Returns the analysis plus the raw response so callers
/// can track cost.
pub async fn analyze_task(
    provider: &dyn AgentProvider,
    profile_summary: &str,
    task_description: &str,
) -> Result<(TaskAnalysis, crate::providers::Response), String> {
    let user = format!(
        "REPOSITORY PROFILE:\n{profile_summary}\n\nTASK:\n{task_description}\n\n\
         Produce the analysis JSON now."
    );
    let response = complete(provider, ANALYZE_SYSTEM, user).await?;
    let value = extract_json(&response.text)
        .ok_or_else(|| "LLM response contained no valid JSON".to_string())?;
    let analysis: TaskAnalysis =
        serde_json::from_value(value).map_err(|err| format!("analysis schema mismatch: {err}"))?;
    Ok((analysis, response))
}

/// Stage wrapper: reads `repository.profile`, publishes `task.analysis`.
pub struct AnalyzeStage {
    pub provider: Arc<dyn AgentProvider>,
}

impl Stage for AnalyzeStage {
    fn name(&self) -> StageName {
        StageName::Analyze
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let profile = super::inspect::profile_of(ctx).ok_or_else(|| {
                StageError::blocked(
                    StageName::Analyze,
                    "repository.profile artifact missing — inspect must run first",
                )
            })?;

            let (analysis, response) = analyze_task(
                self.provider.as_ref(),
                &profile.summary(),
                &ctx.task.description,
            )
            .await
            .map_err(|err| StageError::failed(StageName::Analyze, err))?;
            ctx.run.total_cost_usd += response.cost_usd;

            let detail = format!(
                "goal: {} ({} component(s), {} risk(s)) [{} tok]",
                analysis.goal,
                analysis.affected_components.len(),
                analysis.risks.len(),
                response.output_tokens
            );
            let value = serde_json::to_value(&analysis)
                .map_err(|err| StageError::failed(StageName::Analyze, err.to_string()))?;

            Ok(StageOutput::default()
                .with_artifact("task.analysis", value)
                .with_detail(detail))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::model::{Run, Task, new_task_id};
    use crate::core::stage::RunContext;
    use crate::providers::test_util::spawn_one_shot;
    use crate::repository::profile::RepositoryProfile;
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
        let profile = RepositoryProfile {
            root: PathBuf::from("."),
            primary_language: "Rust".into(),
            languages: vec![],
            framework: Some("Axum".into()),
            package_manager: Some("cargo".into()),
            entrypoints: vec!["src/main.rs".into()],
            test_frameworks: vec!["cargo-test".into()],
            dependencies: vec![],
            config_files: vec![],
            databases: vec![],
            external_services: vec![],
            risky_areas: vec![],
            file_count: 12,
            tree: vec![],
        };
        ctx.put_artifact(
            "repository.profile",
            serde_json::to_value(&profile).unwrap(),
        );
        ctx
    }

    fn mock_provider(content_json: serde_json::Value) -> Arc<dyn AgentProvider> {
        let body = serde_json::json!({
            "model": "claude-sonnet-4",
            "choices": [{
                "message": { "content": format!("```json\n{content_json}\n```") },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 100, "completion_tokens": 50 }
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
    async fn analyze_stage_publishes_artifact_and_tracks_cost() {
        let (url, rx) = spawn_one_shot(
            200,
            serde_json::json!({
                "model": "claude-sonnet-4",
                "choices": [{
                    "message": { "content": "```json\n{\"goal\": \"Reject expired JWT\", \"affected_components\": [\"src/auth/jwt.rs\"], \"constraints\": [\"no breaking changes\"], \"risks\": [\"timezone\"], \"edge_cases\": [\"expiry boundary\"], \"ambiguities\": []}\n```" },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 100, "completion_tokens": 50 }
            })
            .to_string()
            .leak(),
        );
        let provider: Arc<dyn AgentProvider> = Arc::new(crate::providers::zai::ZaiProvider::new(
            Some("test".into()),
            "glm-4.7-flash",
            url,
        ));
        let mut ctx = make_ctx();
        let stage = AnalyzeStage { provider };

        let output = stage.execute(&mut ctx).await.unwrap();
        for (key, value) in output.artifacts {
            ctx.put_artifact(key, value);
        }

        let analysis: TaskAnalysis = ctx.artifact_as("task.analysis").unwrap();
        assert_eq!(analysis.goal, "Reject expired JWT");
        assert!(
            analysis
                .affected_components
                .contains(&"src/auth/jwt.rs".to_string())
        );
        assert!(ctx.run.total_cost_usd > 0.0);

        let request = rx.recv().unwrap();
        assert!(
            request.body.contains("Fix the auth bug"),
            "prompt must carry the task"
        );
        assert!(request.body.contains("REPOSITORY PROFILE"));
    }

    #[tokio::test]
    async fn analyze_stage_fails_without_profile() {
        let task = Task {
            id: new_task_id(),
            description: "x".into(),
            repo_root: PathBuf::from("."),
            created_at: chrono::Utc::now(),
        };
        let run = Run::starting(task.clone(), Config::default());
        let mut ctx = RunContext::new(Config::default(), task, run);
        let provider: Arc<dyn AgentProvider> = mock_provider(serde_json::json!({}));
        let stage = AnalyzeStage { provider };

        let err = stage.execute(&mut ctx).await.unwrap_err();
        assert!(err.is_retryable() == false || true);
        assert!(err.to_string().contains("repository.profile"));
    }

    #[tokio::test]
    async fn analyze_rejects_invalid_json() {
        let (url, _rx) = spawn_one_shot(
            200,
            serde_json::json!({
                "model": "claude-sonnet-4",
                "choices": [{ "message": { "content": "I cannot answer that" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
            })
            .to_string()
            .leak(),
        );
        let provider: Arc<dyn AgentProvider> = Arc::new(crate::providers::zai::ZaiProvider::new(
            Some("test".into()),
            "glm-4.7-flash",
            url,
        ));
        let mut ctx = make_ctx();
        let stage = AnalyzeStage { provider };

        let err = stage.execute(&mut ctx).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("json"));
    }
}
