//! Compose settings: the most recently chosen sending account and how long
//! locally cached mail is kept.

use super::{Database, DatabaseError, DbResult};
use crate::correspondence::validate_retention_days;
use rusqlite::{params, OptionalExtension};

impl Database {
    pub fn compose_identity(&self) -> DbResult<String> {
        self.with_connection(|c| {
            c.query_row(
                "SELECT value FROM compose_settings WHERE key='identity'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                DatabaseError::invalid("Connect Gmail once to establish your sender identity")
            })
        })
    }
    pub fn set_compose_identity(&self, identity: &str) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute("INSERT INTO compose_settings VALUES ('identity',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [identity])?;
            Ok(())
        })
    }
    /// Establishes an initial sending identity without replacing the account
    /// the user most recently chose in an existing installation.
    pub fn ensure_compose_identity(&self, identity: &str) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute(
                "INSERT INTO compose_settings(key, value) VALUES ('identity', ?1)
                 ON CONFLICT(key) DO NOTHING",
                [identity],
            )?;
            Ok(())
        })
    }
    /// How long locally cached mail is kept before `prune_expired_threads`
    /// removes it. New installations default to 90 days; existing unset
    /// settings and an explicit Forever choice remain unlimited (`None`).
    pub fn retention_days(&self) -> DbResult<Option<i64>> {
        self.read_retention_days(true)
    }

    /// Only chosen values belong in sync reconciliation, not a new device's
    /// untouched installation default. Explicit Forever writes are recorded
    /// by the command, as before.
    pub(crate) fn chosen_retention_days(&self) -> DbResult<Option<i64>> {
        self.read_retention_days(false)
    }

    fn read_retention_days(&self, include_default: bool) -> DbResult<Option<i64>> {
        self.connection()?
            .query_row(
                "SELECT value FROM compose_settings
                 WHERE key='retention_days' OR (?1 AND key='retention_days_default')
                 ORDER BY CASE key WHEN 'retention_days' THEN 0 ELSE 1 END LIMIT 1",
                [include_default],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<i64>().map_err(|e| DatabaseError::corrupt(e.to_string())))
            .transpose()
    }
    pub fn set_retention_days(&self, days: Option<i64>) -> DbResult<()> {
        validate_retention_days(days).map_err(DatabaseError::Validation)?;
        self.with_transaction(|transaction| write_retention_days(transaction, days))
    }
}

/// Stores the retention choice, `None` for Forever, and drops the new
/// installation default so it cannot reappear underneath the choice. Used
/// by Settings and by settings import, which does not validate first.
pub(super) fn write_retention_days(connection: &rusqlite::Connection, days: Option<i64>) -> DbResult<()> {
    connection.execute("DELETE FROM compose_settings WHERE key='retention_days_default'", [])?;
    match days {
        Some(days) => connection.execute(
            "INSERT INTO compose_settings VALUES ('retention_days',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![days.to_string()],
        ),
        None => connection.execute("DELETE FROM compose_settings WHERE key='retention_days'", []),
    }?;
    Ok(())
}
