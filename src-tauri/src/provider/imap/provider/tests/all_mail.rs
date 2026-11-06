use super::*;

fn mailbox(native: bool) -> FakeMailbox {
    let mut mailbox = FakeMailbox::new(1);
    if native {
        mailbox.capabilities.push("X-GM-EXT-1".into());
    }
    mailbox
}

fn add(mailbox: &mut FakeMailbox, folder: &str, id: u64, labels: &[&str], subject: &str) {
    let raw = message(&format!("<{id}@all-mail.test>"), subject, "");
    mailbox.add_to(
        folder,
        if folder == "INBOX" { 1 } else { 2 },
        &["\\Seen"],
        &raw,
    );
    let entry = mailbox
        .folders
        .get_mut(folder)
        .unwrap()
        .messages
        .last_mut()
        .unwrap();
    entry.gmail_message_id = Some(id);
    entry.gmail_labels = Some(labels.iter().map(|label| label.to_string()).collect());
}

#[tokio::test]
async fn all_mail_only_archives_are_ingested_without_a_synthetic_label_or_duplicate_body() {
    for native in [false, true] {
        let mut mb = mailbox(native);
        add(&mut mb, "INBOX", 1, &["\\Inbox"], "Inbox invoice");
        add(&mut mb, "All Mail", 1, &["\\Inbox"], "Inbox invoice");
        add(&mut mb, "All Mail", 2, &[], "Archived invoice");
        let mb = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mb.clone());
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 2);
        assert_eq!(db.list_threads(Some("me@example.com")).unwrap().len(), 1);
        assert_eq!(
            mb.lock().unwrap().counts("All Mail").1,
            1,
            "the Inbox copy's body is reused"
        );
        for thread in store.thread_ids_in_mailbox("All Mail").unwrap() {
            let messages = provider.fetch_thread(&thread).await.unwrap();
            assert_eq!(messages.len(), 1);
            assert!(!messages[0]
                .label_ids
                .iter()
                .any(|label| label.contains("All Mail")));
        }
        let request = crate::models::SearchThreadsRequest {
            query: "Archived invoice".into(),
            limit: Some(20),
            offset: Some(0),
            include_archived: Some(true),
        };
        assert_eq!(
            db.search_threads(&request, Some("me@example.com"))
                .unwrap()
                .len(),
            1,
            "cached archived content is searchable without any server operation"
        );
        let before = store.generation().unwrap();
        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(
            store.generation().unwrap(),
            before,
            "unchanged membership must not journal an idle poll"
        );
    }
}

#[tokio::test]
async fn old_all_mail_hits_are_searchable_pinned_and_deduplicated_with_bounded_background_import() {
    for native in [false, true] {
        let mut mb = mailbox(native);
        for id in 1..=3 {
            add(&mut mb, "All Mail", id, &[], &format!("Invoice {id}"));
        }
        let mb = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mb.clone());
        provider.folder_window_override = Some(2);
        provider.sent_round_budget = Some(1);
        let db = store.database();
        provider.baseline_cursor().await.unwrap();
        assert_eq!(
            store.locations_in_mailbox("All Mail").unwrap().len(),
            1,
            "one background chunk"
        );
        add(
            &mut mb.lock().unwrap(),
            "INBOX",
            4,
            &["\\Inbox"],
            "Fresh invoice",
        );
        add(
            &mut mb.lock().unwrap(),
            "All Mail",
            4,
            &["\\Inbox"],
            "Fresh invoice",
        );
        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(
            db.list_threads(Some("me@example.com")).unwrap().len(),
            1,
            "history import never starves new Inbox mail"
        );
        assert_eq!(
            store.locations_in_mailbox("All Mail").unwrap().len(),
            2,
            "bounded All Mail window"
        );
        let result = provider.search_detailed("invoice", None).await.unwrap();
        assert_eq!(
            result.thread_ids.len(),
            4,
            "Inbox and All Mail copies must not duplicate a result"
        );
        assert!(
            store.pinned_uids("All Mail", 2).unwrap().contains(&1),
            "old out-of-window mail remains available after search"
        );
        sync_n(&provider, db.as_ref(), 1).await;
        assert!(store
            .location_message_id("All Mail", 2, 1)
            .unwrap()
            .is_some());
        let server = mb.lock().unwrap();
        assert_eq!(
            server.counts("All Mail").1,
            3,
            "each archived body fetched once"
        );
        if native {
            assert!(server
                .search_log
                .lock()
                .unwrap()
                .iter()
                .any(|(_, query)| query.contains("X-GM-RAW")
                    && query.contains("-in:trash")
                    && query.contains("-in:spam")));
        } else {
            assert!(!server
                .search_log
                .lock()
                .unwrap()
                .iter()
                .any(|(_, query)| query.contains("X-GM-RAW")));
        }
    }
}

#[tokio::test]
async fn default_generic_search_reaches_folder_only_mail_and_reports_mailbox_limits() {
    let mut mb = mailbox(false);
    for index in 0..=policy::MAX_SEARCH_TARGET_MAILBOXES {
        add(
            &mut mb,
            &format!("Projects/{index:02}"),
            index as u64,
            &[],
            &format!("Invoice {index}"),
        );
    }
    let mb = Arc::new(Mutex::new(mb));
    let (provider, _) = provider_with(mb);
    let result = provider.search_detailed("invoice", None).await.unwrap();
    assert!(
        !result.thread_ids.is_empty(),
        "don't discard all server results when there are too many folders"
    );
    assert!(matches!(result.partial, Some(PartialReason::BudgetReached)));
    assert_eq!(
        result.unsynced_mailboxes.len(),
        2,
        "Inbox also consumes a mailbox visit"
    );
    let folder = provider
        .search_detailed("in:Projects/12 invoice", None)
        .await
        .unwrap();
    assert_eq!(
        folder.thread_ids.len(),
        1,
        "an explicitly named folder remains searchable beyond the default visit cap"
    );
}

#[tokio::test]
async fn native_all_mail_membership_includes_unsynced_user_labels_and_filters_system_states() {
    let mut mb = mailbox(true);
    add(
        &mut mb,
        "All Mail",
        1,
        &["\\Sent", "Projects/Uncached", "Quoted \"label\""],
        "Sent invoice",
    );
    // No Sent or user folder is present in the catalog. Membership is complete
    // even when its physical folder copies are unavailable locally.
    mb.folders.get_mut("All Mail").unwrap().messages[0]
        .flags
        .clear();
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    sync_n(&provider, store.database().as_ref(), 1).await;
    let thread = store.thread_ids_in_mailbox("All Mail").unwrap()[0].clone();
    let labels = provider.fetch_thread(&thread).await.unwrap()[0]
        .label_ids
        .clone();
    for label in [
        "SENT",
        "folder:Projects/Uncached",
        "folder:Quoted \"label\"",
    ] {
        assert!(labels.contains(&label.to_string()));
    }
    assert!(!labels.contains(&"UNREAD".into()));
    assert!(!labels.contains(&"INBOX".into()));
}

#[tokio::test]
async fn inbox_membership_learned_through_all_mail_survives_inbox_reconciliation() {
    let mut mb = mailbox(true);
    add(
        &mut mb,
        "All Mail",
        1,
        &["\\Inbox"],
        "Known through All Mail",
    );
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    sync_n(&provider, store.database().as_ref(), 1).await;
    assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
    assert_eq!(provider.list_inbox(None).await.unwrap().thread_ids.len(), 1);
    assert_eq!(
        store
            .database()
            .list_threads(Some("me@example.com"))
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn search_does_not_pin_a_folder_outside_the_background_sync_cap() {
    let mut mb = mailbox(false);
    for index in 0..=policy::MAX_SYNCED_MAILBOXES {
        add(
            &mut mb,
            &format!("Projects/{index:03}"),
            index as u64,
            &[],
            "Invoice",
        );
    }
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    let result = provider
        .search_detailed(
            &format!("in:Projects/{:03} invoice", policy::MAX_SYNCED_MAILBOXES),
            None,
        )
        .await
        .unwrap();
    assert!(result.thread_ids.is_empty());
    assert_eq!(result.unsynced_mailboxes.len(), 1);
    assert!(matches!(result.partial, Some(PartialReason::BudgetReached)));
    assert_eq!(store.search_pin_count().unwrap(), 0);
}

#[tokio::test]
async fn mailbox_visit_limit_is_inclusive_and_incomplete_coverage_is_reported() {
    for total in [
        policy::MAX_SEARCH_TARGET_MAILBOXES - 1,
        policy::MAX_SEARCH_TARGET_MAILBOXES,
        policy::MAX_SEARCH_TARGET_MAILBOXES + 1,
    ] {
        let mut mb = mailbox(false);
        for index in 1..total {
            mb.folder(&format!("Projects/{index:02}"), 2);
        }
        let mb = Arc::new(Mutex::new(mb));
        let (provider, _) = provider_with(mb.clone());
        let result = provider.search_detailed("invoice", None).await.unwrap();
        assert_eq!(
            mb.lock().unwrap().search_log.lock().unwrap().len(),
            total.min(policy::MAX_SEARCH_TARGET_MAILBOXES)
        );
        assert_eq!(
            result.partial.is_some(),
            total > policy::MAX_SEARCH_TARGET_MAILBOXES
        );
        assert_eq!(
            result.unsynced_mailboxes.len(),
            total.saturating_sub(policy::MAX_SEARCH_TARGET_MAILBOXES)
        );
    }
}
