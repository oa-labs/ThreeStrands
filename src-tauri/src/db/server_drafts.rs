//! Links between local drafts and their copies in the account's server
//! drafts (`docs/server-drafts-design.md`): pending pushes, queued server
//! deletions, and pulled versions held back as conflicts.

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::clock::unix_millis;
use crate::correspondence::Draft;
use crate::draft_pull::Applied;
use crate::draft_push::{
    kind_keeps_server_drafts, server_message_local_part, Deletion, Link, RemovedDraft,
    ServerDraftStatus, DRAFT_PUSH_DELAY_MS, DRAFT_PUSH_MAX_WAIT_MS,
};
use crate::models::MailProviderKind;
use crate::provider::ServerDraft;
use rusqlite::{params, OptionalExtension, Transaction};

/// Outbox states after which the draft's server copy is no longer needed.
const SENT_STATES: [&str; 2] = ["sent", "unverifiable"];

impl Database {
    pub(crate) fn keeps_server_drafts(tx: &Transaction, account: &str) -> DbResult<bool> {
        let provider: Option<String> = tx
            .query_row(
                "SELECT provider FROM accounts WHERE email=?1 AND on_this_device=1",
                [account],
                |row| row.get(0),
            )
            .optional()?;
        Ok(provider
            .as_deref()
            .and_then(MailProviderKind::parse)
            .is_some_and(kind_keeps_server_drafts))
    }

    /// Records that `draft` changed and its server copy needs the new
    /// revision. Runs in the transaction that saved the draft. A draft that
    /// moved to another account queues its old account's copy for deletion.
    pub(crate) fn mark_draft_push_pending(tx: &Transaction, draft: &Draft, at: i64) -> DbResult<()> {
        let existing: Option<(String, Option<String>, Option<i64>, String)> = tx
            .query_row(
                "SELECT account, server_id, pending_since, state FROM draft_server_links WHERE draft_id=?1",
                [&draft.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((account, server_id, _, _)) = &existing {
            if *account != draft.account {
                Self::queue_server_draft_deletion(tx, account, server_id.as_deref(), &draft.id, at)?;
                tx.execute("DELETE FROM draft_server_links WHERE draft_id=?1", [&draft.id])?;
            }
        }
        if !Self::keeps_server_drafts(tx, &draft.account)? {
            return Ok(());
        }
        let existing = existing.filter(|(account, ..)| *account == draft.account);
        let pending_since = existing.as_ref().and_then(|(_, _, since, _)| *since).unwrap_or(at);
        let due = (at + DRAFT_PUSH_DELAY_MS).min(pending_since + DRAFT_PUSH_MAX_WAIT_MS);
        match existing {
            // A temporary failure keeps its backoff; editing doesn't hammer the server.
            Some((_, _, _, state)) if state == "retrying" => {
                tx.execute(
                    "UPDATE draft_server_links SET pending_since=?2, due_at=MAX(COALESCE(due_at, ?3), ?3) WHERE draft_id=?1",
                    params![draft.id, pending_since, due],
                )?;
            }
            Some(_) => {
                tx.execute(
                    "UPDATE draft_server_links SET pending_since=?2, due_at=?3, state='pending', error=NULL, attempts=0 WHERE draft_id=?1",
                    params![draft.id, pending_since, due],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO draft_server_links(draft_id, account, pending_since, due_at, state) VALUES (?1, ?2, ?3, ?4, 'pending')",
                    params![draft.id, draft.account, pending_since, due],
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn queue_server_draft_deletion(
        tx: &Transaction,
        account: &str,
        server_id: Option<&str>,
        draft_id: &str,
        at: i64,
    ) -> DbResult<()> {
        tx.execute(
            "INSERT INTO draft_server_deletions(account, server_id, message_id, due_at) VALUES (?1, ?2, ?3, ?4)",
            params![account, server_id, server_message_local_part(draft_id), at],
        )?;
        Ok(())
    }

    /// Makes the worker look at `draft_id`'s server copy now, because the
    /// draft left the drafts table: it was sent, scheduled, or discarded.
    pub(crate) fn draft_left_drafts(tx: &Transaction, draft_id: &str, at: i64) -> DbResult<()> {
        tx.execute("UPDATE draft_server_links SET due_at=?2 WHERE draft_id=?1", params![draft_id, at])?;
        Ok(())
    }

    /// Forgets the server link without touching the server. For a draft that
    /// another device removed: that device deletes the server copy.
    pub(crate) fn forget_server_draft_link(&self, draft_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM draft_server_links WHERE draft_id=?1", [draft_id])?;
            Ok(())
        })
    }

    /// The composer's view of `draft`'s server copy, or `None` when the
    /// draft stays on this device.
    pub(crate) fn server_draft_status(&self, draft: &Draft) -> DbResult<Option<ServerDraftStatus>> {
        self.with_connection(|connection| {
            let row: Option<(i64, String, Option<String>, bool)> = connection
                .query_row(
                    "SELECT pushed_revision, state, error, conflict IS NOT NULL FROM draft_server_links WHERE draft_id=?1 AND account=?2",
                    params![draft.id, draft.account],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            Ok(row.map(|(pushed, state, error, conflict)| {
                if conflict {
                    ServerDraftStatus { state: "conflict".into(), error: None }
                } else if pushed >= draft.revision {
                    ServerDraftStatus { state: "saved".into(), error: None }
                } else if state == "saved" {
                    ServerDraftStatus { state: "pending".into(), error: None }
                } else {
                    ServerDraftStatus { state, error }
                }
            }))
        })
    }

    /// Makes every pending push of `draft_id` (or of every draft) due now.
    pub(crate) fn push_drafts_now(&self, draft_id: Option<&str>, at: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE draft_server_links SET due_at=?1 WHERE state='pending' AND due_at IS NOT NULL AND (?2 IS NULL OR draft_id=?2)",
                params![at, draft_id],
            )?;
            Ok(())
        })
    }

    pub(crate) fn due_draft_accounts(&self, at: i64) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT account FROM draft_server_links WHERE due_at IS NOT NULL AND due_at<=?1
                 UNION SELECT account FROM draft_server_deletions WHERE due_at<=?1",
            )?;
            let rows = query.query_map([at], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn due_draft_links(&self, at: i64) -> DbResult<Vec<Link>> {
        self.with_connection(|connection| {
            // A conflict waits for the user, not the clock.
            let mut query = connection.prepare(
                "SELECT draft_id, account, server_id, server_version, pushed_revision, attempts FROM draft_server_links WHERE due_at IS NOT NULL AND due_at<=?1 AND conflict IS NULL ORDER BY due_at",
            )?;
            let rows = query.query_map([at], link_from_row)?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn account_draft_links(&self, account: &str) -> DbResult<Vec<Link>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT draft_id, account, server_id, server_version, pushed_revision, attempts FROM draft_server_links WHERE account=?1",
            )?;
            let rows = query.query_map([account], link_from_row)?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn due_draft_deletions(&self, at: i64) -> DbResult<Vec<Deletion>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT id, account, server_id, message_id, attempts FROM draft_server_deletions WHERE due_at<=?1 ORDER BY id",
            )?;
            let rows = query.query_map([at], |row| {
                Ok(Deletion {
                    id: row.get(0)?,
                    account: row.get(1)?,
                    server_id: row.get(2)?,
                    message_id: row.get(3)?,
                    attempts: row.get(4)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn local_draft(&self, id: &str) -> DbResult<Option<Draft>> {
        self.with_connection(|connection| {
            let payload: Option<String> = connection
                .query_row("SELECT payload FROM drafts WHERE id=?1", [id], |row| row.get(0))
                .optional()?;
            payload
                .map(|payload| serde_json::from_str(&payload).map_err(serialization_error))
                .transpose()
        })
    }

    /// What should happen to the server copy of a draft no longer in the
    /// local drafts table.
    pub(crate) fn removed_draft_outcome(&self, draft_id: &str) -> DbResult<RemovedDraft> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT state FROM outbox_messages WHERE draft_id=?1 AND state<>'canceled'",
            )?;
            let states = query
                .query_map([draft_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(if states.iter().any(|state| SENT_STATES.contains(&state.as_str())) {
                RemovedDraft::Delete
            } else if states.is_empty() {
                // Discarded.
                RemovedDraft::Delete
            } else {
                // Waiting to send, sending, or failed and restorable: the
                // server copy stays so Undo and Restore bring the draft back.
                RemovedDraft::Wait
            })
        })
    }

    pub(crate) fn retire_draft_link(&self, link: &Link, at: i64) -> DbResult<()> {
        self.with_transaction(|tx| {
            Self::queue_server_draft_deletion(tx, &link.account, link.server_id.as_deref(), &link.draft_id, at)?;
            tx.execute("DELETE FROM draft_server_links WHERE draft_id=?1", [&link.draft_id])?;
            Ok(())
        })
    }

    pub(crate) fn defer_draft_link(&self, draft_id: &str, due: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("UPDATE draft_server_links SET due_at=?2 WHERE draft_id=?1", params![draft_id, due])?;
            Ok(())
        })
    }

    /// Records a confirmed push of `revision`. The link stays pending when
    /// the draft changed again meanwhile.
    pub(crate) fn record_draft_pushed(&self, draft_id: &str, server: &ServerDraft, revision: i64) -> DbResult<i64> {
        self.with_transaction(|tx| {
            let current: Option<i64> = tx
                .query_row("SELECT revision FROM drafts WHERE id=?1", [draft_id], |row| row.get(0))
                .optional()?;
            let newer = current.is_some_and(|current| current > revision);
            tx.execute(
                "UPDATE draft_server_links SET server_id=?2, server_version=COALESCE(?5, server_version), pushed_revision=MAX(pushed_revision, ?3), attempts=0, error=NULL,
                     state=CASE WHEN ?4 THEN 'pending' ELSE 'saved' END,
                     pending_since=CASE WHEN ?4 THEN pending_since ELSE NULL END,
                     due_at=CASE WHEN ?4 THEN due_at ELSE NULL END
                 WHERE draft_id=?1",
                params![draft_id, server.id, revision, newer, server.version],
            )?;
            Ok(current.unwrap_or(revision))
        })
    }

    pub(crate) fn record_draft_push_failure(
        &self,
        draft_id: &str,
        state: &str,
        error: &str,
        due: Option<i64>,
        attempts: u32,
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE draft_server_links SET state=?2, error=?3, due_at=?4, attempts=?5 WHERE draft_id=?1",
                params![draft_id, state, error, due, attempts],
            )?;
            Ok(())
        })
    }

    pub(crate) fn finish_draft_deletion(&self, id: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM draft_server_deletions WHERE id=?1", [id])?;
            Ok(())
        })
    }

    pub(crate) fn defer_draft_deletion(&self, id: i64, attempts: u32, due: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE draft_server_deletions SET attempts=?2, due_at=?3 WHERE id=?1",
                params![id, attempts, due],
            )?;
            Ok(())
        })
    }
}

fn link_from_row(row: &rusqlite::Row) -> rusqlite::Result<Link> {
    Ok(Link {
        draft_id: row.get(0)?,
        account: row.get(1)?,
        server_id: row.get(2)?,
        server_version: row.get(3)?,
        pushed_revision: row.get(4)?,
        attempts: row.get(5)?,
    })
}

impl Database {
    /// Stores `imported`, the server's version of a draft, unless this
    /// device has edits the server hasn't seen: then the server's version is
    /// held back for the user to choose. Checked and written in one
    /// transaction, so a save racing the pull is never overwritten.
    pub(crate) fn apply_server_draft(&self, imported: &Draft, server: &ServerDraft) -> DbResult<Applied> {
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

    pub(crate) fn draft_is_discarded(&self, draft_id: &str) -> DbResult<bool> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM discarded_drafts WHERE id=?1)",
                [draft_id],
                |row| row.get(0),
            )?)
        })
    }

    /// Links an existing local draft to the server copy carrying its
    /// `Message-ID`, without changing either.
    pub(crate) fn adopt_server_copy(&self, draft_id: &str, server_id: &str) -> DbResult<()> {
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
    pub(crate) fn draft_left_this_device(&self, draft_id: &str) -> DbResult<bool> {
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
    pub(crate) fn server_copy_deleted(&self, link: &Link) -> DbResult<bool> {
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
                        params![link.draft_id, unix_millis()],
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
    pub(crate) fn resolve_draft_conflict(&self, id: &str, keep_server: bool) -> DbResult<Draft> {
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
                    params![id, unix_millis()],
                )?;
                return serde_json::from_str::<Draft>(&payload).map_err(serialization_error);
            }
            let mut server: Draft = serde_json::from_str(&held).map_err(serialization_error)?;
            server.revision = revision + 1;
            server.updated_at = unix_millis();
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
        let mut draft = result?;
        draft.server_draft = self.server_draft_status(&draft)?;
        Ok(draft)
    }
}
