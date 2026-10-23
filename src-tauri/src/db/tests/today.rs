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
    database.upsert_thread(ME, &[unread("a", "older", "2026-10-20T09:00:00Z", "Ann <ann@example.org>", &[ME])]).unwrap();
    database.upsert_thread(ME, &[unread("b", "newer", "2026-10-22T09:00:00Z", "Bo <bo@example.org>", &[ME])]).unwrap();
    let mut read = unread("c", "read", "2026-10-23T09:00:00Z", "Cy <cy@example.org>", &[ME]);
    read.labels = vec!["INBOX".into()];
    database.upsert_thread(ME, &[read]).unwrap();
    let mut archived = unread("d", "archived", "2026-10-23T10:00:00Z", "Di <di@example.org>", &[ME]);
    archived.labels = vec!["UNREAD".into()];
    database.upsert_thread(ME, &[archived]).unwrap();

    let page = database.list_today_mail(None).unwrap();
    let ids: Vec<&str> = page.threads.iter().map(|item| item.thread.id.as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(page.total, 2);
    assert!(page.threads[0].thread.last_received_at > page.threads[1].thread.last_received_at);
    assert_eq!(database.list_today_mail(Some("other@example.com")).unwrap().total, 0);
}

#[test]
fn today_mail_reads_signals_from_the_newest_message_someone_else_sent() {
    let database = database();
    clear_seed_threads(&database);
    // The user replied last and marked it unread; the signals still come from Ann's message.
    let mut reply = unread("a2", "replied", "2026-10-22T10:00:00Z", ME, &["ann@example.org"]);
    reply.labels = vec!["INBOX".into(), "SENT".into(), "UNREAD".into()];
    database.upsert_thread(ME, &[
        unread("a1", "replied", "2026-10-22T09:00:00Z", "Ann <ann@example.org>", &["Me <ME@example.com>"]),
        reply,
    ]).unwrap();
    let mut list = unread("n1", "newsletter", "2026-10-22T08:00:00Z", "News <news@example.net>", &["list@example.net"]);
    list.unsubscribe = Some(UnsubscribeMetadata { one_click_url: Some("https://example.net/u".into()), mailto_url: None, web_url: None, list_id: None });
    database.upsert_thread(ME, &[list]).unwrap();
    database.upsert_thread(ME, &[unread("s1", "stranger", "2026-10-22T07:00:00Z", "Sam <sam@example.org>", &[ME])]).unwrap();

    let page = database.list_today_mail(Some(ME)).unwrap();
    let find = |id: &str| page.threads.iter().find(|item| item.thread.provider_thread_id == id).unwrap();

    let replied = find("replied");
    assert_eq!(replied.sender.as_deref(), Some("Ann <ann@example.org>"));
    assert!(replied.addressed_to_me, "own address matches regardless of case or display name");
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
    database.upsert_thread(ME, &[unread("p1", "fresh", "2026-10-22T09:00:00Z", "Pat <PAT@example.org>", &[ME])]).unwrap();

    let page = database.list_today_mail(None).unwrap();
    assert_eq!(page.threads.len(), 1);
    assert!(page.threads[0].corresponded);
}
