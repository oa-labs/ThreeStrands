//! Snippet template persistence.

use super::*;

impl Database {
    pub fn list_snippets(&self) -> Result<Vec<Snippet>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id, name, body, created_at FROM snippets ORDER BY name COLLATE NOCASE")
            .map_err(display_error)?;
        let rows = statement
            .query_map([], snippet_from_row)
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }
}
