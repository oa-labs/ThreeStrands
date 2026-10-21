//! Brings the account's server drafts into local drafts
//! (`docs/server-drafts-design.md`, "Pull and import"). A draft created or
//! edited in another app, or on another device, appears in **Drafts**; one
//! deleted elsewhere leaves, unless this device has edits the server hasn't
//! seen. When both sides changed, the server's version is held back and the
//! user picks one.
use std::{collections::HashMap, path::Path, sync::Arc};

use mail_parser::{Address, MessageParser, MimeHeaders, PartType};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::{
    correspondence::{Attachment, Correspondence, Draft},
    db::{serialization_error, Database, DatabaseError, DbResult},
    draft_push::{server_message_local_part, DraftEvent, Link},
    limits::MAX_ATTACHMENT_BYTES,
    provider::{FetchedServerDraft, MailProvider, ProviderError, ProviderResult, ServerDraft},
};

/// How often each account's server drafts are listed.
pub(crate) const DRAFT_PULL_INTERVAL_MS: i64 = 60_000;
/// The largest draft body this device imports; the composer refuses larger.
const MAX_IMPORTED_BODY_BYTES: usize = 2 * 1024 * 1024;

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The local draft id a server copy names in its `Message-ID`, when it is one
/// ThreeStrands wrote.
fn draft_id_from_message_id(message_id: &str) -> Option<String> {
    let local = message_id.trim_matches(['<', '>']).split('@').next()?;
    let id = local.strip_prefix("draft-")?;
    Uuid::parse_str(id).ok()?;
    (server_message_local_part(id) == local).then(|| id.to_string())
}

fn format_addresses(address: Option<&Address>) -> String {
    address
        .into_iter()
        .flat_map(|list| list.iter())
        .filter_map(|addr| {
            let email = addr.address()?;
            Some(match addr.name().map(str::trim).filter(|name| !name.is_empty()) {
                Some(name) if name.contains([',', ';', '<', '>', '@', '"', '(', ')']) => {
                    format!("\"{}\" <{email}>", name.replace(['"', '\\'], ""))
                }
                Some(name) => format!("{name} <{email}>"),
                None => email.to_string(),
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Turns a server copy into a local draft. `base` is the local draft it
/// replaces, whose ThreeStrands-only fields survive; a signature or
/// forwarded message is part of the server copy's body, so neither stays
/// structured (`docs/server-drafts-design.md`, "Fields email can't carry").
/// Attachment bytes are written under `root`.
pub(crate) fn draft_from_server_copy(
    fetched: &FetchedServerDraft,
    account: &str,
    id: &str,
    base: Option<&Draft>,
    root: &Path,
) -> Result<Draft, String> {
    let message = MessageParser::default()
        .parse(&fetched.raw)
        .ok_or("The server returned a draft that isn't a valid message")?;
    let subject = message.subject().unwrap_or_default().replace(['\r', '\n'], " ");
    let body = message.body_text(0).map(|text| text.into_owned()).unwrap_or_default();
    // Only an authored HTML part: mail-parser renders plain text as HTML when
    // there is none, which would turn a plain draft into a formatted one.
    let body_html = match message.html_part(0).map(|part| &part.body) {
        Some(PartType::Html(html)) => html.to_string(),
        _ => String::new(),
    };
    if body.len() + body_html.len() > MAX_IMPORTED_BODY_BYTES {
        return Err("The draft on the server is too large to open here".into());
    }
    let reply_id = message
        .in_reply_to()
        .as_text_list()
        .and_then(|ids| ids.first().map(|id| id.to_string()))
        .filter(|id| !id.contains(['\r', '\n']));
    let references = message
        .references()
        .as_text_list()
        .map(|ids| ids.iter().map(|id| id.to_string()).filter(|id| !id.contains(['\r', '\n'])).collect())
        .unwrap_or_default();
    let sender = message
        .from()
        .and_then(|from| from.first())
        .and_then(|from| from.address())
        .map(str::to_string);
    // A sending address other than the account's own is kept only when this
    // draft had already chosen it; otherwise the account sends.
    let from = base
        .and_then(|base| base.from.clone())
        .filter(|chosen| sender.as_deref().is_some_and(|sender| sender.eq_ignore_ascii_case(chosen)));

    let mut attachments = Vec::new();
    let mut total = 0_usize;
    for part in message.attachments() {
        let bytes = part.contents();
        total += bytes.len();
        if total > MAX_ATTACHMENT_BYTES {
            return Err("The draft's attachments exceed the 18 MB local limit".into());
        }
        let mime = part
            .content_type()
            .map(|kind| match kind.subtype() {
                Some(subtype) => format!("{}/{subtype}", kind.ctype()),
                None => kind.ctype().to_string(),
            })
            .unwrap_or_else(|| "application/octet-stream".into());
        let content_id = part.content_id().map(|cid| cid.trim_matches(['<', '>']).to_string());
        let inline = content_id
            .as_deref()
            .is_some_and(|cid| body_html.contains(&format!("cid:{cid}")));
        let attachment = Attachment {
            id: Uuid::new_v4().to_string(),
            name: part.attachment_name().unwrap_or("attachment").to_string(),
            size: bytes.len() as u64,
            mime,
            ready: true,
            message_id: None,
            provider_id: None,
            inline,
            content_id: content_id.filter(|_| inline),
        };
        std::fs::write(root.join(&attachment.id), bytes).map_err(|error| error.to_string())?;
        attachments.push(attachment);
    }

    Ok(Draft {
        id: id.to_string(),
        revision: base.map_or(1, |base| base.revision + 1),
        account: account.to_string(),
        from,
        mode: base
            .map(|base| base.mode.clone())
            .unwrap_or_else(|| if reply_id.is_some() { "reply".into() } else { "new".into() }),
        source_id: base.and_then(|base| base.source_id.clone()),
        thread_id: fetched.thread_id.clone(),
        reply_id,
        references,
        to: format_addresses(message.to()),
        cc: format_addresses(message.cc()),
        bcc: format_addresses(message.bcc()),
        subject,
        body,
        body_html,
        signature: None,
        forwarded_content: None,
        follow_up_task_id: base.and_then(|base| base.follow_up_task_id.clone()),
        attachments,
        updated_at: now(),
        server_draft: None,
    })
}

/// What a pull did to one server draft.
#[derive(Debug, PartialEq, Eq)]
enum Applied {
    Unchanged,
    Changed,
    Conflict,
}

impl Database {
    /// Stores `imported`, the server's version of a draft, unless this
    /// device has edits the server hasn't seen: then the server's version is
    /// held back for the user to choose. Checked and written in one
    /// transaction, so a save racing the pull is never overwritten.
    fn apply_server_draft(&self, imported: &Draft, server: &ServerDraft) -> DbResult<Applied> {
        self.with_transaction(|tx| {
            let local: Option<i64> = tx
                .query_row("SELECT revision FROM drafts WHERE id=?1", [&imported.id], |row| row.get(0))
                .optional()?;
            let pushed: Option<i64> = tx
                .query_row("SELECT pushed_revision FROM draft_server_links WHERE draft_id=?1", [&imported.id], |row| row.get(0))
                .optional()?;
            if let (Some(local), Some(pushed)) = (local, pushed) {
                if local > pushed {
                    let held = serde_json::to_string(imported).map_err(serialization_error)?;
                    tx.execute(
                        "UPDATE draft_server_links SET conflict=?2, server_id=?3, server_version=?4 WHERE draft_id=?1",
                        params![imported.id, held, server.id, server.version],
                    )?;
                    return Ok(Applied::Conflict);
                }
            }
            let mut stored = imported.clone();
            // Newer than any revision an open composer holds, so it follows.
            stored.revision = local.map_or(1, |local| local + 1);
            let payload = serde_json::to_string(&stored).map_err(serialization_error)?;
            tx.execute(
                "INSERT INTO drafts(id, revision, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET revision=excluded.revision, payload=excluded.payload",
                params![stored.id, stored.revision, payload],
            )?;
            tx.execute(
                "INSERT INTO draft_server_links(draft_id, account, server_id, server_version, pushed_revision, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'saved')
                 ON CONFLICT(draft_id) DO UPDATE SET account=excluded.account, server_id=excluded.server_id,
                     server_version=excluded.server_version, pushed_revision=excluded.pushed_revision,
                     state='saved', error=NULL, attempts=0, due_at=NULL, pending_since=NULL, conflict=NULL",
                params![stored.id, stored.account, server.id, server.version, stored.revision],
            )?;
            Ok(Applied::Changed)
        })
    }

    /// Links an existing local draft to the server copy carrying its
    /// `Message-ID`, without changing either.
    fn adopt_server_copy(&self, draft_id: &str, server_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE draft_server_links SET server_id=?2 WHERE draft_id=?1 AND server_id IS NULL",
                params![draft_id, server_id],
            )?;
            Ok(())
        })
    }

    /// Whether a server copy naming `draft_id` belongs to a draft this device
    /// already sent, is sending, or is about to delete.
    fn draft_left_this_device(&self, draft_id: &str) -> DbResult<bool> {
        self.with_connection(|connection| {
            let sending: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM outbox_messages WHERE draft_id=?1 AND state<>'canceled')",
                [draft_id],
                |row| row.get(0),
            )?;
            let deleting: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM draft_server_deletions WHERE message_id=?1)",
                [server_message_local_part(draft_id)],
                |row| row.get(0),
            )?;
            let linked: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM draft_server_links WHERE draft_id=?1)",
                [draft_id],
                |row| row.get(0),
            )?;
            let local: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM drafts WHERE id=?1)",
                [draft_id],
                |row| row.get(0),
            )?;
            // Linked but not stored: waiting for its send to settle.
            Ok(sending || deleting || (linked && !local))
        })
    }

    /// The server copy of `link` is gone. Drops the local draft unless it has
    /// edits the server never saw; those are pushed again as a new copy.
    /// Returns whether the local draft was removed.
    fn server_copy_deleted(&self, link: &Link) -> DbResult<bool> {
        self.with_transaction(|tx| {
            // This pull may already have moved the link to a newer copy, as
            // when a server stores every save as a new message.
            let current: Option<Option<String>> = tx
                .query_row("SELECT server_id FROM draft_server_links WHERE draft_id=?1", [&link.draft_id], |row| row.get(0))
                .optional()?;
            if current.flatten() != link.server_id {
                return Ok(false);
            }
            let local: Option<i64> = tx
                .query_row("SELECT revision FROM drafts WHERE id=?1", [&link.draft_id], |row| row.get(0))
                .optional()?;
            match local {
                // Waiting for its send to settle; the push worker owns it.
                None => Ok(false),
                Some(revision) if revision > link.pushed_revision => {
                    tx.execute(
                        "UPDATE draft_server_links SET server_id=NULL, server_version=NULL, conflict=NULL, state='pending',
                             due_at=?2, pending_since=COALESCE(pending_since, ?2) WHERE draft_id=?1",
                        params![link.draft_id, now()],
                    )?;
                    Ok(false)
                }
                Some(_) => {
                    tx.execute("DELETE FROM draft_server_links WHERE draft_id=?1", [&link.draft_id])?;
                    tx.execute("DELETE FROM drafts WHERE id=?1", [&link.draft_id])?;
                    Ok(true)
                }
            }
        })
    }

    /// Settles a conflict: `keep_server` adopts the server's version,
    /// otherwise this device's version is pushed over it.
    pub(crate) fn resolve_draft_conflict(&self, id: &str, keep_server: bool) -> Result<Draft, String> {
        let result = self.with_transaction(|tx| {
            let held: Option<String> = tx
                .query_row("SELECT conflict FROM draft_server_links WHERE draft_id=?1", [id], |row| row.get(0))
                .optional()?
                .flatten();
            let Some(held) = held else {
                return Err(DatabaseError::invalid("This draft no longer has another version"));
            };
            let local: Option<(i64, String)> = tx
                .query_row("SELECT revision, payload FROM drafts WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((revision, payload)) = local else {
                return Err(DatabaseError::invalid("Draft no longer exists"));
            };
            if !keep_server {
                tx.execute(
                    "UPDATE draft_server_links SET conflict=NULL, state='pending', due_at=?2, pending_since=COALESCE(pending_since, ?2) WHERE draft_id=?1",
                    params![id, now()],
                )?;
                return serde_json::from_str::<Draft>(&payload).map_err(serialization_error);
            }
            let mut server: Draft = serde_json::from_str(&held).map_err(serialization_error)?;
            server.revision = revision + 1;
            server.updated_at = now();
            tx.execute(
                "UPDATE drafts SET revision=?2, payload=?3 WHERE id=?1",
                params![id, server.revision, serde_json::to_string(&server).map_err(serialization_error)?],
            )?;
            tx.execute(
                "UPDATE draft_server_links SET conflict=NULL, pushed_revision=?2, state='saved', error=NULL, due_at=NULL, pending_since=NULL WHERE draft_id=?1",
                params![id, server.revision],
            )?;
            Ok(server)
        });
        let mut draft = result.map_err(|error| error.to_string())?;
        draft.server_draft = self.server_draft_status(&draft).map_err(|error| error.to_string())?;
        Ok(draft)
    }
}

impl Correspondence {
    /// Lists `account`'s server drafts and brings changes made elsewhere into
    /// local drafts. Returns whether any local draft changed.
    pub(crate) async fn pull_server_drafts(
        &self,
        account: &str,
        provider: &dyn MailProvider,
    ) -> Result<bool, String> {
        let listed = match provider.list_server_drafts().await {
            Ok(listed) => listed,
            Err(error) => {
                log::warn!(target: "drafts", "listing server drafts failed: {error}");
                return Ok(false);
            }
        };
        let links = self.database.account_draft_links(account)?;
        let by_server: HashMap<&str, &Link> = links
            .iter()
            .filter_map(|link| link.server_id.as_deref().map(|id| (id, link)))
            .collect();
        let mut changed = false;
        for server in &listed {
            let link = by_server.get(server.id.as_str()).copied();
            if link.is_some_and(|link| link.server_version.is_some() && link.server_version == server.version) {
                continue;
            }
            match self.pull_one(account, provider, server, link).await {
                Ok(Applied::Unchanged) => {}
                Ok(Applied::Changed) => changed = true,
                Ok(Applied::Conflict) => {
                    changed = true;
                    if let Some(draft) = link.and_then(|link| self.database.local_draft(&link.draft_id).ok().flatten()) {
                        self.notify_conflict(&draft);
                    }
                }
                Err(error) => log::warn!(target: "drafts", "importing a server draft failed: {error}"),
            }
        }
        let listed_ids: std::collections::HashSet<&str> = listed.iter().map(|server| server.id.as_str()).collect();
        for link in links.iter().filter(|link| link.server_id.as_deref().is_some_and(|id| !listed_ids.contains(id))) {
            changed |= self.database.server_copy_deleted(link)?;
        }
        Ok(changed)
    }

    async fn pull_one(
        &self,
        account: &str,
        provider: &dyn MailProvider,
        server: &ServerDraft,
        link: Option<&Link>,
    ) -> Result<Applied, String> {
        let fetched = match provider.fetch_server_draft(&server.id).await {
            Ok(fetched) => fetched,
            // Deleted between listing and fetching: the next pull sees it gone.
            Err(ProviderError::NotFound) => return Ok(Applied::Unchanged),
            Err(error) => return Err(error.to_string()),
        };
        let draft_id = match link {
            Some(link) => link.draft_id.clone(),
            None => {
                let named = MessageParser::default()
                    .parse_headers(&fetched.raw)
                    .and_then(|message| message.message_id().and_then(draft_id_from_message_id));
                match named {
                    Some(id) if self.database.draft_left_this_device(&id)? => return Ok(Applied::Unchanged),
                    Some(id) => {
                        // This device's draft, created before it learned the server id.
                        if self.database.local_draft(&id)?.is_some() {
                            self.database.adopt_server_copy(&id, &server.id)?;
                        }
                        id
                    }
                    None => Uuid::new_v4().to_string(),
                }
            }
        };
        let base = self.database.local_draft(&draft_id)?;
        if base.as_ref().is_some_and(|base| base.account != account) {
            return Ok(Applied::Unchanged);
        }
        let imported = draft_from_server_copy(&fetched, account, &draft_id, base.as_ref(), &self.root)?;
        Ok(self.database.apply_server_draft(&imported, &fetched.draft)?)
    }

    /// Fetches the server's version of `draft` and holds it for the user,
    /// because both sides changed it. Called before a push would overwrite it.
    pub(crate) async fn hold_server_conflict(
        &self,
        provider: &dyn MailProvider,
        draft: &Draft,
        server_id: &str,
    ) -> ProviderResult<()> {
        let fetched = provider.fetch_server_draft(server_id).await?;
        let imported = draft_from_server_copy(&fetched, &draft.account, &draft.id, Some(draft), &self.root)
            .map_err(ProviderError::Other)?;
        self.database
            .apply_server_draft(&imported, &fetched.draft)
            .map_err(|error| ProviderError::Other(error.to_string()))?;
        (self.draft_events)(DraftEvent::ListChanged);
        Ok(())
    }

    fn notify_conflict(&self, draft: &Draft) {
        (self.draft_events)(DraftEvent::Status(crate::draft_push::ServerDraftEvent {
            id: draft.id.clone(),
            revision: draft.revision,
            status: crate::draft_push::ServerDraftStatus { state: "conflict".into(), error: None },
        }));
    }

    /// Pulls every connected account that keeps server drafts and is due.
    pub(crate) async fn pull_due_accounts(
        &self,
        at: i64,
        providers: &HashMap<String, Arc<dyn MailProvider>>,
        last_pulled: &mut HashMap<String, i64>,
        force: bool,
    ) -> Result<(), String> {
        let mut changed = false;
        for (account, provider) in providers {
            if !provider.server_drafts() {
                continue;
            }
            let due = last_pulled.get(account).is_none_or(|last| at - last >= DRAFT_PULL_INTERVAL_MS);
            if !force && !due {
                continue;
            }
            last_pulled.insert(account.clone(), at);
            changed |= self.pull_server_drafts(account, provider.as_ref()).await?;
        }
        if changed {
            (self.draft_events)(DraftEvent::ListChanged);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft_push::tests::{Harness, ACCOUNT, PAST_DELAY};

    /// A draft as another app would store it.
    fn message(message_id: &str, subject: &str, body: &str) -> String {
        format!(
            "From: you@example.com\r\nTo: \"Doe, Jane\" <jane@example.com>, sam@example.com\r\nCc: lee@example.com\r\nBcc: hidden@example.com\r\nSubject: {subject}\r\nMessage-ID: <{message_id}>\r\nIn-Reply-To: <original@example.com>\r\nReferences: <root@example.com> <original@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
    }

    fn html_message(message_id: &str) -> String {
        format!(
            "From: you@example.com\r\nTo: jane@example.com\r\nSubject: Styled\r\nMessage-ID: <{message_id}>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"a\"\r\n\r\n--a\r\nContent-Type: text/plain\r\n\r\nHello there\r\n--a\r\nContent-Type: text/html\r\n\r\n<p>Hello <b>there</b></p>\r\n--a--\r\n"
        )
    }

    fn with_attachment(message_id: &str) -> String {
        format!(
            "From: you@example.com\r\nTo: jane@example.com\r\nSubject: Notes\r\nMessage-ID: <{message_id}>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached\r\n--b\r\nContent-Type: application/pdf; name=\"notes.pdf\"\r\nContent-Disposition: attachment; filename=\"notes.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0x\r\n--b--\r\n"
        )
    }

    fn only_local(harness: &Harness) -> Draft {
        let drafts = harness.db().drafts().unwrap();
        assert_eq!(drafts.len(), 1, "expected exactly one local draft");
        drafts.into_iter().next().unwrap()
    }

    fn fetches(harness: &Harness) -> usize {
        harness.server.calls().iter().filter(|call| call.starts_with("fetch")).count()
    }

    #[test]
    fn only_message_ids_threestrands_wrote_name_a_local_draft() {
        let id = "8a7c4c4e-3d1f-4a43-9f3e-2b1d7c8e9f00";
        assert_eq!(draft_id_from_message_id(&format!("<draft-{id}@threestrands.local>")).as_deref(), Some(id));
        assert_eq!(draft_id_from_message_id(&format!("draft-{id}@example.org")).as_deref(), Some(id));
        assert_eq!(draft_id_from_message_id("draft-not-a-uuid@threestrands.local"), None);
        assert_eq!(draft_id_from_message_id("CAF1234@mail.gmail.com"), None);
        assert_eq!(draft_id_from_message_id(&format!("op-draft-{id}@threestrands.local")), None);
    }

    #[tokio::test]
    async fn a_draft_written_in_another_app_appears_with_its_headers_and_thread() {
        let harness = Harness::new();
        harness.server.write_elsewhere("g-1", &message("CAF1@mail.gmail.com", "Lunch?", "Are you free Friday?"), Some("t-9"));
        assert!(harness.pull().await);
        let draft = only_local(&harness);
        assert_eq!(draft.account, ACCOUNT);
        assert_eq!(draft.subject, "Lunch?");
        assert!(draft.body.contains("Are you free Friday?"));
        assert_eq!(draft.to, "\"Doe, Jane\" <jane@example.com>, sam@example.com");
        assert_eq!(draft.cc, "lee@example.com");
        assert_eq!(draft.bcc, "hidden@example.com");
        assert_eq!(draft.reply_id.as_deref(), Some("original@example.com"));
        assert_eq!(draft.references, ["root@example.com", "original@example.com"]);
        assert_eq!(draft.thread_id.as_deref(), Some("t-9"));
        assert_eq!(draft.mode, "reply");
        assert_eq!(harness.state(&draft), "saved");

        assert!(!harness.pull().await, "an unchanged copy is not fetched again");
        assert_eq!(fetches(&harness), 1);
    }

    #[tokio::test]
    async fn a_plain_draft_stays_plain_and_a_formatted_one_keeps_its_html() {
        let harness = Harness::new();
        harness.server.write_elsewhere("g-1", &message("CAF6@mail.gmail.com", "Plain", "Just text"), None);
        harness.server.write_elsewhere("g-2", &html_message("CAF7@mail.gmail.com"), None);
        harness.pull().await;
        let drafts = harness.db().drafts().unwrap();
        let plain = drafts.iter().find(|draft| draft.subject == "Plain").unwrap();
        assert_eq!(plain.body_html, "");
        let styled = drafts.iter().find(|draft| draft.subject == "Styled").unwrap();
        assert!(styled.body_html.contains("<b>there</b>"));
        assert!(styled.body.contains("Hello there"));
    }

    #[tokio::test]
    async fn attachments_on_a_server_draft_become_local_files() {
        let harness = Harness::new();
        harness.server.write_elsewhere("g-1", &with_attachment("CAF2@mail.gmail.com"), None);
        harness.pull().await;
        let draft = only_local(&harness);
        assert_eq!(draft.mode, "new");
        let [attachment] = draft.attachments.as_slice() else { panic!("expected one attachment") };
        assert_eq!((attachment.name.as_str(), attachment.mime.as_str(), attachment.ready), ("notes.pdf", "application/pdf", true));
        assert_eq!(std::fs::read(harness.root.0.join(&attachment.id)).unwrap(), b"%PDF-1");
    }

    #[tokio::test]
    async fn this_devices_own_pushes_are_not_imported_back() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "Mine");
        harness.run_after(PAST_DELAY).await;
        assert!(!harness.pull().await);
        assert_eq!(fetches(&harness), 0);
        assert_eq!(harness.db().draft(&draft.id).unwrap().revision, draft.revision);
    }

    #[tokio::test]
    async fn an_edit_made_elsewhere_replaces_a_draft_with_nothing_unsent() {
        let harness = Harness::new();
        let mut draft = harness.new_draft();
        draft.follow_up_task_id = Some("task-1".into());
        let draft = harness.edit(draft, "Mine");
        harness.run_after(PAST_DELAY).await;
        let message_id = format!("draft-{}@threestrands.local", draft.id);
        harness.server.write_elsewhere("server-1", &message(&message_id, "Plans", "Edited on my phone"), None);

        assert!(harness.pull().await);
        let pulled = harness.db().draft(&draft.id).unwrap();
        assert!(pulled.body.contains("Edited on my phone"));
        assert!(pulled.revision > draft.revision, "an open composer can tell the draft changed");
        assert_eq!(pulled.follow_up_task_id.as_deref(), Some("task-1"), "ThreeStrands-only fields survive");
        assert!(pulled.signature.is_none(), "the server copy's signature is part of its body");
        assert_eq!(harness.state(&pulled), "saved");
        harness.run_after(PAST_DELAY).await;
        assert!(!harness.server.calls().iter().any(|call| call.starts_with("update")), "nothing to push back");
    }

    #[tokio::test]
    async fn both_sides_changing_holds_the_server_version_for_the_user() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        let message_id = format!("draft-{}@threestrands.local", draft.id);
        harness.server.write_elsewhere("server-1", &message(&message_id, "Plans", "Edited on my phone"), None);
        let draft = harness.edit(draft, "Edited here, offline");

        assert!(harness.pull().await);
        assert_eq!(harness.state(&draft), "conflict");
        assert!(harness.db().draft(&draft.id).unwrap().body.contains("Edited here"), "this device's edits stay");
        harness.run_after(PAST_DELAY).await;
        assert!(harness.server.drafts()["server-1"].0.contains("Edited on my phone"), "nothing overwrites while the user decides");

        let kept = harness.db().resolve_draft_conflict(&draft.id, true).unwrap();
        assert!(kept.body.contains("Edited on my phone"));
        assert!(kept.revision > draft.revision);
        assert_eq!(kept.server_draft.unwrap().state, "saved");
        harness.run_after(PAST_DELAY).await;
        assert!(!harness.server.calls().iter().any(|call| call.starts_with("update")));
        assert!(harness.db().resolve_draft_conflict(&draft.id, true).is_err(), "nothing left to resolve");
    }

    #[tokio::test]
    async fn keeping_this_devices_version_pushes_it_over_the_other() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        let message_id = format!("draft-{}@threestrands.local", draft.id);
        harness.server.write_elsewhere("server-1", &message(&message_id, "Plans", "Edited on my phone"), None);
        let draft = harness.edit(draft, "Edited here, offline");
        harness.pull().await;

        let kept = harness.db().resolve_draft_conflict(&draft.id, false).unwrap();
        assert!(kept.body.contains("Edited here"));
        assert_eq!(kept.server_draft.unwrap().state, "pending");
        harness.run_after(0).await;
        assert!(harness.server.drafts()["server-1"].0.contains("Edited here"));
        assert_eq!(harness.state(&draft), "saved");
    }

    #[tokio::test]
    async fn a_push_checks_for_edits_made_since_the_last_pull() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        let message_id = format!("draft-{}@threestrands.local", draft.id);
        harness.server.write_elsewhere("server-1", &message(&message_id, "Plans", "Edited on my phone"), None);
        let draft = harness.edit(draft, "Two");
        harness.run_after(PAST_DELAY).await;
        assert!(harness.server.drafts()["server-1"].0.contains("Edited on my phone"), "the other edit isn't overwritten");
        assert_eq!(harness.state(&draft), "conflict");
        let events = harness.events.lock().unwrap().clone();
        assert!(events.iter().any(|event| matches!(event, DraftEvent::Status(status) if status.status.state == "conflict")));
    }

    #[tokio::test]
    async fn a_draft_deleted_elsewhere_leaves_unless_it_has_unsent_edits() {
        let harness = Harness::new();
        let kept = harness.edit(harness.new_draft(), "Settled");
        harness.run_after(PAST_DELAY).await;
        let editing = harness.edit(harness.new_draft(), "Still typing");
        harness.run_after(PAST_DELAY).await;
        let editing = harness.edit(editing, "Still typing, more");
        harness.server.drafts().clear();

        assert!(harness.pull().await);
        assert!(harness.db().draft(&kept.id).is_err(), "sent or discarded elsewhere");
        assert!(harness.db().draft(&editing.id).is_ok(), "unsent edits are never dropped");
        harness.run_after(0).await;
        assert!(harness.only_server_body().contains("Still typing, more"), "pushed again as a new copy");
    }

    #[tokio::test]
    async fn a_copy_of_a_draft_this_device_is_sending_is_not_imported() {
        let harness = Harness::new();
        let mut draft = harness.new_draft();
        draft.to = "jane@example.com".into();
        let draft = harness.edit(draft, "Sending");
        harness.run_after(PAST_DELAY).await;
        harness
            .db()
            .queue(&draft.id, draft.revision, false, &[], None, std::path::Path::new("/unused"))
            .unwrap();
        // As if this device had never learned the copy's server id.
        harness.db().forget_server_draft_link(&draft.id).unwrap();
        harness.pull().await;
        assert!(harness.db().drafts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_copy_this_device_wrote_before_learning_its_id_is_linked_not_duplicated() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        let message_id = format!("draft-{}@threestrands.local", draft.id);
        harness.server.write_elsewhere("g-5", &message(&message_id, "Plans", "One"), None);
        harness.run_after(PAST_DELAY).await;
        harness.pull().await;
        assert_eq!(harness.db().drafts().unwrap().len(), 1);
        assert_eq!(harness.server.drafts().len(), 1);
    }

    #[tokio::test]
    async fn one_unreadable_server_draft_does_not_stop_the_others() {
        let harness = Harness::new();
        let huge = message("CAF3@mail.gmail.com", "Huge", &"x".repeat(MAX_IMPORTED_BODY_BYTES + 1));
        harness.server.write_elsewhere("g-1", &huge, None);
        harness.server.write_elsewhere("g-2", &message("CAF4@mail.gmail.com", "Small", "Hi"), None);
        assert!(harness.pull().await);
        assert_eq!(only_local(&harness).subject, "Small");
    }

    #[tokio::test]
    async fn imported_bodies_at_the_limit_are_accepted() {
        let harness = Harness::new();
        let body = "x".repeat(MAX_IMPORTED_BODY_BYTES - 2);
        harness.server.write_elsewhere("g-1", &message("CAF5@mail.gmail.com", "Exact", &body), None);
        harness.pull().await;
        assert_eq!(only_local(&harness).subject, "Exact");
    }

    #[tokio::test]
    async fn replying_in_the_conversation_reopens_a_reply_written_elsewhere() {
        use base64::Engine;
        let harness = Harness::new();
        harness.server.write_elsewhere("g-1", &message("CAF8@mail.gmail.com", "Re: Topic", "Half a reply"), Some("t-9"));
        harness.pull().await;
        let imported = only_local(&harness);
        let source = serde_json::json!({"id":"source","threadId":"t-9","payload":{"mimeType":"text/plain","headers":[
            {"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},
            {"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<original@example.com>"}],
            "body":{"data":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("Hello")}}});
        harness.db().put_message_metadata("source", &source.to_string()).unwrap();

        let reopened = harness.db().create_draft("reply", Some("source".into()), ACCOUNT, &[]).unwrap();
        assert_eq!(reopened.id, imported.id);
        assert!(reopened.body.contains("Half a reply"));
        assert_eq!(reopened.source_id.as_deref(), Some("source"));
        assert_eq!(harness.state(&reopened), "saved", "linking the source changes nothing on the server");
        assert_eq!(harness.db().drafts().unwrap().len(), 1);
        let again = harness.db().create_draft("reply", Some("source".into()), ACCOUNT, &[]).unwrap();
        assert_eq!(again.id, imported.id);
    }

    #[tokio::test]
    async fn a_copy_replaced_by_another_device_follows_rather_than_leaving() {
        // An IMAP server stores every save as a new message with a new id.
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        let message_id = format!("draft-{}@threestrands.local", draft.id);
        harness.server.drafts().clear();
        harness.server.write_elsewhere("server-9", &message(&message_id, "Plans", "Edited on the laptop"), None);

        assert!(harness.pull().await);
        let pulled = harness.db().draft(&draft.id).expect("a replaced copy is not a deletion");
        assert!(pulled.body.contains("Edited on the laptop"));
        assert_eq!(harness.state(&pulled), "saved");
    }

    #[tokio::test]
    async fn a_push_over_a_copy_replaced_elsewhere_asks_first() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        let message_id = format!("draft-{}@threestrands.local", draft.id);
        harness.server.drafts().clear();
        harness.server.write_elsewhere("server-9", &message(&message_id, "Plans", "Edited on the laptop"), None);
        let draft = harness.edit(draft, "Edited here");
        harness.run_after(PAST_DELAY).await;
        assert_eq!(harness.state(&draft), "conflict");
        assert!(harness.server.drafts()["server-9"].0.contains("Edited on the laptop"));
        assert_eq!(harness.server.drafts().len(), 1, "no second copy");
    }

    #[tokio::test]
    async fn pulls_run_once_per_interval_unless_asked() {
        let harness = Harness::new();
        let mut last = HashMap::new();
        let at = now();
        harness.service.pull_due_accounts(at, &harness.providers(), &mut last, false).await.unwrap();
        harness.service.pull_due_accounts(at + DRAFT_PULL_INTERVAL_MS - 1, &harness.providers(), &mut last, false).await.unwrap();
        let lists = || harness.server.calls().iter().filter(|call| *call == "list").count();
        assert_eq!(lists(), 1);
        harness.service.pull_due_accounts(at + DRAFT_PULL_INTERVAL_MS, &harness.providers(), &mut last, false).await.unwrap();
        assert_eq!(lists(), 2);
        harness.service.pull_due_accounts(at + DRAFT_PULL_INTERVAL_MS, &harness.providers(), &mut last, true).await.unwrap();
        assert_eq!(lists(), 3, "focusing the window pulls right away");
    }
}
