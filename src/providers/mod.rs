//! LLM provider abstraction (spec §12/13/35). The core never calls a
//! specific vendor — it calls `AgentProvider.run()` through the router.

// Framework surface consumed by stage implementations landing in Phases 4–8.
#![allow(dead_code)]

pub mod anthropic;
pub mod openai;
pub mod router;
#[cfg(test)]
pub mod test_util;
pub mod zai;

use std::future::Future;
use std::pin::Pin;

use crate::config::AgentConfig;

pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Response, ProviderError>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: Option<String>,
    pub user: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    /// Ask reasoning-capable models (GLM) to skip chain-of-thought so the
    /// whole budget goes to the answer. Used by edit-heavy JSON stages.
    pub thinking_disabled: bool,
}

impl Prompt {
    pub fn new(user: impl Into<String>) -> Self {
        Self {
            system: None,
            user: user.into(),
            max_tokens: 4096,
            temperature: None,
            thinking_disabled: false,
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn without_thinking(mut self) -> Self {
        self.thinking_disabled = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: String,
    pub cost_usd: f64,
    /// e.g. `stop` | `length` | `tool_calls` — useful for diagnosing
    /// truncated reasoning models.
    pub finish_reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("missing API key for provider `{0}` — set the {1} environment variable")]
    MissingApiKey(String, String),
    #[error("provider request failed: {0}")]
    Request(String),
    #[error("provider response invalid: {0}")]
    Invalid(String),
    #[error("provider returned error {status}: {message}")]
    Api { status: u16, message: String },
}

/// Pluggable agent provider (spec §35). Implementations: Anthropic, OpenAI.
pub trait AgentProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn run<'a>(&'a self, prompt: &'a Prompt) -> ProviderFuture<'a>;
}

/// Rough token pricing (USD per 1M tokens) for cost budgeting.
/// Unknown models cost $0 in tracking — never blocks a run, only undercounts.
pub fn estimate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let (in_rate, out_rate): (f64, f64) = if model.contains("claude-opus") {
        (15.0, 75.0)
    } else if model.contains("claude-sonnet") {
        (3.0, 15.0)
    } else if model.contains("claude-haiku") {
        (0.8, 4.0)
    } else if model.contains("gpt-4o-mini") || model.contains("gpt-4.1-mini") {
        (0.15, 0.6)
    } else if model.contains("gpt-4o") || model.contains("gpt-4.1") {
        (2.5, 10.0)
    } else {
        (0.0, 0.0)
    };
    input_tokens as f64 / 1e6 * in_rate + output_tokens as f64 / 1e6 * out_rate
}

/// Build the provider selected by configuration. Never hardcodes a vendor
/// into the core — the config decides.
pub fn build(config: &AgentConfig) -> Result<Box<dyn AgentProvider>, anyhow::Error> {
    match config.provider.as_str() {
        "zai" | "z-ai" | "zhipu" => Ok(Box::new(zai::ZaiProvider::from_config(config))),
        "anthropic" => Ok(Box::new(anthropic::AnthropicProvider::from_config(config))),
        "openai" => Ok(Box::new(openai::OpenAiProvider::from_config(config))),
        other => {
            anyhow::bail!("unknown agent.provider `{other}` — supported: zai, anthropic, openai")
        }
    }
}
