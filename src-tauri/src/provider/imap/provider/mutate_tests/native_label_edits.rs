use super::*;
use crate::models::ThreadMutation;
use crate::provider::imap::write::encode_modified_utf7;
use all_mail::{fixture, structures, sync};

#[tokio::test]
async fn native_label_rename_and_delete_preserve_messages_split_and_identity() {
    const ACCOUNT: &str = "me@example.com";
    let (server, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let db = store.database();
    let legacy = "folder:Projects/Shared";
    let split = db
        .create_split_inbox("Orchard", "label", legacy, ACCOUNT)
        .unwrap();
    let thread = store.thread_ids_in_mailbox("INBOX").unwrap()[0].clone();
    let local_thread = format!("{ACCOUNT}:{thread}");
    let message_id = store.locations_in_mailbox("All Mail").unwrap()[0]
        .message_id
        .clone();
    let old = ThreadMutation::Label {
        thread_id: local_thread.clone(),
        label_id: legacy.into(),
        value: true,
    };
    db.mutate_thread(&old).unwrap();

    let name = "Projects/Orchard café";
    let wire = encode_modified_utf7(name);
    let renamed = provider.update_label(legacy, name).await.unwrap();
    assert_eq!((&renamed.id[..], &renamed.name[..]), (legacy, name));
    assert_eq!(db.list_split_inboxes().unwrap()[0].match_value, legacy);
    assert_eq!(
        db.list_split_inbox_page(&split.id, 0, 10)
            .unwrap()
            .threads
            .len(),
        1
    );
    assert!(db
        .get_thread(&local_thread)
        .unwrap()
        .thread
        .labels
        .contains(&legacy.to_string()));
    assert!(store
        .message_labels(&message_id)
        .unwrap()
        .unwrap()
        .contains(&wire));

    // The existing queued operation and any undo still address the old ID.
    provider
        .modify_messages(&[message_id.clone()], &[legacy.into()], &[])
        .await
        .unwrap();
    let recreated = provider.create_label("Projects/Shared").await.unwrap();
    assert_ne!(recreated.id, legacy);
    assert!(provider
        .list_labels()
        .await
        .unwrap()
        .iter()
        .any(|label| label.id == recreated.id));
    provider.delete_label(legacy).await.unwrap();
    provider.delete_label(legacy).await.unwrap();
    assert!(!db
        .get_thread(&local_thread)
        .unwrap()
        .thread
        .labels
        .contains(&legacy.to_string()));
    assert_eq!(db.list_split_inboxes().unwrap()[0].match_value, legacy);
    assert!(db
        .list_split_inbox_page(&split.id, 0, 10)
        .unwrap()
        .threads
        .is_empty());
    assert!(
        db.mutate_thread(&old).is_err(),
        "a stale undo cannot edit a retired label"
    );
    assert_eq!(db.retry_failed_mutations().unwrap(), 0);
    assert!(db.claim_mutations(ACCOUNT, 10).unwrap().is_empty());
    assert!(db.get_thread(&local_thread).is_ok());
    db.replace_account_labels(ACCOUNT, &provider.list_labels().await.unwrap())
        .unwrap();
    let relinked = db
        .relink_split_inbox_label(&split.id, &recreated.id)
        .unwrap();
    assert_eq!(relinked.id, split.id);
    assert_eq!(relinked.name, split.name);
    assert_eq!(relinked.match_value, recreated.id);
    let server = server.lock().unwrap();
    assert_eq!(
        server.folders["All Mail"].messages[0].gmail_message_id,
        Some(123)
    );
    assert!(server.folders.contains_key("Projects/Shared"));
    assert!(server.folders.contains_key("Labels/Keep"));
    assert!(!server.folders.contains_key(&wire));
    assert_eq!(server.count_starting("DELETE"), 1);
    for verb in ["UID EXPUNGE", "UID COPY", "UID MOVE", "UID STORE"] {
        assert_eq!(server.count_starting(verb), 0, "{verb}");
    }
}

#[tokio::test]
async fn native_label_rename_recovers_after_acknowledgement_and_listing_failure() {
    let (server, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    server.lock().unwrap().hide_listing_after_rename = true;
    let id = "folder:Projects/Shared";
    assert!(provider.update_label(id, "Projects/Revised").await.is_err());
    assert!(store.pending_label_edit().unwrap().unwrap().acknowledged);
    server.lock().unwrap().list_fails = false;
    sync(&provider, &store).await;
    let labels = provider.list_labels().await.unwrap();
    assert!(labels
        .iter()
        .any(|label| label.id == id && label.name == "Projects/Revised"));
    assert!(store.pending_label_edit().unwrap().is_none());
    assert_eq!(server.lock().unwrap().count_starting("RENAME"), 1);
}

#[tokio::test]
async fn native_label_rename_with_a_lost_reply_never_guesses_a_binding() {
    let (server, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    server.lock().unwrap().lose_rename_reply = true;
    let id = "folder:Projects/Shared";
    assert!(provider.update_label(id, "Projects/Revised").await.is_err());
    assert!(!store.pending_label_edit().unwrap().unwrap().acknowledged);
    sync(&provider, &store).await;
    let labels = provider.list_labels().await.unwrap();
    assert!(!labels.iter().any(|label| label.id == id));
    assert!(labels
        .iter()
        .any(|label| label.id == "folder:Projects/Revised"));
    assert!(store.pending_label_edit().unwrap().is_none());
    assert_eq!(server.lock().unwrap().count_starting("RENAME"), 1);
}

#[tokio::test]
async fn native_label_edit_refuses_system_labels_and_conflicting_destinations() {
    let (server, mut provider, store) = fixture(&structures()[0]);
    server.lock().unwrap().folder("Projects", 10);
    sync(&provider, &store).await;
    assert!(provider.delete_label("INBOX").await.is_err());
    assert!(provider
        .update_label("folder:Projects", "Projects/Nested")
        .await
        .is_err());
    assert!(provider
        .update_label("folder:Projects", "Labels")
        .await
        .is_err());
    assert!(provider
        .update_label("folder:Projects/Shared", "Labels/Keep")
        .await
        .is_err());
    provider.settings.label_container = Some("Projects/Shared".into());
    assert!(provider
        .update_label("folder:Projects", "Work")
        .await
        .unwrap_err()
        .to_string()
        .contains("label container"));
    let server = server.lock().unwrap();
    assert_eq!(server.count_starting("DELETE"), 0);
    assert_eq!(server.count_starting("RENAME"), 0);
    assert!(store.pending_label_edit().unwrap().is_none());
}

#[tokio::test]
async fn native_label_rename_retains_a_configured_labels_container() {
    let (server, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let unchanged = provider.update_label("lf:Keep", "Keep").await.unwrap();
    assert_eq!(unchanged.id, "lf:Keep");
    assert_eq!(server.lock().unwrap().count_starting("RENAME"), 0);
    let renamed = provider.update_label("lf:Keep", "keep").await.unwrap();
    assert_eq!(renamed.id, "lf:Keep");
    assert_eq!(renamed.name, "keep");
    assert!(server.lock().unwrap().folders.contains_key("Labels/keep"));
}

const ACCOUNT: &str = "me@example.com";
const PARENT: &str = "folder:Projects";
const CHILD: &str = "folder:Projects/Shared";

fn hierarchy_fixture() -> (Arc<Mutex<MutServer>>, ImapProvider, ImapStateStore) {
    let (server, provider, store) = fixture(&structures()[0]);
    {
        let mut state = server.lock().unwrap();
        state.folder("Projects", 10);
        state.folder("Projects/Shared/Calendar", 11);
        state.folder("ProjectsElsewhere", 12);
        all_mail::update_membership(
            &mut state,
            "INBOX",
            &[1],
            &[
                "Projects".into(),
                "Projects/Shared/Calendar".into(),
                "ProjectsElsewhere".into(),
            ],
            true,
        );
    }
    (server, provider, store)
}

#[tokio::test]
async fn native_parent_rename_preserves_descendant_rules_and_verified_coordinates() {
    let (server, provider, store) = hierarchy_fixture();
    sync(&provider, &store).await;
    let db = store.database();
    let grandchild = "folder:Projects/Shared/Calendar";
    let splits: Vec<_> = [PARENT, CHILD, grandchild]
        .iter()
        .map(|id| {
            let split = db.create_split_inbox(id, "label", id, ACCOUNT).unwrap();
            assert_eq!(
                db.list_split_inbox_page(&split.id, 0, 10)
                    .unwrap()
                    .threads
                    .len(),
                1
            );
            split
        })
        .collect();
    let message = store.locations_in_mailbox("INBOX").unwrap()[0]
        .message_id
        .clone();
    let child_copy = store
        .locations_in_mailbox("Projects/Shared")
        .unwrap()
        .into_iter()
        .find(|location| location.message_id == message)
        .unwrap();
    let grandchild_copy = store
        .locations_in_mailbox("Projects/Shared/Calendar")
        .unwrap()
        .into_iter()
        .find(|location| location.message_id == message)
        .unwrap();
    store
        .upsert_pin(
            &child_copy.mailbox,
            child_copy.uidvalidity,
            child_copy.uid,
            1,
        )
        .unwrap();
    store
        .upsert_pin(
            &grandchild_copy.mailbox,
            grandchild_copy.uidvalidity,
            grandchild_copy.uid,
            1,
        )
        .unwrap();
    server.lock().unwrap().rename_reset_validity = Some(grandchild_copy.mailbox.clone());

    let decoded_parent = "Travaux café";
    provider.update_label(PARENT, decoded_parent).await.unwrap();
    let parent = encode_modified_utf7(decoded_parent);
    let new_child = format!("{parent}/Shared");
    let new_grandchild = format!("{new_child}/Calendar");
    let labels = provider.list_labels().await.unwrap();
    for (id, name) in [
        (PARENT, decoded_parent.to_owned()),
        (CHILD, format!("{decoded_parent}/Shared")),
        (grandchild, format!("{decoded_parent}/Shared/Calendar")),
    ] {
        assert!(labels
            .iter()
            .any(|label| label.id == id && label.name == name));
    }
    for split in splits {
        assert_eq!(
            db.list_split_inbox_page(&split.id, 0, 10)
                .unwrap()
                .threads
                .len(),
            1
        );
        assert!(db
            .list_split_inboxes()
            .unwrap()
            .iter()
            .any(|rule| rule.id == split.id && rule.match_value == split.match_value));
    }
    assert_eq!(
        store
            .pinned_uids(&new_child, child_copy.uidvalidity)
            .unwrap(),
        vec![child_copy.uid as u32]
    );
    assert_eq!(
        store.locations_in_mailbox(&new_child).unwrap()[0].message_id,
        message
    );
    assert!(store
        .pinned_uids("Projects/Shared", child_copy.uidvalidity)
        .unwrap()
        .is_empty());
    assert!(store
        .locations_in_mailbox(&new_grandchild)
        .unwrap()
        .is_empty());
    assert!(store
        .pinned_uids(&new_grandchild, grandchild_copy.uidvalidity)
        .unwrap()
        .is_empty());
    assert!(store
        .pinned_uids(&grandchild_copy.mailbox, grandchild_copy.uidvalidity)
        .unwrap()
        .is_empty());

    let membership = store.message_labels(&message).unwrap().unwrap();
    for name in [
        &parent,
        &new_child,
        &new_grandchild,
        "ProjectsElsewhere",
        "Labels/Keep",
    ] {
        assert!(membership.iter().any(|value| value == name), "{name}");
    }
    assert!(!membership
        .iter()
        .any(|label| label == "Projects" || label.starts_with("Projects/")));
    assert_ne!(
        provider.create_label("Projects/Shared").await.unwrap().id,
        CHILD
    );
    // An already-queued change or undo using the old ID targets the nested
    // label at its new path, never the replacement at the original path.
    provider
        .modify_messages(std::slice::from_ref(&message), &[], &[CHILD.into()])
        .await
        .unwrap();
    let state = server.lock().unwrap();
    assert_eq!(state.count_starting("RENAME"), 1);
    assert!(state.folders.contains_key(&new_grandchild));
    assert!(state.folders["Projects/Shared"].messages.is_empty());
    assert!(state.folders[&new_child].messages.is_empty());
    assert_eq!(
        state.folders["All Mail"].messages[0].gmail_message_id,
        Some(123)
    );
}

#[tokio::test]
async fn native_parent_delete_removes_only_parent_with_or_without_a_container_placeholder() {
    for placeholder in [false, true] {
        let (server, provider, store) = hierarchy_fixture();
        server.lock().unwrap().keep_deleted_parent_as_container = placeholder;
        sync(&provider, &store).await;
        let db = store.database();
        let parent = db
            .create_split_inbox("Projects", "label", PARENT, ACCOUNT)
            .unwrap();
        let child = db
            .create_split_inbox("Shared", "label", CHILD, ACCOUNT)
            .unwrap();

        provider.delete_label(PARENT).await.unwrap();
        provider.delete_label(PARENT).await.unwrap();
        let labels = provider.list_labels().await.unwrap();
        assert!(!labels.iter().any(|label| label.id == PARENT));
        assert!(labels.iter().any(|label| label.id == CHILD));
        assert!(db
            .list_split_inbox_page(&parent.id, 0, 10)
            .unwrap()
            .threads
            .is_empty());
        assert_eq!(
            db.list_split_inbox_page(&child.id, 0, 10)
                .unwrap()
                .threads
                .len(),
            1
        );
        assert!(db
            .list_split_inboxes()
            .unwrap()
            .iter()
            .any(|rule| rule.id == parent.id && rule.match_value == PARENT));
        let state = server.lock().unwrap();
        assert_eq!(state.count_starting("DELETE"), 1);
        assert_eq!(state.count_starting("RENAME"), 0);
        assert!(state.folders.contains_key("Projects/Shared/Calendar"));
        assert_eq!(
            state.folders["All Mail"].messages[0].gmail_message_id,
            Some(123)
        );
        let labels = state.folders["All Mail"].messages[0]
            .labels
            .as_ref()
            .unwrap();
        assert!(
            labels.contains(&"Projects/Shared".into()) && labels.contains(&"Labels/Keep".into())
        );
        assert!(!labels.contains(&"Projects".into()));
        for verb in ["UID STORE", "UID EXPUNGE", "UID COPY", "UID MOVE"] {
            assert_eq!(state.count_starting(verb), 0, "{verb}");
        }
    }
}

#[tokio::test]
async fn native_parent_rename_recovers_the_whole_tree_only_with_a_tagged_acknowledgement() {
    for lost_reply in [false, true] {
        let (server, provider, store) = hierarchy_fixture();
        sync(&provider, &store).await;
        {
            let mut state = server.lock().unwrap();
            state.lose_rename_reply = lost_reply;
            state.hide_listing_after_rename = !lost_reply;
        }
        assert!(provider.update_label(PARENT, "Work").await.is_err());
        let pending = store.pending_label_edit().unwrap().unwrap();
        assert_eq!(pending.acknowledged, !lost_reply);
        assert_eq!(pending.descendants.len(), 2);
        let message = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        assert!(matches!(
            provider
                .modify_messages(&[message], &[], &[CHILD.into()])
                .await,
            Err(ProviderError::TransientTransport(_))
        ));
        server.lock().unwrap().list_fails = false;
        sync(&provider, &store).await;
        let labels = provider.list_labels().await.unwrap();
        let shared = labels
            .iter()
            .find(|label| label.name == "Work/Shared")
            .unwrap();
        assert_eq!(shared.id == CHILD, !lost_reply);
        assert_eq!(labels.iter().any(|label| label.id == PARENT), !lost_reply);
        assert!(store.pending_label_edit().unwrap().is_none());
        assert_eq!(server.lock().unwrap().count_starting("RENAME"), 1);
    }
}

#[tokio::test]
async fn native_parent_rename_reports_partial_changes_and_keeps_unchanged_children() {
    let (server, provider, store) = hierarchy_fixture();
    sync(&provider, &store).await;
    server.lock().unwrap().rename_parent_only = true;
    let error = provider.update_label(PARENT, "Work").await.unwrap_err();
    assert!(error.to_string().contains("full rename"));
    assert!(store.pending_label_edit().unwrap().is_none());
    let catalog = provider.list_labels().await.unwrap();
    assert!(catalog
        .iter()
        .any(|label| label.id == PARENT && label.name == "Work"));
    assert!(catalog
        .iter()
        .any(|label| label.id == CHILD && label.name == "Projects/Shared"));
    assert_eq!(server.lock().unwrap().count_starting("RENAME"), 1);
}

#[tokio::test]
async fn native_parent_edits_refuse_known_children_missing_from_list() {
    let (server, provider, store) = hierarchy_fixture();
    sync(&provider, &store).await;
    server
        .lock()
        .unwrap()
        .folders
        .remove("Projects/Shared/Calendar");
    assert!(provider
        .update_label(PARENT, "Work")
        .await
        .unwrap_err()
        .to_string()
        .contains("missing from the IMAP listing"));
    assert!(provider.delete_label(PARENT).await.is_err());
    assert!(store.pending_label_edit().unwrap().is_none());
    let state = server.lock().unwrap();
    assert_eq!(state.count_starting("RENAME"), 0);
    assert_eq!(state.count_starting("DELETE"), 0);
}

#[tokio::test]
async fn native_parent_delete_reports_an_unexpected_child_disappearance() {
    let (server, provider, store) = hierarchy_fixture();
    sync(&provider, &store).await;
    let db = store.database();
    let split = db
        .create_split_inbox("Shared", "label", CHILD, ACCOUNT)
        .unwrap();
    server.lock().unwrap().delete_extra_label = Some("Projects/Shared".into());

    let error = provider.delete_label(PARENT).await.unwrap_err();
    assert!(error.to_string().contains("nested label during deletion"));
    assert!(store.pending_label_edit().unwrap().is_none());
    let recreated = provider.create_label("Projects/Shared").await.unwrap();
    assert_ne!(recreated.id, CHILD);
    assert!(db
        .list_split_inbox_page(&split.id, 0, 10)
        .unwrap()
        .threads
        .is_empty());
    assert_eq!(db.list_split_inboxes().unwrap()[0].match_value, CHILD);
    let state = server.lock().unwrap();
    assert_eq!(state.count_starting("DELETE"), 1);
    assert_eq!(
        state.folders["All Mail"].messages[0].gmail_message_id,
        Some(123)
    );
}
