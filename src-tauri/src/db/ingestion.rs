//! Atomic ingestion of provider threads, messages, indexes, and quarantine records.
//! The caller-owned transaction also covers replay of undelivered local mutations.

use super::messages::{compress_body, store_message_metadata};
use super::search::{list_preview, search_preview};
use super::threads::local_thread_id;
use super::{contacts, serialization_error, Database, DbResult};
use crate::mime::NormalizedMessage;
use chrono::Utc;
use rusqlite::{params, Transaction};

impl Database {
    fn apply_thread(
        transaction: &Transaction<'_>,
        account_id: &str,
        messages: &[NormalizedMessage],
    ) -> DbResult<()> {
        let Some(latest) = messages
            .iter()
            .max_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)))
        else {
            return Ok(());
        };
        let provider_thread_id = &latest.thread_id;
        let thread_id = local_thread_id(account_id, provider_thread_id);
        let mut participants: Vec<String> = messages
            .iter()
            .map(|message| message.from.clone())
            .collect();
        participants.sort();
        participants.dedup();
        // Indexed separately from `participants`: recipients should be
        // searchable even though they aren't shown in the thread list's
        // "From" line.
        let mut search_participants: Vec<String> = messages
            .iter()
            .flat_map(|message| {
                std::iter::once(message.from.clone()).chain(message.to.iter().cloned())
            })
            .collect();
        search_participants.sort();
        search_participants.dedup();
        let root = messages
            .iter()
            .min_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)))
            .expect("a latest message implies a root message");
        let mut labels: Vec<String> = messages
            .iter()
            .flat_map(|message| message.labels.iter())
            .filter(|label| {
                is_system_label(label) && label.as_str() != "STARRED" && label.as_str() != "UNREAD"
            })
            .cloned()
            .chain(
                root.labels
                    .iter()
                    .filter(|label| !is_system_label(label) || label.as_str() == "STARRED")
                    .cloned(),
            )
            .chain(
                latest
                    .labels
                    .iter()
                    .filter(|label| label.as_str() == "UNREAD")
                    .cloned(),
            )
            .collect();
        labels.sort();
        labels.dedup();
        let unread = labels.iter().any(|label| label == "UNREAD");
        let starred = labels.iter().any(|label| label == "STARRED");
        let archived = !labels.iter().any(|label| label == "INBOX");
        let trashed = labels.iter().any(|label| label == "TRASH");
        let has_attachments = messages.iter().any(|message| {
            message
                .attachments
                .iter()
                .any(|attachment| !attachment.inline)
        });
        // Only messages someone else sent should bump a thread's place in the
        // inbox order; otherwise replying to a thread buried in the list
        // would jump it straight to the top like a freshly received message.
        let last_received_at = messages
            .iter()
            .filter(|message| !message.labels.iter().any(|label| label == "SENT"))
            .max_by(|a, b| a.date.cmp(&b.date))
            .map(|message| message.date.clone())
            .unwrap_or_else(|| latest.date.clone());
        let mut chronological: Vec<&NormalizedMessage> = messages.iter().collect();
        chronological.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
        let text = crate::quoted_history::searchable_thread_text(
            chronological
                .iter()
                .map(|message| message.body_text.as_str()),
        );
        transaction
            .execute(
                "INSERT INTO threads(
                    id, account_id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed, has_attachments,
                    last_received_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(id) DO UPDATE SET
                    subject=excluded.subject, snippet=excluded.snippet,
                    participants_json=excluded.participants_json,
                    last_message_at=excluded.last_message_at, unread=excluded.unread,
                    starred=excluded.starred, archived=excluded.archived,
                    labels_json=excluded.labels_json, trashed=excluded.trashed,
                    has_attachments=excluded.has_attachments,
                    last_received_at=excluded.last_received_at",
                params![
                    thread_id,
                    account_id,
                    provider_thread_id,
                    latest.subject,
                    list_preview(&text, &latest.snippet),
                    serde_json::to_string(&participants).map_err(serialization_error)?,
                    latest.date,
                    unread,
                    starred,
                    archived,
                    serde_json::to_string(&labels).map_err(serialization_error)?,
                    trashed,
                    has_attachments,
                    last_received_at,
                ],
            )?;
        transaction.execute("DELETE FROM messages WHERE thread_id = ?1", [&thread_id])?;
        transaction.execute(
            "DELETE FROM thread_search WHERE thread_id = ?1",
            [&thread_id],
        )?;
        for message in messages {
            store_message_metadata(transaction, &message.id, &message.metadata_json)?;
            let message_unread = message.labels.iter().any(|label| label == "UNREAD");
            transaction.execute(
                "INSERT INTO messages(
                        id, thread_id, sender, recipients_json, sent_at, body_html, body_text,
                        body_html_z, body_text_z, unsubscribe_json, unread, attachments_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    message.id,
                    thread_id,
                    message.from,
                    serde_json::to_string(&message.to).map_err(serialization_error)?,
                    message.date,
                    "",
                    "",
                    compress_body(&message.body_html),
                    compress_body(&message.body_text),
                    message
                        .unsubscribe
                        .as_ref()
                        .map(|value| serde_json::to_string(value).map_err(serialization_error))
                        .transpose()?,
                    message_unread,
                    serde_json::to_string(&message.attachments).map_err(serialization_error)?,
                ],
            )?;
            contacts::index_contact_message(transaction, account_id, &thread_id, message)?;
        }
        transaction.execute(
            "INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                thread_id,
                latest.subject,
                search_preview(&text, &latest.snippet),
                search_participants.join(" "),
                text.body
            ],
        )?;
        transaction.execute(
            "DELETE FROM pending_search_reindex WHERE thread_id = ?1",
            [&thread_id],
        )?;
        Self::reapply_undelivered_mutations(transaction, &thread_id)
    }

    pub fn upsert_thread(&self, account_id: &str, messages: &[NormalizedMessage]) -> DbResult<()> {
        self.upsert_threads(account_id, &[messages.to_vec()])
    }

    pub fn upsert_threads(
        &self,
        account_id: &str,
        message_groups: &[Vec<NormalizedMessage>],
    ) -> DbResult<()> {
        if message_groups.is_empty() {
            return Ok(());
        }
        self.with_transaction(|transaction| {
            for messages in message_groups {
                Self::apply_thread(transaction, account_id, messages)?;
            }
            Ok(())
        })
    }

    pub fn apply_ingested_threads(
        &self,
        account_id: &str,
        threads: &[(String, Vec<NormalizedMessage>, Vec<(String, String)>)],
    ) -> DbResult<()> {
        if threads.is_empty() {
            return Ok(());
        }
        self.with_transaction(|transaction| {
            for (provider_thread_id, messages, quarantined) in threads {
                transaction.execute(
                    "DELETE FROM quarantined_messages
                         WHERE account_id = ?1 AND provider_thread_id = ?2",
                    params![account_id, provider_thread_id],
                )?;
                if !messages.is_empty() {
                    Self::apply_thread(transaction, account_id, messages)?;
                }
                for (message_id, error) in quarantined {
                    transaction.execute(
                        "INSERT INTO quarantined_messages(
                                account_id, provider_thread_id, message_id, error, created_at
                             ) VALUES (?1, ?2, ?3, ?4, ?5)
                             ON CONFLICT(account_id, message_id) DO UPDATE SET
                                provider_thread_id=excluded.provider_thread_id,
                                error=excluded.error,
                                created_at=excluded.created_at",
                        params![
                            account_id,
                            provider_thread_id,
                            message_id,
                            error,
                            Utc::now().to_rfc3339()
                        ],
                    )?;
                }
            }
            Ok(())
        })
    }
}

fn is_system_label(id: &str) -> bool {
    id.starts_with("CATEGORY_")
        || matches!(
            id,
            "CHAT"
                | "SENT"
                | "INBOX"
                | "IMPORTANT"
                | "TRASH"
                | "DRAFT"
                | "SPAM"
                | "STARRED"
                | "UNREAD"
                | "SCHEDULED"
                | "MUTED"
        )
}

#[cfg(test)]
#[path = "tests/ingestion.rs"]
mod tests;
