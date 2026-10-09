//! Split-inbox rule persistence.

use super::DatabaseError;
use super::{normalize_sender, Database, DbResult};
use crate::models::{SplitInbox, Thread, ThreadPage};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

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
            return Err(DatabaseError::invalid("Split inbox match value cannot be empty"));
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
            let sort_order: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM split_inboxes",
                    [],
                    |row| row.get(0),
                )?;
            let id = Uuid::new_v4().to_string();
            let created_at = Utc::now().to_rfc3339();
            transaction
                .execute(
                    "INSERT INTO split_inboxes(id, name, match_kind, match_value, sort_order, created_at, account_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![id, name, match_kind, match_value, sort_order, created_at, account_id],
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
        let rule = self
            .with_connection(|connection| {
                Ok(connection
                    .query_row(
                        "SELECT id, name, match_kind, match_value, sort_order, created_at, account_id
                         FROM split_inboxes WHERE id = ?1",
                        [split_inbox_id],
                        split_inbox_from_row,
                    )
                    .optional()?)
            })?
            .ok_or(DatabaseError::NotFound("Split inbox"))?;
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
        "label" => thread.labels.iter().any(|label| label == &rule.match_value),
        "pattern" => thread
            .participants
            .iter()
            .any(|participant| normalize_sender(participant).0.contains(&rule.match_value)),
        _ => false,
    }
}

#[cfg(test)]
#[path = "tests/split_inboxes.rs"]
mod tests;
