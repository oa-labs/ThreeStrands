use super::*;
use crate::db::test_support::database;
use rusqlite::OptionalExtension;

#[test]
fn database_combinators_keep_sqlite_errors_typed_and_roll_back() {
    let database = database();
    let error = database
        .with_connection(|connection| {
            connection
                .execute("SELECT missing_database_table", [])
                .map(|_| ())
                .map_err(DatabaseError::from)
        })
        .unwrap_err();
    assert!(matches!(error, DatabaseError::Sqlite(_)));

    let result: DbResult<()> = database.with_transaction(|transaction| {
        transaction
            .execute(
                "INSERT INTO compose_settings(key, value) VALUES ('rollback-test', 'value')",
                [],
            )
            .map_err(DatabaseError::from)?;
        Err(DatabaseError::Message("test rollback".into()))
    });
    assert!(matches!(result, Err(DatabaseError::Message(_))));
    let present: Option<String> = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT value FROM compose_settings WHERE key='rollback-test'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(present, None);
}
