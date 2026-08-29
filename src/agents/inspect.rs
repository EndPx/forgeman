use crate::core::model::StageName;
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};

/// Explorer agent (spec §7 Agent A): understand the repository and publish
/// the `repository.profile` artifact used by every downstream stage.
pub struct InspectStage;

impl Stage for InspectStage {
    fn name(&self) -> StageName {
        StageName::Inspect
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let profile =
                crate::repository::inspector::inspect(&ctx.task.repo_root).map_err(|err| {
                    StageError::failed(
                        StageName::Inspect,
                        format!("repository inspection failed: {err:#}"),
                    )
                })?;

            let detail = format!(
                "{} — {} file(s)",
                profile.primary_language, profile.file_count
            );

            let value = serde_json::to_value(&profile)
                .map_err(|err| StageError::failed(StageName::Inspect, err.to_string()))?;

            Ok(StageOutput::default()
                .with_artifact("repository.profile", value)
                .with_detail(detail))
        })
    }
}

/// Access the parsed profile from the context.
/// Used by the analyzer/planner stages landing in Phase 4.
#[allow(dead_code)]
pub fn profile_of(ctx: &RunContext) -> Option<crate::repository::profile::RepositoryProfile> {
    ctx.artifact_as("repository.profile")
}
