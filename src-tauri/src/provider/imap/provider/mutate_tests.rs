use super::*;
use async_trait::async_trait;

use crate::provider::{
    LabelModel, MailMutate, MailSend, MailSync, ProviderError, ProviderResult, SyncCursor,
};

use crate::db::Database;
use crate::provider::imap::fetch::BodyCache;
use crate::provider::imap::policy::SyncLimits;
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::settings::{ImapAccountSettings, LabelStorage};
use crate::provider::imap::{ImapLocation, ImapStateStore, SyncRoundWrite};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// One message on the fake server: its UID, flags and raw bytes.
#[derive(Clone)]
struct MutMessage {
    uid: u32,
    flags: Vec<String>,
    raw: String,
}

/// One mailbox on the fake server.
#[derive(Clone)]
struct MutFolder {
    uidvalidity: u32,
    uidnext: u32,
    permanent_flags: Vec<String>,
    /// SELECT answers `NO` (an unselectable / vanished mailbox).
    select_fails: bool,
    /// SELECT answers a TRANSIENT `NO [INUSE]` (a busy mailbox the engine
    /// retries), used to exercise the partial-failure retry path.
    select_busy: bool,
    /// SELECT answers `OK [READ-ONLY]` (async-imap still treats this as a
    /// successful select).
    read_only: bool,
    messages: Vec<MutMessage>,
}

impl MutFolder {
    fn new(uidvalidity: u32) -> Self {
        Self {
            uidvalidity,
            uidnext: 1,
            permanent_flags: vec![
                "\\Seen".into(),
                "\\Flagged".into(),
                "\\Deleted".into(),
            ],
            select_fails: false,
            select_busy: false,
            read_only: false,
            messages: Vec::new(),
        }
    }
    fn add(&mut self, flags: &[&str], raw: &str) -> u32 {
        let uid = self.uidnext;
        self.uidnext += 1;
        self.messages.push(MutMessage {
            uid,
            flags: flags.iter().map(|f| f.to_string()).collect(),
            raw: raw.to_string(),
        });
        uid
    }
}

/// The shared server state, mutated in place so a STORE on the live
/// connection is visible to a later sweep.
#[derive(Clone, Default)]
struct MutServer {
    capabilities: Vec<String>,
    folders: BTreeMap<String, MutFolder>,
    /// Every write command the server saw (minus LOGIN/LOGOUT), so a test
    /// can assert what did — and did NOT — reach the wire.
    commands: Arc<Mutex<Vec<String>>>,
    /// When set, a STORE that would add/remove this flag is ACKed OK but
    /// NOT recorded, so the verify FETCH omits/keeps it — the silent
    /// ignore the verified store must catch.
    silently_ignore_flag: Option<String>,
    /// When set, `UID COPY <set> <this dest>` answers `NO [TRYCREATE]` the
    /// first time (and the server does NOT copy). Cleared automatically
    /// after it fires once, so a create-then-retry can succeed.
    copy_trycreate_once: Option<String>,
    /// When set, `UID EXPUNGE` fails with `NO <text>` the first time (and
    /// nothing is expunged); the \Deleted store that preceded it DID land.
    /// Cleared after it fires once so a retry can finish the move. Models
    /// the half-applied copy+delete+expunge move.
    fail_expunge_once: Option<String>,
    /// When set, `CREATE <this exact name>` answers `NO [TRYCREATE] <text>`
    /// the first time (and does NOT create), cleared after it fires once —
    /// so `create_label` creates the missing container and retries. Keyed
    /// by the decoded mailbox name.
    create_trycreate_once: Option<String>,
    drop_copy_response_once: bool,
    omit_copyuid: bool,
    ignore_expunge: bool,
}

impl MutServer {
    fn new() -> Self {
        Self {
            capabilities: vec![
                "IMAP4REV1".into(),
                "MOVE".into(),
                "UIDPLUS".into(),
            ],
            folders: BTreeMap::new(),
            commands: Arc::new(Mutex::new(Vec::new())),
            silently_ignore_flag: None,
            copy_trycreate_once: None,
            fail_expunge_once: None,
            create_trycreate_once: None,
            drop_copy_response_once: false,
            omit_copyuid: false,
            ignore_expunge: false,
        }
    }
    fn folder(&mut self, name: &str, uidvalidity: u32) -> &mut MutFolder {
        self.folders
            .entry(name.to_string())
            .or_insert_with(|| MutFolder::new(uidvalidity))
    }
    fn flags_of(&self, mailbox: &str, uid: u32) -> Vec<String> {
        self.folders
            .get(mailbox)
            .and_then(|f| f.messages.iter().find(|m| m.uid == uid))
            .map(|m| m.flags.clone())
            .unwrap_or_default()
    }
    fn wire(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }
    fn count_starting(&self, prefix: &str) -> usize {
        self.wire()
            .iter()
            .filter(|c| c.to_ascii_uppercase().starts_with(prefix))
            .count()
    }
}

/// Spawn the scripted mutation+read server over a duplex stream, sharing
/// the live `Arc<Mutex<MutServer>>` so a STORE is visible to later sweeps.
fn spawn(server_state: Arc<Mutex<MutServer>>) -> DuplexStream {
    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let mut server = BufReader::new(server);
        server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
        let mut selected = String::new();
        loop {
            let mut line = String::new();
            if server.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let line = line.trim_end().to_string();
            let mut parts = line.splitn(2, ' ');
            let tag = parts.next().unwrap_or("").to_string();
            let rest = parts.next().unwrap_or("").to_string();
            let upper = rest.to_ascii_uppercase();

            // Build the whole response under the lock in a NON-async block,
            // drop the guard, then write it — a MutexGuard must never be
            // held across an await (the server task is Send).
            let (response, done) = {
                let mut state = server_state.lock().unwrap();
                if !upper.starts_with("LOGIN") && !upper.starts_with("LOGOUT") {
                    state.commands.lock().unwrap().push(rest.clone());
                }
                let response = build_response(&mut state, &mut selected, &tag, &rest, &upper);
                if upper.starts_with("UID COPY") && state.drop_copy_response_once {
                    state.drop_copy_response_once = false;
                    (String::new(), true)
                } else {
                    response
                }
            };
            reply(&mut server, &response).await;
            if done {
                break;
            }
        }
    });
    client
}

/// Build the server's response to one command (lock held, no await). Returns
/// `(response_text, should_disconnect)`.
fn build_response(
    state: &mut MutServer,
    selected: &mut String,
    tag: &str,
    rest: &str,
    upper: &str,
) -> (String, bool) {
    if upper.starts_with("LOGIN") {
        (format!("{tag} OK logged in\r\n"), false)
    } else if upper.starts_with("CAPABILITY") {
        let caps = state.capabilities.join(" ");
        (format!("* CAPABILITY {caps}\r\n{tag} OK done\r\n"), false)
    } else if upper.starts_with("SELECT") || upper.starts_with("EXAMINE") {
        let is_select = upper.starts_with("SELECT");
        let name = select_name(rest, is_select);
        *selected = name.clone();
        let Some(folder) = state.folders.get(&name) else {
            return (format!("{tag} NO [NONEXISTENT] no mailbox\r\n"), false);
        };
        if folder.select_fails {
            return (format!("{tag} NO [NONEXISTENT] select failed\r\n"), false);
        }
        if folder.select_busy {
            // A TRANSIENT failure the engine retries (RFC 5530 [INUSE]).
            return (format!("{tag} NO [INUSE] mailbox busy\r\n"), false);
        }
        let permanent = folder.permanent_flags.join(" ");
        let rw = if folder.read_only || !is_select {
            "[READ-ONLY]"
        } else {
            "[READ-WRITE]"
        };
        (
            format!(
                "* {} EXISTS\r\n\
                 * OK [PERMANENTFLAGS ({permanent})] limited\r\n\
                 * OK [UIDVALIDITY {}] valid\r\n\
                 * OK [UIDNEXT {}] next\r\n\
                 {tag} OK {rw} done\r\n",
                folder.messages.len(),
                folder.uidvalidity,
                folder.uidnext
            ),
            false,
        )
    } else if upper.starts_with("UID SEARCH") {
        let uids: Vec<String> = state
            .folders
            .get(selected.as_str())
            .map(|f| f.messages.iter().map(|m| m.uid.to_string()).collect())
            .unwrap_or_default();
        (format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" ")), false)
    } else if upper.starts_with("UID STORE") {
        let flags = store_flags(rest);
        let is_add = upper.contains("+FLAGS");
        let set = parse_uid_set(rest);
        let ignore = state.silently_ignore_flag.clone();
        if let Some(folder) = state.folders.get_mut(selected.as_str()) {
            for message in folder.messages.iter_mut() {
                if !set.contains(&message.uid) {
                    continue;
                }
                for flag in &flags {
                    let ignored = ignore
                        .as_deref()
                        .map(|f| f.eq_ignore_ascii_case(flag))
                        .unwrap_or(false);
                    if ignored {
                        continue; // ACK OK but do not record.
                    }
                    if is_add {
                        if !message.flags.iter().any(|f| f.eq_ignore_ascii_case(flag)) {
                            message.flags.push(flag.clone());
                        }
                    } else {
                        message.flags.retain(|f| !f.eq_ignore_ascii_case(flag));
                    }
                }
            }
        }
        (format!("{tag} OK STORE done\r\n"), false)
    } else if upper.starts_with("UID FETCH") {
        let set = parse_uid_set(rest);
        let wants_body = upper.contains("BODY.PEEK[]");
        let is_identity = upper.contains("HEADER.FIELDS");
        let empty = Vec::new();
        let messages = state
            .folders
            .get(selected.as_str())
            .map(|f| &f.messages)
            .unwrap_or(&empty);
        let mut out = String::new();
        for (seq, uid) in set.iter().enumerate() {
            if let Some(message) = messages.iter().find(|m| &m.uid == uid) {
                out.push_str(&fetch_line(seq as u32 + 1, message, wants_body, is_identity));
            }
        }
        out.push_str(&format!("{tag} OK done\r\n"));
        (out, false)
    } else if upper.starts_with("CREATE") {
        // CREATE <mailbox>: create an empty folder (idempotent-ish — a
        // re-create of an existing one answers NO, which the caller never
        // triggers because create is reached only on TRYCREATE).
        let name = rest["CREATE".len().min(rest.len())..]
            .trim()
            .trim_matches('"')
            .to_string();
        // A scripted one-shot TRYCREATE (missing parent/container) fires
        // once for THIS exact name, so create_label can create the
        // container and retry.
        if state.create_trycreate_once.as_deref() == Some(name.as_str()) {
            state.create_trycreate_once = None;
            (format!("{tag} NO [TRYCREATE] missing parent\r\n"), false)
        } else if let std::collections::btree_map::Entry::Vacant(e) = state.folders.entry(name) {
            // A freshly-created destination gets its own UIDVALIDITY.
            e.insert(MutFolder::new(700));
            (format!("{tag} OK CREATE done\r\n"), false)
        } else {
            (format!("{tag} NO [ALREADYEXISTS] exists\r\n"), false)
        }
    } else if upper.starts_with("DELETE") {
        // DELETE <mailbox>: remove the folder. Deleting an absent one
        // answers NO [NONEXISTENT]; the provider never issues that because
        // it checks the local catalog first.
        let name = rest["DELETE".len().min(rest.len())..]
            .trim()
            .trim_matches('"')
            .to_string();
        if state.folders.remove(&name).is_some() {
            (format!("{tag} OK DELETE done\r\n"), false)
        } else {
            (format!("{tag} NO [NONEXISTENT] no such mailbox\r\n"), false)
        }
    } else if upper.starts_with("RENAME") {
        // RENAME <from> <to>: move the folder (and its messages) to a new
        // name. Absent source -> NO; existing target -> NO. The provider
        // never issues RENAME (run 4 refuses rename), so this exists only
        // for completeness / future use.
        let args: Vec<&str> = rest["RENAME".len().min(rest.len())..].trim().split('"').collect();
        // Tokens are "from" "to"; split on quotes yields ["", from, " ", to, ""].
        let from = args.get(1).unwrap_or(&"").to_string();
        let to = args.get(3).unwrap_or(&"").to_string();
        match state.folders.remove(&from) {
            Some(folder) if !state.folders.contains_key(&to) => {
                state.folders.insert(to, folder);
                (format!("{tag} OK RENAME done\r\n"), false)
            }
            Some(folder) => {
                state.folders.insert(from, folder);
                (format!("{tag} NO [ALREADYEXISTS] target exists\r\n"), false)
            }
            None => (format!("{tag} NO [NONEXISTENT] no such mailbox\r\n"), false),
        }
    } else if upper.starts_with("UID COPY") {
        // UID COPY <set> <dest>: copy the named source UIDs (from the
        // selected mailbox) into <dest>, assigning new destination UIDs and
        // returning a tagged COPYUID (UIDPLUS). A scripted TRYCREATE fires
        // once if armed for this destination.
        let set = parse_uid_set(rest);
        let dest = copy_dest_name(rest);
        if let Some(text) = state.copy_trycreate_once.take() {
            return (format!("{tag} NO [TRYCREATE] {text}\r\n"), false);
        }
        if !state.folders.contains_key(&dest) {
            return (format!("{tag} NO [TRYCREATE] no such mailbox\r\n"), false);
        }
        let source_msgs: Vec<MutMessage> = state
            .folders
            .get(selected.as_str())
            .map(|f| f.messages.iter().filter(|m| set.contains(&m.uid)).cloned().collect())
            .unwrap_or_default();
        let dest_uidvalidity = state.folders.get(&dest).map(|f| f.uidvalidity).unwrap_or(0);
        let mut src_uids = Vec::new();
        let mut dst_uids = Vec::new();
        {
            let folder = state.folders.get_mut(&dest).expect("dest exists");
            for msg in source_msgs {
                let new_uid = folder.uidnext;
                folder.uidnext += 1;
                src_uids.push(msg.uid);
                dst_uids.push(new_uid);
                folder.messages.push(MutMessage {
                    uid: new_uid,
                    flags: msg.flags.clone(),
                    raw: msg.raw.clone(),
                });
            }
        }
        let src = src_uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let dst = dst_uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let response = if state.omit_copyuid {
            format!("{tag} OK copied\r\n")
        } else {
            format!("{tag} OK [COPYUID {dest_uidvalidity} {src} {dst}] copied\r\n")
        };
        (response, false)
    } else if upper.starts_with("UID MOVE") {
        // UID MOVE <set> <dest>: atomic server-side move. Copies to dest
        // (new UIDs), removes from source, emits an UNTAGGED COPYUID then
        // EXPUNGE lines, then the tagged OK.
        let set = parse_uid_set(rest);
        let dest = copy_dest_name(rest);
        if !state.folders.contains_key(&dest) {
            return (format!("{tag} NO [TRYCREATE] no such mailbox\r\n"), false);
        }
        let moved: Vec<MutMessage> = state
            .folders
            .get(selected.as_str())
            .map(|f| f.messages.iter().filter(|m| set.contains(&m.uid)).cloned().collect())
            .unwrap_or_default();
        let dest_uidvalidity = state.folders.get(&dest).map(|f| f.uidvalidity).unwrap_or(0);
        let mut src_uids = Vec::new();
        let mut dst_uids = Vec::new();
        {
            let folder = state.folders.get_mut(&dest).expect("dest exists");
            for msg in &moved {
                let new_uid = folder.uidnext;
                folder.uidnext += 1;
                src_uids.push(msg.uid);
                dst_uids.push(new_uid);
                folder.messages.push(MutMessage {
                    uid: new_uid,
                    flags: msg.flags.clone(),
                    raw: msg.raw.clone(),
                });
            }
        }
        // Remove the moved UIDs from the source.
        if let Some(folder) = state.folders.get_mut(selected.as_str()) {
            folder.messages.retain(|m| !set.contains(&m.uid));
        }
        let src = src_uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let dst = dst_uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let expunges: String = set.iter().map(|_| "* 1 EXPUNGE\r\n".to_string()).collect();
        (
            format!(
                "* OK [COPYUID {dest_uidvalidity} {src} {dst}] moved\r\n{expunges}{tag} OK MOVE done\r\n"
            ),
            false,
        )
    } else if upper.starts_with("UID EXPUNGE") {
        // UID EXPUNGE <set>: remove the named UIDs from the selected
        // mailbox (they must already be \Deleted — the client stores that
        // first). A scripted one-shot failure models the half-applied move.
        if let Some(text) = state.fail_expunge_once.take() {
            return (format!("{tag} NO {text}\r\n"), false);
        }
        if state.ignore_expunge {
            return (format!("{tag} OK EXPUNGE done\r\n"), false);
        }
        let set = parse_uid_set(rest);
        if let Some(folder) = state.folders.get_mut(selected.as_str()) {
            folder
                .messages
                .retain(|m| !(set.contains(&m.uid) && m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Deleted"))));
        }
        (format!("{tag} OK EXPUNGE done\r\n"), false)
    } else if upper.starts_with("LOGOUT") {
        (format!("* BYE\r\n{tag} OK logout\r\n"), true)
    } else {
        (format!("{tag} OK done\r\n"), false)
    }
}

async fn reply(server: &mut BufReader<DuplexStream>, text: &str) {
    server.get_mut().write_all(text.as_bytes()).await.unwrap();
}

/// The mailbox name after `SELECT`/`EXAMINE`, unquoted.
fn select_name(rest: &str, is_select: bool) -> String {
    let verb_len = if is_select { "SELECT".len() } else { "EXAMINE".len() };
    rest[verb_len.min(rest.len())..].trim().trim_matches('"').to_string()
}

/// The parenthesised flag list of a STORE command.
fn store_flags(cmd: &str) -> Vec<String> {
    if let (Some(open), Some(close)) = (cmd.find('('), cmd.rfind(')')) {
        cmd[open + 1..close].split_whitespace().map(str::to_string).collect()
    } else {
        Vec::new()
    }
}

/// The destination mailbox name of a `UID COPY <set> <dest>` /
/// `UID MOVE <set> <dest>` command, unquoted. The destination is token 4
/// (`UID`, `COPY`/`MOVE`, `<set>`, `<dest>`).
fn copy_dest_name(cmd: &str) -> String {
    cmd.split_whitespace()
        .nth(3)
        .unwrap_or("")
        .trim_matches('"')
        .to_string()
}

/// The UID set token after `UID FETCH`/`UID STORE` (comma + range forms).
fn parse_uid_set(cmd: &str) -> Vec<u32> {
    // "UID FETCH <set> ..." / "UID STORE <set> ...": the set is token 3.
    let token = cmd.split_whitespace().nth(2).unwrap_or("");
    let mut uids = Vec::new();
    for part in token.split(',') {
        if let Some((lo, hi)) = part.split_once(':') {
            if let (Ok(lo), Ok(hi)) = (lo.parse::<u32>(), hi.parse::<u32>()) {
                uids.extend(lo..=hi);
            }
        } else if let Ok(uid) = part.parse::<u32>() {
            uids.push(uid);
        }
    }
    uids
}

fn fetch_line(seq: u32, message: &MutMessage, wants_body: bool, is_identity: bool) -> String {
    let flags = message.flags.join(" ");
    if wants_body {
        format!(
            "* {seq} FETCH (UID {} BODY[]<0> {{{}}}\r\n{})\r\n",
            message.uid,
            message.raw.len(),
            message.raw
        )
    } else if is_identity {
        let header_block = message
            .raw
            .split_once("\r\n\r\n")
            .map(|(h, _)| format!("{h}\r\n\r\n"))
            .unwrap_or_else(|| message.raw.clone());
        format!(
            "* {seq} FETCH (UID {} FLAGS ({flags}) RFC822.SIZE {} BODY[HEADER.FIELDS (MESSAGE-ID DATE FROM SUBJECT)] {{{}}}\r\n{})\r\n",
            message.uid,
            message.raw.len(),
            header_block.len(),
            header_block
        )
    } else {
        // FLAGS-only (verify + sweep).
        format!("* {seq} FETCH (UID {} FLAGS ({flags}))\r\n", message.uid)
    }
}

/// A session source that logs into a fresh connection against the SHARED
/// server state each time it is opened.
struct MutSource {
    server: Arc<Mutex<MutServer>>,
}

#[async_trait]
impl ImapSessionSource for MutSource {
    async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
        let client = spawn(self.server.clone());
        let session = async_imap::Client::new(client)
            .login("user", "password")
            .await
            .map_err(|(error, _)| super::super::error::map_imap_error(&error))?;
        Ok(Box::new(super::super::session::AsyncImapSession::new(session)))
    }
}

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

fn infer_special_use(name: &str) -> Option<String> {
    match name {
        "Trash" => Some("\\Trash".into()),
        "Junk" | "Spam" => Some("\\Junk".into()),
        "Sent" => Some("\\Sent".into()),
        "Archive" => Some("\\Archive".into()),
        _ => None,
    }
}

/// Build a provider over the shared server, seeding a catalog row for every
/// folder present (so SELECT's UIDVALIDITY guard matches) and returning
/// the provider + store.
fn provider_with(server: Arc<Mutex<MutServer>>) -> (ImapProvider, ImapStateStore) {
    let database = Arc::new(Database::open_memory());
    database
        .adopt_mail_account("me@example.com", crate::models::MailProviderKind::Imap)
        .unwrap();
    let store = ImapStateStore::new(database.clone(), "me@example.com");
    for (name, folder) in &server.lock().unwrap().folders {
        store
            .upsert_mailbox(&super::super::ImapMailbox {
                name: name.clone(),
                delimiter: Some("/".into()),
                special_use: infer_special_use(name),
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
        account_id: "me@example.com".into(),
        smtp_password: "test-password".into(),
        label_model: LabelModel::ImapLabelFolders,
        settings: test_settings(),
        source: Arc::new(MutSource { server }),
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

fn message(message_id: &str) -> String {
    format!(
        "Message-ID: {message_id}\r\nDate: Mon, 06 Oct 2025 09:00:00 +0000\r\nFrom: a@b.com\r\nSubject: S\r\n\r\nbody",
    )
}

/// Seed one message as a location + thread + a cached body, so modify_* can
/// resolve it. Returns its stable message id. `mailbox`'s UIDVALIDITY must
/// match the server's.
fn seed(
    store: &ImapStateStore,
    provider: &ImapProvider,
    mailbox: &str,
    uidvalidity: i64,
    uid: i64,
    msgid: &str,
    flags: &[&str],
) -> String {
    let message_id = format!("imap:me@example.com:{msgid}");
    store
        .upsert_location(&ImapLocation {
            mailbox: mailbox.into(),
            uidvalidity,
            uid,
            message_id: message_id.clone(),
            flags_json: serde_json::to_string(
                &flags.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
            )
            .unwrap(),
            modseq: None,
        })
        .unwrap();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![(message_id.clone(), format!("imap:t:{msgid}"))],
            changed_threads: vec![format!("imap:t:{msgid}")],
            ..Default::default()
        })
        .unwrap();
    provider
        .cache
        .put(&message_id, message(&format!("<{msgid}@x>")).as_bytes(), 1)
        .unwrap();
    message_id
}

/// The stored flags for a location, as the local store holds them.
fn local_flags(store: &ImapStateStore, mailbox: &str, uid: i64) -> Vec<String> {
    let location = store
        .locations_in_mailbox(mailbox)
        .unwrap()
        .into_iter()
        .find(|l| l.uid == uid)
        .expect("location present");
    serde_json::from_str(&location.flags_json).unwrap()
}

fn server_with_inbox() -> Arc<Mutex<MutServer>> {
    let mut s = MutServer::new();
    s.folder("INBOX", 100);
    Arc::new(Mutex::new(s))
}

// --- mark read / unread ------------------------------------------------

#[tokio::test]
async fn mark_read_adds_seen_and_updates_local_flags() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&[], &message("<m1@x>")); // unread (no \Seen)
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &[]);

    // remove UNREAD == mark read == add \Seen.
    provider
        .modify_messages(&[id], &[], &["UNREAD".into()])
        .await
        .expect("mark read");

    // The server now holds \Seen, and the local flags agree.
    assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));
    assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));
}

#[tokio::test]
async fn mark_unread_removes_seen() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

    // add UNREAD == mark unread == remove \Seen.
    provider
        .modify_messages(&[id], &["UNREAD".into()], &[])
        .await
        .expect("mark unread");

    assert!(!server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));
    assert!(!local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));
}

// --- star / unstar -----------------------------------------------------

#[tokio::test]
async fn star_adds_flagged_and_unstar_removes_it() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

    provider.modify_messages(std::slice::from_ref(&id), &["STARRED".into()], &[]).await.expect("star");
    assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
    assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Flagged"));

    provider.modify_messages(&[id], &[], &["STARRED".into()]).await.expect("unstar");
    assert!(!server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
    assert!(!local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Flagged"));
}

// --- thread-level mark-read across messages and two mailboxes ----------

#[tokio::test]
async fn thread_mark_read_touches_every_message_in_every_mailbox() {
    // Two messages of one thread; one lives in INBOX, the other has copies
    // in INBOX and Archive (two locations, both must agree).
    let mut s = MutServer::new();
    s.folder("INBOX", 100).add(&[], &message("<a@x>")); // uid 1
    s.folder("INBOX", 100).add(&[], &message("<b@x>")); // uid 2
    s.folder("Archive", 95).add(&[], &message("<b@x>")); // uid 1 (copy of b)
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());

    // Seed both messages into ONE thread.
    store
        .upsert_location(&ImapLocation {
            mailbox: "INBOX".into(), uidvalidity: 100, uid: 1,
            message_id: "imap:me@example.com:a".into(), flags_json: "[]".into(), modseq: None,
        }).unwrap();
    store
        .upsert_location(&ImapLocation {
            mailbox: "INBOX".into(), uidvalidity: 100, uid: 2,
            message_id: "imap:me@example.com:b".into(), flags_json: "[]".into(), modseq: None,
        }).unwrap();
    store
        .upsert_location(&ImapLocation {
            mailbox: "Archive".into(), uidvalidity: 95, uid: 1,
            message_id: "imap:me@example.com:b".into(), flags_json: "[]".into(), modseq: None,
        }).unwrap();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![
                ("imap:me@example.com:a".into(), "imap:t:thread".into()),
                ("imap:me@example.com:b".into(), "imap:t:thread".into()),
            ],
            changed_threads: vec!["imap:t:thread".into()],
            ..Default::default()
        }).unwrap();

    provider
        .modify_thread("imap:t:thread", &[], &["UNREAD".into()])
        .await
        .expect("thread mark read");

    // Every copy, in BOTH mailboxes, now has \Seen on the server and locally.
    let g = server.lock().unwrap();
    assert!(g.flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));
    assert!(g.flags_of("INBOX", 2).iter().any(|f| f == "\\Seen"));
    assert!(g.flags_of("Archive", 1).iter().any(|f| f == "\\Seen"));
    drop(g);
    assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));
    assert!(local_flags(&store, "INBOX", 2).iter().any(|f| f == "\\Seen"));
    assert!(local_flags(&store, "Archive", 1).iter().any(|f| f == "\\Seen"));
}

// --- alias thread id ---------------------------------------------------

#[tokio::test]
async fn modify_thread_resolves_an_alias_to_the_survivor() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    store
        .upsert_location(&ImapLocation {
            mailbox: "INBOX".into(), uidvalidity: 100, uid: 1,
            message_id: "imap:me@example.com:m1".into(), flags_json: r#"["\\Seen"]"#.into(),
            modseq: None,
        }).unwrap();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:survivor".into())],
            aliases: vec![("imap:t:old".into(), "imap:t:survivor".into())],
            changed_threads: vec!["imap:t:survivor".into()],
            ..Default::default()
        }).unwrap();

    // Star via the OLD (aliased) id: it resolves to the survivor's message.
    provider.modify_thread("imap:t:old", &["STARRED".into()], &[]).await.expect("star via alias");
    assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
}

// --- a thread keeps rows for messages expunged elsewhere ---------------

#[tokio::test]
async fn modify_thread_skips_messages_with_no_remaining_location() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let live = "imap:me@example.com:m1".to_string();
    store
        .upsert_location(&ImapLocation {
            mailbox: "INBOX".into(), uidvalidity: 100, uid: 1,
            message_id: live.clone(), flags_json: "[]".into(), modseq: None,
        })
        .unwrap();
    // m2 is in the same thread but was expunged on the server: its
    // imap_threads row is permanent, its locations are gone.
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![
                (live.clone(), "imap:t:shared".into()),
                ("imap:me@example.com:m2".into(), "imap:t:shared".into()),
            ],
            changed_threads: vec!["imap:t:shared".into()],
            ..Default::default()
        })
        .unwrap();

    provider
        .modify_thread("imap:t:shared", &[], &["UNREAD".into()])
        .await
        .expect("an expunged sibling must not fail a thread-level mark-read");
    assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));

    // A thread whose messages ALL lost their locations is still refused.
    let err = provider
        .modify_thread("imap:t:gone", &[], &["UNREAD".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
}

// --- idempotent no-op sends nothing ------------------------------------

#[tokio::test]
async fn an_already_satisfied_request_sends_no_write_command() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

    // Mark-read on an already-read message: Ok(()) and ZERO commands sent
    // (not even a SELECT), because the local flags already satisfy it.
    provider.modify_messages(&[id], &[], &["UNREAD".into()]).await.expect("no-op ok");
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0, "an idempotent no-op selects nothing");
    assert_eq!(g.count_starting("UID STORE"), 0, "and sends no STORE");
}

// --- stale UIDVALIDITY sends nothing and is transient ------------------

#[tokio::test]
async fn a_stale_uidvalidity_sends_no_write_and_is_transient() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        // The server's INBOX is at UIDVALIDITY 200, but the stored location
        // says 100 -> the select_for_write guard refuses.
        g.folder("INBOX", 100).uidvalidity = 200;
        g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &[]); // stored UIDVALIDITY 100

    let err = provider
        .modify_messages(&[id], &[], &["UNREAD".into()])
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::TransientTransport(ref m) if m.contains("UIDVALIDITY")),
        "a stale mailbox is a named transient error: {err:?}"
    );
    assert!(err.retry_mutation(), "transient so the engine retries");
    // The guard SELECTed to learn the UIDVALIDITY but issued NO STORE.
    assert_eq!(server.lock().unwrap().count_starting("UID STORE"), 0);
}

// --- flag missing from PERMANENTFLAGS is a permanent error naming it ----

#[tokio::test]
async fn a_flag_outside_permanentflags_is_a_permanent_error_naming_it() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        // PERMANENTFLAGS without \Flagged and no \* -> \Flagged unstorable.
        g.folder("INBOX", 100).permanent_flags = vec!["\\Seen".into(), "\\Deleted".into()];
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

    let err = provider
        .modify_messages(&[id], &["STARRED".into()], &[])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => {
            assert!(m.contains("Flagged"), "names the flag: {m}");
            assert!(m.contains("INBOX"), "names the mailbox: {m}");
        }
        other => panic!("expected a permanent InvalidOperation, got {other:?}"),
    }
    // No \Flagged was stored.
    assert!(!server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
}

// --- a silently-ignored STORE is caught --------------------------------

#[tokio::test]
async fn a_silently_ignored_store_is_caught() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.silently_ignore_flag = Some("\\Flagged".into());
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

    let err = provider
        .modify_messages(&[id], &["STARRED".into()], &[])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("silently ignored"), "{m}"),
        other => panic!("expected InvalidOperation, got {other:?}"),
    }
}

// --- unsupported / mixed request rejected with ZERO commands -----------

#[tokio::test]
async fn an_unsupported_or_mixed_request_is_rejected_before_any_command() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

    // STARRED + INBOX (an unsupported location id) is rejected permanently.
    let err = provider
        .modify_messages(std::slice::from_ref(&id), &["STARRED".into(), "INBOX".into()], &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

    // A label in BOTH lists is contradictory and rejected.
    let err = provider
        .modify_messages(&[id], &["STARRED".into()], &["STARRED".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

    // Not one command reached the wire for either rejected request.
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0, "no SELECT for a rejected request");
    assert_eq!(g.count_starting("UID STORE"), 0, "no STORE for a rejected request");
}

// --- a message with no location at all is a permanent error naming it ---

#[tokio::test]
async fn a_message_with_no_location_is_a_permanent_error_naming_it() {
    let server = server_with_inbox();
    let (provider, store) = provider_with(server.clone());
    // A message id with a thread row but NO location (emptied / expunged).
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:gone".into(), "imap:t:gone".into())],
            changed_threads: vec!["imap:t:gone".into()],
            ..Default::default()
        }).unwrap();

    let err = provider
        .modify_messages(&["imap:me@example.com:gone".into()], &["STARRED".into()], &[])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("gone"), "names the message: {m}"),
        other => panic!("expected a permanent InvalidOperation, got {other:?}"),
    }
    assert_eq!(server.lock().unwrap().count_starting("UID STORE"), 0);
}

// --- partial failure across two mailboxes, then retry touches only B ----

#[tokio::test]
async fn a_partial_failure_persists_the_succeeded_mailbox_and_a_retry_touches_only_the_failed() {
    // One message with copies in INBOX (processed first — mailboxes are
    // visited in name order — and succeeds) and Trash (SELECT transiently
    // fails first time). The INBOX copy's local flags are updated; the
    // mutation returns Trash's error; a retry re-derives only Trash.
    let mut s = MutServer::new();
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    s.folder("Trash", 95).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    s.folder("Trash", 95).select_busy = true;
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());

    for (mailbox, uidvalidity, uid) in [("INBOX", 100, 1), ("Trash", 95, 1)] {
        store
            .upsert_location(&ImapLocation {
                mailbox: mailbox.into(), uidvalidity, uid,
                message_id: "imap:me@example.com:m1".into(),
                flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            }).unwrap();
    }
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:m1".into())],
            changed_threads: vec!["imap:t:m1".into()],
            ..Default::default()
        }).unwrap();

    // Star: INBOX (visited first) succeeds and persists; Trash SELECT
    // transiently fails and the mutation returns that error.
    let err = provider
        .modify_messages(&["imap:me@example.com:m1".into()], &["STARRED".into()], &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::TransientTransport(_)), "Trash select busy: {err:?}");
    assert!(err.retry_mutation(), "transient so the engine retries");

    // INBOX's copy IS starred (server + local); Trash's is NOT yet.
    assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
    assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Flagged"));
    assert!(!server.lock().unwrap().flags_of("Trash", 1).iter().any(|f| f == "\\Flagged"));
    assert!(!local_flags(&store, "Trash", 1).iter().any(|f| f == "\\Flagged"));

    // Heal Trash and retry. INBOX is already satisfied (local flags say
    // starred), so the retry touches ONLY Trash: exactly one SELECT and
    // one STORE this time, both against Trash.
    server.lock().unwrap().commands.lock().unwrap().clear();
    server.lock().unwrap().folder("Trash", 95).select_busy = false;
    provider
        .modify_messages(&["imap:me@example.com:m1".into()], &["STARRED".into()], &[])
        .await
        .expect("retry succeeds");
    assert!(server.lock().unwrap().flags_of("Trash", 1).iter().any(|f| f == "\\Flagged"));
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID STORE"), 1, "retry stores once (Trash only)");
    // The single SELECT was Trash, never INBOX (INBOX was already done).
    assert!(g.wire().iter().any(|c| c.contains("Trash")));
    assert!(!g.wire().iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"INBOX\"")));
}

// --- local flags_json updated; a following sync sweep reports no change -

#[tokio::test]
async fn after_a_mutation_a_following_sweep_sees_no_change() {
    // Mark a message read, then run a full INBOX sweep against the SAME
    // server (which now holds \Seen). The sweep must find the local flags
    // already agree and NOT bump the generation (no flap, no journal).
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());

    // A baseline sweep ingests m1 so the local store knows its id/thread.
    provider.baseline_cursor().await.unwrap();
    let id = store.locations_in_mailbox("INBOX").unwrap()[0].message_id.clone();
    let gen_before = store.generation().unwrap();

    // Mark it read through the mutation path.
    provider.modify_messages(&[id], &[], &["UNREAD".into()]).await.expect("mark read");
    let gen_after_mutation = store.generation().unwrap();
    assert_eq!(
        gen_after_mutation, gen_before,
        "the mutation itself does not bump the generation"
    );
    assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));

    // A following sweep over the same (now \Seen) server state sees the
    // local flags already agree: no content change, no generation bump.
    let cursor = SyncCursor::from_generation(gen_after_mutation);
    let batch = provider.poll(&cursor).await.unwrap();
    assert_eq!(
        store.generation().unwrap(),
        gen_after_mutation,
        "a following sweep sees no change after the mutation reconciled local flags"
    );
    assert!(batch.changed_threads.is_empty(), "and journals nothing");
}

// =======================================================================
// Phase 3 run 3: system-label MOVES (archive / trash / spam + reverses).
// =======================================================================

/// Seed one message as a location + thread + cached body in `mailbox`,
/// WITHOUT advancing a shared uid counter (callers choose the uid). Returns
/// the stable message id. Unlike `seed`, this lets several copies of one
/// message id live in different mailboxes.
#[allow(clippy::too_many_arguments)]
fn seed_loc(
    store: &ImapStateStore,
    provider: &ImapProvider,
    msgid: &str,
    thread: &str,
    mailbox: &str,
    uidvalidity: i64,
    uid: i64,
    flags: &[&str],
) -> String {
    let message_id = format!("imap:me@example.com:{msgid}");
    store
        .upsert_location(&ImapLocation {
            mailbox: mailbox.into(),
            uidvalidity,
            uid,
            message_id: message_id.clone(),
            flags_json: serde_json::to_string(
                &flags.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
            )
            .unwrap(),
            modseq: None,
        })
        .unwrap();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![(message_id.clone(), thread.to_string())],
            changed_threads: vec![thread.to_string()],
            ..Default::default()
        })
        .unwrap();
    provider
        .cache
        .put(&message_id, message(&format!("<{msgid}@x>")).as_bytes(), 1)
        .unwrap();
    message_id
}

/// A server with INBOX + Archive + Trash + Junk, each with its own
/// UIDVALIDITY, MOVE + UIDPLUS advertised (the default).
fn server_with_system_mailboxes() -> Arc<Mutex<MutServer>> {
    let mut s = MutServer::new();
    s.folder("INBOX", 100);
    s.folder("Archive", 200);
    s.folder("Trash", 300);
    s.folder("Junk", 400);
    Arc::new(Mutex::new(s))
}

fn uids_in(server: &Arc<Mutex<MutServer>>, mailbox: &str) -> Vec<u32> {
    server
        .lock()
        .unwrap()
        .folders
        .get(mailbox)
        .map(|f| f.messages.iter().map(|m| m.uid).collect())
        .unwrap_or_default()
}

fn local_mailboxes_of(store: &ImapStateStore, message_id: &str) -> Vec<String> {
    let mut v: Vec<String> = store
        .locations_for_message(message_id)
        .unwrap()
        .into_iter()
        .map(|l| l.mailbox)
        .collect();
    v.sort();
    v
}

// --- archive: MOVE the INBOX copy to Archive ---------------------------

#[tokio::test]
async fn archive_moves_the_inbox_copy_to_archive_via_move() {
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    // archive == add [], remove [INBOX].
    provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .expect("archive");

    // The server moved it out of INBOX into Archive (one copy).
    assert!(uids_in(&server, "INBOX").is_empty(), "source emptied");
    assert_eq!(uids_in(&server, "Archive").len(), 1, "one copy in Archive");
    // MOVE was used (atomic), not COPY.
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID MOVE"), 1);
    assert_eq!(g.count_starting("UID COPY"), 0);
    drop(g);
    // Local state: the message now lives only in Archive.
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
}

// --- unarchive: Archive -> INBOX ---------------------------------------

#[tokio::test]
async fn unarchive_moves_the_archive_copy_to_inbox() {
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("Archive", 200).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Archive", 200, 1, &["\\Seen"]);

    // unarchive == add [INBOX], remove [].
    provider
        .modify_thread("imap:t:m1", &["INBOX".into()], &[])
        .await
        .expect("unarchive");

    assert!(uids_in(&server, "Archive").is_empty());
    assert_eq!(uids_in(&server, "INBOX").len(), 1);
    assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
}

// --- trash / untrash ---------------------------------------------------

#[tokio::test]
async fn trash_then_untrash_round_trips_inbox_and_trash() {
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    // trash == add [TRASH], remove [INBOX].
    provider
        .modify_thread("imap:t:m1", &["TRASH".into()], &["INBOX".into()])
        .await
        .expect("trash");
    assert!(uids_in(&server, "INBOX").is_empty());
    assert_eq!(uids_in(&server, "Trash").len(), 1);
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Trash".to_string()]);

    // untrash == add [INBOX], remove [TRASH]. The Trash copy's new UID:
    let trash_uid = uids_in(&server, "Trash")[0] as i64;
    // The local Trash location row carries that server UID (COPYUID path),
    // so untrash can SELECT Trash with the matching UIDVALIDITY.
    provider
        .modify_thread("imap:t:m1", &["INBOX".into()], &["TRASH".into()])
        .await
        .expect("untrash");
    assert!(uids_in(&server, "Trash").is_empty(), "trash uid {trash_uid} moved out");
    assert_eq!(uids_in(&server, "INBOX").len(), 1);
    assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
}

// --- spam / not-spam ---------------------------------------------------

#[tokio::test]
async fn spam_then_not_spam_round_trips_inbox_and_junk() {
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    provider
        .modify_thread("imap:t:m1", &["SPAM".into()], &["INBOX".into()])
        .await
        .expect("spam");
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Junk".to_string()]);

    provider
        .modify_thread("imap:t:m1", &["INBOX".into()], &["SPAM".into()])
        .await
        .expect("not spam");
    assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
}

// --- a thread of several messages across INBOX + Sent: Sent is untouched

#[tokio::test]
async fn archive_moves_inbox_copies_and_leaves_sent_untouched() {
    let mut s = MutServer::new();
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<a@x>")); // uid 1
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<b@x>")); // uid 2
    s.folder("Sent", 500).add(&["\\Seen"], &message("<b@x>")); // uid 1 (b also in Sent)
    s.folder("Archive", 200);
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());

    let a = seed_loc(&store, &provider, "a", "imap:t:th", "INBOX", 100, 1, &["\\Seen"]);
    let b = seed_loc(&store, &provider, "b", "imap:t:th", "INBOX", 100, 2, &["\\Seen"]);
    // b also has a Sent copy.
    store
        .upsert_location(&ImapLocation {
            mailbox: "Sent".into(), uidvalidity: 500, uid: 1,
            message_id: b.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
        })
        .unwrap();

    provider
        .modify_thread("imap:t:th", &[], &["INBOX".into()])
        .await
        .expect("archive thread");

    // Both INBOX copies moved to Archive; the Sent copy of b is untouched.
    assert!(uids_in(&server, "INBOX").is_empty());
    assert_eq!(uids_in(&server, "Archive").len(), 2);
    assert_eq!(uids_in(&server, "Sent"), vec![1], "Sent copy left in place");
    assert_eq!(local_mailboxes_of(&store, &a), vec!["Archive".to_string()]);
    assert_eq!(local_mailboxes_of(&store, &b), vec!["Archive".to_string(), "Sent".to_string()]);
}

// --- a dead sibling (no location) is skipped ---------------------------

#[tokio::test]
async fn archive_skips_a_dead_sibling_and_moves_the_live_one() {
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<live@x>")); // uid 1
    let (provider, store) = provider_with(server.clone());
    let live = seed_loc(&store, &provider, "live", "imap:t:th", "INBOX", 100, 1, &["\\Seen"]);
    // A dead sibling: thread row only, no location.
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:dead".into(), "imap:t:th".into())],
            changed_threads: vec!["imap:t:th".into()],
            ..Default::default()
        })
        .unwrap();

    provider
        .modify_thread("imap:t:th", &[], &["INBOX".into()])
        .await
        .expect("the dead sibling must not fail the archive");
    assert_eq!(local_mailboxes_of(&store, &live), vec!["Archive".to_string()]);
}

// --- an alias thread id resolves ---------------------------------------

#[tokio::test]
async fn system_move_resolves_an_alias_thread_id() {
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:survivor", "INBOX", 100, 1, &["\\Seen"]);
    store
        .commit_sync_round(&SyncRoundWrite {
            aliases: vec![("imap:t:old".into(), "imap:t:survivor".into())],
            ..Default::default()
        })
        .unwrap();

    provider
        .modify_thread("imap:t:old", &[], &["INBOX".into()])
        .await
        .expect("archive via alias");
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
}

// --- missing Archive is created then used ------------------------------

#[tokio::test]
async fn archive_creates_a_missing_archive_mailbox_then_moves() {
    // No Archive folder on the server; a UIDPLUS-only server so the move is
    // copy+delete+expunge and the COPY draws the TRYCREATE.
    let mut s = MutServer::new();
    s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    s.copy_trycreate_once = Some("Mailbox doesn't exist: Archive".into());
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    // The provider resolves the Archive role by NAME; seed a catalog row
    // so the plan knows "Archive" is the archive mailbox even before it
    // exists on the server (discovery would have listed it; here the test
    // asserts the create-on-TRYCREATE path).
    store
        .upsert_mailbox(&super::super::ImapMailbox {
            name: "Archive".into(),
            delimiter: Some("/".into()),
            special_use: Some("\\Archive".into()),
            uidvalidity: 0,
            uidnext: 0,
            highestmodseq: None,
            permanent_flags_json: None,
            permanent_keywords: None,
        })
        .unwrap();
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .expect("archive creates the destination then moves");

    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("CREATE"), 1, "Archive was created");
    assert!(g.folders.contains_key("Archive"), "Archive now exists");
    assert_eq!(
        g.folders.get("Archive").map(|f| f.messages.len()),
        Some(1),
        "the message landed in Archive"
    );
    drop(g);
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
}

// --- missing Trash is a permanent error naming it, nothing sent --------

#[tokio::test]
async fn trash_with_no_trash_mailbox_is_a_permanent_error_naming_the_role() {
    // INBOX only — no Trash configured.
    let server = server_with_inbox();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    let err = provider
        .modify_thread("imap:t:m1", &["TRASH".into()], &["INBOX".into()])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => {
            assert!(m.to_ascii_lowercase().contains("trash"), "names the role: {m}");
        }
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    // Nothing reached the wire (not even a SELECT): the destination is
    // resolved from the local plan before any command.
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0);
    assert_eq!(g.count_starting("UID MOVE"), 0);
    assert_eq!(g.count_starting("UID COPY"), 0);
}

// --- COPY+\Deleted+EXPUNGE strategy (UIDPLUS only, no MOVE) -------------

#[tokio::test]
async fn archive_uses_copy_delete_expunge_when_only_uidplus() {
    let mut s = MutServer::new();
    s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    s.folder("Archive", 200);
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .expect("archive via copy+delete+expunge");

    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID COPY"), 1);
    assert_eq!(g.count_starting("UID MOVE"), 0, "no MOVE without the capability");
    assert_eq!(g.count_starting("UID EXPUNGE"), 1, "targeted expunge");
    drop(g);
    assert!(uids_in(&server, "INBOX").is_empty());
    assert_eq!(uids_in(&server, "Archive").len(), 1);
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
}

// --- neither MOVE nor UIDPLUS -> permanent, no write command -----------

#[tokio::test]
async fn archive_with_no_move_or_uidplus_is_permanent_and_sends_no_write() {
    let mut s = MutServer::new();
    s.capabilities = vec!["IMAP4REV1".into()]; // neither
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    s.folder("Archive", 200);
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    let err = provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("EXPUNGE"), "names the hazard: {m}"),
        other => panic!("expected Unsupported InvalidOperation, got {other:?}"),
    }
    // The guard SELECTed to read capabilities, but issued no relocation.
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID MOVE"), 0);
    assert_eq!(g.count_starting("UID COPY"), 0);
    assert_eq!(g.count_starting("UID EXPUNGE"), 0);
}

// --- already satisfied sends nothing -----------------------------------

#[tokio::test]
async fn archive_of_a_thread_with_no_inbox_copy_sends_nothing() {
    // The message is already in Archive (archived), not in INBOX. Archive
    // again is a no-op: Ok and NOT A SINGLE command (not even SELECT).
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("Archive", 200).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Archive", 200, 1, &["\\Seen"]);

    provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .expect("archive of an already-archived thread is a no-op Ok");
    // archive's dest is Archive and the message is already there, so it is
    // already satisfied: nothing sent.
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0, "no SELECT for a satisfied archive");
    assert_eq!(g.count_starting("UID MOVE"), 0);
    assert_eq!(g.count_starting("UID COPY"), 0);
    drop(g);
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
}

// --- nothing movable and not satisfied is permanent --------------------

#[tokio::test]
async fn trash_of_a_sent_only_thread_is_permanent_and_sends_nothing() {
    let mut s = MutServer::new();
    s.folder("INBOX", 100);
    s.folder("Trash", 300);
    s.folder("Sent", 500).add(&["\\Seen"], &message("<m1@x>"));
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Sent", 500, 1, &["\\Seen"]);

    let err = provider
        .modify_thread("imap:t:m1", &["TRASH".into()], &["INBOX".into()])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.to_ascii_lowercase().contains("sent"), "{m}"),
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID MOVE") + g.count_starting("UID COPY"), 0);
}

// --- stale UIDVALIDITY on the source sends nothing and is transient ----

#[tokio::test]
async fn system_move_with_a_stale_source_uidvalidity_is_transient_and_sends_no_write() {
    let server = server_with_system_mailboxes();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).uidvalidity = 999; // server moved on
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]); // stored 100

    let err = provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::TransientTransport(ref m) if m.contains("UIDVALIDITY")),
        "{err:?}"
    );
    assert!(err.retry_mutation());
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID MOVE"), 0);
    assert_eq!(g.count_starting("UID COPY"), 0);
}

// --- half-applied move (expunge fails), then retry: exactly one dest copy

#[tokio::test]
async fn a_half_applied_move_then_retry_yields_exactly_one_destination_copy() {
    // UIDPLUS-only (copy+delete+expunge). The first UID EXPUNGE fails after
    // the COPY and \Deleted store landed: the message is now in BOTH INBOX
    // and Archive, and the mutation returns the partial error. The retry
    // must NOT copy again (it sees the recorded Archive location) — it only
    // expunges the source — so Archive ends with exactly one copy.
    let mut s = MutServer::new();
    s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    s.folder("Archive", 200);
    s.fail_expunge_once = Some("expunge refused".into());
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    // First attempt: COPY + \Deleted ok, EXPUNGE fails -> partial error.
    let err = provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(ref m) if m.contains("BOTH")), "{err:?}");
    // The server now has the copy in BOTH mailboxes...
    assert_eq!(uids_in(&server, "Archive").len(), 1, "copy landed in Archive");
    assert_eq!(uids_in(&server, "INBOX").len(), 1, "source \\Deleted but not yet expunged");
    // ...and the local store already records the Archive destination.
    assert!(local_mailboxes_of(&store, &id).contains(&"Archive".to_string()));

    // Retry: the destination copy is already recorded, so NO second COPY —
    // only the source is expunged. Archive stays at exactly one copy.
    let copies_before = server.lock().unwrap().count_starting("UID COPY");
    provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .expect("retry finishes the half-applied move");
    let g = server.lock().unwrap();
    assert_eq!(
        g.count_starting("UID COPY"),
        copies_before,
        "the retry copies NOTHING again"
    );
    drop(g);
    assert_eq!(uids_in(&server, "Archive").len(), 1, "exactly one destination copy");
    assert!(uids_in(&server, "INBOX").is_empty(), "source now expunged");
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
}

// --- a TRANSIENT cause of a half-applied move stays transient -----------

#[tokio::test]
async fn a_transient_expunge_failure_after_the_copy_is_retryable_and_the_retry_completes() {
    let mut s = MutServer::new();
    s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    s.folder("Archive", 200);
    s.fail_expunge_once = Some("[UNAVAILABLE] try later".into());
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    // The engine retries only retry_mutation() errors; a permanent class
    // here would roll the archive back and leave the message in BOTH
    // mailboxes with nobody to finish it.
    let err = provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .unwrap_err();
    assert!(err.retry_mutation(), "a transient cause must stay retryable: {err:?}");
    assert!(err.to_string().contains("BOTH"), "still names the partial state: {err}");
    assert!(local_mailboxes_of(&store, &id).contains(&"Archive".to_string()));

    provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .expect("the engine's retry finishes the move");
    assert_eq!(uids_in(&server, "Archive").len(), 1, "exactly one destination copy");
    assert!(uids_in(&server, "INBOX").is_empty());
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
}

// --- partial failure across two source mailboxes -----------------------

#[tokio::test]
async fn trash_partial_failure_persists_first_source_and_retry_touches_only_the_second() {
    // Two messages of one thread: one whose movable copy is in Archive
    // (trash-from-archive-view), one in INBOX. Source mailboxes are
    // processed in name order (Archive before INBOX). Archive succeeds and
    // is persisted; INBOX's SELECT is transiently busy, so the mutation
    // returns that error AFTER persisting Archive's move.
    let mut s = MutServer::new();
    s.folder("Archive", 200).add(&["\\Seen"], &message("<a@x>")); // uid 1
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<b@x>")); // uid 1
    s.folder("Trash", 300);
    s.folder("INBOX", 100).select_busy = true;
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let a = seed_loc(&store, &provider, "a", "imap:t:th", "Archive", 200, 1, &["\\Seen"]);
    let b = seed_loc(&store, &provider, "b", "imap:t:th", "INBOX", 100, 1, &["\\Seen"]);

    let err = provider
        .modify_thread("imap:t:th", &["TRASH".into()], &["INBOX".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::TransientTransport(_)), "{err:?}");
    // a (Archive, processed first) moved to Trash and is persisted; b is
    // still only in INBOX.
    assert_eq!(local_mailboxes_of(&store, &a), vec!["Trash".to_string()]);
    assert_eq!(local_mailboxes_of(&store, &b), vec!["INBOX".to_string()]);

    // Heal INBOX and retry: a is already in Trash (satisfied), so only b
    // is touched.
    server.lock().unwrap().commands.lock().unwrap().clear();
    server.lock().unwrap().folder("INBOX", 100).select_busy = false;
    provider
        .modify_thread("imap:t:th", &["TRASH".into()], &["INBOX".into()])
        .await
        .expect("retry trashes the remaining copy");
    assert_eq!(local_mailboxes_of(&store, &a), vec!["Trash".to_string()]);
    assert_eq!(local_mailboxes_of(&store, &b), vec!["Trash".to_string()]);
    // The retry SELECTed INBOX (b's source), never Archive again.
    let g = server.lock().unwrap();
    assert!(g.wire().iter().any(|c| c.to_ascii_uppercase().contains("SELECT \"INBOX\"")));
    assert!(!g.wire().iter().any(|c| c.contains("Archive")));
}

// --- combined with a flag / lf: / via modify_messages: rejected, 0 cmds -

#[tokio::test]
async fn a_system_id_mixed_or_via_modify_messages_is_rejected_with_zero_commands() {
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    // INBOX combined with UNREAD (a flag) — not an engine move shape.
    let err = provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into(), "UNREAD".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

    // TRASH combined with a label-folder id.
    let err = provider
        .modify_thread("imap:t:m1", &["TRASH".into(), "lf:Work".into()], &["INBOX".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

    // A system id through modify_messages is rejected (flag path only).
    let err = provider
        .modify_messages(&[id], &["TRASH".into()], &["INBOX".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

    // Not one command reached the wire for any rejected request.
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0);
    assert_eq!(g.count_starting("UID MOVE"), 0);
    assert_eq!(g.count_starting("UID COPY"), 0);
}

// --- a following sweep over the moved state sees no change / no dup ------

#[tokio::test]
async fn after_a_move_a_following_sweep_sees_no_change_and_no_duplicate() {
    // Archive a message, then run a full refresh over the SAME server
    // state (the message now lives only in Archive). INBOX's sweep must not
    // re-ingest the moved message, and Archive's sweep must find the local
    // location already recorded (same UID) — so no new message, no label
    // flap.
    let server = server_with_system_mailboxes();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());

    // Baseline so the local store knows m1 (in INBOX) and its thread.
    provider.baseline_cursor().await.unwrap();
    let id = store.locations_in_mailbox("INBOX").unwrap()[0].message_id.clone();

    // Archive it.
    provider
        .modify_thread(
            &store.thread_of_message(&id).unwrap().unwrap(),
            &[],
            &["INBOX".into()],
        )
        .await
        .expect("archive");
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    let gen_after_move = store.generation().unwrap();

    // A following full refresh over the same server state.
    let cursor = SyncCursor::from_generation(gen_after_move);
    let _ = provider.poll(&cursor).await.unwrap();

    // Exactly one copy locally (Archive), the same UID the move recorded;
    // no duplicate ingested, no stray INBOX row re-created.
    assert_eq!(
        local_mailboxes_of(&store, &id),
        vec!["Archive".to_string()],
        "the sweep neither duplicated the message nor resurrected the INBOX copy"
    );
    assert_eq!(
        store.locations_for_message(&id).unwrap().len(),
        1,
        "exactly one location after the sweep"
    );
}

// =======================================================================
// Phase 3 run 4: USER LABELS in label-folder mode (lf: add/remove + label
// create/delete; rename refused). Driven through a REAL ImapProvider over
// the shared MutServer, so a COPY/EXPUNGE/CREATE/DELETE reaches the wire
// and a following sweep runs against the same state. test_settings() sets
// label_storage = Folders, container = Labels.
// =======================================================================

/// A server with INBOX + Archive + Sent + the label mailbox `Labels/Clients`,
/// MOVE + UIDPLUS advertised. The label container `Labels` is \Noselect
/// (never a folder row); its child `Labels/Clients` is the label mailbox.
fn server_with_label_folder() -> Arc<Mutex<MutServer>> {
    let mut s = MutServer::new();
    s.folder("INBOX", 100);
    s.folder("Archive", 200);
    s.folder("Sent", 500);
    s.folder("Labels/Clients", 600);
    Arc::new(Mutex::new(s))
}

/// Local location count in one mailbox.
fn local_count_in(store: &ImapStateStore, mailbox: &str) -> usize {
    store.locations_in_mailbox(mailbox).unwrap().len()
}

// --- add a label to one message: COPY into the label mailbox -----------

#[tokio::test]
async fn add_label_copies_the_inbox_message_into_the_label_mailbox() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    provider
        .modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[])
        .await
        .expect("add label");

    // The server now holds a copy in Labels/Clients, and the local store
    // recorded that destination location (from COPYUID) with the same flags.
    assert_eq!(uids_in(&server, "Labels/Clients").len(), 1, "one copy on the server");
    let boxes = local_mailboxes_of(&store, &id);
    assert!(boxes.contains(&"INBOX".to_string()));
    assert!(boxes.contains(&"Labels/Clients".to_string()), "{boxes:?}");
    // Exactly one COPY command, no MOVE, no EXPUNGE.
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID COPY"), 1);
    assert_eq!(g.count_starting("UID MOVE"), 0);
    assert_eq!(g.count_starting("UID EXPUNGE"), 0);
}

#[tokio::test]
async fn add_label_to_several_messages_copies_each() {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<a@x>")); // uid 1
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<b@x>")); // uid 2
    }
    let (provider, store) = provider_with(server.clone());
    let a = seed_loc(&store, &provider, "a", "imap:t:a", "INBOX", 100, 1, &["\\Seen"]);
    let b = seed_loc(&store, &provider, "b", "imap:t:b", "INBOX", 100, 2, &["\\Seen"]);

    provider
        .modify_messages(&[a.clone(), b.clone()], &["lf:Clients".into()], &[])
        .await
        .expect("add label to two");

    assert_eq!(uids_in(&server, "Labels/Clients").len(), 2, "two copies on the server");
    assert!(local_mailboxes_of(&store, &a).contains(&"Labels/Clients".to_string()));
    assert!(local_mailboxes_of(&store, &b).contains(&"Labels/Clients".to_string()));
}

// --- add is idempotent: a second add sends no command ------------------

#[tokio::test]
async fn add_label_is_idempotent_and_sends_no_command_the_second_time() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    provider.modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[]).await.expect("add 1");
    server.lock().unwrap().commands.lock().unwrap().clear();
    // Second add: already satisfied — ZERO commands (not even a SELECT).
    provider.modify_messages(&[id], &["lf:Clients".into()], &[]).await.expect("add 2 no-op");
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0, "idempotent add selects nothing");
    assert_eq!(g.count_starting("UID COPY"), 0, "and copies nothing");
}

// --- source priority: Archive used when the message is not in INBOX ----

#[tokio::test]
async fn add_label_source_priority_prefers_inbox_then_archive() {
    // A message with copies in Archive AND Sent (no INBOX): Archive wins.
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("Archive", 200).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        g.folder("Sent", 500).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Archive", 200, 1, &["\\Seen"]);
    store
        .upsert_location(&ImapLocation {
            mailbox: "Sent".into(), uidvalidity: 500, uid: 1,
            message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
        })
        .unwrap();

    provider.modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[]).await.expect("add");

    // The COPY was issued from Archive, not Sent.
    let g = server.lock().unwrap();
    assert!(
        g.wire().iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"ARCHIVE\"")),
        "selected Archive as the source: {:?}", g.wire()
    );
    assert!(!g.wire().iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"SENT\"")));
}

// --- a message whose only copy is in Trash is refused ------------------

#[tokio::test]
async fn add_label_from_a_trash_only_message_is_a_permanent_error() {
    let mut s = MutServer::new();
    s.folder("INBOX", 100);
    s.folder("Trash", 300).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    s.folder("Labels/Clients", 600);
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Trash", 300, 1, &["\\Seen"]);

    let err = provider
        .modify_messages(&[id], &["lf:Clients".into()], &[])
        .await
        .unwrap_err();
    assert!(!err.retry_mutation());
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("no synced copy outside Trash"), "{m}"),
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0, "nothing copied");
}

// --- a missing label mailbox is a permanent error ----------------------

#[tokio::test]
async fn add_label_to_a_missing_label_mailbox_is_permanent() {
    // No Labels/Clients folder exists in the catalog.
    let mut s = MutServer::new();
    s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let server = Arc::new(Mutex::new(s));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    let err = provider
        .modify_messages(&[id], &["lf:Clients".into()], &[])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("does not exist"), "{m}"),
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0);
}

// --- add with a stale source UIDVALIDITY sends nothing and is transient -

#[tokio::test]
async fn add_label_with_a_stale_source_uidvalidity_is_transient_and_sends_no_copy() {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).uidvalidity = 999; // server moved on
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    // Local location says UIDVALIDITY 100 (stale).
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    let err = provider
        .modify_messages(&[id], &["lf:Clients".into()], &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::TransientTransport(ref m) if m.contains("UIDVALIDITY")),
        "{err:?}"
    );
    assert!(err.retry_mutation());
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0, "nothing copied");
}

// --- remove a label: expunge only that copy, keep the others -----------

#[tokio::test]
async fn remove_label_expunges_only_the_label_copy_and_keeps_the_inbox_copy() {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>")); // uid 1
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
    store
        .upsert_location(&ImapLocation {
            mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
            message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
        })
        .unwrap();

    provider
        .modify_messages(std::slice::from_ref(&id), &[], &["lf:Clients".into()])
        .await
        .expect("remove label");

    // The label copy is gone (server + local); the INBOX copy is untouched.
    assert_eq!(uids_in(&server, "Labels/Clients").len(), 0, "label copy expunged");
    assert_eq!(uids_in(&server, "INBOX"), vec![1], "INBOX copy kept");
    assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID EXPUNGE"), 1);
}

// --- remove the LAST copy is refused with ZERO commands ----------------

#[tokio::test]
async fn remove_label_that_is_the_last_copy_is_refused_with_zero_commands() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    // The ONLY synced location is the label copy.
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Labels/Clients", 600, 1, &["\\Seen"]);

    let err = provider
        .modify_messages(&[id], &[], &["lf:Clients".into()])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("only synced location"), "{m}"),
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0, "refused before any SELECT");
    assert_eq!(g.count_starting("UID STORE"), 0);
    assert_eq!(g.count_starting("UID EXPUNGE"), 0);
}

// --- remove without UIDPLUS refused with zero write commands -----------

#[tokio::test]
async fn remove_label_without_uidplus_is_refused_with_no_write() {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.capabilities = vec!["IMAP4REV1".into(), "MOVE".into()]; // NO UIDPLUS
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
    store
        .upsert_location(&ImapLocation {
            mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
            message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
        })
        .unwrap();

    let err = provider
        .modify_messages(&[id], &[], &["lf:Clients".into()])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("UIDPLUS"), "{m}"),
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("UID STORE"), 0, "no \\Deleted store without UIDPLUS");
    assert_eq!(g.count_starting("UID EXPUNGE"), 0, "and never a plain/UID EXPUNGE");
}

// --- remove with no label copy is a satisfied no-op --------------------

#[tokio::test]
async fn remove_label_with_no_label_copy_sends_nothing() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    provider
        .modify_messages(&[id], &[], &["lf:Clients".into()])
        .await
        .expect("satisfied no-op");
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0);
    assert_eq!(g.count_starting("UID EXPUNGE"), 0);
}

// --- half-applied remove (expunge fails once) then retry finishes -------

#[tokio::test]
async fn remove_label_half_applied_expunge_failure_is_retryable_and_the_retry_finishes() {
    for (fail_text, expect_transient) in [
        ("[UNAVAILABLE] try later", true),
        ("[NONEXISTENT] gone", false),
    ] {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.fail_expunge_once = Some(fail_text.into());
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
                message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        // First attempt: COPY-side \Deleted store lands, EXPUNGE fails once.
        let err = provider
            .modify_messages(std::slice::from_ref(&id), &[], &["lf:Clients".into()])
            .await
            .unwrap_err();
        assert_eq!(
            err.retry_mutation(), expect_transient,
            "retry class preserved for {fail_text}: {err:?}"
        );
        // The label copy is still present on the server (flagged \Deleted),
        // and still recorded locally — the half state the retry finishes.
        assert_eq!(uids_in(&server, "Labels/Clients").len(), 1, "copy still present after the failed expunge");
        assert!(local_mailboxes_of(&store, &id).contains(&"Labels/Clients".to_string()));

        // Retry: the one-shot failure is cleared, the expunge completes,
        // the label copy and its local row are gone, INBOX is untouched.
        provider
            .modify_messages(std::slice::from_ref(&id), &[], &["lf:Clients".into()])
            .await
            .expect("retry finishes the remove");
        assert_eq!(uids_in(&server, "Labels/Clients").len(), 0, "copy expunged on retry");
        assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
    }
}

// --- a partial add failure does not create a second copy on retry ------

#[tokio::test]
async fn add_label_retry_after_a_recorded_copy_does_not_double_copy() {
    // Simulate a partial success: the destination row is already recorded
    // locally (an earlier COPY confirmed) but the mutation is retried. The
    // retry must COPY nothing (idempotent).
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>")); // the copy already landed
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
    // Record the destination row as a prior partial success would.
    store
        .upsert_location(&ImapLocation {
            mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
            message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
        })
        .unwrap();

    provider.modify_messages(&[id], &["lf:Clients".into()], &[]).await.expect("retry add");
    // Still exactly one copy on the server (no second copy created).
    assert_eq!(uids_in(&server, "Labels/Clients").len(), 1, "no duplicate copy");
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0, "retry copies nothing");
}

// --- non-Folders storage mode rejects every lf: op with zero commands --

#[tokio::test]
async fn label_ops_in_non_folders_storage_mode_are_rejected_with_zero_commands() {
    use super::super::settings::LabelStorage;
    let server = server_with_label_folder();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (mut provider, store) = provider_with(server.clone());
    // Flip to keyword mode (no container in play).
    provider.settings.label_storage = LabelStorage::Keywords;
    provider.settings.label_container = None;
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    for (add, remove) in [
        (vec!["lf:Clients".to_string()], vec![]),
        (vec![], vec!["lf:Clients".to_string()]),
    ] {
        let err = provider.modify_messages(std::slice::from_ref(&id), &add, &remove).await.unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("keyword labels"), "{m}"),
            other => panic!("expected InvalidOperation naming the mode, got {other:?}"),
        }
    }
    // create / delete likewise refuse naming the mode.
    assert!(matches!(provider.create_label("X").await.unwrap_err(), ProviderError::InvalidOperation(_)));
    assert!(matches!(provider.delete_label("lf:Clients").await.unwrap_err(), ProviderError::InvalidOperation(_)));
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0);
    assert_eq!(server.lock().unwrap().count_starting("CREATE"), 0);
}

// --- mixed / folder: / system / flag ids rejected with zero commands ---

#[tokio::test]
async fn a_mixed_or_multi_label_message_request_is_rejected_before_any_command() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    let bad: Vec<(Vec<String>, Vec<String>)> = vec![
        (vec!["lf:Clients".into(), "STARRED".into()], vec![]), // label + flag
        (vec!["lf:A".into(), "lf:B".into()], vec![]),          // two labels
        (vec!["lf:Clients".into()], vec!["lf:Clients".into()]), // both lists
        (vec!["folder:Projects".into()], vec![]),               // folder: not accepted
        (vec!["lf:Clients".into(), "INBOX".into()], vec![]),    // label + system
    ];
    for (add, remove) in bad {
        let err = provider.modify_messages(std::slice::from_ref(&id), &add, &remove).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "add={add:?} remove={remove:?} -> {err:?}");
        assert!(!err.retry_mutation());
    }
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("SELECT"), 0, "nothing selected for a rejected request");
    assert_eq!(g.count_starting("UID COPY"), 0);
}

// --- modify_thread still rejects a lf: id (engine never sends it there) -

#[tokio::test]
async fn modify_thread_still_rejects_a_label_id_permanently() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

    let err = provider
        .modify_thread("imap:t:m1", &["lf:Clients".into()], &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0);
}

// --- create a label: new, case-insensitive existing, hostile, limits ---

#[tokio::test]
async fn create_label_creates_the_mailbox_and_lists_it() {
    let server = server_with_label_folder();
    let (provider, _store) = provider_with(server.clone());

    let label = provider.create_label("Acme").await.expect("create");
    assert_eq!(label.id, "lf:Acme");
    assert_eq!(label.name, "Acme");
    assert_eq!(label.kind, "user");
    // The mailbox Labels/Acme exists on the server…
    assert!(server.lock().unwrap().folders.contains_key("Labels/Acme"));
    // …and list_labels shows it immediately, with no sync.
    let listed = provider.list_labels().await.unwrap();
    assert!(listed.iter().any(|l| l.id == "lf:Acme" && l.kind == "user"), "{listed:?}");
}

#[tokio::test]
async fn create_label_is_case_insensitively_idempotent() {
    let server = server_with_label_folder(); // already has Labels/Clients
    let (provider, _store) = provider_with(server.clone());

    // "clients" aliases the existing "Clients" case-insensitively: returns
    // the EXISTING label and creates no new mailbox.
    let label = provider.create_label("clients").await.expect("idempotent create");
    assert_eq!(label.id, "lf:Clients", "returns the existing label id");
    assert!(!server.lock().unwrap().folders.contains_key("Labels/clients"), "no new mailbox");
    assert_eq!(server.lock().unwrap().count_starting("CREATE"), 0, "no CREATE issued");
}

#[tokio::test]
async fn create_label_rejects_hostile_names_delimiter_and_over_length() {
    let server = server_with_label_folder();
    let (provider, _store) = provider_with(server.clone());

    for bad in [
        "",                 // empty
        "   ",              // whitespace only
        "a\rb",             // control (CR)
        "a\nb",             // control (LF)
        "Clients/Sub",      // contains the delimiter
        "/leading",         // leading delimiter
        "trailing/",        // trailing delimiter
    ] {
        let err = provider.create_label(bad).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{bad:?} -> {err:?}");
    }
    // Length: below / exact / above MAX_LABEL_NAME_CHARS.
    let max = super::super::policy::MAX_LABEL_NAME_CHARS;
    let below = "x".repeat(max - 1);
    let exact = "x".repeat(max);
    let above = "x".repeat(max + 1);
    assert!(provider.create_label(&below).await.is_ok(), "below the limit");
    assert!(provider.create_label(&exact).await.is_ok(), "exactly at the limit");
    let err = provider.create_label(&above).await.unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)), "above the limit must reject");
    // No mailbox was created for the over-length name.
    assert!(!server.lock().unwrap().folders.contains_key(&format!("Labels/{above}")));
}

#[tokio::test]
async fn create_label_creates_a_missing_container_then_retries() {
    // The server has no Labels container child yet, and CREATE Labels/New
    // answers TRYCREATE once (missing parent). create_label then creates
    // the container and retries, succeeding.
    let mut s = MutServer::new();
    s.folder("INBOX", 100);
    s.create_trycreate_once = Some("Labels/New".into());
    let server = Arc::new(Mutex::new(s));
    let (provider, _store) = provider_with(server.clone());

    let label = provider.create_label("New").await.expect("create after container");
    assert_eq!(label.id, "lf:New");
    let g = server.lock().unwrap();
    // Both the container and the label mailbox exist now.
    assert!(g.folders.contains_key("Labels"), "container created");
    assert!(g.folders.contains_key("Labels/New"), "label mailbox created on retry");
    // Two CREATE lines reached the wire (the failed label, then container +
    // retried label) — at least the container + successful label.
    assert!(g.count_starting("CREATE") >= 2, "{:?}", g.wire());
}

// --- delete a label: allowed, refused (sole copy), refused (mismatch) --

#[tokio::test]
async fn delete_label_allowed_when_every_message_has_another_copy() {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
    store
        .upsert_location(&ImapLocation {
            mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
            message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
        })
        .unwrap();

    provider.delete_label("lf:Clients").await.expect("delete allowed");
    // The mailbox is gone on the server, the catalog row retired, the
    // label copy's local location removed; INBOX untouched.
    assert!(!server.lock().unwrap().folders.contains_key("Labels/Clients"));
    assert!(!store.mailboxes().unwrap().iter().any(|m| m.name == "Labels/Clients"));
    assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
    // DELETE was issued, and INBOX was selected first (never DELETE the
    // selected mailbox).
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("DELETE"), 1);
    let delete_pos = g.wire().iter().position(|c| c.to_ascii_uppercase().starts_with("DELETE")).unwrap();
    assert!(
        g.wire()[..delete_pos].iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"INBOX\"")),
        "INBOX selected before DELETE: {:?}", g.wire()
    );
}

#[tokio::test]
async fn delete_label_refused_when_a_message_has_no_other_copy() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    // The ONLY copy of m1 is in the label mailbox.
    let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Labels/Clients", 600, 1, &["\\Seen"]);

    let err = provider.delete_label("lf:Clients").await.unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("no other synced copy"), "{m}"),
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    // Nothing deleted on the server.
    assert!(server.lock().unwrap().folders.contains_key("Labels/Clients"));
    assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
}

#[tokio::test]
async fn delete_label_refused_when_server_count_exceeds_local_count() {
    // The server's label mailbox holds TWO messages but the local store
    // knows only one — unsynced mail we cannot vouch for, so refuse.
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>")); // uid 1 (known)
        g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<unsynced@x>")); // uid 2 (unknown locally)
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
    store
        .upsert_location(&ImapLocation {
            mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
            message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
        })
        .unwrap();

    let err = provider.delete_label("lf:Clients").await.unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("unsynced"), "{m}"),
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    assert!(server.lock().unwrap().folders.contains_key("Labels/Clients"), "nothing deleted");
    assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
}

#[tokio::test]
async fn delete_label_already_absent_is_idempotent_success() {
    let server = server_with_label_folder();
    let (provider, _store) = provider_with(server.clone());
    // A label with no catalog row: deleting it is success, no command.
    provider.delete_label("lf:DoesNotExist").await.expect("idempotent absent delete");
    assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
    assert_eq!(server.lock().unwrap().count_starting("EXAMINE"), 0);
}

// --- rename (update_label) is refused permanently ----------------------

#[tokio::test]
async fn update_label_rename_is_refused_permanently_with_a_reason() {
    let server = server_with_label_folder();
    let (provider, _store) = provider_with(server.clone());

    let err = provider.update_label("lf:Clients", "Customers").await.unwrap_err();
    assert!(!err.retry_mutation(), "a rename refusal is permanent");
    match err {
        ProviderError::InvalidOperation(m) => {
            assert!(m.to_ascii_lowercase().contains("renaming"), "{m}");
            assert!(m.contains("derived from its name"), "names the reason: {m}");
        }
        other => panic!("expected permanent InvalidOperation, got {other:?}"),
    }
    // Nothing touched the server (no RENAME, the label mailbox is intact).
    let g = server.lock().unwrap();
    assert_eq!(g.count_starting("RENAME"), 0);
    assert!(g.folders.contains_key("Labels/Clients"));
}

// --- add label then a following sweep: no change, no duplicate ---------

#[tokio::test]
async fn after_add_label_a_following_sweep_sees_no_change_and_no_duplicate() {
    let server = server_with_label_folder();
    server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());

    // Baseline so INBOX's m1 is ingested with its id/thread.
    provider.baseline_cursor().await.unwrap();
    let id = store.locations_in_mailbox("INBOX").unwrap()[0].message_id.clone();

    provider.modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[]).await.expect("add label");
    let copies_before = store.locations_for_message(&id).unwrap().len();
    assert_eq!(copies_before, 2, "INBOX + Labels/Clients");
    let gen_after = store.generation().unwrap();

    // A following full refresh over the same server state must not
    // duplicate the labelled copy nor flap labels.
    let cursor = SyncCursor::from_generation(gen_after);
    for _ in 0..4 {
        let _ = provider.poll(&cursor).await.unwrap();
    }
    assert_eq!(
        store.locations_for_message(&id).unwrap().len(),
        2,
        "exactly two locations after the sweep (no duplicate label copy)"
    );
}

#[tokio::test]
async fn delete_label_refuses_a_uidvalidity_reset_even_when_count_and_uids_match() {
    let (server, provider, _store, _id) = paired_label_message();
    server
        .lock()
        .unwrap()
        .folder("Labels/Clients", 600)
        .uidvalidity = 601;
    assert!(provider.delete_label("lf:Clients").await.is_err());
    assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
}

#[tokio::test]
async fn remove_label_catches_a_silently_ignored_permanent_deleted_flag() {
    let (server, provider, store, id) = paired_label_message();
    server.lock().unwrap().silently_ignore_flag = Some("\\Deleted".into());
    let err = provider
        .modify_messages(std::slice::from_ref(&id), &[], &["lf:Clients".into()])
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::InvalidOperation(ref text) if text.contains("silently ignored"))
    );
    assert_eq!(server.lock().unwrap().count_starting("UID EXPUNGE"), 0);
    assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
    assert!(local_mailboxes_of(&store, &id).contains(&"Labels/Clients".into()));
}

#[tokio::test]
async fn remove_label_keeps_its_location_until_expunge_really_removes_the_copy() {
    let (server, provider, store, id) = paired_label_message();
    server.lock().unwrap().ignore_expunge = true;
    let err = provider
        .modify_messages(std::slice::from_ref(&id), &[], &["lf:Clients".into()])
        .await
        .unwrap_err();
    assert!(err.retry_mutation());
    assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
    assert!(local_mailboxes_of(&store, &id).contains(&"Labels/Clients".into()));
}

#[tokio::test]
async fn fallback_move_requires_deleted_before_copy_and_verifies_it_before_expunge() {
    for supported in [false, true] {
        let server = server_with_system_mailboxes();
        {
            let mut g = server.lock().unwrap();
            g.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.silently_ignore_flag = Some("\\Deleted".into());
            if !supported {
                g.folder("INBOX", 100).permanent_flags = vec!["\\Seen".into()];
            }
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(
            &store,
            &provider,
            "m1",
            "imap:t:m1",
            "INBOX",
            100,
            1,
            &["\\Seen"],
        );
        assert!(provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .is_err());
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID COPY"), usize::from(supported));
        assert_eq!(g.count_starting("UID EXPUNGE"), 0);
        drop(g);
        assert_eq!(uids_in(&server, "INBOX"), vec![1]);
        assert!(local_mailboxes_of(&store, &id).contains(&"INBOX".into()));
    }
}

#[tokio::test]
async fn encoded_labels_round_trip_creation_catalog_flags_removal_and_deletion() {
    for (name, wire) in [
        ("R&D", "R&-D"),
        ("Café", "Caf&AOk-"),
        ("日本語", "&ZeVnLIqe-"),
    ] {
        let server = server_with_label_folder();
        server
            .lock()
            .unwrap()
            .folder("INBOX", 100)
            .add(&[], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
        let label = provider.create_label(name).await.unwrap();
        assert_eq!(label.id, format!("lf:{wire}"));
        assert_eq!(label.name, name);
        assert!(provider
            .list_labels()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == label.id && item.name == label.name));
        provider
            .modify_messages(std::slice::from_ref(&id), std::slice::from_ref(&label.id), &[])
            .await
            .unwrap();
        provider
            .modify_messages(std::slice::from_ref(&id), &[], &["UNREAD".into()])
            .await
            .unwrap();
        assert!(server
            .lock()
            .unwrap()
            .flags_of(&format!("Labels/{wire}"), 1)
            .contains(&"\\Seen".into()));
        provider
            .modify_messages(&[id], &[], std::slice::from_ref(&label.id))
            .await
            .unwrap();
        provider.delete_label(&label.id).await.unwrap();
        assert!(!server
            .lock()
            .unwrap()
            .folders
            .contains_key(&format!("Labels/{wire}")));
    }
}

#[tokio::test]
async fn existing_wire_label_ids_and_encoded_container_are_preserved() {
    let server = server_with_inbox();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        g.folder("&ZeVnLIqe-/Caf&AOk-", 600);
    }
    let (mut provider, store) = provider_with(server.clone());
    provider.settings.label_container = Some("&ZeVnLIqe-".into());
    let id = seed_loc(
        &store,
        &provider,
        "m1",
        "imap:t:m1",
        "INBOX",
        100,
        1,
        &["\\Seen"],
    );
    let label = provider.create_label("café").await.unwrap();
    assert_eq!(label.id, "lf:Caf&AOk-");
    assert_eq!(label.name, "Café");
    assert_eq!(server.lock().unwrap().count_starting("CREATE"), 0);
    provider
        .modify_messages(std::slice::from_ref(&id), std::slice::from_ref(&label.id), &[])
        .await
        .unwrap();
    provider
        .modify_messages(&[id], &[], std::slice::from_ref(&label.id))
        .await
        .unwrap();
    provider.delete_label(&label.id).await.unwrap();
}

#[tokio::test]
async fn fallback_move_recovers_a_lost_copy_response_without_a_second_copy() {
    let server = server_with_system_mailboxes();
    {
        let mut g = server.lock().unwrap();
        g.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        g.drop_copy_response_once = true;
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(
        &store,
        &provider,
        "m1",
        "imap:t:m1",
        "INBOX",
        100,
        1,
        &["\\Seen"],
    );
    let err = provider
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .unwrap_err();
    assert!(err.retry_mutation());
    assert!(store
        .pending_copy_identity(&id, "Archive")
        .unwrap()
        .is_some());
    let restarted = ImapProvider {
        source: Arc::new(MutSource {
            server: server.clone(),
        }),
        ..provider
    };
    restarted
        .modify_thread("imap:t:m1", &[], &["INBOX".into()])
        .await
        .unwrap();
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
    assert_eq!(uids_in(&server, "Archive").len(), 1);
    assert!(uids_in(&server, "INBOX").is_empty());
    assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive"]);
    assert_eq!(store.pending_copy_identity(&id, "Archive").unwrap(), None);
}

#[tokio::test]
async fn copy_without_copyuid_reconciles_before_reporting_success() {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.capabilities = vec!["IMAP4REV1".into()];
        g.omit_copyuid = true;
        g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
    provider
        .modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[])
        .await
        .unwrap();
    provider
        .modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[])
        .await
        .unwrap();
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
    assert_eq!(
        local_mailboxes_of(&store, &id),
        vec!["INBOX", "Labels/Clients"]
    );
    assert_eq!(
        store.pending_copy_identity(&id, "Labels/Clients").unwrap(),
        None
    );
    assert!(
        server
            .lock()
            .unwrap()
            .flags_of("Labels/Clients", 1)
            .is_empty(),
        "PEEK must not set Seen"
    );
}

#[tokio::test]
async fn an_intent_saved_before_copy_can_retry_when_no_copy_landed() {
    let server = server_with_label_folder();
    server
        .lock()
        .unwrap()
        .folder("INBOX", 100)
        .add(&[], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
    let identity = crate::provider::imap::identity::derive_message_id(
        store.account_id(),
        &crate::provider::imap::identity::IdentityInputs {
            message_id: Some("<m1@x>".into()),
            ..Default::default()
        },
    );
    store.begin_copy(&id, "Labels/Clients", &identity).unwrap();
    provider
        .modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[])
        .await
        .unwrap();
    assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
    assert_eq!(uids_in(&server, "Labels/Clients").len(), 1);
    assert_eq!(
        store.pending_copy_identity(&id, "Labels/Clients").unwrap(),
        None
    );
}

fn paired_label_message() -> (Arc<Mutex<MutServer>>, ImapProvider, ImapStateStore, String) {
    let server = server_with_label_folder();
    {
        let mut g = server.lock().unwrap();
        g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        g.folder("Labels/Clients", 600)
            .add(&["\\Seen"], &message("<m1@x>"));
    }
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(
        &store,
        &provider,
        "m1",
        "imap:t:m1",
        "INBOX",
        100,
        1,
        &["\\Seen"],
    );
    store
        .upsert_location(&ImapLocation {
            mailbox: "Labels/Clients".into(),
            uidvalidity: 600,
            uid: 1,
            message_id: id.clone(),
            flags_json: r#"["\\Seen"]"#.into(),
            modseq: None,
        })
        .unwrap();
    (server, provider, store, id)
}

#[tokio::test]
async fn delete_label_must_compare_uids_not_just_count() {
    let (server, provider, _store, _id) = paired_label_message();
    {
        let mut g = server.lock().unwrap();
        let f = g.folder("Labels/Clients", 600);
        f.messages.clear();
        f.add(&["\\Seen"], &message("<unsynced-only-copy@x>"));
    }
    let result = provider.delete_label("lf:Clients").await;
    assert!(result.is_err(), "must not delete a different unsynced message just because EXISTS still equals 1; got {result:?}");
    assert!(server
        .lock()
        .unwrap()
        .folders
        .contains_key("Labels/Clients"));
}

#[tokio::test]
async fn remove_label_must_check_and_verify_deleted_flag() {
    let (server, provider, store, id) = paired_label_message();
    {
        let mut g = server.lock().unwrap();
        g.folder("Labels/Clients", 600).permanent_flags = vec!["\\Seen".into()];
        g.silently_ignore_flag = Some("\\Deleted".into());
    }
    let result = provider
        .modify_messages(std::slice::from_ref(&id), &[], &["lf:Clients".into()])
        .await;
    assert!(result.is_err(), "must not report a successful removal when Deleted is unsupported and the server retained the message; got {result:?}; server UIDs {:?}; local copies {:?}", uids_in(&server, "Labels/Clients"), local_mailboxes_of(&store, &id));
}

#[tokio::test]
async fn created_ampersand_label_must_be_removable() {
    let server = server_with_label_folder();
    server
        .lock()
        .unwrap()
        .folder("INBOX", 100)
        .add(&["\\Seen"], &message("<m1@x>"));
    let (provider, store) = provider_with(server.clone());
    let id = seed_loc(
        &store,
        &provider,
        "m1",
        "imap:t:m1",
        "INBOX",
        100,
        1,
        &["\\Seen"],
    );
    let label = provider.create_label("R&D").await.unwrap();
    provider
        .modify_messages(std::slice::from_ref(&id), std::slice::from_ref(&label.id), &[])
        .await
        .unwrap();
    assert_eq!(uids_in(&server, "Labels/R&-D").len(), 1);
    let result = provider.modify_messages(&[id], &[], &[label.id]).await;
    assert!(
        result.is_ok(),
        "created label must round-trip through SELECT when removing; got {result:?}; wire {:?}",
        server.lock().unwrap().wire()
    );
}

#[tokio::test]
async fn copy_response_loss_must_not_duplicate_on_retry() {
    for raw in [
        message("<m1@x>"),
        message("<m1@x>").replace("Message-ID: <m1@x>\r\n", ""),
    ] {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&[], &raw);
            g.drop_copy_response_once = true;
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
        let first = provider
            .modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[])
            .await
            .unwrap_err();
        assert!(first.retry_mutation(), "{first:?}");
        assert_eq!(uids_in(&server, "Labels/Clients").len(), 1);
        assert!(store
            .pending_copy_identity(&id, "Labels/Clients")
            .unwrap()
            .is_some());
        provider
            .modify_messages(std::slice::from_ref(&id), &["lf:Clients".into()], &[])
            .await
            .unwrap();
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
        assert_eq!(
            uids_in(&server, "Labels/Clients").len(),
            1,
            "retry must recover both Message-ID and synthetic identity copies"
        );
        assert_eq!(
            store.pending_copy_identity(&id, "Labels/Clients").unwrap(),
            None
        );
    }
}

#[tokio::test]
async fn remove_label_preserves_last_server_copy_when_backup_was_expunged() {
    let (server, provider, _store, id) = paired_label_message();
    server.lock().unwrap().folder("INBOX", 100).messages.clear();
    let result = provider
        .modify_messages(&[id], &[], &["lf:Clients".into()])
        .await;
    assert!(matches!(result, Err(ProviderError::InvalidOperation(_))));
    assert_eq!(
        uids_in(&server, "Labels/Clients"),
        vec![1],
        "stale local INBOX location is not evidence of a surviving server copy"
    );
}
#[tokio::test]
async fn delete_label_preserves_last_server_copy_when_backup_was_expunged() {
    let (server, provider, _store, _id) = paired_label_message();
    server.lock().unwrap().folder("INBOX", 100).messages.clear();
    let result = provider.delete_label("lf:Clients").await;
    assert!(matches!(result, Err(ProviderError::InvalidOperation(_))));
    assert!(
        server
            .lock()
            .unwrap()
            .folders
            .contains_key("Labels/Clients"),
        "must not delete a label containing the only remaining server copy"
    );
}

#[tokio::test]
async fn removing_or_deleting_a_label_refuses_stale_or_deleted_backup_coordinates() {
    for delete_mailbox in [false, true] {
        for scenario in ["uidvalidity", "deleted", "missing"] {
            let (server, provider, store, id) = paired_label_message();
            {
                let mut server = server.lock().unwrap();
                match scenario {
                    "uidvalidity" => server.folder("INBOX", 100).uidvalidity = 101,
                    "deleted" => server.folder("INBOX", 100).messages[0]
                        .flags
                        .push("\\Deleted".into()),
                    "missing" => {
                        server.folders.remove("INBOX");
                    }
                    _ => unreachable!(),
                }
            }
            let result = if delete_mailbox {
                provider.delete_label("lf:Clients").await
            } else {
                provider
                    .modify_messages(std::slice::from_ref(&id), &[], &["lf:Clients".into()])
                    .await
            };
            assert!(
                matches!(result, Err(ProviderError::InvalidOperation(_))),
                "{scenario}: {result:?}"
            );
            assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
            assert_eq!(local_count_in(&store, "Labels/Clients"), 1);
            let server = server.lock().unwrap();
            for command in ["UID STORE", "UID EXPUNGE", "DELETE"] {
                assert_eq!(server.count_starting(command), 0, "{scenario}: {command}");
            }
        }
    }
}

#[tokio::test]
async fn label_removal_can_use_a_live_archive_backup_despite_a_stale_inbox_copy() {
    for delete_mailbox in [false, true] {
        let (server, provider, store, id) = paired_label_message();
        {
            let mut server = server.lock().unwrap();
            server.folder("INBOX", 100).messages.clear();
            server.folder("Archive", 200).add(&[], &message("<m1@x>"));
        }
        store
            .upsert_location(&ImapLocation {
                mailbox: "Archive".into(),
                uidvalidity: 200,
                uid: 1,
                message_id: id.clone(),
                flags_json: "[]".into(),
                modseq: None,
            })
            .unwrap();
        if delete_mailbox {
            provider.delete_label("lf:Clients").await.unwrap();
        } else {
            provider
                .modify_messages(&[id], &[], &["lf:Clients".into()])
                .await
                .unwrap();
        }
        assert_eq!(uids_in(&server, "Archive"), vec![1]);
        assert!(
            server.lock().unwrap().folder("Archive", 200).messages[0]
                .flags
                .is_empty(),
            "checking a survivor must not mark it read"
        );
        assert_eq!(local_count_in(&store, "Labels/Clients"), 0);
    }
}

#[tokio::test]
async fn label_removal_preserves_transient_backup_check_failures_without_writing() {
    let (server, provider, _store, id) = paired_label_message();
    server.lock().unwrap().folder("INBOX", 100).select_busy = true;
    let result = provider
        .modify_messages(&[id], &[], &["lf:Clients".into()])
        .await;
    assert!(
        matches!(result, Err(ProviderError::TransientTransport(_))),
        "{result:?}"
    );
    assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
    let server = server.lock().unwrap();
    assert_eq!(server.count_starting("UID STORE"), 0);
    assert_eq!(server.count_starting("UID EXPUNGE"), 0);
}

#[tokio::test]
async fn label_removal_checks_every_message_before_destroying_any_copy() {
    for delete_mailbox in [false, true] {
        let (server, provider, store, first_id) = paired_label_message();
        {
            let mut server = server.lock().unwrap();
            // The second INBOX location is stale, but its label copy is
            // live. A safe first message must not cause a partial delete.
            server
                .folder("Labels/Clients", 600)
                .add(&[], &message("<m2@x>"));
        }
        let second_id = seed_loc(&store, &provider, "m2", "imap:t:m2", "INBOX", 100, 2, &[]);
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(),
                uidvalidity: 600,
                uid: 2,
                message_id: second_id.clone(),
                flags_json: "[]".into(),
                modseq: None,
            })
            .unwrap();
        let result = if delete_mailbox {
            provider.delete_label("lf:Clients").await
        } else {
            provider
                .modify_messages(&[first_id, second_id], &[], &["lf:Clients".into()])
                .await
        };
        assert!(
            matches!(result, Err(ProviderError::InvalidOperation(_))),
            "{result:?}"
        );
        assert_eq!(uids_in(&server, "Labels/Clients"), vec![1, 2]);
        let server = server.lock().unwrap();
        for command in ["UID STORE", "UID EXPUNGE", "DELETE"] {
            assert_eq!(server.count_starting(command), 0);
        }
    }
}

#[tokio::test]
async fn server_saves_sent_override_skips_missing_mailbox() {
    let server = server_with_inbox();
    let (mut provider, _store) = provider_with(server.clone());
    provider.settings.server_saves_sent = true;
    let result = provider
        .save_sent_copy(&crate::provider::SentCopyRequest {
            raw: b"Message-ID: <override@x>\r\n\r\nbody".to_vec(),
            outbox_id: "override-test".into(),
            append_attempted: false,
            phase: crate::provider::SentCopyPhase::Pending,
        })
        .await;
    assert!(
        matches!(result, Ok(crate::provider::SentCopyStep::Done)),
        "explicit override must skip Sent-folder requirements: {result:?}"
    );
    assert_eq!(server.lock().unwrap().commands.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn server_owned_sent_copy_needs_neither_message_id_nor_an_imap_session() {
    for explicit_override in [false, true] {
        let server = server_with_inbox();
        let (mut provider, store) = provider_with(server.clone());
        if explicit_override {
            provider.settings.server_saves_sent = true;
        } else {
            store.set_sent_copy_detection(true, 1).unwrap();
        }
        for phase in [
            crate::provider::SentCopyPhase::Pending,
            crate::provider::SentCopyPhase::Verify,
        ] {
            let result = provider
                .save_sent_copy(&crate::provider::SentCopyRequest {
                    raw: b"Subject: no message id\r\n\r\nbody".to_vec(),
                    outbox_id: "server-owned-sent-copy".into(),
                    append_attempted: false,
                    phase,
                })
                .await;
            assert!(
                matches!(result, Ok(crate::provider::SentCopyStep::Done)),
                "{result:?}"
            );
        }
        assert!(server.lock().unwrap().wire().is_empty());
    }
}
