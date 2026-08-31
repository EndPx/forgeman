use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use super::{AgentProvider, Prompt, ProviderError, ProviderFuture, Response, estimate_cost};
use crate::config::AgentConfig;

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// OpenAI Chat Completions provider (also works with most
/// OpenAI-compatible endpoints via `agent.base_url`).
pub struct OpenAiProvider {
    client: Client,
    api_key: Option<String>,
    model: String,
    fallback_model: Option<String>,
    upgrade: std::sync::Arc<std::sync::atomic::AtomicBool>,
    base_url: String,
}

impl OpenAiProvider {
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
            super::resolve_api_key(config, "OPENAI_API_KEY"),
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
struct ChatResponse {
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
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

impl AgentProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn run<'a>(&'a self, prompt: &'a Prompt) -> ProviderFuture<'a> {
        Box::pin(async move {
            let api_key = self.api_key.as_ref().ok_or_else(|| {
                ProviderError::MissingApiKey("openai".to_string(), "OPENAI_API_KEY".to_string())
            })?;

            let mut messages = Vec::new();
            if let Some(system) = &prompt.system {
                messages.push(json!({ "role": "system", "content": system }));
            }
            messages.push(json!({ "role": "user", "content": prompt.user }));

            let body = json!({
                "model": self.resolve_model(),
                "messages": messages,
                "max_tokens": prompt.max_tokens,
                "temperature": prompt.temperature,
            });

            let url = format!(
                "{}/v1/chat/completions",
                self.base_url.trim_end_matches('/')
            );
            let response = self
                .client
                .post(&url)
                .bearer_auth(api_key)
                .json(&body)
                .send()
                .await
                .map_err(|err| ProviderError::Request(err.to_string()))?;

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
                            .and_then(|m| m.as_str().map(str::to_string))
                    })
                    .unwrap_or_else(|| raw.chars().take(300).collect());
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message,
                });
            }

            let parsed: ChatResponse = serde_json::from_str(&raw)
                .map_err(|err| ProviderError::Invalid(format!("cannot parse response: {err}")))?;
            let text = parsed
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .unwrap_or_default();
            let cost_usd = estimate_cost(
                &parsed.model,
                parsed.usage.prompt_tokens,
                parsed.usage.completion_tokens,
            );

            Ok(Response {
                text,
                input_tokens: parsed.usage.prompt_tokens,
                output_tokens: parsed.usage.completion_tokens,
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
    use crate::providers::test_util::spawn_one_shot;

    #[tokio::test]
    async fn parses_response_with_bearer_auth() {
        let (url, rx) = spawn_one_shot(
            200,
            r#"{ "model": "gpt-4o", "choices": [{ "message": { "content": "mock answer" } }], "usage": { "prompt_tokens": 80, "completion_tokens": 40 } }"#,
        );
        let provider = OpenAiProvider::new(Some("sk-test".to_string()), "gpt-4o", url);

        let response = provider.run(&Prompt::new("hello")).await.unwrap();

        assert_eq!(response.text, "mock answer");
        assert_eq!(response.input_tokens, 80);
        assert!(response.cost_usd > 0.0);

        let request = rx.recv().unwrap();
        let headers = request.headers.to_lowercase();
        assert!(headers.contains("authorization: bearer sk-test"));
        assert!(request.body.contains("\"model\":\"gpt-4o\""));
    }

    #[tokio::test]
    async fn missing_api_key_is_a_clear_error() {
        let provider = OpenAiProvider::new(None, "gpt-4o", "http://127.0.0.1:1");
        let err = provider.run(&Prompt::new("hi")).await.unwrap_err();
        assert!(err.to_string().contains("OPENAI_API_KEY"), "got: {err}");
    }
}
