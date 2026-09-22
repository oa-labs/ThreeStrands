//! Optional Three Strands account and application-owned data synchronization.
//! Mail-provider credentials and cached mail never cross this boundary.

use std::{collections::BTreeSet, str::FromStr, sync::Arc, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use keyring::Entry;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::Emitter;
use threestrands_sync_protocol::{
    AccountProfile, EntityType, ResolveConflictRequest, SyncConflict, SyncOperation, SyncRecord,
    SyncRequest, SyncResponse,
};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener, sync::Mutex};
use url::Url;
use uuid::Uuid;

use crate::{db::Database, models::{Account, Snippet, SplitInbox, ThreadTask}};

const SERVICE: &str = "app.threestrands.account";
const SESSION_KEY: &str = "refresh-token";
const ACTIVE_POLL: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountStatus {
    pub configured: bool,
    pub signed_in: bool,
    pub profile: Option<AccountProfile>,
    pub sync_entitled: bool,
    pub enrollment_confirmed: bool,
    pub last_successful_sync: Option<String>,
    pub error: Option<String>,
    pub pending_operations: i64,
    pub conflict_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDevice {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub current: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default = "default_access_lifetime")]
    expires_in: i64,
}

fn default_access_lifetime() -> i64 { 15 * 60 }

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest<'a> {
    redirect_uri: &'a str,
    code_challenge: &'a str,
    state: &'a str,
    device_name: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartResponse { authorization_url: String }

#[derive(Clone)]
pub struct CloudSync {
    database: Arc<Database>,
    base_url: Option<String>,
    http: reqwest::Client,
    access_token: Arc<Mutex<Option<String>>>,
    access_expires_at: Arc<Mutex<Option<std::time::Instant>>>,
    gate: Arc<Mutex<()>>,
}

impl CloudSync {
    pub fn new(database: Arc<Database>) -> Result<Self, String> {
        let base_url = std::env::var("THREESTRANDS_SYNC_URL")
            .ok()
            .or_else(|| option_env!("THREESTRANDS_SYNC_URL").map(str::to_owned))
            .map(|value| value.trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());
        Ok(Self {
            database,
            base_url,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(45))
                .build().map_err(display)?,
            access_token: Arc::new(Mutex::new(None)),
            access_expires_at: Arc::new(Mutex::new(None)),
            gate: Arc::new(Mutex::new(())),
        })
    }

    pub fn status(&self) -> Result<CloudAccountStatus, String> {
        self.database.cloud_account_status(self.base_url.is_some())
    }

    pub async fn sign_in(&self) -> Result<CloudAccountStatus, String> {
        let base = self.base_url.as_ref().ok_or_else(|| "Three Strands account service is not configured".to_string())?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(display)?;
        let redirect = format!("http://127.0.0.1:{}/account/callback", listener.local_addr().map_err(display)?.port());
        let verifier = random_token(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_token(32);
        let device_id = self.database.cloud_device_id()?;
        let device_name = device_name();
        let start = self.http.post(format!("{base}/v1/auth/google/start"))
            .json(&StartRequest { redirect_uri: &redirect, code_challenge: &challenge, state: &state, device_name: &device_name })
            .send().await.map_err(display)?.error_for_status().map_err(http_error)?
            .json::<StartResponse>().await.map_err(display)?;
        open::that(&start.authorization_url).map_err(display)?;
        let code = accept_callback(&listener, &state).await?;
        let tokens = self.http.post(format!("{base}/v1/auth/token"))
            .json(&json!({ "code": code, "codeVerifier": verifier, "deviceId": device_id }))
            .send().await.map_err(display)?.error_for_status().map_err(http_error)?
            .json::<TokenResponse>().await.map_err(display)?;
        session_entry()?.set_password(&tokens.refresh_token).map_err(display)?;
        *self.access_token.lock().await = Some(tokens.access_token);
        *self.access_expires_at.lock().await = Some(std::time::Instant::now() + Duration::from_secs(tokens.expires_in.saturating_sub(30).max(1) as u64));
        let profile = self.fetch_profile().await?;
        self.database.set_cloud_profile(&profile)?;
        self.status()
    }

    pub async fn sign_out(&self) -> Result<(), String> {
        if let Ok(token) = self.access_token().await {
            if let Some(base) = &self.base_url {
                let _ = self.http.post(format!("{base}/v1/auth/revoke")).bearer_auth(token).send().await;
            }
        }
        *self.access_token.lock().await = None;
        *self.access_expires_at.lock().await = None;
        match session_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => return Err(display(error)),
        }
        self.database.clear_cloud_account()
    }

    pub async fn delete_account(&self) -> Result<(), String> {
        let response = self.authorized(reqwest::Method::DELETE, "/v1/me").await?.send().await.map_err(display)?;
        response.error_for_status().map_err(http_error)?;
        self.sign_out().await
    }

    pub async fn devices(&self) -> Result<Vec<CloudDevice>, String> {
        self.authorized(reqwest::Method::GET, "/v1/devices").await?.send().await.map_err(display)?
            .error_for_status().map_err(http_error)?.json().await.map_err(display)
    }

    pub async fn revoke_device(&self, id: &str) -> Result<(), String> {
        let response = self.authorized(reqwest::Method::DELETE, &format!("/v1/devices/{id}")).await?
            .send().await.map_err(display)?;
        response.error_for_status().map_err(http_error)?;
        if id == self.database.cloud_device_id()? { self.sign_out().await?; }
        Ok(())
    }

    pub async fn confirm_enrollment(&self, preferences: Value) -> Result<(), String> {
        let status = self.status()?;
        if !status.signed_in || !status.sync_entitled { return Err("Cross-device sync is not enabled for this account".into()); }
        self.database.confirm_cloud_enrollment()?;
        self.pull_only().await?;
        if self.database.synced_preferences()?.is_none() {
            self.database.enqueue_cloud_entity(EntityType::Preferences, "portable", preferences, None)?;
        }
        self.sync_once().await
    }

    async fn pull_only(&self) -> Result<(), String> {
        loop {
            let cursor = self.database.cloud_sync_batch()?.0;
            let response = self.authorized(reqwest::Method::POST, "/v1/sync").await?
                .json(&SyncRequest { cursor, operations: vec![] }).send().await.map_err(display)?;
            if response.status() == reqwest::StatusCode::FORBIDDEN { self.database.disable_cloud_entitlement()?; }
            let response = response.error_for_status().map_err(http_error)?.json::<SyncResponse>().await.map_err(display)?;
            let more = response.has_more;
            self.database.apply_cloud_response(&response)?;
            if !more { return Ok(()); }
        }
    }

    pub async fn sync_once(&self) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let status = self.status()?;
        if !status.signed_in || !status.sync_entitled || !status.enrollment_confirmed { return Ok(()); }
        let result = self.sync_pages().await;
        match &result {
            Ok(()) => self.database.record_cloud_success()?,
            Err(error) => self.database.record_cloud_error(error)?,
        }
        result
    }

    async fn sync_pages(&self) -> Result<(), String> {
        loop {
            let (cursor, operations) = self.database.cloud_sync_batch()?;
            let response = self.authorized(reqwest::Method::POST, "/v1/sync").await?
                .json(&SyncRequest { cursor, operations }).send().await.map_err(display)?;
            if response.status() == reqwest::StatusCode::FORBIDDEN { self.database.disable_cloud_entitlement()?; }
            let response = response.error_for_status().map_err(http_error)?.json::<SyncResponse>().await.map_err(display)?;
            let more = response.has_more;
            self.database.apply_cloud_response(&response)?;
            if !more { return Ok(()); }
        }
    }

    pub async fn conflicts(&self) -> Result<Vec<SyncConflict>, String> {
        self.authorized(reqwest::Method::GET, "/v1/conflicts").await?.send().await.map_err(display)?
            .error_for_status().map_err(http_error)?.json().await.map_err(display)
    }

    pub async fn resolve_conflict(&self, id: &str, request: ResolveConflictRequest) -> Result<(), String> {
        let record = self.authorized(reqwest::Method::POST, &format!("/v1/conflicts/{id}/resolve")).await?
            .json(&request).send().await.map_err(display)?.error_for_status().map_err(http_error)?
            .json::<SyncRecord>().await.map_err(display)?;
        self.database.apply_cloud_record(&record)?;
        self.database.remove_cloud_conflict(id)?;
        Ok(())
    }

    async fn fetch_profile(&self) -> Result<AccountProfile, String> {
        self.authorized(reqwest::Method::GET, "/v1/me").await?.send().await.map_err(display)?
            .error_for_status().map_err(http_error)?.json().await.map_err(display)
    }

    async fn authorized(&self, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder, String> {
        let base = self.base_url.as_ref().ok_or_else(|| "Three Strands account service is not configured".to_string())?;
        Ok(self.http.request(method, format!("{base}{path}")).bearer_auth(self.access_token().await?))
    }

    async fn access_token(&self) -> Result<String, String> {
        if self.access_expires_at.lock().await.is_some_and(|expiry| expiry > std::time::Instant::now()) {
            if let Some(value) = self.access_token.lock().await.clone() { return Ok(value); }
        }
        let refresh = session_entry()?.get_password().map_err(|error| match error {
            keyring::Error::NoEntry => "Sign in to your Three Strands account".into(),
            other => display(other),
        })?;
        let base = self.base_url.as_ref().ok_or_else(|| "Three Strands account service is not configured".to_string())?;
        let response = self.http.post(format!("{base}/v1/auth/refresh"))
            .json(&json!({ "refreshToken": refresh })).send().await.map_err(display)?
            .error_for_status().map_err(http_error)?.json::<TokenResponse>().await.map_err(display)?;
        session_entry()?.set_password(&response.refresh_token).map_err(display)?;
        *self.access_token.lock().await = Some(response.access_token.clone());
        *self.access_expires_at.lock().await = Some(std::time::Instant::now() + Duration::from_secs(response.expires_in.saturating_sub(30).max(1) as u64));
        Ok(response.access_token)
    }

    pub fn spawn<F>(self, handle: tauri::AppHandle, on_synced: F)
    where
        F: Fn(&tauri::AppHandle) + Send + Sync + 'static,
    {
        tauri::async_runtime::spawn(async move {
            let mut backoff = ACTIVE_POLL;
            loop {
                tokio::time::sleep(backoff).await;
                match self.sync_once().await {
                    Ok(()) => { backoff = ACTIVE_POLL; on_synced(&handle); },
                    Err(_) => backoff = std::cmp::min(backoff.saturating_mul(2), MAX_BACKOFF),
                }
                let _ = handle.emit("cloud-sync-status", ());
            }
        });
    }
}

impl Database {
    fn cloud_account_status(&self, configured: bool) -> Result<CloudAccountStatus, String> {
        let connection = self.connection()?;
        let row = connection.query_row(
            "SELECT user_id,email,display_name,avatar_url,sync_entitled,enrollment_confirmed,last_successful_sync,last_error
             FROM cloud_account_state WHERE singleton=1", [], |row| Ok((
                row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?, row.get::<_, bool>(5)?, row.get(6)?, row.get(7)?
             )),
        ).map_err(display)?;
        let pending_operations = connection.query_row("SELECT COUNT(*) FROM cloud_sync_outbox", [], |row| row.get(0)).map_err(display)?;
        let conflict_count = connection.query_row("SELECT COUNT(*) FROM cloud_sync_conflicts", [], |row| row.get(0)).map_err(display)?;
        let signed_in = row.0.is_some();
        let profile = match (row.0, row.1) {
            (Some(id), Some(email)) => Some(AccountProfile { id, email, display_name: row.2, avatar_url: row.3, entitlements: vec![] }),
            _ => None,
        };
        Ok(CloudAccountStatus { configured, signed_in, profile, sync_entitled: row.4, enrollment_confirmed: row.5,
            last_successful_sync: row.6, error: row.7, pending_operations, conflict_count })
    }

    fn cloud_device_id(&self) -> Result<String, String> {
        self.connection()?.query_row("SELECT device_id FROM cloud_account_state WHERE singleton=1", [], |row| row.get(0)).map_err(display)
    }

    fn set_cloud_profile(&self, profile: &AccountProfile) -> Result<(), String> {
        let entitled = profile.entitlements.iter().any(|value| value.feature == "sync");
        self.connection()?.execute(
            "UPDATE cloud_account_state SET user_id=?1,email=?2,display_name=?3,avatar_url=?4,sync_entitled=?5,last_error=NULL WHERE singleton=1",
            params![profile.id, profile.email, profile.display_name, profile.avatar_url, entitled],
        ).map_err(display)?;
        Ok(())
    }

    fn clear_cloud_account(&self) -> Result<(), String> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        tx.execute("UPDATE cloud_account_state SET user_id=NULL,email=NULL,display_name=NULL,avatar_url=NULL,cursor=0,enrollment_confirmed=0,sync_entitled=0,last_successful_sync=NULL,last_error=NULL WHERE singleton=1", []).map_err(display)?;
        for table in ["cloud_sync_metadata", "cloud_sync_outbox", "cloud_sync_conflicts"] {
            tx.execute(&format!("DELETE FROM {table}"), []).map_err(display)?;
        }
        tx.commit().map_err(display)
    }

    fn confirm_cloud_enrollment(&self) -> Result<(), String> {
        self.connection()?.execute("UPDATE cloud_account_state SET enrollment_confirmed=1 WHERE singleton=1", []).map_err(display)?;
        for task in self.list_tasks(None, None)? { self.enqueue_cloud_entity(EntityType::Task, &task.id, serde_json::to_value(&task).map_err(display)?, None)?; }
        for snippet in self.list_snippets()? { self.enqueue_cloud_entity(EntityType::Snippet, &snippet.id, serde_json::to_value(&snippet).map_err(display)?, None)?; }
        for split in self.list_split_inboxes()? { self.enqueue_cloud_entity(EntityType::SplitInbox, &split.id, serde_json::to_value(&split).map_err(display)?, None)?; }
        for account in self.list_accounts()? {
            let payload = json!({"email":account.email,"displayName":account.display_name,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order});
            self.enqueue_cloud_entity(EntityType::MailAccount, &account.email.to_ascii_lowercase(), payload, None)?;
        }
        for account in self.list_calendar_accounts()? {
            self.enqueue_cloud_entity(EntityType::CalendarAccount, &account.email.to_ascii_lowercase(), json!({"email":account.email}), None)?;
            if let Some(ids) = self.calendar_selection(&account.email)? {
                self.enqueue_cloud_entity(EntityType::CalendarSelection, &account.email.to_ascii_lowercase(), json!({"accountId":account.email,"calendarIds":ids}), None)?;
            }
        }
        self.enqueue_cloud_entity(EntityType::Retention, "mail", json!({"days":self.retention_days()?}), None)
    }

    pub fn enqueue_cloud_entity(&self, entity_type: EntityType, entity_id: &str, payload: Value, fields: Option<BTreeSet<String>>) -> Result<(), String> {
        let connection = self.connection()?;
        let active: bool = connection.query_row("SELECT user_id IS NOT NULL AND enrollment_confirmed AND sync_entitled FROM cloud_account_state WHERE singleton=1", [], |row| row.get(0)).map_err(display)?;
        if !active { return Ok(()); }
        let base: i64 = connection.query_row("SELECT server_version FROM cloud_sync_metadata WHERE entity_type=?1 AND entity_id=?2", params![entity_type.as_str(), entity_id], |row| row.get(0)).optional().map_err(display)?.unwrap_or(0);
        let sequence: i64 = connection.query_row("SELECT COALESCE(MAX(local_sequence),0)+1 FROM cloud_sync_outbox", [], |row| row.get(0)).map_err(display)?;
        let fields = fields.unwrap_or_else(|| payload.as_object().map(|value| value.keys().cloned().collect()).unwrap_or_else(|| BTreeSet::from(["*".into()])));
        let complete_payload = payload.clone();
        let patch = match payload.as_object() {
            Some(object) if !fields.contains("*") => Value::Object(
                object.iter().filter(|(key, _)| fields.contains(*key)).map(|(key, value)| (key.clone(), value.clone())).collect()
            ),
            _ => payload,
        };
        connection.execute("INSERT INTO cloud_sync_outbox(operation_id,entity_type,entity_id,base_version,changed_fields,patch,deleted,local_sequence,created_at) VALUES(?1,?2,?3,?4,?5,?6,0,?7,?8)",
            params![Uuid::new_v4().to_string(),entity_type.as_str(),entity_id,base,serde_json::to_string(&fields).map_err(display)?,patch.to_string(),sequence,Utc::now().to_rfc3339()]).map_err(display)?;
        drop(connection);
        if self.replicated_sync_active()? {
            self.record_replicated_write(entity_type, entity_id, &fields, &complete_payload)?;
        }
        Ok(())
    }

    pub fn enqueue_cloud_deletion(&self, entity_type: EntityType, entity_id: &str) -> Result<(), String> {
        let connection = self.connection()?;
        let active: bool = connection.query_row("SELECT user_id IS NOT NULL AND enrollment_confirmed AND sync_entitled FROM cloud_account_state WHERE singleton=1", [], |row| row.get(0)).map_err(display)?;
        if !active { return Ok(()); }
        let base: i64 = connection.query_row("SELECT server_version FROM cloud_sync_metadata WHERE entity_type=?1 AND entity_id=?2", params![entity_type.as_str(),entity_id], |row| row.get(0)).optional().map_err(display)?.unwrap_or(0);
        let sequence: i64 = connection.query_row("SELECT COALESCE(MAX(local_sequence),0)+1 FROM cloud_sync_outbox", [], |row| row.get(0)).map_err(display)?;
        connection.execute("INSERT INTO cloud_sync_outbox(operation_id,entity_type,entity_id,base_version,changed_fields,deleted,local_sequence,created_at) VALUES(?1,?2,?3,?4,'[\"*\"]',1,?5,?6)",
            params![Uuid::new_v4().to_string(),entity_type.as_str(),entity_id,base,sequence,Utc::now().to_rfc3339()]).map_err(display)?;
        drop(connection);
        if self.replicated_sync_active()? {
            self.record_replicated_deletion(entity_type, entity_id)?;
        }
        Ok(())
    }

    fn cloud_sync_batch(&self) -> Result<(i64, Vec<SyncOperation>), String> {
        let connection = self.connection()?;
        let cursor = connection.query_row("SELECT cursor FROM cloud_account_state WHERE singleton=1", [], |row| row.get(0)).map_err(display)?;
        let device_id = self.cloud_device_id()?;
        let mut statement = connection.prepare("SELECT operation_id,entity_type,entity_id,base_version,changed_fields,patch,deleted,local_sequence FROM cloud_sync_outbox ORDER BY local_sequence LIMIT 500").map_err(display)?;
        let rows = statement.query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?,row.get::<_,String>(4)?,row.get::<_,Option<String>>(5)?,row.get::<_,bool>(6)?,row.get::<_,i64>(7)?))).map_err(display)?;
        let mut operations = Vec::new();
        for row in rows { let row = row.map_err(display)?; operations.push(SyncOperation { operation_id:row.0,device_id:device_id.clone(),entity_type:EntityType::from_str(&row.1)?,entity_id:row.2,base_version:row.3,changed_fields:serde_json::from_str(&row.4).map_err(display)?,patch:row.5.map(|v|serde_json::from_str(&v)).transpose().map_err(display)?,deleted:row.6,local_sequence:row.7 }); }
        Ok((cursor, operations))
    }

    fn apply_cloud_response(&self, response: &SyncResponse) -> Result<(), String> {
        for change in &response.changes { self.apply_cloud_record(change)?; }
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        for ack in &response.acknowledgements {
            tx.execute("DELETE FROM cloud_sync_outbox WHERE operation_id=?1", [&ack.operation_id]).map_err(display)?;
        }
        tx.execute("UPDATE cloud_account_state SET cursor=?1 WHERE singleton=1", [response.next_cursor]).map_err(display)?;
        for conflict in &response.conflicts {
            tx.execute("INSERT INTO cloud_sync_conflicts(id,entity_type,entity_id,current_version,overlapping_fields,cloud_payload,device_patch,device_deleted,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(id) DO UPDATE SET current_version=excluded.current_version,cloud_payload=excluded.cloud_payload,device_patch=excluded.device_patch",
                params![conflict.id,conflict.entity_type.as_str(),conflict.entity_id,conflict.current_version,serde_json::to_string(&conflict.overlapping_fields).map_err(display)?,conflict.cloud_payload.as_ref().map(Value::to_string),conflict.device_patch.as_ref().map(Value::to_string),conflict.device_deleted,conflict.created_at]).map_err(display)?;
        }
        tx.commit().map_err(display)
    }

    fn apply_cloud_record(&self, record: &SyncRecord) -> Result<(), String> {
        self.materialize_entity(record.entity_type, &record.entity_id, record.payload.as_ref(), record.deleted)?;
        self.connection()?.execute("INSERT INTO cloud_sync_metadata(entity_type,entity_id,server_version) VALUES(?1,?2,?3) ON CONFLICT(entity_type,entity_id) DO UPDATE SET server_version=excluded.server_version", params![record.entity_type.as_str(),record.entity_id,record.version]).map_err(display)?;
        Ok(())
    }

    /// The materialization boundary from the operation graph (or the legacy
    /// server response) to application tables: a one-way function from a
    /// resolved, complete entity value to a table write. Shared by the
    /// legacy `apply_cloud_record` and the replicated-sync projection path
    /// in `replicated_sync.rs`, so there is exactly one place that knows
    /// how to turn a `(entity_type, entity_id, payload, deleted)` triple
    /// into local state.
    pub(crate) fn materialize_entity(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        payload: Option<&Value>,
        deleted: bool,
    ) -> Result<(), String> {
        if deleted {
            match entity_type {
                EntityType::Task => { self.connection()?.execute("DELETE FROM tasks WHERE id=?1", [entity_id]).map_err(display)?; }
                EntityType::Snippet => { self.connection()?.execute("DELETE FROM snippets WHERE id=?1", [entity_id]).map_err(display)?; }
                EntityType::SplitInbox => { self.connection()?.execute("DELETE FROM split_inboxes WHERE id=?1", [entity_id]).map_err(display)?; }
                EntityType::MailAccount => { clear_provider_credential("app.threestrands.mail", entity_id)?; if self.get_account(entity_id)?.is_some() { self.remove_account(entity_id)?; } }
                EntityType::CalendarAccount => { clear_provider_credential("app.threestrands.calendar", entity_id)?; let _ = self.remove_calendar_account(entity_id); }
                _ => {}
            }
        } else if let Some(payload) = payload {
            match entity_type {
                EntityType::Task => self.upsert_cloud_task(serde_json::from_value(payload.clone()).map_err(display)?)?,
                EntityType::Snippet => self.upsert_cloud_snippet(serde_json::from_value(payload.clone()).map_err(display)?)?,
                EntityType::SplitInbox => self.upsert_cloud_split(serde_json::from_value(payload.clone()).map_err(display)?)?,
                EntityType::MailAccount => self.upsert_cloud_account(payload)?,
                EntityType::CalendarAccount => self.upsert_cloud_calendar(payload)?,
                EntityType::CalendarSelection => self.upsert_cloud_calendar_selection(payload)?,
                EntityType::Preferences => { self.connection()?.execute("INSERT INTO cloud_preferences(key,value,updated_at) VALUES('portable',?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at", params![payload.to_string(),Utc::now().to_rfc3339()]).map_err(display)?; }
                EntityType::Retention => self.set_retention_days(payload.get("days").and_then(Value::as_i64))?,
            }
        }
        Ok(())
    }

    fn upsert_cloud_task(&self, task: ThreadTask) -> Result<(), String> {
        self.connection()?.execute("INSERT INTO tasks(id,account_id,thread_id,source_message_id,subject_snapshot,title,notes,kind,due_kind,due_value,time_zone,repeat_interval_days,status,completion_source,evidence_text,wait_after,created_at,updated_at,completed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19) ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id,thread_id=excluded.thread_id,source_message_id=excluded.source_message_id,subject_snapshot=excluded.subject_snapshot,title=excluded.title,notes=excluded.notes,kind=excluded.kind,due_kind=excluded.due_kind,due_value=excluded.due_value,time_zone=excluded.time_zone,repeat_interval_days=excluded.repeat_interval_days,status=excluded.status,completion_source=excluded.completion_source,evidence_text=excluded.evidence_text,wait_after=excluded.wait_after,updated_at=excluded.updated_at,completed_at=excluded.completed_at",
            params![task.id,task.account_id,task.thread_id,task.source_message_id,task.subject_snapshot,task.title,task.notes,task.kind,task.due_kind,task.due_value,task.time_zone,task.repeat_interval_days,task.status,task.completion_source,task.evidence_text,task.wait_after,task.created_at,task.updated_at,task.completed_at]).map_err(display)?;
        Ok(())
    }
    fn upsert_cloud_snippet(&self, item: Snippet) -> Result<(), String> { self.connection()?.execute("INSERT INTO snippets(id,name,body,created_at,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET name=excluded.name,body=excluded.body,updated_at=excluded.updated_at",params![item.id,item.name,item.body,item.created_at,Utc::now().to_rfc3339()]).map_err(display)?; Ok(()) }
    fn upsert_cloud_split(&self, item: SplitInbox) -> Result<(), String> { self.connection()?.execute("INSERT INTO split_inboxes(id,name,match_kind,match_value,sort_order,created_at,account_id,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET name=excluded.name,match_kind=excluded.match_kind,match_value=excluded.match_value,sort_order=excluded.sort_order,account_id=excluded.account_id,updated_at=excluded.updated_at",params![item.id,item.name,item.match_kind,item.match_value,item.sort_order,item.created_at,item.account_id,Utc::now().to_rfc3339()]).map_err(display)?; Ok(()) }
    fn upsert_cloud_account(&self, value: &Value) -> Result<(), String> { let item: Account = serde_json::from_value(json!({"email":value["email"],"displayName":value.get("displayName").cloned().unwrap_or(Value::Null),"color":value["color"],"status":"needs_reauth","provider":value["provider"],"sortOrder":value["sortOrder"],"connectedAt":Utc::now().to_rfc3339(),"lastSyncedAt":null})).map_err(display)?; self.connection()?.execute("INSERT INTO accounts(email,display_name,color,status,provider,sort_order,connected_at) VALUES(?1,?2,?3,'needs_reauth',?4,?5,?6) ON CONFLICT(email) DO UPDATE SET display_name=excluded.display_name,color=excluded.color,provider=excluded.provider,sort_order=excluded.sort_order",params![item.email,item.display_name,item.color,item.provider,item.sort_order,item.connected_at]).map_err(display)?; Ok(()) }
    fn upsert_cloud_calendar(&self, value:&Value)->Result<(),String>{let email=value.get("email").and_then(Value::as_str).ok_or("Invalid calendar account")?;self.connection()?.execute("INSERT INTO calendar_accounts(email,connected_at,status) VALUES(?1,?2,'needs_reauth') ON CONFLICT(email) DO NOTHING",params![email,Utc::now().to_rfc3339()]).map_err(display)?;Ok(())}
    fn upsert_cloud_calendar_selection(&self,value:&Value)->Result<(),String>{let email=value.get("accountId").and_then(Value::as_str).ok_or("Invalid calendar selection")?;let ids=value.get("calendarIds").and_then(Value::as_array).ok_or("Invalid calendar selection")?.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>();if self.list_calendar_accounts()?.iter().any(|a|a.email==email){self.set_calendar_selection(email,&ids)?;}Ok(())}
    fn remove_cloud_conflict(&self,id:&str)->Result<(),String>{self.connection()?.execute("DELETE FROM cloud_sync_conflicts WHERE id=?1",[id]).map_err(display)?;Ok(())}
    fn record_cloud_success(&self)->Result<(),String>{self.connection()?.execute("UPDATE cloud_account_state SET last_successful_sync=?1,last_error=NULL WHERE singleton=1",[Utc::now().to_rfc3339()]).map_err(display)?;Ok(())}
    fn record_cloud_error(&self,error:&str)->Result<(),String>{self.connection()?.execute("UPDATE cloud_account_state SET last_error=?1 WHERE singleton=1",[error]).map_err(display)?;Ok(())}
    fn disable_cloud_entitlement(&self)->Result<(),String>{self.connection()?.execute("UPDATE cloud_account_state SET sync_entitled=0 WHERE singleton=1",[]).map_err(display)?;Ok(())}
    pub fn synced_preferences(&self)->Result<Option<Value>,String>{self.connection()?.query_row("SELECT value FROM cloud_preferences WHERE key='portable'",[],|row|row.get::<_,String>(0)).optional().map_err(display)?.map(|value|serde_json::from_str(&value).map_err(display)).transpose()}
}

async fn accept_callback(listener: &TcpListener, expected_state: &str) -> Result<String, String> {
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5 * 60), listener.accept()).await.map_err(|_| "Account sign-in timed out".to_string())?.map_err(display)?;
    let mut buffer = vec![0; 16 * 1024];
    let count = stream.read(&mut buffer).await.map_err(display)?;
    let request = String::from_utf8_lossy(&buffer[..count]);
    let target = request.lines().next().and_then(|line| line.split_whitespace().nth(1)).ok_or("Invalid account callback")?;
    let callback = Url::parse(&format!("http://localhost{target}")).map_err(display)?;
    let query: std::collections::HashMap<_,_> = callback.query_pairs().collect();
    let result = if query.get("state").map(|v|v.as_ref()) != Some(expected_state) { Err("Account sign-in state did not match".into()) }
        else if let Some(error)=query.get("error"){Err(format!("Account sign-in failed: {error}"))}
        else {query.get("code").map(|v|v.to_string()).ok_or("Account sign-in returned no code".into())};
    let body = if result.is_ok() { "Three Strands sign-in complete. You can close this window." } else { "Three Strands could not complete sign-in. Return to the app." };
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
    let _=stream.write_all(response.as_bytes()).await;
    result
}

fn session_entry()->Result<Entry,String>{Entry::new(SERVICE,SESSION_KEY).map_err(display)}
fn clear_provider_credential(service:&str,key:&str)->Result<(),String>{match Entry::new(service,key).map_err(display)?.delete_credential(){Ok(())|Err(keyring::Error::NoEntry)=>Ok(()),Err(error)=>Err(display(error))}}
fn random_token(bytes:usize)->String{let mut value=vec![0;bytes];OsRng.fill_bytes(&mut value);URL_SAFE_NO_PAD.encode(value)}
fn device_name()->String{std::env::var("HOSTNAME").ok().filter(|v|!v.trim().is_empty()).unwrap_or_else(||format!("Three Strands on {}",std::env::consts::OS))}
fn http_error(error:reqwest::Error)->String{if let Some(status)=error.status(){format!("Three Strands service returned {status}")}else{display(error)}}
fn display(value:impl std::fmt::Display)->String{value.to_string()}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_out_database_has_no_cloud_side_effects() {
        let db = Database::open_memory();
        db.enqueue_cloud_entity(EntityType::Snippet,"one",json!({"name":"n","body":"b","createdAt":"now"}),None).unwrap();
        assert_eq!(db.cloud_account_status(true).unwrap().pending_operations,0);
    }

    #[test]
    fn signing_out_preserves_local_workflow_data() {
        let db=Database::open_memory();
        db.create_snippet("Saved","Still here").unwrap();
        db.clear_cloud_account().unwrap();
        assert_eq!(db.list_snippets().unwrap().len(),1);
    }
}
