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
