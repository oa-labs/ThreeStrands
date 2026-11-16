//! Synthetic, repeatable quality checks. Replay validates the evaluation wiring;
//! only explicit live runs measure a model. No mailbox/database/keychain access.
use super::{
    manifest::{fingerprint, Feature},
    provider::{AiProvider, ApiProtocol, Reasoning},
    *,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub const SUITE_VERSION: u32 = 1;
pub const GRADER_VERSION: u32 = 1;
pub const MAX_CASES: usize = 50;
tokio::task_local! { static REPLAY: Value; }
/// Internal task-local replay only. It cannot be enabled by mail, provider output,
/// frontend arguments, or application settings.
pub(super) fn replay_response(protocol: ApiProtocol) -> Option<String> {
    REPLAY
        .try_with(|value| {
            let text = if let Some(text) = value.as_str() {
                text.to_string()
            } else {
                value.to_string()
            };
            match protocol {
                ApiProtocol::Anthropic => {
                    json!({"content":[{"type":"text","text":text}],"stop_reason":"end_turn"})
                }
                _ => json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}]}),
            }
            .to_string()
        })
        .ok()
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Message {
    id: String,
    sender: String,
    sent_at: String,
    body_text: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Check {
    path: String,
    op: String,
    value: Value,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    id: String,
    feature: Feature,
    subject: String,
    messages: Vec<Message>,
    #[serde(default)]
    question: String,
    #[serde(default)]
    instruction: String,
    #[serde(default)]
    profile: Value,
    #[serde(default)]
    attachment: Option<String>,
    #[serde(default)]
    attachment_truncated: bool,
    #[serde(default)]
    other_thread: Option<String>,
    replay: Value,
    checks: Vec<Check>,
}
#[derive(Clone)]
pub struct Options {
    pub live: bool,
    pub provider: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub case_filter: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub path: String,
    pub op: String,
    pub passed: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseResult {
    pub id: String,
    pub feature: String,
    pub passed: bool,
    pub checks: Vec<CheckResult>,
    pub error: Option<String>,
    pub latency_ms: Option<u128>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reported_cost_usd: Option<f64>,
    pub provenance: Vec<requests::RequestRecord>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub suite_version: u32,
    pub grader_version: u32,
    pub suite_revision: String,
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub passed: usize,
    pub failed: usize,
    pub cases: Vec<CaseResult>,
}
fn grade(output: &Value, check: &Check) -> bool {
    let Some(actual) = output.pointer(&check.path) else {
        return false;
    };
    match check.op.as_str() {
        "eq" => actual == &check.value,
        "count" => actual
            .as_array()
            .is_some_and(|items| Some(items.len() as u64) == check.value.as_u64()),
        "contains" => actual
            .as_str()
            .zip(check.value.as_str())
            .is_some_and(|(a, b)| a.to_lowercase().contains(&b.to_lowercase())),
        "not_contains" => actual
            .as_str()
            .zip(check.value.as_str())
            .is_some_and(|(a, b)| !a.to_lowercase().contains(&b.to_lowercase())),
        "one_of" => actual
            .as_str()
            .zip(check.value.as_array())
            .is_some_and(|(a, options)| {
                options
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|b| a.to_lowercase().contains(&b.to_lowercase()))
            }),
        _ => false,
    }
}
fn load_cases() -> Result<Vec<Case>, String> {
    let cases: Vec<Case> = serde_json::from_str(include_str!("../../../evals/ai/cases.json"))
        .map_err(|_| "Invalid evaluation cases")?;
    let mut ids = std::collections::HashSet::new();
    if cases.is_empty() || cases.len() > MAX_CASES {
        return Err("Invalid evaluation case count".into());
    }
    for case in &cases {
        if !ids.insert(&case.id)
            || case.checks.is_empty()
            || case.messages.is_empty()
            || matches!(case.feature, Feature::Connection)
        {
            return Err("Invalid evaluation case contract".into());
        }
        for check in &case.checks {
            if !check.path.starts_with('/')
                || !["eq", "count", "contains", "not_contains", "one_of"]
                    .contains(&check.op.as_str())
            {
                return Err("Invalid evaluation check".into());
            }
        }
    }
    Ok(cases)
}
fn actions(case: &Case) -> Vec<ActionMessageInput> {
    case.messages
        .iter()
        .map(|m| ActionMessageInput {
            id: m.id.clone(),
            sender: m.sender.clone(),
            recipients: vec!["me@example.test".into()],
            sent_at: m.sent_at.clone(),
            body_text: m.body_text.clone(),
        })
        .collect()
}
fn messages(case: &Case) -> Vec<ThreadMessageInput> {
    case.messages
        .iter()
        .map(|m| ThreadMessageInput {
            sender: m.sender.clone(),
            sent_at: m.sent_at.clone(),
            body_text: m.body_text.clone(),
        })
        .collect()
}
const NOW: &str = "2026-10-28T10:00:00-04:00";
const ZONE: &str = "America/New_York";
async fn execute(case: &Case, provider: AiProvider, options: &Options) -> Result<Value, String> {
    let key = options.api_key.as_deref().unwrap_or("replay-no-key");
    let endpoint = options.endpoint.clone();
    let model = options.model.clone();
    let analysis = || AnalyzeRequest {
        provider,
        model: model.clone(),
        endpoint: endpoint.clone(),
        subject: case.subject.clone(),
        messages: actions(case),
        current_time: NOW.into(),
        user_time_zone: ZONE.into(),
        user_addresses: vec!["me@example.test".into()],
        user_names: vec!["Alex".into()],
        goals: vec![],
        focus_from: None,
    };
    match case.feature {
        Feature::Summary => Ok(
            json!({"summary":summarize(SummarizeRequest {provider,model,endpoint,reasoning:Reasoning::Default,subject:case.subject.clone(),messages:messages(case),focus_from:None},key).await?}),
        ),
        Feature::Brief => {
            let brief = Box::pin(brief(analysis(), key)).await?;
            Ok(json!({"summary":brief.summary,"analysis":brief.analysis}))
        }
        Feature::Suggestions => Ok(json!({"analysis":analyze(analysis(),key).await?})),
        Feature::Chat => {
            let attachments = case
                .attachment
                .as_ref()
                .map(|text| super::chat::ChatAttachmentInput {
                    message_id: "m1".into(),
                    filename: "selected.txt".into(),
                    mime_type: "text/plain".into(),
                    text: text.clone(),
                    truncated: case.attachment_truncated,
                })
                .into_iter()
                .collect();
            let other_threads = case
                .other_thread
                .as_ref()
                .map(|body| ChatThreadInput {
                    thread_id: "other-thread".into(),
                    subject: "Earlier decision".into(),
                    messages: vec![ThreadMessageInput {
                        sender: "colleague@example.test".into(),
                        sent_at: NOW.into(),
                        body_text: body.clone(),
                    }],
                })
                .into_iter()
                .collect();
            let answer = Box::pin(chat(
                ChatRequest {
                    provider,
                    model,
                    endpoint,
                    question: case.question.clone(),
                    history: vec![],
                    subject: case.subject.clone(),
                    messages: actions(case),
                    open_tasks: vec![],
                    other_threads,
                    attachments,
                    proposals_allowed: true,
                    current_time: NOW.into(),
                    user_time_zone: ZONE.into(),
                },
                key,
            ))
            .await?;
            Ok(
                json!({"answer":answer.answer,"analysis":answer.analysis,"sourceThreadIds":answer.source_thread_ids,"availability":answer.availability}),
            )
        }
        Feature::Reply => {
            let context = reply_context(case.subject.clone(), messages(case));
            let profile: ReplyWritingProfile = if case.profile.is_null() {
                Default::default()
            } else {
                serde_json::from_value(case.profile.clone())
                    .map_err(|_| "Invalid fixture profile")?
            };
            Ok(
                json!({"body":generate_reply(&context,&case.instruction,&profile,provider,&model,endpoint.as_deref(),Reasoning::Default,key).await?}),
            )
        }
        Feature::DraftReview => {
            let profile = case.profile.as_object();
            let request = DraftReviewRequest {
                subject: case.subject.clone(),
                body: case.messages[0].body_text.clone(),
                goal: case.instruction.clone(),
                about_you: profile
                    .and_then(|p| p.get("aboutYou"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                review_instructions: profile
                    .and_then(|p| p.get("reviewInstructions"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            };
            serde_json::to_value(
                Box::pin(review_draft(
                    request,
                    provider,
                    &model,
                    endpoint.as_deref(),
                    key,
                ))
                .await?,
            )
            .map_err(|_| "Invalid review result".into())
        }
        Feature::FindTimes => serde_json::to_value(
            Box::pin(find_times::find_times(
                find_times::FindTimesRequest {
                    provider,
                    model,
                    endpoint,
                    subject: case.subject.clone(),
                    messages: actions(case),
                    draft_text: String::new(),
                    instruction: None,
                    current: None,
                    current_time: NOW.into(),
                    user_time_zone: ZONE.into(),
                    user_addresses: vec!["me@example.test".into()],
                },
                key,
            ))
            .await?,
        )
        .map_err(|_| "Invalid time search result".into()),
        Feature::Contact => {
            let profile = crate::models::ContactProfile {
                id: "synthetic-contact".into(),
                display_name: None,
                role: None,
                company: None,
                location: None,
                bio: None,
                notes: None,
                links: vec![],
                photo_data: None,
                favorite: false,
                addresses: vec!["colleague@example.test".into()],
                sent_count: 0,
                received_count: 0,
                last_interacted_at: None,
                birthday: None,
                keep_in_touch: Default::default(),
                keep_in_touch_due_at: None,
            };
            serde_json::to_value(
                Box::pin(enrich_contact(
                    ContactEnrichmentRequest {
                        provider,
                        model,
                        endpoint,
                        reasoning: Reasoning::Default,
                        profile,
                        messages: case
                            .messages
                            .iter()
                            .map(|m| ContactMessageInput {
                                id: m.id.clone(),
                                thread_id: "synthetic-thread".into(),
                                sender: m.sender.clone(),
                                sent_at: m.sent_at.clone(),
                                subject: case.subject.clone(),
                                body_text: m.body_text.clone(),
                                from_contact: m.sender == "colleague@example.test",
                                is_thread_starter: true,
                            })
                            .collect(),
                        search_more: false,
                        empty_fields: None,
                    },
                    key,
                ))
                .await?,
            )
            .map_err(|_| "Invalid contact result".into())
        }
        Feature::Connection => Err("Connection tests are not quality evaluations".into()),
    }
}
pub fn suite_revision(filter: Option<&str>) -> Result<String, String> {
    let cases: Vec<_> = load_cases()?
        .into_iter()
        .filter(|case| filter.is_none_or(|filter| case.id.contains(filter)))
        .collect();
    if cases.is_empty() {
        return Err("No evaluation cases matched".into());
    }
    let contract: Vec<_> = cases
        .iter()
        .map(|case| {
            let mut value = serde_json::to_value(case).unwrap();
            value.as_object_mut().unwrap().remove("replay");
            value
        })
        .collect();
    Ok(fingerprint(&serde_json::to_vec(&contract).unwrap()))
}

pub async fn run(options: Options) -> Result<Report, String> {
    let provider = match options.provider.as_str() {
        "openai" => AiProvider::OpenAi,
        "anthropic" => AiProvider::Anthropic,
        "openrouter" => AiProvider::OpenRouter,
        "fireworks" => AiProvider::Fireworks,
        "custom" => AiProvider::Custom,
        _ => return Err("Unsupported evaluation provider".into()),
    };
    if options.model.trim().is_empty() {
        return Err("An evaluation model is required".into());
    }
    if options.live
        && options
            .api_key
            .as_ref()
            .is_none_or(|key| key.trim().is_empty())
    {
        return Err("Set THREESTRANDS_EVAL_API_KEY for an explicit live run".into());
    }
    if options.live && matches!(provider, AiProvider::Custom) && options.endpoint.is_none() {
        return Err("Custom live evaluations require an endpoint".into());
    }
    let cases: Vec<_> = load_cases()?
        .into_iter()
        .filter(|case| {
            options
                .case_filter
                .as_ref()
                .is_none_or(|filter| case.id.contains(filter))
        })
        .collect();
    if cases.is_empty() {
        return Err("No evaluation cases matched".into());
    }
    let suite_revision = suite_revision(options.case_filter.as_deref())?;
    let events = Arc::new(Mutex::new(Vec::<super::usage::UsageEvent>::new()));
    let sink = events.clone();
    let model = options.model.clone();
    let recorder: super::usage::UsageRecorder = Arc::new(move |event| {
        if event.model == model {
            sink.lock().unwrap().push(event.clone());
        }
    });
    let mut results = Vec::new();
    for case in cases {
        events.lock().unwrap().clear();
        let id = uuid::Uuid::new_v4().to_string();
        let mut purpose = RequestPurpose::new(case.feature.id());
        purpose.control = Some(requests::RequestControl {
            request_id: id.clone(),
            price: None,
        });
        let start = std::time::Instant::now();
        let future = super::usage::with_recorder(
            recorder.clone(),
            with_purpose(purpose, Box::pin(execute(&case, provider, &options))),
        );
        let result = if options.live {
            future.await
        } else {
            REPLAY.scope(case.replay.clone(), future).await
        };
        let checks: Vec<_> = case
            .checks
            .iter()
            .map(|check| CheckResult {
                path: check.path.clone(),
                op: check.op.clone(),
                passed: result.as_ref().is_ok_and(|output| grade(output, check)),
            })
            .collect();
        let usage = events.lock().unwrap().clone();
        let known_tokens = options.live
            && usage
                .iter()
                .any(|event| event.input_tokens > 0 || event.output_tokens > 0);
        let known_cost =
            options.live && !usage.is_empty() && usage.iter().all(|event| event.cost_usd.is_some());
        let provenance = requests::RUNTIME
            .snapshot()?
            .records
            .into_iter()
            .filter(|record| record.request_id == id)
            .collect();
        results.push(CaseResult {
            id: case.id,
            feature: case.feature.id().into(),
            passed: result.is_ok() && checks.iter().all(|check| check.passed),
            checks,
            error: result.err().map(|_| "request_or_validation_failed".into()),
            latency_ms: options.live.then(|| start.elapsed().as_millis()),
            input_tokens: known_tokens.then(|| usage.iter().map(|event| event.input_tokens).sum()),
            output_tokens: known_tokens
                .then(|| usage.iter().map(|event| event.output_tokens).sum()),
            reported_cost_usd: known_cost
                .then(|| usage.iter().filter_map(|event| event.cost_usd).sum()),
            provenance,
        });
    }
    let passed = results.iter().filter(|result| result.passed).count();
    Ok(Report {
        suite_version: SUITE_VERSION,
        grader_version: GRADER_VERSION,
        suite_revision,
        mode: if options.live { "live" } else { "replay" }.into(),
        provider: options.provider,
        model: options.model,
        passed,
        failed: results.len() - passed,
        cases: results,
    })
}
/// Refuse incomparable reports rather than presenting a misleading score delta.
pub fn compare(previous: &Report, current: &Report) -> Result<Value, String> {
    if previous.mode != "live"
        || current.mode != "live"
        || previous.suite_revision != current.suite_revision
        || previous.grader_version != current.grader_version
    {
        return Err("Only live reports with identical cases and graders can be compared".into());
    }
    Ok(
        json!({"previousModel":previous.model,"currentModel":current.model,"passedDelta":current.passed as i64-previous.passed as i64,"failedDelta":current.failed as i64-previous.failed as i64}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graders_reject_missing_values_and_detect_factual_regressions() {
        let output = json!({"answer":"The budget is 12,000.","proposals":[]});
        assert!(grade(
            &output,
            &Check {
                path: "/answer".into(),
                op: "contains".into(),
                value: json!("12,000")
            }
        ));
        assert!(!grade(
            &output,
            &Check {
                path: "/answer".into(),
                op: "contains".into(),
                value: json!("15,000")
            }
        ));
        assert!(!grade(
            &output,
            &Check {
                path: "/missing".into(),
                op: "eq".into(),
                value: Value::Null
            }
        ));
        assert!(grade(
            &output,
            &Check {
                path: "/proposals".into(),
                op: "count".into(),
                value: json!(0)
            }
        ));
    }
    #[tokio::test]
    async fn synthetic_replay_runs_the_production_prompts_and_parsers_without_a_provider() {
        let report = run(Options {
            live: false,
            provider: "custom".into(),
            model: "synthetic-replay".into(),
            endpoint: Some("http://127.0.0.1:1".into()),
            api_key: None,
            case_filter: None,
        })
        .await
        .unwrap();
        assert!(report.cases.len() >= 16);
        assert_eq!(
            report.failed,
            0,
            "{}",
            serde_json::to_string_pretty(&report).unwrap()
        );
        assert!(report.cases.iter().all(|case| case.latency_ms.is_none()
            && case.input_tokens.is_none()
            && !case.provenance.is_empty()));
        assert!(compare(&report, &report).is_err());
    }
}
