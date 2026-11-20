//! Pending Android editor text. This table belongs to the installation and
//! is intentionally outside all replica projections and exports.

use serde::{Deserialize, Serialize};

use super::{serialization_error, Database, DbResult};

#[derive(Serialize, Deserialize)]
pub(crate) struct MobileDraft {
    title: String,
    body: String,
    base: Option<String>,
}

impl Database {
    pub(crate) fn initialize_mobile_drafts(&self) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "CREATE TABLE IF NOT EXISTS mobile_note_drafts (
                    target TEXT PRIMARY KEY,
                    draft TEXT NOT NULL
                )",
                [],
            )?;
            Ok(())
        })
    }

    pub(crate) fn mobile_draft(&self, key: &str) -> DbResult<Option<MobileDraft>> {
        use rusqlite::OptionalExtension;
        self.with_connection(|connection| {
            let draft: Option<String> = connection
                .query_row(
                    "SELECT draft FROM mobile_note_drafts WHERE target=?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()?;
            draft
                .map(|value| serde_json::from_str(&value).map_err(serialization_error))
                .transpose()
        })
    }

    pub(crate) fn write_mobile_draft(
        &self,
        key: &str,
        draft: Option<MobileDraft>,
        expected: Option<MobileDraft>,
    ) -> DbResult<()> {
        let encode =
            |draft: MobileDraft| serde_json::to_string(&draft).map_err(serialization_error);
        let draft = draft.map(encode).transpose()?;
        let expected = expected.map(encode).transpose()?;
        self.with_connection(|connection| {
            match (draft, expected) {
                (Some(draft), Some(expected)) => connection.execute(
                    "UPDATE mobile_note_drafts SET draft=?2 WHERE target=?1 AND draft=?3",
                    rusqlite::params![key, draft, expected],
                )?,
                (None, Some(expected)) => connection.execute(
                    "DELETE FROM mobile_note_drafts WHERE target=?1 AND draft=?2",
                    rusqlite::params![key, expected],
                )?,
                (Some(draft), None) => connection.execute(
                    "INSERT INTO mobile_note_drafts(target,draft) VALUES(?1,?2)
                     ON CONFLICT(target) DO UPDATE SET draft=excluded.draft",
                    rusqlite::params![key, draft],
                )?,
                (None, None) => {
                    connection.execute("DELETE FROM mobile_note_drafts WHERE target=?1", [key])?
                }
            };
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_save_cannot_clear_a_newer_durable_draft() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("notes.sqlite");
        let draft = |body: &str| MobileDraft {
            title: "Library".into(),
            body: body.into(),
            base: None,
        };
        {
            let database = Database::open(&path).unwrap();
            database.initialize_mobile_drafts().unwrap();
            database
                .write_mobile_draft("note", Some(draft("Earlier")), None)
                .unwrap();
            database
                .write_mobile_draft("note", Some(draft("New typing")), None)
                .unwrap();
            database
                .write_mobile_draft("note", None, Some(draft("Earlier")))
                .unwrap();
            assert!(database.list_recent_notes(10).unwrap().is_empty());
        }
        {
            let database = Database::open(&path).unwrap();
            let restored = database.mobile_draft("note").unwrap().unwrap();
            assert_eq!(restored.body, "New typing");
            database
                .write_mobile_draft("note", None, Some(restored))
                .unwrap();
            assert!(database.mobile_draft("note").unwrap().is_none());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
