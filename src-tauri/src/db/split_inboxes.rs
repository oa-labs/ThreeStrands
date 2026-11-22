//! Split-inbox rule persistence.

use super::DatabaseError;
use super::{normalize_sender, Database, DbResult};
use crate::models::{SplitInbox, Thread, ThreadPage};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The shared source ID remains the replication value. A binding applies only
// to that exact rule revision and to the provider installed on this device.
const LOCAL_SPLIT_LIST: &str = "SELECT s.id, s.name, s.match_kind,
            CASE WHEN b.split_id IS NULL THEN s.match_value ELSE COALESCE(b.label_id,'') END,
            s.sort_order, s.created_at, s.account_id
       FROM split_inboxes s
       LEFT JOIN accounts a ON a.email=s.account_id
       LEFT JOIN split_inbox_label_bindings b ON b.split_id=s.id AND b.provider=a.provider
          AND s.match_kind='label' AND b.source_label_id=s.match_value";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SplitLabelBinding {
    pub provider: String,
    pub label_id: Option<String>,
}

pub(super) fn bind_split_label(
    connection: &Connection,
    id: &str,
    source_label_id: &str,
    provider: &str,
    label_id: Option<&str>,
) -> DbResult<()> {
    connection.execute(
        "INSERT INTO split_inbox_label_bindings(split_id,provider,source_label_id,label_id)
         VALUES(?1,?2,?3,?4) ON CONFLICT(split_id,provider)
         DO UPDATE SET source_label_id=excluded.source_label_id,label_id=excluded.label_id",
        params![id, provider, source_label_id, label_id],
    )?;
    Ok(())
}

pub(super) fn local_split_inbox_on(connection: &Connection, id: &str) -> DbResult<SplitInbox> {
    Ok(connection
        .query_row(
            &format!("{LOCAL_SPLIT_LIST} WHERE s.id=?1"),
            [id],
            split_inbox_from_row,
        )
        .optional()?
        .ok_or(DatabaseError::NotFound("Split inbox"))?)
}

impl Database {
    pub fn list_split_inboxes(&self) -> DbResult<Vec<SplitInbox>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, match_kind, match_value, sort_order, created_at, account_id
                 FROM split_inboxes ORDER BY sort_order",
            )?;
            let rows = statement.query_map([], split_inbox_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub(crate) fn list_local_split_inboxes(&self) -> DbResult<Vec<SplitInbox>> {
        self.with_connection(|connection| {
            let query = format!("{LOCAL_SPLIT_LIST} ORDER BY s.sort_order");
            let mut statement = connection.prepare(&query)?;
            let rows = statement.query_map([], split_inbox_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub(crate) fn local_split_inbox(&self, id: &str) -> DbResult<SplitInbox> {
        self.with_connection(|connection| local_split_inbox_on(connection, id))
    }

    pub(crate) fn split_label_bindings(&self, id: &str) -> DbResult<Vec<SplitLabelBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT b.provider,b.label_id FROM split_inbox_label_bindings b
                 JOIN split_inboxes s ON s.id=b.split_id AND b.source_label_id=s.match_value
                 WHERE s.id=?1 AND s.match_kind='label' ORDER BY b.provider",
            )?;
            let rows = statement.query_map([id], |row| {
                Ok(SplitLabelBinding {
                    provider: row.get(0)?,
                    label_id: row.get(1)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn create_split_inbox(
        &self,
        name: &str,
        match_kind: &str,
        match_value: &str,
        account_id: &str,
    ) -> DbResult<SplitInbox> {
        let name = name.trim();
        let match_value = match_value.trim();
        if name.is_empty() {
            return Err(DatabaseError::invalid("Split inbox name cannot be empty"));
        }
        if match_value.is_empty() {
            return Err(DatabaseError::invalid(
                "Split inbox match value cannot be empty",
            ));
        }
        if !matches!(match_kind, "domain" | "label" | "pattern") {
            return Err(DatabaseError::invalid("Unknown split inbox match kind"));
        }
        // Domains and patterns are matched case-insensitively against
        // lowercased addresses (see `split_inbox_matches`), so normalize
        // once here rather than on every match. Label ids are case-sensitive
        // Gmail identifiers and must be stored as-is.
        let match_value = if match_kind == "label" {
            match_value.to_string()
        } else {
            match_value.to_ascii_lowercase()
        };
        self.with_transaction(|transaction| {
            let sort_order: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM split_inboxes",
                [],
                |row| row.get(0),
            )?;
            let id = Uuid::new_v4().to_string();
            let created_at = Utc::now().to_rfc3339();
            insert_split_inbox_row(
                transaction,
                SplitInboxRow {
                    id: &id,
                    name,
                    match_kind,
                    match_value: &match_value,
                    sort_order,
                    created_at: &created_at,
                    account_id,
                },
            )?;
            Ok(SplitInbox {
                id,
                name: name.to_string(),
                match_kind: match_kind.to_string(),
                match_value,
                sort_order,
                created_at,
                account_id: account_id.to_string(),
            })
        })
    }

    pub fn update_split_inbox(&self, id: &str, name: &str) -> DbResult<SplitInbox> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DatabaseError::invalid("Split inbox name cannot be empty"));
        }
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE split_inboxes SET name = ?1 WHERE id = ?2",
                params![name, id],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Split inbox"));
            }
            Ok(connection.query_row(
                "SELECT id, name, match_kind, match_value, sort_order, created_at, account_id
                 FROM split_inboxes WHERE id = ?1",
                [id],
                split_inbox_from_row,
            )?)
        })
    }

    /// A replacement for a rule authored through another provider is a
    /// local choice. Replacing a shared label from its own provider still
    /// updates the replicated rule, as it did before.
    pub(crate) fn relink_split_inbox_label(
        &self,
        id: &str,
        label_id: &str,
    ) -> DbResult<(SplitInbox, bool)> {
        self.with_transaction(|tx| {
            let (source_id, provider, locally_bound): (String, String, bool) = tx.query_row(
                "SELECT s.match_value, COALESCE(a.provider,'gmail'),
                      EXISTS(SELECT 1 FROM split_inbox_label_bindings b WHERE b.split_id=s.id
                        AND b.provider=a.provider AND b.source_label_id=s.match_value)
                 FROM split_inboxes s LEFT JOIN accounts a ON a.email=s.account_id
                 WHERE s.id=?1 AND s.match_kind='label' AND EXISTS(
                    SELECT 1 FROM account_labels l
                    WHERE l.account_id=s.account_id AND l.label_id=?2 AND l.kind IN ('user','folder')
                 )",
                params![id, label_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?.ok_or_else(|| DatabaseError::invalid(
                "Choose an available label from this Split Inbox’s account.",
            ))?;
            let imap_id = ["folder:", "lf:", "imap-label:", "kw:"]
                .iter().any(|prefix| source_id.starts_with(prefix));
            let foreign = (provider == "imap" && source_id.starts_with("Label_"))
                || (provider == "gmail" && imap_id);
            let changed_shared_rule = !locally_bound && !foreign;
            if changed_shared_rule {
                if source_id != label_id {
                    tx.execute("UPDATE split_inboxes SET match_value=?2 WHERE id=?1", params![id, label_id])?;
                    tx.execute("DELETE FROM split_inbox_label_bindings WHERE split_id=?1", [id])?;
                }
            } else {
                bind_split_label(tx, id, &source_id, &provider, Some(label_id))?;
            }
            Ok((local_split_inbox_on(tx, id)?, changed_shared_rule))
        })
    }

    pub fn delete_split_inbox(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM split_inboxes WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    pub fn reorder_split_inboxes(&self, ordered_ids: &[String]) -> DbResult<()> {
        self.with_transaction(|transaction| {
            for (index, id) in ordered_ids.iter().enumerate() {
                transaction.execute(
                    "UPDATE split_inboxes SET sort_order = ?1 WHERE id = ?2",
                    params![index as i64, id],
                )?;
            }
            Ok(())
        })
    }

    /// Filters the same unarchived/untrashed base set `list_threads` uses
    /// down to one split inbox's rule, then paginates in memory. That base
    /// set is realistically bounded (an actively-triaged inbox), so
    /// re-filtering it on every page load is simpler than building true
    /// SQL-level cursor pagination over a JSON column with no useful index.
    /// Always scoped to the rule's own account — a split inbox belongs to
    /// one account, so there's no separate `account_id` to pass in.
    pub fn list_split_inbox_page(
        &self,
        split_inbox_id: &str,
        offset: usize,
        limit: usize,
    ) -> DbResult<ThreadPage> {
        let rule = self.local_split_inbox(split_inbox_id)?;
        let matched: Vec<Thread> = self
            .list_threads(Some(&rule.account_id))?
            .into_iter()
            .filter(|thread| split_inbox_matches(&rule, thread))
            .collect();
        let page_limit = limit.min(200);
        let has_more = matched.len() > offset.saturating_add(page_limit);
        let threads = matched.into_iter().skip(offset).take(page_limit).collect();
        Ok(ThreadPage { threads, has_more })
    }
}

fn split_inbox_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SplitInbox> {
    Ok(SplitInbox {
        id: row.get(0)?,
        name: row.get(1)?,
        match_kind: row.get(2)?,
        match_value: row.get(3)?,
        sort_order: row.get(4)?,
        created_at: row.get(5)?,
        account_id: row.get(6)?,
    })
}

/// `match_value` is normalized to lowercase at creation time for `domain`
/// and `pattern` rules (see `Database::create_split_inbox`), so only the
/// participant side needs lowercasing here.
pub(super) fn split_inbox_matches(rule: &SplitInbox, thread: &Thread) -> bool {
    match rule.match_kind.as_str() {
        "domain" => thread
            .participants
            .iter()
            .any(|participant| normalize_sender(participant).1 == rule.match_value),
        "label" => {
            !rule.match_value.is_empty()
                && thread.labels.iter().any(|label| label == &rule.match_value)
        }
        "pattern" => thread
            .participants
            .iter()
            .any(|participant| normalize_sender(participant).0.contains(&rule.match_value)),
        _ => false,
    }
}

impl Database {
    /// Stores a split inbox another device wrote. An existing row keeps its
    /// `created_at`; `updated_at` records when it arrived here.
    pub(crate) fn upsert_synced_split(&self, item: SplitInbox) -> DbResult<()> {
        self.with_transaction(|connection| {
            connection.execute(
                "DELETE FROM split_inbox_label_bindings WHERE split_id=?1 AND EXISTS(
                   SELECT 1 FROM split_inboxes WHERE id=?1 AND
                     (match_kind<>?2 OR match_value<>?3 OR account_id<>?4))",
                params![item.id, item.match_kind, item.match_value, item.account_id],
            )?;
            connection.execute(
                "INSERT INTO split_inboxes(id,name,match_kind,match_value,sort_order,created_at,account_id,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET name=excluded.name,match_kind=excluded.match_kind,match_value=excluded.match_value,sort_order=excluded.sort_order,account_id=excluded.account_id,updated_at=excluded.updated_at",
                params![
                    item.id,
                    item.name,
                    item.match_kind,
                    item.match_value,
                    item.sort_order,
                    item.created_at,
                    item.account_id,
                    Utc::now().to_rfc3339()
                ],
            )?;
            Ok(())
        })
    }

    /// Removes a split inbox another device deleted.
    pub(crate) fn delete_synced_split(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM split_inboxes WHERE id=?1", [id])?;
            Ok(())
        })
    }
}

/// A split inbox as stored. Named fields keep its six text columns from
/// trading places unnoticed.
pub(super) struct SplitInboxRow<'a> {
    pub(super) id: &'a str,
    pub(super) name: &'a str,
    pub(super) match_kind: &'a str,
    pub(super) match_value: &'a str,
    pub(super) sort_order: i64,
    pub(super) created_at: &'a str,
    pub(super) account_id: &'a str,
}

/// Stores a new split inbox row. Used by local creation and settings import.
pub(super) fn insert_split_inbox_row(
    connection: &rusqlite::Connection,
    row: SplitInboxRow<'_>,
) -> DbResult<()> {
    let SplitInboxRow {
        id,
        name,
        match_kind,
        match_value,
        sort_order,
        created_at,
        account_id,
    } = row;
    connection.execute(
        "INSERT INTO split_inboxes(id, name, match_kind, match_value, sort_order, created_at, account_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, name, match_kind, match_value, sort_order, created_at, account_id],
    )?;
    Ok(())
}

/// Settings import: clears every split inbox before the file's are added.
pub(super) fn delete_all_split_inboxes(connection: &rusqlite::Connection) -> DbResult<()> {
    connection.execute("DELETE FROM split_inboxes", [])?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/split_inboxes.rs"]
mod tests;
