use crate::db::test_support::{database, message};
use crate::mime::UnsubscribeMetadata;
use crate::models::UnsubscribeMethod;

#[test]
fn attachment_metadata_sets_thread_flag_and_round_trips_on_message() {
    let database = database();
    let mut normalized = message(
        "attachment-message",
        "attachment-thread",
        "2026-01-01T00:00:00Z",
        "body",
    );
    normalized
        .attachments
        .push(crate::models::MessageAttachment {
            id: "gmail-attachment-id".into(),
            filename: "invoice\u{202e}fdp.ｅｘｅ".into(),
            mime_type: "application/pdf".into(),
            size: 42,
            content_id: None,
            inline: false,
        });
    database
        .upsert_thread("work@example.com", &[normalized])
        .unwrap();

    let detail = database
        .get_thread("work@example.com:attachment-thread")
        .unwrap();
    assert!(detail.thread.has_attachments);
    assert_eq!(
        detail.messages[0].attachments[0].filename,
        "invoice_fdp.exe"
    );
}

#[test]
fn unsubscribe_metadata_is_exposed_and_attempts_are_recorded() {
    let database = database();
    let mut normalized = message(
        "newsletter-message",
        "newsletter",
        "2026-01-01T00:00:00Z",
        "body",
    );
    normalized.unsubscribe = Some(UnsubscribeMetadata {
        one_click_url: Some("https://lists.example/one-click".into()),
        mailto_url: Some("mailto:list@example.com?subject=unsubscribe".into()),
        web_url: Some("https://lists.example/preferences".into()),
        list_id: Some("news.example".into()),
    });
    database
        .upsert_thread("work@example.com", &[normalized])
        .unwrap();

    let detail = database.get_thread("work@example.com:newsletter").unwrap();
    let info = detail.messages[0].unsubscribe.as_ref().unwrap();
    assert_eq!(info.methods.len(), 3);
    assert_eq!(info.list_id.as_deref(), Some("news.example"));

    let target = database.begin_unsubscribe("newsletter-message").unwrap();
    assert!(matches!(target.method, UnsubscribeMethod::OneClick));
    assert_eq!(target.url, "https://lists.example/one-click");
    let pending: String = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT state FROM unsubscribe_requests WHERE id = ?1",
            [&target.request_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, "pending");

    database
        .finish_unsubscribe(&target.request_id, "succeeded", Some(204), None)
        .unwrap();
    let completed: (String, Option<i64>) = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT state, http_status FROM unsubscribe_requests WHERE id = ?1",
            [&target.request_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(completed, ("succeeded".into(), Some(204)));
}

#[test]
fn message_bodies_round_trip_through_compression() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message(
                "m1",
                "t1",
                "2026-01-01T00:00:00Z",
                "Hello, this is the plaintext body ✓",
            )],
        )
        .unwrap();
    let detail = database.get_thread("work@example.com:t1").unwrap();
    assert_eq!(
        detail.messages[0].body_text,
        "Hello, this is the plaintext body ✓"
    );
    // Stored compressed, not as plaintext, in the legacy columns.
    let (body_html, body_text): (String, String) = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT body_html, body_text FROM messages WHERE id = 'm1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(body_html, "");
    assert_eq!(body_text, "");
}

#[test]
fn legacy_uncompressed_bodies_still_read_back_correctly() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2026-01-01T00:00:00Z", "placeholder")],
        )
        .unwrap();
    // Simulate a row written before the compression migration: only the
    // legacy plaintext columns are populated.
    database
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET body_html = 'Legacy <b>html</b>', body_text = 'Legacy text',
                body_html_z = NULL, body_text_z = NULL
             WHERE id = 'm1'",
            [],
        )
        .unwrap();
    let detail = database.get_thread("work@example.com:t1").unwrap();
    assert_eq!(detail.messages[0].body_html, "Legacy <b>html</b>");
    assert_eq!(detail.messages[0].body_text, "Legacy text");
}

#[test]
fn message_metadata_is_stored_compressed_and_round_trips() {
    let database = database();
    let payload = format!("{{\"id\":\"m1\",\"pad\":\"{}\"}}", "x".repeat(10_000));
    database.put_message_metadata("m1", &payload).unwrap();
    let (legacy, compressed): (String, Option<Vec<u8>>) = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT payload, payload_z FROM message_metadata WHERE id='m1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(legacy, "");
    assert!(compressed.unwrap().len() < payload.len() / 10);
    assert_eq!(
        database.message_metadata("m1").unwrap().as_deref(),
        Some(payload.as_str())
    );
    assert_eq!(database.message_metadata("missing").unwrap(), None);
}

#[test]
fn synced_messages_store_compressed_metadata_that_attachment_lookup_reads() {
    let database = database();
    let mut synced = message("m1", "t1", "2026-01-01T00:00:00Z", "body");
    synced.metadata_json =
        r#"{"id":"m1","threadId":"t1","payload":{"mimeType":"text/plain"}}"#.into();
    database
        .upsert_thread("work@example.com", &[synced])
        .unwrap();
    let stored_plaintext: String = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT payload FROM message_metadata WHERE id='m1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_plaintext, "");
    let (account, raw) = database.attachment_message("m1").unwrap();
    assert_eq!(account, "work@example.com");
    assert_eq!(raw.id, "m1");
}
