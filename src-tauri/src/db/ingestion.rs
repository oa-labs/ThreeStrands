//! Atomic ingestion of provider threads, messages, indexes, and quarantine records.
//! The caller-owned transaction also covers replay of undelivered local mutations.

use super::messages::{compress_body, store_message_metadata};
use super::search::{lead_with_preheader, list_preview, preview_context, search_preview};
use super::threads::local_thread_id;
use super::{contacts, serialization_error, Database, DatabaseError, DbResult};
use crate::mime::NormalizedMessage;
use chrono::Utc;
use rusqlite::{params, Transaction};

/// A provider thread id, its normalized messages, and the `(message_id, error)`
/// pairs quarantined from it.
type IngestedThread = (String, Vec<NormalizedMessage>, Vec<(String, String)>);

impl Database {
    fn apply_thread(
        transaction: &Transaction<'_>,
        account_id: &str,
        messages: &[NormalizedMessage],
    ) -> DbResult<()> {
        let mut unique = std::collections::HashSet::new();
        if messages.iter().any(|message| !unique.insert(message.id.as_str())) {
            return Err(DatabaseError::invalid("Duplicate message in ingested thread"));
        }
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
        let latest_html = chronological.last().map(|message| message.body_html.as_str());
        let mut text = crate::quoted_history::searchable_thread_text_with(
            chronological
                .iter()
                .map(|message| message.body_text.as_str()),
            &preview_context(&latest.subject, latest_html),
        );
        lead_with_preheader(&mut text, latest_html);
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
        // Keep stable message occurrences across a refresh so device-local
        // attachment stars survive. Remove only messages absent from the new
        // thread, then update existing rows in place below.
        let incoming_ids: Vec<&str> = messages.iter().map(|message| message.id.as_str()).collect();
        let incoming_json=serde_json::to_string(&incoming_ids).map_err(serialization_error)?;
        let foreign_message:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM messages m JOIN threads t ON t.id=m.thread_id WHERE m.id IN (SELECT value FROM json_each(?1)) AND (t.account_id<>?2 OR m.thread_id<>?3))",params![incoming_json,account_id,thread_id],|row|row.get(0))?;
        if foreign_message { return Err(DatabaseError::invalid("Message belongs to another thread or account")); }
        transaction.execute("DELETE FROM messages WHERE thread_id = ?1 AND id NOT IN (SELECT value FROM json_each(?2))", params![thread_id,incoming_json])?;
        // The account's "sent by me" identity set, loaded ONCE per thread and
        // reused for every message below — not once per message (no N+1).
        let mine = crate::db::sending_addresses_for_account(transaction, account_id);
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
                        body_html_z, body_text_z, unsubscribe_json, unread, attachments_json, cc_bcc_json, cc_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     ON CONFLICT(id) DO UPDATE SET thread_id=excluded.thread_id,
                        sender=excluded.sender,recipients_json=excluded.recipients_json,
                        cc_bcc_json=excluded.cc_bcc_json,cc_json=excluded.cc_json,sent_at=excluded.sent_at,
                        body_html=excluded.body_html,body_text=excluded.body_text,body_html_z=excluded.body_html_z,
                        body_text_z=excluded.body_text_z,unsubscribe_json=excluded.unsubscribe_json,
                        unread=excluded.unread,attachments_json=excluded.attachments_json",
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
                    serde_json::to_string(&message.cc_bcc).map_err(serialization_error)?,
                    serde_json::to_string(&message.cc).map_err(serialization_error)?,
                ],
            )?;
            transaction.execute("DELETE FROM contact_interactions WHERE message_id=?1", [&message.id])?;
            contacts::index_contact_message(transaction, account_id, &thread_id, message, &mine)?;
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
        Self::reapply_undelivered_mutations(transaction, &thread_id)?;
        // After the user's own pending changes, so a conversation they just
        // moved is judged where they left it.
        Self::apply_sender_rules(transaction, account_id, &thread_id, latest, &mine)
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
        threads: &[IngestedThread],
    ) -> DbResult<()> {
        self.apply_ingested_threads_with_aliases(account_id, threads, &[])
    }

    pub fn apply_ingested_threads_with_aliases(
        &self,
        account_id: &str,
        threads: &[IngestedThread],
        aliases: &[(String, String)],
    ) -> DbResult<()> {
        if threads.is_empty() {
            return Ok(());
        }
        self.with_transaction(|transaction| {
            // Retire old cached threads before inserting their members under
            // the survivor, regardless of fetch order or batch boundaries.
            // Do this only when that survivor was successfully fetched; a
            // failed fetch leaves the old cache and workflow links intact.
            let mut retained_file_stars:Vec<(String,String)>=Vec::new();
            for (old, survivor) in aliases {
                if old == survivor || !threads.iter().any(|(id, messages, _)| {
                    id == survivor || messages.iter().any(|m| &m.thread_id == survivor)
                }) {
                    continue;
                }
                let old_local = local_thread_id(account_id, old);
                let survivor_local = local_thread_id(account_id, survivor);
                {
                    let mut stars=transaction.prepare("SELECT s.message_id,s.attachment_id FROM file_stars s JOIN messages m ON m.id=s.message_id WHERE m.thread_id=?1")?;
                    retained_file_stars.extend(stars.query_map([&old_local],|row|Ok((row.get(0)?,row.get(1)?)))?.collect::<Result<Vec<_>,_>>()?);
                }
                transaction.execute(
                    "UPDATE tasks SET thread_id = ?3 WHERE account_id = ?1 AND thread_id = ?2",
                    params![account_id, old_local, survivor_local],
                )?;
                transaction.execute("DELETE FROM thread_search WHERE thread_id = ?1", [&old_local])?;
                transaction.execute("DELETE FROM threads WHERE id = ?1 AND account_id = ?2",
                    params![old_local, account_id])?;
                transaction.execute(
                    "DELETE FROM quarantined_messages WHERE account_id = ?1 AND provider_thread_id = ?2",
                    params![account_id, old],
                )?;
            }
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
            for (message_id,attachment_id) in retained_file_stars {
                transaction.execute("INSERT OR IGNORE INTO file_stars SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM file_attachments a JOIN messages m ON m.id=a.message_id JOIN threads t ON t.id=m.thread_id WHERE a.message_id=?1 AND a.attachment_id=?2 AND t.account_id=?3)",params![message_id,attachment_id,account_id])?;
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
