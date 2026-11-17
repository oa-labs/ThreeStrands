//! Local drafts and drafts discarded within the restore window. Every write
//! is guarded by the revision the caller read, and an edit the server copy
//! needs marks its push pending in the same transaction.

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::clock::unix_millis;
use crate::correspondence::{Draft, DISCARDED_DRAFT_RETENTION_MS, DISCARD_SERVER_DELAY_MS};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

/// Where a follow-up draft handoff's one draft stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DraftHandoffState {
    /// In Drafts, to open or keep editing.
    Open,
    /// Queued or scheduled in Outbox, or failed there.
    Outbox,
    Sent,
    /// No longer on this device: discarded, or sent or deleted elsewhere.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftHandoff {
    pub handoff_id: String,
    pub draft_id: String,
    pub state: DraftHandoffState,
    /// The draft while it is in Drafts.
    pub draft: Option<Draft>,
    /// Whether this request created the draft.
    pub created: bool,
}

impl Database {
    pub fn drafts(&self) -> DbResult<Vec<Draft>> {
        self.with_connection(|c| {
            let mut q = c.prepare("SELECT payload FROM drafts ORDER BY rowid DESC")?;
            let rows = q.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| serde_json::from_str(&r?).map_err(serialization_error))
                .collect()
        })
    }
    pub fn draft(&self, id: &str) -> DbResult<Draft> {
        let value: String = self
            .with_connection(|c| {
                Ok(
                    c.query_row("SELECT payload FROM drafts WHERE id=?1", [id], |r| r.get(0))
                        .optional()?,
                )
            })?
            .ok_or_else(|| DatabaseError::invalid("Draft no longer exists"))?;
        serde_json::from_str(&value).map_err(serialization_error)
    }
    /// Discards a draft. It is held for [`DISCARDED_DRAFT_RETENTION_MS`] so
    /// [`restore_discarded_draft`](Self::restore_discarded_draft) can bring it
    /// back, and its server copy waits [`DISCARD_SERVER_DELAY_MS`] before it
    /// is deleted.
    pub fn discard_draft(&self, id: &str) -> DbResult<()> {
        let at = unix_millis();
        self.with_transaction(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO discarded_drafts(id, revision, payload, discarded_at)
                 SELECT id, revision, payload, ?2 FROM drafts WHERE id=?1",
                params![id, at],
            )?;
            tx.execute("DELETE FROM drafts WHERE id=?1", [id])?;
            Self::draft_left_drafts(tx, id, at + DISCARD_SERVER_DELAY_MS)?;
            Ok(())
        })
    }
    /// Brings back a draft discarded within [`DISCARDED_DRAFT_RETENTION_MS`].
    /// A server copy not yet deleted stays as it is; otherwise the draft is
    /// saved to the server again.
    pub fn restore_discarded_draft(&self, id: &str) -> DbResult<Draft> {
        self.with_transaction(|tx| {
            let held: Option<(i64, String)> = tx
                .query_row("SELECT revision, payload FROM discarded_drafts WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((revision, payload)) = held else {
                return Err(DatabaseError::invalid("This draft can no longer be restored"));
            };
            let mut draft: Draft = serde_json::from_str(&payload).map_err(serialization_error)?;
            draft.revision = revision + 1;
            draft.updated_at = unix_millis();
            tx.execute(
                "INSERT INTO drafts VALUES (?1,?2,?3)",
                params![draft.id, draft.revision, serde_json::to_string(&draft).map_err(serialization_error)?],
            )?;
            tx.execute("DELETE FROM discarded_drafts WHERE id=?1", [id])?;
            // Never delete the copy of a draft that came back.
            tx.execute(
                "DELETE FROM draft_server_deletions WHERE message_id=?1",
                [crate::draft_push::server_message_local_part(id)],
            )?;
            let in_sync = tx.execute(
                "UPDATE draft_server_links SET pushed_revision=?2, due_at=NULL, pending_since=NULL WHERE draft_id=?1 AND pushed_revision=?3 AND state='saved' AND server_id IS NOT NULL",
                params![id, draft.revision, revision],
            )?;
            if in_sync == 0 {
                Self::mark_draft_push_pending(tx, &draft, unix_millis())?;
            }
            Ok(draft)
        })
    }
    /// Forgets discarded drafts past [`DISCARDED_DRAFT_RETENTION_MS`].
    pub(crate) fn purge_discarded_drafts(&self, at: i64) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute(
                "DELETE FROM discarded_drafts WHERE discarded_at < ?1",
                [at - DISCARDED_DRAFT_RETENTION_MS],
            )?;
            Ok(())
        })
    }
    pub(crate) fn discarded_draft_attachments(&self) -> DbResult<Vec<String>> {
        self.with_connection(|c| {
            let mut q = c.prepare("SELECT payload FROM discarded_drafts")?;
            let rows = q.query_map([], |r| r.get::<_, String>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                let draft: Draft = serde_json::from_str(&row?).map_err(serialization_error)?;
                ids.extend(draft.attachments.into_iter().map(|a| a.id));
            }
            Ok(ids)
        })
    }
    /// Stores a newly created draft. Its server copy waits for the first edit.
    pub(crate) fn insert_draft(&self, d: &mut Draft) -> DbResult<()> {
        self.write_draft(d, false, |tx, d, payload| {
            tx.execute(
                "INSERT INTO drafts VALUES (?1,?2,?3)",
                params![d.id, d.revision, payload],
            )
        })?;
        Ok(())
    }
    /// Stores a follow-up draft as the one draft of `handoff_id`, in one
    /// transaction with the handoff's record, so a handoff never has two
    /// drafts. False, storing nothing, when the handoff already has its draft.
    /// The draft starts with content, so its server copy is due like an edit.
    pub(crate) fn insert_handoff_draft(&self, handoff_id: &str, d: &mut Draft) -> DbResult<bool> {
        let now = chrono::Utc::now();
        let changed = self.write_draft(d, true, |tx, d, payload| {
            let claimed = tx.execute(
                "INSERT OR IGNORE INTO draft_handoffs(handoff_id, draft_id, account, created_at) VALUES (?1,?2,?3,?4)",
                params![handoff_id, d.id, d.account, now.to_rfc3339()],
            )?;
            if claimed == 0 {
                return Ok(0);
            }
            let cutoff = now - chrono::Duration::days(crate::meeting_policy::DRAFT_HANDOFF_RETENTION_DAYS);
            tx.execute("DELETE FROM draft_handoffs WHERE created_at < ?1", [cutoff.to_rfc3339()])?;
            tx.execute(
                "DELETE FROM draft_handoffs WHERE handoff_id NOT IN
                   (SELECT handoff_id FROM draft_handoffs ORDER BY created_at DESC, rowid DESC LIMIT ?1)",
                [crate::meeting_policy::MAX_DRAFT_HANDOFFS as i64],
            )?;
            tx.execute(
                "INSERT INTO drafts VALUES (?1,?2,?3)",
                params![d.id, d.revision, payload],
            )
        })?;
        Ok(changed == 1)
    }
    /// Where the draft of `handoff_id` stands, or `None` for a handoff this
    /// device never opened (or has forgotten).
    pub fn draft_handoff_status(&self, handoff_id: &str) -> DbResult<Option<DraftHandoff>> {
        let found: Option<String> = self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT draft_id FROM draft_handoffs WHERE handoff_id=?1",
                [handoff_id],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        let Some(draft_id) = found else {
            return Ok(None);
        };
        let draft = self
            .with_connection(|c| {
                Ok(
                    c.query_row("SELECT payload FROM drafts WHERE id=?1", [&draft_id], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?,
                )
            })?
            .map(|payload| serde_json::from_str::<Draft>(&payload).map_err(serialization_error))
            .transpose()?;
        let state = match &draft {
            Some(_) => DraftHandoffState::Open,
            None => {
                let outbox: Option<String> = self.with_connection(|c| {
                    Ok(c.query_row(
                        "SELECT state FROM outbox_messages WHERE draft_id=?1 ORDER BY rowid DESC LIMIT 1",
                        [&draft_id],
                        |r| r.get(0),
                    )
                    .optional()?)
                })?;
                match outbox.as_deref() {
                    Some("sent") => DraftHandoffState::Sent,
                    Some(_) => DraftHandoffState::Outbox,
                    None => DraftHandoffState::Missing,
                }
            }
        };
        Ok(Some(DraftHandoff {
            handoff_id: handoff_id.to_string(),
            draft_id,
            state,
            draft,
            created: false,
        }))
    }
    /// Writes one draft row. `push` marks an edit that the account's server
    /// drafts need, in the same transaction. Returns the rows `write` changed.
    fn write_draft(
        &self,
        d: &mut Draft,
        push: bool,
        write: impl FnOnce(&rusqlite::Transaction, &Draft, &str) -> rusqlite::Result<usize>,
    ) -> DbResult<usize> {
        d.server_draft = None;
        let payload = serde_json::to_string(d).map_err(serialization_error)?;
        let draft = &*d;
        self.with_transaction(|tx| {
            let changed = write(tx, draft, &payload)?;
            if changed == 1 && push {
                Self::mark_draft_push_pending(tx, draft, unix_millis())?;
            }
            Ok(changed)
        })
    }
    /// For a change the server copy doesn't carry, such as which local
    /// message a reply answers. A server copy that was current stays current.
    pub(crate) fn update_draft_without_push(
        &self,
        d: &mut Draft,
        expected: i64,
    ) -> DbResult<usize> {
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
    pub(crate) fn update_draft(&self, d: &mut Draft, expected: i64) -> DbResult<usize> {
        self.write_draft(d, true, |tx, d, payload| {
            tx.execute(
                "UPDATE drafts SET revision=?1,payload=?2 WHERE id=?3 AND revision=?4",
                params![d.revision, payload, d.id, expected],
            )
        })
    }
}
