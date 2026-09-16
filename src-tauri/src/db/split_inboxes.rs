//! Split-inbox rule persistence.

use super::*;

impl Database {
    pub fn list_split_inboxes(&self) -> Result<Vec<SplitInbox>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, match_kind, match_value, sort_order, created_at
                 FROM split_inboxes ORDER BY sort_order",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([], split_inbox_from_row)
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }
}
