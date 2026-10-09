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
