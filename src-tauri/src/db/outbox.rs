//! The send queue. Each state change is one statement guarded by the state it
//! leaves, so a concurrent transition is never clobbered, and a scheduled
//! send's report is recorded in the same transaction.

use super::{record_schedule_report, serialization_error, Database, DatabaseError, DbResult};
use crate::clock::unix_millis;
use crate::correspondence::{Draft, OutboxItem, SentCopyStatus};
use rusqlite::params;

/// A Sent-copy job that is due, with the message it saves.
pub(crate) struct SentCopyJob {
    pub(crate) id: String,
    pub(crate) account: String,
    pub(crate) raw: Vec<u8>,
    pub(crate) state: String,
    pub(crate) attempts: i64,
    pub(crate) append_attempted: bool,
}

impl Database {
    pub fn outbox(&self) -> DbResult<Vec<OutboxItem>> {
        self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT id,payload,state,deadline,error,provider_id,archive_on_send,schedule_json,last_attempt_at,sent_copy_state,sent_copy_error FROM outbox_messages ORDER BY rowid DESC",
            )?;
            let rows = q.query_map([], |r| {
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
            })?;
            rows.map(|r| {
                let (id, payload, state, deadline, error, provider_id, archive_on_send, schedule_json, last_attempt_at, sent_copy_state, sent_copy_error) =
                    r?;
                Ok(OutboxItem {
                    id,
                    draft: serde_json::from_str(&payload).map_err(serialization_error)?,
                    state,
                    deadline,
                    error,
                    provider_id,
                    last_attempt_at,
                    archive_on_send,
                    schedule: schedule_json.map(|s|serde_json::from_str(&s)).transpose().map_err(serialization_error)?,
                    // A row with no Sent-copy job (Gmail, legacy, or a server that
                    // saves its own copy) has a NULL sent_copy_state -> no status.
                    sent_copy: sent_copy_state.map(|state| SentCopyStatus { state, error: sent_copy_error }),
                })
            })
            .collect()
        })
    }
    pub(crate) fn archive_on_send(&self, id: &str) -> DbResult<bool> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT archive_on_send FROM outbox_messages WHERE id=?1",
                [id],
                |row| row.get(0),
            )?)
        })
    }
    /// The prepared MIME of a queued send.
    pub(crate) fn outbox_raw(&self, id: &str) -> DbResult<Vec<u8>> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT raw FROM outbox_messages WHERE id=?1",
                [id],
                |r| r.get(0),
            )?)
        })
    }
    pub(crate) fn has_scheduled_sends(&self) -> DbResult<bool> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM outbox_messages WHERE schedule_json IS NOT NULL)",
                [], |row| row.get::<_, bool>(0),
            )?)
        })
    }
    pub fn pending_undo(&self) -> DbResult<bool> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM outbox_messages WHERE state='undo_pending' AND deadline>?1",
                [unix_millis()],
                |r| r.get::<_, i64>(0),
            )? > 0)
        })
    }

    /// Moves draft `item.draft` at `revision` into the outbox as `item`, with
    /// its prepared MIME.
    pub(crate) fn insert_queued_send(&self, item: &OutboxItem, revision: i64, raw: &[u8]) -> DbResult<()> {
        let d = &item.draft;
        let payload = serde_json::to_string(d).map_err(serialization_error)?;
        self.with_transaction(|tx| {
            let removed = tx.execute(
                "DELETE FROM drafts WHERE id=?1 AND revision=?2",
                params![d.id, revision],
            )?;
            if removed != 1 {
                return Err(DatabaseError::invalid("Draft changed while preparing send"));
            }
            Self::draft_left_drafts(tx, &d.id, unix_millis())?;
            tx.execute("INSERT INTO outbox_messages(id,draft_id,revision,account,state,deadline,payload,raw,archive_on_send) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![item.id,d.id,revision,d.account,item.state,item.deadline,payload,raw,item.archive_on_send])?;
            Ok(())
        })
    }
    pub fn cancel_send(&self, id: &str, recover: bool) -> DbResult<Draft> {
        let item = self
            .outbox()?
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| DatabaseError::invalid("Outbox item not found"))?;
        let allowed = if recover {
            item.state == "failed"
        } else {
            ["undo_pending", "ready", "scheduled", "overdue"].contains(&item.state.as_str())
        };
        if !allowed {
            return Err(DatabaseError::invalid(
                "Delivery has started or is uncertain; it cannot be safely canceled or retried",
            ));
        }
        let mut d = item.draft;
        d.revision += 1;
        let payload = serde_json::to_string(&d).map_err(serialization_error)?;
        self.with_transaction(|tx| {
            let count = tx.execute(
                "UPDATE outbox_messages SET state='canceled' WHERE id=?1 AND state=?2",
                params![id, item.state],
            )?;
            if count != 1 {
                return Err(DatabaseError::invalid("Delivery has already started"));
            }
            tx.execute(
                "INSERT INTO drafts VALUES (?1,?2,?3)",
                params![d.id, d.revision, payload],
            )?;
            Self::mark_draft_push_pending(tx, &d, unix_millis())?;
            if let Some(schedule) = &item.schedule { record_schedule_report(tx,id,&schedule.report.owner_installation_id,unix_millis())?; }
            Ok(())
        })?;
        Ok(d)
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
    pub fn mark_unverifiable(&self, id: &str) -> DbResult<()> {
        let owner=self.outbox()?.into_iter().find(|o|o.id==id).and_then(|o|o.schedule).map(|s|s.report.owner_installation_id).unwrap_or_default();
        self.change_outbox("UPDATE outbox_messages SET state='unverifiable', error='This message may have been sent, but delivery could not be confirmed. Confirm with the recipient before sending it again.' WHERE id=?1 AND state='uncertain'",[id],id,&owner,unix_millis())?;
        Ok(())
    }
    /// Pauses undo-pending/ready outbox items for one account, so removing a
    /// connected account never pauses another account's in-flight sends.
    #[cfg(test)]
    pub fn pause_ready_sends_for(&self, account: &str) -> DbResult<()> {
        self.pause_ready_sends_for_owned(account,"")
    }
    pub fn pause_ready_sends_for_owned(&self, account: &str, owner: &str) -> DbResult<()> {
        let items=self.outbox()?;
        for item in items.into_iter().filter(|o|o.draft.account==account && ["undo_pending","ready","scheduled","overdue"].contains(&o.state.as_str())) {
            self.change_outbox("UPDATE outbox_messages SET state='failed',error='Account disconnected. Reconnect and restore this draft to send.' WHERE id=?1 AND state IN ('undo_pending','ready','scheduled','overdue')",[&item.id],&item.id,owner,unix_millis())?;
        }
        Ok(())
    }
    /// Fails every queued or scheduled send whose account is gone from the
    /// catalog, whatever its due time. Removing an account on another device
    /// purges it here without touching this device's outbox, and a send left
    /// waiting would otherwise go out unasked once the account is added again.
    pub fn fail_sends_for_removed_accounts(&self, owner: &str) -> DbResult<()> {
        let accounts: Vec<String> = self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT DISTINCT account FROM outbox_messages
                 WHERE state IN ('undo_pending','ready','scheduled','overdue')
                   AND lower(account) NOT IN (SELECT lower(email) FROM accounts)",
            )?;
            let rows = q.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })?;
        for account in accounts {
            self.pause_ready_sends_for_owned(&account, owner)?;
        }
        Ok(())
    }

    /// Records that a send left `uncertain` was found in the account's sent
    /// mail.
    pub(crate) fn mark_uncertain_send_sent(&self, id: &str, provider_id: &str, owner: &str, at: i64) -> DbResult<()> {
        self.change_outbox("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2 AND state='uncertain'",params![provider_id,id],id,owner,at)?;
        Ok(())
    }
    /// Notes on a waiting send that the account now signs in as someone else.
    pub(crate) fn pause_send_for_other_sender(&self, id: &str) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute("UPDATE outbox_messages SET error='Paused: reconnect the original sender account to continue.' WHERE id=?1 AND state IN ('undo_pending','ready')", [id])?;
            Ok(())
        })
    }
    /// Sets when an `uncertain` send is next checked against sent mail.
    pub(crate) fn defer_uncertain_send(&self, id: &str, deadline: i64) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute(
                "UPDATE outbox_messages SET deadline=?1 WHERE id=?2",
                params![deadline, id],
            )?;
            Ok(())
        })
    }
    /// Leaves a queued send queued with a plain-language note, stamping
    /// `last_attempt_at` (not `attempts` or `state`) so the retry gate
    /// rate-limits the next attempt.
    pub(crate) fn note_send_prepare_retry(&self, id: &str, message: &str, owner: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET error=?2,last_attempt_at=?3 WHERE id=?1 AND state IN ('undo_pending','ready')",
            params![id, message, at],
            id,
            owner,
            at,
        )?;
        Ok(())
    }
    /// Moves a queued send to `failed`, so the draft stays restorable.
    pub(crate) fn fail_queued_send(&self, id: &str, message: &str, owner: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET state='failed',error=?2 WHERE id=?1 AND state IN ('undo_pending','ready')",
            params![id, message],
            id,
            owner,
            at,
        )?;
        Ok(())
    }
    /// Claims a due send for one delivery attempt. Returns 1 when this call
    /// claimed it, 0 when the row moved on, is not yet due, is a scheduled
    /// send past `scheduled_floor`, or its schedule changed revision or owner.
    pub(crate) fn claim_due_send(
        &self,
        id: &str,
        at: i64,
        scheduled_floor: i64,
        report_revision: Option<i64>,
        owner: &str,
    ) -> DbResult<usize> {
        self.change_outbox("UPDATE outbox_messages SET state='sending',attempts=attempts+1,last_attempt_at=?2,error=NULL WHERE id=?1 AND state IN ('undo_pending','ready','scheduled') AND deadline<=?2 AND (state!='scheduled' OR deadline>=?3) AND (schedule_json IS NULL OR (json_extract(schedule_json,'$.report.reportRevision')=?4 AND json_extract(schedule_json,'$.report.ownerInstallationId')=?5))",params![id,at,scheduled_floor,report_revision,owner],id,owner,at)
    }
    /// Records a send the provider accepted. `sent_copy_next_at` seeds a
    /// Sent-copy job in the same statement, for a provider whose sent copy
    /// this device saves.
    pub(crate) fn record_send_accepted(
        &self,
        id: &str,
        provider_id: &str,
        sent_copy_next_at: Option<i64>,
        owner: &str,
        at: i64,
    ) -> DbResult<()> {
        match sent_copy_next_at {
            Some(next_at) => self.change_outbox(
                "UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL,\
                 sent_copy_state='pending',sent_copy_next_at=?3,sent_copy_attempts=0,\
                 sent_copy_append_attempted=0,sent_copy_error=NULL WHERE id=?2",
                params![provider_id, id, next_at],
                id,
                owner,
                at,
            )?,
            None => self.change_outbox("UPDATE outbox_messages SET state='sent',provider_id=?1,error=NULL WHERE id=?2",params![provider_id,id],id,owner,at)?,
        };
        Ok(())
    }
    /// Records a delivery attempt that failed (`definite`) or whose outcome
    /// is unknown.
    pub(crate) fn record_send_failed(&self, id: &str, definite: bool, message: &str, owner: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET state=?1,error=?2 WHERE id=?3",
            params![
                if definite { "failed" } else { "uncertain" },
                message,
                id
            ], id,owner,at,
        )?;
        Ok(())
    }

    /// Accounts with a Sent-copy job due at `at`.
    pub(crate) fn sent_copy_job_accounts(&self, at: i64) -> DbResult<Vec<String>> {
        self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT DISTINCT account FROM outbox_messages
                 WHERE sent_copy_state IN ('pending','verify') AND sent_copy_next_at <= ?1",
            )?;
            let rows = q.query_map([at], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
    /// Up to `limit` Sent-copy jobs due at `at` on `accounts`, oldest first.
    pub(crate) fn due_sent_copy_jobs(&self, at: i64, accounts: &[&str], limit: i64) -> DbResult<Vec<SentCopyJob>> {
        let available = serde_json::to_string(accounts).map_err(serialization_error)?;
        self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT id, account, raw, sent_copy_state, sent_copy_attempts, \
                 sent_copy_append_attempted FROM outbox_messages \
                 WHERE sent_copy_state IN ('pending','verify') AND sent_copy_next_at <= ?1 \
                 AND account IN (SELECT value FROM json_each(?3)) \
                 ORDER BY sent_copy_next_at ASC, rowid ASC LIMIT ?2",
            )?;
            let rows = q.query_map(params![at, limit, available], |r| {
                Ok(SentCopyJob {
                    id: r.get(0)?,
                    account: r.get(1)?,
                    raw: r.get(2)?,
                    state: r.get(3)?,
                    attempts: r.get(4)?,
                    append_attempted: r.get::<_, i64>(5)? != 0,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
    /// Write-ahead flag set before a Sent-copy append, so a crash mid-append
    /// is remembered.
    pub(crate) fn mark_sent_copy_append_attempted(&self, id: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET sent_copy_append_attempted=1 WHERE id=?1 AND sent_copy_state='pending'",
            params![id],
            id,
            "",
            at,
        )?;
        Ok(())
    }
    pub(crate) fn complete_sent_copy_job(&self, id: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET sent_copy_state='done',sent_copy_error=NULL \
             WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
            params![id],
            id,
            "",
            at,
        )?;
        Ok(())
    }
    /// Moves a Sent-copy job to `verify`, checked again at `next_at`.
    pub(crate) fn recheck_sent_copy_job(&self, id: &str, next_at: i64, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET sent_copy_state='verify',sent_copy_next_at=?2,\
             sent_copy_error=NULL WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
            params![id, next_at],
            id,
            "",
            at,
        )?;
        Ok(())
    }
    /// Gives up on a Sent-copy job after a permanent failure.
    pub(crate) fn abandon_sent_copy_job(&self, id: &str, message: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET sent_copy_state='abandoned',sent_copy_error=?2 \
             WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
            params![id, message],
            id,
            "",
            at,
        )?;
        Ok(())
    }
    /// Gives up on a Sent-copy job that used its last attempt.
    pub(crate) fn abandon_sent_copy_job_after_attempts(&self, id: &str, attempts: i64, message: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET sent_copy_state='abandoned',\
             sent_copy_attempts=?2,sent_copy_error=?3 \
             WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
            params![id, attempts, message],
            id,
            "",
            at,
        )?;
        Ok(())
    }
    /// Counts a transient Sent-copy failure and retries at `next_at`,
    /// keeping the job's state.
    pub(crate) fn retry_sent_copy_job(&self, id: &str, attempts: i64, next_at: i64, message: &str, at: i64) -> DbResult<()> {
        self.change_outbox(
            "UPDATE outbox_messages SET sent_copy_attempts=?2,sent_copy_next_at=?3,\
             sent_copy_error=?4 WHERE id=?1 AND sent_copy_state IN ('pending','verify')",
            params![id, attempts, next_at, message],
            id,
            "",
            at,
        )?;
        Ok(())
    }
}
