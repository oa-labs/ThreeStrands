use super::*;
use crate::db::test_support::database;
use crate::models::Note;
use threestrands_sync_transport::TransportInstanceId;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest-of-image";

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

fn count(db: &Database, sql: &str) -> i64 {
    db.test_connection()
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

/// Stands in for sealing: objects with made-up bytes, enough for the
/// bookkeeping these tests check. `replicated_sync` tests the real thing.
fn seal(db: &Database, id: &str) {
    seal_by(db, id, &"ab".repeat(16));
}

fn seal_by(db: &Database, id: &str, device: &str) {
    let sealed = SealedAttachment {
        sealed_by: device.to_string(),
        index_cid: format!("index{}{}", &id[..8], &device[..4]),
        objects: vec![
            (
                format!("chunk{}", &id[..8]),
                "attachment_chunk",
                vec![1, 2, 3],
            ),
            (format!("index{}", &id[..8]), "attachment_index", vec![4]),
        ],
    };
    db.store_sealed_note_attachment(id, &sealed, &[TransportInstanceId("one".into())])
        .unwrap();
}

#[test]
fn an_attachment_keeps_its_bytes_and_a_safe_name_and_type() {
    let db = database();
    let note = db.create_note("Plan", "").unwrap();
    let image = db
        .add_note_attachment(&note.id, "../../Screens/shot\u{7}.png", "text/html", PNG)
        .unwrap();
    assert_eq!(image.name, "shot.png");
    // An image is known by its bytes, never by the type it arrived with.
    assert_eq!(image.mime_type, "image/png");
    assert!(image.image && image.available);
    assert!(is_attachment_id(&image.id));
    assert_eq!(db.note_attachment_bytes(&image.id).unwrap().unwrap().1, PNG);

    // A file claiming to be an image without an image's bytes isn't one.
    let fake = db
        .add_note_attachment(&note.id, "fake.png", "image/png", b"<svg onload=x>")
        .unwrap();
    assert_eq!(fake.mime_type, "application/octet-stream");
    assert!(!fake.image);
    let pdf = db
        .add_note_attachment(&note.id, "Plan.pdf", "Application/PDF; x=1", b"%PDF")
        .unwrap();
    assert_eq!(pdf.mime_type, "application/pdf");
    let odd = db
        .add_note_attachment(&note.id, "", "not a type", b"")
        .unwrap();
    assert_eq!(
        (odd.name.as_str(), odd.mime_type.as_str()),
        ("Attachment", "application/octet-stream")
    );

    // Today's note can take a file before it is first saved; other notes must exist.
    assert!(db
        .add_note_attachment("daily:2026-11-09", "a.txt", "text/plain", b"a")
        .is_ok());
    assert!(db
        .add_note_attachment("daily:2026-13-40", "a.txt", "text/plain", b"a")
        .is_err());
    assert!(db
        .add_note_attachment("missing", "a.txt", "text/plain", b"a")
        .is_err());
}

#[test]
fn file_size_is_bounded_below_at_and_above_the_limit() {
    let db = database();
    let note = db.create_note("Plan", "").unwrap();
    let limit = MAX_NOTE_ATTACHMENT_BYTES as usize;
    for (size, ok) in [(limit - 1, true), (limit, true), (limit + 1, false)] {
        let result = db.add_note_attachment(&note.id, "big.bin", "", &vec![0; size]);
        assert_eq!(result.is_ok(), ok, "{size}");
    }
    let storage = db.notes_storage().unwrap();
    assert_eq!(
        (storage.attachment_count, storage.attachment_bytes),
        (2, (2 * limit - 1) as u64)
    );
}

#[test]
fn the_number_of_files_is_bounded() {
    let db = database();
    let note = db.create_note("Plan", "").unwrap();
    {
        let connection = db.test_connection().unwrap();
        for index in 0..MAX_SYNCED_NOTE_ATTACHMENTS - 1 {
            connection
                .execute(
                    "INSERT INTO note_attachments(id, note_id, name, mime_type, size, sha256, created_at)
                     VALUES (?1, ?2, 'a', 'text/plain', 0, '', '')",
                    params![format!("{index:032x}"), note.id],
                )
                .unwrap();
        }
    }
    db.add_note_attachment(&note.id, "last.txt", "text/plain", b"a")
        .unwrap();
    let error = db
        .add_note_attachment(&note.id, "one-more.txt", "text/plain", b"a")
        .unwrap_err();
    assert!(error.to_string().contains("2,000"), "{error}");
}

#[test]
fn an_attachment_reaches_the_replica_only_once_sealed_and_only_while_syncing() {
    let local = database();
    let note = local.create_note("Plan", "").unwrap();
    local
        .add_note_attachment(&note.id, "a.png", "", PNG)
        .unwrap();
    assert!(local.note_attachments_to_seal().unwrap().is_empty());

    let db = synced();
    let note = db.create_note("Plan", "").unwrap();
    let file = db.add_note_attachment(&note.id, "a.png", "", PNG).unwrap();
    assert_eq!(db.note_attachments_to_seal().unwrap(), [file.id.clone()]);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sync_values WHERE entity_type='note_attachment'"
        ),
        0
    );
    seal(&db, &file.id);
    assert!(db.note_attachments_to_seal().unwrap().is_empty());
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sync_deliveries WHERE state='pending'"
        ),
        2
    );
    // The objects aren't part of this device's snapshot.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sync_objects WHERE object_kind LIKE 'attachment_%' AND state_sequence IS NULL"
        ),
        2
    );
    let payloads = db.note_attachment_sync_payloads().unwrap();
    assert_eq!(payloads.len(), 1);
    EntityType::NoteAttachment
        .validate_payload(&payloads[0].1)
        .unwrap();
}

#[test]
fn another_device_learns_of_a_file_and_stores_it_only_when_it_matches() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Plan", "").unwrap();
    let file = a.add_note_attachment(&note.id, "a.png", "", PNG).unwrap();
    seal(&a, &file.id);
    sync_into(&b, &a);

    let seen = b.note_attachment(&file.id).unwrap().unwrap();
    assert_eq!(
        (seen.name.as_str(), seen.size, seen.available),
        ("a.png", PNG.len() as u64, false)
    );
    let pending = b.note_attachments_to_fetch().unwrap();
    assert_eq!(pending.len(), 1);
    let fetch = &pending[0];

    assert!(b
        .store_fetched_note_attachment(fetch, b"\x89PNG\r\n\x1a\nother-image")
        .is_err());
    b.record_note_attachment_fetch_failure(&file.id, "not there yet")
        .unwrap();
    // A failed download waits before it is tried again.
    assert!(b.note_attachments_to_fetch().unwrap().is_empty());

    b.store_fetched_note_attachment(fetch, PNG).unwrap();
    assert!(b.note_attachment(&file.id).unwrap().unwrap().available);
    assert_eq!(b.note_attachment_bytes(&file.id).unwrap().unwrap().1, PNG);
}

#[test]
fn deleting_a_note_deletes_its_files_everywhere_and_retires_its_objects() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Plan", "").unwrap();
    let kept = a.create_note("Other", "").unwrap();
    let file = a.add_note_attachment(&note.id, "a.png", "", PNG).unwrap();
    let other = a.add_note_attachment(&kept.id, "b.png", "", PNG).unwrap();
    seal(&a, &file.id);
    seal(&a, &other.id);
    a.test_connection()
        .unwrap()
        .execute("UPDATE sync_deliveries SET state='delivered'", [])
        .unwrap();
    sync_into(&b, &a);
    assert!(b.note_attachment(&file.id).unwrap().is_some());

    a.trash_note(&note.id).unwrap();
    a.delete_note(&note.id).unwrap();
    assert!(a.note_attachment(&file.id).unwrap().is_none());
    assert!(a.note_attachment(&other.id).unwrap().is_some());
    // Its objects leave each connector once a snapshot without it is published.
    assert_eq!(count(&a, "SELECT COUNT(*) FROM sync_retired_objects"), 2);
    assert_eq!(
        count(&a, "SELECT COUNT(*) FROM note_attachment_objects"),
        2,
        "only the kept file's objects remain"
    );

    sync_into(&b, &a);
    assert!(b.note_attachment(&file.id).unwrap().is_none());
    assert!(b.note_attachment(&other.id).unwrap().is_some());
}

#[test]
fn leaving_a_group_keeps_files_this_device_has_and_seals_them_again() {
    let (a, b) = (synced(), synced());
    let note = a.create_note("Plan", "").unwrap();
    let mine = a.add_note_attachment(&note.id, "a.png", "", PNG).unwrap();
    seal(&a, &mine.id);
    let theirs = b.create_note("Theirs", "").unwrap();
    let never_downloaded = b.add_note_attachment(&theirs.id, "b.png", "", PNG).unwrap();
    seal(&b, &never_downloaded.id);
    sync_into(&a, &b);
    assert!(a.note_attachment(&never_downloaded.id).unwrap().is_some());

    a.leave_sync_space().unwrap();
    assert!(a.note_attachment(&never_downloaded.id).unwrap().is_none());
    assert!(a.note_attachment(&mine.id).unwrap().unwrap().available);
    assert_eq!(a.note_attachments_to_seal().unwrap(), [mine.id]);
    assert_eq!(count(&a, "SELECT COUNT(*) FROM note_attachment_objects"), 0);
}

#[test]
fn a_record_sealed_again_elsewhere_retires_this_devices_copy() {
    let a = synced();
    let note = a.create_note("Plan", "").unwrap();
    let file = a.add_note_attachment(&note.id, "a.png", "", PNG).unwrap();
    seal(&a, &file.id);
    a.test_connection()
        .unwrap()
        .execute("UPDATE sync_deliveries SET state='delivered'", [])
        .unwrap();
    let mut payload = a.note_attachment_sync_payloads().unwrap().remove(0).1;
    payload["sealedBy"] = Value::String("cd".repeat(16));
    payload["index"] = Value::String("elsewhere".into());
    a.upsert_synced_note_attachment(&file.id, &payload).unwrap();
    assert_eq!(count(&a, "SELECT COUNT(*) FROM note_attachment_objects"), 0);
    assert_eq!(count(&a, "SELECT COUNT(*) FROM sync_retired_objects"), 2);
    // The same file stays; nothing needs downloading.
    assert!(a.note_attachment(&file.id).unwrap().unwrap().available);
    assert!(a.note_attachments_to_fetch().unwrap().is_empty());

    payload["id"] = Value::String("ef".repeat(16));
    assert!(a.upsert_synced_note_attachment(&file.id, &payload).is_err());
}

#[test]
fn the_same_file_sealed_on_two_devices_settles_without_asking() {
    let (a, b) = (synced(), synced());
    let id = "0123456789abcdef0123456789abcdef";
    for db in [&a, &b] {
        let note = Note {
            id: "import-ffffffffffffffffffffffffffffffff".into(),
            title: "Plan".into(),
            authored_title: None,
            body: format!("![](attachment:{id})"),
            daily_date: None,
            created_at: "2026-09-01T10:00:00Z".into(),
            updated_at: "2026-09-01T10:00:00Z".into(),
            trashed: false,
            trashed_at: None,
        };
        db.import_note(
            note,
            &[(id.into(), "a.png".into(), String::new(), PNG.to_vec())],
        )
        .unwrap();
    }
    seal_by(&a, id, &"aa".repeat(16));
    seal_by(&b, id, &"bb".repeat(16));
    sync_into(&b, &a);
    sync_into(&a, &b);
    sync_into(&b, &a);
    for db in [&a, &b] {
        let all = db.list_frontier_conflicts().unwrap();
        assert!(all.is_empty(), "{all:?}");
        assert!(db.note_attachment(id).unwrap().unwrap().available);
    }
    // Both settle on one sealed copy; the other device's own is retired.
    let winner = |db: &Database| -> String {
        db.test_connection()
            .unwrap()
            .query_row(
                "SELECT sealed_by FROM note_attachments WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(winner(&a), winner(&b));
    let kept = count(&a, "SELECT COUNT(*) FROM note_attachment_objects")
        + count(&b, "SELECT COUNT(*) FROM note_attachment_objects");
    assert_eq!(kept, 2, "only the winning device keeps its objects");
}
