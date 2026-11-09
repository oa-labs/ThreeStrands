use super::*;
use crate::db::test_support::database;
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
