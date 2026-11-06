use super::*;
use crate::db::test_support::{database, message};

const ACCOUNT: &str = "you@example.com";
const FUTURE: &str = "2099-01-01T09:00:00Z";
fn id() -> String {
    format!("{ACCOUNT}:conversation")
}
fn setup() -> Database {
    let db = database();
    db.adopt_account(ACCOUNT).unwrap();
    let mut item = message("first", "conversation", "2026-01-01T09:00:00Z", "hello");
    item.from = "other@example.com".into();
    item.labels = vec!["INBOX".into(), "UNREAD".into()];
    db.upsert_thread(ACCOUNT, &[item]).unwrap();
    db
}
fn snooze(db: &Database) -> ConversationSnooze {
    db.snooze_threads(&[id()], FUTURE).unwrap().remove(0)
}
fn state(db: &Database) -> String {
    db.list_snooze_records(Some(ACCOUNT)).unwrap()[0]
        .state
        .clone()
}
fn archived(db: &Database) -> bool {
    db.get_thread(&id()).unwrap().thread.archived
}
fn due() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(FUTURE)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn snoozing_archives_through_the_queue_preserves_unread_and_lists_by_account() {
    let db = setup();
    snooze(&db);
    assert!(archived(&db));
    assert!(db.get_thread(&id()).unwrap().thread.unread);
    let page = db.list_snoozed_page(Some(ACCOUNT), 0, 20).unwrap();
    assert_eq!(page.threads.len(), 1);
    assert_eq!(
        page.threads[0].snoozed_until.as_deref(),
        Some("2099-01-01T09:00:00+00:00")
    );
    assert!(db
        .list_snoozed_page(Some("other@example.com"), 0, 20)
        .unwrap()
        .threads
        .is_empty());
    assert!(!db
        .list_threads(Some(ACCOUNT))
        .unwrap()
        .iter()
        .any(|thread| thread.id == id()));
    assert_eq!(db.claim_mutations(ACCOUNT, 10).unwrap().len(), 1);
}

#[test]
fn returns_at_exact_time_and_catches_up_when_overdue_without_repeating() {
    for elapsed in [0, 60 * 60 * 48] {
        let db = setup();
        snooze(&db);
        assert!(!db
            .wake_due_snoozes(ACCOUNT, due() - chrono::Duration::milliseconds(1))
            .unwrap());
        assert!(archived(&db));
        assert!(db
            .wake_due_snoozes(ACCOUNT, due() + chrono::Duration::seconds(elapsed))
            .unwrap());
        assert!(!archived(&db));
        assert_eq!(state(&db), "returned");
        assert!(!db.wake_due_snoozes(ACCOUNT, due()).unwrap());
        assert!(db.get_thread(&id()).unwrap().thread.unread);
    }
}

#[test]
fn returns_unread_only_when_this_device_chose_it() {
    for returns_unread in [false, true] {
        let db = setup();
        assert!(!db.snooze_returns_unread().unwrap());
        db.set_snooze_returns_unread(returns_unread).unwrap();
        assert_eq!(db.snooze_returns_unread().unwrap(), returns_unread);
        db.with_transaction(|tx| Database::apply_mutation(tx, &ThreadMutation::Read { thread_id: id(), value: true })).unwrap();
        snooze(&db);
        assert!(db.wake_due_snoozes(ACCOUNT, due()).unwrap());
        let thread = db.get_thread(&id()).unwrap();
        assert!(!thread.thread.archived);
        assert_eq!(thread.thread.unread, returns_unread);
        assert_eq!(thread.messages.iter().any(|message| message.unread), returns_unread);
    }
}

#[test]
fn new_incoming_reply_wakes_but_own_sent_messages_and_old_refreshes_do_not() {
    let db = setup();
    snooze(&db);
    let mut old = message("first", "conversation", "2026-01-01T09:00:00Z", "hello");
    old.from = "other@example.com".into();
    old.labels = vec![];
    db.upsert_thread(ACCOUNT, &[old.clone()]).unwrap();
    assert_eq!(state(&db), "active");
    let mut sent = message("sent", "conversation", "2026-01-02T09:00:00Z", "my reply");
    sent.from = ACCOUNT.into();
    sent.labels = vec!["SENT".into()];
    db.upsert_thread(ACCOUNT, &[old.clone(), sent.clone()])
        .unwrap();
    assert_eq!(state(&db), "active");
    let mut reply = message(
        "reply",
        "conversation",
        "2026-01-01T08:00:00Z",
        "clock skew",
    );
    reply.from = "other@example.com".into();
    reply.labels = vec!["INBOX".into(), "UNREAD".into()];
    db.upsert_thread(ACCOUNT, &[old, sent, reply]).unwrap();
    assert_eq!(state(&db), "returned");
    assert!(!archived(&db));
}

#[test]
fn explicit_archive_trash_spam_and_unarchive_cancel_the_return() {
    for mutation in [
        ThreadMutation::Archive {
            thread_id: id(),
            value: true,
        },
        ThreadMutation::Archive {
            thread_id: id(),
            value: false,
        },
        ThreadMutation::Trash {
            thread_id: id(),
            value: true,
        },
        ThreadMutation::Spam {
            thread_id: id(),
            value: true,
        },
    ] {
        let db = setup();
        snooze(&db);
        db.mutate_thread(&mutation).unwrap();
        assert_eq!(state(&db), "cancelled");
        assert!(!db.wake_due_snoozes(ACCOUNT, due()).unwrap());
    }
}

#[test]
fn provider_trash_and_spam_cancel_instead_of_resurrecting_mail() {
    for label in ["TRASH", "SPAM"] {
        let db = setup();
        snooze(&db);
        let mut mail = message("first", "conversation", "2026-01-01T09:00:00Z", "hello");
        mail.from = "other@example.com".into();
        mail.labels = vec![label.into()];
        db.upsert_thread(ACCOUNT, &[mail]).unwrap();
        assert_eq!(state(&db), "cancelled");
        assert!(!db.wake_due_snoozes(ACCOUNT, due()).unwrap());
    }
}

#[test]
fn invalid_time_or_invalid_batch_rolls_everything_back() {
    let db = setup();
    for until in ["invalid", "2000-01-01T09:00:00Z"] {
        assert!(db.snooze_threads(&[id()], until).is_err());
    }
    assert!(db
        .snooze_threads(&[id(), "missing".into()], FUTURE)
        .is_err());
    assert!(!archived(&db));
    assert!(db.list_snooze_records(None).unwrap().is_empty());
    assert!(db.claim_mutations(ACCOUNT, 10).unwrap().is_empty());
}

#[test]
fn reschedule_and_return_undo_restore_prior_schedule_and_original_archive_state() {
    let db = setup();
    let first = snooze(&db);
    db.snooze_threads(&[id()], "2099-02-01T09:00:00Z").unwrap();
    db.restore_snoozes(&[first.clone()], &[]).unwrap();
    assert_eq!(db.list_snooze_records(None).unwrap(), vec![first.clone()]);
    db.return_snoozed_threads(&[id()]).unwrap();
    assert!(!archived(&db));
    db.restore_snoozes(&[first.clone()], &[]).unwrap();
    assert!(archived(&db));
    let mut cancelled = first;
    cancelled.state = "cancelled".into();
    db.restore_snoozes(&[cancelled.clone()], &[]).unwrap();
    assert!(!archived(&db));
    db.restore_snoozes(&[cancelled], &[id()]).unwrap();
    assert!(archived(&db));
}

#[test]
fn pending_archive_return_archive_preserves_last_requested_provider_action() {
    let db = setup();
    snooze(&db);
    db.return_snoozed_threads(&[id()]).unwrap();
    snooze(&db);
    let mutations = db.claim_mutations(ACCOUNT, 10).unwrap();
    assert_eq!(mutations.len(), 3);
    assert!(matches!(
        mutations.last().unwrap().mutation,
        ThreadMutation::Archive { value: true, .. }
    ));
}

#[test]
fn replicated_schedule_survives_missing_cache_and_never_contains_mail_content() {
    let source = setup();
    source.set_beta_features_enabled(true).unwrap();
    let item = snooze(&source);
    let payload = serde_json::json!({"schedule": item});
    assert!(!payload.to_string().contains("hello"));
    let peer = database();
    peer.adopt_account(ACCOUNT).unwrap();
    peer.materialize_entity(EntityType::ConversationSnooze, &id(), Some(&payload), false)
        .unwrap();
    assert_eq!(
        peer.snooze_refresh_ids(ACCOUNT, Utc::now()).unwrap(),
        vec!["conversation"]
    );
    assert!(peer
        .list_snoozed_page(Some(ACCOUNT), 0, 20)
        .unwrap()
        .threads
        .is_empty());
    assert!(source
        .entity_recorded(EntityType::ConversationSnooze, &id())
        .unwrap());
}

#[test]
fn synchronized_payload_rejects_identity_mismatch_invalid_dates_and_extra_content() {
    let db = setup();
    let mut item = snooze(&db);
    item.id = "other".into();
    assert!(db.upsert_synced_snooze(&item).is_err());
    item.id = id();
    item.until = "invalid".into();
    assert!(db.upsert_synced_snooze(&item).is_err());
    let payload = serde_json::json!({"schedule": item, "body": "private"});
    assert!(EntityType::ConversationSnooze
        .validate_payload(&payload)
        .is_err());
}

fn sync_into(peer: &Database, source: &Database) {
    let touched = peer
        .merge_replica_state(&source.load_replica_state().unwrap())
        .unwrap();
    peer.materialize_touched_entities(&touched).unwrap();
}

#[test]
fn schedule_reschedule_and_cancellation_replicate_without_mail_or_returning_cancelled_work() {
    let source = setup();
    source.set_beta_features_enabled(true).unwrap();
    let peer = setup();
    peer.set_beta_features_enabled(true).unwrap();
    snooze(&source);
    sync_into(&peer, &source);
    assert_eq!(state(&peer), "active");
    assert!(peer
        .list_threads(Some(ACCOUNT))
        .unwrap()
        .iter()
        .all(|thread| thread.id != id()));
    source
        .snooze_threads(&[id()], "2099-02-01T09:00:00Z")
        .unwrap();
    sync_into(&peer, &source);
    assert_eq!(
        peer.list_snooze_records(None).unwrap()[0].until,
        "2099-02-01T09:00:00+00:00"
    );
    source
        .mutate_thread(&ThreadMutation::Archive {
            thread_id: id(),
            value: true,
        })
        .unwrap();
    sync_into(&peer, &source);
    assert_eq!(state(&peer), "cancelled");
    assert!(!peer
        .wake_due_snoozes(ACCOUNT, due() + chrono::Duration::days(100))
        .unwrap());
}

#[test]
fn changes_made_while_sync_is_disabled_are_published_when_it_resumes() {
    let source = setup();
    source.set_beta_features_enabled(true).unwrap();
    let peer = setup();
    snooze(&source);
    sync_into(&peer, &source);
    source.set_beta_features_enabled(false).unwrap();
    source
        .mutate_thread(&ThreadMutation::Archive {
            thread_id: id(),
            value: true,
        })
        .unwrap();
    source.set_beta_features_enabled(true).unwrap();
    source.publish_dirty_snoozes().unwrap();
    sync_into(&peer, &source);
    assert_eq!(state(&peer), "cancelled");
}

#[test]
fn imported_return_waits_for_hydration_and_preserves_server_trash() {
    for trashed in [false, true] {
        let source = setup();
        let mut item = snooze(&source);
        item.state = "returned".into();
        let peer = database();
        peer.adopt_account(ACCOUNT).unwrap();
        peer.upsert_synced_snooze(&item).unwrap();
        assert_eq!(
            peer.snooze_refresh_ids(ACCOUNT, Utc::now()).unwrap(),
            vec!["conversation"]
        );
        let mut mail = message("first", "conversation", "2026-01-01T09:00:00Z", "hello");
        mail.from = "other@example.com".into();
        mail.labels = if trashed {
            vec!["TRASH".into()]
        } else {
            vec![]
        };
        peer.upsert_thread(ACCOUNT, &[mail]).unwrap();
        assert_eq!(archived(&peer), trashed);
        assert!(peer
            .snooze_refresh_ids(ACCOUNT, Utc::now())
            .unwrap()
            .is_empty());
    }
}

#[test]
fn retention_keeps_active_snoozed_mail_until_return_and_aliases_keep_the_schedule() {
    let db = setup();
    snooze(&db);
    db.set_retention_days(Some(30)).unwrap();
    db.prune_expired_threads().unwrap();
    assert!(db.get_thread(&id()).is_ok());
    let original = db.list_snooze_records(None).unwrap()[0].clone();
    let mut mail = message("first", "survivor", "2026-01-01T09:00:00Z", "hello");
    mail.from = "other@example.com".into();
    mail.labels = vec![];
    // Alias cleanup happens before ingestion. A later failure must restore
    // both the original schedule and its message references atomically.
    assert!(db.apply_ingested_threads_with_aliases(
        ACCOUNT,
        &[("survivor".into(), vec![mail.clone(), mail.clone()], vec![])],
        &[("conversation".into(), "survivor".into())],
    ).is_err());
    assert_eq!(db.list_snooze_records(None).unwrap(), vec![original.clone()]);
    assert!(db.get_thread(&id()).is_ok());
    db.apply_ingested_threads_with_aliases(
        ACCOUNT,
        &[("survivor".into(), vec![mail], vec![])],
        &[("conversation".into(), "survivor".into())],
    )
    .unwrap();
    let survivor = format!("{ACCOUNT}:survivor");
    let records = db.list_snooze_records(None).unwrap();
    assert_eq!(
        records
            .iter()
            .find(|item| item.id == original.id)
            .unwrap()
            .state,
        "cancelled"
    );
    let replacement = records.iter().find(|item| item.id == survivor).unwrap();
    assert_eq!(replacement.state, "active");
    assert_eq!(replacement.until, original.until);
    assert_eq!(replacement.message_ids, original.message_ids);
    assert!(db.get_thread(&survivor).unwrap().thread.archived);
}

#[test]
fn restart_preserves_the_schedule_and_queued_provider_work() {
    let path = crate::db::test_support::TempDbPath::new();
    let saved;
    {
        let db = Database::open(&path.path).unwrap();
        db.adopt_account(ACCOUNT).unwrap();
        let item = message("persisted", "conversation", "2026-01-01T09:00:00Z", "hello");
        db.upsert_thread(ACCOUNT, &[item]).unwrap();
        saved = snooze(&db);
    }
    let reopened = Database::open(&path.path).unwrap();
    assert_eq!(reopened.list_snooze_records(None).unwrap(), vec![saved]);
    assert!(archived(&reopened));
    assert_eq!(reopened.claim_mutations(ACCOUNT, 10).unwrap().len(), 1);
    assert!(reopened
        .wake_due_snoozes(ACCOUNT, due() + chrono::Duration::days(1))
        .unwrap());
    assert!(!archived(&reopened));
}

#[test]
fn a_due_schedule_not_verified_in_this_sync_round_stays_pending() {
    let db = setup();
    snooze(&db);
    assert!(!db.wake_verified_snoozes(ACCOUNT, due(), &[]).unwrap());
    assert_eq!(state(&db), "active");
    assert!(archived(&db));
}
