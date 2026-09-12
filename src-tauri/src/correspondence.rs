//! Local drafts and a send queue deliberately separate from retryable label mutations.
use crate::{
    auth::GoogleAuth,
    db::Database,
    gmail::{GmailClient, GmailProvider},
    mime::{GmailMessage, MimePart},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use mail_builder::MessageBuilder;
use mail_parser::MessageParser;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

const MAX_BYTES: usize = 24 * 1024 * 1024; // Conservative encoded MIME limit.
const UNDO_MS: i64 = 10_000;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(error)
}
fn now() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn migrate(connection: &mut Connection) -> Result<(), String> {
    let tx = connection.transaction().map_err(error)?;
    let version: i64 = tx
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(error)?;
    if version < 1 {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS message_metadata(id TEXT PRIMARY KEY, payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS compose_settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS drafts(id TEXT PRIMARY KEY, revision INTEGER NOT NULL, payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS outbox_messages(id TEXT PRIMARY KEY, draft_id TEXT NOT NULL, revision INTEGER NOT NULL, account TEXT NOT NULL, state TEXT NOT NULL, deadline INTEGER NOT NULL, payload TEXT NOT NULL, raw BLOB NOT NULL, error TEXT, provider_id TEXT, UNIQUE(draft_id, revision));
        PRAGMA user_version=1;").map_err(error)?;
    }
    if version < 2 {
        tx.execute_batch(
            "ALTER TABLE outbox_messages ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE outbox_messages ADD COLUMN last_attempt_at INTEGER;
            PRAGMA user_version=2;",
        )
        .map_err(error)?;
    }
    if version < 3 {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0;
            CREATE INDEX IF NOT EXISTS threads_trashed ON threads(trashed);
            PRAGMA user_version=3;",
        )
        .map_err(error)?;
    }
    if version < 4 {
        // Gmail thread IDs are unique only within one account, so the bare
        // provider ID can no longer be the uniqueness key once a second
        // account exists. Existing rows all get the 'default' placeholder;
        // `Database::adopt_account` rewrites it onto the real address the
        // first time an account's identity is confirmed.
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN account_id TEXT NOT NULL DEFAULT 'default';
            CREATE UNIQUE INDEX IF NOT EXISTS threads_account_provider_unique
                ON threads(account_id, provider_thread_id);
            PRAGMA user_version=4;",
        )
        .map_err(error)?;
    }
    tx.commit().map_err(error)?;
    connection.execute("UPDATE outbox_messages SET state='uncertain', error='Application stopped during delivery. Check sent mail before sending again.' WHERE state='sending'", []).map_err(error)?;
    connection
        .execute(
            "UPDATE outbox_messages SET deadline=?1 WHERE state='undo_pending'",
            [now() + UNDO_MS],
        )
        .map_err(error)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub ready: bool,
    pub message_id: Option<String>,
    pub provider_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    pub revision: i64,
    pub account: String,
    pub mode: String,
    pub source_id: Option<String>,
    pub thread_id: Option<String>,
    pub reply_id: Option<String>,
    pub references: Vec<String>,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    #[serde(default)]
    pub body_html: String,
    pub attachments: Vec<Attachment>,
    pub updated_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    pub id: String,
    pub draft: Draft,
    pub state: String,
    pub deadline: i64,
    pub error: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    Identity,
    ListDrafts,
    ListOutbox,
    Create {
        mode: String,
        source_id: Option<String>,
    },
    Save {
        draft: Draft,
    },
    Discard {
        id: String,
    },
    Queue {
        id: String,
        revision: i64,
    },
    Cancel {
        id: String,
    },
    Recover {
        id: String,
    },
    Reconcile {
        id: String,
    },
    Attach {
        id: String,
    },
    RemoveAttachment {
        id: String,
        attachment_id: String,
    },
    FetchAttachment {
        id: String,
        attachment_id: String,
    },
}

pub fn addresses(value: &str) -> Result<Vec<(String, String)>, String> {
    if value.trim().is_empty() {
        return Ok(vec![]);
    }
    if value.contains(['\r', '\n']) {
        return Err("Addresses cannot contain newlines".into());
    }
    let raw = format!("To: {value}\r\n\r\n");
    let message = MessageParser::default()
        .parse(raw.as_bytes())
        .ok_or("Invalid recipients")?;
    let list = message.to().ok_or("Invalid recipients")?;
    let mut result = vec![];
    for a in list.iter() {
        let address = a.address().ok_or("Invalid email address")?;
        let (local, domain) = address
            .rsplit_once('@')
            .ok_or("Enter a complete email address")?;
        if local.is_empty() || domain.is_empty() || address.contains([' ', '\t', '<', '>']) {
            return Err("Invalid email address".into());
        }
        result.push((
            a.name().unwrap_or_default().to_string(),
            address.to_string(),
        ));
    }
    if result.is_empty() {
        return Err("Enter an email address".into());
    }
    Ok(result)
}
fn header<'a>(part: &'a MimePart, name: &str) -> &'a str {
    part.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
        .unwrap_or_default()
}
fn reference_ids(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|s| s.trim_matches(['<', '>']).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
fn forward_attachments(part: &MimePart, message_id: &str, result: &mut Vec<Attachment>) {
    if !part.filename.is_empty() {
        result.push(Attachment {
            id: Uuid::new_v4().to_string(),
            name: part.filename.clone(),
            size: part.body.size,
            mime: part.mime_type.clone(),
            ready: false,
            message_id: Some(message_id.into()),
            provider_id: part.body.attachment_id.clone(),
        });
    }
    for child in &part.parts {
        forward_attachments(child, message_id, result);
    }
}

impl Database {
    pub fn compose_identity(&self) -> Result<String, String> {
        self.connection()?
            .query_row(
                "SELECT value FROM compose_settings WHERE key='identity'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?
            .ok_or("Connect Gmail once to establish your sender identity".into())
    }
    pub fn set_compose_identity(&self, identity: &str) -> Result<(), String> {
        self.connection()?.execute("INSERT INTO compose_settings VALUES ('identity',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [identity]).map_err(error)?;
        Ok(())
    }
    pub fn drafts(&self) -> Result<Vec<Draft>, String> {
        let c = self.connection()?;
        let mut q = c
            .prepare("SELECT payload FROM drafts ORDER BY rowid DESC")
            .map_err(error)?;
        let rows = q.query_map([], |r| r.get::<_, String>(0)).map_err(error)?;
        rows.map(|r| serde_json::from_str(&r.map_err(error)?).map_err(error))
            .collect()
    }
    pub fn draft(&self, id: &str) -> Result<Draft, String> {
        let value: String = self
            .connection()?
            .query_row("SELECT payload FROM drafts WHERE id=?1", [id], |r| r.get(0))
            .map_err(|_| "Draft no longer exists".to_string())?;
        serde_json::from_str(&value).map_err(error)
    }
    pub fn create_draft(&self, mode: &str, source_id: Option<String>) -> Result<Draft, String> {
        if !["new", "reply", "replyAll", "forward"].contains(&mode) {
            return Err("Unknown compose mode".into());
        }
        let account = self.compose_identity()?;
        if mode != "new" {
            if let Some(existing) = self
                .drafts()?
                .into_iter()
                .find(|d| d.account == account && d.mode == mode && d.source_id == source_id)
            {
                return Ok(existing);
            }
        }
        let mut d = Draft {
            id: Uuid::new_v4().to_string(),
            revision: 0,
            account: account.clone(),
            mode: mode.into(),
            source_id: source_id.clone(),
            thread_id: None,
            reply_id: None,
            references: vec![],
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            body: String::new(),
            body_html: String::new(),
            attachments: vec![],
            updated_at: now(),
        };
        if mode != "new" {
            let source_id = source_id.ok_or("Select a source message")?;
            let raw: String = self
                .connection()?
                .query_row(
                    "SELECT payload FROM message_metadata WHERE id=?1",
                    [&source_id],
                    |r| r.get(0),
                )
                .map_err(|_| {
                    "Reply metadata is unavailable offline. Refresh mail while connected first."
                        .to_string()
                })?;
            let source: GmailMessage = serde_json::from_str(&raw).map_err(error)?;
            let normalized = crate::mime::normalize(&source)?;
            let part = &source.payload;
            let from = header(part, "From");
            d.subject = normalized.subject.clone();
            let quote = if normalized.body_text.trim().is_empty() {
                mail_parser::decoders::html::html_to_text(&normalized.body_html)
            } else {
                normalized.body_text
            };
            d.body = format!(
                "\n\nOn {}, {} wrote:\n{}",
                normalized.date,
                from,
                quote
                    .lines()
                    .map(|l| format!("> {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            if mode == "forward" {
                d.subject = format!("Fwd: {}", d.subject);
                d.body=format!("\n\n---------- Forwarded message ----------\nFrom: {from}\nDate: {}\nSubject: {}\nTo: {}\n\n{quote}",normalized.date,normalized.subject,header(part,"To"));
                forward_attachments(part, &source.id, &mut d.attachments);
            } else {
                let reply = header(part, "Reply-To");
                let own = addresses(from)?
                    .iter()
                    .any(|(_, a)| a.eq_ignore_ascii_case(&account));
                let target = if own {
                    header(part, "To")
                } else if !reply.is_empty() {
                    reply
                } else {
                    from
                };
                let mut seen = HashSet::from([account.to_lowercase()]);
                let mut collect = |value: &str| -> Result<String, String> {
                    Ok(addresses(value)?
                        .into_iter()
                        .filter_map(|(_, a)| {
                            if seen.insert(a.to_lowercase()) {
                                Some(a)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", "))
                };
                d.to = collect(target)?;
                if mode == "replyAll" {
                    let additional = collect(header(part, "To"))?;
                    if !additional.is_empty() {
                        if !d.to.is_empty() {
                            d.to.push_str(", ");
                        }
                        d.to.push_str(&additional);
                    }
                    d.cc = collect(header(part, "Cc"))?;
                }
                let id = header(part, "Message-ID").trim().trim_matches(['<', '>']);
                if id.is_empty() {
                    return Err(
                        "Source message has no Message-ID; refresh it before replying".into(),
                    );
                }
                d.reply_id = Some(id.into());
                d.thread_id = Some(source.thread_id);
                d.references = reference_ids(header(part, "References"));
                d.references.push(id.into());
                // Gmail requires matching subjects for an existing thread.
            }
        }
        self.connection()?
            .execute(
                "INSERT INTO drafts VALUES (?1,?2,?3)",
                params![d.id, d.revision, json(&d)?],
            )
            .map_err(error)?;
        Ok(d)
    }
    pub fn save_draft(&self, mut draft: Draft) -> Result<Draft, String> {
        let old = self.draft(&draft.id)?;
        // Only editable fields cross the trust boundary; routing and file locators are native-owned.
        if draft.body.len() + draft.body_html.len() > 2 * 1024 * 1024
            || draft.subject.len() > 998
            || draft.to.len() + draft.cc.len() + draft.bcc.len() > 32000
        {
            return Err("Draft is too large".into());
        }
        draft.account = old.account;
        draft.mode = old.mode;
        draft.source_id = old.source_id;
        draft.thread_id = old.thread_id;
        draft.reply_id = old.reply_id;
        draft.references = old.references;
        draft.attachments = old.attachments;
        if draft.subject != old.subject {
            draft.thread_id = None;
        }
        let expected = draft.revision;
        draft.revision += 1;
        draft.updated_at = now();
        let changed = self
            .connection()?
            .execute(
                "UPDATE drafts SET revision=?1,payload=?2 WHERE id=?3 AND revision=?4",
                params![draft.revision, json(&draft)?, draft.id, expected],
            )
            .map_err(error)?;
        if changed != 1 {
            return Err("Draft changed elsewhere. Reopen it before editing.".into());
        }
        Ok(draft)
    }
    pub fn discard_draft(&self, id: &str) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM drafts WHERE id=?1", [id])
            .map_err(error)?;
        Ok(())
    }
    fn save_attachment_draft(&self, mut d: Draft) -> Result<Draft, String> {
        let expected = d.revision;
        d.revision += 1;
        d.updated_at = now();
        let count = self
            .connection()?
            .execute(
                "UPDATE drafts SET revision=?1,payload=?2 WHERE id=?3 AND revision=?4",
                params![d.revision, json(&d)?, d.id, expected],
            )
            .map_err(error)?;
        if count != 1 {
            return Err("Draft changed while preparing attachment; retry".into());
        }
        Ok(d)
    }
    pub fn outbox(&self) -> Result<Vec<OutboxItem>, String> {
        let c = self.connection()?;
        let mut q = c
            .prepare(
                "SELECT id,payload,state,deadline,error FROM outbox_messages ORDER BY rowid DESC",
            )
            .map_err(error)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(error)?;
        rows.map(|r| {
            let (id, payload, state, deadline, error) = r.map_err(error)?;
            Ok(OutboxItem {
                id,
                draft: serde_json::from_str(&payload).map_err(crate::correspondence::error)?,
                state,
                deadline,
                error,
            })
        })
        .collect()
    }
    pub fn queue(&self, id: &str, revision: i64, root: &Path) -> Result<OutboxItem, String> {
        if let Some(item) = self
            .outbox()?
            .into_iter()
            .find(|o| o.draft.id == id && o.draft.revision == revision)
        {
            return Ok(item);
        }
        let d = self.draft(id)?;
        if d.revision != revision {
            return Err("Draft is still saving".into());
        }
        if d.account != self.compose_identity()? {
            return Err("Reconnect the draft's account before sending".into());
        }
        let operation_id = Uuid::new_v4().to_string();
        let raw = build_mime(&d, &operation_id, root)?;
        let item = OutboxItem {
            id: operation_id,
            draft: d.clone(),
            state: "undo_pending".into(),
            deadline: now() + UNDO_MS,
            error: None,
        };
        let mut c = self.connection()?;
        let tx = c.transaction().map_err(error)?;
        let removed = tx
            .execute(
                "DELETE FROM drafts WHERE id=?1 AND revision=?2",
                params![id, revision],
            )
            .map_err(error)?;
        if removed != 1 {
            return Err("Draft changed while preparing send".into());
        }
        tx.execute("INSERT INTO outbox_messages(id,draft_id,revision,account,state,deadline,payload,raw) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![item.id,id,revision,d.account,item.state,item.deadline,json(&d)?,raw]).map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(item)
    }
    pub fn cancel_send(&self, id: &str, recover: bool) -> Result<Draft, String> {
        let item = self
            .outbox()?
            .into_iter()
            .find(|o| o.id == id)
            .ok_or("Outbox item not found")?;
        let allowed = if recover {
            item.state == "failed"
        } else {
            item.state == "undo_pending" || item.state == "ready"
        };
        if !allowed {
            return Err(
                "Delivery has started or is uncertain; it cannot be safely canceled or retried"
                    .into(),
            );
        }
        let mut d = item.draft;
        d.revision += 1;
        let mut c = self.connection()?;
        let tx = c.transaction().map_err(error)?;
        let count = tx
            .execute(
                "UPDATE outbox_messages SET state='canceled' WHERE id=?1 AND state=?2",
                params![id, item.state],
            )
            .map_err(error)?;
        if count != 1 {
            return Err("Delivery has already started".into());
        }
        tx.execute(
            "INSERT INTO drafts VALUES (?1,?2,?3)",
            params![d.id, d.revision, json(&d)?],
        )
        .map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(d)
    }
    pub fn pending_undo(&self) -> Result<bool, String> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT count(*) FROM outbox_messages WHERE state='undo_pending' AND deadline>?1",
                [now()],
                |r| r.get::<_, i64>(0),
            )
            .map_err(error)?
            > 0)
    }
    /// Pauses undo-pending/ready outbox items for one account, so removing a
    /// connected account never pauses another account's in-flight sends.
    pub fn pause_ready_sends_for(&self, account: &str) -> Result<(), String> {
        self.connection()?.execute("UPDATE outbox_messages SET state='failed',error='Account disconnected. Reconnect and restore this draft to send.' WHERE account=?1 AND state IN ('undo_pending','ready')",[account]).map_err(error)?;
        Ok(())
    }
}

fn build_mime(d: &Draft, id: &str, root: &Path) -> Result<Vec<u8>, String> {
    if d.subject.contains(['\r', '\n']) {
        return Err("Subject cannot contain newlines".into());
    }
    let to = addresses(&d.to)?;
    let cc = addresses(&d.cc)?;
    let bcc = addresses(&d.bcc)?;
    if to.len() + cc.len() + bcc.len() == 0 {
        return Err("Add at least one recipient".into());
    }
    let mut builder = MessageBuilder::new()
        .from(d.account.clone())
        .to(to)
        .cc(cc)
        .bcc(bcc)
        .subject(d.subject.clone())
        .text_body(d.body.clone())
        .message_id(format!("{id}@dispatch.local"));
    if !d.body_html.trim().is_empty() {
        builder = builder.html_body(d.body_html.clone());
    }
    if let Some(reply) = &d.reply_id {
        if reply.contains(['\r', '\n']) || d.references.iter().any(|r| r.contains(['\r', '\n'])) {
            return Err("Invalid reply headers".into());
        }
        builder = builder
            .in_reply_to(reply.clone())
            .references(d.references.clone());
    }
    let mut size = d.body.len() + d.body_html.len();
    for attachment in &d.attachments {
        if !attachment.ready {
            return Err(format!(
                "Download or remove {} before sending",
                attachment.name
            ));
        }
        if Uuid::parse_str(&attachment.id).is_err() {
            return Err("Invalid attachment ID".into());
        }
        let path = root.join(&attachment.id);
        let metadata = std::fs::metadata(&path)
            .map_err(|_| format!("Attachment missing: {}", attachment.name))?;
        size += metadata.len() as usize;
        if size > MAX_BYTES * 3 / 4 {
            return Err("Attachments exceed the 18 MB local limit".into());
        }
        let bytes = std::fs::read(path).map_err(error)?;
        builder = builder.attachment(attachment.mime.clone(), attachment.name.clone(), bytes);
    }
    let raw = builder.write_to_vec().map_err(error)?;
    if raw.len() > MAX_BYTES {
        return Err("Encoded message exceeds the 24 MB local limit".into());
    }
    Ok(raw)
}

#[derive(Clone)]
pub struct Correspondence {
    pub database: Arc<Database>,
    pub auth: Option<GoogleAuth>,
    pub root: PathBuf,
    pub gate: Arc<tokio::sync::Mutex<()>>,
    pub edits: Arc<tokio::sync::Mutex<()>>,
}
impl Correspondence {
    fn provider(&self) -> Result<GmailClient, String> {
        Ok(GmailClient::new(
            self.auth.clone().ok_or("Google OAuth is not configured")?,
        ))
    }
    pub fn is_connected(&self) -> bool {
        self.auth.as_ref().is_some_and(GoogleAuth::available)
    }
    pub async fn refresh_identity(&self) -> Result<String, String> {
        let identity = self.provider()?.sender_identity().await.map_err(error)?;
        self.database.set_compose_identity(&identity)?;
        if let Some(auth) = &self.auth {
            auth.accept_identity(&identity)?;
        }
        self.database.adopt_account(&identity)?;
        Ok(identity)
    }
    pub async fn request(&self, request: Request) -> Result<serde_json::Value, String> {
        // Never hold a data lock while a user is choosing files; undo remains responsive.
        let selected_files = if matches!(&request, Request::Attach { .. }) {
            rfd::AsyncFileDialog::new()
                .pick_files()
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let _edits = self.edits.lock().await;
        use Request::*;
        match request {
            Identity => {
                let identity = self.database.compose_identity().map_err(|_| {
                    "Sender identity unavailable. Connect and refresh Gmail first.".to_string()
                });
                Ok(serde_json::to_value(identity?).map_err(error)?)
            }
            ListDrafts => Ok(serde_json::to_value(self.database.drafts()?).map_err(error)?),
            ListOutbox => Ok(serde_json::to_value(self.database.outbox()?).map_err(error)?),
            Create { mode, source_id } => {
                if self.database.compose_identity().is_err() {
                    self.refresh_identity().await?;
                }
                if let Some(id) = &source_id {
                    let missing = self
                        .database
                        .connection()?
                        .query_row(
                            "SELECT count(*) FROM message_metadata WHERE id=?1",
                            [id],
                            |r| r.get::<_, i64>(0),
                        )
                        .map_err(error)?
                        == 0;
                    if missing {
                        let source = self.provider()?.get_message(id).await.map_err(error)?;
                        self.database
                            .connection()?
                            .execute(
                                "INSERT OR REPLACE INTO message_metadata VALUES (?1,?2)",
                                params![id, json(&source)?],
                            )
                            .map_err(error)?;
                    }
                }
                Ok(
                    serde_json::to_value(self.database.create_draft(&mode, source_id)?)
                        .map_err(error)?,
                )
            }
            Save { draft } => {
                Ok(serde_json::to_value(self.database.save_draft(draft)?).map_err(error)?)
            }
            Discard { id } => {
                self.database.discard_draft(&id)?;
                self.cleanup()?;
                Ok(serde_json::Value::Null)
            }
            Queue { id, revision } => Ok(serde_json::to_value(
                self.database.queue(&id, revision, &self.root)?,
            )
            .map_err(error)?),
            Cancel { id } => {
                Ok(serde_json::to_value(self.database.cancel_send(&id, false)?).map_err(error)?)
            }
            Recover { id } => {
                Ok(serde_json::to_value(self.database.cancel_send(&id, true)?).map_err(error)?)
            }
            Reconcile { id } => {
                self.reconcile(&id).await?;
                Ok(serde_json::Value::Null)
            }
            Attach { id } => {
                let mut d = self.database.draft(&id)?;
                if selected_files.is_empty() {
                    return serde_json::to_value(d).map_err(error);
                }
                let mut copied = Vec::new();
                let result = (|| -> Result<Draft, String> {
                    for file in selected_files {
                        let metadata = std::fs::metadata(file.path()).map_err(error)?;
                        if !metadata.is_file() {
                            return Err("Choose a regular file".into());
                        }
                        let size = metadata.len();
                        if size + d.attachments.iter().map(|a| a.size).sum::<u64>()
                            > 18 * 1024 * 1024
                        {
                            return Err("Attachments exceed the 18 MB local limit".into());
                        }
                        let attachment = Attachment {
                            id: Uuid::new_v4().to_string(),
                            name: file.file_name(),
                            size,
                            mime: "application/octet-stream".into(),
                            ready: true,
                            message_id: None,
                            provider_id: None,
                        };
                        let target = self.root.join(&attachment.id);
                        copied.push(target.clone());
                        // Limit the read even if the source grows after the metadata check.
                        use std::io::Read;
                        let mut bytes = Vec::new();
                        std::fs::File::open(file.path())
                            .map_err(error)?
                            .take(18 * 1024 * 1024 + 1)
                            .read_to_end(&mut bytes)
                            .map_err(error)?;
                        if bytes.len() as u64 != size {
                            return Err("File changed while attaching; select it again".into());
                        }
                        std::fs::write(target, bytes).map_err(error)?;
                        d.attachments.push(attachment);
                    }
                    self.database.save_attachment_draft(d)
                })();
                if result.is_err() {
                    for path in copied {
                        let _ = std::fs::remove_file(path);
                    }
                }
                serde_json::to_value(result?).map_err(error)
            }
            RemoveAttachment { id, attachment_id } => {
                let mut d = self.database.draft(&id)?;
                d.attachments.retain(|a| a.id != attachment_id);
                let d = self.database.save_attachment_draft(d)?;
                self.cleanup()?;
                Ok(serde_json::to_value(d).map_err(error)?)
            }
            FetchAttachment { id, attachment_id } => {
                let mut d = self.database.draft(&id)?;
                if self.refresh_identity().await? != d.account {
                    return Err("Reconnect the draft's account".into());
                }
                let a = d
                    .attachments
                    .iter_mut()
                    .find(|a| a.id == attachment_id)
                    .ok_or("Attachment not found")?;
                let message = a.message_id.as_ref().ok_or("No attachment source")?;
                let data = if let Some(provider_id) = &a.provider_id {
                    self.provider()?
                        .attachment_bytes(message, provider_id)
                        .await
                        .map_err(error)?
                } else {
                    let source = self.provider()?.get_message(message).await.map_err(error)?;
                    fn find(part: &MimePart, name: &str) -> Option<String> {
                        if part.filename == name {
                            return part.body.data.clone();
                        }
                        part.parts.iter().find_map(|p| find(p, name))
                    }
                    URL_SAFE_NO_PAD
                        .decode(
                            find(&source.payload, &a.name)
                                .ok_or("Attachment content unavailable")?
                                .trim_end_matches('='),
                        )
                        .map_err(error)?
                };
                if data.len() > 18 * 1024 * 1024 {
                    return Err("Attachment exceeds the 18 MB local limit".into());
                }
                std::fs::write(self.root.join(&a.id), &data).map_err(error)?;
                a.ready = true;
                a.size = data.len() as u64;
                Ok(serde_json::to_value(self.database.save_attachment_draft(d)?).map_err(error)?)
            }
        }
    }
    pub fn cleanup(&self) -> Result<(), String> {
        let mut retained: HashSet<String> = self
            .database
            .drafts()?
            .iter()
            .flat_map(|d| d.attachments.iter().map(|a| a.id.clone()))
            .collect();
        for item in self.database.outbox()? {
            if item.state != "canceled" {
                retained.extend(item.draft.attachments.iter().map(|a| a.id.clone()));
            }
        }
        for entry in std::fs::read_dir(&self.root).map_err(error)? {
            let entry = entry.map_err(error)?;
            let name = entry.file_name().to_string_lossy().to_string();
            if Uuid::parse_str(&name).is_ok() && !retained.contains(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
    pub async fn reconcile(&self, id: &str) -> Result<(), String> {
        let item = self
            .database
            .outbox()?
            .into_iter()
            .find(|o| o.id == id)
            .ok_or("Outbox item not found")?;
        if item.state != "uncertain" {
            return Ok(());
        }
        let provider = self.provider()?;
        if provider.sender_identity().await.map_err(error)? != item.draft.account {
            return Err("Reconnect the original sender account".into());
        }
        if let Some(message) = provider
            .find_sent(id, &item.draft.account)
            .await
            .map_err(error)?
        {
            self.database.connection()?.execute("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2 AND state='uncertain'",params![message.id,id]).map_err(error)?;
            let messages = provider
                .get_thread(&message.thread_id)
                .await
                .map_err(error)?;
            let normalized = messages
                .iter()
                .map(crate::mime::normalize)
                .collect::<Result<Vec<_>, _>>()?;
            self.database
                .upsert_gmail_thread(&item.draft.account, &normalized)?;
        } else {
            return Err("Delivery is still uncertain. No automatic retry was made. Check Gmail Sent before composing another message.".into());
        }
        Ok(())
    }
    pub async fn tick(&self) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        if !self.is_connected() {
            return Ok(());
        }
        let items = self.database.outbox()?;
        if !items
            .iter()
            .any(|o| ["undo_pending", "ready", "uncertain"].contains(&o.state.as_str()))
        {
            return Ok(());
        }
        let provider = self.provider()?;
        let identity = provider.sender_identity().await.map_err(error)?;
        for item in items {
            if item.draft.account != identity {
                self.database.connection()?.execute("UPDATE outbox_messages SET error='Paused: reconnect the original sender account to continue.' WHERE id=?1 AND state IN ('undo_pending','ready')", [&item.id]).map_err(error)?;
                continue;
            }
            if item.state == "uncertain" {
                if item.deadline <= now() {
                    self.database
                        .connection()?
                        .execute(
                            "UPDATE outbox_messages SET deadline=?1 WHERE id=?2",
                            params![now() + 60_000, item.id],
                        )
                        .map_err(error)?;
                    let _ = self.reconcile(&item.id).await;
                }
                continue;
            }
            if !["undo_pending", "ready"].contains(&item.state.as_str()) || item.deadline > now() {
                continue;
            }
            // Obtain authorization before claiming delivery; transport errors after the claim are uncertain.
            let request = provider.prepare_send().await.map_err(error)?;
            let sender = &provider;
            let sent = self
                .dispatch_due(&item, now(), |raw, thread| async move {
                    sender.deliver_once(request, &raw, thread.as_deref()).await
                })
                .await?;
            if let Some(sent) = sent {
                if let Ok(messages) = provider.get_thread(&sent.thread_id).await {
                    if let Ok(normalized) = messages
                        .iter()
                        .map(crate::mime::normalize)
                        .collect::<Result<Vec<_>, _>>()
                    {
                        self.database.upsert_gmail_thread(&identity, &normalized)?;
                    }
                }
            }
        }
        Ok(())
    }
    async fn dispatch_due<F, Fut>(
        &self,
        item: &OutboxItem,
        at: i64,
        deliver: F,
    ) -> Result<Option<crate::gmail::SentMessage>, String>
    where
        F: FnOnce(Vec<u8>, Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<crate::gmail::SentMessage, (bool, String)>>,
    {
        let raw: Vec<u8> = {
            let c = self.database.connection()?;
            c.query_row(
                "SELECT raw FROM outbox_messages WHERE id=?1",
                [&item.id],
                |r| r.get(0),
            )
            .map_err(error)?
        };
        let claimed=self.database.connection()?.execute("UPDATE outbox_messages SET state='sending',attempts=attempts+1,last_attempt_at=?2,error=NULL WHERE id=?1 AND state IN ('undo_pending','ready') AND deadline<=?2",params![item.id,at]).map_err(error)?;
        if claimed != 1 {
            return Ok(None);
        }
        match deliver(raw, item.draft.thread_id.clone()).await {
            Ok(sent) => {
                self.database.connection()?.execute("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2",params![sent.id,item.id]).map_err(error)?;
                Ok(Some(sent))
            }
            Err((definite, message)) => {
                self.database
                    .connection()?
                    .execute(
                        "UPDATE outbox_messages SET state=?1,error=?2 WHERE id=?3",
                        params![
                            if definite { "failed" } else { "uncertain" },
                            message,
                            item.id
                        ],
                    )
                    .map_err(error)?;
                Ok(None)
            }
        }
    }
    pub async fn run(self) {
        loop {
            let delay = if self.tick().await.is_ok() { 2 } else { 30 };
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::MimeHeaders;
    fn database() -> Database {
        let db = Database::open_memory();
        db.set_compose_identity("you@example.com").unwrap();
        db
    }
    fn saved(db: &Database) -> Draft {
        let mut d = db.create_draft("new", None).unwrap();
        d.to = "Jane <jane@example.com>".into();
        d.subject = "Hello".into();
        d.body = "Saved work ✓".into();
        db.save_draft(d).unwrap()
    }
    #[test]
    fn draft_revision_rejects_stale_writes_and_header_injection() {
        let db = database();
        let d = saved(&db);
        let stale = d.clone();
        let mut newer = d.clone();
        newer.body = "New version".into();
        db.save_draft(newer).unwrap();
        assert!(db.save_draft(stale).is_err());
        assert_eq!(db.draft(&d.id).unwrap().body, "New version");
        assert!(addresses("valid@example.com\r\nBcc: victim@example.com").is_err());
        let parsed =
            addresses("\"Doe, Jane\" <jane@example.com>, José <jose@example.com>").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "Doe, Jane");
    }
    #[test]
    fn queue_is_durable_unique_and_cancel_restores_the_snapshot() {
        let db = database();
        let d = saved(&db);
        let item = db.queue(&d.id, d.revision, Path::new("/unused")).unwrap();
        assert!(item.deadline > now());
        assert_eq!(
            db.queue(&d.id, d.revision, Path::new("/unused"))
                .unwrap()
                .id,
            item.id
        );
        assert_eq!(db.outbox().unwrap().len(), 1);
        assert!(db.drafts().unwrap().is_empty());
        let restored = db.cancel_send(&item.id, false).unwrap();
        assert_eq!(restored.body, d.body);
        assert!(restored.revision > d.revision);
        assert!(db.cancel_send(&item.id, false).is_err());
    }
    #[test]
    fn pausing_ready_sends_for_one_account_never_touches_another_accounts_outbox() {
        let db = database();
        let a = saved(&db);
        let item_a = db.queue(&a.id, a.revision, Path::new("/unused")).unwrap();

        db.set_compose_identity("other@example.com").unwrap();
        let b = saved(&db);
        let item_b = db.queue(&b.id, b.revision, Path::new("/unused")).unwrap();

        db.pause_ready_sends_for("you@example.com").unwrap();

        let outbox = db.outbox().unwrap();
        let state_of = |id: &str| outbox.iter().find(|o| o.id == id).unwrap().state.clone();
        assert_eq!(state_of(&item_a.id), "failed");
        assert_eq!(state_of(&item_b.id), "undo_pending");
    }
    #[test]
    fn restart_keeps_drafts_and_never_retries_an_interrupted_send() {
        let path = std::env::temp_dir().join(format!("dispatch-{}.sqlite", Uuid::new_v4()));
        let id;
        {
            let db = Database::open(&path).unwrap();
            db.set_compose_identity("you@example.com").unwrap();
            let d = saved(&db);
            id = d.id.clone();
            let queued = saved(&db);
            let item = db
                .queue(&queued.id, queued.revision, Path::new("/unused"))
                .unwrap();
            db.connection()
                .unwrap()
                .execute(
                    "UPDATE outbox_messages SET state='sending' WHERE id=?1",
                    [item.id],
                )
                .unwrap();
        }
        {
            let db = Database::open(&path).unwrap();
            assert_eq!(db.draft(&id).unwrap().body, "Saved work ✓");
            let item = db.outbox().unwrap().remove(0);
            assert_eq!(item.state, "uncertain");
            assert!(db.cancel_send(&item.id, true).is_err());
        }
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn mime_preserves_unicode_attachments_and_reply_headers() {
        let db = database();
        let mut d = saved(&db);
        d.reply_id = Some("original@example.com".into());
        d.references = vec!["ancestor@example.com".into(), "original@example.com".into()];
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let id = Uuid::new_v4().to_string();
        let bytes = vec![0, 255, 1, 2, 3];
        std::fs::write(root.join(&id), &bytes).unwrap();
        d.attachments.push(Attachment {
            id,
            name: "résumé.bin".into(),
            size: 5,
            mime: "application/octet-stream".into(),
            ready: true,
            message_id: None,
            provider_id: None,
        });
        let raw = build_mime(&d, "test-id", &root).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.message_id(), Some("test-id@dispatch.local"));
        assert!(parsed.body_text(0).unwrap().contains("Saved work ✓"));
        let attachment = parsed.attachment(0).unwrap();
        assert_eq!(attachment.contents(), bytes);
        assert_eq!(attachment.attachment_name(), Some("résumé.bin"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mime_includes_rich_html_and_plain_text_fallback() {
        let db = database();
        let mut d = saved(&db);
        d.body = "Formatted message".into();
        d.body_html = "<p><strong>Formatted</strong> message</p>".into();
        let raw = build_mime(&d, "rich-id", Path::new("/unused")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.body_text(0).as_deref(), Some("Formatted message"));
        assert_eq!(
            parsed.body_html(0).as_deref(),
            Some("<p><strong>Formatted</strong> message</p>")
        );
    }
    #[test]
    fn reply_all_excludes_self_preserves_cc_and_forward_is_not_a_reply() {
        let db = database();
        let source = serde_json::json!({"id":"source","threadId":"thread","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"Reply-To","value":"reply@example.com"},{"name":"To","value":"you@example.com, colleague@example.com"},{"name":"Cc","value":"cc@example.com, colleague@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO message_metadata VALUES ('source',?1)",
                [source.to_string()],
            )
            .unwrap();
        let reply = db.create_draft("replyAll", Some("source".into())).unwrap();
        assert_eq!(reply.to, "reply@example.com, colleague@example.com");
        assert_eq!(reply.cc, "cc@example.com");
        assert_eq!(reply.thread_id, Some("thread".into()));
        let forward = db.create_draft("forward", Some("source".into())).unwrap();
        assert!(forward.to.is_empty());
        assert!(forward.reply_id.is_none());
        assert!(forward.thread_id.is_none());
        assert_eq!(forward.subject, "Fwd: Topic");
    }
    #[test]
    fn migrations_preserve_existing_mail_and_are_repeatable() {
        let db = database();
        let before = db.list_threads().unwrap().len();
        migrate(&mut db.connection().unwrap()).unwrap();
        assert_eq!(db.list_threads().unwrap().len(), before);
    }
    fn service() -> Correspondence {
        Correspondence {
            database: Arc::new(database()),
            auth: None,
            root: PathBuf::from("/unused"),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            edits: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
    #[tokio::test]
    async fn fake_clock_and_provider_prove_undo_deadline_and_single_dispatch() {
        let service = service();
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, &service.root)
            .unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let send = |_, _| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async {
                Ok(crate::gmail::SentMessage {
                    id: "sent".into(),
                    thread_id: "thread".into(),
                })
            }
        };
        service
            .dispatch_due(&item, item.deadline - 1, &send)
            .await
            .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        service
            .dispatch_due(&item, item.deadline, &send)
            .await
            .unwrap();
        service
            .dispatch_due(&item, item.deadline + 1, &send)
            .await
            .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(service.database.outbox().unwrap()[0].state, "sent");
        assert!(service.database.cancel_send(&item.id, false).is_err());
    }
    #[tokio::test]
    async fn canceled_messages_never_dispatch_and_transport_timeouts_are_not_retried() {
        let service = service();
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, &service.root)
            .unwrap();
        service.database.cancel_send(&item.id, false).unwrap();
        service
            .dispatch_due(&item, item.deadline, |_, _| async {
                panic!("Canceled mail must not send");
                #[allow(unreachable_code)]
                Ok(crate::gmail::SentMessage {
                    id: String::new(),
                    thread_id: String::new(),
                })
            })
            .await
            .unwrap();
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, &service.root)
            .unwrap();
        service
            .dispatch_due(&item, item.deadline, |_, _| async {
                Err((false, "Connection dropped after acceptance".into()))
            })
            .await
            .unwrap();
        assert_eq!(service.database.outbox().unwrap()[0].state, "uncertain");
        service
            .dispatch_due(&item, item.deadline + 1, |_, _| async {
                panic!("Uncertain mail must not retry");
                #[allow(unreachable_code)]
                Ok(crate::gmail::SentMessage {
                    id: String::new(),
                    thread_id: String::new(),
                })
            })
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn explicit_provider_rejection_can_restore_a_draft() {
        let service = service();
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, &service.root)
            .unwrap();
        service
            .dispatch_due(&item, item.deadline, |_, _| async {
                Err((true, "Invalid recipient".into()))
            })
            .await
            .unwrap();
        assert_eq!(service.database.outbox().unwrap()[0].state, "failed");
        assert_eq!(
            service.database.cancel_send(&item.id, true).unwrap().body,
            d.body
        );
    }
    #[tokio::test]
    async fn provider_acceptance_followed_by_commit_failure_remains_uncertain_after_restart() {
        let service = service();
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, &service.root)
            .unwrap();
        let result=service.dispatch_due(&item,item.deadline,|_,_|async {
            service.database.connection().unwrap().execute_batch("CREATE TRIGGER fail_ack BEFORE UPDATE ON outbox_messages WHEN NEW.state='sent' BEGIN SELECT RAISE(FAIL,'disk failure'); END;").unwrap();
            Ok(crate::gmail::SentMessage{id:"accepted".into(),thread_id:"thread".into()})
        }).await;
        assert!(result.is_err());
        assert_eq!(service.database.outbox().unwrap()[0].state, "sending");
        migrate(&mut service.database.connection().unwrap()).unwrap();
        assert_eq!(service.database.outbox().unwrap()[0].state, "uncertain");
    }
    #[test]
    fn missing_and_unprepared_attachments_cannot_be_queued() {
        let db = database();
        let mut d = saved(&db);
        d.attachments.push(Attachment {
            id: Uuid::new_v4().to_string(),
            name: "missing.txt".into(),
            size: 10,
            mime: "text/plain".into(),
            ready: false,
            message_id: None,
            provider_id: None,
        });
        assert!(build_mime(&d, "id", Path::new("/unused")).is_err());
        d.attachments[0].ready = true;
        assert!(build_mime(&d, "id", Path::new("/unused")).is_err());
    }
}
