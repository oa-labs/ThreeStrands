use crate::db::test_support::{database, message};
use crate::mime::{MimeHeader, MimePart, NormalizedMessage, RawMessage};

const ACCOUNT: &str = "you@example.com";

/// The local id of a conversation stored under `ACCOUNT`.
fn local(thread: &str) -> String {
    format!("{ACCOUNT}:{thread}")
}

fn from(id: &str, thread: &str, date: &str, sender: &str, labels: &[&str]) -> NormalizedMessage {
    let mut item = message(id, thread, date, "body");
    item.from = sender.into();
    item.to = vec![ACCOUNT.into()];
    item.labels = labels.iter().map(|label| (*label).into()).collect();
    item
}

#[test]
fn sender_history_counts_automated_mail_the_contact_index_leaves_out() {
    let database = database();
    let mut newest = from("n3", "news-3", "2026-10-07T09:00:00Z", "Launch News <News@Launch.example>", &["INBOX", "UNREAD"]);
    // Bulk mail: the contact index skips it, the sender history must not.
    newest.unsubscribe = Some(crate::mime::UnsubscribeMetadata { one_click_url: None, mailto_url: Some("mailto:leave@launch.example".into()), web_url: None, list_id: None });
    database.upsert_thread(ACCOUNT, &[newest]).unwrap();
    database.upsert_thread(ACCOUNT, &[from("n2", "news-2", "2026-09-30T09:00:00Z", "news@launch.example", &["INBOX"])]).unwrap();
    database.upsert_thread(ACCOUNT, &[from("n1", "news-1", "2026-09-23T09:00:00Z", "\"Launch\" <news@launch.example>", &[])]).unwrap();
    database.upsert_thread(ACCOUNT, &[from("n0", "news-0", "2026-09-16T09:00:00Z", "news@launch.example", &["TRASH"])]).unwrap();
    // A look-alike address that merely contains the sender's.
    database.upsert_thread(ACCOUNT, &[from("x", "other", "2026-10-08T09:00:00Z", "breaking-news@launch.example", &["INBOX"])]).unwrap();
    // The same sender in another account stays out.
    database.upsert_thread("work@example.org", &[from("w", "work", "2026-10-09T09:00:00Z", "news@launch.example", &["INBOX"])]).unwrap();

    let history = database.sender_history(ACCOUNT, " News@Launch.example ", 10).unwrap();
    assert_eq!(history.received_count, 4);
    assert_eq!(history.read_count, 3);
    assert_eq!(history.first_at.as_deref(), Some("2026-09-16T09:00:00Z"));
    assert_eq!(history.recent_received_at.first().map(String::as_str), Some("2026-10-07T09:00:00Z"));
    assert_eq!(history.inbox_thread_ids, vec![local("news-3"), local("news-2")]);
    assert_eq!(
        history.threads.iter().map(|item| item.thread_id.clone()).collect::<Vec<_>>(),
        vec![local("news-3"), local("news-2"), local("news-1")],
        "trashed conversations are not listed",
    );
    assert_eq!(history.last_sent_at, None);
}

#[test]
fn sender_history_reports_the_users_latest_message_and_lists_each_conversation_once() {
    let database = database();
    let person = "Ana Ruiz <ana@studio.example>";
    database
        .upsert_thread(ACCOUNT, &[
            from("a1", "chat", "2026-08-01T10:00:00Z", person, &["INBOX"]),
            from("a2", "chat", "2026-08-03T10:00:00Z", person, &["INBOX"]),
        ])
        .unwrap();
    let mut reply = message("r1", "chat-reply", "2026-08-02T10:00:00Z", "Thanks");
    reply.from = ACCOUNT.into();
    reply.to = vec![person.into()];
    database.upsert_thread(ACCOUNT, &[reply]).unwrap();

    let history = database.sender_history(ACCOUNT, "ana@studio.example", 1).unwrap();
    assert_eq!(history.received_count, 2);
    assert_eq!(history.threads.len(), 1);
    assert_eq!(history.threads[0].thread_id, local("chat"));
    assert_eq!(history.threads[0].sent_at, "2026-08-03T10:00:00Z");
    assert_eq!(history.inbox_thread_ids, vec![local("chat")]);
    assert_eq!(history.last_sent_at.as_deref(), Some("2026-08-02T10:00:00Z"));
}

#[test]
fn sender_history_of_an_unknown_or_malformed_address_is_empty() {
    let database = database();
    for email in ["nobody@nowhere.example", "", "not-an-address"] {
        let history = database.sender_history(ACCOUNT, email, 10).unwrap();
        assert_eq!(history.received_count, 0);
        assert!(history.threads.is_empty());
        assert!(history.inbox_thread_ids.is_empty());
    }
}

#[test]
fn message_trust_reads_the_stored_headers_and_is_absent_without_them() {
    let database = database();
    let raw = RawMessage {
        id: "signed".into(),
        thread_id: "signed-thread".into(),
        label_ids: vec!["INBOX".into()],
        snippet: String::new(),
        internal_date: String::new(),
        payload: MimePart {
            headers: vec![
                MimeHeader { name: "Authentication-Results".into(), value: "mx.example; dkim=pass header.d=launch.example; spf=pass; dmarc=pass header.from=launch.example".into() },
                MimeHeader { name: "From".into(), value: "news@launch.example".into() },
                MimeHeader { name: "Reply-To".into(), value: "desk@elsewhere.example".into() },
            ],
            ..MimePart::default()
        },
    };
    let mut signed = from("signed", "signed-thread", "2026-10-01T00:00:00Z", "news@launch.example", &["INBOX"]);
    signed.metadata_json = serde_json::to_string(&raw).unwrap();
    // The fixture default stores `{}`, which is not a provider payload.
    let bare = from("bare", "bare-thread", "2026-10-01T00:00:00Z", "news@launch.example", &["INBOX"]);
    database.upsert_thread(ACCOUNT, &[signed]).unwrap();
    database.upsert_thread(ACCOUNT, &[bare]).unwrap();

    let trust = database.message_trust("signed").unwrap().unwrap();
    assert_eq!(trust.from_domain, "launch.example");
    let verdicts = trust.authentication.unwrap();
    assert_eq!(verdicts.dmarc.as_deref(), Some("pass"));
    assert_eq!(verdicts.dkim_domains, vec!["launch.example".to_string()]);
    assert_eq!(trust.reply_to.as_deref(), Some("desk@elsewhere.example"));

    assert!(database.message_trust("bare").unwrap().is_none());
    assert!(database.message_trust("missing").unwrap().is_none());
}
