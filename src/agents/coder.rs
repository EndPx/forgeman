//! Coder agent (spec §12/§5 Phase 5): implements the plan by producing file
//! edits with the LLM and applying them through the audited tool layer.
//!
//! Design note: GLM-style completion models are most reliable producing
//! whole-file writes rather than fuzzy diffs, so the edit contract is
//! `write` (complete file content) and `delete`. Every edit is confined to
//! the repository root by the tool layer (spec §37).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crate::core::model::StageName;
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};
use crate::providers::AgentProvider;
use crate::tools;

use super::analyze::TaskAnalysis;
use super::llm::extract_json;
use super::plan::ImplementationPlan;

pub const CODER_SYSTEM: &str = "\
You are ForgeMan's coder agent. You implement the requested change by \
editing the repository. You receive the task, the task analysis, the plan, \
and the current content of the relevant files. Respond with ONLY a valid \
JSON object, no prose, no markdown fences, matching this exact schema:\n\
{\n\
  \"summary\": \"one-sentence description of the change\",\n\
  \"edits\": [\n\
    { \"path\": \"relative/file/path.ext\", \"action\": \"write\", \
\"content\": \"COMPLETE new file content\" },\n\
    { \"path\": \"obsolete/file.ext\", \"action\": \"delete\", \"content\": \"\" }\n\
  ]\n\
}\n\
Rules: `content` must be the COMPLETE final file content (not a diff, not a \
snippet). Make the MINIMAL change that satisfies the plan. Only touch files \
named in the plan, the analysis, or the RELEVANT FILES section. Do NOT add \
dependencies, do NOT modify Cargo.toml or package.json, do NOT rename the \
package, do NOT create new binaries or modules unless the plan explicitly \
requires it. Edit existing files IN PLACE — do not split existing code into \
new files. Every module your code requires must be written by an edit in \
this same set. Never create files outside the repository. Keep existing \
code style.";

/// Maximum files embedded in the coder prompt (context budget).
const MAX_CONTEXT_FILES: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEdit {
    pub path: String,
    /// `write` (complete file content) or `delete`.
    pub action: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoderOutput {
    pub summary: String,
    #[serde(default)]
    pub edits: Vec<FileEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationChanges {
    pub summary: String,
    pub written_files: Vec<String>,
    pub deleted_files: Vec<String>,
}

/// Ask the LLM for edits. Returns output plus raw response for cost.
pub async fn implement(
    provider: &dyn AgentProvider,
    task_description: &str,
    analysis: &TaskAnalysis,
    plan: &ImplementationPlan,
    file_context: &[(String, String)],
    feedback: &str,
) -> Result<(CoderOutput, crate::providers::Response), String> {
    let analysis_json = serde_json::to_string_pretty(analysis)
        .map_err(|err| format!("cannot serialize analysis: {err}"))?;
    let plan_json = serde_json::to_string_pretty(plan)
        .map_err(|err| format!("cannot serialize plan: {err}"))?;

    let mut context = String::new();
    if file_context.is_empty() {
        context.push_str("(no existing file content — create new files as needed)\n");
    }
    for (path, content) in file_context {
        context.push_str(&format!(
            "\n--- FILE: {path} ---\n{content}\n--- END FILE ---\n"
        ));
    }

    let feedback_block = if feedback.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nPREVIOUS EDIT SET WAS REJECTED BY THE SANITY CHECK:\n{feedback}\n\
             Correct that problem in this edit set."
        )
    };

    let user = format!(
        "TASK:\n{task_description}\n\nTASK ANALYSIS:\n{analysis_json}\n\n\
         IMPLEMENTATION PLAN:\n{plan_json}\n\nRELEVANT FILES:{context}{feedback_block}\n\n\
         Produce the edits JSON now."
    );
    produce_edits(provider, CODER_SYSTEM, user).await
}

/// Low-level edit request shared by the coder and the improver: send the
/// prompt, parse the edit JSON, refuse empty edit sets.
pub async fn produce_edits(
    provider: &dyn AgentProvider,
    system: &str,
    user: String,
) -> Result<(CoderOutput, crate::providers::Response), String> {
    // Whole-file edits are long, and GLM-style reasoning models spend part
    // of the budget thinking — give them headroom and skip the thinking.
    let response = super::llm::complete_edits(provider, system, user).await?;
    let value = extract_json(&response.text)
        .ok_or_else(|| "LLM response contained no valid JSON".to_string())?;
    let output: CoderOutput =
        serde_json::from_value(value).map_err(|err| format!("coder schema mismatch: {err}"))?;
    if output.edits.is_empty() {
        return Err("coder produced no edits".to_string());
    }
    Ok((output, response))
}

/// Files whose current content is relevant to the edit, selected by the
/// plan and analysis (spec §8 relevance-based context selection).
pub fn collect_relevant_files(
    repo_root: &std::path::Path,
    plan: &ImplementationPlan,
    analysis: &TaskAnalysis,
) -> Vec<(String, String)> {
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    for step in &plan.steps {
        for file in &step.affected_files {
            candidates.insert(file.replace('\\', "/"));
        }
    }
    for component in &analysis.affected_components {
        candidates.insert(component.replace('\\', "/"));
    }

    let existing = crate::repository::inspector::list_text_files(repo_root).unwrap_or_default();
    let mut context = Vec::new();
    for candidate in candidates {
        if context.len() >= MAX_CONTEXT_FILES {
            break;
        }
        if existing.iter().any(|f| f == &candidate)
            && let Ok(content) = tools::read_file(repo_root, &candidate)
        {
            context.push((candidate, content));
        }
    }
    context
}

pub struct CoderStage {
    pub provider: Arc<dyn AgentProvider>,
}

impl Stage for CoderStage {
    fn name(&self) -> StageName {
        StageName::Implement
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let analysis: TaskAnalysis = ctx.artifact_as("task.analysis").ok_or_else(|| {
                StageError::blocked(
                    StageName::Implement,
                    "task.analysis artifact missing — analyze must run first",
                )
            })?;
            let plan: ImplementationPlan = ctx.artifact_as("plan").ok_or_else(|| {
                StageError::blocked(
                    StageName::Implement,
                    "plan artifact missing — plan must run first",
                )
            })?;

            let file_context = collect_relevant_files(&ctx.task.repo_root, &plan, &analysis);
            let feedback = ctx
                .artifact("edit.sanity_error")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let (output, response) = implement(
                self.provider.as_ref(),
                &ctx.task.description,
                &analysis,
                &plan,
                &file_context,
                &feedback,
            )
            .await
            .map_err(|err| StageError::failed(StageName::Implement, err))?;
            ctx.run.total_cost_usd += response.cost_usd;

            let changes = apply_edits(ctx, &output)?;
            sanity_check(ctx, &changes)
                .map_err(|violation| StageError::failed(StageName::Implement, violation))?;

            let detail = format!(
                "{} file(s) written, {} deleted — {} [{} tok]",
                changes.written_files.len(),
                changes.deleted_files.len(),
                changes.summary,
                response.output_tokens
            );
            let value = serde_json::to_value(&changes)
                .map_err(|err| StageError::failed(StageName::Implement, err.to_string()))?;
            Ok(StageOutput::default()
                .with_artifact("implementation.changes", value)
                .with_detail(detail))
        })
    }
}

/// Apply the LLM's edits through the audited tool layer.
pub fn apply_edits(
    ctx: &mut RunContext,
    output: &CoderOutput,
) -> Result<ImplementationChanges, StageError> {
    let mut written = Vec::new();
    let mut deleted = Vec::new();

    for edit in &output.edits {
        let arguments = serde_json::json!({
            "path": edit.path,
            "action": edit.action,
            "content_bytes": edit.content.len(),
        });
        let started = std::time::Instant::now();
        let result = match edit.action.as_str() {
            "write" => tools::write_file(&ctx.task.repo_root, &edit.path, &edit.content)
                .map(|_| format!("wrote {} ({} bytes)", edit.path, edit.content.len())),
            "delete" => tools::delete_file(&ctx.task.repo_root, &edit.path)
                .map(|_| format!("deleted {}", edit.path)),
            other => Err(format!("unknown edit action `{other}` for {}", edit.path)),
        };

        // Traversal or tool refusal: escalate immediately (non-retryable).
        if let Err(message) = &result
            && message.contains("rejected")
        {
            return Err(StageError::blocked(
                StageName::Implement,
                format!("tool refused edit: {message}"),
            ));
        }

        tools::record_tool_execution(ctx, tool_name(&edit.action), arguments, started, &result);
        result.map_err(|message| {
            StageError::failed(StageName::Implement, format!("edit failed: {message}"))
        })?;

        match edit.action.as_str() {
            "write" => written.push(edit.path.clone()),
            "delete" => deleted.push(edit.path.clone()),
            _ => {}
        }
    }

    Ok(ImplementationChanges {
        summary: output.summary.clone(),
        written_files: written,
        deleted_files: deleted,
    })
}

/// Structural sanity check applied right after the edits land and before any
/// test iteration is spent: catch missing local modules (node) and Python
/// syntax errors while the rejection reason can still reach the model.
///
/// Returns the violation text (also stored as `edit.sanity_error` so the
/// retried stage's prompt carries the feedback), or `Ok(())` when clean.
pub fn sanity_check(ctx: &mut RunContext, changes: &ImplementationChanges) -> Result<(), String> {
    if let Err(violation) = check_node_imports(&ctx.task.repo_root, &changes.written_files) {
        ctx.put_artifact("edit.sanity_error", serde_json::json!(violation));
        return Err(violation);
    }
    if let Err(violation) = check_python_syntax(&ctx.task.repo_root, &changes.written_files) {
        ctx.put_artifact("edit.sanity_error", serde_json::json!(violation));
        return Err(violation);
    }
    ctx.artifacts.remove("edit.sanity_error");
    Ok(())
}

const NODE_SOURCE_EXT: &[&str] = &["js", "mjs", "cjs", "jsx", "tsx"];

fn check_node_imports(repo_root: &Path, written: &[String]) -> Result<(), String> {
    let has_node_project = repo_root.join("package.json").exists();
    let touched_node = written
        .iter()
        .any(|file| NODE_SOURCE_EXT.contains(&file.rsplit('.').next().unwrap_or("")));
    if !has_node_project || !touched_node {
        return Ok(());
    }

    for file in written {
        let ext = file.rsplit('.').next().unwrap_or("");
        if !NODE_SOURCE_EXT.contains(&ext) {
            continue;
        }
        let Ok(content) = fs_err_read(repo_root, file) else {
            continue;
        };
        for specifier in relative_imports(&content) {
            let base = file.rsplit_once('/').map(|(dir, _)| dir);
            let target = match base {
                Some(dir) => format!("{dir}/{specifier}"),
                None => specifier.clone(),
            };
            let normalized = normalize_relative(&target);
            if module_exists(repo_root, &normalized) {
                continue;
            }
            return Err(format!(
                "rejected: `{file}` imports local module `{specifier}` but that file does not \
                 exist after applying the edits — include an edit that writes it (or fix the path)"
            ));
        }
    }
    Ok(())
}

/// Extract relative specifiers from `require("./x")`, `import … from "./x"`,
/// and `import("./x")`.
fn relative_imports(content: &str) -> Vec<String> {
    let mut specifiers = Vec::new();
    let bytes = content.as_bytes();
    let mut index = 0;
    while let Some(found) = content[index..].find("./") {
        let at = index + found;
        // The character before "./" must introduce a path context (quote,
        // paren, or space) and we then read the quoted string.
        if at > 0 && !matches!(bytes[at - 1], b'\'' | b'"' | b'(' | b' ') {
            index = at + 2;
            continue;
        }
        let quote_start = if at > 0 && matches!(bytes[at - 1], b'\'' | b'"') {
            at - 1
        } else {
            // skip to the next quote after "./" (dynamic import case)
            match content[at + 2..].find(['\'', '"']) {
                Some(offset) => at + 2 + offset,
                None => {
                    index = at + 2;
                    continue;
                }
            }
        };
        let quote = bytes[quote_start];
        let Some(length) = content[quote_start + 1..].find(quote as char) else {
            break;
        };
        let specifier = content[quote_start + 1..quote_start + 1 + length].to_string();
        index = quote_start + 1 + length;
        if specifier.starts_with("./") || specifier.starts_with("../") {
            specifiers.push(specifier);
        }
    }
    specifiers.sort();
    specifiers.dedup();
    specifiers
}

fn normalize_relative(target: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in target.split('/') {
        match part {
            "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn module_exists(repo_root: &Path, module: &str) -> bool {
    let base = repo_root.join(module);
    [
        base.clone(),
        base.with_extension("js"),
        base.with_extension("mjs"),
        base.with_extension("cjs"),
        base.join("index.js"),
        base.join("index.mjs"),
    ]
    .iter()
    .any(|candidate| candidate.is_file())
}

fn fs_err_read(repo_root: &Path, relative: &str) -> Result<String, ()> {
    std::fs::read_to_string(repo_root.join(relative)).map_err(|_| ())
}

fn check_python_syntax(repo_root: &Path, written: &[String]) -> Result<(), String> {
    let py_files: Vec<&String> = written
        .iter()
        .filter(|file| file.ends_with(".py"))
        .collect();
    if py_files.is_empty() {
        return Ok(());
    }
    let mut command = std::process::Command::new("python");
    command.arg("-m").arg("py_compile");
    for file in &py_files {
        command.arg(repo_root.join(file));
    }
    command.current_dir(repo_root);
    let output = match command.output() {
        Ok(output) => output,
        // Python unavailable: the check is best-effort; the test stage will
        // surface syntax errors anyway.
        Err(_) => return Ok(()),
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first_error = stderr
        .lines()
        .find(|line| line.contains("Error") || line.contains("error"))
        .unwrap_or("syntax error");
    Err(format!(
        "rejected: edited Python file(s) do not compile — {first_error}"
    ))
}

fn tool_name(action: &str) -> &'static str {
    match action {
        "write" => tools::FILE_WRITE,
        "delete" => tools::FILE_DELETE,
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::super::plan::PlanStep;
    use super::*;
    use crate::config::Config;
    use crate::core::model::{Run, Task, new_task_id};
    use crate::core::stage::RunContext;
    use crate::providers::test_util::spawn_one_shot;
    use std::path::PathBuf;

    fn make_ctx(repo_root: &PathBuf) -> RunContext {
        let task = Task {
            id: new_task_id(),
            description: "Add expiration validation".into(),
            repo_root: repo_root.clone(),
            created_at: chrono::Utc::now(),
        };
        let run = Run::starting(task.clone(), Config::default());
        let mut ctx = RunContext::new(Config::default(), task, run);
        let analysis = TaskAnalysis {
            goal: "Reject expired JWT".into(),
            affected_components: vec!["src/auth.rs".to_string()],
            constraints: vec![],
            assumptions: vec![],
            risks: vec![],
            edge_cases: vec![],
            ambiguities: vec![],
        };
        ctx.put_artifact("task.analysis", serde_json::to_value(&analysis).unwrap());
        let plan = ImplementationPlan {
            summary: "validate exp".into(),
            steps: vec![PlanStep {
                description: "edit auth".into(),
                affected_files: vec!["src/auth.rs".to_string()],
            }],
            validation_criteria: vec![],
            rollback: "git revert".into(),
        };
        ctx.put_artifact("plan", serde_json::to_value(&plan).unwrap());
        ctx
    }

    fn mock_provider(content: String) -> Arc<dyn AgentProvider> {
        let body = serde_json::json!({
            "model": "claude-sonnet-4",
            "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 300, "completion_tokens": 120 }
        })
        .to_string();
        let (url, _rx) = spawn_one_shot(200, body.leak());
        Arc::new(crate::providers::zai::ZaiProvider::new(
            Some("test".into()),
            "glm-4.7-flash",
            url,
        ))
    }

    fn edits_json(edits: serde_json::Value) -> String {
        serde_json::json!({ "summary": "apply fix", "edits": edits }).to_string()
    }

    #[tokio::test]
    async fn coder_writes_files_and_logs_tools() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("src/auth.rs"), "old content\n").unwrap();
        let mut ctx = make_ctx(&repo.path().to_path_buf());

        let edits = serde_json::json!([
            { "path": "src/auth.rs", "action": "write", "content": "validated exp claim\n" },
            { "path": "src/extra.rs", "action": "write", "content": "pub fn helper() {}\n" }
        ]);
        let provider = mock_provider(format!("```json\n{}\n```", edits_json(edits)));
        let stage = CoderStage { provider };

        let output = stage.execute(&mut ctx).await.unwrap();
        for (key, value) in output.artifacts {
            ctx.put_artifact(key, value);
        }

        assert_eq!(
            std::fs::read_to_string(repo.path().join("src/auth.rs")).unwrap(),
            "validated exp claim\n"
        );
        assert!(repo.path().join("src/extra.rs").is_file());

        let changes: ImplementationChanges = ctx.artifact_as("implementation.changes").unwrap();
        assert_eq!(changes.written_files.len(), 2);
        assert_eq!(ctx.run.tool_executions.len(), 2);
        assert_eq!(ctx.run.tool_executions[0].tool, tools::FILE_WRITE);
        assert!(ctx.run.total_cost_usd > 0.0);
    }

    #[test]
    fn sanity_check_rejects_missing_local_module() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("package.json"), r#"{ "private": true }"#).unwrap();
        std::fs::write(
            repo.path().join("src/report.js"),
            "const x = require('./pricing.js');
",
        )
        .unwrap();
        let changes = ImplementationChanges {
            summary: "s".into(),
            written_files: vec!["src/report.js".into()],
            deleted_files: vec![],
        };
        let task = Task {
            id: "t".into(),
            description: "d".into(),
            repo_root: repo.path().to_path_buf(),
            created_at: chrono::Utc::now(),
        };
        let run = Run::starting(task.clone(), Config::default());
        let mut ctx = RunContext::new(Config::default(), task, run);
        let err = sanity_check(&mut ctx, &changes).unwrap_err();
        assert!(err.contains("pricing.js"), "got: {err}");
        assert!(ctx.artifact("edit.sanity_error").is_some());
    }

    #[test]
    fn sanity_check_passes_when_modules_exist() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("package.json"), r#"{ "private": true }"#).unwrap();
        std::fs::write(
            repo.path().join("src/report.js"),
            "const x = require('./pricing.js');
",
        )
        .unwrap();
        std::fs::write(repo.path().join("src/pricing.js"), "module.exports = {};").unwrap();
        let changes = ImplementationChanges {
            summary: "s".into(),
            written_files: vec!["src/report.js".into(), "src/pricing.js".into()],
            deleted_files: vec![],
        };
        let task = Task {
            id: "t".into(),
            description: "d".into(),
            repo_root: repo.path().to_path_buf(),
            created_at: chrono::Utc::now(),
        };
        let run = Run::starting(task.clone(), Config::default());
        let mut ctx = RunContext::new(Config::default(), task, run);
        assert!(sanity_check(&mut ctx, &changes).is_ok());
    }

    #[tokio::test]
    async fn coder_blocks_path_traversal() {
        let repo = tempfile::tempdir().unwrap();
        let mut ctx = make_ctx(&repo.path().to_path_buf());

        let edits = serde_json::json!([
            { "path": "../escaped.txt", "action": "write", "content": "evil" }
        ]);
        let provider = mock_provider(format!("```json\n{}\n```", edits_json(edits)));
        let stage = CoderStage { provider };

        let err = stage.execute(&mut ctx).await.unwrap_err();
        assert!(err.to_string().contains("rejected"), "got: {err}");
        // Nothing may be written outside the repo root.
        let parent = repo.path().parent().unwrap().join("escaped.txt");
        assert!(!parent.exists(), "traversal write must not land");
    }

    #[tokio::test]
    async fn coder_fails_when_no_edits_produced() {
        let repo = tempfile::tempdir().unwrap();
        let mut ctx = make_ctx(&repo.path().to_path_buf());
        let provider = mock_provider("{\"summary\": \"nothing\", \"edits\": []}".to_string());
        let stage = CoderStage { provider };

        let err = stage.execute(&mut ctx).await.unwrap_err();
        assert!(err.to_string().contains("no edits"));
    }
}
