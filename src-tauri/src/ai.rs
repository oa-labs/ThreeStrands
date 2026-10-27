//! AI features. Each sends bounded mail content to the provider the user
//! chose and validates what comes back before the app shows it.

mod brief;
mod chat;
mod compose;
mod contact;
mod context;
pub mod find_times;
mod output;
mod proposals;
mod provider;
#[cfg(test)]
mod test_support;
mod usage;

pub use brief::{
    analyze, brief, brief_focus_start, summarize, AnalyzeRequest, GoalPromptInput, PriorCoverage,
    SummarizeRequest, MAX_PROMPT_GOALS, MAX_PROMPT_USER_ADDRESSES,
};
pub use chat::{chat, chat_search_terms, ChatAttachmentInput, ChatRequest, ChatThreadInput};
pub(crate) use chat::{MAX_CHAT_TURN_CHARS, MAX_MAILBOX_CHAT_THREADS};
pub use compose::{generate_reply, reply_context, review_draft, DraftReviewRequest, DraftReviewResult};
pub(crate) use compose::{MAX_WRITING_PROFILE_ABOUT_CHARS, MAX_WRITING_PROFILE_INSTRUCTIONS_CHARS};
pub use contact::{enrich_contact, ContactEnrichmentRequest, ContactEnrichmentResult, ContactMessageInput};
pub use context::{sent_message_count, ActionMessageInput, ThreadMessageInput};
pub use provider::{
    configured, get_key, list_models, set, test_connection, AiProvider, ModelInfo, Reasoning,
};
pub use usage::{set_usage_recorder, with_purpose, RequestPurpose};

#[cfg(test)]
mod tests {
    use crate::ai::brief::{
        action_output_schema, analyze, brief, brief_output_schema, build_summary_prompt, summarize,
        AnalyzeRequest, SummarizeRequest,
    };
    use crate::ai::chat::{
        build_chat_prompt, chat, chat_output_schema, ChatRequest, ChatThreadInput,
    };
    use crate::ai::compose::{generate_reply, reply_context};
    use crate::ai::contact::{
        contact_output_schema, enrich_contact, CONTACT_FIELDS, ContactEnrichmentRequest,
        ContactMessageInput,
    };
    use crate::ai::context::{
        action_context, ActionMessageInput, MAX_BODY_CHARS, MAX_MESSAGES, ThreadMessageInput,
    };
    use crate::ai::provider::{
        AiProvider, REQUEST_TIMEOUT, Reasoning, STRUCTURED_OUTPUT_TOKENS, TEXT_OUTPUT_TOKENS,
    };
    use crate::ai::test_support::chat_request;
    use crate::models::{ActionProposal, ContactProfile, ReplyAssistContext, ReplyAssistMessage};
    use serde_json::json;
    use axum::{extract::State, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    /// Strict structured output rejects schemas whose objects leave a property
    /// optional or allow extra keys, and a schema that drifts from the serde
    /// types would make every constrained reply fail validation.
    #[test]
    fn output_schemas_are_strict_and_match_the_validated_types() {
        fn assert_strict(schema: &serde_json::Value) {
            if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                assert_eq!(schema["additionalProperties"], false, "{schema}");
                let mut required = schema["required"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect::<Vec<_>>();
                let mut keys = properties.keys().map(String::as_str).collect::<Vec<_>>();
                required.sort_unstable();
                keys.sort_unstable();
                assert_eq!(required, keys, "{schema}");
                properties.values().for_each(assert_strict);
            }
            if let Some(items) = schema.get("items") {
                assert_strict(items);
            }
            if let Some(variants) = schema.get("anyOf").and_then(|v| v.as_array()) {
                variants.iter().for_each(assert_strict);
            }
        }
        /// Builds the instance a strict provider would emit when choosing the
        /// first enum value and null for every nullable property.
        fn sample(schema: &serde_json::Value) -> serde_json::Value {
            if let Some(values) = schema.get("enum").and_then(|v| v.as_array()) {
                return values[0].clone();
            }
            if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                return properties
                    .iter()
                    .map(|(key, value)| (key.clone(), sample(value)))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
            }
            match &schema["type"] {
                serde_json::Value::Array(types) if types.contains(&json!("null")) => {
                    serde_json::Value::Null
                }
                kind if kind == "array" => json!([]),
                kind if kind == "number" => json!(0.5),
                _ => json!("text"),
            }
        }

        let action = action_output_schema().schema;
        assert_strict(&action);
        let variants = action["properties"]["proposals"]["items"]["anyOf"]
            .as_array()
            .unwrap();
        assert_eq!(variants.len(), 2);
        for variant in variants {
            serde_json::from_value::<ActionProposal>(sample(variant))
                .unwrap_or_else(|error| panic!("{error}: {variant}"));
        }

        let chat = chat_output_schema().schema;
        assert_strict(&chat);
        assert_eq!(
            chat["properties"]["proposals"],
            action["properties"]["proposals"]
        );

        let brief = brief_output_schema().schema;
        assert_strict(&brief);
        assert_eq!(
            brief["properties"]["proposals"],
            action["properties"]["proposals"]
        );

        let contact = contact_output_schema(&CONTACT_FIELDS).schema;
        assert_strict(&contact);
        assert_eq!(
            contact["properties"]["suggestions"]["items"]["properties"]["field"]["enum"],
            json!(CONTACT_FIELDS)
        );
    }

    fn quoting_thread() -> Vec<ThreadMessageInput> {
        let original = "Can you check whether the nightly feed import still fails for the west region accounts?";
        vec![
            ThreadMessageInput {
                sender: "a@example.com".into(),
                sent_at: "2026-10-01T09:00:00Z".into(),
                body_text: format!("{original}\n\nAshlyn"),
            },
            ThreadMessageInput {
                sender: "b@example.com".into(),
                sent_at: "2026-10-02T09:00:00Z".into(),
                body_text: format!("It is fixed now.\n\nOn Thu, Oct 1, 2026 at 9:00 AM, A <a@example.com> wrote:\n> {original}\n>\n> Ashlyn"),
            },
        ]
    }

    #[test]
    fn every_thread_payload_drops_quoted_history_the_model_already_has() {
        let messages = quoting_thread();

        let summary = build_summary_prompt("Feed", &messages, None);
        assert_eq!(summary.matches("nightly feed import").count(), 1);
        assert!(summary.contains("It is fixed now."));
        assert!(!summary.contains("wrote:"));

        let context = reply_context("Feed".into(), quoting_thread());
        assert_eq!(context.messages[1].body_text, "It is fixed now.");
        // Generating a reply re-applies the bounds to the reviewed context; the
        // result must not change.
        let reapplied = reply_context("Feed".into(), context.messages.iter().map(|message| ThreadMessageInput {
            sender: message.sender.clone(),
            sent_at: message.sent_at.clone(),
            body_text: message.body_text.clone(),
        }).collect());
        assert_eq!(reapplied.messages[1].body_text, "It is fixed now.");

        let actions: Vec<ActionMessageInput> = messages.iter().enumerate().map(|(index, message)| ActionMessageInput {
            id: format!("m{index}"),
            sender: message.sender.clone(),
            recipients: Vec::new(),
            sent_at: message.sent_at.clone(),
            body_text: message.body_text.clone(),
        }).collect();
        assert_eq!(action_context(&actions, None)[1].body_text, "It is fixed now.");

        let mut request = chat_request("Is it fixed?");
        request.other_threads = vec![ChatThreadInput { thread_id: "other".into(), subject: "Feed".into(), messages }];
        let prompt: serde_json::Value = serde_json::from_str(&build_chat_prompt(&request, &[]).unwrap()).unwrap();
        assert_eq!(prompt["otherThreads"][0]["messages"][1]["bodyText"], "It is fixed now.");
    }

    #[test]
    fn keeps_quoted_text_the_model_would_not_otherwise_receive() {
        // The original sits outside the message window, so its quote is the only copy sent.
        let mut messages = quoting_thread();
        let reply = messages.pop().unwrap();
        let reply_body = reply.body_text.clone();
        messages.extend((0..MAX_MESSAGES - 1).map(|index| ThreadMessageInput {
            sender: "c@example.com".into(),
            sent_at: format!("2026-10-01T1{index}:00:00Z"),
            body_text: format!("Unrelated note {index}."),
        }));
        messages.push(reply);
        let context = reply_context("Feed".into(), messages);
        assert_eq!(context.messages.last().unwrap().body_text, reply_body);

        // The quoted tail of an earlier message lies past that message's cap.
        let tail = "The west region import fails only when the partner file arrives after midnight.";
        let long = format!("{}\n{tail}", "x".repeat(MAX_BODY_CHARS));
        let quote_of_tail = format!("Noted.\n\nOn Mon, A wrote:\n> {tail}");
        let context = reply_context("Feed".into(), vec![
            ThreadMessageInput { sender: "a@example.com".into(), sent_at: "1".into(), body_text: long },
            ThreadMessageInput { sender: "b@example.com".into(), sent_at: "2".into(), body_text: quote_of_tail.clone() },
        ]);
        assert_eq!(context.messages[1].body_text, quote_of_tail);
    }

    #[tokio::test]
    async fn every_feature_requests_its_reasoning_headroom_output_budget() {
        type Budgets = Arc<Mutex<Vec<serde_json::Value>>>;
        async fn respond(
            State(budgets): State<Budgets>,
            Json(payload): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            budgets.lock().unwrap().push(payload["max_tokens"].clone());
            Json(json!({"choices":[{"message":{"content":"[]"}}]}))
        }
        let budgets: Budgets = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/chat/completions", post(respond))
            .with_state(Arc::clone(&budgets));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let take = || std::mem::take(&mut *budgets.lock().unwrap());
        let action_message = || ActionMessageInput {
            id: "m1".into(),
            sender: "jane@example.com".into(),
            recipients: Vec::new(),
            sent_at: "2026-09-25T10:00:00Z".into(),
            body_text: "Can we meet Friday?".into(),
        };
        let analyze_request = || AnalyzeRequest {
            provider: AiProvider::Custom,
            model: "test".into(),
            endpoint: Some(endpoint.clone()),
            subject: "Hello".into(),
            messages: vec![action_message()],
            current_time: "2026-09-25T10:00:00Z".into(),
            user_time_zone: "UTC".into(),
            user_addresses: vec![],
            goals: vec![],
            focus_from: None,
        };

        let _ = summarize(
            SummarizeRequest {
                provider: AiProvider::Custom,
                model: "test".into(),
                endpoint: Some(endpoint.clone()),
                reasoning: Reasoning::Default,
                subject: "Hello".into(),
                messages: vec![ThreadMessageInput {
                    sender: "jane@example.com".into(),
                    sent_at: "2026-09-25".into(),
                    body_text: "Hello".into(),
                }],
                focus_from: None,
            },
            "key",
        )
        .await;
        assert_eq!(take(), vec![json!(TEXT_OUTPUT_TOKENS)], "summary");

        let _ = generate_reply(
            &ReplyAssistContext {
                subject: "Hello".into(),
                messages: vec![ReplyAssistMessage {
                    sender: "jane@example.com".into(),
                    sent_at: "2026-09-25".into(),
                    body_text: "Hello".into(),
                }],
            },
            "",
            AiProvider::Custom,
            "test",
            Some(&endpoint),
            Reasoning::Default,
            "key",
        )
        .await;
        assert_eq!(take(), vec![json!(TEXT_OUTPUT_TOKENS)], "reply");

        let _ = analyze(analyze_request(), "key").await;
        assert_eq!(take(), vec![json!(STRUCTURED_OUTPUT_TOKENS)], "analysis");

        let _ = brief(analyze_request(), "key").await;
        assert_eq!(take(), vec![json!(STRUCTURED_OUTPUT_TOKENS)], "brief");

        let _ = chat(
            ChatRequest {
                provider: AiProvider::Custom,
                model: "test".into(),
                endpoint: Some(endpoint.clone()),
                question: "When is the meeting?".into(),
                history: Vec::new(),
                subject: "Hello".into(),
                messages: vec![action_message()],
                open_tasks: Vec::new(),
                other_threads: Vec::new(),
                attachments: Vec::new(),
                proposals_allowed: false,
                current_time: "2026-09-25T10:00:00Z".into(),
                user_time_zone: "UTC".into(),
            },
            "key",
        )
        .await;
        assert_eq!(take(), vec![json!(STRUCTURED_OUTPUT_TOKENS)], "chat");

        let _ = enrich_contact(
            ContactEnrichmentRequest {
                provider: AiProvider::Custom,
                model: "test".into(),
                endpoint: Some(endpoint.clone()),
                reasoning: Reasoning::Default,
                profile: ContactProfile {
                    id: "contact-jane".into(),
                    display_name: None,
                    role: None,
                    company: None,
                    location: None,
                    bio: None,
                    notes: None,
                    links: Vec::new(),
                    photo_data: None,
                    favorite: false,
                    addresses: vec!["jane@example.com".into()],
                    sent_count: 0,
                    received_count: 0,
                    last_interacted_at: None,
                    birthday: None,
                    keep_in_touch: Default::default(),
                    keep_in_touch_due_at: None,
                },
                messages: vec![ContactMessageInput {
                    id: "m1".into(),
                    thread_id: "t1".into(),
                    sender: "jane@example.com".into(),
                    sent_at: "2026-09-25".into(),
                    subject: "Hello".into(),
                    body_text: "Hello".into(),
                    from_contact: true,
                    is_thread_starter: true,
                }],
                search_more: false,
                empty_fields: None,
            },
            "key",
        )
        .await;
        assert_eq!(take(), vec![json!(STRUCTURED_OUTPUT_TOKENS)], "contact");
        server.abort();

        // A reasoning model filling the larger budget at a modest ~150
        // tokens per second must finish before the request times out.
        assert!(REQUEST_TIMEOUT.as_secs() * 150 >= STRUCTURED_OUTPUT_TOKENS as u64);
        assert!(TEXT_OUTPUT_TOKENS <= STRUCTURED_OUTPUT_TOKENS);
    }
}
