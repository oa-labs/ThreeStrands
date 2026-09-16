//! Thread persistence.

use super::*;

impl Database {
    /// `account_id` merges every account when absent, or scopes to one account.
    pub fn list_threads(&self, account_id: Option<&str>) -> Result<Vec<Thread>, String> {
        self.list_threads_where(account_id, "archived = 0 AND trashed = 0")
    }

    pub fn list_all_mail(&self, account_id: Option<&str>) -> Result<Vec<Thread>, String> {
        self.list_threads_where(account_id, "trashed = 0")
    }

    pub fn list_trash(&self, account_id: Option<&str>) -> Result<Vec<Thread>, String> {
        self.list_threads_where(account_id, "trashed = 1")
    }

    pub fn count_unread_inbox(&self) -> Result<i64, String> {
        self.connection()?
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE archived = 0 AND trashed = 0 AND unread = 1",
                [],
                |row| row.get(0),
            )
            .map_err(display_error)
    }

    pub fn list_unread_counts(&self) -> Result<HashMap<String, i64>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT account_id, COUNT(*)
                 FROM threads
                 WHERE archived = 0 AND trashed = 0 AND unread = 1
                 GROUP BY account_id",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(display_error)?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(display_error)
    }
}
