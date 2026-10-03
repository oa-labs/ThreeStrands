//! Local drafts and a send queue deliberately separate from retryable label mutations.
use crate::{
    auth::AccountAuth,
    db::Database,
    limits::MAX_ATTACHMENT_BYTES,
    mime::{MimePart, RawMessage},
    provider::{DeliveryReceipt, MailProvider},
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use chrono::Utc;
use mail_builder::MessageBuilder;
use mail_parser::MessageParser;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

const MAX_ENCODED_MIME_BYTES: usize = 24 * 1024 * 1024;
const UNDO_MS: i64 = 10_000;

pub(crate) fn validate_retention_days(days: Option<i64>) -> Result<(), String> {
    if matches!(days, None | Some(30 | 90 | 365)) {
        Ok(())
    } else {
        Err("Retention must be unlimited or 30, 90, or 365 days".to_string())
    }
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(error)
}
fn now() -> i64 {
    Utc::now().timestamp_millis()
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
    #[serde(default)]
    pub inline: bool,
    #[serde(default)]
    pub content_id: Option<String>,
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
    #[serde(default)]
    pub follow_up_task_id: Option<String>,
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
    pub provider_id: Option<String>,
    /// Set when this send was queued via "send and archive", or when its
    /// conversation was archived while delivery was pending: once the
    /// message actually sends, the thread's INBOX label is stripped again
    /// server-side, since Gmail re-adds INBOX to the thread for the newly
    /// delivered sent message. Without this, a thread archived optimistically
    /// at queue time reappears in the inbox once the delayed send lands.
    pub archive_on_send: bool,
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
        /// The sending account. Required for reply/replyAll/forward (the
        /// source thread's owning account); optional for "new", which falls
        /// back to the most-recently-used account.
        account: Option<String>,
    },
    SetAccount {
        id: String,
        account: String,
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
        #[serde(default)]
        archive_on_send: bool,
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
    AttachInline {
        id: String,
        name: String,
        mime: String,
        data: String,
    },
    ReadInline {
        id: String,
        attachment_id: String,
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

/// Reads addresses already stored from received mail, where a display name
/// may hold an unquoted comma (`Daniel O'Connor, CFA® <dan@example.com>`)
/// that strict parsing rejects. Falls back to the one bracketed or bare
/// address in the value; anything else yields nothing. Never use this to
/// validate addresses the user enters.
pub fn stored_addresses(value: &str) -> Vec<(String, String)> {
    if let Ok(list) = addresses(value) {
        return list;
    }
    let value = value.trim();
    if value.contains(['\r', '\n']) {
        return vec![];
    }
    let (name, address) = match value.strip_suffix('>').and_then(|rest| rest.rsplit_once('<')) {
        Some((name, address)) => (name.trim().trim_matches('"').trim(), address.trim()),
        None => ("", value),
    };
    let valid = address
        .rsplit_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.') && !domain.starts_with('.'))
        && !address.contains([' ', '\t', '<', '>', ',', '"']);
    if valid { vec![(name.to_string(), address.to_string())] } else { vec![] }
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
            inline: false,
            content_id: None,
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
    /// Establishes an initial sending identity without replacing the account
    /// the user most recently chose in an existing installation.
    pub fn ensure_compose_identity(&self, identity: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT INTO compose_settings(key, value) VALUES ('identity', ?1)
                 ON CONFLICT(key) DO NOTHING",
                [identity],
            )
            .map(|_| ())
            .map_err(error)
    }
    /// How long locally cached mail is kept before `prune_expired_threads`
    /// removes it. `None` means unlimited (the default, so nobody's mail
    /// silently disappears the first time this ships).
    pub fn retention_days(&self) -> Result<Option<i64>, String> {
        self.connection()?
            .query_row(
                "SELECT value FROM compose_settings WHERE key='retention_days'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(error)?
            .map(|value| value.parse::<i64>().map_err(error))
            .transpose()
    }
    pub fn set_retention_days(&self, days: Option<i64>) -> Result<(), String> {
        validate_retention_days(days)?;
        let connection = self.connection()?;
        match days {
            Some(days) => connection.execute(
                "INSERT INTO compose_settings VALUES ('retention_days',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![days.to_string()],
            ),
            None => connection.execute("DELETE FROM compose_settings WHERE key='retention_days'", []),
        }
        .map_err(error)?;
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
    pub fn create_draft(
        &self,
        mode: &str,
        source_id: Option<String>,
        account: &str,
    ) -> Result<Draft, String> {
        if !["new", "reply", "replyAll", "forward"].contains(&mode) {
            return Err("Unknown compose mode".into());
        }
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
            account: account.to_string(),
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
            follow_up_task_id: None,
            attachments: vec![],
            updated_at: now(),
        };
        if mode != "new" {
            let source_id = source_id.ok_or("Select a source message")?;
            let raw = self
                .message_metadata(&source_id)
                .ok()
                .flatten()
                .ok_or_else(|| {
                    "Reply metadata is unavailable offline. Refresh mail while connected first."
                        .to_string()
                })?;
            let source: RawMessage = serde_json::from_str(&raw).map_err(error)?;
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
                    .any(|(_, a)| a.eq_ignore_ascii_case(account));
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
        draft.follow_up_task_id = draft.follow_up_task_id.or(old.follow_up_task_id);
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
    /// Changes which account a "new" message sends from. Reply/replyAll/
    /// forward stay locked to their source thread's account, so this
    /// rejects any other mode rather than silently ignoring it.
    pub fn set_draft_account(&self, id: &str, account: &str) -> Result<Draft, String> {
        let mut d = self.draft(id)?;
        if d.mode != "new" {
            return Err("Only new messages can change the sending account".into());
        }
        d.account = account.to_string();
        let expected = d.revision;
        d.revision += 1;
        d.updated_at = now();
        let changed = self
            .connection()?
            .execute(
                "UPDATE drafts SET revision=?1,payload=?2 WHERE id=?3 AND revision=?4",
                params![d.revision, json(&d)?, d.id, expected],
            )
            .map_err(error)?;
        if changed != 1 {
            return Err("Draft changed elsewhere. Reopen it before editing.".into());
        }
        self.set_compose_identity(account)?;
        Ok(d)
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
                "SELECT id,payload,state,deadline,error,provider_id,archive_on_send FROM outbox_messages ORDER BY rowid DESC",
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
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, bool>(6)?,
                ))
            })
            .map_err(error)?;
        rows.map(|r| {
            let (id, payload, state, deadline, error, provider_id, archive_on_send) =
                r.map_err(error)?;
            Ok(OutboxItem {
                id,
                draft: serde_json::from_str(&payload).map_err(crate::correspondence::error)?,
                state,
                deadline,
                error,
                provider_id,
                archive_on_send,
            })
        })
        .collect()
    }
    fn archive_on_send(&self, id: &str) -> Result<bool, String> {
        self.connection()?
            .query_row(
                "SELECT archive_on_send FROM outbox_messages WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(error)
    }
    pub fn queue(
        &self,
        id: &str,
        revision: i64,
        archive_on_send: bool,
        root: &Path,
    ) -> Result<OutboxItem, String> {
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
        // Gated on the draft's own account status, not whichever account is
        // currently active in the UI — otherwise queuing a reply from a
        // non-active account would be rejected even though it can send fine.
        if self
            .get_account(&d.account)?
            .is_some_and(|a| a.status == "needs_reauth")
        {
            return Err("Reconnect the draft's account before sending".into());
        }
        let operation_id = Uuid::new_v4().to_string();
        let sender_name = self
            .get_account(&d.account)?
            .and_then(|account| account.display_name);
        let raw = build_mime(&d, sender_name.as_deref(), &operation_id, root)?;
        let item = OutboxItem {
            id: operation_id,
            draft: d.clone(),
            state: "undo_pending".into(),
            deadline: now() + UNDO_MS,
            error: None,
            provider_id: None,
            archive_on_send,
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
        tx.execute("INSERT INTO outbox_messages(id,draft_id,revision,account,state,deadline,payload,raw,archive_on_send) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![item.id,id,revision,d.account,item.state,item.deadline,json(&d)?,raw,item.archive_on_send]).map_err(error)?;
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

fn build_mime(
    d: &Draft,
    sender_name: Option<&str>,
    id: &str,
    root: &Path,
) -> Result<Vec<u8>, String> {
    if d.subject.contains(['\r', '\n']) {
        return Err("Subject cannot contain newlines".into());
    }
    let to = addresses(&d.to)?;
    let cc = addresses(&d.cc)?;
    let bcc = addresses(&d.bcc)?;
    if to.len() + cc.len() + bcc.len() == 0 {
        return Err("Add at least one recipient".into());
    }
    let builder = MessageBuilder::new();
    let builder = if let Some(name) = sender_name {
        builder.from((name.to_string(), d.account.clone()))
    } else {
        builder.from(d.account.clone())
    };
    let mut builder = builder
        .to(to)
        .cc(cc)
        .bcc(bcc)
        .subject(d.subject.clone())
        .text_body(d.body.clone())
        .message_id(format!("{id}@threestrands.local"));
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
        if size > MAX_ATTACHMENT_BYTES {
            return Err("Attachments exceed the 18 MB local limit".into());
        }
        let bytes = std::fs::read(path).map_err(error)?;
        builder = if attachment.inline {
            let content_id = attachment
                .content_id
                .as_deref()
                .ok_or("Inline image has no content ID")?;
            builder.inline(attachment.mime.clone(), content_id.to_string(), bytes)
        } else {
            builder.attachment(attachment.mime.clone(), attachment.name.clone(), bytes)
        };
    }
    let raw = builder.write_to_vec().map_err(error)?;
    if raw.len() > MAX_ENCODED_MIME_BYTES {
        return Err("Encoded message exceeds the 24 MB local limit".into());
    }
    Ok(raw)
}

#[derive(Clone)]
pub struct Correspondence {
    pub database: Arc<Database>,
    /// Every connected account, shared with `AppState` in `lib.rs` (the same
    /// `Arc`, so add/remove/reconnect there is immediately visible here).
    /// Lets a draft addressed to any connected account resolve and send
    /// through *that* account's credentials, regardless of which account is
    /// active in the UI.
    pub accounts: crate::AccountRegistry,
    pub root: PathBuf,
    pub gate: Arc<tokio::sync::Mutex<()>>,
    pub edits: Arc<tokio::sync::Mutex<()>>,
}
impl Correspondence {
    async fn auth_for(&self, account: &str) -> Option<AccountAuth> {
        self.accounts
            .lock()
            .await
            .get(account)
            .map(|connected| connected.auth.clone())
    }
    /// The backend a draft on `account` reads and sends through. Returned as
    /// a trait object so the compose pipeline never names a concrete service.
    pub(crate) async fn provider_for(
        &self,
        account: &str,
    ) -> Result<Arc<dyn MailProvider>, String> {
        let auth = self.auth_for(account).await.ok_or_else(|| {
            format!("{account} is not connected. Reconnect it before continuing.")
        })?;
        Ok(auth.provider())
    }
    /// The account the compose identity bootstrap runs against. Not the only
    /// account drafts can send from — see `auth_for`.
    async fn primary_auth(&self) -> Option<AccountAuth> {
        self.auth_for(&self.database.primary_account_id()).await
    }
    pub async fn is_connected(&self) -> bool {
        self.primary_auth()
            .await
            .is_some_and(|auth| auth.available())
    }
    pub async fn refresh_identity(&self) -> Result<String, String> {
        let auth = self
            .primary_auth()
            .await
            .ok_or("No mail account is configured")?;
        let identity = auth
            .provider()
            .sender_identity()
            .await
            .map_err(error)?;
        self.database.set_compose_identity(&identity)?;
        auth.accept_identity(&identity)?;
        self.database.adopt_mail_account(&identity, auth.mail_provider())?;
        // `accept_identity` rekeyed `auth` onto the real address; move its
        // registry entry so the map is keyed by that address too. Doing it
        // here covers every caller — startup, `connect_google`, and each
        // `sync_account` — rather than each remembering to.
        crate::rekey_placeholder_account(&self.accounts, &identity).await;
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
            Create {
                mode,
                source_id,
                account,
            } => {
                let resolved_account = match account {
                    Some(account) => account,
                    None if mode == "new" => {
                        if self.database.compose_identity().is_err() {
                            self.refresh_identity().await?;
                        }
                        self.database.compose_identity()?
                    }
                    None => return Err("Reopen the thread before replying".into()),
                };
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
                        let source = self
                            .provider_for(&resolved_account)
                            .await?
                            .fetch_message(id)
                            .await
                            .map_err(error)?;
                        self.database.put_message_metadata(id, &json(&source)?)?;
                    }
                }
                let draft = self
                    .database
                    .create_draft(&mode, source_id, &resolved_account)?;
                if mode == "new" {
                    let _ = self.database.set_compose_identity(&resolved_account);
                }
                Ok(serde_json::to_value(draft).map_err(error)?)
            }
            SetAccount { id, account } => Ok(serde_json::to_value(
                self.database.set_draft_account(&id, &account)?,
            )
            .map_err(error)?),
            Save { draft } => {
                Ok(serde_json::to_value(self.database.save_draft(draft)?).map_err(error)?)
            }
            Discard { id } => {
                self.database.discard_draft(&id)?;
                self.cleanup()?;
                Ok(serde_json::Value::Null)
            }
            Queue {
                id,
                revision,
                archive_on_send,
            } => Ok(serde_json::to_value(self.database.queue(
                &id,
                revision,
                archive_on_send,
                &self.root,
            )?)
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
                            > MAX_ATTACHMENT_BYTES as u64
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
                            inline: false,
                            content_id: None,
                        };
                        let target = self.root.join(&attachment.id);
                        copied.push(target.clone());
                        // Limit the read even if the source grows after the metadata check.
                        use std::io::Read;
                        let mut bytes = Vec::new();
                        std::fs::File::open(file.path())
                            .map_err(error)?
                            .take((MAX_ATTACHMENT_BYTES + 1) as u64)
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
            AttachInline {
                id,
                name,
                mime,
                data,
            } => {
                let mime = mime.to_ascii_lowercase();
                if !crate::image_format::is_supported_raster_mime(&mime) {
                    return Err("Paste a supported image format".into());
                }
                let bytes = STANDARD
                    .decode(data)
                    .map_err(|_| "Pasted image data is invalid")?;
                if bytes.is_empty() {
                    return Err("Pasted image is empty".into());
                }
                let mut d = self.database.draft(&id)?;
                if bytes.len() as u64
                    + d.attachments
                        .iter()
                        .map(|attachment| attachment.size)
                        .sum::<u64>()
                    > MAX_ATTACHMENT_BYTES as u64
                {
                    return Err("Attachments exceed the 18 MB local limit".into());
                }
                let attachment_id = Uuid::new_v4().to_string();
                let content_id = format!("{attachment_id}@threestrands.local");
                let safe_name = Path::new(&name)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
                    .unwrap_or("pasted-image")
                    .to_string();
                let attachment = Attachment {
                    id: attachment_id.clone(),
                    name: safe_name,
                    size: bytes.len() as u64,
                    mime,
                    ready: true,
                    message_id: None,
                    provider_id: None,
                    inline: true,
                    content_id: Some(content_id),
                };
                let target = self.root.join(&attachment_id);
                std::fs::write(&target, bytes).map_err(error)?;
                d.attachments.push(attachment);
                match self.database.save_attachment_draft(d) {
                    Ok(saved) => serde_json::to_value(saved).map_err(error),
                    Err(error) => {
                        let _ = std::fs::remove_file(target);
                        Err(error)
                    }
                }
            }
            ReadInline { id, attachment_id } => {
                // Queueing removes the editable draft row, but the UI keeps
                // rendering its reply optimistically during undo/delivery.
                // Resolve inline images from the outbox payload in that gap.
                let d = match self.database.draft(&id) {
                    Ok(draft) => draft,
                    Err(draft_error) => self
                        .database
                        .outbox()?
                        .into_iter()
                        .find(|item| item.draft.id == id && item.state != "canceled")
                        .map(|item| item.draft)
                        .ok_or(draft_error)?,
                };
                let attachment = d
                    .attachments
                    .iter()
                    .find(|attachment| attachment.id == attachment_id && attachment.inline)
                    .ok_or("Inline image not found")?;
                if !attachment.ready {
                    return Err("Inline image is unavailable".into());
                }
                let bytes = std::fs::read(self.root.join(&attachment.id))
                    .map_err(|_| "Inline image data is unavailable")?;
                if bytes.len() as u64 != attachment.size {
                    return Err("Inline image data is incomplete".into());
                }
                Ok(serde_json::Value::String(format!(
                    "data:{};base64,{}",
                    attachment.mime,
                    STANDARD.encode(bytes)
                )))
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
                let provider = self.provider_for(&d.account).await?;
                if provider.sender_identity().await.map_err(error)? != d.account {
                    return Err("Reconnect the draft's account".into());
                }
                let a = d
                    .attachments
                    .iter_mut()
                    .find(|a| a.id == attachment_id)
                    .ok_or("Attachment not found")?;
                let message = a.message_id.as_ref().ok_or("No attachment source")?;
                let data = if let Some(provider_id) = &a.provider_id {
                    provider
                        .attachment_bytes(message, provider_id)
                        .await
                        .map_err(error)?
                } else {
                    let source = provider.fetch_message(message).await.map_err(error)?;
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
                if data.len() > MAX_ATTACHMENT_BYTES {
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
        let provider = self.provider_for(&item.draft.account).await?;
        if provider.sender_identity().await.map_err(error)? != item.draft.account {
            return Err("Reconnect the original sender account".into());
        }
        if let Some(message) = provider
            .find_sent_copy(id, &item.draft.account)
            .await
            .map_err(error)?
        {
            self.database.connection()?.execute("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2 AND state='uncertain'",params![message.provider_message_id,id]).map_err(error)?;
            if let Some(thread_id) = message.thread_id.as_deref() {
                if self.database.archive_on_send(&item.id)? {
                    let _ = provider
                        .modify_thread(thread_id, &[], &["INBOX".to_string()])
                        .await;
                }
                let messages = provider.fetch_thread(thread_id).await.map_err(error)?;
                let normalized = messages
                    .iter()
                    .map(crate::mime::normalize)
                    .collect::<Result<Vec<_>, _>>()?;
                self.database
                    .upsert_thread(&item.draft.account, &normalized)?;
            }
        } else {
            return Err("Delivery is still uncertain. No automatic retry was made. Check Gmail Sent before composing another message.".into());
        }
        Ok(())
    }
    pub async fn tick(&self) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let items = self.database.outbox()?;
        if !items
            .iter()
            .any(|o| ["undo_pending", "ready", "uncertain"].contains(&o.state.as_str()))
        {
            return Ok(());
        }
        // Grouped by account and processed independently: one account's
        // disconnected/rate-limited provider must never block another
        // account's queued sends.
        let mut by_account: HashMap<String, Vec<OutboxItem>> = HashMap::new();
        for item in items {
            by_account
                .entry(item.draft.account.clone())
                .or_default()
                .push(item);
        }
        for (account, items) in by_account {
            let Ok(provider) = self.provider_for(&account).await else {
                continue;
            };
            let Ok(identity) = provider.sender_identity().await else {
                continue;
            };
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
                if !["undo_pending", "ready"].contains(&item.state.as_str())
                    || item.deadline > now()
                {
                    continue;
                }
                // Obtain authorization before claiming delivery; transport errors after the claim are uncertain.
                let permit = provider.prepare_delivery().await.map_err(error)?;
                let sent = self
                    .dispatch_due(&item, now(), |raw, thread| async move {
                        permit.send_once(&raw, thread.as_deref()).await
                    })
                    .await?;
                if let Some(sent) = sent {
                    if let Some(thread_id) = sent.thread_id.as_deref() {
                        if self.database.archive_on_send(&item.id)? {
                            // Gmail attaches INBOX to the thread when the
                            // sent message lands, which would otherwise
                            // silently undo the archive done at queue time.
                            // Re-assert it here, before the refetch below,
                            // so the upserted thread reflects the archived
                            // state regardless of timing.
                            let _ = provider
                                .modify_thread(thread_id, &[], &["INBOX".to_string()])
                                .await;
                        }
                        if let Ok(messages) = provider.fetch_thread(thread_id).await {
                            if let Ok(normalized) = messages
                                .iter()
                                .map(crate::mime::normalize)
                                .collect::<Result<Vec<_>, _>>()
                            {
                                self.database.upsert_thread(&identity, &normalized)?;
                            }
                        }
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
    ) -> Result<Option<DeliveryReceipt>, String>
    where
        F: FnOnce(Vec<u8>, Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<DeliveryReceipt, (bool, String)>>,
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
                self.database.connection()?.execute("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2",params![sent.provider_message_id,item.id]).map_err(error)?;
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
        db.adopt_account("you@example.com").unwrap();
        db
    }
    fn saved(db: &Database) -> Draft {
        let account = db.compose_identity().unwrap();
        let mut d = db.create_draft("new", None, &account).unwrap();
        d.to = "Jane <jane@example.com>".into();
        d.subject = "Hello".into();
        d.body = "Saved work ✓".into();
        db.save_draft(d).unwrap()
    }
    #[test]
    fn retention_days_defaults_to_unlimited_and_round_trips() {
        let db = database();
        assert_eq!(db.retention_days().unwrap(), None);
        db.set_retention_days(Some(90)).unwrap();
        assert_eq!(db.retention_days().unwrap(), Some(90));
        db.set_retention_days(None).unwrap();
        assert_eq!(db.retention_days().unwrap(), None);
    }

    #[test]
    fn retention_days_rejects_values_outside_the_supported_options() {
        let db = database();
        db.set_retention_days(Some(90)).unwrap();

        for days in [-1, 0, 1, 31, 366] {
            assert!(db.set_retention_days(Some(days)).is_err());
            assert_eq!(db.retention_days().unwrap(), Some(90));
        }
    }

    #[test]
    fn reconnect_bootstrap_sets_only_a_missing_compose_identity() {
        let db = Database::open_memory();
        db.adopt_account("first@example.com").unwrap();
        db.ensure_compose_identity("first@example.com").unwrap();
        assert_eq!(db.compose_identity().unwrap(), "first@example.com");

        db.adopt_account("second@example.com").unwrap();
        db.ensure_compose_identity("second@example.com").unwrap();
        assert_eq!(db.compose_identity().unwrap(), "first@example.com");
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
    fn stored_addresses_accept_unquoted_commas_that_strict_parsing_rejects() {
        let raw = "Daniel O'Connor, CFA® <doconnor@wealth.example>";
        assert!(addresses(raw).is_err());
        assert_eq!(stored_addresses(raw), vec![("Daniel O'Connor, CFA®".to_string(), "doconnor@wealth.example".to_string())]);
        assert_eq!(stored_addresses("Smith, Pat, PhD <pat@lab.example>"), vec![("Smith, Pat, PhD".to_string(), "pat@lab.example".to_string())]);
        assert_eq!(stored_addresses("\"Doe, Jane\" <jane@example.com>"), vec![("Doe, Jane".to_string(), "jane@example.com".to_string())]);
        assert_eq!(stored_addresses("bare@example.com"), vec![(String::new(), "bare@example.com".to_string())]);
    }

    #[test]
    fn stored_addresses_yield_nothing_for_values_without_one_clear_address() {
        for raw in ["", "Daniel, CFA", "Name <not-an-address>", "Name <a b@example.com>", "Name <@example.com>", "a@example.com\r\nBcc: victim@example.com"] {
            assert!(stored_addresses(raw).is_empty(), "{raw}");
        }
    }

    #[test]
    fn queue_is_durable_unique_and_cancel_restores_the_snapshot() {
        let db = database();
        db.set_account_display_name("you@example.com", Some("Joel Reed"))
            .unwrap();
        let d = saved(&db);
        let item = db.queue(&d.id, d.revision, false, Path::new("/unused")).unwrap();
        let raw: Vec<u8> = db
            .connection()
            .unwrap()
            .query_row(
                "SELECT raw FROM outbox_messages WHERE id=?1",
                [&item.id],
                |row| row.get(0),
            )
            .unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            parsed
                .from()
                .and_then(|from| from.first())
                .and_then(|from| from.name()),
            Some("Joel Reed")
        );
        assert!(item.deadline > now());
        assert_eq!(
            db.queue(&d.id, d.revision, false, Path::new("/unused"))
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
    fn archive_after_queue_keeps_only_matching_sends_archived() {
        use crate::models::ThreadMutation;

        let db = database();
        for (account, provider_thread_id) in [
            ("you@example.com", "reply-thread"),
            ("you@example.com", "other-thread"),
            ("other@example.com", "reply-thread"),
        ] {
            db.connection().unwrap().execute(
                "INSERT INTO threads SELECT ?1, ?2, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed, ?3,
                    summary, summary_generated_at, has_attachments, last_received_at
                 FROM threads WHERE id='welcome'",
                params![format!("{account}:{provider_thread_id}"), provider_thread_id, account],
            ).unwrap();
        }
        let queue_for = |account: &str, thread_id: &str, archive_on_send: bool| {
            db.set_compose_identity(account).unwrap();
            db.adopt_account(account).unwrap();
            let draft = saved(&db);
            // Native reply creation owns routing fields; a regular save
            // deliberately refuses to change them.
            db.connection().unwrap().execute(
                "UPDATE drafts SET payload=json_set(payload, '$.mode', 'reply', '$.threadId', ?1)
                 WHERE id=?2",
                params![thread_id, draft.id],
            ).unwrap();
            db.queue(&draft.id, draft.revision, archive_on_send, Path::new("/unused")).unwrap()
        };
        let target = queue_for("you@example.com", "reply-thread", false);
        let already_archiving = queue_for("you@example.com", "reply-thread", true);
        let other_thread = queue_for("you@example.com", "other-thread", false);
        let other_account = queue_for("other@example.com", "reply-thread", false);

        let archive = |value| db.mutate_thread(&ThreadMutation::Archive {
            thread_id: "you@example.com:reply-thread".into(), value,
        }).unwrap();
        archive(true);
        assert!(db.archive_on_send(&target.id).unwrap());
        assert!(db.archive_on_send(&already_archiving.id).unwrap());
        assert!(!db.archive_on_send(&other_thread.id).unwrap());
        assert!(!db.archive_on_send(&other_account.id).unwrap());

        archive(false);
        assert!(!db.archive_on_send(&target.id).unwrap());
        assert!(!db.archive_on_send(&already_archiving.id).unwrap());

        // A completed send follows the ordinary archive mutation, rather
        // than retaining a stale post-send instruction.
        db.connection().unwrap().execute(
            "UPDATE outbox_messages SET state='sent' WHERE id=?1", [&target.id],
        ).unwrap();
        archive(true);
        assert!(!db.archive_on_send(&target.id).unwrap());
        assert!(db.archive_on_send(&already_archiving.id).unwrap());
    }
    #[test]
    fn pausing_ready_sends_for_one_account_never_touches_another_accounts_outbox() {
        let db = database();
        let a = saved(&db);
        let item_a = db.queue(&a.id, a.revision, false, Path::new("/unused")).unwrap();

        db.set_compose_identity("other@example.com").unwrap();
        let b = saved(&db);
        let item_b = db.queue(&b.id, b.revision, false, Path::new("/unused")).unwrap();

        db.pause_ready_sends_for("you@example.com").unwrap();

        let outbox = db.outbox().unwrap();
        let state_of = |id: &str| outbox.iter().find(|o| o.id == id).unwrap().state.clone();
        assert_eq!(state_of(&item_a.id), "failed");
        assert_eq!(state_of(&item_b.id), "undo_pending");
    }
    #[test]
    fn restart_keeps_drafts_and_never_retries_an_interrupted_send() {
        let temp = crate::db::tests::TempDbPath::new();
        let path = &temp.path;
        let id;
        {
            let db = Database::open(path).unwrap();
            db.set_compose_identity("you@example.com").unwrap();
            let d = saved(&db);
            id = d.id.clone();
            let queued = saved(&db);
            let item = db
                .queue(&queued.id, queued.revision, false, Path::new("/unused"))
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
            let db = Database::open(path).unwrap();
            assert_eq!(db.draft(&id).unwrap().body, "Saved work ✓");
            let item = db.outbox().unwrap().remove(0);
            assert_eq!(item.state, "uncertain");
            assert!(db.cancel_send(&item.id, true).is_err());
        }
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
            inline: false,
            content_id: None,
        });
        let raw = build_mime(&d, Some("Joel Reed"), "test-id", &root).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.message_id(), Some("test-id@threestrands.local"));
        assert_eq!(
            parsed
                .from()
                .and_then(|from| from.first())
                .and_then(|from| from.name()),
            Some("Joel Reed")
        );
        assert_eq!(
            parsed
                .from()
                .and_then(|from| from.first())
                .and_then(|from| from.address()),
            Some(d.account.as_str())
        );
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
        let raw = build_mime(&d, None, "rich-id", Path::new("/unused")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.body_text(0).as_deref(), Some("Formatted message"));
        assert_eq!(
            parsed.body_html(0).as_deref(),
            Some("<p><strong>Formatted</strong> message</p>")
        );
    }
    #[test]
    fn mime_embeds_inline_images_with_their_content_id() {
        let db = database();
        let mut d = saved(&db);
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let attachment_id = Uuid::new_v4().to_string();
        let content_id = format!("{attachment_id}@threestrands.local");
        let bytes = vec![137, 80, 78, 71];
        std::fs::write(root.join(&attachment_id), &bytes).unwrap();
        d.body_html = format!("<p>Screenshot</p><img src=\"cid:{content_id}\">");
        d.attachments.push(Attachment {
            id: attachment_id,
            name: "screenshot.png".into(),
            size: bytes.len() as u64,
            mime: "image/png".into(),
            ready: true,
            message_id: None,
            provider_id: None,
            inline: true,
            content_id: Some(content_id.clone()),
        });

        let raw = build_mime(&d, None, "inline-id", &root).unwrap();
        let source = String::from_utf8_lossy(&raw);
        assert!(source.contains(&format!("Content-ID: <{content_id}>")));
        assert!(source.contains("Content-Disposition: inline"));
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert!(parsed
            .body_html(0)
            .unwrap()
            .contains(&format!("src=\"cid:{content_id}\"")));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn reply_all_excludes_self_preserves_cc_and_forward_is_not_a_reply() {
        let db = database();
        let source = serde_json::json!({"id":"source","threadId":"thread","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"Reply-To","value":"reply@example.com"},{"name":"To","value":"you@example.com, colleague@example.com"},{"name":"Cc","value":"cc@example.com, colleague@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO message_metadata(id, payload) VALUES ('source',?1)",
                [source.to_string()],
            )
            .unwrap();
        let reply = db
            .create_draft("replyAll", Some("source".into()), "you@example.com")
            .unwrap();
        assert_eq!(reply.to, "reply@example.com, colleague@example.com");
        assert_eq!(reply.cc, "cc@example.com");
        assert_eq!(reply.thread_id, Some("thread".into()));
        let forward = db
            .create_draft("forward", Some("source".into()), "you@example.com")
            .unwrap();
        assert!(forward.to.is_empty());
        assert!(forward.reply_id.is_none());
        assert!(forward.thread_id.is_none());
        assert_eq!(forward.subject, "Fwd: Topic");
    }
    #[test]
    fn reply_drafts_are_stamped_with_the_passed_account_not_the_global_compose_identity() {
        let db = database();
        // Simulate a different account being "active" for new messages than
        // the one this reply must actually send from.
        db.set_compose_identity("active@example.com").unwrap();
        let source = serde_json::json!({"id":"source-b","threadId":"thread-b","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source-b@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO message_metadata(id, payload) VALUES ('source-b',?1)",
                [source.to_string()],
            )
            .unwrap();
        let reply = db
            .create_draft("reply", Some("source-b".into()), "you@example.com")
            .unwrap();
        assert_eq!(reply.account, "you@example.com");
    }
    #[test]
    fn only_new_messages_can_change_their_sending_account() {
        let db = database();
        let d = saved(&db);
        let updated = db.set_draft_account(&d.id, "other@example.com").unwrap();
        assert_eq!(updated.account, "other@example.com");
        assert!(updated.revision > d.revision);

        let source = serde_json::json!({"id":"source-c","threadId":"thread-c","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source-c@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO message_metadata(id, payload) VALUES ('source-c',?1)",
                [source.to_string()],
            )
            .unwrap();
        let reply = db
            .create_draft("reply", Some("source-c".into()), "you@example.com")
            .unwrap();
        assert!(db.set_draft_account(&reply.id, "nope@example.com").is_err());
    }
    #[test]
    fn queuing_a_draft_no_longer_depends_on_which_account_is_currently_active() {
        let db = database();
        // The globally "active" compose identity differs from the drafted
        // account; queuing must still succeed, since replying from a
        // non-active account is exactly what multi-account send-as needs.
        db.set_compose_identity("active@example.com").unwrap();
        let mut d = db.create_draft("new", None, "you@example.com").unwrap();
        d.to = "Jane <jane@example.com>".into();
        d.subject = "Hello".into();
        d.body = "Body".into();
        let d = db.save_draft(d).unwrap();
        let item = db.queue(&d.id, d.revision, false, Path::new("/unused")).unwrap();
        assert_eq!(item.draft.account, "you@example.com");
    }
    #[test]
    fn queuing_rejects_a_draft_whose_account_needs_reauth() {
        let db = database();
        db.adopt_account("you@example.com").unwrap();
        db.connection()
            .unwrap()
            .execute(
                "UPDATE accounts SET status='needs_reauth' WHERE email='you@example.com'",
                [],
            )
            .unwrap();
        let d = saved(&db);
        assert!(db.queue(&d.id, d.revision, false, Path::new("/unused")).is_err());
    }
    #[test]
    fn rerunning_migrations_on_a_current_database_keeps_its_threads() {
        let db = database();
        let before = db.list_threads(None).unwrap().len();
        assert!(before > 0, "the fixture database must hold mail for this check to mean anything");
        crate::schema::migrate(&mut db.connection().unwrap()).unwrap();
        assert_eq!(db.list_threads(None).unwrap().len(), before);
    }
    fn service() -> Correspondence {
        Correspondence {
            database: Arc::new(database()),
            accounts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
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
            .queue(&d.id, d.revision, false, &service.root)
            .unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let send = |_, _| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async {
                Ok(DeliveryReceipt {
                    provider_message_id: "sent".into(),
                    thread_id: Some("thread".into()),
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
            .queue(&d.id, d.revision, false, &service.root)
            .unwrap();
        service.database.cancel_send(&item.id, false).unwrap();
        service
            .dispatch_due(&item, item.deadline, |_, _| async {
                panic!("Canceled mail must not send");
                #[allow(unreachable_code)]
                Ok(DeliveryReceipt {
                    provider_message_id: String::new(),
                    thread_id: Some(String::new()),
                })
            })
            .await
            .unwrap();
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, false, &service.root)
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
                Ok(DeliveryReceipt {
                    provider_message_id: String::new(),
                    thread_id: Some(String::new()),
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
            .queue(&d.id, d.revision, false, &service.root)
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
            .queue(&d.id, d.revision, false, &service.root)
            .unwrap();
        let result=service.dispatch_due(&item,item.deadline,|_,_|async {
            service.database.connection().unwrap().execute_batch("CREATE TRIGGER fail_ack BEFORE UPDATE ON outbox_messages WHEN NEW.state='sent' BEGIN SELECT RAISE(FAIL,'disk failure'); END;").unwrap();
            Ok(DeliveryReceipt{provider_message_id:"accepted".into(),thread_id:Some("thread".into())})
        }).await;
        assert!(result.is_err());
        assert_eq!(service.database.outbox().unwrap()[0].state, "sending");
        crate::schema::migrate(&mut service.database.connection().unwrap()).unwrap();
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
            inline: false,
            content_id: None,
        });
        assert!(build_mime(&d, None, "id", Path::new("/unused")).is_err());
        d.attachments[0].ready = true;
        assert!(build_mime(&d, None, "id", Path::new("/unused")).is_err());
    }
    #[test]
    fn mime_rejects_header_injection_and_unsafe_attachment_ids() {
        let db = database();
        let base = saved(&db);

        for subject in ["Hello\r\nBcc: attacker@example.net", "Hello\nBcc: attacker@example.net", "Hello\r"] {
            let mut d = base.clone();
            d.subject = subject.into();
            assert_eq!(
                build_mime(&d, None, "id", Path::new("/unused")).unwrap_err(),
                "Subject cannot contain newlines"
            );
        }

        let mut d = base.clone();
        d.reply_id = Some("original@example.com\r\nBcc: attacker@example.net".into());
        assert_eq!(build_mime(&d, None, "id", Path::new("/unused")).unwrap_err(), "Invalid reply headers");

        let mut d = base.clone();
        d.reply_id = Some("original@example.com".into());
        d.references = vec!["ancestor@example.com".into(), "x@example.com\nBcc: attacker@example.net".into()];
        assert_eq!(build_mime(&d, None, "id", Path::new("/unused")).unwrap_err(), "Invalid reply headers");

        // A path-shaped attachment id must never be joined onto the
        // attachment root, even when a file exists at the traversal target.
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        let inner = root.join("attachments");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(root.join("x"), b"outside the attachment root").unwrap();
        for id in ["../x", "../../x", "/etc/passwd", "not-a-uuid", ""] {
            let mut d = base.clone();
            d.attachments.push(Attachment {
                id: id.into(),
                name: "x".into(),
                size: 1,
                mime: "text/plain".into(),
                ready: true,
                message_id: None,
                provider_id: None,
                inline: false,
                content_id: None,
            });
            assert_eq!(build_mime(&d, None, "id", &inner).unwrap_err(), "Invalid attachment ID", "id {id:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
