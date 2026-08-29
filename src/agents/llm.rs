//! Shared LLM plumbing for agent stages: tolerant JSON extraction and a
//! small completion helper. Every agent stage funnels through here so
//! prompt/parsing behavior stays consistent.

use crate::providers::{AgentProvider, Prompt, Response};

/// Extract a JSON value from an LLM response that may be wrapped in prose
/// or markdown code fences. Tries strict parse first, then the slice
/// between the outermost braces.
pub fn extract_json(text: &str) -> Option<serde_json::Value> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(value);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]).ok()
}

/// Run one completion against a provider and return the raw response.
pub async fn complete(
    provider: &dyn AgentProvider,
    system: &str,
    user: String,
) -> Result<Response, String> {
    let prompt = Prompt::new(user).with_system(system);
    provider.run(&prompt).await.map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_json() {
        let value = extract_json(r#"{"goal": "x"}"#).unwrap();
        assert_eq!(value["goal"], "x");
    }

    #[test]
    fn extracts_json_from_markdown_fence() {
        let text = "Here is the analysis:\n```json\n{\"goal\": \"fix auth\"}\n```\nDone.";
        let value = extract_json(text).unwrap();
        assert_eq!(value["goal"], "fix auth");
    }

    #[test]
    fn extracts_json_with_surrounding_prose() {
        let text = "Sure! {\"goal\": \"a\", \"nested\": {\"b\": 1}} hope that helps";
        let value = extract_json(text).unwrap();
        assert_eq!(value["nested"]["b"], 1);
    }

    #[test]
    fn returns_none_for_garbage() {
        assert!(extract_json("no json here at all").is_none());
        assert!(extract_json("{broken").is_none());
    }
}
