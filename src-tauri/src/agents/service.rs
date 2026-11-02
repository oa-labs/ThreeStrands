//! Dispatch only the next persisted action; credentials never enter job state.
use super::{
    policy::*,
    research::*,
    transport::{self, Failure},
    Lease, StepResult,
};
use crate::db::Database;
use std::sync::Arc;

/// Serialize setup snapshots/commits and credential writes, never network I/O.
pub(crate) static SETUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub(crate) fn key(which: &str) -> Result<Option<String>, String> {
    let account = match which {
        "model" => "agents-anthropic-api-key",
        "search" => "agents-exa-api-key",
        _ => return Err("Unknown research credential".into()),
    };
    match keyring::Entry::new("app.threestrands.mail", account)
        .map_err(|_| "Research keychain unavailable")?
        .get_password()
    {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("Research keychain unavailable".into()),
    }
}
pub(crate) fn set_key(which: &str, value: &str) -> Result<(), String> {
    let account = match which {
        "model" => "agents-anthropic-api-key",
        "search" => "agents-exa-api-key",
        _ => return Err("Unknown research credential".into()),
    };
    let entry = keyring::Entry::new("app.threestrands.mail", account)
        .map_err(|_| "Research keychain unavailable")?;
    if value.trim().is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("Research keychain unavailable".into()),
        }
    } else {
        if value.len() > MAX_SECRET {
            return Err("Research key is too long".into());
        }
        entry
            .set_password(value.trim())
            .map_err(|_| "Research keychain unavailable".into())
    }
}
pub(crate) async fn keys() -> Result<(String, String), String> {
    tokio::task::spawn_blocking(|| {
        Ok((
            key("model")?.ok_or("missing_credentials")?,
            key("search")?.ok_or("missing_credentials")?,
        ))
    })
    .await
    .map_err(|_| "Research keychain unavailable".to_string())?
}
static SEARCH_LIMITER: tokio::sync::Mutex<Option<tokio::time::Instant>> =
    tokio::sync::Mutex::const_new(None);
async fn search_slot() {
    let mut slot = SEARCH_LIMITER.lock().await;
    if let Some(last) = *slot {
        tokio::time::sleep_until(last + std::time::Duration::from_secs(SEARCH_INTERVAL_SECONDS))
            .await;
    }
    *slot = Some(tokio::time::Instant::now());
}
pub(crate) async fn check_connection(model_key: &str, search_key: &str) -> transport::Result<()> {
    let model = transport::provider("verify", model_key, None, 1).await?;
    if model["id"] != MODEL {
        return Err(Failure::Fail("unsupported_model"));
    }
    search_slot().await;
    let result = transport::provider(
        "search",
        search_key,
        Some(&transport::search_request("Exa official website")),
        1,
    )
    .await?;
    transport::parse_search(&result, chrono::Utc::now().timestamp())?;
    Ok(())
}
pub(crate) struct Production {
    pub database: Arc<Database>,
}
#[async_trait::async_trait]
impl super::runner::Executor for Production {
    async fn execute(&self, lease: &Lease) -> Result<StepResult, ()> {
        let db = self.database.clone();
        let id = lease.run.task_id.clone();
        let workflow = tokio::task::spawn_blocking(move || db.agent_workflow(&id))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        if workflow == WORKFLOW {
            return Ok(super::fixture_step(lease.run.checkpoint));
        }
        let result = self.step(lease).await;
        Ok(match result {
            Ok(s) => StepResult::Research(s),
            Err(f) => f.step(),
        })
    }
}
impl Production {
    async fn reserve(&self, l: &Lease, kind: &'static str) -> transport::Result<Work> {
        let db = self.database.clone();
        let l = l.clone();
        tokio::task::spawn_blocking(move || {
            db.reserve_research(&l, kind, chrono::Utc::now().timestamp())
        })
        .await
        .map_err(|_| Failure::Pause("interrupted"))?
        .map_err(|_| Failure::Pause("budget_or_configuration"))
    }
    async fn step(&self, l: &Lease) -> transport::Result<ResearchStep> {
        let now = chrono::Utc::now().timestamp();
        let db = self.database.clone();
        let lease = l.clone();
        let w = tokio::task::spawn_blocking(move || db.research_work(&lease, now))
            .await
            .map_err(|_| Failure::Pause("interrupted"))?
            .map_err(|_| Failure::Pause("configuration_changed"))?
            .ok_or(Failure::Pause("configuration_changed"))?;
        if now >= w.expires_at {
            return Err(Failure::Pause("evidence_expired"));
        }
        match w.next.as_str() {
            "count" | "model" => {
                let count = w.next == "count";
                let body = w.request(count);
                if serde_json::to_vec(&body)
                    .map_err(|_| Failure::Fail("context_invalid"))?
                    .len()
                    > MAX_PROTOCOL_BYTES
                {
                    return Err(Failure::Pause("context_limit"));
                }
                let model_key = tokio::task::spawn_blocking(|| key("model"))
                    .await
                    .map_err(|_| Failure::Pause("missing_credentials"))?
                    .map_err(|_| Failure::Pause("missing_credentials"))?
                    .ok_or(Failure::Pause("missing_credentials"))?;
                self.reserve(l, if count { "count" } else { "model" })
                    .await?;
                let v = transport::provider(
                    if count { "count" } else { "model" },
                    &model_key,
                    Some(&body),
                    l.run.attempts,
                )
                .await?;
                if count {
                    Ok(ResearchStep::Count(
                        v["input_tokens"]
                            .as_u64()
                            .ok_or(Failure::Fail("response_invalid"))?,
                    ))
                } else {
                    if let Some(call) = parse_model(&v).map_err(Failure::Fail)? {
                        validate_call(&w, &call).map_err(Failure::Fail)?;
                    }
                    Ok(ResearchStep::Model(v))
                }
            }
            "action" => {
                let call = w
                    .pending
                    .as_ref()
                    .ok_or(Failure::Fail("protocol_invalid"))?;
                // validate_call already ran before the assistant block was committed.
                match call.name.as_str() {
                    "search_web" => {
                        let q = call.input["query"]
                            .as_str()
                            .ok_or(Failure::Fail("invalid_tool_arguments"))?;
                        if !w.queries().iter().any(|a| a == q) {
                            return Err(Failure::Fail("query_not_granted"));
                        }
                        let (_, search_key) = keys()
                            .await
                            .map_err(|_| Failure::Pause("missing_credentials"))?;
                        search_slot().await;
                        self.reserve(l, "search").await?;
                        let v = transport::provider(
                            "search",
                            &search_key,
                            Some(&transport::search_request(q)),
                            l.run.attempts,
                        )
                        .await?;
                        let (s, cost) = transport::parse_search(&v, now)?;
                        Ok(ResearchStep::Sources(s, cost))
                    }
                    "read_page" => {
                        let id = call.input["source_id"]
                            .as_str()
                            .ok_or(Failure::Fail("invalid_tool_arguments"))?;
                        let source = w
                            .sources
                            .iter()
                            .find(|s| s.id == id)
                            .ok_or(Failure::Fail("unknown_source"))?;
                        self.reserve(l, "page").await?;
                        let db = self.database.clone();
                        let lease = l.clone();
                        let s = transport::page(source, l.run.attempts, move || {
                            db.research_work(&lease, chrono::Utc::now().timestamp())
                                .ok()
                                .flatten()
                                .is_some()
                        })
                        .await?;
                        Ok(ResearchStep::Page(s))
                    }
                    "ask_user" => Ok(ResearchStep::Question(
                        call.input["question"]
                            .as_str()
                            .ok_or(Failure::Fail("invalid_tool_arguments"))?
                            .into(),
                    )),
                    "publish_artifact" => {
                        let r: Report = serde_json::from_value(call.input.clone())
                            .map_err(|_| Failure::Fail("invalid_report"))?;
                        validate_report(&r, &w.sources).map_err(Failure::Fail)?;
                        Ok(ResearchStep::Publish(r))
                    }
                    _ => Err(Failure::Fail("tool_not_granted")),
                }
            }
            _ => Err(Failure::Fail("protocol_invalid")),
        }
    }
}
