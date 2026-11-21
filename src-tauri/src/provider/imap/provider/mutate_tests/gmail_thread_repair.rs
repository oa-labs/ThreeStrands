use super::*;
use crate::models::CreateTaskRequest;

const ACCOUNT: &str = "me@example.com";

async fn sync(prov: &ImapProvider, store: &ImapStateStore) {
    crate::sync::sync_with(store.database().as_ref(), ACCOUNT, prov)
        .await
        .unwrap();
}

fn with_identifiers(server: &mut MutServer, uids: &[(u64, u64)]) {
    for (row, (message, thread)) in server
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .iter_mut()
        .zip(uids)
    {
        row.gmail_message_id = Some(*message);
        row.gmail_thread_id = Some(*thread);
    }
}

fn enable_native(server: &Arc<Mutex<MutServer>>) {
    server
        .lock()
        .unwrap()
        .capabilities
        .push("X-GM-EXT-1".into());
}

fn reply(root: &str) -> String {
    format!("Message-ID: <orchard-reply@example.test>\r\nIn-Reply-To: {root}\r\nReferences: {root}\r\nDate: Tue, 07 Oct 2025 09:00:00 +0000\r\nFrom: sender@example.test\r\nSubject: Orchard\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=fixture\r\n\r\n--fixture\r\nContent-Type: text/plain\r\n\r\nFollow up\r\n--fixture\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=review.txt\r\n\r\nReview notes\r\n--fixture--\r\n")
}

#[tokio::test]
async fn gmail_thread_repair_splits_a_header_group_and_merges_verified_siblings() {
    let server = server_with_inbox();
    let root = "<orchard-root@example.test>";
    {
        let mut server = server.lock().unwrap();
        let inbox = server.folder("INBOX", 100);
        inbox.add(&[], &message(root));
        inbox.add(&[], &reply(root));
        inbox.add(&[], &message("<separate-header@example.test>"));
        with_identifiers(&mut server, &[(1101, 2001), (1102, 2002), (1103, 2002)]);
    }
    let (provider, store) = provider_with(server.clone());
    sync(&provider, &store).await;
    let db = store.database();
    let ids: BTreeMap<_, _> = store
        .locations_in_mailbox("INBOX")
        .unwrap()
        .into_iter()
        .map(|location| (location.uid, location.message_id))
        .collect();
    let old = db.get_thread_for_message(&ids[&2]).unwrap();
    assert_eq!(old.messages.len(), 2);
    assert!(old.messages.iter().any(|message| message.id == ids[&1]));
    let attachment = old
        .messages
        .iter()
        .find(|message| message.id == ids[&2])
        .unwrap()
        .attachments[0]
        .id
        .clone();
    db.set_file_star(ACCOUNT, &ids[&2], &attachment, true)
        .unwrap();
    let task = db
        .create_task(&CreateTaskRequest {
            account_id: ACCOUNT.into(),
            thread_id: Some(old.thread.id.clone()),
            source_message_id: Some(ids[&2].clone()),
            subject_snapshot: Some("Orchard".into()),
            title: "Review nursery orders".into(),
            notes: Some("Check delivery dates".into()),
            kind: "follow_up".into(),
            due_kind: "none".into(),
            due_value: None,
            time_zone: None,
            repeat_interval_days: None,
            evidence_text: None,
            goal_id: None,
        })
        .unwrap();

    let draft = db
        .create_draft(
            "reply",
            Some(ids[&2].clone()),
            ACCOUNT,
            &[crate::provider::SendingIdentity {
                address: ACCOUNT.into(),
                ..Default::default()
            }],
        )
        .unwrap();
    assert_eq!(
        draft.thread_id.as_deref(),
        Some(old.thread.provider_thread_id.as_str())
    );

    enable_native(&server);
    for _ in 0..3 {
        sync(&provider, &store).await;
    }
    let first = db.get_thread_for_message(&ids[&1]).unwrap();
    let second = db.get_thread_for_message(&ids[&2]).unwrap();
    let third = db.get_thread_for_message(&ids[&3]).unwrap();
    assert_eq!(first.thread.id, old.thread.id);
    assert_eq!(first.messages.len(), 1);
    assert_eq!(second.thread.id, third.thread.id);
    assert_ne!(first.thread.id, second.thread.id);
    assert_eq!(second.messages.len(), 2);
    assert_eq!(store.gmail_message_id(&ids[&2]).unwrap(), Some(1102));
    assert_eq!(
        db.file_stars(&[ids[&2].clone()]).unwrap(),
        vec![(ids[&2].clone(), attachment)]
    );
    let tasks = db.list_tasks(Some(ACCOUNT), None).unwrap();
    assert_eq!(tasks[0].id, task.id);
    assert_eq!(
        tasks[0].thread_id.as_deref(),
        Some(second.thread.id.as_str())
    );
    assert_eq!(tasks[0].source_message_id, task.source_message_id);
    assert_eq!(tasks[0].notes, task.notes);
    let mut stale_task = task.clone();
    stale_task.notes = Some("Updated on another device".into());
    db.upsert_synced_task(stale_task).unwrap();
    let received = db.list_tasks(Some(ACCOUNT), None).unwrap().remove(0);
    assert_eq!(
        received.thread_id.as_deref(),
        Some(second.thread.id.as_str())
    );
    assert_eq!(received.notes.as_deref(), Some("Updated on another device"));
    let repaired_draft = db.draft(&draft.id).unwrap();
    assert_eq!(
        repaired_draft.thread_id.as_deref(),
        Some(second.thread.provider_thread_id.as_str())
    );
    assert_eq!(repaired_draft.revision, draft.revision);
    assert_eq!(repaired_draft.source_id, draft.source_id);
    assert_eq!(repaired_draft.reply_id, draft.reply_id);
    assert_eq!(db.list_threads(Some(ACCOUNT)).unwrap().len(), 2);
    let native_fetches = || {
        server
            .lock()
            .unwrap()
            .wire()
            .iter()
            .filter(|line| line.contains("(UID X-GM-MSGID X-GM-THRID)"))
            .count()
    };
    let probes = native_fetches();
    sync(&provider, &store).await;
    assert!(store.gmail_thread_probes(128).unwrap().is_empty());
    assert_eq!(native_fetches(), probes);
}

#[tokio::test]
async fn gmail_thread_repair_keeps_links_to_a_fully_merged_conversation() {
    let server = server_with_inbox();
    {
        let mut server = server.lock().unwrap();
        let inbox = server.folder("INBOX", 100);
        inbox.add(&[], &message("<tea-order@example.test>"));
        inbox.add(&[], &message("<tea-receipt@example.test>"));
        with_identifiers(&mut server, &[(2110, 3001), (2111, 3001)]);
    }
    let (provider, store) = provider_with(server.clone());
    sync(&provider, &store).await;
    let db = store.database();
    let before = db.list_threads(Some(ACCOUNT)).unwrap();
    assert_eq!(before.len(), 2);
    enable_native(&server);
    for _ in 0..3 {
        sync(&provider, &store).await;
    }
    let after = db.list_threads(Some(ACCOUNT)).unwrap();
    assert_eq!(after.len(), 1);
    for old in before {
        let detail = db.get_thread(&old.id).unwrap();
        assert_eq!(detail.thread.id, after[0].id);
        assert_eq!(detail.messages.len(), 2);
    }
}

#[tokio::test]
async fn gmail_thread_repair_waits_for_complete_evidence_and_an_active_snooze() {
    let server = server_with_inbox();
    let root = "<seed-list@example.test>";
    {
        let mut server = server.lock().unwrap();
        let inbox = server.folder("INBOX", 100);
        inbox.add(&[], &message(root));
        inbox.add(&[], &reply(root));
        with_identifiers(&mut server, &[(1101, 2001)]);
    }
    let (provider, store) = provider_with(server.clone());
    sync(&provider, &store).await;
    let db = store.database();
    let before = db.list_threads(Some(ACCOUNT)).unwrap();
    let original = before[0].provider_thread_id.clone();
    enable_native(&server);
    sync(&provider, &store).await;
    assert_eq!(store.messages_in_thread(&original).unwrap().len(), 2);
    assert!(store.bound_gmail_thread(2001).unwrap().is_none());

    // Provider refresh alone leaves the application snooze and its queued
    // archive untouched. These are deliberately protected from a split.
    db.snooze_threads(&[before[0].id.clone()], "2099-03-01T12:00:00Z")
        .unwrap();
    with_identifiers(&mut server.lock().unwrap(), &[(1101, 2001), (1102, 2002)]);
    provider.refresh_all().await.unwrap();
    assert_eq!(store.messages_in_thread(&original).unwrap().len(), 2);
    assert!(store.bound_gmail_thread(2002).unwrap().is_none());
    assert_eq!(db.list_snooze_records(Some(ACCOUNT)).unwrap().len(), 1);
}
