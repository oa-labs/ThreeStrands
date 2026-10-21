//! Server drafts in the account's `\Drafts` mailbox
//! (`docs/server-drafts-design.md`, "IMAP").
//!
//! IMAP messages are immutable, so every save is a new copy: `APPEND` the new
//! one, and only after it is stored, remove the old one. A failure part way
//! leaves a duplicate, never nothing; listing shows only the newest copy of
//! each `Message-ID` and removes this app's stale duplicates. A copy's id is
//! `UIDVALIDITY:UID`, which also serves as its version, since a copy never
//! changes after it is stored.
use crate::provider::{
    imap::{
        write::{self, build_uid_set, build_uid_set_batches, find_by_message_id, ImapWriteSession, StoreOp, WriteCaps},
        MailboxRole,
    },
    FetchedServerDraft, ProviderError, ProviderResult, ServerDraft,
};

use super::{mutate::as_write_session, ImapProvider};

/// A copy's place in the Drafts mailbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CopyId {
    uidvalidity: u32,
    uid: u32,
}

impl CopyId {
    fn parse(id: &str) -> Option<Self> {
        let (validity, uid) = id.split_once(':')?;
        Some(Self { uidvalidity: validity.parse().ok()?, uid: uid.parse().ok()? })
    }

    fn server_draft(self) -> ServerDraft {
        let id = format!("{}:{}", self.uidvalidity, self.uid);
        ServerDraft { version: Some(id.clone()), id }
    }
}

/// Only this app's own drafts carry these, so only their stale duplicates
/// are ever removed automatically.
fn is_own_draft_message_id(normalized: &str) -> bool {
    normalized.trim_matches(['<', '>']).starts_with("draft-")
}

fn message_id_of(header: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(header).ok()?;
    let unfolded = text.replace("\r\n", "\n").replace("\n ", " ").replace("\n\t", " ");
    unfolded.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("message-id")
            .then(|| crate::provider::imap::identity::normalize_message_id(value))
            .filter(|id| !id.is_empty())
    })
}

impl ImapProvider {
    /// The account's Drafts mailbox (wire name), or `None` when it has none.
    pub(super) fn drafts_mailbox(&self) -> Option<String> {
        let plan = self.build_sync_plan().ok()?;
        self.mailbox_with_role(&plan, MailboxRole::Drafts)
    }

    fn require_drafts_mailbox(&self) -> ProviderResult<String> {
        self.drafts_mailbox().ok_or_else(|| {
            ProviderError::InvalidOperation("This account has no Drafts mailbox, so drafts stay on this device".into())
        })
    }

    pub(super) async fn find_draft_copy(&self, message_id: &str) -> ProviderResult<Option<ServerDraft>> {
        let mailbox = self.require_drafts_mailbox()?;
        let mut session = self.open_session().await?;
        let result = async {
            let session = as_write_session(session.as_mut())?;
            let uidvalidity = examined_validity(session, &mailbox).await?;
            let located = find_by_message_id(session, message_id, uidvalidity).await?;
            Ok(located.uids.iter().max().map(|&uid| CopyId { uidvalidity, uid }.server_draft()))
        }
        .await;
        let _ = session.logout().await;
        result
    }

    /// Stores `raw` as a new copy and, when given, removes `replaces` once the
    /// new copy is stored.
    pub(super) async fn write_draft_copy(&self, raw: &[u8], replaces: Option<&str>) -> ProviderResult<ServerDraft> {
        let mailbox = self.require_drafts_mailbox()?;
        let message_id = crate::provider::imap::send::raw_message_id(raw)
            .ok_or_else(|| ProviderError::InvalidOperation("A draft needs a Message-ID to be saved to the server".into()))?;
        let mut session = self.open_session().await?;
        let result = async {
            let capabilities = session.capabilities().await?;
            let session = as_write_session(session.as_mut())?;
            let status = session.select(&mailbox).await?;
            let uidvalidity = status
                .uid_validity
                .ok_or_else(|| ProviderError::InvalidOperation("The Drafts mailbox reported no UIDVALIDITY".into()))?;
            let caps = WriteCaps::derive(&capabilities, &status);
            let before = find_by_message_id(session, &message_id, uidvalidity).await?.uids;
            // Flags the mailbox can't keep are left off rather than refused;
            // `\Seen` always goes, as it does on the Sent copy.
            let mut flags: Vec<String> = ["\\Draft"]
                .into_iter()
                .filter(|flag| caps.can_store_flag(flag))
                .map(str::to_string)
                .collect();
            flags.push("\\Seen".into());
            session.append(&mailbox, &flags, raw).await.map_err(write::WriteError::into_provider_error)?;
            let after = find_by_message_id(session, &message_id, uidvalidity).await?.uids;
            let Some(&uid) = after.iter().filter(|uid| !before.contains(uid)).max() else {
                // Stored but not yet visible to SEARCH. A retry finds it by
                // Message-ID instead of appending a second copy.
                return Err(ProviderError::RetryableServer("The saved draft isn't visible on the server yet".into()));
            };
            if let Some(old) = replaces.and_then(CopyId::parse).filter(|old| old.uidvalidity == uidvalidity && old.uid != uid) {
                remove_copies(session, &[old.uid], caps.has_uidplus).await?;
            }
            Ok(CopyId { uidvalidity, uid }.server_draft())
        }
        .await;
        let _ = session.logout().await;
        result
    }

    pub(super) async fn delete_draft_copy(&self, id: &str) -> ProviderResult<()> {
        let mailbox = self.require_drafts_mailbox()?;
        let copy = CopyId::parse(id).ok_or(ProviderError::NotFound)?;
        let mut session = self.open_session().await?;
        let result = async {
            let capabilities = session.capabilities().await?;
            let session = as_write_session(session.as_mut())?;
            let status = session.select(&mailbox).await?;
            if status.uid_validity != Some(copy.uidvalidity) || !copy_exists(session, copy.uid).await? {
                return Err(ProviderError::NotFound);
            }
            remove_copies(session, &[copy.uid], WriteCaps::derive(&capabilities, &status).has_uidplus).await
        }
        .await;
        let _ = session.logout().await;
        result
    }

    /// The newest copy of each draft. Stale duplicates of this app's own
    /// drafts, left by a save interrupted between storing and removing, are
    /// removed.
    pub(super) async fn list_draft_copies(&self) -> ProviderResult<Vec<ServerDraft>> {
        let mailbox = self.require_drafts_mailbox()?;
        let mut session = self.open_session().await?;
        let result = async {
            let capabilities = session.capabilities().await?;
            let session = as_write_session(session.as_mut())?;
            let status = session.select(&mailbox).await?;
            let uidvalidity = status
                .uid_validity
                .ok_or_else(|| ProviderError::InvalidOperation("The Drafts mailbox reported no UIDVALIDITY".into()))?;
            let uids = session.uid_search("NOT DELETED").await?;
            let mut newest: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            let mut unnamed = Vec::new();
            let mut stale = Vec::new();
            for set in build_uid_set_batches(&uids) {
                for fetched in session.uid_fetch(&set, "BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)]").await? {
                    let Some(uid) = fetched.uid else { continue };
                    match fetched.header().and_then(message_id_of) {
                        None => unnamed.push(uid),
                        Some(id) => match newest.get(&id).copied() {
                            Some(kept) => {
                                let (keep, drop) = if uid > kept { (uid, kept) } else { (kept, uid) };
                                newest.insert(id.clone(), keep);
                                if is_own_draft_message_id(&id) {
                                    stale.push(drop);
                                }
                            }
                            None => {
                                newest.insert(id, uid);
                            }
                        },
                    }
                }
            }
            if !stale.is_empty() {
                remove_copies(session, &stale, WriteCaps::derive(&capabilities, &status).has_uidplus).await?;
            }
            let mut listed: Vec<u32> = newest.into_values().chain(unnamed).collect();
            listed.sort_unstable();
            Ok(listed.into_iter().map(|uid| CopyId { uidvalidity, uid }.server_draft()).collect())
        }
        .await;
        let _ = session.logout().await;
        result
    }

    pub(super) async fn fetch_draft_copy(&self, id: &str) -> ProviderResult<FetchedServerDraft> {
        let mailbox = self.require_drafts_mailbox()?;
        let copy = CopyId::parse(id).ok_or(ProviderError::NotFound)?;
        let mut session = self.open_session().await?;
        let result = async {
            let session = as_write_session(session.as_mut())?;
            if examined_validity(session, &mailbox).await? != copy.uidvalidity {
                return Err(ProviderError::NotFound);
            }
            let set = build_uid_set(&[copy.uid]).ok_or(ProviderError::NotFound)?;
            let fetched = session.uid_fetch(&set, "BODY.PEEK[]").await?;
            let raw = fetched
                .iter()
                .find(|message| message.uid == Some(copy.uid))
                .and_then(|message| message.body())
                .ok_or(ProviderError::NotFound)?
                .to_vec();
            crate::provider::imap::policy::check_append_bytes(raw.len())?;
            // IMAP conversations are threaded locally, so a draft names none.
            Ok(FetchedServerDraft { draft: copy.server_draft(), thread_id: None, raw })
        }
        .await;
        let _ = session.logout().await;
        result
    }

    pub(super) async fn draft_copy_version(&self, id: &str) -> ProviderResult<Option<String>> {
        let mailbox = self.require_drafts_mailbox()?;
        let Some(copy) = CopyId::parse(id) else { return Ok(None) };
        let mut session = self.open_session().await?;
        let result = async {
            let session = as_write_session(session.as_mut())?;
            if examined_validity(session, &mailbox).await? != copy.uidvalidity {
                return Ok(None);
            }
            Ok(copy_exists(session, copy.uid).await?.then(|| copy.server_draft().id))
        }
        .await;
        let _ = session.logout().await;
        result
    }
}

async fn examined_validity(session: &mut dyn ImapWriteSession, mailbox: &str) -> ProviderResult<u32> {
    session
        .examine(mailbox)
        .await?
        .uid_validity
        .ok_or_else(|| ProviderError::InvalidOperation("The Drafts mailbox reported no UIDVALIDITY".into()))
}

async fn copy_exists(session: &mut dyn ImapWriteSession, uid: u32) -> ProviderResult<bool> {
    let set = build_uid_set(&[uid]).ok_or(ProviderError::NotFound)?;
    Ok(session.uid_search(&format!("UID {set} NOT DELETED")).await?.contains(&uid))
}

/// Marks `uids` deleted in the selected mailbox and, with `UIDPLUS`, expunges
/// exactly those. Without it they stay marked: a plain `EXPUNGE` would also
/// remove other clients' deleted mail.
async fn remove_copies(session: &mut dyn ImapWriteSession, uids: &[u32], has_uidplus: bool) -> ProviderResult<()> {
    let deleted = vec!["\\Deleted".to_string()];
    for set in build_uid_set_batches(uids) {
        session.uid_store(&set, StoreOp::Add, &deleted).await?;
        if has_uidplus {
            session.uid_expunge(&set).await?;
        }
    }
    Ok(())
}
