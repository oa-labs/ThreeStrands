//! Tests for the local label-catalog cache (`db/labels.rs`).

use super::{
    cache_label_listing, check_label_catalog_size, LabelCatalogRejection, MAX_LABELS_PER_ACCOUNT,
};
use crate::db::Database;
use crate::models::Label;

fn label(id: &str, name: &str) -> Label {
    Label {
        id: id.into(),
        name: name.into(),
        kind: "user".into(),
    }
}

fn catalog(database: &Database, account: &str) -> Vec<(String, String, String)> {
    database
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT label_id, name, kind FROM account_labels WHERE account_id = ?1 ORDER BY label_id",
            )?;
            let rows = statement
                .query_map([account], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap()
}

#[test]
fn replace_account_labels_replaces_the_whole_catalog() {
    let database = Database::open_memory();
    database
        .replace_account_labels("a@x", &[label("l1", "Work"), label("l2", "Personal")])
        .unwrap();
    assert_eq!(catalog(&database, "a@x").len(), 2);

    // Replacing with a smaller set drops the labels no longer listed.
    database
        .replace_account_labels("a@x", &[label("l1", "Work renamed")])
        .unwrap();
    let rows = catalog(&database, "a@x");
    assert_eq!(rows, vec![("l1".into(), "Work renamed".into(), "user".into())]);
}

#[test]
fn replace_account_labels_is_per_account() {
    let database = Database::open_memory();
    // The SAME label_id in two accounts stays separate and independent.
    database
        .replace_account_labels("a@x", &[label("shared", "A name")])
        .unwrap();
    database
        .replace_account_labels("b@x", &[label("shared", "B name")])
        .unwrap();
    assert_eq!(catalog(&database, "a@x"), vec![("shared".into(), "A name".into(), "user".into())]);
    assert_eq!(catalog(&database, "b@x"), vec![("shared".into(), "B name".into(), "user".into())]);

    // Replacing one account's catalog leaves the other's untouched.
    database.replace_account_labels("a@x", &[]).unwrap();
    assert!(catalog(&database, "a@x").is_empty());
    assert_eq!(catalog(&database, "b@x").len(), 1);
}

#[test]
fn upsert_and_delete_single_labels() {
    let database = Database::open_memory();
    database.upsert_account_label("a@x", &label("l1", "Work")).unwrap();
    assert_eq!(catalog(&database, "a@x"), vec![("l1".into(), "Work".into(), "user".into())]);
    // Upsert on the same id updates in place rather than duplicating.
    database.upsert_account_label("a@x", &label("l1", "Work (renamed)")).unwrap();
    assert_eq!(catalog(&database, "a@x"), vec![("l1".into(), "Work (renamed)".into(), "user".into())]);
    database.delete_account_label("a@x", "l1").unwrap();
    assert!(catalog(&database, "a@x").is_empty());
    // Deleting an id that is not cached is a harmless no-op.
    database.delete_account_label("a@x", "l1").unwrap();
}

#[test]
fn label_ids_named_is_case_insensitive_and_can_return_several_ids() {
    let database = Database::open_memory();
    database
        .replace_account_labels(
            "a@x",
            &[label("l1", "Work"), label("l2", "work"), label("l3", "Other")],
        )
        .unwrap();
    // Case-insensitive (ASCII NOCASE), and several ids when several labels
    // share a name — sorted for determinism.
    assert_eq!(
        database.label_ids_named("a@x", "WORK").unwrap(),
        vec!["l1".to_string(), "l2".to_string()]
    );
    assert_eq!(database.label_ids_named("a@x", "other").unwrap(), vec!["l3".to_string()]);
    // An unknown name returns nothing.
    assert!(database.label_ids_named("a@x", "nope").unwrap().is_empty());
    // Scoped per account.
    assert!(database.label_ids_named("b@x", "Work").unwrap().is_empty());
}

#[test]
fn label_ids_named_matches_special_characters_literally_not_as_patterns() {
    let database = Database::open_memory();
    // Names containing SQL-wildcard and quote characters. The lookup is
    // `=`/NOCASE, not LIKE, so these are matched literally.
    database
        .replace_account_labels(
            "a@x",
            &[
                label("pct", "a%b"),
                label("usc", "a_b"),
                label("sq", "a'b"),
                label("dq", "a\"b"),
                label("bs", "a\\b"),
                label("plain", "aXb"),
            ],
        )
        .unwrap();
    // `%` and `_` are literal, not wildcards: each matches only its own name.
    assert_eq!(database.label_ids_named("a@x", "a%b").unwrap(), vec!["pct".to_string()]);
    assert_eq!(database.label_ids_named("a@x", "a_b").unwrap(), vec!["usc".to_string()]);
    // A literal single-char wildcard would match "aXb"; `=` does not.
    assert!(database.label_ids_named("a@x", "a%").unwrap().is_empty());
    assert!(database.label_ids_named("a@x", "a_").unwrap().is_empty());
    assert_eq!(database.label_ids_named("a@x", "a'b").unwrap(), vec!["sq".to_string()]);
    assert_eq!(database.label_ids_named("a@x", "a\"b").unwrap(), vec!["dq".to_string()]);
    assert_eq!(database.label_ids_named("a@x", "a\\b").unwrap(), vec!["bs".to_string()]);
}

#[test]
fn label_ids_named_treats_a_huge_name_as_a_non_match_not_a_panic() {
    let database = Database::open_memory();
    database.replace_account_labels("a@x", &[label("l1", "Work")]).unwrap();
    let huge = "z".repeat(100_000);
    assert!(database.label_ids_named("a@x", &huge).unwrap().is_empty());
}

#[test]
fn check_label_catalog_size_below_exact_and_above_the_limit() {
    let below = vec![label("l", "n"); MAX_LABELS_PER_ACCOUNT - 1];
    let exact = vec![label("l", "n"); MAX_LABELS_PER_ACCOUNT];
    let above = vec![label("l", "n"); MAX_LABELS_PER_ACCOUNT + 1];
    assert_eq!(check_label_catalog_size(&below), Ok(()));
    assert_eq!(check_label_catalog_size(&exact), Ok(()));
    assert_eq!(
        check_label_catalog_size(&above),
        Err(LabelCatalogRejection::TooMany(MAX_LABELS_PER_ACCOUNT + 1))
    );
}

#[test]
fn replace_account_labels_refuses_an_over_limit_catalog_without_touching_the_cache() {
    let database = Database::open_memory();
    // Distinct ids so the primary key does not collapse the rows.
    let ok: Vec<Label> = (0..MAX_LABELS_PER_ACCOUNT)
        .map(|i| label(&format!("l{i}"), "n"))
        .collect();
    database.replace_account_labels("a@x", &ok).unwrap();
    assert_eq!(catalog(&database, "a@x").len(), MAX_LABELS_PER_ACCOUNT);

    let over: Vec<Label> = (0..=MAX_LABELS_PER_ACCOUNT)
        .map(|i| label(&format!("x{i}"), "n"))
        .collect();
    assert!(database.replace_account_labels("a@x", &over).is_err());
    // The previous cache is untouched (not truncated, not emptied).
    assert_eq!(catalog(&database, "a@x").len(), MAX_LABELS_PER_ACCOUNT);
}

#[test]
fn cache_label_listing_caches_on_success_and_an_over_limit_listing_keeps_the_previous_cache() {
    let database = Database::open_memory();
    // Successful listing: cached.
    cache_label_listing(&database, "a@x", &[label("l1", "Work")]);
    assert_eq!(catalog(&database, "a@x"), vec![("l1".into(), "Work".into(), "user".into())]);

    // An over-limit listing is logged and swallowed, leaving the previous
    // cache in place (write-through never fails the user's action).
    let over: Vec<Label> = (0..=MAX_LABELS_PER_ACCOUNT)
        .map(|i| label(&format!("x{i}"), "n"))
        .collect();
    cache_label_listing(&database, "a@x", &over);
    assert_eq!(
        catalog(&database, "a@x"),
        vec![("l1".into(), "Work".into(), "user".into())],
        "an over-limit listing must not disturb the previous cache"
    );
}

// The `list_labels` command (an async Tauri command resolving a live provider
// and app state) is not callable from a unit test, so the "cache this
// listing" step was extracted into `cache_label_listing`, tested directly
// above: it caches a successful listing and leaves the cache untouched for an
// over-limit one. The command's own code additionally guarantees that a
// FAILED provider listing never reaches `cache_label_listing` (it returns the
// error before the call), so a provider error leaves the cache alone — that
// one line of ordering in the command is NOT covered by an automated test
// here. The `SyncService` create/update/delete write-through is covered in
// `sync.rs` tests.

#[test]
fn account_removal_purges_only_that_accounts_cached_labels() {
    let database = Database::open_memory();
    database.adopt_account("remove@x").unwrap();
    database.adopt_account("keep@x").unwrap();
    database.replace_account_labels("remove@x", &[label("l1", "Work")]).unwrap();
    database.replace_account_labels("keep@x", &[label("l2", "Keep")]).unwrap();

    database.remove_account("remove@x").unwrap();

    assert!(catalog(&database, "remove@x").is_empty(), "the removed account's cache is purged");
    assert_eq!(catalog(&database, "keep@x").len(), 1, "another account's cache is untouched");
}
