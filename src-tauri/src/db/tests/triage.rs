use crate::db::test_support::{database, message};
use crate::models::{TriageAction, TriageContext, TriageEvent, TriageEventKind};

#[test]
fn triage_events_attribute_senders_and_rank_quick_dismissals() {
    let database = database();
    let mut quick_message = message(
        "quick-message",
        "quick-thread",
        "2026-01-01T00:00:00Z",
        "quick body",
    );
    quick_message.from = "Noise <Newsletter@Example.com>".into();
    database
        .upsert_thread("work@example.com", &[quick_message])
        .unwrap();

    let mut engaged_message = message(
        "engaged-message",
        "engaged-thread",
        "2026-01-02T00:00:00Z",
        "engaged body",
    );
    engaged_message.from = "A Person <person@example.com>".into();
    database
        .upsert_thread("work@example.com", &[engaged_message])
        .unwrap();

    for _ in 0..2 {
        database
            .record_triage_event(&TriageEvent {
                thread_id: "work@example.com:quick-thread".into(),
                kind: TriageEventKind::Open,
                context: TriageContext::Inbox,
                action: None,
                opened: false,
                dwell_ms: None,
                scrolled: false,
                batch: false,
            })
            .unwrap();
        database
            .record_triage_event(&TriageEvent {
                thread_id: "work@example.com:quick-thread".into(),
                kind: TriageEventKind::Disposition,
                context: TriageContext::Inbox,
                action: Some(TriageAction::Archive),
                opened: true,
                dwell_ms: Some(400),
                scrolled: false,
                batch: false,
            })
            .unwrap();
    }
    database
        .record_triage_event(&TriageEvent {
            thread_id: "work@example.com:quick-thread".into(),
            kind: TriageEventKind::Response,
            context: TriageContext::Inbox,
            action: None,
            opened: true,
            dwell_ms: None,
            scrolled: false,
            batch: false,
        })
        .unwrap();
    database
        .record_triage_event(&TriageEvent {
            thread_id: "work@example.com:engaged-thread".into(),
            kind: TriageEventKind::Open,
            context: TriageContext::Inbox,
            action: None,
            opened: false,
            dwell_ms: None,
            scrolled: false,
            batch: false,
        })
        .unwrap();
    database
        .record_triage_event(&TriageEvent {
            thread_id: "work@example.com:engaged-thread".into(),
            kind: TriageEventKind::Close,
            context: TriageContext::Inbox,
            action: None,
            opened: false,
            dwell_ms: Some(2400),
            scrolled: true,
            batch: false,
        })
        .unwrap();

    let stats = database
        .list_triage_sender_stats("work@example.com", 100)
        .unwrap();
    assert_eq!(stats[0].sender_email, "newsletter@example.com");
    assert_eq!(stats[0].sender_domain, "example.com");
    assert_eq!(stats[0].exposure_count, 2);
    assert_eq!(stats[0].archive_count, 2);
    assert_eq!(stats[0].quick_disposition_count, 2);
    assert_eq!(stats[0].quick_disposition_rate, 1.0);
    assert_eq!(stats[0].response_count, 1);
    assert_eq!(stats[1].sender_email, "person@example.com");
    assert_eq!(stats[1].engaged_view_count, 1);
    assert_eq!(stats[1].quick_disposition_count, 0);

    // Context is retained in the raw log but does not contaminate the
    // inbox-derived candidate stats.
    database
        .record_triage_event(&TriageEvent {
            thread_id: "work@example.com:quick-thread".into(),
            kind: TriageEventKind::Disposition,
            context: TriageContext::Other,
            action: Some(TriageAction::Trash),
            opened: true,
            dwell_ms: Some(100),
            scrolled: false,
            batch: false,
        })
        .unwrap();
    let stats = database
        .list_triage_sender_stats("work@example.com", 100)
        .unwrap();
    assert_eq!(stats[0].trash_count, 0);
}

/// Phase 4 run 2 (B): a message From ANY of the account's sending identities
/// counts as "sent by me", so a sent-only thread teaches nothing even when the
/// sender is a SECONDARY identity (not the login). Proven by recording a
/// triage event on such a thread and asserting NO triage row was written
/// (`sender_identity_for_thread` returns `None` for a sent-only thread).
#[test]
fn a_sent_only_thread_from_a_secondary_identity_teaches_nothing() {
    use crate::db::imap::write_settings_row;
    use crate::provider::imap::{Identity, ImapAccountSettings, LabelStorage, SecurityMode};
    let database = database();
    // Give the account an identity list: login first, then a secondary one.
    let settings = ImapAccountSettings {
        imap_host: "127.0.0.1".into(),
        imap_port: 1143,
        imap_security: SecurityMode::StartTls,
        imap_username: "me@proton.me".into(),
        smtp_host: "127.0.0.1".into(),
        smtp_port: 1025,
        smtp_security: SecurityMode::StartTls,
        smtp_username: "me@proton.me".into(),
        mailbox_overrides: Default::default(),
        archive_mailbox: None,
        label_storage: LabelStorage::None,
        label_container: None,
        identities: vec![
            Identity { address: "me@proton.me".into(), display_name: None },
            Identity { address: "work@proton.me".into(), display_name: Some("Work".into()) },
        ],
        pinned_fingerprints: Default::default(),
        server_saves_sent: false,
    };
    {
        let connection = database.connection().unwrap();
        write_settings_row(&connection, "me@proton.me", &settings).unwrap();
    }

    // A thread whose only message is From the SECONDARY identity.
    let mut sent = message(
        "sent-msg",
        "sent-thread",
        "2026-01-01T00:00:00Z",
        "sent body",
    );
    sent.from = "Work <work@proton.me>".into();
    database.upsert_thread("me@proton.me", &[sent]).unwrap();

    // Recording a disposition on this sent-only thread must write NOTHING,
    // because the sender is one of my identities.
    database
        .record_triage_event(&TriageEvent {
            thread_id: "me@proton.me:sent-thread".into(),
            kind: TriageEventKind::Disposition,
            context: TriageContext::Inbox,
            action: Some(TriageAction::Archive),
            opened: true,
            dwell_ms: Some(100),
            scrolled: false,
            batch: false,
        })
        .unwrap();
    let count: i64 = database
        .connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM triage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "a sent-only thread from a secondary identity must teach nothing"
    );

    // A thread from an OUTSIDE sender still learns (control).
    let mut inbound = message(
        "in-msg",
        "in-thread",
        "2026-01-02T00:00:00Z",
        "inbound body",
    );
    inbound.from = "Someone <someone@else.com>".into();
    database.upsert_thread("me@proton.me", &[inbound]).unwrap();
    database
        .record_triage_event(&TriageEvent {
            thread_id: "me@proton.me:in-thread".into(),
            kind: TriageEventKind::Disposition,
            context: TriageContext::Inbox,
            action: Some(TriageAction::Archive),
            opened: true,
            dwell_ms: Some(100),
            scrolled: false,
            batch: false,
        })
        .unwrap();
    let stats = database.list_triage_sender_stats("me@proton.me", 100).unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].sender_email, "someone@else.com");
}

/// Phase 4 run 2 (B): a Gmail / single-identity account (no
/// `imap_account_settings` row) is UNCHANGED — its sent-only thread (From the
/// account's own login address) still teaches nothing, exactly as before the
/// identity set widened.
#[test]
fn a_gmail_single_identity_sent_only_thread_is_unchanged() {
    let database = database();
    let mut sent = message(
        "g-sent",
        "g-thread",
        "2026-01-01T00:00:00Z",
        "sent body",
    );
    sent.from = "Me <you@example.com>".into();
    database.upsert_thread("you@example.com", &[sent]).unwrap();
    database
        .record_triage_event(&TriageEvent {
            thread_id: "you@example.com:g-thread".into(),
            kind: TriageEventKind::Disposition,
            context: TriageContext::Inbox,
            action: Some(TriageAction::Archive),
            opened: true,
            dwell_ms: Some(100),
            scrolled: false,
            batch: false,
        })
        .unwrap();
    let count: i64 = database
        .connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM triage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "a single-identity sent-only thread teaches nothing, as before");
}
