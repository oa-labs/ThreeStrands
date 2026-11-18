use super::*;
use crate::db::test_support::database;

const DAY: &str = "2026-11-16";

fn task_ref(id: &str) -> NoteReference {
    NoteReference {
        version: 1,
        kind: "task".into(),
        id: id.into(),
        account: None,
        calendar: None,
        note: None,
        contact: None,
        series: None,
    }
}

fn standalone(db: &Database, title: &str) -> crate::models::ThreadTask {
    db.adopt_account("me@example.com").unwrap();
    db.create_task(
        &serde_json::from_value(
            serde_json::json!({"accountId": "me@example.com", "title": title, "kind": "action"}),
        )
        .unwrap(),
    )
    .unwrap()
}

fn add(reference: NoteReference, label: &str, carried: Option<&str>) -> PlanEntry {
    PlanEntry::Add {
        addition: PlanAddition::Link {
            reference,
            label: label.into(),
        },
        carried: carried.map(String::from),
    }
}

fn existing(reference: &NoteReference) -> PlanEntry {
    PlanEntry::Existing {
        key: reference.target().unwrap(),
    }
}

#[test]
fn a_save_writes_carried_items_new_tasks_and_not_today_decisions_in_one_go() {
    let db = database();
    let deck = standalone(&db, "Send the deck");
    let board = standalone(&db, "Update the board deck");
    let survey = standalone(&db, "Reply to the survey");
    db.save_daily_note(DAY, "# Monday\n\n## Meetings\n\n- 9am Sync\n")
        .unwrap();
    let note = db.get_daily_note(DAY).unwrap().unwrap();
    let changes = PlanChanges {
        items: vec![
            add(task_ref(&deck.id), "Send the deck", Some("2026-11-13")),
            PlanEntry::Add {
                addition: PlanAddition::NewTask {
                    title: "Draft memo".into(),
                },
                carried: None,
            },
        ],
        remove: vec![],
        not_today: vec![
            NotTodayEntry {
                reference: task_ref(&board.id),
                label: "Update the board deck".into(),
                decision: PlanMarker::Deferred("2026-11-20".into()),
            },
            NotTodayEntry {
                reference: task_ref(&survey.id),
                label: "Reply to the survey".into(),
                decision: PlanMarker::Dropped,
            },
        ],
    };
    let saved = db
        .save_daily_plan(DAY, Some(&note.updated_at), "op-1", &changes)
        .unwrap();
    let body = &saved.note.body;
    assert!(body.starts_with("# Monday\n\n## Plan\n\n- [Send the deck]("));
    assert!(body.contains(") <!-- threestrands-plan:carried 2026-11-13 -->\n- ( ) Draft memo"));
    assert!(body.contains("### Not today\n\n- [Update the board deck]("));
    assert!(
        body.contains("<!-- threestrands-plan:deferred 2026-11-20 -->\n- [Reply to the survey](")
    );
    assert!(body.contains("<!-- threestrands-plan:dropped -->"));
    assert!(body.ends_with("\n\n## Meetings\n\n- 9am Sync\n"));
    assert_eq!(saved.added.len(), 2, "the deck and the new task's marker");
    assert_eq!(saved.not_today.len(), 2);
    assert!(saved.changed);

    let plan = db.daily_plan(DAY).unwrap();
    assert_eq!(plan.items.len(), 2);
    assert_eq!(
        plan.items[0].marker,
        Some(PlanMarker::Carried("2026-11-13".into()))
    );
    assert_eq!(plan.not_today.len(), 2);
}

#[test]
fn a_lost_response_is_retried_with_the_same_operation_and_returns_the_first_result() {
    let db = database();
    let deck = standalone(&db, "Send the deck");
    let changes = PlanChanges {
        items: vec![add(task_ref(&deck.id), "Send the deck", None)],
        ..Default::default()
    };
    let first = db.save_daily_plan(DAY, None, "op-retry", &changes).unwrap();
    // The retry still names the version the routine read, which the first save moved on.
    let again = db.save_daily_plan(DAY, None, "op-retry", &changes).unwrap();
    assert_eq!(again.note.updated_at, first.note.updated_at);
    assert_eq!(again.added, first.added);
    assert_eq!(
        db.get_daily_note(DAY)
            .unwrap()
            .unwrap()
            .body
            .matches("Send the deck")
            .count(),
        1,
        "no duplicate line"
    );
    // The same id can't be spent on another day.
    assert!(db
        .save_daily_plan("2026-11-17", None, "op-retry", &changes)
        .is_err());
}

#[test]
fn edit_plan_reorders_and_removes_items_without_touching_other_text() {
    let db = database();
    let deck = standalone(&db, "Send the deck");
    let memo = standalone(&db, "Write the memo");
    let body = format!(
        "## Plan\n\n- [Send]({})\n  - with appendix\n\nWritten by hand\n\n- [Memo]({})\n- Think about Q4\n",
        task_ref(&deck.id).href().unwrap(),
        task_ref(&memo.id).href().unwrap()
    );
    let note = db.save_daily_note(DAY, &body).unwrap().unwrap();
    let changes = PlanChanges {
        items: vec![existing(&task_ref(&memo.id)), existing(&task_ref(&deck.id))],
        remove: vec!["line:".to_string() + &body.find("- Think").unwrap().to_string()],
        not_today: vec![],
    };
    let saved = db
        .save_daily_plan(DAY, Some(&note.updated_at), "op-order", &changes)
        .unwrap();
    assert_eq!(
        saved.note.body,
        format!(
            "## Plan\n\n- [Memo]({})\n\nWritten by hand\n\n- [Send]({})\n  - with appendix\n",
            task_ref(&memo.id).href().unwrap(),
            task_ref(&deck.id).href().unwrap()
        )
    );
    assert_eq!(saved.removed.len(), 1);
    // Taken off, not dropped: nothing goes under Not today.
    assert!(!saved.note.body.contains("Not today"));
}

#[test]
fn a_link_already_in_the_plan_is_left_as_it_is() {
    let db = database();
    let deck = standalone(&db, "Send the deck");
    let first = db
        .save_daily_plan(
            DAY,
            None,
            "op-a",
            &PlanChanges {
                items: vec![add(task_ref(&deck.id), "Send the deck", None)],
                ..Default::default()
            },
        )
        .unwrap();
    let again = db
        .save_daily_plan(
            DAY,
            Some(&first.note.updated_at),
            "op-b",
            &PlanChanges {
                items: vec![add(task_ref(&deck.id), "Send the deck", Some("2026-11-15"))],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(again.already, [task_ref(&deck.id).target().unwrap()]);
    assert!(!again.changed);
    assert_eq!(again.note.body, first.note.body);
}

#[test]
fn saves_refuse_stale_versions_conflicts_trash_unclosed_code_bad_dates_and_size() {
    let db = database();
    let deck = standalone(&db, "Send the deck");
    let one = PlanChanges {
        items: vec![add(task_ref(&deck.id), "Send the deck", None)],
        ..Default::default()
    };
    let note = db.save_daily_note(DAY, "Journal\n").unwrap().unwrap();
    // Stale.
    assert!(db
        .save_daily_plan(DAY, Some("stale"), "op-1", &one)
        .is_err());
    assert!(db.save_daily_plan(DAY, None, "op-2", &one).is_err());
    // A refusal leaves no receipt, so the same id can be used once it's fixed.
    let fixed = db
        .save_daily_plan(DAY, Some(&note.updated_at), "op-1", &one)
        .unwrap();
    assert!(fixed.changed);

    // Unknown items, carried from the same day, deferred to the past, carried under Not today.
    let fresh = db.get_daily_note(DAY).unwrap().unwrap().updated_at;
    let refusals = [
        PlanChanges {
            items: vec![add(task_ref("missing"), "x", None)],
            ..Default::default()
        },
        PlanChanges {
            items: vec![PlanEntry::Existing {
                key: "task:nowhere".into(),
            }],
            ..Default::default()
        },
        PlanChanges {
            remove: vec!["task:nowhere".into()],
            ..Default::default()
        },
        PlanChanges {
            items: vec![add(task_ref(&deck.id), "x", Some(DAY))],
            remove: vec![task_ref(&deck.id).target().unwrap()],
            ..Default::default()
        },
        PlanChanges {
            not_today: vec![NotTodayEntry {
                reference: task_ref(&deck.id),
                label: "x".into(),
                decision: PlanMarker::Deferred(DAY.into()),
            }],
            ..Default::default()
        },
        PlanChanges {
            not_today: vec![NotTodayEntry {
                reference: task_ref(&deck.id),
                label: "x".into(),
                decision: PlanMarker::Carried("2026-11-10".into()),
            }],
            ..Default::default()
        },
    ];
    for (at, changes) in refusals.iter().enumerate() {
        assert!(
            db.save_daily_plan(DAY, Some(&fresh), &format!("refused-{at}"), changes)
                .is_err(),
            "case {at}"
        );
    }
    assert_eq!(db.get_daily_note(DAY).unwrap().unwrap().updated_at, fresh);

    // Too many lines at once.
    let many = PlanChanges {
        items: (0..=MAX_PLAN_ADDITIONS)
            .map(|at| PlanEntry::Add {
                addition: PlanAddition::NewTask {
                    title: format!("Task {at}"),
                },
                carried: None,
            })
            .collect(),
        ..Default::default()
    };
    assert!(db
        .save_daily_plan(DAY, Some(&fresh), "op-many", &many)
        .is_err());

    // An unclosed code block.
    let fenced = db
        .save_daily_note("2026-11-18", "```\ncode")
        .unwrap()
        .unwrap();
    assert!(db
        .save_daily_plan("2026-11-18", Some(&fenced.updated_at), "op-fence", &one)
        .is_err());

    // Trash.
    let trashed = db
        .save_daily_note("2026-11-19", "Journal\n")
        .unwrap()
        .unwrap();
    db.trash_note(&trashed.id).unwrap();
    assert!(db
        .save_daily_plan("2026-11-19", Some(&trashed.updated_at), "op-trash", &one)
        .is_err());

    // A sync conflict on the note.
    let conflicted = db
        .save_daily_note("2026-11-20", "Journal\n")
        .unwrap()
        .unwrap();
    db.connection()
        .unwrap()
        .execute_batch(&format!(
            "INSERT INTO sync_values(entity_type,entity_id,field,device_id,counter,lamport,value) VALUES
             ('note','{0}','body','d1',1,1,'\"a\"'),('note','{0}','body','d2',1,1,'\"b\"')",
            conflicted.id
        ))
        .unwrap();
    assert!(db
        .save_daily_plan(
            "2026-11-20",
            Some(&conflicted.updated_at),
            "op-conflict",
            &one
        )
        .is_err());

    // Body size: a save that would pass the note size limit writes nothing.
    let big = "x".repeat(threestrands_sync_protocol::MAX_NOTE_BODY_BYTES - 40);
    let large = db.save_daily_note("2026-11-21", &big).unwrap().unwrap();
    assert!(db
        .save_daily_plan("2026-11-21", Some(&large.updated_at), "op-size", &one)
        .is_err());
    assert_eq!(db.get_daily_note("2026-11-21").unwrap().unwrap().body, big);
}

#[test]
fn undo_restores_the_body_only_while_the_note_is_as_the_save_left_it() {
    let db = database();
    let deck = standalone(&db, "Send the deck");
    let note = db.save_daily_note(DAY, "Journal\n").unwrap().unwrap();
    let saved = db
        .save_daily_plan(
            DAY,
            Some(&note.updated_at),
            "op-undo",
            &PlanChanges {
                items: vec![add(task_ref(&deck.id), "Send the deck", None)],
                ..Default::default()
            },
        )
        .unwrap();
    let undone = db.undo_daily_plan_save(DAY, "op-undo").unwrap();
    assert_eq!(undone.body, "Journal\n");
    // Undoing twice changes nothing more.
    assert_eq!(
        db.undo_daily_plan_save(DAY, "op-undo").unwrap().body,
        "Journal\n"
    );

    // After later writing, Undo refuses rather than overwrite it.
    let later = db
        .save_daily_plan(
            DAY,
            Some(&undone.updated_at),
            "op-later",
            &PlanChanges {
                items: vec![add(task_ref(&deck.id), "Send the deck", None)],
                ..Default::default()
            },
        )
        .unwrap();
    let edited = format!("{}More writing\n", later.note.body);
    db.save_note(
        &later.note.id,
        &later.note.title,
        &edited,
        &later.note.updated_at,
    )
    .unwrap();
    assert!(db.undo_daily_plan_save(DAY, "op-later").is_err());
    assert_eq!(db.get_daily_note(DAY).unwrap().unwrap().body, edited);
    assert!(db.undo_daily_plan_save(DAY, "unknown").is_err());
    assert!(saved.changed);
}

#[test]
fn notes_on_the_day_from_earlier_versions_stay_put_and_their_receipts_still_load() {
    // Earlier versions could append a paragraph under `### Shutdown`; a save
    // now leaves it as it is, and the plan never counts it as an item.
    let db = database();
    let body = "## Plan\n\n- a\n\n### Shutdown\n\nWent well.\n\n## Meetings\n\n- 9am Sync\n";
    let note = db.save_daily_note(DAY, body).unwrap().unwrap();
    assert_eq!(db.daily_plan(DAY).unwrap().items.len(), 1);
    let saved = db
        .save_daily_plan(
            DAY,
            Some(&note.updated_at),
            "op-earlier",
            &PlanChanges {
                remove: vec![db.daily_plan(DAY).unwrap().items[0].key.clone()],
                ..Default::default()
            },
        )
        .unwrap();
    assert!(saved.note.body.contains("### Shutdown\n\nWent well."));

    // A receipt saved with the old flag still reads back.
    let receipt = serde_json::json!({
        "operationId": "op-old", "note": saved.note, "added": [], "removed": [],
        "notToday": [], "already": [], "changed": true, "shutdown": true,
    });
    assert!(serde_json::from_value::<crate::db::plan_save::PlanSaveResult>(receipt).is_ok());

    // A request can no longer ask for one.
    let request =
        serde_json::json!({ "items": [], "remove": [], "notToday": [], "shutdown": "Fine." });
    assert!(serde_json::from_value::<PlanChanges>(request).is_err());
}
