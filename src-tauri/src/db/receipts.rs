//! Operation receipts: this device's record of a guarded write it committed,
//! written in the same transaction as the write. A retry with the same
//! operation id, such as after a lost IPC response, finds the receipt and
//! returns the first result instead of writing again. Receipts are local:
//! they never sync and aren't part of a settings export, so they don't
//! promise exactly-once handling across devices; saved note content does
//! that job there.
use super::{DatabaseError, DbResult};
use crate::meeting_policy::{MAX_OPERATION_RECEIPTS, OPERATION_RECEIPT_RETENTION_DAYS};
use rusqlite::{params, OptionalExtension, Transaction};

/// A committed operation as its receipt recorded it.
pub(crate) struct Receipt {
    pub(crate) kind: String,
    pub(crate) scope: String,
    pub(crate) result: String,
    pub(crate) undone_at: Option<String>,
}

/// An operation id: a UUID chosen by the client before its first attempt.
pub(crate) fn checked_operation_id(id: &str) -> DbResult<()> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| DatabaseError::invalid("Invalid operation id"))
}

pub(crate) fn find(tx: &Transaction<'_>, operation_id: &str) -> DbResult<Option<Receipt>> {
    Ok(tx
        .query_row(
            "SELECT kind, scope, result, undone_at FROM operation_receipts WHERE operation_id=?1",
            [operation_id],
            |row| {
                Ok(Receipt {
                    kind: row.get(0)?,
                    scope: row.get(1)?,
                    result: row.get(2)?,
                    undone_at: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// The receipt for `operation_id` when it records this kind of operation on
/// this scope. An id reused for anything else is refused rather than
/// returning an unrelated result.
pub(crate) fn matching(
    tx: &Transaction<'_>,
    operation_id: &str,
    kind: &str,
    scope: &str,
) -> DbResult<Option<Receipt>> {
    match find(tx, operation_id)? {
        Some(receipt) if receipt.kind == kind && receipt.scope == scope => Ok(Some(receipt)),
        Some(_) => Err(DatabaseError::invalid(
            "This change was already used for something else. Try again.",
        )),
        None => Ok(None),
    }
}

/// Records a committed operation, then forgets receipts past the retention
/// period or beyond the most kept.
pub(crate) fn record(
    tx: &Transaction<'_>,
    operation_id: &str,
    kind: &str,
    scope: &str,
    note_id: Option<&str>,
    result: &str,
) -> DbResult<()> {
    let now = chrono::Utc::now();
    tx.execute(
        "INSERT INTO operation_receipts(operation_id, kind, scope, note_id, result, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![operation_id, kind, scope, note_id, result, now.to_rfc3339()],
    )?;
    let cutoff = now - chrono::Duration::days(OPERATION_RECEIPT_RETENTION_DAYS);
    tx.execute(
        "DELETE FROM operation_receipts WHERE created_at < ?1",
        [cutoff.to_rfc3339()],
    )?;
    tx.execute(
        "DELETE FROM operation_receipts WHERE operation_id NOT IN
           (SELECT operation_id FROM operation_receipts ORDER BY created_at DESC, rowid DESC LIMIT ?1)",
        [MAX_OPERATION_RECEIPTS as i64],
    )?;
    Ok(())
}

/// Marks an operation undone, keeping its receipt so a repeated undo is a
/// no-op and a repeated apply still returns the first result.
pub(crate) fn mark_undone(tx: &Transaction<'_>, operation_id: &str) -> DbResult<()> {
    tx.execute(
        "UPDATE operation_receipts SET undone_at=?2 WHERE operation_id=?1",
        params![operation_id, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}
