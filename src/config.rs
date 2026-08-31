use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const FORGEMAN_DIR: &str = ".forgeman";
pub const CONFIG_RELATIVE_PATH: &str = ".forgeman/config.toml";

/// Default configuration per the ForgeMan specification:
/// max 5 iterations, 20 minute timeout, $5 budget.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Model provider backing the coding agent: `anthropic` | `openai`.
    pub provider: String,
    pub model: String,
    /// Override the provider API base URL (self-hosted / OpenAI-compatible
    /// endpoints). Defaults to the vendor's public API.
    pub base_url: Option<String>,
    /// Optional stronger model the coder/improver upgrades to when the first
    /// iteration fails to verify (model tiering, spec §13).
    pub fallback_model: Option<String>,
    /// Name of the environment variable holding the API key — lets any
    /// OpenAI-compatible endpoint (OpenRouter, Groq, DeepSeek, …) be used
    /// without code changes. Defaults are per provider (ZAI_API_KEY, …).
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionConfig {
    pub max_iterations: u32,
    pub timeout_minutes: u64,
    /// Attempts per stage before ForgeMan escalates instead of retrying forever.
    pub max_stage_attempts: u32,
    /// Cooldown (seconds) before re-running a stage round that failed purely
    /// on provider rate limits. Infra pauses do not consume attempts.
    pub infra_pause_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    pub enabled: bool,
    /// `restricted` (no network) | `open` (bridge network in Docker).
    pub network: String,
    /// Container image override. Defaults are chosen per ecosystem
    /// (rust:1, node:22-alpine, python:3.12-slim).
    pub image: Option<String>,
    /// Docker memory limit (e.g. "1g").
    pub memory: String,
    /// Docker CPU limit.
    pub cpus: f64,
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

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "unnamed".to_string(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            // Z.AI GLM flash tier — free, used by default for this project.
            provider: "zai".to_string(),
            model: "glm-4.7-flash".to_string(),
            base_url: None,
            // Optional stronger model the coder/improver upgrades to when the
            // first iteration fails to verify (tiered fallback, spec §13).
            fallback_model: None,
            api_key_env: None,
        }
    }
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            timeout_minutes: 20,
            max_stage_attempts: 3,
            infra_pause_seconds: 90,
        }
    }
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            network: "restricted".to_string(),
            image: None,
            memory: "1g".to_string(),
            cpus: 1.0,
        }
    }
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self {
            tests: true,
            performance: false,
            security: false,
        }
    }
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self { max_cost_usd: 5.0 }
    }
}

impl AgentConfig {
    /// Environment overrides (set via `.env` or the shell) win over the
    /// config file so one `.env` can re-point ForgeMan at any endpoint.
    pub fn apply_env_overrides(&mut self, get: &dyn Fn(&str) -> Option<String>) {
        if let Some(value) = get("FORGEMAN_PROVIDER") {
            self.provider = value;
        }
        if let Some(value) = get("FORGEMAN_MODEL") {
            self.model = value;
        }
        if let Some(value) = get("FORGEMAN_FALLBACK_MODEL") {
            self.fallback_model = Some(value);
        }
        if let Some(value) = get("FORGEMAN_BASE_URL") {
            self.base_url = Some(value);
        }
        if let Some(value) = get("FORGEMAN_API_KEY_ENV") {
            self.api_key_env = Some(value);
        }
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
            let mut config = Self::default();
            config
                .agent
                .apply_env_overrides(&|key| std::env::var(key).ok());
            return Ok(config);
        }

        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let mut config: Self = toml::from_str(&raw)
            .with_context(|| format!("invalid config file {}", path.display()))?;
        config
            .agent
            .apply_env_overrides(&|key| std::env::var(key).ok());
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
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&path, render_default_toml())
            .with_context(|| format!("failed to write {}", path.display()))?;
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
        assert_eq!(config.agent.provider, "zai");
        assert_eq!(config.agent.model, "glm-4.7-flash");
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
    fn env_overrides_win_over_config() {
        let mut config = Config::default();
        config.agent.provider = "openai".to_string();
        config.agent.model = "gpt-4o".to_string();
        let values = [
            ("FORGEMAN_PROVIDER", "zai"),
            ("FORGEMAN_MODEL", "glm-4.7-flash"),
            ("FORGEMAN_BASE_URL", "https://api.example.test/v4"),
            ("FORGEMAN_API_KEY_ENV", "OPENROUTER_API_KEY"),
        ];
        let get = |key: &str| -> Option<String> {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        };
        config.agent.apply_env_overrides(&get);
        assert_eq!(config.agent.provider, "zai");
        assert_eq!(config.agent.model, "glm-4.7-flash");
        assert_eq!(
            config.agent.base_url.as_deref(),
            Some("https://api.example.test/v4")
        );
        assert_eq!(
            config.agent.api_key_env.as_deref(),
            Some("OPENROUTER_API_KEY")
        );
        // Unset vars leave the config value untouched.
        assert!(config.agent.fallback_model.is_none());
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
