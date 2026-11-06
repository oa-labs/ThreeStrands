//! Identity compatibility fixtures through the real wire parser, durable store,
//! and sync engine. No extension implementation is implied by advertising it.
use super::*;
use crate::provider::imap::{
    fetch,
    identity::{derive_message_id, fixtures::Corpus},
};

const ACCOUNT: &str = "me@example.com";

#[tokio::test]
async fn identity_fixtures_match_on_body_and_index_fetches_with_optional_capabilities() {
    let corpus = Corpus::load();
    for profile in &corpus.profiles {
        let mut mailbox = FakeMailbox::new(100);
        mailbox.capabilities = profile.capabilities.clone();
        for message in &corpus.messages {
            mailbox.add(&["\\Seen"], &message.raw);
        }
        let counts = mailbox.command_counts.clone();
        let mut session = open_scripted_session(mailbox).await;
        let capabilities = session.capabilities().await.unwrap();
        assert_eq!(
            capabilities.iter().any(|cap| cap.contains("X-GM-EXT-1")),
            profile.capabilities.iter().any(|cap| cap == "X-GM-EXT-1"),
            "{} really advertises its configured capabilities",
            profile.name,
        );
        let uids = (1..=corpus.messages.len())
            .map(|uid| uid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let body_tier = fetch::fetch_identity(session.as_mut(), &uids)
            .await
            .unwrap();
        let index_tier = fetch::fetch_identity_index(session.as_mut(), &uids)
            .await
            .unwrap();
        assert_eq!(body_tier.len(), corpus.messages.len());
        assert_eq!(index_tier.len(), corpus.messages.len());
        for ((body, (index, _)), expected) in
            body_tier.iter().zip(&index_tier).zip(&corpus.messages)
        {
            assert_eq!(body, index, "{} / {}", profile.name, expected.key);
            assert_eq!(
                derive_message_id(ACCOUNT, &body.inputs),
                expected.expected_id,
                "{}",
                expected.key
            );
            // The live adapter currently ignores OBJECTID even when advertised.
            assert_eq!(body.inputs.email_id, None);
        }
        assert_eq!(
            index_tier[1].1.in_reply_to.as_deref(),
            Some("<Root@Example.test>")
        );
        assert_eq!(
            counts.lock().unwrap()["INBOX"].1,
            0,
            "identity passes never download bodies"
        );
        session.logout().await.unwrap();
    }
}

#[tokio::test]
async fn identity_fixtures_preserve_workflow_links_across_move_reset_and_database_reopen() {
    let corpus = Corpus::load();
    for profile in &corpus.profiles {
        let path = crate::db::test_support::TempDbPath::new();
        let mut mailbox = FakeMailbox::new(100);
        mailbox.add(&[], &corpus.message("root").raw);
        mailbox.add(&[], &corpus.message("missing-id").raw);
        mailbox.add_to("Sent", 90, &["\\Seen"], &corpus.message("root").raw);
        mailbox.folder("Archive", 80);
        let mailbox = Arc::new(Mutex::new(mailbox));
        let root_id = &corpus.message("root").expected_id;
        let missing_id = &corpus.message("missing-id").expected_id;
        let root_thread;
        let missing_thread;
        let body_fetches;
        {
            let db = Arc::new(Database::open(&path.path).unwrap());
            let (provider, store) = provider_with_database(mailbox.clone(), db.clone());
            sync_n(&provider, db.as_ref(), 1).await;
            root_thread = db.get_thread_for_message(root_id).unwrap().thread.id;
            missing_thread = db.get_thread_for_message(missing_id).unwrap().thread.id;
            assert_eq!(
                store.locations_for_message(root_id).unwrap().len(),
                2,
                "one message in Inbox and Sent"
            );
            assert_eq!(db.get_thread(&root_thread).unwrap().messages.len(), 1);
            db.test_with_connection(|connection| {
                connection.execute(
                    "INSERT INTO tasks(id,account_id,thread_id,source_message_id,title,kind,due_kind,status,created_at,updated_at)
                     VALUES ('identity-task',?1,?2,?3,'Preserve source','action','none','open','now','now')",
                    rusqlite::params![ACCOUNT, root_thread, root_id],
                )?;
                Ok(())
            }).unwrap();
            let server = mailbox.lock().unwrap();
            body_fetches = server.counts("INBOX").1 + server.counts("Sent").1;
            assert_eq!(
                body_fetches, 2,
                "overlapping locations reuse the cached body"
            );
        } // Close SQLite as well as the provider; only disk and server survive.
        {
            let mut server = mailbox.lock().unwrap();
            server.capabilities = profile.capabilities.clone();
            // Root moves to Archive; no-ID mail survives an INBOX UID reset.
            let inbox = server.inbox();
            inbox.uidvalidity = 200;
            inbox.messages.remove(0);
            inbox.messages[0].uid = 71;
            inbox.uidnext = 72;
            let archive = server.folder("Archive", 80);
            archive.uidnext = 41;
            archive.add_sized(&["\\Seen"], &corpus.message("root").raw, None);
        }
        let db = Arc::new(Database::open(&path.path).unwrap());
        let (provider, store) = provider_with_database(mailbox.clone(), db.clone());
        sync_n(&provider, db.as_ref(), 2).await;
        assert_eq!(
            db.get_thread_for_message(root_id).unwrap().thread.id,
            root_thread,
            "{}",
            profile.name
        );
        assert_eq!(
            db.get_thread_for_message(missing_id).unwrap().thread.id,
            missing_thread
        );
        assert_eq!(store.location_message_id("INBOX", 100, 1).unwrap(), None);
        assert_eq!(
            store
                .location_message_id("INBOX", 200, 71)
                .unwrap()
                .as_deref(),
            Some(missing_id.as_str())
        );
        assert_eq!(
            store
                .location_message_id("Archive", 80, 41)
                .unwrap()
                .as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(db.list_all_mail(Some(ACCOUNT)).unwrap().len(), 2);
        db.test_with_connection(|connection| {
            let links: (String, String) = connection.query_row(
                "SELECT thread_id,source_message_id FROM tasks WHERE id='identity-task'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(links, (root_thread, root_id.clone()));
            Ok(())
        })
        .unwrap();
        let server = mailbox.lock().unwrap();
        assert_eq!(
            server.counts("INBOX").1 + server.counts("Sent").1 + server.counts("Archive").1,
            body_fetches,
            "restart, capability advertisement, and UID reset must not refetch known bodies"
        );
    }
}

#[tokio::test]
async fn identity_fixtures_do_not_reuse_the_previous_uidvalidity_owner() {
    let corpus = Corpus::load();
    for profile in &corpus.profiles {
        let mut mailbox = FakeMailbox::new(100);
        mailbox.capabilities = profile.capabilities.clone();
        assert_eq!(mailbox.add(&[], &corpus.message("root").raw), 1);
        let mailbox = Arc::new(Mutex::new(mailbox));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        let root = &corpus.message("root").expected_id;
        assert_eq!(
            store.location_message_id("INBOX", 100, 1).unwrap().as_ref(),
            Some(root)
        );
        {
            let mut server = mailbox.lock().unwrap();
            let inbox = server.inbox();
            inbox.uidvalidity = 200;
            inbox.messages.clear();
            inbox.uidnext = 1;
            assert_eq!(server.add(&[], &corpus.message("unrelated").raw), 1);
        }
        sync_n(&provider, db.as_ref(), 1).await;
        let new_owner = &corpus.message("unrelated").expected_id;
        assert_ne!(root, new_owner);
        assert_eq!(store.location_message_id("INBOX", 100, 1).unwrap(), None);
        assert_eq!(
            store.location_message_id("INBOX", 200, 1).unwrap().as_ref(),
            Some(new_owner)
        );
        let detail = db.get_thread_for_message(new_owner).unwrap();
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(
            detail.messages[0].body_text.trim(),
            "unrelated body",
            "a recycled UID cannot select the old owner's body"
        );
        assert_eq!(mailbox.lock().unwrap().counts("INBOX").1, 2);
    }
}

#[tokio::test]
async fn identity_fixtures_keep_same_subject_mail_separate_and_join_a_late_parent() {
    let corpus = Corpus::load();
    for profile in &corpus.profiles {
        let mut mailbox = FakeMailbox::new(100);
        mailbox.capabilities = profile.capabilities.clone();
        for key in ["root", "unrelated", "late-reply"] {
            mailbox.add(&[], &corpus.message(key).raw);
        }
        let mailbox = Arc::new(Mutex::new(mailbox));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        let thread_for = |key| {
            db.get_thread_for_message(&corpus.message(key).expected_id)
                .unwrap()
                .thread
                .id
        };
        let root = thread_for("root");
        let unrelated = thread_for("unrelated");
        let late = thread_for("late-reply");
        assert_ne!(root, unrelated, "subject alone is never identity");
        {
            let mut server = mailbox.lock().unwrap();
            for key in ["reply", "late-parent"] {
                server.add(&[], &corpus.message(key).raw);
            }
        }
        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(thread_for("reply"), root);
        assert_eq!(
            thread_for("late-parent"),
            late,
            "late parent retains the existing reply's thread ID"
        );
        assert_eq!(thread_for("unrelated"), unrelated);
        assert_eq!(db.list_all_mail(Some(ACCOUNT)).unwrap().len(), 3);
        assert_eq!(db.get_thread(&root).unwrap().messages.len(), 2);
        assert_eq!(db.get_thread(&late).unwrap().messages.len(), 2);
    }
}
