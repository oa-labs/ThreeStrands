//! Persistent conversation return schedules. Mail stays with the provider;
//! only account/thread/message references and the chosen time replicate.
use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::{
    mime::NormalizedMessage,
    models::{ThreadMutation, ThreadPage},
};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use threestrands_sync_protocol::EntityType;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConversationSnooze {
    pub id: String,
    pub account_id: String,
    pub provider_thread_id: String,
    pub until: String,
    pub state: String,
    pub message_ids: Vec<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

impl ConversationSnooze {
    fn validate(&self) -> DbResult<()> {
        EntityType::ConversationSnooze
            .validate_payload(&serde_json::json!({"schedule": self}))
            .map_err(DatabaseError::invalid)?;
        if self.id != super::threads::local_thread_id(&self.account_id, &self.provider_thread_id)
            || DateTime::parse_from_rfc3339(&self.until).is_err()
        {
            return Err(DatabaseError::invalid("Invalid conversation snooze"));
        }
        Ok(())
    }
}

fn get(tx: &Transaction<'_>, id: &str) -> DbResult<Option<ConversationSnooze>> {
    let json: Option<String> = tx
        .query_row(
            "SELECT record_json FROM conversation_snoozes WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str(&value).map_err(serialization_error))
        .transpose()
}

fn store(tx: &Transaction<'_>, item: &ConversationSnooze, publish: bool) -> DbResult<()> {
    item.validate()?;
    tx.execute(
        "INSERT INTO conversation_snoozes(id, account_id, record_json) VALUES (?1,?2,?3)
        ON CONFLICT(id) DO UPDATE SET record_json=excluded.record_json",
        params![
            item.id,
            item.account_id,
            serde_json::to_string(item).map_err(serialization_error)?
        ],
    )?;
    let enabled: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_spaces WHERE enabled=1)",
        [],
        |row| row.get(0),
    )?;
    let recording = publish && (enabled || crate::replicated_sync::enabled());
    tx.execute(
        "UPDATE conversation_snoozes SET replica_dirty=?2 WHERE id=?1",
        params![item.id, publish && !recording],
    )?;
    if recording {
        super::sync_state::record_replicated_write_in_transaction(
            tx,
            EntityType::ConversationSnooze,
            &item.id,
            &["schedule".to_string()].into(),
            &serde_json::json!({"schedule": item}),
        )?;
    }
    Ok(())
}

fn return_to_inbox(tx: &Transaction<'_>, item: &ConversationSnooze) -> DbResult<()> {
    let eligible: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1 AND trashed=0
        AND NOT EXISTS(SELECT 1 FROM json_each(labels_json) WHERE value='SPAM'))",
        [&item.id],
        |row| row.get(0),
    )?;
    if eligible {
        Database::apply_mutation(
            tx,
            &ThreadMutation::Archive {
                thread_id: item.id.clone(),
                value: false,
            },
        )?;
    }
    Ok(())
}

impl Database {
    /// One transaction covers the entire batch, the archive queue, and replica.
    pub(crate) fn snooze_threads(
        &self,
        ids: &[String],
        until: &str,
    ) -> DbResult<Vec<ConversationSnooze>> {
        let due = DateTime::parse_from_rfc3339(until)
            .map_err(|_| DatabaseError::invalid("Choose a valid return time"))?;
        if due <= Utc::now() {
            return Err(DatabaseError::invalid("Choose a future return time"));
        }
        let until = due.with_timezone(&Utc).to_rfc3339();
        self.with_transaction(|tx| {
            let mut result = Vec::new();
            for id in ids.iter().collect::<std::collections::BTreeSet<_>>() {
                let (account_id, provider_thread_id, eligible): (String, String, bool) = tx.query_row(
                    "SELECT account_id, provider_thread_id, trashed=0 AND NOT EXISTS(SELECT 1 FROM json_each(labels_json) WHERE value='SPAM') FROM threads WHERE id=?1",
                    [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?.ok_or(DatabaseError::NotFound("Thread"))?;
                if !eligible { return Err(DatabaseError::invalid("Trash and spam cannot be snoozed")); }
                let mut statement = tx.prepare("SELECT id FROM messages WHERE thread_id=?1 ORDER BY id")?;
                let message_ids = statement.query_map([id], |row| row.get(0))?.collect::<Result<Vec<String>, _>>()?;
                let mut statement = tx.prepare("SELECT DISTINCT token FROM imap_message_tokens WHERE account_id=?1 AND message_id IN (SELECT id FROM messages WHERE thread_id=?2) ORDER BY token")?;
                let references = statement.query_map(params![account_id, id], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?.into_iter().filter(|token| token.contains('@') && token.is_ascii() && !token.chars().any(char::is_control)).collect();
                let item = ConversationSnooze { id: id.clone(), account_id, provider_thread_id, until: until.clone(), state: "active".into(), message_ids, references };
                store(tx, &item, true)?;
                Self::apply_mutation(tx, &ThreadMutation::Archive { thread_id: id.clone(), value: true })?;
                result.push(item);
            }
            Ok(result)
        })
    }

    pub(crate) fn return_snoozed_threads(&self, ids: &[String]) -> DbResult<()> {
        self.with_transaction(|tx| {
            for id in ids {
                if let Some(mut item) = get(tx, id)? {
                    item.state = "returned".into();
                    store(tx, &item, true)?;
                    return_to_inbox(tx, &item)?;
                }
            }
            Ok(())
        })
    }

    /// Undo restores each prior schedule or original archive state atomically.
    pub(crate) fn restore_snoozes(
        &self,
        records: &[ConversationSnooze],
        archived_ids: &[String],
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            for item in records {
                item.validate()?;
                if get(tx, &item.id)?.is_none() {
                    return Err(DatabaseError::NotFound("Snooze"));
                }
                store(tx, item, true)?;
                Self::apply_mutation(
                    tx,
                    &ThreadMutation::Archive {
                        thread_id: item.id.clone(),
                        value: item.state == "active" || archived_ids.contains(&item.id),
                    },
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn publish_dirty_snoozes(&self) -> DbResult<()> {
        self.with_transaction(|tx| {
            let mut statement =
                tx.prepare("SELECT record_json FROM conversation_snoozes WHERE replica_dirty=1")?;
            let records = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for json in records {
                let item: ConversationSnooze =
                    serde_json::from_str(&json).map_err(serialization_error)?;
                store(tx, &item, true)?;
            }
            Ok(())
        })
    }

    pub(super) fn migrate_snooze_alias(
        tx: &Transaction<'_>,
        old: &str,
        survivor: &str,
        provider_id: &str,
    ) -> DbResult<()> {
        if let Some(mut item) = get(tx, old)? {
            if item.state == "active" {
                let mut replacement = item.clone();
                replacement.id = survivor.to_string();
                replacement.provider_thread_id = provider_id.to_string();
                if get(tx, survivor)?.is_none() {
                    store(tx, &replacement, true)?;
                    tx.execute(
                        "UPDATE conversation_snoozes SET pending_apply=1 WHERE id=?1",
                        [survivor],
                    )?;
                }
                item.state = "cancelled".into();
                store(tx, &item, true)?;
            }
        }
        Ok(())
    }

    pub(crate) fn list_snooze_records(
        &self,
        account: Option<&str>,
    ) -> DbResult<Vec<ConversationSnooze>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT record_json FROM conversation_snoozes WHERE (?1 IS NULL OR account_id=?1) ORDER BY id")?;
            let rows = statement.query_map([account], |row| row.get::<_, String>(0))?;
            rows.map(|row| serde_json::from_str(&row?).map_err(serialization_error)).collect()
        })
    }

    pub(crate) fn list_snoozed_page(
        &self,
        account: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> DbResult<ThreadPage> {
        self.list_threads_page_ordered(account, "trashed=0 AND NOT EXISTS(SELECT 1 FROM json_each(t.labels_json) WHERE value='SPAM') AND EXISTS(SELECT 1 FROM conversation_snoozes s WHERE s.id=t.id AND json_extract(s.record_json,'$.state')='active')",
            "(SELECT json_extract(s.record_json,'$.until') FROM conversation_snoozes s WHERE s.id=t.id) ASC, t.id ASC", offset, limit)
    }

    pub(super) fn cancel_snooze_for_mutation(
        tx: &Transaction<'_>,
        mutation: &ThreadMutation,
    ) -> DbResult<()> {
        let cancels = matches!(
            mutation,
            ThreadMutation::Archive { .. }
                | ThreadMutation::Trash { value: true, .. }
                | ThreadMutation::Spam { value: true, .. }
        ) || matches!(mutation, ThreadMutation::Label { label_id, .. } if ["INBOX", "TRASH", "SPAM"].contains(&label_id.as_str()));
        if cancels {
            if let Some(mut item) = get(tx, mutation.thread_id())? {
                if item.state != "cancelled" {
                    item.state = "cancelled".into();
                    store(tx, &item, true)?;
                    tx.execute(
                        "UPDATE conversation_snoozes SET pending_apply=0 WHERE id=?1",
                        [&item.id],
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Called on provider evidence, before replaying local optimistic changes.
    pub(super) fn reconcile_ingested_snooze(
        tx: &Transaction<'_>,
        account: &str,
        id: &str,
        messages: &[NormalizedMessage],
    ) -> DbResult<()> {
        let Some(mut item) = get(tx, id)? else {
            return Ok(());
        };
        let pending: bool = tx.query_row(
            "SELECT pending_apply FROM conversation_snoozes WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE conversation_snoozes SET pending_apply=0 WHERE id=?1",
            [id],
        )?;
        if item.state == "returned" {
            if pending
                && !messages.iter().any(|message| {
                    message
                        .labels
                        .iter()
                        .any(|label| label == "TRASH" || label == "SPAM")
                })
            {
                return_to_inbox(tx, &item)?;
            }
            return Ok(());
        }
        if item.state != "active" {
            return Ok(());
        }
        if messages.iter().any(|message| {
            message
                .labels
                .iter()
                .any(|label| label == "TRASH" || label == "SPAM")
        }) {
            item.state = "cancelled".into();
            store(tx, &item, true)?;
            return Ok(());
        }
        let mine = super::sending_addresses_for_account(tx, account);
        let new_reply = messages.iter().any(|message| {
            !item.message_ids.contains(&message.id)
                && !message.labels.iter().any(|label| label == "SENT")
                && !mine.contains(&super::normalize_sender(&message.from).0)
        });
        if new_reply {
            item.state = "returned".into();
            store(tx, &item, true)?;
            return_to_inbox(tx, &item)?;
        } else if pending
            && DateTime::parse_from_rfc3339(&item.until).is_ok_and(|until| until > Utc::now())
        {
            Self::apply_mutation(
                tx,
                &ThreadMutation::Archive {
                    thread_id: id.to_string(),
                    value: true,
                },
            )?;
        }
        Ok(())
    }

    /// Due returns are run only after current provider evidence was fetched.
    #[cfg(test)]
    fn wake_due_snoozes(&self, account: &str, at: DateTime<Utc>) -> DbResult<bool> {
        let ids = self
            .list_snooze_records(Some(account))?
            .into_iter()
            .map(|record| record.provider_thread_id)
            .collect::<Vec<_>>();
        self.wake_verified_snoozes(account, at, &ids)
    }

    pub(crate) fn wake_verified_snoozes(
        &self,
        account: &str,
        at: DateTime<Utc>,
        verified: &[String],
    ) -> DbResult<bool> {
        self.with_transaction(|tx| {
            let mut statement = tx.prepare("SELECT record_json FROM conversation_snoozes WHERE account_id=?1 AND json_extract(record_json,'$.state')='active'")?;
            let rows = statement.query_map([account], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            let mut changed = false;
            for json in rows {
                let mut item: ConversationSnooze = serde_json::from_str(&json).map_err(serialization_error)?;
                if !verified.contains(&item.provider_thread_id) { continue; }
                if DateTime::parse_from_rfc3339(&item.until).map_err(|_| DatabaseError::invalid("Invalid snooze time"))? > at { continue; }
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)", [&item.id], |row| row.get(0))?;
                item.state = if exists { "returned" } else { "cancelled" }.into();
                store(tx, &item, true)?;
                if exists { return_to_inbox(tx, &item)?; }
                changed = true;
            }
            Ok(changed)
        })
    }

    pub(crate) fn snooze_refresh_ids(
        &self,
        account: &str,
        at: DateTime<Utc>,
    ) -> DbResult<Vec<String>> {
        let records = self.list_snooze_records(Some(account))?;
        self.with_connection(|connection| {
            let mut ids = Vec::new();
            for item in records {
                let pending: bool = connection.query_row(
                    "SELECT pending_apply FROM conversation_snoozes WHERE id=?1",
                    [&item.id],
                    |row| row.get(0),
                )?;
                let exists: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)",
                    [&item.id],
                    |row| row.get(0),
                )?;
                if (pending && item.state != "cancelled")
                    || (item.state == "active"
                        && (!exists
                            || DateTime::parse_from_rfc3339(&item.until)
                                .is_ok_and(|due| due <= at)))
                {
                    ids.push(item.provider_thread_id);
                }
            }
            Ok(ids)
        })
    }

    pub(crate) fn upsert_synced_snooze(&self, item: &ConversationSnooze) -> DbResult<()> {
        item.validate()?;
        self.with_transaction(|tx| {
            if get(tx, &item.id)?.as_ref() == Some(item) {
                return Ok(());
            }
            store(tx, item, false)?;
            tx.execute(
                "UPDATE conversation_snoozes SET pending_apply=?2 WHERE id=?1",
                params![item.id, item.state != "cancelled"],
            )?;
            Ok(())
        })
    }

    pub(super) fn cancel_deleted_snooze(tx: &Transaction<'_>, id: &str) -> DbResult<()> {
        if let Some(mut item) = get(tx, id)? {
            if item.state != "cancelled" {
                item.state = "cancelled".into();
                store(tx, &item, true)?;
            }
            tx.execute(
                "UPDATE conversation_snoozes SET pending_apply=0 WHERE id=?1",
                [id],
            )?;
        }
        Ok(())
    }

    pub(crate) fn delete_synced_snooze(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM conversation_snoozes WHERE id=?1", [id])?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "tests/snoozes.rs"]
mod tests;
