use crate::db::test_support::{clear_seed_threads, database, message};
use crate::mime::{NormalizedMessage, UnsubscribeMetadata};

const ME: &str = "me@example.com";

fn unread(id: &str, thread_id: &str, date: &str, from: &str, to: &[&str]) -> NormalizedMessage {
    let mut message = message(id, thread_id, date, "Body");
    message.from = from.into();
    message.to = to.iter().map(|address| address.to_string()).collect();
    message.labels = vec!["INBOX".into(), "UNREAD".into()];
    message
}

#[test]
fn today_mail_lists_only_unread_conversations_newest_first_with_a_total() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            ME,
            &[unread(
                "a",
                "older",
                "2026-10-20T09:00:00Z",
                "Ann <ann@example.org>",
                &[ME],
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            ME,
            &[unread(
                "b",
                "newer",
                "2026-10-22T09:00:00Z",
                "Bo <bo@example.org>",
                &[ME],
            )],
        )
        .unwrap();
    let mut read = unread(
        "c",
        "read",
        "2026-10-23T09:00:00Z",
        "Cy <cy@example.org>",
        &[ME],
    );
    read.labels = vec!["INBOX".into()];
    database.upsert_thread(ME, &[read]).unwrap();
    let mut archived = unread(
        "d",
        "archived",
        "2026-10-23T10:00:00Z",
        "Di <di@example.org>",
        &[ME],
    );
    archived.labels = vec!["UNREAD".into()];
    database.upsert_thread(ME, &[archived]).unwrap();

    let page = database.list_today_mail(None).unwrap();
    let ids: Vec<&str> = page
        .threads
        .iter()
        .map(|item| item.thread.id.as_str())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(page.total, 2);
    assert!(page.threads[0].thread.last_received_at > page.threads[1].thread.last_received_at);
    assert_eq!(
        database
            .list_today_mail(Some("other@example.com"))
            .unwrap()
            .total,
        0
    );
}

#[test]
fn today_mail_reads_signals_from_the_newest_message_someone_else_sent() {
    let database = database();
    clear_seed_threads(&database);
    // The user replied last and marked it unread; the signals still come from Ann's message.
    let mut reply = unread(
        "a2",
        "replied",
        "2026-10-22T10:00:00Z",
        ME,
        &["ann@example.org"],
    );
    reply.labels = vec!["INBOX".into(), "SENT".into(), "UNREAD".into()];
    database
        .upsert_thread(
            ME,
            &[
                unread(
                    "a1",
                    "replied",
                    "2026-10-22T09:00:00Z",
                    "Ann <ann@example.org>",
                    &["Me <ME@example.com>"],
                ),
                reply,
            ],
        )
        .unwrap();
    let mut list = unread(
        "n1",
        "newsletter",
        "2026-10-22T08:00:00Z",
        "News <news@example.net>",
        &["list@example.net"],
    );
    list.unsubscribe = Some(UnsubscribeMetadata {
        one_click_url: Some("https://example.net/u".into()),
        mailto_url: None,
        web_url: None,
        list_id: None,
    });
    database.upsert_thread(ME, &[list]).unwrap();
    database
        .upsert_thread(
            ME,
            &[unread(
                "s1",
                "stranger",
                "2026-10-22T07:00:00Z",
                "Sam <sam@example.org>",
                &[ME],
            )],
        )
        .unwrap();

    let page = database.list_today_mail(Some(ME)).unwrap();
    let find = |id: &str| {
        page.threads
            .iter()
            .find(|item| item.thread.provider_thread_id == id)
            .unwrap()
    };

    let replied = find("replied");
    assert_eq!(replied.sender.as_deref(), Some("Ann <ann@example.org>"));
    assert!(
        replied.addressed_to_me,
        "own address matches regardless of case or display name"
    );
    assert!(replied.corresponded);
    assert!(!replied.list_unsubscribe);

    let newsletter = find("newsletter");
    assert!(newsletter.list_unsubscribe);
    assert!(!newsletter.addressed_to_me);
    assert!(!newsletter.corresponded);

    let stranger = find("stranger");
    assert!(stranger.addressed_to_me);
    assert!(!stranger.corresponded);
}

#[test]
fn today_mail_counts_mail_the_user_sent_the_sender_elsewhere_as_corresponded() {
    let database = database();
    clear_seed_threads(&database);
    let mut sent = message("o1", "outbound", "2026-10-01T09:00:00Z", "Hi");
    sent.from = ME.into();
    sent.to = vec!["Pat <pat@example.org>".into()];
    sent.labels = vec!["SENT".into()];
    database.upsert_thread(ME, &[sent]).unwrap();
    database
        .upsert_thread(
            ME,
            &[unread(
                "p1",
                "fresh",
                "2026-10-22T09:00:00Z",
                "Pat <PAT@example.org>",
                &[ME],
            )],
        )
        .unwrap();

    let page = database.list_today_mail(None).unwrap();
    assert_eq!(page.threads.len(), 1);
    assert!(page.threads[0].corresponded);
}

fn attachment(filename: &str, mime_type: &str) -> crate::models::MessageAttachment {
    crate::models::MessageAttachment {
        id: filename.into(),
        filename: filename.into(),
        mime_type: mime_type.into(),
        size: 10,
        content_id: None,
        inline: false,
    }
}

#[test]
fn today_mail_reports_invitations_and_the_size_of_the_to_line() {
    let database = database();
    clear_seed_threads(&database);
    // A request sent as a calendar part with parameters, and one sent as a plain .ics file.
    let mut part = unread(
        "i1",
        "part",
        "2026-10-22T09:00:00Z",
        "Meredith <m@example.org>",
        &[ME],
    );
    part.attachments = vec![attachment(
        "invite",
        "text/calendar; method=REQUEST; charset=UTF-8",
    )];
    database.upsert_thread(ME, &[part]).unwrap();
    let mut file = unread(
        "i2",
        "file",
        "2026-10-22T08:00:00Z",
        "Organizer <o@example.net>",
        &[ME, "a@example.org"],
    );
    file.attachments = vec![attachment("Meeting.ICS", "application/octet-stream")];
    database.upsert_thread(ME, &[file]).unwrap();
    let mut pdf = unread(
        "p1",
        "pdf",
        "2026-10-22T07:00:00Z",
        "Pat <p@example.org>",
        &[ME, "a@example.org", "b@example.org", "c@example.org"],
    );
    pdf.attachments = vec![attachment("agenda.pdf", "application/pdf")];
    database.upsert_thread(ME, &[pdf]).unwrap();

    let page = database.list_today_mail(Some(ME)).unwrap();
    let find = |id: &str| {
        page.threads
            .iter()
            .find(|item| item.thread.provider_thread_id == id)
            .unwrap()
    };
    assert!(find("part").has_invitation);
    assert!(find("file").has_invitation);
    assert!(!find("pdf").has_invitation);
    assert_eq!(find("part").to_count, 1);
    assert_eq!(find("file").to_count, 2);
    assert_eq!(find("pdf").to_count, 4);
}

fn sent(id: &str, thread_id: &str, date: &str, to: &[&str], body: &str) -> NormalizedMessage {
    let mut message = message(id, thread_id, date, body);
    message.from = format!("Me <{ME}>");
    message.to = to.iter().map(|address| address.to_string()).collect();
    message.labels = vec!["SENT".into()];
    message
}

const SINCE: &str = "2026-10-12T00:00:00Z";

fn promise_threads(database: &crate::db::Database) -> Vec<String> {
    database
        .list_open_promises(Some(ME), SINCE)
        .unwrap()
        .into_iter()
        .map(|promise| promise.thread.provider_thread_id)
        .collect()
}

#[test]
fn open_promises_quote_the_newest_sent_message_and_its_recipients() {
    let database = database();
    clear_seed_threads(&database);
    database.upsert_thread(ME, &[
        unread("j1", "jill", "2026-10-22T09:00:00Z", "Jill <jill@example.org>", &[ME]),
        sent("j2", "jill", "2026-10-22T10:00:00Z", &["Jill <jill@example.org>", ME],
            "Sure thing. I'll reconnect your server, run the report and send it over.\n\nOn Oct 22, Jill wrote:\n> I'll send the invoices."),
    ]).unwrap();

    let promises = database.list_open_promises(Some(ME), SINCE).unwrap();
    assert_eq!(promises.len(), 1);
    assert_eq!(
        promises[0].excerpt,
        "I'll reconnect your server, run the report and send it over"
    );
    assert_eq!(
        promises[0].recipients,
        vec!["Jill <jill@example.org>".to_string()],
        "the user's own address is left off"
    );
    assert_eq!(promises[0].message_id, "j2");
    assert!(database
        .list_open_promises(Some("other@example.com"), SINCE)
        .unwrap()
        .is_empty());
}

#[test]
fn open_promises_read_only_the_users_own_text() {
    let database = database();
    clear_seed_threads(&database);
    // The promise is only in the quoted message under the user's reply.
    database
        .upsert_thread(
            ME,
            &[sent(
                "q1",
                "quoted",
                "2026-10-22T10:00:00Z",
                &["ann@example.org"],
                "Thanks, that works.\n\nOn Oct 21, Ann wrote:\n> I'll send the contract over.",
            )],
        )
        .unwrap();
    // Someone else made the promise.
    database
        .upsert_thread(
            ME,
            &[unread(
                "o1",
                "theirs",
                "2026-10-22T10:00:00Z",
                "Bo <bo@example.org>",
                &[ME],
            )],
        )
        .unwrap();
    assert!(promise_threads(&database).is_empty());
}

#[test]
fn a_later_message_to_the_same_person_keeps_the_promise() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            ME,
            &[sent(
                "p1",
                "promise",
                "2026-10-20T10:00:00Z",
                &["dana@example.org"],
                "I'll send the deck Friday.",
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            ME,
            &[sent(
                "w1",
                "waiting",
                "2026-10-20T10:00:00Z",
                &["eli@example.org"],
                "Let me check with Pete.",
            )],
        )
        .unwrap();
    assert_eq!(promise_threads(&database), vec!["promise", "waiting"]);

    // Sent in another conversation, after the promise.
    database
        .upsert_thread(
            ME,
            &[sent(
                "p2",
                "deck",
                "2026-10-23T10:00:00Z",
                &["Dana <DANA@example.org>"],
                "Here's the deck.",
            )],
        )
        .unwrap();
    assert_eq!(promise_threads(&database), vec!["waiting"]);
}

#[test]
fn open_promises_skip_old_trashed_and_tracked_conversations() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            ME,
            &[sent(
                "old",
                "old",
                "2026-10-01T10:00:00Z",
                &["a@example.org"],
                "I'll send it.",
            )],
        )
        .unwrap();
    let mut trashed = sent(
        "tr",
        "trashed",
        "2026-10-22T10:00:00Z",
        &["b@example.org"],
        "I'll send it.",
    );
    trashed.labels = vec!["SENT".into(), "TRASH".into()];
    database.upsert_thread(ME, &[trashed]).unwrap();
    database
        .upsert_thread(
            ME,
            &[sent(
                "tk",
                "tracked",
                "2026-10-22T10:00:00Z",
                &["c@example.org"],
                "I'll send it.",
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            ME,
            &[sent(
                "no",
                "plain",
                "2026-10-22T10:00:00Z",
                &["d@example.org"],
                "Sounds good, see you then.",
            )],
        )
        .unwrap();
    assert_eq!(promise_threads(&database), vec!["tracked"]);

    let thread_id = database.list_open_promises(Some(ME), SINCE).unwrap()[0]
        .thread
        .id
        .clone();
    let request: crate::models::CreateTaskRequest = serde_json::from_value(serde_json::json!({
        "accountId": ME, "threadId": thread_id, "subjectSnapshot": "Subject", "title": "Send it", "kind": "follow_up",
    })).unwrap();
    database.create_task(&request).unwrap();
    assert!(promise_threads(&database).is_empty());
}

#[test]
fn tracked_promises_stay_hidden_after_reopening_for_every_task_status() {
    use crate::db::test_support::TempDbPath;
    use crate::db::Database;

    for status in ["open", "in_progress", "completed", "cancelled"] {
        let path = TempDbPath::new();
        {
            let database = Database::open(&path.path).unwrap();
            database
                .upsert_thread(
                    ME,
                    &[
                        sent(
                            "older",
                            "tracked",
                            "2026-10-21T10:00:00Z",
                            &["c@example.org"],
                            "I'll send the draft.",
                        ),
                        sent(
                            "newest",
                            "tracked",
                            "2026-10-22T10:00:00Z",
                            &["c@example.org"],
                            "I'll send the final copy.",
                        ),
                    ],
                )
                .unwrap();
            let promise = database
                .list_open_promises(Some(ME), SINCE)
                .unwrap()
                .remove(0);
            let request = serde_json::from_value(serde_json::json!({
                "accountId": ME, "threadId": promise.thread.id,
                "sourceMessageId": promise.message_id, "subjectSnapshot": "Subject",
                "title": promise.excerpt, "kind": "action",
                "evidenceText": promise.excerpt,
            }))
            .unwrap();
            let task = database.create_task(&request).unwrap();
            database.set_task_status(&task.id, status, "user").unwrap();
        }
        let database = Database::open(&path.path).unwrap();
        // Neither the tracked message nor its older promise comes back.
        assert!(promise_threads(&database).is_empty(), "status: {status}");
        if matches!(status, "completed" | "cancelled") {
            // A new promise still appears, even in the same conversation.
            database
                .upsert_thread(
                    ME,
                    &[sent(
                        "later",
                        "tracked",
                        "2026-10-23T10:00:00Z",
                        &["c@example.org"],
                        "I'll send the appendix.",
                    )],
                )
                .unwrap();
            let promises = database.list_open_promises(Some(ME), SINCE).unwrap();
            assert_eq!(promises.len(), 1);
            assert_eq!(promises[0].message_id, "later");
        }
    }
}

#[test]
fn a_reply_does_not_complete_a_task_created_from_a_promise() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            ME,
            &[sent(
                "promise",
                "tracked",
                "2026-10-22T10:00:00Z",
                &["c@example.org"],
                "I'll send the draft.",
            )],
        )
        .unwrap();
    let promise = database
        .list_open_promises(Some(ME), SINCE)
        .unwrap()
        .remove(0);
    let request = serde_json::from_value(serde_json::json!({
        "accountId": ME, "threadId": promise.thread.id,
        "sourceMessageId": promise.message_id, "subjectSnapshot": "Subject",
        "title": promise.excerpt, "kind": "action",
    }))
    .unwrap();
    database.create_task(&request).unwrap();
    database
        .upsert_thread(
            ME,
            &[sent(
                "acknowledgement",
                "tracked",
                "2026-10-22T11:00:00Z",
                &["c@example.org"],
                "I'm going to work on it.",
            )],
        )
        .unwrap();
    assert_eq!(database.reconcile_waiting_tasks().unwrap(), 0);
    assert_eq!(
        database.list_tasks(Some(ME), Some("open")).unwrap().len(),
        1
    );
    database
        .upsert_thread(
            ME,
            &[unread(
                "reply",
                "tracked",
                "2026-10-23T10:00:00Z",
                "c@example.org",
                &[ME],
            )],
        )
        .unwrap();
    assert_eq!(database.reconcile_waiting_tasks().unwrap(), 0);
    assert_eq!(
        database.list_tasks(Some(ME), Some("open")).unwrap().len(),
        1
    );
    assert!(promise_threads(&database).is_empty());
}

const DAY_START: &str = "2026-11-10T00:00:00Z";
const DAY_END: &str = "2026-11-11T00:00:00Z";

fn sent_between(database: &crate::db::Database, account: Option<&str>) -> Vec<String> {
    database
        .list_sent_between(account, DAY_START, DAY_END)
        .unwrap()
        .into_iter()
        .map(|item| item.thread.provider_thread_id)
        .collect()
}

#[test]
fn sent_between_uses_a_start_inclusive_end_exclusive_day() {
    let database = database();
    clear_seed_threads(&database);
    for (id, date) in [
        ("before-start", "2026-11-09T23:59:59Z"),
        ("at-start", "2026-11-10T00:00:00Z"),
        ("before-end", "2026-11-10T23:59:59Z"),
        ("at-end", "2026-11-11T00:00:00Z"),
    ] {
        database
            .upsert_thread(
                ME,
                &[sent(id, id, date, &["Jill <jill@example.org>"], "Hi")],
            )
            .unwrap();
    }
    assert_eq!(
        sent_between(&database, Some(ME)),
        vec!["before-end", "at-start"]
    );
    // Bounds as the app writes them, with fractional seconds, pick the same instants.
    let found = database
        .list_sent_between(
            Some(ME),
            "2026-11-10T00:00:00.000Z",
            "2026-11-11T00:00:00.000Z",
        )
        .unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].last_sent_at, "2026-11-10T23:59:59Z");
}

#[test]
fn sent_between_counts_only_the_users_own_messages_in_one_account() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            ME,
            &[
                unread(
                    "a1",
                    "chat",
                    "2026-11-10T08:00:00Z",
                    "Jill <jill@example.org>",
                    &[ME],
                ),
                sent(
                    "a2",
                    "chat",
                    "2026-11-10T09:00:00Z",
                    &["Jill <jill@example.org>", ME],
                    "One",
                ),
                sent(
                    "a3",
                    "chat",
                    "2026-11-10T10:00:00Z",
                    &["Jill <jill@example.org>", "Bo <bo@example.org>"],
                    "Two",
                ),
                sent(
                    "a4",
                    "chat",
                    "2026-11-12T10:00:00Z",
                    &["Jill <jill@example.org>"],
                    "Later day",
                ),
            ],
        )
        .unwrap();
    // Only received mail on the day, and mail sent from another account.
    database
        .upsert_thread(
            ME,
            &[unread(
                "b1",
                "received",
                "2026-11-10T08:00:00Z",
                "Jill <jill@example.org>",
                &[ME],
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            "other@example.com",
            &[{
                let mut message = sent(
                    "c1",
                    "elsewhere",
                    "2026-11-10T11:00:00Z",
                    &["Jill <jill@example.org>"],
                    "Hi",
                );
                message.from = "Me <other@example.com>".into();
                message
            }],
        )
        .unwrap();

    let found = database
        .list_sent_between(Some(ME), DAY_START, DAY_END)
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].thread.provider_thread_id, "chat");
    assert_eq!(found[0].sent_count, 2);
    assert_eq!(found[0].last_sent_at, "2026-11-10T10:00:00Z");
    assert_eq!(
        found[0].recipients,
        vec![
            "Jill <jill@example.org>".to_string(),
            "Bo <bo@example.org>".to_string()
        ]
    );
    assert_eq!(
        sent_between(&database, Some("other@example.com")),
        vec!["elsewhere"]
    );
    let mut everyone = sent_between(&database, None);
    everyone.sort();
    assert_eq!(everyone, vec!["chat", "elsewhere"]);
}

#[test]
fn sent_between_skips_trashed_conversations() {
    let database = database();
    clear_seed_threads(&database);
    let mut message = sent(
        "t1",
        "gone",
        "2026-11-10T09:00:00Z",
        &["Jill <jill@example.org>"],
        "Hi",
    );
    message.labels = vec!["SENT".into(), "TRASH".into()];
    database.upsert_thread(ME, &[message]).unwrap();
    assert!(sent_between(&database, Some(ME)).is_empty());
}

#[test]
fn commitments_only_surface_unanswered_direct_requests_in_starred_mail() {
    let db = database();
    clear_seed_threads(&db);
    let make = |id, thread, body: &str, from, to: &[&str], date| {
        let mut message = unread(id, thread, date, from, to);
        message.labels.push("STARRED".into());
        message.body_text = body.into();
        message
    };
    let first = "2026-10-23T09:00:00Z";
    let second = "2026-10-23T10:00:00Z";
    db.upsert_thread(
        ME,
        &[make(
            "request",
            "estimate",
            "Could you confirm the estimate?",
            "mira@example.test",
            &[ME],
            first,
        )],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[make(
            "copy",
            "team",
            "Please reply today.",
            "pat@example.test",
            &[ME, "other@example.test"],
            first,
        )],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[
            make(
                "before",
                "answered",
                "Please confirm the deadline.",
                "pat@example.test",
                &[ME],
                first,
            ),
            make(
                "answer",
                "answered",
                "Yes, Friday works.",
                ME,
                &["pat@example.test"],
                second,
            ),
        ],
    )
    .unwrap();
    db.upsert_thread(
        ME,
        &[make(
            "quote",
            "forward",
            "For your files.\n\n---------- Forwarded message ----------\nPlease reply tomorrow.",
            "pat@example.test",
            &[ME],
            first,
        )],
    )
    .unwrap();
    let rows = db.list_commitment_replies(Some(ME), false).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, "request");
    assert_eq!(rows[0].excerpt, "Could you confirm the estimate?");
    assert!(db
        .list_commitment_replies(Some("other@example.test"), false)
        .unwrap()
        .is_empty());
}

#[test]
fn commitments_use_cached_ai_only_for_the_current_message_and_when_enabled() {
    let db = database();
    clear_seed_threads(&db);
    let date = "2026-10-23T09:00:00Z";
    let mut request = unread("request", "delivery", date, "mira@example.test", &[ME]);
    request.body_text = "Which delivery date works for you?".into();
    request.labels.push("STARRED".into());
    db.upsert_thread(ME, &[request.clone()]).unwrap();
    let thread_id = super::super::threads::local_thread_id(ME, "delivery");
    let analysis = serde_json::from_value(serde_json::json!({
        "proposals": [{"type": "task", "kind": "action", "title": "Reply with a delivery date", "confidence": 0.9,
            "evidence": {"sourceMessageId": "request", "excerpt": "Which delivery date works for you?"}}],
        "hiddenCount": 0
    })).unwrap();
    db.save_thread_analysis(&thread_id, date, &analysis, date, true, None)
        .unwrap();
    assert!(db
        .list_commitment_replies(Some(ME), false)
        .unwrap()
        .is_empty());
    let rows = db.list_commitment_replies(Some(ME), true).unwrap();
    assert_eq!(
        rows[0].suggested_title.as_deref(),
        Some("Reply with a delivery date")
    );

    let mut newer = unread(
        "update",
        "delivery",
        "2026-10-23T10:00:00Z",
        "mira@example.test",
        &[ME],
    );
    newer.body_text = "We found a slot.".into();
    newer.labels.push("STARRED".into());
    db.upsert_thread(ME, &[request, newer]).unwrap();
    assert!(db
        .list_commitment_replies(Some(ME), true)
        .unwrap()
        .is_empty());
}
