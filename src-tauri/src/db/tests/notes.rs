use super::*;
use crate::db::test_support::database;
use std::collections::HashSet;
use threestrands_sync_protocol::{note_body_bytes, MAX_NOTE_BODY_BYTES};

const DAY: &str = "2026-11-09";

fn synced() -> Database {
    let db = database();
    db.set_beta_features_enabled(true).unwrap();
    db
}

fn sync_into(peer: &Database, source: &Database) {
    let touched = peer
        .merge_replica_state(&source.load_replica_state().unwrap())
        .unwrap();
    peer.materialize_touched_entities(&touched).unwrap();
}

fn sync_both(a: &Database, b: &Database) {
    sync_into(a, b);
    sync_into(b, a);
    sync_into(a, b);
}

/// Each field of the note the replica holds, with every surviving value's
/// counter, so a test can see which fields a write touched.
fn replica_fields(db: &Database, id: &str) -> Vec<(String, i64)> {
    let connection = db.test_connection().unwrap();
    let mut statement = connection
        .prepare(
            "SELECT field, counter FROM sync_values WHERE entity_type='note' AND entity_id=?1
             ORDER BY field, counter",
        )
        .unwrap();
    let rows = statement
        .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

fn sql_count(db: &Database, sql: &str, id: &str) -> i64 {
    db.test_connection()
        .unwrap()
        .query_row(sql, [id], |row| row.get(0))
        .unwrap()
}

fn note_conflicts(db: &Database) -> Vec<(String, String)> {
    db.list_frontier_conflicts()
        .unwrap()
        .into_iter()
        .filter(|conflict| conflict.entity_type == "note")
        .map(|conflict| (conflict.entity_id, conflict.field))
        .collect()
}

#[test]
fn creating_and_editing_records_only_changed_fields_and_saving_unchanged_text_writes_nothing() {
    let db = synced();
    let note = db.create_note("  Project   plan ", "First draft").unwrap();
    assert_eq!(note.title, "Project plan");
    let created = replica_fields(&db, &note.id);
    let fields: Vec<&str> = created.iter().map(|(field, _)| field.as_str()).collect();
    assert_eq!(
        fields,
        ["_entity", "body", "createdAt", "id", "title", "trashed"]
    );

    let same = db
        .update_note(&note.id, "Project plan", "First draft")
        .unwrap();
    assert_eq!(same, note);
    assert_eq!(replica_fields(&db, &note.id), created);

    db.update_note(&note.id, "Project plan", "Second draft")
        .unwrap();
    let after = replica_fields(&db, &note.id);
    let counter = |rows: &[(String, i64)], field: &str| {
        rows.iter().find(|(name, _)| name == field).unwrap().1
    };
    assert!(counter(&after, "body") > counter(&created, "body"));
    for field in ["_entity", "createdAt", "id", "title", "trashed"] {
        assert_eq!(counter(&after, field), counter(&created, field), "{field}");
    }
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, "Second draft");
}

#[test]
fn titles_are_unique_among_notes_in_use_and_trashed_notes_cant_be_edited() {
    let db = database();
    let first = db.create_note("Plan", "").unwrap();
    assert!(db.create_note("plan", "").is_err());
    let second = db.create_note("Other", "").unwrap();
    assert!(db.update_note(&second.id, "PLAN", "").is_err());
    assert!(db.create_note("2026-11-09", "").is_err());
    assert!(db.create_note("a [link]", "").is_err());

    db.trash_note(&first.id).unwrap();
    assert!(db.update_note(&first.id, "Plan", "edit").is_err());
    let replacement = db.create_note("Plan", "").unwrap();
    assert_ne!(replacement.id, first.id);
    assert!(db.delete_note(&replacement.id).is_err());
    db.delete_note(&first.id).unwrap();
    assert!(db.get_note(&first.id).unwrap().is_none());
}

#[test]
fn restoring_a_note_whose_title_was_taken_meanwhile_is_refused() {
    let db = database();
    let first = db.create_note("Plan", "old").unwrap();
    db.trash_note(&first.id).unwrap();
    let replacement = db.create_note("plan", "new").unwrap();
    let error = db.restore_note(&first.id).unwrap_err().to_string();
    assert!(error.contains("Rename that note"), "{error}");
    assert!(db.get_note(&first.id).unwrap().unwrap().trashed);

    db.update_note(&replacement.id, "New plan", "new").unwrap();
    assert!(!db.restore_note(&first.id).unwrap().trashed);
}

#[test]
fn a_day_deleted_from_trash_can_be_written_again_on_every_device() {
    let (a, b) = (synced(), synced());
    a.save_daily_note(DAY, "first").unwrap();
    let id = format!("daily:{DAY}");
    a.trash_note(&id).unwrap();
    a.delete_note(&id).unwrap();
    sync_both(&a, &b);
    assert!(b.get_daily_note(DAY).unwrap().is_none());

    a.save_daily_note(DAY, "again").unwrap();
    sync_both(&a, &b);
    for db in [&a, &b] {
        let note = db
            .get_daily_note(DAY)
            .unwrap()
            .expect("the day's note is back");
        assert_eq!(note.body, "again");
        assert!(!note.trashed);
    }
}

#[test]
fn a_daily_note_exists_only_once_written_and_keeps_its_date_as_title() {
    let db = synced();
    assert!(db.save_daily_note(DAY, "").unwrap().is_none());
    assert!(db.get_daily_note(DAY).unwrap().is_none());
    assert!(replica_fields(&db, "daily:2026-11-09").is_empty());

    let note = db.save_daily_note(DAY, "Standup").unwrap().unwrap();
    assert_eq!(note.id, "daily:2026-11-09");
    assert_eq!(note.title, DAY);
    assert_eq!(note.daily_date.as_deref(), Some(DAY));
    let renamed = db
        .update_note(&note.id, "Something else", "Standup")
        .unwrap();
    assert_eq!(renamed.title, DAY);
    let fields: Vec<String> = replica_fields(&db, &note.id)
        .into_iter()
        .map(|(field, _)| field)
        .collect();
    assert!(!fields.contains(&"createdAt".to_string()));
    assert!(fields.contains(&"dailyDate".to_string()));
    assert!(db.get_daily_note("2026-02-30").is_err());
    assert!(db.save_daily_note("2026-13-01", "x").is_err());
}

#[test]
fn notes_reach_a_peer_with_links_search_and_deletion() {
    let (a, b) = (synced(), synced());
    let plan = a.create_note("Plan", "Ship the budget").unwrap();
    let daily = a
        .save_daily_note(DAY, "Talked about [[Plan|the plan]] and [[Missing]]")
        .unwrap()
        .unwrap();
    sync_into(&b, &a);

    assert_eq!(
        b.get_note(&plan.id).unwrap().unwrap().body,
        "Ship the budget"
    );
    assert_eq!(b.search_notes("budg", 10).unwrap()[0].id, plan.id);
    let links = b.note_links(&plan.id).unwrap();
    assert_eq!(links.backlinks.len(), 1);
    assert_eq!(links.backlinks[0].note_id, daily.id);
    assert_eq!(
        links.backlinks[0].context,
        "Talked about [[Plan|the plan]] and [[Missing]]"
    );
    assert_eq!(b.note_links(&daily.id).unwrap().unlinked, ["Missing"]);

    a.update_note(&plan.id, "Plan", "Ship the forecast")
        .unwrap();
    sync_into(&b, &a);
    assert!(b.search_notes("budget", 10).unwrap().is_empty());
    assert_eq!(b.search_notes("forecast", 10).unwrap().len(), 1);

    b.record_note_visit(&plan.id).unwrap();
    a.trash_note(&plan.id).unwrap();
    a.delete_note(&plan.id).unwrap();
    sync_into(&b, &a);
    assert!(b.get_note(&plan.id).unwrap().is_none());
    for table in ["note_links", "note_search", "note_visits"] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE note_id=?1");
        assert_eq!(sql_count(&b, &sql, &plan.id), 0, "{table}");
    }
    assert_eq!(
        b.note_links(&daily.id).unwrap().unlinked,
        ["Plan", "Missing"]
    );
}

#[test]
fn a_day_started_offline_on_two_devices_is_one_note_with_only_its_body_in_conflict() {
    let (a, b) = (synced(), synced());
    a.save_daily_note(DAY, "From A").unwrap();
    b.save_daily_note(DAY, "From B").unwrap();
    sync_both(&a, &b);

    for db in [&a, &b] {
        assert_eq!(db.list_recent_notes(10).unwrap().len(), 1);
        assert_eq!(
            note_conflicts(db),
            [("daily:2026-11-09".to_string(), "body".to_string())]
        );
    }
    let shown = a.get_daily_note(DAY).unwrap().unwrap().body;
    assert_eq!(b.get_daily_note(DAY).unwrap().unwrap().body, shown);

    // Editing now would discard the other version, so it waits for a choice.
    let error = a.save_daily_note(DAY, "typed over both").unwrap_err();
    assert_eq!(error.to_string(), CONFLICTED);
    assert_eq!(a.get_daily_note(DAY).unwrap().unwrap().body, shown);
    assert_eq!(note_conflicts(&a).len(), 1);

    // Choosing the version this device wasn't showing updates it here too.
    let other = if shown == "From A" {
        "From B"
    } else {
        "From A"
    };
    let conflict = a
        .list_frontier_conflicts()
        .unwrap()
        .into_iter()
        .find(|conflict| conflict.entity_type == "note")
        .unwrap();
    let chosen = conflict
        .candidates
        .iter()
        .find(|candidate| candidate.value == Some(serde_json::json!(other)))
        .unwrap();
    a.resolve_frontier_conflict(
        EntityType::Note,
        &conflict.entity_id,
        "body",
        &chosen.operation_id,
    )
    .unwrap();
    assert_eq!(a.get_daily_note(DAY).unwrap().unwrap().body, other);
    assert_eq!(a.search_notes(other, 10).unwrap().len(), 1);
    sync_into(&b, &a);
    assert_eq!(b.get_daily_note(DAY).unwrap().unwrap().body, other);
    assert!(note_conflicts(&a).is_empty());
    assert!(note_conflicts(&b).is_empty());
    a.save_daily_note(DAY, "settled").unwrap();
}

#[test]
fn an_edit_racing_trash_survives_in_the_trashed_note() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Draft", "v1").unwrap();
    sync_into(&b, &a);
    a.update_note(&note.id, "Draft", "v2 written while B trashed it")
        .unwrap();
    b.trash_note(&note.id).unwrap();
    sync_both(&a, &b);

    for db in [&a, &b] {
        let stored = db.get_note(&note.id).unwrap().unwrap();
        assert!(stored.trashed);
        assert!(stored.trashed_at.is_some());
        assert_eq!(stored.body, "v2 written while B trashed it");
        assert!(note_conflicts(db).is_empty());
        assert_eq!(db.list_trashed_notes(10).unwrap()[0].id, note.id);
        assert!(db.list_recent_notes(10).unwrap().is_empty());
    }
    b.restore_note(&note.id).unwrap();
    sync_into(&a, &b);
    assert!(!a.get_note(&note.id).unwrap().unwrap().trashed);
}

#[test]
fn trashing_on_two_devices_at_once_isnt_a_conflict() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Old", "").unwrap();
    sync_into(&b, &a);
    a.trash_note(&note.id).unwrap();
    b.trash_note(&note.id).unwrap();
    sync_both(&a, &b);
    assert!(note_conflicts(&a).is_empty());
    assert!(a.get_note(&note.id).unwrap().unwrap().trashed);
}

#[test]
fn an_edit_racing_permanent_deletion_leaves_the_note_deleted() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Gone", "v1").unwrap();
    a.trash_note(&note.id).unwrap();
    sync_into(&b, &a);
    a.delete_note(&note.id).unwrap();
    b.restore_note(&note.id).unwrap();
    b.update_note(&note.id, "Gone", "v2").unwrap();
    sync_both(&a, &b);
    assert!(a.get_note(&note.id).unwrap().is_none());
    assert!(b.get_note(&note.id).unwrap().is_none());
}

#[test]
fn edits_made_while_sync_is_off_stay_local_until_it_is_on() {
    let db = database();
    let note = db.create_note("Offline", "v1").unwrap();
    assert!(replica_fields(&db, &note.id).is_empty());
    db.update_note(&note.id, "Offline", "v2").unwrap();
    assert!(replica_fields(&db, &note.id).is_empty());

    db.set_beta_features_enabled(true).unwrap();
    db.reconcile_replicated_sync_backlog().unwrap();
    let recorded = replica_fields(&db, &note.id);
    assert_eq!(recorded.len(), 6);
    let peer = synced();
    sync_into(&peer, &db);
    assert_eq!(peer.get_note(&note.id).unwrap().unwrap().body, "v2");

    // Only the fields edited while off are recorded again.
    db.set_beta_features_enabled(false).unwrap();
    db.update_note(&note.id, "Offline renamed", "v2").unwrap();
    assert_eq!(replica_fields(&db, &note.id), recorded);
    db.set_beta_features_enabled(true).unwrap();
    db.reconcile_replicated_sync_backlog().unwrap();
    let after = replica_fields(&db, &note.id);
    for ((field, before), (_, now)) in recorded.iter().zip(&after) {
        assert_eq!(before < now, field == "title", "{field}");
    }
}

#[test]
fn an_edit_made_while_sync_was_off_conflicts_with_a_peer_edit_instead_of_replacing_it() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Shared", "v1").unwrap();
    sync_into(&b, &a);
    b.set_beta_features_enabled(false).unwrap();
    b.update_note(&note.id, "Shared", "from B offline").unwrap();
    a.update_note(&note.id, "Shared", "from A").unwrap();

    // Sync records the backlog before it pulls (`sync_once`), so B's edit
    // and A's are concurrent and both are kept.
    b.set_beta_features_enabled(true).unwrap();
    b.reconcile_replicated_sync_backlog().unwrap();
    sync_both(&a, &b);
    let shown = a.get_note(&note.id).unwrap().unwrap().body;
    assert_eq!(b.get_note(&note.id).unwrap().unwrap().body, shown);
    for db in [&a, &b] {
        assert_eq!(note_conflicts(db), [(note.id.clone(), "body".to_string())]);
    }
}

#[test]
fn projection_leaves_a_note_with_unrecorded_local_changes_alone() {
    let db = synced();
    let note = db.create_note("Mine", "v1").unwrap();
    db.set_beta_features_enabled(false).unwrap();
    db.update_note(&note.id, "Mine", "local").unwrap();
    let mut payload = sync_payload(&note);
    payload["body"] = serde_json::json!("remote");
    db.upsert_synced_note(&note.id, &payload).unwrap();
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, "local");
}

#[test]
fn deleting_a_note_the_replica_holds_is_recorded_even_while_sync_is_off() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Temp", "").unwrap();
    sync_into(&b, &a);
    a.set_beta_features_enabled(false).unwrap();
    a.trash_note(&note.id).unwrap();
    a.delete_note(&note.id).unwrap();
    a.set_beta_features_enabled(true).unwrap();
    a.reconcile_replicated_sync_backlog().unwrap();
    sync_into(&b, &a);
    assert!(b.get_note(&note.id).unwrap().is_none());
}

#[test]
fn a_body_is_refused_above_the_encoded_value_limit() {
    let db = synced();
    let note = db.create_note("Long", "").unwrap();
    let at_limit = "a".repeat(MAX_NOTE_BODY_BYTES - 2);
    assert_eq!(note_body_bytes(&at_limit), MAX_NOTE_BODY_BYTES);
    db.update_note(&note.id, "Long", &at_limit[1..]).unwrap();
    db.update_note(&note.id, "Long", &at_limit).unwrap();
    let error = db
        .update_note(&note.id, "Long", &format!("{at_limit}a"))
        .unwrap_err();
    assert_eq!(error.to_string(), TOO_LONG);
    // Newlines take two bytes once encoded, so a shorter body can be over.
    let newlines = "\n".repeat(MAX_NOTE_BODY_BYTES / 2);
    assert!(db.update_note(&note.id, "Long", &newlines).is_err());
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, at_limit);
    assert!(db.create_note("Longer", &format!("{at_limit}a")).is_err());
}

#[test]
fn the_notes_budget_refuses_growth_past_it_but_allows_trimming() {
    let db = synced();
    let filler = db.create_note("Filler", "").unwrap();
    let note = db.create_note("Note", "abc").unwrap();
    let own = |db: &Database| sql_count(db, "SELECT sync_bytes FROM notes WHERE id=?1", &note.id);
    let set_filler = |bytes: i64| {
        db.test_connection()
            .unwrap()
            .execute(
                "UPDATE notes SET sync_bytes=?1 WHERE id=?2",
                params![bytes, filler.id],
            )
            .unwrap();
    };
    let limit = MAX_SYNCED_NOTES_TOTAL_BYTES as i64;
    // Growing by one byte to exactly the limit is allowed.
    set_filler(limit - own(&db) - 1);
    db.update_note(&note.id, "Note", "abcd").unwrap();
    assert_eq!(db.notes_storage().unwrap().used_bytes, limit as u64);
    // One byte past it is refused.
    let error = db.update_note(&note.id, "Note", "abcde").unwrap_err();
    assert_eq!(error.to_string(), OUT_OF_SPACE);
    assert!(db.create_note("New", "").is_err());
    // Over the budget (another device's notes arrived), edits that don't
    // grow the note still save.
    set_filler(limit);
    db.update_note(&note.id, "Note", "wxyz").unwrap();
    db.update_note(&note.id, "Note", "w").unwrap();
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, "w");
    assert_eq!(
        db.notes_storage().unwrap().limit_bytes,
        MAX_SYNCED_NOTES_TOTAL_BYTES as u64
    );
}

#[test]
fn recent_notes_follow_visits_and_edits_and_search_skips_trash() {
    let db = database();
    let first = db.create_note("First", "alpha words").unwrap();
    let second = db.create_note("Second", "alpha more").unwrap();
    db.record_note_visit(&first.id).unwrap();
    let recent = db.list_recent_notes(10).unwrap();
    assert_eq!(recent[0].id, first.id);
    assert_eq!(recent[0].preview, "alpha words");
    assert!(recent[0].visited_at.is_some());
    db.update_note(&second.id, "Second", "alpha edited")
        .unwrap();
    assert_eq!(db.list_recent_notes(10).unwrap()[0].id, second.id);
    assert_eq!(db.list_recent_notes(1).unwrap().len(), 1);

    assert_eq!(db.search_notes("alpha", 10).unwrap().len(), 2);
    assert!(db.search_notes("   ", 10).unwrap().is_empty());
    let hit = &db.search_notes("words", 10).unwrap()[0];
    assert!(hit.match_snippet.contains("\u{1}words\u{2}"));
    db.trash_note(&first.id).unwrap();
    assert_eq!(db.search_notes("alpha", 10).unwrap().len(), 1);
    use crate::models::NoteSearchScope;
    let all = db
        .search_notes_in_scope("alpha", 10, NoteSearchScope::All)
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all.iter().filter(|hit| hit.trashed).count(), 1);
    let trash = db
        .search_notes_in_scope("alpha", 10, NoteSearchScope::Trash)
        .unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(trash[0].id, first.id);
    assert!(trash[0].trashed);
    assert!(db
        .search_notes_in_scope("edited", 10, NoteSearchScope::Trash)
        .unwrap()
        .is_empty());
    assert!(db
        .search_notes_in_scope(" ", 10, NoteSearchScope::All)
        .unwrap()
        .is_empty());
    assert_eq!(
        db.search_notes_in_scope("alpha", 1, NoteSearchScope::All)
            .unwrap()
            .len(),
        1
    );
    db.restore_note(&first.id).unwrap();
    assert!(db
        .search_notes_in_scope("alpha", 10, NoteSearchScope::Trash)
        .unwrap()
        .is_empty());
    assert_eq!(db.search_notes("alpha", 10).unwrap().len(), 2);
    db.record_note_visit("missing").unwrap();
    assert_eq!(
        sql_count(
            &db,
            "SELECT COUNT(*) FROM note_visits WHERE note_id=?1",
            "missing"
        ),
        0
    );
}

#[test]
fn a_long_body_preview_is_cut_with_an_ellipsis() {
    assert_eq!(preview("a\n\n b"), "a b");
    let long = "x".repeat(PREVIEW_CHARS + 1);
    let cut = preview(&long);
    assert_eq!(cut.chars().count(), PREVIEW_CHARS);
    assert!(cut.ends_with('…'));
    assert_eq!(
        preview(&"x".repeat(PREVIEW_CHARS)),
        "x".repeat(PREVIEW_CHARS)
    );
}

#[test]
fn a_synced_note_must_match_its_entity_and_pass_validation() {
    let db = synced();
    let payload = serde_json::json!({"id": "n-1", "title": "T", "body": "", "createdAt": "2026-11-09T00:00:00Z"});
    assert!(db.upsert_synced_note("n-2", &payload).is_err());
    assert!(db
        .check_projection_readiness(
            EntityType::Note,
            &serde_json::json!({"id": "n-1", "title": "T"})
        )
        .is_err());
    db.upsert_synced_note("n-1", &payload).unwrap();
    let stored = db.get_note("n-1").unwrap().unwrap();
    assert!(!stored.trashed);
    assert_eq!(stored.created_at, "2026-11-09T00:00:00Z");
    // Applying the same payload again changes nothing.
    db.upsert_synced_note("n-1", &payload).unwrap();
    assert_eq!(db.get_note("n-1").unwrap().unwrap(), stored);
    // Projection writes nothing back to the replica.
    assert!(replica_fields(&db, "n-1").is_empty());
}

#[test]
fn a_save_based_on_an_older_version_writes_nothing_and_returns_the_current_note() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Shared", "v1").unwrap();
    sync_into(&b, &a);
    let loaded = b.get_note(&note.id).unwrap().unwrap();

    let saved = b
        .save_note(&note.id, "Shared", "v2 from B", &loaded.updated_at)
        .unwrap();
    assert!(saved.saved);
    let current = saved.note.unwrap();
    assert_eq!(current.body, "v2 from B");

    // A's edit, made after it saw B's, arrives while B keeps typing from the
    // version it loaded.
    sync_into(&a, &b);
    a.update_note(&note.id, "Shared", "v3 from A").unwrap();
    sync_into(&b, &a);
    let stale = b
        .save_note(&note.id, "Shared", "v4 from B", &current.updated_at)
        .unwrap();
    assert!(!stale.saved);
    assert_eq!(stale.note.as_ref().unwrap().body, "v3 from A");
    assert_eq!(b.get_note(&note.id).unwrap().unwrap().body, "v3 from A");

    // Saving again from the version now shown is the user's choice to replace it.
    let kept = b
        .save_note(
            &note.id,
            "Shared",
            "v4 from B",
            &stale.note.unwrap().updated_at,
        )
        .unwrap();
    assert!(kept.saved);

    a.trash_note(&note.id).unwrap();
    a.delete_note(&note.id).unwrap();
    sync_into(&b, &a);
    let deleted = b
        .save_note(&note.id, "Shared", "v5", &kept.note.unwrap().updated_at)
        .unwrap();
    assert_eq!(
        deleted,
        NoteSave {
            saved: false,
            note: None,
            relink: None,
        }
    );
}

#[test]
fn a_daily_save_checks_whether_the_day_gained_a_note_meanwhile() {
    let (a, b) = (synced(), synced());
    let empty = b.save_daily_note_edit(DAY, "", None).unwrap();
    assert_eq!(
        empty,
        NoteSave {
            saved: true,
            note: None,
            relink: None,
        }
    );

    a.save_daily_note_edit(DAY, "From A", None).unwrap();
    sync_into(&b, &a);
    let stale = b.save_daily_note_edit(DAY, "From B", None).unwrap();
    assert!(!stale.saved);
    let current = stale.note.unwrap();
    assert_eq!(current.body, "From A");

    let saved = b
        .save_daily_note_edit(DAY, "From A, then B", Some(&current.updated_at))
        .unwrap();
    assert!(saved.saved);
    assert_eq!(saved.note.unwrap().body, "From A, then B");
    assert!(b.save_daily_note_edit("2026-02-30", "x", None).is_err());
}

fn rename(db: &Database, id: &str, title: &str) -> NoteSave {
    let note = db.get_note(id).unwrap().unwrap();
    db.save_note(id, title, &note.body, &note.updated_at)
        .unwrap()
}

#[test]
fn renaming_a_note_points_links_in_other_notes_at_the_new_title() {
    let db = synced();
    let plan = db.create_note("Project Plan", "the plan").unwrap();
    let meeting = db
        .create_note(
            "Meeting",
            "See [[project plan]] and [[Project Plan|it]].\n`[[Project Plan]]`",
        )
        .unwrap();
    db.save_daily_note(DAY, "Worked on [[Project  Plan]]")
        .unwrap();
    let trashed = db.create_note("Old", "Was [[Project Plan]]").unwrap();
    db.trash_note(&trashed.id).unwrap();

    let saved = rename(&db, &plan.id, "Roadmap");
    assert_eq!(
        saved.relink,
        Some(NoteRelink {
            updated: 2,
            kept_old_link: vec![]
        })
    );
    assert_eq!(
        db.get_note(&meeting.id).unwrap().unwrap().body,
        "See [[Roadmap]] and [[Roadmap|it]].\n`[[Project Plan]]`"
    );
    assert_eq!(
        db.get_daily_note(DAY).unwrap().unwrap().body,
        "Worked on [[Roadmap]]"
    );
    assert_eq!(
        db.get_note(&trashed.id).unwrap().unwrap().body,
        "Was [[Project Plan]]"
    );
    // Only the body changed in the linking notes, and the index follows.
    assert_eq!(db.note_links(&plan.id).unwrap().backlinks.len(), 3);
    assert!(replica_fields(&db, &meeting.id)
        .iter()
        .any(|(field, counter)| field == "body" && *counter > 1));

    // Changing only case or spacing keeps links working without rewriting them.
    assert_eq!(rename(&db, &plan.id, "roadmap").relink, None);
    assert_eq!(
        db.get_note(&meeting.id).unwrap().unwrap().body,
        "See [[Roadmap]] and [[Roadmap|it]].\n`[[Project Plan]]`"
    );
}

#[test]
fn a_note_keeps_the_old_link_when_the_rename_would_pass_its_size_limit() {
    let db = synced();
    let plan = db.create_note("P", "").unwrap();
    let link = "[[P]]";
    let filler = "a".repeat(MAX_NOTE_BODY_BYTES - 2 - link.len());
    let full = db.create_note("Full", &format!("{filler}{link}")).unwrap();
    let below = db
        .create_note("Below", &format!("{}{link}", &filler[1..]))
        .unwrap();

    let saved = rename(&db, &plan.id, "PQ");
    assert_eq!(
        saved.relink,
        Some(NoteRelink {
            updated: 1,
            kept_old_link: vec!["Full".to_string()]
        })
    );
    assert!(db
        .get_note(&full.id)
        .unwrap()
        .unwrap()
        .body
        .ends_with("[[P]]"));
    assert!(db
        .get_note(&below.id)
        .unwrap()
        .unwrap()
        .body
        .ends_with("[[PQ]]"));
    assert_eq!(db.get_note(&plan.id).unwrap().unwrap().title, "PQ");
}

#[test]
fn a_note_in_conflict_keeps_the_old_link_instead_of_settling_the_conflict() {
    let (a, b) = (synced(), synced());
    let plan = a.create_note("Plan", "").unwrap();
    let journal = a.create_note("Journal", "about [[Plan]]").unwrap();
    sync_into(&b, &a);
    a.update_note(&journal.id, "Journal", "A: about [[Plan]]")
        .unwrap();
    b.update_note(&journal.id, "Journal", "B: about [[Plan]]")
        .unwrap();
    sync_both(&a, &b);
    assert!(note_conflicts(&a).contains(&(journal.id.clone(), "body".to_string())));

    let saved = rename(&a, &plan.id, "Plan B");
    assert_eq!(
        saved.relink,
        Some(NoteRelink {
            updated: 0,
            kept_old_link: vec!["Journal".to_string()]
        })
    );
    assert!(note_conflicts(&a).contains(&(journal.id.clone(), "body".to_string())));
}

#[test]
fn titles_are_suggested_by_contained_text_and_links_resolve_by_key() {
    let db = database();
    let roadmap = db.create_note("Product Roadmap", "").unwrap();
    let road = db.create_note("Road trip", "").unwrap();
    let gone = db.create_note("Roadkill", "").unwrap();
    db.trash_note(&gone.id).unwrap();
    db.save_daily_note(DAY, "road").unwrap();

    let titles = |query: &str| -> Vec<String> {
        db.suggest_note_titles(query, 10)
            .unwrap()
            .into_iter()
            .map(|note| note.title)
            .collect()
    };
    assert_eq!(titles("road"), ["Road trip", "Product Roadmap"]);
    assert_eq!(titles("  ROAD   trip "), ["Road trip"]);
    assert!(titles("").contains(&"Product Roadmap".to_string()));
    assert_eq!(db.suggest_note_titles("", 1).unwrap().len(), 1);

    assert_eq!(
        db.find_note_by_title("product   ROADMAP")
            .unwrap()
            .map(|note| note.id),
        Some(roadmap.id)
    );
    assert_eq!(
        db.find_note_by_title("Road trip")
            .unwrap()
            .map(|note| note.id),
        Some(road.id)
    );
    assert_eq!(db.find_note_by_title("Roadkill").unwrap(), None);

    let links = db
        .note_links(
            &db.create_note("Hub", "[[Road trip]] [[road TRIP]] [[Nowhere]]")
                .unwrap()
                .id,
        )
        .unwrap();
    assert_eq!(links.links, ["Road trip", "Nowhere"]);
    assert_eq!(links.unlinked, ["Nowhere"]);
}

#[test]
fn export_takes_every_note_outside_trash_days_first() {
    let db = database();
    db.create_note("beta", "").unwrap();
    db.create_note("Alpha", "").unwrap();
    let gone = db.create_note("Gone", "").unwrap();
    db.trash_note(&gone.id).unwrap();
    db.save_daily_note("2026-11-10", "later").unwrap();
    db.save_daily_note(DAY, "earlier").unwrap();
    let titles: Vec<String> = db
        .notes_for_export()
        .unwrap()
        .into_iter()
        .map(|note| note.title)
        .collect();
    assert_eq!(titles, [DAY, "2026-11-10", "Alpha", "beta"]);
}

#[test]
fn independently_created_titles_converge_without_publishing_projection_names() {
    let (a, b, c) = (synced(), synced(), synced());
    let first = a.create_note("Project plan", "first").unwrap();
    let second = b.create_note("PROJECT PLAN", "second").unwrap();
    let natural = c.create_note("Project plan 2", "natural suffix").unwrap();
    let links = c.create_note("Links", "[[project plan]]").unwrap();
    // Start with opposite local incumbents and bring a natural suffix in later.
    sync_both(&a, &b);
    sync_both(&c, &b);
    sync_both(&a, &c);
    sync_both(&a, &b);
    let (winner, loser) = if first.id < second.id {
        (&first, &second)
    } else {
        (&second, &first)
    };
    for db in [&a, &b, &c] {
        assert_eq!(
            db.get_note(&winner.id).unwrap().unwrap().title,
            winner.title
        );
        assert_eq!(
            db.get_note(&loser.id).unwrap().unwrap().title,
            format!("{} 3", loser.title)
        );
        assert_eq!(
            db.get_note(&natural.id).unwrap().unwrap().title,
            natural.title
        );
        assert_eq!(
            db.find_note_by_title("project   PLAN").unwrap().unwrap().id,
            winner.id
        );
        assert_eq!(
            db.note_links(&winner.id).unwrap().backlinks[0].note_id,
            links.id
        );
        assert!(db.note_links(&loser.id).unwrap().backlinks.is_empty());
        let titles = db.suggest_note_titles("project", 10).unwrap();
        assert_eq!(titles.len(), 3);
        assert_eq!(
            titles
                .iter()
                .map(|n| title_key(&n.title))
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        assert!(note_conflicts(db).is_empty());
    }
    let before = replica_fields(&a, &loser.id);
    let projected = a.get_note(&loser.id).unwrap().unwrap();
    a.update_note(&loser.id, &projected.title, "edited body")
        .unwrap();
    let after = replica_fields(&a, &loser.id);
    assert_eq!(
        before.iter().find(|(f, _)| f == "title"),
        after.iter().find(|(f, _)| f == "title")
    );
    assert_eq!(
        a.note_sync_payloads()
            .unwrap()
            .into_iter()
            .find(|(id, _)| id == &loser.id)
            .unwrap()
            .1["title"],
        loser.title
    );
    sync_both(&a, &b);
    assert_eq!(b.get_note(&loser.id).unwrap().unwrap().body, "edited body");
    assert_eq!(
        b.get_note(&loser.id).unwrap().unwrap().title,
        projected.title
    );
    let stable = b.get_note(&loser.id).unwrap().unwrap();
    sync_into(&b, &a);
    assert_eq!(b.get_note(&loser.id).unwrap().unwrap(), stable);
    a.update_note(&loser.id, "Distinct title", "edited body")
        .unwrap();
    sync_both(&a, &b);
    assert_eq!(
        b.get_note(&loser.id).unwrap().unwrap().title,
        "Distinct title"
    );
}

#[test]
fn synced_restore_collisions_and_long_unicode_titles_stay_unique() {
    let (a, b) = (synced(), synced());
    let title = "界".repeat(MAX_NOTE_TITLE_CHARS);
    let old = a.create_note(&title, "old").unwrap();
    a.trash_note(&old.id).unwrap();
    sync_both(&a, &b);
    let replacement = b.create_note(&title, "new").unwrap();
    a.restore_note(&old.id).unwrap();
    sync_both(&a, &b);
    let (winner, loser) = if old.id < replacement.id {
        (&old, &replacement)
    } else {
        (&replacement, &old)
    };
    for db in [&a, &b] {
        let restored = db.get_note(&loser.id).unwrap().unwrap();
        assert_eq!(restored.title.chars().count(), MAX_NOTE_TITLE_CHARS);
        assert_eq!(
            restored.title,
            format!("{} 2", "界".repeat(MAX_NOTE_TITLE_CHARS - 2))
        );
        assert!(clean_note_title(&restored.title).is_ok());
        assert_eq!(
            db.find_note_by_title(&title).unwrap().unwrap().id,
            winner.id
        );
    }
    // Removing the winner frees the authored title on both peers.
    a.trash_note(&winner.id).unwrap();
    sync_both(&a, &b);
    assert_eq!(a.get_note(&loser.id).unwrap().unwrap().title, title);
    assert_eq!(b.get_note(&loser.id).unwrap().unwrap().title, title);
}

#[test]
fn offline_body_edits_of_disambiguated_notes_keep_the_authored_title() {
    let (a, b) = (synced(), synced());
    let first = a.create_note("Plan", "first").unwrap();
    let second = b.create_note("Plan", "second").unwrap();
    sync_both(&a, &b);
    let loser = if first.id > second.id {
        &first
    } else {
        &second
    };
    let projected = a.get_note(&loser.id).unwrap().unwrap();
    assert_eq!(projected.title, "Plan 2");
    a.set_beta_features_enabled(false).unwrap();
    a.update_note(&loser.id, &projected.title, "offline body")
        .unwrap();
    a.set_beta_features_enabled(true).unwrap();
    a.reconcile_replicated_sync_backlog().unwrap();
    sync_both(&a, &b);
    assert_eq!(b.get_note(&loser.id).unwrap().unwrap().title, "Plan 2");
    assert_eq!(b.get_note(&loser.id).unwrap().unwrap().body, "offline body");
    assert_eq!(
        a.note_sync_payloads()
            .unwrap()
            .into_iter()
            .find(|(id, _)| id == &loser.id)
            .unwrap()
            .1["title"],
        "Plan"
    );
}

#[test]
fn synced_titles_are_independent_of_arrival_order_and_replays() {
    let (a, b) = (synced(), synced());
    let payloads = [
        serde_json::json!({"id":"n-a", "title":"Plan", "body":"one"}),
        serde_json::json!({"id":"n-b", "title":"plan", "body":"two"}),
        serde_json::json!({"id":"n-c", "title":"Plan 2", "body":"three"}),
    ];
    for payload in &payloads {
        a.upsert_synced_note(payload["id"].as_str().unwrap(), payload)
            .unwrap();
    }
    for payload in payloads.iter().rev() {
        b.upsert_synced_note(payload["id"].as_str().unwrap(), payload)
            .unwrap();
    }
    for db in [&a, &b] {
        assert_eq!(db.get_note("n-a").unwrap().unwrap().title, "Plan");
        assert_eq!(db.get_note("n-b").unwrap().unwrap().title, "plan 3");
        assert_eq!(db.get_note("n-c").unwrap().unwrap().title, "Plan 2");
        let stable = db.get_note("n-b").unwrap().unwrap();
        db.upsert_synced_note("n-b", &payloads[1]).unwrap();
        assert_eq!(db.get_note("n-b").unwrap().unwrap(), stable);
        assert!(replica_fields(db, "n-b").is_empty());
        assert_eq!(db.search_notes("plan", 10).unwrap().len(), 3);
    }
}

#[test]
fn upgrading_existing_duplicate_titles_repairs_names_and_search_without_sync_writes() {
    let db = synced();
    let first = db.create_note("Plan", "first body").unwrap();
    let second = db.create_note("Different", "second body").unwrap();
    let before = replica_fields(&db, &second.id);
    {
        let mut connection = db.test_connection().unwrap();
        connection
            .execute(
                "UPDATE notes SET title='Plan', title_key='plan' WHERE id=?1",
                [&second.id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE note_search SET title='Plan' WHERE note_id=?1",
                [&second.id],
            )
            .unwrap();
        connection
            .execute_batch("ALTER TABLE notes DROP COLUMN authored_title; PRAGMA user_version=102;")
            .unwrap();
        crate::schema::migrate(&mut connection).unwrap();
    }
    let (winner, loser) = if first.id < second.id {
        (&first, &second)
    } else {
        (&second, &first)
    };
    assert_eq!(db.get_note(&winner.id).unwrap().unwrap().title, "Plan");
    assert_eq!(db.get_note(&loser.id).unwrap().unwrap().title, "Plan 2");
    assert_eq!(db.get_note(&first.id).unwrap().unwrap().body, "first body");
    assert_eq!(
        db.get_note(&second.id).unwrap().unwrap().body,
        "second body"
    );
    assert_eq!(db.search_notes("plan", 10).unwrap().len(), 2);
    assert_eq!(replica_fields(&db, &second.id), before);
}

#[test]
fn two_devices_importing_one_note_agree_except_on_text_that_differs() {
    let (a, b) = (synced(), synced());
    let imported = |created_at: &str, body: &str| Note {
        id: "import-0123456789abcdef0123456789abcdef".into(),
        title: "Retreat".into(),
        authored_title: None,
        body: body.into(),
        daily_date: None,
        created_at: created_at.into(),
        updated_at: created_at.into(),
        trashed: false,
        trashed_at: None,
    };
    // Each device read the file with its own modification time.
    assert!(a
        .import_note(imported("2026-09-01T10:00:00Z", "Plans"), &[])
        .unwrap());
    assert!(b
        .import_note(imported("2026-09-03T08:00:00Z", "Plans"), &[])
        .unwrap());
    sync_both(&a, &b);
    for db in [&a, &b] {
        let all = db.list_frontier_conflicts().unwrap();
        assert!(
            all.iter().all(|conflict| conflict.entity_type != "note"),
            "{all:?}"
        );
        assert!(!db
            .import_note(imported("2026-09-01T10:00:00Z", "Plans"), &[])
            .unwrap());
    }

    // Text that really differs is still a conflict to resolve.
    let (c, d) = (synced(), synced());
    c.import_note(imported("2026-09-01T10:00:00Z", "Plans"), &[])
        .unwrap();
    d.import_note(imported("2026-09-01T10:00:00Z", "Other plans"), &[])
        .unwrap();
    sync_both(&c, &d);
    let id = "import-0123456789abcdef0123456789abcdef".to_string();
    assert_eq!(note_conflicts(&c), [(id, "body".to_string())]);
}

#[test]
fn a_note_trashed_on_another_device_while_typing_comes_back_stale_not_as_an_error() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Shared", "v1").unwrap();
    a.save_daily_note(DAY, "today").unwrap();
    sync_both(&a, &b);
    let loaded = b.get_note(&note.id).unwrap().unwrap();
    let day = b.get_daily_note(DAY).unwrap().unwrap();

    // A trashes both while B is typing; trashing changes no text.
    a.trash_note(&note.id).unwrap();
    a.trash_note(&day.id).unwrap();
    sync_both(&a, &b);
    let trashed = b.get_note(&note.id).unwrap().unwrap();
    assert!(trashed.trashed);
    assert_eq!(trashed.updated_at, loaded.updated_at);

    let save = b
        .save_note(&note.id, "Shared", "typed on B", &loaded.updated_at)
        .unwrap();
    assert!(!save.saved);
    assert!(save.note.unwrap().trashed);
    let save = b
        .save_daily_note_edit(DAY, "typed on B", Some(&day.updated_at))
        .unwrap();
    assert!(!save.saved);
    assert!(save.note.unwrap().trashed);
    assert_eq!(b.get_note(&note.id).unwrap().unwrap().body, "v1");

    // Restoring it, then saving over the version it now is, keeps the typing.
    let restored = b.restore_note(&note.id).unwrap();
    let save = b
        .save_note(&note.id, "Shared", "typed on B", &restored.updated_at)
        .unwrap();
    assert!(save.saved);
    assert_eq!(save.note.unwrap().body, "typed on B");
}

#[test]
fn note_tasks_project_save_toggle_convert_and_restore_without_creating_task_entities() {
    let db = synced();
    let note = db
        .create_note("Checklist vs Tasks", "- [ ] Milk\n- ( ) Send proposal\n")
        .unwrap();
    let tasks = db.list_note_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Send proposal");
    let marker = &tasks[0].marker_id;
    let done = db
        .patch_note_item(&note.id, marker, &note.updated_at, Some(true), None)
        .unwrap();
    assert!(db.list_note_tasks().unwrap()[0].checked);
    assert!(done.body.contains("- [ ] Milk"));
    assert!(done.body.contains("- (x) Send proposal"));
    assert!(db
        .patch_note_item(&note.id, marker, &note.updated_at, Some(false), None)
        .is_err());
    let square = db
        .patch_note_item(&note.id, marker, &done.updated_at, None, Some(false))
        .unwrap();
    assert!(db.list_note_tasks().unwrap().is_empty());
    let round = db
        .patch_note_item(&note.id, marker, &square.updated_at, None, Some(true))
        .unwrap();
    assert_eq!(db.list_note_tasks().unwrap()[0].marker_id, *marker);
    db.trash_note(&note.id).unwrap();
    assert!(db.list_note_tasks().unwrap().is_empty());
    assert!(db
        .patch_note_item(&note.id, marker, &round.updated_at, Some(false), None)
        .is_err());
    db.restore_note(&note.id).unwrap();
    assert_eq!(db.list_note_tasks().unwrap().len(), 1);
    assert_eq!(
        sql_count(
            &db,
            "SELECT count(*) FROM sync_values WHERE entity_type='task' AND entity_id=?1",
            marker
        ),
        0
    );
    db.trash_note(&note.id).unwrap();
    db.delete_note(&note.id).unwrap();
    assert!(db.list_note_tasks().unwrap().is_empty());
}

#[test]
fn captured_conversation_references_are_atomic_idempotent_and_preserve_authored_text() {
    let db = database();
    let before = db.get_daily_note(DAY).unwrap();
    assert!(before.is_none());
    let captured = db.capture_note_conversation(DAY, "welcome", None).unwrap();
    assert!(captured.added);
    let doc = db.note_document(&captured.note.id).unwrap();
    assert_eq!(doc.links.len(), 1);
    assert!(doc.items.is_empty());
    let reference = &doc.links[0].reference;
    assert_eq!(
        db.resolve_note_conversation(reference).unwrap().as_deref(),
        Some("welcome")
    );
    assert_eq!(db.note_backlinks(reference).unwrap().len(), 1);
    let duplicate = db
        .capture_note_conversation(DAY, "welcome", Some(&captured.note.updated_at))
        .unwrap();
    assert!(!duplicate.added);
    assert_eq!(duplicate.note.updated_at, captured.note.updated_at);
    assert!(db.capture_note_conversation(DAY, "roadmap", None).is_err());
    let saved = db
        .save_daily_note_edit(DAY, "User text\n```\n", Some(&captured.note.updated_at))
        .unwrap()
        .note
        .unwrap();
    assert!(db
        .capture_note_conversation(DAY, "roadmap", Some(&saved.updated_at))
        .is_err());
    assert_eq!(db.get_note(&saved.id).unwrap().unwrap().body, saved.body);
    db.trash_note(&saved.id).unwrap();
    assert!(db.note_backlinks(reference).unwrap().is_empty());
    assert!(db
        .capture_note_conversation(DAY, "welcome", Some(&saved.updated_at))
        .is_err());
}

#[test]
fn note_task_projection_converges_and_conflicts_cannot_be_toggled() {
    let a = synced();
    let b = synced();
    let note = a.create_note("Follow up", "- ( ) Call\n").unwrap();
    sync_both(&a, &b);
    let marker = a.list_note_tasks().unwrap()[0].marker_id.clone();
    let remote = b.get_note(&note.id).unwrap().unwrap();
    b.patch_note_item(&note.id, &marker, &remote.updated_at, Some(true), None)
        .unwrap();
    a.update_note(
        &note.id,
        "Follow up",
        &(note.body.clone() + "\nLocal text\n"),
    )
    .unwrap();
    sync_both(&a, &b);
    assert_eq!(
        a.list_note_tasks().unwrap()[0].checked,
        b.list_note_tasks().unwrap()[0].checked
    );
    assert!(a.list_note_tasks().unwrap()[0].conflicted);
    let current = a.get_note(&note.id).unwrap().unwrap();
    assert!(a
        .patch_note_item(
            &note.id,
            &marker,
            &current.updated_at,
            Some(!a.list_note_tasks().unwrap()[0].checked),
            None
        )
        .is_err());
    assert_eq!(a.get_note(&note.id).unwrap().unwrap().body, current.body);
}

#[test]
fn event_note_links_keep_provider_identity_across_rescheduling_and_account_collisions() {
    let db = database();
    db.adopt_calendar_account("you@example.com").unwrap();
    let mut event:crate::models::ScheduleEvent=serde_json::from_value(serde_json::json!({
        "id":"cal:instance-original","seriesId":"cal:series","accountId":"you@example.com","calendarId":"cal",
        "title":"Weekly review","start":"2026-11-10T09:00:00Z","end":"2026-11-10T10:00:00Z",
        "allDay":false,"attendees":[],"attendeeResponses":{},"canRespond":false,"canEdit":true
    })).unwrap();
    db.cache_note_events("you@example.com", &[], "", "", std::slice::from_ref(&event))
        .unwrap();
    let choices = db.note_link_choices("event", "Weekly").unwrap();
    assert_eq!(choices.len(), 2);
    let instance = choices
        .iter()
        .find(|c| c.reference.id == event.id)
        .unwrap()
        .reference
        .clone();
    let series = choices
        .iter()
        .find(|c| c.reference.id == "cal:series")
        .unwrap()
        .reference
        .clone();
    let note = db
        .create_note(
            "Review notes",
            &format!("[Meeting]({})", instance.href().unwrap()),
        )
        .unwrap();
    event.start = "2026-11-11T14:00:00Z".into();
    event.end = "2026-11-11T15:00:00Z".into();
    db.cache_note_events("you@example.com", &[], "", "", std::slice::from_ref(&event))
        .unwrap();
    assert_eq!(
        db.resolve_note_event(&instance).unwrap().unwrap().start,
        event.start
    );
    assert!(db.resolve_note_event(&series).unwrap().is_some());
    assert!(db
        .resolve_note_event(&crate::note_entities::NoteReference {
            account: Some("other@example.com".into()),
            ..instance.clone()
        })
        .unwrap()
        .is_none());
    assert_eq!(db.note_backlinks(&instance).unwrap()[0].note_id, note.id);
    db.forget_note_event("you@example.com", "cal", &event.id)
        .unwrap();
    assert!(db.resolve_note_event(&instance).unwrap().is_none());
    assert_eq!(db.note_document(&note.id).unwrap().links.len(), 1);
    db.cache_note_events("you@example.com", &[], "", "", std::slice::from_ref(&event))
        .unwrap();
    db.remove_calendar_account("you@example.com").unwrap();
    assert!(db.resolve_note_event(&instance).unwrap().is_none());
    db.adopt_calendar_account("you@example.com").unwrap();
    assert!(db.resolve_note_event(&instance).unwrap().is_none());
    assert_eq!(db.note_document(&note.id).unwrap().links.len(), 1);
}

#[test]
fn conversation_note_links_follow_verified_aliases_but_never_match_subjects_or_other_accounts() {
    let db = database();
    let detail = db.get_thread("welcome").unwrap();
    let reference = crate::note_entities::NoteReference {
        version: 1,
        kind: "conversation".into(),
        id: "old-provider-id".into(),
        account: Some(detail.thread.account_id.clone()),
        calendar: None,
        note: None,
        contact: None,
        series: None,
    };
    let note = db
        .create_note(
            "Mail references",
            &format!("[Mail]({})", reference.href().unwrap()),
        )
        .unwrap();
    assert!(db.resolve_note_conversation(&reference).unwrap().is_none());
    db.test_connection()
        .unwrap()
        .execute(
            "INSERT INTO imap_thread_aliases(account_id,old_id,new_id) VALUES (?1,?2,?3)",
            rusqlite::params![
                detail.thread.account_id,
                reference.id,
                detail.thread.provider_thread_id
            ],
        )
        .unwrap();
    assert_eq!(
        db.resolve_note_conversation(&reference).unwrap().as_deref(),
        Some("welcome")
    );
    let current = crate::note_entities::NoteReference {
        id: detail.thread.provider_thread_id,
        ..reference.clone()
    };
    assert_eq!(db.note_backlinks(&current).unwrap()[0].note_id, note.id);
    assert!(db
        .resolve_note_conversation(&crate::note_entities::NoteReference {
            account: Some("other@example.com".into()),
            ..reference
        })
        .unwrap()
        .is_none());
}

#[test]
fn v103_upgrade_indexes_existing_round_tasks_without_authoring_note_edits() {
    let db = synced();
    let note = db.create_note("Earlier note", "").unwrap();
    let before = replica_fields(&db, &note.id);
    {
        let mut connection = db.test_connection().unwrap();
        connection
            .execute(
                "UPDATE notes SET body='- [ ] Checklist\n- ( ) Existing task\n' WHERE id=?1",
                [&note.id],
            )
            .unwrap();
        connection.execute_batch("DROP TABLE note_tasks; DROP TABLE note_entity_links; DROP TABLE note_index_warnings; DROP TABLE note_event_cache; PRAGMA user_version=103;").unwrap();
        crate::schema::migrate(&mut connection).unwrap();
    }
    assert_eq!(db.list_note_tasks().unwrap().len(), 1);
    let loaded = db.get_note(&note.id).unwrap().unwrap();
    assert_eq!(loaded.updated_at, note.updated_at);
    assert_eq!(loaded.body, "- [ ] Checklist\n- ( ) Existing task\n");
    assert_eq!(replica_fields(&db, &note.id), before);
    assert!(!db.note_document(&note.id).unwrap().items[1].persisted);
}

#[test]
fn adding_task_identity_past_the_body_budget_rolls_back_text_and_projection() {
    let db = database();
    let note = db.create_note("Budget", "Kept text").unwrap();
    let body = format!("- ( ) {}", "x".repeat(MAX_NOTE_BODY_BYTES - 10));
    assert!(note_body_bytes(&body) <= MAX_NOTE_BODY_BYTES);
    assert!(db
        .save_note(&note.id, &note.title, &body, &note.updated_at)
        .is_err());
    assert_eq!(db.get_note(&note.id).unwrap().unwrap(), note);
    assert!(db.list_note_tasks().unwrap().is_empty());
}

#[test]
fn received_over_limit_notes_remain_readable_with_warning_and_no_partial_task_index() {
    let db = database();
    let id = uuid::Uuid::new_v4().to_string();
    let payload = serde_json::json!({"id":id,"title":"From an older device","body":"- ( ) Task\n".repeat(threestrands_sync_protocol::MAX_NOTE_TASKS+1),"createdAt":"2026-11-10T00:00:00Z","trashed":false});
    db.upsert_synced_note(&id, &payload).unwrap();
    assert!(db.list_note_tasks().unwrap().is_empty());
    assert!(!db.get_note(&id).unwrap().unwrap().body.is_empty());
    assert_eq!(db.note_task_warnings().unwrap().len(), 1);
    assert!(db.note_document(&id).unwrap().warning.is_some());
    let mut trimmed = payload.clone();
    trimmed["body"] = serde_json::json!("- ( ) One task\n");
    db.upsert_synced_note(&id, &trimmed).unwrap();
    assert_eq!(db.list_note_tasks().unwrap().len(), 1);
    assert!(db.note_task_warnings().unwrap().is_empty());
}

#[test]
fn note_task_ids_and_entity_references_survive_folder_export_and_import() {
    let source = database();
    let note = source
        .create_note("Outbound", "- ( ) Follow through\n")
        .unwrap();
    let marker = source.list_note_tasks().unwrap()[0].marker_id.clone();
    let reference = crate::note_entities::NoteReference {
        version: 1,
        kind: "task".into(),
        id: marker.clone(),
        account: None,
        calendar: None,
        note: Some(note.id.clone()),
        contact: None,
        series: None,
    };
    let saved = source
        .update_note(
            &note.id,
            &note.title,
            &format!("{}\n[Task]({})\n", note.body, reference.href().unwrap()),
        )
        .unwrap();
    struct Folder(std::path::PathBuf);
    impl Folder {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let folder =
        Folder(std::env::temp_dir().join(format!("note-task-round-trip-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(folder.path()).unwrap();
    crate::notes_export::export(
        folder.path(),
        &source.notes_for_export().unwrap(),
        &[],
        |_| Ok(None),
        false,
    )
    .unwrap();
    let target = database();
    let plan = crate::notes_import::plan(
        folder.path(),
        &crate::notes_import::Existing {
            ids: HashSet::new(),
            titles: HashSet::new(),
        },
    )
    .unwrap();
    let result = crate::notes_import::run(plan, |note, files| {
        target.import_note(note, files).map_err(|e| e.to_string())
    });
    assert_eq!(result.notes, 1);
    let imported = target.get_note(&note.id).unwrap().unwrap();
    assert_eq!(imported.body, saved.body);
    assert_eq!(target.list_note_tasks().unwrap()[0].marker_id, marker);
    assert_eq!(
        target.note_backlinks(&reference).unwrap()[0].note_id,
        note.id
    );
}

fn saved_contact(db: &Database, name: &str, addresses: &[&str]) -> crate::models::ContactProfile {
    db.save_contact_profile(&crate::models::SaveContactRequest {
        birthday: None,
        keep_in_touch: None,
        id: None,
        display_name: Some(name.into()),
        role: None,
        company: None,
        location: None,
        bio: None,
        notes: None,
        links: vec![],
        photo_data: None,
        favorite: false,
        addresses: addresses.iter().map(|a| a.to_string()).collect(),
    })
    .unwrap()
}

#[test]
fn note_contact_choices_search_names_and_addresses_without_confusing_identical_names() {
    let db = database();
    let ada = saved_contact(&db, "Ada", &["ada@example.com", "ada@work.example"]);
    let other = saved_contact(&db, "Ada", &["other@example.com"]);
    let choices = db.note_link_choices("contact", "Ada").unwrap();
    assert_eq!(choices.len(), 2);
    let targets: HashSet<_> = choices
        .iter()
        .map(|choice| choice.reference.contact.clone().unwrap())
        .collect();
    assert_eq!(targets, HashSet::from([ada.id.clone(), other.id]));
    let choices = db.note_link_choices("contact", "ada@work.example").unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].label, "Ada");
    assert!(choices[0].detail.contains("ada@work.example"));
    assert!(choices[0].reference.validate());
    assert_eq!(
        db.resolve_note_contact(&choices[0].reference).unwrap(),
        Some(ada.id)
    );
    assert!(db
        .note_link_choices("contact", "Nobody here")
        .unwrap()
        .is_empty());
}

#[test]
fn note_conversation_choices_search_participants_and_keep_account_scoped_provider_ids() {
    let db = database();
    for account in ["personal@example.com", "work@example.com"] {
        db.adopt_account(account).unwrap();
        let mut message = crate::db::test_support::message(
            &format!("message-{account}"),
            "same-provider-thread",
            "2026-11-10T12:00:00Z",
            "review",
        );
        message.subject = "Project review".into();
        message.from = account.into();
        message.to = vec!["Jane <jane@example.com>".into()];
        db.upsert_thread(account, &[message]).unwrap();
    }
    let choices = db.note_link_choices("conversation", "Jane").unwrap();
    assert_eq!(choices.len(), 2);
    for choice in &choices {
        assert_eq!(choice.label, "Project review");
        assert_eq!(choice.reference.id, "same-provider-thread");
        assert_eq!(choice.sent_at.as_deref(), Some("2026-11-10T12:00:00Z"));
        assert!(choice.detail.contains("jane@example.com"));
        assert!(choice.reference.validate());
    }
    let first = db
        .resolve_note_conversation(&choices[0].reference)
        .unwrap()
        .unwrap();
    let second = db
        .resolve_note_conversation(&choices[1].reference)
        .unwrap()
        .unwrap();
    assert_ne!(first, second);
    let contacts = db.note_link_choices("contact", "jane@example.com").unwrap();
    assert_eq!(contacts.len(), 1);
    assert!(contacts[0].reference.contact.is_none());
    assert!(db
        .resolve_note_contact(&contacts[0].reference)
        .unwrap()
        .is_some());
}

#[test]
fn note_contact_choices_rank_recent_correspondence_before_alphabetical_names() {
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    saved_contact(&db, "Mention Z recent", &["recent@example.com"]);
    saved_contact(&db, "Mention A older", &["older@example.com"]);
    for (id, address, date) in [
        ("recent", "recent@example.com", "2026-11-10T12:00:00Z"),
        ("older", "older@example.com", "2026-11-01T12:00:00Z"),
    ] {
        let mut message = crate::db::test_support::message(id, id, date, "hello");
        message.from = "you@example.com".into();
        message.to = vec![address.into()];
        db.upsert_thread("you@example.com", &[message]).unwrap();
    }
    let choices = db.note_link_choices("contact", "Mention").unwrap();
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        vec!["Mention Z recent", "Mention A older"]
    );
}

fn contact_reference(address: &str, contact: Option<&str>) -> crate::note_entities::NoteReference {
    crate::note_entities::NoteReference {
        version: 1,
        kind: "contact".into(),
        id: address.into(),
        account: None,
        calendar: None,
        note: None,
        contact: contact.map(Into::into),
        series: None,
    }
}

#[test]
fn contact_references_resolve_through_merges_and_share_backlinks() {
    let db = database();
    let pete = saved_contact(&db, "Pete B", &["pete@example.com"]);
    let work = saved_contact(&db, "Pete (work)", &["pete@work.example"]);
    let with_id = contact_reference("pete@example.com", Some(&pete.id));
    let without_id = contact_reference("pete@example.com", None);
    let first = db
        .create_note(
            "Standup",
            &format!("- met with [Pete B]({})", with_id.href().unwrap()),
        )
        .unwrap();
    let second = db
        .create_note(
            "Planning",
            &format!(
                "[Pete]({}) and [again]({})",
                without_id.href().unwrap(),
                with_id.href().unwrap()
            ),
        )
        .unwrap();
    let third = db
        .create_note(
            "Vendor call",
            &format!(
                "[Pete]({})",
                contact_reference("pete@work.example", Some(&work.id))
                    .href()
                    .unwrap()
            ),
        )
        .unwrap();

    assert_eq!(
        db.resolve_note_contact(&with_id).unwrap(),
        Some(pete.id.clone())
    );
    assert_eq!(
        db.resolve_note_contact(&without_id).unwrap(),
        Some(pete.id.clone())
    );

    // One target for an address, whichever form the link took; each row keeps
    // the note's own first link so opening it finds that text.
    let rows = db.note_backlinks(&without_id).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row.note_id.as_str())
            .collect::<HashSet<_>>(),
        HashSet::from([first.id.as_str(), second.id.as_str()])
    );
    let planning = rows.iter().find(|row| row.note_id == second.id).unwrap();
    assert_eq!(planning.reference.as_ref(), Some(&without_id));
    assert_eq!(db.note_backlinks(&with_id).unwrap().len(), 2);

    // Merging Pete into the work contact deletes Pete's record; the link
    // follows the address to the contact that kept it.
    db.merge_contacts(&work.id, std::slice::from_ref(&pete.id))
        .unwrap();
    assert_eq!(
        db.resolve_note_contact(&with_id).unwrap(),
        Some(work.id.clone())
    );
    let merged = db.get_contact_profile(&work.id).unwrap().unwrap();
    let all: HashSet<String> = merged
        .addresses
        .iter()
        .flat_map(|address| {
            db.note_backlinks(&contact_reference(address, None))
                .unwrap()
        })
        .map(|row| row.note_id)
        .collect();
    assert_eq!(
        all,
        HashSet::from([first.id.clone(), second.id.clone(), third.id.clone()])
    );

    // Trash hides the backlink; an address nobody owns resolves to nothing.
    db.trash_note(&first.id).unwrap();
    assert_eq!(db.note_backlinks(&without_id).unwrap().len(), 1);
    assert_eq!(
        db.resolve_note_contact(&contact_reference("nobody@example.com", None))
            .unwrap(),
        None
    );
    assert!(db.resolve_note_contact(&reference_of_kind("task")).is_err());
}

fn reference_of_kind(kind: &str) -> crate::note_entities::NoteReference {
    crate::note_entities::NoteReference {
        version: 1,
        kind: kind.into(),
        id: "x".into(),
        account: None,
        calendar: None,
        note: None,
        contact: None,
        series: None,
    }
}

#[test]
fn upgrading_indexes_contact_references_received_before_they_were_understood() {
    let db = database();
    let reference = contact_reference("pete@example.com", Some("c1"));
    let note = db
        .create_note(
            "Standup",
            &format!("- met with [Pete B]({})", reference.href().unwrap()),
        )
        .unwrap();
    {
        // The note arrived on a version that indexed links by text and
        // didn't know contact references.
        let mut connection = db.test_connection().unwrap();
        connection
            .execute_batch(
                "DROP INDEX note_entity_link_targets;
                 ALTER TABLE note_entity_links DROP COLUMN target;
                 CREATE INDEX note_entity_link_targets ON note_entity_links(href);
                 DELETE FROM note_entity_links;
                 PRAGMA user_version=104;",
            )
            .unwrap();
        crate::schema::migrate(&mut connection).unwrap();
    }
    let rows = db
        .note_backlinks(&contact_reference("pete@example.com", None))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].note_id, note.id);
    assert_eq!(
        db.get_note(&note.id).unwrap().unwrap().updated_at,
        note.updated_at
    );
}

#[test]
fn a_contact_reference_to_someone_only_mailed_opens_their_derived_contact() {
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    let mut sent =
        crate::db::test_support::message("sent", "thread", "2026-09-01T12:00:00Z", "hello");
    sent.from = "you@example.com".into();
    sent.to = vec!["Jane <jane@example.com>".into()];
    db.upsert_thread("you@example.com", &[sent]).unwrap();
    let reference = contact_reference("jane@example.com", None);
    assert_eq!(
        db.resolve_note_contact(&reference).unwrap().as_deref(),
        Some("derived:jane@example.com")
    );
    // Saving Jane later makes the same link open the saved contact.
    let jane = saved_contact(&db, "Jane", &["jane@example.com"]);
    assert_eq!(db.resolve_note_contact(&reference).unwrap(), Some(jane.id));
}

fn meeting(event: &str, link_note: bool) -> crate::note_meetings::MeetingEntry {
    crate::note_meetings::MeetingEntry {
        event: crate::note_entities::NoteReference {
            version: 1,
            kind: "event".into(),
            id: event.into(),
            account: Some("you@example.com".into()),
            calendar: Some("work".into()),
            note: None,
            contact: None,
            series: None,
        },
        time: "9:30am".into(),
        attendees: vec![crate::note_meetings::MeetingAttendee {
            label: "Pete B".into(),
            reference: contact_reference("pete@example.com", None),
        }],
        name: "CS Leadership [Sync]".into(),
        link_note,
    }
}

fn notes_titled(db: &Database, title: &str) -> Vec<Note> {
    db.list_recent_notes(500)
        .unwrap()
        .into_iter()
        .chain(db.list_trashed_notes(500).unwrap())
        .filter(|note| note.title == title)
        .map(|note| db.get_note(&note.id).unwrap().unwrap())
        .collect()
}

#[test]
fn a_meeting_line_goes_into_its_days_meetings_section_once_per_occurrence() {
    let db = database();
    let first = db
        .add_meeting_to_daily_note(DAY, None, &meeting("evt-1", false))
        .unwrap();
    assert!(first.added);
    assert_eq!(first.previous_body, "");
    assert!(first.meeting_note.is_none());
    assert!(first
        .note
        .body
        .starts_with("## Meetings\n\n- [9:30am](threestrands:event/"));
    assert!(first.note.body.ends_with(" for CS Leadership \\[Sync\\]\n"));
    let event = meeting("evt-1", false).event;
    assert_eq!(db.note_backlinks(&event).unwrap()[0].note_id, first.note.id);
    assert_eq!(
        db.note_backlinks(&contact_reference("pete@example.com", None))
            .unwrap()
            .len(),
        1
    );

    // The same occurrence again adds nothing, even after the user typed
    // around it; another occurrence lands under the same heading.
    let edited = db
        .save_daily_note_edit(
            DAY,
            &format!("Morning\n\n{}\n## Later\n", first.note.body),
            Some(&first.note.updated_at),
        )
        .unwrap()
        .note
        .unwrap();
    let again = db
        .add_meeting_to_daily_note(DAY, Some(&edited.updated_at), &meeting("evt-1", true))
        .unwrap();
    assert!(!again.added);
    assert_eq!(again.note.updated_at, edited.updated_at);
    assert!(notes_titled(&db, "CS Leadership -Sync-").is_empty());
    let second = db
        .add_meeting_to_daily_note(DAY, Some(&edited.updated_at), &meeting("evt-2", false))
        .unwrap();
    let lines: Vec<&str> = second.note.body.lines().collect();
    assert_eq!(lines[0], "Morning");
    assert_eq!(lines[2], "## Meetings");
    assert!(lines[4].contains("evt%2D1"));
    assert!(lines[5].contains("evt%2D2"));
    assert_eq!(&lines[6..], ["", "## Later"]);
    assert_eq!(second.previous_body, edited.body);

    // A link the user wrote to an occurrence counts as already there.
    let typed = format!(
        "{}\nSee [standup]({})\n",
        second.note.body,
        meeting("evt-3", false).event.href().unwrap()
    );
    let typed = db
        .save_daily_note_edit(DAY, &typed, Some(&second.note.updated_at))
        .unwrap()
        .note
        .unwrap();
    assert!(
        !db.add_meeting_to_daily_note(DAY, Some(&typed.updated_at), &meeting("evt-3", false))
            .unwrap()
            .added
    );
}

#[test]
fn a_linked_meeting_note_is_created_once_and_shared_by_later_occurrences() {
    let db = database();
    let first = db
        .add_meeting_to_daily_note(DAY, None, &meeting("evt-1", true))
        .unwrap();
    assert!(first.created_meeting_note);
    let created = first.meeting_note.unwrap();
    assert_eq!(created.title, "CS Leadership -Sync-");
    assert_eq!(created.body, "");
    assert!(first.note.body.contains(" for [[CS Leadership -Sync-]]\n"));
    assert_eq!(db.note_links(&created.id).unwrap().backlinks.len(), 1);

    let next_day = db
        .add_meeting_to_daily_note("2026-11-16", None, &meeting("evt-1b", true))
        .unwrap();
    assert!(!next_day.created_meeting_note);
    assert_eq!(next_day.meeting_note.unwrap().id, created.id);
    assert_eq!(notes_titled(&db, "CS Leadership -Sync-").len(), 1);
    assert_eq!(db.note_links(&created.id).unwrap().backlinks.len(), 2);

    // A same-titled note in Trash doesn't hold the title.
    db.trash_note(&created.id).unwrap();
    let after_trash = db
        .add_meeting_to_daily_note("2026-11-23", None, &meeting("evt-1c", true))
        .unwrap();
    assert!(after_trash.created_meeting_note);
    assert_ne!(after_trash.meeting_note.unwrap().id, created.id);
}

#[test]
fn a_refused_meeting_writes_neither_note() {
    let db = database();
    let note = db
        .save_daily_note_edit(DAY, "Notes\n```\ncode\n", None)
        .unwrap()
        .note
        .unwrap();
    // A stale version, an unclosed code block, Trash, and the size limit.
    assert!(db
        .add_meeting_to_daily_note(DAY, None, &meeting("evt-1", true))
        .is_err());
    let fence = db
        .add_meeting_to_daily_note(DAY, Some(&note.updated_at), &meeting("evt-1", true))
        .unwrap_err()
        .to_string();
    assert!(fence.contains("code block"), "{fence}");
    let long = format!("{}\n", "x".repeat(MAX_NOTE_BODY_BYTES - 200));
    let near = db
        .save_daily_note_edit(DAY, &long, Some(&note.updated_at))
        .unwrap()
        .note
        .unwrap();
    let mut wide = meeting("evt-1", true);
    wide.attendees = (0..40)
        .map(|i| crate::note_meetings::MeetingAttendee {
            label: format!("Person {i}"),
            reference: contact_reference(&format!("p{i}@example.com"), None),
        })
        .collect();
    assert!(db
        .add_meeting_to_daily_note(DAY, Some(&near.updated_at), &wide)
        .is_err());
    db.trash_note(&near.id).unwrap();
    assert!(db
        .add_meeting_to_daily_note(DAY, Some(&near.updated_at), &meeting("evt-1", true))
        .is_err());
    assert!(notes_titled(&db, "CS Leadership -Sync-").is_empty());
    assert_eq!(db.get_note(&near.id).unwrap().unwrap().body, near.body);
    assert!(db
        .note_backlinks(&meeting("evt-1", false).event)
        .unwrap()
        .is_empty());
}

#[test]
fn a_meeting_is_refused_while_its_days_note_is_in_conflict() {
    let a = synced();
    let b = synced();
    a.save_daily_note(DAY, "From A").unwrap();
    b.save_daily_note(DAY, "From B").unwrap();
    sync_both(&a, &b);
    let current = a.get_daily_note(DAY).unwrap().unwrap();
    let error = a
        .add_meeting_to_daily_note(DAY, Some(&current.updated_at), &meeting("evt-1", true))
        .unwrap_err()
        .to_string();
    assert!(error.contains("conflict"), "{error}");
    assert_eq!(a.get_daily_note(DAY).unwrap().unwrap().body, current.body);
    assert!(notes_titled(&a, "CS Leadership -Sync-").is_empty());
}

fn standup(id: &str, series: Option<&str>) -> crate::note_entities::NoteReference {
    crate::note_entities::NoteReference {
        version: 1,
        kind: "event".into(),
        id: id.into(),
        account: Some("you@example.com".into()),
        calendar: Some("work".into()),
        note: None,
        contact: None,
        series: series.map(Into::into),
    }
}

fn schedule_event(id: &str, series: Option<&str>) -> crate::models::ScheduleEvent {
    serde_json::from_value(serde_json::json!({
        "id": id, "seriesId": series, "accountId": "you@example.com", "calendarId": "work",
        "title": "Standup", "start": "2026-11-12T15:00:00Z", "end": "2026-11-12T15:15:00Z", "allDay": false,
        "attendees": [], "attendeeResponses": {}, "canRespond": false, "canEdit": true,
    }))
    .unwrap()
}

#[test]
fn a_recurring_series_lists_the_notes_about_every_occurrence() {
    let db = database();
    db.adopt_calendar_account("you@example.com").unwrap();
    let link = |label: &str, reference: &crate::note_entities::NoteReference| {
        format!("[{label}]({})", reference.href().unwrap())
    };
    let tuesday = db
        .create_note(
            "Tuesday standup",
            &link("Standup", &standup("standup_1110", Some("standup"))),
        )
        .unwrap();
    let series = db
        .create_note("Standup notes", &link("Standup", &standup("standup", None)))
        .unwrap();
    // Written before this device knew the occurrence's series.
    let thursday = db
        .create_note(
            "Thursday standup",
            &link("Standup", &standup("standup_1112", None)),
        )
        .unwrap();
    let other = db
        .create_note(
            "Retro",
            &link("Retro", &standup("retro_1112", Some("retro"))),
        )
        .unwrap();
    let ids = |rows: Vec<crate::models::NoteBacklink>| {
        rows.into_iter()
            .map(|row| row.note_id)
            .collect::<HashSet<_>>()
    };

    // An occurrence lists only its own notes, whichever form its link took.
    assert_eq!(
        ids(db.note_backlinks(&standup("standup_1110", None)).unwrap()),
        HashSet::from([tuesday.id.clone()])
    );
    assert_eq!(
        ids(db
            .note_backlinks(&standup("standup_1110", Some("standup")))
            .unwrap()),
        HashSet::from([tuesday.id.clone()])
    );
    let whole = standup("standup", None);
    assert_eq!(
        ids(db.note_backlinks(&whole).unwrap()),
        HashSet::from([tuesday.id.clone(), series.id.clone()])
    );

    // Loading Thursday in Calendar tells this device its series, and the
    // older link counts from then on; so does a note written afterwards.
    db.cache_note_events(
        "you@example.com",
        &["work".into()],
        "2026-11-12T00:00:00Z",
        "2026-11-13T00:00:00Z",
        &[schedule_event("standup_1112", Some("standup"))],
    )
    .unwrap();
    assert_eq!(
        ids(db.note_backlinks(&whole).unwrap()),
        HashSet::from([tuesday.id.clone(), series.id.clone(), thursday.id.clone()])
    );
    let later = db
        .create_note(
            "Thursday follow-up",
            &link("Standup", &standup("standup_1112", None)),
        )
        .unwrap();
    assert!(ids(db.note_backlinks(&whole).unwrap()).contains(&later.id));
    assert!(!ids(db.note_backlinks(&whole).unwrap()).contains(&other.id));

    // Link to… offers the occurrence with its series, and the series alone.
    let choices = db.note_link_choices("event", "standup").unwrap();
    assert!(choices
        .iter()
        .any(|choice| choice.reference == standup("standup_1112", Some("standup"))));
    assert!(choices.iter().any(|choice| choice.reference == whole));
}
