//! Optimistic local mutations and their durable provider-delivery queue.
//! Apply/replay helpers use the caller's transaction and never acquire a lock.

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::models::ThreadMutation;
use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PendingMutation {
    pub id: String,
    pub provider_thread_id: String,
    pub target_message_id: Option<String>,
    pub mutation: ThreadMutation,
    pub attempts: u32,
}

impl Database {
    fn updated_thread_labels_json(
        transaction: &Transaction<'_>,
        thread_id: &str,
        remove: &[&str],
        add: Option<&str>,
    ) -> DbResult<String> {
        let labels: String = transaction
            .query_row(
                "SELECT labels_json FROM threads WHERE id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(DatabaseError::NotFound("Thread"))?;
        let mut labels: Vec<String> = serde_json::from_str(&labels).map_err(serialization_error)?;
        labels.retain(|item| !remove.contains(&item.as_str()));
        // Removal alone preserves the remaining order and duplicates.
        if let Some(label) = add {
            labels.push(label.to_string());
            labels.sort();
            labels.dedup();
        }
        serde_json::to_string(&labels).map_err(serialization_error)
    }

    fn apply_mutation(transaction: &Transaction<'_>, mutation: &ThreadMutation) -> DbResult<()> {
        let kind = match mutation {
            ThreadMutation::Archive { .. } => "archive",
            ThreadMutation::Trash { .. } => "trash",
            ThreadMutation::Spam { .. } => "spam",
            ThreadMutation::Read { .. } => "read",
            ThreadMutation::Star { .. } => "star",
            ThreadMutation::Label { .. } => "label",
        };
        if Self::apply_mutation_locally(transaction, mutation)? == 0 {
            return Err(DatabaseError::NotFound("Thread"));
        }
        Self::queue_mutation(transaction, mutation, kind)
    }

    /// Applies a mutation's effect to the local thread row (and, for read
    /// state, its messages) without queueing it. Returns the number of
    /// thread rows changed.
    fn apply_mutation_locally(
        transaction: &Transaction<'_>,
        mutation: &ThreadMutation,
    ) -> DbResult<usize> {
        let value = match mutation {
            ThreadMutation::Archive { value, .. }
            | ThreadMutation::Trash { value, .. }
            | ThreadMutation::Spam { value, .. }
            | ThreadMutation::Read { value, .. }
            | ThreadMutation::Star { value, .. }
            | ThreadMutation::Label { value, .. } => *value,
        };
        let changed = match mutation {
            ThreadMutation::Spam { thread_id, value } => {
                let labels_json = Self::updated_thread_labels_json(
                    transaction,
                    thread_id,
                    &["SPAM", "INBOX"],
                    Some(if *value { "SPAM" } else { "INBOX" }),
                )?;
                transaction.execute(
                    "UPDATE threads SET labels_json = ?1, archived = ?2 WHERE id = ?3",
                    params![labels_json, value, thread_id],
                )?
            }
            ThreadMutation::Label {
                thread_id,
                label_id,
                value,
            } => {
                let labels_json = Self::updated_thread_labels_json(
                    transaction,
                    thread_id,
                    &[label_id.as_str()],
                    value.then_some(label_id.as_str()),
                )?;
                // Only the flag this label controls changes. Archive, read
                // and star mutations set their columns without rewriting
                // `labels_json`, so deriving every flag from it here would
                // undo them.
                let flag = match label_id.as_str() {
                    "UNREAD" => Some(("unread", *value)),
                    "STARRED" => Some(("starred", *value)),
                    "INBOX" => Some(("archived", !*value)),
                    _ => None,
                };
                match flag {
                    Some((column, flag_value)) => transaction.execute(
                        &format!(
                            "UPDATE threads SET labels_json = ?1, {column} = ?2 WHERE id = ?3"
                        ),
                        params![labels_json, flag_value, thread_id],
                    )?,
                    None => transaction.execute(
                        "UPDATE threads SET labels_json = ?1 WHERE id = ?2",
                        params![labels_json, thread_id],
                    )?,
                }
            }
            ThreadMutation::Read { thread_id, value } => {
                let changed = transaction.execute(
                    "UPDATE threads SET unread = ?1 WHERE id = ?2",
                    params![!value, thread_id],
                )?;
                // Mirrors Gmail: marking read clears every message in the
                // thread, but marking unread only brings back the most
                // recent message as unread, not the whole history.
                transaction.execute(
                    "UPDATE messages SET unread = 0 WHERE thread_id = ?1",
                    [thread_id],
                )?;
                if !value {
                    transaction.execute(
                        "UPDATE messages SET unread = 1
                             WHERE id = (
                                 SELECT id FROM messages WHERE thread_id = ?1
                                 ORDER BY sent_at DESC, id DESC LIMIT 1
                             )",
                        [thread_id],
                    )?;
                }
                changed
            }
            _ => {
                let column = match mutation {
                    ThreadMutation::Archive { .. } => "archived",
                    ThreadMutation::Trash { .. } => "trashed",
                    ThreadMutation::Spam { .. } => unreachable!(),
                    ThreadMutation::Read { .. } => unreachable!(),
                    ThreadMutation::Star { .. } => "starred",
                    ThreadMutation::Label { .. } => unreachable!(),
                };
                let stored_value = match mutation {
                    ThreadMutation::Read { .. } => unreachable!(),
                    ThreadMutation::Spam { .. } => unreachable!(),
                    _ => value,
                };
                let sql = format!("UPDATE threads SET {column} = ?1 WHERE id = ?2");
                transaction.execute(&sql, params![stored_value, mutation.thread_id()])?
            }
        };
        Ok(changed)
    }

    /// Re-applies the thread's undelivered mutations, oldest first, after a
    /// provider copy of it was written. That copy can predate a mutation the
    /// user made while it was being fetched, or one still waiting to be
    /// delivered; without this, sync would visibly undo the change until the
    /// mutation reached the provider and a later poll fetched it back.
    pub(super) fn reapply_undelivered_mutations(
        transaction: &Transaction<'_>,
        thread_id: &str,
    ) -> DbResult<()> {
        let payloads = {
            let mut statement = transaction.prepare(
                "SELECT payload_json FROM mutations
                 WHERE thread_id = ?1 AND state IN ('pending', 'running')
                 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = statement.query_map([thread_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for payload in payloads {
            let mutation: ThreadMutation =
                serde_json::from_str(&payload).map_err(serialization_error)?;
            Self::apply_mutation_locally(transaction, &mutation)?;
        }
        Ok(())
    }

    fn queue_mutation(
        transaction: &Transaction<'_>,
        mutation: &ThreadMutation,
        kind: &str,
    ) -> DbResult<()> {
        let (account_id, provider_thread_id): (String, String) = transaction.query_row(
            "SELECT account_id, provider_thread_id FROM threads WHERE id = ?1",
            [mutation.thread_id()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if let ThreadMutation::Archive { value, .. } = mutation {
            // A separate Archive during Undo Send has the same lasting intent
            // as Send & Mark Done. Keep it with the outbox item so delivery
            // can remove INBOX again after Gmail adds the sent message.
            transaction.execute(
                "UPDATE outbox_messages SET archive_on_send = ?1
                 WHERE account = ?2 AND json_extract(payload, '$.threadId') = ?3
                   AND state IN ('undo_pending', 'ready', 'sending', 'uncertain')",
                params![value, account_id, provider_thread_id],
            )?;
        }
        // Metadata actions use one stable representative message rather than
        // rewriting the state of every message in a Gmail conversation.
        // Capture the target now so an offline mutation cannot drift if a new
        // message arrives before it is delivered.
        let target_message_id: Option<String> = match mutation {
            ThreadMutation::Star { .. } | ThreadMutation::Label { .. } => transaction
                .query_row(
                    "SELECT id FROM messages WHERE thread_id = ?1
                     ORDER BY sent_at ASC, id ASC LIMIT 1",
                    [mutation.thread_id()],
                    |row| row.get(0),
                )
                .optional()?,
            ThreadMutation::Read { value: false, .. } => transaction
                .query_row(
                    "SELECT id FROM messages WHERE thread_id = ?1
                     ORDER BY sent_at DESC, id DESC LIMIT 1",
                    [mutation.thread_id()],
                    |row| row.get(0),
                )
                .optional()?,
            _ => None,
        };
        let payload = serde_json::to_string(mutation).map_err(serialization_error)?;
        let duplicate: bool = transaction.query_row(
            "SELECT EXISTS(
                    SELECT 1 FROM mutations
                    WHERE thread_id = ?1 AND kind = ?2 AND payload_json = ?3
                      AND state IN ('pending', 'running')
                 )",
            params![mutation.thread_id(), kind, payload],
            |row| row.get(0),
        )?;
        if !duplicate {
            transaction
                .execute(
                    "INSERT INTO mutations(
                    id, account_id, thread_id, target_message_id, kind, payload_json, state, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
                    params![
                        Uuid::new_v4().to_string(),
                        account_id,
                        mutation.thread_id(),
                        target_message_id,
                        kind,
                        payload,
                        Utc::now().to_rfc3339(),
                    ],
                )?;
        }
        Ok(())
    }

    pub fn mutate_thread(&self, mutation: &ThreadMutation) -> DbResult<()> {
        self.with_transaction(|transaction| {
            Self::apply_mutation(transaction, mutation)?;
            Ok(())
        })
    }

    pub fn mutate_threads(&self, mutations: &[ThreadMutation]) -> DbResult<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        self.with_transaction(|transaction| {
            for mutation in mutations {
                Self::apply_mutation(transaction, mutation)?;
            }
            Ok(())
        })
    }

    /// Claims only `account_id`'s pending mutations, so one account's poller
    /// never picks up and tries to deliver another account's mutation
    /// through the wrong Gmail session.
    ///
    /// Uses a `LEFT JOIN` rather than an inner join: a mutation whose thread
    /// row is gone (deleted from Gmail, or — historically — wiped by a full
    /// resync) would otherwise never match an inner join and would sit as
    /// "pending" forever, claimed by nothing and reported nowhere. Such rows
    /// are instead failed immediately with a clear reason and excluded from
    /// the claimed batch.
    pub fn claim_mutations(
        &self,
        account_id: &str,
        limit: usize,
    ) -> DbResult<Vec<PendingMutation>> {
        self.with_transaction(|transaction| {
            let (claimable, orphaned) = {
                let mut statement = transaction.prepare(
                    "SELECT m.id, t.provider_thread_id, m.target_message_id, m.payload_json,
                                m.attempts
                         FROM mutations m LEFT JOIN threads t ON t.id = m.thread_id
                         WHERE m.state = 'pending' AND m.account_id = ?1
                           AND NOT EXISTS (
                               SELECT 1 FROM accounts a
                               WHERE a.email = m.account_id AND a.status = 'needs_reauth'
                           )
                           AND (m.next_attempt_at IS NULL OR m.next_attempt_at <= ?2)
                         ORDER BY m.created_at LIMIT ?3",
                )?;
                let rows = statement
                    .query_map(
                        params![account_id, Utc::now().to_rfc3339(), limit as i64],
                        |row| {
                            let id: String = row.get(0)?;
                            let provider_thread_id: Option<String> = row.get(1)?;
                            let target_message_id: Option<String> = row.get(2)?;
                            let payload: String = row.get(3)?;
                            let attempts: u32 = row.get(4)?;
                            Ok((id, provider_thread_id, target_message_id, payload, attempts))
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut claimable = Vec::new();
                let mut orphaned = Vec::new();
                for (id, provider_thread_id, target_message_id, payload, attempts) in rows {
                    match provider_thread_id {
                        Some(provider_thread_id) => {
                            let mutation = serde_json::from_str(&payload).map_err(|error| {
                                DatabaseError::from(rusqlite::Error::FromSqlConversionFailure(
                                    payload.len(),
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                ))
                            })?;
                            claimable.push(PendingMutation {
                                id,
                                provider_thread_id,
                                target_message_id,
                                mutation,
                                attempts: attempts + 1,
                            });
                        }
                        None => orphaned.push(id),
                    }
                }
                (claimable, orphaned)
            };
            for id in &orphaned {
                transaction.execute(
                    "UPDATE mutations SET state = 'failed',
                            last_error = 'Target thread no longer exists locally'
                         WHERE id = ?1",
                    [id],
                )?;
            }
            for mutation in &claimable {
                transaction.execute(
                    "UPDATE mutations SET state = 'running', attempts = attempts + 1
                         WHERE id = ?1 AND state = 'pending'",
                    [&mutation.id],
                )?;
            }
            Ok(claimable)
        })
    }

    pub fn complete_mutation(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE mutations SET state = 'done', last_error = NULL,
                        next_attempt_at = NULL WHERE id = ?1",
                [id],
            )?;
            Ok(())
        })
    }

    pub fn reject_mutation(
        &self,
        id: &str,
        error: &str,
        next_attempt_at: Option<&str>,
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE mutations SET state = ?1, last_error = ?2, next_attempt_at = ?3
                     WHERE id = ?4",
                params![
                    if next_attempt_at.is_some() {
                        "pending"
                    } else {
                        "failed"
                    },
                    error,
                    next_attempt_at,
                    id
                ],
            )?;
            Ok(())
        })
    }

    /// Requeues every permanently failed mutation with a fresh attempt budget,
    /// for when the user wants to try again after fixing the cause (for
    /// example reconnecting an account). Returns how many were requeued.
    pub fn retry_failed_mutations(&self) -> DbResult<usize> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE mutations SET state = 'pending', attempts = 0, last_error = NULL,
                    next_attempt_at = NULL
                 WHERE state = 'failed'",
                [],
            )?)
        })
    }
}

#[cfg(test)]
#[path = "tests/mutations.rs"]
mod tests;
