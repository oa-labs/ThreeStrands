//! Thread persistence.

use super::messages::resolve_body;
use super::split_inboxes::split_inbox_matches;
use super::{decode_json, Database, DbResult};
use crate::mime::{readable_body_text, UnsubscribeMetadata};
use crate::models::{MailboxUnreadCounts, Message, Thread, ThreadDetail, ThreadPage};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;

pub(super) const THREAD_COLUMNS: &str = "t.id, t.provider_thread_id, t.subject, t.snippet,
    t.participants_json, t.last_message_at, t.unread, t.starred, t.archived,
    t.labels_json, t.trashed, t.account_id, t.summary, t.summary_generated_at,
    t.has_attachments, t.last_received_at, t.summary_revision";

impl Database {
    /// `account_id` merges every account when absent, or scopes to one account.
    pub fn list_threads(&self, account_id: Option<&str>) -> DbResult<Vec<Thread>> {
        self.list_threads_where(account_id, "archived = 0 AND trashed = 0")
    }

    pub fn list_all_mail(&self, account_id: Option<&str>) -> DbResult<Vec<Thread>> {
        self.list_threads_where(account_id, "trashed = 0")
    }

    pub fn list_trash(&self, account_id: Option<&str>) -> DbResult<Vec<Thread>> {
        self.list_threads_where(account_id, "trashed = 1")
    }

    pub fn count_unread_inbox(&self) -> DbResult<i64> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM threads WHERE archived = 0 AND trashed = 0 AND unread = 1",
                [],
                |row| row.get(0),
            )?)
        })
    }

    pub fn list_unread_counts(&self) -> DbResult<HashMap<String, i64>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT account_id, COUNT(*)
                 FROM threads
                 WHERE archived = 0 AND trashed = 0 AND unread = 1
                 GROUP BY account_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
        })
    }

    /// The Inbox is unarchived/untrashed threads *minus* anything claimed by
    /// a split inbox rule — a split inbox is meant to pull its matches out
    /// of the Inbox, not just mirror them into a second view. Split inbox
    /// rule sets are realistically small, so filtering the base set in
    /// memory (rather than the SQL-level pagination `list_threads_page_where`
    /// uses) is simpler than pushing the rule matching into SQL.
    pub fn list_threads_page(
        &self,
        account_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> DbResult<ThreadPage> {
        let rules = self.list_split_inboxes()?;
        if rules.is_empty() {
            return self.list_threads_page_where(
                account_id,
                "archived = 0 AND trashed = 0",
                offset,
                limit,
            );
        }
        let matched: Vec<Thread> = self
            .list_threads(account_id)?
            .into_iter()
            .filter(|thread| {
                !rules.iter().any(|rule| {
                    rule.account_id == thread.account_id && split_inbox_matches(rule, thread)
                })
            })
            .collect();
        let page_limit = limit.min(200);
        let has_more = matched.len() > offset.saturating_add(page_limit);
        let threads = matched.into_iter().skip(offset).take(page_limit).collect();
        Ok(ThreadPage { threads, has_more })
    }

    pub fn list_all_mail_page(
        &self,
        account_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> DbResult<ThreadPage> {
        self.list_threads_page_where(account_id, "trashed = 0", offset, limit)
    }

    pub fn list_trash_page(
        &self,
        account_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> DbResult<ThreadPage> {
        self.list_threads_page_where(account_id, "trashed = 1", offset, limit)
    }

    fn list_threads_where(&self, account_id: Option<&str>, filter: &str) -> DbResult<Vec<Thread>> {
        self.with_connection(|connection| {
            let sql = format!(
                "SELECT {THREAD_COLUMNS}
                 FROM threads AS t
                 WHERE {filter} {}
                 ORDER BY last_received_at DESC",
                if account_id.is_some() {
                    "AND account_id = ?1"
                } else {
                    ""
                }
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = match account_id {
                Some(id) => statement.query_map([id], thread_from_row)?,
                None => statement.query_map([], thread_from_row)?,
            };
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    fn list_threads_page_where(
        &self,
        account_id: Option<&str>,
        filter: &str,
        offset: usize,
        limit: usize,
    ) -> DbResult<ThreadPage> {
        self.with_connection(|connection| {
            let page_limit = limit.min(200);
            let fetch_limit = page_limit.saturating_add(1) as i64;
            let sql = format!(
                "SELECT {THREAD_COLUMNS}
                 FROM threads AS t
                 WHERE {filter} {}
                 ORDER BY last_received_at DESC
                 LIMIT ?{} OFFSET ?{}",
                if account_id.is_some() {
                    "AND account_id = ?1"
                } else {
                    ""
                },
                if account_id.is_some() { 2 } else { 1 },
                if account_id.is_some() { 3 } else { 2 },
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = match account_id {
                Some(id) => {
                    statement.query_map(params![id, fetch_limit, offset as i64], thread_from_row)?
                }
                None => {
                    statement.query_map(params![fetch_limit, offset as i64], thread_from_row)?
                }
            };
            let mut threads = rows.collect::<Result<Vec<_>, _>>()?;
            let has_more = threads.len() > page_limit;
            threads.truncate(page_limit);
            Ok(ThreadPage { threads, has_more })
        })
    }

    pub fn get_thread(&self, id: &str) -> DbResult<ThreadDetail> {
        self.with_connection(|connection| {
            let thread = connection
                .query_row(
                    &format!("SELECT {THREAD_COLUMNS} FROM threads AS t WHERE t.id = ?1"),
                    [id],
                    thread_from_row,
                )
                .optional()?
                .ok_or_else(|| "Thread not found".to_string())?;

            let mut statement = connection.prepare(
                "SELECT id, thread_id, sender, recipients_json, sent_at, body_html, body_text,
                            body_html_z, body_text_z, unsubscribe_json, unread, attachments_json
                     FROM messages WHERE thread_id = ?1 ORDER BY sent_at",
            )?;
            let rows = statement.query_map([id], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    sender: row.get(2)?,
                    recipients: decode_json(row.get::<_, String>(3)?)?,
                    sent_at: row.get(4)?,
                    body_html: resolve_body(7, row.get(5)?, row.get(7)?)?,
                    body_text: readable_body_text(resolve_body(8, row.get(6)?, row.get(8)?)?),
                    unsubscribe: row
                        .get::<_, Option<String>>(9)?
                        .and_then(|value| serde_json::from_str::<UnsubscribeMetadata>(&value).ok())
                        .map(|value| value.info()),
                    unread: row.get::<_, i64>(10)? != 0,
                    attachments: serde_json::from_str(&row.get::<_, String>(11)?).map_err(
                        |error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                11,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        },
                    )?,
                })
            })?;
            let mut messages = rows.collect::<Result<Vec<_>, _>>()?;
            for message in &mut messages {
                for attachment in &mut message.attachments {
                    attachment.filename =
                        crate::attachment_security::normalize_filename(&attachment.filename);
                }
            }
            Ok(ThreadDetail { thread, messages })
        })
    }

    pub fn get_thread_for_message(&self, message_id: &str) -> DbResult<ThreadDetail> {
        let thread_id = self
            .with_connection(|connection| {
                Ok(connection
                    .query_row(
                        "SELECT thread_id FROM messages WHERE id = ?1",
                        [message_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?)
            })?
            .ok_or_else(|| "Reply source message not found".to_string())?;
        self.get_thread(&thread_id)
    }

    /// Saves a summary written from the thread as of `revision` (its
    /// `last_message_at` when the summary request read it). A slower request
    /// for an older revision never replaces one saved for a newer revision.
    /// Returns whether the summary was saved.
    pub fn set_thread_summary(
        &self,
        thread_id: &str,
        summary: &str,
        generated_at: &str,
        revision: &str,
    ) -> DbResult<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE threads SET summary = ?1, summary_generated_at = ?2, summary_revision = ?3
                 WHERE id = ?4 AND (summary_revision IS NULL OR summary_revision <= ?3)",
                params![summary, generated_at, revision, thread_id],
            )? > 0)
        })
    }

    pub fn delete_thread(&self, account_id: &str, provider_thread_id: &str) -> DbResult<()> {
        self.with_transaction(|transaction| {
            let thread_id: Option<String> = transaction
                .query_row(
                    "SELECT id FROM threads WHERE account_id = ?1 AND provider_thread_id = ?2",
                    params![account_id, provider_thread_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(thread_id) = thread_id {
                transaction.execute(
                    "DELETE FROM thread_search WHERE thread_id = ?1",
                    [&thread_id],
                )?;
                transaction.execute("DELETE FROM threads WHERE id = ?1", [&thread_id])?;
            }
            transaction.execute(
                "DELETE FROM quarantined_messages
                     WHERE account_id = ?1 AND provider_thread_id = ?2",
                params![account_id, provider_thread_id],
            )?;
            Ok(())
        })
    }

    /// Deletes threads (and their messages, via `ON DELETE CASCADE`) whose
    /// newest message is older than the configured retention window.
    /// Starred and trashed threads are always kept regardless of age. A
    /// no-op when retention is unset (unlimited). Returns the number of
    /// threads removed.
    pub fn prune_expired_threads(&self) -> DbResult<usize> {
        let Some(days) = self.retention_days()? else {
            return Ok(0);
        };
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        self.with_transaction(|transaction| {
            transaction
                .execute(
                    "DELETE FROM thread_search WHERE thread_id IN
                        (SELECT id FROM threads WHERE last_message_at < ?1 AND starred = 0 AND trashed = 0)",
                    [&cutoff],
                )?;
            let removed = transaction
                .execute(
                    "DELETE FROM threads WHERE last_message_at < ?1 AND starred = 0 AND trashed = 0",
                    [&cutoff],
                )?;
            Ok(removed)
        })
    }

    /// Unread totals for the Inbox and each split inbox tab, scoped to one
    /// account (or merged across all when `account_id` is `None`). A thread
    /// counts toward the Inbox only when no split inbox rule *belonging to
    /// that thread's own account* claims it, mirroring the exclusion
    /// `list_threads_page` applies — a split inbox never pulls mail out of a
    /// different account's inbox.
    pub fn mailbox_unread_counts(&self, account_id: Option<&str>) -> DbResult<MailboxUnreadCounts> {
        let rules = self.list_split_inboxes()?;
        let threads = self.list_threads(account_id)?;
        let mut splits: HashMap<String, i64> = HashMap::new();
        let mut inbox = 0i64;
        for thread in &threads {
            if !thread.unread {
                continue;
            }
            let mut matched_any = false;
            for rule in &rules {
                if rule.account_id == thread.account_id && split_inbox_matches(rule, thread) {
                    *splits.entry(rule.id.clone()).or_insert(0) += 1;
                    matched_any = true;
                }
            }
            if !matched_any {
                inbox += 1;
            }
        }
        Ok(MailboxUnreadCounts { inbox, splits })
    }
}

/// `threads.id`, derived from the pair that's actually unique: Gmail thread
/// IDs are unique only within one account, not across two different
/// accounts, so the bare provider ID can't be used as the local primary key
/// once more than one account is connected.
pub(super) fn local_thread_id(account_id: &str, provider_thread_id: &str) -> String {
    format!("{account_id}:{provider_thread_id}")
}

fn thread_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: row.get(0)?,
        provider_thread_id: row.get(1)?,
        subject: row.get(2)?,
        snippet: row.get(3)?,
        participants: decode_json(row.get::<_, String>(4)?)?,
        last_message_at: row.get(5)?,
        unread: row.get(6)?,
        starred: row.get(7)?,
        archived: row.get(8)?,
        labels: decode_json(row.get::<_, String>(9)?)?,
        trashed: row.get(10)?,
        account_id: row.get(11)?,
        summary: row.get(12)?,
        summary_generated_at: row.get(13)?,
        has_attachments: row.get(14)?,
        last_received_at: row.get(15)?,
        summary_revision: row.get(16)?,
        match_snippet: None,
    })
}

#[cfg(test)]
#[path = "tests/threads.rs"]
mod tests;
