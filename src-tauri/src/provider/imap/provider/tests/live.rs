use super::super::super::connection::{ConnectionConfig, ImapConnectionManager, TlsMode};
use super::super::super::session::ImapSession;
use super::super::super::tls::parse_sha256_fingerprint;
use super::super::{BodyCache, ImapProvider, ImapProviderConfig, ImapStateStore};
use crate::db::Database;
use crate::provider::{LabelModel, MailSync};
use crate::provider::MailMutate;
use std::sync::Arc;

const HOST: &str = "127.0.0.1";
const PORT: u16 = 11143;
const USER: &str = "test@threestrands.test";
const PASSWORD: &str = "testpassword";

fn gated() -> Option<ConnectionConfig> {
    if std::env::var("THREESTRANDS_IMAP_IT").ok().as_deref() != Some("1") {
        eprintln!("skipping: THREESTRANDS_IMAP_IT != 1");
        return None;
    }
    let fp = parse_sha256_fingerprint(&std::env::var("DOVECOT_TEST_FP").ok()?)
        .expect("DOVECOT_TEST_FP must be 64 hex digits");
    Some(ConnectionConfig {
        host: HOST.into(),
        port: PORT,
        tls_mode: TlsMode::StartTls,
        pinned_fingerprint: Some(fp),
    })
}

fn live_provider(config: ConnectionConfig) -> ImapProvider {
    let database = Arc::new(Database::open_memory());
    let store = ImapStateStore::new(database.clone(), USER);
    let cache = BodyCache::new(store.clone());
    ImapProvider::new(ImapProviderConfig {
        account_id: USER.into(),
        username: USER.into(),
        password: PASSWORD.into(),
        smtp_password: None,
        manager: ImapConnectionManager::new(config),
        store,
        cache,
        label_model: LabelModel::ImapLabelFolders,
        settings: super::test_settings(),
    })
}

/// Flags carry their wire spelling including the `\` prefix.
fn mentions_seen(flags: &[String]) -> bool {
    flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"))
}

#[tokio::test]
async fn live_provider_syncs_inbox_keeps_seen_absent_and_is_stable_on_re_poll() {
    let Some(config) = gated() else { return };
    let provider = live_provider(config.clone());

    // baseline_cursor + list_inbox + fetch_thread return the seeded mail.
    let cursor = provider.baseline_cursor().await.expect("baseline");
    let threads = provider.list_inbox(None).await.expect("list_inbox");
    assert!(!threads.thread_ids.is_empty(), "the harness seeds INBOX");
    let mut any_message = false;
    for thread_id in &threads.thread_ids {
        let messages = provider.fetch_thread(thread_id).await.expect("fetch_thread");
        any_message |= !messages.is_empty();
    }
    assert!(any_message, "seeded threads have messages");

    // \Seen is still absent on the server after sync (BODY.PEEK + EXAMINE).
    let mut probe = ImapConnectionManager::new(config)
        .connect_leased(
            super::super::super::connection::ConnectionRole::Command,
            USER,
            PASSWORD,
        )
        .await
        .expect("probe login")
        .0;
    probe.examine("INBOX").await.expect("examine");
    let uids = probe.uid_search("ALL").await.expect("search");
    let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let rows = super::super::super::fetch::fetch_flags_only(&mut probe, &set)
        .await
        .expect("flags");
    assert!(
        rows.iter().all(|(_, flags)| !mentions_seen(flags)),
        "sync must not set \\Seen"
    );
    let _ = probe.logout().await;

    // The baseline cursor sits right after the INBOX round, so the first
    // poll from it delivers what the folder rounds (Sent, here the
    // shared Bcc-to-self copy) journaled during the same walk. After
    // that, a poll from the returned cursor is stable: it reports no
    // changes and the generation does not move.
    let first = provider.poll(&cursor).await.expect("delivery poll");
    let gen_before = first.cursor.generation().unwrap();
    let batch = provider.poll(&first.cursor).await.expect("re-poll");
    assert!(batch.changed_threads.is_empty(), "a stable re-poll is empty");
    assert_eq!(batch.cursor.generation(), Some(gen_before), "no idle bump");
}

/// (l, live) After baseline + enough polls to finish Sent, the shared
/// Message-ID (seeded in BOTH INBOX and Sent) resolves to ONE message
/// labelled INBOX+SENT, `\Seen` is still absent on the INBOX copy, and
/// the seeded Trash + Junk messages are index-synced locally but never
/// reported as changed_threads.
#[tokio::test]
async fn live_provider_syncs_sent_trash_and_junk_as_designed() {
    let Some(config) = gated() else { return };
    let provider = live_provider(config.clone());

    // Baseline, then several polls so the Sent chunked backfill (and
    // the Trash/Junk index rounds) all complete. The seed is tiny, so a
    // handful of polls is plenty; cursor is carried forward each time.
    let mut cursor = provider.baseline_cursor().await.expect("baseline");
    for _ in 0..5 {
        let batch = provider.poll(&cursor).await.expect("poll");
        cursor = batch.cursor;
    }

    // The shared Bcc-to-self Message-ID is ONE message with INBOX+SENT.
    // Find it by its defining property: a stable id with locations in
    // BOTH INBOX and Sent (no need to re-derive the id by hand).
    let inbox_locs = provider.store.locations_in_mailbox("INBOX").expect("inbox");
    let mut shared_message_id: Option<String> = None;
    for location in &inbox_locs {
        let all = provider
            .store
            .locations_for_message(&location.message_id)
            .expect("locations");
        let mailboxes: std::collections::BTreeSet<&str> =
            all.iter().map(|l| l.mailbox.as_str()).collect();
        if mailboxes.contains("INBOX") && mailboxes.contains("Sent") {
            shared_message_id = Some(location.message_id.clone());
            break;
        }
    }
    let shared_message_id =
        shared_message_id.expect("the shared Bcc-to-self message is in INBOX and Sent");

    let thread = provider
        .store
        .thread_of_message(&shared_message_id)
        .expect("thread_of_message")
        .expect("a thread for the shared message");
    let messages = provider.fetch_thread(&thread).await.expect("fetch_thread");
    // Exactly one message id for the shared Message-ID, with both labels.
    let shared_messages: Vec<_> = messages
        .iter()
        .filter(|m| m.id == shared_message_id)
        .collect();
    assert_eq!(shared_messages.len(), 1, "one message, not two copies");
    let labels = &shared_messages[0].label_ids;
    assert!(labels.contains(&"INBOX".to_string()), "INBOX: {labels:?}");
    assert!(labels.contains(&"SENT".to_string()), "SENT: {labels:?}");

    // \Seen is still absent on the INBOX copy of the shared message.
    let mut probe = ImapConnectionManager::new(config)
        .connect_leased(
            super::super::super::connection::ConnectionRole::Command,
            USER,
            PASSWORD,
        )
        .await
        .expect("probe login")
        .0;
    probe.examine("INBOX").await.expect("examine");
    let uids = probe.uid_search("ALL").await.expect("search");
    let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let rows = super::super::super::fetch::fetch_flags_only(&mut probe, &set)
        .await
        .expect("flags");
    assert!(
        rows.iter().all(|(_, flags)| !mentions_seen(flags)),
        "sync must not set \\Seen on the INBOX copy"
    );
    let _ = probe.logout().await;

    // Trash and Junk are index-synced locally (TRASH / SPAM labels) …
    let trash = provider.store.locations_in_mailbox("Trash").expect("trash");
    let junk = provider.store.locations_in_mailbox("Junk").expect("junk");
    assert!(!trash.is_empty(), "Trash is index-synced");
    assert!(!junk.is_empty(), "Junk is index-synced");
    for (mailbox, location, expected) in [
        ("Trash", &trash[0], "TRASH"),
        ("Junk", &junk[0], "SPAM"),
    ] {
        let t = provider
            .store
            .thread_of_message(&location.message_id)
            .expect("thread")
            .expect("a thread");
        let msgs = provider.fetch_thread(&t).await.expect("fetch_thread");
        assert!(
            msgs.iter().any(|m| m.label_ids.contains(&expected.to_string())),
            "{mailbox} message carries {expected}"
        );
        // … but never hot, so never reported to the engine.
        assert!(
            !provider.store.is_thread_hot(&t).expect("hot?"),
            "{mailbox}-only thread is not hot (never reported)"
        );
    }
}

/// (Slice 5b-2, live) After baseline, UID MOVE an INBOX message to
/// Archive through the test's OWN session, poll, and assert the thread
/// is still present (now without INBOX, since Archive carries no system
/// location label), with NO `NotFound`, and that `\Seen` is untouched.
/// Archive is synced in 5b-2 so the move's destination is swept and the
/// thread never even enters the emptied-thread grace. The Dovecot
/// harness seeds Archive with one message (see seed.sh); this adds one
/// more via UID MOVE. Skips cleanly without docker.
#[tokio::test]
async fn live_provider_follows_an_inbox_to_archive_move_without_notfound_or_seen() {
    let Some(config) = gated() else { return };
    let provider = live_provider(config.clone());

    // Baseline + a few polls so INBOX and Archive are both synced.
    let mut cursor = provider.baseline_cursor().await.expect("baseline");
    for _ in 0..5 {
        cursor = provider.poll(&cursor).await.expect("poll").cursor;
    }

    // Pick an INBOX message to move, remembering its stable id so we can
    // follow it across the move.
    let inbox = provider.store.locations_in_mailbox("INBOX").expect("inbox");
    assert!(!inbox.is_empty(), "the harness seeds INBOX");
    let moved_uid = inbox[0].uid as u32;
    let moved_id = inbox[0].message_id.clone();

    // UID MOVE it to Archive through our own command session. SELECT
    // (read-write) is required for MOVE; this is the TEST's session, not
    // the provider's EXAMINE-only sync session, so the \Seen invariant
    // is about what SYNC does, not this move.
    let mut mover = ImapConnectionManager::new(config.clone())
        .connect_leased(
            super::super::super::connection::ConnectionRole::Command,
            USER,
            PASSWORD,
        )
        .await
        .expect("mover login")
        .0;
    mover
        .run_command_capture_code("SELECT INBOX")
        .await
        .expect("select inbox");
    mover
        .run_command_capture_code(&format!("UID MOVE {moved_uid} Archive"))
        .await
        .expect("uid move to Archive");
    let _ = mover.logout().await;

    // Poll until the move is reflected (INBOX location gone, Archive
    // location present for the moved id).
    let mut followed = false;
    for _ in 0..6 {
        cursor = provider.poll(&cursor).await.expect("poll").cursor;
        let locs = provider.store.locations_for_message(&moved_id).expect("locations");
        let mailboxes: std::collections::BTreeSet<&str> =
            locs.iter().map(|l| l.mailbox.as_str()).collect();
        if !mailboxes.contains("INBOX") && mailboxes.contains("Archive") {
            followed = true;
            break;
        }
    }
    assert!(followed, "the move from INBOX to Archive was followed");

    // The thread is still present and fetchable (NO NotFound), with
    // Archive's no-INBOX labels.
    let thread = provider
        .store
        .thread_of_message(&moved_id)
        .expect("thread_of_message")
        .expect("a thread for the moved message");
    let messages = provider.fetch_thread(&thread).await.expect("fetch_thread not NotFound");
    assert!(!messages.is_empty(), "the moved thread still has a message");
    let labels: Vec<String> =
        messages.iter().flat_map(|m| m.label_ids.clone()).collect();
    assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after archiving: {labels:?}");

    // \Seen is still absent on the moved message's Archive copy (sync is
    // EXAMINE + BODY.PEEK, so it never set it).
    let mut probe = ImapConnectionManager::new(config)
        .connect_leased(
            super::super::super::connection::ConnectionRole::Command,
            USER,
            PASSWORD,
        )
        .await
        .expect("probe login")
        .0;
    probe.examine("Archive").await.expect("examine archive");
    let uids = probe.uid_search("ALL").await.expect("search");
    let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let rows = super::super::super::fetch::fetch_flags_only(&mut probe, &set)
        .await
        .expect("flags");
    assert!(
        rows.iter().all(|(_, flags)| !mentions_seen(flags)),
        "sync must not set \\Seen on the archived copy"
    );
    let _ = probe.logout().await;
}

/// (Slice 5b-2 run B, live) With `label_storage = Folders` and
/// `label_container = Labels` (super::test_settings), after baseline +
/// a few polls the labelled+foldered message (seeded in INBOX, the user
/// folder Folders/Projects, and the label folder Labels/Clients)
/// carries `INBOX` + `lf:Clients` + `folder:Folders/Projects`; the
/// old message that exists ONLY in the user folder is index-synced but
/// never hot (never reported); and `list_labels` contains both dynamic
/// labels. Skips cleanly without docker.
#[tokio::test]
async fn live_provider_reports_folder_and_lf_labels_and_lists_them() {
    let Some(config) = gated() else { return };
    let provider = live_provider(config);

    // Baseline + a few polls so INBOX, the user folder and the label
    // folder are all visited.
    let mut cursor = provider.baseline_cursor().await.expect("baseline");
    for _ in 0..6 {
        cursor = provider.poll(&cursor).await.expect("poll").cursor;
    }

    // The labelled message is the one with a copy in all three of
    // INBOX, Folders/Projects and Labels/Clients. Find it by that
    // defining property.
    let inbox_locs = provider.store.locations_in_mailbox("INBOX").expect("inbox");
    let mut labelled: Option<String> = None;
    for location in &inbox_locs {
        let all = provider
            .store
            .locations_for_message(&location.message_id)
            .expect("locations");
        let boxes: std::collections::BTreeSet<&str> =
            all.iter().map(|l| l.mailbox.as_str()).collect();
        if boxes.contains("INBOX")
            && boxes.contains("Folders/Projects")
            && boxes.contains("Labels/Clients")
        {
            labelled = Some(location.message_id.clone());
            break;
        }
    }
    let labelled = labelled.expect("the labelled+foldered message is in all three places");

    let thread = provider
        .store
        .thread_of_message(&labelled)
        .expect("thread_of_message")
        .expect("a thread");
    let messages = provider.fetch_thread(&thread).await.expect("fetch_thread");
    let labels: Vec<String> =
        messages.iter().flat_map(|m| m.label_ids.clone()).collect();
    assert!(labels.contains(&"INBOX".to_string()), "INBOX: {labels:?}");
    assert!(labels.contains(&"lf:Clients".to_string()), "lf:Clients: {labels:?}");
    assert!(
        labels.contains(&"folder:Folders/Projects".to_string()),
        "folder:Folders/Projects: {labels:?}"
    );

    // The folder-only old message is index-synced (a location exists)
    // but its thread is NOT hot (it has no INBOX/Sent location), so it
    // is never reported to the engine.
    let folder_locs =
        provider.store.locations_in_mailbox("Folders/Projects").expect("folder");
    let folder_only = folder_locs
        .iter()
        .find(|l| {
            let all = provider
                .store
                .locations_for_message(&l.message_id)
                .expect("locations");
            all.iter().all(|loc| loc.mailbox != "INBOX")
        })
        .expect("the folder-only message is index-synced");
    let t = provider
        .store
        .thread_of_message(&folder_only.message_id)
        .expect("thread")
        .expect("a thread");
    assert!(
        !provider.store.is_thread_hot(&t).expect("hot?"),
        "a folder-only thread is never hot (never reported)"
    );

    // list_labels contains the dynamic labels (read from the local plan).
    let listed = provider.list_labels().await.expect("list_labels");
    let ids: std::collections::BTreeSet<&str> =
        listed.iter().map(|l| l.id.as_str()).collect();
    assert!(ids.contains("lf:Clients"), "list_labels has lf:Clients: {ids:?}");
    assert!(
        ids.contains("folder:Folders/Projects"),
        "list_labels has folder:Folders/Projects: {ids:?}"
    );
    // Kinds are correct.
    let lf = listed.iter().find(|l| l.id == "lf:Clients").unwrap();
    assert_eq!(lf.kind, "user");
    let folder = listed.iter().find(|l| l.id == "folder:Folders/Projects").unwrap();
    assert_eq!(folder.kind, "folder");
}

/// (Phase 3 run 4, live) Create a scratch label, COPY a scratch INBOX
/// message into it, sync and observe the `lf:` label, remove the label
/// (expunging only that copy), then delete the scratch label. Operates
/// ONLY on copies this test creates — the seeded fixtures are never
/// moved, flagged or expunged. Panics loudly if cleanup cannot finish.
/// Skips cleanly without docker.
#[tokio::test]
async fn live_provider_adds_removes_and_deletes_a_user_label() {
    let Some(config) = gated() else { return };
    let provider = live_provider(config.clone());

    // Baseline + a few polls so INBOX and Labels/Clients are synced.
    let mut cursor = provider.baseline_cursor().await.expect("baseline");
    for _ in 0..5 {
        cursor = provider.poll(&cursor).await.expect("poll").cursor;
    }

    // Create OUR OWN scratch copy inside INBOX (UID COPY of a seeded
    // INBOX message to a new UID in INBOX) so we never touch a fixture.
    let seeded = provider.store.locations_in_mailbox("INBOX").expect("inbox");
    assert!(!seeded.is_empty(), "the harness seeds INBOX");
    let source_uid = seeded[0].uid as u32;

    let mut worker = ImapConnectionManager::new(config.clone())
        .connect_leased(
            super::super::super::connection::ConnectionRole::Command,
            USER,
            PASSWORD,
        )
        .await
        .expect("worker login")
        .0;
    // Make a scratch copy inside INBOX; read its new UID from the TYPED
    // COPYUID (the Debug string of the code is not parseable).
    use super::super::super::write::ImapWriteSession as _;
    worker.select("INBOX").await.expect("select inbox");
    let scratch_uid: u32 = worker
        .uid_copy(&source_uid.to_string(), "INBOX")
        .await
        .expect("scratch copy")
        .copyuid
        .expect("the harness advertises UIDPLUS, so COPYUID is returned")
        .dest
        .first()
        .copied()
        .expect("a scratch UID from COPYUID");

    // A scratch label name unlikely to collide with a fixture.
    let label_name = "ScratchP3R4";
    let label_mailbox = format!("Labels/{label_name}");

    // Guarantee cleanup even on assertion failure: a guard that removes
    // the scratch INBOX copy and deletes the scratch label mailbox.
    let cleanup = |config: super::super::super::connection::ConnectionConfig| async move {
        let mut c = ImapConnectionManager::new(config)
            .connect_leased(
                super::super::super::connection::ConnectionRole::Command,
                USER,
                PASSWORD,
            )
            .await
            .expect("cleanup login")
            .0;
        // Remove the scratch INBOX copy.
        c.run_command_capture_code("SELECT INBOX").await.expect("select inbox");
        let _ = c
            .run_command_capture_code(&format!("UID STORE {scratch_uid} +FLAGS.SILENT (\\Deleted)"))
            .await;
        let _ = c.run_command_capture_code(&format!("UID EXPUNGE {scratch_uid}")).await;
        // Delete the scratch label mailbox (after selecting INBOX).
        let _ = c.run_command_capture_code(&format!("DELETE \"{label_mailbox}\"")).await;
        let _ = c.logout().await;
    };

    // Create the label through the provider; add it to the scratch copy.
    let created = provider.create_label(label_name).await.expect("create_label");
    assert_eq!(created.id, format!("lf:{label_name}"));

    // Resync so the provider learns the scratch INBOX copy's id/thread.
    for _ in 0..6 {
        cursor = provider.poll(&cursor).await.expect("poll").cursor;
    }
    // Find the stable id of the scratch INBOX copy by its UID.
    let scratch_id = provider
        .store
        .locations_in_mailbox("INBOX")
        .expect("inbox")
        .into_iter()
        .find(|l| l.uid as u32 == scratch_uid)
        .map(|l| l.message_id);
    let scratch_id = match scratch_id {
        Some(id) => id,
        None => {
            cleanup(config.clone()).await;
            panic!("the scratch INBOX copy was not synced");
        }
    };

    // Add the label, sync, and observe lf:<name> on the message.
    if let Err(error) = provider.modify_messages(std::slice::from_ref(&scratch_id), std::slice::from_ref(&created.id), &[]).await {
        cleanup(config.clone()).await;
        panic!("add label failed: {error:?}");
    }
    for _ in 0..6 {
        cursor = provider.poll(&cursor).await.expect("poll").cursor;
    }
    let thread = provider
        .store
        .thread_of_message(&scratch_id)
        .expect("thread")
        .expect("a thread");
    let messages = provider.fetch_thread(&thread).await.expect("fetch_thread");
    let labelled = messages.iter().any(|m| m.label_ids.contains(&created.id));
    if !labelled {
        cleanup(config.clone()).await;
        panic!("the lf: label was not observed after adding it");
    }

    // Remove the label (expunges only the label copy), then delete the
    // now-empty scratch label.
    if let Err(error) = provider.modify_messages(std::slice::from_ref(&scratch_id), &[], std::slice::from_ref(&created.id)).await {
        cleanup(config.clone()).await;
        panic!("remove label failed: {error:?}");
    }
    if let Err(error) = provider.delete_label(&created.id).await {
        cleanup(config.clone()).await;
        panic!("delete_label failed: {error:?}");
    }

    // Clean up the scratch INBOX copy (the label mailbox is already
    // deleted by delete_label). Panic loudly if cleanup fails.
    let mut c = ImapConnectionManager::new(config.clone())
        .connect_leased(
            super::super::super::connection::ConnectionRole::Command,
            USER,
            PASSWORD,
        )
        .await
        .expect("final cleanup login")
        .0;
    c.run_command_capture_code("SELECT INBOX").await.expect("select inbox");
    c.run_command_capture_code(&format!("UID STORE {scratch_uid} +FLAGS.SILENT (\\Deleted)"))
        .await
        .expect("flag scratch copy deleted");
    c.run_command_capture_code(&format!("UID EXPUNGE {scratch_uid}"))
        .await
        .expect("expunge scratch copy — cleanup must succeed");
    let _ = c.logout().await;
    let _ = worker.logout().await;
}
