//! Snippet template persistence.

use super::*;

impl Database {
    pub fn list_snippets(&self) -> DbResult<Vec<Snippet>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id, name, body, created_at FROM snippets ORDER BY name COLLATE NOCASE")?;
            let rows = statement.query_map([], snippet_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}
