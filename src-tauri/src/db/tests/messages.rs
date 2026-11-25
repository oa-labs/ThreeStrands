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

#[test]
fn a_sent_message_is_found_on_this_device_only_for_its_own_account() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message(
                "sent-message",
                "sent-thread",
                "2026-01-01T00:00:00Z",
                "body",
            )],
        )
        .unwrap();
    assert_eq!(
        database
            .thread_for_account_message("work@example.com", "sent-message")
            .unwrap()
            .as_deref(),
        Some("work@example.com:sent-thread")
    );
    assert_eq!(
        database
            .thread_for_account_message("other@example.com", "sent-message")
            .unwrap(),
        None
    );
    assert_eq!(
        database
            .thread_for_account_message("work@example.com", "not-synced-yet")
            .unwrap(),
        None
    );
}

mod completed_message_lookup {
    use super::*;
    use crate::models::CompletedByEmail;

    const SENT_AT: &str = "2026-01-01T12:00:00Z";
    fn sent_at_ms() -> i64 {
        chrono::DateTime::parse_from_rfc3339(SENT_AT)
            .unwrap()
            .timestamp_millis()
    }
    fn record(message_id: Option<&str>) -> CompletedByEmail {
        CompletedByEmail {
            account_id: "work@example.com".into(),
            subject: "Contract".into(),
            sent_at: sent_at_ms(),
            message_id: message_id.map(Into::into),
            thread_id: None,
            recipients: vec!["ann@example.org".into()],
        }
    }
    /// A synced message with its own id, sent from `from` to Ann.
    fn synced(db: &crate::db::Database, id: &str, thread: &str, from: &str, date: &str) {
        let mut m = message(id, thread, date, "body");
        m.subject = "Contract".into();
        m.from = from.into();
        m.to = vec!["Ann <ann@example.org>".into()];
        db.upsert_thread("work@example.com", &[m]).unwrap();
    }
    fn found(db: &crate::db::Database, record: &CompletedByEmail) -> Option<String> {
        db.find_completed_message(record)
            .unwrap()
            .map(|l| l.message_id)
    }

    #[test]
    fn the_recorded_id_wins() {
        let db = database();
        synced(&db, "receipt-id", "t1", "work@example.com", SENT_AT);
        synced(&db, "other", "t2", "work@example.com", SENT_AT);
        let location = db
            .find_completed_message(&record(Some("receipt-id")))
            .unwrap()
            .unwrap();
        assert_eq!(location.message_id, "receipt-id");
        assert_eq!(location.thread_id, "work@example.com:t1");
    }

    #[test]
    fn an_id_miss_falls_back_to_the_one_message_the_account_sent_to_those_recipients() {
        let db = database();
        synced(
            &db,
            "server-assigned",
            "t1",
            "work@example.com",
            "2026-01-01T12:00:20Z",
        );
        assert_eq!(
            found(&db, &record(Some("receipt-id"))).as_deref(),
            Some("server-assigned")
        );
        assert_eq!(
            found(&db, &record(None)).as_deref(),
            Some("server-assigned")
        );
    }

    #[test]
    fn two_candidates_are_no_match() {
        let db = database();
        synced(&db, "a", "t1", "work@example.com", SENT_AT);
        synced(&db, "b", "t2", "work@example.com", "2026-01-01T12:01:00Z");
        assert_eq!(found(&db, &record(Some("receipt-id"))), None);
    }

    #[test]
    fn a_message_outside_the_window_is_no_match() {
        let db = database();
        // Beyond the clock skew before, and beyond the window after.
        synced(
            &db,
            "early",
            "t1",
            "work@example.com",
            "2026-01-01T11:58:00Z",
        );
        synced(
            &db,
            "late",
            "t2",
            "work@example.com",
            "2026-01-01T12:20:00Z",
        );
        assert_eq!(found(&db, &record(None)), None);
        // Just inside either edge matches.
        let db = database();
        synced(
            &db,
            "edge",
            "t1",
            "work@example.com",
            "2026-01-01T11:59:30Z",
        );
        assert_eq!(found(&db, &record(None)).as_deref(), Some("edge"));
    }

    #[test]
    fn a_message_the_account_did_not_send_or_to_others_is_no_match() {
        let db = database();
        synced(&db, "received", "t1", "ann@example.org", SENT_AT);
        assert_eq!(found(&db, &record(None)), None);
        let mut other = record(None);
        other.recipients = vec!["bob@example.org".into()];
        synced(&db, "mine", "t2", "work@example.com", SENT_AT);
        assert_eq!(found(&db, &other), None);
        let mut subject = record(None);
        subject.subject = "Something else".into();
        assert_eq!(found(&db, &subject), None);
        assert_eq!(found(&db, &record(None)).as_deref(), Some("mine"));
    }

    #[test]
    fn a_reply_matches_only_inside_its_conversation() {
        let db = database();
        synced(&db, "in-thread", "t1", "work@example.com", SENT_AT);
        synced(&db, "elsewhere", "t2", "work@example.com", SENT_AT);
        assert_eq!(
            found(&db, &record(None)),
            None,
            "two candidates without a conversation"
        );
        let mut reply = record(None);
        reply.thread_id = Some("t1".into());
        assert_eq!(found(&db, &reply).as_deref(), Some("in-thread"));
        reply.thread_id = Some("t3".into());
        assert_eq!(found(&db, &reply), None);
    }
}
