//! The AI provider seam: which vendor and protocol a request goes to, the
//! API key in the keychain, and the HTTP calls with their fallbacks.

use keyring::Entry;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error_text::display;

use super::usage::record_usage;

const SERVICE: &str = "app.threestrands.mail";
const KEY: &str = "ai-provider-api-key";

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    None,
    OpenAi,
    Anthropic,
    OpenRouter,
    Fireworks,
    Straitly,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApiProtocol {
    Anthropic,
    OpenAiCompatible,
    Disabled,
}

pub(super) struct ProviderDescriptor {
    pub(super) protocol: ApiProtocol,
    pub(super) base_url: Option<&'static str>,
}

/// Whether a request asks the model to skip reasoning. The webview sends
/// `Off` only for work routed to the user's fast model; everything else keeps
/// the model's own default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reasoning {
    #[default]
    Default,
    Off,
}

impl AiProvider {
    /// The request parameter that turns reasoning off, for providers that
    /// document one. Fireworks accepts `reasoning_effort: "none"` on models
    /// that can skip reasoning and rejects it on models that cannot, which the
    /// caller answers by retrying with the model's default. Anthropic models
    /// do not think unless asked, so they need nothing.
    fn reasoning_off_parameter(self) -> Option<(&'static str, serde_json::Value)> {
        match self {
            Self::Fireworks => Some(("reasoning_effort", json!("none"))),
            _ => None,
        }
    }

    #[cfg(test)]
    const ALL: [Self; 7] = [
        Self::None,
        Self::OpenAi,
        Self::Anthropic,
        Self::OpenRouter,
        Self::Fireworks,
        Self::Straitly,
        Self::Custom,
    ];

    pub(super) fn descriptor(self) -> ProviderDescriptor {
        match self {
            Self::None => ProviderDescriptor {
                protocol: ApiProtocol::Disabled,
                base_url: None,
            },
            Self::OpenAi => ProviderDescriptor {
                protocol: ApiProtocol::OpenAiCompatible,
                base_url: Some("https://api.openai.com/v1"),
            },
            Self::Anthropic => ProviderDescriptor {
                protocol: ApiProtocol::Anthropic,
                base_url: Some("https://api.anthropic.com/v1"),
            },
            Self::OpenRouter => ProviderDescriptor {
                protocol: ApiProtocol::OpenAiCompatible,
                base_url: Some("https://openrouter.ai/api/v1"),
            },
            Self::Fireworks => ProviderDescriptor {
                protocol: ApiProtocol::OpenAiCompatible,
                base_url: Some("https://api.fireworks.ai/inference/v1"),
            },
            Self::Straitly => ProviderDescriptor {
                protocol: ApiProtocol::OpenAiCompatible,
                base_url: Some("https://api.straitly.ai/v1"),
            },
            Self::Custom => ProviderDescriptor {
                protocol: ApiProtocol::OpenAiCompatible,
                base_url: None,
            },
        }
    }

    /// The provider's settings identifier, as stored with usage records.
    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::OpenRouter => "openrouter",
            Self::Fireworks => "fireworks",
            Self::Straitly => "straitly",
            Self::Custom => "custom",
        }
    }

    /// Whether the provider's `/models` catalog answers without a key. A key
    /// must never be sent where it is not needed: the keychain holds one key
    /// for whichever provider the user set up.
    fn catalog_is_public(self) -> bool {
        matches!(self, Self::OpenRouter | Self::Straitly)
    }

    fn base_url(self, endpoint: Option<&str>) -> Result<String, String> {
        if let Some(base_url) = self.descriptor().base_url {
            return Ok(base_url.to_string());
        }
        if matches!(self, Self::Custom) {
            return endpoint
                .map(|value| value.trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Set an endpoint URL in AI settings".to_string());
        }
        Err("Select an AI provider in settings".to_string())
    }
}

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

/// Whether the provider stopped because it reached the output token limit
/// rather than because the model finished. Reasoning models can spend the
/// whole budget thinking and return no answer at all.
pub(super) fn reply_was_cut_off(reply: &ProviderReply) -> bool {
    matches!(reply.stop_reason.as_deref(), Some("length" | "max_tokens"))
}

pub(super) const CUT_OFF_REPLY_ERROR: &str =
    "The AI model ran out of output space before answering. Try again, or choose a model that reasons less.";

/// Content-free diagnostic metadata. Provider stop reasons are untrusted too.
pub(super) fn describe_reply(reply: &ProviderReply) -> String {
    let reason = match reply.stop_reason.as_deref() {
        Some(
            reason @ ("length" | "max_tokens" | "stop" | "end_turn" | "tool_use" | "content_filter"
            | "refusal"),
        ) => reason,
        None => "unreported",
        _ => "other",
    };
    format!(
        "{} chars, stop reason: {reason}",
        reply.content.chars().count()
    )
}

const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Long enough for a reasoning model to fill `STRUCTURED_OUTPUT_TOKENS` at a
/// modest generation speed, so a slow answer is not cut off as a timeout.
pub(super) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Output budgets are ceilings, not targets: a provider bills only the tokens
/// the model writes, and the prompts and parsers still bound the visible
/// answer. Reasoning models spend part of the budget thinking before they
/// answer, and one that exhausts it returns nothing usable, so these favor
/// headroom over thrift. Structured calls (contact enrichment, thread
/// analysis, briefs, chat) get the larger budget; plain-text summaries and
/// reply drafts get the smaller one.
pub(crate) const STRUCTURED_OUTPUT_TOKENS: usize = 16_000;
pub(crate) const TEXT_OUTPUT_TOKENS: usize = 8_000;

/// A JSON Schema the provider should constrain its reply to. Every provider
/// requires an object at the root, so array results are wrapped in a single
/// named property that the tolerant parsers also accept.
pub(crate) struct OutputSchema {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) schema: serde_json::Value,
}

/// A failed provider call. `rejected_request` marks the statuses a provider
/// uses when it cannot honor a request parameter such as `response_format`
/// (OpenRouter answers 404 when `require_parameters` finds no endpoint), as
/// opposed to auth, quota, or availability failures that a retry would not fix.
struct ProviderError {
    rejected_request: bool,
    retry_after: Option<std::time::Duration>,
    message: String,
}

impl From<String> for ProviderError {
    fn from(message: String) -> Self {
        Self {
            rejected_request: false,
            retry_after: None,
            message,
        }
    }
}

/// One request to a model: where it goes and what it asks. Named fields keep
/// the two prompts, and the endpoint and key, from trading places unnoticed.
pub(crate) struct AiCall<'a> {
    pub(crate) feature: crate::ai::manifest::Feature,
    pub(crate) provider: AiProvider,
    pub(crate) model: &'a str,
    pub(crate) endpoint: Option<&'a str>,
    pub(crate) system_prompt: &'a str,
    pub(crate) prompt: &'a str,
    pub(crate) max_tokens: usize,
    pub(crate) temperature: f64,
    pub(crate) schema: Option<&'a OutputSchema>,
    pub(crate) api_key: &'a str,
}

/// Requests structured output when a schema is supplied. Support varies by
/// provider, model, and (on OpenRouter) routed endpoint, so a request the
/// provider rejects is retried once without the schema; callers parse either
/// reply with the same tolerant, evidence-validating parser.
pub(super) async fn call_provider(call: AiCall<'_>) -> Result<String, String> {
    call_provider_reply(call, Reasoning::Default)
        .await
        .map(|reply| reply.content)
}

/// A provider's reply text together with the reason it reported for stopping
/// (`finish_reason` or `stop_reason`), which distinguishes a reply cut off at
/// the token limit from one the model chose to end.
pub(crate) struct ProviderReply {
    pub(crate) content: String,
    pub(crate) stop_reason: Option<String>,
}

/// Like `call_provider`, but keeps the reported stop reason for diagnostics
/// and can ask the model to skip reasoning.
pub(crate) async fn call_provider_reply(
    call: AiCall<'_>,
    reasoning: Reasoning,
) -> Result<ProviderReply, String> {
    let context = super::manifest::PreparedContext::new(call.prompt, super::usage::scope())?;
    let request = ProviderRequest {
        context: Some(&context.manifest),
        feature: call.feature,
        provider: call.provider,
        model: call.model,
        system_prompt: call.system_prompt,
        prompt: &context.payload,
        max_tokens: call.max_tokens,
        temperature: call.temperature,
        reasoning,
    };
    let base_url = call.provider.base_url(call.endpoint)?;
    let future = send_with_fallbacks(&base_url, request, call.schema, call.api_key);
    if super::requests::running() {
        future.await
    } else {
        #[cfg(test)]
        let runtime = super::requests::Runtime::new();
        #[cfg(not(test))]
        let runtime = super::requests::RUNTIME.clone();
        runtime.run(call.feature.id(), None, future).await
    }
}

/// Sends a request, dropping optional parameters when the provider rejects
/// it. A rejection does not say which parameter it objected to, so the
/// combinations are tried from most to least capable: first without the
/// request to skip reasoning (support varies by model), then without the
/// structured-output schema but with the reasoning switch restored (support
/// varies by provider, model, and on OpenRouter the routed endpoint), then
/// without either. Callers parse every reply with the same tolerant,
/// evidence-validating parser.
async fn send_with_fallbacks(
    base_url: &str,
    mut request: ProviderRequest<'_>,
    schema: Option<&OutputSchema>,
    api_key: &str,
) -> Result<ProviderReply, String> {
    let requested = request.reasoning;
    let mut attempts = vec![(requested, schema)];
    if request.disables_reasoning() {
        attempts.push((Reasoning::Default, schema));
    }
    if schema.is_some() {
        attempts.push((requested, None));
        if request.disables_reasoning() {
            attempts.push((Reasoning::Default, None));
        }
    }
    let mut attempts = attempts.into_iter().peekable();
    let mut transient_retries = 0;
    while let Some((reasoning, schema)) = attempts.next() {
        request.reasoning = reasoning;
        let result = loop {
            let result = send_provider_request(base_url, &request, schema, api_key).await;
            if let Err(error) = &result {
                if let Some(delay) = error.retry_after {
                    if transient_retries < super::requests::MAX_TRANSIENT_RETRIES {
                        transient_retries += 1;
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
            }
            break result;
        };
        match result {
            Err(error) if error.rejected_request && attempts.peek().is_some() => {
                let (next_reasoning, next_schema) = *attempts.peek().unwrap();
                log::warn!(
                    target: "ai_provider",
                    "provider rejected the request for {} (reasoning {}, {}); retrying with reasoning {}, {}: {}",
                    request.model,
                    describe_reasoning(reasoning),
                    describe_schema(schema),
                    describe_reasoning(next_reasoning),
                    describe_schema(next_schema),
                    error.message
                );
            }
            result => return result.map_err(|error| error.message),
        }
    }
    unreachable!("at least one attempt is always made")
}

fn describe_reasoning(reasoning: Reasoning) -> &'static str {
    match reasoning {
        Reasoning::Default => "at the model default",
        Reasoning::Off => "off",
    }
}

fn describe_schema(schema: Option<&OutputSchema>) -> String {
    schema.map_or_else(
        || "no schema".to_string(),
        |schema| format!("schema {}", schema.name),
    )
}

pub(super) struct ProviderRequest<'a> {
    pub(super) context: Option<&'a super::manifest::ContextManifest>,
    pub(super) feature: crate::ai::manifest::Feature,
    pub(super) provider: AiProvider,
    pub(super) model: &'a str,
    pub(super) system_prompt: &'a str,
    pub(super) prompt: &'a str,
    pub(super) max_tokens: usize,
    pub(super) temperature: f64,
    pub(super) reasoning: Reasoning,
}

impl ProviderRequest<'_> {
    /// Whether this request carries a parameter that turns reasoning off.
    fn disables_reasoning(&self) -> bool {
        self.reasoning == Reasoning::Off && self.provider.reasoning_off_parameter().is_some()
    }
}

/// Reads the reason a successful response gave for stopping. Missing or
/// malformed fields read as unreported.
fn parse_stop_reason(protocol: ApiProtocol, body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let reason = match protocol {
        ApiProtocol::Anthropic => value.get("stop_reason"),
        ApiProtocol::OpenAiCompatible => value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("finish_reason")),
        ApiProtocol::Disabled => None,
    };
    reason
        .and_then(|reason| reason.as_str())
        .map(str::to_string)
}

/// Attaches a provider's credentials: Anthropic takes an `x-api-key` header
/// plus its API version, the OpenAI-compatible providers a bearer token.
fn authorize(
    request: reqwest::RequestBuilder,
    protocol: ApiProtocol,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match protocol {
        ApiProtocol::Anthropic => request
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        ApiProtocol::OpenAiCompatible | ApiProtocol::Disabled => request.bearer_auth(api_key),
    }
}

async fn send_provider_request(
    base_url: &str,
    request: &ProviderRequest<'_>,
    schema: Option<&OutputSchema>,
    api_key: &str,
) -> Result<ProviderReply, ProviderError> {
    let context = match request.context {
        Some(context) => context.clone(),
        None => {
            super::manifest::PreparedContext::new(request.prompt, super::usage::scope())?.manifest
        }
    };
    let schema_bytes = schema.map(|schema| serde_json::to_vec(&schema.schema).expect("JSON value"));
    let record = super::requests::RequestRecord {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        request_id: String::new(),
        at: chrono::Utc::now().to_rfc3339(),
        feature: request.feature.id().into(),
        provider: request.provider.id().into(),
        model: request.model.into(),
        endpoint_revision: super::manifest::fingerprint(base_url.as_bytes()),
        prompt_version: request.feature.prompt_version(),
        schema_version: request.feature.schema_version(),
        prompt_revision: super::manifest::fingerprint(request.system_prompt.as_bytes()),
        schema_revision: schema_bytes
            .as_ref()
            .map(|bytes| super::manifest::fingerprint(bytes)),
        context,
        response_revision: None,
        outcome: "dispatched".into(),
        request_outcome: "running".into(),
        charged_usd: 0.0,
        price: None,
    };
    let reservation = super::requests::reserve_current(
        record,
        request.system_prompt.len()
            + request.prompt.len()
            + schema_bytes.as_ref().map_or(0, Vec::len),
        request.max_tokens,
    )?;
    if let Some(text) = super::evaluation::replay_response(request.provider.descriptor().protocol) {
        let parsed = parse_provider_reply(request.provider.descriptor().protocol, &text);
        reservation.finish(
            &text,
            0,
            0,
            None,
            false,
            if parsed.is_ok() {
                "received"
            } else {
                "rejected"
            },
        )?;
        return parsed.map_err(ProviderError::from);
    }
    let protocol = request.provider.descriptor().protocol;
    let (path, body) = match protocol {
        ApiProtocol::Anthropic => ("messages", anthropic_body(request, schema)),
        ApiProtocol::OpenAiCompatible => ("chat/completions", openai_body(request, schema)),
        ApiProtocol::Disabled => {
            return Err("Select an AI provider in settings".to_string().into())
        }
    };
    let response = authorize(
        ai_client()?.post(format!("{base_url}/{path}")),
        protocol,
        api_key,
    )
    .json(&body)
    .send()
    .await
    .map_err(display)?;
    let status = response.status().as_u16();
    let response = match checked(response).await {
        Ok(response) => response,
        Err(error) => {
            reservation.failed(&format!("http_{status}"))?;
            return Err(error);
        }
    };
    let text = response.text().await.map_err(display)?;
    record_usage(request, &text);
    let parsed = parse_provider_reply(protocol, &text);
    let (input, output, cost) = super::usage::parse_usage(protocol, &text);
    let has_usage = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .is_some_and(|value| {
            value.get("usage").is_some_and(|usage| {
                usage.is_object()
                    && (usage.get("input_tokens").is_some() || usage.get("prompt_tokens").is_some())
            })
        });
    reservation.finish(
        &text,
        input,
        output,
        cost,
        has_usage,
        if parsed.is_ok() {
            "received"
        } else {
            "rejected"
        },
    )?;
    parsed.map_err(ProviderError::from)
}

/// Validate completion before parsing content: a cut-off answer can still be
/// valid JSON, plain text, or an empty content field after reasoning.
fn parse_provider_reply(protocol: ApiProtocol, body: &str) -> Result<ProviderReply, String> {
    let mut reply = ProviderReply {
        content: String::new(),
        stop_reason: parse_stop_reason(protocol, body),
    };
    if reply_was_cut_off(&reply) {
        return Err(CUT_OFF_REPLY_ERROR.to_string());
    }
    if matches!(
        reply.stop_reason.as_deref(),
        Some("content_filter" | "refusal")
    ) {
        return Err("The AI provider declined the request".to_string());
    }
    reply.content = match protocol {
        ApiProtocol::Anthropic => parse_anthropic_content(body)?,
        ApiProtocol::OpenAiCompatible => parse_openai_content(body)?,
        ApiProtocol::Disabled => return Err("Select an AI provider in settings".to_string()),
    };
    Ok(reply)
}

fn openai_body(request: &ProviderRequest<'_>, schema: Option<&OutputSchema>) -> serde_json::Value {
    let mut body = json!({
        "model": request.model,
        "temperature": request.temperature,
        "max_tokens": request.max_tokens,
        "messages": [
            {"role": "system", "content": request.system_prompt},
            {"role": "user", "content": request.prompt},
        ],
    });
    if let Some(schema) = schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": schema.name,
                "strict": true,
                "schema": schema.schema,
            },
        });
        // OpenRouter otherwise may route to an endpoint that silently ignores
        // `response_format`; requiring it yields a rejection we can fall back on.
        if matches!(request.provider, AiProvider::OpenRouter) {
            body["provider"] = json!({"require_parameters": true});
        }
    }
    if request.disables_reasoning() {
        if let Some((parameter, value)) = request.provider.reasoning_off_parameter() {
            body[parameter] = value;
        }
    }
    // OpenRouter reports each request's price only when asked.
    if matches!(request.provider, AiProvider::OpenRouter) {
        body["usage"] = json!({"include": true});
    }
    body
}

fn anthropic_body(
    request: &ProviderRequest<'_>,
    schema: Option<&OutputSchema>,
) -> serde_json::Value {
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
        "system": request.system_prompt,
        "messages": [{"role": "user", "content": request.prompt}],
    });
    if let Some(schema) = schema {
        // Forcing a single tool is how the Messages API constrains output to a
        // schema. The tool is never executed; its input is the result.
        body["tools"] = json!([{
            "name": schema.name,
            "description": schema.description,
            "input_schema": schema.schema,
        }]);
        body["tool_choice"] = json!({"type": "tool", "name": schema.name});
    }
    body
}

/// Sends a bounded, content-free prompt so the settings screen can verify the
/// provider, endpoint, model, and key without exposing any mail data.
///
/// The one-token budget keeps the test cheap, so a model that reasons or
/// says more than one token stops at the limit. That reply still proves the
/// provider accepted the endpoint, key, and model, so it counts as success.
pub async fn test_connection(
    provider: AiProvider,
    model: &str,
    endpoint: Option<&str>,
    api_key: &str,
) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err("Set a model name in AI settings".to_string());
    }
    call_provider(AiCall {
        feature: crate::ai::manifest::Feature::Connection,
        provider,
        model: model.trim(),
        endpoint,
        system_prompt: "You are a connection test. Reply with exactly OK.",
        prompt: "Reply with exactly OK.",
        max_tokens: 1,
        temperature: 0.0,
        schema: None,
        api_key,
    })
    .await
    .map(|_| ())
    .or_else(|error| {
        if error == CUT_OFF_REPLY_ERROR {
            Ok(())
        } else {
            Err(error)
        }
    })
}

/// One model offered by a provider's list endpoint. `label` is a human name
/// when the provider gives one (OpenRouter `name`, Anthropic `display_name`);
/// `id` is the value the user selects and the request sends.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: Option<String>,
    /// The catalog's price, when it lists a usable one.
    pub price: Option<CatalogPrice>,
}

/// A catalog price in US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPrice {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

/// Fetches the models a provider offers, for the settings model picker. Reads
/// nothing but the provider's own catalog: no mail data leaves the device.
/// Returns the list verbatim; the webview filters and sorts it.
///
/// A public catalog (OpenRouter, Straitly) is always fetched anonymously: the
/// keychain holds one key for whichever provider the user set up, and a key
/// must never reach a vendor that does not need it. The other providers
/// require `api_key` and error without one.
pub async fn list_models(
    provider: AiProvider,
    api_key: Option<&str>,
) -> Result<Vec<ModelInfo>, String> {
    let protocol = provider.descriptor().protocol;
    if matches!(protocol, ApiProtocol::Disabled) {
        return Err("Select an AI provider in settings".to_string());
    }
    fetch_models(provider, &provider.base_url(None)?, api_key).await
}

async fn fetch_models(
    provider: AiProvider,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<ModelInfo>, String> {
    let protocol = provider.descriptor().protocol;
    let url = match protocol {
        ApiProtocol::Anthropic => format!("{base_url}/models?limit=1000"),
        _ => format!("{base_url}/models"),
    };
    let mut request = ai_client()?.get(url);
    if !provider.catalog_is_public() {
        let api_key = api_key.ok_or_else(|| "No AI API key configured".to_string())?;
        request = authorize(request, protocol, api_key);
    }
    let text = request
        .send()
        .await
        .map_err(display)?
        .error_for_status()
        .map_err(display)?
        .text()
        .await
        .map_err(display)?;
    parse_models(&text)
}

/// Parses a `/models` list (`{ "data": [{ "id", "name" | "display_name"?,
/// "pricing"? }] }`). The OpenAI-compatible providers share one shape, where
/// OpenRouter and Straitly add `name` and `pricing` (USD per token as decimal
/// strings); Anthropic's differs only in calling the name `display_name`. A
/// missing, unparsable, negative (OpenRouter's variable-price routers), or
/// out-of-range price leaves that model without one and never fails the list.
fn parse_models(body: &str) -> Result<Vec<ModelInfo>, String> {
    #[derive(Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        id: String,
        #[serde(default, alias = "display_name")]
        name: Option<String>,
        #[serde(default)]
        pricing: Option<serde_json::Value>,
    }
    let parsed: ModelsResponse = serde_json::from_str(body)
        .map_err(|_| "The AI provider returned an invalid response".to_string())?;
    Ok(parsed
        .data
        .into_iter()
        .filter(|entry| !entry.id.trim().is_empty())
        .map(|entry| ModelInfo {
            id: entry.id,
            label: entry.name.filter(|name| !name.trim().is_empty()),
            price: entry.pricing.as_ref().and_then(catalog_price),
        })
        .collect())
}

/// Reads `prompt` and `completion` (USD per token) from a catalog `pricing`
/// object as per-million prices, held to the same bounds as a request's price.
fn catalog_price(pricing: &serde_json::Value) -> Option<CatalogPrice> {
    let per_million = |field: &str| -> Option<f64> {
        let raw = match pricing.get(field)? {
            serde_json::Value::String(text) => text.trim().parse::<f64>().ok()?,
            serde_json::Value::Number(number) => number.as_f64()?,
            _ => return None,
        };
        // Ten significant digits drop float noise (0.0000024 * 1e6).
        let scaled = format!("{:.9e}", raw * 1_000_000.0).parse::<f64>().ok()?;
        (scaled.is_finite() && (0.0..=MAX_CATALOG_PRICE).contains(&scaled)).then_some(scaled)
    };
    Some(CatalogPrice {
        input_per_million: per_million("prompt")?,
        output_per_million: per_million("completion")?,
    })
}

/// The same ceiling as `ModelPrice::valid` in `requests.rs`.
const MAX_CATALOG_PRICE: f64 = 1_000_000.0;

fn ai_client() -> Result<reqwest::Client, String> {
    crate::http_client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(display)
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
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        refusal: Option<String>,
    }
    let parsed: ChatResponse = serde_json::from_str(body)
        .map_err(|_| "The AI provider returned an invalid response".to_string())?;
    let message = parsed
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message)
        .ok_or_else(|| "AI provider returned no choices".to_string())?;
    match (message.content, message.refusal) {
        (_, Some(_)) => Err("The AI provider declined the request".to_string()),
        (Some(content), None) => Ok(content),
        (None, None) => Err("AI provider returned no content".to_string()),
    }
}

/// Returns a forced tool call's input as JSON text when present, otherwise the
/// first text block, so structured and plain replies share one parser.
fn parse_anthropic_content(body: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct MessagesResponse {
        content: Vec<ContentBlock>,
    }
    #[derive(Deserialize)]
    struct ContentBlock {
        #[serde(default, rename = "type")]
        kind: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        input: Option<serde_json::Value>,
    }
    let parsed: MessagesResponse = serde_json::from_str(body)
        .map_err(|_| "The AI provider returned an invalid response".to_string())?;
    if let Some(input) = parsed
        .content
        .iter()
        .find(|block| block.kind.as_deref() == Some("tool_use"))
        .and_then(|block| block.input.as_ref())
    {
        return serde_json::to_string(input).map_err(display);
    }
    parsed
        .content
        .into_iter()
        .find_map(|block| block.text)
        .ok_or_else(|| "AI provider returned no content".to_string())
}

fn retry_delay(header: &str) -> Option<std::time::Duration> {
    let seconds = header.parse::<u64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc2822(header).ok().map(|at| {
            (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
                .num_seconds()
                .max(0) as u64
        })
    })?;
    (seconds <= super::requests::MAX_RETRY_SECONDS).then(|| std::time::Duration::from_secs(seconds))
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    // Retry only explicit rate-limit/transient HTTP responses, never ambiguous
    // transport failures or timeouts. Long Retry-After values are surfaced.
    let retry_after = if matches!(status.as_u16(), 429 | 502 | 503 | 504) {
        match response.headers().get(reqwest::header::RETRY_AFTER) {
            Some(header) => header.to_str().ok().and_then(retry_delay),
            None => Some(std::time::Duration::from_secs(1)),
        }
    } else {
        None
    };
    // Error bodies can echo prompts, credentials, or private generated text.
    Err(ProviderError {
        rejected_request: matches!(status.as_u16(), 400 | 404 | 422),
        retry_after,
        message: format!("AI provider returned {status}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::contact::{contact_output_schema, contact_suggestion_values, CONTACT_FIELDS};
    use axum::{extract::State, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    #[test]
    fn parse_models_reads_id_and_optional_name() {
        // OpenAI omits `name`; OpenRouter supplies it. One payload shape covers
        // every OpenAI-compatible provider.
        let body = r#"{"data":[
            {"id":"gpt-4o","object":"model"},
            {"id":"anthropic/claude-3.5-sonnet","name":"Claude 3.5 Sonnet"},
            {"id":"  ","name":"blank id dropped"},
            {"id":"text-embedding-3-small"}
        ]}"#;
        let models = parse_models(body).expect("parses");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec![
                "gpt-4o",
                "anthropic/claude-3.5-sonnet",
                "text-embedding-3-small"
            ]
        );
        assert_eq!(models[0].label, None);
        assert_eq!(models[1].label.as_deref(), Some("Claude 3.5 Sonnet"));
    }

    #[test]
    fn parse_models_reads_anthropic_display_name() {
        let body = r#"{"data":[
            {"type":"model","id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5"},
            {"type":"model","id":"claude-3-5-haiku-latest"},
            {"type":"model","id":"","display_name":"blank id dropped"}
        ]}"#;
        let models = parse_models(body).expect("parses");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["claude-sonnet-4-5", "claude-3-5-haiku-latest"]
        );
        assert_eq!(models[0].label.as_deref(), Some("Claude Sonnet 4.5"));
        assert_eq!(models[1].label, None);
    }

    #[test]
    fn parse_models_reads_catalog_prices_per_million() {
        let body = r#"{"data":[
            {"id":"or/model","name":"OR","pricing":{"prompt":"0.0000024","completion":"0.000012","request":"0","image":"0"}},
            {"id":"straitly/model","pricing":{"prompt":"0.000003","completion":"0.000015","input_cache_read":"0.0000003"}},
            {"id":"or/auto","pricing":{"prompt":"-1","completion":"-1"}},
            {"id":"free","pricing":{"prompt":"0","completion":"0"}},
            {"id":"none"},
            {"id":"garbage","pricing":{"prompt":"abc","completion":"0.1"}},
            {"id":"half","pricing":{"prompt":"0.1"}},
            {"id":"huge","pricing":{"prompt":"1","completion":"1.5"}},
            {"id":"odd","pricing":"free"},
            {"id":"numeric","pricing":{"prompt":0.000001,"completion":0.000002}}
        ]}"#;
        let models = parse_models(body).expect("a bad price never fails the list");
        let price = |id: &str| models.iter().find(|m| m.id == id).unwrap().price;
        assert_eq!(
            price("or/model"),
            Some(CatalogPrice {
                input_per_million: 2.4,
                output_per_million: 12.0
            })
        );
        assert_eq!(
            price("straitly/model"),
            Some(CatalogPrice {
                input_per_million: 3.0,
                output_per_million: 15.0
            })
        );
        assert_eq!(
            price("free"),
            Some(CatalogPrice {
                input_per_million: 0.0,
                output_per_million: 0.0
            })
        );
        assert_eq!(
            price("numeric"),
            Some(CatalogPrice {
                input_per_million: 1.0,
                output_per_million: 2.0
            })
        );
        for id in ["or/auto", "none", "garbage", "half", "huge", "odd"] {
            assert_eq!(price(id), None, "{id}");
        }
        assert_eq!(models.len(), 10);
    }

    #[test]
    fn catalog_prices_accept_the_exact_limit_and_reject_above_it() {
        let at = serde_json::json!({"prompt": "1", "completion": "0.5"});
        assert_eq!(catalog_price(&at).unwrap().input_per_million, 1_000_000.0);
        let above = serde_json::json!({"prompt": "1.000001", "completion": "0"});
        assert_eq!(catalog_price(&above), None);
    }

    #[test]
    fn straitly_is_an_openai_compatible_provider_with_a_public_catalog() {
        assert_eq!(AiProvider::Straitly.id(), "straitly");
        let descriptor = AiProvider::Straitly.descriptor();
        assert_eq!(descriptor.protocol, ApiProtocol::OpenAiCompatible);
        assert_eq!(descriptor.base_url, Some("https://api.straitly.ai/v1"));
        assert!(AiProvider::Straitly.catalog_is_public());
        assert!(AiProvider::OpenRouter.catalog_is_public());
        for provider in [
            AiProvider::OpenAi,
            AiProvider::Anthropic,
            AiProvider::Fireworks,
            AiProvider::Custom,
        ] {
            assert!(!provider.catalog_is_public());
        }
        assert_eq!(
            serde_json::from_str::<AiProvider>("\"straitly\"")
                .unwrap()
                .id(),
            "straitly"
        );
        assert!(openai_body(&provider_request(AiProvider::Straitly), None)
            .get("usage")
            .is_none());
        assert!(AiProvider::Straitly.reasoning_off_parameter().is_none());
    }

    #[test]
    fn parse_models_rejects_malformed_payloads() {
        assert!(parse_models("not json").is_err());
        assert!(parse_models(r#"{"models":[]}"#).is_err());
    }

    #[tokio::test]
    async fn list_models_without_key_errors_for_keyed_providers_only() {
        // OpenRouter is the only one whose catalog lists anonymously; the others
        // must report the missing key rather than fire a doomed request.
        assert!(list_models(AiProvider::OpenAi, None).await.is_err());
        assert!(list_models(AiProvider::Anthropic, None).await.is_err());
        assert!(list_models(AiProvider::Fireworks, None).await.is_err());
        assert!(list_models(AiProvider::None, None).await.is_err());
        assert!(list_models(AiProvider::Custom, None).await.is_err());
    }

    #[tokio::test]
    async fn list_models_never_sends_the_key_to_openrouter_but_does_to_keyed_providers() {
        // The keychain holds one key, issued for whichever provider the user set
        // up. OpenRouter's catalog is public, so listing it must not hand that
        // key to OpenRouter; a keyed provider still authenticates.
        let captured = Arc::new(Mutex::new(Vec::<axum::http::HeaderMap>::new()));
        async fn respond(
            State(captured): State<Arc<Mutex<Vec<axum::http::HeaderMap>>>>,
            headers: axum::http::HeaderMap,
        ) -> Json<serde_json::Value> {
            captured.lock().unwrap().push(headers);
            Json(json!({"data": [{"id": "model-a"}]}))
        }
        let app = Router::new()
            .route("/models", axum::routing::get(respond))
            .with_state(captured.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let models = fetch_models(AiProvider::OpenRouter, &base_url, Some("secret-key"))
            .await
            .unwrap();
        assert_eq!(models[0].id, "model-a");
        fetch_models(AiProvider::Straitly, &base_url, Some("secret-key"))
            .await
            .unwrap();
        fetch_models(AiProvider::OpenAi, &base_url, Some("secret-key"))
            .await
            .unwrap();
        fetch_models(AiProvider::Anthropic, &base_url, Some("secret-key"))
            .await
            .unwrap();

        let headers = captured.lock().unwrap();
        assert_eq!(headers.len(), 4);
        for public in [&headers[0], &headers[1]] {
            assert!(public.get("authorization").is_none());
            assert!(public.get("x-api-key").is_none());
        }
        assert_eq!(
            headers[2].get("authorization").unwrap(),
            "Bearer secret-key"
        );
        assert_eq!(headers[3].get("x-api-key").unwrap(), "secret-key");
        assert!(headers[3].get("anthropic-version").is_some());
        server.abort();
    }

    #[test]
    fn base_url_resolves_known_providers() {
        assert_eq!(
            AiProvider::OpenAi.base_url(None).unwrap(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            AiProvider::OpenRouter.base_url(None).unwrap(),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            AiProvider::Straitly.base_url(None).unwrap(),
            "https://api.straitly.ai/v1"
        );
        assert_eq!(
            AiProvider::Fireworks.base_url(None).unwrap(),
            "https://api.fireworks.ai/inference/v1"
        );
    }

    #[test]
    fn base_url_uses_endpoint_for_custom_provider() {
        assert_eq!(
            AiProvider::Custom
                .base_url(Some("https://example.com/v1/"))
                .unwrap(),
            "https://example.com/v1"
        );
        assert!(AiProvider::Custom.base_url(None).is_err());
        assert!(AiProvider::Custom.base_url(Some("")).is_err());
    }

    #[tokio::test]
    async fn connection_test_rejects_a_blank_model_before_network_use() {
        let error = test_connection(AiProvider::OpenAi, "  ", None, "test-key")
            .await
            .unwrap_err();
        assert_eq!(error, "Set a model name in AI settings");
    }

    #[test]
    fn rust_and_typescript_provider_identifiers_match() {
        let typescript = include_str!("../../../src/aiSettings.ts");
        let typescript_ids: Vec<&str> = typescript
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("{ id: \"")
                    .and_then(|rest| rest.split('"').next())
            })
            .collect();
        let rust_ids: Vec<String> = AiProvider::ALL
            .iter()
            .map(|provider| serde_json::to_value(provider).unwrap())
            .map(|value| value.as_str().unwrap().to_string())
            .collect();

        assert_eq!(rust_ids, typescript_ids);
    }

    #[test]
    fn parses_openai_style_response() {
        let body = r#"{"choices":[{"message":{"content":"- Point one\n- Point two"}}]}"#;
        assert_eq!(
            parse_openai_content(body).unwrap(),
            "- Point one\n- Point two"
        );
    }

    #[test]
    fn parses_anthropic_style_response() {
        let body = r#"{"content":[{"type":"text","text":"- Point one"}]}"#;
        assert_eq!(parse_anthropic_content(body).unwrap(), "- Point one");
    }

    #[test]
    fn parses_anthropic_forced_tool_input_as_json_text() {
        let body = r#"{"content":[{"type":"tool_use","id":"t1","name":"contact_suggestions","input":{"suggestions":[]}}]}"#;
        let content = parse_anthropic_content(body).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&content).unwrap(),
            json!({"suggestions": []})
        );
    }

    #[test]
    fn reads_the_reported_stop_reason_for_each_protocol() {
        assert_eq!(
            parse_stop_reason(
                ApiProtocol::OpenAiCompatible,
                r#"{"choices":[{"message":{"content":"[{"},"finish_reason":"length"}]}"#
            )
            .as_deref(),
            Some("length")
        );
        assert_eq!(
            parse_stop_reason(
                ApiProtocol::Anthropic,
                r#"{"content":[{"type":"text","text":"x"}],"stop_reason":"max_tokens"}"#
            )
            .as_deref(),
            Some("max_tokens")
        );
        for body in [
            r#"{"choices":[{"message":{"content":"x"}}]}"#,
            r#"{"choices":[{"message":{"content":"x"},"finish_reason":null}]}"#,
            "not json",
        ] {
            assert_eq!(
                parse_stop_reason(ApiProtocol::OpenAiCompatible, body),
                None,
                "{body}"
            );
        }
    }

    #[test]
    fn rejected_reply_summary_contains_only_metadata() {
        let private = "private correspondence and writing profile";
        for (stop_reason, expected) in [
            (Some("length"), "length"),
            (None, "unreported"),
            (Some(private), "other"),
        ] {
            let description = describe_reply(&ProviderReply {
                content: private.repeat(1000),
                stop_reason: stop_reason.map(str::to_string),
            });
            assert_eq!(
                description,
                format!("{} chars, stop reason: {expected}", private.len() * 1000)
            );
            assert!(!description.contains(private));
        }
    }

    #[test]
    fn openai_refusal_is_reported_instead_of_parsed() {
        let body =
            r#"{"choices":[{"message":{"content":null,"refusal":"I can't help with that."}}]}"#;
        assert_eq!(
            parse_openai_content(body).unwrap_err(),
            "The AI provider declined the request"
        );
    }

    fn provider_request(provider: AiProvider) -> ProviderRequest<'static> {
        ProviderRequest {
            context: None,
            feature: crate::ai::manifest::Feature::Connection,
            provider,
            model: "model",
            system_prompt: "system",
            prompt: "prompt",
            max_tokens: 100,
            temperature: 0.1,
            reasoning: Reasoning::Default,
        }
    }

    #[test]
    fn structured_requests_carry_the_schema_in_each_protocol_shape() {
        let schema = contact_output_schema(&CONTACT_FIELDS);
        for provider in [
            AiProvider::OpenAi,
            AiProvider::OpenRouter,
            AiProvider::Fireworks,
            AiProvider::Straitly,
            AiProvider::Custom,
        ] {
            let request = provider_request(provider);
            let body = openai_body(&request, Some(&schema));
            assert_eq!(body["response_format"]["type"], "json_schema");
            assert_eq!(
                body["response_format"]["json_schema"]["name"],
                "contact_suggestions"
            );
            assert_eq!(body["response_format"]["json_schema"]["strict"], true);
            assert_eq!(
                body["response_format"]["json_schema"]["schema"],
                schema.schema
            );
            assert_eq!(
                body["provider"]["require_parameters"] == true,
                matches!(provider, AiProvider::OpenRouter)
            );
            let plain = openai_body(&request, None);
            assert!(plain.get("response_format").is_none());
            assert!(plain.get("provider").is_none());
        }
        let request = provider_request(AiProvider::Anthropic);
        let body = anthropic_body(&request, Some(&schema));
        assert_eq!(body["tools"][0]["name"], "contact_suggestions");
        assert_eq!(body["tools"][0]["input_schema"], schema.schema);
        assert_eq!(
            body["tool_choice"],
            json!({"type": "tool", "name": "contact_suggestions"})
        );
        let plain = anthropic_body(&request, None);
        assert!(plain.get("tools").is_none());
        assert!(plain.get("tool_choice").is_none());
    }

    #[tokio::test]
    async fn structured_output_falls_back_only_when_the_provider_rejects_the_schema() {
        use axum::http::StatusCode;
        type RequestLog = Arc<Mutex<Vec<serde_json::Value>>>;
        async fn respond(
            State((requests, status)): State<(RequestLog, StatusCode)>,
            Json(payload): Json<serde_json::Value>,
        ) -> (StatusCode, Json<serde_json::Value>) {
            let structured = payload.get("response_format").is_some();
            requests.lock().unwrap().push(payload);
            if structured && status != StatusCode::OK {
                return (status, Json(json!({"error": "unsupported"})));
            }
            let content = if structured {
                r#"{"suggestions":[]}"#
            } else {
                "[]"
            };
            (
                StatusCode::OK,
                Json(json!({"choices":[{"message":{"content":content}}]})),
            )
        }
        for (status, expected_calls, succeeds) in [
            (StatusCode::OK, 1, true),
            (StatusCode::BAD_REQUEST, 2, true),
            (StatusCode::NOT_FOUND, 2, true),
            (StatusCode::UNPROCESSABLE_ENTITY, 2, true),
            (StatusCode::UNAUTHORIZED, 1, false),
            (StatusCode::TOO_MANY_REQUESTS, 3, false),
        ] {
            let requests: RequestLog = Arc::new(Mutex::new(Vec::new()));
            let app = Router::new()
                .route("/chat/completions", post(respond))
                .with_state((Arc::clone(&requests), status));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let result = call_provider(AiCall {
                feature: crate::ai::manifest::Feature::Connection,
                provider: AiProvider::Custom,
                model: "model",
                endpoint: Some(&endpoint),
                system_prompt: "system",
                prompt: "prompt",
                max_tokens: 100,
                temperature: 0.1,
                schema: Some(&contact_output_schema(&CONTACT_FIELDS)),
                api_key: "key",
            })
            .await;
            assert_eq!(result.is_ok(), succeeds, "{status}: {result:?}");
            if let Ok(content) = result {
                assert!(contact_suggestion_values(&content).unwrap().is_empty());
            }
            let calls = requests.lock().unwrap();
            assert_eq!(calls.len(), expected_calls, "{status}");
            assert!(calls[0].get("response_format").is_some());
            if expected_calls == 2 {
                assert!(calls[1].get("response_format").is_none());
            }
            drop(calls);
            server.abort();
        }
    }

    #[test]
    fn only_fireworks_requests_turn_reasoning_off_and_only_when_asked() {
        let request = |provider, reasoning| ProviderRequest {
            context: None,
            feature: crate::ai::manifest::Feature::Connection,
            reasoning,
            ..provider_request(provider)
        };
        let off = openai_body(&request(AiProvider::Fireworks, Reasoning::Off), None);
        assert_eq!(off["reasoning_effort"], json!("none"));
        assert!(off.get("thinking").is_none());
        let structured = openai_body(
            &request(AiProvider::Fireworks, Reasoning::Off),
            Some(&contact_output_schema(&CONTACT_FIELDS)),
        );
        assert_eq!(structured["reasoning_effort"], json!("none"));
        assert!(structured.get("response_format").is_some());
        assert!(
            openai_body(&request(AiProvider::Fireworks, Reasoning::Default), None)
                .get("reasoning_effort")
                .is_none()
        );
        // Providers without a documented, verified switch keep the model's
        // default rather than receiving a parameter they may reject.
        for provider in [
            AiProvider::OpenAi,
            AiProvider::OpenRouter,
            AiProvider::Custom,
        ] {
            let body = openai_body(&request(provider, Reasoning::Off), None);
            assert!(body.get("reasoning_effort").is_none(), "{provider:?}");
            assert!(body.get("reasoning").is_none(), "{provider:?}");
        }
        // Anthropic models do not think unless asked.
        let anthropic = anthropic_body(&request(AiProvider::Anthropic, Reasoning::Off), None);
        assert!(anthropic.get("thinking").is_none());
    }

    #[test]
    fn reasoning_mode_reads_the_webview_values_and_defaults_to_the_model() {
        assert_eq!(
            serde_json::from_value::<Reasoning>(json!("off")).unwrap(),
            Reasoning::Off
        );
        assert_eq!(
            serde_json::from_value::<Reasoning>(json!("default")).unwrap(),
            Reasoning::Default
        );
        assert!(serde_json::from_value::<Reasoning>(json!("none")).is_err());
        assert_eq!(Reasoning::default(), Reasoning::Default);
    }

    #[tokio::test]
    async fn a_rejected_reasoning_switch_is_dropped_before_the_schema() {
        use axum::http::StatusCode;
        type RequestLog = Arc<Mutex<Vec<serde_json::Value>>>;
        // Rejects whichever optional parameters it is told not to support.
        async fn respond(
            State((requests, rejects_reasoning, rejects_schema)): State<(RequestLog, bool, bool)>,
            Json(payload): Json<serde_json::Value>,
        ) -> (StatusCode, Json<serde_json::Value>) {
            let rejected = (rejects_reasoning && payload.get("reasoning_effort").is_some())
                || (rejects_schema && payload.get("response_format").is_some());
            requests.lock().unwrap().push(payload);
            if rejected {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "unsupported"})),
                );
            }
            (
                StatusCode::OK,
                Json(json!({"choices":[{"message":{"content":"{\"suggestions\":[]}"}}]})),
            )
        }
        let schema = contact_output_schema(&CONTACT_FIELDS);
        // (rejects reasoning, rejects schema) -> which parameters each call carried.
        for (rejects_reasoning, rejects_schema, expected) in [
            (false, false, vec![(true, true)]),
            (true, false, vec![(true, true), (false, true)]),
            (
                true,
                true,
                vec![(true, true), (false, true), (true, false), (false, false)],
            ),
            // A rejection does not name the parameter, so the reasoning
            // switch is dropped first, then restored once the schema goes.
            (
                false,
                true,
                vec![(true, true), (false, true), (true, false)],
            ),
        ] {
            let requests: RequestLog = Arc::new(Mutex::new(Vec::new()));
            let app = Router::new()
                .route("/chat/completions", post(respond))
                .with_state((Arc::clone(&requests), rejects_reasoning, rejects_schema));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

            let request = ProviderRequest {
                context: None,
                feature: crate::ai::manifest::Feature::Connection,
                reasoning: Reasoning::Off,
                ..provider_request(AiProvider::Fireworks)
            };
            let reply = super::super::requests::Runtime::new()
                .run(
                    "connection",
                    None,
                    send_with_fallbacks(&base_url, request, Some(&schema), "key"),
                )
                .await;
            let case = (rejects_reasoning, rejects_schema);
            assert!(reply.is_ok(), "{case:?}: {:?}", reply.err());
            let carried = requests
                .lock()
                .unwrap()
                .iter()
                .map(|call| {
                    (
                        call.get("reasoning_effort").is_some(),
                        call.get("response_format").is_some(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(carried, expected, "{case:?}");
            server.abort();
        }
    }

    #[tokio::test]
    async fn rate_limit_retries_preserve_the_reasoning_switch() {
        use axum::http::StatusCode;
        async fn respond(
            State(calls): State<Arc<Mutex<usize>>>,
            Json(payload): Json<serde_json::Value>,
        ) -> StatusCode {
            assert_eq!(payload["reasoning_effort"], "none");
            *calls.lock().unwrap() += 1;
            StatusCode::TOO_MANY_REQUESTS
        }
        let calls = Arc::new(Mutex::new(0));
        let app = Router::new()
            .route("/chat/completions", post(respond))
            .with_state(Arc::clone(&calls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let request = ProviderRequest {
            context: None,
            feature: crate::ai::manifest::Feature::Connection,
            reasoning: Reasoning::Off,
            ..provider_request(AiProvider::Fireworks)
        };
        assert!(super::super::requests::Runtime::new()
            .run(
                "connection",
                None,
                send_with_fallbacks(&base_url, request, None, "key")
            )
            .await
            .is_err());
        assert_eq!(*calls.lock().unwrap(), 3);
        server.abort();
    }

    #[test]
    fn retry_after_respects_the_bound_and_never_turns_a_long_delay_into_an_immediate_retry() {
        assert_eq!(retry_delay("0"), Some(std::time::Duration::ZERO));
        assert_eq!(retry_delay("5"), Some(std::time::Duration::from_secs(5)));
        for value in ["6", "999", "invalid", "-1"] {
            assert!(retry_delay(value).is_none());
        }
    }

    #[tokio::test]
    async fn transient_retry_recovers_and_reserves_each_attempt_before_dispatch() {
        async fn respond(State(calls): State<Arc<Mutex<usize>>>) -> axum::response::Response {
            use axum::response::IntoResponse;
            let mut count = calls.lock().unwrap();
            *count += 1;
            if *count == 1 {
                return (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    [("Retry-After", "0")],
                )
                    .into_response();
            }
            Json(json!({"choices":[{"message":{"content":"Recovered"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3}})).into_response()
        }
        let calls = Arc::new(Mutex::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/chat/completions", post(respond))
            .with_state(calls.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let runtime = super::super::requests::Runtime::new();
        let control = super::super::requests::RequestControl {
            request_id: uuid::Uuid::new_v4().to_string(),
            price: Some(super::super::requests::ModelPrice {
                input_per_million: 1.0,
                output_per_million: 1.0,
            }),
        };
        let result = runtime
            .run(
                "summary",
                Some(control),
                call_provider(AiCall {
                    feature: crate::ai::manifest::Feature::Summary,
                    provider: AiProvider::Custom,
                    model: "retry-test",
                    endpoint: Some(&endpoint),
                    system_prompt: "system",
                    prompt: "private",
                    max_tokens: 100,
                    temperature: 0.2,
                    schema: None,
                    api_key: "key",
                }),
            )
            .await
            .unwrap();
        assert_eq!(result, "Recovered");
        assert_eq!(*calls.lock().unwrap(), 2);
        let snapshot = runtime.snapshot().unwrap();
        assert_eq!(snapshot.records.len(), 2);
        assert_eq!(snapshot.records[1].outcome, "http_429");
        assert_eq!(snapshot.records[0].outcome, "received");
        assert!(snapshot.charged_usd > 0.00001); // failed-attempt billing stays reserved
        server.abort();
    }

    #[test]
    fn only_openrouter_requests_ask_for_reported_cost() {
        let request = |provider| ProviderRequest {
            context: None,
            feature: crate::ai::manifest::Feature::Connection,
            provider,
            model: "m",
            system_prompt: "s",
            prompt: "p",
            max_tokens: 10,
            temperature: 0.1,
            reasoning: Reasoning::Default,
        };
        assert_eq!(
            openai_body(&request(AiProvider::OpenRouter), None)["usage"],
            json!({"include": true})
        );
        assert!(openai_body(&request(AiProvider::OpenAi), None)
            .get("usage")
            .is_none());
    }

    #[test]
    fn completion_status_is_checked_before_parsing_for_both_protocols() {
        for (protocol, payload) in [
            (
                ApiProtocol::OpenAiCompatible,
                json!({"choices":[{"message":{"content":null},"finish_reason":"length"}]}),
            ),
            (
                ApiProtocol::Anthropic,
                json!({"content":[],"stop_reason":"max_tokens"}),
            ),
        ] {
            assert_eq!(
                parse_provider_reply(protocol, &payload.to_string())
                    .err()
                    .unwrap(),
                CUT_OFF_REPLY_ERROR
            );
        }
        for (protocol, payload) in [
            (
                ApiProtocol::OpenAiCompatible,
                json!({"choices":[{"message":{"content":"Partial private text"},"finish_reason":"content_filter"}]}),
            ),
            (
                ApiProtocol::Anthropic,
                json!({"content":[{"type":"text","text":"Private refusal"}],"stop_reason":"refusal"}),
            ),
        ] {
            assert_eq!(
                parse_provider_reply(protocol, &payload.to_string())
                    .err()
                    .unwrap(),
                "The AI provider declined the request"
            );
        }
        assert_eq!(parse_openai_content(r#"{"choices":[{"message":{"content":"partial text","refusal":"private refusal"}}]}"#).unwrap_err(), "The AI provider declined the request");
        // Serde errors can quote offending values; never propagate those strings.
        let error =
            parse_openai_content(r#"{"choices":[{"message":{"content":42,"refusal":"private"}}]}"#)
                .unwrap_err();
        assert_eq!(error, "The AI provider returned an invalid response");
    }

    #[tokio::test]
    async fn truncated_replies_are_rejected_even_when_the_content_is_valid() {
        async fn handler(Json(payload): Json<serde_json::Value>) -> Json<serde_json::Value> {
            Json(
                json!({"choices": [{"message": {"content": payload["model"]}, "finish_reason": "length"}]}),
            )
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(handler)),
            )
            .await
            .unwrap();
        });
        for content in [
            "A plausible but incomplete reply",
            r#"{"answer":"A valid JSON object"}"#,
            "",
        ] {
            let result = call_provider(AiCall {
                feature: crate::ai::manifest::Feature::Connection,
                provider: AiProvider::Custom,
                model: content,
                endpoint: Some(&endpoint),
                system_prompt: "system",
                prompt: "private email",
                max_tokens: 100,
                temperature: 0.2,
                schema: None,
                api_key: "key",
            })
            .await;
            assert_eq!(result.unwrap_err(), CUT_OFF_REPLY_ERROR);
        }
        server.abort();
    }

    #[tokio::test]
    async fn a_connection_test_cut_off_at_its_one_token_budget_still_succeeds() {
        // Reasoning models spend the single token before answering, so the
        // provider reports a cut-off; it still accepted the endpoint, key,
        // and model. A refusal is a real failure and stays one.
        async fn handler(Json(payload): Json<serde_json::Value>) -> Json<serde_json::Value> {
            let finish_reason = if payload["model"] == "refuses" {
                "content_filter"
            } else {
                "length"
            };
            Json(json!({"choices": [{"message": {"content": ""}, "finish_reason": finish_reason}]}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(handler)),
            )
            .await
            .unwrap();
        });
        test_connection(AiProvider::Custom, "reasons", Some(&endpoint), "key")
            .await
            .unwrap();
        assert_eq!(
            test_connection(AiProvider::Custom, "refuses", Some(&endpoint), "key")
                .await
                .unwrap_err(),
            "The AI provider declined the request"
        );
        server.abort();
    }

    #[tokio::test]
    async fn provider_error_bodies_are_never_exposed() {
        async fn handler() -> (axum::http::StatusCode, &'static str) {
            (
                axum::http::StatusCode::BAD_REQUEST,
                "private email, profile, and secret key",
            )
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(handler)),
            )
            .await
            .unwrap();
        });
        let error = call_provider(AiCall {
            feature: crate::ai::manifest::Feature::Connection,
            provider: AiProvider::Custom,
            model: "test",
            endpoint: Some(&endpoint),
            system_prompt: "system",
            prompt: "private email",
            max_tokens: 100,
            temperature: 0.2,
            schema: None,
            api_key: "key",
        })
        .await
        .unwrap_err();
        assert_eq!(error, "AI provider returned 400 Bad Request");
        server.abort();
    }

    #[test]
    fn a_reply_stopped_at_the_token_limit_counts_as_cut_off() {
        let reply = |stop: Option<&str>| ProviderReply {
            content: String::new(),
            stop_reason: stop.map(str::to_string),
        };
        assert!(reply_was_cut_off(&reply(Some("length"))));
        assert!(reply_was_cut_off(&reply(Some("max_tokens"))));
        for stop in [Some("stop"), Some("end_turn"), Some("tool_use"), None] {
            assert!(!reply_was_cut_off(&reply(stop)), "{stop:?}");
        }
    }
}
