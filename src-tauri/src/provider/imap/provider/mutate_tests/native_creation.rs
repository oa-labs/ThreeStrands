use super::*;
use crate::provider::imap::{policy, search::quote_search_string, write::encode_modified_utf7};
use all_mail::{fixture, structures, sync};

#[derive(Clone, Copy)]
pub(super) enum CreateOutcome {
    DropReply,
    Ignore,
    Deny,
    Race,
    SystemView,
    ListFails,
}

pub(super) fn list_response(state: &MutServer, tag: &str) -> (String, bool) {
    if state.list_fails {
        return (format!("{tag} NO listing unavailable\r\n"), false);
    }
    let mut out = String::new();
    for name in state.folders.keys() {
        let special = state
            .list_attributes
            .get(name)
            .map(String::as_str)
            .or_else(|| match name.as_str() {
                "All Mail" => Some("\\All"),
                "Trash" => Some("\\Trash"),
                "Junk" => Some("\\Junk"),
                "Sent" => Some("\\Sent"),
                _ => None,
            });
        let mut attributes = special.unwrap_or("").to_string();
        if state.list_noselect.contains(name) {
            attributes.push_str(" \\Noselect");
        }
        let name_argument = if state.literal_list_names {
            format!("{{{}}}\r\n{name}", name.len())
        } else {
            quote_search_string(name)
        };
        out.push_str(&format!(
            "* LIST ({attributes}) {} {name_argument}\r\n",
            quote_search_string(&state.delimiter),
        ));
    }
    out.push_str(&format!("{tag} OK LIST done\r\n"));
    (out, false)
}

pub(super) fn create_response(state: &mut MutServer, tag: &str, rest: &str) -> (String, bool) {
    let name = all_mail::parse_astrings(rest["CREATE".len()..].trim())[0].clone();
    let outcome = state.create_outcome.take();
    if matches!(outcome, Some(CreateOutcome::Deny)) {
        return (format!("{tag} NO [NOPERM] denied\r\n"), false);
    }
    if matches!(outcome, Some(CreateOutcome::Ignore)) {
        return (format!("{tag} OK CREATE done\r\n"), false);
    }
    if state.folders.contains_key(&name) {
        return (format!("{tag} NO [ALREADYEXISTS] exists\r\n"), false);
    }
    state.folder(&name, 700);
    match outcome {
        Some(CreateOutcome::DropReply) => (String::new(), true),
        Some(CreateOutcome::Race) => (format!("{tag} NO [ALREADYEXISTS] exists\r\n"), false),
        Some(CreateOutcome::SystemView) => {
            state.list_attributes.insert(name, "\\Flagged".into());
            (format!("{tag} OK CREATE done\r\n"), false)
        }
        Some(CreateOutcome::ListFails) => {
            state.list_fails = true;
            (format!("{tag} OK CREATE done\r\n"), false)
        }
        _ => (format!("{tag} OK CREATE done\r\n"), false),
    }
}

#[tokio::test]
async fn native_create_and_apply_supports_no_container_hierarchy_unicode_and_quoted_names() {
    for (index, raw) in structures().into_iter().enumerate() {
        let (state, mut provider, store) = fixture(&raw);
        state.lock().unwrap().literal_list_names = index != 0;
        provider.settings.label_storage = LabelStorage::None;
        provider.settings.label_container = None;
        sync(&provider, &store).await;
        let id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        for name in [
            "New label",
            "Projects/日本語 & café",
            "Quotes/a\"b\\tail (Q4)",
            "Pairs/a\\\\b\\\"c",
            "Archive",
        ] {
            let wire = encode_modified_utf7(name);
            let label = provider.create_label(name).await.unwrap();
            assert_eq!(label.id, format!("folder:{wire}"));
            assert_eq!(label.name, name);
            assert_eq!(label.kind, "user");
            assert!(state.lock().unwrap().folders.contains_key(&wire));
            assert!(provider
                .list_labels()
                .await
                .unwrap()
                .iter()
                .any(|listed| listed.id == label.id
                    && listed.name == label.name
                    && listed.kind == label.kind));
            provider
                .modify_messages(&[id.clone()], &[label.id.clone()], &[])
                .await
                .unwrap();
            let membership = store.message_labels(&id).unwrap().unwrap();
            for expected in ["\\Inbox", "Projects/Shared", "Labels/Keep", &wire] {
                assert!(membership.iter().any(|value| value == expected));
            }
            let count = state.lock().unwrap().count_starting("CREATE");
            assert_eq!(provider.create_label(name).await.unwrap().id, label.id);
            assert_eq!(state.lock().unwrap().count_starting("CREATE"), count);
        }
        assert_eq!(state.lock().unwrap().count_starting("CREATE"), 5);
        assert_eq!(state.lock().unwrap().count_starting("UID COPY"), 0);
        assert_eq!(state.lock().unwrap().count_starting("DELETE"), 0);
    }
}

#[tokio::test]
async fn native_creation_preserves_existing_lf_ids_and_uses_full_paths_for_new_labels() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    let original = provider.list_labels().await.unwrap();
    let database = store.database();
    let split = database
        .create_split_inbox("Keep", "label", "lf:Keep", "me@example.com")
        .unwrap();
    let thread = database.list_threads(Some("me@example.com")).unwrap()[0]
        .id
        .clone();
    database
        .mutate_thread(&crate::models::ThreadMutation::Label {
            thread_id: thread,
            label_id: "lf:Keep".into(),
            value: false,
        })
        .unwrap();
    let existing = provider.create_label("Labels/Keep").await.unwrap();
    assert_eq!(existing.id, "lf:Keep");
    assert!(original
        .iter()
        .any(|label| label.id == existing.id && label.name == existing.name));
    assert_eq!(state.lock().unwrap().count_starting("CREATE"), 0);
    let nested = provider.create_label("Labels/New/Nested").await.unwrap();
    assert_eq!(nested.id, "lf:New/Nested");
    assert_eq!(nested.name, "New/Nested");
    let root = provider.create_label("Root").await.unwrap();
    assert_eq!(root.id, "folder:Root");
    assert!(state.lock().unwrap().folders.contains_key("Root"));
    assert!(!state.lock().unwrap().folders.contains_key("Labels/Root"));
    let after = provider.list_labels().await.unwrap();
    assert!(original.iter().all(|old| after
        .iter()
        .any(|label| label.id == old.id && label.name == old.name)));
    assert!(database
        .list_split_inboxes()
        .unwrap()
        .iter()
        .any(|rule| rule.id == split.id && rule.match_value == "lf:Keep"));
    let pending = database.claim_mutations("me@example.com", 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        matches!(&pending[0].mutation, crate::models::ThreadMutation::Label { label_id, .. } if label_id == "lf:Keep")
    );
}

#[tokio::test]
async fn native_create_uses_live_delimiter_and_accepts_only_complete_hierarchy_segments() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    state.lock().unwrap().delimiter = ":".into();
    // A slash is ordinary data with this delimiter; cached slash rules must
    // not reject it. Empty live hierarchy components remain invalid.
    let label = provider.create_label("Work//Queue:日本語").await.unwrap();
    assert_eq!(
        label.id,
        format!("folder:{}", encode_modified_utf7("Work//Queue:日本語"))
    );
    for name in [":Leading", "Trailing:", "Empty::Part", "Blank: :Part"] {
        assert!(provider.create_label(name).await.is_err());
    }
    assert_eq!(state.lock().unwrap().count_starting("CREATE"), 1);
}

#[tokio::test]
async fn native_create_recovers_lost_replies_and_another_clients_create_without_duplicates() {
    for outcome in [CreateOutcome::DropReply, CreateOutcome::Race] {
        let lost_reply = matches!(outcome, CreateOutcome::DropReply);
        let (state, provider, store) = fixture(&structures()[0]);
        sync(&provider, &store).await;
        state.lock().unwrap().create_outcome = Some(outcome);
        let result = provider.create_label("Recovery").await;
        assert_eq!(result.is_err(), lost_reply);
        let label = provider.create_label("Recovery").await.unwrap();
        assert_eq!(label.id, "folder:Recovery");
        assert_eq!(state.lock().unwrap().count_starting("CREATE"), 1);
        assert!(provider
            .list_labels()
            .await
            .unwrap()
            .iter()
            .any(|listed| listed.id == label.id
                && listed.name == label.name
                && listed.kind == label.kind));
    }
}

#[tokio::test]
async fn unverified_native_creates_do_not_publish_a_label() {
    for outcome in [
        CreateOutcome::Ignore,
        CreateOutcome::Deny,
        CreateOutcome::SystemView,
        CreateOutcome::ListFails,
    ] {
        let (state, provider, store) = fixture(&structures()[0]);
        sync(&provider, &store).await;
        state.lock().unwrap().create_outcome = Some(outcome);
        assert!(provider.create_label("Unverified").await.is_err());
        assert!(!store
            .mailboxes()
            .unwrap()
            .iter()
            .any(|row| row.name == "Unverified"));
        assert!(!provider
            .list_labels()
            .await
            .unwrap()
            .iter()
            .any(|label| label.id == "folder:Unverified"));
    }
}

#[tokio::test]
async fn native_creation_refuses_system_views_containers_invalid_input_and_capability_loss() {
    let (state, mut provider, store) = fixture(&structures()[0]);
    {
        let mut guard = state.lock().unwrap();
        guard.folder("Priority", 8);
        guard
            .list_attributes
            .insert("Priority".into(), "\\Important".into());
        guard.folder("Container", 9);
        guard.list_noselect.insert("Container".into());
    }
    provider.settings.archive_mailbox = Some("ReservedRole".into());
    sync(&provider, &store).await;
    for name in [
        "INBOX",
        "All Mail",
        "Trash",
        "Priority",
        "Container",
        "ReservedRole",
        "\\Reserved",
        "",
        "  ",
        "Bad\r\nLOGOUT",
        "Bad\0Name",
        "Bad\tName",
        "/Leading",
        "Trailing/",
        "Empty//Part",
    ] {
        assert!(provider.create_label(name).await.is_err(), "{name:?}");
    }
    state
        .lock()
        .unwrap()
        .capabilities
        .retain(|cap| cap != "X-GM-EXT-1");
    assert!(provider.create_label("Unavailable").await.is_err());
    assert_eq!(state.lock().unwrap().count_starting("CREATE"), 0);
    assert_eq!(state.lock().unwrap().count_starting("UID STORE"), 0);
}

#[tokio::test]
async fn native_label_creation_respects_shared_name_boundaries() {
    let (state, provider, store) = fixture(&structures()[0]);
    sync(&provider, &store).await;
    for size in [
        policy::MAX_LABEL_NAME_CHARS - 1,
        policy::MAX_LABEL_NAME_CHARS,
        policy::MAX_LABEL_NAME_CHARS + 1,
    ] {
        let result = provider.create_label(&"é".repeat(size)).await;
        assert_eq!(result.is_ok(), size <= policy::MAX_LABEL_NAME_CHARS);
    }
    assert_eq!(state.lock().unwrap().count_starting("CREATE"), 2);
}
