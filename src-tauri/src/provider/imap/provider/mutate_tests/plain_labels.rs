use super::*;
use crate::provider::imap::write::encode_modified_utf7;

#[tokio::test]
async fn renaming_a_label_folder_preserves_its_saved_id_and_routes_future_label_writes() {
    let server = server_with_label_folder();
    server
        .lock()
        .unwrap()
        .folder("INBOX", 100)
        .add(&["\\Seen"], &message("<first@example.test>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(
        &store,
        &provider,
        "first",
        "imap:t:first",
        "INBOX",
        100,
        1,
        &["\\Seen"],
    );
    let label_id = "lf:Clients";
    let split = store
        .database()
        .create_split_inbox("Clients", "label", label_id, "me@example.com")
        .unwrap();
    provider
        .modify_messages(std::slice::from_ref(&id), &[label_id.into()], &[])
        .await
        .unwrap();

    let new_name = "Équipe";
    let path = format!("Labels/{}", encode_modified_utf7(new_name));
    let renamed = provider.update_label(label_id, new_name).await.unwrap();
    assert_eq!(renamed.id, label_id);
    assert_eq!(renamed.name, new_name);
    assert_eq!(
        store.database().list_split_inboxes().unwrap()[0].match_value,
        split.match_value
    );
    assert_eq!(
        local_mailboxes_of(&store, &id),
        vec!["INBOX".to_string(), path.clone()]
    );

    let recreated = provider.create_label("Clients").await.unwrap();
    assert_ne!(recreated.id, label_id);
    provider
        .modify_messages(std::slice::from_ref(&id), &[], &[label_id.into()])
        .await
        .unwrap();
    provider
        .modify_messages(std::slice::from_ref(&id), &[label_id.into()], &[])
        .await
        .unwrap();
    assert_eq!(
        local_mailboxes_of(&store, &id),
        vec!["INBOX".to_string(), path.clone()]
    );
    assert!(uids_in(&server, "Labels/Clients").is_empty());
    assert_eq!(uids_in(&server, &path).len(), 1);
    provider.delete_label(label_id).await.unwrap();
    assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
    assert!(store
        .database()
        .list_split_inboxes()
        .unwrap()
        .iter()
        .any(|rule| rule.id == split.id && rule.match_value == label_id));
    assert!(provider
        .list_labels()
        .await
        .unwrap()
        .iter()
        .any(|label| label.id == recreated.id));
}

#[tokio::test]
async fn physical_folders_can_be_created_and_renamed_but_delete_refuses_the_only_copy() {
    let server = server_with_label_folder();
    let (mut provider, store) = provider_with(server.clone());
    provider.settings.label_storage = LabelStorage::None;
    provider.settings.label_container = None;
    let folder = provider.create_folder("Projects/Équipe").await.unwrap();
    assert_eq!(folder.kind, "folder");
    assert_eq!(
        provider.create_folder("Projects/Équipe").await.unwrap().id,
        folder.id
    );
    let old = encode_modified_utf7("Projects/Équipe");
    let validity = {
        let mut g = server.lock().unwrap();
        let folder = g.folders.get_mut(&old).unwrap();
        folder.add(&[], &message("<only@example.test>"));
        folder.uidvalidity
    };
    let id = seed_loc(
        &store,
        &provider,
        "only",
        "imap:t:only",
        &old,
        i64::from(validity),
        1,
        &[],
    );
    let rename = provider
        .update_label(&folder.id, "Projects/Planning")
        .await
        .unwrap();
    assert_eq!(rename.id, folder.id);
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Projects/Planning"]);
    assert!(provider
        .modify_messages(std::slice::from_ref(&id), &[], &[rename.id.clone()])
        .await
        .is_err());
    assert!(provider.delete_label(&rename.id).await.is_err());
    assert_eq!(uids_in(&server, "Projects/Planning"), vec![1]);

    let empty = provider.create_folder("Empty").await.unwrap();
    provider.delete_label(&empty.id).await.unwrap();
    let replacement = provider.create_folder("Empty").await.unwrap();
    assert_ne!(replacement.id, empty.id);
    provider.delete_label(&empty.id).await.unwrap();
    assert!(server.lock().unwrap().folders.contains_key("Empty"));
}

#[tokio::test]
async fn keyword_writes_verify_each_mailbox_and_preserve_other_flags_and_copies() {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        let inbox = g.folder("INBOX", 100);
        inbox.permanent_flags.push("\\*".into());
        inbox.add(
            &["\\Seen", "$Forwarded", "Existing"],
            &message("<keyword@example.test>"),
        );
        let archive = g.folder("Archive", 200);
        archive.permanent_flags.push("Team".into());
        archive.add(&["\\Flagged"], &message("<keyword@example.test>"));
    }
    let (mut provider, store) = provider_with(server.clone());
    provider.settings.label_storage = LabelStorage::Keywords;
    provider.settings.label_container = None;
    let id = seed_loc(
        &store,
        &provider,
        "keyword",
        "imap:t:keyword",
        "INBOX",
        100,
        1,
        &["\\Seen", "$Forwarded", "Existing"],
    );
    store
        .upsert_location(&ImapLocation {
            mailbox: "Archive".into(),
            uidvalidity: 200,
            uid: 1,
            message_id: id.clone(),
            flags_json: r#"["\\Flagged"]"#.into(),
            modseq: None,
        })
        .unwrap();
    let keyword = provider.create_label("Team").await.unwrap();
    assert_eq!(keyword.id, "kw:Team");
    let listed = provider.list_labels().await.unwrap();
    assert!(listed.iter().any(|label| label.id == keyword.id));
    assert!(listed.iter().any(|label| label.id == "kw:Existing"));
    assert!(!listed.iter().any(|label| label.id == "kw:$Forwarded"));
    assert!(provider.create_label("$Junk").await.is_err());

    let labels = [keyword.id.clone()];
    provider
        .modify_messages(std::slice::from_ref(&id), &labels, &[])
        .await
        .unwrap();
    let plan = provider.build_sync_plan().unwrap();
    for location in store.locations_for_message(&id).unwrap() {
        let flags: Vec<String> = serde_json::from_str(&location.flags_json).unwrap();
        assert!(plan
            .labels_for_copy(&location.mailbox, &flags)
            .contains(&keyword.id));
    }
    provider
        .modify_messages(std::slice::from_ref(&id), &[], &labels)
        .await
        .unwrap();
    assert_eq!(uids_in(&server, "INBOX"), vec![1]);
    assert_eq!(uids_in(&server, "Archive"), vec![1]);
    let server = server.lock().unwrap();
    assert_eq!(
        server.folders["INBOX"].messages[0].flags,
        vec!["\\Seen", "$Forwarded", "Existing"]
    );
    assert_eq!(
        server.folders["Archive"].messages[0].flags,
        vec!["\\Flagged"]
    );
    for command in ["UID COPY", "UID MOVE", "DELETE", "UID EXPUNGE"] {
        assert_eq!(server.count_starting(command), 0, "{command}");
    }
}

#[tokio::test]
async fn refused_or_ignored_keywords_cannot_be_reported_as_successful() {
    let server = server_with_label_folder();
    server
        .lock()
        .unwrap()
        .folder("INBOX", 100)
        .add(&[], &message("<keyword@example.test>"));
    let (mut provider, store) = provider_with(server.clone());
    provider.settings.label_storage = LabelStorage::Keywords;
    provider.settings.label_container = None;
    let id = seed_loc(
        &store,
        &provider,
        "keyword",
        "imap:t:keyword",
        "INBOX",
        100,
        1,
        &[],
    );
    assert!(provider.create_label("Team").await.is_err());
    let label = ["kw:Team".into()];
    assert!(provider
        .modify_messages(std::slice::from_ref(&id), &label, &[])
        .await
        .is_err());
    assert_eq!(server.lock().unwrap().count_starting("UID STORE"), 0);

    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).permanent_flags.push("\\*".into());
        g.silently_ignore_flag = Some("Team".into());
    }
    assert!(provider
        .modify_messages(std::slice::from_ref(&id), &label, &[])
        .await
        .is_err());
    assert_eq!(
        store.locations_for_message(&id).unwrap()[0].flags_json,
        "[]"
    );
}
