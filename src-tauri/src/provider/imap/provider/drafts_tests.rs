//! Server drafts against a scripted IMAP server holding a Drafts mailbox.
use super::*;
use async_trait::async_trait;

use crate::db::Database;
use crate::provider::imap::fetch::BodyCache;
use crate::provider::imap::policy::SyncLimits;
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::settings::{ImapAccountSettings, LabelStorage};
use crate::provider::imap::ImapStateStore;
use crate::provider::{MailSend, ServerDraft};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};

#[derive(Clone)]
struct Stored {
    uid: u32,
    flags: Vec<String>,
    raw: Vec<u8>,
}

/// The scripted server: one `Drafts` mailbox.
struct DraftsServer {
    uidplus: bool,
    permanent_flags: Vec<String>,
    uidvalidity: u32,
    next_uid: u32,
    messages: Vec<Stored>,
    commands: Vec<String>,
    /// The next APPEND stores the message but drops the connection before
    /// the tagged OK.
    drop_after_append: bool,
}

impl DraftsServer {
    fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            uidplus: true,
            permanent_flags: vec!["\\Seen".into(), "\\Draft".into(), "\\Deleted".into()],
            uidvalidity: 42,
            next_uid: 1,
            messages: Vec::new(),
            commands: Vec::new(),
            drop_after_append: false,
        }))
    }

    fn live(&self) -> Vec<&Stored> {
        self.messages.iter().filter(|m| !m.flags.iter().any(|f| f == "\\Deleted")).collect()
    }

    fn count(&self, prefix: &str) -> usize {
        self.commands.iter().filter(|c| c.to_ascii_uppercase().starts_with(prefix)).count()
    }
}

fn message_id_header(raw: &[u8]) -> Option<String> {
    String::from_utf8_lossy(raw).split("\r\n").find_map(|line| {
        let (name, _) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("message-id").then(|| line.to_string())
    })
}

fn uids_in(set: &str) -> Vec<u32> {
    set.split(',')
        .flat_map(|part| match part.split_once(':') {
            Some((a, b)) => (a.parse().unwrap_or(0)..=b.parse().unwrap_or(0)).collect::<Vec<_>>(),
            None => part.parse().ok().into_iter().collect(),
        })
        .collect()
}

fn respond(state: &mut DraftsServer, tag: &str, rest: &str) -> String {
    let upper = rest.to_ascii_uppercase();
    if upper.starts_with("CAPABILITY") {
        let caps = if state.uidplus { "IMAP4REV1 UIDPLUS" } else { "IMAP4REV1" };
        format!("* CAPABILITY {caps}\r\n{tag} OK done\r\n")
    } else if upper.starts_with("SELECT") || upper.starts_with("EXAMINE") {
        if !rest.contains("Drafts") {
            return format!("{tag} NO [NONEXISTENT] no mailbox\r\n");
        }
        format!(
            "* {} EXISTS\r\n* OK [PERMANENTFLAGS ({})] ok\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT {}] ok\r\n{tag} OK done\r\n",
            state.messages.len(),
            state.permanent_flags.join(" "),
            state.uidvalidity,
            state.next_uid
        )
    } else if upper.starts_with("UID SEARCH") {
        let live = state.live();
        let hits: Vec<u32> = if let Some(start) = rest.find('"') {
            let needle = &rest[start + 1..rest.rfind('"').unwrap()];
            live.iter().filter(|m| message_id_header(&m.raw).is_some_and(|h| h.contains(needle))).map(|m| m.uid).collect()
        } else if let Some(set) = upper.strip_prefix("UID SEARCH UID ") {
            let wanted = uids_in(set.split_whitespace().next().unwrap_or(""));
            live.iter().filter(|m| wanted.contains(&m.uid)).map(|m| m.uid).collect()
        } else {
            live.iter().map(|m| m.uid).collect()
        };
        let list = hits.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
        format!("* SEARCH {list}\r\n{tag} OK done\r\n")
    } else if upper.starts_with("UID FETCH") {
        let set = rest.split_whitespace().nth(2).unwrap_or("");
        let wanted = uids_in(set);
        let mut out = String::new();
        for (seq, message) in state.messages.iter().enumerate().filter(|(_, m)| wanted.contains(&m.uid)) {
            let (section, bytes) = if upper.contains("HEADER.FIELDS") {
                let header = message_id_header(&message.raw).map(|h| format!("{h}\r\n\r\n")).unwrap_or_else(|| "\r\n".into());
                ("BODY[HEADER.FIELDS (MESSAGE-ID)]", header.into_bytes())
            } else {
                ("BODY[]", message.raw.clone())
            };
            out.push_str(&format!("* {} FETCH (UID {} {section} {{{}}}\r\n", seq + 1, message.uid, bytes.len()));
            out.push_str(&String::from_utf8_lossy(&bytes));
            out.push_str(")\r\n");
        }
        format!("{out}{tag} OK done\r\n")
    } else if upper.starts_with("UID STORE") {
        let set = rest.split_whitespace().nth(2).unwrap_or("");
        let wanted = uids_in(set);
        for message in state.messages.iter_mut().filter(|m| wanted.contains(&m.uid)) {
            if !message.flags.iter().any(|f| f == "\\Deleted") {
                message.flags.push("\\Deleted".into());
            }
        }
        format!("{tag} OK done\r\n")
    } else if upper.starts_with("UID EXPUNGE") {
        let wanted = uids_in(rest.split_whitespace().nth(2).unwrap_or(""));
        state.messages.retain(|m| !(wanted.contains(&m.uid) && m.flags.iter().any(|f| f == "\\Deleted")));
        format!("{tag} OK done\r\n")
    } else if upper.starts_with("LOGOUT") {
        format!("* BYE\r\n{tag} OK done\r\n")
    } else {
        format!("{tag} OK done\r\n")
    }
}

fn spawn(state: Arc<Mutex<DraftsServer>>) -> DuplexStream {
    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let mut server = BufReader::new(server);
        server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
        loop {
            let mut line = String::new();
            if server.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end().to_string();
            let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
            let (tag, rest) = (tag.to_string(), rest.to_string());
            let upper = rest.to_ascii_uppercase();
            if !upper.starts_with("LOGIN") {
                state.lock().unwrap().commands.push(rest.clone());
            }
            if upper.starts_with("LOGIN") {
                server.get_mut().write_all(format!("{tag} OK logged in\r\n").as_bytes()).await.unwrap();
                continue;
            }
            if upper.starts_with("APPEND") {
                let len: usize = rest[rest.rfind('{').unwrap() + 1..rest.rfind('}').unwrap()].parse().unwrap();
                server.get_mut().write_all(b"+ go\r\n").await.unwrap();
                let mut buf = vec![0u8; len + 2];
                if server.read_exact(&mut buf).await.is_err() {
                    break;
                }
                buf.truncate(len);
                let flags = rest[rest.find('(').unwrap() + 1..rest.find(')').unwrap()]
                    .split_whitespace()
                    .map(str::to_string)
                    .collect();
                let drop = {
                    let mut state = state.lock().unwrap();
                    let uid = state.next_uid;
                    state.next_uid += 1;
                    state.messages.push(Stored { uid, flags, raw: buf });
                    std::mem::take(&mut state.drop_after_append)
                };
                if drop {
                    break;
                }
                server.get_mut().write_all(format!("{tag} OK appended\r\n").as_bytes()).await.unwrap();
                continue;
            }
            let response = respond(&mut state.lock().unwrap(), &tag, &rest);
            server.get_mut().write_all(response.as_bytes()).await.unwrap();
            if upper.starts_with("LOGOUT") {
                break;
            }
        }
    });
    client
}

struct Source(Arc<Mutex<DraftsServer>>);

#[async_trait]
impl ImapSessionSource for Source {
    async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
        let session = async_imap::Client::new(spawn(self.0.clone()))
            .login("user", "password")
            .await
            .map_err(|(error, _)| super::super::error::map_imap_error(&error))?;
        Ok(Box::new(super::super::session::AsyncImapSession::new(session)))
    }
}

fn provider(server: Arc<Mutex<DraftsServer>>, with_drafts: bool) -> ImapProvider {
    use super::super::settings::SecurityMode;
    let database = Arc::new(Database::open_memory());
    database.adopt_mail_account("me@example.com", crate::models::MailProviderKind::Imap).unwrap();
    let store = ImapStateStore::new(database, "me@example.com");
    let uidvalidity = server.lock().unwrap().uidvalidity;
    for (name, special) in [("INBOX", None), ("Drafts", Some("\\Drafts"))] {
        if name == "Drafts" && !with_drafts {
            continue;
        }
        store
            .upsert_mailbox(&super::super::ImapMailbox {
                name: name.into(),
                delimiter: Some("/".into()),
                special_use: special.map(str::to_string),
                uidvalidity: uidvalidity as i64,
                uidnext: 1,
                highestmodseq: None,
                permanent_flags_json: None,
                permanent_keywords: None,
            })
            .unwrap();
    }
    ImapProvider {
        cache: BodyCache::new(store.clone()),
        store,
        account_id: "me@example.com".into(),
        smtp_password: "test-password".into(),
        label_model: crate::provider::LabelModel::ImapLabelFolders,
        settings: ImapAccountSettings {
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
        },
        source: Arc::new(Source(server)),
        now: || 1_700_000_000,
        limits: SyncLimits::inbox(),
        sent_round_budget: None,
        folder_window_override: None,
        extra_synced_candidates: None,
        force_user_folders: None,
        folder_rounds_per_poll_override: None,
        search_deadline_override: None,
        thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
    }
}

fn draft(message_id: &str, body: &str) -> Vec<u8> {
    format!("From: me@example.com\r\nTo: jane@example.com\r\nSubject: Plans\r\nMessage-ID: <{message_id}>\r\n\r\n{body}\r\n").into_bytes()
}

const OURS: &str = "draft-8a7c4c4e-3d1f-4a43-9f3e-2b1d7c8e9f00@example.com";

#[tokio::test]
async fn an_account_keeps_server_drafts_only_with_a_drafts_mailbox() {
    assert!(provider(DraftsServer::new(), true).server_drafts());
    let without = provider(DraftsServer::new(), false);
    assert!(!without.server_drafts());
    assert!(matches!(without.create_server_draft(&draft(OURS, "x"), None).await, Err(ProviderError::InvalidOperation(_))));
}

#[tokio::test]
async fn a_new_draft_is_appended_with_draft_and_seen_flags() {
    let server = DraftsServer::new();
    let imap = provider(server.clone(), true);
    let created = imap.create_server_draft(&draft(OURS, "First"), None).await.unwrap();
    assert_eq!(created, ServerDraft { id: "42:1".into(), version: Some("42:1".into()) });
    let state = server.lock().unwrap();
    assert_eq!(state.messages[0].flags, ["\\Draft", "\\Seen"]);
    assert_eq!(state.messages[0].raw, draft(OURS, "First"));
}

#[tokio::test]
async fn a_mailbox_that_cannot_keep_the_draft_flag_still_gets_the_draft() {
    // Proton Mail Bridge's PERMANENTFLAGS has no \Draft.
    let server = DraftsServer::new();
    server.lock().unwrap().permanent_flags = vec!["\\Seen".into(), "\\Deleted".into()];
    let imap = provider(server.clone(), true);
    imap.create_server_draft(&draft(OURS, "First"), None).await.unwrap();
    assert_eq!(server.lock().unwrap().messages[0].flags, ["\\Seen"]);
}

#[tokio::test]
async fn an_update_stores_the_new_copy_before_removing_the_old_one() {
    let server = DraftsServer::new();
    let imap = provider(server.clone(), true);
    let first = imap.create_server_draft(&draft(OURS, "First"), None).await.unwrap();
    let second = imap.update_server_draft(&first.id, &draft(OURS, "Second"), None).await.unwrap();
    assert_eq!(second.id, "42:2");
    let state = server.lock().unwrap();
    assert_eq!(state.live().len(), 1);
    assert!(String::from_utf8_lossy(&state.live()[0].raw).contains("Second"));
    let append = state.commands.iter().rposition(|c| c.starts_with("APPEND")).unwrap();
    let store = state.commands.iter().rposition(|c| c.starts_with("UID STORE")).unwrap();
    assert!(append < store, "append before remove: {:?}", state.commands);
    assert_eq!(state.count("UID EXPUNGE"), 1);
    assert_eq!(state.count("EXPUNGE"), 0, "never a plain EXPUNGE");
}

#[tokio::test]
async fn without_uidplus_old_copies_are_only_marked_deleted() {
    let server = DraftsServer::new();
    server.lock().unwrap().uidplus = false;
    let imap = provider(server.clone(), true);
    let first = imap.create_server_draft(&draft(OURS, "First"), None).await.unwrap();
    imap.update_server_draft(&first.id, &draft(OURS, "Second"), None).await.unwrap();
    let state = server.lock().unwrap();
    assert_eq!(state.messages.len(), 2, "nothing expunged");
    assert_eq!(state.live().len(), 1);
    assert_eq!(state.count("UID EXPUNGE") + state.count("EXPUNGE"), 0);
}

#[tokio::test]
async fn a_save_interrupted_after_storing_is_found_by_message_id_not_appended_twice() {
    let server = DraftsServer::new();
    server.lock().unwrap().drop_after_append = true;
    let imap = provider(server.clone(), true);
    assert!(imap.create_server_draft(&draft(OURS, "First"), None).await.is_err());
    let found = imap.find_server_draft(OURS).await.unwrap();
    assert_eq!(found, Some(ServerDraft { id: "42:1".into(), version: Some("42:1".into()) }));
    assert_eq!(server.lock().unwrap().count("APPEND"), 1);
}

#[tokio::test]
async fn listing_shows_the_newest_copy_and_removes_this_apps_stale_duplicates() {
    let server = DraftsServer::new();
    {
        let mut state = server.lock().unwrap();
        for (uid, raw) in [(1, draft(OURS, "old")), (2, draft("CAF@other.example", "theirs")), (3, draft(OURS, "new")), (4, draft("CAF@other.example", "theirs again"))] {
            state.messages.push(Stored { uid, flags: vec![], raw });
        }
        state.next_uid = 5;
    }
    let imap = provider(server.clone(), true);
    let listed: Vec<String> = imap.list_server_drafts().await.unwrap().into_iter().map(|d| d.id).collect();
    assert_eq!(listed, ["42:3", "42:4"]);
    let state = server.lock().unwrap();
    let live: Vec<u32> = state.live().iter().map(|m| m.uid).collect();
    assert_eq!(live, [2, 3, 4], "only this app's own duplicate is removed");
}

#[tokio::test]
async fn fetching_returns_the_stored_message() {
    let server = DraftsServer::new();
    let imap = provider(server.clone(), true);
    let created = imap.create_server_draft(&draft(OURS, "Body text"), None).await.unwrap();
    let fetched = imap.fetch_server_draft(&created.id).await.unwrap();
    assert_eq!(fetched.raw, draft(OURS, "Body text"));
    assert_eq!(fetched.thread_id, None);
    assert_eq!(fetched.draft, created);
}

#[tokio::test]
async fn a_copy_that_is_gone_or_from_another_uidvalidity_reports_not_found() {
    let server = DraftsServer::new();
    let imap = provider(server.clone(), true);
    let created = imap.create_server_draft(&draft(OURS, "x"), None).await.unwrap();
    assert_eq!(imap.server_draft_version(&created.id).await.unwrap(), Some(created.id.clone()));
    imap.delete_server_draft(&created.id).await.unwrap();
    assert_eq!(imap.server_draft_version(&created.id).await.unwrap(), None);
    assert!(matches!(imap.delete_server_draft(&created.id).await, Err(ProviderError::NotFound)));
    assert!(matches!(imap.fetch_server_draft("41:1").await, Err(ProviderError::NotFound)));
    assert_eq!(imap.server_draft_version("not-an-id").await.unwrap(), None);
}
