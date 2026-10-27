//! Drafts, queued sends, and the requests the composer makes, with the
//! database operations that create, save, and queue drafts.

use crate::clock::unix_millis;
use crate::{db::Database, mime::RawMessage};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};
use uuid::Uuid;

use super::mime::{addresses, attribution_date, build_mime, forward_attachments, header, plain_text_html, reference_ids};
use super::{error, UNDO_MS};

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
    /// The user's edited copy of `forwarded_content`, saved by the composer
    /// after the compose sanitizer. When set, it sends in place of the
    /// original, which stays unchanged so the user can restore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_edit: Option<ForwardedContent>,
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
    pub fn sender_address(&self) -> &str {
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
        draft: Box<Draft>,
    },
    Discard {
        id: String,
    },
    /// Brings back a draft discarded within the last hour (Undo).
    RestoreDraft {
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

impl Database {
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
            forwarded_edit: None,
            follow_up_task_id: None,
            attachments: vec![],
            updated_at: unix_millis(),
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
        d.signature = self.default_draft_signature(&d.account, d.sender_address(), &d.mode)?;
        self.insert_draft(&mut d)?;
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
            forwarded_edit: None,
            follow_up_task_id: None,
            attachments: forward_attachments(&source, &normalized)?,
            updated_at: unix_millis(),
            server_draft: None,
        };
        self.insert_draft(&mut d)?;
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
        let forwarded_edit_len = draft.forwarded_edit.as_ref().map_or(0, |edit| edit.html.len() + edit.text.len());
        if draft.body.len() + draft.body_html.len() + forwarded_edit_len > 2 * 1024 * 1024
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
        // An edited copy exists only beside the original it replaces.
        if draft.forwarded_content.is_none() {
            draft.forwarded_edit = None;
        }
        draft.follow_up_task_id = draft.follow_up_task_id.or(old.follow_up_task_id);
        if draft.subject != old.subject {
            draft.thread_id = None;
        }
        let expected = draft.revision;
        draft.revision += 1;
        draft.updated_at = unix_millis();
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
        d.updated_at = unix_millis();
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
        d.updated_at = unix_millis();
        let changed = self.update_draft(&mut d, expected)?;
        if changed != 1 {
            return Err("Draft changed elsewhere. Reopen it before editing.".into());
        }
        Ok(d)
    }
    pub(super) fn save_attachment_draft(&self, mut d: Draft) -> Result<Draft, String> {
        let expected = d.revision;
        d.revision += 1;
        d.updated_at = unix_millis();
        if self.update_draft(&mut d, expected)? != 1 {
            return Err("Draft changed while preparing attachment; retry".into());
        }
        Ok(d)
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
            deadline: unix_millis() + UNDO_MS,
            error: None,
            provider_id: None,
            last_attempt_at: None,
            archive_on_send,
            schedule: None,
            sent_copy: None,
        };
        self.insert_queued_send(&item, revision, &raw)?;
        Ok(item)
    }
}
