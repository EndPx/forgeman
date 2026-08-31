//! Z.AI provider (GLM models, OpenAI-compatible chat completions API).
//! Default model: glm-4.7-flash (free tier).

use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use super::{AgentProvider, Prompt, ProviderError, ProviderFuture, Response, estimate_cost};
use crate::config::AgentConfig;

const DEFAULT_BASE_URL: &str = "https://api.z.ai/api/paas/v4";

pub struct ZaiProvider {
    client: Client,
    api_key: Option<String>,
    model: String,
    /// Stronger model the provider switches to once the run signals an
    /// upgrade (tiered fallback, spec §13). None = no tiering.
    fallback_model: Option<String>,
    upgrade: std::sync::Arc<std::sync::atomic::AtomicBool>,
    base_url: String,
}

impl ZaiProvider {
    pub fn new(
        api_key: Option<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("reqwest client builds"),
            api_key,
            model: model.into(),
            fallback_model: None,
            upgrade: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            base_url: base_url.into(),
        }
    }

    /// Enable tiered fallback: when `upgrade` is set, calls use
    /// `fallback_model` instead of the primary model.
    pub fn with_tiering(
        mut self,
        fallback_model: Option<String>,
        upgrade: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.fallback_model = fallback_model;
        self.upgrade = upgrade;
        self
    }

    pub fn from_config(
        config: &AgentConfig,
        upgrade: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let api_key = super::resolve_api_key(config, "ZAI_API_KEY")
            .or_else(|| std::env::var("Z_AI_API_KEY").ok());
        Self::new(
            api_key,
            config.model.clone(),
            config
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        )
        .with_tiering(config.fallback_model.clone(), upgrade)
    }

    fn resolve_model(&self) -> String {
        let upgraded = self.upgrade.load(std::sync::atomic::Ordering::Relaxed);
        if upgraded {
            self.fallback_model
                .clone()
                .unwrap_or_else(|| self.model.clone())
        } else {
            self.model.clone()
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    model: String,
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}
#[derive(Deserialize, Clone)]
struct Choice {
    #[serde(default)]
    message: Option<Message>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Message {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

impl AgentProvider for ZaiProvider {
    fn name(&self) -> &'static str {
        "zai"
    }

    fn run<'a>(&'a self, prompt: &'a Prompt) -> ProviderFuture<'a> {
        Box::pin(async move {
            let api_key = self.api_key.as_ref().ok_or_else(|| {
                ProviderError::MissingApiKey("zai".to_string(), "ZAI_API_KEY".to_string())
            })?;

            let mut messages = Vec::new();
            if let Some(system) = &prompt.system {
                messages.push(json!({ "role": "system", "content": system }));
            }
            messages.push(json!({ "role": "user", "content": prompt.user }));

            let mut body = json!({
                "model": self.model,
                "messages": messages,
                "max_tokens": prompt.max_tokens,
                "stream": false,
            });
            // GLM reasoning models: skip chain-of-thought when asked, so the
            // token budget lands on the actual answer.
            if prompt.thinking_disabled {
                body["thinking"] = json!({ "type": "disabled" });
            }

            let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
            // Free tier regularly returns 429 (overloaded). Retry with a
            // bounded exponential backoff instead of burning an orchestrator
            // attempt — 5s/15s/30s gives the quota time to recover.
            let mut attempt = 0u32;
            let response = loop {
                attempt += 1;
                let response = self
                    .client
                    .post(&url)
                    .bearer_auth(api_key)
                    .header("accept", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|err| ProviderError::Request(err.to_string()))?;
                if response.status().as_u16() == 429 && attempt < 3 {
                    tokio::time::sleep(std::time::Duration::from_secs(2 * u64::from(attempt)))
                        .await;
                    continue;
                }
                break response;
            };

            let status = response.status();
            let raw = response
                .text()
                .await
                .map_err(|err| ProviderError::Request(err.to_string()))?;
            if !status.is_success() {
                let message = serde_json::from_str::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .or_else(|| v.pointer("/error"))
                            .or_else(|| v.get("message"))
                            .map(|m| m.to_string())
                    })
                    .unwrap_or_else(|| raw.chars().take(300).collect());
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message,
                });
            }

            let parsed: ChatResponse = serde_json::from_str(&raw)
                .map_err(|err| ProviderError::Invalid(format!("cannot parse response: {err}")))?;

            let first = parsed.choices.first().cloned();
            let text = first
                .as_ref()
                .and_then(|c| {
                    c.message
                        .as_ref()
                        .and_then(|m| m.content.clone())
                        .or_else(|| c.text.clone())
                })
                .unwrap_or_default();
            let finish_reason = first.as_ref().and_then(|c| c.finish_reason.clone());

            // GLM flash models spend completion tokens on reasoning first;
            // an empty answer with finish_reason=length means the reasoning
            // consumed the entire budget before any content was emitted.
            if text.is_empty() && finish_reason.as_deref() == Some("length") {
                return Err(ProviderError::Invalid(
                    "model exhausted max_tokens on reasoning before producing content — \
                     increase max_tokens"
                        .to_string(),
                ));
            }

            let (input_tokens, output_tokens) = parsed
                .usage
                .as_ref()
                .map(|u| (u.prompt_tokens, u.completion_tokens))
                .unwrap_or((0, 0));
            // GLM flash-tier models are free; unknown models track as $0.
            let cost_usd = estimate_cost(&parsed.model, input_tokens, output_tokens);

            Ok(Response {
                text,
                input_tokens,
                output_tokens,
                model: parsed.model,
                cost_usd,
                finish_reason,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::test_util::spawn_one_shot;

    #[tokio::test]
    async fn parses_response_with_bearer_auth() {
        let (url, rx) = spawn_one_shot(
            200,
            r#"{ "model": "glm-4.7-flash", "choices": [{ "message": { "content": "halo dari mock" } }], "usage": { "prompt_tokens": 20, "completion_tokens": 10 } }"#,
        );
        let provider = ZaiProvider::new(Some("test-key".into()), "glm-4.7-flash", url);

        let response = provider.run(&Prompt::new("ping")).await.unwrap();

        assert_eq!(response.text, "halo dari mock");
        assert_eq!(response.model, "glm-4.7-flash");

        let request = rx.recv().unwrap();
        let headers = request.headers.to_lowercase();
        assert!(headers.contains("authorization: bearer test-key"));
        assert!(request.body.contains("\"model\":\"glm-4.7-flash\""));
        assert!(request.body.contains("\"stream\":false"));
    }

    #[tokio::test]
    async fn missing_api_key_is_a_clear_error() {
        let provider = ZaiProvider::new(None, "glm-4.7-flash", "http://127.0.0.1:1");
        let err = provider.run(&Prompt::new("hi")).await.unwrap_err();
        assert!(err.to_string().contains("ZAI_API_KEY"), "got: {err}");
    }

    /// Live smoke test against the real Z.AI API.
    /// Run locally with: cargo test zai_live_ping -- --ignored --nocapture
    /// Requires ZAI_API_KEY in the environment or .env. Never runs in CI.
    #[tokio::test]
    #[ignore = "requires live ZAI_API_KEY — run manually, never in CI"]
    async fn zai_live_ping() {
        crate::env::load_dotenv(std::path::Path::new(".env"));
        let config = AgentConfig::default();
        let provider = ZaiProvider::from_config(
            &config,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let response = provider
            .run(&Prompt::new("Reply with exactly: OK").with_max_tokens(1024))
            .await
            .expect("live Z.AI call should succeed");
        println!("model: {}", response.model);
        println!("finish_reason: {:?}", response.finish_reason);
        println!(
            "tokens: {} in / {} out",
            response.input_tokens, response.output_tokens
        );
        println!("reply: {}", response.text);
        assert!(!response.text.is_empty());
    }
}
