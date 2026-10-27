//! Records what each successful provider request cost and what it was for,
//! for the AI activity log.

use serde::Deserialize;

use std::sync::{Arc, RwLock};

use super::provider::{AiProvider, ApiProtocol, ProviderRequest};

/// One provider request that returned a successful response, whether or not
/// its content later proved usable. Tokens are zero when the provider did not
/// report usage; `cost_usd` is set only when the provider reported a price.
#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub provider: AiProvider,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    /// What the request was for, when the caller said; see `with_purpose`.
    pub purpose: Option<RequestPurpose>,
}

/// What a provider request was for, as the AI activity log shows it: a
/// feature id plus, for mail, the subject and how many messages were sent.
/// Never email text.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestPurpose {
    pub feature: &'static str,
    pub subject: Option<String>,
    pub message_count: Option<usize>,
}

impl RequestPurpose {
    pub fn new(feature: &'static str) -> Self {
        Self { feature, subject: None, message_count: None }
    }

    pub fn about(feature: &'static str, subject: &str, message_count: Option<usize>) -> Self {
        Self { feature, subject: Some(subject.to_string()), message_count }
    }
}

tokio::task_local! {
    static REQUEST_PURPOSE: RequestPurpose;
}

/// Runs `future` with every provider request it makes tagged with `purpose`.
pub async fn with_purpose<F: std::future::Future>(purpose: RequestPurpose, future: F) -> F::Output {
    REQUEST_PURPOSE.scope(purpose, future).await
}

pub type UsageRecorder = Arc<dyn Fn(&UsageEvent) + Send + Sync>;

static USAGE_RECORDER: RwLock<Option<UsageRecorder>> = RwLock::new(None);

/// Installs the sink that persists provider usage. The app sets this once at
/// startup; until then usage is not recorded.
pub fn set_usage_recorder(recorder: UsageRecorder) {
    if let Ok(mut slot) = USAGE_RECORDER.write() {
        *slot = Some(recorder);
    }
}

pub(super) fn record_usage(request: &ProviderRequest<'_>, body: &str) {
    let (input_tokens, output_tokens, cost_usd) =
        parse_usage(request.provider.descriptor().protocol, body);
    let recorder = USAGE_RECORDER.read().ok().and_then(|slot| slot.clone());
    if let Some(recorder) = recorder {
        recorder(&UsageEvent {
            provider: request.provider,
            model: request.model.to_string(),
            input_tokens,
            output_tokens,
            cost_usd,
            purpose: REQUEST_PURPOSE.try_with(Clone::clone).ok(),
        });
    }
}

/// Reads token counts, and a reported cost when present, from a successful
/// response body. Missing or malformed usage reads as zero tokens.
fn parse_usage(protocol: ApiProtocol, body: &str) -> (u64, u64, Option<f64>) {
    #[derive(Deserialize, Default)]
    struct Envelope {
        #[serde(default)]
        usage: Option<Usage>,
    }
    #[derive(Deserialize, Default)]
    struct Usage {
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cost: Option<f64>,
    }
    let usage = serde_json::from_str::<Envelope>(body)
        .ok()
        .and_then(|envelope| envelope.usage)
        .unwrap_or_default();
    match protocol {
        ApiProtocol::Anthropic => (
            usage
                .input_tokens
                .unwrap_or(0)
                .saturating_add(usage.cache_creation_input_tokens.unwrap_or(0))
                .saturating_add(usage.cache_read_input_tokens.unwrap_or(0)),
            usage.output_tokens.unwrap_or(0),
            None,
        ),
        ApiProtocol::OpenAiCompatible => (
            usage.prompt_tokens.unwrap_or(0),
            usage.completion_tokens.unwrap_or(0),
            usage.cost.filter(|cost| cost.is_finite() && *cost >= 0.0),
        ),
        ApiProtocol::Disabled => (0, 0, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::brief::parse_action_proposals;
    use crate::ai::provider::call_provider;
    use axum::{routing::post, Json, Router};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[test]
    fn usage_is_read_from_each_protocol_and_tolerates_missing_fields() {
        assert_eq!(
            parse_usage(
                ApiProtocol::OpenAiCompatible,
                r#"{"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":30}}"#
            ),
            (120, 30, None)
        );
        assert_eq!(
            parse_usage(
                ApiProtocol::OpenAiCompatible,
                r#"{"usage":{"prompt_tokens":5,"completion_tokens":2,"cost":0.0042}}"#
            ),
            (5, 2, Some(0.0042))
        );
        assert_eq!(
            parse_usage(
                ApiProtocol::OpenAiCompatible,
                r#"{"usage":{"prompt_tokens":5,"cost":-1}}"#
            ),
            (5, 0, None)
        );
        assert_eq!(
            parse_usage(
                ApiProtocol::Anthropic,
                r#"{"content":[],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":40,"cache_creation_input_tokens":10}}"#
            ),
            (150, 20, None)
        );
        assert_eq!(
            parse_usage(ApiProtocol::OpenAiCompatible, r#"{"choices":[]}"#),
            (0, 0, None)
        );
        assert_eq!(
            parse_usage(ApiProtocol::OpenAiCompatible, "not json"),
            (0, 0, None)
        );
    }

    #[tokio::test]
    async fn usage_is_recorded_for_successful_responses_even_when_the_content_is_unusable() {
        async fn respond() -> Json<serde_json::Value> {
            Json(
                json!({"choices":[{"message":{"content":"not json"}}],"usage":{"prompt_tokens":77,"completion_tokens":11}}),
            )
        }
        let recorded: Arc<Mutex<Vec<UsageEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&recorded);
        // The recorder is process-wide; keep only this test's model.
        set_usage_recorder(Arc::new(move |event| {
            if event.model == "usage-test-model" {
                sink.lock().unwrap().push(event.clone());
            }
        }));
        let app = Router::new().route("/chat/completions", post(respond));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let purpose = RequestPurpose::about("brief", "Lunch", Some(3));
        let content = with_purpose(purpose.clone(), call_provider(
            AiProvider::Custom,
            "usage-test-model",
            Some(&endpoint),
            "system",
            "prompt",
            100,
            0.1,
            None,
            "key",
        ))
        .await
        .unwrap();
        assert!(parse_action_proposals(&content, &[]).is_err());
        server.abort();

        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].provider.id(), "custom");
        assert_eq!(
            (
                events[0].input_tokens,
                events[0].output_tokens,
                events[0].cost_usd
            ),
            (77, 11, None)
        );
        // The activity log learns what the request was for, never its text.
        assert_eq!(events[0].purpose, Some(purpose));
    }
}
