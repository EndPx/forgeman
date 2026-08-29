use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The repository intelligence profile (spec §7). Produced by the inspector
/// and published as the `repository.profile` artifact for downstream stages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryProfile {
    pub root: PathBuf,
    pub primary_language: String,
    pub languages: Vec<LanguageShare>,
    pub framework: Option<String>,
    pub package_manager: Option<String>,
    pub entrypoints: Vec<String>,
    pub test_frameworks: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub config_files: Vec<String>,
    pub databases: Vec<String>,
    pub external_services: Vec<String>,
    pub risky_areas: Vec<RiskArea>,
    pub file_count: usize,
    /// Bounded tree for the internal repository map (depth- and size-limited).
    pub tree: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageShare {
    pub language: String,
    pub files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskArea {
    pub path: String,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub is_dir: bool,
}

impl RepositoryProfile {
    /// Compact human summary used in stage detail and downstream prompts.
    #[allow(dead_code)] // consumed by analyzer/planner stages in Phase 4
    pub fn summary(&self) -> String {
        [
            format!("language: {}", self.primary_language),
            format!(
                "framework: {}",
                self.framework.as_deref().unwrap_or("none detected")
            ),
            format!(
                "package manager: {}",
                self.package_manager.as_deref().unwrap_or("none detected")
            ),
            format!(
                "entrypoints: {}",
                join_or(&self.entrypoints, "none detected")
            ),
            format!("tests: {}", join_or(&self.test_frameworks, "none detected")),
            format!("databases: {}", join_or(&self.databases, "none detected")),
            format!(
                "external services: {}",
                join_or(&self.external_services, "none detected")
            ),
            format!("files: {}", self.file_count),
            format!("risk areas: {}", self.risky_areas.len()),
        ]
        .join("\n")
    }
}

fn join_or(items: &[String], fallback: &str) -> String {
    if items.is_empty() {
        fallback.to_string()
    } else {
        items.join(", ")
    }
}
