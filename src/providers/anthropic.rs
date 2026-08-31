use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use super::{AgentProvider, Prompt, ProviderError, ProviderFuture, Response, estimate_cost};
use crate::config::AgentConfig;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: Client,
    api_key: Option<String>,
    model: String,
    fallback_model: Option<String>,
    upgrade: std::sync::Arc<std::sync::atomic::AtomicBool>,
    base_url: String,
}

impl AnthropicProvider {
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

    /// Tiered fallback (spec §13): `upgrade` switches to `fallback_model`.
    pub fn with_tiering(
        mut self,
        fallback_model: Option<String>,
        upgrade: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.fallback_model = fallback_model;
        self.upgrade = upgrade;
        self
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

    pub fn from_config(
        config: &AgentConfig,
        upgrade: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self::new(
            super::resolve_api_key(config, "ANTHROPIC_API_KEY"),
            config.model.clone(),
            config
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        )
        .with_tiering(config.fallback_model.clone(), upgrade)
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    model: String,
    content: Vec<ContentBlock>,
    usage: Usage,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

impl AgentProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn run<'a>(&'a self, prompt: &'a Prompt) -> ProviderFuture<'a> {
        Box::pin(async move {
            let api_key = self.api_key.as_ref().ok_or_else(|| {
                ProviderError::MissingApiKey(
                    "anthropic".to_string(),
                    "ANTHROPIC_API_KEY".to_string(),
                )
            })?;

            let mut messages = json!({
                "model": self.resolve_model(),
                "max_tokens": prompt.max_tokens,
                "messages": [{ "role": "user", "content": prompt.user }],
            });
            if let Some(system) = &prompt.system {
                messages["system"] = json!(system);
            }
            if let Some(temperature) = prompt.temperature {
                messages["temperature"] = json!(temperature);
            }

            let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
            let response = self
                .client
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", API_VERSION)
                .json(&messages)
                .send()
                .await
                .map_err(|err| ProviderError::Request(err.to_string()))?;

            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|err| ProviderError::Request(err.to_string()))?;
            if !status.is_success() {
                let message = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .and_then(|m| m.as_str().map(str::to_string))
                    })
                    .unwrap_or_else(|| body.chars().take(300).collect());
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message,
                });
            }

            let parsed: MessagesResponse = serde_json::from_str(&body)
                .map_err(|err| ProviderError::Invalid(format!("cannot parse response: {err}")))?;
            let text = parsed
                .content
                .into_iter()
                .filter_map(|block| block.text)
                .collect::<Vec<_>>()
                .join("");
            let cost_usd = estimate_cost(
                &parsed.model,
                parsed.usage.input_tokens,
                parsed.usage.output_tokens,
            );

            Ok(Response {
                text,
                input_tokens: parsed.usage.input_tokens,
                output_tokens: parsed.usage.output_tokens,
                model: parsed.model,
                cost_usd,
                finish_reason: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::test_util::{CapturedRequest, spawn_one_shot};
    use std::sync::mpsc::Receiver;

    const MOCK_BODY: &str = r#"{
        "model": "claude-sonnet-4",
        "content": [{ "type": "text", "text": "Hello from mock" }],
        "usage": { "input_tokens": 100, "output_tokens": 50 }
    }"#;

    fn setup() -> (AnthropicProvider, Receiver<CapturedRequest>) {
        let (url, rx) = spawn_one_shot(200, MOCK_BODY);
        let provider = AnthropicProvider::new(Some("test-key".to_string()), "claude-sonnet-4", url);
        (provider, rx)
    }

    #[tokio::test]
    async fn parses_response_and_estimates_cost() {
        let (provider, rx) = setup();
        let response = provider.run(&Prompt::new("hi")).await.unwrap();

        assert_eq!(response.text, "Hello from mock");
        assert_eq!(response.input_tokens, 100);
        assert_eq!(response.output_tokens, 50);
        assert!(response.cost_usd > 0.0, "sonnet pricing must estimate");

        let request = rx.recv().unwrap();
        assert!(request.headers.contains("x-api-key: test-key"));
        assert!(request.headers.contains("anthropic-version: 2023-06-01"));
        assert!(request.body.contains("\"model\":\"claude-sonnet-4\""));
        assert!(request.body.contains("\"content\":\"hi\""));
    }

    #[tokio::test]
    async fn missing_api_key_is_a_clear_error() {
        let provider = AnthropicProvider::new(None, "claude-sonnet-4", "http://127.0.0.1:1");
        let err = provider.run(&Prompt::new("hi")).await.unwrap_err();
        assert!(err.to_string().contains("ANTHROPIC_API_KEY"), "got: {err}");
    }

    #[tokio::test]
    async fn api_error_surfaces_provider_message() {
        let (url, _rx) = spawn_one_shot(401, r#"{ "error": { "message": "invalid x-api-key" } }"#);
        let provider = AnthropicProvider::new(Some("bad".into()), "claude-sonnet-4", url);
        let err = provider.run(&Prompt::new("hi")).await.unwrap_err();
        match err {
            ProviderError::Api { status, message } => {
                assert_eq!(status, 401);
                assert!(message.contains("invalid x-api-key"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }
}
