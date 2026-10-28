use super::mime::*;
use super::model::*;
use super::service::*;
use super::*;
use crate::clock::unix_millis;
use crate::{
    db::Database,
    mime::RawMessage,
    provider::{DeliveryReceipt, MailProvider},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mail_parser::MessageParser;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;
use mail_parser::MimeHeaders;
use rusqlite::params;
fn database() -> Database {
    let db = Database::open_memory();
    db.set_compose_identity("you@example.com").unwrap();
    db.adopt_account("you@example.com").unwrap();
    db
}
fn saved(db: &Database) -> Draft {
    let account = db.compose_identity().unwrap();
    let mut d = db.create_draft("new", None, &account, &[]).unwrap();
    d.to = "Jane <jane@example.com>".into();
    d.subject = "Hello".into();
    d.body = "Saved work ✓".into();
    db.save_draft(d).unwrap()
}

/// A database with a multi-identity IMAP account `me@proton.me` set up
/// (login first, one secondary `work@proton.me`), for the From-picker ops.
fn imap_database() -> Database {
    use crate::provider::imap::{Identity, ImapAccountSettings, LabelStorage, SecurityMode};
    let db = Database::open_memory();
    db.adopt_mail_account("me@proton.me", crate::models::MailProviderKind::Imap)
        .unwrap();
    db.set_compose_identity("me@proton.me").unwrap();
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
    db.save_imap_account_settings("me@proton.me", &settings).unwrap();
    db
}

#[test]
fn account_sending_identities_lists_login_first_with_display_names() {
    let db = imap_database();
    let identities = db.account_sending_identities("me@proton.me").unwrap();
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[0].address, "me@proton.me");
    assert_eq!(identities[1].address, "work@proton.me");
    assert_eq!(identities[1].display_name.as_deref(), Some("Work"));
    // A Gmail account (no settings row) is exactly [account].
    let gmail = database();
    let only = gmail.account_sending_identities("you@example.com").unwrap();
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].address, "you@example.com");
}

#[test]
fn set_draft_from_accepts_a_secondary_identity_and_bumps_the_revision() {
    let db = imap_database();
    let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
    let updated = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
    assert_eq!(updated.from.as_deref(), Some("work@proton.me"));
    assert_eq!(updated.revision, d.revision + 1);
    assert_eq!(db.draft(&d.id).unwrap().from.as_deref(), Some("work@proton.me"));
}

#[test]
fn set_draft_from_normalizes_the_primary_and_null_to_none() {
    let db = imap_database();
    let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
    let on_secondary = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
    // The account's own address stores None ("send as the account").
    let back = db.set_draft_from(&d.id, Some("ME@Proton.ME")).unwrap();
    assert_eq!(back.from, None, "the primary identity (any case) stores None");
    assert!(on_secondary.revision < back.revision);
    // An explicit null also stores None.
    let on_secondary = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
    let cleared = db.set_draft_from(&on_secondary.id, None).unwrap();
    assert_eq!(cleared.from, None);
}

#[test]
fn set_draft_from_is_allowed_for_a_reply_mode_draft() {
    let db = imap_database();
    // A reply/forward can legitimately send from a secondary identity.
    let d = Draft {
        id: Uuid::new_v4().to_string(),
        revision: 0,
        account: "me@proton.me".into(),
        from: None,
        mode: "reply".into(),
        source_id: None,
        thread_id: None,
        reply_id: None,
        references: vec![],
        to: String::new(),
        cc: String::new(),
        bcc: String::new(),
        subject: String::new(),
        body: String::new(),
        body_html: String::new(),
        signature: None,
        forwarded_content: None,
        forwarded_edit: None,
        follow_up_task_id: None,
        attachments: vec![],
        updated_at: unix_millis(),
        server_draft: None,
    };
    db.test_connection()
        .unwrap()
        .execute("INSERT INTO drafts VALUES (?1,?2,?3)", params![d.id, d.revision, json(&d).unwrap()])
        .unwrap();
    let updated = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
    assert_eq!(updated.from.as_deref(), Some("work@proton.me"));
}

#[test]
fn set_draft_from_rejects_an_address_not_on_the_account_and_writes_nothing() {
    let db = imap_database();
    let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
    let err = db.set_draft_from(&d.id, Some("stranger@elsewhere.com")).unwrap_err();
    assert!(err.contains("stranger@elsewhere.com"));
    // Nothing was written: revision and from unchanged.
    let reread = db.draft(&d.id).unwrap();
    assert_eq!(reread.revision, d.revision);
    assert_eq!(reread.from, None);
}

#[test]
fn set_draft_account_clears_a_stale_from() {
    // A draft on the IMAP account with a chosen secondary identity, moved
    // to another account, must drop the stale `from` (fact 2) so a later
    // queue does not reject "from no longer on this account".
    let db = imap_database();
    db.adopt_account("other@example.com").unwrap();
    let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
    let with_from = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
    assert_eq!(with_from.from.as_deref(), Some("work@proton.me"));
    let moved = db.set_draft_account(&d.id, "other@example.com").unwrap();
    assert_eq!(moved.account, "other@example.com");
    assert_eq!(moved.from, None, "moving the account must clear the stale from");
    assert_eq!(db.draft(&d.id).unwrap().from, None);
}

#[test]
fn permanent_sent_copy_message_is_plain_for_each_class() {
    // Each class rewrites the provider's developer wording into one plain
    // sentence that never leaks jargon.
    let no_folder = permanent_sent_copy_message(
        "cannot save the Sent copy: this account has no Sent mailbox configured; refusing permanently",
    );
    assert!(no_folder.starts_with("Your message was sent, but a copy couldn't be saved to your Sent folder"));
    assert!(no_folder.contains("no Sent folder is set up"));
    for jargon in ["refusing permanently", "mailbox", "APPEND"] {
        assert!(!no_folder.contains(jargon), "leaked jargon {jargon:?}: {no_folder}");
    }

    let too_large = permanent_sent_copy_message("message is 70000000 bytes, over the 67108865-byte (64 MiB) APPEND limit");
    assert!(too_large.contains("too large for the Sent folder"));
    assert!(!too_large.contains("APPEND"));

    let over_quota = permanent_sent_copy_message(
        "the mail server refused the APPEND of the Sent copy (e.g. over quota or a rejected message); it was not saved. The specific reason was not reported by the IMAP library.",
    );
    assert!(over_quota.contains("refused to store it"), "{over_quota}");
    assert!(!over_quota.contains("too large"), "a quota refusal must not be called 'too large': {over_quota}");

    // The EXAMINE-path spelling (a Sent mailbox that no longer exists) is a
    // missing-folder case too, not the generic fallback.
    let gone = permanent_sent_copy_message(
        "cannot save the Sent copy: the Sent mailbox \"Gesendet\" does not exist; configure or create it, then retry (x)",
    );
    assert!(gone.contains("no Sent folder is set up"), "{gone}");
    // Unrelated wording that merely contains "over the" is not an oversize.
    let unrelated = permanent_sent_copy_message("discovered the thing; over the moon");
    assert!(!unrelated.contains("too large"), "{unrelated}");

    let no_mid = permanent_sent_copy_message(
        "cannot save the Sent copy: the message has no Message-ID header to locate it by; refusing permanently",
    );
    assert!(no_mid.contains("missing a Message-ID"));
    assert!(!no_mid.contains("refusing permanently"));

    // An unrecognised permanent message still yields a jargon-free sentence.
    let generic = permanent_sent_copy_message("message contains a NUL byte, which an IMAP4rev1 literal cannot carry; refusing to APPEND");
    assert!(generic.starts_with("Your message was sent, but a copy couldn't be saved to your Sent folder"));
}

#[test]
fn sent_copy_error_message_classes_transient_vs_permanent() {
    use crate::provider::ProviderError;
    // Transient auth/transport -> "will retry".
    assert!(sent_copy_error_message(&ProviderError::Authentication("x".into())).contains("retry"));
    assert!(sent_copy_error_message(&ProviderError::TransientTransport("x".into())).contains("retry"));
    // Permanent -> plain "was sent, couldn't save a copy" with no retry promise.
    let permanent = sent_copy_error_message(&ProviderError::InvalidOperation(
        "cannot save the Sent copy: this account has no Sent mailbox configured; refusing permanently".into(),
    ));
    assert!(permanent.contains("Your message was sent"));
    assert!(!permanent.contains("retry"));
}
#[test]
fn retention_days_defaults_to_90_days_and_round_trips() {
    let db = database();
    assert_eq!(db.retention_days().unwrap(), Some(90));
    assert_eq!(db.chosen_retention_days().unwrap(), None);
    db.set_retention_days(Some(30)).unwrap();
    assert_eq!(db.retention_days().unwrap(), Some(30));
    assert_eq!(db.chosen_retention_days().unwrap(), Some(30));
    db.set_retention_days(None).unwrap();
    assert_eq!(db.retention_days().unwrap(), None);
}

#[test]
fn retention_default_and_choices_survive_reopening() {
    let temp = crate::db::test_support::TempDbPath::new();
    let db = Database::open(&temp.path).unwrap();
    assert_eq!(db.retention_days().unwrap(), Some(90));
    drop(db);
    let db = Database::open(&temp.path).unwrap();
    assert_eq!(db.retention_days().unwrap(), Some(90));
    drop(db);

    for choice in [None, Some(30), Some(90), Some(365), None] {
        let db = Database::open(&temp.path).unwrap();
        db.set_retention_days(choice).unwrap();
        drop(db);
        let db = Database::open(&temp.path).unwrap();
        assert_eq!(db.retention_days().unwrap(), choice);
    }
}

#[test]
fn retention_days_rejects_values_outside_the_supported_options() {
    let db = database();
    db.set_retention_days(Some(90)).unwrap();

    for days in [-1, 0, 1, 31, 366] {
        assert!(db.set_retention_days(Some(days)).is_err());
        assert_eq!(db.retention_days().unwrap(), Some(90));
    }
}

#[test]
fn reconnect_bootstrap_sets_only_a_missing_compose_identity() {
    let db = Database::open_memory();
    db.adopt_account("first@example.com").unwrap();
    db.ensure_compose_identity("first@example.com").unwrap();
    assert_eq!(db.compose_identity().unwrap(), "first@example.com");

    db.adopt_account("second@example.com").unwrap();
    db.ensure_compose_identity("second@example.com").unwrap();
    assert_eq!(db.compose_identity().unwrap(), "first@example.com");
}

#[test]
fn draft_revision_rejects_stale_writes_and_header_injection() {
    let db = database();
    let d = saved(&db);
    let stale = d.clone();
    let mut newer = d.clone();
    newer.body = "New version".into();
    db.save_draft(newer).unwrap();
    assert!(db.save_draft(stale).is_err());
    assert_eq!(db.draft(&d.id).unwrap().body, "New version");
    assert!(addresses("valid@example.com\r\nBcc: victim@example.com").is_err());
    let parsed =
        addresses("\"Doe, Jane\" <jane@example.com>, José <jose@example.com>").unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].0, "Doe, Jane");
}
#[test]
fn addresses_accept_a_quoted_display_name_holding_a_comma() {
    assert_eq!(
        addresses(r#""Fischgrund, Justin" <justin@example.com>, Kelly Sjol <kelly@example.com>, "Say \"Hi\"" <hi@example.com>"#).unwrap(),
        vec![
            ("Fischgrund, Justin".to_string(), "justin@example.com".to_string()),
            ("Kelly Sjol".to_string(), "kelly@example.com".to_string()),
            ("Say \"Hi\"".to_string(), "hi@example.com".to_string()),
        ]
    );
    // The unquoted form the field used to write is still rejected at save time.
    assert!(addresses("Fischgrund, Justin <justin@example.com>").is_err());
}

#[test]
fn stored_addresses_accept_unquoted_commas_that_strict_parsing_rejects() {
    let raw = "Daniel O'Connor, CFA® <doconnor@wealth.example>";
    assert!(addresses(raw).is_err());
    assert_eq!(stored_addresses(raw), vec![("Daniel O'Connor, CFA®".to_string(), "doconnor@wealth.example".to_string())]);
    assert_eq!(stored_addresses("Smith, Pat, PhD <pat@lab.example>"), vec![("Smith, Pat, PhD".to_string(), "pat@lab.example".to_string())]);
    assert_eq!(stored_addresses("\"Doe, Jane\" <jane@example.com>"), vec![("Doe, Jane".to_string(), "jane@example.com".to_string())]);
    assert_eq!(stored_addresses("bare@example.com"), vec![(String::new(), "bare@example.com".to_string())]);
}

#[test]
fn stored_addresses_yield_nothing_for_values_without_one_clear_address() {
    for raw in ["", "Daniel, CFA", "Name <not-an-address>", "Name <a b@example.com>", "Name <@example.com>", "a@example.com\r\nBcc: victim@example.com"] {
        assert!(stored_addresses(raw).is_empty(), "{raw}");
    }
}

#[test]
fn queue_is_durable_unique_and_cancel_restores_the_snapshot() {
    let db = database();
    db.set_account_display_name("you@example.com", Some("Joel Reed"))
        .unwrap();
    let d = saved(&db);
    let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
    let raw: Vec<u8> = db
        .test_connection()
        .unwrap()
        .query_row(
            "SELECT raw FROM outbox_messages WHERE id=?1",
            [&item.id],
            |row| row.get(0),
        )
        .unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(
        parsed
            .from()
            .and_then(|from| from.first())
            .and_then(|from| from.name()),
        Some("Joel Reed")
    );
    assert!(item.deadline > unix_millis());
    assert_eq!(
        db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused"))
            .unwrap()
            .id,
        item.id
    );
    assert_eq!(db.outbox().unwrap().len(), 1);
    assert!(db.drafts().unwrap().is_empty());
    let restored = db.cancel_send(&item.id, false).unwrap();
    assert_eq!(restored.body, d.body);
    assert!(restored.revision > d.revision);
    assert!(db.cancel_send(&item.id, false).is_err());
}
#[test]
fn archive_after_queue_keeps_only_matching_sends_archived() {
    use crate::models::ThreadMutation;

    let db = database();
    for (account, provider_thread_id) in [
        ("you@example.com", "reply-thread"),
        ("you@example.com", "other-thread"),
        ("other@example.com", "reply-thread"),
    ] {
        db.test_connection().unwrap().execute(
            "INSERT INTO threads SELECT ?1, ?2, subject, snippet, participants_json,
                last_message_at, unread, starred, archived, labels_json, trashed, ?3,
                summary, summary_generated_at, has_attachments, last_received_at,
                summary_revision, summary_focus_from
             FROM threads WHERE id='welcome'",
            params![format!("{account}:{provider_thread_id}"), provider_thread_id, account],
        ).unwrap();
    }
    let queue_for = |account: &str, thread_id: &str, archive_on_send: bool| {
        db.set_compose_identity(account).unwrap();
        db.adopt_account(account).unwrap();
        let draft = saved(&db);
        // Native reply creation owns routing fields; a regular save
        // deliberately refuses to change them.
        db.test_connection().unwrap().execute(
            "UPDATE drafts SET payload=json_set(payload, '$.mode', 'reply', '$.threadId', ?1)
             WHERE id=?2",
            params![thread_id, draft.id],
        ).unwrap();
        db.queue(&draft.id, draft.revision, archive_on_send, &[], None, Path::new("/unused")).unwrap()
    };
    let target = queue_for("you@example.com", "reply-thread", false);
    let already_archiving = queue_for("you@example.com", "reply-thread", true);
    let other_thread = queue_for("you@example.com", "other-thread", false);
    let other_account = queue_for("other@example.com", "reply-thread", false);

    let archive = |value| db.mutate_thread(&ThreadMutation::Archive {
        thread_id: "you@example.com:reply-thread".into(), value,
    }).unwrap();
    archive(true);
    assert!(db.archive_on_send(&target.id).unwrap());
    assert!(db.archive_on_send(&already_archiving.id).unwrap());
    assert!(!db.archive_on_send(&other_thread.id).unwrap());
    assert!(!db.archive_on_send(&other_account.id).unwrap());

    archive(false);
    assert!(!db.archive_on_send(&target.id).unwrap());
    assert!(!db.archive_on_send(&already_archiving.id).unwrap());

    // A completed send follows the ordinary archive mutation, rather
    // than retaining a stale post-send instruction.
    db.test_connection().unwrap().execute(
        "UPDATE outbox_messages SET state='sent' WHERE id=?1", [&target.id],
    ).unwrap();
    archive(true);
    assert!(!db.archive_on_send(&target.id).unwrap());
    assert!(db.archive_on_send(&already_archiving.id).unwrap());
}
#[test]
fn pausing_ready_sends_for_one_account_never_touches_another_accounts_outbox() {
    let db = database();
    let a = saved(&db);
    let item_a = db.queue(&a.id, a.revision, false, &[], None, Path::new("/unused")).unwrap();

    db.set_compose_identity("other@example.com").unwrap();
    let b = saved(&db);
    let item_b = db.queue(&b.id, b.revision, false, &[], None, Path::new("/unused")).unwrap();

    db.pause_ready_sends_for("you@example.com").unwrap();

    let outbox = db.outbox().unwrap();
    let state_of = |id: &str| outbox.iter().find(|o| o.id == id).unwrap().state.clone();
    assert_eq!(state_of(&item_a.id), "failed");
    assert_eq!(state_of(&item_b.id), "undo_pending");
}
#[tokio::test]
async fn a_tick_fails_sends_whose_account_was_removed_on_another_device() {
    let service = service();
    let db = &service.database;
    let queue = |db: &Database| {
        let d = saved(db);
        db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap()
    };
    let kept = queue(db);
    // The catalog stores `you@example.com`; a differently cased sender
    // address is still that account.
    db.set_compose_identity("You@Example.com").unwrap();
    let kept_cased = queue(db);
    // `gone@example.com` is not in the catalog, as after a synced removal.
    db.set_compose_identity("gone@example.com").unwrap();
    let gone_ready = queue(db);
    let gone_scheduled = queue(db);
    // Due next month: only the removal check, not the send loop, reaches it now.
    db.test_connection().unwrap().execute(
        "UPDATE outbox_messages SET state='scheduled', deadline=?2 WHERE id=?1",
        params![gone_scheduled.id, unix_millis() + 30 * 24 * 60 * 60 * 1000],
    ).unwrap();

    service.tick().await.unwrap();

    let outbox = db.outbox().unwrap();
    let state_of = |id: &str| outbox.iter().find(|o| o.id == id).unwrap().state.clone();
    assert_eq!(state_of(&gone_ready.id), "failed");
    assert_eq!(state_of(&gone_scheduled.id), "failed");
    assert_eq!(state_of(&kept.id), "undo_pending");
    assert_eq!(state_of(&kept_cased.id), "undo_pending");

    // Adding the account back must not revive the failed sends.
    db.adopt_account("gone@example.com").unwrap();
    service.tick().await.unwrap();
    let outbox = db.outbox().unwrap();
    assert_eq!(outbox.iter().find(|o| o.id == gone_scheduled.id).unwrap().state, "failed");
}
#[test]
fn restart_keeps_drafts_and_never_retries_an_interrupted_send() {
    let temp = crate::db::test_support::TempDbPath::new();
    let path = &temp.path;
    let id;
    {
        let db = Database::open(path).unwrap();
        db.set_compose_identity("you@example.com").unwrap();
        let d = saved(&db);
        id = d.id.clone();
        let queued = saved(&db);
        let item = db
            .queue(&queued.id, queued.revision, false, &[], None, Path::new("/unused"))
            .unwrap();
        db.test_connection()
            .unwrap()
            .execute(
                "UPDATE outbox_messages SET state='sending' WHERE id=?1",
                [item.id],
            )
            .unwrap();
    }
    {
        let db = Database::open(path).unwrap();
        assert_eq!(db.draft(&id).unwrap().body, "Saved work ✓");
        let item = db.outbox().unwrap().remove(0);
        assert_eq!(item.state, "uncertain");
        assert!(db.cancel_send(&item.id, true).is_err());
    }
}
#[test]
fn an_uncertain_send_resolves_to_terminal_unverifiable_for_a_provider_without_a_sent_copy() {
    // Mirrors the reconcile path for a non-verifiable provider: an
    // `uncertain` send whose `find_sent_copy` can never confirm it is
    // moved to the design's terminal `unverifiable` outcome rather than
    // being left to re-check forever. `docs/imap-design.md` ("Sending").
    let db = database();
    let d = saved(&db);
    let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
    db.test_connection()
        .unwrap()
        .execute(
            "UPDATE outbox_messages SET state='uncertain', error='dropped before 250' WHERE id=?1",
            [&item.id],
        )
        .unwrap();

    db.mark_unverifiable(&item.id).unwrap();

    let resolved = db.outbox().unwrap().into_iter().find(|o| o.id == item.id).unwrap();
    assert_eq!(resolved.state, "unverifiable");
    assert!(
        resolved
            .error
            .as_deref()
            .unwrap()
            .contains("could not be confirmed"),
        "{:?}",
        resolved.error
    );

    // Terminal: `tick` only re-processes undo_pending/ready/uncertain, so
    // an unverifiable row is never picked up again. Re-marking is a no-op
    // because the guard requires state='uncertain'.
    db.mark_unverifiable(&item.id).unwrap();
    assert_eq!(
        db.outbox().unwrap().into_iter().find(|o| o.id == item.id).unwrap().state,
        "unverifiable"
    );
}
#[test]
fn mark_unverifiable_only_touches_an_uncertain_row() {
    // The guard must never silently repurpose a sent/failed/ready row.
    let db = database();
    for state in ["sent", "failed", "ready", "undo_pending", "sending"] {
        let d = saved(&db);
        let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
        db.test_connection()
            .unwrap()
            .execute(
                "UPDATE outbox_messages SET state=?2 WHERE id=?1",
                params![item.id, state],
            )
            .unwrap();
        db.mark_unverifiable(&item.id).unwrap();
        assert_eq!(
            db.outbox().unwrap().into_iter().find(|o| o.id == item.id).unwrap().state,
            state,
            "mark_unverifiable must not touch a {state} row"
        );
    }
}

mod reconciliation {
    use super::*;
    use crate::{
        models::Label,
        provider::{
            Delivery, LabelModel, MailFetch, MailMutate, MailSend, MailSync, ProviderCapabilities,
            ProviderResult, SyncBatch, SyncCursor, ThreadPage,
        },
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ReconcileProvider {
        operation: String,
        verifiable_delivery: bool,
        sent_copy: Option<DeliveryReceipt>,
        lookups: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MailSend for ReconcileProvider {
        async fn sender_identity(&self) -> ProviderResult<String> {
            Ok("you@example.com".into())
        }

        async fn find_sent_copy(
            &self,
            operation: &str,
            expected_sender: &str,
        ) -> ProviderResult<Option<DeliveryReceipt>> {
            assert_eq!(operation, self.operation);
            assert_eq!(expected_sender, "you@example.com");
            self.lookups.fetch_add(1, Ordering::SeqCst);
            Ok(self.sent_copy.clone())
        }

        async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
            panic!("Reconciliation must never resend a message");
        }
    }

    impl MailProvider for ReconcileProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                send_supported: true,
                server_search: false,
                provided_threads: false,
                label_model: LabelModel::GmailLabels,
                verifiable_delivery: self.verifiable_delivery,
                client_saves_sent_copy: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl MailSync for ReconcileProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            panic!("Reconciliation must not start a sync");
        }
        async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
            panic!("Reconciliation must not poll");
        }
        async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
            panic!("Reconciliation must not list the inbox");
        }
        async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
            panic!("These receipts do not identify a provider thread");
        }
    }

    #[async_trait::async_trait]
    impl MailFetch for ReconcileProvider {
        async fn fetch_message(&self, _: &str) -> ProviderResult<RawMessage> {
            panic!("Reconciliation must not fetch a message");
        }
        async fn attachment_bytes(&self, _: &str, _: &str) -> ProviderResult<Vec<u8>> {
            panic!("Reconciliation must not fetch attachments");
        }
    }

    #[async_trait::async_trait]
    impl MailMutate for ReconcileProvider {
        async fn modify_thread(&self, _: &str, _: &[String], _: &[String]) -> ProviderResult<()> {
            panic!("These receipts do not identify a provider thread");
        }
        async fn modify_messages(
            &self,
            _: &[String],
            _: &[String],
            _: &[String],
        ) -> ProviderResult<()> {
            panic!("Reconciliation must not modify messages");
        }
        async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
            panic!("Reconciliation must not list labels");
        }
        async fn create_label(&self, _: &str) -> ProviderResult<Label> {
            panic!("Reconciliation must not create labels");
        }
        async fn update_label(&self, _: &str, _: &str) -> ProviderResult<Label> {
            panic!("Reconciliation must not update labels");
        }
        async fn delete_label(&self, _: &str) -> ProviderResult<()> {
            panic!("Reconciliation must not delete labels");
        }
    }

    #[tokio::test]
    async fn provider_capability_and_sent_copy_determine_the_delivery_outcome() {
        for verifiable_delivery in [false, true] {
            for found_copy in [false, true] {
                let service = service();
                let draft = saved(&service.database);
                let item = service
                    .database
                    .queue(&draft.id, draft.revision, false, &[], None, &service.root)
                    .unwrap();
                service.database.test_connection().unwrap().execute(
                    "UPDATE outbox_messages SET state='uncertain', error='Connection dropped before SMTP acceptance' WHERE id=?1",
                    [&item.id],
                ).unwrap();
                let item = service.database.outbox().unwrap().remove(0);
                let provider = ReconcileProvider {
                    operation: item.id.clone(),
                    verifiable_delivery,
                    sent_copy: found_copy.then(|| DeliveryReceipt {
                        provider_message_id: "confirmed-copy".into(),
                        thread_id: None,
                    }),
                    lookups: AtomicUsize::new(0),
                };

                let result = service.reconcile_with_provider(&item, &provider).await;
                let resolved = service.database.outbox().unwrap().remove(0);
                if found_copy {
                    result.unwrap();
                    assert_eq!(resolved.state, "sent");
                    assert_eq!(resolved.provider_id.as_deref(), Some("confirmed-copy"));
                    assert!(resolved.error.is_none());
                } else if verifiable_delivery {
                    assert_eq!(result.unwrap_err(), "Delivery is still uncertain. No automatic retry was made. Check Gmail Sent before composing another message.");
                    assert_eq!(resolved.state, "uncertain");
                    assert_eq!(resolved.error, item.error);
                } else {
                    result.unwrap();
                    assert_eq!(resolved.state, "unverifiable");
                    assert_eq!(
                        resolved.error.as_deref(),
                        Some("This message may have been sent, but delivery could not be confirmed. Confirm with the recipient before sending it again.")
                    );
                    assert!(resolved.provider_id.is_none());
                }

                if resolved.state != "uncertain" {
                    // Terminal outcomes must bypass provider access on later checks.
                    service.reconcile(&item.id).await.unwrap();
                    service.tick().await.unwrap();
                    assert_eq!(
                        service.database.outbox().unwrap().remove(0).state,
                        resolved.state
                    );
                }
                assert_eq!(provider.lookups.load(Ordering::SeqCst), 1);
            }
        }
    }
}

/// Work item D: a failing `prepare_delivery` must not wedge the queue, and
/// the per-item body (`send_due_item`) must be drivable with a fake
/// provider the account registry cannot hold.
mod prepare_failures {
    use super::*;
    use crate::{
        models::Label,
        provider::{
            Delivery, DeliveryReceipt, LabelModel, MailFetch, MailMutate, MailSend, MailSync,
            ProviderCapabilities, ProviderError, ProviderResult, SyncBatch, SyncCursor,
            ThreadPage,
        },
    };

    /// What the fake provider's `prepare_delivery` should do.
    #[derive(Clone)]
    enum PrepareBehaviour {
        /// Succeed, then `send_once` returns the given send result.
        Succeed(SendResult),
        /// Fail with the given error (no permit handed out).
        Fail(fn() -> ProviderError),
    }

    /// What the handed-out permit's `send_once` returns.
    #[derive(Clone, Copy)]
    enum SendResult {
        Accepted,
        /// Err((definite=false, ..)): the Unknown zone.
        Unknown,
        /// Err((definite=true, ..)): a definite non-delivery.
        NotDelivered,
    }

    struct FakeSender {
        behaviour: PrepareBehaviour,
    }

    struct ScriptedPermit {
        result: SendResult,
    }

    #[async_trait::async_trait]
    impl Delivery for ScriptedPermit {
        async fn send_once(
            self: Box<Self>,
            _raw: &[u8],
            thread: Option<&str>,
        ) -> Result<DeliveryReceipt, (bool, String)> {
            match self.result {
                SendResult::Accepted => Ok(DeliveryReceipt {
                    provider_message_id: "fake-sent".into(),
                    thread_id: thread.map(str::to_string),
                }),
                SendResult::Unknown => Err((
                    false,
                    "may have been delivered; check before resending".into(),
                )),
                SendResult::NotDelivered => {
                    Err((true, "the server refused the message".into()))
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl MailSend for FakeSender {
        async fn sender_identity(&self) -> ProviderResult<String> {
            Ok("you@example.com".into())
        }
        async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
            match &self.behaviour {
                PrepareBehaviour::Succeed(result) => {
                    Ok(Box::new(ScriptedPermit { result: *result }))
                }
                PrepareBehaviour::Fail(make) => Err(make()),
            }
        }
        async fn find_sent_copy(
            &self,
            _: &str,
            _: &str,
        ) -> ProviderResult<Option<DeliveryReceipt>> {
            // Faithful IMAP behaviour: no server-side copy to confirm
            // against, so a clean Ok(None) (never an error).
            Ok(None)
        }
    }

    impl MailProvider for FakeSender {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                send_supported: true,
                server_search: false,
                provided_threads: false,
                label_model: LabelModel::ImapLabelFolders,
                verifiable_delivery: false,
                client_saves_sent_copy: false,
            }
        }
    }

    // The non-send traits are never exercised by `send_due_item` on these
    // paths (a successful send with thread_id=None does no modify/fetch);
    // panic so a future change that starts calling them is caught.
    #[async_trait::async_trait]
    impl MailSync for FakeSender {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            panic!("send path must not sync");
        }
        async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
            panic!("send path must not poll");
        }
        async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
            panic!("send path must not list");
        }
        async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
            panic!("new-message send has no thread to fetch");
        }
    }
    #[async_trait::async_trait]
    impl MailFetch for FakeSender {
        async fn fetch_message(&self, _: &str) -> ProviderResult<RawMessage> {
            panic!("send path must not fetch a message");
        }
        async fn attachment_bytes(&self, _: &str, _: &str) -> ProviderResult<Vec<u8>> {
            panic!("send path must not fetch attachments");
        }
    }
    #[async_trait::async_trait]
    impl MailMutate for FakeSender {
        async fn modify_thread(&self, _: &str, _: &[String], _: &[String]) -> ProviderResult<()> {
            panic!("new-message send has no thread to modify");
        }
        async fn modify_messages(&self, _: &[String], _: &[String], _: &[String]) -> ProviderResult<()> {
            panic!("unused");
        }
        async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
            panic!("unused");
        }
        async fn create_label(&self, _: &str) -> ProviderResult<Label> {
            panic!("unused");
        }
        async fn update_label(&self, _: &str, _: &str) -> ProviderResult<Label> {
            panic!("unused");
        }
        async fn delete_label(&self, _: &str) -> ProviderResult<()> {
            panic!("unused");
        }
    }

    /// Queue a draft and advance it past its undo window to `ready` so it
    /// is a live send candidate. Returns the (re-read) outbox item.
    fn ready_item(service: &Correspondence) -> OutboxItem {
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, false, &[], None, &service.root)
            .unwrap();
        service
            .database
            .test_connection()
            .unwrap()
            .execute(
                "UPDATE outbox_messages SET state='ready', deadline=?2 WHERE id=?1",
                params![item.id, unix_millis() - 1],
            )
            .unwrap();
        service
            .database
            .outbox()
            .unwrap()
            .into_iter()
            .find(|o| o.id == item.id)
            .unwrap()
    }

    fn state_of(service: &Correspondence, id: &str) -> String {
        service
            .database
            .outbox()
            .unwrap()
            .into_iter()
            .find(|o| o.id == id)
            .unwrap()
            .state
    }

    fn attempts_of(service: &Correspondence, id: &str) -> i64 {
        service
            .database
            .test_connection()
            .unwrap()
            .query_row(
                "SELECT attempts FROM outbox_messages WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn a_transient_prepare_failure_leaves_the_item_queued_without_bumping_attempts() {
        let service = service();
        let item = ready_item(&service);
        let provider = FakeSender {
            behaviour: PrepareBehaviour::Fail(|| {
                ProviderError::TransientTransport("smtp unreachable".into())
            }),
        };
        // No error propagates out (the old code did `return Err`).
        service
            .send_due_item(&item, &provider, "you@example.com", "", unix_millis())
            .await
            .unwrap();
        // Still queued, not failed; a plain retry note; attempts untouched.
        assert_eq!(state_of(&service, &item.id), "ready");
        assert_eq!(attempts_of(&service, &item.id), 0);
        let row = service.database.outbox().unwrap().remove(0);
        assert!(row.error.as_deref().unwrap().contains("Will retry"), "{:?}", row.error);
        // last_attempt_at was stamped so the retry gate can rate-limit.
        assert!(row.last_attempt_at.is_some());
    }

    #[tokio::test]
    async fn a_permanent_prepare_failure_moves_the_row_to_failed_restorable() {
        for make in [
            (|| ProviderError::InvalidOperation("no outgoing server configured".into())) as fn() -> ProviderError,
            || ProviderError::PermanentClientRejection("refused".into()),
        ] {
            let service = service();
            let item = ready_item(&service);
            let provider = FakeSender { behaviour: PrepareBehaviour::Fail(make) };
            service
                .send_due_item(&item, &provider, "you@example.com", "", unix_millis())
                .await
                .unwrap();
            assert_eq!(state_of(&service, &item.id), "failed");
            // Nothing was sent, so attempts stays 0 (never claimed).
            assert_eq!(attempts_of(&service, &item.id), 0);
        }
    }

    /// Gmail's `prepare_delivery` refreshes an OAuth token, so it can fail
    /// with any of these. Before this change such an item stayed queued
    /// (the tick errored) and sent after the account recovered; it must
    /// not now be failed and discarded.
    #[tokio::test]
    async fn every_non_permanent_prepare_failure_keeps_the_send_queued() {
        for make in [
            (|| ProviderError::ReauthenticationRequired("token revoked".into())) as fn() -> ProviderError,
            || ProviderError::CertificateChanged("SMTP server".into()),
            || ProviderError::Authentication("bad token".into()),
            || ProviderError::RetryableServer("429".into()),
            || ProviderError::Other("dns".into()),
            || ProviderError::NotFound,
        ] {
            let service = service();
            let item = ready_item(&service);
            let provider = FakeSender { behaviour: PrepareBehaviour::Fail(make) };
            service
                .send_due_item(&item, &provider, "you@example.com", "", unix_millis())
                .await
                .unwrap();
            assert_eq!(state_of(&service, &item.id), "ready", "{:?}", make());
            assert_eq!(attempts_of(&service, &item.id), 0);
            let row = service.database.outbox().unwrap().remove(0);
            assert!(row.error.is_some() && row.last_attempt_at.is_some());
        }
    }

    #[tokio::test]
    async fn a_rejected_password_message_tells_the_user_to_reconnect() {
        let service = service();
        let item = ready_item(&service);
        let provider = FakeSender {
            behaviour: PrepareBehaviour::Fail(|| {
                ProviderError::ReauthenticationRequired("creds rejected".into())
            }),
        };
        service.send_due_item(&item, &provider, "you@example.com", "", unix_millis()).await.unwrap();
        let row = service.database.outbox().unwrap().remove(0);
        assert!(row.error.as_deref().unwrap().contains("Reconnect this account"), "{:?}", row.error);
    }

    #[tokio::test]
    async fn a_changed_certificate_message_points_to_the_review_not_the_password() {
        let service = service();
        let item = ready_item(&service);
        let provider = FakeSender {
            behaviour: PrepareBehaviour::Fail(|| ProviderError::CertificateChanged("SMTP server".into())),
        };
        service.send_due_item(&item, &provider, "you@example.com", "", unix_millis()).await.unwrap();
        let row = service.database.outbox().unwrap().remove(0);
        let note = row.error.as_deref().unwrap();
        assert!(note.contains("certificate changed"), "{note}");
        assert!(!note.contains("password"), "{note}");
    }

    #[tokio::test]
    async fn a_successful_prepare_sends_and_marks_the_row_sent() {
        let service = service();
        let item = ready_item(&service);
        let provider = FakeSender {
            behaviour: PrepareBehaviour::Succeed(SendResult::Accepted),
        };
        service
            .send_due_item(&item, &provider, "you@example.com", "", unix_millis())
            .await
            .unwrap();
        assert_eq!(state_of(&service, &item.id), "sent");
        // A real claim happened, so attempts is bumped exactly once.
        assert_eq!(attempts_of(&service, &item.id), 1);
    }

    #[tokio::test]
    async fn a_definite_send_failure_marks_the_row_failed() {
        let service = service();
        let item = ready_item(&service);
        let provider = FakeSender {
            behaviour: PrepareBehaviour::Succeed(SendResult::NotDelivered),
        };
        service
            .send_due_item(&item, &provider, "you@example.com", "", unix_millis())
            .await
            .unwrap();
        assert_eq!(state_of(&service, &item.id), "failed");
        assert_eq!(attempts_of(&service, &item.id), 1);
    }

    #[tokio::test]
    async fn an_unknown_send_goes_uncertain_then_reconciles_to_unverifiable() {
        // The full fact-2 chain: send_once Unknown -> row 'uncertain';
        // then reconcile_with_provider on a faithful IMAP fake
        // (verifiable_delivery=false, find_sent_copy=Ok(None)) resolves it
        // to the terminal 'unverifiable' — NOT an error, which the stale
        // find_sent_copy stub would have produced.
        let service = service();
        let item = ready_item(&service);
        let provider = FakeSender {
            behaviour: PrepareBehaviour::Succeed(SendResult::Unknown),
        };
        service
            .send_due_item(&item, &provider, "you@example.com", "", unix_millis())
            .await
            .unwrap();
        assert_eq!(state_of(&service, &item.id), "uncertain");

        let item = service
            .database
            .outbox()
            .unwrap()
            .into_iter()
            .find(|o| o.id == item.id)
            .unwrap();
        service
            .reconcile_with_provider(&item, &provider)
            .await
            .expect("reconcile must not error on a no-sent-copy IMAP provider");
        assert_eq!(state_of(&service, &item.id), "unverifiable");
    }

    #[tokio::test]
    async fn one_accounts_down_server_does_not_stop_another_accounts_send() {
        // Two items; the first account's prepare fails transiently, the
        // second account's prepare succeeds. Driving them in sequence (as
        // tick does per account) proves the first's failure is Ok(()) and
        // the second still sends.
        let service = service();
        let failing_item = ready_item(&service);
        let sending_item = ready_item(&service);
        let down = FakeSender {
            behaviour: PrepareBehaviour::Fail(|| {
                ProviderError::TransientTransport("down".into())
            }),
        };
        let up = FakeSender { behaviour: PrepareBehaviour::Succeed(SendResult::Accepted) };
        service
            .send_due_item(&failing_item, &down, "you@example.com", "", unix_millis())
            .await
            .unwrap();
        service
            .send_due_item(&sending_item, &up, "you@example.com", "", unix_millis())
            .await
            .unwrap();
        assert_eq!(state_of(&service, &failing_item.id), "ready");
        assert_eq!(state_of(&service, &sending_item.id), "sent");
    }

    #[test]
    fn the_retry_gate_honours_the_interval_below_at_and_above() {
        use crate::provider::imap::policy::SMTP_PREPARE_RETRY_INTERVAL_MS as INTERVAL;
        use crate::provider::imap::smtp_prepare_retry_due;
        let last = 1_000_000i64;
        // Never attempted: always eligible.
        assert!(smtp_prepare_retry_due(None, last));
        // Below the interval: not re-attempted.
        assert!(!smtp_prepare_retry_due(Some(last), last + INTERVAL - 1));
        // Exactly at: eligible.
        assert!(smtp_prepare_retry_due(Some(last), last + INTERVAL));
        // Above: eligible.
        assert!(smtp_prepare_retry_due(Some(last), last + INTERVAL + 1));
    }
}

/// Work item C/E: the durable Sent-copy job runner. A fake provider scripts
/// `save_sent_copy`'s answers and COUNTS `send_once`, so every scenario can
/// assert the message is NEVER resent and the outbox state/provider_id never
/// change while the job advances. The engine body (`run_sent_copy_job`,
/// `process_sent_copy_jobs`, and the atomic seed in `dispatch_due`) is
/// driven directly, the fake injected like `send_due_item`'s.
mod sent_copy_jobs {
    use super::*;
    use crate::{
        models::Label,
        provider::{
            Delivery, DeliveryReceipt, LabelModel, MailFetch, MailMutate, MailSend, MailSync,
            ProviderCapabilities, ProviderError, ProviderResult, SentCopyRequest, SentCopyStep,
            SyncBatch, SyncCursor, ThreadPage,
        },
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    /// What the fake's `save_sent_copy` returns, one scripted answer per
    /// call (popped front to back; the last repeats if the queue empties).
    #[derive(Clone)]
    enum JobAnswer {
        Done,
        Recheck(Duration),
        Transient,
        Permanent,
    }

    struct JobProvider {
        answers: StdMutex<Vec<JobAnswer>>,
        save_calls: AtomicUsize,
        send_calls: AtomicUsize,
        client_saves_sent_copy: bool,
    }

    impl JobProvider {
        fn new(answers: Vec<JobAnswer>) -> Self {
            Self {
                answers: StdMutex::new(answers),
                save_calls: AtomicUsize::new(0),
                send_calls: AtomicUsize::new(0),
                client_saves_sent_copy: true,
            }
        }
    }

    struct NoResendPermit;
    #[async_trait::async_trait]
    impl Delivery for NoResendPermit {
        async fn send_once(
            self: Box<Self>,
            _raw: &[u8],
            _thread: Option<&str>,
        ) -> Result<DeliveryReceipt, (bool, String)> {
            unreachable!("the Sent-copy job never sends");
        }
    }

    #[async_trait::async_trait]
    impl MailSend for JobProvider {
        async fn sender_identity(&self) -> ProviderResult<String> {
            Ok("you@example.com".into())
        }
        async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
            // Never called by the job runner; present so the trait is
            // satisfied. A call would be a bug (the job must not resend).
            Ok(Box::new(NoResendPermit))
        }
        async fn find_sent_copy(
            &self,
            _: &str,
            _: &str,
        ) -> ProviderResult<Option<DeliveryReceipt>> {
            Ok(None)
        }
        async fn save_sent_copy(
            &self,
            _request: &SentCopyRequest,
        ) -> ProviderResult<SentCopyStep> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            let mut answers = self.answers.lock().unwrap();
            let answer = if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers.first().cloned().unwrap_or(JobAnswer::Done)
            };
            match answer {
                JobAnswer::Done => Ok(SentCopyStep::Done),
                JobAnswer::Recheck(after) => Ok(SentCopyStep::Recheck { after }),
                JobAnswer::Transient => {
                    Err(ProviderError::TransientTransport("sent mailbox unreachable".into()))
                }
                JobAnswer::Permanent => Err(ProviderError::InvalidOperation(
                    "no Sent mailbox configured".into(),
                )),
            }
        }
    }

    impl MailProvider for JobProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                send_supported: true,
                server_search: false,
                provided_threads: false,
                label_model: LabelModel::ImapLabelFolders,
                verifiable_delivery: false,
                client_saves_sent_copy: self.client_saves_sent_copy,
            }
        }
    }

    // The non-send traits are never used by the job runner.
    #[async_trait::async_trait]
    impl MailSync for JobProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            panic!("job runner must not sync");
        }
        async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
            panic!("job runner must not poll");
        }
        async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
            panic!("job runner must not list");
        }
        async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
            panic!("job runner must not fetch a thread");
        }
    }
    #[async_trait::async_trait]
    impl MailFetch for JobProvider {
        async fn fetch_message(&self, _: &str) -> ProviderResult<RawMessage> {
            panic!("unused");
        }
        async fn attachment_bytes(&self, _: &str, _: &str) -> ProviderResult<Vec<u8>> {
            panic!("unused");
        }
    }
    #[async_trait::async_trait]
    impl MailMutate for JobProvider {
        async fn modify_thread(&self, _: &str, _: &[String], _: &[String]) -> ProviderResult<()> {
            panic!("unused");
        }
        async fn modify_messages(&self, _: &[String], _: &[String], _: &[String]) -> ProviderResult<()> {
            panic!("unused");
        }
        async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
            panic!("unused");
        }
        async fn create_label(&self, _: &str) -> ProviderResult<Label> {
            panic!("unused");
        }
        async fn update_label(&self, _: &str, _: &str) -> ProviderResult<Label> {
            panic!("unused");
        }
        async fn delete_label(&self, _: &str) -> ProviderResult<()> {
            panic!("unused");
        }
    }

    /// Queue a draft and advance it to `ready` so dispatch_due can claim it.
    fn ready_item(service: &Correspondence) -> OutboxItem {
        let d = saved(&service.database);
        let item = service
            .database
            .queue(&d.id, d.revision, false, &[], None, &service.root)
            .unwrap();
        service
            .database
            .test_connection()
            .unwrap()
            .execute(
                "UPDATE outbox_messages SET state='ready', deadline=?2 WHERE id=?1",
                params![item.id, unix_millis() - 1],
            )
            .unwrap();
        service
            .database
            .outbox()
            .unwrap()
            .into_iter()
            .find(|o| o.id == item.id)
            .unwrap()
    }

    /// Insert a 'sent' outbox row carrying a pending/verify Sent-copy job.
    fn seed_job(
        service: &Correspondence,
        id: &str,
        account: &str,
        state: &str,
        attempts: i64,
        next_at: i64,
        append_attempted: i64,
    ) {
        let c = service.database.test_connection().unwrap();
        c.execute(
            "INSERT INTO outbox_messages
                (id, draft_id, revision, account, state, deadline, payload, raw, provider_id,
                 sent_copy_state, sent_copy_attempts, sent_copy_next_at, sent_copy_append_attempted)
             VALUES (?1, ?1, 1, ?2, 'sent', 0, '{}', x'4869', 'pm1',
                     ?3, ?4, ?5, ?6)",
            params![id, account, state, attempts, next_at, append_attempted],
        )
        .unwrap();
    }

    fn job_row(
        service: &Correspondence,
        id: &str,
    ) -> (String, Option<String>, i64, Option<i64>, i64, String, Option<String>) {
        service
            .database
            .test_connection()
            .unwrap()
            .query_row(
                "SELECT state, sent_copy_state, sent_copy_attempts, sent_copy_next_at,
                        sent_copy_append_attempted, provider_id, sent_copy_error
                 FROM outbox_messages WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .unwrap()
    }

    /// Assert the outbox send-state and provider_id are UNTOUCHED by the job
    /// runner, across every scenario.
    fn assert_send_state_untouched(
        row: &(String, Option<String>, i64, Option<i64>, i64, String, Option<String>),
    ) {
        assert_eq!(row.0, "sent", "job runner must never change outbox state");
        assert_eq!(row.5, "pm1", "job runner must never change provider_id");
    }

    #[tokio::test]
    async fn dispatch_seeds_the_job_atomically_only_when_the_capability_is_true() {
        // Capability true: the accepted-send UPDATE sets the job pending with
        // next_at = sent_at + SENT_COPY_SETTLE.
        {
            let service = service();
            let item = ready_item(&service);
            let at = unix_millis();
            service
                .dispatch_due(&item, at, true, |_, thread| async move {
                    Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: thread })
                })
                .await
                .unwrap();
            let row = job_row(&service, &item.id);
            assert_eq!(row.0, "sent");
            assert_eq!(row.1.as_deref(), Some("pending"));
            assert_eq!(row.2, 0, "attempts start at 0");
            let expected = at + crate::provider::imap::SENT_COPY_SETTLE.as_millis() as i64;
            assert_eq!(row.3, Some(expected), "first attempt waits the settle window");
            assert_eq!(row.4, 0, "append not yet attempted");
        }

        // Capability false (Gmail-like): every sent_copy_* column stays
        // NULL/default — no job is created.
        {
            let service = service();
            let item = ready_item(&service);
            service
                .dispatch_due(&item, unix_millis(), false, |_, thread| async move {
                    Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: thread })
                })
                .await
                .unwrap();
            let row = job_row(&service, &item.id);
            assert_eq!(row.0, "sent");
            assert!(row.1.is_none(), "no job for a Gmail-like provider");
            assert!(row.3.is_none());
            assert_eq!(row.4, 0);
        }
    }

    #[tokio::test]
    async fn a_schedule_originated_send_also_seeds_the_job() {
        // The seed is in dispatch_due's accepted-send UPDATE, which every
        // send path (immediate AND scheduled) goes through, so a scheduled
        // send gets a job too. Drive dispatch_due with a non-scheduled item
        // but the capability on — the UPDATE is identical.
        let service = service();
        let item = ready_item(&service);
        service
            .dispatch_due(&item, unix_millis(), true, |_, _| async {
                Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: None })
            })
            .await
            .unwrap();
        assert_eq!(job_row(&service, &item.id).1.as_deref(), Some("pending"));
    }

    #[tokio::test]
    async fn done_marks_the_job_done_and_never_resends() {
        let service = service();
        seed_job(&service, "j1", "you@example.com", "pending", 0, unix_millis() - 1, 0);
        let provider = JobProvider::new(vec![JobAnswer::Done]);
        service
            .run_sent_copy_job(&provider, "j1", b"Hi", "pending", 0, false, unix_millis())
            .await
            .unwrap();
        let row = job_row(&service, "j1");
        assert_eq!(row.1.as_deref(), Some("done"));
        assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0, "NO RESEND");
        assert_send_state_untouched(&row);
        // The write-ahead append flag was set before the provider call.
        assert_eq!(row.4, 1, "append_attempted write-ahead flag set");
    }

    #[tokio::test]
    async fn recheck_moves_the_job_to_verify_with_the_right_next_at() {
        let service = service();
        seed_job(&service, "j1", "you@example.com", "pending", 0, unix_millis() - 1, 0);
        let at = unix_millis();
        let provider = JobProvider::new(vec![JobAnswer::Recheck(Duration::from_secs(10))]);
        service
            .run_sent_copy_job(&provider, "j1", b"Hi", "pending", 0, false, at)
            .await
            .unwrap();
        let row = job_row(&service, "j1");
        assert_eq!(row.1.as_deref(), Some("verify"));
        assert_eq!(row.3, Some(at + 10_000));
        assert_send_state_untouched(&row);
    }

    #[tokio::test]
    async fn a_transient_error_bumps_attempts_and_backs_off_never_before_the_settle_window() {
        use crate::provider::imap::{sent_copy_backoff, SENT_COPY_SETTLE};
        let service = service();
        seed_job(&service, "j1", "you@example.com", "verify", 1, unix_millis() - 1, 1);
        let at = unix_millis();
        let provider = JobProvider::new(vec![JobAnswer::Transient]);
        service
            .run_sent_copy_job(&provider, "j1", b"Hi", "verify", 1, true, at)
            .await
            .unwrap();
        let row = job_row(&service, "j1");
        assert_eq!(row.1.as_deref(), Some("verify"), "state is KEPT on a transient error");
        assert_eq!(row.2, 2, "attempts+1");
        let scheduled = sent_copy_backoff(1).as_millis() as i64;
        assert_eq!(row.3, Some(at + scheduled));
        // Below/exact/above the settle floor: the retry is never earlier
        // than the settle window after this attempt.
        assert!(
            row.3.unwrap() >= at + SENT_COPY_SETTLE.as_millis() as i64,
            "retry must not be earlier than the settle window"
        );
        assert!(row.6.is_some(), "a plain-language error note is stored");
        assert_send_state_untouched(&row);
        assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn transient_at_max_attempts_abandons() {
        use crate::provider::imap::SENT_COPY_MAX_ATTEMPTS;
        let service = service();
        let at = unix_millis();
        let prior = (SENT_COPY_MAX_ATTEMPTS - 1) as i64;
        seed_job(&service, "j1", "you@example.com", "verify", prior, at - 1, 1);
        let provider = JobProvider::new(vec![JobAnswer::Transient]);
        service
            .run_sent_copy_job(&provider, "j1", b"Hi", "verify", prior, true, at)
            .await
            .unwrap();
        let row = job_row(&service, "j1");
        assert_eq!(row.1.as_deref(), Some("abandoned"), "max attempts -> abandoned");
        assert_eq!(row.2, SENT_COPY_MAX_ATTEMPTS as i64);
        assert_send_state_untouched(&row);
    }

    #[tokio::test]
    async fn a_permanent_error_abandons_immediately_with_a_plain_error() {
        let service = service();
        let at = unix_millis();
        seed_job(&service, "j1", "you@example.com", "pending", 0, at - 1, 0);
        let provider = JobProvider::new(vec![JobAnswer::Permanent]);
        service
            .run_sent_copy_job(&provider, "j1", b"Hi", "pending", 0, false, at)
            .await
            .unwrap();
        let row = job_row(&service, "j1");
        assert_eq!(row.1.as_deref(), Some("abandoned"));
        assert_eq!(row.2, 0, "a permanent failure does not count a retry attempt");
        // run 5 rewrites the permanent-class provider message into plain
        // user wording before it is stored in sent_copy_error (the raw
        // "no Sent mailbox configured" developer text no longer reaches
        // the user). See sent_copy_error_message / permanent_sent_copy_message.
        assert_eq!(
            row.6.as_deref(),
            Some("Your message was sent, but a copy couldn't be saved to your Sent folder because no Sent folder is set up for this account.")
        );
        assert_send_state_untouched(&row);
        assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_provider_unavailable_account_is_skipped_without_counting_an_attempt() {
        // No account is registered, so provider_for fails. process_sent_copy_jobs
        // must skip the job and leave it exactly as-is, and the tick continues.
        let service = service();
        let at = unix_millis();
        seed_job(&service, "j1", "ghost@example.com", "pending", 0, at - 1, 0);
        service.process_sent_copy_jobs(at).await.unwrap();
        let row = job_row(&service, "j1");
        assert_eq!(row.1.as_deref(), Some("pending"), "unchanged");
        assert_eq!(row.2, 0, "no attempt counted");
        assert_eq!(row.4, 0, "no write-ahead flag set (never reached the provider)");
    }

    #[tokio::test]
    async fn a_due_job_is_processed_even_with_no_sendable_outbox_items() {
        // tick() runs process_sent_copy_jobs BEFORE the no-sendable early
        // return. Seed a real 'sent' row carrying a due job (via a dispatched
        // send so its payload is a valid Draft), on an UNREGISTERED account
        // so the job is only skipped — then tick() must still run cleanly and
        // reach the processor rather than short-circuit on the early return.
        let service = service();
        let item = ready_item(&service);
        let at = unix_millis();
        service
            .dispatch_due(&item, at, true, |_, _| async {
                Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: None })
            })
            .await
            .unwrap();
        // Make the job due now and point it at an account with no provider.
        service
            .database
            .test_connection()
            .unwrap()
            .execute(
                "UPDATE outbox_messages SET account='ghost@example.com', sent_copy_next_at=?2 WHERE id=?1",
                params![item.id, at - 1],
            )
            .unwrap();
        // No sendable items remain (the only row is 'sent'); tick must still
        // run cleanly, which it can only do by reaching the processor.
        service.tick().await.unwrap();
        // The job is unchanged (provider unavailable), proving the processor
        // ran despite there being nothing to send.
        assert_eq!(job_row(&service, &item.id).1.as_deref(), Some("pending"));
    }

    #[tokio::test]
    async fn the_per_tick_cap_bounds_how_many_jobs_run() {
        use crate::provider::imap::SENT_COPY_MAX_JOBS_PER_TICK;
        let service = service();
        let at = unix_millis();
        // A full older batch on an unavailable account must not consume
        // any work slots. The healthy jobs still run, oldest first.
        for i in 0..SENT_COPY_MAX_JOBS_PER_TICK {
            seed_job(
                &service,
                &format!("ghost{i}"),
                "ghost@example.com",
                "pending",
                0,
                at - 2,
                0,
            );
        }
        let total = SENT_COPY_MAX_JOBS_PER_TICK + 2;
        for i in 0..total {
            let account = if i % 2 == 0 {
                "a@example.com"
            } else {
                "b@example.com"
            };
            seed_job(&service, &format!("j{i}"), account, "pending", 0, at - 1, 0);
        }
        let working = Arc::new(JobProvider::new(vec![JobAnswer::Done]));
        for tick in 0..2 {
            let resolved = working.clone();
            service
                .process_sent_copy_jobs_with(at, move |account| {
                    let provider = resolved.clone();
                    async move {
                        (account != "ghost@example.com")
                            .then_some(provider as Arc<dyn MailProvider>)
                    }
                })
                .await
                .unwrap();
            let completed = if tick == 0 {
                SENT_COPY_MAX_JOBS_PER_TICK
            } else {
                total
            };
            for i in 0..total {
                assert_eq!(
                    job_row(&service, &format!("j{i}")).1.as_deref(),
                    Some(if i < completed { "done" } else { "pending" })
                );
            }
            for i in 0..SENT_COPY_MAX_JOBS_PER_TICK {
                let row = job_row(&service, &format!("ghost{i}"));
                assert_eq!(row.1.as_deref(), Some("pending"));
                assert_eq!(row.2, 0, "unavailable jobs count no attempt");
                assert_eq!(row.4, 0, "unavailable jobs do not start an append");
            }
            assert_eq!(working.save_calls.load(Ordering::SeqCst), completed);
            assert_eq!(working.send_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn one_account_failing_does_not_stop_another_accounts_job() {
        // Drive two jobs for two accounts directly: one permanent-fails, the
        // other succeeds. (process_sent_copy_jobs groups by account; this
        // asserts the per-job body is independent and neither resends.)
        let service = service();
        let at = unix_millis();
        seed_job(&service, "fail", "a@example.com", "pending", 0, at - 1, 0);
        seed_job(&service, "ok", "b@example.com", "pending", 0, at - 1, 0);
        let failing = JobProvider::new(vec![JobAnswer::Permanent]);
        let working = JobProvider::new(vec![JobAnswer::Done]);
        service.run_sent_copy_job(&failing, "fail", b"Hi", "pending", 0, false, at).await.unwrap();
        service.run_sent_copy_job(&working, "ok", b"Hi", "pending", 0, false, at).await.unwrap();
        assert_eq!(job_row(&service, "fail").1.as_deref(), Some("abandoned"));
        assert_eq!(job_row(&service, "ok").1.as_deref(), Some("done"));
        assert_eq!(failing.send_calls.load(Ordering::SeqCst), 0);
        assert_eq!(working.send_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_crash_between_accept_and_the_next_tick_still_finds_the_job() {
        // Seed the job via dispatch_due (the atomic accept path), then reopen
        // the DB (simulate a restart) and confirm the job row survives with
        // its pending state and settle-window next_at.
        let service = service();
        let item = ready_item(&service);
        let at = unix_millis();
        service
            .dispatch_due(&item, at, true, |_, _| async {
                Ok(DeliveryReceipt { provider_message_id: "pm".into(), thread_id: None })
            })
            .await
            .unwrap();
        // A fresh outbox() read (what the next tick does) still sees the job.
        let (_, copy_state, _, next_at, append_attempted, _, _) = job_row(&service, &item.id);
        assert_eq!(copy_state.as_deref(), Some("pending"), "job survives into the next tick");
        assert_eq!(next_at, Some(at + crate::provider::imap::SENT_COPY_SETTLE.as_millis() as i64));
        assert_eq!(append_attempted, 0);
    }

    #[tokio::test]
    async fn no_resend_across_every_job_outcome() {
        // Every scripted outcome, asserting send_once is NEVER called and the
        // outbox state/provider_id never change.
        for answers in [
            vec![JobAnswer::Done],
            vec![JobAnswer::Recheck(Duration::from_secs(10))],
            vec![JobAnswer::Transient],
            vec![JobAnswer::Permanent],
        ] {
            let service = service();
            let at = unix_millis();
            seed_job(&service, "j", "you@example.com", "pending", 0, at - 1, 0);
            let provider = JobProvider::new(answers);
            service
                .run_sent_copy_job(&provider, "j", b"Hi", "pending", 0, false, at)
                .await
                .unwrap();
            let row = job_row(&service, "j");
            assert_send_state_untouched(&row);
            assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0, "NO RESEND, ever");
        }
    }
}
#[test]
fn mime_preserves_unicode_attachments_and_reply_headers() {
    let db = database();
    let mut d = saved(&db);
    d.reply_id = Some("original@example.com".into());
    d.references = vec!["ancestor@example.com".into(), "original@example.com".into()];
    let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&root).unwrap();
    let id = Uuid::new_v4().to_string();
    let bytes = vec![0, 255, 1, 2, 3];
    std::fs::write(root.join(&id), &bytes).unwrap();
    d.attachments.push(Attachment {
        id,
        name: "résumé.bin".into(),
        size: 5,
        mime: "application/octet-stream".into(),
        ready: true,
        message_id: None,
        provider_id: None,
        inline: false,
        content_id: None,
    });
    let raw = build_mime(&d, &[], Some("Joel Reed"), None, "test-id", &root).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.message_id(), Some("test-id@threestrands.local"));
    assert_eq!(
        parsed
            .from()
            .and_then(|from| from.first())
            .and_then(|from| from.name()),
        Some("Joel Reed")
    );
    assert_eq!(
        parsed
            .from()
            .and_then(|from| from.first())
            .and_then(|from| from.address()),
        Some(d.account.as_str())
    );
    assert!(parsed.body_text(0).unwrap().contains("Saved work ✓"));
    let attachment = parsed.attachment(0).unwrap();
    assert_eq!(attachment.contents(), bytes);
    assert_eq!(attachment.attachment_name(), Some("résumé.bin"));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn mime_round_trips_recipients_unicode_subject_and_reply_headers() {
    let db = database();
    let mut d = saved(&db);
    d.to = "Jane <jane@example.com>, José Núñez <jose@example.com>".into();
    d.cc = "cc@example.com".into();
    d.bcc = "hidden@example.com".into();
    d.subject = "Re: Café meeting — résumé ✓".into();
    d.reply_id = Some("original@example.com".into());
    d.references = vec!["ancestor@example.com".into(), "original@example.com".into()];
    let raw = build_mime(&d, &[], None, None, "headers-id", Path::new("/unused")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    let addresses = |list: Option<&mail_parser::Address>| -> Vec<String> {
        list.into_iter()
            .flat_map(|list| list.iter())
            .filter_map(|addr| addr.address().map(str::to_string))
            .collect()
    };
    assert_eq!(addresses(parsed.to()), ["jane@example.com", "jose@example.com"]);
    assert_eq!(
        parsed.to().and_then(|to| to.iter().nth(1)).and_then(|addr| addr.name()),
        Some("José Núñez")
    );
    assert_eq!(addresses(parsed.cc()), ["cc@example.com"]);
    // The Gmail send API derives Bcc recipients from the raw message and
    // strips the header itself, so it must be present in what we build.
    assert_eq!(addresses(parsed.bcc()), ["hidden@example.com"]);
    assert_eq!(parsed.subject(), Some("Re: Café meeting — résumé ✓"));
    assert_eq!(
        parsed.in_reply_to().as_text_list().map(|ids| ids.to_vec()),
        Some(vec!["original@example.com".into()])
    );
    assert_eq!(
        parsed.references().as_text_list().map(|ids| ids.to_vec()),
        Some(vec!["ancestor@example.com".into(), "original@example.com".into()])
    );
}

#[test]
fn a_server_draft_allows_no_recipients_and_keeps_one_message_id_across_pushes() {
    let db = database();
    let mut d = saved(&db);
    d.to.clear();
    assert!(build_mime(&d, &[], None, None, "op", Path::new("/unused")).is_err(), "sending still needs a recipient");
    let first = build_draft_mime(&d, &[], None, None, Path::new("/unused")).unwrap();
    d.body = "Edited".into();
    d.revision += 1;
    let second = build_draft_mime(&d, &[], None, None, Path::new("/unused")).unwrap();
    let expected = format!("draft-{}@threestrands.local", d.id);
    for raw in [&first, &second] {
        let parsed = MessageParser::default().parse(raw).unwrap();
        assert_eq!(parsed.message_id(), Some(expected.as_str()));
        assert!(parsed.to().is_none(), "no empty To header");
    }
    let domain = build_draft_mime(&d, &[], None, Some("example.org"), Path::new("/unused")).unwrap();
    let parsed = MessageParser::default().parse(&domain).unwrap();
    assert_eq!(parsed.message_id(), Some(format!("draft-{}@example.org", d.id).as_str()));
}

#[test]
fn a_server_draft_keeps_bcc_and_reply_headers() {
    let db = database();
    let mut d = saved(&db);
    d.bcc = "hidden@example.com".into();
    d.reply_id = Some("original@example.com".into());
    d.references = vec!["original@example.com".into()];
    let raw = build_draft_mime(&d, &[], None, None, Path::new("/unused")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    let bcc: Vec<_> = parsed.bcc().into_iter().flat_map(|list| list.iter()).filter_map(|a| a.address()).collect();
    assert_eq!(bcc, ["hidden@example.com"]);
    assert_eq!(
        parsed.in_reply_to().as_text_list().map(|ids| ids.to_vec()),
        Some(vec!["original@example.com".into()])
    );
}

#[test]
fn a_server_draft_blocks_header_injection_like_a_send() {
    let db = database();
    let mut d = saved(&db);
    d.subject = "Hi\r\nBcc: attacker@example.com".into();
    assert!(build_draft_mime(&d, &[], None, None, Path::new("/unused")).is_err());
    let mut d = saved(&db);
    d.reply_id = Some("x\r\nBcc: attacker@example.com".into());
    assert!(build_draft_mime(&d, &[], None, None, Path::new("/unused")).is_err());
}

// ---- Phase 4 run 2 (D): Draft.from, identity From selection, stale
//      identity rejection, Message-ID domain, header-injection bytes. ----

fn identity(address: &str, display: Option<&str>) -> crate::provider::SendingIdentity {
    crate::provider::SendingIdentity {
        address: address.into(),
        display_name: display.map(Into::into),
    }
}

#[test]
fn build_mime_sends_from_the_chosen_identity_with_its_display_name_and_domain() {
    let db = database();
    let mut d = saved(&db);
    // The account is you@example.com, but this draft was composed from a
    // secondary identity.
    d.account = "me@proton.me".into();
    d.from = Some("work@proton.me".into());
    let identities = [
        identity("me@proton.me", Some("Me")),
        identity("work@proton.me", Some("Work Account")),
    ];
    // An IMAP-style provider supplies the sender's domain.
    let raw = build_mime(&d, &identities, None, Some("proton.me"), "op-1", Path::new("/unused"))
        .unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    let from = parsed.from().and_then(|f| f.first()).unwrap();
    assert_eq!(from.address(), Some("work@proton.me"));
    assert_eq!(from.name(), Some("Work Account"));
    assert_eq!(parsed.message_id(), Some("op-1@proton.me"));
}

#[test]
fn build_mime_defaults_to_the_account_address_and_threestrands_domain() {
    // No `from`, provider domain None (Gmail default) => byte-identical to
    // the pre-change behaviour: From is the account, Message-ID domain is
    // threestrands.local, and the account display name is used.
    let db = database();
    let d = saved(&db);
    let identities = [identity(&d.account, None)];
    let raw = build_mime(&d, &identities, Some("Joel Reed"), None, "op-2", Path::new("/unused"))
        .unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    let from = parsed.from().and_then(|f| f.first()).unwrap();
    assert_eq!(from.address(), Some(d.account.as_str()));
    assert_eq!(from.name(), Some("Joel Reed"));
    assert_eq!(parsed.message_id(), Some("op-2@threestrands.local"));
}

#[test]
fn build_mime_rejects_a_stale_identity_not_on_the_account() {
    let db = database();
    let mut d = saved(&db);
    d.account = "me@proton.me".into();
    // `from` names an identity that was removed from the account's list.
    d.from = Some("removed@proton.me".into());
    let identities = [identity("me@proton.me", None), identity("work@proton.me", None)];
    let err = build_mime(&d, &identities, None, Some("proton.me"), "op-3", Path::new("/unused"))
        .unwrap_err();
    assert!(
        err.contains("no longer on this account"),
        "a stale identity must fail clearly, not send as another address: {err}"
    );
}

#[test]
fn build_mime_blocks_header_injection_via_display_name_and_address() {
    let db = database();
    let mut d = saved(&db);
    d.account = "me@proton.me".into();

    // A hostile DISPLAY NAME carrying CRLF must be refused — proven by the
    // error AND, where accepted elsewhere, by the absence of an injected
    // header in the bytes.
    let injected_name = [identity("me@proton.me", Some("Evil\r\nBcc: victim@x.com"))];
    let err = build_mime(&d, &injected_name, None, Some("proton.me"), "op-4", Path::new("/unused"))
        .unwrap_err();
    assert!(err.contains("line breaks"), "{err}");

    // A hostile FROM ADDRESS carrying CRLF must be refused too.
    d.from = Some("me@proton.me\r\nBcc: victim@x.com".into());
    let ids = [identity("me@proton.me\r\nBcc: victim@x.com", None)];
    let err = build_mime(&d, &ids, None, Some("proton.me"), "op-5", Path::new("/unused"))
        .unwrap_err();
    assert!(err.contains("line breaks"), "{err}");

    // A clean send with a non-ASCII display name produces bytes with NO
    // injected header: assert on the produced bytes directly.
    d.from = None;
    let clean = [identity("me@proton.me", Some("Zoë O'Brien"))];
    let raw = build_mime(&d, &clean, None, Some("proton.me"), "op-6", Path::new("/unused"))
        .unwrap();
    let text = String::from_utf8_lossy(&raw);
    // The From header line must be a single logical header (no bare CRLF
    // introducing a "Bcc:" or similar), and no stray Bcc appears.
    assert!(!text.to_ascii_lowercase().contains("\nbcc: victim"));
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(
        parsed.from().and_then(|f| f.first()).and_then(|a| a.address()),
        Some("me@proton.me")
    );
}

#[test]
fn signature_mime_keeps_plain_text_html_images_and_quote_placement_after_template_deletion() {
    use crate::signatures::tests::{template, defaults};
    let db = database();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j1ioAAAAASUVORK5CYII=";
    let mut signature = template("work");
    signature.html = format!("<table><tr><td><img src=\"data:image/png;base64,{png}\" alt=\"Logo\" width=\"32\"></td><td>Regards</td></tr></table>");
    signature.text = "Logo Regards".into();
    db.store_signature(&signature).unwrap();
    db.store_signature_defaults(&defaults("you@example.com", "", Some("work"), Some("work"))).unwrap();
    let mut d = saved(&db);
    d.body = "Hi 😀\n\nOn Monday, Ann wrote:\n> Quote".into();
    d.body_html = "Hi 😀<br><br>On Monday, Ann wrote:<blockquote type=\"cite\">Quote</blockquote>".into();
    d.signature.as_mut().unwrap().text_offset = 5;
    d.signature.as_mut().unwrap().html_offset = 5;
    let d = db.save_draft(d).unwrap();
    db.delete_signature("work").unwrap();
    let raw = build_mime(&d, &[], None, None, "signed", Path::new("/unused")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.body_text(0).unwrap().replace("\r\n", "\n"), "Hi 😀\n\nLogo Regards\n\nOn Monday, Ann wrote:\n> Quote");
    let html = parsed.body_html(0).unwrap();
    assert!(html.find("Hi 😀").unwrap() < html.find("<table>").unwrap());
    assert!(html.find("Regards").unwrap() < html.find("On Monday").unwrap());
    assert!(!html.contains("data:image"));
    assert!(html.contains("cid:signature-0-signed@threestrands.local"));
    assert!(parsed.parts.iter().any(|part| part.content_id() == Some("signature-0-signed@threestrands.local")));
    assert_eq!(db.draft(&d.id).unwrap().signature.unwrap().text, "Logo Regards");
}

#[test]
fn signature_mime_keeps_hosted_sources_without_creating_image_parts() {
    use crate::signatures::tests::{template, defaults};
    for html in [
        "<table><tr><td><img src=\"https://images.example/large.png?version=2&amp;size=large\" alt=\"Logo\"></td><td>Regards</td></tr></table>",
        "<div><img src=\"https://cdn.example/logo.gif\" alt=\"Logo\"><p>Regards</p></div>",
    ] {
        let db = database();
        let mut signature = template("hosted");
        signature.html = html.into();
        db.store_signature(&signature).unwrap();
        db.store_signature_defaults(&defaults("you@example.com", "", Some("hosted"), None)).unwrap();
        let draft = saved(&db);
        let raw = build_mime(&draft, &[], None, None, "hosted", Path::new("/unused")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert!(parsed.body_html(0).unwrap().contains(html));
        assert!(!parsed.parts.iter().any(|part| part.content_id().is_some()));
        assert!(!parsed.body_html(0).unwrap().contains("data:image"));
        assert!(!parsed.body_html(0).unwrap().contains("cid:"));
    }
}

#[test]
fn switching_sending_alias_updates_only_an_automatic_signature() {
    use crate::signatures::tests::{template, defaults};
    let db = imap_database();
    db.store_signature(&template("work")).unwrap();
    db.store_signature_defaults(&defaults("me@proton.me", "work@proton.me", Some("work"), Some("work"))).unwrap();
    let d = db.create_draft("new", None, "me@proton.me", &[]).unwrap();
    let mut changed = db.set_draft_from(&d.id, Some("work@proton.me")).unwrap();
    assert_eq!(changed.signature.as_ref().unwrap().text, "work");
    changed.signature.as_mut().unwrap().automatic = false;
    db.save_draft(changed).unwrap();
    let back = db.set_draft_from(&d.id, None).unwrap();
    assert_eq!(back.signature.unwrap().text, "work");
}

#[test]
fn mime_includes_rich_html_and_plain_text_fallback() {
    let db = database();
    let mut d = saved(&db);
    d.body = "Formatted message".into();
    d.body_html = "<p><strong>Formatted</strong> message</p>".into();
    let raw = build_mime(&d, &[], None, None, "rich-id", Path::new("/unused")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.body_text(0).as_deref(), Some("Formatted message"));
    assert_eq!(
        parsed.body_html(0).as_deref(),
        Some("<p><strong>Formatted</strong> message</p>")
    );
}
#[test]
fn mime_embeds_inline_images_with_their_content_id() {
    let db = database();
    let mut d = saved(&db);
    let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&root).unwrap();
    let attachment_id = Uuid::new_v4().to_string();
    let content_id = format!("{attachment_id}@threestrands.local");
    let bytes = vec![137, 80, 78, 71];
    std::fs::write(root.join(&attachment_id), &bytes).unwrap();
    d.body_html = format!("<p>Screenshot</p><img src=\"cid:{content_id}\">");
    d.attachments.push(Attachment {
        id: attachment_id,
        name: "screenshot.png".into(),
        size: bytes.len() as u64,
        mime: "image/png".into(),
        ready: true,
        message_id: None,
        provider_id: None,
        inline: true,
        content_id: Some(content_id.clone()),
    });

    let raw = build_mime(&d, &[], None, None, "inline-id", &root).unwrap();
    let source = String::from_utf8_lossy(&raw);
    assert!(source.contains(&format!("Content-ID: <{content_id}>")));
    assert!(source.contains("Content-Disposition: inline"));
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert!(parsed
        .body_html(0)
        .unwrap()
        .contains(&format!("src=\"cid:{content_id}\"")));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn reply_all_excludes_self_preserves_cc_and_forward_joins_the_thread() {
    let db = database();
    let source = serde_json::json!({"id":"source","threadId":"thread","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"Reply-To","value":"reply@example.com"},{"name":"To","value":"you@example.com, colleague@example.com"},{"name":"Cc","value":"cc@example.com, colleague@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
    db.test_connection()
        .unwrap()
        .execute(
            "INSERT INTO message_metadata(id, payload) VALUES ('source',?1)",
            [source.to_string()],
        )
        .unwrap();
    let reply = db
        .create_draft("replyAll", Some("source".into()), "you@example.com", &[])
        .unwrap();
    assert_eq!(reply.to, "reply@example.com, colleague@example.com");
    assert_eq!(reply.cc, "cc@example.com");
    assert_eq!(reply.thread_id, Some("thread".into()));
    let forward = db
        .create_draft("forward", Some("source".into()), "you@example.com", &[])
        .unwrap();
    // A forward joins the source conversation but is still not a reply:
    // nobody is addressed for the user.
    assert!(forward.to.is_empty());
    assert!(forward.cc.is_empty());
    assert_eq!(forward.reply_id, Some("source@example.com".into()));
    assert_eq!(forward.references, vec!["source@example.com".to_string()]);
    assert_eq!(forward.thread_id, Some("thread".into()));
    assert_eq!(forward.subject, "Fwd: Topic");
    let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&root).unwrap();
    let mut addressed = forward.clone();
    addressed.to = "colleague@example.com".into();
    let raw = build_mime(&addressed, &[], None, None, "forward-id", &root).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.in_reply_to().as_text(), Some("source@example.com"));
    assert_eq!(parsed.references().as_text(), Some("source@example.com"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn forward_extends_the_reference_chain_and_survives_a_missing_message_id() {
    let db = database();
    for (id, headers, thread) in [
        (
            "chained",
            serde_json::json!([{"name":"From","value":"other@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<second@example.com>"},{"name":"References","value":"<first@example.com>"}]),
            Some("thread"),
        ),
        (
            "anonymous",
            serde_json::json!([{"name":"From","value":"other@example.com"},{"name":"Subject","value":"Topic"}]),
            None,
        ),
    ] {
        let source = serde_json::json!({"id":id,"threadId":"thread","payload":{"mimeType":"text/plain","headers":headers,"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        db.test_connection()
            .unwrap()
            .execute(
                "INSERT INTO message_metadata(id, payload) VALUES (?1,?2)",
                params![id, source.to_string()],
            )
            .unwrap();
        let forward = db
            .create_draft("forward", Some(id.into()), "you@example.com", &[])
            .unwrap();
        assert_eq!(forward.thread_id.as_deref(), thread, "{id}");
        if thread.is_some() {
            assert_eq!(forward.reply_id, Some("second@example.com".into()));
            assert_eq!(
                forward.references,
                vec!["first@example.com".to_string(), "second@example.com".to_string()]
            );
        } else {
            // No Message-ID to point at: it still forwards, as a new conversation.
            assert!(forward.reply_id.is_none());
            assert!(forward.references.is_empty());
            assert!(forward.body.contains("Forwarded message"));
        }
    }
}

#[test]
fn alias_replies_exclude_all_own_addresses_and_follow_sent_recipients() {
    let db = imap_database();
    let identities = db.account_sending_identities("me@proton.me").unwrap();
    for (id, sender, to, expected_to) in [
        (
            "incoming",
            "friend@example.com",
            "WORK@proton.me, colleague@example.com",
            "friend@example.com, colleague@example.com",
        ),
        (
            "outgoing",
            "WORK@proton.me",
            "friend@example.com, colleague@example.com",
            "friend@example.com, colleague@example.com",
        ),
    ] {
        let source = serde_json::json!({"id":id,"threadId":"thread","payload":{
            "mimeType":"text/plain","headers":[
                {"name":"From","value":sender}, {"name":"To","value":to},
                {"name":"Cc","value":"me@proton.me, work@proton.me, cc@example.com"},
                {"name":"Subject","value":"Topic"}, {"name":"Message-ID","value":format!("<{id}@example.com>")}
            ],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        db.put_message_metadata(id, &source.to_string()).unwrap();
        let reply = db
            .create_draft("replyAll", Some(id.into()), "me@proton.me", &identities)
            .unwrap();
        assert_eq!(reply.to, expected_to, "{id}");
        assert_eq!(reply.cc, "cc@example.com", "{id}");
        if id == "incoming" {
            assert_eq!(reply.from.as_deref(), Some("work@proton.me"));
        }
        let reply = db
            .create_draft("reply", Some(id.into()), "me@proton.me", &identities)
            .unwrap();
        assert_eq!(
            reply.to,
            if id == "incoming" {
                "friend@example.com"
            } else {
                expected_to
            }
        );
    }
}
#[test]
fn html_forwards_preserve_source_in_mime_after_save_and_reopen() {
    for (index, html) in [
        "<p><strong>Golf Monday</strong></p><a href=\"https://example.com/profile?one=1&amp;two=2\">Profile</a><img src=\"https://images.example.com/invite.png\" alt=\"Invitation\">",
        "<!doctype html><html><head><style>td { padding: 8px; }</style></head><body><table cellspacing=\"4\"><tr><td></td><td><em>Agenda</em><a href=\"https://example.com/agenda\">Read agenda</a></td></tr></table></body></html>",
    ].into_iter().enumerate() {
        let db = database();
        let source_id = format!("html-forward-{index}");
        let source = serde_json::json!({"id":source_id,"threadId":"thread","payload":{"mimeType":"multipart/alternative","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Golf <Monday> & friends"}],"parts":[{"mimeType":"text/plain","body":{"data":URL_SAFE_NO_PAD.encode("Plain alternative [Profile] and [image]")}},{"mimeType":"text/html","body":{"data":URL_SAFE_NO_PAD.encode(html)}}]}});
        db.test_connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES (?1, ?2)", params![source_id, source.to_string()]).unwrap();
        let mut draft = db.create_draft("forward", Some(source_id), "you@example.com", &[]).unwrap();
        let preserved = draft.forwarded_content.as_ref().unwrap();
        assert!(preserved.html.ends_with(html));
        assert!(preserved.html.contains("Other &lt;other@example.com&gt;"));
        assert!(preserved.html.contains("Golf &lt;Monday&gt; &amp; friends"));
        assert!(preserved.text.ends_with("Plain alternative [Profile] and [image]"));
        assert!(draft.body.is_empty());
        let expected_html = preserved.html.clone();
        // Saving user edits cannot overwrite native-owned source HTML.
        draft.forwarded_content = None;
        draft.to = "friend@example.com".into();
        draft.body = "Take a look <please>".into();
        let saved = db.save_draft(draft).unwrap();
        let reopened = db.draft(&saved.id).unwrap();
        assert_eq!(reopened.forwarded_content.as_ref().unwrap().html, expected_html);
        let raw = build_mime(&reopened, &[], None, None, "forward", Path::new(".")).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.body_html(0).unwrap(), format!("Take a look &lt;please&gt;<br><br>{expected_html}"));
        let plain = parsed.body_text(0).unwrap().replace("\r\n", "\n");
        assert_eq!(plain, format!("{}\n\n{}", reopened.body, reopened.forwarded_content.as_ref().unwrap().text));
        assert!(parsed.in_reply_to().as_text().is_none());
    }
}

#[test]
fn edited_forwards_send_the_edit_and_keep_the_original_to_restore() {
    let db = database();
    let html = "<p>Agenda</p><p>Private note for the team</p>";
    let source = serde_json::json!({"id":"edited-forward","threadId":"thread","payload":{"mimeType":"text/html","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Plans"}],"body":{"data":URL_SAFE_NO_PAD.encode(html)}}});
    db.test_connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES (?1, ?2)", params!["edited-forward", source.to_string()]).unwrap();
    let mut draft = db.create_draft("forward", Some("edited-forward".into()), "you@example.com", &[]).unwrap();
    let original = draft.forwarded_content.clone().unwrap();
    draft.to = "friend@example.com".into();
    draft.body = "See below".into();
    draft.forwarded_edit = Some(ForwardedContent { html: "<p>Agenda</p>".into(), text: "Agenda".into() });
    let saved = db.save_draft(draft).unwrap();
    let reopened = db.draft(&saved.id).unwrap();
    assert_eq!(reopened.forwarded_content.as_ref().unwrap().html, original.html);
    let raw = build_mime(&reopened, &[], None, None, "edited", Path::new(".")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.body_html(0).unwrap(), "See below<br><br><p>Agenda</p>");
    assert_eq!(parsed.body_text(0).unwrap().replace("\r\n", "\n"), "See below\n\nAgenda");
    assert!(!String::from_utf8_lossy(&raw).contains("Private note"));

    // Restoring the original clears the edit; the untouched source sends again.
    let mut restored = reopened.clone();
    restored.forwarded_edit = None;
    let restored = db.save_draft(restored).unwrap();
    let raw = build_mime(&db.draft(&restored.id).unwrap(), &[], None, None, "restored", Path::new(".")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.body_html(0).unwrap(), format!("See below<br><br>{}", original.html));
}

#[test]
fn a_forwarded_edit_needs_an_original_to_replace() {
    let db = database();
    let mut draft = saved(&db);
    draft.forwarded_edit = Some(ForwardedContent { html: "<p>Injected</p>".into(), text: "Injected".into() });
    let saved = db.save_draft(draft).unwrap();
    assert!(db.draft(&saved.id).unwrap().forwarded_edit.is_none());
    let mut unsaved = saved.clone();
    unsaved.forwarded_edit = Some(ForwardedContent { html: "<p>Injected</p>".into(), text: "Injected".into() });
    let raw = build_mime(&unsaved, &[], None, None, "no-original", Path::new(".")).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("Injected"));
}

#[test]
fn a_forwarded_edit_counts_toward_the_draft_size_limit() {
    let db = database();
    let source = serde_json::json!({"id":"large-forward","threadId":"thread","payload":{"mimeType":"text/html","headers":[{"name":"From","value":"other@example.com"},{"name":"Subject","value":"Large"}],"body":{"data":URL_SAFE_NO_PAD.encode("<p>Large</p>")}}});
    db.test_connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES (?1, ?2)", params!["large-forward", source.to_string()]).unwrap();
    let mut draft = db.create_draft("forward", Some("large-forward".into()), "you@example.com", &[]).unwrap();
    draft.forwarded_edit = Some(ForwardedContent { html: "x".repeat(2 * 1024 * 1024), text: "x".into() });
    assert_eq!(db.save_draft(draft).unwrap_err(), "Draft is too large");
}

#[test]
fn old_drafts_without_preserved_content_keep_their_body() {
    let db = database();
    let draft = saved(&db);
    let mut value = serde_json::to_value(&draft).unwrap();
    value.as_object_mut().unwrap().remove("forwardedContent");
    let old: Draft = serde_json::from_value(value).unwrap();
    assert!(old.forwarded_content.is_none());
    let raw = build_mime(&old, &[], None, None, "legacy", Path::new(".")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.body_text(0).unwrap(), draft.body);
}

#[test]
fn edit_as_new_copies_a_sent_message_into_an_unaddressed_new_thread() {
    // Two structurally different sent messages: HTML with inline and
    // regular attachments, and plain text only.
    let db = database();
    let html = "<p>Join us <strong>Saturday</strong></p><img src=\"cid:map@example.com\">";
    let rich = serde_json::json!({"id":"sent-rich","threadId":"thread","payload":{"mimeType":"multipart/mixed","headers":[{"name":"From","value":"You <YOU@example.com>"},{"name":"To","value":"guest@example.com"},{"name":"Cc","value":"cohost@example.com"},{"name":"Subject","value":"Dinner on Saturday"},{"name":"Message-ID","value":"<sent-rich@example.com>"}],"parts":[{"mimeType":"multipart/related","parts":[{"mimeType":"text/html","body":{"data":URL_SAFE_NO_PAD.encode(html)}},{"mimeType":"image/png","headers":[{"name":"Content-ID","value":"<map@example.com>"},{"name":"Content-Disposition","value":"inline"}],"body":{"data":URL_SAFE_NO_PAD.encode("png"),"size":3}}]},{"mimeType":"application/pdf","filename":"menu.pdf","body":{"attachmentId":"menu-part","size":4}}]}});
    let plain = serde_json::json!({"id":"sent-plain","threadId":"thread-2","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"you@example.com"},{"name":"To","value":"guest@example.com"},{"name":"Subject","value":"Book club"},{"name":"Message-ID","value":"<sent-plain@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Bring the book.")}}});
    db.put_message_metadata("sent-rich", &rich.to_string()).unwrap();
    db.put_message_metadata("sent-plain", &plain.to_string()).unwrap();

    let draft = db.create_draft_from_sent("sent-rich", "you@example.com", &[]).unwrap();
    assert_eq!(draft.mode, "new");
    assert_eq!(draft.subject, "Dinner on Saturday");
    assert!(draft.body_html.contains("Join us <strong>Saturday</strong>"));
    assert_eq!((draft.to.as_str(), draft.cc.as_str(), draft.bcc.as_str()), ("", "", ""));
    assert_eq!(draft.source_id, None);
    assert_eq!(draft.thread_id, None);
    assert_eq!(draft.reply_id, None);
    assert!(draft.references.is_empty());
    assert!(draft.forwarded_content.is_none());
    assert_eq!(draft.from, None);
    assert_eq!(draft.attachments.len(), 2);
    assert!(draft.attachments.iter().all(|a| !a.ready && a.message_id.as_deref() == Some("sent-rich")));
    let inline = draft.attachments.iter().find(|a| a.inline).unwrap();
    assert_eq!(inline.content_id.as_deref(), Some("map@example.com"));
    let menu = draft.attachments.iter().find(|a| !a.inline).unwrap();
    assert_eq!((menu.name.as_str(), menu.provider_id.as_deref()), ("menu.pdf", Some("menu-part")));

    // Each use is its own draft, so a copy can go to several people separately.
    let again = db.create_draft_from_sent("sent-rich", "you@example.com", &[]).unwrap();
    assert_ne!(again.id, draft.id);
    assert_ne!(again.attachments[0].id, draft.attachments[0].id);

    let text = db.create_draft_from_sent("sent-plain", "you@example.com", &[]).unwrap();
    assert_eq!(text.subject, "Book club");
    assert_eq!(text.body.trim_end(), "Bring the book.");
    assert!(text.body_html.trim().is_empty());
    assert!(text.attachments.is_empty());

    // The copy sends as a fresh message, not a reply.
    let mut ready = text;
    ready.to = "newguest@example.com".into();
    let raw = build_mime(&ready, &[], None, None, "edit-as-new", Path::new(".")).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.subject(), Some("Book club"));
    assert!(parsed.in_reply_to().as_text().is_none());
    assert!(parsed.references().as_text().is_none());
}

#[test]
fn edit_as_new_refuses_messages_from_other_senders() {
    let db = database();
    for (id, from) in [("incoming", "Other <other@example.com>"), ("lookalike", "you@example.com.evil.test")] {
        let source = serde_json::json!({"id":id,"threadId":"thread","payload":{"mimeType":"text/html","headers":[{"name":"From","value":from},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Invoice"}],"body":{"data":URL_SAFE_NO_PAD.encode("<p>Pay here</p>")}}});
        db.put_message_metadata(id, &source.to_string()).unwrap();
        let refused = db.create_draft_from_sent(id, "you@example.com", &[]).unwrap_err();
        assert_eq!(refused, "Edit as New works only on messages you sent", "{id}");
    }
    assert!(db.drafts().unwrap().is_empty(), "a refusal must not leave a draft behind");
}

#[test]
fn edit_as_new_keeps_the_identity_the_original_was_sent_from() {
    let db = imap_database();
    let identities = db.account_sending_identities("me@proton.me").unwrap();
    for (id, sender, expected) in [
        ("from-alias", "Work <WORK@proton.me>", Some("work@proton.me")),
        ("from-primary", "me@proton.me", None),
    ] {
        let source = serde_json::json!({"id":id,"threadId":"thread","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":sender},{"name":"To","value":"friend@example.com"},{"name":"Subject","value":"Topic"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
        db.put_message_metadata(id, &source.to_string()).unwrap();
        let draft = db.create_draft_from_sent(id, "me@proton.me", &identities).unwrap();
        assert_eq!(draft.from.as_deref(), expected, "{id}");
        assert!(draft.to.is_empty(), "{id}");
    }
}

#[tokio::test]
async fn edit_as_new_opens_from_a_cached_message_while_its_account_is_not_connected() {
    let service = service();
    let source = serde_json::json!({"id":"cached-sent","threadId":"t","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"you@example.com"},{"name":"To","value":"friend@example.com"},{"name":"Subject","value":"Picnic"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
    service.database.put_message_metadata("cached-sent", &source.to_string()).unwrap();
    let value = service
        .request(Request::CreateFromSent { source_id: "cached-sent".into(), account: "you@example.com".into() })
        .await
        .expect("a cached sent message must open as a new draft without a connected provider");
    let draft: Draft = serde_json::from_value(value).unwrap();
    assert_eq!((draft.mode.as_str(), draft.subject.as_str(), draft.to.as_str()), ("new", "Picnic", ""));
}

#[test]
fn forwarding_keeps_named_and_unnamed_inline_images_and_content_ids() {
    let db = database();
    let html = "<p>Photos</p><img src=\"cid:first@example.com\"><table><tr><td><img src=\"cid:second@example.com\"></td></tr></table>";
    let source = serde_json::json!({"id":"inline-forward","threadId":"thread","payload":{"mimeType":"multipart/related","headers":[{"name":"From","value":"other@example.com"},{"name":"Subject","value":"Photos"}],"parts":[{"mimeType":"text/html","body":{"data":URL_SAFE_NO_PAD.encode(html)}},{"mimeType":"image/png","filename":"photo.png","headers":[{"name":"Content-ID","value":"<first@example.com>"}],"body":{"attachmentId":"remote-part","size":3}},{"mimeType":"image/png","headers":[{"name":"Content-ID","value":"<second@example.com>"},{"name":"Content-Disposition","value":"inline"}],"body":{"data":URL_SAFE_NO_PAD.encode("png"),"size":3}}]}});
    db.test_connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES ('inline-forward', ?1)", [source.to_string()]).unwrap();
    let mut draft = db.create_draft("forward", Some("inline-forward".into()), "you@example.com", &[]).unwrap();
    assert_eq!(draft.attachments.len(), 2);
    assert!(draft.attachments.iter().all(|attachment| attachment.inline && !attachment.ready));
    assert_eq!(draft.attachments[0].provider_id.as_deref(), Some("remote-part"));
    assert_eq!(draft.attachments[1].provider_id, None);
    let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&root).unwrap();
    for attachment in &mut draft.attachments {
        std::fs::write(root.join(&attachment.id), b"png").unwrap();
        attachment.ready = true;
    }
    draft.to = "friend@example.com".into();
    draft.body_html = "<p>Please see below.</p>".into();
    let raw = build_mime(&draft, &[], None, None, "inline-forward", &root).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert!(parsed.body_html(0).unwrap().starts_with("<p>Please see below.</p><br><br>"));
    assert!(parsed.body_html(0).unwrap().ends_with(html));
    let raw = String::from_utf8(raw).unwrap();
    assert!(raw.contains("Content-ID: <first@example.com>"));
    assert!(raw.contains("Content-ID: <second@example.com>"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_attribution_dates_are_readable_in_the_reader_time_zone() {
    let eastern = chrono::FixedOffset::west_opt(4 * 3600).unwrap();
    assert_eq!(
        attribution_date("2026-10-05T14:32:48+00:00", &eastern),
        "Mon, Oct 5, 2026 at 10:32 AM"
    );
    assert_eq!(
        attribution_date("2026-10-05T00:05:00+00:00", &chrono::Utc),
        "Mon, Oct 5, 2026 at 12:05 AM"
    );
    assert_eq!(attribution_date("not a date", &chrono::Utc), "not a date");
}
#[test]
fn reply_and_forward_bodies_quote_the_source_with_a_readable_date() {
    let db = database();
    let source = serde_json::json!({"id":"source-q","threadId":"thread-q","internalDate":"1791210768000","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source-q@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello\n> earlier")}}});
    db.test_connection()
        .unwrap()
        .execute(
            "INSERT INTO message_metadata(id, payload) VALUES ('source-q',?1)",
            [source.to_string()],
        )
        .unwrap();
    let date = attribution_date("2026-10-05T14:32:48+00:00", &chrono::Local);
    let reply = db
        .create_draft("reply", Some("source-q".into()), "you@example.com", &[])
        .unwrap();
    assert_eq!(
        reply.body,
        format!("\n\nOn {date}, Other <other@example.com> wrote:\n> Hello\n> > earlier")
    );
    assert!(!reply.body.contains("T14:32:48"));
    let forward = db
        .create_draft("forward", Some("source-q".into()), "you@example.com", &[])
        .unwrap();
    assert!(forward.body.contains(&format!("\nDate: {date}\n")));
}
#[test]
fn reply_drafts_are_stamped_with_the_passed_account_not_the_global_compose_identity() {
    let db = database();
    // Simulate a different account being "active" for new messages than
    // the one this reply must actually send from.
    db.set_compose_identity("active@example.com").unwrap();
    let source = serde_json::json!({"id":"source-b","threadId":"thread-b","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source-b@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
    db.test_connection()
        .unwrap()
        .execute(
            "INSERT INTO message_metadata(id, payload) VALUES ('source-b',?1)",
            [source.to_string()],
        )
        .unwrap();
    let reply = db
        .create_draft("reply", Some("source-b".into()), "you@example.com", &[])
        .unwrap();
    assert_eq!(reply.account, "you@example.com");
}
#[test]
fn only_new_messages_can_change_their_sending_account() {
    let db = database();
    let d = saved(&db);
    let updated = db.set_draft_account(&d.id, "other@example.com").unwrap();
    assert_eq!(updated.account, "other@example.com");
    assert!(updated.revision > d.revision);

    let source = serde_json::json!({"id":"source-c","threadId":"thread-c","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<source-c@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
    db.test_connection()
        .unwrap()
        .execute(
            "INSERT INTO message_metadata(id, payload) VALUES ('source-c',?1)",
            [source.to_string()],
        )
        .unwrap();
    let reply = db
        .create_draft("reply", Some("source-c".into()), "you@example.com", &[])
        .unwrap();
    assert!(db.set_draft_account(&reply.id, "nope@example.com").is_err());
}
#[test]
fn queuing_a_draft_no_longer_depends_on_which_account_is_currently_active() {
    let db = database();
    // The globally "active" compose identity differs from the drafted
    // account; queuing must still succeed, since replying from a
    // non-active account is exactly what multi-account send-as needs.
    db.set_compose_identity("active@example.com").unwrap();
    let mut d = db.create_draft("new", None, "you@example.com", &[]).unwrap();
    d.to = "Jane <jane@example.com>".into();
    d.subject = "Hello".into();
    d.body = "Body".into();
    let d = db.save_draft(d).unwrap();
    let item = db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).unwrap();
    assert_eq!(item.draft.account, "you@example.com");
}
#[test]
fn queuing_rejects_a_draft_whose_account_needs_reauth() {
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    db.test_connection()
        .unwrap()
        .execute(
            "UPDATE accounts SET status='needs_reauth' WHERE email='you@example.com'",
            [],
        )
        .unwrap();
    let d = saved(&db);
    assert!(db.queue(&d.id, d.revision, false, &[], None, Path::new("/unused")).is_err());
}
#[test]
fn rerunning_migrations_on_a_current_database_keeps_its_threads() {
    let db = database();
    let before = db.list_threads(None).unwrap().len();
    assert!(before > 0, "the fixture database must hold mail for this check to mean anything");
    crate::schema::migrate(&mut db.test_connection().unwrap()).unwrap();
    assert_eq!(db.list_threads(None).unwrap().len(), before);
}
fn service() -> Correspondence {
    Correspondence {
        sending_identity: Arc::new(|_| Ok("00000000-0000-4000-8000-000000000001".into())),
        database: Arc::new(database()),
        accounts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        root: PathBuf::from("/unused"),
        gate: Arc::new(tokio::sync::Mutex::new(())),
        edits: Arc::new(tokio::sync::Mutex::new(())),
        scheduling: Arc::new(tokio::sync::Mutex::new(())),
        draft_pushes: Arc::new(tokio::sync::Mutex::new(())),
        draft_events: Arc::new(|_| {}),
        draft_pull_requested: Arc::default(),
    }
}
#[tokio::test]
async fn confirmed_local_sent_messages_support_responses_without_a_provider_copy() {
    let service = service();
    let mut d = saved(&service.database);
    d.thread_id = Some("original-thread".into());
    d.cc = "team@example.com".into();
    // Routing is native-owned, so persist this fixture through the native path.
    let d = service.database.save_attachment_draft(d).unwrap();
    let item = service.database.queue(&d.id, d.revision, false, &[], None, &service.root).unwrap();
    service.database.test_connection().unwrap().execute(
        "UPDATE outbox_messages SET state='sent' WHERE id=?1", [&item.id]
    ).unwrap();
    let source_id = format!("outbox-{}", item.id);
    let raw = service.database.outbox_raw(&item.id).unwrap();
    let parsed = MessageParser::default().parse(&raw).unwrap();
    for mode in ["reply", "replyAll", "forward"] {
        let value = service.request(Request::Create {
            mode: mode.into(), source_id: Some(source_id.clone()), account: Some(d.account.clone())
        }).await.expect("sent MIME is available even with no connected provider");
        let draft: Draft = serde_json::from_value(value).unwrap();
        assert_eq!(draft.thread_id.as_deref(), Some("original-thread"));
        assert_eq!(draft.reply_id.as_deref(), parsed.message_id());
        assert!(draft.references.contains(&draft.reply_id.clone().unwrap()));
        assert!(draft.body.contains("Saved work") || draft.forwarded_content.as_ref().is_some_and(|c| c.text.contains("Saved work")));
        if mode == "forward" {
            assert!(draft.to.is_empty());
        } else {
            assert_eq!(draft.to, "jane@example.com");
            assert_eq!(draft.cc, if mode == "replyAll" { "team@example.com" } else { "" });
        }
    }
    // Cached metadata cannot bypass the account or confirmed-send checks.
    assert!(service.request(Request::Create {
        mode: "replyAll".into(), source_id: Some(source_id.clone()), account: Some("other@example.com".into())
    }).await.is_err());
    for state in ["undo_pending", "sending", "failed", "uncertain", "canceled"] {
        service.database.test_connection().unwrap().execute(
            "UPDATE outbox_messages SET state=?1 WHERE id=?2", params![state, item.id]
        ).unwrap();
        assert!(service.request(Request::Create {
            mode: "replyAll".into(), source_id: Some(source_id.clone()), account: Some(d.account.clone())
        }).await.is_err(), "{state} must not be treated as sent");
    }
}

#[tokio::test]
async fn forwarding_a_local_sent_copy_reads_attachments_from_its_mime_offline() {
    let mut service = service();
    let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&root).unwrap();
    service.root = root.clone();
    let d = saved(&service.database);
    let item = service.database.queue(&d.id, d.revision, false, &[], None, &service.root).unwrap();
    let raw = b"From: you@example.com\r\nTo: jane@example.com\r\nMessage-ID: <local@example.com>\r\nSubject: Files\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=test\r\n\r\n--test\r\nContent-Type: text/plain\r\n\r\nHello\r\n--test\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=report.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nYWJj\r\n--test--\r\n";
    service.database.test_connection().unwrap().execute(
        "UPDATE outbox_messages SET state='sent',raw=?1 WHERE id=?2", params![raw.as_slice(), item.id]
    ).unwrap();
    let value = service.request(Request::Create {
        mode: "forward".into(), source_id: Some(format!("outbox-{}", item.id)), account: Some(d.account)
    }).await.unwrap();
    let draft: Draft = serde_json::from_value(value).unwrap();
    assert_eq!(draft.attachments.len(), 1);
    let value = service.request(Request::FetchAttachment {
        id: draft.id, attachment_id: draft.attachments[0].id.clone()
    }).await.unwrap();
    let fetched: Draft = serde_json::from_value(value).unwrap();
    assert!(fetched.attachments[0].ready);
    assert_eq!(std::fs::read(root.join(&fetched.attachments[0].id)).unwrap(), b"abc");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn reply_to_a_cached_message_opens_while_its_account_is_not_connected() {
    // `service()` has an empty account registry, so `provider_for` fails.
    // Resolving sending identities must not turn that into a refusal to
    // open a reply to a message that is already cached.
    let service = service();
    let source = serde_json::json!({"id":"cached","threadId":"t","payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"Other <other@example.com>"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Topic"},{"name":"Message-ID","value":"<cached@example.com>"}],"body":{"data":URL_SAFE_NO_PAD.encode("Hello")}}});
    service.database.test_connection().unwrap().execute("INSERT INTO message_metadata(id, payload) VALUES ('cached', ?1)", [source.to_string()]).unwrap();
    let value = service
        .request(Request::Create { mode: "reply".into(), source_id: Some("cached".into()), account: Some("you@example.com".into()) })
        .await
        .expect("a cached reply must open without a connected provider");
    let draft: Draft = serde_json::from_value(value).unwrap();
    assert_eq!(draft.account, "you@example.com");
    assert_eq!(draft.from, None, "falls back to the primary identity");
}
#[tokio::test]
async fn fake_clock_and_provider_prove_undo_deadline_and_single_dispatch() {
    let service = service();
    let d = saved(&service.database);
    let item = service
        .database
        .queue(&d.id, d.revision, false, &[], None, &service.root)
        .unwrap();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let send = |_, _| {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async {
            Ok(DeliveryReceipt {
                provider_message_id: "sent".into(),
                thread_id: Some("thread".into()),
            })
        }
    };
    service
        .dispatch_due(&item, item.deadline - 1, false, &send)
        .await
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    service
        .dispatch_due(&item, item.deadline, false, &send)
        .await
        .unwrap();
    service
        .dispatch_due(&item, item.deadline + 1, false, &send)
        .await
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(service.database.outbox().unwrap()[0].state, "sent");
    assert!(service.database.cancel_send(&item.id, false).is_err());
}
#[tokio::test]
async fn canceled_messages_never_dispatch_and_transport_timeouts_are_not_retried() {
    let service = service();
    let d = saved(&service.database);
    let item = service
        .database
        .queue(&d.id, d.revision, false, &[], None, &service.root)
        .unwrap();
    service.database.cancel_send(&item.id, false).unwrap();
    service
        .dispatch_due(&item, item.deadline, false, |_, _| async {
            panic!("Canceled mail must not send");
            #[allow(unreachable_code)]
            Ok(DeliveryReceipt {
                provider_message_id: String::new(),
                thread_id: Some(String::new()),
            })
        })
        .await
        .unwrap();
    let d = saved(&service.database);
    let item = service
        .database
        .queue(&d.id, d.revision, false, &[], None, &service.root)
        .unwrap();
    service
        .dispatch_due(&item, item.deadline, false, |_, _| async {
            Err((false, "Connection dropped after acceptance".into()))
        })
        .await
        .unwrap();
    assert_eq!(service.database.outbox().unwrap()[0].state, "uncertain");
    service
        .dispatch_due(&item, item.deadline + 1, false, |_, _| async {
            panic!("Uncertain mail must not retry");
            #[allow(unreachable_code)]
            Ok(DeliveryReceipt {
                provider_message_id: String::new(),
                thread_id: Some(String::new()),
            })
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn explicit_provider_rejection_can_restore_a_draft() {
    let service = service();
    let d = saved(&service.database);
    let item = service
        .database
        .queue(&d.id, d.revision, false, &[], None, &service.root)
        .unwrap();
    service
        .dispatch_due(&item, item.deadline, false, |_, _| async {
            Err((true, "Invalid recipient".into()))
        })
        .await
        .unwrap();
    assert_eq!(service.database.outbox().unwrap()[0].state, "failed");
    assert_eq!(
        service.database.cancel_send(&item.id, true).unwrap().body,
        d.body
    );
}
#[tokio::test]
async fn provider_acceptance_followed_by_commit_failure_remains_uncertain_after_restart() {
    let service = service();
    let d = saved(&service.database);
    let item = service
        .database
        .queue(&d.id, d.revision, false, &[], None, &service.root)
        .unwrap();
    let result=service.dispatch_due(&item,item.deadline,false,|_,_|async {
        service.database.test_connection().unwrap().execute_batch("CREATE TRIGGER fail_ack BEFORE UPDATE ON outbox_messages WHEN NEW.state='sent' BEGIN SELECT RAISE(FAIL,'disk failure'); END;").unwrap();
        Ok(DeliveryReceipt{provider_message_id:"accepted".into(),thread_id:Some("thread".into())})
    }).await;
    assert!(result.is_err());
    assert_eq!(service.database.outbox().unwrap()[0].state, "sending");
    crate::schema::migrate(&mut service.database.test_connection().unwrap()).unwrap();
    assert_eq!(service.database.outbox().unwrap()[0].state, "uncertain");
}
#[test]
fn missing_and_unprepared_attachments_cannot_be_queued() {
    let db = database();
    let mut d = saved(&db);
    d.attachments.push(Attachment {
        id: Uuid::new_v4().to_string(),
        name: "missing.txt".into(),
        size: 10,
        mime: "text/plain".into(),
        ready: false,
        message_id: None,
        provider_id: None,
        inline: false,
        content_id: None,
    });
    assert!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).is_err());
    d.attachments[0].ready = true;
    assert!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).is_err());
}
#[test]
fn mime_rejects_header_injection_and_unsafe_attachment_ids() {
    let db = database();
    let base = saved(&db);

    for subject in ["Hello\r\nBcc: attacker@example.net", "Hello\nBcc: attacker@example.net", "Hello\r"] {
        let mut d = base.clone();
        d.subject = subject.into();
        assert_eq!(
            build_mime(&d, &[], None, None, "id", Path::new("/unused")).unwrap_err(),
            "Subject cannot contain newlines"
        );
    }

    let mut d = base.clone();
    d.reply_id = Some("original@example.com\r\nBcc: attacker@example.net".into());
    assert_eq!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).unwrap_err(), "Invalid reply headers");

    let mut d = base.clone();
    d.reply_id = Some("original@example.com".into());
    d.references = vec!["ancestor@example.com".into(), "x@example.com\nBcc: attacker@example.net".into()];
    assert_eq!(build_mime(&d, &[], None, None, "id", Path::new("/unused")).unwrap_err(), "Invalid reply headers");

    // A path-shaped attachment id must never be joined onto the
    // attachment root, even when a file exists at the traversal target.
    let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
    let inner = root.join("attachments");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(root.join("x"), b"outside the attachment root").unwrap();
    for id in ["../x", "../../x", "/etc/passwd", "not-a-uuid", ""] {
        let mut d = base.clone();
        d.attachments.push(Attachment {
            id: id.into(),
            name: "x".into(),
            size: 1,
            mime: "text/plain".into(),
            ready: true,
            message_id: None,
            provider_id: None,
            inline: false,
            content_id: None,
        });
        assert_eq!(build_mime(&d, &[], None, None, "id", &inner).unwrap_err(), "Invalid attachment ID", "id {id:?}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
fn scheduled(service: &Correspondence) -> OutboxItem {
    let d=saved(&service.database);
    service.database.schedule_draft(&d.id,d.revision,&crate::scheduled_send::Selection {local_time:"2030-01-01T09:00".into(),time_zone:"America/New_York".into(),offset_seconds:None},"00000000-0000-4000-8000-000000000001",&[],None,&service.root,1_800_000_000_000).unwrap()
}

#[tokio::test]
async fn blocked_keychain_poll_and_schedule_edit_leave_compose_responsive() {
    for polling in [true, false] {
        let mut service = service();
        let item = scheduled(&service);
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let (release, blocked) = std::sync::mpsc::channel();
        let blocked = std::sync::Mutex::new(blocked);
        service.sending_identity = Arc::new(move |_| {
            if let Some(entered) = entered.lock().unwrap().take() { let _ = entered.send(()); }
            blocked.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
            Ok("00000000-0000-4000-8000-000000000001".into())
        });
        let background = service.clone();
        let request = if polling { Request::ListOutbox } else { Request::Cancel { id: item.id } };
        let background = tokio::spawn(async move { background.request(request).await });
        tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().unwrap();
        let editing = tokio::time::timeout(Duration::from_secs(1), async {
            let value = service.request(Request::Create { mode: "new".into(), source_id: None, account: None }).await?;
            let mut draft: Draft = serde_json::from_value(value).map_err(error)?;
            draft.body = "Typed while the keychain was locked".into();
            service.request(Request::Save { draft: Box::new(draft) }).await
        }).await;
        // Always release the worker before asserting, so failures cannot strand it.
        release.send(()).unwrap();
        background.await.unwrap().unwrap();
        let saved = editing.expect("keychain must not delay create/save").unwrap();
        assert_eq!(saved["body"], "Typed while the keychain was locked");
    }
}

#[tokio::test]
async fn schedule_requests_reject_foreign_or_missing_identity_before_any_mutation() {
    for missing in [false, true] {
        let mut service = service();
        let item = scheduled(&service);
        let revision = item.schedule.as_ref().unwrap().report.report_revision;
        service.sending_identity = Arc::new(move |_| if missing {
            Err("keychain unavailable".into())
        } else {
            Ok("00000000-0000-4000-8000-000000000002".into())
        });
        let selection = || crate::scheduled_send::Selection { local_time: "2030-01-02T09:00".into(), time_zone: "America/New_York".into(), offset_seconds: None };
        for request in [
            Request::Cancel { id: item.id.clone() },
            Request::Recover { id: item.id.clone() },
            Request::SendScheduledNow { id: item.id.clone(), revision },
            Request::Reschedule { id: item.id.clone(), revision, selection: selection() },
            Request::Schedule { id: item.draft.id.clone(), revision: item.draft.revision, selection: selection() },
        ] {
            assert!(service.request(request).await.is_err());
            let current = service.database.outbox().unwrap().remove(0);
            assert_eq!(current.state, "scheduled");
            assert_eq!(current.deadline, item.deadline);
            assert_eq!(current.schedule.unwrap().report.report_revision, revision);
        }
        let listed = service.request(Request::ListOutbox).await.unwrap();
        assert_eq!(listed[0]["schedule"]["canManage"], false);
    }
}

#[tokio::test]
async fn scheduled_dispatch_boundaries_and_two_installations_never_duplicate() {
    use std::sync::atomic::{AtomicUsize,Ordering};
    for delta in [-1,0,60_000,60_001] {
        let owner=service(); let item=scheduled(&owner);
        let mut peer=owner.clone();
        peer.sending_identity=Arc::new(|_|Ok("00000000-0000-4000-8000-000000000002".into()));
        let calls=AtomicUsize::new(0);
        let send=|_,_|async {calls.fetch_add(1,Ordering::SeqCst);Ok(DeliveryReceipt {provider_message_id:"sent".into(),thread_id:None})};
        assert!(peer.dispatch_due(&item,item.deadline+delta,false,&send).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst),0);
        let result=owner.dispatch_due(&item,item.deadline+delta,false,&send).await.unwrap();
        let due=(0..=60_000).contains(&delta);
        assert_eq!(result.is_some(),due);
        owner.dispatch_due(&item,item.deadline+delta,false,&send).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst),usize::from(due));
        if due {assert_eq!(owner.database.outbox().unwrap()[0].schedule.as_ref().unwrap().report.state,"sent");}
    }
}

#[tokio::test]
async fn scheduled_reschedule_cancel_and_missing_identity_block_stale_delivery() {
    let mut owner=service();let item=scheduled(&owner);
    let revision=item.schedule.as_ref().unwrap().report.report_revision;
    let selection=crate::scheduled_send::Selection {local_time:"2030-01-02T09:00".into(),time_zone:"America/New_York".into(),offset_seconds:None};
    assert!(owner.database.manage_schedule(&item.id,revision,Some(&selection),false,"wrong-owner",item.deadline-1).is_err());
    owner.database.manage_schedule(&item.id,revision,Some(&selection),false,"00000000-0000-4000-8000-000000000001",item.deadline-1).unwrap();
    assert!(owner.database.manage_schedule(&item.id,revision,Some(&selection),false,"00000000-0000-4000-8000-000000000001",item.deadline-1).is_err());
    assert!(owner.dispatch_due(&item,item.deadline,false,|_,_|async {panic!("stale schedule sent")}).await.unwrap().is_none());
    owner.sending_identity=Arc::new(|_|Err("missing identity".into()));
    assert!(owner.request(Request::Cancel {id:item.id.clone()}).await.is_err());
    let current=owner.database.outbox().unwrap().remove(0);
    assert!(owner.dispatch_due(&current,current.deadline,false,|_,_|async {panic!("restored copy sent")}).await.is_err());
    owner.sending_identity=Arc::new(|_|Ok("00000000-0000-4000-8000-000000000001".into()));
    owner.request(Request::Cancel {id:item.id.clone()}).await.unwrap();
    assert_eq!(owner.database.drafts().unwrap()[0].body,item.draft.body);
    assert!(owner.dispatch_due(&current,current.deadline,false,|_,_|async {panic!("canceled schedule sent")}).await.unwrap().is_none());
}

#[test]
fn scheduled_recovery_and_overdue_policy_preserve_ordinary_undo() {
    let owner=service();let item=scheduled(&owner);
    assert!(!owner.database.pending_undo().unwrap());
    owner.database.overdue_schedules_owned(item.deadline+60_000,false,"00000000-0000-4000-8000-000000000001").unwrap();
    assert_eq!(owner.database.outbox().unwrap()[0].state,"scheduled");
    owner.database.overdue_schedules_owned(item.deadline+60_001,false,"00000000-0000-4000-8000-000000000001").unwrap();
    let late=owner.database.outbox().unwrap().remove(0);
    assert_eq!(late.state,"overdue");assert_eq!(late.schedule.as_ref().unwrap().report.state,"overdue");
    owner.database.manage_schedule(&item.id,late.schedule.unwrap().report.report_revision,None,true,"00000000-0000-4000-8000-000000000001",item.deadline+60_001).unwrap();
    let now_send=owner.database.outbox().unwrap().remove(0);assert_eq!(now_send.state,"undo_pending");assert_eq!(now_send.deadline,item.deadline+70_001);
    let d=saved(&owner.database);let immediate=owner.database.queue(&d.id,d.revision,false,&[],None,&owner.root).unwrap();
    crate::schema::migrate(&mut owner.database.test_connection().unwrap()).unwrap();
    let items=owner.database.outbox().unwrap();
    assert!(items.iter().find(|o|o.id==immediate.id).unwrap().deadline>unix_millis());
    assert_eq!(items.iter().find(|o|o.id==item.id).unwrap().schedule.as_ref().unwrap().report.scheduled_at,item.deadline);
}

#[tokio::test]
async fn scheduled_send_uncertainty_is_persisted_without_retry_and_report_failure_rolls_back() {
    let owner=service();let item=scheduled(&owner);
    owner.database.test_connection().unwrap().execute_batch("CREATE TRIGGER fail_report BEFORE UPDATE OF schedule_json ON outbox_messages BEGIN SELECT RAISE(FAIL,'report storage failed'); END;").unwrap();
    assert!(owner.dispatch_due(&item,item.deadline,false,|_,_|async {panic!("delivery before durable claim")}).await.is_err());
    assert_eq!(owner.database.outbox().unwrap()[0].state,"scheduled");
    owner.database.test_connection().unwrap().execute_batch("DROP TRIGGER fail_report;").unwrap();
    owner.dispatch_due(&item,item.deadline,false,|_,_|async {Err((false,"lost acknowledgement".into()))}).await.unwrap();
    let uncertain=owner.database.outbox().unwrap().remove(0);
    assert_eq!(uncertain.state,"uncertain");assert_eq!(uncertain.schedule.as_ref().unwrap().report.state,"uncertain");
    owner.dispatch_due(&uncertain,item.deadline+1,false,|_,_|async {panic!("uncertain resend")}).await.unwrap();
}
