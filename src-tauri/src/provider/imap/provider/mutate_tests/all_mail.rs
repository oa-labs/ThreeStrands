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
        ("1", vec!["".into()]),
        ("1", vec!["INBOX".into()]),
        ("1", vec!["label\r\nLOGOUT".into()]),
        ("1", vec!["label\tname".into()]),
        (
            "1",
            vec!["x".repeat(crate::provider::imap::policy::MAX_MAILBOX_NAME_BYTES + 1)],
        ),
    ] {
        assert!(writer
            .uid_store_labels(uids, StoreOp::Add, &labels)
            .await
            .is_err());
    }
    assert_eq!(state.lock().unwrap().count_starting("UID STORE"), 0);
    session.logout().await.unwrap();
}

/// Parse the fixture's atom/quoted-string subset, preserving backslashes in
/// atoms and decoding escapes only inside quoted strings.
pub(super) fn parse_astrings(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        let mut value = String::new();
        if ch == '"' {
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => break,
                    '\\' => {
                        if let Some(ch) = chars.next() {
                            value.push(ch);
                        }
                    }
                    _ => value.push(ch),
                }
            }
        } else {
            value.push(ch);
            while chars.peek().is_some_and(|ch| !ch.is_whitespace()) {
                value.push(chars.next().unwrap());
            }
        }
        out.push(value);
    }
    out
}

pub(super) fn label_arguments(command: &str) -> Vec<String> {
    parse_astrings(&command[command.find('(').unwrap() + 1..command.rfind(')').unwrap()])
}

#[tokio::test]
async fn existing_native_labels_toggle_without_a_container_and_keep_their_ids() {
    for raw in structures() {
        let (state, mut provider, store) = fixture(&raw);
        provider.settings.label_storage = LabelStorage::None;
        provider.settings.label_container = None;
        provider.label_model = LabelModel::ImapNoUserLabels;
        sync(&provider, &store).await;
        assert_eq!(
            provider.capabilities().label_model,
            LabelModel::ImapNativeLabels
        );
        let labels = provider.list_labels().await.unwrap();
        assert!(labels
            .iter()
            .any(|label| label.id == "folder:Projects/Shared" && label.kind == "user"));
        let id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        for (add, remove) in [
            (vec![], vec!["folder:Projects/Shared".into()]),
            (vec!["folder:Projects/Shared".into()], vec![]),
        ] {
            provider
                .modify_messages(&[id.clone()], &add, &remove)
                .await
                .unwrap();
            let labels = store.message_labels(&id).unwrap().unwrap();
            assert_eq!(labels.contains(&"Projects/Shared".into()), !add.is_empty());
            assert!(labels.contains(&"\\Inbox".into()) && labels.contains(&"Labels/Keep".into()));
            let count = state.lock().unwrap().count_starting("UID STORE");
            provider
                .modify_messages(&[id.clone()], &add, &remove)
                .await
                .unwrap();
            assert_eq!(
                state.lock().unwrap().count_starting("UID STORE"),
                count,
                "retry is idempotent"
            );
            sync(&provider, &store).await;
        }
        let guard = state.lock().unwrap();
        assert_eq!(guard.count_starting("UID COPY"), 0);
        assert_eq!(guard.count_starting("UID MOVE"), 0);
        assert_eq!(guard.count_starting("UID EXPUNGE"), 0);
        assert!(guard
            .wire()
            .iter()
            .filter(|command| command.starts_with("UID STORE"))
            .all(|command| command.contains("X-GM-LABELS") && !command.contains("\\Deleted")));
    }
}

#[tokio::test]
async fn native_label_removal_and_lost_reply_retry_keep_all_mail_and_other_labels() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let id = store.locations_in_mailbox("INBOX").unwrap()[0]
        .message_id
        .clone();
    let thread = store.thread_of_message(&id).unwrap().unwrap();
    state.lock().unwrap().drop_label_response_once = true;
    assert!(provider
        .modify_messages(&[id.clone()], &[], &["lf:Keep".into()])
        .await
        .is_err());
    provider
        .modify_messages(&[id.clone()], &[], &["lf:Keep".into()])
        .await
        .unwrap();
    assert_eq!(state.lock().unwrap().count_starting("UID STORE"), 1);
    let labels = store.message_labels(&id).unwrap().unwrap();
    assert!(!labels.contains(&"Labels/Keep".into()));
    assert!(labels.contains(&"Projects/Shared".into()) && labels.contains(&"\\Inbox".into()));
    assert!(store
        .locations_in_mailbox("Labels/Keep")
        .unwrap()
        .is_empty());
    let mail = provider.fetch_thread(&thread).await.unwrap();
    assert_eq!(mail.len(), 1);
    assert!(!mail[0].label_ids.contains(&"lf:Keep".into()));
    assert_eq!(mail[0].id, id);
}

#[tokio::test]
async fn native_labels_handle_quoted_nested_and_modified_utf7_names_without_reencoding() {
    for name in [
        "Labels".to_string(),
        "Archive".to_string(),
        "Projects/Sent".to_string(),
        "Projects/space (Q4)".to_string(),
        "Projects/a\"quote\\tail".to_string(),
        crate::provider::imap::write::encode_modified_utf7("Projects/日本語 & café"),
    ]
    .iter()
    {
        let (state, provider, store) = fixture(&structures()[0]);
        state.lock().unwrap().folder(name, 9);
        store
            .upsert_mailbox(&crate::provider::imap::ImapMailbox {
                name: name.clone(),
                delimiter: Some("/".into()),
                special_use: None,
                uidvalidity: 9,
                uidnext: 1,
                highestmodseq: None,
                permanent_flags_json: None,
                permanent_keywords: None,
            })
            .unwrap();
        sync(&provider, &store).await;
        let id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        let label = format!("folder:{name}");
        provider
            .modify_messages(&[id.clone()], &[label.clone()], &[])
            .await
            .unwrap();
        assert!(store.message_labels(&id).unwrap().unwrap().contains(name));
        provider
            .modify_messages(&[id.clone()], &[], &[label])
            .await
            .unwrap();
        assert!(!store.message_labels(&id).unwrap().unwrap().contains(name));
        assert!(state.lock().unwrap().folders.contains_key(name));
    }
}

#[tokio::test]
async fn ignored_native_label_write_and_uidvalidity_change_do_not_publish_success() {
    for reset in [false, true] {
        let (state, provider, store) = fixture(&structures()[0]);
        sync(&provider, &store).await;
        let id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        let original = store.message_labels(&id).unwrap();
        {
            let mut guard = state.lock().unwrap();
            if reset {
                guard.reset_label_validity_once = true;
            } else {
                guard.silently_ignore_flag = Some("Labels/Keep".into());
            }
        }
        assert!(provider
            .modify_messages(&[id.clone()], &[], &["lf:Keep".into()])
            .await
            .is_err());
        assert_eq!(store.message_labels(&id).unwrap(), original);
    }
}

#[tokio::test]
async fn native_label_unknown_ids_mixed_shapes_and_capability_loss_send_no_store() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let id = store.locations_in_mailbox("INBOX").unwrap()[0]
        .message_id
        .clone();
    for (add, remove) in [
        (vec!["folder:Not present".into()], vec![]),
        (vec!["lf:Keep".into(), "STARRED".into()], vec![]),
        (vec!["lf:Keep".into()], vec!["lf:Keep".into()]),
    ] {
        assert!(provider
            .modify_messages(&[id.clone()], &add, &remove)
            .await
            .is_err());
    }
    assert!(provider.create_label("new").await.is_err());
    assert!(provider.delete_label("lf:Keep").await.is_err());
    assert!(provider.update_label("lf:Keep", "renamed").await.is_err());
    state
        .lock()
        .unwrap()
        .capabilities
        .retain(|cap| cap != "X-GM-EXT-1");
    for _ in 0..2 {
        assert!(provider
            .modify_messages(&[id.clone()], &[], &["lf:Keep".into()])
            .await
            .is_err());
    }
    assert_eq!(state.lock().unwrap().count_starting("UID STORE"), 0);
    assert_eq!(state.lock().unwrap().count_starting("CREATE"), 0);
    assert!(!store.native_labels().unwrap());
    assert!(store.uses_native_labels().unwrap());
    assert_eq!(
        provider.capabilities().label_model,
        LabelModel::ImapNoUserLabels
    );
}

#[tokio::test]
async fn external_native_label_changes_refresh_without_changing_message_identity() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let id = store.locations_in_mailbox("INBOX").unwrap()[0]
        .message_id
        .clone();
    let thread = store.thread_of_message(&id).unwrap().unwrap();
    for add in [false, true] {
        update_membership(
            &mut state.lock().unwrap(),
            "All Mail",
            &[1],
            &["Projects/Shared".into()],
            add,
        );
        sync(&provider, &store).await;
        let mail = provider.fetch_thread(&thread).await.unwrap();
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].id, id);
        assert_eq!(
            mail[0].label_ids.contains(&"folder:Projects/Shared".into()),
            add
        );
    }
}

#[tokio::test]
async fn native_label_removal_refuses_ambiguous_ids_or_a_last_independent_location() {
    for ambiguous in [true, false] {
        let (state, provider, store) = fixture(&structures()[0]);
        sync(&provider, &store).await;
        let id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        let original = store.message_labels(&id).unwrap();
        {
            let mut guard = state.lock().unwrap();
            if ambiguous {
                let all = guard.folders.get_mut("All Mail").unwrap();
                let mut duplicate = all.messages[0].clone();
                duplicate.uid = all.uidnext;
                all.uidnext += 1;
                all.messages.push(duplicate);
            } else {
                for (name, folder) in &mut guard.folders {
                    if name != "Labels/Keep" {
                        folder.messages.clear();
                    }
                }
            }
        }
        assert!(provider
            .modify_messages(&[id.clone()], &[], &["lf:Keep".into()])
            .await
            .is_err());
        assert_eq!(store.message_labels(&id).unwrap(), original);
        let guard = state.lock().unwrap();
        assert_eq!(guard.count_starting("UID STORE"), 0);
        assert_eq!(guard.count_starting("UID EXPUNGE"), 0);
        assert_eq!(guard.folders["Labels/Keep"].messages.len(), 1);
    }
}

#[tokio::test]
async fn native_user_label_catalog_excludes_advertised_system_views() {
    let (state, mut provider, store) = fixture(&structures()[0]);
    provider.settings.label_container = Some("Starred View".into());
    for (name, attribute) in [
        ("Priority View", "\\Important"),
        ("Starred View", "\\Flagged"),
    ] {
        state.lock().unwrap().folder(name, 9);
        store
            .upsert_mailbox(&crate::provider::imap::ImapMailbox {
                name: name.into(),
                delimiter: Some("/".into()),
                special_use: Some(attribute.into()),
                uidvalidity: 9,
                uidnext: 1,
                highestmodseq: None,
                permanent_flags_json: None,
                permanent_keywords: None,
            })
            .unwrap();
    }
    sync(&provider, &store).await;
    let labels = provider.list_labels().await.unwrap();
    let id = store.locations_in_mailbox("INBOX").unwrap()[0]
        .message_id
        .clone();
    for name in ["Priority View", "Starred View"] {
        let label_id = format!("folder:{name}");
        assert!(!labels.iter().any(|label| label.id == label_id));
        assert!(provider
            .modify_messages(&[id.clone()], &[label_id], &[])
            .await
            .is_err());
    }
    assert_eq!(state.lock().unwrap().count_starting("UID STORE"), 0);
}

#[tokio::test]
async fn first_native_session_cannot_enter_physical_label_folder_operations() {
    for create in [true, false] {
        let (state, provider, store) = fixture(&structures()[0]);
        assert!(!store.uses_native_labels().unwrap());
        let result = if create {
            provider.create_label("New").await.map(|_| ())
        } else {
            provider.delete_label("lf:Keep").await
        };
        assert!(result.is_err());
        assert!(store.native_labels().unwrap());
        let guard = state.lock().unwrap();
        for command in ["CREATE", "DELETE", "UID COPY", "UID STORE", "UID EXPUNGE"] {
            assert_eq!(guard.count_starting(command), 0);
        }
    }
}
