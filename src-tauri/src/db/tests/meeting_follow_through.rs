use super::*;
use crate::db::meetings::FollowThroughState;
use crate::db::test_support::database;
use crate::meeting_follow_through::FOLLOW_THROUGH_MARKER;
use chrono::TimeZone;

const ME: &str = "me@example.com";
const DAY: &str = "2026-11-16";
const OP: &str = "11111111-1111-4111-8111-111111111111";
const OP2: &str = "22222222-2222-4222-8222-222222222222";

fn occurrence(id: &str) -> NoteReference {
    NoteReference {
        version: 1,
        kind: "event".into(),
        id: id.into(),
        account: Some(ME.into()),
        calendar: Some("work".into()),
        note: None,
        contact: None,
        series: None,
    }
}

fn href(id: &str) -> String {
    occurrence(id).href().unwrap()
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .with_ymd_and_hms(2026, 11, 16, 18, 0, 0)
        .unwrap()
}

fn accepted(items: Vec<FollowThroughItem>) -> AcceptedFollowThrough {
    AcceptedFollowThrough {
        date: DAY.into(),
        meeting: "Sync".into(),
        summary: None,
        items,
        recording_id: None,
        attach_transcript: false,
    }
}

fn decision(text: &str) -> FollowThroughItem {
    FollowThroughItem::Decision { text: text.into() }
}

fn task(text: &str) -> FollowThroughItem {
    FollowThroughItem::Task { text: text.into() }
}

/// A daily note with the meeting's line and a neighbor's.
fn daily(db: &Database) -> Note {
    db.save_daily_note(
        DAY,
        &format!(
            "## Meetings\n\n- [9:30am]({}) for Sync\n  - Agenda\n- [11am]({}) for Other\n\n## Journal\n\nPrivate\n",
            href("evt-1"),
            href("evt-2")
        ),
    )
    .unwrap()
    .unwrap()
}

fn save(
    db: &Database,
    note: &Note,
    operation: &str,
    items: Vec<FollowThroughItem>,
) -> DbResult<FollowThroughSave> {
    db.save_meeting_follow_through(
        &occurrence("evt-1"),
        &FollowThroughDestination {
            note_id: note.id.clone(),
        },
        &note.updated_at,
        operation,
        &accepted(items),
    )
}

fn state(db: &Database, id: &str) -> FollowThroughState {
    db.meeting_readiness(&[occurrence(id)], None, now())
        .unwrap()[0]
        .follow_through
}

fn receipts(db: &Database) -> i64 {
    db.test_connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operation_receipts", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn saving_nests_one_section_under_the_meeting_line_and_marks_only_that_meeting_reviewed() {
    let db = database();
    let note = daily(&db);
    assert_eq!(state(&db, "evt-1"), FollowThroughState::NotReviewed);

    let saved = save(
        &db,
        &note,
        OP,
        vec![
            decision("Ship on Friday"),
            task("Send the deck"),
            FollowThroughItem::Checklist {
                text: "Book the room".into(),
            },
            FollowThroughItem::Action {
                text: "Dana checks pricing".into(),
            },
        ],
    )
    .unwrap();
    assert!(!saved.replayed);
    let body = &saved.note.body;
    assert!(body.starts_with(&format!(
        "## Meetings\n\n- [9:30am]({0}) for Sync\n  - Agenda\n- Follow-through 2026-11-16: [Sync]({0}) {FOLLOW_THROUGH_MARKER}\n  - Decision: Ship on Friday\n  - ( ) Send the deck <!-- threestrands-task:",
        href("evt-1")
    )), "{body}");
    assert!(body.ends_with(&format!(
        "  - [ ] Book the room\n  - Action: Dana checks pricing\n- [11am]({}) for Other\n\n## Journal\n\nPrivate\n",
        href("evt-2")
    )), "{body}");
    assert_eq!(&body[saved.start..saved.end], saved.section);
    // Other text is untouched: removing the section gives back the note.
    let mut undone = body.clone();
    undone.replace_range(saved.start..saved.end, "");
    assert_eq!(undone, note.body);

    // The round task is an ordinary note task; the checklist stays in the note.
    assert_eq!(saved.task_ids.len(), 1);
    assert_eq!(saved.checklist_ids.len(), 1);
    let tasks = db.list_note_tasks().unwrap();
    let listed: Vec<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(listed, [saved.task_ids[0].as_str()]);
    assert_eq!(tasks[0].title, "Send the deck");

    assert_eq!(state(&db, "evt-1"), FollowThroughState::Reviewed);
    assert_eq!(state(&db, "evt-2"), FollowThroughState::NotReviewed);
    let context = db
        .meeting_context(&occurrence("evt-1"), None, now())
        .unwrap();
    assert_eq!(context.follow_through.items.len(), 1);
    assert_eq!(context.follow_through.items[0].date.as_deref(), Some(DAY));
    assert_eq!(context.follow_through.items[0].note_id, note.id);
}

#[test]
fn a_retry_with_the_same_operation_returns_the_first_result_without_writing_again() {
    let db = database();
    let note = daily(&db);
    let first = save(&db, &note, OP, vec![task("Send the deck")]).unwrap();
    // A lost response: the client retries with the version it started from.
    let again = save(&db, &note, OP, vec![task("Send the deck")]).unwrap();
    assert!(again.replayed);
    assert_eq!(again.note.updated_at, first.note.updated_at);
    assert_eq!(again.task_ids, first.task_ids);
    let current = db.get_note(&note.id).unwrap().unwrap();
    assert_eq!(current.body, first.note.body);
    assert_eq!(db.list_note_tasks().unwrap().len(), 1);
    assert_eq!(receipts(&db), 1);

    // The same id for another meeting is refused, not answered with this result.
    let error = db
        .save_meeting_follow_through(
            &occurrence("evt-2"),
            &FollowThroughDestination {
                note_id: note.id.clone(),
            },
            &current.updated_at,
            OP,
            &accepted(vec![decision("x")]),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("already used"), "{error}");

    // A new operation is a new section, checked against the current version.
    let stale = save(&db, &note, OP2, vec![decision("Again")]).unwrap_err();
    assert!(stale.to_string().contains("changed"), "{stale}");
    let second = save(&db, &current, OP2, vec![decision("Again")]).unwrap();
    assert_eq!(
        crate::meeting_follow_through::sections(&note.id, &second.note.body, &occurrence("evt-1"))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn refusals_write_nothing() {
    let db = database();
    let note = daily(&db);
    let unrelated = db.create_note("Unrelated", "Nothing about it").unwrap();
    let error = save(&db, &unrelated, OP, vec![decision("x")]).unwrap_err();
    assert!(
        error.to_string().contains("isn't about this meeting"),
        "{error}"
    );

    // A linked task that no longer exists fails the whole save.
    let missing = FollowThroughItem::Link {
        task: NoteReference {
            version: 1,
            kind: "task".into(),
            id: "gone".into(),
            account: None,
            calendar: None,
            note: None,
            contact: None,
            series: None,
        },
        title: "Gone".into(),
    };
    let error = save(&db, &note, OP, vec![task("Kept?"), missing]).unwrap_err();
    assert!(error.to_string().contains("no longer exists"), "{error}");

    // Over the size budget: nothing is written either.
    let big = db
        .create_note(
            "Big",
            &format!(
                "- [Sync]({})\n{}",
                href("evt-1"),
                "x".repeat(threestrands_sync_protocol::MAX_NOTE_BODY_BYTES - 200)
            ),
        )
        .unwrap();
    let long = "y".repeat(crate::meeting_policy::MAX_FOLLOW_THROUGH_ITEM_CHARS);
    let error = save(&db, &big, OP, vec![decision(&long)]).unwrap_err();
    assert!(error.to_string().contains("too long"), "{error}");
    assert_eq!(db.get_note(&big.id).unwrap().unwrap().body, big.body);

    // In Trash.
    db.trash_note(&big.id).unwrap();
    let trashed = db.get_note(&big.id).unwrap().unwrap();
    let error = save(&db, &trashed, OP, vec![decision("x")]).unwrap_err();
    assert!(error.to_string().contains("Trash"), "{error}");

    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, note.body);
    assert!(db.list_note_tasks().unwrap().is_empty());
    assert_eq!(receipts(&db), 0);
    assert_eq!(state(&db, "evt-1"), FollowThroughState::NotReviewed);
}

#[test]
fn a_reusable_meeting_note_gets_a_section_that_names_the_occurrence() {
    let db = database();
    let reusable = db.create_note("Sync notes", "Running notes").unwrap();
    db.save_daily_note(
        DAY,
        &format!("- [9:30am]({}) for [[Sync notes]]\n", href("evt-1")),
    )
    .unwrap();
    let saved = save(&db, &reusable, OP, vec![decision("Ship")]).unwrap();
    assert_eq!(
        saved.note.body,
        format!(
            "Running notes\n\n- Follow-through 2026-11-16: [Sync]({}) {FOLLOW_THROUGH_MARKER}\n  - Decision: Ship\n",
            href("evt-1")
        )
    );
    assert_eq!(state(&db, "evt-1"), FollowThroughState::Reviewed);
    let context = db
        .meeting_context(&occurrence("evt-1"), None, now())
        .unwrap();
    assert_eq!(context.follow_through.items[0].note_id, reusable.id);
}

#[test]
fn a_linked_task_is_written_with_its_current_title_and_not_copied() {
    let db = database();
    db.adopt_account(ME).unwrap();
    db.upsert_thread(
        ME,
        &[crate::db::test_support::message(
            "m1",
            "t1",
            "2026-11-10T09:00:00Z",
            "Body",
        )],
    )
    .unwrap();
    let existing = db
        .create_task(
            &serde_json::from_value(serde_json::json!({
                "accountId": ME, "threadId": format!("{ME}:t1"),
                "subjectSnapshot": "Subject", "title": "Review the contract", "kind": "action"
            }))
            .unwrap(),
        )
        .unwrap();
    let note = daily(&db);
    let reference = NoteReference {
        version: 1,
        kind: "task".into(),
        id: existing.id.clone(),
        account: None,
        calendar: None,
        note: None,
        contact: None,
        series: None,
    };
    let saved = save(
        &db,
        &note,
        OP,
        vec![FollowThroughItem::Link {
            task: reference.clone(),
            title: "Stale title".into(),
        }],
    )
    .unwrap();
    assert!(saved.section.contains(&format!(
        "[Review the contract]({})",
        reference.href().unwrap()
    )));
    assert!(saved.task_ids.is_empty());
    assert!(db.list_note_tasks().unwrap().is_empty());
}

#[test]
fn undo_removes_exactly_the_section_and_refuses_after_later_changes() {
    let db = database();
    let note = daily(&db);
    save(&db, &note, OP, vec![task("Send the deck")]).unwrap();
    let undone = db.undo_meeting_follow_through(OP).unwrap();
    assert_eq!(undone.note.body, note.body);
    assert!(db.list_note_tasks().unwrap().is_empty());
    assert_eq!(state(&db, "evt-1"), FollowThroughState::NotReviewed);
    // Repeating undo changes nothing.
    let again = db.undo_meeting_follow_through(OP).unwrap();
    assert_eq!(again.note.updated_at, undone.note.updated_at);
    // Retrying the save still returns its first result rather than writing.
    assert!(
        save(&db, &note, OP, vec![task("Send the deck")])
            .unwrap()
            .replayed
    );
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, note.body);

    // Completing an inserted task afterwards keeps the section.
    let current = db.get_note(&note.id).unwrap().unwrap();
    let saved = save(&db, &current, OP2, vec![task("Send the deck")]).unwrap();
    let marker = saved.task_ids[0].rsplit(':').next().unwrap().to_string();
    db.patch_note_item(&note.id, &marker, &saved.note.updated_at, Some(true), None)
        .unwrap();
    let error = db.undo_meeting_follow_through(OP2).unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");
    assert!(db
        .get_note(&note.id)
        .unwrap()
        .unwrap()
        .body
        .contains("(x) Send the deck"));
}

#[test]
fn undo_refuses_after_an_unrelated_edit_and_never_restores_an_old_body() {
    let db = database();
    let note = daily(&db);
    let saved = save(&db, &note, OP, vec![decision("Ship")]).unwrap();
    let edited = format!("{}More writing\n", saved.note.body);
    db.save_daily_note(DAY, &edited).unwrap();
    assert!(db.undo_meeting_follow_through(OP).is_err());
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, edited);
    assert!(db
        .undo_meeting_follow_through("33333333-3333-4333-8333-333333333333")
        .is_err());
}

#[test]
fn receipts_are_bounded_and_go_with_their_note() {
    let db = database();
    let note = daily(&db);
    db.with_transaction(|tx| {
        for index in 0..crate::meeting_policy::MAX_OPERATION_RECEIPTS + 1 {
            receipts::record(
                tx,
                &uuid::Uuid::new_v4().to_string(),
                "test",
                &format!("scope-{index}"),
                Some(&note.id),
                "{}",
            )?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        receipts(&db),
        crate::meeting_policy::MAX_OPERATION_RECEIPTS as i64
    );
    let other = db.create_note("Other", "x").unwrap();
    db.trash_note(&other.id).unwrap();
    db.with_transaction(|tx| receipts::record(tx, OP, "test", "s", Some(&other.id), "{}"))
        .unwrap();
    db.delete_note(&other.id).unwrap();
    let left: i64 = db
        .test_connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operation_receipts WHERE note_id=?1",
            [&other.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn follow_through_saved_on_another_device_counts_there_too() {
    let a = database();
    a.set_beta_features_enabled(true).unwrap();
    let b = database();
    b.set_beta_features_enabled(true).unwrap();
    let note = daily(&a);
    let sync = |peer: &Database, source: &Database| {
        let touched = peer
            .merge_replica_state(&source.load_replica_state().unwrap())
            .unwrap();
        peer.materialize_touched_entities(&touched).unwrap();
    };
    sync(&b, &a);
    assert_eq!(state(&b, "evt-1"), FollowThroughState::NotReviewed);
    save(&a, &note, OP, vec![decision("Ship")]).unwrap();
    sync(&b, &a);
    assert_eq!(state(&b, "evt-1"), FollowThroughState::Reviewed);
    // Receipts stay on the device that wrote them.
    assert_eq!(receipts(&a), 1);
    assert_eq!(receipts(&b), 0);
}

fn handoff_draft(db: &Database) -> crate::correspondence::Draft {
    match db.build_draft("new", None, ME, &[], false).unwrap() {
        crate::correspondence::BuiltDraft::New(draft) => draft,
        crate::correspondence::BuiltDraft::Existing(_) => panic!("a new message is never reused"),
    }
}

#[test]
fn a_handoff_has_one_draft_and_reports_where_it_stands() {
    use crate::db::drafts::DraftHandoffState;
    let db = database();
    db.adopt_account(ME).unwrap();
    assert!(db.draft_handoff_status(OP).unwrap().is_none());

    let mut first = handoff_draft(&db);
    first.subject = "Follow-up: Sync".into();
    assert!(db.insert_handoff_draft(OP, &mut first).unwrap());
    // A duplicate click or a retry after a lost response makes no second draft.
    let mut second = handoff_draft(&db);
    assert!(!db.insert_handoff_draft(OP, &mut second).unwrap());
    assert_eq!(db.drafts().unwrap().len(), 1);
    let status = db.draft_handoff_status(OP).unwrap().unwrap();
    assert_eq!(status.state, DraftHandoffState::Open);
    assert_eq!(status.draft.unwrap().id, first.id);

    // Nothing is queued: the outbox stays empty until the user sends.
    assert!(db.outbox().unwrap().is_empty());

    db.discard_draft(&first.id).unwrap();
    let status = db.draft_handoff_status(OP).unwrap().unwrap();
    assert_eq!(status.state, DraftHandoffState::Missing);
    assert!(status.draft.is_none());
    // Reopening never recreates it silently.
    let mut again = handoff_draft(&db);
    assert!(!db.insert_handoff_draft(OP, &mut again).unwrap());
    assert!(db.drafts().unwrap().is_empty());

    db.test_connection()
        .unwrap()
        .execute(
            "INSERT INTO outbox_messages(id, draft_id, revision, account, state, deadline, payload, raw) VALUES ('o1', ?1, 1, ?2, 'sent', 0, '{}', x'')",
            rusqlite::params![first.id, ME],
        )
        .unwrap();
    assert_eq!(
        db.draft_handoff_status(OP).unwrap().unwrap().state,
        DraftHandoffState::Sent
    );

    // Removing the account forgets its handoffs.
    db.remove_account(ME).unwrap();
    assert!(db.draft_handoff_status(OP).unwrap().is_none());
}

fn seed_transcribed_recording(
    db: &Database,
    id: &str,
    event_ref: Option<&NoteReference>,
    title: &str,
) {
    use crate::db::recordings::{NewRecording, RecordingState, TrackState, TranscriptRow};
    use crate::recording::capture::Track;
    db.create_recording(&NewRecording {
        id,
        event_ref,
        title,
        attendees: &["ann@example.org".into()],
        started_at: "2026-11-16T09:30:00Z",
        audio_dir: "/tmp/audio",
        tracks: &[
            (Track::Mic, TrackState::Active),
            (Track::System, TrackState::Active),
        ],
    })
    .unwrap();
    db.finish_recording(
        id,
        "2026-11-16T10:00:00Z",
        1_800_000,
        RecordingState::Recorded,
        None,
    )
    .unwrap();
    db.save_transcript(
        id,
        &[
            TranscriptRow {
                track: Track::System,
                start_ms: 5_000,
                end_ms: 9_000,
                text: "Let's ship on Friday.".into(),
                rms: None,
                dropped_as_echo: false,
            },
            TranscriptRow {
                track: Track::Mic,
                start_ms: 10_000,
                end_ms: 14_000,
                text: "I'll send the deck today.".into(),
                rms: None,
                dropped_as_echo: false,
            },
        ],
        "speech_analyzer",
        "en-US",
        None,
    )
    .unwrap();
}

#[test]
fn saving_with_recap_transcript_and_undo_manages_attachment_and_reviewed_flag() {
    let db = database();
    let note = daily(&db);
    seed_transcribed_recording(&db, "rec-1", Some(&occurrence("evt-1")), "Sync");

    let saved = db
        .save_meeting_follow_through(
            &occurrence("evt-1"),
            &FollowThroughDestination {
                note_id: note.id.clone(),
            },
            &note.updated_at,
            OP,
            &AcceptedFollowThrough {
                date: DAY.into(),
                meeting: "Sync".into(),
                summary: Some("Aligned on shipping Friday.".into()),
                items: vec![
                    decision("Ship on Friday"),
                    FollowThroughItem::Question {
                        text: "Does legal need the deck?".into(),
                    },
                ],
                recording_id: Some("rec-1".into()),
                attach_transcript: true,
            },
        )
        .unwrap();

    let att_id = saved
        .attachment_id
        .clone()
        .expect("transcript attachment id");
    assert_eq!(saved.recording_id.as_deref(), Some("rec-1"));
    assert!(saved
        .section
        .contains("Summary: Aligned on shipping Friday."));
    assert!(saved
        .section
        .contains("Open question: Does legal need the deck?"));
    assert!(saved.section.contains(&format!(
        "Transcript: [Transcript – Sync – {DAY}.md](attachment:{att_id})"
    )));

    let (meta, bytes) = db.note_attachment_bytes(&att_id).unwrap().unwrap();
    assert_eq!(meta.mime_type, "text/markdown");
    assert_eq!(meta.name, format!("Transcript – Sync – {DAY}.md"));
    let md = String::from_utf8(bytes).unwrap();
    assert!(md.contains("Let's ship on Friday."));
    assert!(md.contains("I'll send the deck today."));

    let rec = db.recording("rec-1").unwrap().unwrap();
    assert_eq!(rec.reviewed_operation_id.as_deref(), Some(OP));

    // Undoing removes the section, deletes the attachment, and clears reviewed_operation_id.
    let undone = db.undo_meeting_follow_through(OP).unwrap();
    assert_eq!(undone.note.body, note.body);
    assert!(db.note_attachment(&att_id).unwrap().is_none());
    let rec_after = db.recording("rec-1").unwrap().unwrap();
    assert_eq!(rec_after.reviewed_operation_id, None);
}

#[test]
fn save_adhoc_follow_through_writes_daily_note_with_transcript_and_undoes() {
    let db = database();
    seed_transcribed_recording(&db, "rec-adhoc", None, "Coffee chat");

    let saved = db
        .save_adhoc_follow_through(
            DAY,
            "2:15 PM",
            None,
            OP,
            &AcceptedFollowThrough {
                date: DAY.into(),
                meeting: "Coffee chat".into(),
                summary: Some("Agreed on Q4 hiring plan.".into()),
                items: vec![task("Share rubric")],
                recording_id: Some("rec-adhoc".into()),
                attach_transcript: true,
            },
        )
        .unwrap();

    let att_id = saved.attachment_id.clone().unwrap();
    assert!(saved.note.body.contains("## Meetings"));
    assert!(saved.note.body.contains(&format!(
        "- 2:15 PM for Coffee chat {FOLLOW_THROUGH_MARKER}"
    )));
    assert!(saved
        .note
        .body
        .contains("Summary: Agreed on Q4 hiring plan."));
    assert!(saved.note.body.contains("( ) Share rubric"));
    assert!(saved.note.body.contains(&format!("(attachment:{att_id})")));
    assert_eq!(
        db.recording("rec-adhoc")
            .unwrap()
            .unwrap()
            .reviewed_operation_id
            .as_deref(),
        Some(OP)
    );

    let undone = db.undo_meeting_follow_through(OP).unwrap();
    assert!(!undone.note.body.contains("Coffee chat"));
    assert!(db.note_attachment(&att_id).unwrap().is_none());
    assert_eq!(
        db.recording("rec-adhoc")
            .unwrap()
            .unwrap()
            .reviewed_operation_id,
        None
    );
}

#[test]
fn occurrence_notes_for_recap_never_returns_exclude_from_ai_notes() {
    let db = database();
    let occ = occurrence("evt-1");
    let included = db
        .save_daily_note(
            DAY,
            &format!(
                "- [9:30am]({}) for Sync\n  - Discuss launch milestones\n",
                href("evt-1")
            ),
        )
        .unwrap()
        .unwrap();
    let secret = db
        .create_note(
            "Private Sync Notes",
            &format!("About [Sync]({}): TOP_SECRET_SALARY_DATA", href("evt-1")),
        )
        .unwrap();
    db.set_note_exclude_from_ai(&secret.id, true).unwrap();

    let (notes_text, any_excluded) = db.occurrence_notes_for_recap(&occ).unwrap();
    assert!(any_excluded);
    let combined = notes_text.expect("non-excluded note should still be included");
    assert!(combined.contains("Discuss launch milestones"));
    assert!(!combined.contains("TOP_SECRET_SALARY_DATA"));

    // When the only note is excluded, notes_text is None and any_excluded is true.
    db.set_note_exclude_from_ai(&included.id, true).unwrap();
    let (none_text, still_excluded) = db.occurrence_notes_for_recap(&occ).unwrap();
    assert!(still_excluded);
    assert!(none_text.is_none());
}
