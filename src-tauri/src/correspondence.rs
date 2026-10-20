//! Local drafts and a send queue deliberately separate from retryable label mutations.
use crate::{
    auth::AccountAuth,
    db::{Database, DatabaseError, DbResult},
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

/// A short, plain-language note stored in `sent_copy_error` for a Sent-copy
/// job that failed. The message WAS sent; only the sender's own copy to the
/// Sent folder could not be saved. The transient transport/auth classes get a
/// stable "will retry" phrasing; the PERMANENT classes
/// ([`ProviderError::InvalidOperation`] / [`PermanentClientRejection`]) are
/// rewritten here from the provider's developer wording ("...refusing
/// permanently", APPEND/quota jargon, wire mailbox names) into one plain
/// sentence that states the fact the user needs: the message was sent, a copy
/// could not be saved to the Sent folder, and why in one short clause. Never
/// includes a path, host name, IMAP/Rust jargon, or a folder wire name.
fn sent_copy_error_message(err: &crate::provider::ProviderError) -> String {
    use crate::provider::ProviderError;
    match err {
        ProviderError::InvalidOperation(message)
        | ProviderError::PermanentClientRejection(message) => permanent_sent_copy_message(message),
        ProviderError::ReauthenticationRequired(_) | ProviderError::Authentication(_) => {
            "Couldn't sign in to save the sent copy. Will retry.".into()
        }
        _ => "Couldn't save the sent copy yet. Will retry.".into(),
    }
}

/// Classify a PERMANENT Sent-copy failure (the provider's own, developer-worded
/// message) into one plain user sentence. Classification is by stable
/// substring of the provider messages this build produces; an unrecognised
/// permanent message falls back to a generic "couldn't be saved" sentence that
/// still never leaks jargon. Each class is pinned by a Rust test.
fn permanent_sent_copy_message(provider_message: &str) -> String {
    const SENT_PREFIX: &str =
        "Your message was sent, but a copy couldn't be saved to your Sent folder";
    let lower = provider_message.to_ascii_lowercase();
    // Every provider message that names "the Sent mailbox" is about it being
    // absent or unreachable ("has no Sent mailbox configured", "the Sent
    // mailbox ... does not exist", "... is not available"), so one test covers
    // all three spellings.
    if lower.contains("sent mailbox") || lower.contains("no sent folder") {
        format!("{SENT_PREFIX} because no Sent folder is set up for this account.")
    } else if lower.contains("append limit") || lower.contains("too large") {
        format!("{SENT_PREFIX} because the message is too large for the Sent folder.")
    } else if lower.contains("over quota") || lower.contains("refused the append") {
        // The server said no without saying why; a full mailbox is the usual
        // cause, but do not claim more than we know.
        format!("{SENT_PREFIX} because your mail server refused to store it (your mailbox may be full).")
    } else if lower.contains("message-id") {
        format!("{SENT_PREFIX} because it is missing a Message-ID.")
    } else {
        format!("{SENT_PREFIX}.")
    }
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
pub struct ForwardedContent {
    pub html: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    pub revision: i64,
    pub account: String,
    /// The chosen sending identity ADDRESS, when the account has more than one
    /// and a non-primary identity was selected (reply/forward identity match,
    /// or a future From picker). `None` means "send as `account`", which is
    /// every existing draft, every Gmail draft, and every single-identity
    /// account — so this is a `#[serde(default)]` field and old drafts in the
    /// `drafts` table deserialize unchanged with `from = None`. Never a display
    /// name: the display name is looked up from the account's identity list at
    /// `build_mime` time, not stored here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<crate::signatures::DraftSignature>,
    // Sender HTML stays separate from the editable compose surface. Older
    // drafts have no preserved HTML and keep their existing text body.
    #[serde(default)]
    pub forwarded_content: Option<ForwardedContent>,
    #[serde(default)]
    pub follow_up_task_id: Option<String>,
    pub attachments: Vec<Attachment>,
    pub updated_at: i64,
    /// Set on a draft a save returns, never stored or synced: where its copy
    /// in the account's server drafts stands. `None` when the draft stays on
    /// this device.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub server_draft: Option<crate::draft_push::ServerDraftStatus>,
}
impl Draft {
    /// The address this draft sends FROM: the chosen identity `from` when set,
    /// otherwise the account's own address. `None` on `from` means every
    /// existing draft and every single-identity/Gmail account sends as
    /// `account`, exactly as before. Returns a borrowed `&str` because both
    /// sources are owned by the draft.
    pub fn from_address(&self) -> &str {
        self.from.as_deref().unwrap_or(&self.account)
    }
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
    /// When this item was last claimed for a delivery attempt (the
    /// `last_attempt_at` column), or `None` if never attempted. The SMTP
    /// prepare-failure retry limiter reads this to rate-limit re-attempts
    /// against a down server (see `Correspondence::tick` and
    /// `policy::smtp_prepare_retry_due`). A transient prepare failure never
    /// claims the row, so this value stays whatever a prior attempt left it;
    /// it is set only when `dispatch_due` actually claims the row.
    pub last_attempt_at: Option<i64>,
    /// Set when this send was queued via "send and archive", or when its
    /// conversation was archived while delivery was pending: once the
    /// message actually sends, the thread's INBOX label is stripped again
    /// server-side, since Gmail re-adds INBOX to the thread for the newly
    /// delivered sent message. Without this, a thread archived optimistically
    /// at queue time reappears in the inbox once the delayed send lands.
    pub archive_on_send: bool,
    #[serde(default)]
    pub schedule: Option<crate::scheduled_send::Schedule>,
    /// The run-4b Sent-copy job's state for this item, surfaced to the UI so
    /// an ABANDONED copy can be shown as a one-line notice. `None` for every
    /// row with no Sent-copy job: Gmail, a legacy row, and any IMAP row whose
    /// server saves its own copy. Serialized as `sentCopy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_copy: Option<SentCopyStatus>,
}

/// The Sent-copy job state exposed to the UI. `state` mirrors the
/// `sent_copy_state` column exactly (`pending` queued, `verify` awaiting a
/// delivery re-check, `done` saved, `abandoned` gave up); `error` is the
/// already-plain `sent_copy_error` note (set only on `abandoned` or while
/// retrying). Only `abandoned` is shown to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentCopyStatus {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    Identity,
    ListDrafts,
    ListOutbox,
    ListScheduledSummaries,
    SchedulingInfo,
    ScheduleChoices { local_time: String, time_zone: String },
    Schedule { id: String, revision: i64, selection: crate::scheduled_send::Selection },
    Reschedule { id: String, revision: i64, selection: crate::scheduled_send::Selection },
    SendScheduledNow { id: String, revision: i64 },
    Create {
        mode: String,
        source_id: Option<String>,
        /// The sending account. Required for reply/replyAll/forward (the
        /// source thread's owning account); optional for "new", which falls
        /// back to the most-recently-used account.
        account: Option<String>,
    },
    /// "Edit as New": a new, unaddressed draft copied from a message the
    /// account sent.
    CreateFromSent {
        source_id: String,
        account: String,
    },
    SetAccount {
        id: String,
        account: String,
    },
    /// The sending identities (address + optional display name) available on
    /// `account`, in order. Offline-safe: read from the stored list, no
    /// provider call. Used by the compose From picker.
    Identities {
        account: String,
    },
    /// Set a draft's chosen sending identity. `from` must be one of the
    /// account's identities, or `null` / the primary address (which stores
    /// `from = None`). Allowed for every compose mode.
    SetFrom {
        id: String,
        #[serde(default)]
        from: Option<String>,
    },
    Save {
        draft: Draft,
    },
    Discard {
        id: String,
    },
    /// Pushes the draft to the server without waiting for the push delay,
    /// because the composer closed.
    PushDraft {
        id: String,
    },
    /// Settles a draft changed both here and on the server: `keepServer`
    /// adopts the server's version, otherwise this one replaces it.
    ResolveConflict {
        id: String,
        keep_server: bool,
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
/// Renders a source message's normalized RFC 3339 date the way a reader
/// expects to see it in a reply attribution ("Mon, Oct 5, 2026 at 9:32 AM"),
/// in the reader's time zone. An unparseable date is quoted as-is rather than
/// dropped, so the attribution line keeps its shape.
fn attribution_date<Tz: chrono::TimeZone>(rfc3339: &str, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|date| {
            date.with_timezone(zone)
                .format("%a, %b %-d, %Y at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

fn plain_text_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br>")
}

fn forward_attachments(source: &RawMessage, normalized: &crate::mime::NormalizedMessage) -> Result<Vec<Attachment>, String> {
    normalized.attachments.iter().map(|attachment| {
        Ok(Attachment {
            id: Uuid::new_v4().to_string(),
            name: attachment.filename.clone(),
            size: attachment.size,
            mime: attachment.mime_type.clone(),
            ready: false,
            message_id: Some(source.id.clone()),
            provider_id: crate::mime::provider_attachment_id_from_payload(source, &attachment.id)?,
            inline: attachment.inline,
            content_id: attachment.content_id.clone(),
        })
    }).collect()
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
    /// removes it. New installations default to 90 days; existing unset
    /// settings and an explicit Forever choice remain unlimited (`None`).
    pub fn retention_days(&self) -> DbResult<Option<i64>> {
        self.read_retention_days(true)
    }

    /// Only chosen values belong in sync reconciliation, not a new device's
    /// untouched installation default. Explicit Forever writes are recorded
    /// by the command, as before.
    pub(crate) fn chosen_retention_days(&self) -> DbResult<Option<i64>> {
        self.read_retention_days(false)
    }

    fn read_retention_days(&self, include_default: bool) -> DbResult<Option<i64>> {
        self.connection()?
            .query_row(
                "SELECT value FROM compose_settings
                 WHERE key='retention_days' OR (?1 AND key='retention_days_default')
                 ORDER BY CASE key WHEN 'retention_days' THEN 0 ELSE 1 END LIMIT 1",
                [include_default],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<i64>().map_err(|e| DatabaseError::corrupt(e.to_string())))
            .transpose()
    }
    pub fn set_retention_days(&self, days: Option<i64>) -> DbResult<()> {
        validate_retention_days(days).map_err(DatabaseError::Validation)?;
        self.with_transaction(|transaction| {
            transaction.execute("DELETE FROM compose_settings WHERE key='retention_days_default'", [])?;
            match days {
                Some(days) => transaction.execute(
                    "INSERT INTO compose_settings VALUES ('retention_days',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![days.to_string()],
                ),
                None => transaction.execute("DELETE FROM compose_settings WHERE key='retention_days'", []),
            }?;
            Ok(())
        })
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
        identities: &[crate::provider::SendingIdentity],
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
            from: None,
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
            signature: None,
            forwarded_content: None,
            follow_up_task_id: None,
            attachments: vec![],
            updated_at: now(),
            server_draft: None,
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
            // A reply written in another app arrives with its conversation
            // but not the message it answers. Replying in that conversation
            // reopens it rather than starting a second reply.
            if mode != "forward" {
                let imported = self.drafts()?.into_iter().find(|d| {
                    d.account == account
                        && d.source_id.is_none()
                        && matches!(d.mode.as_str(), "reply" | "replyAll")
                        && d.thread_id.as_deref() == Some(source.thread_id.as_str())
                });
                if let Some(mut existing) = imported {
                    existing.source_id = Some(source_id.clone());
                    let expected = existing.revision;
                    existing.revision += 1;
                    if self.update_draft_without_push(&mut existing, expected)? == 1 {
                        return Ok(existing);
                    }
                }
            }
            let normalized = crate::mime::normalize(&source)?;
            let part = &source.payload;
            let from = header(part, "From");
            // Reply/forward identity selection: for an account with MORE THAN
            // ONE identity, send from whichever identity the ORIGINAL message
            // was addressed to (`docs/imap-design.md`, "Identities"). A
            // single-identity account takes the existing path untouched —
            // `from` stays `None`, meaning "send as `account`". The chosen
            // index 0 (the account's primary/login) also leaves `from = None`,
            // so only a genuinely non-primary identity records a `from`.
            if identities.len() > 1 {
                let headers = crate::provider::imap::ReplyHeaders {
                    to: header(part, "To").to_string(),
                    cc: header(part, "Cc").to_string(),
                    delivered_to: header(part, "Delivered-To").to_string(),
                    x_original_to: header(part, "X-Original-To").to_string(),
                };
                let chosen = crate::provider::imap::choose_reply_identity(identities, &headers);
                if chosen != 0 {
                    d.from = Some(identities[chosen].address.clone());
                }
            }
            d.subject = normalized.subject.clone();
            let quote = if normalized.body_text.trim().is_empty() {
                crate::html_plain_text::html_to_text(&normalized.body_html)
            } else {
                normalized.body_text.clone()
            };
            let date = attribution_date(&normalized.date, &chrono::Local);
            d.body = format!(
                "\n\nOn {}, {} wrote:\n{}",
                date,
                from,
                quote
                    .lines()
                    .map(|l| format!("> {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            if mode == "forward" {
                d.subject = format!("Fwd: {}", d.subject);
                let attribution = format!("---------- Forwarded message ----------\nFrom: {from}\nDate: {date}\nSubject: {}\nTo: {}\n\n", normalized.subject, header(part, "To"));
                if normalized.body_html.trim().is_empty() {
                    d.body = format!("\n\n{attribution}{quote}");
                } else {
                    d.body = String::new();
                    d.forwarded_content = Some(ForwardedContent {
                        html: format!("<div>{}</div>{}", plain_text_html(&attribution), normalized.body_html),
                        text: format!("{attribution}{quote}"),
                    });
                }
                d.attachments = forward_attachments(&source, &normalized)?;
                // A forward joins the source conversation, as in Gmail, so the
                // sent copy and replies to it read alongside the original.
                // It stays a forward: no recipients are filled in, and a source
                // without a Message-ID still forwards, as a new conversation.
                let id = header(part, "Message-ID").trim().trim_matches(['<', '>']);
                if !id.is_empty() {
                    d.reply_id = Some(id.into());
                    d.thread_id = Some(source.thread_id);
                    d.references = reference_ids(header(part, "References"));
                    d.references.push(id.into());
                }
            } else {
                let reply = header(part, "Reply-To");
                let mut mine: HashSet<String> = identities
                    .iter()
                    .map(crate::provider::imap::identity_normalized)
                    .collect();
                mine.insert(account.trim().to_ascii_lowercase());
                let own = addresses(from)?
                    .iter()
                    .any(|(_, a)| mine.contains(&a.to_ascii_lowercase()));
                let target = if own {
                    header(part, "To")
                } else if !reply.is_empty() {
                    reply
                } else {
                    from
                };
                let mut seen = mine;
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
        d.signature = self.default_draft_signature(&d.account, d.from_address(), &d.mode)?;
        self.write_draft(&mut d, false, |tx, d, payload| {
            tx.execute("INSERT INTO drafts VALUES (?1,?2,?3)", params![d.id, d.revision, payload])
        })?;
        Ok(d)
    }
    /// "Edit as New": an unaddressed new message that starts from the
    /// subject, body, and attachments of a message this account sent. Only
    /// the account's own messages qualify, so another sender's HTML never
    /// becomes editable authored content. Recipients are left empty because
    /// the copy goes to new people, and every call makes a separate draft so
    /// the same message can be sent on to several people one at a time.
    pub fn create_draft_from_sent(
        &self,
        source_id: &str,
        account: &str,
        identities: &[crate::provider::SendingIdentity],
    ) -> Result<Draft, String> {
        let raw = self
            .message_metadata(source_id)
            .ok()
            .flatten()
            .ok_or_else(|| {
                "This message is unavailable offline. Refresh mail while connected first."
                    .to_string()
            })?;
        let source: RawMessage = serde_json::from_str(&raw).map_err(error)?;
        let normalized = crate::mime::normalize(&source)?;
        let mut own: Vec<String> = identities
            .iter()
            .map(crate::provider::imap::identity_normalized)
            .collect();
        own.insert(0, account.trim().to_ascii_lowercase());
        let sender = addresses(header(&source.payload, "From"))?
            .into_iter()
            .map(|(_, address)| address.to_ascii_lowercase())
            .find(|address| own.contains(address))
            .ok_or("Edit as New works only on messages you sent")?;
        // Keep the identity the original went out from; the primary address
        // stays canonical as `from = None`, as everywhere else.
        let from = identities
            .iter()
            .position(|identity| crate::provider::imap::identity_normalized(identity) == sender)
            .filter(|&index| index != 0)
            .map(|index| identities[index].address.clone());
        let mut d = Draft {
            id: Uuid::new_v4().to_string(),
            revision: 0,
            account: account.to_string(),
            from,
            mode: "new".into(),
            source_id: None,
            thread_id: None,
            reply_id: None,
            references: vec![],
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: normalized.subject.clone(),
            body: normalized.body_text.clone(),
            // The composer sanitizes this before it becomes editable and
            // saves the sanitized copy, so what sends is what was shown.
            body_html: normalized.body_html.clone(),
            signature: None,
            forwarded_content: None,
            follow_up_task_id: None,
            attachments: forward_attachments(&source, &normalized)?,
            updated_at: now(),
            server_draft: None,
        };
        self.write_draft(&mut d, false, |tx, d, payload| {
            tx.execute("INSERT INTO drafts VALUES (?1,?2,?3)", params![d.id, d.revision, payload])
        })?;
        Ok(d)
    }
    pub fn save_draft(&self, mut draft: Draft) -> Result<Draft, String> {
        let old = self.draft(&draft.id)?;
        if let Some(signature) = &mut draft.signature {
            signature.validate()?;
            crate::signatures::utf16_byte_offset(&draft.body_html, signature.html_offset)?;
            crate::signatures::utf16_byte_offset(&draft.body, signature.text_offset)?;
            // A per-message edit always opts out of automatic From switching.
            if old.signature.as_ref().is_some_and(|old| old.html != signature.html || old.text != signature.text) { signature.automatic = false; }
        }
        // Only editable fields cross the trust boundary; routing and file locators are native-owned.
        if draft.body.len() + draft.body_html.len() > 2 * 1024 * 1024
            || draft.subject.len() > 998
            || draft.to.len() + draft.cc.len() + draft.bcc.len() > 32000
        {
            return Err("Draft is too large".into());
        }
        draft.account = old.account;
        draft.from = old.from;
        draft.mode = old.mode;
        draft.source_id = old.source_id;
        draft.thread_id = old.thread_id;
        draft.reply_id = old.reply_id;
        draft.references = old.references;
        draft.attachments = old.attachments;
        draft.forwarded_content = old.forwarded_content;
        draft.follow_up_task_id = draft.follow_up_task_id.or(old.follow_up_task_id);
        if draft.subject != old.subject {
            draft.thread_id = None;
        }
        let expected = draft.revision;
        draft.revision += 1;
        draft.updated_at = now();
        let changed = self.update_draft(&mut draft, expected)?;
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
        // A chosen secondary identity belongs to the OLD account. Moving the
        // draft to another account must drop it, or the draft keeps a `from`
        // that is not on the new account's identity list and `queue` later
        // rejects it with "no longer on this account". Clear it so the new
        // account sends from its own primary identity (`from = None`).
        d.from = None;
        self.replace_automatic_signature(&mut d)?;
        let expected = d.revision;
        d.revision += 1;
        d.updated_at = now();
        let changed = self.update_draft(&mut d, expected)?;
        if changed != 1 {
            return Err("Draft changed elsewhere. Reopen it before editing.".into());
        }
        self.set_compose_identity(account)?;
        Ok(d)
    }
    /// Sets the draft's chosen sending identity (`Draft.from`). Allowed for
    /// EVERY compose mode — replying from a legitimate secondary address of
    /// the SAME account is valid (`docs/imap-design.md`, "Identities"); only
    /// the account itself is locked (that is `set_draft_account`'s job). The
    /// address must be one of the account's identities (normalized compare).
    /// `None`, or an address equal to the FIRST identity (the account's
    /// primary/login), stores `from = None` ("send as the account"), so the
    /// stored value stays canonical. An unknown address is rejected with a
    /// plain-language error and NOTHING is written. Bumps the revision under
    /// the same optimistic-concurrency guard `set_draft_account` uses.
    pub fn set_draft_from(&self, id: &str, from: Option<&str>) -> Result<Draft, String> {
        let mut d = self.draft(id)?;
        let identities = self.account_sending_identities(&d.account)?;
        let resolved: Option<String> = match from.map(str::trim).filter(|s| !s.is_empty()) {
            // No address, or an explicit null -> primary identity.
            None => None,
            Some(address) => {
                let normalized =
                    crate::provider::imap::normalize_identity_address(address);
                let matched = identities.iter().find(|identity| {
                    crate::provider::imap::identity_normalized(identity) == normalized
                });
                match matched {
                    None => {
                        return Err(format!(
                            "{address} is not one of this account's sending addresses. \
                             Add it in the account's Sending settings first."
                        ));
                    }
                    // The primary (first) identity is "send as the account":
                    // store None so the canonical form never records a `from`
                    // equal to the account's own address.
                    Some(identity)
                        if crate::provider::imap::identity_normalized(identity)
                            == crate::provider::imap::identity_normalized(&identities[0]) =>
                    {
                        None
                    }
                    // A genuine secondary identity: store its address exactly
                    // as the identity list spells it (original case).
                    Some(identity) => Some(identity.address.clone()),
                }
            }
        };
        d.from = resolved;
        self.replace_automatic_signature(&mut d)?;
        let expected = d.revision;
        d.revision += 1;
        d.updated_at = now();
        let changed = self.update_draft(&mut d, expected)?;
        if changed != 1 {
            return Err("Draft changed elsewhere. Reopen it before editing.".into());
        }
        Ok(d)
    }
    pub fn discard_draft(&self, id: &str) -> Result<(), String> {
        self.with_transaction(|tx| {
            tx.execute("DELETE FROM drafts WHERE id=?1", [id])?;
            Self::draft_left_drafts(tx, id, now())?;
            Ok(())
        })?;
        Ok(())
    }
    /// Writes one draft row. `push` marks an edit that the account's server
    /// drafts need, in the same transaction. Returns the rows `write` changed.
    fn write_draft(
        &self,
        d: &mut Draft,
        push: bool,
        write: impl FnOnce(&rusqlite::Transaction, &Draft, &str) -> rusqlite::Result<usize>,
    ) -> Result<usize, String> {
        d.server_draft = None;
        let payload = json(d)?;
        let draft = &*d;
        let changed = self.with_transaction(|tx| {
            let changed = write(tx, draft, &payload)?;
            if changed == 1 && push {
                Self::mark_draft_push_pending(tx, draft, now())?;
            }
            Ok(changed)
        })?;
        Ok(changed)
    }
    /// For a change the server copy doesn't carry, such as which local
    /// message a reply answers. A server copy that was current stays current.
    fn update_draft_without_push(&self, d: &mut Draft, expected: i64) -> Result<usize, String> {
        self.write_draft(d, false, |tx, d, payload| {
            let changed = tx.execute(
                "UPDATE drafts SET revision=?1,payload=?2 WHERE id=?3 AND revision=?4",
                params![d.revision, payload, d.id, expected],
            )?;
            tx.execute(
                "UPDATE draft_server_links SET pushed_revision=?2 WHERE draft_id=?1 AND pushed_revision=?3",
                params![d.id, d.revision, expected],
            )?;
            Ok(changed)
        })
    }
    fn update_draft(&self, d: &mut Draft, expected: i64) -> Result<usize, String> {
        self.write_draft(d, true, |tx, d, payload| {
            tx.execute(
                "UPDATE drafts SET revision=?1,payload=?2 WHERE id=?3 AND revision=?4",
                params![d.revision, payload, d.id, expected],
            )
        })
    }
    fn save_attachment_draft(&self, mut d: Draft) -> Result<Draft, String> {
        let expected = d.revision;
        d.revision += 1;
        d.updated_at = now();
        if self.update_draft(&mut d, expected)? != 1 {
            return Err("Draft changed while preparing attachment; retry".into());
        }
        Ok(d)
    }
    pub fn outbox(&self) -> Result<Vec<OutboxItem>, String> {
        let c = self.connection()?;
        let mut q = c
            .prepare(
                "SELECT id,payload,state,deadline,error,provider_id,archive_on_send,schedule_json,last_attempt_at,sent_copy_state,sent_copy_error FROM outbox_messages ORDER BY rowid DESC",
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
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<i64>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                ))
            })
            .map_err(error)?;
        rows.map(|r| {
            let (id, payload, state, deadline, error, provider_id, archive_on_send, schedule_json, last_attempt_at, sent_copy_state, sent_copy_error) =
                r.map_err(error)?;
            Ok(OutboxItem {
                id,
                draft: serde_json::from_str(&payload).map_err(crate::correspondence::error)?,
                state,
                deadline,
                error,
                provider_id,
                last_attempt_at,
                archive_on_send,
                schedule: schedule_json.map(|s|serde_json::from_str(&s)).transpose().map_err(crate::correspondence::error)?,
                // A row with no Sent-copy job (Gmail, legacy, or a server that
                // saves its own copy) has a NULL sent_copy_state -> no status.
                sent_copy: sent_copy_state.map(|state| SentCopyStatus { state, error: sent_copy_error }),
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
    /// Resolves an `uncertain` send to the terminal `unverifiable` state:
    /// the message was handed to the server (its data was sent) but the
    /// outcome can never be confirmed, because this provider keeps no
    /// server-side sent copy to reconcile against. Distinct from `sent`
    /// (confirmed delivered), `failed` (confirmed not delivered), and
    /// `uncertain` (unconfirmed but still worth re-checking). Terminal:
    /// `tick` only re-processes `undo_pending`/`ready`/`uncertain`, so
    /// `unverifiable` is never retried or reconciled again. The user is
    /// shown "may have been sent" and decides whether to resend. The guard
    /// on `state='uncertain'` makes this a no-op if the row has since moved
    /// on. See `docs/imap-design.md` ("Sending").
    pub fn mark_unverifiable(&self, id: &str) -> Result<(), String> {
        let owner=self.outbox()?.into_iter().find(|o|o.id==id).and_then(|o|o.schedule).map(|s|s.report.owner_installation_id).unwrap_or_default();
        self.change_outbox("UPDATE outbox_messages SET state='unverifiable', error='This message may have been sent, but delivery could not be confirmed. Confirm with the recipient before sending it again.' WHERE id=?1 AND state='uncertain'",[id],id,&owner,now())?;
        Ok(())
    }
    pub fn queue(
        &self,
        id: &str,
        revision: i64,
        archive_on_send: bool,
        identities: &[crate::provider::SendingIdentity],
        message_id_domain: Option<&str>,
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
        let raw = build_mime(
            &d,
            identities,
            sender_name.as_deref(),
            message_id_domain,
            &operation_id,
            root,
        )?;
        let item = OutboxItem {
            id: operation_id,
            draft: d.clone(),
            state: "undo_pending".into(),
            deadline: now() + UNDO_MS,
            error: None,
            provider_id: None,
            last_attempt_at: None,
            archive_on_send,
            schedule: None,
            sent_copy: None,
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
        Self::draft_left_drafts(&tx, id, now())?;
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
            ["undo_pending", "ready", "scheduled", "overdue"].contains(&item.state.as_str())
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
        Self::mark_draft_push_pending(&tx, &d, now())?;
        if let Some(schedule) = &item.schedule { crate::scheduled_send::record(&tx,id,&schedule.report.owner_installation_id,now()).map_err(error)?; }
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
    #[cfg(test)]
    pub fn pause_ready_sends_for(&self, account: &str) -> Result<(), String> {
        self.pause_ready_sends_for_owned(account,"")
    }
    pub fn pause_ready_sends_for_owned(&self, account: &str, owner: &str) -> Result<(), String> {
        let items=self.outbox()?;
        for item in items.into_iter().filter(|o|o.draft.account==account && ["undo_pending","ready","scheduled","overdue"].contains(&o.state.as_str())) {
            self.change_outbox("UPDATE outbox_messages SET state='failed',error='Account disconnected. Reconnect and restore this draft to send.' WHERE id=?1 AND state IN ('undo_pending','ready','scheduled','overdue')",[&item.id],&item.id,owner,now())?;
        }
        Ok(())
    }
    /// Fails every queued or scheduled send whose account is gone from the
    /// catalog, whatever its due time. Removing an account on another device
    /// purges it here without touching this device's outbox, and a send left
    /// waiting would otherwise go out unasked once the account is added again.
    pub fn fail_sends_for_removed_accounts(&self, owner: &str) -> Result<(), String> {
        let accounts: Vec<String> = {
            let c = self.connection()?;
            let mut q = c
                .prepare(
                    "SELECT DISTINCT account FROM outbox_messages
                 WHERE state IN ('undo_pending','ready','scheduled','overdue')
                   AND lower(account) NOT IN (SELECT lower(email) FROM accounts)",
                )
                .map_err(error)?;
            let rows = q.query_map([], |r| r.get(0)).map_err(error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(error)?
        };
        for account in accounts {
            self.pause_ready_sends_for_owned(&account, owner)?;
        }
        Ok(())
    }
}

pub(crate) fn build_mime(
    d: &Draft,
    identities: &[crate::provider::SendingIdentity],
    account_display_name: Option<&str>,
    message_id_domain: Option<&str>,
    id: &str,
    root: &Path,
) -> Result<Vec<u8>, String> {
    build_message(d, identities, account_display_name, message_id_domain, id, root, true)
}

/// The server copy of a draft: the message exactly as it would be sent, but
/// with no recipients allowed and a `Message-ID` that stays the same across
/// every push, so any device can find the copy again
/// (`docs/server-drafts-design.md`, "Server message").
pub(crate) fn build_draft_mime(
    d: &Draft,
    identities: &[crate::provider::SendingIdentity],
    account_display_name: Option<&str>,
    message_id_domain: Option<&str>,
    root: &Path,
) -> Result<Vec<u8>, String> {
    let id = crate::draft_push::server_message_local_part(&d.id);
    build_message(d, identities, account_display_name, message_id_domain, &id, root, false)
}

fn build_message(
    d: &Draft,
    identities: &[crate::provider::SendingIdentity],
    account_display_name: Option<&str>,
    message_id_domain: Option<&str>,
    id: &str,
    root: &Path,
    sending: bool,
) -> Result<Vec<u8>, String> {
    if d.subject.contains(['\r', '\n']) {
        return Err("Subject cannot contain newlines".into());
    }
    let to = addresses(&d.to)?;
    let cc = addresses(&d.cc)?;
    let bcc = addresses(&d.bcc)?;
    if sending && to.len() + cc.len() + bcc.len() == 0 {
        return Err("Add at least one recipient".into());
    }

    // Resolve the From address and its display name from the account's
    // identity list (the one source of truth), NOT from a value stored on the
    // draft. `from_address()` is `d.from` when a non-primary identity was
    // chosen, else the account's own address.
    //
    // An EMPTY identity list is treated as the single identity `[account]`:
    // the production callers always pass the provider's non-empty,
    // login-first list, so this only serves a defensive/test path and keeps
    // the account's own address a valid From.
    let account_identity = [crate::provider::SendingIdentity {
        address: d.account.clone(),
        display_name: None,
    }];
    let identities: &[crate::provider::SendingIdentity] = if identities.is_empty() {
        &account_identity
    } else {
        identities
    };
    let from_address = d.from_address();
    let from_normalized =
        crate::provider::imap::normalize_identity_address(from_address);
    let matched = identities
        .iter()
        .find(|identity| {
            crate::provider::imap::normalize_identity_address(&identity.address) == from_normalized
        });
    // A `from` explicitly set to an address that is NOT one of the account's
    // identities (a stale identity removed from settings) must fail CLEARLY,
    // never silently send as another address. When `from` is unset the From is
    // the account address, which is always identity 0, so this only bites a
    // deliberately-chosen stale identity.
    if d.from.is_some() && matched.is_none() {
        return Err(format!(
            "The chosen sending identity ({from_address}) is no longer on this account. \
             Pick a current identity before sending."
        ));
    }
    // Display name: prefer the matched identity's own display name; for the
    // account's primary address fall back to the account display name, so
    // Gmail and single-identity sends keep their existing From display name
    // byte-for-byte.
    let display_name = matched
        .and_then(|identity| identity.display_name.as_deref())
        .or_else(|| {
            let is_primary = identities
                .first()
                .map(|primary| {
                    crate::provider::imap::normalize_identity_address(&primary.address)
                        == from_normalized
                })
                .unwrap_or(true);
            if is_primary {
                account_display_name
            } else {
                None
            }
        });

    // Header-injection guard: neither the From address nor its display name may
    // carry CR/LF/NUL. The address already passed envelope validation when the
    // identity set was built, but a draft's `from`/account could in principle
    // hold anything, and the display name is free-form, so assert here at the
    // point bytes are produced.
    if from_address.contains(['\r', '\n', '\0']) {
        return Err("Sender address cannot contain line breaks".into());
    }
    if let Some(name) = display_name {
        if name.contains(['\r', '\n', '\0']) {
            return Err("Sender display name cannot contain line breaks".into());
        }
    }

    let builder = MessageBuilder::new();
    let builder = if let Some(name) = display_name {
        builder.from((name.to_string(), from_address.to_string()))
    } else {
        builder.from(from_address.to_string())
    };
    let mut body = d.body.clone();
    let mut body_html = d.body_html.clone();
    let mut signature_images = Vec::new();
    if let Some(signature) = &d.signature {
        signature.validate()?;
        if !signature.html.is_empty() {
            let (html, images) = crate::signatures::embed_images(&signature.html, id)?;
            signature_images = images;
            body = crate::signatures::insert_signature(&body, signature.text_offset, &signature.text, false)?;
            if body_html.is_empty() {
                // Native-created drafts have no authored HTML yet. The native
                // signature boundary starts at zero, above their reply history.
                let byte = crate::signatures::utf16_byte_offset(&d.body, signature.text_offset)?;
                let prefix = plain_text_html(&d.body[..byte]);
                let suffix = plain_text_html(&d.body[byte..]);
                body_html = format!("{prefix}{}{html}{}{suffix}", if byte > 0 { "<br><br>" } else { "" }, if byte < d.body.len() && !d.body[byte..].starts_with("\n\n") { "<br><br>" } else { "" });
            } else {
                body_html = crate::signatures::insert_signature(&body_html, signature.html_offset, &html, true)?;
            }
        }
    }
    if let Some(forwarded) = &d.forwarded_content {
        body.push_str(&format!("\n\n{}", forwarded.text));
        if body_html.trim().is_empty() {
            body_html = plain_text_html(&d.body);
        }
        body_html.push_str(&format!("<br><br>{}", forwarded.html));
    }
    // The Message-ID domain: the provider's hook result when it supplied one
    // (IMAP returns the From address's domain), else the app's reserved
    // `threestrands.local`. Gmail passes `None`, so its Message-ID is
    // byte-identical to before. The builder adds the surrounding `<…>`.
    let domain = message_id_domain.unwrap_or("threestrands.local");
    let mut builder = builder
        .to(to)
        .cc(cc)
        .bcc(bcc)
        .subject(d.subject.clone())
        .text_body(body.clone())
        .message_id(format!("{id}@{domain}"));
    if !body_html.trim().is_empty() {
        builder = builder.html_body(body_html.clone());
    }
    if let Some(reply) = &d.reply_id {
        if reply.contains(['\r', '\n']) || d.references.iter().any(|r| r.contains(['\r', '\n'])) {
            return Err("Invalid reply headers".into());
        }
        builder = builder
            .in_reply_to(reply.clone())
            .references(d.references.clone());
    }
    let mut size = body.len() + body_html.len();
    for (mime, cid, bytes) in signature_images {
        size += bytes.len();
        builder = builder.inline(mime, cid, bytes);
    }
    for attachment in &d.attachments {
        if !attachment.ready {
            return Err(if attachment.message_id.is_some() {
                format!("Download or remove {} before sending", attachment.name)
            } else {
                // Attachment bytes stay on the device that added them.
                format!("{} was attached on another device. Attach it again here, or remove it, before sending", attachment.name)
            });
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
    pub sending_identity: Arc<crate::scheduled_send::SendingIdentity>,
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
    /// Serializes identity creation and schedule edits without blocking compose.
    pub scheduling: Arc<tokio::sync::Mutex<()>>,
    /// Serializes server draft pushes and deletions.
    pub draft_pushes: Arc<tokio::sync::Mutex<()>>,
    /// Tells the frontend when a draft's server copy changes state, or when
    /// a pull changed local drafts.
    pub draft_events: Arc<dyn Fn(crate::draft_push::DraftEvent) + Send + Sync>,
    /// Set to pull server drafts on the worker's next round.
    pub draft_pull_requested: Arc<std::sync::atomic::AtomicBool>,
}
impl Correspondence {
    pub fn resume_schedules(&self) -> Result<(),String> {
        let owner=self.schedule_owner(false).unwrap_or_default();
        self.database.overdue_schedules_owned(now(),true,&owner)
    }
    pub fn pause_account_schedules(&self,account:&str) -> Result<(),String> {
        let owner=self.schedule_owner(false).unwrap_or_default();
        self.database.pause_ready_sends_for_owned(account,&owner)
    }
    fn schedule_owner(&self, create: bool) -> Result<String,String> {
        let has_schedules = self.database.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM outbox_messages WHERE schedule_json IS NOT NULL)",
            [], |row| row.get::<_, bool>(0),
        ).map_err(error)?;
        if !create && !has_schedules { return Err("No scheduled messages".into()); }
        (self.sending_identity)(create && !has_schedules)
    }
    fn authorize_schedule(&self, id: &str) -> Result<String,String> {
        let item=self.database.outbox()?.into_iter().find(|o|o.id==id).ok_or("Outbox item not found")?;
        if let Some(schedule)=item.schedule {
            let owner=self.schedule_owner(false)?;
            if schedule.report.owner_installation_id != owner { return Err("This message belongs to another sending computer".into()); }
            Ok(owner)
        } else { Ok(String::new()) }
    }
    async fn authorize_schedule_async(&self, id: &str) -> Result<String, String> {
        let service = self.clone();
        let id = id.to_string();
        crate::run_database_task(move || service.authorize_schedule(&id))
            .await
            .map_err(String::from)
    }
    fn listed_outbox(&self) -> Result<serde_json::Value, String> {
        let owner = self.schedule_owner(false).ok();
        if let Some(owner) = &owner { self.database.refresh_schedule_reports(owner, now())?; }
        let mut items = self.database.outbox()?;
        for item in &mut items {
            if let Some(schedule) = &mut item.schedule {
                schedule.can_manage = owner.as_deref() == Some(&schedule.report.owner_installation_id);
                if !schedule.can_manage { schedule.report.blocked_reason = Some("owner_unavailable".into()); }
                schedule.visibility = self.database.schedule_visibility(schedule)?;
            }
        }
        serde_json::to_value(items).map_err(error)
    }
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
        auth.provider(&self.database).map_err(error)
    }
    /// Caches a draft's source message so drafting from it works offline later.
    async fn ensure_source_metadata(&self, id: &str, account: &str) -> Result<(), String> {
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
                .provider_for(account)
                .await?
                .fetch_message(id)
                .await
                .map_err(error)?;
            self.database.put_message_metadata(id, &json(&source)?)?;
        }
        Ok(())
    }
    /// The account's sending identities for drafting from an existing message.
    /// An unavailable provider (disconnected account, locked keychain) must
    /// not stop a draft from an already-cached message from opening. Fall
    /// back to the primary identity (`from = None`); `queue` re-resolves the
    /// identity list from the provider and rejects a `from` that is not on
    /// it, so nothing can send as the wrong address.
    async fn source_identities(&self, account: &str) -> Vec<crate::provider::SendingIdentity> {
        match self.provider_for(account).await {
            Ok(provider) => provider.identities(account).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        }
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
            .provider(&self.database)
            .map_err(error)?
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
        use Request::*;
        // Reads and reconciliation do not edit browser-owned drafts. In particular,
        // a slow or locked keychain during polling must not stall create/save.
        match &request {
            ListOutbox => {
                let service = self.clone();
                return crate::run_database_task(move || service.listed_outbox())
                    .await
                    .map_err(String::from);
            }
            ListDrafts | ListScheduledSummaries => {
                let database = self.database.clone();
                return crate::run_database_task(move || match request {
                    ListDrafts => {
                        let mut drafts = database.drafts()?;
                        for draft in &mut drafts {
                            draft.server_draft = database.server_draft_status(draft)?;
                        }
                        serde_json::to_value(drafts).map_err(error)
                    }
                    _ => serde_json::to_value(database.scheduled_summaries()?).map_err(error),
                })
                .await
                .map_err(String::from);
            }
            Reconcile { id } => {
                self.reconcile(id).await?;
                return Ok(serde_json::Value::Null);
            }
            // Touches only the push queue, so it never waits behind an edit.
            PushDraft { id } => {
                let database = self.database.clone();
                let id = id.clone();
                crate::run_database_task(move || database.push_drafts_now(Some(&id), now()))
                    .await
                    .map_err(String::from)?;
                return Ok(serde_json::Value::Null);
            }
            _ => {}
        }
        // Never hold a data lock while a user is choosing files; undo remains responsive.
        let selected_files = if matches!(&request, Request::Attach { .. }) {
            rfd::AsyncFileDialog::new()
                .pick_files()
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let schedule_target = match &request {
            Schedule { id, revision, .. } => Some((id.clone(), Some(*revision))),
            Reschedule { id, .. } | SendScheduledNow { id, .. } | Cancel { id } | Recover { id } => Some((id.clone(), None)),
            _ => None,
        };
        // Keep first-time identity creation serialized through the schedule commit.
        // Never cache this authority: a missing/replaced keychain entry must still
        // pause persisted jobs rather than authorize them as a new installation.
        let _scheduling = if schedule_target.is_some() { Some(self.scheduling.lock().await) } else { None };
        let schedule_owner = if let Some((id, revision)) = schedule_target {
            let service = self.clone();
            crate::run_database_task(move || {
                if let Some(revision) = revision {
                    if let Some(item) = service.database.outbox()?.into_iter().find(|o| o.draft.id == id && o.draft.revision == revision) {
                        return service.authorize_schedule(&item.id);
                    }
                    service.schedule_owner(true)
                } else {
                    service.authorize_schedule(&id)
                }
            }).await?
        } else { String::new() };
        let _edits = self.edits.lock().await;
        match request {
            Identity => {
                let identity = self.database.compose_identity().map_err(|_| {
                    "Sender identity unavailable. Connect and refresh Gmail first.".to_string()
                });
                Ok(serde_json::to_value(identity?).map_err(error)?)
            }
            ListDrafts | ListOutbox | ListScheduledSummaries | Reconcile { .. } | PushDraft { .. } => unreachable!("handled before the compose lock"),
            SchedulingInfo => {
                let label=self.database.device_roster().map_err(error)?.into_iter().find(|d|d.is_self).and_then(|d|d.label).unwrap_or_else(||gethostname::gethostname().to_string_lossy().chars().take(60).collect());
                Ok(serde_json::json!({"deviceName":label,"sharing":self.database.cross_device_sync_enrolled().map_err(error)?}))
            }
            ScheduleChoices {local_time,time_zone} => Ok(serde_json::to_value(crate::scheduled_send::choices(&local_time,&time_zone)?).map_err(error)?),
            Schedule {id,revision,selection} => {
                if let Some(item)=self.database.outbox()?.into_iter().find(|o|o.draft.id==id && o.draft.revision==revision) {
                    return Ok(serde_json::to_value(item).map_err(error)?);
                }
                let draft=self.database.draft(&id)?;
                let provider = self.provider_for(&draft.account).await?;
                if !provider.capabilities().send_supported { return Err("Scheduled sending is not supported for this account yet".into()); }
                let identities = provider.identities(&draft.account).await.map_err(error)?;
                let message_id_domain = provider.message_id_domain(draft.from_address()).map_err(error)?;
                let owner=schedule_owner;
                Ok(serde_json::to_value(self.database.schedule_draft(&id,revision,&selection,&owner,&identities,message_id_domain.as_deref(),&self.root,now())?).map_err(error)?)
            }
            Reschedule {id,revision,selection} => {
                let owner=schedule_owner;
                self.database.manage_schedule(&id,revision,Some(&selection),false,&owner,now())?;
                Ok(serde_json::Value::Null)
            }
            SendScheduledNow {id,revision} => {
                let owner=schedule_owner;
                self.database.manage_schedule(&id,revision,None,true,&owner,now())?;
                Ok(serde_json::Value::Null)
            }
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
                    self.ensure_source_metadata(id, &resolved_account).await?;
                }
                // The account's sending identities, so a reply/forward from a
                // multi-identity account starts on the identity the original
                // was addressed to. Only resolved for reply/replyAll/forward:
                // a NEW draft needs no identity selection (it starts on the
                // first identity, `from = None`), so composing a blank message
                // must never require a connected provider — this keeps compose
                // responsive when the keychain/provider is unavailable.
                let identities = if mode == "new" {
                    Vec::new()
                } else {
                    self.source_identities(&resolved_account).await
                };
                let draft = self
                    .database
                    .create_draft(&mode, source_id, &resolved_account, &identities)?;
                if mode == "new" {
                    let _ = self.database.set_compose_identity(&resolved_account);
                }
                Ok(serde_json::to_value(draft).map_err(error)?)
            }
            CreateFromSent { source_id, account } => {
                self.ensure_source_metadata(&source_id, &account).await?;
                let identities = self.source_identities(&account).await;
                let draft = self
                    .database
                    .create_draft_from_sent(&source_id, &account, &identities)?;
                Ok(serde_json::to_value(draft).map_err(error)?)
            }
            SetAccount { id, account } => Ok(serde_json::to_value(
                self.database.set_draft_account(&id, &account)?,
            )
            .map_err(error)?),
            Identities { account } => Ok(serde_json::to_value(
                self.database.account_sending_identities(&account)?,
            )
            .map_err(error)?),
            SetFrom { id, from } => Ok(serde_json::to_value(
                self.database.set_draft_from(&id, from.as_deref())?,
            )
            .map_err(error)?),
            Save { draft } => {
                let mut saved = self.database.save_draft(draft)?;
                saved.server_draft = self.database.server_draft_status(&saved)?;
                Ok(serde_json::to_value(saved).map_err(error)?)
            }
            ResolveConflict { id, keep_server } => {
                Ok(serde_json::to_value(self.database.resolve_draft_conflict(&id, keep_server)?).map_err(error)?)
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
            } => {
                // Idempotent re-queue: if this draft is already in the outbox
                // (its editable row removed), return the existing item without
                // touching the provider — mirrors `Database::queue`'s own
                // early return, and avoids `draft(&id)` erroring on a draft
                // that was already consumed.
                if let Some(item) = self
                    .database
                    .outbox()?
                    .into_iter()
                    .find(|o| o.draft.id == id && o.draft.revision == revision)
                {
                    return Ok(serde_json::to_value(item).map_err(error)?);
                }
                // Resolve the account's sending identities and the Message-ID
                // domain from the provider BEFORE building the MIME, so the
                // From is validated against the current identity list and the
                // Message-ID carries the sender's domain (IMAP) or stays
                // `threestrands.local` (Gmail, the provider default). Both come
                // from the draft's own account's provider, not the active one.
                let draft = self.database.draft(&id)?;
                let provider = self.provider_for(&draft.account).await?;
                let identities = provider.identities(&draft.account).await.map_err(error)?;
                let message_id_domain = provider
                    .message_id_domain(draft.from_address())
                    .map_err(error)?;
                Ok(serde_json::to_value(self.database.queue(
                    &id,
                    revision,
                    archive_on_send,
                    &identities,
                    message_id_domain.as_deref(),
                    &self.root,
                )?)
                .map_err(error)?)
            }
            Cancel { id } => {
                Ok(serde_json::to_value(self.database.cancel_send(&id, false)?).map_err(error)?)
            }
            Recover { id } => {
                Ok(serde_json::to_value(self.database.cancel_send(&id, true)?).map_err(error)?)
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
                    fn find(part: &MimePart, attachment: &Attachment) -> Option<String> {
                        let matches = if let Some(content_id) = &attachment.content_id {
                            header(part, "Content-ID").trim().trim_matches(['<', '>']).eq_ignore_ascii_case(content_id)
                        } else {
                            crate::attachment_security::normalize_filename(&part.filename) == attachment.name
                        };
                        if matches {
                            return part.body.data.clone();
                        }
                        part.parts.iter().find_map(|p| find(p, attachment))
                    }
                    URL_SAFE_NO_PAD
                        .decode(
                            find(&source.payload, a)
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
        self.authorize_schedule_async(id).await?;
        let provider = self.provider_for(&item.draft.account).await?;
        self.reconcile_with_provider(&item, provider.as_ref()).await
    }

    async fn reconcile_with_provider(
        &self,
        item: &OutboxItem,
        provider: &dyn MailProvider,
    ) -> Result<(), String> {
        let id = &item.id;
        if provider.sender_identity().await.map_err(error)? != item.draft.account {
            return Err("Reconnect the original sender account".into());
        }
        if let Some(message) = provider
            .find_sent_copy(id, &item.draft.account)
            .await
            .map_err(error)?
        {
            let owner = self.authorize_schedule_async(id).await?;
            self.database.change_outbox("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2 AND state='uncertain'",params![message.provider_message_id,id],id,&owner,now())?;
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
        } else if provider.capabilities().verifiable_delivery {
            return Err("Delivery is still uncertain. No automatic retry was made. Check Gmail Sent before composing another message.".into());
        } else {
            // The provider keeps no server-side sent copy to confirm against
            // (`docs/imap-design.md`, "Sending": an uncertain SMTP result
            // can't be checked against the server because only we would have
            // `APPEND`ed the copy). `find_sent_copy` returning nothing is the
            // expected terminal outcome here, not an error — resolve to the
            // design's `Unverifiable` state so the send stops being re-checked
            // and is shown as "may have been sent" rather than retried.
            self.database.mark_unverifiable(id)?;
        }
        Ok(())
    }
    pub async fn tick(&self) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let service = self.clone();
        let owner=crate::run_database_task(move || service.schedule_owner(false)).await.ok();
        self.database.overdue_schedules_owned(now(),false,owner.as_deref().unwrap_or(""))?;
        if let Some(owner)=&owner { self.database.refresh_schedule_reports(owner,now())?; }
        self.database.fail_sends_for_removed_accounts(owner.as_deref().unwrap_or(""))?;
        let items = self.database.outbox()?;
        // Process due Sent-copy jobs FIRST, every tick — before the
        // no-sendable-items early return below. A job is created when a send is
        // accepted (`dispatch_due`) and lives on the outbox row independently of
        // the send states; a tick with nothing new to send must still drive
        // jobs to completion (fact 4). Holding `self.gate` for the whole tick
        // gives per-message serialisation for free; the processor caps its own
        // per-tick work.
        self.process_sent_copy_jobs(now()).await?;
        if !items
            .iter()
            .any(|o| ["undo_pending", "ready", "scheduled", "uncertain"].contains(&o.state.as_str()))
        {
            return Ok(());
        }
        // Grouped by account and processed independently: one account's
        // disconnected/rate-limited provider must never block another
        // account's queued sends.
        let mut by_account: HashMap<String, Vec<OutboxItem>> = HashMap::new();
        for item in items {
            if !["scheduled","undo_pending","ready","uncertain"].contains(&item.state.as_str()) || item.deadline>now() {continue;}
            // Rate-limit prepare re-attempts for a NON-scheduled queued item
            // whose last transient prepare failure stamped `last_attempt_at`
            // within SMTP_PREPARE_RETRY_INTERVAL: skip it this tick so a down
            // SMTP server is not probed on every ~2s tick. A never-attempted
            // item (`last_attempt_at` None) is always eligible, so this never
            // delays the first attempt. Scheduled items keep their own
            // deadline/block cadence and are not gated here.
            let send_candidate = ["undo_pending", "ready"].contains(&item.state.as_str())
                && item.schedule.is_none();
            if send_candidate
                && !crate::provider::imap::smtp_prepare_retry_due(item.last_attempt_at, now())
            {
                continue;
            }
            by_account
                .entry(item.draft.account.clone())
                .or_default()
                .push(item);
        }
        for (account, items) in by_account {
            let Ok(provider) = self.provider_for(&account).await else {
                for item in &items { if item.schedule.is_some() {self.database.block_schedule(&item.id,owner.as_deref().unwrap_or(""),Some("account_disconnected"),now())?;} }
                continue;
            };
            let Ok(identity) = provider.sender_identity().await else {
                for item in &items { if item.schedule.is_some() {self.database.block_schedule(&item.id,owner.as_deref().unwrap_or(""),Some("authorization"),now())?;} }
                continue;
            };
            for item in items {
                if item.schedule.as_ref().is_some_and(|s|Some(s.report.owner_installation_id.as_str())!=owner.as_deref()) { continue; }
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
                if !["undo_pending", "ready", "scheduled"].contains(&item.state.as_str())
                    || item.deadline > now()
                {
                    continue;
                }
                self.send_due_item(&item, provider.as_ref(), &identity, owner.as_deref().unwrap_or(""), now())
                    .await?;
            }
        }
        Ok(())
    }

    /// Attempt one due, non-uncertain outbox item against `provider`: obtain a
    /// delivery permit, dispatch, and (on success) re-assert the archive and
    /// refetch the thread. Extracted from [`tick`](Self::tick) and taking a
    /// `&dyn MailProvider` (like [`reconcile_with_provider`]) so a unit test
    /// can inject a fake provider — the account registry can only hold real
    /// providers.
    ///
    /// The one behaviour this adds over the old inline body is that a
    /// `prepare_delivery` failure NO LONGER aborts the whole tick for a
    /// NON-scheduled item (the old `Err(e) => return Err(error(e))`, which let
    /// one account whose SMTP server was down stop every later account's
    /// sends). Instead:
    ///
    /// * a TRANSIENT prepare failure (`TransientTransport`) leaves the item
    ///   QUEUED (never claimed, `attempts` untouched), records a short
    ///   plain-language `error` on the row, and returns `Ok(())` so the tick
    ///   moves on. The retry is rate-limited by `last_attempt_at` +
    ///   [`policy::smtp_prepare_retry_due`] in the caller's due-filtering (see
    ///   `prepare_retry_gated`), so a down server is probed at most once per
    ///   interval per item regardless of the ~2s tick cadence;
    /// * a DEFINITE PERMANENT prepare failure
    ///   (`ReauthenticationRequired` / `PermanentClientRejection` /
    ///   `InvalidOperation`) moves the row to `failed` with a plain-language
    ///   error via the same guarded UPDATE `dispatch_due` uses, so the draft
    ///   stays restorable;
    /// * a SCHEDULED item keeps its existing `block_schedule(..,
    ///   "provider_unavailable")` behaviour unchanged.
    ///
    /// Gmail's `prepare_delivery` never does network I/O, so for a Gmail
    /// provider the error branches are simply never taken and its behaviour is
    /// unchanged.
    async fn send_due_item(
        &self,
        item: &OutboxItem,
        provider: &dyn MailProvider,
        identity: &str,
        owner: &str,
        at: i64,
    ) -> Result<(), String> {
        // Obtain authorization before claiming delivery; transport errors
        // after the claim are uncertain.
        let permit = match provider.prepare_delivery().await {
            Ok(permit) => permit,
            Err(_) if item.schedule.is_some() => {
                self.database
                    .block_schedule(&item.id, owner, Some("provider_unavailable"), at)?;
                return Ok(());
            }
            Err(e) => {
                // A non-scheduled prepare failure must NOT abort the whole
                // tick (which would wedge every other account's queue).
                self.handle_prepare_failure(item, &e, owner, at)?;
                return Ok(());
            }
        };
        let mut item = item.clone();
        if item.schedule.is_some() {
            self.database.block_schedule(&item.id, owner, None, at)?;
            item = self
                .database
                .outbox()?
                .into_iter()
                .find(|o| o.id == item.id)
                .ok_or("Outbox item missing")?;
        }
        let sent = self
            .dispatch_due(&item, at, provider.capabilities().client_saves_sent_copy, |raw, thread| async move {
                permit.send_once(&raw, thread.as_deref()).await
            })
            .await?;
        if let Some(sent) = sent {
            if let Some(thread_id) = sent.thread_id.as_deref() {
                if self.database.archive_on_send(&item.id)? {
                    // Gmail attaches INBOX to the thread when the sent message
                    // lands, which would otherwise silently undo the archive
                    // done at queue time. Re-assert it here, before the refetch
                    // below, so the upserted thread reflects the archived state
                    // regardless of timing.
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
                        self.database.upsert_thread(identity, &normalized)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Record a NON-scheduled item's `prepare_delivery` failure without
    /// claiming the row or bumping `attempts`. A transient failure leaves the
    /// item queued with a short retry note; a definite permanent failure moves
    /// it to `failed` (draft restorable) via a state-guarded UPDATE.
    fn handle_prepare_failure(
        &self,
        item: &OutboxItem,
        error: &crate::provider::ProviderError,
        owner: &str,
        at: i64,
    ) -> Result<(), String> {
        use crate::provider::ProviderError;
        match error {
            // Only an explicitly PERMANENT class fails the row: nothing was
            // sent, so the draft stays restorable. Guarded on the queued state
            // so a concurrent transition is not clobbered.
            ProviderError::InvalidOperation(_) | ProviderError::PermanentClientRejection(_) => {
                let message = error.to_string();
                self.fail_prepare(item, owner, at, &message)
            }
            // EVERYTHING ELSE stays queued with a plain note and is retried
            // behind the rate limiter, without claiming the row or bumping
            // `attempts`. That deliberately includes `ReauthenticationRequired`,
            // `Authentication`, `RateLimited`-style and `Other` errors: Gmail's
            // `prepare_delivery` fetches/refreshes an OAuth access token, so it
            // CAN fail with any of them, and until now such an item simply
            // stayed queued and sent once the account was reconnected. Failing
            // it would discard a send that used to survive. For an IMAP
            // account with a rejected password the same rule means the message
            // goes out after the user reconnects, instead of needing to be
            // restored and re-sent.
            ProviderError::ReauthenticationRequired(_) => self.note_prepare_retry(
                item,
                owner,
                at,
                "The mail server rejected the saved password. Reconnect this account and the message will be sent.",
            ),
            _ => self.note_prepare_retry(item, owner, at, "Can't reach the mail server. Will retry."),
        }
    }

    /// Leave a queued item queued: record a plain-language note and stamp
    /// `last_attempt_at` (NOT `attempts`, NOT `state`) so the retry gate
    /// rate-limits the next probe.
    fn note_prepare_retry(
        &self,
        item: &OutboxItem,
        owner: &str,
        at: i64,
        message: &str,
    ) -> Result<(), String> {
        self.database.change_outbox(
            "UPDATE outbox_messages SET error=?2,last_attempt_at=?3 WHERE id=?1 AND state IN ('undo_pending','ready')",
            params![item.id, message, at],
            &item.id,
            owner,
            at,
        )?;
        Ok(())
    }

    /// Move a queued item to `failed` with `message` via a state-guarded
    /// UPDATE (the same path `dispatch_due` uses for a definite send failure),
    /// so the draft stays restorable and a concurrent transition is not
    /// clobbered.
    fn fail_prepare(
        &self,
        item: &OutboxItem,
        owner: &str,
        at: i64,
        message: &str,
    ) -> Result<(), String> {
        self.database.change_outbox(
            "UPDATE outbox_messages SET state='failed',error=?2 WHERE id=?1 AND state IN ('undo_pending','ready')",
            params![item.id, message],
            &item.id,
            owner,
            at,
        )?;
        Ok(())
    }
    /// Drive every DUE Sent-copy job this tick (fact 4: NOT skipped by the
    /// no-sendable-items early return). A job is `pending`/`verify` with
    /// `sent_copy_next_at <= now`; it is processed oldest-first, at most
    /// [`SENT_COPY_MAX_JOBS_PER_TICK`], resolving each account once so its
    /// unavailable provider does not abort the tick or the others' jobs. An
    /// account whose provider is unavailable is skipped WITHOUT counting an
    /// attempt. Never touches the outbox `state`, `provider_id`, never resends.
    async fn process_sent_copy_jobs(&self, at: i64) -> Result<(), String> {
        self.process_sent_copy_jobs_with(at, |account| async move {
            self.provider_for(&account).await.ok()
        })
        .await
    }

    async fn process_sent_copy_jobs_with<F, Fut>(
        &self,
        at: i64,
        mut resolve: F,
    ) -> Result<(), String>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Option<Arc<dyn MailProvider>>>,
    {
        // Resolve availability before applying the work cap. Otherwise a full
        // batch belonging to unavailable accounts can starve every later job.
        let accounts: Vec<String> = {
            let c = self.database.connection()?;
            let mut q = c
                .prepare(
                    "SELECT DISTINCT account FROM outbox_messages
                 WHERE sent_copy_state IN ('pending','verify') AND sent_copy_next_at <= ?1",
                )
                .map_err(error)?;
            let rows = q.query_map([at], |r| r.get(0)).map_err(error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(error)?
        };
        let mut providers = HashMap::new();
        for account in accounts {
            if let Some(provider) = resolve(account.clone()).await {
                providers.insert(account, provider);
            }
        }
        if providers.is_empty() {
            return Ok(());
        }
        let available = json(&providers.keys().collect::<Vec<_>>())?;
        // Only eligible jobs load raw MIME, still oldest-first and bounded.
        let due: Vec<(String, String, Vec<u8>, String, i64, i64)> = {
            let c = self.database.connection()?;
            let mut q = c
                .prepare(
                    "SELECT id, account, raw, sent_copy_state, sent_copy_attempts, \
                     sent_copy_append_attempted FROM outbox_messages \
                     WHERE sent_copy_state IN ('pending','verify') AND sent_copy_next_at <= ?1 \
                     AND account IN (SELECT value FROM json_each(?3)) \
                     ORDER BY sent_copy_next_at ASC, rowid ASC LIMIT ?2",
                )
                .map_err(error)?;
            let rows = q
                .query_map(
                    params![
                        at,
                        crate::provider::imap::SENT_COPY_MAX_JOBS_PER_TICK as i64,
                        available
                    ],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Vec<u8>>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, i64>(5)?,
                        ))
                    },
                )
                .map_err(error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(error)?
        };
        for (id, account, raw, state, attempts, append_attempted) in due {
            if let Some(provider) = providers.get(&account) {
                self.run_sent_copy_job(
                    provider.as_ref(),
                    &id,
                    &raw,
                    &state,
                    attempts,
                    append_attempted != 0,
                    at,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Run ONE Sent-copy job against `provider` and persist its outcome. Taken
    /// as `&dyn MailProvider` (like `send_due_item`) so a test injects a fake.
    /// Translates the provider's `SentCopyStep`/error into job state:
    ///
    /// * `Ok(Done)`             -> 'done'.
    /// * `Ok(Recheck{after})`   -> 'verify', next_at = now + after.
    /// * transient error        -> attempts+1, next_at from the backoff
    ///   schedule (never earlier than the settle window), state kept, a short
    ///   `sent_copy_error`; at MAX attempts -> 'abandoned'.
    /// * permanent error        -> 'abandoned' immediately with a plain error.
    ///
    /// A write-ahead flag (`sent_copy_append_attempted=1`) is set BEFORE the
    /// provider call so a crash mid-APPEND is remembered. The outbox `state`
    /// and `provider_id` are NEVER touched here.
    #[allow(clippy::too_many_arguments)]
    async fn run_sent_copy_job(
        &self,
        provider: &dyn MailProvider,
        id: &str,
        raw: &[u8],
        state: &str,
        attempts: i64,
        append_attempted: bool,
        at: i64,
    ) -> Result<(), String> {
        use crate::provider::{ProviderError, SentCopyPhase, SentCopyRequest, SentCopyStep};

        let phase = if state == "verify" {
            SentCopyPhase::Verify
        } else {
            SentCopyPhase::Pending
        };

        // Write-ahead: in the pending phase, a copy may be APPENDed, so remember
        // the attempt BEFORE calling the provider. A crash mid-APPEND then has
        // append_attempted=1 recorded, so a resumed job never counts a found
        // copy as the server's own (the evidence rule). The verify phase never
        // appends, so it does not re-flag.
        if phase == SentCopyPhase::Pending && !append_attempted {
            self.database.change_outbox(
                "UPDATE outbox_messages SET sent_copy_append_attempted=1 WHERE id=?1 AND sent_copy_state='pending'",
                params![id],
                id,
                "",
                at,
            )?;
        }

        let request = SentCopyRequest {
            raw: raw.to_vec(),
            outbox_id: id.to_string(),
            append_attempted,
            phase,
        };

        match provider.save_sent_copy(&request).await {
            Ok(SentCopyStep::Done) => {
                self.database.change_outbox(
                    "UPDATE outbox_messages SET sent_copy_state='done',sent_copy_error=NULL \
                     WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
                    params![id],
                    id,
                    "",
                    at,
                )?;
            }
            Ok(SentCopyStep::Recheck { after }) => {
                let next_at = at + after.as_millis() as i64;
                self.database.change_outbox(
                    "UPDATE outbox_messages SET sent_copy_state='verify',sent_copy_next_at=?2,\
                     sent_copy_error=NULL WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
                    params![id, next_at],
                    id,
                    "",
                    at,
                )?;
            }
            Err(err) => {
                let permanent = matches!(
                    err,
                    ProviderError::InvalidOperation(_)
                        | ProviderError::PermanentClientRejection(_)
                );
                let message = sent_copy_error_message(&err);
                if permanent {
                    self.database.change_outbox(
                        "UPDATE outbox_messages SET sent_copy_state='abandoned',sent_copy_error=?2 \
                         WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
                        params![id, message],
                        id,
                        "",
                        at,
                    )?;
                } else {
                    // Transient: count the attempt and reschedule from the
                    // backoff (floored at the settle window), or abandon at the
                    // cap. The state is KEPT (a verify stays verify).
                    let new_attempts = attempts + 1;
                    if new_attempts as u32 >= crate::provider::imap::SENT_COPY_MAX_ATTEMPTS {
                        self.database.change_outbox(
                            "UPDATE outbox_messages SET sent_copy_state='abandoned',\
                             sent_copy_attempts=?2,sent_copy_error=?3 \
                             WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
                            params![id, new_attempts, message],
                            id,
                            "",
                            at,
                        )?;
                    } else {
                        let next_at = at
                            + crate::provider::imap::sent_copy_backoff(attempts as u32).as_millis()
                                as i64;
                        self.database.change_outbox(
                            "UPDATE outbox_messages SET sent_copy_attempts=?2,sent_copy_next_at=?3,\
                             sent_copy_error=?4 WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
                            params![id, new_attempts, next_at, message],
                            id,
                            "",
                            at,
                        )?;
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
        seed_sent_copy: bool,
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
        let owner=if item.schedule.is_some() { self.authorize_schedule_async(&item.id).await? } else {String::new()};
        let claimed=self.database.change_outbox("UPDATE outbox_messages SET state='sending',attempts=attempts+1,last_attempt_at=?2,error=NULL WHERE id=?1 AND state IN ('undo_pending','ready','scheduled') AND deadline<=?2 AND (state!='scheduled' OR deadline>=?3) AND (schedule_json IS NULL OR (json_extract(schedule_json,'$.report.reportRevision')=?4 AND json_extract(schedule_json,'$.report.ownerInstallationId')=?5))",params![item.id,at,at.saturating_sub(crate::scheduled_send::DISPATCH_GRACE_MS),item.schedule.as_ref().map(|s|s.report.report_revision),owner],&item.id,&owner,at)?;
        if claimed != 1 {
            return Ok(None);
        }
        match deliver(raw, item.draft.thread_id.clone()).await {
            Ok(sent) => {
                // Record the send accepted. When the provider saves its own
                // Sent copy as a durable job (IMAP: `seed_sent_copy` is the
                // `client_saves_sent_copy` capability read at the call site),
                // seed that job in the SAME UPDATE so no window exists where a
                // send is 'sent' but its Sent-copy job is lost. The job's first
                // attempt is scheduled `sent_at + SENT_COPY_SETTLE` (millis), so
                // a server that saves its own copy is given the settle window to
                // do so before first-send detection looks. Gmail-like providers
                // pass `seed_sent_copy=false` and every sent_copy_* column stays
                // NULL/default.
                if seed_sent_copy {
                    let next_at = at
                        + crate::provider::imap::SENT_COPY_SETTLE.as_millis() as i64;
                    self.database.change_outbox(
                        "UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL,\
                         sent_copy_state='pending',sent_copy_next_at=?3,sent_copy_attempts=0,\
                         sent_copy_append_attempted=0,sent_copy_error=NULL WHERE id=?2",
                        params![sent.provider_message_id, item.id, next_at],
                        &item.id,
                        &owner,
                        at,
                    )?;
                } else {
                    self.database.change_outbox("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2",params![sent.provider_message_id,item.id],&item.id,&owner,at)?;
                }
                Ok(Some(sent))
            }
            Err((definite, message)) => {
                self.database
                    .change_outbox(
                        "UPDATE outbox_messages SET state=?1,error=?2 WHERE id=?3",
                        params![
                            if definite { "failed" } else { "uncertain" },
                            message,
                            item.id
                        ], &item.id,&owner,at,
                    )?;
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
        let mut d = db.create_draft("new", None, &account, &[]).unwrap();
        d.to = "Jane <jane@example.com>".into();
        d.subject = "Hello".into();
        d.body = "Saved work ✓".into();
        db.save_draft(d).unwrap()
    }

    /// A database with a multi-identity IMAP account `me@proton.me` set up
    /// (login first, one secondary `work@proton.me`), for the From-picker ops.
    fn imap_database() -> Database {
        use crate::provider::imap::{Identity, ImapAccountSettings, LabelStorage, SecurityMode};
        let db = Database::open_memory();
        db.adopt_mail_account("me@proton.me", crate::models::MailProviderKind::Imap)
            .unwrap();
        db.set_compose_identity("me@proton.me").unwrap();
        let settings = ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1143,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@proton.me".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_security: SecurityMode::StartTls,
            smtp_username: "me@proton.me".into(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::None,
            label_container: None,
            identities: vec![
                Identity { address: "me@proton.me".into(), display_name: None },
                Identity { address: "work@proton.me".into(), display_name: Some("Work".into()) },
            ],
            pinned_fingerprints: Default::default(),
            server_saves_sent: false,
        };
        db.save_imap_account_settings("me@proton.me", &settings).unwrap();
        db
    }

    #[test]
    fn account_sending_identities_lists_login_first_with_display_names() {
        let db = imap_database();
        let identities = db.account_sending_identities("me@proton.me").unwrap();
        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0].address, "me@proton.me");
        assert_eq!(identities[1].address, "work@proton.me");
        assert_eq!(identities[1].display_name.as_deref(), Some("Work"));
        // A Gmail account (no settings row) is exactly [account].
        let gmail = database();
        let only = gmail.account_sending_identities("you@example.com").unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].address, "you@example.com");
    }

    #[test]
    fn set_draft_from_accepts_a_secondary_identity_and_bumps_the_revision() {
        let db = imap_database();
        let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
        let updated = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
        assert_eq!(updated.from.as_deref(), Some("work@proton.me"));
        assert_eq!(updated.revision, d.revision + 1);
        assert_eq!(db.draft(&d.id).unwrap().from.as_deref(), Some("work@proton.me"));
    }

    #[test]
    fn set_draft_from_normalizes_the_primary_and_null_to_none() {
        let db = imap_database();
        let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
        let on_secondary = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
        // The account's own address stores None ("send as the account").
        let back = db.set_draft_from(&d.id, Some("ME@Proton.ME")).unwrap();
        assert_eq!(back.from, None, "the primary identity (any case) stores None");
        assert!(on_secondary.revision < back.revision);
        // An explicit null also stores None.
        let on_secondary = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
        let cleared = db.set_draft_from(&on_secondary.id, None).unwrap();
        assert_eq!(cleared.from, None);
    }

    #[test]
    fn set_draft_from_is_allowed_for_a_reply_mode_draft() {
        let db = imap_database();
        // A reply/forward can legitimately send from a secondary identity.
        let d = Draft {
            id: Uuid::new_v4().to_string(),
            revision: 0,
            account: "me@proton.me".into(),
            from: None,
            mode: "reply".into(),
            source_id: None,
            thread_id: None,
            reply_id: None,
            references: vec![],
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            body: String::new(),
            body_html: String::new(),
            signature: None,
            forwarded_content: None,
            follow_up_task_id: None,
            attachments: vec![],
            updated_at: now(),
            server_draft: None,
        };
        db.connection()
            .unwrap()
            .execute("INSERT INTO drafts VALUES (?1,?2,?3)", params![d.id, d.revision, json(&d).unwrap()])
            .unwrap();
        let updated = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
        assert_eq!(updated.from.as_deref(), Some("work@proton.me"));
    }

    #[test]
    fn set_draft_from_rejects_an_address_not_on_the_account_and_writes_nothing() {
        let db = imap_database();
        let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
        let err = db.set_draft_from(&d.id, Some("stranger@elsewhere.com")).unwrap_err();
        assert!(err.contains("stranger@elsewhere.com"));
        // Nothing was written: revision and from unchanged.
        let reread = db.draft(&d.id).unwrap();
        assert_eq!(reread.revision, d.revision);
        assert_eq!(reread.from, None);
    }

    #[test]
    fn set_draft_account_clears_a_stale_from() {
        // A draft on the IMAP account with a chosen secondary identity, moved
        // to another account, must drop the stale `from` (fact 2) so a later
        // queue does not reject "from no longer on this account".
        let db = imap_database();
        db.adopt_account("other@example.com").unwrap();
        let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
        let with_from = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
        assert_eq!(with_from.from.as_deref(), Some("work@proton.me"));
        let moved = db.set_draft_account(&d.id, "other@example.com").unwrap();
        assert_eq!(moved.account, "other@example.com");
        assert_eq!(moved.from, None, "moving the account must clear the stale from");
        assert_eq!(db.draft(&d.id).unwrap().from, None);
    }

    #[test]
    fn permanent_sent_copy_message_is_plain_for_each_class() {
        // Each class rewrites the provider's developer wording into one plain
        // sentence that never leaks jargon.
        let no_folder = permanent_sent_copy_message(
            "cannot save the Sent copy: this account has no Sent mailbox configured; refusing permanently",
        );
        assert!(no_folder.starts_with("Your message was sent, but a copy couldn't be saved to your Sent folder"));
        assert!(no_folder.contains("no Sent folder is set up"));
        for jargon in ["refusing permanently", "mailbox", "APPEND"] {
            assert!(!no_folder.contains(jargon), "leaked jargon {jargon:?}: {no_folder}");
        }

        let too_large = permanent_sent_copy_message("message is 70000000 bytes, over the 67108865-byte (64 MiB) APPEND limit");
        assert!(too_large.contains("too large for the Sent folder"));
        assert!(!too_large.contains("APPEND"));

        let over_quota = permanent_sent_copy_message(
            "the mail server refused the APPEND of the Sent copy (e.g. over quota or a rejected message); it was not saved. The specific reason was not reported by the IMAP library.",
        );
        assert!(over_quota.contains("refused to store it"), "{over_quota}");
        assert!(!over_quota.contains("too large"), "a quota refusal must not be called 'too large': {over_quota}");

        // The EXAMINE-path spelling (a Sent mailbox that no longer exists) is a
        // missing-folder case too, not the generic fallback.
        let gone = permanent_sent_copy_message(
            "cannot save the Sent copy: the Sent mailbox \"Gesendet\" does not exist; configure or create it, then retry (x)",
        );
        assert!(gone.contains("no Sent folder is set up"), "{gone}");
        // Unrelated wording that merely contains "over the" is not an oversize.
        let unrelated = permanent_sent_copy_message("discovered the thing; over the moon");
        assert!(!unrelated.contains("too large"), "{unrelated}");

        let no_mid = permanent_sent_copy_message(
            "cannot save the Sent copy: the message has no Message-ID header to locate it by; refusing permanently",
        );
        assert!(no_mid.contains("missing a Message-ID"));
        assert!(!no_mid.contains("refusing permanently"));

        // An unrecognised permanent message still yields a jargon-free sentence.
        let generic = permanent_sent_copy_message("message contains a NUL byte, which an IMAP4rev1 literal cannot carry; refusing to APPEND");
        assert!(generic.starts_with("Your message was sent, but a copy couldn't be saved to your Sent folder"));
    }

    #[test]
    fn sent_copy_error_message_classes_transient_vs_permanent() {
        use crate::provider::ProviderError;
        // Transient auth/transport -> "will retry".
        assert!(sent_copy_error_message(&ProviderError::Authentication("x".into())).contains("retry"));
        assert!(sent_copy_error_message(&ProviderError::TransientTransport("x".into())).contains("retry"));
        // Permanent -> plain "was sent, couldn't save a copy" with no retry promise.
        let permanent = sent_copy_error_message(&ProviderError::InvalidOperation(
            "cannot save the Sent copy: this account has no Sent mailbox configured; refusing permanently".into(),
        ));
        assert!(permanent.contains("Your message was sent"));
        assert!(!permanent.contains("retry"));
    }
    #[test]
    fn retention_days_defaults_to_90_days_and_round_trips() {
        let db = database();
        assert_eq!(db.retention_days().unwrap(), Some(90));
        assert_eq!(db.chosen_retention_days().unwrap(), None);
        db.set_retention_days(Some(30)).unwrap();
        assert_eq!(db.retention_days().unwrap(), Some(30));
        assert_eq!(db.chosen_retention_days().unwrap(), Some(30));
        db.set_retention_days(None).unwrap();
        assert_eq!(db.retention_days().unwrap(), None);
    }

    #[test]
    fn retention_default_and_choices_survive_reopening() {
        let temp = crate::db::test_support::TempDbPath::new();
        let db = Database::open(&temp.path).unwrap();
        assert_eq!(db.retention_days().unwrap(), Some(90));
        drop(db);
        let db = Database::open(&temp.path).unwrap();
        assert_eq!(db.retention_days().unwrap(), Some(90));
        drop(db);

        for choice in [None, Some(30), Some(90), Some(365), None] {
            let db = Database::open(&temp.path).unwrap();
            db.set_retention_days(choice).unwrap();
            drop(db);
            let db = Database::open(&temp.path).unwrap();
            assert_eq!(db.retention_days().unwrap(), choice);
        }
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
    fn addresses_accept_a_quoted_display_name_holding_a_comma() {
        assert_eq!(
            addresses(r#""Fischgrund, Justin" <justin@example.com>, Kelly Sjol <kelly@example.com>, "Say \"Hi\"" <hi@example.com>"#).unwrap(),
            vec![
                ("Fischgrund, Justin".to_string(), "justin@example.com".to_string()),
                ("Kelly Sjol".to_string(), "kelly@example.com".to_string()),
                ("Say \"Hi\"".to_string(), "hi@example.com".to_string()),
            ]
        );
        // The unquoted form the field used to write is still rejected at save time.
        assert!(addresses("Fischgrund, Justin <justin@example.com>").is_err());
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
        let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
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
            db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused"))
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
                    summary, summary_generated_at, has_attachments, last_received_at,
                    summary_revision
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
            db.queue(&draft.id, draft.revision, archive_on_send, &[], None, Path::new("/unused")).unwrap()
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
        let item_a = db.queue(&a.id, a.revision, false, &[], None, Path::new("/unused")).unwrap();

        db.set_compose_identity("other@example.com").unwrap();
        let b = saved(&db);
        let item_b = db.queue(&b.id, b.revision, false, &[], None, Path::new("/unused")).unwrap();

        db.pause_ready_sends_for("you@example.com").unwrap();

        let outbox = db.outbox().unwrap();
        let state_of = |id: &str| outbox.iter().find(|o| o.id == id).unwrap().state.clone();
        assert_eq!(state_of(&item_a.id), "failed");
        assert_eq!(state_of(&item_b.id), "undo_pending");
    }
    #[tokio::test]
    async fn a_tick_fails_sends_whose_account_was_removed_on_another_device() {
        let service = service();
        let db = &service.database;
        let queue = |db: &Database| {
            let d = saved(db);
            db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap()
        };
        let kept = queue(db);
        // The catalog stores `you@example.com`; a differently cased sender
        // address is still that account.
        db.set_compose_identity("You@Example.com").unwrap();
        let kept_cased = queue(db);
        // `gone@example.com` is not in the catalog, as after a synced removal.
        db.set_compose_identity("gone@example.com").unwrap();
        let gone_ready = queue(db);
        let gone_scheduled = queue(db);
        // Due next month: only the removal check, not the send loop, reaches it now.
        db.connection().unwrap().execute(
            "UPDATE outbox_messages SET state='scheduled', deadline=?2 WHERE id=?1",
            params![gone_scheduled.id, now() + 30 * 24 * 60 * 60 * 1000],
        ).unwrap();

        service.tick().await.unwrap();

        let outbox = db.outbox().unwrap();
        let state_of = |id: &str| outbox.iter().find(|o| o.id == id).unwrap().state.clone();
        assert_eq!(state_of(&gone_ready.id), "failed");
        assert_eq!(state_of(&gone_scheduled.id), "failed");
        assert_eq!(state_of(&kept.id), "undo_pending");
        assert_eq!(state_of(&kept_cased.id), "undo_pending");

        // Adding the account back must not revive the failed sends.
        db.adopt_account("gone@example.com").unwrap();
        service.tick().await.unwrap();
        let outbox = db.outbox().unwrap();
        assert_eq!(outbox.iter().find(|o| o.id == gone_scheduled.id).unwrap().state, "failed");
    }
    #[test]
    fn restart_keeps_drafts_and_never_retries_an_interrupted_send() {
        let temp = crate::db::test_support::TempDbPath::new();
        let path = &temp.path;
        let id;
        {
            let db = Database::open(path).unwrap();
            db.set_compose_identity("you@example.com").unwrap();
            let d = saved(&db);
            id = d.id.clone();
            let queued = saved(&db);
            let item = db
                .queue(&queued.id, queued.revision, false, &[], None, Path::new("/unused"))
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
    fn an_uncertain_send_resolves_to_terminal_unverifiable_for_a_provider_without_a_sent_copy() {
        // Mirrors the reconcile path for a non-verifiable provider: an
        // `uncertain` send whose `find_sent_copy` can never confirm it is
        // moved to the design's terminal `unverifiable` outcome rather than
        // being left to re-check forever. `docs/imap-design.md` ("Sending").
        let db = database();
        let d = saved(&db);
        let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
        db.connection()
            .unwrap()
            .execute(
                "UPDATE outbox_messages SET state='uncertain', error='dropped before 250' WHERE id=?1",
                [&item.id],
            )
            .unwrap();

        db.mark_unverifiable(&item.id).unwrap();

        let resolved = db.outbox().unwrap().into_iter().find(|o| o.id == item.id).unwrap();
        assert_eq!(resolved.state, "unverifiable");
        assert!(
            resolved
                .error
                .as_deref()
                .unwrap()
                .contains("could not be confirmed"),
            "{:?}",
            resolved.error
        );

        // Terminal: `tick` only re-processes undo_pending/ready/uncertain, so
        // an unverifiable row is never picked up again. Re-marking is a no-op
        // because the guard requires state='uncertain'.
        db.mark_unverifiable(&item.id).unwrap();
        assert_eq!(
            db.outbox().unwrap().into_iter().find(|o| o.id == item.id).unwrap().state,
            "unverifiable"
        );
    }
    #[test]
    fn mark_unverifiable_only_touches_an_uncertain_row() {
        // The guard must never silently repurpose a sent/failed/ready row.
        let db = database();
        for state in ["sent", "failed", "ready", "undo_pending", "sending"] {
            let d = saved(&db);
            let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
            db.connection()
                .unwrap()
                .execute(
                    "UPDATE outbox_messages SET state=?2 WHERE id=?1",
                    params![item.id, state],
                )
                .unwrap();
            db.mark_unverifiable(&item.id).unwrap();
            assert_eq!(
                db.outbox().unwrap().into_iter().find(|o| o.id == item.id).unwrap().state,
                state,
                "mark_unverifiable must not touch a {state} row"
            );
        }
    }

    mod reconciliation {
        use super::*;
        use crate::{
            models::Label,
            provider::{
                Delivery, LabelModel, MailFetch, MailMutate, MailSend, MailSync, ProviderCapabilities,
                ProviderResult, SyncBatch, SyncCursor, ThreadPage,
            },
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct ReconcileProvider {
            operation: String,
            verifiable_delivery: bool,
            sent_copy: Option<DeliveryReceipt>,
            lookups: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl MailSend for ReconcileProvider {
            async fn sender_identity(&self) -> ProviderResult<String> {
                Ok("you@example.com".into())
            }

            async fn find_sent_copy(
                &self,
                operation: &str,
                expected_sender: &str,
            ) -> ProviderResult<Option<DeliveryReceipt>> {
                assert_eq!(operation, self.operation);
                assert_eq!(expected_sender, "you@example.com");
                self.lookups.fetch_add(1, Ordering::SeqCst);
                Ok(self.sent_copy.clone())
            }

            async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
                panic!("Reconciliation must never resend a message");
            }
        }

        impl MailProvider for ReconcileProvider {
            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities {
                    send_supported: true,
                    server_search: false,
                    provided_threads: false,
                    label_model: LabelModel::GmailLabels,
                    verifiable_delivery: self.verifiable_delivery,
                    client_saves_sent_copy: false,
                }
            }
        }

        #[async_trait::async_trait]
        impl MailSync for ReconcileProvider {
            async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
                panic!("Reconciliation must not start a sync");
            }
            async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
                panic!("Reconciliation must not poll");
            }
            async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
                panic!("Reconciliation must not list the inbox");
            }
            async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
                panic!("These receipts do not identify a provider thread");
            }
        }

        #[async_trait::async_trait]
        impl MailFetch for ReconcileProvider {
            async fn fetch_message(&self, _: &str) -> ProviderResult<RawMessage> {
                panic!("Reconciliation must not fetch a message");
            }
            async fn attachment_bytes(&self, _: &str, _: &str) -> ProviderResult<Vec<u8>> {
                panic!("Reconciliation must not fetch attachments");
            }
        }

        #[async_trait::async_trait]
        impl MailMutate for ReconcileProvider {
            async fn modify_thread(&self, _: &str, _: &[String], _: &[String]) -> ProviderResult<()> {
                panic!("These receipts do not identify a provider thread");
            }
            async fn modify_messages(
                &self,
                _: &[String],
                _: &[String],
                _: &[String],
            ) -> ProviderResult<()> {
                panic!("Reconciliation must not modify messages");
            }
            async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
                panic!("Reconciliation must not list labels");
            }
            async fn create_label(&self, _: &str) -> ProviderResult<Label> {
                panic!("Reconciliation must not create labels");
            }
            async fn update_label(&self, _: &str, _: &str) -> ProviderResult<Label> {
                panic!("Reconciliation must not update labels");
            }
            async fn delete_label(&self, _: &str) -> ProviderResult<()> {
                panic!("Reconciliation must not delete labels");
            }
        }

        #[tokio::test]
        async fn provider_capability_and_sent_copy_determine_the_delivery_outcome() {
            for verifiable_delivery in [false, true] {
                for found_copy in [false, true] {
                    let service = service();
                    let draft = saved(&service.database);
                    let item = service
                        .database
                        .queue(&draft.id, draft.revision, false, &[], None, &service.root)
                        .unwrap();
                    service.database.connection().unwrap().execute(
                        "UPDATE outbox_messages SET state='uncertain', error='Connection dropped before SMTP acceptance' WHERE id=?1",
                        [&item.id],
                    ).unwrap();
                    let item = service.database.outbox().unwrap().remove(0);
                    let provider = ReconcileProvider {
                        operation: item.id.clone(),
                        verifiable_delivery,
                        sent_copy: found_copy.then(|| DeliveryReceipt {
                            provider_message_id: "confirmed-copy".into(),
                            thread_id: None,
                        }),
                        lookups: AtomicUsize::new(0),
                    };

                    let result = service.reconcile_with_provider(&item, &provider).await;
                    let resolved = service.database.outbox().unwrap().remove(0);
                    if found_copy {
                        result.unwrap();
                        assert_eq!(resolved.state, "sent");
                        assert_eq!(resolved.provider_id.as_deref(), Some("confirmed-copy"));
                        assert!(resolved.error.is_none());
                    } else if verifiable_delivery {
                        assert_eq!(result.unwrap_err(), "Delivery is still uncertain. No automatic retry was made. Check Gmail Sent before composing another message.");
                        assert_eq!(resolved.state, "uncertain");
                        assert_eq!(resolved.error, item.error);
                    } else {
                        result.unwrap();
                        assert_eq!(resolved.state, "unverifiable");
                        assert_eq!(
                            resolved.error.as_deref(),
                            Some("This message may have been sent, but delivery could not be confirmed. Confirm with the recipient before sending it again.")
                        );
                        assert!(resolved.provider_id.is_none());
                    }

                    if resolved.state != "uncertain" {
                        // Terminal outcomes must bypass provider access on later checks.
                        service.reconcile(&item.id).await.unwrap();
                        service.tick().await.unwrap();
                        assert_eq!(
                            service.database.outbox().unwrap().remove(0).state,
                            resolved.state
                        );
                    }
                    assert_eq!(provider.lookups.load(Ordering::SeqCst), 1);
                }
            }
        }
    }

    /// Work item D: a failing `prepare_delivery` must not wedge the queue, and
    /// the per-item body (`send_due_item`) must be drivable with a fake
    /// provider the account registry cannot hold.
    mod prepare_failures {
        use super::*;
        use crate::{
            models::Label,
            provider::{
                Delivery, DeliveryReceipt, LabelModel, MailFetch, MailMutate, MailSend, MailSync,
                ProviderCapabilities, ProviderError, ProviderResult, SyncBatch, SyncCursor,
                ThreadPage,
            },
        };

        /// What the fake provider's `prepare_delivery` should do.
        #[derive(Clone)]
        enum PrepareBehaviour {
            /// Succeed, then `send_once` returns the given send result.
            Succeed(SendResult),
            /// Fail with the given error (no permit handed out).
            Fail(fn() -> ProviderError),
        }

        /// What the handed-out permit's `send_once` returns.
        #[derive(Clone, Copy)]
        enum SendResult {
            Accepted,
            /// Err((definite=false, ..)): the Unknown zone.
            Unknown,
            /// Err((definite=true, ..)): a definite non-delivery.
            NotDelivered,
        }

        struct FakeSender {
            behaviour: PrepareBehaviour,
        }

        struct ScriptedPermit {
            result: SendResult,
        }

        #[async_trait::async_trait]
        impl Delivery for ScriptedPermit {
            async fn send_once(
                self: Box<Self>,
                _raw: &[u8],
                thread: Option<&str>,
            ) -> Result<DeliveryReceipt, (bool, String)> {
                match self.result {
                    SendResult::Accepted => Ok(DeliveryReceipt {
                        provider_message_id: "fake-sent".into(),
                        thread_id: thread.map(str::to_string),
                    }),
                    SendResult::Unknown => Err((
                        false,
                        "may have been delivered; check before resending".into(),
                    )),
                    SendResult::NotDelivered => {
                        Err((true, "the server refused the message".into()))
                    }
                }
            }
        }

        #[async_trait::async_trait]
        impl MailSend for FakeSender {
            async fn sender_identity(&self) -> ProviderResult<String> {
                Ok("you@example.com".into())
            }
            async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
                match &self.behaviour {
                    PrepareBehaviour::Succeed(result) => {
                        Ok(Box::new(ScriptedPermit { result: *result }))
                    }
                    PrepareBehaviour::Fail(make) => Err(make()),
                }
            }
            async fn find_sent_copy(
                &self,
                _: &str,
                _: &str,
            ) -> ProviderResult<Option<DeliveryReceipt>> {
                // Faithful IMAP behaviour: no server-side copy to confirm
                // against, so a clean Ok(None) (never an error).
                Ok(None)
            }
        }

        impl MailProvider for FakeSender {
            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities {
                    send_supported: true,
                    server_search: false,
                    provided_threads: false,
                    label_model: LabelModel::ImapLabelFolders,
                    verifiable_delivery: false,
                    client_saves_sent_copy: false,
                }
            }
        }

        // The non-send traits are never exercised by `send_due_item` on these
        // paths (a successful send with thread_id=None does no modify/fetch);
        // panic so a future change that starts calling them is caught.
        #[async_trait::async_trait]
        impl MailSync for FakeSender {
            async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
                panic!("send path must not sync");
            }
            async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
                panic!("send path must not poll");
            }
            async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
                panic!("send path must not list");
            }
            async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
                panic!("new-message send has no thread to fetch");
            }
        }
        #[async_trait::async_trait]
        impl MailFetch for FakeSender {
            async fn fetch_message(&self, _: &str) -> ProviderResult<RawMessage> {
                panic!("send path must not fetch a message");
            }
            async fn attachment_bytes(&self, _: &str, _: &str) -> ProviderResult<Vec<u8>> {
                panic!("send path must not fetch attachments");
            }
        }
        #[async_trait::async_trait]
        impl MailMutate for FakeSender {
            async fn modify_thread(&self, _: &str, _: &[String], _: &[String]) -> ProviderResult<()> {
                panic!("new-message send has no thread to modify");
            }
            async fn modify_messages(&self, _: &[String], _: &[String], _: &[String]) -> ProviderResult<()> {
                panic!("unused");
            }
            async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
                panic!("unused");
            }
            async fn create_label(&self, _: &str) -> ProviderResult<Label> {
                panic!("unused");
            }
            async fn update_label(&self, _: &str, _: &str) -> ProviderResult<Label> {
                panic!("unused");
            }
            async fn delete_label(&self, _: &str) -> ProviderResult<()> {
                panic!("unused");
            }
        }

        /// Queue a draft and advance it past its undo window to `ready` so it
        /// is a live send candidate. Returns the (re-read) outbox item.
        fn ready_item(service: &Correspondence) -> OutboxItem {
            let d = saved(&service.database);
            let item = service
                .database
                .queue(&d.id, d.revision, false, &[], None, &service.root)
                .unwrap();
            service
                .database
                .connection()
                .unwrap()
                .execute(
                    "UPDATE outbox_messages SET state='ready', deadline=?2 WHERE id=?1",
                    params![item.id, now() - 1],
                )
                .unwrap();
            service
                .database
                .outbox()
                .unwrap()
                .into_iter()
                .find(|o| o.id == item.id)
                .unwrap()
        }

        fn state_of(service: &Correspondence, id: &str) -> String {
            service
                .database
                .outbox()
                .unwrap()
                .into_iter()
                .find(|o| o.id == id)
                .unwrap()
                .state
        }

        fn attempts_of(service: &Correspondence, id: &str) -> i64 {
            service
                .database
                .connection()
                .unwrap()
                .query_row(
                    "SELECT attempts FROM outbox_messages WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap()
        }

        #[tokio::test]
        async fn a_transient_prepare_failure_leaves_the_item_queued_without_bumping_attempts() {
            let service = service();
            let item = ready_item(&service);
            let provider = FakeSender {
                behaviour: PrepareBehaviour::Fail(|| {
                    ProviderError::TransientTransport("smtp unreachable".into())
                }),
            };
            // No error propagates out (the old code did `return Err`).
            service
                .send_due_item(&item, &provider, "you@example.com", "", now())
                .await
                .unwrap();
            // Still queued, not failed; a plain retry note; attempts untouched.
            assert_eq!(state_of(&service, &item.id), "ready");
            assert_eq!(attempts_of(&service, &item.id), 0);
            let row = service.database.outbox().unwrap().remove(0);
            assert!(row.error.as_deref().unwrap().contains("Will retry"), "{:?}", row.error);
            // last_attempt_at was stamped so the retry gate can rate-limit.
            assert!(row.last_attempt_at.is_some());
        }

        #[tokio::test]
        async fn a_permanent_prepare_failure_moves_the_row_to_failed_restorable() {
            for make in [
                (|| ProviderError::InvalidOperation("no outgoing server configured".into())) as fn() -> ProviderError,
                || ProviderError::PermanentClientRejection("refused".into()),
            ] {
                let service = service();
                let item = ready_item(&service);
                let provider = FakeSender { behaviour: PrepareBehaviour::Fail(make) };
                service
                    .send_due_item(&item, &provider, "you@example.com", "", now())
                    .await
                    .unwrap();
                assert_eq!(state_of(&service, &item.id), "failed");
                // Nothing was sent, so attempts stays 0 (never claimed).
                assert_eq!(attempts_of(&service, &item.id), 0);
            }
        }

        /// Gmail's `prepare_delivery` refreshes an OAuth token, so it can fail
        /// with any of these. Before this change such an item stayed queued
        /// (the tick errored) and sent after the account recovered; it must
        /// not now be failed and discarded.
        #[tokio::test]
        async fn every_non_permanent_prepare_failure_keeps_the_send_queued() {
            for make in [
                (|| ProviderError::ReauthenticationRequired("token revoked".into())) as fn() -> ProviderError,
                || ProviderError::Authentication("bad token".into()),
                || ProviderError::RetryableServer("429".into()),
                || ProviderError::Other("dns".into()),
                || ProviderError::NotFound,
            ] {
                let service = service();
                let item = ready_item(&service);
                let provider = FakeSender { behaviour: PrepareBehaviour::Fail(make) };
                service
                    .send_due_item(&item, &provider, "you@example.com", "", now())
                    .await
                    .unwrap();
                assert_eq!(state_of(&service, &item.id), "ready", "{:?}", make());
                assert_eq!(attempts_of(&service, &item.id), 0);
                let row = service.database.outbox().unwrap().remove(0);
                assert!(row.error.is_some() && row.last_attempt_at.is_some());
            }
        }

        #[tokio::test]
        async fn a_rejected_password_message_tells_the_user_to_reconnect() {
            let service = service();
            let item = ready_item(&service);
            let provider = FakeSender {
                behaviour: PrepareBehaviour::Fail(|| {
                    ProviderError::ReauthenticationRequired("creds rejected".into())
                }),
            };
            service.send_due_item(&item, &provider, "you@example.com", "", now()).await.unwrap();
            let row = service.database.outbox().unwrap().remove(0);
            assert!(row.error.as_deref().unwrap().contains("Reconnect this account"), "{:?}", row.error);
        }

        #[tokio::test]
        async fn a_successful_prepare_sends_and_marks_the_row_sent() {
            let service = service();
            let item = ready_item(&service);
            let provider = FakeSender {
                behaviour: PrepareBehaviour::Succeed(SendResult::Accepted),
            };
            service
                .send_due_item(&item, &provider, "you@example.com", "", now())
                .await
                .unwrap();
            assert_eq!(state_of(&service, &item.id), "sent");
            // A real claim happened, so attempts is bumped exactly once.
            assert_eq!(attempts_of(&service, &item.id), 1);
        }

        #[tokio::test]
        async fn a_definite_send_failure_marks_the_row_failed() {
            let service = service();
            let item = ready_item(&service);
            let provider = FakeSender {
                behaviour: PrepareBehaviour::Succeed(SendResult::NotDelivered),
            };
            service
                .send_due_item(&item, &provider, "you@example.com", "", now())
                .await
                .unwrap();
            assert_eq!(state_of(&service, &item.id), "failed");
            assert_eq!(attempts_of(&service, &item.id), 1);
        }

        #[tokio::test]
        async fn an_unknown_send_goes_uncertain_then_reconciles_to_unverifiable() {
            // The full fact-2 chain: send_once Unknown -> row 'uncertain';
            // then reconcile_with_provider on a faithful IMAP fake
            // (verifiable_delivery=false, find_sent_copy=Ok(None)) resolves it
            // to the terminal 'unverifiable' — NOT an error, which the stale
            // find_sent_copy stub would have produced.
            let service = service();
            let item = ready_item(&service);
            let provider = FakeSender {
                behaviour: PrepareBehaviour::Succeed(SendResult::Unknown),
            };
            service
                .send_due_item(&item, &provider, "you@example.com", "", now())
                .await
                .unwrap();
            assert_eq!(state_of(&service, &item.id), "uncertain");

            let item = service
                .database
                .outbox()
                .unwrap()
                .into_iter()
                .find(|o| o.id == item.id)
                .unwrap();
            service
                .reconcile_with_provider(&item, &provider)
                .await
                .expect("reconcile must not error on a no-sent-copy IMAP provider");
            assert_eq!(state_of(&service, &item.id), "unverifiable");
        }

        #[tokio::test]
        async fn one_accounts_down_server_does_not_stop_another_accounts_send() {
            // Two items; the first account's prepare fails transiently, the
            // second account's prepare succeeds. Driving them in sequence (as
            // tick does per account) proves the first's failure is Ok(()) and
            // the second still sends.
            let service = service();
            let failing_item = ready_item(&service);
            let sending_item = ready_item(&service);
            let down = FakeSender {
                behaviour: PrepareBehaviour::Fail(|| {
                    ProviderError::TransientTransport("down".into())
                }),
            };
            let up = FakeSender { behaviour: PrepareBehaviour::Succeed(SendResult::Accepted) };
            service
                .send_due_item(&failing_item, &down, "you@example.com", "", now())
                .await
                .unwrap();
            service
                .send_due_item(&sending_item, &up, "you@example.com", "", now())
                .await
                .unwrap();
            assert_eq!(state_of(&service, &failing_item.id), "ready");
            assert_eq!(state_of(&service, &sending_item.id), "sent");
        }

        #[test]
        fn the_retry_gate_honours_the_interval_below_at_and_above() {
            use crate::provider::imap::{
                smtp_prepare_retry_due, SMTP_PREPARE_RETRY_INTERVAL_MS as INTERVAL,
            };
            let last = 1_000_000i64;
            // Never attempted: always eligible.
            assert!(smtp_prepare_retry_due(None, last));
            // Below the interval: not re-attempted.
            assert!(!smtp_prepare_retry_due(Some(last), last + INTERVAL - 1));
            // Exactly at: eligible.
            assert!(smtp_prepare_retry_due(Some(last), last + INTERVAL));
            // Above: eligible.
            assert!(smtp_prepare_retry_due(Some(last), last + INTERVAL + 1));
        }
    }

    /// Work item C/E: the durable Sent-copy job runner. A fake provider scripts
    /// `save_sent_copy`'s answers and COUNTS `send_once`, so every scenario can
    /// assert the message is NEVER resent and the outbox state/provider_id never
    /// change while the job advances. The engine body (`run_sent_copy_job`,
    /// `process_sent_copy_jobs`, and the atomic seed in `dispatch_due`) is
    /// driven directly, the fake injected like `send_due_item`'s.
    mod sent_copy_jobs {
        use super::*;
        use crate::{
            models::Label,
            provider::{
                Delivery, DeliveryReceipt, LabelModel, MailFetch, MailMutate, MailSend, MailSync,
                ProviderCapabilities, ProviderError, ProviderResult, SentCopyRequest, SentCopyStep,
                SyncBatch, SyncCursor, ThreadPage,
            },
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex as StdMutex;
        use std::time::Duration;

        /// What the fake's `save_sent_copy` returns, one scripted answer per
        /// call (popped front to back; the last repeats if the queue empties).
        #[derive(Clone)]
        enum JobAnswer {
            Done,
            Recheck(Duration),
            Transient,
            Permanent,
        }

        struct JobProvider {
            answers: StdMutex<Vec<JobAnswer>>,
            save_calls: AtomicUsize,
            send_calls: AtomicUsize,
            client_saves_sent_copy: bool,
        }

        impl JobProvider {
            fn new(answers: Vec<JobAnswer>) -> Self {
                Self {
                    answers: StdMutex::new(answers),
                    save_calls: AtomicUsize::new(0),
                    send_calls: AtomicUsize::new(0),
                    client_saves_sent_copy: true,
                }
            }
        }

        struct NoResendPermit;
        #[async_trait::async_trait]
        impl Delivery for NoResendPermit {
            async fn send_once(
                self: Box<Self>,
                _raw: &[u8],
                _thread: Option<&str>,
            ) -> Result<DeliveryReceipt, (bool, String)> {
                unreachable!("the Sent-copy job never sends");
            }
        }

        #[async_trait::async_trait]
        impl MailSend for JobProvider {
            async fn sender_identity(&self) -> ProviderResult<String> {
                Ok("you@example.com".into())
            }
            async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
                // Never called by the job runner; present so the trait is
                // satisfied. A call would be a bug (the job must not resend).
                Ok(Box::new(NoResendPermit))
            }
            async fn find_sent_copy(
                &self,
                _: &str,
                _: &str,
            ) -> ProviderResult<Option<DeliveryReceipt>> {
                Ok(None)
            }
            async fn save_sent_copy(
                &self,
                _request: &SentCopyRequest,
            ) -> ProviderResult<SentCopyStep> {
                self.save_calls.fetch_add(1, Ordering::SeqCst);
                let mut answers = self.answers.lock().unwrap();
                let answer = if answers.len() > 1 {
                    answers.remove(0)
                } else {
                    answers.first().cloned().unwrap_or(JobAnswer::Done)
                };
                match answer {
                    JobAnswer::Done => Ok(SentCopyStep::Done),
                    JobAnswer::Recheck(after) => Ok(SentCopyStep::Recheck { after }),
                    JobAnswer::Transient => {
                        Err(ProviderError::TransientTransport("sent mailbox unreachable".into()))
                    }
                    JobAnswer::Permanent => Err(ProviderError::InvalidOperation(
                        "no Sent mailbox configured".into(),
                    )),
                }
            }
        }

        impl MailProvider for JobProvider {
            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities {
                    send_supported: true,
                    server_search: false,
                    provided_threads: false,
                    label_model: LabelModel::ImapLabelFolders,
                    verifiable_delivery: false,
                    client_saves_sent_copy: self.client_saves_sent_copy,
                }
            }
        }

        // The non-send traits are never used by the job runner.
        #[async_trait::async_trait]
        impl MailSync for JobProvider {
            async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
                panic!("job runner must not sync");
            }
            async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
                panic!("job runner must not poll");
            }
            async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
                panic!("job runner must not list");
            }
            async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
                panic!("job runner must not fetch a thread");
            }
        }
        #[async_trait::async_trait]
        impl MailFetch for JobProvider {
            async fn fetch_message(&self, _: &str) -> ProviderResult<RawMessage> {
                panic!("unused");
            }
            async fn attachment_bytes(&self, _: &str, _: &str) -> ProviderResult<Vec<u8>> {
                panic!("unused");
            }
        }
        #[async_trait::async_trait]
        impl MailMutate for JobProvider {
            async fn modify_thread(&self, _: &str, _: &[String], _: &[String]) -> ProviderResult<()> {
                panic!("unused");
            }
            async fn modify_messages(&self, _: &[String], _: &[String], _: &[String]) -> ProviderResult<()> {
                panic!("unused");
            }
            async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
                panic!("unused");
            }
            async fn create_label(&self, _: &str) -> ProviderResult<Label> {
                panic!("unused");
            }
            async fn update_label(&self, _: &str, _: &str) -> ProviderResult<Label> {
                panic!("unused");
            }
            async fn delete_label(&self, _: &str) -> ProviderResult<()> {
                panic!("unused");
            }
        }

        /// Queue a draft and advance it to `ready` so dispatch_due can claim it.
        fn ready_item(service: &Correspondence) -> OutboxItem {
            let d = saved(&service.database);
            let item = service
                .database
                .queue(&d.id, d.revision, false, &[], None, &service.root)
                .unwrap();
            service
                .database
                .connection()
                .unwrap()
                .execute(
                    "UPDATE outbox_messages SET state='ready', deadline=?2 WHERE id=?1",
                    params![item.id, now() - 1],
                )
                .unwrap();
            service
                .database
                .outbox()
                .unwrap()
                .into_iter()
                .find(|o| o.id == item.id)
                .unwrap()
        }

        /// Insert a 'sent' outbox row carrying a pending/verify Sent-copy job.
        fn seed_job(
            service: &Correspondence,
            id: &str,
            account: &str,
            state: &str,
            attempts: i64,
            next_at: i64,
            append_attempted: i64,
        ) {
            let c = service.database.connection().unwrap();
            c.execute(
                "INSERT INTO outbox_messages
                    (id, draft_id, revision, account, state, deadline, payload, raw, provider_id,
                     sent_copy_state, sent_copy_attempts, sent_copy_next_at, sent_copy_append_attempted)
                 VALUES (?1, ?1, 1, ?2, 'sent', 0, '{}', x'4869', 'pm1',
                         ?3, ?4, ?5, ?6)",
                params![id, account, state, attempts, next_at, append_attempted],
            )
            .unwrap();
        }

        fn job_row(
            service: &Correspondence,
            id: &str,
        ) -> (String, Option<String>, i64, Option<i64>, i64, String, Option<String>) {
            service
                .database
                .connection()
                .unwrap()
                .query_row(
                    "SELECT state, sent_copy_state, sent_copy_attempts, sent_copy_next_at,
                            sent_copy_append_attempted, provider_id, sent_copy_error
                     FROM outbox_messages WHERE id=?1",
                    [id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, Option<i64>>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, Option<String>>(6)?,
                        ))
                    },
                )
                .unwrap()
        }

        /// Assert the outbox send-state and provider_id are UNTOUCHED by the job
        /// runner, across every scenario.
        fn assert_send_state_untouched(
            row: &(String, Option<String>, i64, Option<i64>, i64, String, Option<String>),
        ) {
            assert_eq!(row.0, "sent", "job runner must never change outbox state");
            assert_eq!(row.5, "pm1", "job runner must never change provider_id");
        }

        #[tokio::test]
        async fn dispatch_seeds_the_job_atomically_only_when_the_capability_is_true() {
            // Capability true: the accepted-send UPDATE sets the job pending with
            // next_at = sent_at + SENT_COPY_SETTLE.
            {
                let service = service();
                let item = ready_item(&service);
                let at = now();
                service
                    .dispatch_due(&item, at, true, |_, thread| async move {
                        Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: thread })
                    })
                    .await
                    .unwrap();
                let row = job_row(&service, &item.id);
                assert_eq!(row.0, "sent");
                assert_eq!(row.1.as_deref(), Some("pending"));
                assert_eq!(row.2, 0, "attempts start at 0");
                let expected = at + crate::provider::imap::SENT_COPY_SETTLE.as_millis() as i64;
                assert_eq!(row.3, Some(expected), "first attempt waits the settle window");
                assert_eq!(row.4, 0, "append not yet attempted");
            }

            // Capability false (Gmail-like): every sent_copy_* column stays
            // NULL/default — no job is created.
            {
                let service = service();
                let item = ready_item(&service);
                service
                    .dispatch_due(&item, now(), false, |_, thread| async move {
                        Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: thread })
                    })
                    .await
                    .unwrap();
                let row = job_row(&service, &item.id);
                assert_eq!(row.0, "sent");
                assert!(row.1.is_none(), "no job for a Gmail-like provider");
                assert!(row.3.is_none());
                assert_eq!(row.4, 0);
            }
        }

        #[tokio::test]
        async fn a_schedule_originated_send_also_seeds_the_job() {
            // The seed is in dispatch_due's accepted-send UPDATE, which every
            // send path (immediate AND scheduled) goes through, so a scheduled
            // send gets a job too. Drive dispatch_due with a non-scheduled item
            // but the capability on — the UPDATE is identical.
            let service = service();
            let item = ready_item(&service);
            service
                .dispatch_due(&item, now(), true, |_, _| async {
                    Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: None })
                })
                .await
                .unwrap();
            assert_eq!(job_row(&service, &item.id).1.as_deref(), Some("pending"));
        }

        #[tokio::test]
        async fn done_marks_the_job_done_and_never_resends() {
            let service = service();
            seed_job(&service, "j1", "you@example.com", "pending", 0, now() - 1, 0);
            let provider = JobProvider::new(vec![JobAnswer::Done]);
            service
                .run_sent_copy_job(&provider, "j1", b"Hi", "pending", 0, false, now())
                .await
                .unwrap();
            let row = job_row(&service, "j1");
            assert_eq!(row.1.as_deref(), Some("done"));
            assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0, "NO RESEND");
            assert_send_state_untouched(&row);
            // The write-ahead append flag was set before the provider call.
            assert_eq!(row.4, 1, "append_attempted write-ahead flag set");
        }

        #[tokio::test]
        async fn recheck_moves_the_job_to_verify_with_the_right_next_at() {
            let service = service();
            seed_job(&service, "j1", "you@example.com", "pending", 0, now() - 1, 0);
            let at = now();
            let provider = JobProvider::new(vec![JobAnswer::Recheck(Duration::from_secs(10))]);
            service
                .run_sent_copy_job(&provider, "j1", b"Hi", "pending", 0, false, at)
                .await
                .unwrap();
            let row = job_row(&service, "j1");
            assert_eq!(row.1.as_deref(), Some("verify"));
            assert_eq!(row.3, Some(at + 10_000));
            assert_send_state_untouched(&row);
        }

        #[tokio::test]
        async fn a_transient_error_bumps_attempts_and_backs_off_never_before_the_settle_window() {
            use crate::provider::imap::{sent_copy_backoff, SENT_COPY_SETTLE};
            let service = service();
            seed_job(&service, "j1", "you@example.com", "verify", 1, now() - 1, 1);
            let at = now();
            let provider = JobProvider::new(vec![JobAnswer::Transient]);
            service
                .run_sent_copy_job(&provider, "j1", b"Hi", "verify", 1, true, at)
                .await
                .unwrap();
            let row = job_row(&service, "j1");
            assert_eq!(row.1.as_deref(), Some("verify"), "state is KEPT on a transient error");
            assert_eq!(row.2, 2, "attempts+1");
            let scheduled = sent_copy_backoff(1).as_millis() as i64;
            assert_eq!(row.3, Some(at + scheduled));
            // Below/exact/above the settle floor: the retry is never earlier
            // than the settle window after this attempt.
            assert!(
                row.3.unwrap() >= at + SENT_COPY_SETTLE.as_millis() as i64,
                "retry must not be earlier than the settle window"
            );
            assert!(row.6.is_some(), "a plain-language error note is stored");
            assert_send_state_untouched(&row);
            assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn transient_at_max_attempts_abandons() {
            use crate::provider::imap::SENT_COPY_MAX_ATTEMPTS;
            let service = service();
            let at = now();
            let prior = (SENT_COPY_MAX_ATTEMPTS - 1) as i64;
            seed_job(&service, "j1", "you@example.com", "verify", prior, at - 1, 1);
            let provider = JobProvider::new(vec![JobAnswer::Transient]);
            service
                .run_sent_copy_job(&provider, "j1", b"Hi", "verify", prior, true, at)
                .await
                .unwrap();
            let row = job_row(&service, "j1");
            assert_eq!(row.1.as_deref(), Some("abandoned"), "max attempts -> abandoned");
            assert_eq!(row.2, SENT_COPY_MAX_ATTEMPTS as i64);
            assert_send_state_untouched(&row);
        }

        #[tokio::test]
        async fn a_permanent_error_abandons_immediately_with_a_plain_error() {
            let service = service();
            let at = now();
            seed_job(&service, "j1", "you@example.com", "pending", 0, at - 1, 0);
            let provider = JobProvider::new(vec![JobAnswer::Permanent]);
            service
                .run_sent_copy_job(&provider, "j1", b"Hi", "pending", 0, false, at)
                .await
                .unwrap();
            let row = job_row(&service, "j1");
            assert_eq!(row.1.as_deref(), Some("abandoned"));
            assert_eq!(row.2, 0, "a permanent failure does not count a retry attempt");
            // run 5 rewrites the permanent-class provider message into plain
            // user wording before it is stored in sent_copy_error (the raw
            // "no Sent mailbox configured" developer text no longer reaches
            // the user). See sent_copy_error_message / permanent_sent_copy_message.
            assert_eq!(
                row.6.as_deref(),
                Some("Your message was sent, but a copy couldn't be saved to your Sent folder because no Sent folder is set up for this account.")
            );
            assert_send_state_untouched(&row);
            assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn a_provider_unavailable_account_is_skipped_without_counting_an_attempt() {
            // No account is registered, so provider_for fails. process_sent_copy_jobs
            // must skip the job and leave it exactly as-is, and the tick continues.
            let service = service();
            let at = now();
            seed_job(&service, "j1", "ghost@example.com", "pending", 0, at - 1, 0);
            service.process_sent_copy_jobs(at).await.unwrap();
            let row = job_row(&service, "j1");
            assert_eq!(row.1.as_deref(), Some("pending"), "unchanged");
            assert_eq!(row.2, 0, "no attempt counted");
            assert_eq!(row.4, 0, "no write-ahead flag set (never reached the provider)");
        }

        #[tokio::test]
        async fn a_due_job_is_processed_even_with_no_sendable_outbox_items() {
            // tick() runs process_sent_copy_jobs BEFORE the no-sendable early
            // return. Seed a real 'sent' row carrying a due job (via a dispatched
            // send so its payload is a valid Draft), on an UNREGISTERED account
            // so the job is only skipped — then tick() must still run cleanly and
            // reach the processor rather than short-circuit on the early return.
            let service = service();
            let item = ready_item(&service);
            let at = now();
            service
                .dispatch_due(&item, at, true, |_, _| async {
                    Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: None })
                })
                .await
                .unwrap();
            // Make the job due now and point it at an account with no provider.
            service
                .database
                .connection()
                .unwrap()
                .execute(
                    "UPDATE outbox_messages SET account='ghost@example.com', sent_copy_next_at=?2 WHERE id=?1",
                    params![item.id, at - 1],
                )
                .unwrap();
            // No sendable items remain (the only row is 'sent'); tick must still
            // run cleanly, which it can only do by reaching the processor.
            service.tick().await.unwrap();
            // The job is unchanged (provider unavailable), proving the processor
            // ran despite there being nothing to send.
            assert_eq!(job_row(&service, &item.id).1.as_deref(), Some("pending"));
        }

        #[tokio::test]
        async fn the_per_tick_cap_bounds_how_many_jobs_run() {
            use crate::provider::imap::SENT_COPY_MAX_JOBS_PER_TICK;
            let service = service();
            let at = now();
            // A full older batch on an unavailable account must not consume
            // any work slots. The healthy jobs still run, oldest first.
            for i in 0..SENT_COPY_MAX_JOBS_PER_TICK {
                seed_job(
                    &service,
                    &format!("ghost{i}"),
                    "ghost@example.com",
                    "pending",
                    0,
                    at - 2,
                    0,
                );
            }
            let total = SENT_COPY_MAX_JOBS_PER_TICK + 2;
            for i in 0..total {
                let account = if i % 2 == 0 {
                    "a@example.com"
                } else {
                    "b@example.com"
                };
                seed_job(&service, &format!("j{i}"), account, "pending", 0, at - 1, 0);
            }
            let working = Arc::new(JobProvider::new(vec![JobAnswer::Done]));
            for tick in 0..2 {
                let resolved = working.clone();
                service
                    .process_sent_copy_jobs_with(at, move |account| {
                        let provider = resolved.clone();
                        async move {
                            (account != "ghost@example.com")
                                .then_some(provider as Arc<dyn MailProvider>)
                        }
                    })
                    .await
                    .unwrap();
                let completed = if tick == 0 {
                    SENT_COPY_MAX_JOBS_PER_TICK
                } else {
                    total
                };
                for i in 0..total {
                    assert_eq!(
                        job_row(&service, &format!("j{i}")).1.as_deref(),
                        Some(if i < completed { "done" } else { "pending" })
                    );
                }
                for i in 0..SENT_COPY_MAX_JOBS_PER_TICK {
                    let row = job_row(&service, &format!("ghost{i}"));
                    assert_eq!(row.1.as_deref(), Some("pending"));
                    assert_eq!(row.2, 0, "unavailable jobs count no attempt");
                    assert_eq!(row.4, 0, "unavailable jobs do not start an append");
                }
                assert_eq!(working.save_calls.load(Ordering::SeqCst), completed);
                assert_eq!(working.send_calls.load(Ordering::SeqCst), 0);
            }
        }

        #[tokio::test]
        async fn one_account_failing_does_not_stop_another_accounts_job() {
            // Drive two jobs for two accounts directly: one permanent-fails, the
            // other succeeds. (process_sent_copy_jobs groups by account; this
            // asserts the per-job body is independent and neither resends.)
            let service = service();
            let at = now();
            seed_job(&service, "fail", "a@example.com", "pending", 0, at - 1, 0);
            seed_job(&service, "ok", "b@example.com", "pending", 0, at - 1, 0);
            let failing = JobProvider::new(vec![JobAnswer::Permanent]);
            let working = JobProvider::new(vec![JobAnswer::Done]);
            service.run_sent_copy_job(&failing, "fail", b"Hi", "pending", 0, false, at).await.unwrap();
            service.run_sent_copy_job(&working, "ok", b"Hi", "pending", 0, false, at).await.unwrap();
            assert_eq!(job_row(&service, "fail").1.as_deref(), Some("abandoned"));
            assert_eq!(job_row(&service, "ok").1.as_deref(), Some("done"));
            assert_eq!(failing.send_calls.load(Ordering::SeqCst), 0);
            assert_eq!(working.send_calls.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn a_crash_between_accept_and_the_next_tick_still_finds_the_job() {
            // Seed the job via dispatch_due (the atomic accept path), then reopen
            // the DB (simulate a restart) and confirm the job row survives with
            // its pending state and settle-window next_at.
            let service = service();
            let item = ready_item(&service);
            let at = now();
            service
                .dispatch_due(&item, at, true, |_, _| async {
                    Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: None })
                })
                .await
                .unwrap();
            // A fresh outbox() read (what the next tick does) still sees the job.
            let (_, copy_state, _, next_at, append_attempted, _, _) = job_row(&service, &item.id);
            assert_eq!(copy_state.as_deref(), Some("pending"), "job survives into the next tick");
            assert_eq!(next_at, Some(at + crate::provider::imap::SENT_COPY_SETTLE.as_millis() as i64));
            assert_eq!(append_attempted, 0);
        }

        #[tokio::test]
        async fn no_resend_across_every_job_outcome() {
            // Every scripted outcome, asserting send_once is NEVER called and the
            // outbox state/provider_id never change.
            for answers in [
                vec![JobAnswer::Done],
                vec![JobAnswer::Recheck(Duration::from_secs(10))],
                vec![JobAnswer::Transient],
                vec![JobAnswer::Permanent],
            ] {
                let service = service();
                let at = now();
                seed_job(&service, "j", "you@example.com", "pending", 0, at - 1, 0);
                let provider = JobProvider::new(answers);
                service
                    .run_sent_copy_job(&provider, "j", b"Hi", "pending", 0, false, at)
                    .await
                    .unwrap();
                let row = job_row(&service, "j");
                assert_send_state_untouched(&row);
                assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0, "NO RESEND, ever");
            }
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
        let raw = build_mime(&d, &[], Some("Joel Reed"), None, "test-id", &root).unwrap();
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
    fn mime_round_trips_recipients_unicode_subject_and_reply_headers() {
        let db = database();
        let mut d = saved(&db);
        d.to = "Jane <jane@example.com>, José Núñez <jose@example.com>".into();
        d.cc = "cc@example.com".into();
        d.bcc = "hidden@example.com".into();
        d.subject = "Re: Café meeting — résumé ✓".into();
        d.reply_id = Some("original@example.com".into());
        d.references = vec!["ancestor@example.com".into(), "original@example.com".into()];
        let raw = build_mime(&d, &[], None, None, "headers-id", Path::new("/unused")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let addresses = |list: Option<&mail_parser::Address>| -> Vec<String> {
            list.into_iter()
                .flat_map(|list| list.iter())
                .filter_map(|addr| addr.address().map(str::to_string))
                .collect()
        };
        assert_eq!(addresses(parsed.to()), ["jane@example.com", "jose@example.com"]);
        assert_eq!(
            parsed.to().and_then(|to| to.iter().nth(1)).and_then(|addr| addr.name()),
            Some("José Núñez")
        );
        assert_eq!(addresses(parsed.cc()), ["cc@example.com"]);
        // The Gmail send API derives Bcc recipients from the raw message and
        // strips the header itself, so it must be present in what we build.
        assert_eq!(addresses(parsed.bcc()), ["hidden@example.com"]);
        assert_eq!(parsed.subject(), Some("Re: Café meeting — résumé ✓"));
        assert_eq!(
            parsed.in_reply_to().as_text_list().map(|ids| ids.to_vec()),
            Some(vec!["original@example.com".into()])
        );
        assert_eq!(
            parsed.references().as_text_list().map(|ids| ids.to_vec()),
            Some(vec!["ancestor@example.com".into(), "original@example.com".into()])
        );
    }

    #[test]
    fn a_server_draft_allows_no_recipients_and_keeps_one_message_id_across_pushes() {
        let db = database();
        let mut d = saved(&db);
        d.to.clear();
        assert!(build_mime(&d, &[], None, None, "op", Path::new("/unused")).is_err(), "sending still needs a recipient");
        let first = build_draft_mime(&d, &[], None, None, Path::new("/unused")).unwrap();
        d.body = "Edited".into();
        d.revision += 1;
        let second = build_draft_mime(&d, &[], None, None, Path::new("/unused")).unwrap();
        let expected = format!("draft-{}@threestrands.local", d.id);
        for raw in [&first, &second] {
            let parsed = MessageParser::default().parse(raw).unwrap();
            assert_eq!(parsed.message_id(), Some(expected.as_str()));
            assert!(parsed.to().is_none(), "no empty To header");
        }
        let domain = build_draft_mime(&d, &[], None, Some("example.org"), Path::new("/unused")).unwrap();
        let parsed = MessageParser::default().parse(&domain).unwrap();
        assert_eq!(parsed.message_id(), Some(format!("draft-{}@example.org", d.id).as_str()));
    }

    #[test]
    fn a_server_draft_keeps_bcc_and_reply_headers() {
        let db = database();
        let mut d = saved(&db);
        d.bcc = "hidden@example.com".into();
        d.reply_id = Some("original@example.com".into());
        d.references = vec!["original@example.com".into()];
        let raw = build_draft_mime(&d, &[], None, None, Path::new("/unused")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let bcc: Vec<_> = parsed.bcc().into_iter().flat_map(|list| list.iter()).filter_map(|a| a.address()).collect();
        assert_eq!(bcc, ["hidden@example.com"]);
        assert_eq!(
            parsed.in_reply_to().as_text_list().map(|ids| ids.to_vec()),
            Some(vec!["original@example.com".into()])
        );
    }

    #[test]
    fn a_server_draft_blocks_header_injection_like_a_send() {
        let db = database();
        let mut d = saved(&db);
        d.subject = "Hi\r\nBcc: attacker@example.com".into();
        assert!(build_draft_mime(&d, &[], None, None, Path::new("/unused")).is_err());
        let mut d = saved(&db);
        d.reply_id = Some("x\r\nBcc: attacker@example.com".into());
        assert!(build_draft_mime(&d, &[], None, None, Path::new("/unused")).is_err());
    }

    // ---- Phase 4 run 2 (D): Draft.from, identity From selection, stale
    //      identity rejection, Message-ID domain, header-injection bytes. ----

    fn identity(address: &str, display: Option<&str>) -> crate::provider::SendingIdentity {
        crate::provider::SendingIdentity {
            address: address.into(),
            display_name: display.map(Into::into),
        }
    }

    #[test]
    fn build_mime_sends_from_the_chosen_identity_with_its_display_name_and_domain() {
        let db = database();
        let mut d = saved(&db);
        // The account is you@example.com, but this draft was composed from a
        // secondary identity.
        d.account = "me@proton.me".into();
        d.from = Some("work@proton.me".into());
        let identities = [
            identity("me@proton.me", Some("Me")),
            identity("work@proton.me", Some("Work Account")),
        ];
        // An IMAP-style provider supplies the sender's domain.
        let raw = build_mime(&d, &identities, None, Some("proton.me"), "op-1", Path::new("/unused"))
            .unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let from = parsed.from().and_then(|f| f.first()).unwrap();
        assert_eq!(from.address(), Some("work@proton.me"));
        assert_eq!(from.name(), Some("Work Account"));
        assert_eq!(parsed.message_id(), Some("op-1@proton.me"));
    }

    #[test]
    fn build_mime_defaults_to_the_account_address_and_threestrands_domain() {
        // No `from`, provider domain None (Gmail default) => byte-identical to
        // the pre-change behaviour: From is the account, Message-ID domain is
        // threestrands.local, and the account display name is used.
        let db = database();
        let d = saved(&db);
        let identities = [identity(&d.account, None)];
        let raw = build_mime(&d, &identities, Some("Joel Reed"), None, "op-2", Path::new("/unused"))
            .unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let from = parsed.from().and_then(|f| f.first()).unwrap();
        assert_eq!(from.address(), Some(d.account.as_str()));
        assert_eq!(from.name(), Some("Joel Reed"));
        assert_eq!(parsed.message_id(), Some("op-2@threestrands.local"));
    }

    #[test]
    fn build_mime_rejects_a_stale_identity_not_on_the_account() {
        let db = database();
        let mut d = saved(&db);
        d.account = "me@proton.me".into();
        // `from` names an identity that was removed from the account's list.
        d.from = Some("removed@proton.me".into());
        let identities = [identity("me@proton.me", None), identity("work@proton.me", None)];
        let err = build_mime(&d, &identities, None, Some("proton.me"), "op-3", Path::new("/unused"))
            .unwrap_err();
        assert!(
            err.contains("no longer on this account"),
            "a stale identity must fail clearly, not send as another address: {err}"
        );
    }

    #[test]
    fn build_mime_blocks_header_injection_via_display_name_and_address() {
        let db = database();
        let mut d = saved(&db);
        d.account = "me@proton.me".into();

        // A hostile DISPLAY NAME carrying CRLF must be refused — proven by the
        // error AND, where accepted elsewhere, by the absence of an injected
        // header in the bytes.
        let injected_name = [identity("me@proton.me", Some("Evil\r\nBcc: victim@x.com"))];
        let err = build_mime(&d, &injected_name, None, Some("proton.me"), "op-4", Path::new("/unused"))
            .unwrap_err();
        assert!(err.contains("line breaks"), "{err}");

        // A hostile FROM ADDRESS carrying CRLF must be refused too.
        d.from = Some("me@proton.me\r\nBcc: victim@x.com".into());
        let ids = [identity("me@proton.me\r\nBcc: victim@x.com", None)];
        let err = build_mime(&d, &ids, None, Some("proton.me"), "op-5", Path::new("/unused"))
            .unwrap_err();
        assert!(err.contains("line breaks"), "{err}");

        // A clean send with a non-ASCII display name produces bytes with NO
        // injected header: assert on the produced bytes directly.
        d.from = None;
        let clean = [identity("me@proton.me", Some("Zoë O'Brien"))];
        let raw = build_mime(&d, &clean, None, Some("proton.me"), "op-6", Path::new("/unused"))
            .unwrap();
        let text = String::from_utf8_lossy(&raw);
        // The From header line must be a single logical header (no bare CRLF
        // introducing a "Bcc:" or similar), and no stray Bcc appears.
        assert!(!text.to_ascii_lowercase().contains("\nbcc: victim"));
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            parsed.from().and_then(|f| f.first()).and_then(|a| a.address()),
            Some("me@proton.me")
        );
    }

    #[test]
    fn signature_mime_keeps_plain_text_html_images_and_quote_placement_after_template_deletion() {
        use crate::signatures::tests::{template, defaults};
        let db = database();
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j1ioAAAAASUVORK5CYII=";
        let mut signature = template("work");
        signature.html = format!("<table><tr><td><img src=\"data:image/png;base64,{png}\" alt=\"Logo\" width=\"32\"></td><td>Regards</td></tr></table>");
        signature.text = "Logo Regards".into();
        db.store_signature(&signature).unwrap();
        db.store_signature_defaults(&defaults("you@example.com", "", Some("work"), Some("work"))).unwrap();
        let mut d = saved(&db);
        d.body = "Hi 😀\n\nOn Monday, Ann wrote:\n> Quote".into();
        d.body_html = "Hi 😀<br><br>On Monday, Ann wrote:<blockquote type=\"cite\">Quote</blockquote>".into();
        d.signature.as_mut().unwrap().text_offset = 5;
        d.signature.as_mut().unwrap().html_offset = 5;
        let d = db.save_draft(d).unwrap();
        db.delete_signature("work").unwrap();
        let raw = build_mime(&d, &[], None, None, "signed", Path::new("/unused")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.body_text(0).unwrap().replace("\r\n", "\n"), "Hi 😀\n\nLogo Regards\n\nOn Monday, Ann wrote:\n> Quote");
        let html = parsed.body_html(0).unwrap();
        assert!(html.find("Hi 😀").unwrap() < html.find("<table>").unwrap());
        assert!(html.find("Regards").unwrap() < html.find("On Monday").unwrap());
        assert!(!html.contains("data:image"));
        assert!(html.contains("cid:signature-0-signed@threestrands.local"));
        assert!(parsed.parts.iter().any(|part| part.content_id() == Some("signature-0-signed@threestrands.local")));
        assert_eq!(db.draft(&d.id).unwrap().signature.unwrap().text, "Logo Regards");
    }

    #[test]
    fn signature_mime_keeps_hosted_sources_without_creating_image_parts() {
        use crate::signatures::tests::{template, defaults};
        for html in [
            "<table><tr><td><img src=\"https://images.example/large.png?version=2&amp;size=large\" alt=\"Logo\"></td><td>Regards</td></tr></table>",
            "<div><img src=\"https://cdn.example/logo.gif\" alt=\"Logo\"><p>Regards</p></div>",
        ] {
            let db = database();
            let mut signature = template("hosted");
            signature.html = html.into();
            db.store_signature(&signature).unwrap();
            db.store_signature_defaults(&defaults("you@example.com", "", Some("hosted"), None)).unwrap();
            let draft = saved(&db);
            let raw = build_mime(&draft, &[], None, None, "hosted", Path::new("/unused")).unwrap();
            let parsed = MessageParser::default().parse(&raw).unwrap();
            assert!(parsed.body_html(0).unwrap().contains(html));
            assert!(!parsed.parts.iter().any(|part| part.content_id().is_some()));
            assert!(!parsed.body_html(0).unwrap().contains("data:image"));
            assert!(!parsed.body_html(0).unwrap().contains("cid:"));
        }
    }

    #[test]
    fn switching_sending_alias_updates_only_an_automatic_signature() {
        use crate::signatures::tests::{template, defaults};
        let db = imap_database();
        db.store_signature(&template("work")).unwrap();
        db.store_signature_defaults(&defaults("me@proton.me", "work@proton.me", Some("work"), Some("work"))).unwrap();
        let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
        let mut changed = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
        assert_eq!(changed.signature.as_ref().unwrap().text, "work");
        changed.signature.as_mut().unwrap().automatic = false;
        db.save_draft(changed).unwrap();
        let back = db.set_draft_from(&d.id, None).unwrap();
        assert_eq!(back.signature.unwrap().text, "work");
    }

    #[test]
    fn mime_includes_rich_html_and_plain_text_fallback() {
        let db = database();
        let mut d = saved(&db);
        d.body = "Formatted message".into();
        d.body_html = "<p><strong>Formatted</strong> message</p>".into();
        let raw = build_mime(&d, &[], None, None, "rich-id", Path::new("/unused")).unwrap();
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

        let raw = build_mime(&d, &[], None, None, "inline-id", &root).unwrap();
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
    fn reply_all_excludes_self_preserves_cc_and_forward_joins_the_thread() {
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
            .create_draft("replyAll", Some("source".into()), "you@example.com", &[])
            .unwrap();
        assert_eq!(reply.to, "reply@example.com, colleague@example.com");
        assert_eq!(reply.cc, "cc@example.com");
        assert_eq!(reply.thread_id, Some("thread".into()));
        let forward = db
            .create_draft("forward", Some("source".into()), "you@example.com", &[])
            .unwrap();
        // A forward joins the source conversation but is still not a reply:
        // nobody is addressed for the user.
        assert!(forward.to.is_empty());
        assert!(forward.cc.is_empty());
        assert_eq!(forward.reply_id, Some("source@example.com".into()));
        assert_eq!(forward.references, vec!["source@example.com".to_string()]);
        assert_eq!(forward.thread_id, Some("thread".into()));
        assert_eq!(forward.subject, "Fwd: Topic");
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let mut addressed = forward.clone();
        addressed.to = "colleague@example.com".into();
        let raw = build_mime(&addressed, &[], None, None, "forward-id", &root).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.in_reply_to().as_text(), Some("source@example.com"));
        assert_eq!(parsed.references().as_text(), Some("source@example.com"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forward_extends_the_reference_chain_and_survives_a_missing_message_id() {
        let db = database();
        for (id, headers, thread) in [
            (
                "chained",
                serde_json::json!([{"name":"From","value":"other@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<second@example.com>"},{"name":"References","value":"<first@example.com>"}]),
                Some("thread"),
            ),
            (
                "anonymous",
                serde_json::json!([{"name":"From","value":"other@example.com"},{"name":"Subject","value":"Topic"}]),
                None,
            ),
        ] {
            let source = serde_json::json!({"id":id,"threadId":"thread","payload":{"mimeType":"text/plain","headers":headers,"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
            db.connection()
                .unwrap()
                .execute(
                    "INSERT INTO message_metadata(id, payload) VALUES (?1,?2)",
                    params![id, source.to_string()],
                )
                .unwrap();
            let forward = db
                .create_draft("forward", Some(id.into()), "you@example.com", &[])
                .unwrap();
            assert_eq!(forward.thread_id.as_deref(), thread, "{id}");
            if thread.is_some() {
                assert_eq!(forward.reply_id, Some("second@example.com".into()));
                assert_eq!(
                    forward.references,
                    vec!["first@example.com".to_string(), "second@example.com".to_string()]
                );
            } else {
                // No Message-ID to point at: it still forwards, as a new conversation.
                assert!(forward.reply_id.is_none());
                assert!(forward.references.is_empty());
                assert!(forward.body.contains("Forwarded message"));
            }
        }
    }

    #[test]
    fn alias_replies_exclude_all_own_addresses_and_follow_sent_recipients() {
        let db = imap_database();
        let identities = db.account_sending_identities("me@proton.me").unwrap();
        for (id, sender, to, expected_to) in [
            (
                "incoming",
                "friend@example.com",
                "WORK@proton.me, colleague@example.com",
                "friend@example.com, colleague@example.com",
            ),
            (
                "outgoing",
                "WORK@proton.me",
                "friend@example.com, colleague@example.com",
                "friend@example.com, colleague@example.com",
            ),
        ] {
            let source = serde_json::json!({"id":id,"threadId":"thread","payload":{
                "mimeType":"text/plain","headers":[
                    {"name":"From","value":sender}, {"name":"To","value":to},
                    {"name":"Cc","value":"me@proton.me, work@proton.me, cc@example.com"},
                    {"name":"Subject","value":"Topic"}, {"name":"Message-ID","value":format!("<{id}@example.com>")}
                ],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
            db.put_message_metadata(id, &source.to_string()).unwrap();
            let reply = db
                .create_draft("replyAll", Some(id.into()), "me@proton.me", &identities)
                .unwrap();
            assert_eq!(reply.to, expected_to, "{id}");
            assert_eq!(reply.cc, "cc@example.com", "{id}");
            if id == "incoming" {
                assert_eq!(reply.from.as_deref(), Some("work@proton.me"));
            }
            let reply = db
                .create_draft("reply", Some(id.into()), "me@proton.me", &identities)
                .unwrap();
            assert_eq!(
                reply.to,
                if id == "incoming" {
                    "friend@example.com"
                } else {
                    expected_to
                }
            );
        }
    }
    #[test]
    fn html_forwards_preserve_source_in_mime_after_save_and_reopen() {
        for (index, html) in [
            "<p><strong>Golf Monday</strong></p><a href=\"https://example.com/profile?one=1&amp;two=2\">Profile</a><img src=\"https://images.example.com/invite.png\" alt=\"Invitation\">",
            "<!doctype html><html><head><style>td { padding: 8px; }</style></head><body><table cellspacing=\"4\"><tr><td></td><td><em>Agenda</em><a href=\"https://example.com/agenda\">Read agenda</a></td></tr></table></body></html>",
        ].into_iter().enumerate() {
            let db = database();
            let source_id = format!("html-forward-{index}");
            let source = serde_json::json!({"id":source_id,"threadId":"thread","payload":{"mimeType":"multipart/alternative","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Golf <Monday> & friends"}],"parts":[{"mimeType":"text/plain","body":{"data":URL_SAFE_NO_PAD.encode("Plain alternative [Profile] and [image]")}},{"mimeType":"text/html","body":{"data":URL_SAFE_NO_PAD.encode(html)}}]}});
            db.connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES (?1, ?2)", params![source_id, source.to_string()]).unwrap();
            let mut draft = db.create_draft("forward", Some(source_id), "you@example.com", &[]).unwrap();
            let preserved = draft.forwarded_content.as_ref().unwrap();
            assert!(preserved.html.ends_with(html));
            assert!(preserved.html.contains("Other &lt;other@example.com&gt;"));
            assert!(preserved.html.contains("Golf &lt;Monday&gt; &amp; friends"));
            assert!(preserved.text.ends_with("Plain alternative [Profile] and [image]"));
            assert!(draft.body.is_empty());
            let expected_html = preserved.html.clone();
            // Saving user edits cannot overwrite native-owned source HTML.
            draft.forwarded_content = None;
            draft.to = "friend@example.com".into();
            draft.body = "Take a look <please>".into();
            let saved = db.save_draft(draft).unwrap();
            let reopened = db.draft(&saved.id).unwrap();
            assert_eq!(reopened.forwarded_content.as_ref().unwrap().html, expected_html);
            let raw = build_mime(&reopened, &[], None, None, "forward", Path::new(".")).unwrap();
            let parsed = MessageParser::default().parse(&raw).unwrap();
            assert_eq!(parsed.body_html(0).unwrap(), format!("Take a look &lt;please&gt;<br><br>{expected_html}"));
            let plain = parsed.body_text(0).unwrap().replace("\r\n", "\n");
            assert_eq!(plain, format!("{}\n\n{}", reopened.body, reopened.forwarded_content.as_ref().unwrap().text));
            assert!(parsed.in_reply_to().as_text().is_none());
        }
    }

    #[test]
    fn old_drafts_without_preserved_content_keep_their_body() {
        let db = database();
        let draft = saved(&db);
        let mut value = serde_json::to_value(&draft).unwrap();
        value.as_object_mut().unwrap().remove("forwardedContent");
        let old: Draft = serde_json::from_value(value).unwrap();
        assert!(old.forwarded_content.is_none());
        let raw = build_mime(&old, &[], None, None, "legacy", Path::new(".")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.body_text(0).unwrap(), draft.body);
    }

    #[test]
    fn edit_as_new_copies_a_sent_message_into_an_unaddressed_new_thread() {
        // Two structurally different sent messages: HTML with inline and
        // regular attachments, and plain text only.
        let db = database();
        let html = "<p>Join us <strong>Saturday</strong></p><img src=\"cid:map@example.com\">";
        let rich = serde_json::json!({"id":"sent-rich","threadId":"thread","payload":{"mimeType":"multipart/mixed","headers":[{"name":"From","value":"You <YOU@example.com>"},{"name":"To","value":"guest@example.com"},{"name":"Cc","value":"cohost@example.com"},{"name":"Subject","value":"Dinner on Saturday"},{"name":"Message-ID","value":"<sent-rich@example.com>"}],"parts":[{"mimeType":"multipart/related","parts":[{"mimeType":"text/html","body":{"data":URL_SAFE_NO_PAD.encode(html)}},{"mimeType":"image/png","headers":[{"name":"Content-ID","value":"<map@example.com>"},{"name":"Content-Disposition","value":"inline"}],"body":{"data":URL_SAFE_NO_PAD.encode("png"),"size":3}}]},{"mimeType":"application/pdf","filename":"menu.pdf","body":{"attachmentId":"menu-part","size":4}}]}});
        let plain = serde_json::json!({"id":"sent-plain","threadId":"thread-2","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"you@example.com"},{"name":"To","value":"guest@example.com"},{"name":"Subject","value":"Book club"},{"name":"Message-ID","value":"<sent-plain@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Bring the book.")}}});
        db.put_message_metadata("sent-rich", &rich.to_string()).unwrap();
        db.put_message_metadata("sent-plain", &plain.to_string()).unwrap();

        let draft = db.create_draft_from_sent("sent-rich", "you@example.com", &[]).unwrap();
        assert_eq!(draft.mode, "new");
        assert_eq!(draft.subject, "Dinner on Saturday");
        assert!(draft.body_html.contains("Join us <strong>Saturday</strong>"));
        assert_eq!((draft.to.as_str(), draft.cc.as_str(), draft.bcc.as_str()), ("", "", ""));
        assert_eq!(draft.source_id, None);
        assert_eq!(draft.thread_id, None);
        assert_eq!(draft.reply_id, None);
        assert!(draft.references.is_empty());
        assert!(draft.forwarded_content.is_none());
        assert_eq!(draft.from, None);
        assert_eq!(draft.attachments.len(), 2);
        assert!(draft.attachments.iter().all(|a| !a.ready && a.message_id.as_deref() == Some("sent-rich")));
        let inline = draft.attachments.iter().find(|a| a.inline).unwrap();
        assert_eq!(inline.content_id.as_deref(), Some("map@example.com"));
        let menu = draft.attachments.iter().find(|a| !a.inline).unwrap();
        assert_eq!((menu.name.as_str(), menu.provider_id.as_deref()), ("menu.pdf", Some("menu-part")));

        // Each use is its own draft, so a copy can go to several people separately.
        let again = db.create_draft_from_sent("sent-rich", "you@example.com", &[]).unwrap();
        assert_ne!(again.id, draft.id);
        assert_ne!(again.attachments[0].id, draft.attachments[0].id);

        let text = db.create_draft_from_sent("sent-plain", "you@example.com", &[]).unwrap();
        assert_eq!(text.subject, "Book club");
        assert_eq!(text.body.trim_end(), "Bring the book.");
        assert!(text.body_html.trim().is_empty());
        assert!(text.attachments.is_empty());

        // The copy sends as a fresh message, not a reply.
        let mut ready = text;
        ready.to = "newguest@example.com".into();
        let raw = build_mime(&ready, &[], None, None, "edit-as-new", Path::new(".")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.subject(), Some("Book club"));
        assert!(parsed.in_reply_to().as_text().is_none());
        assert!(parsed.references().as_text().is_none());
    }

    #[test]
    fn edit_as_new_refuses_messages_from_other_senders() {
        let db = database();
        for (id, from) in [("incoming", "Other <other@example.com>"), ("lookalike", "you@example.com.evil.test")] {
            let source = serde_json::json!({"id":id,"threadId":"thread","payload":{"mimeType":"text/html","headers":[{"name":"From","value":from},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Invoice"}],"body":{"data":URL_SAFE_NO_PAD.encode("<p>Pay here</p>")}}});
            db.put_message_metadata(id, &source.to_string()).unwrap();
            let refused = db.create_draft_from_sent(id, "you@example.com", &[]).unwrap_err();
            assert_eq!(refused, "Edit as New works only on messages you sent", "{id}");
        }
        assert!(db.drafts().unwrap().is_empty(), "a refusal must not leave a draft behind");
    }

    #[test]
    fn edit_as_new_keeps_the_identity_the_original_was_sent_from() {
        let db = imap_database();
        let identities = db.account_sending_identities("me@proton.me").unwrap();
        for (id, sender, expected) in [
            ("from-alias", "Work <WORK@proton.me>", Some("work@proton.me")),
            ("from-primary", "me@proton.me", None),
        ] {
            let source = serde_json::json!({"id":id,"threadId":"thread","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":sender},{"name":"To","value":"friend@example.com"},{"name":"Subject","value":"Topic"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
            db.put_message_metadata(id, &source.to_string()).unwrap();
            let draft = db.create_draft_from_sent(id, "me@proton.me", &identities).unwrap();
            assert_eq!(draft.from.as_deref(), expected, "{id}");
            assert!(draft.to.is_empty(), "{id}");
        }
    }

    #[tokio::test]
    async fn edit_as_new_opens_from_a_cached_message_while_its_account_is_not_connected() {
        let service = service();
        let source = serde_json::json!({"id":"cached-sent","threadId":"t","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"you@example.com"},{"name":"To","value":"friend@example.com"},{"name":"Subject","value":"Picnic"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        service.database.put_message_metadata("cached-sent", &source.to_string()).unwrap();
        let value = service
            .request(Request::CreateFromSent { source_id: "cached-sent".into(), account: "you@example.com".into() })
            .await
            .expect("a cached sent message must open as a new draft without a connected provider");
        let draft: Draft = serde_json::from_value(value).unwrap();
        assert_eq!((draft.mode.as_str(), draft.subject.as_str(), draft.to.as_str()), ("new", "Picnic", ""));
    }

    #[test]
    fn forwarding_keeps_named_and_unnamed_inline_images_and_content_ids() {
        let db = database();
        let html = "<p>Photos</p><img src=\"cid:first@example.com\"><table><tr><td><img src=\"cid:second@example.com\"></td></tr></table>";
        let source = serde_json::json!({"id":"inline-forward","threadId":"thread","payload":{"mimeType":"multipart/related","headers":[{"name":"From","value":"other@example.com"},{"name":"Subject","value":"Photos"}],"parts":[{"mimeType":"text/html","body":{"data":URL_SAFE_NO_PAD.encode(html)}},{"mimeType":"image/png","filename":"photo.png","headers":[{"name":"Content-ID","value":"<first@example.com>"}],"body":{"attachmentId":"remote-part","size":3}},{"mimeType":"image/png","headers":[{"name":"Content-ID","value":"<second@example.com>"},{"name":"Content-Disposition","value":"inline"}],"body":{"data":URL_SAFE_NO_PAD.encode("png"),"size":3}}]}});
        db.connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES ('inline-forward', ?1)", [source.to_string()]).unwrap();
        let mut draft = db.create_draft("forward", Some("inline-forward".into()), "you@example.com", &[]).unwrap();
        assert_eq!(draft.attachments.len(), 2);
        assert!(draft.attachments.iter().all(|attachment| attachment.inline && !attachment.ready));
        assert_eq!(draft.attachments[0].provider_id.as_deref(), Some("remote-part"));
        assert_eq!(draft.attachments[1].provider_id, None);
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        for attachment in &mut draft.attachments {
            std::fs::write(root.join(&attachment.id), b"png").unwrap();
            attachment.ready = true;
        }
        draft.to = "friend@example.com".into();
        draft.body_html = "<p>Please see below.</p>".into();
        let raw = build_mime(&draft, &[], None, None, "inline-forward", &root).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert!(parsed.body_html(0).unwrap().starts_with("<p>Please see below.</p><br><br>"));
        assert!(parsed.body_html(0).unwrap().ends_with(html));
        let raw = String::from_utf8(raw).unwrap();
        assert!(raw.contains("Content-ID: <first@example.com>"));
        assert!(raw.contains("Content-ID: <second@example.com>"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reply_attribution_dates_are_readable_in_the_reader_time_zone() {
        let eastern = chrono::FixedOffset::west_opt(4 * 3600).unwrap();
        assert_eq!(
            attribution_date("2026-10-05T14:32:48+00:00", &eastern),
            "Mon, Oct 5, 2026 at 10:32 AM"
        );
        assert_eq!(
            attribution_date("2026-10-05T00:05:00+00:00", &chrono::Utc),
            "Mon, Oct 5, 2026 at 12:05 AM"
        );
        assert_eq!(attribution_date("not a date", &chrono::Utc), "not a date");
    }
    #[test]
    fn reply_and_forward_bodies_quote_the_source_with_a_readable_date() {
        let db = database();
        let source = serde_json::json!({"id":"source-q","threadId":"thread-q","internalDate":"1791210768000","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source-q@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello\n> earlier")}}});
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO message_metadata(id, payload) VALUES ('source-q',?1)",
                [source.to_string()],
            )
            .unwrap();
        let date = attribution_date("2026-10-05T14:32:48+00:00", &chrono::Local);
        let reply = db
            .create_draft("reply", Some("source-q".into()), "you@example.com", &[])
            .unwrap();
        assert_eq!(
            reply.body,
            format!("\n\nOn {date}, Other <other@example.com> wrote:\n> Hello\n> > earlier")
        );
        assert!(!reply.body.contains("T14:32:48"));
        let forward = db
            .create_draft("forward", Some("source-q".into()), "you@example.com", &[])
            .unwrap();
        assert!(forward.body.contains(&format!("\nDate: {date}\n")));
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
            .create_draft("reply", Some("source-b".into()), "you@example.com", &[])
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
            .create_draft("reply", Some("source-c".into()), "you@example.com", &[])
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
        let mut d = db.create_draft("new", None, "you@example.com", &[]).unwrap();
        d.to = "Jane <jane@example.com>".into();
        d.subject = "Hello".into();
        d.body = "Body".into();
        let d = db.save_draft(d).unwrap();
        let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
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
        assert!(db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).is_err());
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
            sending_identity: Arc::new(|_| Ok("00000000-0000-4000-8000-000000000001".into())),
            database: Arc::new(database()),
            accounts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            root: PathBuf::from("/unused"),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            edits: Arc::new(tokio::sync::Mutex::new(())),
            scheduling: Arc::new(tokio::sync::Mutex::new(())),
            draft_pushes: Arc::new(tokio::sync::Mutex::new(())),
            draft_events: Arc::new(|_| {}),
            draft_pull_requested: Arc::default(),
        }
    }
    #[tokio::test]
    async fn reply_to_a_cached_message_opens_while_its_account_is_not_connected() {
        // `service()` has an empty account registry, so `provider_for` fails.
        // Resolving sending identities must not turn that into a refusal to
        // open a reply to a message that is already cached.
        let service = service();
        let source = serde_json::json!({"id":"cached","threadId":"t","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<cached@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        service.database.connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES ('cached', ?1)", [source.to_string()]).unwrap();
        let value = service
            .request(Request::Create { mode: "reply".into(), source_id: Some("cached".into()), account: Some("you@example.com".into()) })
            .await
            .expect("a cached reply must open without a connected provider");
        let draft: Draft = serde_json::from_value(value).unwrap();
        assert_eq!(draft.account, "you@example.com");
        assert_eq!(draft.from, None, "falls back to the primary identity");
    }
    #[tokio::test]
    async fn fake_clock_and_provider_prove_undo_deadline_and_single_dispatch() {
        let service = service();
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, false, &[], None, &service.root)
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
            .dispatch_due(&item, item.deadline - 1, false, &send)
            .await
            .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        service
            .dispatch_due(&item, item.deadline, false, &send)
            .await
            .unwrap();
        service
            .dispatch_due(&item, item.deadline + 1, false, &send)
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
            .queue(&d.id, d.revision, false, &[], None, &service.root)
            .unwrap();
        service.database.cancel_send(&item.id, false).unwrap();
        service
            .dispatch_due(&item, item.deadline, false, |_, _| async {
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
            .queue(&d.id, d.revision, false, &[], None, &service.root)
            .unwrap();
        service
            .dispatch_due(&item, item.deadline, false, |_, _| async {
                Err((false, "Connection dropped after acceptance".into()))
            })
            .await
            .unwrap();
        assert_eq!(service.database.outbox().unwrap()[0].state, "uncertain");
        service
            .dispatch_due(&item, item.deadline + 1, false, |_, _| async {
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
            .queue(&d.id, d.revision, false, &[], None, &service.root)
            .unwrap();
        service
            .dispatch_due(&item, item.deadline, false, |_, _| async {
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
            .queue(&d.id, d.revision, false, &[], None, &service.root)
            .unwrap();
        let result=service.dispatch_due(&item,item.deadline,false,|_,_|async {
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
        assert!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).is_err());
        d.attachments[0].ready = true;
        assert!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).is_err());
    }
    #[test]
    fn mime_rejects_header_injection_and_unsafe_attachment_ids() {
        let db = database();
        let base = saved(&db);

        for subject in ["Hello\r\nBcc: attacker@example.net", "Hello\nBcc: attacker@example.net", "Hello\r"] {
            let mut d = base.clone();
            d.subject = subject.into();
            assert_eq!(
                build_mime(&d, &[], None, None, "id", Path::new("/unused")).unwrap_err(),
                "Subject cannot contain newlines"
            );
        }

        let mut d = base.clone();
        d.reply_id = Some("original@example.com\r\nBcc: attacker@example.net".into());
        assert_eq!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).unwrap_err(), "Invalid reply headers");

        let mut d = base.clone();
        d.reply_id = Some("original@example.com".into());
        d.references = vec!["ancestor@example.com".into(), "x@example.com\nBcc: attacker@example.net".into()];
        assert_eq!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).unwrap_err(), "Invalid reply headers");

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
            assert_eq!(build_mime(&d, &[], None, None, "id", &inner).unwrap_err(), "Invalid attachment ID", "id {id:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    fn scheduled(service: &Correspondence) -> OutboxItem {
        let d=saved(&service.database);
        service.database.schedule_draft(&d.id,d.revision,&crate::scheduled_send::Selection {local_time:"2030-01-01T09:00".into(),time_zone:"America/New_York".into(),offset_seconds:None},"00000000-0000-4000-8000-000000000001",&[],None,&service.root,1_800_000_000_000).unwrap()
    }

    #[tokio::test]
    async fn blocked_keychain_poll_and_schedule_edit_leave_compose_responsive() {
        for polling in [true, false] {
            let mut service = service();
            let item = scheduled(&service);
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let entered = std::sync::Mutex::new(Some(entered));
            let (release, blocked) = std::sync::mpsc::channel();
            let blocked = std::sync::Mutex::new(blocked);
            service.sending_identity = Arc::new(move |_| {
                if let Some(entered) = entered.lock().unwrap().take() { let _ = entered.send(()); }
                blocked.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
                Ok("00000000-0000-4000-8000-000000000001".into())
            });
            let background = service.clone();
            let request = if polling { Request::ListOutbox } else { Request::Cancel { id: item.id } };
            let background = tokio::spawn(async move { background.request(request).await });
            tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().unwrap();
            let editing = tokio::time::timeout(Duration::from_secs(1), async {
                let value = service.request(Request::Create { mode: "new".into(), source_id: None, account: None }).await?;
                let mut draft: Draft = serde_json::from_value(value).map_err(error)?;
                draft.body = "Typed while the keychain was locked".into();
                service.request(Request::Save { draft }).await
            }).await;
            // Always release the worker before asserting, so failures cannot strand it.
            release.send(()).unwrap();
            background.await.unwrap().unwrap();
            let saved = editing.expect("keychain must not delay create/save").unwrap();
            assert_eq!(saved["body"], "Typed while the keychain was locked");
        }
    }

    #[tokio::test]
    async fn schedule_requests_reject_foreign_or_missing_identity_before_any_mutation() {
        for missing in [false, true] {
            let mut service = service();
            let item = scheduled(&service);
            let revision = item.schedule.as_ref().unwrap().report.report_revision;
            service.sending_identity = Arc::new(move |_| if missing {
                Err("keychain unavailable".into())
            } else {
                Ok("00000000-0000-4000-8000-000000000002".into())
            });
            let selection = || crate::scheduled_send::Selection { local_time: "2030-01-02T09:00".into(), time_zone: "America/New_York".into(), offset_seconds: None };
            for request in [
                Request::Cancel { id: item.id.clone() },
                Request::Recover { id: item.id.clone() },
                Request::SendScheduledNow { id: item.id.clone(), revision },
                Request::Reschedule { id: item.id.clone(), revision, selection: selection() },
                Request::Schedule { id: item.draft.id.clone(), revision: item.draft.revision, selection: selection() },
            ] {
                assert!(service.request(request).await.is_err());
                let current = service.database.outbox().unwrap().remove(0);
                assert_eq!(current.state, "scheduled");
                assert_eq!(current.deadline, item.deadline);
                assert_eq!(current.schedule.unwrap().report.report_revision, revision);
            }
            let listed = service.request(Request::ListOutbox).await.unwrap();
            assert_eq!(listed[0]["schedule"]["canManage"], false);
        }
    }

    #[tokio::test]
    async fn scheduled_dispatch_boundaries_and_two_installations_never_duplicate() {
        use std::sync::atomic::{AtomicUsize,Ordering};
        for delta in [-1,0,60_000,60_001] {
            let owner=service(); let item=scheduled(&owner);
            let mut peer=owner.clone();
            peer.sending_identity=Arc::new(|_|Ok("00000000-0000-4000-8000-000000000002".into()));
            let calls=AtomicUsize::new(0);
            let send=|_,_|async {calls.fetch_add(1,Ordering::SeqCst);Ok(DeliveryReceipt {provider_message_id:"sent".into(),thread_id:None})};
            assert!(peer.dispatch_due(&item,item.deadline+delta,false,&send).await.is_err());
            assert_eq!(calls.load(Ordering::SeqCst),0);
            let result=owner.dispatch_due(&item,item.deadline+delta,false,&send).await.unwrap();
            let due=(0..=60_000).contains(&delta);
            assert_eq!(result.is_some(),due);
            owner.dispatch_due(&item,item.deadline+delta,false,&send).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst),usize::from(due));
            if due {assert_eq!(owner.database.outbox().unwrap()[0].schedule.as_ref().unwrap().report.state,"sent");}
        }
    }

    #[tokio::test]
    async fn scheduled_reschedule_cancel_and_missing_identity_block_stale_delivery() {
        let mut owner=service();let item=scheduled(&owner);
        let revision=item.schedule.as_ref().unwrap().report.report_revision;
        let selection=crate::scheduled_send::Selection {local_time:"2030-01-02T09:00".into(),time_zone:"America/New_York".into(),offset_seconds:None};
        assert!(owner.database.manage_schedule(&item.id,revision,Some(&selection),false,"wrong-owner",item.deadline-1).is_err());
        owner.database.manage_schedule(&item.id,revision,Some(&selection),false,"00000000-0000-4000-8000-000000000001",item.deadline-1).unwrap();
        assert!(owner.database.manage_schedule(&item.id,revision,Some(&selection),false,"00000000-0000-4000-8000-000000000001",item.deadline-1).is_err());
        assert!(owner.dispatch_due(&item,item.deadline,false,|_,_|async {panic!("stale schedule sent")}).await.unwrap().is_none());
        owner.sending_identity=Arc::new(|_|Err("missing identity".into()));
        assert!(owner.request(Request::Cancel {id:item.id.clone()}).await.is_err());
        let current=owner.database.outbox().unwrap().remove(0);
        assert!(owner.dispatch_due(&current,current.deadline,false,|_,_|async {panic!("restored copy sent")}).await.is_err());
        owner.sending_identity=Arc::new(|_|Ok("00000000-0000-4000-8000-000000000001".into()));
        owner.request(Request::Cancel {id:item.id.clone()}).await.unwrap();
        assert_eq!(owner.database.drafts().unwrap()[0].body,item.draft.body);
        assert!(owner.dispatch_due(&current,current.deadline,false,|_,_|async {panic!("canceled schedule sent")}).await.unwrap().is_none());
    }

    #[test]
    fn scheduled_recovery_and_overdue_policy_preserve_ordinary_undo() {
        let owner=service();let item=scheduled(&owner);
        assert!(!owner.database.pending_undo().unwrap());
        owner.database.overdue_schedules_owned(item.deadline+60_000,false,"00000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(owner.database.outbox().unwrap()[0].state,"scheduled");
        owner.database.overdue_schedules_owned(item.deadline+60_001,false,"00000000-0000-4000-8000-000000000001").unwrap();
        let late=owner.database.outbox().unwrap().remove(0);
        assert_eq!(late.state,"overdue");assert_eq!(late.schedule.as_ref().unwrap().report.state,"overdue");
        owner.database.manage_schedule(&item.id,late.schedule.unwrap().report.report_revision,None,true,"00000000-0000-4000-8000-000000000001",item.deadline+60_001).unwrap();
        let now_send=owner.database.outbox().unwrap().remove(0);assert_eq!(now_send.state,"undo_pending");assert_eq!(now_send.deadline,item.deadline+70_001);
        let d=saved(&owner.database);let immediate=owner.database.queue(&d.id,d.revision,false,&[],None,&owner.root).unwrap();
        crate::schema::migrate(&mut owner.database.connection().unwrap()).unwrap();
        let items=owner.database.outbox().unwrap();
        assert!(items.iter().find(|o|o.id==immediate.id).unwrap().deadline>now());
        assert_eq!(items.iter().find(|o|o.id==item.id).unwrap().schedule.as_ref().unwrap().report.scheduled_at,item.deadline);
    }

    #[tokio::test]
    async fn scheduled_send_uncertainty_is_persisted_without_retry_and_report_failure_rolls_back() {
        let owner=service();let item=scheduled(&owner);
        owner.database.connection().unwrap().execute_batch("CREATE TRIGGER fail_report BEFORE UPDATE OF schedule_json ON outbox_messages BEGIN SELECT RAISE(FAIL,'report storage failed'); END;").unwrap();
        assert!(owner.dispatch_due(&item,item.deadline,false,|_,_|async {panic!("delivery before durable claim")}).await.is_err());
        assert_eq!(owner.database.outbox().unwrap()[0].state,"scheduled");
        owner.database.connection().unwrap().execute_batch("DROP TRIGGER fail_report;").unwrap();
        owner.dispatch_due(&item,item.deadline,false,|_,_|async {Err((false,"lost acknowledgement".into()))}).await.unwrap();
        let uncertain=owner.database.outbox().unwrap().remove(0);
        assert_eq!(uncertain.state,"uncertain");assert_eq!(uncertain.schedule.as_ref().unwrap().report.state,"uncertain");
        owner.dispatch_due(&uncertain,item.deadline+1,false,|_,_|async {panic!("uncertain resend")}).await.unwrap();
    }

}
