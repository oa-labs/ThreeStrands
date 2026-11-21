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
async fn native_label_edit_refuses_system_labels_parents_and_existing_destinations() {
    let (server, provider, store) = fixture(&structures()[0]);
    server.lock().unwrap().folder("Projects", 10);
    sync(&provider, &store).await;
    assert!(provider.delete_label("INBOX").await.is_err());
    assert!(provider.delete_label("folder:Projects").await.is_err());
    assert!(provider
        .update_label("folder:Projects/Shared", "Labels/Keep")
        .await
        .is_err());
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
