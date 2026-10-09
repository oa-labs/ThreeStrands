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
        Err(DatabaseError::invalid("test rollback"))
    });
    assert!(matches!(result, Err(DatabaseError::Validation(_))));
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

#[test]
fn rejected_input_and_missing_records_keep_distinct_categories() {
    let database = database();
    let missing = database.update_split_inbox("no-such-split", "Renamed").unwrap_err();
    assert!(matches!(missing, DatabaseError::NotFound("Split inbox")), "{missing}");
    assert_eq!(missing.to_string(), "Split inbox not found");

    let rejected = database.update_split_inbox("no-such-split", "  ").unwrap_err();
    assert!(matches!(rejected, DatabaseError::Validation(_)), "{rejected}");
    assert_eq!(rejected.to_string(), "Split inbox name cannot be empty");

    // The frontend still receives the user-facing sentence, unprefixed.
    assert_eq!(String::from(rejected), "Split inbox name cannot be empty");
}
