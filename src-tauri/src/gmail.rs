use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{
    sync::Mutex,
    time::{sleep, Instant},
};

use crate::{auth::GoogleAuth, mime::GmailMessage, models::Label};

const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Gmail history cursor is invalid or expired")]
    InvalidCursor,
    #[error("Gmail rate limit persisted after retries")]
    RateLimited,
    #[error("Gmail object was not found")]
    NotFound,
    #[error("{0}")]
    Other(String),
}

pub type ProviderResult<T> = Result<T, ProviderError>;

#[derive(Debug)]
pub struct ThreadPage {
    pub thread_ids: Vec<String>,
    pub next_page_token: Option<String>,
}

#[derive(Debug)]
pub struct HistoryPage {
    pub thread_ids: Vec<String>,
    pub history_id: String,
    pub next_page_token: Option<String>,
}

#[async_trait]
pub trait GmailProvider: Send + Sync {
    async fn profile_history_id(&self) -> ProviderResult<String>;
    async fn list_threads(&self, page: Option<&str>) -> ProviderResult<ThreadPage>;
    async fn get_thread(&self, id: &str) -> ProviderResult<Vec<GmailMessage>>;
    async fn history(&self, cursor: &str, page: Option<&str>) -> ProviderResult<HistoryPage>;
    async fn modify_thread(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()>;
    async fn list_labels(&self) -> ProviderResult<Vec<Label>>;
    async fn create_label(&self, name: &str) -> ProviderResult<Label>;
    async fn update_label(&self, id: &str, name: &str) -> ProviderResult<Label>;
    async fn delete_label(&self, id: &str) -> ProviderResult<()>;
}

#[derive(Clone)]
pub struct GmailClient {
    http: reqwest::Client,
    auth: GoogleAuth,
    next_thread_fetch: Arc<Mutex<Instant>>,
}

impl GmailClient {
    pub fn new(auth: GoogleAuth) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth,
            next_thread_fetch: Arc::new(Mutex::new(Instant::now())),
        }
    }

    async fn pace_thread_fetch(&self) {
        // `threads.get` costs 10 quota units. Four hundred calls per minute
        // consume 4,000 of Gmail's 6,000 per-user units, leaving headroom for
        // lists, history, labels, mutations, and another active client.
        const INTERVAL: Duration = Duration::from_millis(150);
        let mut next = self.next_thread_fetch.lock().await;
        let now = Instant::now();
        if *next > now {
            sleep(*next - now).await;
        }
        *next = Instant::now() + INTERVAL;
    }

    async fn request(&self, method: Method, url: String) -> ProviderResult<RequestBuilder> {
        let token = self
            .auth
            .access_token()
            .await
            .map_err(ProviderError::Other)?;
        Ok(self.http.request(method, url).bearer_auth(token))
    }

    async fn send(
        &self,
        request: RequestBuilder,
        history: bool,
    ) -> ProviderResult<reqwest::Response> {
        for attempt in 0..=4 {
            let cloned = request
                .try_clone()
                .ok_or_else(|| ProviderError::Other("Unable to retry Gmail request".into()))?;
            let response = cloned
                .send()
                .await
                .map_err(|error| ProviderError::Other(error.to_string()))?;
            if response.status().is_success() {
                return Ok(response);
            }
            if history && response.status() == StatusCode::NOT_FOUND {
                return Err(ProviderError::InvalidCursor);
            }
            if response.status() == StatusCode::NOT_FOUND {
                return Err(ProviderError::NotFound);
            }
            let status = response.status();
            let retry_after = retry_after(&response);
            let body = response.text().await.unwrap_or_default();
            let quota_limited = status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN && is_quota_error(&body));
            if quota_limited
                || status == StatusCode::SERVICE_UNAVAILABLE
                || status.is_server_error()
            {
                if attempt == 4 {
                    return Err(ProviderError::RateLimited);
                }
                let delay = retry_after.unwrap_or_else(|| retry_delay(attempt, quota_limited));
                sleep(delay).await;
                continue;
            }
            return Err(ProviderError::Other(format!(
                "Gmail returned {status}: {body}"
            )));
        }
        Err(ProviderError::RateLimited)
    }

    async fn json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        history: bool,
    ) -> ProviderResult<T> {
        self.send(request, history)
            .await?
            .json()
            .await
            .map_err(|error| ProviderError::Other(error.to_string()))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    history_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadList {
    #[serde(default)]
    threads: Vec<ThreadRef>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct ThreadRef {
    id: String,
}

#[derive(Deserialize)]
struct GmailThread {
    #[serde(default)]
    messages: Vec<GmailMessage>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryList {
    #[serde(default)]
    history: Vec<HistoryRecord>,
    history_id: String,
    next_page_token: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryRecord {
    #[serde(default)]
    messages: Vec<HistoryMessage>,
    #[serde(default)]
    messages_added: Vec<HistoryContainer>,
    #[serde(default)]
    messages_deleted: Vec<HistoryContainer>,
    #[serde(default)]
    labels_added: Vec<HistoryContainer>,
    #[serde(default)]
    labels_removed: Vec<HistoryContainer>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryContainer {
    message: HistoryMessage,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryMessage {
    thread_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModifyRequest<'a> {
    add_label_ids: &'a [String],
    remove_label_ids: &'a [String],
}

#[derive(Deserialize)]
struct LabelList {
    #[serde(default)]
    labels: Vec<GmailLabel>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GmailLabel {
    #[serde(default)]
    id: String,
    name: String,
    #[serde(default)]
    r#type: String,
}

impl From<GmailLabel> for Label {
    fn from(value: GmailLabel) -> Self {
        Self {
            id: value.id,
            name: value.name,
            kind: value.r#type.to_ascii_lowercase(),
        }
    }
}

#[async_trait]
impl GmailProvider for GmailClient {
    async fn profile_history_id(&self) -> ProviderResult<String> {
        let request = self.request(Method::GET, format!("{API}/profile")).await?;
        Ok(self.json::<Profile>(request, false).await?.history_id)
    }

    async fn list_threads(&self, page: Option<&str>) -> ProviderResult<ThreadPage> {
        let mut request = self
            .request(
                Method::GET,
                format!("{API}/threads?maxResults=100&labelIds=INBOX"),
            )
            .await?;
        if let Some(page) = page {
            request = request.query(&[("pageToken", page)]);
        }
        let result: ThreadList = self.json(request, false).await?;
        Ok(ThreadPage {
            thread_ids: result.threads.into_iter().map(|thread| thread.id).collect(),
            next_page_token: result.next_page_token,
        })
    }

    async fn get_thread(&self, id: &str) -> ProviderResult<Vec<GmailMessage>> {
        self.pace_thread_fetch().await;
        let request = self
            .request(Method::GET, format!("{API}/threads/{id}?format=full"))
            .await?;
        Ok(self.json::<GmailThread>(request, false).await?.messages)
    }

    async fn history(&self, cursor: &str, page: Option<&str>) -> ProviderResult<HistoryPage> {
        let mut request = self
            .request(
                Method::GET,
                format!("{API}/history?startHistoryId={cursor}&maxResults=100"),
            )
            .await?;
        if let Some(page) = page {
            request = request.query(&[("pageToken", page)]);
        }
        let result: HistoryList = self.json(request, true).await?;
        let mut ids = Vec::new();
        for record in result.history {
            ids.extend(record.messages.into_iter().map(|item| item.thread_id));
            for item in record
                .messages_added
                .into_iter()
                .chain(record.messages_deleted)
                .chain(record.labels_added)
                .chain(record.labels_removed)
            {
                ids.push(item.message.thread_id);
            }
        }
        ids.sort();
        ids.dedup();
        Ok(HistoryPage {
            thread_ids: ids,
            history_id: result.history_id,
            next_page_token: result.next_page_token,
        })
    }

    async fn modify_thread(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        let request = self
            .request(Method::POST, format!("{API}/threads/{id}/modify"))
            .await?
            .json(&ModifyRequest {
                add_label_ids: add,
                remove_label_ids: remove,
            });
        self.send(request, false).await?;
        Ok(())
    }

    async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
        let request = self.request(Method::GET, format!("{API}/labels")).await?;
        Ok(self
            .json::<LabelList>(request, false)
            .await?
            .labels
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn create_label(&self, name: &str) -> ProviderResult<Label> {
        let request = self
            .request(Method::POST, format!("{API}/labels"))
            .await?
            .json(&serde_json::json!({ "name": name }));
        Ok(self.json::<GmailLabel>(request, false).await?.into())
    }

    async fn update_label(&self, id: &str, name: &str) -> ProviderResult<Label> {
        let request = self
            .request(Method::PATCH, format!("{API}/labels/{id}"))
            .await?
            .json(&serde_json::json!({ "name": name }));
        Ok(self.json::<GmailLabel>(request, false).await?.into())
    }

    async fn delete_label(&self, id: &str) -> ProviderResult<()> {
        let request = self
            .request(Method::DELETE, format!("{API}/labels/{id}"))
            .await?;
        self.send(request, false).await?;
        Ok(())
    }
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.clamp(1, 60)))
}

fn retry_delay(attempt: u32, quota_limited: bool) -> Duration {
    let base = if quota_limited { 15 } else { 1 };
    Duration::from_secs((base * (1_u64 << attempt.min(5))).min(60))
}

fn is_quota_error(body: &str) -> bool {
    let normalized = body.to_ascii_lowercase();
    normalized.contains("ratelimitexceeded")
        || normalized.contains("user_rate_limit_exceeded")
        || normalized.contains("quota exceeded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_gmail_quota_errors_returned_as_forbidden() {
        assert!(is_quota_error(
            r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#
        ));
        assert!(is_quota_error(
            r#"{"error":{"message":"Quota exceeded for quota metric 'Total Query Cost'"}}"#
        ));
        assert!(!is_quota_error(
            r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#
        ));
    }

    #[test]
    fn quota_retries_wait_for_the_usage_window() {
        assert_eq!(retry_delay(0, true), Duration::from_secs(15));
        assert_eq!(retry_delay(2, true), Duration::from_secs(60));
        assert_eq!(retry_delay(2, false), Duration::from_secs(4));
    }
}
