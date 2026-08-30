//! Improve agent (spec §17/Phase 8, part of the Iterative Improvement
//! Engine): takes the previous iteration's failure analysis and produces +
//! applies fix edits. Runs instead of Implement in iterations 1+.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::core::model::StageName;
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};
use crate::providers::AgentProvider;
use crate::tools;

use super::analyze::TaskAnalysis;
use super::coder::{
    CoderOutput, ImplementationChanges, apply_edits, collect_relevant_files, produce_edits,
};
use super::diagnose::FailureAnalysis;
use super::plan::ImplementationPlan;

pub const IMPROVE_SYSTEM: &str = "\
You are ForgeMan's improvement agent. The previous implementation attempt \
failed validation. You receive the failure analysis (root cause, evidence, \
recommended action) and the current content of the relevant files. Produce \
the MINIMAL set of edits that fixes the diagnosed problem WITHOUT undoing \
correct parts of the previous work. Respond with ONLY a valid JSON object, \
no prose, no markdown fences, matching this exact schema:\n\
{\n\
  \"summary\": \"one-sentence description of the fix\",\n\
  \"edits\": [\n\
    { \"path\": \"relative/file/path.ext\", \"action\": \"write\", \
\"content\": \"COMPLETE new file content\" },\n\
    { \"path\": \"obsolete/file.ext\", \"action\": \"delete\", \"content\": \"\" }\n\
  ]\n\
}\n\
Rules: `content` must be the COMPLETE final file content. Only touch files \
involved in the fix. Edit existing files IN PLACE — do not split existing \
code into new files. Every module your code requires must be written by an \
edit in this same set. Keep existing code style.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementChanges {
    pub summary: String,
    pub written_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub root_cause_addressed: String,
}

pub struct ImproveStage {
    pub provider: Arc<dyn AgentProvider>,
}

impl Stage for ImproveStage {
    fn name(&self) -> StageName {
        StageName::Improve
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let analysis: TaskAnalysis = ctx.artifact_as("task.analysis").ok_or_else(|| {
                StageError::blocked(
                    StageName::Improve,
                    "task.analysis artifact missing — analyze must run first",
                )
            })?;
            let plan: ImplementationPlan = ctx.artifact_as("plan").ok_or_else(|| {
                StageError::blocked(
                    StageName::Improve,
                    "plan artifact missing — plan must run first",
                )
            })?;
            let failure: FailureAnalysis =
                ctx.artifact_as("failure.analysis").ok_or_else(|| {
                    StageError::blocked(
                        StageName::Improve,
                        "failure.analysis artifact missing — diagnose must run first",
                    )
                })?;

            // Context: plan files + suspected files + analysis components.
            let mut file_context = collect_relevant_files(&ctx.task.repo_root, &plan, &analysis);
            let have: BTreeSet<String> =
                file_context.iter().map(|(path, _)| path.clone()).collect();
            for suspected in &failure.suspected_files {
                let path = suspected.replace('\\', "/");
                if !have.contains(&path)
                    && let Ok(content) = tools::read_file(&ctx.task.repo_root, &path)
                {
                    file_context.push((path, content));
                }
            }

            let failure_json = serde_json::to_string_pretty(&failure)
                .map_err(|err| StageError::failed(StageName::Improve, err.to_string()))?;
            let mut files_block = String::new();
            if file_context.is_empty() {
                files_block.push_str("(no existing file content available)\n");
            }
            for (path, content) in &file_context {
                files_block.push_str(&format!(
                    "\n--- FILE: {path} ---\n{content}\n--- END FILE ---\n"
                ));
            }

            let mut user = format!(
                "TASK:\n{task}\n\nFAILURE ANALYSIS (previous attempt):\n{failure_json}\n\n\
                 RELEVANT FILES:{files_block}\n\n\
                 Produce the fix edits JSON now.",
                task = ctx.task.description
            );
            if let Some(sanity) = ctx.artifact("edit.sanity_error").and_then(|v| v.as_str()) {
                user.push_str(&format!(
                    "\n\nPREVIOUS EDIT SET WAS REJECTED BY THE SANITY CHECK:\n{sanity}\n\
                     Correct that problem in this edit set."
                ));
            }

            let (output, response) = produce_edits(self.provider.as_ref(), IMPROVE_SYSTEM, user)
                .await
                .map_err(|err| StageError::failed(StageName::Improve, err))?;
            ctx.run.total_cost_usd += response.cost_usd;

            let changes: ImplementationChanges = apply_edits(ctx, &output)?;
            super::coder::sanity_check(ctx, &changes)
                .map_err(|violation| StageError::failed(StageName::Improve, violation))?;
            let improvement = ImprovementChanges {
                summary: changes.summary,
                written_files: changes.written_files,
                deleted_files: changes.deleted_files,
                root_cause_addressed: failure.root_cause.clone(),
            };

            let detail = format!(
                "fixing \"{}\": {} file(s) written [{} tok]",
                improvement.root_cause_addressed,
                improvement.written_files.len(),
                response.output_tokens
            );
            let value = serde_json::to_value(&improvement)
                .map_err(|err| StageError::failed(StageName::Improve, err.to_string()))?;
            Ok(StageOutput::default()
                .with_artifact("improvement.changes", value)
                .with_detail(detail))
        })
    }
}

/// Keep the coder output type import meaningful for the public API.
#[allow(dead_code)]
type ExportedCoderOutput = CoderOutput;
