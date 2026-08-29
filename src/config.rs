use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const FORGEMAN_DIR: &str = ".forgeman";
pub const CONFIG_RELATIVE_PATH: &str = ".forgeman/config.toml";

/// Default configuration per the ForgeMan specification:
/// max 5 iterations, 20 minute timeout, $5 budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub project: ProjectConfig,
    pub agent: AgentConfig,
    pub execution: ExecutionConfig,
    pub sandbox: SandboxConfig,
    pub evaluation: EvaluationConfig,
    pub budget: BudgetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// Model provider backing the coding agent (pluggable from Phase 3).
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionConfig {
    pub max_iterations: u32,
    pub timeout_minutes: u64,
    /// Attempts per stage before ForgeMan escalates instead of retrying forever.
    pub max_stage_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    pub enabled: bool,
    /// `restricted` | `open` (Docker-backed sandbox arrives in Phase 10).
    pub network: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvaluationConfig {
    pub tests: bool,
    pub performance: bool,
    pub security: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetConfig {
    pub max_cost_usd: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project: ProjectConfig::default(),
            agent: AgentConfig::default(),
            execution: ExecutionConfig::default(),
            sandbox: SandboxConfig::default(),
            evaluation: EvaluationConfig::default(),
            budget: BudgetConfig::default(),
        }
    }
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self { name: "unnamed".to_string() }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self { provider: "anthropic".to_string(), model: "claude".to_string() }
    }
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self { max_iterations: 5, timeout_minutes: 20, max_stage_attempts: 3 }
    }
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self { enabled: false, network: "restricted".to_string() }
    }
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self { tests: true, performance: false, security: false }
    }
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self { max_cost_usd: 5.0 }
    }
}

impl Config {
    /// Load configuration for the target repository.
    ///
    /// Resolution order:
    /// 1. explicitly provided config file (must exist)
    /// 2. `<repo>/.forgeman/config.toml` when present
    /// 3. built-in defaults
    pub fn load(explicit: Option<&Path>, repo_root: &Path) -> Result<Self> {
        let path = match explicit {
            Some(p) => p.to_path_buf(),
            None => repo_root.join(CONFIG_RELATIVE_PATH),
        };

        if explicit.is_none() && !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!("failed to read config file {}", path.display())
        })?;
        let config: Self = toml::from_str(&raw)
            .with_context(|| format!("invalid config file {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Write the default configuration to `<repo>/.forgeman/config.toml`.
    /// Refuses to overwrite an existing config so user edits survive.
    pub fn scaffold(repo_root: &Path) -> Result<PathBuf> {
        let path = repo_root.join(CONFIG_RELATIVE_PATH);
        if path.exists() {
            anyhow::bail!(
                "config already exists at {} — edit it instead of re-running init",
                path.display()
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create {}", parent.display())
            })?;
        }
        std::fs::write(&path, render_default_toml()).with_context(|| {
            format!("failed to write {}", path.display())
        })?;
        Ok(path)
    }

    fn validate(&self) -> Result<()> {
        if self.execution.max_iterations == 0 {
            anyhow::bail!("execution.max_iterations must be at least 1");
        }
        if self.execution.max_stage_attempts == 0 {
            anyhow::bail!("execution.max_stage_attempts must be at least 1");
        }
        if self.budget.max_cost_usd < 0.0 {
            anyhow::bail!("budget.max_cost_usd cannot be negative");
        }
        Ok(())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.execution.timeout_minutes * 60)
    }
}

fn render_default_toml() -> String {
    let config = Config::default();
    toml::to_string_pretty(&config).expect("default config serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let config = Config::default();
        assert_eq!(config.execution.max_iterations, 5);
        assert_eq!(config.execution.timeout_minutes, 20);
        assert_eq!(config.execution.max_stage_attempts, 3);
        assert_eq!(config.budget.max_cost_usd, 5.0);
        assert!(config.evaluation.tests);
        assert!(!config.evaluation.performance);
        assert!(!config.evaluation.security);
        assert_eq!(config.agent.provider, "anthropic");
        assert_eq!(config.sandbox.network, "restricted");
        assert_eq!(config.timeout(), Duration::from_secs(20 * 60));
    }

    #[test]
    fn scaffold_writes_and_loads_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Config::scaffold(tmp.path()).unwrap();
        assert!(path.exists());

        // Loading from the scaffolded file must produce spec defaults.
        let config = Config::load(None, tmp.path()).unwrap();
        assert_eq!(config.execution.max_iterations, 5);

        // Scaffolding again must refuse to clobber user edits.
        assert!(Config::scaffold(tmp.path()).is_err());
    }

    #[test]
    fn partial_config_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("custom.toml");
        std::fs::write(&path, "[execution]\nmax_iterations = 2\n").unwrap();

        let config = Config::load(Some(&path), tmp.path()).unwrap();
        assert_eq!(config.execution.max_iterations, 2);
        assert_eq!(config.execution.timeout_minutes, 20);
        assert_eq!(config.budget.max_cost_usd, 5.0);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.toml");
        std::fs::write(&path, "[execution]\nmax_iterationsx = 2\n").unwrap();
        assert!(Config::load(Some(&path), tmp.path()).is_err());
    }

    #[test]
    fn zero_iterations_is_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.toml");
        std::fs::write(&path, "[execution]\nmax_iterations = 0\n").unwrap();
        assert!(Config::load(Some(&path), tmp.path()).is_err());
    }
}
