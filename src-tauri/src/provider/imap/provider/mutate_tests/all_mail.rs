use super::*;
use crate::provider::imap::write::StoreOp;

/// Model label membership globally, with independent per-mailbox UIDs. Removing
/// Inbox must not remove the message from All Mail or either user-label folder.
pub(super) fn update_membership(
    state: &mut MutServer,
    selected: &str,
    uids: &[u32],
    changing: &[String],
    add: bool,
) {
    let messages = state
        .folders
        .get(selected)
        .map(|f| {
            f.messages
                .iter()
                .filter(|m| uids.contains(&m.uid))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for mut message in messages {
        let Some(id) = message.gmail_message_id else {
            continue;
        };
        let mut labels = message.labels.clone().unwrap_or_default();
        for label in changing {
            if state.silently_ignore_flag.as_ref() == Some(label) {
                continue;
            }
            if add {
                if !labels.contains(label) {
                    labels.push(label.clone());
                }
            } else {
                labels.retain(|value| value != label);
            }
        }
        labels.sort();
        labels.dedup();
        message.labels = Some(labels.clone());
        let has = |name: &str| labels.iter().any(|label| label.eq_ignore_ascii_case(name));
        for (name, folder) in &mut state.folders {
            let wanted = match name.as_str() {
                "INBOX" => has("\\Inbox"),
                "All Mail" => !has("\\Trash") && !has("\\Spam"),
                "Trash" => has("\\Trash"),
                "Junk" => has("\\Spam"),
                "Sent" => has("\\Sent") && !has("\\Trash") && !has("\\Spam"),
                _ => labels.contains(name),
            };
            if wanted {
                if let Some(existing) = folder
                    .messages
                    .iter_mut()
                    .find(|m| m.gmail_message_id == Some(id))
                {
                    existing.labels = message.labels.clone();
                } else {
                    let mut copy = message.clone();
                    copy.uid = folder.uidnext;
                    folder.uidnext += 1;
                    folder.messages.push(copy);
                }
            } else {
                folder.messages.retain(|m| m.gmail_message_id != Some(id));
            }
        }
    }
}

fn fixture(raw: &str) -> (Arc<Mutex<MutServer>>, ImapProvider, ImapStateStore) {
    let mut state = MutServer::new();
    state.capabilities.push("X-GM-EXT-1".into());
    for (name, validity) in [
        ("INBOX", 1),
        ("All Mail", 2),
        ("Trash", 3),
        ("Junk", 4),
        ("Projects/Shared", 5),
        ("Labels/Keep", 6),
    ] {
        state.folder(name, validity);
    }
    state.folder("INBOX", 1).add(&["\\Seen"], raw);
    let initial = &mut state.folder("INBOX", 1).messages[0];
    initial.gmail_message_id = Some(123);
    initial.labels = Some(vec![
        "\\Inbox".into(),
        "Projects/Shared".into(),
        "Labels/Keep".into(),
    ]);
    update_membership(&mut state, "INBOX", &[1], &[], true);
    let state = Arc::new(Mutex::new(state));
    let (provider, store) = provider_with(state.clone());
    (state, provider, store)
}

fn structures() -> Vec<String> {
    vec![message("<native-root@example.test>"),
        "From: Other <other@example.test>\r\nTo: me@example.com\r\nDate: Tue, 06 Oct 2026 10:00:00 +0000\r\nSubject: Headerless MIME\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nsecond body\r\n--x\r\nContent-Type: text/html\r\n\r\n<p>second body</p>\r\n--x--\r\n".into()]
}

async fn sync(provider: &ImapProvider, store: &ImapStateStore) {
    crate::sync::sync_with(store.database().as_ref(), "me@example.com", provider)
        .await
        .unwrap();
}

#[tokio::test]
async fn archive_undo_trash_spam_and_restore_preserve_other_memberships() {
    for raw in structures() {
        let (state, provider, store) = fixture(&raw);
        sync(&provider, &store).await;
        let thread = store.thread_ids_in_mailbox("INBOX").unwrap()[0].clone();
        let id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        for (add, remove, expected, absent) in [
            (vec![], vec!["INBOX"], None, "\\Inbox"),
            (vec!["INBOX"], vec![], Some("\\Inbox"), "\\Trash"),
            (vec!["SPAM"], vec!["INBOX"], Some("\\Spam"), "\\Inbox"),
            (vec!["INBOX"], vec!["SPAM"], Some("\\Inbox"), "\\Spam"),
            (vec!["TRASH"], vec!["INBOX"], Some("\\Trash"), "\\Inbox"),
            (vec!["INBOX"], vec!["TRASH"], Some("\\Inbox"), "\\Trash"),
        ] {
            provider
                .modify_thread(
                    &thread,
                    &add.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    &remove.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                )
                .await
                .unwrap();
            sync(&provider, &store).await;
            let labels = store.message_labels(&id).unwrap().unwrap();
            if let Some(expected) = expected {
                assert!(labels.contains(&expected.into()));
            }
            assert!(!labels.contains(&absent.into()));
            assert!(
                labels.contains(&"Projects/Shared".into())
                    && labels.contains(&"Labels/Keep".into())
            );
            let fetched = provider.fetch_thread(&thread).await.unwrap();
            assert_eq!(fetched.len(), 1);
            assert_eq!(fetched[0].id, id);
            assert!(fetched[0]
                .label_ids
                .contains(&"folder:Projects/Shared".into()));
            assert!(fetched[0].label_ids.contains(&"lf:Keep".into()));
        }
        let guard = state.lock().unwrap();
        assert!(!guard.folders.contains_key("Archive"));
        for verb in ["UID MOVE", "UID COPY", "UID EXPUNGE", "CREATE"] {
            assert_eq!(
                guard.count_starting(verb),
                0,
                "{verb} must not implement a label action"
            );
        }
        assert!(!guard
            .wire()
            .iter()
            .any(|command| command.contains("\\Deleted")));
        assert!(guard.folders["All Mail"]
            .messages
            .iter()
            .any(|m| m.gmail_message_id == Some(123)));
    }
}

#[tokio::test]
async fn a_lost_archive_reply_is_retried_without_deleting_or_copying_mail() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let thread = store.thread_ids_in_mailbox("INBOX").unwrap()[0].clone();
    state.lock().unwrap().drop_label_response_once = true;
    assert!(provider
        .modify_thread(&thread, &[], &["INBOX".into()])
        .await
        .is_err());
    assert!(state.lock().unwrap().folders["INBOX"].messages.is_empty());
    provider
        .modify_thread(&thread, &[], &["INBOX".into()])
        .await
        .unwrap();
    sync(&provider, &store).await;
    assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
    assert_eq!(state.lock().unwrap().count_starting("UID STORE"), 1);
    assert_eq!(provider.fetch_thread(&thread).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_ignored_membership_store_does_not_commit_a_false_archive() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let thread = store.thread_ids_in_mailbox("INBOX").unwrap()[0].clone();
    let id = store.locations_in_mailbox("INBOX").unwrap()[0]
        .message_id
        .clone();
    state.lock().unwrap().silently_ignore_flag = Some("\\Inbox".into());
    assert!(provider
        .modify_thread(&thread, &[], &["INBOX".into()])
        .await
        .is_err());
    assert!(store
        .message_labels(&id)
        .unwrap()
        .unwrap()
        .contains(&"\\Inbox".into()));
    assert!(!store.locations_in_mailbox("INBOX").unwrap().is_empty());
    assert!(state.lock().unwrap().folders["Projects/Shared"]
        .messages
        .iter()
        .any(|m| m.gmail_message_id == Some(123)));
}

#[tokio::test]
async fn another_clients_archive_and_snooze_return_use_all_mail_membership() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let thread = store.thread_ids_in_mailbox("INBOX").unwrap()[0].clone();
    update_membership(
        &mut state.lock().unwrap(),
        "All Mail",
        &[1],
        &["\\Inbox".into()],
        false,
    );
    sync(&provider, &store).await;
    assert!(!provider.fetch_thread(&thread).await.unwrap()[0]
        .label_ids
        .contains(&"INBOX".into()));
    provider
        .modify_thread(&thread, &["INBOX".into()], &[])
        .await
        .unwrap();
    sync(&provider, &store).await;
    let local = format!("me@example.com:{thread}");
    let until = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    store
        .database()
        .snooze_threads(&[local.clone()], &until)
        .unwrap();
    provider
        .modify_thread(&thread, &[], &["INBOX".into()])
        .await
        .unwrap();
    provider.prepare_snoozed_thread(&thread, &[]).await.unwrap();
    provider
        .modify_thread(&thread, &["INBOX".into()], &[])
        .await
        .unwrap();
    assert!(store
        .message_labels(&store.messages_in_thread(&thread).unwrap()[0])
        .unwrap()
        .unwrap()
        .contains(&"\\Inbox".into()));
}

#[tokio::test]
async fn changing_uidvalidity_during_verification_cannot_commit_stale_coordinates() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let thread = store.thread_ids_in_mailbox("INBOX").unwrap()[0].clone();
    state.lock().unwrap().reset_label_validity_once = true;
    assert!(provider
        .modify_thread(&thread, &[], &["INBOX".into()])
        .await
        .is_err());
    assert!(store
        .locations_in_mailbox("All Mail")
        .unwrap()
        .iter()
        .all(|l| l.uidvalidity == 2));
    provider
        .modify_thread(&thread, &[], &["INBOX".into()])
        .await
        .unwrap();
    let all = store.locations_in_mailbox("All Mail").unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(
        all[0].uidvalidity, 3,
        "retry replaces the old incarnation rather than duplicating it"
    );
}

#[tokio::test]
async fn invalid_label_write_arguments_send_no_store_commands() {
    let (state, provider, _) = fixture(&structures()[0]);
    let mut session = provider.open_session().await.unwrap();
    let writer = session.as_write_session().unwrap();
    for (uids, labels) in [
        ("1\r\nLOGOUT", vec!["\\Inbox".into()]),
        ("0", vec!["\\Inbox".into()]),
        ("1", vec!["\\Deleted".into()]),
        ("1", vec!["\\Inbox) FLAGS (\\Deleted".into()]),
    ] {
        assert!(writer
            .uid_store_labels(uids, StoreOp::Add, &labels)
            .await
            .is_err());
    }
    assert_eq!(state.lock().unwrap().count_starting("UID STORE"), 0);
    session.logout().await.unwrap();
}
