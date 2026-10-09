//! Account-scoped provider mail sync progress, recovery, and error reports.
//! Replicated application-state sync lives in crate::sync_state, separately.

use super::{Database, DbResult};
use crate::models::{FailedMutation, QuarantinedMessage, SyncStatus};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};

/// Resume point for the sent-mail backfill: a provider search page token
/// (`None` for the first page), how many of that page's results were already
/// handled, and how many results all earlier pages held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentBackfillProgress {
    pub page: Option<String>,
    pub offset: usize,
    pub scanned: usize,
}

impl Database {
    pub fn sync_status(&self, account_id: &str) -> DbResult<SyncStatus> {
        self.with_connection(|connection| {
            let (cursor, last_successful_sync, mut error): (
                Option<String>,
                Option<String>,
                Option<String>,
            ) = connection
                .query_row(
                    "SELECT cursor, last_successful_sync, last_error
                     FROM sync_state WHERE account_id = ?1",
                    [account_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            if error.is_none() {
                error = connection
                    .query_row(
                        "SELECT last_error FROM mutations
                         WHERE state = 'failed' AND account_id = ?1 ORDER BY created_at DESC LIMIT 1",
                        [account_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
            }
            let pending_mutations = connection
                .query_row(
                    "SELECT count(*) FROM mutations WHERE state IN ('pending', 'running') AND account_id = ?1",
                    [account_id],
                    |row| row.get(0),
                )?;
            let failed_mutations = {
                let mut statement = connection
                    .prepare(
                        "SELECT id, kind, thread_id, attempts,
                                COALESCE(last_error, 'Unknown failure'), created_at
                         FROM mutations
                         WHERE state = 'failed' AND account_id = ?1
                         ORDER BY created_at DESC",
                    )?;
                let failed = statement
                    .query_map([account_id], |row| {
                        Ok(FailedMutation {
                            id: row.get(0)?,
                            kind: row.get(1)?,
                            thread_id: row.get(2)?,
                            attempts: row.get(3)?,
                            error: row.get(4)?,
                            created_at: row.get(5)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                failed
            };
            let quarantined_messages = {
                let mut statement = connection
                    .prepare(
                        "SELECT message_id, provider_thread_id, error, created_at
                         FROM quarantined_messages
                         WHERE account_id = ?1
                         ORDER BY created_at DESC, message_id
                         LIMIT 100",
                    )?;
                let quarantined = statement
                    .query_map([account_id], |row| {
                        Ok(QuarantinedMessage {
                            message_id: row.get(0)?,
                            thread_id: row.get(1)?,
                            error: row.get(2)?,
                            created_at: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                quarantined
            };
            Ok(SyncStatus {
                state: if error.is_some() { "error" } else { "idle" },
                last_successful_sync,
                cursor,
                pending_mutations,
                failed_mutations,
                quarantined_messages,
                error,
            })
        })
    }

    pub fn cursor(&self, account_id: &str) -> DbResult<Option<String>> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT cursor FROM sync_state WHERE account_id = ?1",
                [account_id],
                |row| row.get(0),
            )?)
        })
    }

    pub fn finish_sync(&self, account_id: &str, cursor: &str) -> DbResult<()> {
        let now = Utc::now().to_rfc3339();
        self.with_transaction(|transaction| {
            transaction.execute(
                "UPDATE sync_state SET cursor = ?1, last_successful_sync = ?2, last_error = NULL
                     WHERE account_id = ?3",
                params![cursor, now, account_id],
            )?;
            transaction.execute(
                "UPDATE accounts SET last_synced_at = ?1 WHERE email = ?2",
                params![now, account_id],
            )?;
            transaction.execute(
                "DELETE FROM sync_recovery_threads WHERE account_id = ?1",
                [account_id],
            )?;
            transaction.execute(
                "DELETE FROM sync_recovery WHERE account_id = ?1",
                [account_id],
            )?;
            Ok(())
        })
    }

    pub fn fail_sync(&self, account_id: &str, error: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_state SET last_error = ?1 WHERE account_id = ?2",
                params![error, account_id],
            )?;
            Ok(())
        })
    }

    /// Drops `account_id`'s history cursor ahead of a full inbox resync.
    ///
    /// This intentionally never deletes cached threads: a thread the user has
    /// archived or trashed locally is, by definition, no longer visible in
    /// Gmail's live INBOX listing, so recovery refreshes the current and locally
    /// cached inbox union without wiping unrelated archived mail.
    ///
    /// An interrupted full resync remains in recovery. Keeping the old normal
    /// cursor here would make the next startup perform an incremental sync
    /// against a snapshot that never finished.
    pub fn clear_cursor(&self, account_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_state SET cursor = NULL WHERE account_id = ?1",
                [account_id],
            )?;
            Ok(())
        })
    }

    pub fn recovery_cursor(&self, account_id: &str) -> DbResult<Option<String>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT history_id FROM sync_recovery WHERE account_id = ?1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    pub fn begin_sync_recovery(
        &self,
        account_id: &str,
        history_id: &str,
        thread_ids: &[String],
    ) -> DbResult<()> {
        self.with_transaction(|transaction| {
            transaction.execute(
                "DELETE FROM sync_recovery_threads WHERE account_id = ?1",
                [account_id],
            )?;
            transaction.execute(
                "INSERT INTO sync_recovery(account_id, history_id) VALUES (?1, ?2)
                     ON CONFLICT(account_id) DO UPDATE SET history_id = excluded.history_id",
                params![account_id, history_id],
            )?;
            {
                let mut statement = transaction.prepare(
                    "INSERT INTO sync_recovery_threads(account_id, provider_thread_id)
                         VALUES (?1, ?2)",
                )?;
                for thread_id in thread_ids {
                    statement.execute(params![account_id, thread_id])?;
                }
            }
            Ok(())
        })
    }

    pub fn pending_sync_recovery_threads(
        &self,
        account_id: &str,
        limit: usize,
    ) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT provider_thread_id FROM sync_recovery_threads
                     WHERE account_id = ?1 ORDER BY provider_thread_id LIMIT ?2",
            )?;
            let ids = statement
                .query_map(params![account_id, limit as i64], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

    pub fn complete_sync_recovery_threads(
        &self,
        account_id: &str,
        thread_ids: &[String],
    ) -> DbResult<()> {
        self.with_transaction(|transaction| {
            {
                let mut statement = transaction.prepare(
                    "DELETE FROM sync_recovery_threads
                         WHERE account_id = ?1 AND provider_thread_id = ?2",
                )?;
                for thread_id in thread_ids {
                    statement.execute(params![account_id, thread_id])?;
                }
            }
            Ok(())
        })
    }

    pub fn discard_sync_recovery(&self, account_id: &str) -> DbResult<()> {
        self.with_transaction(|transaction| {
            transaction.execute(
                "DELETE FROM sync_recovery_threads WHERE account_id = ?1",
                [account_id],
            )?;
            transaction.execute(
                "DELETE FROM sync_recovery WHERE account_id = ?1",
                [account_id],
            )?;
            Ok(())
        })
    }

    /// Whether it's been at least `interval_secs` since this account's last
    /// inbox reconciliation pass (or one has never run).
    pub fn reconciliation_due(&self, account_id: &str, interval_secs: i64) -> DbResult<bool> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT last_reconciled_at IS NULL
                        OR (strftime('%s', 'now') - strftime('%s', last_reconciled_at)) >= ?2
                     FROM sync_state WHERE account_id = ?1",
                params![account_id, interval_secs],
                |row| row.get(0),
            )?)
        })
    }

    pub fn mark_reconciled(&self, account_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_state SET last_reconciled_at = ?1 WHERE account_id = ?2",
                params![Utc::now().to_rfc3339(), account_id],
            )?;
            Ok(())
        })
    }

    /// Where the sent-mail backfill should resume, or `None` once it has
    /// finished (or the account has no sync state yet).
    pub fn sent_backfill_progress(
        &self,
        account_id: &str,
    ) -> DbResult<Option<SentBackfillProgress>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT sent_backfill_page, sent_backfill_offset, sent_backfill_scanned
                     FROM sync_state
                     WHERE account_id = ?1 AND sent_backfill_completed_at IS NULL",
                    [account_id],
                    |row| {
                        Ok(SentBackfillProgress {
                            page: row.get(0)?,
                            offset: row.get::<_, i64>(1)?.max(0) as usize,
                            scanned: row.get::<_, i64>(2)?.max(0) as usize,
                        })
                    },
                )
                .optional()?)
        })
    }

    /// Persists the next resume point, or marks the backfill finished.
    pub fn record_sent_backfill(
        &self,
        account_id: &str,
        next: Option<&SentBackfillProgress>,
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            match next {
                Some(progress) => connection.execute(
                    "UPDATE sync_state SET sent_backfill_page = ?1, sent_backfill_offset = ?2,
                        sent_backfill_scanned = ?3
                     WHERE account_id = ?4",
                    params![
                        progress.page,
                        progress.offset as i64,
                        progress.scanned as i64,
                        account_id
                    ],
                )?,
                None => connection.execute(
                    "UPDATE sync_state SET sent_backfill_page = NULL, sent_backfill_offset = 0,
                        sent_backfill_completed_at = ?1
                     WHERE account_id = ?2",
                    params![Utc::now().to_rfc3339(), account_id],
                )?,
            };
            Ok(())
        })
    }

    /// Gmail thread ids this account currently caches as inbox mail (not
    /// archived), for diffing against Gmail's live INBOX listing.
    pub fn local_inbox_provider_thread_ids(&self, account_id: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT provider_thread_id FROM threads WHERE account_id = ?1 AND archived = 0",
            )?;
            let ids = statement
                .query_map([account_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

    /// Gmail thread ids already present in the local cache for one account.
    /// Remote search uses this to fetch only historical matches that the
    /// inbox-oriented synchronizer has never seen.
    pub fn local_provider_thread_ids(&self, account_id: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT provider_thread_id FROM threads WHERE account_id = ?1")?;
            let ids = statement
                .query_map([account_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

    /// Forgets failed mutations and quarantine records once the user has seen
    /// them. Both are reports only: a quarantined message is reingested (and
    /// its record replaced) whenever its thread syncs again, and a failed
    /// mutation is never retried on its own.
    pub fn dismiss_sync_problems(&self) -> DbResult<()> {
        self.with_transaction(|transaction| {
            transaction.execute("DELETE FROM mutations WHERE state = 'failed'", [])?;
            transaction.execute("DELETE FROM quarantined_messages", [])?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "tests/mail_sync.rs"]
mod tests;
