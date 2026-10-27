//! The compose service: answers composer requests and runs the send queue,
//! Sent copies, and server-draft upkeep against each account's provider.

use crate::clock::unix_millis;
use crate::{
    auth::AccountAuth,
    db::Database,
    limits::MAX_ATTACHMENT_BYTES,
    mime::MimePart,
    provider::{DeliveryReceipt, MailProvider},
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

use super::mime::header;
use super::{error, json, Attachment, Draft, OutboxItem, Request};

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
pub(super) fn sent_copy_error_message(err: &crate::provider::ProviderError) -> String {
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
pub(super) fn permanent_sent_copy_message(provider_message: &str) -> String {
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
        Ok(self.database.overdue_schedules_owned(unix_millis(),true,&owner)?)
    }
    pub fn pause_account_schedules(&self,account:&str) -> Result<(),String> {
        let owner=self.schedule_owner(false).unwrap_or_default();
        Ok(self.database.pause_ready_sends_for_owned(account,&owner)?)
    }
    fn schedule_owner(&self, create: bool) -> Result<String,String> {
        let has_schedules = self.database.has_scheduled_sends()?;
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
        if let Some(owner) = &owner { self.database.refresh_schedule_reports(owner, unix_millis())?; }
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
        let missing = !self.database.has_message_metadata(id)?;
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
                crate::run_database_task(move || database.push_drafts_now(Some(&id), unix_millis()))
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
                    return serde_json::to_value(item).map_err(error);
                }
                let draft=self.database.draft(&id)?;
                let provider = self.provider_for(&draft.account).await?;
                if !provider.capabilities().send_supported { return Err("Scheduled sending is not supported for this account yet".into()); }
                let identities = provider.identities(&draft.account).await.map_err(error)?;
                let message_id_domain = provider.message_id_domain(draft.sender_address()).map_err(error)?;
                let owner=schedule_owner;
                Ok(serde_json::to_value(self.database.schedule_draft(&id,revision,&selection,&owner,&identities,message_id_domain.as_deref(),&self.root,unix_millis())?).map_err(error)?)
            }
            Reschedule {id,revision,selection} => {
                let owner=schedule_owner;
                self.database.manage_schedule(&id,revision,Some(&selection),false,&owner,unix_millis())?;
                Ok(serde_json::Value::Null)
            }
            SendScheduledNow {id,revision} => {
                let owner=schedule_owner;
                self.database.manage_schedule(&id,revision,None,true,&owner,unix_millis())?;
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
                let mut saved = self.database.save_draft(*draft)?;
                saved.server_draft = self.database.server_draft_status(&saved)?;
                Ok(serde_json::to_value(saved).map_err(error)?)
            }
            ResolveConflict { id, keep_server } => {
                Ok(serde_json::to_value(self.database.resolve_draft_conflict(&id, keep_server)?).map_err(error)?)
            }
            RestoreDraft { id } => {
                let mut restored = self.database.restore_discarded_draft(&id)?;
                restored.server_draft = self.database.server_draft_status(&restored)?;
                Ok(serde_json::to_value(restored).map_err(error)?)
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
                    return serde_json::to_value(item).map_err(error);
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
                    .message_id_domain(draft.sender_address())
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
        self.database.purge_discarded_drafts(unix_millis())?;
        let mut retained: HashSet<String> = self
            .database
            .drafts()?
            .iter()
            .flat_map(|d| d.attachments.iter().map(|a| a.id.clone()))
            .collect();
        // Undo brings a discarded draft back with its files.
        retained.extend(self.database.discarded_draft_attachments()?);
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

    pub(super) async fn reconcile_with_provider(
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
            self.database.mark_uncertain_send_sent(id, &message.provider_message_id, &owner, unix_millis())?;
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
        self.database.overdue_schedules_owned(unix_millis(),false,owner.as_deref().unwrap_or(""))?;
        if let Some(owner)=&owner { self.database.refresh_schedule_reports(owner,unix_millis())?; }
        self.database.fail_sends_for_removed_accounts(owner.as_deref().unwrap_or(""))?;
        let items = self.database.outbox()?;
        // Process due Sent-copy jobs FIRST, every tick — before the
        // no-sendable-items early return below. A job is created when a send is
        // accepted (`dispatch_due`) and lives on the outbox row independently of
        // the send states; a tick with nothing new to send must still drive
        // jobs to completion (fact 4). Holding `self.gate` for the whole tick
        // gives per-message serialisation for free; the processor caps its own
        // per-tick work.
        self.process_sent_copy_jobs(unix_millis()).await?;
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
            if !["scheduled","undo_pending","ready","uncertain"].contains(&item.state.as_str()) || item.deadline>unix_millis() {continue;}
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
                && !crate::provider::imap::smtp_prepare_retry_due(item.last_attempt_at, unix_millis())
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
                for item in &items { if item.schedule.is_some() {self.database.block_schedule(&item.id,owner.as_deref().unwrap_or(""),Some("account_disconnected"),unix_millis())?;} }
                continue;
            };
            let Ok(identity) = provider.sender_identity().await else {
                for item in &items { if item.schedule.is_some() {self.database.block_schedule(&item.id,owner.as_deref().unwrap_or(""),Some("authorization"),unix_millis())?;} }
                continue;
            };
            for item in items {
                if item.schedule.as_ref().is_some_and(|s|Some(s.report.owner_installation_id.as_str())!=owner.as_deref()) { continue; }
                if item.draft.account != identity {
                    self.database.pause_send_for_other_sender(&item.id)?;
                    continue;
                }
                if item.state == "uncertain" {
                    if item.deadline <= unix_millis() {
                        self.database.defer_uncertain_send(&item.id, unix_millis() + 60_000)?;
                        let _ = self.reconcile(&item.id).await;
                    }
                    continue;
                }
                if !["undo_pending", "ready", "scheduled"].contains(&item.state.as_str())
                    || item.deadline > unix_millis()
                {
                    continue;
                }
                self.send_due_item(&item, provider.as_ref(), &identity, owner.as_deref().unwrap_or(""), unix_millis())
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
    pub(super) async fn send_due_item(
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
        self.database.note_send_prepare_retry(&item.id, message, owner, at)?;
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
        self.database.fail_queued_send(&item.id, message, owner, at)?;
        Ok(())
    }
    /// Drive every DUE Sent-copy job this tick (fact 4: NOT skipped by the
    /// no-sendable-items early return). A job is `pending`/`verify` with
    /// `sent_copy_next_at <= now`; it is processed oldest-first, at most
    /// [`SENT_COPY_MAX_JOBS_PER_TICK`], resolving each account once so its
    /// unavailable provider does not abort the tick or the others' jobs. An
    /// account whose provider is unavailable is skipped WITHOUT counting an
    /// attempt. Never touches the outbox `state`, `provider_id`, never resends.
    pub(super) async fn process_sent_copy_jobs(&self, at: i64) -> Result<(), String> {
        self.process_sent_copy_jobs_with(at, |account| async move {
            self.provider_for(&account).await.ok()
        })
        .await
    }

    pub(super) async fn process_sent_copy_jobs_with<F, Fut>(
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
        let accounts = self.database.sent_copy_job_accounts(at)?;
        let mut providers = HashMap::new();
        for account in accounts {
            if let Some(provider) = resolve(account.clone()).await {
                providers.insert(account, provider);
            }
        }
        if providers.is_empty() {
            return Ok(());
        }
        let available: Vec<&str> = providers.keys().map(String::as_str).collect();
        // Only eligible jobs load raw MIME, still oldest-first and bounded.
        let due = self.database.due_sent_copy_jobs(
            at,
            &available,
            crate::provider::imap::SENT_COPY_MAX_JOBS_PER_TICK as i64,
        )?;
        for job in due {
            if let Some(provider) = providers.get(&job.account) {
                self.run_sent_copy_job(
                    provider.as_ref(),
                    &job.id,
                    &job.raw,
                    &job.state,
                    job.attempts,
                    job.append_attempted,
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
    pub(super) async fn run_sent_copy_job(
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
            self.database.mark_sent_copy_append_attempted(id, at)?;
        }

        let request = SentCopyRequest {
            raw: raw.to_vec(),
            outbox_id: id.to_string(),
            append_attempted,
            phase,
        };

        match provider.save_sent_copy(&request).await {
            Ok(SentCopyStep::Done) => {
                self.database.complete_sent_copy_job(id, at)?;
            }
            Ok(SentCopyStep::Recheck { after }) => {
                let next_at = at + after.as_millis() as i64;
                self.database.recheck_sent_copy_job(id, next_at, at)?;
            }
            Err(err) => {
                let permanent = matches!(
                    err,
                    ProviderError::InvalidOperation(_)
                        | ProviderError::PermanentClientRejection(_)
                );
                let message = sent_copy_error_message(&err);
                if permanent {
                    self.database.abandon_sent_copy_job(id, &message, at)?;
                } else {
                    // Transient: count the attempt and reschedule from the
                    // backoff (floored at the settle window), or abandon at the
                    // cap. The state is KEPT (a verify stays verify).
                    let new_attempts = attempts + 1;
                    if new_attempts as u32 >= crate::provider::imap::SENT_COPY_MAX_ATTEMPTS {
                        self.database.abandon_sent_copy_job_after_attempts(id, new_attempts, &message, at)?;
                    } else {
                        let next_at = at
                            + crate::provider::imap::sent_copy_backoff(attempts as u32).as_millis()
                                as i64;
                        self.database.retry_sent_copy_job(id, new_attempts, next_at, &message, at)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) async fn dispatch_due<F, Fut>(
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
        let raw = self.database.outbox_raw(&item.id)?;
        let owner=if item.schedule.is_some() { self.authorize_schedule_async(&item.id).await? } else {String::new()};
        let claimed=self.database.claim_due_send(&item.id,at,at.saturating_sub(crate::scheduled_send::DISPATCH_GRACE_MS),item.schedule.as_ref().map(|s|s.report.report_revision),&owner)?;
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
                let sent_copy_next_at = seed_sent_copy
                    .then(|| at + crate::provider::imap::SENT_COPY_SETTLE.as_millis() as i64);
                self.database.record_send_accepted(&item.id, &sent.provider_message_id, sent_copy_next_at, &owner, at)?;
                Ok(Some(sent))
            }
            Err((definite, message)) => {
                self.database.record_send_failed(&item.id, definite, &message, &owner, at)?;
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
