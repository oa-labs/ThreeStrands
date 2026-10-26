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
                MimeHeader { name: "Authentication-Results".into(), value: "mx.example; dkim=pass header.d=launch.example; spf=pass smtp.mailfrom=bounce@launch.example; dmarc=pass header.from=launch.example".into() },
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
    assert_eq!(verdicts.spf_domains, vec!["launch.example".to_string()]);
    assert_eq!(trust.reply_to.as_deref(), Some("desk@elsewhere.example"));

    assert!(database.message_trust("bare").unwrap().is_none());
    assert!(database.message_trust("missing").unwrap().is_none());
}

fn delivery_message(id: &str, sender: &str, reply_to: &str, dmarc: &str) -> NormalizedMessage {
    let domain = sender.rsplit_once('@').unwrap().1;
    let raw = RawMessage {
        id: id.into(), thread_id: id.into(), label_ids: vec![], snippet: String::new(), internal_date: String::new(),
        payload: MimePart { headers: vec![
            MimeHeader { name: "From".into(), value: sender.into() },
            MimeHeader { name: "Reply-To".into(), value: reply_to.into() },
            MimeHeader { name: "Authentication-Results".into(), value: format!("mx.example; dmarc={dmarc} header.from={domain}; dkim=pass header.d={domain}") },
        ], ..MimePart::default() },
    };
    let mut item = from(id, id, "2026-10-01T00:00:00Z", sender, &["INBOX"]);
    item.metadata_json = serde_json::to_string(&raw).unwrap();
    item
}

#[test]
fn sender_feedback_is_explicit_scoped_to_exact_requester_and_account_and_reversible() {
    let database = database();
    let sender = "requests@signing.example";
    let requester = "ana@partner.example";
    database.upsert_thread(ACCOUNT, &[delivery_message("original", sender, requester, "pass")]).unwrap();
    database.upsert_thread(ACCOUNT, &[delivery_message("next", sender, requester, "pass")]).unwrap();
    assert!(database.sender_familiarity("original", sender, "").unwrap().delivery_patterns.is_empty());

    database.confirm_sender_delivery("original", &["https://signing.example".into(), "https://help.example".into(), "https://signing.example/".into()]).unwrap();
    database.confirm_sender_delivery("original", &["https://help.example".into(), "https://signing.example".into()]).unwrap();
    let patterns = database.sender_familiarity("next", sender, "").unwrap().delivery_patterns;
    assert_eq!(patterns.len(), 1, "duplicate confirmations must be idempotent");
    let pattern = &patterns[0];
    assert_eq!(pattern.sender_email, sender);
    assert_eq!(pattern.reply_to.as_deref(), Some(requester));
    assert_eq!(pattern.link_origins, ["https://help.example", "https://signing.example"]);
    assert!(pattern.authenticated);
    assert_eq!(pattern.source_message_id, "original");

    // Same Reply-To, different delivery service: return the prior evidence
    // so the UI can call out a changed pattern, never copy trust to it.
    database.upsert_thread(ACCOUNT, &[delivery_message("changed", "requests@new-service.example", requester, "pass")]).unwrap();
    assert_eq!(database.sender_familiarity("changed", "requests@new-service.example", "").unwrap().delivery_patterns.len(), 1);
    database.upsert_thread(ACCOUNT, &[delivery_message("another-person", sender, "bob@partner.example", "pass")]).unwrap();
    assert!(database.sender_familiarity("another-person", sender, "").unwrap().delivery_patterns.is_empty());
    database.upsert_thread("other@example.com", &[delivery_message("other-account", sender, requester, "pass")]).unwrap();
    assert!(database.sender_familiarity("other-account", sender, "").unwrap().delivery_patterns.is_empty());
    database.forget_sender_delivery(&pattern.id).unwrap();
    database.forget_sender_delivery(&pattern.id).unwrap();
    assert!(database.sender_familiarity("next", sender, "").unwrap().delivery_patterns.is_empty());
}

#[test]
fn sender_feedback_survives_reopening_and_cached_source_removal() {
    let temp = crate::db::test_support::TempDbPath::new();
    {
        let database = crate::db::Database::open(&temp.path).unwrap();
        database.upsert_thread(ACCOUNT, &[delivery_message("confirmed", "notifications@portal.example", "person@customer.example", "pass")]).unwrap();
        database.confirm_sender_delivery("confirmed", &["https://portal.example".into()]).unwrap();
        database.connection().unwrap().execute("DELETE FROM threads WHERE id = ?1", [local("confirmed")]).unwrap();
    }
    let database = crate::db::Database::open(&temp.path).unwrap();
    database.upsert_thread(ACCOUNT, &[delivery_message("future", "notifications@portal.example", "person@customer.example", "pass")]).unwrap();
    let patterns = database.sender_familiarity("future", "notifications@portal.example", "").unwrap().delivery_patterns;
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].source_message_id, "confirmed");
}

#[test]
fn sender_feedback_without_authentication_never_teaches_a_reusable_pattern() {
    let database = database();
    database.upsert_thread(ACCOUNT, &[delivery_message("unverified", "requests@service.example", "ana@partner.example", "fail")]).unwrap();
    database.confirm_sender_delivery("unverified", &[]).unwrap();
    let patterns = database.sender_familiarity("unverified", "requests@service.example", "").unwrap().delivery_patterns;
    assert_eq!(patterns.len(), 1);
    assert!(!patterns[0].authenticated);
}

#[test]
fn sender_feedback_requires_cached_headers_and_never_stores_document_urls_or_tokens() {
    let database = database();
    assert!(database.confirm_sender_delivery("missing", &[]).is_err());
    database.upsert_thread(ACCOUNT, &[from("bare-feedback", "bare-feedback", "2026-10-01T00:00:00Z", "requests@service.example", &["INBOX"])]).unwrap();
    assert!(database.confirm_sender_delivery("bare-feedback", &[]).is_err());
    database.upsert_thread(ACCOUNT, &[delivery_message("valid-feedback", "requests@service.example", "ana@partner.example", "pass")]).unwrap();
    for value in ["not-a-url", "javascript:alert(1)", "https://service.example/document/secret", "https://service.example?token=secret", "https://service.example/#secret", "https://user:secret@service.example/"] {
        assert!(database.confirm_sender_delivery("valid-feedback", &[value.into()]).is_err(), "{value}");
    }
    assert!(database.sender_familiarity("valid-feedback", "requests@service.example", "").unwrap().delivery_patterns.is_empty());
}

fn save_contact(database: &crate::db::Database, name: &str, addresses: &[&str]) {
    database
        .save_contact_profile(&crate::models::SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some(name.into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: Vec::new(),
            photo_data: None,
            favorite: false,
            addresses: addresses.iter().map(|address| (*address).into()).collect(),
        })
        .unwrap();
}

#[test]
fn sender_familiarity_reads_what_the_user_sent_received_and_saved() {
    let database = database();
    // The user wrote to Ana twice, and Ana wrote before the newest message.
    let mut chat = vec![
        from("a1", "chat", "2026-08-01T10:00:00Z", "Ana Ruiz <Ana@Studio.example>", &["INBOX"]),
        from("a2", "chat", "2026-09-03T10:00:00Z", "Ana Ruiz <ana@studio.example>", &["INBOX"]),
    ];
    for (id, date) in [("s1", "2026-08-02T10:00:00Z"), ("s2", "2026-09-02T10:00:00Z")] {
        let mut sent = message(id, "chat", date, "Thanks");
        sent.from = ACCOUNT.into();
        sent.to = vec!["Ana Ruiz <ana@studio.example>".into()];
        chat.push(sent);
    }
    database.upsert_thread(ACCOUNT, &chat).unwrap();
    save_contact(&database, "Jane Rivera", &["jane@partner.example"]);

    let ana = database.sender_familiarity("a2", " ANA@studio.example ", "Ana Ruiz").unwrap();
    assert_eq!(ana.sent_count, 2);
    assert_eq!(ana.first_sent_at.as_deref(), Some("2026-08-02T10:00:00Z"));
    assert!(ana.received_before);
    assert_eq!(ana.contact_name, None);
    assert!(ana.name_matches.is_empty(), "the sender's own address is not another address");
    assert_eq!(ana.known_domains, vec!["partner.example".to_string(), "studio.example".to_string()]);

    // Ana's first message had nothing before it.
    assert!(!database.sender_familiarity("a1", "ana@studio.example", "Ana Ruiz").unwrap().received_before);

    let jane = database.sender_familiarity("missing", "jane@partner.example", "Jane Rivera").unwrap();
    assert_eq!(jane.contact_name.as_deref(), Some("Jane Rivera"));
    assert_eq!(jane.sent_count, 0);
}

#[test]
fn sender_familiarity_lists_known_addresses_that_share_the_display_name() {
    let database = database();
    let mut sent = message("s1", "chat", "2026-08-02T10:00:00Z", "Thanks");
    sent.from = ACCOUNT.into();
    sent.to = vec!["Ana Ruiz <ana@studio.example>".into()];
    database.upsert_thread(ACCOUNT, &[sent]).unwrap();
    save_contact(&database, "Jane Rivera", &["jane@partner.example", "j.rivera@partner.example"]);
    // Only received mail does not make an address known.
    database.upsert_thread(ACCOUNT, &[from("x1", "spam", "2026-08-05T10:00:00Z", "Jane Rivera <jane.rivera@freemail.example>", &["INBOX"])]).unwrap();

    let impostor = database.sender_familiarity("x2", "jane.rivera.office@freemail.example", "  \"jane  RIVERA\" ").unwrap();
    assert_eq!(impostor.name_matches, vec!["j.rivera@partner.example".to_string(), "jane@partner.example".to_string()]);
    assert_eq!(impostor.sent_count, 0);
    assert_eq!(impostor.contact_name, None);
    assert_eq!(database.sender_familiarity("x2", "ana.r@freemail.example", "Ana Ruiz").unwrap().name_matches, vec!["ana@studio.example".to_string()]);
    // One-word or very short names are too common to compare.
    assert!(database.sender_familiarity("x2", "x@freemail.example", "Jane").unwrap().name_matches.is_empty());
    assert!(database.sender_familiarity("x2", "x@freemail.example", "A Ru").unwrap().name_matches.is_empty());
}
