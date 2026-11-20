use super::*;
use crate::db::test_support::database;
use threestrands_sync_protocol::MAX_NOTE_ATTACHMENT_BYTES;

const DAY: &str = "2026-11-09";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest-of-image";

fn request(target: AddTarget, expected: Option<&str>, quote: Option<&str>) -> AddToNoteRequest {
    AddToNoteRequest {
        target,
        expected: expected.map(str::to_string),
        thread: "welcome".into(),
        message: "welcome-1".into(),
        quote: quote.map(str::to_string),
        date_label: "Oct 30".into(),
        files: Vec::new(),
    }
}

fn existing(id: &str) -> AddTarget {
    AddTarget::Note { id: id.into() }
}

fn file(name: &str, mime: &str, bytes: &[u8]) -> AddedFile {
    AddedFile {
        name: name.into(),
        mime_type: mime.into(),
        bytes: bytes.to_vec(),
    }
}

fn subject(db: &Database) -> String {
    db.connection()
        .unwrap()
        .query_row("SELECT subject FROM threads WHERE id='welcome'", [], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn a_quote_is_appended_with_its_source_line_after_a_blank_line() {
    let db = database();
    let note = db.create_note("Prep", "Agenda\n- one\n").unwrap();
    let added = db
        .add_to_note(
            &request(
                existing(&note.id),
                Some(&note.updated_at),
                Some("first line\nsecond > line\n> nested"),
            ),
            Vec::new(),
        )
        .unwrap();
    assert!(added.added && !added.created_note);
    assert_eq!(added.previous_body, "Agenda\n- one\n");
    let body = db.get_note(&note.id).unwrap().unwrap().body;
    let prefix = "Agenda\n- one\n\n> first line\n> second > line\n> > nested\n> — [";
    assert!(body.starts_with(prefix), "{body}");
    assert!(
        body.contains("](threestrands:conversation/") && body.ends_with(" · Oct 30\n"),
        "{body}"
    );
    // Adding again appends another quote: only a bare link is deduplicated.
    let again = db
        .add_to_note(
            &request(
                existing(&note.id),
                Some(&added.note.updated_at),
                Some("more"),
            ),
            Vec::new(),
        )
        .unwrap();
    assert!(again.added);
    assert_eq!(
        db.get_note(&note.id)
            .unwrap()
            .unwrap()
            .body
            .matches("> — [")
            .count(),
        2
    );
}

#[test]
fn a_link_alone_is_deduplicated_and_labelled_like_a_capture() {
    let db = database();
    db.connection()
        .unwrap()
        .execute(
            "UPDATE threads SET subject = 'Re: Q3 plan (v2.1) - ann_lee@example.com [draft] #ops' WHERE id = 'welcome'",
            [],
        )
        .unwrap();
    let first = db
        .add_to_note(
            &request(existing(&format!("daily:{DAY}")), None, None),
            Vec::new(),
        )
        .unwrap();
    assert!(first.added && first.previous_body.is_empty());
    assert!(
        first.note.body.starts_with("- [Re: Q3 plan (v2.1) - ann_lee@example.com \\[draft\\] \\#ops](threestrands:conversation/"),
        "{}",
        first.note.body
    );
    let doc = db.note_document(&first.note.id).unwrap();
    assert_eq!(doc.links.len(), 1);
    assert_eq!(
        db.resolve_note_conversation(&doc.links[0].reference)
            .unwrap()
            .as_deref(),
        Some("welcome")
    );
    let duplicate = db
        .add_to_note(
            &request(existing(&first.note.id), Some(&first.note.updated_at), None),
            Vec::new(),
        )
        .unwrap();
    assert!(!duplicate.added);
    assert_eq!(duplicate.note.updated_at, first.note.updated_at);
    // Files with an already present link add only the files.
    let with_file = db
        .add_to_note(
            &request(existing(&first.note.id), Some(&first.note.updated_at), None),
            vec![file("a.pdf", "application/pdf", b"pdf")],
        )
        .unwrap();
    assert!(with_file.added);
    assert_eq!(
        with_file
            .note
            .body
            .matches("threestrands:conversation/")
            .count(),
        1
    );
    assert!(with_file.note.body.contains("[a.pdf](attachment:"));
    let _ = subject(&db);
}

#[test]
fn a_stale_trashed_conflicted_or_fenced_note_is_refused_unchanged() {
    let db = database();
    let note = db.create_note("Prep", "text\n").unwrap();
    assert!(db
        .add_to_note(
            &request(existing(&note.id), Some("old"), Some("q")),
            Vec::new()
        )
        .is_err());
    assert!(db
        .add_to_note(&request(existing(&note.id), None, Some("q")), Vec::new())
        .is_err());
    let fenced = db
        .update_note(&note.id, "Prep", "User text\n```\ncode\n")
        .unwrap();
    let error = db
        .add_to_note(
            &request(existing(&note.id), Some(&fenced.updated_at), Some("q")),
            Vec::new(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("code block"), "{error}");
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, fenced.body);
    let closed = db.update_note(&note.id, "Prep", "text\n").unwrap();
    db.trash_note(&note.id).unwrap();
    assert!(db
        .add_to_note(
            &request(existing(&note.id), Some(&closed.updated_at), Some("q")),
            Vec::new()
        )
        .is_err());
    assert!(db
        .add_to_note(&request(existing("missing"), None, Some("q")), Vec::new())
        .is_err());
    assert!(db
        .add_to_note(
            &request(existing("daily:not-a-day"), None, Some("q")),
            Vec::new()
        )
        .is_err());
}

#[test]
fn a_daily_note_that_does_not_exist_yet_is_created_and_a_later_edit_is_refused() {
    let db = database();
    let id = format!("daily:{DAY}");
    let first = db
        .add_to_note(&request(existing(&id), None, Some("hi")), Vec::new())
        .unwrap();
    assert_eq!(first.note.daily_date.as_deref(), Some(DAY));
    assert!(first.note.body.starts_with("> hi\n> — ["));
    assert!(db
        .add_to_note(&request(existing(&id), None, Some("again")), Vec::new())
        .is_err());
}

#[test]
fn files_are_copied_into_the_note_and_undo_takes_back_text_and_files() {
    let db = database();
    let note = db.create_note("Prep", "Before\n").unwrap();
    let mut req = request(existing(&note.id), Some(&note.updated_at), Some("q"));
    req.files = vec!["p1".into(), "p2".into()];
    let added = db
        .add_to_note(
            &req,
            vec![
                file("../shot.png", "image/png", PNG),
                file("report.pdf", "application/pdf", b"%PDF"),
            ],
        )
        .unwrap();
    assert_eq!(added.attachments.len(), 2);
    assert!(added.attachments[0].image && !added.attachments[1].image);
    assert_eq!(added.attachments[0].name, "shot.png");
    let body = db.get_note(&note.id).unwrap().unwrap().body;
    assert!(
        body.contains(&format!(
            "![shot.png](attachment:{})",
            added.attachments[0].id
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "[report.pdf](attachment:{})",
            added.attachments[1].id
        )),
        "{body}"
    );
    let (stored, bytes) = db
        .note_attachment_bytes(&added.attachments[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (stored.note_id.as_str(), bytes.as_slice()),
        (note.id.as_str(), PNG)
    );

    let undo = UndoAddToNote {
        note_id: note.id.clone(),
        expected: added.note.updated_at.clone(),
        previous_body: added.previous_body.clone(),
        created_note: false,
        attachments: added.attachments.iter().map(|a| a.id.clone()).collect(),
    };
    db.update_note(&note.id, "Prep", &format!("{body}more\n"))
        .unwrap();
    assert!(
        db.undo_add_to_note(&undo).is_err(),
        "undo refuses after an edit"
    );
    assert!(db
        .note_attachment(&added.attachments[0].id)
        .unwrap()
        .is_some());
    db.update_note(&note.id, "Prep", &body).unwrap();
    let current = db.get_note(&note.id).unwrap().unwrap();
    let undone = db
        .undo_add_to_note(&UndoAddToNote {
            expected: current.updated_at,
            ..undo
        })
        .unwrap();
    assert_eq!(undone.body, "Before\n");
    for attachment in &added.attachments {
        assert!(db.note_attachment(&attachment.id).unwrap().is_none());
    }
}

#[test]
fn a_file_over_the_note_limit_refuses_the_whole_addition() {
    let db = database();
    let note = db.create_note("Prep", "Before\n").unwrap();
    let mut req = request(existing(&note.id), Some(&note.updated_at), Some("q"));
    req.files = vec!["p1".into(), "p2".into()];
    let at_limit = vec![0u8; MAX_NOTE_ATTACHMENT_BYTES as usize];
    let over = vec![0u8; MAX_NOTE_ATTACHMENT_BYTES as usize + 1];
    let error = db
        .add_to_note(
            &req,
            vec![
                file("small.bin", "application/octet-stream", b"x"),
                file("big.bin", "application/octet-stream", &over),
            ],
        )
        .unwrap_err();
    assert!(error.to_string().contains("20 MB"), "{error}");
    assert_eq!(db.get_note(&note.id).unwrap().unwrap().body, "Before\n");
    assert!(db.note_attachments_for_export().unwrap().is_empty());
    req.files = vec!["p1".into()];
    assert!(db
        .add_to_note(
            &req,
            vec![file("exact.bin", "application/octet-stream", &at_limit)]
        )
        .is_ok());
}

#[test]
fn a_new_note_is_created_with_the_addition_and_undo_trashes_it() {
    let db = database();
    let mut req = request(
        AddTarget::New {
            title: "  Board   prep ".into(),
        },
        None,
        Some("q"),
    );
    let added = db.add_to_note(&req, Vec::new()).unwrap();
    assert!(
        added.created_note && added.note.title == "Board prep" && added.note.daily_date.is_none()
    );
    assert!(
        db.add_to_note(&req, Vec::new()).is_err(),
        "the title is taken"
    );
    req.expected = Some("x".into());
    assert!(db.add_to_note(&req, Vec::new()).is_err());
    assert!(db
        .add_to_note(
            &request(AddTarget::New { title: "  ".into() }, None, Some("q")),
            Vec::new()
        )
        .is_err());
    let undone = db
        .undo_add_to_note(&UndoAddToNote {
            note_id: added.note.id.clone(),
            expected: added.note.updated_at.clone(),
            previous_body: String::new(),
            created_note: true,
            attachments: Vec::new(),
        })
        .unwrap();
    assert!(undone.trashed && undone.body.is_empty());
}

#[test]
fn limits_and_message_ownership_are_checked() {
    let db = database();
    let note = db.create_note("Prep", "").unwrap();
    let long = "x".repeat(crate::note_additions::MAX_ADDITION_QUOTE_CHARS + 1);
    assert!(db
        .add_to_note(
            &request(existing(&note.id), Some(&note.updated_at), Some(&long)),
            Vec::new()
        )
        .is_err());
    assert!(!db.message_in_thread("nope", "welcome").unwrap());
}
