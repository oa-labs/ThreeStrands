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

fn contact_ref(address: &str) -> NoteReference {
    NoteReference {
        kind: "contact".into(),
        ..task_ref(address)
    }
}

fn link(reference: NoteReference, label: &str) -> PlanAddition {
    PlanAddition::Link {
        reference,
        label: label.into(),
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

fn contact(db: &Database, address: &str) -> crate::models::ContactProfile {
    db.save_contact_profile(&crate::models::SaveContactRequest {
        birthday: None,
        keep_in_touch: None,
        id: None,
        display_name: Some("Ann".into()),
        role: None,
        company: None,
        phone: None,
        location: None,
        bio: None,
        notes: None,
        links: vec![],
        photo_data: None,
        favorite: false,
        addresses: vec![address.into()],
        time_zone: None,
    })
    .unwrap()
}

#[test]
fn a_plan_is_read_from_the_day_note_with_each_items_current_state() {
    let db = database();
    let task = standalone(&db, "Send the deck");
    let ann = contact(&db, "ann@example.org");
    db.mark_contacted(&ann.id).unwrap();
    let other = db
        .create_note("Projects", "- ( ) Write the memo\n")
        .unwrap();
    let marker = db.list_note_tasks().unwrap()[0].marker_id.clone();
    let other = db.get_note(&other.id).unwrap().unwrap();
    assert!(other.body.contains(&marker), "saving persisted the marker");
    let note_task = NoteReference {
        note: Some(other.id.clone()),
        ..task_ref(&marker)
    };
    db.save_daily_note(
        DAY,
        &format!(
            "## Plan\n\n- [Old title]({}) <!-- threestrands-plan:carried 2026-11-15 -->\n- [Memo]({})\n- ( ) Plan-only task\n- [Ann]({})\n- [Gone]({})\n- A thought\n",
            task_ref(&task.id).href().unwrap(),
            note_task.href().unwrap(),
            contact_ref("ann@example.org").href().unwrap(),
            task_ref("missing").href().unwrap(),
        ),
    )
    .unwrap();

    let plan = db.daily_plan(DAY).unwrap();
    assert!(plan.revision.is_some());
    let summary: Vec<_> = plan
        .items
        .iter()
        .map(|item| (item.kind, item.title.as_str(), item.state))
        .collect();
    assert_eq!(
        summary,
        [
            (PlanItemKind::Task, "Send the deck", PlanItemState::Open),
            (
                PlanItemKind::NoteTask,
                "Write the memo",
                PlanItemState::Open
            ),
            (PlanItemKind::NewTask, "Plan-only task", PlanItemState::Open),
            // Marked contacted today, on or after the plan's day.
            (PlanItemKind::Contact, "Ann", PlanItemState::Done),
            (PlanItemKind::Task, "Gone", PlanItemState::Unavailable),
            (PlanItemKind::Text, "A thought", PlanItemState::Text),
        ]
    );
    assert_eq!(
        plan.items[0].marker,
        Some(PlanMarker::Carried("2026-11-15".into()))
    );
    assert_eq!(plan.items[0].task_id.as_deref(), Some(task.id.as_str()));
    assert_eq!(
        plan.items[1].task_id,
        Some(format!("note:{}:{marker}", other.id))
    );
    assert!(plan.items[2]
        .task_id
        .as_deref()
        .unwrap()
        .starts_with("note:daily:2026-11-16:"));
    assert_eq!(plan.items[3].contact_id.as_deref(), Some(ann.id.as_str()));

    // A contact last touched before the plan's day isn't done.
    db.connection()
        .unwrap()
        .execute(
            "UPDATE contacts SET kit_last_touch_at='2026-11-01T09:00:00Z' WHERE id=?1",
            [&ann.id],
        )
        .unwrap();
    assert_eq!(
        db.daily_plan(DAY).unwrap().items[3].state,
        PlanItemState::Open
    );
    // A day without a note has an empty plan.
    let empty = db.daily_plan("2026-11-17").unwrap();
    assert!(empty.revision.is_none() && empty.items.is_empty());
}

#[test]
fn adding_to_a_plan_writes_the_section_once_and_skips_links_already_there() {
    let db = database();
    let task = standalone(&db, "Send the deck");
    db.save_daily_note(DAY, "# Monday\n\n## Meetings\n\n- 9am Sync\n")
        .unwrap();
    let note = db.get_daily_note(DAY).unwrap().unwrap();

    let first = db
        .add_to_daily_plan(
            DAY,
            Some(&note.updated_at),
            &[
                link(task_ref(&task.id), "Send the deck"),
                PlanAddition::NewTask {
                    title: "Draft memo".into(),
                },
            ],
        )
        .unwrap();
    assert!(first
        .note
        .body
        .starts_with("# Monday\n\n## Plan\n\n- [Send the deck]("));
    assert!(first.note.body.contains("- ( ) Draft memo"));
    assert!(first.note.body.ends_with("\n\n## Meetings\n\n- 9am Sync\n"));
    assert_eq!(first.changed.len(), 2);
    assert!(first.already.is_empty());

    let again = db
        .add_to_daily_plan(
            DAY,
            Some(&first.note.updated_at),
            &[link(task_ref(&task.id), "Send the deck")],
        )
        .unwrap();
    assert!(again.changed.is_empty());
    assert_eq!(again.already, [task_ref(&task.id).target().unwrap()]);
    assert_eq!(again.note.body, first.note.body);
    assert_eq!(db.daily_plan(DAY).unwrap().items.len(), 2);

    // The plan-only task is an ordinary note task in Tasks.
    assert!(db
        .list_note_tasks()
        .unwrap()
        .iter()
        .any(|item| item.title == "Draft memo"));
}

#[test]
fn removing_from_a_plan_takes_out_one_item_and_keeps_other_text() {
    let db = database();
    let task = standalone(&db, "Send the deck");
    let saved = db
        .save_daily_note(
            DAY,
            &format!(
                "## Plan\n\n- [Send]({})\n  - with appendix\n- Keep this\n\nJournal\n",
                task_ref(&task.id).href().unwrap()
            ),
        )
        .unwrap()
        .unwrap();
    let key = task_ref(&task.id).target().unwrap();
    let removed = db
        .remove_from_daily_plan(DAY, Some(&saved.updated_at), &key)
        .unwrap();
    assert_eq!(removed.note.body, "## Plan\n\n- Keep this\n\nJournal\n");
    assert_eq!(removed.previous_body, saved.body);
    assert!(db
        .remove_from_daily_plan(DAY, Some(&removed.note.updated_at), &key)
        .is_err());
}

#[test]
fn plan_writes_refuse_stale_versions_unknown_items_unsaved_note_tasks_and_unclosed_code() {
    let db = database();
    let task = standalone(&db, "Send the deck");
    let saved = db.save_daily_note(DAY, "Journal\n").unwrap().unwrap();
    assert!(db
        .add_to_daily_plan(DAY, Some("stale"), &[link(task_ref(&task.id), "x")])
        .is_err());
    assert!(db
        .add_to_daily_plan(
            DAY,
            Some(&saved.updated_at),
            &[link(task_ref("missing"), "x")]
        )
        .is_err());
    let event = NoteReference {
        kind: "event".into(),
        account: Some("a@example.com".into()),
        calendar: Some("c".into()),
        ..task_ref("e")
    };
    assert!(db
        .add_to_daily_plan(DAY, Some(&saved.updated_at), &[link(event, "x")])
        .is_err());
    // A note task whose marker exists only provisionally can't be linked yet.
    let other = db.create_note("Projects", "").unwrap();
    db.connection()
        .unwrap()
        .execute(
            "UPDATE notes SET body='- ( ) Unsaved marker' WHERE id=?1",
            [&other.id],
        )
        .unwrap();
    let provisional = NoteReference {
        note: Some(other.id.clone()),
        ..task_ref("33333333-3333-4333-8333-333333333333")
    };
    assert!(db
        .add_to_daily_plan(DAY, Some(&saved.updated_at), &[link(provisional, "x")])
        .is_err());
    assert!(db
        .add_to_daily_plan(DAY, Some(&saved.updated_at), &[])
        .is_err());
    assert_eq!(db.get_daily_note(DAY).unwrap().unwrap().body, "Journal\n");

    let fenced = db
        .save_daily_note("2026-11-18", "```\ncode")
        .unwrap()
        .unwrap();
    assert!(db
        .add_to_daily_plan(
            "2026-11-18",
            Some(&fenced.updated_at),
            &[PlanAddition::NewTask { title: "x".into() }]
        )
        .is_err());
}
