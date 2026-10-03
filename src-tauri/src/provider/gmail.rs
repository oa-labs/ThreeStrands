use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use rand::Rng;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    Method, RequestBuilder, StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{
    sync::Mutex,
    time::{sleep, Instant},
};

use crate::{
    auth::{AccessTokenError, OAuthCredential},
    mime::RawMessage,
    models::Label,
    provider::{
        Delivery, DeliveryReceipt, MailFetch, MailMutate, MailProvider, MailSend, MailSync,
        ProviderCapabilities, ProviderError, ProviderResult, SyncBatch, SyncCursor, ThreadPage,
    },
};

const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

#[derive(Clone)]
pub struct GmailClient {
    http: reqwest::Client,
    auth: OAuthCredential,
    next_thread_fetch: Arc<Mutex<Instant>>,
    /// The Gmail API base every request is built from. Always `API` outside
    /// tests; tests point it at a loopback stand-in.
    api: Arc<str>,
}

impl GmailClient {
    pub fn new(auth: OAuthCredential) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(45))
                .build()
                .expect("valid Gmail HTTP client configuration"),
            auth,
            next_thread_fetch: Arc::new(Mutex::new(Instant::now())),
            api: API.into(),
        }
    }

    #[cfg(test)]
    fn with_api_base(mut self, api: &str) -> Self {
        self.api = api.into();
        self
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

    async fn access_token(&self) -> ProviderResult<String> {
        self.auth.access_token().await.map_err(|error| match error {
            AccessTokenError::ReauthenticationRequired(message) => {
                ProviderError::ReauthenticationRequired(message)
            }
            AccessTokenError::Transient(message) => ProviderError::Authentication(message),
        })
    }

    async fn request(&self, method: Method, url: String) -> ProviderResult<RequestBuilder> {
        let token = self.access_token().await?;
        Ok(self.http.request(method, url).bearer_auth(token))
    }

    /// Rebuilds `request` with a replacement for the bearer token Gmail just
    /// rejected. Replaces the header rather than appending a second one.
    async fn reauthorize(&self, request: RequestBuilder) -> ProviderResult<RequestBuilder> {
        let (client, request) = request.build_split();
        let mut request =
            request.map_err(|error| ProviderError::InvalidOperation(error.to_string()))?;
        if let Some(rejected) = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        {
            self.auth.expire_access_token(rejected);
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.access_token().await?))
            .map_err(|error| ProviderError::Authentication(error.to_string()))?;
        authorization.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, authorization);
        Ok(RequestBuilder::from_parts(client, request))
    }

    async fn send(
        &self,
        mut request: RequestBuilder,
        history: bool,
    ) -> ProviderResult<reqwest::Response> {
        let mut reauthorized = false;
        for attempt in 0..=4 {
            let cloned = request.try_clone().ok_or_else(|| {
                ProviderError::InvalidOperation("Unable to retry Gmail request".into())
            })?;
            let response = match cloned.send().await {
                Ok(response) => response,
                Err(_) if attempt < 4 => {
                    sleep(retry_delay(attempt, false) + jitter()).await;
                    continue;
                }
                Err(error) => {
                    return Err(ProviderError::TransientTransport(error.to_string()));
                }
            };
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
            if status == StatusCode::UNAUTHORIZED && !reauthorized {
                // Google can revoke or rotate an access token before its
                // local expiry. Replay once with a refreshed token; only a
                // rejection of that token proves the grant itself unusable.
                reauthorized = true;
                request = self.reauthorize(request).await?;
                continue;
            }
            let retry_after = retry_after(&response);
            let body = response.text().await.unwrap_or_default();
            let quota_limited = status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN && is_quota_error(&body));
            if quota_limited
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::SERVICE_UNAVAILABLE
                || status.is_server_error()
            {
                if attempt == 4 {
                    return Err(ProviderError::RetryableServer(format!("{status}: {body}")));
                }
                let delay = retry_after.unwrap_or_else(|| retry_delay(attempt, quota_limited));
                sleep(delay + jitter()).await;
                continue;
            }
            let detail = format!("{status}: {body}");
            return Err(classify_client_rejection(status, detail));
        }
        Err(ProviderError::RetryableServer(
            "retry budget exhausted".into(),
        ))
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
            .map_err(|error| ProviderError::RetryableServer(error.to_string()))
    }
}

fn classify_client_rejection(status: StatusCode, detail: String) -> ProviderError {
    if status == StatusCode::UNAUTHORIZED {
        // `send` has already replayed once with a refreshed token, so this
        // 401 persisted across a refresh. Treat it as a revoked/invalid
        // credential and require a fresh grant.
        ProviderError::ReauthenticationRequired(detail)
    } else if status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY {
        ProviderError::InvalidOperation(detail)
    } else {
        ProviderError::PermanentClientRejection(detail)
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
    messages: Vec<RawMessage>,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchModifyRequest<'a> {
    ids: &'a [String],
    add_label_ids: &'a [String],
    remove_label_ids: &'a [String],
}

fn message_modify_url(api: &str, ids: &[String]) -> Option<String> {
    match ids {
        [] => None,
        [id] => Some(format!("{api}/messages/{id}/modify")),
        _ => Some(format!("{api}/messages/batchModify")),
    }
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
impl MailSync for GmailClient {
    async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
        let request = self.request(Method::GET, format!("{}/profile", self.api)).await?;
        Ok(SyncCursor::new(
            self.json::<Profile>(request, false).await?.history_id,
        ))
    }

    async fn list_inbox(&self, page: Option<&str>) -> ProviderResult<ThreadPage> {
        let mut request = self
            .request(
                Method::GET,
                format!("{}/threads?maxResults=100&labelIds=INBOX", self.api),
            )
            .await?;
        if let Some(page) = page {
            request = request.query(&[("pageToken", page)]);
        }
        let result: ThreadList = self.json(request, false).await?;
        Ok(ThreadPage {
            thread_ids: result.threads.into_iter().map(|thread| thread.id).collect(),
            next: result.next_page_token,
        })
    }

    async fn search(&self, query: &str, page: Option<&str>) -> ProviderResult<ThreadPage> {
        let mut request = self
            .request(Method::GET, format!("{}/threads", self.api))
            .await?
            .query(&[
                ("maxResults", "100"),
                ("includeSpamTrash", "true"),
                ("q", query),
            ]);
        if let Some(page) = page {
            request = request.query(&[("pageToken", page)]);
        }
        let result: ThreadList = self.json(request, false).await?;
        Ok(ThreadPage {
            thread_ids: result.threads.into_iter().map(|thread| thread.id).collect(),
            next: result.next_page_token,
        })
    }

    async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>> {
        self.pace_thread_fetch().await;
        let request = self
            .request(Method::GET, format!("{}/threads/{id}?format=full", self.api))
            .await?;
        Ok(self.json::<GmailThread>(request, false).await?.messages)
    }

    async fn poll(&self, cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
        let position = HistoryPosition::parse(cursor);
        let mut request = self
            .request(
                Method::GET,
                format!(
                    "{}/history?startHistoryId={}&maxResults=100",
                    self.api, position.history_id
                ),
            )
            .await?;
        if let Some(page) = position.page.as_deref() {
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
        let more = result.next_page_token.is_some();
        Ok(SyncBatch {
            changed_threads: ids,
            cursor: position
                .after_page(result.history_id, result.next_page_token)
                .encode(),
            more,
        })
    }

}

#[async_trait]
impl MailFetch for GmailClient {
    async fn fetch_message(&self, id: &str) -> ProviderResult<RawMessage> {
        let request = self
            .request(Method::GET, format!("{}/messages/{id}?format=full", self.api))
            .await?;
        self.json(request, false).await
    }

    async fn attachment_bytes(&self, message: &str, handle: &str) -> ProviderResult<Vec<u8>> {
        #[derive(Deserialize)]
        struct Body {
            data: String,
        }
        let request = self
            .request(
                Method::GET,
                format!("{}/messages/{message}/attachments/{handle}", self.api),
            )
            .await?;
        let body: Body = self.json(request, false).await?;
        crate::mime::decode_attachment_data(&body.data).map_err(ProviderError::Other)
    }
}

/// Gmail's sync position: a `historyId`, plus the page token when a polling
/// round spans several pages. Encoded as `historyId` alone in the common
/// single-page case so that cursors already persisted by earlier versions
/// keep parsing.
#[derive(Debug, PartialEq)]
struct HistoryPosition {
    history_id: String,
    page: Option<String>,
}

impl HistoryPosition {
    /// Where the next `history.list` request resumes after one page. Gmail
    /// page tokens belong to the `startHistoryId` that produced them, so a
    /// mid-round position keeps the round's original start. Only the final
    /// page's newest history id, reported on every page, starts the next
    /// round; adopting it early would pair a page token with a start it was
    /// never issued for.
    fn after_page(&self, newest_history_id: String, next_page: Option<String>) -> Self {
        match next_page {
            Some(page) => Self {
                history_id: self.history_id.clone(),
                page: Some(page),
            },
            None => Self {
                history_id: newest_history_id,
                page: None,
            },
        }
    }

    fn parse(cursor: &SyncCursor) -> Self {
        match cursor.as_str().split_once(' ') {
            Some((history_id, page)) => Self {
                history_id: history_id.to_string(),
                page: Some(page.to_string()),
            },
            None => Self {
                history_id: cursor.as_str().to_string(),
                page: None,
            },
        }
    }

    fn encode(&self) -> SyncCursor {
        match &self.page {
            Some(page) => SyncCursor::new(format!("{} {page}", self.history_id)),
            None => SyncCursor::new(&self.history_id),
        }
    }
}

#[async_trait]
impl MailMutate for GmailClient {
    async fn modify_thread(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        let request = self
            .request(Method::POST, format!("{}/threads/{id}/modify", self.api))
            .await?
            .json(&ModifyRequest {
                add_label_ids: add,
                remove_label_ids: remove,
            });
        self.send(request, false).await?;
        Ok(())
    }

    async fn modify_messages(
        &self,
        ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        let url = message_modify_url(&self.api, ids).ok_or_else(|| {
            ProviderError::InvalidOperation("No Gmail messages to modify".into())
        })?;
        let request = self.request(Method::POST, url).await?;
        let request = if ids.len() == 1 {
            request.json(&ModifyRequest {
                add_label_ids: add,
                remove_label_ids: remove,
            })
        } else {
            request.json(&BatchModifyRequest {
                ids,
                add_label_ids: add,
                remove_label_ids: remove,
            })
        };
        self.send(request, false).await?;
        Ok(())
    }

    async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
        let request = self.request(Method::GET, format!("{}/labels", self.api)).await?;
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
            .request(Method::POST, format!("{}/labels", self.api))
            .await?
            .json(&serde_json::json!({ "name": name }));
        Ok(self.json::<GmailLabel>(request, false).await?.into())
    }

    async fn update_label(&self, id: &str, name: &str) -> ProviderResult<Label> {
        let request = self
            .request(Method::PATCH, format!("{}/labels/{id}", self.api))
            .await?
            .json(&serde_json::json!({ "name": name }));
        Ok(self.json::<GmailLabel>(request, false).await?.into())
    }

    async fn delete_label(&self, id: &str) -> ProviderResult<()> {
        let request = self
            .request(Method::DELETE, format!("{}/labels/{id}", self.api))
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

/// Small random delay added on top of a computed backoff so that multiple
/// accounts (or app instances) recovering from the same outage don't retry
/// in lockstep. Kept separate from `retry_delay` so that function's
/// exact-value tests stay deterministic.
fn jitter() -> Duration {
    Duration::from_millis(rand::thread_rng().gen_range(0..250))
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
    fn a_cursor_persisted_before_paging_existed_still_parses() {
        // Databases in the field hold a bare historyId. Upgrading must not
        // read that as a paging position or force a full resynchronization.
        let position = HistoryPosition::parse(&SyncCursor::new("12345"));
        assert_eq!(position.history_id, "12345");
        assert_eq!(position.page, None);
        assert_eq!(position.encode(), SyncCursor::new("12345"));
    }

    #[test]
    fn a_mid_round_cursor_round_trips_its_page_token() {
        let cursor = HistoryPosition {
            history_id: "12345".into(),
            page: Some("tok-2".into()),
        }
        .encode();
        assert_eq!(cursor, SyncCursor::new("12345 tok-2"));
        let parsed = HistoryPosition::parse(&cursor);
        assert_eq!(parsed.history_id, "12345");
        assert_eq!(parsed.page.as_deref(), Some("tok-2"));
    }

    #[test]
    fn a_history_round_keeps_its_start_until_the_final_page() {
        let start = HistoryPosition::parse(&SyncCursor::new("100"));

        let second = start.after_page("250".into(), Some("tok-2".into()));
        assert_eq!(
            second,
            HistoryPosition {
                history_id: "100".into(),
                page: Some("tok-2".into()),
            },
            "a page token must be replayed with the startHistoryId that issued it"
        );

        let third = second.after_page("260".into(), Some("tok-3".into()));
        assert_eq!(third.history_id, "100");
        assert_eq!(third.page.as_deref(), Some("tok-3"));

        let done = third.after_page("261".into(), None);
        assert_eq!(done.encode(), SyncCursor::new("261"));
    }

    mod unauthorized_replay {
        use std::sync::{Arc, Mutex as StdMutex};

        use axum::{
            extract::State,
            http::{HeaderMap, StatusCode as HttpStatus},
            routing::{get, post},
            Router,
        };

        use super::*;
        use crate::auth::Tokens;

        #[derive(Clone, Default)]
        struct Seen {
            /// Every Authorization header on each Gmail request, in order.
            api: Arc<StdMutex<Vec<Vec<String>>>>,
            token_refreshes: Arc<StdMutex<usize>>,
        }

        /// Gmail stand-in accepting only `accepted`, plus a token endpoint
        /// that answers every refresh with `invalid_grant`.
        async fn serve(accepted: Option<&'static str>) -> (String, Seen) {
            let seen = Seen::default();
            let router = Router::new()
                .route(
                    "/api",
                    get(move |State(seen): State<Seen>, headers: HeaderMap| async move {
                        let values = headers
                            .get_all(AUTHORIZATION)
                            .iter()
                            .map(|value| value.to_str().unwrap().to_string())
                            .collect::<Vec<_>>();
                        let ok = accepted
                            .is_some_and(|token| values == [format!("Bearer {token}")]);
                        seen.api.lock().unwrap().push(values);
                        if ok {
                            (HttpStatus::OK, "{}")
                        } else {
                            (HttpStatus::UNAUTHORIZED, r#"{"error":{"code":401}}"#)
                        }
                    }),
                )
                .route(
                    "/token",
                    post(|State(seen): State<Seen>| async move {
                        *seen.token_refreshes.lock().unwrap() += 1;
                        (HttpStatus::BAD_REQUEST, r#"{"error":"invalid_grant"}"#)
                    }),
                )
                .with_state(seen.clone());
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            (format!("http://{address}"), seen)
        }

        fn client(base: &str, cached_access_token: &str) -> GmailClient {
            GmailClient::new(OAuthCredential::in_memory_for_test(
                &format!("{base}/token"),
                Tokens {
                    access_token: cached_access_token.into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: u64::MAX,
                },
            ))
        }

        #[tokio::test]
        async fn a_rejected_token_is_replaced_once_and_the_request_replayed() {
            let (base, seen) = serve(Some("fresh")).await;
            // Another request already refreshed past the token this one
            // carried, so the replay uses the cached replacement.
            let client = client(&base, "fresh");
            let request = client.http.get(format!("{base}/api")).bearer_auth("stale");

            client.send(request, false).await.unwrap();

            assert_eq!(
                *seen.api.lock().unwrap(),
                vec![vec!["Bearer stale".to_string()], vec!["Bearer fresh".to_string()]],
                "the replay must replace, not append, the Authorization header"
            );
            assert_eq!(*seen.token_refreshes.lock().unwrap(), 0);
        }

        #[tokio::test]
        async fn a_401_refreshes_before_asking_for_reconnection() {
            let (base, seen) = serve(None).await;
            let client = client(&base, "revoked");
            let request = client.request(Method::GET, format!("{base}/api")).await.unwrap();

            let error = client.send(request, false).await.unwrap_err();

            assert!(error.requires_reauthentication(), "unexpected error: {error}");
            assert_eq!(*seen.token_refreshes.lock().unwrap(), 1);
            assert_eq!(seen.api.lock().unwrap().len(), 1);
        }

        #[tokio::test]
        async fn a_401_on_the_replacement_token_requires_reconnection() {
            let (base, seen) = serve(None).await;
            let client = client(&base, "fresh");
            let request = client.http.get(format!("{base}/api")).bearer_auth("stale");

            let error = client.send(request, false).await.unwrap_err();

            assert!(error.requires_reauthentication(), "unexpected error: {error}");
            assert_eq!(seen.api.lock().unwrap().len(), 2, "replays at most once");
        }
    }

    /// Sync and label requests go to the configured API base like delivery
    /// does, so a stand-in server can observe every Gmail call.
    mod api_base {
        use std::sync::{Arc, Mutex as StdMutex};

        use axum::{extract::{RawQuery, State}, routing::get, Json, Router};
        use serde_json::json;

        use super::*;
        use crate::auth::Tokens;

        async fn serve() -> (String, Arc<StdMutex<Vec<String>>>) {
            let seen = Arc::new(StdMutex::new(Vec::new()));
            let router = Router::new()
                .route(
                    "/threads",
                    get(|State(seen): State<Arc<StdMutex<Vec<String>>>>, RawQuery(query): RawQuery| async move {
                        seen.lock().unwrap().push(format!("/threads?{}", query.unwrap_or_default()));
                        Json(json!({"threads": [{"id": "t1"}, {"id": "t2"}], "nextPageToken": "p2"}))
                    }),
                )
                .route(
                    "/labels",
                    get(|State(seen): State<Arc<StdMutex<Vec<String>>>>| async move {
                        seen.lock().unwrap().push("/labels".into());
                        Json(json!({"labels": [{"id": "Label_1", "name": "Receipts", "type": "user"}]}))
                    }),
                )
                .with_state(seen.clone());
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            (format!("http://{address}"), seen)
        }

        fn client(base: &str) -> GmailClient {
            GmailClient::new(OAuthCredential::in_memory_for_test(
                &format!("{base}/token"),
                Tokens {
                    access_token: "access".into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: u64::MAX,
                },
            ))
            .with_api_base(base)
        }

        #[tokio::test]
        async fn inbox_listing_and_labels_use_the_configured_api_base() {
            let (base, seen) = serve().await;
            let client = client(&base);

            let page = client.list_inbox(None).await.unwrap();
            let labels = client.list_labels().await.unwrap();

            assert_eq!(page.thread_ids, ["t1", "t2"]);
            assert_eq!(page.next.as_deref(), Some("p2"));
            assert_eq!(labels[0].id, "Label_1");
            assert_eq!(labels[0].kind, "user");
            assert_eq!(
                *seen.lock().unwrap(),
                ["/threads?maxResults=100&labelIds=INBOX", "/labels"]
            );
        }
    }

    mod delivery {
        use std::{
            collections::HashMap,
            sync::{Arc, Mutex as StdMutex},
        };

        use axum::{
            extract::{Path, Query, State},
            http::StatusCode as HttpStatus,
            routing::{get, post},
            Json, Router,
        };

        use super::*;
        use crate::auth::Tokens;

        const OPERATION: &str = "op-1";
        const SENDER: &str = "me@example.com";

        #[derive(Clone, Default)]
        struct Stub {
            send_status: Arc<StdMutex<Option<(u16, String)>>>,
            send_bodies: Arc<StdMutex<Vec<serde_json::Value>>>,
            search: Arc<StdMutex<serde_json::Value>>,
            searches: Arc<StdMutex<Vec<String>>>,
            messages: Arc<StdMutex<HashMap<String, (u16, serde_json::Value)>>>,
        }

        async fn serve(stub: Stub) -> String {
            let router = Router::new()
                .route(
                    "/messages/send",
                    post(
                        |State(stub): State<Stub>, Json(body): Json<serde_json::Value>| async move {
                            stub.send_bodies.lock().unwrap().push(body);
                            let (status, body) =
                                stub.send_status.lock().unwrap().clone().unwrap();
                            (HttpStatus::from_u16(status).unwrap(), body)
                        },
                    ),
                )
                .route(
                    "/messages",
                    get(
                        |State(stub): State<Stub>,
                         Query(query): Query<HashMap<String, String>>| async move {
                            stub.searches
                                .lock()
                                .unwrap()
                                .push(query.get("q").cloned().unwrap_or_default());
                            Json(stub.search.lock().unwrap().clone())
                        },
                    ),
                )
                .route(
                    "/messages/{id}",
                    get(|State(stub): State<Stub>, Path(id): Path<String>| async move {
                        let (status, body) = stub
                            .messages
                            .lock()
                            .unwrap()
                            .get(&id)
                            .cloned()
                            .unwrap_or((404, serde_json::json!({"error": {"code": 404}})));
                        (HttpStatus::from_u16(status).unwrap(), Json(body))
                    }),
                )
                .with_state(stub);
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            format!("http://{address}")
        }

        fn client(base: &str) -> GmailClient {
            GmailClient::new(OAuthCredential::in_memory_for_test(
                &format!("{base}/token"),
                Tokens {
                    access_token: "token".into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: u64::MAX,
                },
            ))
            .with_api_base(base)
        }

        async fn send_with(status: u16, body: &str) -> Result<DeliveryReceipt, (bool, String)> {
            let stub = Stub::default();
            *stub.send_status.lock().unwrap() = Some((status, body.to_string()));
            let base = serve(stub.clone()).await;
            let delivery = client(&base).prepare_delivery().await.unwrap();
            let result = delivery.send_once(b"raw message", Some("thread-9")).await;
            assert_eq!(
                stub.send_bodies.lock().unwrap().len(),
                1,
                "send is non-idempotent and must be attempted exactly once"
            );
            result
        }

        #[tokio::test]
        async fn an_accepted_send_returns_the_gmail_receipt() {
            let stub = Stub::default();
            *stub.send_status.lock().unwrap() =
                Some((200, r#"{"id":"m-1","threadId":"t-1"}"#.into()));
            let base = serve(stub.clone()).await;
            let delivery = client(&base).prepare_delivery().await.unwrap();

            let receipt = delivery.send_once(b"hello", Some("t-1")).await.unwrap();

            assert_eq!(receipt.provider_message_id, "m-1");
            assert_eq!(receipt.thread_id.as_deref(), Some("t-1"));
            let bodies = stub.send_bodies.lock().unwrap();
            assert_eq!(
                bodies[0],
                serde_json::json!({"raw": "aGVsbG8", "threadId": "t-1"}),
                "raw is URL-safe base64 without padding and the thread is attached"
            );
        }

        #[tokio::test]
        async fn a_400_rejection_is_a_definite_failure() {
            let (definite, message) = send_with(400, r#"{"error":{"code":400}}"#)
                .await
                .unwrap_err();
            assert!(definite, "an explicit client rejection proves nothing was sent");
            assert!(message.contains("HTTP 400"), "{message}");
            assert!(message.contains("Restore the draft"), "{message}");
        }

        #[tokio::test]
        async fn a_408_timeout_is_uncertain() {
            let (definite, message) = send_with(408, "").await.unwrap_err();
            assert!(!definite, "a request timeout may follow acceptance");
            assert!(message.contains("HTTP 408"), "{message}");
            assert!(message.contains("Delivery is uncertain"), "{message}");
        }

        #[tokio::test]
        async fn a_503_server_error_is_uncertain() {
            let (definite, message) = send_with(503, "").await.unwrap_err();
            assert!(!definite, "a server error may follow acceptance");
            assert!(message.contains("HTTP 503"), "{message}");
            assert!(message.contains("Delivery is uncertain"), "{message}");
        }

        #[tokio::test]
        async fn a_malformed_success_body_is_uncertain() {
            let (definite, message) = send_with(200, "not json").await.unwrap_err();
            assert!(!definite, "Gmail answered 2xx, so the message may have been sent");
            assert!(message.contains("could not be read"), "{message}");

            let (definite, _) = send_with(200, r#"{"id":"m-1"}"#).await.unwrap_err();
            assert!(!definite, "a 2xx body missing threadId is also unreadable");
        }

        #[tokio::test]
        async fn a_transport_failure_is_uncertain() {
            // Reserve a loopback port, then close it so the connection is refused.
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            drop(listener);
            let delivery = client(&base).prepare_delivery().await.unwrap();

            let (definite, message) = delivery.send_once(b"raw", None).await.unwrap_err();

            assert!(!definite, "a transport failure cannot prove the send did not land");
            assert!(message.contains("Check sent mail"), "{message}");
        }

        fn message(id: &str, message_id: &str, from: &str, labels: &[&str]) -> serde_json::Value {
            serde_json::json!({
                "id": id,
                "threadId": format!("thread-{id}"),
                "labelIds": labels,
                "payload": {
                    "headers": [
                        {"name": "Message-ID", "value": message_id},
                        {"name": "From", "value": from},
                    ],
                },
            })
        }

        async fn find_with(
            candidates: &[(&str, u16, serde_json::Value)],
        ) -> (ProviderResult<Option<DeliveryReceipt>>, Stub) {
            let stub = Stub::default();
            *stub.search.lock().unwrap() = if candidates.is_empty() {
                serde_json::json!({"resultSizeEstimate": 0})
            } else {
                serde_json::json!({
                    "messages": candidates
                        .iter()
                        .map(|(id, _, _)| serde_json::json!({"id": id, "threadId": format!("thread-{id}")}))
                        .collect::<Vec<_>>(),
                })
            };
            for (id, status, body) in candidates {
                stub.messages
                    .lock()
                    .unwrap()
                    .insert(id.to_string(), (*status, body.clone()));
            }
            let base = serve(stub.clone()).await;
            let result = client(&base).find_sent_copy(OPERATION, SENDER).await;
            (result, stub)
        }

        #[tokio::test]
        async fn a_verified_sent_copy_is_found() {
            let (result, stub) = find_with(&[(
                "m-1",
                200,
                message(
                    "m-1",
                    "<op-1@threestrands.local>",
                    "Me <ME@example.com>",
                    &["SENT"],
                ),
            )])
            .await;

            let receipt = result.unwrap().expect("verified copy");
            assert_eq!(receipt.provider_message_id, "m-1");
            assert_eq!(receipt.thread_id.as_deref(), Some("thread-m-1"));
            assert_eq!(
                *stub.searches.lock().unwrap(),
                vec!["in:sent rfc822msgid:op-1@threestrands.local".to_string()]
            );
        }

        #[tokio::test]
        async fn an_empty_search_finds_no_sent_copy() {
            let (result, _) = find_with(&[]).await;
            assert!(result.unwrap().is_none());
        }

        #[tokio::test]
        async fn search_hits_that_fail_verification_are_not_a_sent_copy() {
            let candidates = [
                (
                    "wrong-id",
                    message("wrong-id", "<op-2@threestrands.local>", SENDER, &["SENT"]),
                ),
                (
                    "wrong-sender",
                    message(
                        "wrong-sender",
                        "<op-1@threestrands.local>",
                        "other@example.com",
                        &["SENT"],
                    ),
                ),
                (
                    "two-senders",
                    message(
                        "two-senders",
                        "<op-1@threestrands.local>",
                        "me@example.com, other@example.com",
                        &["SENT"],
                    ),
                ),
                (
                    "not-sent",
                    message("not-sent", "<op-1@threestrands.local>", SENDER, &["INBOX"]),
                ),
            ];
            for (id, body) in candidates {
                let (result, _) = find_with(&[(id, 200, body)]).await;
                assert!(
                    result.unwrap().is_none(),
                    "candidate {id} must not verify as the sent copy"
                );
            }
        }

        #[tokio::test]
        async fn verification_skips_unverified_hits_before_a_verified_one() {
            let (result, _) = find_with(&[
                (
                    "decoy",
                    200,
                    message("decoy", "<op-1@threestrands.local>", SENDER, &["DRAFT"]),
                ),
                (
                    "real",
                    200,
                    message("real", "op-1@threestrands.local", SENDER, &["SENT"]),
                ),
            ])
            .await;
            assert_eq!(result.unwrap().unwrap().provider_message_id, "real");
        }

        #[tokio::test]
        async fn a_candidate_that_cannot_be_fetched_is_an_error_not_absence() {
            let (result, _) = find_with(&[(
                "vanished",
                404,
                serde_json::json!({"error": {"code": 404}}),
            )])
            .await;
            assert!(
                matches!(result, Err(ProviderError::NotFound)),
                "unexpected result: {:?}",
                result.map(|found| found.map(|r| r.provider_message_id))
            );
        }
    }

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

    #[test]
    fn only_temporary_authentication_errors_retry_mutations() {
        assert!(ProviderError::TransientTransport("offline".into()).retry_mutation());
        assert!(ProviderError::RetryableServer("503".into()).retry_mutation());
        assert!(
            ProviderError::Authentication("token endpoint unavailable".into()).retry_mutation()
        );
        assert!(
            ProviderError::ReauthenticationRequired("invalid_grant".into())
                .requires_reauthentication()
        );
        assert!(
            !ProviderError::ReauthenticationRequired("401".into()).retry_mutation(),
            "persistent authentication failures must pause instead of rescheduling"
        );
        assert!(!ProviderError::InvalidOperation("bad label".into()).retry_mutation());
        assert!(
            !ProviderError::PermanentClientRejection("permission denied".into()).retry_mutation()
        );
    }

    #[test]
    fn persistent_unauthorized_response_requires_reauthentication() {
        assert!(matches!(
            classify_client_rejection(StatusCode::UNAUTHORIZED, "401".into()),
            ProviderError::ReauthenticationRequired(_)
        ));
    }

    #[test]
    fn spam_message_modify_payload_adds_spam_and_removes_inbox() {
        let ids = vec!["message-1".to_string(), "message-2".to_string()];
        let add = vec!["SPAM".to_string()];
        let remove = vec!["INBOX".to_string()];
        let payload = serde_json::to_value(BatchModifyRequest {
            ids: &ids,
            add_label_ids: &add,
            remove_label_ids: &remove,
        })
        .unwrap();

        assert_eq!(
            payload,
            serde_json::json!({
                "ids": ["message-1", "message-2"],
                "addLabelIds": ["SPAM"],
                "removeLabelIds": ["INBOX"],
            }),
        );
        assert!(message_modify_url(API, &ids)
            .unwrap()
            .ends_with("/messages/batchModify"));
        assert!(message_modify_url(API, &ids[..1])
            .unwrap()
            .ends_with("/messages/message-1/modify"));
    }

    #[test]
    fn empty_message_batch_has_no_modify_url() {
        assert_eq!(message_modify_url(API, &[]), None);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SentMessage {
    id: String,
    thread_id: String,
}

impl From<SentMessage> for DeliveryReceipt {
    fn from(value: SentMessage) -> Self {
        Self {
            provider_message_id: value.id,
            thread_id: Some(value.thread_id),
        }
    }
}

/// A Gmail delivery permit: the `messages/send` request with its bearer token
/// already attached, so nothing that can fail for authorization reasons
/// remains between claiming the outbox row and the send itself.
struct GmailDelivery(RequestBuilder);

#[async_trait]
impl Delivery for GmailDelivery {
    // Send is non-idempotent: never route this through the retrying helper.
    async fn send_once(
        self: Box<Self>,
        raw: &[u8],
        thread: Option<&str>,
    ) -> Result<DeliveryReceipt, (bool, String)> {
        use base64::Engine;
        let mut body =
            serde_json::json!({"raw":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)});
        if let Some(thread) = thread {
            body["threadId"] = thread.into();
        }
        let response = self.0.json(&body).send().await.map_err(|_| {
            (
                false,
                "Connection ended during delivery. Check sent mail before sending again.".into(),
            )
        })?;
        if !response.status().is_success() {
            let status = response.status();
            // A server error or timeout can follow acceptance; only explicit client rejection is definite.
            let definite = status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT;
            return Err((
                definite,
                format!(
                    "Gmail returned HTTP {}. {}",
                    status.as_u16(),
                    if definite {
                        "Restore the draft and retry after resolving the error."
                    } else {
                        "Delivery is uncertain; check sent mail."
                    }
                ),
            ));
        }
        response
            .json::<SentMessage>()
            .await
            .map(Into::into)
            .map_err(|_| {
                (
                    false,
                    "Gmail accepted the request but its result could not be read. Check sent mail."
                        .into(),
                )
            })
    }
}

#[async_trait]
impl MailSend for GmailClient {
    async fn sender_identity(&self) -> ProviderResult<String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Identity {
            email_address: String,
        }
        let request = self.request(Method::GET, format!("{}/profile", self.api)).await?;
        Ok(self.json::<Identity>(request, false).await?.email_address)
    }

    async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
        Ok(Box::new(GmailDelivery(
            self.request(Method::POST, format!("{}/messages/send", self.api))
                .await?
                .timeout(Duration::from_secs(60)),
        )))
    }

    async fn find_sent_copy(
        &self,
        operation: &str,
        expected_sender: &str,
    ) -> ProviderResult<Option<DeliveryReceipt>> {
        #[derive(Deserialize)]
        struct Found {
            #[serde(default)]
            messages: Vec<SentMessage>,
        }
        let request = self
            .request(Method::GET, format!("{}/messages", self.api))
            .await?
            .query(&[(
                "q",
                format!("in:sent rfc822msgid:{operation}@threestrands.local"),
            )]);
        let matches: Found = self.json(request, false).await?;
        // Verify the message identity and sender, rather than relying on search alone.
        for candidate in matches.messages {
            let message = self.fetch_message(&candidate.id).await?;
            let matches_id = message.payload.headers.iter().any(|h| {
                h.name.eq_ignore_ascii_case("Message-ID")
                    && h.value.trim().trim_matches(['<', '>'])
                        == format!("{operation}@threestrands.local")
            });
            let matches_sender = message
                .payload
                .headers
                .iter()
                .filter(|h| h.name.eq_ignore_ascii_case("From"))
                .any(|h| {
                    crate::correspondence::addresses(&h.value).is_ok_and(|list| {
                        list.len() == 1 && list[0].1.eq_ignore_ascii_case(expected_sender)
                    })
                });
            if matches_id && matches_sender && message.label_ids.iter().any(|l| l == "SENT") {
                return Ok(Some(candidate.into()));
            }
        }
        Ok(None)
    }
}

impl MailProvider for GmailClient {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            server_search: true,
        }
    }
}
