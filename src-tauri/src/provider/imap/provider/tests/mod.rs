use super::*;
use async_trait::async_trait;

use crate::provider::{
    LabelModel, MailFetch, MailMutate, MailProvider, MailSync, ProviderError, ProviderResult,
    SyncCursor,
};

use crate::db::Database;
use crate::provider::imap::fetch::BodyCache;
use crate::provider::imap::policy::{self, SyncLimits};
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::settings::{ImapAccountSettings, LabelStorage};
use crate::provider::imap::threading::{self, ThreadingInput};
use crate::provider::imap::{ImapLocation, ImapStateStore, SyncRoundWrite};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// One seeded message in a fake folder: its UID, flags, and raw RFC 5322
/// bytes. `advertised_size`, when set, overrides the RFC822.SIZE the
/// identity pass reports (used to simulate an oversize message without
/// allocating 64 MiB).
#[derive(Clone)]
struct FakeMessage {
    uid: u32,
    flags: Vec<String>,
    raw: String,
    advertised_size: Option<usize>,
}

/// One named mailbox (folder) on the fake server: its UIDVALIDITY, next
/// UID, and messages. `fail_after_body_fetches`, when set, makes the server
/// DROP the connection after that many whole-body fetches in one session.
#[derive(Clone, Default)]
struct FakeFolder {
    uidvalidity: u32,
    uidnext: u32,
    messages: Vec<FakeMessage>,
    fail_after_body_fetches: Option<usize>,
    /// EXAMINE of this folder answers `NO [NONEXISTENT]` (a catalog row
    /// whose mailbox was renamed or deleted on the server).
    examine_fails: bool,
    /// `UID SEARCH` in this folder never answers, and the server stops
    /// serving the connection (a stalled server). Search-deadline tests.
    search_stalls: bool,
}

impl FakeFolder {
    fn new(uidvalidity: u32) -> Self {
        Self {
            uidvalidity,
            uidnext: 1,
            messages: Vec::new(),
            fail_after_body_fetches: None,
            examine_fails: false,
            search_stalls: false,
        }
    }
    fn add_sized(&mut self, flags: &[&str], raw: &str, advertised_size: Option<usize>) -> u32 {
        let uid = self.uidnext;
        self.uidnext += 1;
        self.messages.push(FakeMessage {
            uid,
            flags: flags.iter().map(|f| f.to_string()).collect(),
            raw: raw.to_string(),
            advertised_size,
        });
        uid
    }
}

/// A tiny in-memory server with one or more named mailboxes, shared
/// (Arc<Mutex>) so a test can mutate it (add mail, change a flag, expunge,
/// move a message between folders) between polls and the next opened
/// session sees the change. `FakeMailbox::new` creates an INBOX, and the
/// backward-compatible `add`/`add_sized` operate on INBOX, so the Slice 5a
/// tests are unchanged; Slice 5b-1 tests add Trash/Junk/Sent folders.
///
/// `command_counts` records how many SEARCH / whole-body / identity fetches
/// the server answered per mailbox, so a cadence test can assert that an
/// unchanged, not-due folder issued no SEARCH/FETCH.
#[derive(Clone, Default)]
struct FakeMailbox {
    folders: std::collections::BTreeMap<String, FakeFolder>,
    /// Per-mailbox `(searches, body_fetches, identity_fetches)` counters,
    /// accumulated across every session opened against this snapshot's
    /// shared handle. Shared so a test reads them after sync.
    command_counts: Arc<Mutex<std::collections::BTreeMap<String, (usize, usize, usize)>>>,
    /// When true, the server answers `LIST` with a NO (catalog refresh must
    /// swallow it and never abort the poll).
    list_fails: bool,
    /// Extra `\Noselect` container names the server reports in `LIST` (they
    /// must never be added to the catalog).
    noselect: Vec<String>,
    logout_stalls: bool,
}

impl FakeMailbox {
    /// A server with just an INBOX at the given UIDVALIDITY.
    fn new(uidvalidity: u32) -> Self {
        let mut folders = std::collections::BTreeMap::new();
        folders.insert("INBOX".to_string(), FakeFolder::new(uidvalidity));
        Self {
            folders,
            command_counts: Arc::new(Mutex::new(Default::default())),
            list_fails: false,
            noselect: Vec::new(),
            logout_stalls: false,
        }
    }
    /// Ensure a folder exists at `uidvalidity`, returning a mutable handle.
    fn folder(&mut self, name: &str, uidvalidity: u32) -> &mut FakeFolder {
        self.folders
            .entry(name.to_string())
            .or_insert_with(|| FakeFolder::new(uidvalidity))
    }
    /// Mutable INBOX handle (always present).
    fn inbox(&mut self) -> &mut FakeFolder {
        self.folders.get_mut("INBOX").expect("INBOX always present")
    }
    fn add(&mut self, flags: &[&str], raw: &str) -> u32 {
        self.inbox().add_sized(flags, raw, None)
    }
    fn add_sized(&mut self, flags: &[&str], raw: &str, advertised_size: Option<usize>) -> u32 {
        self.inbox().add_sized(flags, raw, advertised_size)
    }
    /// Add a message to a named folder (created at `uidvalidity` if new).
    fn add_to(&mut self, mailbox: &str, uidvalidity: u32, flags: &[&str], raw: &str) -> u32 {
        self.folder(mailbox, uidvalidity).add_sized(flags, raw, None)
    }
    /// The command counts seen for a mailbox so far.
    fn counts(&self, mailbox: &str) -> (usize, usize, usize) {
        self.command_counts
            .lock()
            .unwrap()
            .get(mailbox)
            .copied()
            .unwrap_or((0, 0, 0))
    }
}

/// A scripted IMAP server that answers EXAMINE / UID SEARCH / UID FETCH /
/// LOGOUT from a `FakeMailbox` snapshot, over a tokio duplex stream.
/// EXAMINE selects the active folder; subsequent SEARCH/FETCH act on it. It
/// parses commands by TYPE, so it is robust to order and batching.
fn spawn_server(mailbox: FakeMailbox) -> (DuplexStream, tokio::task::JoinHandle<()>) {
    let (client, server) = tokio::io::duplex(1 << 20);
    let counts = mailbox.command_counts.clone();
    let folders = mailbox.folders.clone();
    let list_fails = mailbox.list_fails;
    let noselect = mailbox.noselect.clone();
    let logout_stalls = mailbox.logout_stalls;
    let task = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
        let mut body_fetches = 0usize;
        // The currently EXAMINEd folder; defaults to INBOX.
        let mut selected = "INBOX".to_string();
        loop {
            let mut line = String::new();
            if server.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let line = line.trim_end();
            let Some((tag, rest)) = line.split_once(' ') else {
                continue;
            };
            let upper = rest.to_ascii_uppercase();
            let empty = FakeFolder::default();
            if upper.starts_with("LOGIN") {
                reply(&mut server, &format!("{tag} OK logged in\r\n")).await;
            } else if upper.starts_with("LIST") {
                if list_fails {
                    reply(&mut server, &format!("{tag} NO LIST failed\r\n")).await;
                    continue;
                }
                // Enumerate every folder with its inferred special-use
                // attribute, so the catalog refresh can notice new ones.
                let mut body = String::new();
                for name in folders.keys() {
                    let attrs = infer_special_use(name)
                        .map(|a| format!("{a} "))
                        .unwrap_or_default();
                    body.push_str(&format!("* LIST ({}) \"/\" \"{}\"\r\n", attrs.trim(), name));
                }
                // Any \Noselect containers the server advertises.
                for name in &noselect {
                    body.push_str(&format!("* LIST (\\Noselect) \"/\" \"{name}\"\r\n"));
                }
                body.push_str(&format!("{tag} OK done\r\n"));
                reply(&mut server, &body).await;
            } else if upper.starts_with("EXAMINE") {
                // "EXAMINE <mailbox>" — select it. The name may be quoted.
                let name = rest["EXAMINE".len().min(rest.len())..]
                    .trim()
                    .trim_matches('"')
                    .to_string();
                selected = if name.is_empty() { "INBOX".into() } else { name };
                let folder = folders.get(&selected).unwrap_or(&empty);
                if folder.examine_fails {
                    reply(
                        &mut server,
                        &format!("{tag} NO [NONEXISTENT] Mailbox does not exist\r\n"),
                    )
                    .await;
                    continue;
                }
                let body = format!(
                    "* {} EXISTS\r\n* OK [UIDVALIDITY {}] .\r\n* OK [UIDNEXT {}] .\r\n{tag} OK [READ-ONLY] done\r\n",
                    folder.messages.len(),
                    folder.uidvalidity,
                    folder.uidnext
                );
                reply(&mut server, &body).await;
            } else if upper.starts_with("UID SEARCH") {
                let folder = folders.get(&selected).unwrap_or(&empty);
                counts.lock().unwrap().entry(selected.clone()).or_default().0 += 1;
                if folder.search_stalls {
                    // Never answer anything again on this connection.
                    std::future::pending::<()>().await;
                }
                let uids: Vec<String> =
                    folder.messages.iter().map(|m| m.uid.to_string()).collect();
                let body = format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" "));
                reply(&mut server, &body).await;
            } else if upper.starts_with("UID FETCH") {
                let folder = folders.get(&selected).unwrap_or(&empty);
                let wants_body = upper.contains("BODY.PEEK[]");
                let is_identity = upper.contains("HEADER.FIELDS");
                {
                    let mut c = counts.lock().unwrap();
                    let entry = c.entry(selected.clone()).or_default();
                    if wants_body {
                        entry.1 += 1;
                    } else if is_identity {
                        entry.2 += 1;
                    }
                }
                if wants_body {
                    if let Some(limit) = folder.fail_after_body_fetches {
                        if body_fetches >= limit {
                            let uid = parse_uid_set(rest, folder)
                                .first()
                                .copied()
                                .unwrap_or(0);
                            reply(
                                &mut server,
                                &format!("* 1 FETCH (UID {uid} BODY[]<0> {{4096}}\r\n"),
                            )
                            .await;
                            break;
                        }
                    }
                    body_fetches += 1;
                }
                let flags_only = !wants_body && !is_identity;
                let set = parse_uid_set(rest, folder);
                let mut out = String::new();
                for (seq, uid) in set.iter().enumerate() {
                    if let Some(message) = folder.messages.iter().find(|m| &m.uid == uid) {
                        out.push_str(&fetch_response(
                            seq as u32 + 1,
                            message,
                            wants_body,
                            flags_only,
                        ));
                    }
                }
                out.push_str(&format!("{tag} OK done\r\n"));
                reply(&mut server, &out).await;
            } else if upper.starts_with("LOGOUT") {
                if logout_stalls {
                    // Keep the connection open without answering cleanup.
                    let mut byte = [0u8; 1];
                    let _ = tokio::io::AsyncReadExt::read(&mut server, &mut byte).await;
                    break;
                }
                reply(&mut server, &format!("* BYE\r\n{tag} OK logout\r\n")).await;
                break;
            } else {
                reply(&mut server, &format!("{tag} OK done\r\n")).await;
            }
        }
    });
    (client, task)
}

async fn reply(server: &mut BufReader<DuplexStream>, text: &str) {
    server.get_mut().write_all(text.as_bytes()).await.unwrap();
}

/// Expand a UID set token ("1", "1,2,3") against a folder.
fn parse_uid_set(command: &str, folder: &FakeFolder) -> Vec<u32> {
    // The set is the token after "UID FETCH ".
    let after = command["UID FETCH ".len().min(command.len())..].trim_start();
    let set_token = after.split_whitespace().next().unwrap_or("");
    let mut uids = Vec::new();
    for part in set_token.split(',') {
        if let Ok(uid) = part.parse::<u32>() {
            if folder.messages.iter().any(|m| m.uid == uid) {
                uids.push(uid);
            }
        }
    }
    uids
}

/// Render one FETCH response line for a message. `wants_body` is the whole
/// `BODY.PEEK[]` pass; `flags_only` is the FLAGS-only sweep; otherwise it
/// is the identity pass.
fn fetch_response(seq: u32, message: &FakeMessage, wants_body: bool, flags_only: bool) -> String {
    let flags = message.flags.join(" ");
    if wants_body {
        format!(
            "* {seq} FETCH (UID {} BODY[]<0> {{{}}}\r\n{})\r\n",
            message.uid,
            message.raw.len(),
            message.raw
        )
    } else if flags_only {
        format!("* {seq} FETCH (UID {} FLAGS ({}))\r\n", message.uid, flags)
    } else {
        // Header fields block = the message's header section (up to the
        // blank line). The real server returns only the requested fields,
        // but returning the whole header block is a valid superset the
        // parser tolerates.
        let header_block = message
            .raw
            .split_once("\r\n\r\n")
            .map(|(h, _)| format!("{h}\r\n\r\n"))
            .unwrap_or_else(|| message.raw.clone());
        let size = message.advertised_size.unwrap_or(message.raw.len());
        format!(
            "* {seq} FETCH (UID {} FLAGS ({}) RFC822.SIZE {} BODY[HEADER.FIELDS (MESSAGE-ID DATE FROM SUBJECT)] {{{}}}\r\n{})\r\n",
            message.uid,
            flags,
            size,
            header_block.len(),
            header_block
        )
    }
}

/// A session source that, each time it is opened, logs into a freshly
/// spawned scripted server serving the CURRENT shared mailbox snapshot.
struct FakeSource {
    mailbox: Arc<Mutex<FakeMailbox>>,
}

#[async_trait]
impl ImapSessionSource for FakeSource {
    async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
        let snapshot = self.mailbox.lock().unwrap().clone();
        let (client, _task) = spawn_server(snapshot);
        let session = async_imap::Client::new(client)
            .login("user", "password")
            .await
            .map_err(|(error, _)| super::super::error::map_imap_error(&error))?;
        Ok(Box::new(super::super::session::AsyncImapSession::new(session)))
    }
}

/// Open a bare scripted session over a one-shot server serving `mailbox`,
/// for tests that drive `refresh_inbox_with` directly (so a failure can be
/// injected into ONE round).
async fn open_scripted_session(mailbox: FakeMailbox) -> Box<dyn ImapSession> {
    let (client, _task) = spawn_server(mailbox);
    let session = async_imap::Client::new(client)
        .login("user", "password")
        .await
        .map_err(|(error, _)| super::super::error::map_imap_error(&error))
        .expect("scripted login");
    Box::new(super::super::session::AsyncImapSession::new(session))
}

/// Build an ImapProvider backed by the shared fake mailbox and an
/// in-memory database. Seeds a catalog row for every folder present on the
/// fake server (inferring the special-use attribute from the folder name)
/// so the sync plan resolves Trash/Junk/Sent roles exactly as discovery
/// would. INBOX-only tests get an INBOX catalog row and nothing else.
fn provider_with(mailbox: Arc<Mutex<FakeMailbox>>) -> (ImapProvider, ImapStateStore) {
    let database = Arc::new(Database::open_memory());
    database
        .adopt_mail_account("me@example.com", crate::models::MailProviderKind::Imap)
        .unwrap();
    let store = ImapStateStore::new(database.clone(), "me@example.com");

    // Seed the catalog from the fake server's folders.
    for (name, folder) in &mailbox.lock().unwrap().folders {
        let special_use = infer_special_use(name);
        store
            .upsert_mailbox(&super::super::ImapMailbox {
                name: name.clone(),
                delimiter: Some("/".into()),
                special_use,
                uidvalidity: folder.uidvalidity as i64,
                uidnext: folder.uidnext as i64,
                highestmodseq: None,
                permanent_flags_json: None,
                permanent_keywords: None,
            })
            .unwrap();
    }

    let cache = BodyCache::new(store.clone());
    let provider = ImapProvider {
        store: store.clone(),
        cache,
        username: "me@example.com".into(),
        account_id: "me@example.com".into(),
        smtp_password: "test-password".into(),
        label_model: LabelModel::ImapLabelFolders,
        settings: test_settings(),
        source: Arc::new(FakeSource { mailbox }),
        now: || 1_700_000_000,
        limits: SyncLimits::inbox(),
        sent_round_budget: None,
        folder_window_override: None,
        extra_synced_candidates: None,
        force_user_folders: None,
        folder_rounds_per_poll_override: None,
        search_deadline_override: None,
        thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
    };
    (provider, store)
}

/// Infer an RFC 6154 attribute from a well-known folder name for the test
/// catalog (so the plan resolves roles by attribute, the primary path).
fn infer_special_use(name: &str) -> Option<String> {
    match name {
        "Trash" => Some("\\Trash".into()),
        "Junk" | "Spam" => Some("\\Junk".into()),
        "Sent" => Some("\\Sent".into()),
        "Archive" => Some("\\Archive".into()),
        "Drafts" => Some("\\Drafts".into()),
        "All Mail" => Some("\\All".into()),
        _ => None,
    }
}

/// Minimal non-secret settings for the fake provider.
fn test_settings() -> ImapAccountSettings {
    use super::super::settings::SecurityMode;
    ImapAccountSettings {
        imap_host: "127.0.0.1".into(),
        imap_port: 1143,
        imap_security: SecurityMode::StartTls,
        imap_username: "me@example.com".into(),
        smtp_host: "127.0.0.1".into(),
        smtp_port: 1025,
        smtp_security: SecurityMode::StartTls,
        smtp_username: "me@example.com".into(),
        mailbox_overrides: Default::default(),
        archive_mailbox: None,
        label_storage: LabelStorage::Folders,
        label_container: Some("Labels".into()),
        identities: vec![],
        pinned_fingerprints: Default::default(),
        server_saves_sent: false,
    }
}

fn message(message_id: &str, subject: &str, extra_headers: &str) -> String {
    format!(
        "Message-ID: {message_id}\r\nDate: Mon, 06 Oct 2025 09:00:00 +0000\r\nFrom: a@b.com\r\nSubject: {subject}\r\n{extra_headers}\r\n{subject} body",
    )
}

#[tokio::test]
async fn initial_full_sync_ingests_inbox_threads_through_the_engine() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "Hello", ""));
    mb.add(&["\\Seen"], &message("<m2@x>", "Second", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox);

    // Drive the real engine loop. `sync_with` needs a Database handle; the
    // provider carries its own store over the same in-memory db.
    let changed = crate::sync::sync_with(provider.store.database().as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert!(changed, "a full sync reports a change");

    // Two INBOX threads are now listed from the local store.
    let threads = provider.list_inbox(None).await.unwrap();
    assert_eq!(threads.thread_ids.len(), 2);

    // Each thread's messages are fetchable with the right labels.
    for thread_id in &threads.thread_ids {
        let messages = provider.fetch_thread(thread_id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].label_ids.contains(&"INBOX".to_string()));
    }
    // The generation advanced and both messages are cached once.
    assert!(store.generation().unwrap() >= 1);
}

#[tokio::test]
async fn a_second_poll_sees_new_mail_a_flag_change_and_a_deletion() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "One", ""));
    let uid2 = mb.add(&[], &message("<m2@x>", "Two", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store) = provider_with(mailbox.clone());
    let db = provider.store.database().clone();

    // First sync establishes the cursor and ingests both.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let cursor_after_first = db.cursor("me@example.com").unwrap().unwrap();

    // Mutate the server: new mail, flag change on m1, expunge m2.
    {
        let mut mb = mailbox.lock().unwrap();
        mb.add(&[], &message("<m3@x>", "Three", ""));
        if let Some(first) = mb.inbox().messages.iter_mut().find(|m| m.uid == 1) {
            first.flags = vec!["\\Seen".into()];
        }
        mb.inbox().messages.retain(|m| m.uid != uid2);
    }

    // Second sync ingests the delta.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let cursor_after_second = db.cursor("me@example.com").unwrap().unwrap();
    assert_ne!(cursor_after_first, cursor_after_second, "the generation advanced");

    // m3 is now present; m2's thread lost its only location.
    let threads = provider.list_inbox(None).await.unwrap();
    // Two live INBOX threads remain (m1 and m3); m2 was expunged.
    assert_eq!(threads.thread_ids.len(), 2);
}

#[tokio::test]
async fn an_ingest_failure_then_a_re_poll_with_the_same_cursor_loses_nothing() {
    // The at-least-once property: polling twice with the SAME cursor
    // returns the same changes, so an ingest that failed before the engine
    // persisted the cursor is retried intact.
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "One", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox);

    // Refresh once so m1 is journalled under generation 1.
    provider.baseline_cursor().await.unwrap();
    let before = store.generation().unwrap();
    assert!(before >= 1);

    // Poll from generation 0 (the pre-ingest position): m1's thread is
    // returned because it was journalled at a generation > 0.
    let g0 = SyncCursor::from_generation(0);
    let batch1 = provider.poll(&g0).await.unwrap();
    assert_eq!(batch1.changed_threads.len(), 1, "m1's thread is returned");
    assert!(!batch1.more);

    // Simulate the engine NOT persisting the new cursor (ingest failed),
    // then polling AGAIN with the SAME cursor g0. The same thread must
    // come back — nothing is lost.
    let batch2 = provider.poll(&g0).await.unwrap();
    assert_eq!(
        batch1.changed_threads, batch2.changed_threads,
        "the same cursor returns the same changes (at-least-once)"
    );
}

#[tokio::test]
async fn poll_rejects_a_non_generation_or_future_cursor_as_invalid() {
    let mb = FakeMailbox::new(100);
    let (provider, _store) = provider_with(Arc::new(Mutex::new(mb)));
    // A non-generation cursor (e.g. a Gmail historyId) is InvalidCursor.
    let err = provider
        .poll(&SyncCursor::new("12345 pagetok"))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidCursor));
    // A cursor newer than the store's generation (store reset) is invalid.
    let err = provider
        .poll(&SyncCursor::from_generation(999))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidCursor));
}

#[tokio::test]
async fn a_uidvalidity_reset_drops_locations_but_keeps_cached_bodies() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "One", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = provider.store.database().clone();

    provider.baseline_cursor().await.unwrap();
    let before_locations = store.locations_in_mailbox("INBOX").unwrap();
    assert_eq!(before_locations.len(), 1);
    let cached_before = store
        .locations_for_message(&before_locations[0].message_id)
        .unwrap();
    assert_eq!(cached_before.len(), 1);
    let message_id = before_locations[0].message_id.clone();
    assert!(provider.cache.contains(&message_id).unwrap());

    // Bump UIDVALIDITY and re-seed the SAME message at a new UID.
    {
        let mut mb = mailbox.lock().unwrap();
        mb.inbox().uidvalidity = 200;
        mb.inbox().uidnext = 1;
        mb.inbox().messages.clear();
        mb.add(&[], &message("<m1@x>", "One", ""));
    }
    provider.baseline_cursor().await.unwrap();

    // The body cache survived the reset (keyed by the stable id).
    assert!(
        provider.cache.contains(&message_id).unwrap(),
        "a UIDVALIDITY reset keeps cached bodies"
    );
    // And the mailbox still has exactly one live location.
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1);
    let _ = db;
}

#[tokio::test]
async fn body_peek_only_the_fetch_items_never_set_seen() {
    // Guiding rule 3, at the provider level: the only body request this
    // provider issues is BODY.PEEK[] (via SessionBodyFetcher/BODY_ITEMS),
    // and sync uses EXAMINE, never SELECT. Prove the item constant is peek.
    assert!(super::super::fetch::BODY_ITEMS.contains("BODY.PEEK["));
    assert!(!super::super::fetch::BODY_ITEMS
        .replace("BODY.PEEK[", "")
        .contains("BODY["));
}

#[tokio::test]
async fn fetch_thread_resolves_an_alias_to_the_surviving_thread() {
    let mb = FakeMailbox::new(100);
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    // Seed a thread with one message, and an alias old -> survivor.
    store
        .upsert_location(&ImapLocation {
            mailbox: "INBOX".into(),
            uidvalidity: 1,
            uid: 1,
            message_id: "imap:me@example.com:m1".into(),
            flags_json: "[\"\\\\Seen\"]".into(),
            modseq: None,
        })
        .unwrap();
    provider
        .cache
        .put(
            "imap:me@example.com:m1",
            message("<m1@x>", "Hi", "").as_bytes(),
            1,
        )
        .unwrap();
    let round = SyncRoundWrite {
        thread_assignments: vec![(
            "imap:me@example.com:m1".into(),
            "imap:t:survivor".into(),
        )],
        aliases: vec![("imap:t:old".into(), "imap:t:survivor".into())],
        changed_threads: vec!["imap:t:survivor".into()],
        ..Default::default()
    };
    store.commit_sync_round(&round).unwrap();

    // Fetching by the OLD (aliased) id resolves to the survivor's messages.
    let messages = provider.fetch_thread("imap:t:old").await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].thread_id, "imap:t:survivor");
}

#[tokio::test]
async fn fetch_thread_of_an_empty_thread_is_not_found() {
    let mb = FakeMailbox::new(100);
    let (provider, _store) = provider_with(Arc::new(Mutex::new(mb)));
    let err = provider.fetch_thread("imap:t:gone").await.unwrap_err();
    assert!(matches!(err, ProviderError::NotFound));
}

#[tokio::test]
async fn capabilities_report_the_neither_tier_and_the_accounts_label_model() {
    let (provider, _store) = provider_with(Arc::new(Mutex::new(FakeMailbox::new(1))));
    let caps = provider.capabilities();
    // Phase 5 run 4: IMAP now finds server-side matches (used when
    // "include archived" is on and for the Sent backfill), so
    // server_search is true.
    assert!(caps.server_search);
    assert!(!caps.provided_threads);
    // Phase 4 run 3: IMAP accounts can now send, so send_supported is true.
    // verifiable_delivery stays false — there is no server-side sent copy
    // to confirm an uncertain send against.
    assert!(caps.send_supported);
    assert!(!caps.verifiable_delivery);
    // Run 4b: IMAP drives its own Sent-copy job after an accepted send.
    assert!(caps.client_saves_sent_copy);
    assert_eq!(caps.label_model, LabelModel::ImapLabelFolders);
}

#[tokio::test]
async fn unsupported_mutations_are_rejected_as_a_permanent_invalid_operation() {
    // Phase 3 run 2 changed the contract: STARRED/UNREAD are now SUPPORTED
    // flag mutations and no longer rejected blanket. What still fails
    // permanently is every OTHER label id (a system location, a user
    // folder, an unknown id) and the label CRUD methods — so the mutation
    // queue rolls those back to `failed` rather than retrying forever
    // (InvalidOperation is neither retryable nor a reauth).
    let (provider, _store) = provider_with(Arc::new(Mutex::new(FakeMailbox::new(1))));

    // An unsupported location/label id is still a permanent rejection,
    // sent through the (supported-label) validation BEFORE any network.
    for bad in [
        vec!["INBOX".to_string()],
        vec!["TRASH".to_string()],
        vec!["SENT".to_string()],
        vec!["SPAM".to_string()],
        vec!["lf:Work".to_string()],
        vec!["folder:Clients/Acme".to_string()],
        vec!["kw:anything".to_string()],
    ] {
        let err = provider
            .modify_messages(&["imap:me@example.com:x".into()], &bad, &[])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::InvalidOperation(_)),
            "{bad:?} must be a permanent InvalidOperation, got {err:?}"
        );
        assert!(!err.retry_mutation());
        assert!(!err.requires_reauthentication());
    }

    // A SUPPORTED flag change on a thread that resolves to NO messages
    // (emptied / unknown) is a permanent InvalidOperation naming the
    // thread — never a silent success.
    let err = provider
        .modify_thread("imap:t:gone", &["STARRED".into()], &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
    assert!(!err.retry_mutation());

    // (Phase 4 run 3 CONTRACT CHANGE: `prepare_delivery` is now
    // IMPLEMENTED — it connects + authenticates — so it no longer returns
    // the old "not supported until phase 4" InvalidOperation. The stale
    // sub-assertion that used to live here has been removed; the real
    // prepare_delivery behaviour is covered by
    // `provider::imap::send::provider_level_tests`.)
    // Label RENAME (`update_label`) is refused PERMANENTLY in run 4 (the
    // `lf:` id is name-derived and there is no label-id migration), and the
    // refusal is decided before any session, so it holds on this fake too.
    let err = provider.update_label("lf:Old", "New").await.unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
    assert!(!err.retry_mutation(), "rename refusal is permanent");
    // list_labels returns the system labels including the Slice 5b-1
    // Sent/Spam/Trash roles.
    let labels = provider.list_labels().await.unwrap();
    let ids: Vec<_> = labels.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["INBOX", "SENT", "SPAM", "TRASH", "UNREAD", "STARRED"]);
}

#[test]
fn label_model_is_chosen_from_the_accounts_label_storage() {
    assert_eq!(label_model_for(LabelStorage::Keywords), LabelModel::ImapKeywords);
    assert_eq!(label_model_for(LabelStorage::Folders), LabelModel::ImapLabelFolders);
    assert_eq!(label_model_for(LabelStorage::None), LabelModel::ImapNoUserLabels);
}

// ---- SLICE5A_FIXES item 1: at-least-once across a mid-round failure ----

/// Snapshot of the durable sync state the round touches, so a test can
/// assert a failed round left NOTHING behind.
fn durable_snapshot(store: &ImapStateStore) -> (u64, Vec<ImapLocation>, Vec<String>) {
    let generation = store.generation().unwrap();
    let locations = store.locations_in_mailbox("INBOX").unwrap();
    let threads: Vec<String> = store
        .all_message_threads()
        .unwrap()
        .into_iter()
        .map(|(m, t, _)| format!("{m}->{t}"))
        .collect();
    (generation, locations, threads)
}

#[tokio::test]
async fn a_failure_after_the_first_batch_of_new_mail_leaves_no_partial_state() {
    // Two new messages; the server drops the connection after the FIRST
    // body fetch. The round must fail having written NOTHING durable, and
    // a re-run against a healthy server must end exactly as a clean round.
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "One", ""));
    mb.add(&[], &message("<m2@x>", "Two", ""));
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb.clone())));

    let before = durable_snapshot(&store);

    // Inject the mid-round drop.
    let mut failing = mb.clone();
    failing.inbox().fail_after_body_fetches = Some(1);
    let mut session = open_scripted_session(failing).await;
    let result = provider.refresh_inbox_with(session.as_mut()).await;
    assert!(result.is_err(), "a dropped connection fails the round");

    // Durable state is UNCHANGED — no location, thread, journal or
    // generation write survived the partial round.
    assert_eq!(durable_snapshot(&store), before, "a failed round writes nothing");

    // Re-run cleanly: both messages are threaded, journaled, ingestible.
    let mut healthy = open_scripted_session(mb).await;
    provider.refresh_inbox_with(healthy.as_mut()).await.unwrap();
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
    let threads = provider.list_inbox(None).await.unwrap();
    assert_eq!(threads.thread_ids.len(), 2);
    for thread_id in &threads.thread_ids {
        assert_eq!(provider.fetch_thread(thread_id).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn a_failure_after_a_flag_update_leaves_no_partial_state() {
    // One message synced; then its flag changes AND the connection drops
    // on the (new) body fetch of a second new message in the same round.
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "One", ""));
    let mailbox = Arc::new(Mutex::new(mb.clone()));
    let (provider, store) = provider_with(mailbox.clone());

    // Clean first round establishes m1.
    let mut s0 = open_scripted_session(mb.clone()).await;
    provider.refresh_inbox_with(s0.as_mut()).await.unwrap();
    let before = durable_snapshot(&store);

    // Next round: m1 is marked \Seen AND a new m2 arrives; the server
    // drops the connection on m2's body fetch, after the flag change was
    // computed but before the atomic commit.
    let mut next = mb.clone();
    next.inbox().messages[0].flags = vec!["\\Seen".into()];
    next.add(&[], &message("<m2@x>", "Two", ""));
    let mut failing = next.clone();
    failing.inbox().fail_after_body_fetches = Some(0); // drop on the first body fetch
    let mut session = open_scripted_session(failing).await;
    assert!(provider.refresh_inbox_with(session.as_mut()).await.is_err());

    // The flag change was NOT applied (round rolled back whole).
    assert_eq!(durable_snapshot(&store), before, "flag change did not leak");

    // A healthy re-run applies both the flag change and the new message.
    let mut healthy = open_scripted_session(next).await;
    provider.refresh_inbox_with(healthy.as_mut()).await.unwrap();
    let m1 = store
        .locations_in_mailbox("INBOX")
        .unwrap()
        .into_iter()
        .find(|l| l.uid == 1)
        .unwrap();
    let flags: Vec<String> = serde_json::from_str(&m1.flags_json).unwrap();
    assert!(flags.iter().any(|f| f == "\\Seen"), "flag change applied on re-run");
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
}

#[tokio::test]
async fn a_failure_after_a_deletion_leaves_no_partial_state() {
    // Two messages synced; then one is expunged AND a new message arrives,
    // and the connection drops on the new body fetch.
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "One", ""));
    mb.add(&[], &message("<m2@x>", "Two", ""));
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb.clone())));

    let mut s0 = open_scripted_session(mb.clone()).await;
    provider.refresh_inbox_with(s0.as_mut()).await.unwrap();
    let before = durable_snapshot(&store);
    assert_eq!(before.1.len(), 2);

    // m2 expunged, m3 new; drop on m3's body fetch.
    let mut next = mb.clone();
    next.inbox().messages.retain(|m| m.uid != 2);
    next.add(&[], &message("<m3@x>", "Three", ""));
    let mut failing = next.clone();
    failing.inbox().fail_after_body_fetches = Some(0);
    let mut session = open_scripted_session(failing).await;
    assert!(provider.refresh_inbox_with(session.as_mut()).await.is_err());

    // The deletion did NOT leak: m2 is still present locally.
    assert_eq!(durable_snapshot(&store), before, "deletion did not leak");

    // Healthy re-run: m2 gone, m3 present.
    let mut healthy = open_scripted_session(next).await;
    provider.refresh_inbox_with(healthy.as_mut()).await.unwrap();
    let uids: Vec<i64> = store
        .locations_in_mailbox("INBOX")
        .unwrap()
        .into_iter()
        .map(|l| l.uid)
        .collect();
    assert!(!uids.contains(&2), "m2 expunged on re-run");
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
}

// ---- SLICE5A_FIXES item 2: one bad message does not wedge the round ----

#[tokio::test]
async fn an_oversize_message_in_the_middle_of_a_batch_does_not_fail_the_round() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<ok1@x>", "First", ""));
    // Middle message advertises a size over the cache limit, so its body
    // is skipped during sync (we never allocate 64 MiB in a unit test).
    let huge_uid = mb.add_sized(
        &[],
        &message("<huge@x>", "Huge", ""),
        Some(super::super::policy::MAX_RAW_MESSAGE_BYTES + 1),
    );
    mb.add(&[], &message("<ok2@x>", "Third", ""));
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb.clone())));

    let mut session = open_scripted_session(mb).await;
    // The round SUCCEEDS despite the oversize message in the middle: one
    // bad message must not wedge or abort the round.
    provider.refresh_inbox_with(session.as_mut()).await.unwrap();

    // All three are recorded as locations (the oversize one too) and
    // threaded — nothing was lost.
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 3);
    let threads = provider.list_inbox(None).await.unwrap();
    assert_eq!(threads.thread_ids.len(), 3, "every message threaded");

    // The oversize message's body was SKIPPED (never cached); the two
    // normal messages' bodies are cached.
    let huge_id = store
        .locations_in_mailbox("INBOX")
        .unwrap()
        .into_iter()
        .find(|l| l.uid == huge_uid as i64)
        .unwrap()
        .message_id;
    assert!(
        !provider.cache.contains(&huge_id).unwrap(),
        "the oversize message's body is skipped, not cached"
    );
    // The two normal messages are fully fetchable.
    let mut fetched = 0;
    for thread_id in &threads.thread_ids {
        if let Ok(messages) = provider.fetch_thread(thread_id).await {
            fetched += messages.len();
        }
    }
    assert!(fetched >= 2, "the two normal messages fetch");
}

// ---- SLICE5A_FIXES item 4: thread age is persisted, not hash order ----

#[tokio::test]
async fn the_genuinely_older_thread_survives_a_merge_regardless_of_id_hash_order() {
    // Two persisted threads whose id-hash order is the OPPOSITE of their
    // creation order. A late linking message must keep the genuinely OLDER
    // thread's id (lower creation generation), alias the other to it, and
    // report both changed.
    let mb = FakeMailbox::new(100);
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));

    // Find two message tokens whose thread ids sort in a known hash order,
    // then seed them with creation generations OPPOSITE to that order.
    use super::super::threading::new_thread_id_for_test;
    let t_a = new_thread_id_for_test("a@x");
    let t_b = new_thread_id_for_test("b@x");
    // Pick the one with the LARGER id hash to be the OLDER thread, so hash
    // order and age disagree.
    let (older_token, older_tid, newer_token, newer_tid) = if t_a > t_b {
        ("a@x", t_a, "b@x", t_b)
    } else {
        ("b@x", t_b, "a@x", t_a)
    };

    // Older thread created at generation 1, newer at generation 2.
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![(
                "imap:me@example.com:older".into(),
                older_tid.clone(),
            )],
            changed_threads: vec![older_tid.clone()],
            ..Default::default()
        })
        .unwrap();
    provider
        .cache
        .put(
            "imap:me@example.com:older",
            message(&format!("<{older_token}>"), "Older", "").as_bytes(),
            1,
        )
        .unwrap();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![(
                "imap:me@example.com:newer".into(),
                newer_tid.clone(),
            )],
            changed_threads: vec![newer_tid.clone()],
            ..Default::default()
        })
        .unwrap();
    provider
        .cache
        .put(
            "imap:me@example.com:newer",
            message(&format!("<{newer_token}>"), "Newer", "").as_bytes(),
            1,
        )
        .unwrap();

    // A late message referencing both links the two threads.
    let mut state = provider.load_thread_state().unwrap();
    let linker = super::super::threading::ThreadingInput {
        message_id: "imap:me@example.com:link".into(),
        message_id_header: Some("<link@x>".into()),
        in_reply_to: None,
        references: Some(format!("<{older_token}> <{newer_token}>")),
    };
    let outcome = super::super::threading::thread_batch(&mut state, &[linker]);

    // The genuinely OLDER thread id survives, the newer is aliased to it,
    // and both are reported changed.
    assert_eq!(
        outcome.assignments[0].1, older_tid,
        "the older thread's id survives the merge"
    );
    assert_eq!(outcome.aliases, vec![(newer_tid.clone(), older_tid.clone())]);
    assert!(outcome.changed_threads.contains(&older_tid));
    assert!(outcome.changed_threads.contains(&newer_tid));
}

// ---- SLICE5A_FIXES item 5: idle polls do not move the generation -------

#[tokio::test]
async fn repeated_idle_polls_do_not_bump_the_generation_or_journal() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<m1@x>", "One", ""));
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));

    // Establish a baseline (ingests m1, generation moves to 1).
    let baseline = provider.baseline_cursor().await.unwrap();
    let gen_after_baseline = store.generation().unwrap();
    assert!(gen_after_baseline >= 1);
    let journal_after_baseline = store.journal_since(0).unwrap();

    // Several idle polls (nothing changed on the server).
    let loads_after_baseline = provider
        .thread_state_loads
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(loads_after_baseline, 1, "the baseline threaded one new message");
    for _ in 0..3 {
        let batch = provider.poll(&baseline).await.unwrap();
        assert!(
            batch.changed_threads.is_empty(),
            "an idle poll from the current cursor reports no changes"
        );
        // The generation did NOT move, and the journal is unchanged.
        assert_eq!(store.generation().unwrap(), gen_after_baseline, "idle: no bump");
        assert_eq!(store.journal_since(0).unwrap(), journal_after_baseline);
        // The returned cursor is the (unchanged) current generation.
        assert_eq!(batch.cursor.generation(), Some(gen_after_baseline));
        // An idle round must not rebuild the threader state (which reads
        // every cached body).
        assert_eq!(
            provider
                .thread_state_loads
                .load(std::sync::atomic::Ordering::Relaxed),
            loads_after_baseline,
            "idle poll must not reload thread state"
        );
    }
}

#[tokio::test]
async fn merge_moves_all_existing_messages() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<a@x>", "A", ""));
    mb.add(&[], &message("<b@x>", "B", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    provider.baseline_cursor().await.unwrap();
    mailbox.lock().unwrap().add(
        &[],
        &message("<c@x>", "Link", "References: <a@x> <b@x>\r\n"),
    );
    let cursor = SyncCursor::from_generation(store.generation().unwrap());
    provider.poll(&cursor).await.unwrap();
    let rows = store.all_message_threads().unwrap();
    let survivor = store.resolve_thread_alias(&rows[0].1).unwrap();
    let fetched = provider.fetch_thread(&survivor).await.unwrap();
    assert_eq!(
        fetched.len(),
        3,
        "merging two threads must retain both roots and the linker"
    );
}

#[tokio::test]
async fn merged_threads_ingest_atomically_and_replay_through_the_engine() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<a@x>", "A", ""));
    mb.add(&[], &message("<b@x>", "B", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    let previous_cursor = db.cursor("me@example.com").unwrap().unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 2);
    mailbox.lock().unwrap().add(
        &[],
        &message("<c@x>", "Link", "References: <a@x> <b@x>\r\n"),
    );
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    let threads = db.list_all_mail(Some("me@example.com")).unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(db.get_thread(&threads[0].id).unwrap().messages.len(), 3);
    // Replay the journal after a simulated crash before cursor storage.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE sync_state SET cursor = ?1 WHERE account_id = ?2",
            rusqlite::params![previous_cursor, "me@example.com"],
        )?;
        Ok(())
    })
    .unwrap();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    assert_eq!(db.get_thread(&threads[0].id).unwrap().messages.len(), 3);
    // A later reply to either original root reaches the same survivor.
    mailbox
        .lock()
        .unwrap()
        .add(&[], &message("<d@x>", "Reply", "In-Reply-To: <b@x>\r\n"));
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    assert_eq!(db.get_thread(&threads[0].id).unwrap().messages.len(), 4);
}

#[tokio::test]
async fn out_of_order_references_survive_between_rounds() {
    let mut mb = FakeMailbox::new(100);
    mb.add(
        &[],
        &message("<reply@x>", "Reply", "References: <missing@x>\r\n"),
    );
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let cursor = provider.baseline_cursor().await.unwrap();
    mailbox.lock().unwrap().add(
        &[],
        &message("<sibling@x>", "Sibling", "References: <missing@x>\r\n"),
    );
    provider.poll(&cursor).await.unwrap();
    assert_eq!(
        store.thread_ids_in_mailbox("INBOX").unwrap().len(),
        1,
        "replies to the same absent parent are one thread across polls"
    );
    mailbox.lock().unwrap().add(
        &[],
        &message("<third@x>", "Third", "In-Reply-To: <missing@x>\r\n"),
    );
    provider.poll(&cursor).await.unwrap();
    mailbox
        .lock()
        .unwrap()
        .add(&[], &message("<missing@x>", "Late parent", ""));
    provider.poll(&cursor).await.unwrap();
    let threads = store.thread_ids_in_mailbox("INBOX").unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(provider.fetch_thread(&threads[0]).await.unwrap().len(), 4);
}

#[tokio::test]
async fn window_bounds_the_mailbox_across_polls() {
    let mut mb = FakeMailbox::new(100);
    for n in 1..=3 {
        mb.add(&[], &message(&format!("<m{n}@x>"), "Subject", ""));
    }
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.limits.mailbox_window = 2;
    let cursor = provider.baseline_cursor().await.unwrap();
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
    provider.poll(&cursor).await.unwrap();
    assert_eq!(
        store.locations_in_mailbox("INBOX").unwrap().len(),
        2,
        "an unchanged mailbox must stay within its window"
    );
    mailbox
        .lock()
        .unwrap()
        .add(&[], &message("<m4@x>", "Fourth", ""));
    provider.poll(&cursor).await.unwrap();
    let uids: Vec<_> = store
        .locations_in_mailbox("INBOX")
        .unwrap()
        .iter()
        .map(|l| l.uid)
        .collect();
    assert_eq!(uids, vec![3, 4]);
}

#[tokio::test]
async fn cache_miss_checks_uidvalidity_before_fetching() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<original@x>", "Original", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    provider.baseline_cursor().await.unwrap();
    let id = store.locations_in_mailbox("INBOX").unwrap()[0]
        .message_id
        .clone();
    store
        .database()
        .with_connection(|c| {
            c.execute("DELETE FROM imap_bodies", [])?;
            Ok(())
        })
        .unwrap();
    let mut reset = FakeMailbox::new(200);
    reset.add(&[], &message("<different@x>", "Different", ""));
    *mailbox.lock().unwrap() = reset;
    let result = provider.fetch_message(&id).await;
    assert!(
        result.is_err(),
        "a stale UID must not load another message as the original: {result:?}"
    );
    assert!(!provider.cache.contains(&id).unwrap());
    provider.baseline_cursor().await.unwrap();
    let new_id = &store.locations_in_mailbox("INBOX").unwrap()[0].message_id;
    assert_ne!(new_id, &id);
    let raw = provider.fetch_message(new_id).await.unwrap();
    assert_eq!(crate::mime::normalize(&raw).unwrap().body_text, "Different body");
}

#[tokio::test]
async fn understated_oversize_does_not_abort_the_round() {
    for advertised_size in [Some(10), None] {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<ok@x>", "Good", ""));
        let mut huge = format!(
            "Message-ID: <huge@x>\r\n\r\n{}",
            "x".repeat(policy::MAX_RAW_MESSAGE_BYTES)
        );
        huge.truncate(policy::MAX_RAW_FETCH_BYTES);
        mb.add_sized(&[], &huge, advertised_size);
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
        let result = provider.baseline_cursor().await;
        assert!(
            result.is_ok(),
            "oversize should be skipped, not abort sync: {result:?}"
        );
        let locations = store.locations_in_mailbox("INBOX").unwrap();
        assert_eq!(locations.len(), 2);
        assert!(provider.cache.contains(&locations[0].message_id).unwrap());
        assert!(!provider.cache.contains(&locations[1].message_id).unwrap());
        let generation = store.generation().unwrap();
        provider
            .poll(&SyncCursor::from_generation(generation))
            .await
            .unwrap();
        assert_eq!(store.generation().unwrap(), generation);
    }
}

#[tokio::test]
async fn removing_account_purges_live_provider_state() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<private@x>", "Private mail", ""));
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    provider.baseline_cursor().await.unwrap();
    store.database().remove_account("me@example.com").unwrap();
    assert!(
        store.all_message_threads().unwrap().is_empty(),
        "removal must clear provider thread mappings"
    );
    assert!(
        store.locations_in_mailbox("INBOX").unwrap().is_empty(),
        "removal must clear provider locations"
    );
    assert_eq!(
        provider.cache.total_size().unwrap(),
        0,
        "removal must clear raw cached mail"
    );
}

#[tokio::test]
async fn removing_account_deletes_its_sent_copy_detection_row() {
    // The per-device detection result is LOCAL provider state; removing the
    // account must purge it like every other imap_* table.
    let (_provider, store) = provider_with(Arc::new(Mutex::new(FakeMailbox::new(1))));
    store.set_sent_copy_detection(true, 1_000).unwrap();
    assert_eq!(store.sent_copy_detection().unwrap(), Some(true));
    store.database().remove_account("me@example.com").unwrap();
    assert!(
        store.sent_copy_detection().unwrap().is_none(),
        "account removal must delete the Sent-copy detection row"
    );
}

// ---- SLICE5B1 item 3/5: the token loader threads like the body oracle --
//
// Each corpus below syncs a sequence of messages through the provider
// (which writes tokens at assignment and seeds from them), then threads one
// more "probe" message TWO ways against the resulting store: once with the
// production token loader and once with the retained body-parsing oracle.
// The two ThreadState seeds must drive thread_batch to the SAME assignment,
// aliases and changed-thread set — proving the token loader is equivalent
// to the body loader it replaced, without ever reading imap_bodies.

/// Thread `probe` against the store with BOTH loaders and assert the
/// outcomes are identical. Returns the (identical) outcome.
async fn assert_loaders_agree(
    provider: &ImapProvider,
    probe: ThreadingInput,
) -> threading::ThreadingOutcome {
    let batch_tokens = probe.token_set();
    let mut token_state = provider
        .load_thread_state_from_tokens(&batch_tokens)
        .unwrap();
    let mut oracle_state = provider.load_thread_state().unwrap();
    let token_outcome = threading::thread_batch(&mut token_state, &[probe.clone()]);
    let oracle_outcome = threading::thread_batch(&mut oracle_state, &[probe]);
    assert_eq!(
        token_outcome, oracle_outcome,
        "token loader and body oracle must thread identically"
    );
    token_outcome
}

/// Sync a sequence of messages (each added, then one poll) so each lands in
/// its own round, exercising cross-round persistence.
async fn sync_each(provider: &ImapProvider, mailbox: &Arc<Mutex<FakeMailbox>>, msgs: &[String]) {
    provider.baseline_cursor().await.unwrap();
    for raw in msgs {
        mailbox.lock().unwrap().add(&[], raw);
        let cursor = SyncCursor::from_generation(provider.store.generation().unwrap());
        provider.poll(&cursor).await.unwrap();
    }
}

fn probe(id: &str, msgid: &str, in_reply_to: Option<&str>, references: Option<&str>) -> ThreadingInput {
    ThreadingInput {
        message_id: id.to_string(),
        message_id_header: Some(msgid.to_string()),
        in_reply_to: in_reply_to.map(str::to_string),
        references: references.map(str::to_string),
    }
}

#[tokio::test]
async fn loader_equivalence_late_parent() {
    // A reply to an absent parent, then a sibling: a late parent probe must
    // join the same thread under both loaders.
    let mb = FakeMailbox::new(100);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store) = provider_with(mailbox.clone());
    sync_each(
        &provider,
        &mailbox,
        &[
            message("<reply@x>", "Reply", "References: <missing@x>\r\n"),
            message("<sibling@x>", "Sibling", "References: <missing@x>\r\n"),
        ],
    )
    .await;
    assert_loaders_agree(&provider, probe("imap:p:late", "<missing@x>", None, None)).await;
}

#[tokio::test]
async fn loader_equivalence_absent_parent_siblings_across_rounds() {
    let mb = FakeMailbox::new(100);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store) = provider_with(mailbox.clone());
    sync_each(
        &provider,
        &mailbox,
        &[
            message("<s1@x>", "S1", "In-Reply-To: <ghost@x>\r\n"),
            message("<s2@x>", "S2", "In-Reply-To: <ghost@x>\r\n"),
            message("<s3@x>", "S3", "References: <ghost@x>\r\n"),
        ],
    )
    .await;
    let outcome =
        assert_loaders_agree(&provider, probe("imap:p:s4", "<s4@x>", Some("<ghost@x>"), None))
            .await;
    // All siblings share one thread, so the probe joins exactly it.
    assert!(outcome.aliases.is_empty(), "no merge: all share one thread");
}

#[tokio::test]
async fn loader_equivalence_merge_of_two_persisted_threads() {
    // Two unrelated roots become two persisted threads; a probe that
    // references both must merge them identically under both loaders.
    let mb = FakeMailbox::new(100);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store) = provider_with(mailbox.clone());
    sync_each(
        &provider,
        &mailbox,
        &[message("<a@x>", "A", ""), message("<b@x>", "B", "")],
    )
    .await;
    let outcome = assert_loaders_agree(
        &provider,
        probe("imap:p:link", "<link@x>", None, Some("<a@x> <b@x>")),
    )
    .await;
    assert_eq!(outcome.aliases.len(), 1, "the probe merges the two threads");
}

#[tokio::test]
async fn loader_equivalence_capped_ancestry() {
    // A seeded message whose References exceed the cap: the token set
    // persisted is the capped set, and the loader must still thread a reply
    // that points at the nearest (kept) ancestor identically.
    let near = "<near@x>";
    let huge: String = (0..policy::MAX_REFERENCES + 20)
        .map(|i| {
            if i == policy::MAX_REFERENCES + 19 {
                near.to_string()
            } else {
                format!("<old{i}@x>")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mb = FakeMailbox::new(100);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store) = provider_with(mailbox.clone());
    sync_each(
        &provider,
        &mailbox,
        &[
            message(near, "Near", ""),
            message("<capped@x>", "Capped", &format!("References: {huge}\r\n")),
        ],
    )
    .await;
    assert_loaders_agree(
        &provider,
        probe("imap:p:reply", "<reply2@x>", Some(near), None),
    )
    .await;
}

#[tokio::test]
async fn loader_equivalence_duplicate_message_id() {
    let mb = FakeMailbox::new(100);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store) = provider_with(mailbox.clone());
    // Same Message-ID header twice (distinct stable ids via distinct UIDs).
    sync_each(
        &provider,
        &mailbox,
        &[message("<dup@x>", "First", ""), message("<dup@x>", "Second", "")],
    )
    .await;
    assert_loaders_agree(&provider, probe("imap:p:d", "<dup@x>", None, None)).await;
}

#[tokio::test]
async fn loader_threads_a_reply_to_a_message_with_no_cached_body() {
    // An oversize message's body is skipped (never cached). At sync time it
    // still threads on its IDENTITY-PASS Message-ID header, and 5b-1
    // PERSISTS that token at assignment — so a later reply via that header
    // joins its thread using tokens alone, with no body ever read.
    //
    // This is the one corpus case where the retained body-parsing oracle
    // CANNOT match: reloading from bodies loses the header of a message
    // whose body was never cached (the oracle sees only the stable id).
    // That lossy reload is exactly the weakness 5b-1 removes, so here we
    // assert the token loader's correct, non-lossy outcome directly rather
    // than against the oracle.
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<normal@x>", "Normal", ""));
    mb.add_sized(
        &[],
        &message("<nobody@x>", "NoBody", ""),
        Some(policy::MAX_RAW_MESSAGE_BYTES + 1),
    );
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    provider.baseline_cursor().await.unwrap();

    // The oversize message really has no cached body.
    let nobody_id = store
        .locations_in_mailbox("INBOX")
        .unwrap()
        .into_iter()
        .find(|l| !provider.cache.contains(&l.message_id).unwrap())
        .map(|l| l.message_id)
        .expect("the oversize message's body was skipped");

    // Its persisted token set includes the identity-pass header AND its
    // stable id — written at assignment, with no body read. The header is
    // stored in the threader's NORMALIZED form (brackets/case stripped).
    let tokens = store
        .seed_state_for_tokens(&["nobody@x".to_string()])
        .unwrap();
    assert_eq!(tokens.len(), 1, "exactly the no-body message's thread is seeded");
    let (_, (_, token_set)) = tokens.into_iter().next().unwrap();
    assert!(token_set.contains(&"nobody@x".to_string()));
    assert!(token_set.contains(&nobody_id));

    // A reply via that header joins the no-body message's thread.
    let mut state = provider
        .load_thread_state_from_tokens(&["nobody@x".to_string()])
        .unwrap();
    let outcome = threading::thread_batch(
        &mut state,
        &[probe("imap:p:r", "<reply3@x>", Some("<nobody@x>"), None)],
    );
    let nobody_thread = store
        .thread_of_message(&nobody_id)
        .unwrap()
        .map(|t| store.resolve_thread_alias(&t).unwrap())
        .unwrap();
    assert_eq!(
        outcome.assignments[0].1, nobody_thread,
        "the reply joins the no-body message's thread from tokens alone"
    );
}

// ---- SLICE5B1 item 3: the hot path never reads imap_bodies -------------

#[tokio::test]
async fn the_round_loader_does_not_read_cached_bodies() {
    // After an initial sync (which backfills + writes tokens), delete the
    // whole body cache, then sync a NEW linking message. Threading must
    // still merge correctly using ONLY persisted tokens — proving the per
    // round loader reads no bodies. (The backfill already ran at the first
    // threading round, so a later round never parses a body.)
    let mut mb = FakeMailbox::new(100);
    mb.add(&[], &message("<a@x>", "A", ""));
    mb.add(&[], &message("<b@x>", "B", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    provider.baseline_cursor().await.unwrap();
    // Drop every cached body: a body read now would fail to find anything.
    store
        .database()
        .with_connection(|c| {
            c.execute("DELETE FROM imap_bodies", [])?;
            Ok(())
        })
        .unwrap();
    // A late linker referencing both roots arrives and is synced.
    mailbox
        .lock()
        .unwrap()
        .add(&[], &message("<c@x>", "Link", "References: <a@x> <b@x>\r\n"));
    let cursor = SyncCursor::from_generation(store.generation().unwrap());
    provider.poll(&cursor).await.unwrap();
    // The two roots merged into one thread using tokens alone.
    let rows = store.all_message_threads().unwrap();
    let survivor = store.resolve_thread_alias(&rows[0].1).unwrap();
    let members = store.messages_in_thread(&survivor).unwrap();
    assert_eq!(members.len(), 3, "the merge happened from tokens, no body read");
}

// ---- SLICE5B1 item 4: the upgrade backfill ------------------------------

#[tokio::test]
async fn upgrade_backfill_fills_tokens_then_a_late_linker_threads_like_before() {
    // Simulate a pre-5b1 (schema-58-era) database: threads + cached bodies
    // exist but NO token rows, and the account is marked backfill-needed.
    // Running a threading round must backfill tokens from the bodies, then
    // thread a late linking message to the SAME result as the oracle.
    let mb = FakeMailbox::new(100);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());

    // Insert two threaded messages WITH cached bodies but no tokens, via
    // the store (as a 0.94.x database would hold them).
    for (id, msgid, tid) in [
        ("imap:me@example.com:a", "<a@x>", "imap:t:a"),
        ("imap:me@example.com:b", "<b@x>", "imap:t:b"),
    ] {
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![(id.into(), tid.into())],
                changed_threads: vec![tid.into()],
                // Deliberately NO message_tokens — the pre-5b1 shape.
                ..Default::default()
            })
            .unwrap();
        provider
            .cache
            .put(id, message(msgid, "S", "").as_bytes(), 1)
            .unwrap();
    }
    // Clear the token rows commit_sync_round would never have written here
    // anyway, and force "backfill needed".
    store
        .database()
        .with_connection(|c| {
            c.execute("DELETE FROM imap_message_tokens", [])?;
            c.execute(
                "UPDATE imap_sync_state SET tokens_backfilled = 0 WHERE account_id = ?1",
                ["me@example.com"],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(!store.tokens_backfilled().unwrap(), "set up as needing backfill");
    assert_eq!(store.messages_missing_tokens(100).unwrap().len(), 2);

    // A late message linking <a@x> and <b@x>. Threading it runs the
    // backfill first, then merges from the now-present tokens.
    let linker = probe("imap:me@example.com:link", "<link@x>", None, Some("<a@x> <b@x>"));
    provider.ensure_tokens_backfilled().unwrap();
    assert!(store.tokens_backfilled().unwrap(), "backfill marked done");
    assert!(store.messages_missing_tokens(100).unwrap().is_empty());

    // Now the token loader and body oracle agree, and the probe merges.
    let outcome = assert_loaders_agree(&provider, linker).await;
    assert_eq!(outcome.aliases.len(), 1, "the late linker merged both threads");
}

#[tokio::test]
async fn upgrade_backfill_is_idempotent_and_crash_resumable() {
    let mb = FakeMailbox::new(100);
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    // One threaded message with a body, no tokens, backfill needed.
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:m".into(), "imap:t:m".into())],
            changed_threads: vec!["imap:t:m".into()],
            ..Default::default()
        })
        .unwrap();
    provider
        .cache
        .put("imap:me@example.com:m", message("<m@x>", "M", "").as_bytes(), 1)
        .unwrap();
    store
        .database()
        .with_connection(|c| {
            c.execute("DELETE FROM imap_message_tokens", [])?;
            c.execute("UPDATE imap_sync_state SET tokens_backfilled = 0", [])?;
            Ok(())
        })
        .unwrap();

    // Simulate a crash AFTER one message's tokens were written but BEFORE
    // the done-flag was set: write the row directly, leave the flag off.
    store
        .write_message_tokens("imap:me@example.com:m", &["<m@x>".into()])
        .unwrap();
    assert!(!store.tokens_backfilled().unwrap());
    // The queue is now empty (that message has tokens), so resume finishes
    // and marks done without re-doing work.
    assert!(store.messages_missing_tokens(100).unwrap().is_empty());
    provider.ensure_tokens_backfilled().unwrap();
    assert!(store.tokens_backfilled().unwrap());
    // Running again is a no-op.
    provider.ensure_tokens_backfilled().unwrap();
    assert!(store.tokens_backfilled().unwrap());
}

#[tokio::test]
async fn a_message_with_no_cached_body_backfills_only_its_stable_id() {
    let mb = FakeMailbox::new(100);
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:nb".into(), "imap:t:nb".into())],
            changed_threads: vec!["imap:t:nb".into()],
            ..Default::default()
        })
        .unwrap();
    // No cache.put — the message has no cached body.
    store
        .database()
        .with_connection(|c| {
            c.execute("DELETE FROM imap_message_tokens", [])?;
            c.execute("UPDATE imap_sync_state SET tokens_backfilled = 0", [])?;
            Ok(())
        })
        .unwrap();
    provider.ensure_tokens_backfilled().unwrap();
    let tokens: Vec<String> = store
        .database()
        .with_connection(|c| {
            let mut st = c.prepare(
                "SELECT token FROM imap_message_tokens WHERE message_id = 'imap:me@example.com:nb'",
            )?;
            let rows = st
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(tokens, vec!["imap:me@example.com:nb".to_string()],
        "no body => only the stable-id anchor token");
}

// ---- SLICE5A_FIXES item 6: gated live test against the Dovecot harness --
//
// ------------------------------------------------------------------
// Slice 5b-1: multi-mailbox machinery + Trash/Junk
// ------------------------------------------------------------------

/// Labels the engine ended up with for a thread, via fetch_thread's union.
async fn thread_labels(provider: &ImapProvider, thread_id: &str) -> Vec<String> {
    let messages = provider.fetch_thread(thread_id).await.unwrap();
    messages.into_iter().flat_map(|m| m.label_ids).collect()
}

/// (a) A hot INBOX message moved to Trash by another client -> TRASH, no
/// INBOX, and its thread is re-journaled so the engine re-ingests it.
#[tokio::test]
async fn a_hot_inbox_message_moved_to_trash_becomes_trash_and_is_rejournaled() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    mb.folder("Trash", 50); // present but empty for now
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    // First sync: m1 is in INBOX, hot, ingested.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let threads = provider.list_inbox(None).await.unwrap();
    assert_eq!(threads.thread_ids.len(), 1);
    let thread_id = threads.thread_ids[0].clone();
    assert!(store.is_thread_hot(&thread_id).unwrap(), "an INBOX thread is hot");
    assert_eq!(thread_labels(&provider, &thread_id).await, vec!["INBOX"]);

    // Another client moves m1 from INBOX to Trash (same Message-ID -> same
    // stable id, so it is the same message in a new mailbox).
    {
        let mut mb = mailbox.lock().unwrap();
        mb.inbox().messages.retain(|m| m.uid != 1);
        mb.add_to("Trash", 50, &["\\Seen"], &message("<m1@x>", "Hello", ""));
    }
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    // The INBOX location is gone; the Trash location is present.
    assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
    assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 1);
    // The thread was already hot, so it was journaled and re-ingested; its
    // labels are now TRASH WITHOUT INBOX — never silently "archived".
    let labels = thread_labels(&provider, &thread_id).await;
    assert!(labels.contains(&"TRASH".to_string()), "has TRASH: {labels:?}");
    assert!(!labels.contains(&"INBOX".to_string()), "no INBOX: {labels:?}");
}

#[tokio::test]
async fn an_index_mailbox_merge_inherits_hotness_and_keeps_reporting_changes() {
    for folder in ["Trash", "Junk"] {
        let mut mb = FakeMailbox::new(100);
        mb.add_to(folder, 50, &["\\Seen"], &message("<cold@x>", "Cold", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        let cold_id = store.locations_in_mailbox(folder).unwrap()[0]
            .message_id
            .clone();
        let survivor = store.thread_of_message(&cold_id).unwrap().unwrap();
        assert!(!store.is_thread_hot(&survivor).unwrap());

        mailbox
            .lock()
            .unwrap()
            .add(&["\\Seen"], &message("<hot@x>", "Hot", ""));
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        let hot_id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        let old_hot = store.thread_of_message(&hot_id).unwrap().unwrap();
        assert!(store.is_thread_hot(&old_hot).unwrap());
        let before_merge = store.generation().unwrap();

        mailbox.lock().unwrap().add_to(
            folder,
            50,
            &["\\Seen"],
            &message("<link@x>", "Link", "References: <cold@x> <hot@x>\r\n"),
        );
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert_eq!(store.resolve_thread_alias(&old_hot).unwrap(), survivor);
        assert!(
            store.is_thread_hot(&survivor).unwrap(),
            "{folder} merge retains hotness"
        );
        let changed = store.journal_since(before_merge).unwrap();
        assert!(changed.contains(&old_hot), "retired hot id is journaled");
        assert!(
            changed.contains(&survivor),
            "previously cold survivor is journaled"
        );
        let cached = db
            .list_all_mail(Some("me@example.com"))
            .unwrap()
            .into_iter()
            .chain(db.list_trash(Some("me@example.com")).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(cached.len(), 1, "the engine ingests one surviving thread");
        assert_eq!(cached[0].provider_thread_id, survivor);
        assert_eq!(db.get_thread(&cached[0].id).unwrap().messages.len(), 3);

        let after_merge = store.generation().unwrap();
        mailbox.lock().unwrap().inbox().messages[0].flags =
            vec!["\\Seen".into(), "\\Flagged".into()];
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert!(
            store
                .journal_since(after_merge)
                .unwrap()
                .contains(&survivor),
            "later INBOX flag changes must still reach the engine"
        );
        let updated = db
            .list_all_mail(Some("me@example.com"))
            .unwrap()
            .into_iter()
            .chain(db.list_trash(Some("me@example.com")).unwrap())
            .find(|thread| thread.provider_thread_id == survivor)
            .unwrap();
        assert!(
            updated.starred,
            "the engine applies the flag change after the merge"
        );
    }
}

/// (b) A never-hot thread that only ever appears in Trash -> locations,
/// threads and tokens recorded, NOT journaled, NOT ingested, no body
/// fetched; fetch_thread still works (cache-miss server fetch).
#[tokio::test]
async fn a_never_hot_trash_only_thread_is_recorded_but_not_journaled() {
    let mut mb = FakeMailbox::new(100); // empty INBOX
    mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox);
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    // Recorded: a Trash location + a thread + tokens.
    let trash = store.locations_in_mailbox("Trash").unwrap();
    assert_eq!(trash.len(), 1);
    let message_id = trash[0].message_id.clone();
    let thread_id = store.thread_of_message(&message_id).unwrap().unwrap();
    assert!(!store.is_thread_hot(&thread_id).unwrap(), "a Trash-only thread is not hot");

    // NOT journaled: the engine saw no mail.
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 0);

    // No body was fetched during index-only sync.
    assert!(provider.cache.get(&message_id).unwrap().is_none(), "no body cached at ingest");

    // fetch_thread still works: it fetches the body from Trash on demand.
    let messages = provider.fetch_thread(&thread_id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].label_ids.contains(&"TRASH".to_string()));
}

/// (c) Junk likewise, with the SPAM label.
#[tokio::test]
async fn a_never_hot_junk_only_thread_is_recorded_with_the_spam_label() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "Spammy", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox);
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let junk = store.locations_in_mailbox("Junk").unwrap();
    assert_eq!(junk.len(), 1);
    let thread_id = store.thread_of_message(&junk[0].message_id).unwrap().unwrap();
    assert!(!store.is_thread_hot(&thread_id).unwrap());
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 0, "not journaled");

    let labels = thread_labels(&provider, &thread_id).await;
    assert!(labels.contains(&"SPAM".to_string()), "SPAM label: {labels:?}");
}

/// (d) Cadence gating: an unchanged, not-due folder issues no SEARCH/FETCH
/// on the second poll; when the sweep is due a full sweep runs again and
/// catches an external flag change.
#[tokio::test]
async fn cadence_gating_skips_an_unchanged_folder_until_the_sweep_is_due() {
    use std::sync::atomic::{AtomicI64, Ordering};
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());

    // A controllable clock so we can advance past the sweep interval.
    static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);
    CLOCK.store(1_700_000_000, Ordering::SeqCst);
    provider.now = || CLOCK.load(Ordering::SeqCst);
    let db = store.database();

    // First sync sweeps Trash once (counts: 1 search).
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let after_first = mailbox.lock().unwrap().counts("Trash").0;
    assert_eq!(after_first, 1, "Trash swept once on the baseline");

    // Second poll, counters unchanged and not sweep-due: NO new Trash
    // SEARCH is issued.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(
        mailbox.lock().unwrap().counts("Trash").0,
        after_first,
        "an unchanged, not-due Trash issues no SEARCH"
    );

    // An external client marks the Trash message \Flagged — this moves
    // NEITHER EXISTS nor UIDNEXT, so only the periodic sweep catches it.
    mailbox.lock().unwrap().folder("Trash", 50).messages[0].flags =
        vec!["\\Seen".into(), "\\Flagged".into()];
    // Advance the clock past the sweep interval.
    CLOCK.store(1_700_000_000 + super::super::policy::FOLDER_SWEEP_INTERVAL_SECS + 1, Ordering::SeqCst);
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert!(
        mailbox.lock().unwrap().counts("Trash").0 > after_first,
        "the due sweep issues a SEARCH again"
    );
    // The flag change is now reflected locally.
    let trash = store.locations_in_mailbox("Trash").unwrap();
    let flags: Vec<String> = serde_json::from_str(&trash[0].flags_json).unwrap();
    assert!(flags.iter().any(|f| f == "\\Flagged"), "the sweep caught the flag change");
}

/// (e) A UIDVALIDITY reset in Trash drops ONLY Trash's locations; INBOX is
/// untouched.
#[tokio::test]
async fn a_uidvalidity_reset_in_trash_only_drops_trash_locations() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1);
    assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 1);

    // Reset Trash's UIDVALIDITY (new mailbox incarnation) and force a sweep.
    {
        let mut mb = mailbox.lock().unwrap();
        let trash = mb.folder("Trash", 999);
        trash.uidvalidity = 999;
        trash.uidnext = 1;
        trash.messages.clear();
        trash.add_sized(&["\\Seen"], &message("<t2@x>", "FreshTrash", ""), None);
    }
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    // INBOX is untouched; Trash was dropped and resynced to the new UID.
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1, "INBOX untouched");
    let trash = store.locations_in_mailbox("Trash").unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(trash[0].uidvalidity, 999, "Trash resynced under the new UIDVALIDITY");
}

/// (f) INBOX, Trash, Junk AND Sent are synced; Drafts, All Mail, Starred
/// (\Flagged, no role), the label container and Archive are never selected
/// or fetched. (Sent joined the synced set in run 3 and Archive in 5b-2 —
/// intentional contract changes; this test previously named only
/// INBOX/Trash/Junk/Sent as synced and asserted Archive was never synced.)
#[tokio::test]
async fn only_inbox_trash_junk_sent_and_archive_are_ever_synced() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
    mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "Spam", ""));
    mb.add_to("Sent", 90, &["\\Seen"], &message("<s1@x>", "Sent", "")); // run 3: synced
    mb.add_to("Archive", 95, &["\\Seen"], &message("<ar1@x>", "Arch", "")); // 5b-2: synced
    // Folders that MUST never be synced:
    mb.add_to("Drafts", 70, &["\\Seen"], &message("<d1@x>", "Draft", ""));
    mb.add_to("All Mail", 80, &["\\Seen"], &message("<a1@x>", "All", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    // Synced mailboxes have locations (Sent and Archive included this run).
    for synced in ["INBOX", "Trash", "Junk", "Sent", "Archive"] {
        assert!(!store.locations_in_mailbox(synced).unwrap().is_empty(), "{synced} synced");
    }
    // Never-synced mailboxes have NO locations and were never SEARCHed.
    let counts = mailbox.lock().unwrap();
    for never in ["Drafts", "All Mail"] {
        assert!(store.locations_in_mailbox(never).unwrap().is_empty(), "{never} not synced");
        assert_eq!(counts.counts(never).0, 0, "{never} never SEARCHed");
    }
}

/// (g) A folder that cannot be synced (its mailbox was renamed or deleted on
/// the server, so EXAMINE answers NO) must NOT wedge the account. INBOX is
/// the only mailbox whose failure aborts a poll: the engine ingests only
/// after a poll succeeds, so an aborted poll would stop ALL mail flowing
/// because of an index-tier folder. Folder failures are skipped (and
/// logged) and retried next poll.
#[tokio::test]
async fn a_failing_folder_does_not_wedge_inbox_sync() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    mb.folder("Trash", 50).examine_fails = true;
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    // Initial sync succeeds and INBOX mail is ingested despite Trash failing.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .expect("a broken folder must not fail the sync");
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

    // New INBOX mail keeps flowing on later polls while Trash stays broken.
    mailbox
        .lock()
        .unwrap()
        .add(&["\\Seen"], &message("<i2@x>", "Second", ""));
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .expect("still not wedged");
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 2);

    // And once Trash is fixed it syncs on a later poll (retried, not dropped).
    {
        let mut mb = mailbox.lock().unwrap();
        let trash = mb.folder("Trash", 50);
        trash.examine_fails = false;
    }
    mailbox
        .lock()
        .unwrap()
        .add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 1);
}

/// (i) Idle polls do not bump the generation or rebuild threader state,
/// even with Trash/Junk present (the multi-mailbox walk stays idle-safe).
#[tokio::test]
async fn idle_polls_with_folders_present_do_not_bump_the_generation() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox);
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let generation_after_first = store.generation().unwrap();
    let loads_after_first =
        provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);

    // An idle re-sync: nothing changed anywhere.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(
        store.generation().unwrap(),
        generation_after_first,
        "an idle multi-mailbox poll does not bump the generation"
    );
    assert_eq!(
        provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
        loads_after_first,
        "an idle poll does not rebuild threader state"
    );
}

// ------------------------------------------------------------------
// Slice 5b-1 run 3: Sent sync, chunked acquisition, catalog refresh
// ------------------------------------------------------------------

/// Seed `n` Sent messages (UIDs 1..=n) with distinct Message-IDs.
fn seed_sent(mb: &mut FakeMailbox, uidvalidity: u32, n: usize) {
    for i in 1..=n {
        mb.add_to("Sent", uidvalidity, &["\\Seen"], &message(&format!("<s{i}@x>"), "Sent", ""));
    }
}

/// Run one baseline + `polls` extra polls through the engine.
async fn sync_n(provider: &ImapProvider, db: &crate::db::Database, polls: usize) {
    for _ in 0..=polls {
        crate::sync::sync_with(db, "me@example.com", provider).await.unwrap();
    }
}

/// The Sent backfill watermark NULL (complete) / Some (pending).
fn sent_marker(store: &ImapStateStore) -> Option<i64> {
    store.sent_backfill_low_uid("Sent").unwrap()
}

/// (a) The SAME Message-ID in INBOX and Sent is ONE message id with both
/// labels (INBOX+SENT) and exactly ONE body fetch (shared stable id).
#[tokio::test]
async fn the_same_message_in_inbox_and_sent_is_one_message_with_both_labels() {
    let shared = "<shared@x>";
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message(shared, "Bcc self", ""));
    mb.add_to("Sent", 90, &["\\Seen"], &message(shared, "Bcc self", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    sync_n(&provider, db.as_ref(), 1).await;

    // One stable id with two locations (INBOX + Sent).
    let inbox = store.locations_in_mailbox("INBOX").unwrap();
    let sent = store.locations_in_mailbox("Sent").unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(sent.len(), 1);
    assert_eq!(inbox[0].message_id, sent[0].message_id, "one id, two locations");

    // fetch_thread unions the labels: INBOX and SENT, no duplicate message.
    let thread_id = store.thread_of_message(&inbox[0].message_id).unwrap().unwrap();
    let messages = provider.fetch_thread(&thread_id).await.unwrap();
    assert_eq!(messages.len(), 1, "one message, not two");
    let labels = &messages[0].label_ids;
    assert!(labels.contains(&"INBOX".to_string()), "has INBOX: {labels:?}");
    assert!(labels.contains(&"SENT".to_string()), "has SENT: {labels:?}");

    // Exactly one body cached for the shared id (bodies dedup by stable id).
    assert!(provider.cache.contains(&inbox[0].message_id).unwrap());
    // The body was fetched at most twice total across both mailboxes'
    // ingest, but cached ONCE (deduped). The key assertion is one message.
}

/// (b) Chunking: with a tiny per-round budget, N Sent messages are
/// acquired NEWEST FIRST over several rounds, with no duplicates, and the
/// engine ends with all of them. The engine drives several polls per
/// `sync_with` (a full sync is baseline + an incremental pass, and later
/// syncs poll once), so this asserts the INVARIANTS — the acquired set is
/// always the newest contiguous suffix, grows monotonically, never
/// duplicates, and the backfill marker clears exactly when all are in —
/// rather than a brittle per-call count.
#[tokio::test]
async fn sent_is_acquired_in_newest_first_chunks_over_several_polls() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", "")); // keep INBOX non-empty
    seed_sent(&mut mb, 90, 5); // Sent UIDs 1..=5
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(1); // one UID per round
    let db = store.database();

    // Drive the account to completion, checking invariants after each
    // `sync_with` until the backfill marker clears.
    let mut last_len = 0usize;
    for _ in 0..10 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let uids = sent_uids(&store);
        // No duplicates (sorted, unique).
        let mut dedup = uids.clone();
        dedup.dedup();
        assert_eq!(dedup, uids, "no duplicate Sent locations");
        // Newest-first: the acquired set is always the top-K contiguous
        // suffix of 1..=5, i.e. [6-k ..= 5].
        if !uids.is_empty() {
            let k = uids.len() as i64;
            let expected: Vec<i64> = ((6 - k)..=5).collect();
            assert_eq!(uids, expected, "acquired set is the newest contiguous suffix");
        }
        // Monotonic growth.
        assert!(uids.len() >= last_len, "acquisition never loses ground");
        last_len = uids.len();
        if sent_marker(&store).is_none() && uids.len() == 5 {
            break;
        }
    }
    assert_eq!(sent_uids(&store), vec![1, 2, 3, 4, 5], "all five eventually acquired");
    assert!(sent_marker(&store).is_none(), "backfill complete, marker cleared");

    // Every Sent message is now fetchable with the SENT label.
    for location in store.locations_in_mailbox("Sent").unwrap() {
        let thread = store.thread_of_message(&location.message_id).unwrap().unwrap();
        let labels = thread_labels(&provider, &thread).await;
        assert!(labels.contains(&"SENT".to_string()), "SENT label: {labels:?}");
    }
}

/// The Sent UIDs currently held locally, ascending.
fn sent_uids(store: &ImapStateStore) -> Vec<i64> {
    let mut uids: Vec<i64> = store
        .locations_in_mailbox("Sent")
        .unwrap()
        .into_iter()
        .map(|l| l.uid)
        .collect();
    uids.sort_unstable();
    uids
}

/// (c) Resume after a failure mid-chunk: a dropped connection on a body
/// fetch commits NOTHING for that round (one transaction), so the next
/// round re-derives the same remaining set, loses nothing and acquires
/// nothing twice.
#[tokio::test]
async fn a_failure_mid_sent_chunk_loses_nothing_and_acquires_nothing_twice() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    seed_sent(&mut mb, 90, 3);
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(3); // try to take all 3 at once

    // Baseline: INBOX commits; the Sent round drops its connection after
    // the first body fetch, so NO Sent location is committed.
    {
        let mut guard = mailbox.lock().unwrap();
        guard.folder("Sent", 90).fail_after_body_fetches = Some(1);
    }
    let db = store.database();
    // The folder failure is swallowed by refresh_all_with (not INBOX), so
    // the poll still succeeds overall.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert!(sent_uids(&store).is_empty(), "a failed Sent chunk commits no locations");
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1, "INBOX still committed");

    // Heal the server and poll again: all 3 Sent messages acquired, each
    // exactly once (no duplicate UID/message rows).
    {
        let mut guard = mailbox.lock().unwrap();
        guard.folder("Sent", 90).fail_after_body_fetches = None;
    }
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(sent_uids(&store), vec![1, 2, 3], "resumed cleanly, nothing lost or doubled");
    assert!(sent_marker(&store).is_none(), "backfill complete after resume");
}

/// (d) Cadence gating does NOT skip Sent while the backfill is pending even
/// when EXISTS/UIDNEXT are unchanged; once complete, the normal cadence
/// skip resumes. (The engine drives several polls per `sync_with`, so this
/// compares SEARCH activity while pending vs. after completion rather than
/// per-call counts.)
#[tokio::test]
async fn cadence_does_not_skip_sent_while_backfill_pending_then_resumes() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    seed_sent(&mut mb, 90, 4);
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(1); // one per round => several rounds
    let db = store.database();

    // Baseline leaves the backfill pending (not all 4 acquired yet with a
    // budget of 1 across baseline's two internal rounds).
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert!(sent_marker(&store).is_some(), "backfill still pending after baseline");

    // While pending, each sync_with keeps SEARCHing Sent (not skipped),
    // because EXISTS/UIDNEXT unchanged does not mean the window is fully
    // acquired. Drive to completion, confirming SEARCH count rises.
    let mut prev_searches = mailbox.lock().unwrap().counts("Sent").0;
    for _ in 0..10 {
        if sent_marker(&store).is_none() {
            break;
        }
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let searches = mailbox.lock().unwrap().counts("Sent").0;
        assert!(
            searches > prev_searches,
            "a pending Sent backfill is NOT skipped by cadence"
        );
        prev_searches = searches;
    }
    assert_eq!(sent_uids(&store), vec![1, 2, 3, 4], "all acquired");
    assert!(sent_marker(&store).is_none(), "complete now");

    // Now complete, counters unchanged, not sweep-due => Sent is SKIPPED.
    let before_skip = mailbox.lock().unwrap().counts("Sent").0;
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(
        mailbox.lock().unwrap().counts("Sent").0,
        before_skip,
        "a complete, unchanged, not-due Sent is skipped again"
    );
}

/// (e) Sent is NON-evicting: a message acquired then pushed past the window
/// by newer mail is NOT evicted, while an evicting class (Trash) in the
/// same situation IS; a server-side expunge of an acquired Sent message
/// DOES delete it.
#[tokio::test]
async fn sent_does_not_evict_aged_out_messages_but_trash_does() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    seed_sent(&mut mb, 90, 2); // Sent UIDs 1,2
    mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "T1", "")); // Trash UID 1
    mb.add_to("Trash", 50, &["\\Seen"], &message("<t2@x>", "T2", "")); // Trash UID 2
    let mailbox = Arc::new(Mutex::new(mb));
    // Tiny window (2) for BOTH Sent and Trash via the test knob below.
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(10);
    provider.folder_window_override = Some(2);
    let db = store.database();

    sync_n(&provider, db.as_ref(), 2).await;
    assert_eq!(sent_uids(&store), vec![1, 2], "both Sent acquired");
    assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 2);

    // Newer mail pushes the oldest (UID 1) past the 2-message window in
    // BOTH mailboxes.
    {
        let mut g = mailbox.lock().unwrap();
        g.add_to("Sent", 90, &["\\Seen"], &message("<s3@x>", "S3", "")); // Sent UID 3
        g.add_to("Trash", 50, &["\\Seen"], &message("<t3@x>", "T3", "")); // Trash UID 3
    }
    sync_n(&provider, db.as_ref(), 2).await;

    // Sent keeps UID 1 (non-evicting acquisition bound): all three held.
    assert_eq!(sent_uids(&store), vec![1, 2, 3], "Sent never evicts an aged-out message");
    // Trash evicted UID 1 (sliding window): only the newest two remain.
    let trash_uids: Vec<i64> = {
        let mut u: Vec<i64> = store
            .locations_in_mailbox("Trash")
            .unwrap()
            .into_iter()
            .map(|l| l.uid)
            .collect();
        u.sort_unstable();
        u
    };
    assert_eq!(trash_uids, vec![2, 3], "Trash evicts the aged-out UID 1");

    // A server-side expunge of an acquired Sent message DOES delete it.
    {
        let mut g = mailbox.lock().unwrap();
        g.folder("Sent", 90).messages.retain(|m| m.uid != 2);
    }
    sync_n(&provider, db.as_ref(), 2).await;
    assert!(!sent_uids(&store).contains(&2), "an expunged Sent message is deleted");
}

/// (f) A Sent copy never makes the message UNREAD even without \Seen.
#[tokio::test]
async fn a_sent_copy_is_never_unread_even_without_seen() {
    let mut mb = FakeMailbox::new(100);
    // No \Seen flag on the Sent message.
    mb.add_to("Sent", 90, &[], &message("<s1@x>", "Unseen sent", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox);
    let db = store.database();

    sync_n(&provider, db.as_ref(), 1).await;
    let sent = store.locations_in_mailbox("Sent").unwrap();
    assert_eq!(sent.len(), 1);
    let thread = store.thread_of_message(&sent[0].message_id).unwrap().unwrap();
    let labels = thread_labels(&provider, &thread).await;
    assert!(labels.contains(&"SENT".to_string()), "SENT: {labels:?}");
    assert!(!labels.contains(&"UNREAD".to_string()), "a Sent copy is never UNREAD: {labels:?}");
}

/// (g) A UIDVALIDITY reset in Sent only: Sent's locations are dropped and
/// resynced; INBOX locations and the message->thread mapping are untouched.
#[tokio::test]
async fn a_uidvalidity_reset_in_sent_only_drops_sent_locations() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    seed_sent(&mut mb, 90, 2);
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(10);
    let db = store.database();

    sync_n(&provider, db.as_ref(), 1).await;
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1);
    assert_eq!(sent_uids(&store), vec![1, 2]);
    let inbox_thread = {
        let inbox = store.locations_in_mailbox("INBOX").unwrap();
        store.thread_of_message(&inbox[0].message_id).unwrap().unwrap()
    };

    // Reset Sent's UIDVALIDITY and reseed with a fresh message.
    {
        let mut g = mailbox.lock().unwrap();
        let sent = g.folder("Sent", 999);
        sent.uidvalidity = 999;
        sent.uidnext = 1;
        sent.messages.clear();
        sent.add_sized(&["\\Seen"], &message("<s9@x>", "Fresh", ""), None);
    }
    sync_n(&provider, db.as_ref(), 2).await;

    // INBOX untouched; its thread mapping preserved.
    assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1, "INBOX untouched");
    let inbox_after = store.locations_in_mailbox("INBOX").unwrap();
    assert_eq!(
        store.thread_of_message(&inbox_after[0].message_id).unwrap().unwrap(),
        inbox_thread,
        "INBOX message->thread mapping kept"
    );
    // Sent dropped and resynced under the new UIDVALIDITY.
    let sent = store.locations_in_mailbox("Sent").unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].uidvalidity, 999, "Sent resynced under the new UIDVALIDITY");
}

/// (h) Catalog refresh adds a mailbox created AFTER setup without
/// clobbering counters, ignores \Noselect, and a failing LIST does not
/// abort the poll.
#[tokio::test]
async fn catalog_refresh_notices_a_new_mailbox_without_clobbering_counters() {
    use std::sync::atomic::{AtomicI64, Ordering};
    // Start with just INBOX in the catalog; the server will later grow a
    // Junk folder that did not exist at setup.
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    static CLOCK_H: AtomicI64 = AtomicI64::new(1_700_000_000);
    CLOCK_H.store(1_700_000_000, Ordering::SeqCst);
    provider.now = || CLOCK_H.load(Ordering::SeqCst);
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    // Only INBOX in the catalog so far.
    let names: Vec<String> = store.mailboxes().unwrap().into_iter().map(|m| m.name).collect();
    assert_eq!(names, vec!["INBOX".to_string()]);

    // Record INBOX's learned counters, to prove the refresh never clobbers
    // them on a mailbox that already exists.
    let inbox_before = store
        .mailboxes()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "INBOX")
        .unwrap();
    assert!(inbox_before.uidvalidity != 0, "INBOX learned a real uidvalidity");

    // A Junk folder appears on the server after setup. Advance the clock
    // past the catalog sweep interval so the refresh is due again.
    {
        let mut g = mailbox.lock().unwrap();
        g.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "Spam", ""));
    }
    CLOCK_H.store(
        1_700_000_000 + super::super::policy::FOLDER_SWEEP_INTERVAL_SECS + 1,
        Ordering::SeqCst,
    );

    // Re-run with a due catalog refresh.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    // Junk is now in the catalog (noticed by the refresh) and gets synced.
    let names: Vec<String> = store.mailboxes().unwrap().into_iter().map(|m| m.name).collect();
    assert!(names.contains(&"Junk".to_string()), "new Junk noticed: {names:?}");

    // INBOX's counters were not clobbered by the catalog-only upsert.
    let inbox_after = store
        .mailboxes()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "INBOX")
        .unwrap();
    assert_eq!(
        inbox_after.uidvalidity, inbox_before.uidvalidity,
        "catalog refresh never clobbers an existing mailbox's counters"
    );
}

/// Catalog refresh: a failing LIST is swallowed and the poll still ingests
/// INBOX; \Noselect containers are never added.
#[tokio::test]
async fn catalog_refresh_failure_does_not_abort_and_noselect_is_skipped() {
    use std::sync::atomic::{AtomicI64, Ordering};
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    mb.list_fails = true; // the server's LIST answers an error
    mb.noselect.push("Folders".into()); // a \Noselect container the LIST reports
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    static CLOCK_C: AtomicI64 = AtomicI64::new(1_700_000_000);
    CLOCK_C.store(1_700_000_000, Ordering::SeqCst);
    provider.now = || CLOCK_C.load(Ordering::SeqCst);
    let db = store.database();

    // The poll succeeds and INBOX mail is ingested despite the LIST error.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .expect("a failing catalog LIST must not abort the poll");
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

    // Heal LIST; advance past the catalog sweep interval so the refresh is
    // due again. The \Noselect container is reported but never added.
    {
        let mut g = mailbox.lock().unwrap();
        g.list_fails = false;
    }
    CLOCK_C.store(
        1_700_000_000 + super::super::policy::FOLDER_SWEEP_INTERVAL_SECS + 1,
        Ordering::SeqCst,
    );
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let names: Vec<String> = store.mailboxes().unwrap().into_iter().map(|m| m.name).collect();
    assert!(!names.contains(&"Folders".to_string()), "\\Noselect never added: {names:?}");
}

/// (i) A Sent-first reply whose parent arrives later in INBOX merges into
/// ONE thread; the older id survives, an alias is recorded, and the engine
/// ends with one local thread.
#[tokio::test]
async fn a_sent_first_reply_merges_with_a_later_inbox_parent_into_one_thread() {
    // The reply is in Sent first (we sent it), referencing a parent
    // Message-ID that only arrives in INBOX on a later poll.
    let mut mb = FakeMailbox::new(100);
    mb.add_to(
        "Sent",
        90,
        &["\\Seen"],
        &message("<reply@x>", "Re: Hi", "In-Reply-To: <parent@x>\r\nReferences: <parent@x>\r\n"),
    );
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(10);
    let db = store.database();

    sync_n(&provider, db.as_ref(), 1).await;
    // The reply made its thread hot (Sent location) and is one thread.
    let sent = store.locations_in_mailbox("Sent").unwrap();
    let reply_thread = store.thread_of_message(&sent[0].message_id).unwrap().unwrap();
    assert!(store.is_thread_hot(&reply_thread).unwrap(), "a Sent reply is hot");

    // The parent now arrives in INBOX, linking to the Sent reply.
    {
        let mut g = mailbox.lock().unwrap();
        g.add(&["\\Seen"], &message("<parent@x>", "Hi", ""));
    }
    sync_n(&provider, db.as_ref(), 1).await;

    // Exactly ONE local thread remains; both messages are in it.
    let rows = store.all_message_threads().unwrap();
    let survivors: std::collections::BTreeSet<String> = rows
        .iter()
        .map(|(_, t, _)| store.resolve_thread_alias(t).unwrap())
        .collect();
    assert_eq!(survivors.len(), 1, "the Sent reply and INBOX parent are one thread");
    let survivor = survivors.into_iter().next().unwrap();
    assert_eq!(store.messages_in_thread(&survivor).unwrap().len(), 2);
    // The thread carries both INBOX and SENT labels across its copies.
    let labels = thread_labels(&provider, &survivor).await;
    assert!(labels.contains(&"INBOX".to_string()) && labels.contains(&"SENT".to_string()),
        "merged thread spans INBOX and SENT: {labels:?}");
}

/// (j) A Sent-only thread is hot, journaled, ingestible with a body,
/// carries SENT and no INBOX, and does NOT appear in list_inbox; an idle
/// reconcile poll does not re-journal it or treat it as INBOX drift.
///
/// Full recovery lists INBOX and then consumes the baseline's replay of
/// other hot threads through its incremental pass.
#[tokio::test]
async fn a_sent_only_thread_is_hot_journaled_and_not_treated_as_inbox_drift() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Sent", 90, &["\\Seen"], &message("<s1@x>", "Only sent", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(10);

    // baseline_cursor refreshes every synced mailbox and journals the Sent
    // thread (hot). It is reported to the engine on a poll from BELOW its
    // generation — the at-least-once path the engine ingests through.
    let baseline = provider.baseline_cursor().await.unwrap();
    let sent = store.locations_in_mailbox("Sent").unwrap();
    assert_eq!(sent.len(), 1, "the Sent message was acquired");
    let thread = store.thread_of_message(&sent[0].message_id).unwrap().unwrap();
    assert!(store.is_thread_hot(&thread).unwrap(), "a Sent-only thread is hot");

    // Journaled: a poll from generation 0 returns the Sent thread (this is
    // how the engine ingests a hot thread).
    let from_zero = provider.poll(&SyncCursor::from_generation(0)).await.unwrap();
    assert!(
        from_zero.changed_threads.iter().any(|t| {
            store.resolve_thread_alias(t).unwrap() == store.resolve_thread_alias(&thread).unwrap()
        }),
        "a hot Sent-only thread is journaled/reported: {:?}",
        from_zero.changed_threads
    );

    // SENT and no INBOX; a body is fetchable; and it is NOT in list_inbox.
    let labels = thread_labels(&provider, &thread).await;
    assert!(labels.contains(&"SENT".to_string()) && !labels.contains(&"INBOX".to_string()));
    assert_eq!(provider.fetch_thread(&thread).await.unwrap().len(), 1, "has a body");
    assert!(
        !provider.list_inbox(None).await.unwrap().thread_ids.contains(&thread),
        "a Sent-only thread is not in list_inbox"
    );

    // The incremental pass the engine runs right after the baseline delivers
    // the Sent-only thread (the baseline cursor sits before the hot-thread
    // replay). Once delivered, a poll from the NEW cursor
    // is idle: it neither bumps the generation nor re-journals the thread,
    // so the Sent-only thread is never treated as INBOX drift.
    let generation = store.generation().unwrap();
    let delivered = provider.poll(&baseline).await.unwrap();
    assert!(
        delivered.changed_threads.iter().any(|t| {
            store.resolve_thread_alias(t).unwrap() == store.resolve_thread_alias(&thread).unwrap()
        }),
        "the pass after the baseline delivers the Sent-only thread"
    );
    assert_eq!(store.generation().unwrap(), generation, "delivery does not bump");
    let idle = provider.poll(&delivered.cursor).await.unwrap();
    assert_eq!(store.generation().unwrap(), generation, "idle reconcile does not bump");
    assert!(idle.changed_threads.is_empty(), "idle reconcile re-journals nothing");
}

/// Full recovery ingests both INBOX's snapshot and the replay of hot
/// threads outside INBOX on its first incremental pass.
#[tokio::test]
async fn sent_only_threads_acquired_at_baseline_reach_the_engine() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    mb.add_to("Sent", 90, &["\\Seen"], &message("<s1@x>", "Only sent", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox);
    provider.sent_round_budget = Some(10);
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(
        db.list_all_mail(Some("me@example.com")).unwrap().len(),
        2,
        "the INBOX thread AND the Sent-only thread are ingested by the first sync"
    );
    // And nothing is lost or duplicated by the next pass.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 2);
}

#[tokio::test]
async fn sent_baseline_crash_before_engine_recovery_is_saved_replays_acquired_mail() {
    let mut mb = FakeMailbox::new(100);
    seed_sent(&mut mb, 90, 3);
    let (mut provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    provider.sent_round_budget = Some(1);
    let db = store.database();
    // Two interruptions after baseline commits, before begin_sync_recovery.
    for expected in [vec![3], vec![2, 3]] {
        provider.baseline_cursor().await.unwrap();
        assert_eq!(sent_uids(&store), expected);
        assert!(db.cursor("me@example.com").unwrap().is_none());
        assert!(db.recovery_cursor("me@example.com").unwrap().is_none());
    }
    // Reconstruct the provider's transient state, keeping only persisted
    // state and the fake server across the simulated process restart.
    let provider = ImapProvider {
        cache: BodyCache::new(store.clone()),
        thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        ..provider
    };
    sync_n(&provider, db.as_ref(), 1).await;
    assert_eq!(
        db.list_all_mail(Some("me@example.com")).unwrap().len(),
        3,
        "every previously acquired chunk reaches the engine on recovery"
    );
    let generation = store.generation().unwrap();
    sync_n(&provider, db.as_ref(), 2).await;
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 3);
    assert_eq!(
        store.generation().unwrap(),
        generation,
        "idle polls do not replay"
    );
}

#[tokio::test]
async fn baseline_replay_survives_journal_pruning_and_excludes_cold_threads() {
    let mut mb = FakeMailbox::new(100);
    seed_sent(&mut mb, 90, 1);
    mb.add_to("Trash", 80, &["\\Seen"], &message("<cold@x>", "Cold", ""));
    let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
    let db = store.database();
    provider.baseline_cursor().await.unwrap();
    let old = store.generation().unwrap();
    // Age the acquired Sent message out of journal retention.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE imap_sync_state SET generation = generation + ?1 WHERE account_id = ?2",
            rusqlite::params![(policy::JOURNAL_RETENTION_GENERATIONS + 1) as i64, "me@example.com"],
        )?;
        Ok(())
    })
    .unwrap();
    store.prune_journal().unwrap();
    assert!(store.journal_since(0).unwrap().is_empty());
    assert!(!policy::journal_cursor_is_answerable(
        old,
        store.generation().unwrap()
    ));

    sync_n(&provider, db.as_ref(), 1).await;
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    assert!(
        db.list_trash(Some("me@example.com")).unwrap().is_empty(),
        "a cold Trash-only thread remains index-only during baseline replay"
    );
}

#[tokio::test]
async fn baseline_replays_deletions_of_previously_hot_sent_threads() {
    let mut mb = FakeMailbox::new(100);
    seed_sent(&mut mb, 90, 1);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();
    sync_n(&provider, db.as_ref(), 1).await;
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    mailbox.lock().unwrap().folder("Sent", 90).messages.clear();
    // The provider commits the expunge; the engine does not consume it.
    provider.refresh_all().await.unwrap();
    assert!(sent_uids(&store).is_empty());
    // A full recovery still needs the now-locationless hot thread id. Under
    // the Slice 5b-2 EMPTIED_THREAD_GRACE_WALKS grace the thread is NOT
    // deleted on first observation of emptiness — it SURVIVES the grace and
    // is deleted only after EMPTIED_THREAD_GRACE_WALKS completed coverage epochs pass
    // (this test previously asserted immediate deletion). Drive polls until
    // the grace expires and the engine deletes it, bounded.
    db.clear_cursor("me@example.com").unwrap();
    let mut deleted = false;
    for _ in 0..8 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
            deleted = true;
            break;
        }
    }
    assert!(deleted, "the long-empty hot thread is deleted once the grace expires");
}

#[tokio::test]
async fn retained_sent_flags_are_synced_outside_the_acquisition_window() {
    let mut mb = FakeMailbox::new(100);
    seed_sent(&mut mb, 90, 2);
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.folder_window_override = Some(2);
    let db = store.database();
    sync_n(&provider, db.as_ref(), 1).await;
    mailbox
        .lock()
        .unwrap()
        .add_to("Sent", 90, &["\\Seen"], &message("<s3@x>", "New", ""));
    sync_n(&provider, db.as_ref(), 1).await;
    assert_eq!(
        sent_uids(&store),
        vec![1, 2, 3],
        "non-evicting Sent retains UID 1"
    );
    let before_flag_change = store.generation().unwrap();
    let old = store
        .locations_in_mailbox("Sent")
        .unwrap()
        .into_iter()
        .find(|l| l.uid == 1)
        .unwrap();
    let old_thread = store.thread_of_message(&old.message_id).unwrap().unwrap();
    let before_body_fetches = mailbox.lock().unwrap().counts("Sent").1;
    mailbox.lock().unwrap().folder("Sent", 90).messages[0].flags =
        vec!["\\Seen".into(), "\\Flagged".into()];
    // Unchanged EXISTS/UIDNEXT: the periodic sweep must discover the flag.
    provider.now = || 1_700_000_000 + policy::FOLDER_SWEEP_INTERVAL_SECS;
    sync_n(&provider, db.as_ref(), 1).await;
    let old = store
        .locations_in_mailbox("Sent")
        .unwrap()
        .into_iter()
        .find(|l| l.uid == 1)
        .unwrap();
    let flags: Vec<String> = serde_json::from_str(&old.flags_json).unwrap();
    assert!(
        flags.contains(&"\\Flagged".into()),
        "a Sent sweep updates retained messages outside the acquisition window: {flags:?}"
    );
    assert!(store
        .journal_since(before_flag_change)
        .unwrap()
        .contains(&old_thread));
    assert!(
        db.list_all_mail(Some("me@example.com"))
            .unwrap()
            .iter()
            .any(|thread| thread.provider_thread_id == old_thread && thread.starred),
        "the engine receives the retained Sent thread's updated star"
    );
    assert_eq!(
        mailbox.lock().unwrap().counts("Sent").1,
        before_body_fetches,
        "a retained flag update does not re-download bodies"
    );
}

/// (k) Idle polls with Sent complete: generation unchanged, no threader
/// rebuild.
#[tokio::test]
async fn idle_polls_with_sent_complete_do_not_bump_the_generation() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    seed_sent(&mut mb, 90, 2);
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox);
    provider.sent_round_budget = Some(10);
    let db = store.database();

    sync_n(&provider, db.as_ref(), 1).await;
    assert!(sent_marker(&store).is_none(), "backfill complete");
    let generation = store.generation().unwrap();
    let loads = provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(store.generation().unwrap(), generation, "idle poll does not bump generation");
    assert_eq!(
        provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
        loads,
        "idle poll does not rebuild threader state"
    );
}

// ------------------------------------------------------------------
// Slice 5b-2: fair scheduling, Archive sync, emptied-hot-thread grace
// ------------------------------------------------------------------

/// Count the open tasks linked to a provider thread id (via its local id).
fn task_count_for(store: &ImapStateStore, provider_thread_id: &str) -> i64 {
    let local = format!("me@example.com:{provider_thread_id}");
    store
        .database()
        .with_connection(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM tasks WHERE thread_id = ?1",
                [&local],
                |r| r.get(0),
            )?)
        })
        .unwrap()
}

/// Attach a task to a provider thread's LOCAL thread row (as the triage
/// surface would), so a test can prove the grace leaves local state intact.
fn attach_task(store: &ImapStateStore, provider_thread_id: &str, task_id: &str) {
    let local = format!("me@example.com:{provider_thread_id}");
    store
        .database()
        .with_connection(|c| {
            c.execute(
                "INSERT INTO tasks(id, account_id, thread_id, subject_snapshot, title, kind,
                    due_kind, status, created_at, updated_at)
                 VALUES (?1, 'me@example.com', ?2, 'S', 'T', 'action', 'none', 'open',
                    '2026-09-20T00:00:00Z', '2026-09-20T00:00:00Z')",
                rusqlite::params![task_id, local],
            )?;
            Ok(())
        })
        .unwrap();
}

/// (a) A hot INBOX message moved to ARCHIVE by another client: because
/// Archive is now synced (5b-2), its new location is picked up, the thread
/// survives as ONE local thread with NO INBOX label (archived == no system
/// location label), and the engine never deletes it.
#[tokio::test]
async fn a_hot_inbox_message_moved_to_archive_survives_without_an_inbox_label() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    mb.folder("Archive", 95); // present but empty for now
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let threads = provider.list_inbox(None).await.unwrap();
    assert_eq!(threads.thread_ids.len(), 1);
    let thread_id = threads.thread_ids[0].clone();
    assert_eq!(thread_labels(&provider, &thread_id).await, vec!["INBOX"]);

    // Another client moves m1 from INBOX to Archive (same Message-ID ->
    // same stable id).
    {
        let mut mb = mailbox.lock().unwrap();
        mb.inbox().messages.retain(|m| m.uid != 1);
        mb.add_to("Archive", 95, &["\\Seen"], &message("<m1@x>", "Hello", ""));
    }
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    // INBOX location gone, Archive location present, ONE engine thread.
    assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
    assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1);
    assert_eq!(
        db.list_all_mail(Some("me@example.com")).unwrap().len(),
        1,
        "the engine keeps exactly one local thread"
    );
    // The thread carries NO INBOX label — archived has no system location
    // label — and is not NotFound.
    let labels = thread_labels(&provider, &thread_id).await;
    assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after archiving: {labels:?}");
}

/// (b) A hot INBOX message moved into a mailbox NOT swept this poll (forced
/// via `examine_fails` on the destination): the thread has zero locations
/// momentarily, SURVIVES the grace (its local copy — including a linked
/// task — is untouched), then the destination recovers and the thread is
/// re-journaled with the right labels.
#[tokio::test]
async fn a_move_into_an_unswept_mailbox_survives_the_grace_then_recovers() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    // Archive exists but EXAMINE fails this poll (stands in for "a mailbox
    // not swept this poll" — the scheduler visited it but it errored, so
    // the move's destination is not seen).
    mb.folder("Archive", 95).examine_fails = true;
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let thread_id = provider.list_inbox(None).await.unwrap().thread_ids[0].clone();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    // Attach a task to the local thread; it must survive the grace.
    attach_task(&store, &thread_id, "task-b");
    assert_eq!(task_count_for(&store, &thread_id), 1);

    // Another client moves m1 out of INBOX into the (un-examinable) Archive.
    {
        let mut mb = mailbox.lock().unwrap();
        mb.inbox().messages.retain(|m| m.uid != 1);
        mb.add_to("Archive", 95, &["\\Seen"], &message("<m1@x>", "Hello", ""));
    }
    // Poll: INBOX location gone, Archive EXAMINE fails so the destination
    // is not seen -> the hot thread has zero locations. The grace keeps it.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
    assert_eq!(
        db.list_all_mail(Some("me@example.com")).unwrap().len(),
        1,
        "the thread survives the grace while its destination is unswept"
    );
    assert_eq!(task_count_for(&store, &thread_id), 1, "the linked task survived the grace");

    // The destination recovers: Archive is examinable again.
    mailbox.lock().unwrap().folder("Archive", 95).examine_fails = false;
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1, "Archive location found");
    // Still one thread, task intact, and now re-journaled with correct
    // labels (no INBOX; Archive contributes no system label).
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    assert_eq!(task_count_for(&store, &thread_id), 1, "task preserved after recovery");
    let labels = thread_labels(&provider, &thread_id).await;
    assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after the move: {labels:?}");
}

/// (c) A true server-side expunge of the last copy: the local thread is
/// kept for the grace, then deleted after EMPTIED_THREAD_GRACE_WALKS
/// completed coverage epochs; idle polls during the grace do not bump the
/// generation.
#[tokio::test]
async fn a_true_expunge_keeps_the_thread_for_the_grace_then_deletes_it() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

    // The server expunges the only copy.
    mailbox.lock().unwrap().inbox().messages.clear();

    // First poll after the expunge: the thread has zero locations and is
    // hot, so the grace keeps it (NOT deleted yet).
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(
        db.list_all_mail(Some("me@example.com")).unwrap().len(),
        1,
        "the first empty observation does not delete (grace)"
    );
    let gen_during_grace = store.generation().unwrap();

    // An idle poll during the grace (nothing changed) does not bump the
    // generation.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    // Keep polling until the thread is finally deleted once the grace is spent.
    let mut deleted = false;
    for _ in 0..8 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
            deleted = true;
            break;
        }
    }
    assert!(deleted, "the thread is deleted after EMPTIED_THREAD_GRACE_WALKS completed coverage epochs");
    // The generation moved only when the grace expired (the deletion was
    // journaled), not on the idle grace polls: it is >= the grace-era value.
    assert!(store.generation().unwrap() >= gen_during_grace);
}

/// (d) A folder that keeps failing EXAMINE is never visited in the current
/// coverage epoch, so the epoch cannot advance and the emptied-thread grace
/// does not progress: nothing is deleted early. Once the folder recovers,
/// the epoch advances and the grace expires.
#[tokio::test]
async fn a_failing_folder_blocks_the_coverage_epoch_and_the_grace() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    // A Trash folder that always fails EXAMINE -> it is never visited in
    // the current epoch, so the epoch never completes.
    mb.folder("Trash", 50).examine_fails = true;
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    let walks_before = store.complete_walks().unwrap();

    // Expunge the INBOX copy: the thread goes empty.
    mailbox.lock().unwrap().inbox().messages.clear();

    // Many polls, but Trash keeps failing so the coverage epoch cannot
    // advance and the grace never progresses: the thread is NOT deleted.
    for _ in 0..6 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    assert_eq!(
        store.complete_walks().unwrap(),
        walks_before,
        "a failing folder blocks coverage-epoch advance"
    );
    assert_eq!(
        db.list_all_mail(Some("me@example.com")).unwrap().len(),
        1,
        "the emptied thread is NOT deleted while coverage cannot complete"
    );

    // Heal Trash: walks complete again, the grace advances, deletion lands.
    mailbox.lock().unwrap().folder("Trash", 50).examine_fails = false;
    let mut deleted = false;
    for _ in 0..8 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
            deleted = true;
            break;
        }
    }
    assert!(deleted, "once walks complete again the grace expires and the thread is deleted");
}

/// (k) Archive is an EVICTING window (Folder class) at a tiny limit, while
/// Sent (acquisition bound) is NOT, in the same account.
#[tokio::test]
async fn archive_evicts_at_a_tiny_window_while_sent_does_not() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    // Archive and Sent each start with 2 messages.
    for i in 1..=2 {
        mb.add_to("Archive", 95, &["\\Seen"], &message(&format!("<a{i}@x>"), "A", ""));
        mb.add_to("Sent", 90, &["\\Seen"], &message(&format!("<s{i}@x>"), "S", ""));
    }
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.sent_round_budget = Some(10);
    provider.folder_window_override = Some(2); // tiny window for both
    let db = store.database();

    sync_n(&provider, db.as_ref(), 2).await;
    let archive_uids = |s: &ImapStateStore| {
        let mut u: Vec<i64> = s.locations_in_mailbox("Archive").unwrap().into_iter().map(|l| l.uid).collect();
        u.sort_unstable();
        u
    };
    assert_eq!(archive_uids(&store), vec![1, 2], "both Archive acquired");
    assert_eq!(sent_uids(&store), vec![1, 2], "both Sent acquired");

    // Newer mail pushes the oldest (UID 1) past the 2-message window in both.
    {
        let mut g = mailbox.lock().unwrap();
        g.add_to("Archive", 95, &["\\Seen"], &message("<a3@x>", "A3", ""));
        g.add_to("Sent", 90, &["\\Seen"], &message("<s3@x>", "S3", ""));
    }
    sync_n(&provider, db.as_ref(), 2).await;

    // Archive EVICTS the aged-out UID 1 (sliding window); Sent keeps it.
    assert_eq!(archive_uids(&store), vec![2, 3], "Archive evicts the aged-out UID 1");
    assert_eq!(sent_uids(&store), vec![1, 2, 3], "Sent never evicts (acquisition bound)");
}

/// (l) Fairness integration: with 20 synthetic extra `synced_now` entries
/// injected through the test-only scheduler knob and a small budget, every
/// one is VISITED within ceil(20/budget) polls. The synthetic candidates
/// are scheduled by name only (no plan entry, so no server round), proving
/// the scheduler's rotation without a 20-folder fixture.
#[tokio::test]
async fn fairness_every_injected_folder_is_scheduled_within_ceil_n_over_budget_polls() {
    // The scheduler is a pure function; here we drive it the way
    // refresh_all_with does, through the injected candidates, and record
    // which names schedule_visits returns across polls. We assert on the
    // SELECTION (schedule_visits output) which is what bounds server work.
    let budget = policy::FOLDER_ROUNDS_PER_POLL; // production budget
    let n = 20usize;
    // Build 20 synthetic user-folder candidates (never covered at start).
    let names: Vec<String> = (0..n).map(|i| format!("folder-{i:02}")).collect();
    let epoch = 1i64;
    let mut visited_in_epoch: std::collections::BTreeMap<String, i64> =
        names.iter().map(|n| (n.clone(), 0i64)).collect();
    let mut ever: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let polls = n.div_ceil(budget);
    for _ in 0..polls {
        let candidates: Vec<policy::ScheduleCandidate> = names
            .iter()
            .map(|name| policy::ScheduleCandidate {
                mailbox: name.clone(),
                last_attempt: 0,
                last_visited_at: 0,
                visited_in_epoch: visited_in_epoch[name],
                is_system_role: false,
            })
            .collect();
        let visits = policy::schedule_visits(&candidates, epoch, budget);
        for v in &visits {
            ever.insert(v.clone());
            visited_in_epoch.insert(v.clone(), epoch);
        }
    }
    assert_eq!(ever.len(), n, "every injected folder visited within ceil(N/budget) polls");

    // And prove the knob is wired into refresh_all_with: an injected
    // candidate is accepted (does not panic / abort the walk) and the walk
    // still completes INBOX. The synthetic names have no plan entry, so
    // they take no server round.
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox);
    provider.extra_synced_candidates = Some(
        names
            .iter()
            .map(|name| policy::ScheduleCandidate {
                mailbox: name.clone(),
                last_attempt: 0,
                last_visited_at: 0,
                visited_in_epoch: 0,
                is_system_role: false,
            })
            .collect(),
    );
    let db = store.database();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .expect("injected synthetic candidates must not wedge the walk");
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1, "INBOX still synced");
}

// ------------------------------------------------------------------
// Slice 5b-2 coverage-epoch fix: a "complete walk" is coverage ACROSS
// polls, not a single poll that visited every mailbox. With more synced
// mailboxes than the per-poll budget, no single poll covers them all, so
// the epoch (and the emptied-thread grace) must still advance once every
// synced mailbox has been visited across successive polls.
// ------------------------------------------------------------------

/// (1) More synced mailboxes than the per-poll budget: the coverage epoch
/// advances only after EVERY synced mailbox was visited (over several
/// polls), and a truly-expunged hot thread's last copy is deleted after
/// exactly two completed epochs — not never (the old per-poll definition
/// would have deadlocked the epoch here forever).
#[tokio::test]
async fn coverage_epoch_advances_across_polls_and_grace_eventually_deletes() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    // Two extra server-backed folders, forced to USER-FOLDER class so they
    // count against the per-poll budget (not system roles, which are always
    // visited). They start non-empty so they are examinable.
    mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "J", ""));
    mb.add_to("Archive", 95, &["\\Seen"], &message("<a1@x>", "A", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.force_user_folders =
        Some(["Junk".to_string(), "Archive".to_string()].into_iter().collect());
    provider.folder_rounds_per_poll_override = Some(1); // one user folder per poll
    let db = store.database();

    // Baseline + one poll. With a budget of 1 and two user folders, no
    // single poll covers both — the epoch must still advance across polls.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

    // Expunge the only INBOX copy: the hot thread goes empty.
    mailbox.lock().unwrap().inbox().messages.clear();

    // Drive polls until the thread is deleted. Each poll visits INBOX +
    // (budget 1) one user folder, so it takes two polls to complete one
    // coverage epoch, and two completed epochs to spend the grace. The old
    // per-poll logic never advances the epoch here, so the thread would
    // linger forever — this loop would never see the deletion.
    let mut deleted = false;
    for _ in 0..12 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
            deleted = true;
            break;
        }
    }
    assert!(
        deleted,
        "a truly-expunged hot thread is deleted after the grace, even when no single poll covers every synced mailbox"
    );
    // The epoch really did advance beyond the baseline (coverage worked).
    assert!(store.complete_walks().unwrap() >= policy::EMPTIED_THREAD_GRACE_WALKS);
}

/// (2) A folder that keeps failing EXAMINE prevents the coverage epoch from
/// advancing (that folder is never visited in the current epoch), so
/// nothing is deleted early; once it recovers, the epoch advances and the
/// grace expires. Uses forced user folders + budget 1 so coverage genuinely
/// depends on the failing folder succeeding.
#[tokio::test]
async fn a_failing_folder_blocks_epoch_advance_until_it_recovers() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "J", ""));
    mb.add_to("Archive", 95, &["\\Seen"], &message("<a1@x>", "A", ""));
    // Archive (a forced user folder) always fails EXAMINE.
    mb.folder("Archive", 95).examine_fails = true;
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.force_user_folders =
        Some(["Junk".to_string(), "Archive".to_string()].into_iter().collect());
    provider.folder_rounds_per_poll_override = Some(1);
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let epoch_before = store.complete_walks().unwrap();

    // Expunge the INBOX copy: the thread goes empty.
    mailbox.lock().unwrap().inbox().messages.clear();

    // Many polls, but Archive never succeeds, so the epoch cannot advance
    // (Archive is never visited in the current epoch) and nothing is
    // deleted early.
    for _ in 0..10 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    assert_eq!(
        store.complete_walks().unwrap(),
        epoch_before,
        "a persistently failing folder blocks coverage-epoch advance"
    );
    assert_eq!(
        db.list_all_mail(Some("me@example.com")).unwrap().len(),
        1,
        "nothing is deleted early while coverage cannot complete"
    );

    // Heal Archive: the epoch advances and the grace eventually expires.
    mailbox.lock().unwrap().folder("Archive", 95).examine_fails = false;
    let mut deleted = false;
    for _ in 0..12 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
            deleted = true;
            break;
        }
    }
    assert!(deleted, "once the folder recovers the epoch advances and the thread is deleted");
}

/// (3) A poll that records visits but does NOT complete a coverage epoch
/// (partial coverage under a tiny budget) must not bump the generation —
/// visit recording is cadence/bookkeeping state, not a content change.
#[tokio::test]
async fn partial_coverage_polls_do_not_bump_the_generation() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "J", ""));
    mb.add_to("Archive", 95, &["\\Seen"], &message("<a1@x>", "A", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.force_user_folders =
        Some(["Junk".to_string(), "Archive".to_string()].into_iter().collect());
    provider.folder_rounds_per_poll_override = Some(1);
    let db = store.database();

    // Baseline brings everything in and settles.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    // Let it reach a steady state where nothing content-changes.
    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    let generation = store.generation().unwrap();
    let loads = provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);

    // Several more idle polls. Each records a visit for the one scheduled
    // user folder (partial coverage, budget 1) but nothing content-changed,
    // so the generation must not move and the threader is not rebuilt —
    // whether or not a given poll happens to complete a coverage epoch.
    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(
            store.generation().unwrap(),
            generation,
            "an idle poll (partial or full coverage) does not bump the generation"
        );
    }
    assert_eq!(
        provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
        loads,
        "idle polls do not rebuild threader state"
    );
}

// ------------------------------------------------------------------
// Slice 5b-2 run B: folder:/lf: labels, user/label-folder sync, and
// vanished-mailbox retirement, all driven through sync::sync_with over
// the multi-mailbox fake.
// ------------------------------------------------------------------

/// Collect every label id across all messages of a thread, sorted + deduped
/// (the shared `thread_labels` above returns them unsorted/undeduped).
async fn thread_label_set(provider: &ImapProvider, thread_id: &str) -> Vec<String> {
    let mut labels = thread_labels(provider, thread_id).await;
    labels.sort();
    labels.dedup();
    labels
}

/// The stable message id of the (single) message currently in `mailbox`.
fn message_id_in(store: &ImapStateStore, mailbox: &str) -> String {
    store
        .locations_in_mailbox(mailbox)
        .expect("locations")
        .first()
        .map(|l| l.message_id.clone())
        .unwrap_or_else(|| panic!("no message in {mailbox}"))
}

/// The resolved thread id for a stable message id.
fn thread_of(store: &ImapStateStore, message_id: &str) -> String {
    let t = store
        .thread_of_message(message_id)
        .expect("thread_of_message")
        .expect("a thread");
    store.resolve_thread_alias(&t).expect("resolve")
}

/// (a) A hot INBOX message that another client MOVED into a user folder:
/// the thread survives, its labels become `folder:<name>` WITHOUT INBOX,
/// and the engine keeps ONE local thread with the folder label on its root.
#[tokio::test]
async fn an_inbox_message_moved_into_a_user_folder_keeps_the_thread_with_a_folder_label() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<u1@x>", "Hello", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    // Baseline: the message is in INBOX, hot.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    let mid = message_id_in(&store, "INBOX");
    let tid = thread_of(&store, &mid);
    assert!(store.is_thread_hot(&tid).unwrap(), "an INBOX message is hot");
    assert_eq!(thread_label_set(&provider, &tid).await, vec!["INBOX"]);

    // Another client moves it: INBOX copy gone, a copy appears in the user
    // folder Clients/Acme (same Message-ID = same stable id).
    {
        let mut m = mailbox.lock().unwrap();
        m.inbox().messages.clear();
        m.add_to("Clients/Acme", 70, &["\\Seen"], &message("<u1@x>", "Hello", ""));
    }
    // The destination folder is brand new on the server, so re-arm the
    // catalog refresh to notice it (the fixed test clock otherwise keeps the
    // periodic refresh from coming due again).
    store.force_catalog_refresh_due_for_test();
    // Poll enough times to visit INBOX (sees the deletion) and the user
    // folder (sees the new copy) within the emptied-thread grace.
    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }

    // One local thread survives, labelled folder:Clients/Acme, no INBOX.
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    let mid = message_id_in(&store, "Clients/Acme");
    let tid = thread_of(&store, &mid);
    let labels = thread_label_set(&provider, &tid).await;
    assert!(labels.contains(&"folder:Clients/Acme".to_string()), "{labels:?}");
    assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after the move: {labels:?}");
}

/// (b) Proton-style label copy: an INBOX message with an additional copy in
/// `Labels/Clients` -> `lf:Clients` + INBOX, and the `lf:` label is on
/// EVERY message of the thread even when only a REPLY was labelled.
#[tokio::test]
async fn a_label_folder_copy_adds_lf_to_every_message_of_the_thread() {
    let mut mb = FakeMailbox::new(100);
    // Root in INBOX; a reply in INBOX; the REPLY also copied into the label
    // folder (Proton labels one message of the thread).
    mb.add(&["\\Seen"], &message("<root@x>", "Topic", ""));
    mb.add(&["\\Seen"], &message("<reply@x>", "Re: Topic", "In-Reply-To: <root@x>\r\n"));
    mb.add_to(
        "Labels/Clients",
        70,
        &["\\Seen"],
        &message("<reply@x>", "Re: Topic", "In-Reply-To: <root@x>\r\n"),
    );
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }

    let mid = message_id_in(&store, "Labels/Clients");
    let tid = thread_of(&store, &mid);
    // The thread has INBOX and lf:Clients.
    let labels = thread_label_set(&provider, &tid).await;
    assert!(labels.contains(&"INBOX".to_string()), "{labels:?}");
    assert!(labels.contains(&"lf:Clients".to_string()), "{labels:?}");

    // Every message of the thread carries lf:Clients, even the root, which
    // was never itself in the label folder (fact 1 — thread-wide union).
    let messages = provider.fetch_thread(&tid).await.unwrap();
    assert_eq!(messages.len(), 2, "root + reply");
    for m in &messages {
        assert!(
            m.label_ids.contains(&"lf:Clients".to_string()),
            "message {} carries lf:Clients: {:?}",
            m.id,
            m.label_ids
        );
    }
}

/// (c) A message in TWO label folders gets both `lf:` labels; (i) dynamic
/// ids with spaces/colon/non-ASCII round-trip through fetch_thread.
#[tokio::test]
async fn a_message_in_two_label_folders_gets_both_lf_labels() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m@x>", "Two labels", ""));
    mb.add_to("Labels/Clients", 70, &["\\Seen"], &message("<m@x>", "Two labels", ""));
    mb.add_to("Labels/Zoë's: tag", 71, &["\\Seen"], &message("<m@x>", "Two labels", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    for _ in 0..5 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    let mid = message_id_in(&store, "Labels/Clients");
    let tid = thread_of(&store, &mid);
    let labels = thread_label_set(&provider, &tid).await;
    assert!(labels.contains(&"INBOX".to_string()), "{labels:?}");
    assert!(labels.contains(&"lf:Clients".to_string()), "{labels:?}");
    assert!(labels.contains(&"lf:Zoë's: tag".to_string()), "{labels:?}");
}

/// (e) A never-hot thread that only ever lives in a user folder is RECORDED
/// (index-synced locations + a local thread) but NEVER hot — so it is never
/// reported to the engine as a changed hot thread.
#[tokio::test]
async fn a_user_folder_only_thread_is_recorded_but_never_hot() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i@x>", "Inbox", "")); // keeps INBOX non-trivial
    mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<folder-only@x>", "Folder only", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    // The folder-only message is index-synced (its location is recorded).
    assert!(
        !store.locations_in_mailbox("Clients/Acme").unwrap().is_empty(),
        "the user folder is index-synced"
    );
    // Its thread exists locally but is NOT hot (never reported).
    let mid = message_id_in(&store, "Clients/Acme");
    let tid = thread_of(&store, &mid);
    assert!(!store.is_thread_hot(&tid).unwrap(), "a user-folder-only thread is never hot");
    // And it still resolves to its folder: label when fetched directly.
    let labels = thread_label_set(&provider, &tid).await;
    assert_eq!(labels, vec!["folder:Clients/Acme"]);
}

/// (f) `list_labels` content, order and kinds: the six system labels, then
/// `lf:` labels (kind `user`, relative name) sorted, then `folder:` labels
/// (kind `folder`, full name) sorted — INCLUDING a label folder with no
/// messages in it.
#[tokio::test]
async fn list_labels_publishes_system_then_lf_then_folder_in_order() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
    mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<c@x>", "C", ""));
    mb.add_to("Labels/Work", 71, &["\\Seen"], &message("<w@x>", "W", ""));
    // An EMPTY label folder (no messages) must still be published.
    mb.folder("Labels/Empty", 72);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    let labels = provider.list_labels().await.unwrap();
    let shape: Vec<(&str, &str)> =
        labels.iter().map(|l| (l.id.as_str(), l.kind.as_str())).collect();
    assert_eq!(
        shape,
        vec![
            ("INBOX", "system"),
            ("SENT", "system"),
            ("SPAM", "system"),
            ("TRASH", "system"),
            ("UNREAD", "system"),
            ("STARRED", "system"),
            ("lf:Empty", "user"),
            ("lf:Work", "user"),
            ("folder:Clients/Acme", "folder"),
        ]
    );
    // The dynamic labels carry a display name = id without its prefix.
    let empty = labels.iter().find(|l| l.id == "lf:Empty").unwrap();
    assert_eq!(empty.name, "Empty");
    let acme = labels.iter().find(|l| l.id == "folder:Clients/Acme").unwrap();
    assert_eq!(acme.name, "Clients/Acme");
}

/// (g) With MORE real user folders than the per-poll budget, every one is
/// visited within ceil(N/budget) polls and the coverage epoch advances so
/// an expunged hot thread is deleted after the grace — proving user folders
/// (not just the synthetic knob) feed the scheduler/epoch machinery.
#[tokio::test]
async fn many_real_user_folders_are_all_covered_and_the_epoch_advances() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
    // Three real user folders, budget 1: no single poll covers them all.
    for (i, name) in ["Clients/A", "Clients/B", "Clients/C"].iter().enumerate() {
        mb.add_to(name, 60 + i as u32, &["\\Seen"], &message(&format!("<f{i}@x>"), "F", ""));
    }
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.folder_rounds_per_poll_override = Some(1);
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

    // Expunge the INBOX copy: the hot thread goes empty.
    mailbox.lock().unwrap().inbox().messages.clear();

    // Drive polls: three user folders at budget 1 means a coverage epoch
    // takes several polls, and two epochs to spend the grace. The thread is
    // deleted only once the epoch machinery advances across polls.
    let mut deleted = false;
    for _ in 0..20 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
            deleted = true;
            break;
        }
    }
    assert!(deleted, "coverage over real user folders advances the epoch and spends the grace");
    assert!(store.complete_walks().unwrap() >= policy::EMPTIED_THREAD_GRACE_WALKS);
    // Every user folder was index-synced (all visited).
    for name in ["Clients/A", "Clients/B", "Clients/C"] {
        assert!(!store.locations_in_mailbox(name).unwrap().is_empty(), "{name} visited");
    }
}

/// (h) Vanished mailbox: a catalog row the server stops LISTing is retired
/// (catalog + sync-state + locations deleted), the affected hot thread is
/// journaled and kept through the grace, the epoch is no longer held open,
/// an empty/failed LIST retires nothing, INBOX is never retired, and a
/// re-appearing mailbox is re-added.
#[tokio::test]
async fn a_vanished_mailbox_is_retired_and_stops_holding_the_epoch_open() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
    // A user folder holding a HOT thread's only non-INBOX copy, plus an
    // INBOX copy so the thread is hot.
    mb.add(&["\\Seen"], &message("<shared@x>", "Shared", ""));
    mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<shared@x>", "Shared", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();

    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert!(!store.locations_in_mailbox("Clients/Acme").unwrap().is_empty());
    // The shared message is the one copied into Clients/Acme; resolve its
    // thread by that unique location (INBOX also holds the unrelated <i@x>).
    let shared_id = message_id_in(&store, "Clients/Acme");
    let tid = thread_of(&store, &shared_id);
    assert!(store.is_thread_hot(&tid).unwrap());

    // An EMPTY/failed LIST retires nothing: a LIST that fails is swallowed,
    // and the catalog is untouched.
    {
        let mut m = mailbox.lock().unwrap();
        m.list_fails = true;
    }
    store.force_catalog_refresh_due_for_test();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert!(
        store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Acme"),
        "a failed LIST retires nothing"
    );

    // Now the user folder genuinely vanishes from the server and LIST
    // succeeds without it.
    {
        let mut m = mailbox.lock().unwrap();
        m.list_fails = false;
        m.folders.remove("Clients/Acme");
    }
    store.force_catalog_refresh_due_for_test();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

    // Retired: catalog row, sync-state row and locations are gone.
    assert!(
        !store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Acme"),
        "the vanished mailbox's catalog row is retired"
    );
    assert!(store.locations_in_mailbox("Clients/Acme").unwrap().is_empty());
    assert!(store.mailbox_sync_state("Clients/Acme").unwrap().is_none());
    // INBOX was never retired.
    assert!(store.mailboxes().unwrap().iter().any(|mb| mb.name == "INBOX"));

    // The shared thread still has its INBOX copy, so it stays hot and
    // present (the retirement journaled it; it did not delete it).
    let tid = thread_of(&store, &shared_id);
    assert!(store.is_thread_hot(&tid).unwrap());
    assert!(!thread_label_set(&provider, &tid).await.is_empty());

    // The epoch is no longer held open by the vanished folder: a couple of
    // polls complete coverage over the REMAINING synced set.
    let epoch_before = store.complete_walks().unwrap();
    for _ in 0..3 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    assert!(
        store.complete_walks().unwrap() > epoch_before,
        "with the broken folder retired, the coverage epoch advances again"
    );

    // Re-appearance: the mailbox comes back and is re-added, its message
    // re-syncs (bodies untouched — threading is intact).
    {
        let mut m = mailbox.lock().unwrap();
        m.add_to("Clients/Acme", 99, &["\\Seen"], &message("<shared@x>", "Shared", ""));
    }
    store.force_catalog_refresh_due_for_test();
    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    assert!(
        store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Acme"),
        "a re-appearing mailbox is re-added"
    );
    assert!(!store.locations_in_mailbox("Clients/Acme").unwrap().is_empty());
}

/// A retirement that touches NO hot thread is a catalog-only change: it does
/// not bump the generation (nothing for the engine to re-ingest).
#[tokio::test]
async fn retiring_a_cold_only_mailbox_does_not_bump_the_generation() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
    // A user folder with a COLD (never-INBOX, never-hot) thread only.
    mb.add_to("Clients/Cold", 70, &["\\Seen"], &message("<cold@x>", "Cold", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    // Settle.
    for _ in 0..3 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    let gen_before = store.generation().unwrap();

    // Vanish the cold-only folder.
    {
        let mut m = mailbox.lock().unwrap();
        m.folders.remove("Clients/Cold");
    }
    store.force_catalog_refresh_due_for_test();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    assert!(
        !store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Cold"),
        "the cold-only folder is retired"
    );
    // No hot thread was affected, so the generation did not move for the
    // retirement itself (idle polls aside).
    assert_eq!(
        store.generation().unwrap(),
        gen_before,
        "retiring a cold-only mailbox is a catalog-only change"
    );
}

/// (k) Idle polls after the dynamic-label set is synced do not bump the
/// generation or rebuild the threader.
#[tokio::test]
async fn idle_polls_with_user_and_label_folders_are_stable() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
    mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<c@x>", "C", ""));
    mb.add_to("Labels/Work", 71, &["\\Seen"], &message("<w@x>", "W", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
    }
    let generation = store.generation().unwrap();
    let loads = provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);
    for _ in 0..4 {
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(store.generation().unwrap(), generation, "idle poll must not bump generation");
    }
    assert_eq!(
        provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
        loads,
        "idle polls do not rebuild threader state"
    );
}

#[tokio::test]
async fn expired_threads_do_not_rejournal_on_idle_polls() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<review-gone@x>", "Gone", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();
    sync_n(&provider, db.as_ref(), 1).await;
    mailbox.lock().unwrap().inbox().messages.clear();
    sync_n(&provider, db.as_ref(), 5).await;
    assert!(db.list_all_mail(Some("me@example.com")).unwrap().is_empty());
    let before = store.generation().unwrap();
    sync_n(&provider, db.as_ref(), 3).await;
    assert_eq!(
        store.generation().unwrap(),
        before,
        "a deleted thread must not re-journal forever"
    );
}

#[tokio::test]
async fn mailbox_cap_does_not_block_coverage_forever() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<review-cap@x>", "Cap", ""));
    for i in 0..=policy::MAX_SYNCED_MAILBOXES {
        mb.folder(&format!("Folder/{i:04}"), 10);
    }
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_with(mailbox.clone());
    provider.folder_rounds_per_poll_override = Some(policy::MAX_SYNCED_MAILBOXES);
    let db = store.database();
    sync_n(&provider, db.as_ref(), 1).await;
    mailbox.lock().unwrap().inbox().messages.clear();
    sync_n(&provider, db.as_ref(), 5).await;
    assert!(
        db.list_all_mail(Some("me@example.com")).unwrap().is_empty(),
        "capped-out folders must not make an actual expunge immortal; epoch={}",
        store.complete_walks().unwrap()
    );
}

#[tokio::test]
async fn failed_folders_do_not_starve_healthy_folders() {
    let mut mb = FakeMailbox::new(100);
    for i in 0..policy::FOLDER_ROUNDS_PER_POLL {
        mb.folder(&format!("A-fails-{i:02}"), 10).examine_fails = true;
    }
    mb.add_to(
        "Z-healthy",
        10,
        &["\\Seen"],
        &message("<healthy1@x>", "Healthy", ""),
    );
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    sync_n(&provider, store.database().as_ref(), 5).await;
    assert_eq!(store.locations_in_mailbox("Z-healthy").unwrap().len(), 1);
    assert_eq!(
        store.complete_walks().unwrap(),
        0,
        "failed attempts do not claim coverage"
    );
    mailbox.lock().unwrap().add_to(
        "Z-healthy",
        10,
        &["\\Seen"],
        &message("<healthy2@x>", "More healthy mail", ""),
    );
    let provider = ImapProvider {
        cache: BodyCache::new(store.clone()),
        thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        ..provider
    };
    sync_n(&provider, store.database().as_ref(), 3).await;
    assert_eq!(
        store.locations_in_mailbox("Z-healthy").unwrap().len(),
        2,
        "persisted attempt ordering keeps servicing healthy folders"
    );
    assert_eq!(store.complete_walks().unwrap(), 0);
}

#[tokio::test]
async fn expiry_journal_survives_restart_before_engine_consumption() {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<expiry-replay@x>", "Gone", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_with(mailbox.clone());
    let db = store.database();
    sync_n(&provider, db.as_ref(), 1).await;
    let thread = store.thread_ids_in_mailbox("INBOX").unwrap().pop().unwrap();
    mailbox.lock().unwrap().inbox().messages.clear();
    sync_n(&provider, db.as_ref(), 1).await; // engine first observes emptiness
    let cursor = SyncCursor::new(db.cursor("me@example.com").unwrap().unwrap());
    // Commit expiry and its acknowledgment without letting the engine
    // consume the journal or advance its cursor.
    let mut pending = provider.poll(&cursor).await.unwrap();
    for _ in 0..policy::EMPTIED_THREAD_GRACE_WALKS {
        pending = provider.poll(&pending.cursor).await.unwrap();
    }
    assert!(store
        .journal_since(cursor.generation().unwrap())
        .unwrap()
        .contains(&thread));
    assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
    assert!(matches!(
        provider.fetch_thread(&thread).await,
        Err(ProviderError::NotFound)
    ));
    let generation = store.generation().unwrap();
    let provider = ImapProvider {
        cache: BodyCache::new(store.clone()),
        thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        ..provider
    };
    sync_n(&provider, db.as_ref(), 1).await;
    assert!(db.list_all_mail(Some("me@example.com")).unwrap().is_empty());
    sync_n(&provider, db.as_ref(), 2).await;
    assert_eq!(store.generation().unwrap(), generation);
}

// Compiles always; runs only when THREESTRANDS_IMAP_IT=1 AND the Dovecot
// container's fingerprint is in DOVECOT_TEST_FP. Skips cleanly otherwise,
// exactly like the Slice 4 live tests in fetch.rs. The orchestrator runs it
// with docker up.
mod live;

// ======================================================================
// Phase 5 run 3: acquire-by-UID + search pins.
// ======================================================================
//
// These drive `acquire_uids` (the explicit-UID ingest run 4 will call) and
// assert that a search-acquired OLD hit survives the delta's window
// eviction and the emptied-thread grace, is removed when the server
// expunges it or its UIDVALIDITY resets, pins idempotently, rolls back a
// mid-acquire failure, and honours the cap and TTL. The provider is built
// with a TINY `folder_window_override` so an acquired old UID is genuinely
// out of window and WOULD be evicted without a pin.
mod search_pins;

// =======================================================================
// Phase 5 run 4: MailSync::search execution (render -> EXAMINE/SEARCH ->
// acquire -> local filter). These drive a REAL ImapProvider over a REAL
// AsyncImapSession against the scripted fake server. The fake server's
// UID SEARCH returns EVERY UID in the EXAMINEd folder (it does not evaluate
// criteria — the gated live Dovecot test proves real criteria matching), so
// these tests exercise the DRIVER: budget split, newest-first, dedup, the
// deadline/budget partial paths, the unsynced skip, the no-network Skipped/
// NoServerMatches paths, the has:attachment filter, and the UIDVALIDITY
// guard. No network, no openssl / /dev/tcp.
// =======================================================================
mod search_exec;
