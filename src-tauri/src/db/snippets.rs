//! Snippet template persistence.

use super::DatabaseError;
use super::{Database, DbResult};
use crate::models::Snippet;
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

impl Database {
    pub fn list_snippets(&self) -> DbResult<Vec<Snippet>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, body, created_at FROM snippets ORDER BY name COLLATE NOCASE",
            )?;
            let rows = statement.query_map([], snippet_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn create_snippet(&self, name: &str, body: &str) -> DbResult<Snippet> {
        let name = name.trim();
        let body = body.trim();
        if name.is_empty() {
            return Err(DatabaseError::invalid("Snippet name cannot be empty"));
        }
        if body.is_empty() {
            return Err(DatabaseError::invalid("Snippet body cannot be empty"));
        }
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO snippets(id, name, body, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, name, body, created_at],
            )?;
            Ok(())
        })?;
        Ok(Snippet {
            id,
            name: name.to_string(),
            body: body.to_string(),
            created_at,
        })
    }

    pub fn update_snippet(&self, id: &str, name: &str, body: &str) -> DbResult<Snippet> {
        let name = name.trim();
        let body = body.trim();
        if name.is_empty() {
            return Err(DatabaseError::invalid("Snippet name cannot be empty"));
        }
        if body.is_empty() {
            return Err(DatabaseError::invalid("Snippet body cannot be empty"));
        }
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE snippets SET name = ?1, body = ?2 WHERE id = ?3",
                params![name, body, id],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Snippet"));
            }
            Ok(connection.query_row(
                "SELECT id, name, body, created_at FROM snippets WHERE id = ?1",
                [id],
                snippet_from_row,
            )?)
        })
    }

    pub fn delete_snippet(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM snippets WHERE id = ?1", [id])?;
            Ok(())
        })
    }
}

fn snippet_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Snippet> {
    Ok(Snippet {
        id: row.get(0)?,
        name: row.get(1)?,
        body: row.get(2)?,
        created_at: row.get(3)?,
    })
}
