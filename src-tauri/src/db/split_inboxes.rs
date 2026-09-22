//! Split-inbox rule persistence.

use super::*;

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
}
