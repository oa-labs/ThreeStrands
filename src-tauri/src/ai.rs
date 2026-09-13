use keyring::Entry;
use serde::Deserialize;
use serde_json::json;

const SERVICE: &str = "app.dispatch.mail";
const KEY: &str = "ai-provider-api-key";

pub fn configured() -> bool {
    entry()
        .and_then(|entry| entry.get_password().map_err(display))
        .is_ok()
}

pub fn set(key: &str) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return clear();
    }
    entry()?.set_password(trimmed).map_err(display)
}

/// The actual secret, for use in an outgoing request. `None` when no key has
/// been saved (distinct from an I/O error talking to the keychain).
pub fn get_key() -> Result<Option<String>, String> {
    match entry()?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(display(error)),
    }
}

fn clear() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn entry() -> Result<Entry, String> {
    Entry::new(SERVICE, KEY).map_err(display)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// One thread message reduced to what the summarizer needs, oldest first.
pub struct ThreadMessageInput {
    pub sender: String,
    pub sent_at: String,
    pub body_text: String,
}

pub struct SummarizeRequest {
    pub provider: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub subject: String,
    pub messages: Vec<ThreadMessageInput>,
}

/// Bounds the prompt to a handful of recent messages, and each message to a
/// reasonable length, so a long thread doesn't blow past a provider's context
/// window or run up an outsized bill for a shortcut meant to save a click.
const MAX_MESSAGES: usize = 15;
const MAX_BODY_CHARS: usize = 6000;

const SYSTEM_PROMPT: &str = "You summarize email threads for a mail client. Reply with 2 to 5 short plain-text bullet lines capturing the key facts, decisions, and any action items. Each line must start with \"- \". Do not use markdown formatting, headings, or a preamble - output only the bullet lines.";

pub async fn summarize(request: SummarizeRequest, api_key: &str) -> Result<String, String> {
    let prompt = build_prompt(&request.subject, &request.messages);
    let content = match request.provider.as_str() {
        "anthropic" => call_anthropic(&request.model, &prompt, api_key).await?,
        "openai" | "openrouter" | "fireworks" | "custom" => {
            let base = base_url(&request.provider, request.endpoint.as_deref())?;
            call_openai_compatible(&base, &request.model, &prompt, api_key).await?
        }
        other => return Err(format!("Unknown AI provider: {other}")),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("The AI provider returned an empty summary".to_string());
    }
    Ok(trimmed.to_string())
}

fn build_prompt(subject: &str, messages: &[ThreadMessageInput]) -> String {
    let mut out = format!("Subject: {subject}\n\n");
    let start = messages.len().saturating_sub(MAX_MESSAGES);
    for message in &messages[start..] {
        let body: String = message.body_text.chars().take(MAX_BODY_CHARS).collect();
        out.push_str(&format!(
            "From: {}\nDate: {}\n{}\n\n---\n\n",
            message.sender,
            message.sent_at,
            body.trim()
        ));
    }
    out
}

fn base_url(provider: &str, endpoint: Option<&str>) -> Result<String, String> {
    match provider {
        "openai" => Ok("https://api.openai.com/v1".to_string()),
        "openrouter" => Ok("https://openrouter.ai/api/v1".to_string()),
        "fireworks" => Ok("https://api.fireworks.ai/inference/v1".to_string()),
        "custom" => endpoint
            .map(|value| value.trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Set an endpoint URL in AI settings".to_string()),
        other => Err(format!("Unknown AI provider: {other}")),
    }
}

async fn call_openai_compatible(
    base_url: &str,
    model: &str,
    prompt: &str,
    api_key: &str,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "temperature": 0.2,
        "max_tokens": 300,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": prompt},
        ],
    });
    let response = reqwest::Client::new()
        .post(format!("{base_url}/chat/completions"))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(display)?;
    let response = checked(response).await?;
    let text = response.text().await.map_err(display)?;
    parse_openai_content(&text)
}

async fn call_anthropic(model: &str, prompt: &str, api_key: &str) -> Result<String, String> {
    let body = json!({
        "model": model,
        "max_tokens": 300,
        "system": SYSTEM_PROMPT,
        "messages": [{"role": "user", "content": prompt}],
    });
    let response = reqwest::Client::new()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(display)?;
    let response = checked(response).await?;
    let text = response.text().await.map_err(display)?;
    parse_anthropic_content(&text)
}

fn parse_openai_content(body: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct ChatResponse {
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: ChoiceMessage,
    }
    #[derive(Deserialize)]
    struct ChoiceMessage {
        content: String,
    }
    let parsed: ChatResponse = serde_json::from_str(body).map_err(display)?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .ok_or_else(|| "AI provider returned no choices".to_string())
}

fn parse_anthropic_content(body: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct MessagesResponse {
        content: Vec<ContentBlock>,
    }
    #[derive(Deserialize)]
    struct ContentBlock {
        #[serde(default)]
        text: Option<String>,
    }
    let parsed: MessagesResponse = serde_json::from_str(body).map_err(display)?;
    parsed
        .content
        .into_iter()
        .find_map(|block| block.text)
        .ok_or_else(|| "AI provider returned no content".to_string())
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response, String> {
    if response.status().is_success() {
        Ok(response)
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let truncated: String = body.chars().take(300).collect();
        Err(format!("AI provider returned {status}: {truncated}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_resolves_known_providers() {
        assert_eq!(base_url("openai", None).unwrap(), "https://api.openai.com/v1");
        assert_eq!(
            base_url("openrouter", None).unwrap(),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            base_url("fireworks", None).unwrap(),
            "https://api.fireworks.ai/inference/v1"
        );
    }

    #[test]
    fn base_url_uses_endpoint_for_custom_provider() {
        assert_eq!(
            base_url("custom", Some("https://example.com/v1/")).unwrap(),
            "https://example.com/v1"
        );
        assert!(base_url("custom", None).is_err());
        assert!(base_url("custom", Some("")).is_err());
    }

    #[test]
    fn parses_openai_style_response() {
        let body = r#"{"choices":[{"message":{"content":"- Point one\n- Point two"}}]}"#;
        assert_eq!(parse_openai_content(body).unwrap(), "- Point one\n- Point two");
    }

    #[test]
    fn parses_anthropic_style_response() {
        let body = r#"{"content":[{"type":"text","text":"- Point one"}]}"#;
        assert_eq!(parse_anthropic_content(body).unwrap(), "- Point one");
    }

    #[test]
    fn build_prompt_caps_message_count_and_length() {
        let messages: Vec<ThreadMessageInput> = (0..20)
            .map(|index| ThreadMessageInput {
                sender: format!("sender{index}@example.com"),
                sent_at: "2026-01-01T00:00:00Z".to_string(),
                body_text: "x".repeat(MAX_BODY_CHARS + 500),
            })
            .collect();
        let prompt = build_prompt("Subject line", &messages);
        assert!(prompt.contains("sender19@example.com"));
        assert!(!prompt.contains("sender0@example.com"));
        assert!(prompt.matches("sender").count() == MAX_MESSAGES);
    }
}
