//! Wire-level tests for the Phase 3 write layer.
//!
//! These drive a REAL [`AsyncImapSession`](super::super::session::AsyncImapSession)
//! (so async-imap's own command formatting and the `imap-proto` response
//! parser run) over a tokio duplex stream against a small scripted server.
//! Everything is exercised end to end with no network: the server understands
//! SELECT (and a `[READ-ONLY]` variant), STORE (and a mode that silently
//! ignores a flag), COPY (tagged `COPYUID`), MOVE (UNTAGGED `COPYUID` + the
//! `* n EXPUNGE` lines), and `UID EXPUNGE`, with configurable CAPABILITY and
//! PERMANENTFLAGS. It also records every write command it saw, so a test can
//! assert that a guarded-but-stale write, or a client-rejected flag/name,
//! reached the wire with NOTHING.
//!
//! The matrix the brief asks for is covered: COPYUID from both shapes; a
//! verified store catching a silent ignore; a stale UIDVALIDITY sending
//! nothing; strategy selection for {MOVE+UIDPLUS, UIDPLUS only, MOVE only,
//! neither}; hostile inputs never reaching the wire; a partial relocate
//! failure reporting the partial state; the typed TRYCREATE outcome; and
//! batching beyond `MAX_UIDS_PER_COMMAND`.

use std::sync::{Arc, Mutex};

use async_imap::Client;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::task::JoinHandle;

use crate::provider::imap::session::AsyncImapSession;
use crate::provider::imap::ImapStateStore;
use crate::provider::ProviderError;

use super::*;

/// What a scripted server should do, built per test. All fields default to a
/// well-behaved MOVE+UIDPLUS server.
#[derive(Clone)]
struct ServerScript {
    /// CAPABILITY words the server advertises (post-login).
    capabilities: Vec<String>,
    /// PERMANENTFLAGS the writable SELECT reports (wire spellings; `\*` means
    /// the server permits new keywords).
    permanent_flags: Vec<String>,
    /// UIDVALIDITY the SELECT reports.
    uidvalidity: u32,
    /// When true, SELECT answers `OK [READ-ONLY]` instead of `[READ-WRITE]`.
    select_read_only: bool,
    /// When set, a STORE that would add this flag is ACKed OK but the flag is
    /// NOT actually recorded, so the follow-up FETCH omits it — the "silent
    /// ignore" the verified store must catch.
    silently_ignore_flag: Option<String>,
    /// When set, COPY answers `NO [TRYCREATE]` with this text.
    copy_trycreate: Option<String>,
    /// When set, the STORE that marks the source `\Deleted` (relocate step 2)
    /// fails with this NO text — to exercise the partial-relocate path.
    fail_deleted_store: Option<String>,
    /// When set, `UID EXPUNGE` fails with this NO text — partial relocate
    /// after COPY + \Deleted both succeeded.
    fail_expunge: Option<String>,
}

impl Default for ServerScript {
    fn default() -> Self {
        Self {
            capabilities: vec![
                "IMAP4REV1".into(),
                "MOVE".into(),
                "UIDPLUS".into(),
                "UNSELECT".into(),
            ],
            permanent_flags: vec!["\\Seen".into(), "\\Flagged".into(), "\\Deleted".into()],
            uidvalidity: 100,
            select_read_only: false,
            silently_ignore_flag: None,
            copy_trycreate: None,
            fail_deleted_store: None,
            fail_expunge: None,
        }
    }
}

/// The record of what the server saw: every raw command line (minus the tag),
/// so a test can assert what did — and did NOT — reach the wire.
#[derive(Clone, Default)]
struct WireLog {
    commands: Arc<Mutex<Vec<String>>>,
}

impl WireLog {
    fn seen(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }
    /// Count of commands whose uppercased text starts with `prefix`.
    fn count_starting(&self, prefix: &str) -> usize {
        self.seen()
            .iter()
            .filter(|c| c.to_ascii_uppercase().starts_with(prefix))
            .count()
    }
}

/// Spawn a scripted server over a duplex stream and return a logged-in
/// [`AsyncImapSession`] plus the wire log and the server task handle.
async fn connect(
    script: ServerScript,
) -> (AsyncImapSession<DuplexStream>, WireLog, JoinHandle<()>) {
    let (client, server) = tokio::io::duplex(1 << 20);
    let log = WireLog::default();
    let server_log = log.clone();
    let task = tokio::spawn(async move {
        serve(server, script, server_log).await;
    });
    let session = Client::new(client)
        .login("user", "password")
        .await
        .expect("scripted login should succeed");
    (AsyncImapSession::new(session), log, task)
}

/// Reply helper.
async fn reply(server: &mut BufReader<DuplexStream>, text: &str) {
    server.get_mut().write_all(text.as_bytes()).await.unwrap();
}

/// Extract the trailing parenthesised flag list of a STORE command.
fn store_flags(raw_cmd: &str) -> Vec<String> {
    if let (Some(open), Some(close)) = (raw_cmd.find('('), raw_cmd.rfind(')')) {
        raw_cmd[open + 1..close]
            .split_whitespace()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    }
}

/// The scripted server loop. Logs each command, then answers per the script.
async fn serve(server: DuplexStream, script: ServerScript, log: WireLog) {
    let mut server = BufReader::new(server);
    reply(&mut server, "* OK ready\r\n").await;
    // Flags currently stored on the (single) message, for the verified store's
    // re-fetch. Only the simple cases the tests use are tracked; FETCH reports
    // exactly these.
    let mut stored_flags: Vec<String> = vec!["\\Seen".into()];
    let mut expunged = false;
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

        // Record every command except LOGIN (credentials) and LOGOUT.
        if !upper.starts_with("LOGIN") && !upper.starts_with("LOGOUT") {
            log.commands.lock().unwrap().push(rest.clone());
        }

        if upper.starts_with("LOGIN") {
            reply(&mut server, &format!("{tag} OK logged in\r\n")).await;
        } else if upper.starts_with("CAPABILITY") {
            reply(
                &mut server,
                &format!(
                    "* CAPABILITY {}\r\n{tag} OK done\r\n",
                    script.capabilities.join(" ")
                ),
            )
            .await;
        } else if upper.starts_with("SELECT") {
            let permanent = script.permanent_flags.join(" ");
            let rw = if script.select_read_only {
                "[READ-ONLY]"
            } else {
                "[READ-WRITE]"
            };
            let body = format!(
                "* 3 EXISTS\r\n\
                 * OK [PERMANENTFLAGS ({permanent})] limited\r\n\
                 * OK [UIDVALIDITY {}] uids valid\r\n\
                 * OK [UIDNEXT 50] next\r\n\
                 {tag} OK {rw} SELECT done\r\n",
                script.uidvalidity
            );
            reply(&mut server, &body).await;
        } else if upper.starts_with("UID STORE") {
            let flags = store_flags(&rest);
            let is_add = upper.contains("+FLAGS");
            if is_add {
                // A scripted \Deleted-store failure (partial-relocate test).
                if script.fail_deleted_store.is_some()
                    && flags.iter().any(|f| f.eq_ignore_ascii_case("\\Deleted"))
                {
                    let text = script.fail_deleted_store.clone().unwrap();
                    reply(&mut server, &format!("{tag} NO {text}\r\n")).await;
                    continue;
                }
                for flag in &flags {
                    let ignored = script
                        .silently_ignore_flag
                        .as_deref()
                        .map(|f| f.eq_ignore_ascii_case(flag))
                        .unwrap_or(false);
                    if ignored {
                        continue; // ACK OK but do not record it.
                    }
                    if !stored_flags.iter().any(|f| f.eq_ignore_ascii_case(flag)) {
                        stored_flags.push(flag.clone());
                    }
                }
            } else {
                stored_flags.retain(|f| !flags.iter().any(|g| g.eq_ignore_ascii_case(f)));
            }
            reply(&mut server, &format!("{tag} OK STORE done\r\n")).await;
        } else if upper.starts_with("UID FETCH") {
            if expunged {
                reply(&mut server, &format!("{tag} OK FETCH done\r\n")).await;
                continue;
            }
            let flags = stored_flags.join(" ");
            reply(
                &mut server,
                &format!("* 1 FETCH (UID 10 FLAGS ({flags}))\r\n{tag} OK FETCH done\r\n"),
            )
            .await;
        } else if upper.starts_with("UID COPY") {
            if let Some(text) = &script.copy_trycreate {
                reply(&mut server, &format!("{tag} NO [TRYCREATE] {text}\r\n")).await;
            } else {
                // Tagged COPYUID (the COPY shape).
                reply(
                    &mut server,
                    &format!("{tag} OK [COPYUID {} 10 90] copied\r\n", script.uidvalidity),
                )
                .await;
            }
        } else if upper.starts_with("UID MOVE") {
            // UNTAGGED COPYUID first, then EXPUNGE lines, then a bare OK.
            reply(
                &mut server,
                &format!(
                    "* OK [COPYUID {} 10 90] moved\r\n* 1 EXPUNGE\r\n{tag} OK MOVE done\r\n",
                    script.uidvalidity
                ),
            )
            .await;
        } else if upper.starts_with("UID EXPUNGE") {
            if let Some(text) = &script.fail_expunge {
                reply(&mut server, &format!("{tag} NO {text}\r\n")).await;
            } else {
                expunged = stored_flags.iter().any(|flag| flag == "\\Deleted");
                reply(
                    &mut server,
                    &format!("* 1 EXPUNGE\r\n{tag} OK EXPUNGE done\r\n"),
                )
                .await;
            }
        } else if upper.starts_with("LOGOUT") {
            reply(&mut server, &format!("* BYE\r\n{tag} OK logout\r\n")).await;
            break;
        } else {
            reply(&mut server, &format!("{tag} OK done\r\n")).await;
        }
    }
}

fn store() -> ImapStateStore {
    ImapStateStore::new(Arc::new(crate::db::Database::open_memory()), "me@example.com")
}

// ---------------------------------------------------------------------------
// select_for_write guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn select_for_write_returns_a_context_when_uidvalidity_matches() {
    let (mut session, _log, task) = connect(ServerScript::default()).await;
    let store = store();
    // The catalog row exists (discovery creates it); persisting capabilities
    // must update only the capability columns, never the sync counters.
    store
        .upsert_mailbox(&crate::provider::imap::ImapMailbox {
            name: "INBOX".into(),
            delimiter: Some("/".into()),
            special_use: None,
            uidvalidity: 100,
            uidnext: 5,
            highestmodseq: None,
            permanent_flags_json: None,
            permanent_keywords: None,
        })
        .unwrap();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100)
        .await
        .expect("matching UIDVALIDITY yields a context");
    assert_eq!(ctx.uidvalidity, 100);
    assert!(ctx.caps.has_move && ctx.caps.has_uidplus);
    assert!(ctx.caps.can_store_flag("\\Seen"));
    // The writable PERMANENTFLAGS were persisted to the catalog.
    let rows = store.mailboxes().unwrap();
    let inbox = rows.iter().find(|m| m.name == "INBOX").unwrap();
    assert!(inbox.permanent_flags_json.as_ref().unwrap().contains("Seen"));
    assert_eq!(inbox.uidnext, 5, "the sync counter is untouched");
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_stale_uidvalidity_refuses_the_guard_and_sends_no_write() {
    let (mut session, log, task) = connect(ServerScript::default()).await;
    let store = store();
    // Expected 999, server reports 100 -> stale.
    let err = select_for_write(&mut session, &store, "INBOX", 999)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::TransientTransport(ref m) if m.contains("UIDVALIDITY")),
        "stale guard is a named transient error: {err:?}"
    );
    // The guard SELECTed and read CAPABILITY only; it never issued a write.
    assert_eq!(log.count_starting("UID STORE"), 0);
    assert_eq!(log.count_starting("UID COPY"), 0);
    assert_eq!(log.count_starting("UID MOVE"), 0);
    assert_eq!(log.count_starting("UID EXPUNGE"), 0);
    let _ = session.logout().await;
    task.await.unwrap();
}

// ---------------------------------------------------------------------------
// verified store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_verified_store_succeeds_when_the_flag_is_actually_set() {
    let (mut session, _log, task) = connect(ServerScript::default()).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    verified_store(&mut session, &ctx, &[10], StoreOp::Add, &["\\Flagged".into()])
        .await
        .expect("the server set \\Flagged and the re-fetch confirmed it");
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_verified_store_catches_a_silently_ignored_flag() {
    let mut script = ServerScript::default();
    // The server ACKs the STORE but never records \Flagged.
    script.silently_ignore_flag = Some("\\Flagged".into());
    let (mut session, _log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let err = verified_store(&mut session, &ctx, &[10], StoreOp::Add, &["\\Flagged".into()])
        .await
        .unwrap_err();
    match err {
        ProviderError::InvalidOperation(m) => {
            assert!(m.contains("silently ignored"), "{m}");
            assert!(m.contains("Flagged"), "{m}");
        }
        other => panic!("expected InvalidOperation naming the flag, got {other:?}"),
    }
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_store_of_a_flag_outside_permanentflags_is_refused_before_the_wire() {
    // PERMANENTFLAGS has no \* and no \Answered; an arbitrary keyword is
    // refused by can_store_flag before any STORE is sent.
    let (mut session, log, task) = connect(ServerScript::default()).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let err = verified_store(&mut session, &ctx, &[10], StoreOp::Add, &["MyLabel".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidOperation(_)));
    assert_eq!(log.count_starting("UID STORE"), 0, "nothing reached the wire");
    let _ = session.logout().await;
    task.await.unwrap();
}

// ---------------------------------------------------------------------------
// COPYUID from both shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn copy_parses_the_tagged_copyuid() {
    let (mut session, _log, task) = connect(ServerScript::default()).await;
    let dest = build_mailbox_arg("Archive").unwrap();
    let outcome = session.uid_copy("10", &dest).await.unwrap();
    let copyuid = outcome.copyuid.expect("a tagged COPYUID");
    assert_eq!(copyuid.uidvalidity, 100);
    assert_eq!(copyuid.source, vec![10]);
    assert_eq!(copyuid.dest, vec![90]);
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn move_parses_the_untagged_copyuid_before_the_expunge_lines() {
    let (mut session, _log, task) = connect(ServerScript::default()).await;
    let dest = build_mailbox_arg("Archive").unwrap();
    let outcome = session.uid_move("10", &dest).await.unwrap();
    let copyuid = outcome.copyuid.expect("an untagged COPYUID");
    assert_eq!(copyuid.uidvalidity, 100);
    assert_eq!(copyuid.source, vec![10]);
    assert_eq!(copyuid.dest, vec![90]);
    let _ = session.logout().await;
    task.await.unwrap();
}

// ---------------------------------------------------------------------------
// relocate: strategy selection + partial failure + TRYCREATE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn relocate_uses_move_when_advertised() {
    let (mut session, log, task) = connect(ServerScript::default()).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let outcome = relocate_uids(&mut session, &ctx, &[10], "Archive")
        .await
        .expect("MOVE relocate");
    assert_eq!(outcome.copyuid.unwrap().dest, vec![90]);
    assert_eq!(log.count_starting("UID MOVE"), 1);
    assert_eq!(log.count_starting("UID COPY"), 0, "MOVE is atomic, no COPY");
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn relocate_uses_copy_delete_expunge_when_only_uidplus() {
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
    let (mut session, log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    assert_eq!(
        ctx.caps.relocation_strategy(),
        RelocationStrategy::CopyDeleteExpunge
    );
    let outcome = relocate_uids(&mut session, &ctx, &[10], "Archive")
        .await
        .expect("copy+delete+expunge relocate");
    assert_eq!(outcome.copyuid.unwrap().dest, vec![90]);
    assert_eq!(log.count_starting("UID COPY"), 1);
    assert_eq!(log.count_starting("UID STORE"), 1, "the \\Deleted store");
    assert_eq!(log.count_starting("UID EXPUNGE"), 1, "targeted expunge only");
    // The expunge named exactly the moved UID set.
    assert!(log
        .seen()
        .iter()
        .any(|c| c.to_ascii_uppercase() == "UID EXPUNGE 10"));
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn relocate_is_unsupported_without_move_or_uidplus_and_sends_nothing() {
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into()]; // neither MOVE nor UIDPLUS
    let (mut session, log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let err = relocate_uids(&mut session, &ctx, &[10], "Archive")
        .await
        .unwrap_err()
        .into_provider_error();
    match err {
        ProviderError::InvalidOperation(m) => assert!(m.contains("EXPUNGE"), "{m}"),
        other => panic!("expected Unsupported InvalidOperation, got {other:?}"),
    }
    assert_eq!(log.count_starting("UID MOVE"), 0);
    assert_eq!(log.count_starting("UID COPY"), 0);
    assert_eq!(log.count_starting("UID EXPUNGE"), 0);
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_partial_relocate_reports_the_message_in_both_mailboxes() {
    // COPY succeeds, but the \Deleted store on the source fails.
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // copy-delete-expunge path
    script.fail_deleted_store = Some("server rejected the store".into());
    let (mut session, log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let err = relocate_uids(&mut session, &ctx, &[10], "Archive")
        .await
        .unwrap_err()
        .into_provider_error();
    match err {
        ProviderError::InvalidOperation(m) => {
            assert!(m.contains("partial relocate"), "{m}");
            assert!(m.contains("BOTH"), "{m}");
            assert!(m.contains("idempotent"), "{m}");
        }
        other => panic!("expected a partial-state InvalidOperation, got {other:?}"),
    }
    // COPY ran, the \Deleted store ran (and failed), but no expunge followed.
    assert_eq!(log.count_starting("UID COPY"), 1);
    assert_eq!(
        log.count_starting("UID EXPUNGE"),
        0,
        "never expunge after a failed delete"
    );
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_partial_relocate_after_a_failed_expunge_reports_the_partial_state() {
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
    script.fail_expunge = Some("expunge refused".into());
    let (mut session, _log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let err = relocate_uids(&mut session, &ctx, &[10], "Archive")
        .await
        .unwrap_err()
        .into_provider_error();
    match err {
        ProviderError::InvalidOperation(m) => {
            assert!(m.contains("EXPUNGE"), "{m}");
            assert!(m.contains("BOTH"), "{m}");
        }
        other => panic!("expected partial-state InvalidOperation, got {other:?}"),
    }
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_copy_trycreate_surfaces_as_the_typed_outcome() {
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // copy path
    script.copy_trycreate = Some("Mailbox doesn't exist: Labels/New".into());
    let (mut session, log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let err = relocate_uids(&mut session, &ctx, &[10], "Labels/New")
        .await
        .unwrap_err();
    match err {
        WriteError::TryCreate(text) => assert!(text.contains("Labels/New"), "{text}"),
        other => panic!("expected a typed TryCreate, got {other:?}"),
    }
    // Nothing destructive ran: the COPY failed, so no \Deleted store, no
    // expunge.
    assert_eq!(log.count_starting("UID STORE"), 0);
    assert_eq!(log.count_starting("UID EXPUNGE"), 0);
    let _ = session.logout().await;
    task.await.unwrap();
}

// ---------------------------------------------------------------------------
// recording relocate (run 3): a partial failure hands back the destination it
// already created, and the source-only finisher completes the retry.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_recording_relocate_hands_back_the_copyuid_when_the_expunge_fails() {
    // UIDPLUS-only (copy+delete+expunge). The expunge fails AFTER the COPY and
    // \Deleted store landed, so the destination holds the copy. The recording
    // relocate must return that destination mapping so the caller can persist
    // it and keep the retry idempotent.
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
    script.fail_expunge = Some("expunge refused".into());
    let (mut session, log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let failure = relocate_uids_recording(&mut session, &ctx, &[10], "Archive")
        .await
        .expect_err("the expunge failure is surfaced");
    // The COPY landed (UID 10 -> 90), so the destination mapping is present.
    let copyuid = failure.copyuid.expect("the copied destination is reported");
    assert_eq!(copyuid.source, vec![10]);
    assert_eq!(copyuid.dest, vec![90]);
    // And the error is the partial-state error with its retry classification.
    match failure.error.into_provider_error() {
        ProviderError::InvalidOperation(m) => {
            assert!(m.contains("BOTH"), "{m}");
            assert!(m.contains("EXPUNGE"), "{m}");
        }
        other => panic!("expected a partial-state InvalidOperation, got {other:?}"),
    }
    assert_eq!(log.count_starting("UID COPY"), 1);
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_recording_relocate_trycreate_reports_no_destination_landed() {
    // The COPY draws a TRYCREATE before anything lands: no destination mapping.
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
    script.copy_trycreate = Some("Mailbox doesn't exist: Archive".into());
    let (mut session, log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    let failure = relocate_uids_recording(&mut session, &ctx, &[10], "Archive")
        .await
        .expect_err("TRYCREATE is surfaced");
    assert!(failure.copyuid.is_none(), "nothing landed in the destination");
    assert!(matches!(failure.error, WriteError::TryCreate(_)));
    assert_eq!(log.count_starting("UID STORE"), 0);
    assert_eq!(log.count_starting("UID EXPUNGE"), 0);
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn expunge_source_copies_marks_deleted_then_expunges_without_copying() {
    // The source-only finisher used on an idempotent retry: it stores \Deleted
    // and UID EXPUNGEs the source UID, and never issues a COPY/MOVE.
    let mut script = ServerScript::default();
    script.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
    let (mut session, log, task) = connect(script).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    expunge_source_copies(&mut session, &ctx, &[10])
        .await
        .expect("the source copy is deleted and expunged");
    assert_eq!(log.count_starting("UID STORE"), 1, "one \\Deleted store");
    assert!(log
        .seen()
        .iter()
        .any(|c| c.to_ascii_uppercase().contains("+FLAGS")
            && c.to_ascii_uppercase().contains("DELETED")));
    assert_eq!(log.count_starting("UID EXPUNGE"), 1, "one targeted expunge");
    assert_eq!(log.count_starting("UID COPY"), 0, "no copy on the finisher");
    assert_eq!(log.count_starting("UID MOVE"), 0, "no move on the finisher");
    let _ = session.logout().await;
    task.await.unwrap();
}

// ---------------------------------------------------------------------------
// batching + hostile inputs never reach the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_store_over_the_uid_batch_limit_is_split_into_several_commands() {
    let (mut session, log, task) = connect(ServerScript::default()).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    // One more than the per-command cap -> two STORE commands (and two FETCH
    // verifications).
    let n = crate::provider::imap::policy::MAX_UIDS_PER_COMMAND as u32 + 1;
    let uids: Vec<u32> = (1..=n).collect();
    verified_store(&mut session, &ctx, &uids, StoreOp::Add, &["\\Flagged".into()])
        .await
        .unwrap();
    assert_eq!(log.count_starting("UID STORE"), 2, "split into two batches");
    assert_eq!(log.count_starting("UID FETCH"), 2, "each batch is verified");
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_hostile_destination_name_never_reaches_the_wire() {
    let (mut session, log, task) = connect(ServerScript::default()).await;
    let store = store();
    let ctx = select_for_write(&mut session, &store, "INBOX", 100).await.unwrap();
    // relocate_uids encodes the destination through build_mailbox_arg, which
    // rejects a CRLF-injection name before any command is built.
    let err = relocate_uids(&mut session, &ctx, &[10], "Archive\r\nA1 DELETE INBOX")
        .await
        .unwrap_err()
        .into_provider_error();
    assert!(matches!(err, ProviderError::InvalidOperation(_)));
    assert_eq!(log.count_starting("UID MOVE"), 0);
    assert_eq!(log.count_starting("UID COPY"), 0);
    // Critically, the injected `DELETE INBOX` never appeared on the wire.
    assert!(log
        .seen()
        .iter()
        .all(|c| !c.to_ascii_uppercase().contains("DELETE INBOX")));
    let _ = session.logout().await;
    task.await.unwrap();
}

#[tokio::test]
async fn a_read_only_select_is_detectable_through_the_response_code_classifier() {
    // Most servers answer NO for a read-only mailbox; some answer OK
    // [READ-ONLY]. async-imap's typed `select()` treats OK [READ-ONLY] as a
    // successful select, so the guard still returns a context, but the SELECT
    // outcome's code is classifiable. This proves the classifier recognises it
    // (the write layer's run-2 caller consults it before storing).
    use async_imap::imap_proto::ResponseCode;
    assert!(super::select_is_read_only(&ResponseCode::ReadOnly));
    assert!(!super::select_is_read_only(&ResponseCode::ReadWrite));
}

// ===========================================================================
// Phase 4 run 4a: the Sent-copy APPEND primitive.
//
// These drive a REAL `AsyncImapSession` over a duplex stream against a scripted
// server that implements a genuine message store: EXAMINE, APPEND (with the
// `+`-continuation literal handshake, storing the bytes and honouring flags and
// assigning UIDs), `UID SEARCH HEADER Message-ID` (SUBSTRING match, as the real
// RFC says), and `UID FETCH ... BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)]`. Each
// failing case makes the server actually DO the failing thing (the lost-reply
// case stores the message THEN drops the connection before the tagged OK) and
// asserts both the outcome AND the number of stored copies, so none passes
// vacuously.
// ===========================================================================
mod sent_copy_tests {
    use super::*;
    use crate::provider::imap::policy;
    use std::time::Duration;

    /// One stored message in the fake Sent mailbox.
    #[derive(Clone)]
    struct Stored {
        uid: u32,
        bytes: Vec<u8>,
        flags: Vec<String>,
    }

    /// The scripted Sent-mailbox server's knobs.
    #[derive(Clone, Default)]
    struct SentScript {
        /// EXAMINE answers `NO [TRYCREATE] <text>` instead of OK.
        examine_trycreate: Option<String>,
        /// EXAMINE answers `NO [NONEXISTENT] <text>`.
        examine_nonexistent: Option<String>,
        /// APPEND answers `NO [OVERQUOTA] <text>` (a definite pre-store NO).
        append_overquota: Option<String>,
        /// After the APPEND literal is received and STORED, drop the connection
        /// WITHOUT sending the tagged OK — the lost-reply case.
        drop_after_store: bool,
        /// Pretend a just-stored message is invisible to SEARCH for this many
        /// SEARCH calls after the store (a server with delayed visibility).
        hide_from_search_for: usize,
    }

    /// Shared mutable state the server task owns.
    #[derive(Clone)]
    struct SentState {
        messages: Arc<Mutex<Vec<Stored>>>,
        next_uid: Arc<Mutex<u32>>,
        uidvalidity: u32,
        /// How many SEARCHes have run since the last store (for delayed
        /// visibility).
        searches_since_store: Arc<Mutex<usize>>,
        /// How many APPEND commands the server fully stored (asserted by the
        /// lost-reply test to prove the failed attempt really stored).
        stored_count: Arc<Mutex<usize>>,
    }

    impl SentState {
        fn new(uidvalidity: u32) -> Self {
            Self {
                messages: Arc::new(Mutex::new(Vec::new())),
                next_uid: Arc::new(Mutex::new(1)),
                uidvalidity,
                searches_since_store: Arc::new(Mutex::new(usize::MAX)),
                stored_count: Arc::new(Mutex::new(0)),
            }
        }
        fn count(&self) -> usize {
            self.messages.lock().unwrap().len()
        }
        fn stored_count(&self) -> usize {
            *self.stored_count.lock().unwrap()
        }
    }

    /// Spawn the scripted Sent server and return a logged-in session, the
    /// shared state, the wire log and the task handle. `preload` seeds messages
    /// already present (for the AlreadyPresent / near-miss tests).
    async fn connect_sent(
        script: SentScript,
        preload: &[(&str, &[&str])],
    ) -> (
        AsyncImapSession<DuplexStream>,
        SentState,
        WireLog,
        JoinHandle<()>,
    ) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let log = WireLog::default();
        let server_log = log.clone();
        let state = SentState::new(777);
        // Seed preloaded messages.
        {
            let mut msgs = state.messages.lock().unwrap();
            let mut next = state.next_uid.lock().unwrap();
            for (body, flags) in preload {
                msgs.push(Stored {
                    uid: *next,
                    bytes: body.as_bytes().to_vec(),
                    flags: flags.iter().map(|f| f.to_string()).collect(),
                });
                *next += 1;
            }
        }
        let server_state = state.clone();
        let task = tokio::spawn(async move {
            serve_sent(server, script, server_state, server_log).await;
        });
        let session = Client::new(client)
            .login("user", "password")
            .await
            .expect("scripted login should succeed");
        (AsyncImapSession::new(session), state, log, task)
    }

    /// Does a stored message's raw bytes carry a Message-ID header whose value
    /// CONTAINS `needle` (the SEARCH HEADER substring semantics)?
    fn message_id_contains(bytes: &[u8], needle: &str) -> bool {
        let text = String::from_utf8_lossy(bytes);
        for line in text.split("\r\n") {
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("message-id") {
                    return value.contains(needle);
                }
            }
        }
        false
    }

    /// Extract the quoted search needle from `HEADER Message-ID "<...>"`.
    fn search_needle(rest: &str) -> Option<String> {
        let (open, close) = (rest.find('"')?, rest.rfind('"')?);
        if close > open {
            Some(rest[open + 1..close].to_string())
        } else {
            None
        }
    }

    /// Extract the literal byte count `{N}` at the end of an APPEND command.
    fn append_literal_len(rest: &str) -> Option<usize> {
        let open = rest.rfind('{')?;
        let close = rest.rfind('}')?;
        rest.get(open + 1..close)?.parse().ok()
    }

    /// The scripted Sent-mailbox server loop.
    async fn serve_sent(
        server: DuplexStream,
        script: SentScript,
        state: SentState,
        log: WireLog,
    ) {
        let mut server = BufReader::new(server);
        reply(&mut server, "* OK ready\r\n").await;
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

            if !upper.starts_with("LOGIN") && !upper.starts_with("LOGOUT") {
                log.commands.lock().unwrap().push(rest.clone());
            }

            if upper.starts_with("LOGIN") {
                reply(&mut server, &format!("{tag} OK logged in\r\n")).await;
            } else if upper.starts_with("CAPABILITY") {
                reply(
                    &mut server,
                    &format!("* CAPABILITY IMAP4REV1 UIDPLUS\r\n{tag} OK done\r\n"),
                )
                .await;
            } else if upper.starts_with("EXAMINE") {
                if let Some(text) = &script.examine_trycreate {
                    reply(&mut server, &format!("{tag} NO [TRYCREATE] {text}\r\n")).await;
                } else if let Some(text) = &script.examine_nonexistent {
                    reply(&mut server, &format!("{tag} NO [NONEXISTENT] {text}\r\n")).await;
                } else {
                    let n = state.messages.lock().unwrap().len();
                    reply(
                        &mut server,
                        &format!(
                            "* {n} EXISTS\r\n\
                             * OK [UIDVALIDITY {}] uids valid\r\n\
                             * OK [UIDNEXT 500] next\r\n\
                             {tag} OK [READ-ONLY] EXAMINE done\r\n",
                            state.uidvalidity
                        ),
                    )
                    .await;
                }
            } else if upper.starts_with("APPEND") {
                let Some(len) = append_literal_len(&rest) else {
                    reply(&mut server, &format!("{tag} BAD malformed APPEND\r\n")).await;
                    continue;
                };
                if let Some(text) = &script.append_overquota {
                    // A definite pre-store NO: nothing is read as a literal.
                    reply(&mut server, &format!("{tag} NO [OVERQUOTA] {text}\r\n")).await;
                    continue;
                }
                // Continuation, then read exactly `len` + 2 (CRLF) bytes.
                reply(&mut server, "+ OK send literal\r\n").await;
                let mut buf = vec![0u8; len + 2];
                use tokio::io::AsyncReadExt;
                if server.read_exact(&mut buf).await.is_err() {
                    break;
                }
                buf.truncate(len); // drop the trailing CRLF the client adds.
                // Parse the flag list from the command (between the first '('
                // and the matching ')').
                let flags = store_flags(&rest);
                let uid = {
                    let mut next = state.next_uid.lock().unwrap();
                    let u = *next;
                    *next += 1;
                    u
                };
                state.messages.lock().unwrap().push(Stored {
                    uid,
                    bytes: buf,
                    flags,
                });
                *state.stored_count.lock().unwrap() += 1;
                *state.searches_since_store.lock().unwrap() = 0;
                if script.drop_after_store {
                    // Stored, but the tagged OK never arrives: the client sees
                    // the connection drop (a transient-class error).
                    break;
                }
                reply(&mut server, &format!("{tag} OK [APPENDUID {} {uid}] appended\r\n", state.uidvalidity)).await;
            } else if upper.starts_with("UID SEARCH") {
                let needle = search_needle(&rest).unwrap_or_default();
                let hidden = {
                    let mut s = state.searches_since_store.lock().unwrap();
                    let hide = *s < script.hide_from_search_for;
                    *s = s.saturating_add(1);
                    hide
                };
                let mut uids: Vec<u32> = if hidden {
                    Vec::new()
                } else {
                    state
                        .messages
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|m| message_id_contains(&m.bytes, &needle))
                        .map(|m| m.uid)
                        .collect()
                };
                uids.sort_unstable();
                let list = uids
                    .iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                reply(
                    &mut server,
                    &format!("* SEARCH {list}\r\n{tag} OK SEARCH done\r\n"),
                )
                .await;
            } else if upper.starts_with("UID FETCH") {
                // Serve BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)] for the named UID.
                // Parse the leading UID (the set is a single uid in this path).
                let uid: Option<u32> = rest
                    .split_whitespace()
                    .nth(2)
                    .and_then(|s| s.split(':').next())
                    .and_then(|s| s.parse().ok());
                // Extract an OWNED header string, then DROP the guard before any
                // await (a MutexGuard is not Send across .await).
                let header = uid.and_then(|uid| {
                    let msgs = state.messages.lock().unwrap();
                    msgs.iter()
                        .find(|m| m.uid == uid)
                        .map(|m| extract_message_id_header(&m.bytes))
                });
                if let (Some(uid), Some(header)) = (uid, header) {
                    let octets = header.len();
                    reply(
                        &mut server,
                        &format!(
                            "* 1 FETCH (UID {uid} BODY[HEADER.FIELDS (MESSAGE-ID)] {{{octets}}}\r\n{header})\r\n{tag} OK FETCH done\r\n",
                        ),
                    )
                    .await;
                    continue;
                }
                reply(&mut server, &format!("{tag} OK FETCH done\r\n")).await;
            } else if upper.starts_with("LOGOUT") {
                reply(&mut server, &format!("* BYE\r\n{tag} OK logout\r\n")).await;
                break;
            } else {
                reply(&mut server, &format!("{tag} OK done\r\n")).await;
            }
        }
    }

    /// The `Message-ID:` header line (with its CRLF and the trailing blank
    /// line a HEADER.FIELDS section carries), or just a blank line if absent.
    fn extract_message_id_header(bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        for line in text.split("\r\n") {
            if let Some((name, _)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("message-id") {
                    return format!("{line}\r\n\r\n");
                }
            }
        }
        "\r\n".to_string()
    }

    fn instant_config(rechecks: usize) -> SaveSentConfig {
        SaveSentConfig {
            relocate_rechecks: rechecks,
            recheck_delay: Duration::ZERO,
        }
    }

    /// A raw message with the given Message-ID, a Bcc header, a non-ASCII body
    /// line, and a line that is a single `.` (dot-stuffing edge).
    fn raw_with(message_id: &str) -> Vec<u8> {
        format!(
            "From: me@proton.me\r\nTo: a@x.test\r\nBcc: blind@z.test\r\nMessage-ID: {message_id}\r\nSubject: Grüße\r\n\r\nhello\r\n.\r\nafter the dot\r\n"
        )
        .into_bytes()
    }

    // ---- happy path -------------------------------------------------------

    #[tokio::test]
    async fn append_then_located_returns_appended_with_byte_identical_seen_only_copy() {
        let (mut session, state, log, task) =
            connect_sent(SentScript::default(), &[]).await;
        let raw = raw_with("<abc@proton.me>");
        let outcome = save_sent_copy(&mut session, "Sent", &raw, "<abc@proton.me>", instant_config(2))
            .await
            .expect("append + locate");
        match outcome {
            SentCopy::Appended { uidvalidity, uid } => {
                assert_eq!(uidvalidity, 777);
                assert!(uid >= 1);
            }
            other => panic!("expected Appended, got {other:?}"),
        }
        // Exactly one stored copy, byte-identical to raw (incl. the Bcc header,
        // the non-ASCII subject, CRLF, and the single-dot line).
        let msgs = state.messages.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].bytes, raw, "stored bytes are byte-identical to raw");
        assert!(
            String::from_utf8_lossy(&msgs[0].bytes).contains("Bcc: blind@z.test"),
            "the Sent copy KEEPS its Bcc header"
        );
        // Flags = \Seen only.
        assert_eq!(msgs[0].flags, vec!["\\Seen".to_string()]);
        // One APPEND reached the wire.
        assert_eq!(log.count_starting("APPEND"), 1);
        drop(msgs);
        let _ = session.logout().await;
        task.await.unwrap();
    }

    // ---- second call is idempotent ---------------------------------------

    #[tokio::test]
    async fn a_second_call_returns_already_present_and_appends_nothing() {
        // Pre-seed the Sent mailbox with the SAME Message-ID already present.
        let raw = raw_with("<dup@proton.me>");
        let seeded = String::from_utf8(raw.clone()).unwrap();
        let seeded_leak: &'static str = Box::leak(seeded.into_boxed_str());
        let (mut session, state, log, task) =
            connect_sent(SentScript::default(), &[(seeded_leak, &["\\Seen"])]).await;
        let outcome = save_sent_copy(&mut session, "Sent", &raw, "<dup@proton.me>", instant_config(2))
            .await
            .expect("already present");
        match outcome {
            SentCopy::AlreadyPresent { uids, .. } => assert_eq!(uids.len(), 1),
            other => panic!("expected AlreadyPresent, got {other:?}"),
        }
        assert_eq!(state.count(), 1, "still exactly one copy");
        assert_eq!(log.count_starting("APPEND"), 0, "nothing appended on the second call");
        let _ = session.logout().await;
        task.await.unwrap();
    }

    // ---- the lost-reply case (the core idempotency proof) ----------------

    #[tokio::test]
    async fn a_lost_reply_then_retry_leaves_exactly_one_copy() {
        // First attempt: the server STORES the message, then drops the
        // connection before the tagged OK — the client sees a transient error.
        let raw = raw_with("<lost@proton.me>");
        let (mut session, state, _log, task) =
            connect_sent(SentScript { drop_after_store: true, ..Default::default() }, &[]).await;
        let err = save_sent_copy(&mut session, "Sent", &raw, "<lost@proton.me>", instant_config(1))
            .await
            .expect_err("the dropped connection is surfaced");
        // The message was REALLY stored by the failed attempt (otherwise this
        // test would prove nothing).
        assert_eq!(state.stored_count(), 1, "the failed attempt actually stored the message");
        assert_eq!(state.count(), 1);
        // The error keeps a transient (retryable) class — NOT a permanent
        // InvalidOperation that would strand the message.
        match err.into_provider_error() {
            ProviderError::TransientTransport(_) => {}
            other => panic!("a dropped connection must be transient, got {other:?}"),
        }
        task.await.unwrap();

        // The retry runs over a FRESH connection against the SAME server state
        // (the stored message persists). Its step 2 finds the copy ->
        // AlreadyPresent, and still exactly one copy on the server.
        let (client, server) = tokio::io::duplex(1 << 20);
        let log2 = WireLog::default();
        let server_log = log2.clone();
        let state2 = state.clone();
        let task2 = tokio::spawn(async move {
            serve_sent(server, SentScript::default(), state2, server_log).await;
        });
        let mut session2 = AsyncImapSession::new(
            Client::new(client).login("user", "password").await.unwrap(),
        );
        let retry = save_sent_copy(&mut session2, "Sent", &raw, "<lost@proton.me>", instant_config(1))
            .await
            .expect("the retry finds the already-stored copy");
        match retry {
            SentCopy::AlreadyPresent { uids, .. } => assert_eq!(uids.len(), 1),
            other => panic!("expected AlreadyPresent on retry, got {other:?}"),
        }
        assert_eq!(state.count(), 1, "exactly ONE copy after the retry — no duplicate");
        assert_eq!(log2.count_starting("APPEND"), 0, "the retry appends nothing");
        let _ = session2.logout().await;
        task2.await.unwrap();
    }

    // ---- a definite server NO before storing -----------------------------

    #[tokio::test]
    async fn an_overquota_no_is_permanent_and_stores_nothing() {
        let raw = raw_with("<oq@proton.me>");
        let (mut session, state, _log, task) = connect_sent(
            SentScript { append_overquota: Some("mailbox full".into()), ..Default::default() },
            &[],
        )
        .await;
        let err = save_sent_copy(&mut session, "Sent", &raw, "<oq@proton.me>", instant_config(1))
            .await
            .expect_err("OVERQUOTA is a definite NO");
        match err.into_provider_error() {
            ProviderError::PermanentClientRejection(_) => {}
            other => panic!("expected a permanent OVERQUOTA rejection, got {other:?}"),
        }
        assert_eq!(state.count(), 0, "nothing stored on a pre-store NO");
        let _ = session.logout().await;
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_trycreate_examine_is_a_typed_missing_mailbox_error_and_creates_nothing() {
        for script in [
            SentScript { examine_trycreate: Some("Mailbox doesn't exist: Sent".into()), ..Default::default() },
            SentScript { examine_nonexistent: Some("Unknown mailbox: Sent".into()), ..Default::default() },
        ] {
            let raw = raw_with("<nm@proton.me>");
            let (mut session, state, log, task) = connect_sent(script, &[]).await;
            let err = save_sent_copy(&mut session, "Sent", &raw, "<nm@proton.me>", instant_config(1))
                .await
                .expect_err("a missing Sent mailbox is an error");
            match err.into_provider_error() {
                ProviderError::InvalidOperation(m) => assert!(m.contains("Sent"), "{m}"),
                other => panic!("expected a permanent missing-mailbox error, got {other:?}"),
            }
            assert_eq!(state.count(), 0);
            assert_eq!(log.count_starting("APPEND"), 0, "no APPEND, no CREATE");
            assert_eq!(log.count_starting("CREATE"), 0, "this primitive never creates the mailbox");
            let _ = session.logout().await;
            task.await.unwrap();
        }
    }

    // ---- APPEND OK but SEARCH never finds it -> AppendedUnlocated ---------

    #[tokio::test]
    async fn append_ok_but_never_located_is_appended_unlocated() {
        // The server stores the message but hides it from SEARCH for more
        // re-checks than we allow, so the locate never succeeds.
        let raw = raw_with("<unloc@proton.me>");
        let (mut session, state, log, task) = connect_sent(
            SentScript { hide_from_search_for: 999, ..Default::default() },
            &[],
        )
        .await;
        let outcome = save_sent_copy(&mut session, "Sent", &raw, "<unloc@proton.me>", instant_config(2))
            .await
            .expect("stored but unlocatable is still success");
        match outcome {
            SentCopy::AppendedUnlocated { uidvalidity } => assert_eq!(uidvalidity, 777),
            other => panic!("expected AppendedUnlocated, got {other:?}"),
        }
        assert_eq!(state.count(), 1, "the message IS stored");
        assert_eq!(log.count_starting("APPEND"), 1, "appended exactly once");
        let _ = session.logout().await;
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_delayed_visibility_server_locates_within_the_rechecks() {
        // Hidden for the first SEARCH (the pre-APPEND existence check and the
        // first post-APPEND check), then visible — so a re-check finds it and
        // we get Appended, having appended exactly once (not twice).
        let raw = raw_with("<delay@proton.me>");
        let (mut session, state, log, task) = connect_sent(
            SentScript { hide_from_search_for: 1, ..Default::default() },
            &[],
        )
        .await;
        let outcome = save_sent_copy(&mut session, "Sent", &raw, "<delay@proton.me>", instant_config(3))
            .await
            .expect("located within the re-checks");
        match outcome {
            SentCopy::Appended { .. } => {}
            other => panic!("expected Appended after a re-check, got {other:?}"),
        }
        assert_eq!(state.count(), 1, "appended exactly once despite the delay");
        assert_eq!(log.count_starting("APPEND"), 1);
        let _ = session.logout().await;
        task.await.unwrap();
    }

    // ---- near-miss Message-IDs -------------------------------------------

    #[tokio::test]
    async fn a_near_miss_message_id_does_not_count_as_present() {
        // The mailbox holds <xabc@x> and <abc@xy>; searching for <abc@x> must
        // NOT match either (SEARCH HEADER is substring, but the verify step
        // compares the normalized token EXACTLY), so the primitive APPENDs the
        // real <abc@x> rather than reporting AlreadyPresent.
        let m1 = String::from_utf8(raw_with("<xabc@x>")).unwrap();
        let m2 = String::from_utf8(raw_with("<abc@xy>")).unwrap();
        let (mut session, state, log, task) = connect_sent(
            SentScript::default(),
            &[
                (Box::leak(m1.into_boxed_str()), &["\\Seen"]),
                (Box::leak(m2.into_boxed_str()), &["\\Seen"]),
            ],
        )
        .await;
        let raw = raw_with("<abc@x>");
        let outcome = save_sent_copy(&mut session, "Sent", &raw, "<abc@x>", instant_config(2))
            .await
            .expect("near-misses are not matches");
        // It APPENDED (did not treat a near-miss as already-present) and then
        // located its own exact copy.
        match outcome {
            SentCopy::Appended { .. } => {}
            other => panic!("a near-miss must not short-circuit to AlreadyPresent: {other:?}"),
        }
        assert_eq!(state.count(), 3, "the two near-misses plus the new exact copy");
        assert_eq!(log.count_starting("APPEND"), 1);
        let _ = session.logout().await;
        task.await.unwrap();
    }

    // ---- hostile / malformed inputs refused with ZERO commands -----------

    #[tokio::test]
    async fn a_hostile_message_id_is_refused_before_any_command() {
        for hostile in ["<a\"b@x>", "<a\\b@x>", "<a b@x>", "<a\r\nb@x>", "<a\0b@x>"] {
            let (mut session, state, log, task) = connect_sent(SentScript::default(), &[]).await;
            let raw = raw_with("<ok@x>");
            let err = save_sent_copy(&mut session, "Sent", &raw, hostile, instant_config(1))
                .await
                .unwrap_err();
            assert!(matches!(err.into_provider_error(), ProviderError::InvalidOperation(_)));
            assert_eq!(state.count(), 0);
            // Nothing reached the wire: not even an EXAMINE.
            assert_eq!(log.count_starting("EXAMINE"), 0, "hostile id refused before any command");
            assert_eq!(log.count_starting("APPEND"), 0);
            let _ = session.logout().await;
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn empty_nul_and_oversize_content_are_refused_before_any_command() {
        // Empty.
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (Vec::new(), "empty"),
            (b"From: a\r\nMessage-ID: <x@x>\r\n\r\nhas a \0 nul".to_vec(), "nul"),
            (vec![b'x'; policy::APPEND_MAX_BYTES + 1], "oversize"),
        ];
        for (content, label) in cases {
            let (mut session, state, log, task) = connect_sent(SentScript::default(), &[]).await;
            let err = save_sent_copy(&mut session, "Sent", &content, "<x@x>", instant_config(1))
                .await
                .unwrap_err();
            match err.into_provider_error() {
                ProviderError::InvalidOperation(_) | ProviderError::PermanentClientRejection(_) => {}
                other => panic!("{label}: expected a typed rejection, got {other:?}"),
            }
            assert_eq!(state.count(), 0, "{label}: nothing stored");
            assert_eq!(log.count_starting("APPEND"), 0, "{label}: no APPEND reached the wire");
            assert_eq!(log.count_starting("EXAMINE"), 0, "{label}: refused before any command");
            let _ = session.logout().await;
            task.await.unwrap();
        }
    }

    // ---- a non-ASCII Sent mailbox name round-trips as modified UTF-7 ------

    #[tokio::test]
    async fn a_non_ascii_sent_mailbox_name_is_appended_as_modified_utf7() {
        // The Sent mailbox is "Gesendet/Entwürfe" (ü). The APPEND command on
        // the wire must carry the modified-UTF-7 encoding, not raw UTF-8.
        let (mut session, state, log, task) = connect_sent(SentScript::default(), &[]).await;
        let raw = raw_with("<nonascii@x>");
        let mailbox = "Gesendet/Entw\u{00fc}rfe";
        let outcome = save_sent_copy(&mut session, mailbox, &raw, "<nonascii@x>", instant_config(2))
            .await
            .expect("append to a non-ASCII mailbox");
        assert!(matches!(outcome, SentCopy::Appended { .. }));
        assert_eq!(state.count(), 1);
        // The modified-UTF-7 encoding of "Gesendet/Entwürfe": ü -> &APw-.
        let expected_wire = super::encode_modified_utf7(mailbox);
        assert_eq!(expected_wire, "Gesendet/Entw&APw-rfe");
        let saw_encoded = log
            .seen()
            .iter()
            .any(|c| c.to_ascii_uppercase().starts_with("APPEND") && c.contains(&expected_wire));
        assert!(saw_encoded, "APPEND must carry the modified-UTF-7 name: {:?}", log.seen());
        // And NOT the raw UTF-8 ü in ANY command (EXAMINE included — EXAMINE
        // must also use the modified-UTF-7 name, not the decoded one).
        let saw_raw_utf8 = log.seen().iter().any(|c| c.contains('\u{00fc}'));
        assert!(!saw_raw_utf8, "the raw UTF-8 name must never reach the wire");
        let _ = session.logout().await;
        task.await.unwrap();
    }

    // ---- object-safety / both real impls compile -------------------------

    #[test]
    fn the_write_trait_is_object_safe_with_append() {
        // If `append` broke object safety, `&mut dyn ImapWriteSession` would
        // not type-check; `save_sent_copy` takes exactly that, so this
        // compiling is the proof. A pointer-cast to the trait object pins it.
        fn _assert_object_safe(s: &mut dyn ImapWriteSession) -> &mut dyn ImapWriteSession {
            s
        }
    }
}

// ---------------------------------------------------------------------------
// Live tests against the Dovecot harness (GATED).
// ---------------------------------------------------------------------------
//
// These exercise the write layer end to end against a real IMAP server, but
// the orchestrator — not this environment — runs docker, so they are gated and
// SKIP CLEANLY when the harness is absent. They require BOTH:
//   * `THREESTRANDS_IMAP_IT=1` (the integration-test opt-in the brief names), and
//   * `DOVECOT_TEST_FP=<64-hex SHA-256>` (the live container's pinned cert
//     fingerprint, read at runtime like the Slice 1/2/3 live tests).
// Run after `scripts/dovecot-test-server.sh up`:
//   THREESTRANDS_IMAP_IT=1 DOVECOT_TEST_FP="$(scripts/dovecot-test-server.sh fingerprint)" \
//     cargo test --lib provider::imap::write::wire_tests::live_ -- --nocapture
//
// They create their OWN throwaway mailboxes (`P3R1-*`) and act on their OWN
// freshly-APPENDed messages, so they never disturb the seeded fixtures other
// live tests assert on; each test cleans up the mailboxes it made. No
// `openssl s_client` / `/dev/tcp` probes anywhere.
#[cfg(test)]
mod live {
    use super::*;
    use crate::provider::imap::connection::{
        ConnectionConfig, ConnectionRole, ImapConnectionManager, TlsMode,
    };
    use crate::provider::imap::session::ImapSession;

    /// Returns a connected, write-capable session + its lease, or `None` (with
    /// a printed reason) when the harness is not present — so the default
    /// `cargo test` skips cleanly.
    async fn live_session() -> Option<(
        crate::provider::imap::connection::ConnectedSession,
        crate::provider::imap::connection::ConnectionLease,
    )> {
        if std::env::var("THREESTRANDS_IMAP_IT").ok().as_deref() != Some("1") {
            eprintln!("skipping: THREESTRANDS_IMAP_IT != 1 (live IMAP tests opt-in)");
            return None;
        }
        let Ok(fp_str) = std::env::var("DOVECOT_TEST_FP") else {
            eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
            return None;
        };
        let fp = crate::provider::imap::tls::parse_sha256_fingerprint(&fp_str)
            .expect("DOVECOT_TEST_FP must be a 64-hex-digit SHA-256 fingerprint");
        let cfg = ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 11143,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(fp),
        };
        let mgr = ImapConnectionManager::new(cfg);
        let (session, lease) = mgr
            .connect_leased(ConnectionRole::Command, "test@threestrands.test", "testpassword")
            .await
            .expect("live connect should succeed");
        Some((session, lease))
    }

    /// Delete scratch mailboxes. A mailbox cannot be deleted while it is the
    /// selected one on this connection, so move the selection to INBOX first;
    /// a failure is loud, since a leaked mailbox pollutes the other live
    /// suites that enumerate the account.
    async fn drop_scratch(
        session: &mut crate::provider::imap::connection::ConnectedSession,
        mailboxes: &[&str],
    ) {
        session.select("INBOX").await.expect("select INBOX before cleanup");
        for mailbox in mailboxes {
            session
                .run_command_capture_code(&format!("DELETE {}", build_mailbox_arg(mailbox).unwrap()))
                .await
                .unwrap_or_else(|e| panic!("cleanup DELETE {mailbox} failed: {e:?}"));
        }
    }

    /// Create `mailbox` and COPY one message into it from the harness's seeded
    /// INBOX (a COPY neither alters INBOX nor sets `\\Seen`). A raw `APPEND`
    /// literal cannot be sent through `run_command_capture_code` (no
    /// continuation handling), and a half-sent literal desynchronises the
    /// connection, so seeding goes through the audited copy primitive instead.
    async fn seed_message(
        session: &mut crate::provider::imap::connection::ConnectedSession,
        mailbox: &str,
    ) {
        // Self-heal a leftover from an aborted earlier run, then start empty.
        let _ = session.select("INBOX").await;
        let _ = session
            .run_command_capture_code(&format!("DELETE {}", build_mailbox_arg(mailbox).unwrap()))
            .await;
        session.create_mailbox(mailbox).await.expect("create scratch mailbox");
        session.select("INBOX").await.expect("select INBOX");
        let inbox_uids = session.uid_search("ALL").await.expect("search INBOX");
        let first = inbox_uids.first().expect("the harness seeds INBOX with mail");
        let dest = build_mailbox_arg(mailbox).unwrap();
        session
            .uid_copy(&first.to_string(), &dest)
            .await
            .expect("seed copy into the scratch mailbox");
    }

    #[tokio::test]
    async fn live_store_seen_is_verified() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let store = ImapStateStore::new(
            std::sync::Arc::new(crate::db::Database::open_memory()),
            "test@threestrands.test",
        );
        let mailbox = "P3R1-Store";
        seed_message(&mut session, mailbox).await;
        let status = session.select(mailbox).await.unwrap();
        let uidvalidity = status.uid_validity.unwrap();
        let uids = session.uid_search("ALL").await.unwrap();
        let ctx = select_for_write(&mut session, &store, mailbox, uidvalidity)
            .await
            .unwrap();
        verified_store(&mut session, &ctx, &uids, StoreOp::Add, &["\\Seen".into()])
            .await
            .expect("storing \\Seen is verified against the live server");
        drop_scratch(&mut session, &[mailbox]).await;
        let _ = session.logout().await;
    }

    #[tokio::test]
    async fn live_arbitrary_keyword_without_wildcard_is_handled() {
        // The Dovecot container advertises `\*` (per the spike findings), so an
        // arbitrary keyword IS storable there; assert the verified store
        // SUCCEEDS and the re-fetch confirms it. On a server WITHOUT `\*`,
        // can_store_flag would refuse before the wire — both are correct; the
        // test asserts the behaviour matches the advertised capability.
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let store = ImapStateStore::new(
            std::sync::Arc::new(crate::db::Database::open_memory()),
            "test@threestrands.test",
        );
        let mailbox = "P3R1-Keyword";
        seed_message(&mut session, mailbox).await;
        let status = session.select(mailbox).await.unwrap();
        let uidvalidity = status.uid_validity.unwrap();
        let uids = session.uid_search("ALL").await.unwrap();
        let ctx = select_for_write(&mut session, &store, mailbox, uidvalidity)
            .await
            .unwrap();
        let result =
            verified_store(&mut session, &ctx, &uids, StoreOp::Add, &["P3R1Label".into()]).await;
        if ctx.caps.permits_new_keywords {
            result.expect("a \\*-capable server stores the keyword and the re-fetch confirms it");
        } else {
            assert!(result.is_err(), "without \\* an arbitrary keyword is refused");
        }
        drop_scratch(&mut session, &[mailbox]).await;
        let _ = session.logout().await;
    }

    #[tokio::test]
    async fn live_move_to_archive_returns_copyuid_and_removes_the_source() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let store = ImapStateStore::new(
            std::sync::Arc::new(crate::db::Database::open_memory()),
            "test@threestrands.test",
        );
        let source = "P3R1-MoveSrc";
        let dest = "P3R1-MoveDst";
        let _ = session.select("INBOX").await;
        let _ = session
            .run_command_capture_code(&format!("DELETE {}", build_mailbox_arg(dest).unwrap()))
            .await;
        session.create_mailbox(dest).await.expect("create destination");
        seed_message(&mut session, source).await;
        let status = session.select(source).await.unwrap();
        let uidvalidity = status.uid_validity.unwrap();
        let uids = session.uid_search("ALL").await.unwrap();
        let ctx = select_for_write(&mut session, &store, source, uidvalidity)
            .await
            .unwrap();
        let outcome = relocate_uids(&mut session, &ctx, &uids, dest)
            .await
            .expect("relocate to the destination");
        // On a UIDPLUS server a COPYUID is returned.
        if ctx.caps.has_uidplus {
            let copyuid = outcome.copyuid.as_ref().expect("UIDPLUS server returns COPYUID");
            assert_eq!(copyuid.source, uids, "COPYUID names exactly the moved source UIDs");
            // The mapped destination UIDs are really present in the destination.
            session.select(dest).await.unwrap();
            let in_dest = session.uid_search("ALL").await.unwrap();
            assert_eq!(in_dest, copyuid.dest, "COPYUID dest UIDs match the real destination");
        }
        // The source UID is gone.
        let after = session.select(source).await.unwrap();
        let remaining = session.uid_search("ALL").await.unwrap();
        assert!(
            remaining.is_empty() || after.exists == 0,
            "the moved message no longer exists in the source"
        );
        drop_scratch(&mut session, &[source, dest]).await;
        let _ = session.logout().await;
    }

    #[tokio::test]
    async fn live_uidvalidity_guard_with_a_wrong_value_sends_no_write() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let store = ImapStateStore::new(
            std::sync::Arc::new(crate::db::Database::open_memory()),
            "test@threestrands.test",
        );
        let mailbox = "P3R1-Guard";
        seed_message(&mut session, mailbox).await;
        // A deliberately wrong expected UIDVALIDITY must refuse the guard.
        let err = select_for_write(&mut session, &store, mailbox, 1)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::TransientTransport(_)));
        drop_scratch(&mut session, &[mailbox]).await;
        let _ = session.logout().await;
    }

    /// Live round trip of the exact MOVE/COPY/EXPUNGE sequence
    /// `ImapProvider::modify_thread` emits for archive/trash/spam and their
    /// reverses: relocate a message from a scratch source into a scratch
    /// destination, then relocate it back. Uses the recording relocate so the
    /// crash-honest path runs against the real server. Only scratch mailboxes
    /// and a COPY of a seeded INBOX message are touched; every message and
    /// mailbox created is removed, so the seeded fixtures are never disturbed.
    #[tokio::test]
    async fn live_move_round_trip_both_directions() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let store = ImapStateStore::new(
            std::sync::Arc::new(crate::db::Database::open_memory()),
            "test@threestrands.test",
        );
        let a = "P3R1-RtA";
        let b = "P3R1-RtB";
        // Seed a copy of a seeded INBOX message into scratch A (never touches
        // the seeded message itself), and create empty scratch B.
        seed_message(&mut session, a).await;
        let _ = session.select("INBOX").await;
        let _ = session
            .run_command_capture_code(&format!("DELETE {}", build_mailbox_arg(b).unwrap()))
            .await;
        session.create_mailbox(b).await.expect("create scratch B");

        // Forward: A -> B.
        let status = session.select(a).await.unwrap();
        let uidvalidity = status.uid_validity.unwrap();
        let uids = session.uid_search("ALL").await.unwrap();
        assert_eq!(uids.len(), 1, "scratch A seeded with exactly one message");
        let ctx = select_for_write(&mut session, &store, a, uidvalidity).await.unwrap();
        let fwd = relocate_uids_recording(&mut session, &ctx, &uids, b)
            .await
            .expect("forward relocate A -> B");
        // The source is now empty and B holds exactly one copy.
        session.select(a).await.unwrap();
        assert!(session.uid_search("ALL").await.unwrap().is_empty(), "A emptied");
        let b_status = session.select(b).await.unwrap();
        let b_uids = session.uid_search("ALL").await.unwrap();
        assert_eq!(b_uids.len(), 1, "exactly one copy landed in B (no duplicate)");
        if let Some(copyuid) = &fwd.copyuid {
            assert_eq!(copyuid.dest, b_uids, "COPYUID dest matches the real destination");
        }

        // Reverse: B -> A (the reverse direction archive/unarchive uses).
        let b_uidvalidity = b_status.uid_validity.unwrap();
        let ctx = select_for_write(&mut session, &store, b, b_uidvalidity).await.unwrap();
        relocate_uids_recording(&mut session, &ctx, &b_uids, a)
            .await
            .expect("reverse relocate B -> A");
        session.select(b).await.unwrap();
        assert!(session.uid_search("ALL").await.unwrap().is_empty(), "B emptied");
        session.select(a).await.unwrap();
        assert_eq!(session.uid_search("ALL").await.unwrap().len(), 1, "exactly one copy back in A");

        drop_scratch(&mut session, &[a, b]).await;
        let _ = session.logout().await;
    }

    /// A move must not set `\Seen` on the message it relocates: the harness's
    /// PERMANENTFLAGS include `\Seen`, so a careless implementation could. COPY
    /// preserves flags; the relocate adds only `\Deleted` to the SOURCE copy
    /// (then expunges it). Assert the destination copy's flags carry no
    /// `\Seen` that the source copy did not already have.
    #[tokio::test]
    async fn live_move_does_not_set_seen_on_the_moved_message() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let store = ImapStateStore::new(
            std::sync::Arc::new(crate::db::Database::open_memory()),
            "test@threestrands.test",
        );
        let a = "P3R1-SeenA";
        let b = "P3R1-SeenB";
        seed_message(&mut session, a).await;
        let _ = session.select("INBOX").await;
        let _ = session
            .run_command_capture_code(&format!("DELETE {}", build_mailbox_arg(b).unwrap()))
            .await;
        session.create_mailbox(b).await.expect("create scratch B");

        // The seeded copy in A must not already be \Seen (a COPY of an unseen
        // INBOX message is unseen).
        let status = session.select(a).await.unwrap();
        let uidvalidity = status.uid_validity.unwrap();
        let uids = session.uid_search("ALL").await.unwrap();
        let before = session.uid_fetch(&uids[0].to_string(), "FLAGS").await.unwrap();
        let source_seen = before.iter().any(|m| {
            m.flags().any(|f| matches!(f, async_imap::types::Flag::Seen))
        });

        let ctx = select_for_write(&mut session, &store, a, uidvalidity).await.unwrap();
        relocate_uids_recording(&mut session, &ctx, &uids, b)
            .await
            .expect("relocate A -> B");

        // The destination copy's \Seen state must equal the source's (the move
        // never SET it).
        session.select(b).await.unwrap();
        let b_uids = session.uid_search("ALL").await.unwrap();
        let after = session.uid_fetch(&b_uids[0].to_string(), "FLAGS").await.unwrap();
        let dest_seen = after.iter().any(|m| {
            m.flags().any(|f| matches!(f, async_imap::types::Flag::Seen))
        });
        assert_eq!(dest_seen, source_seen, "the move must not change \\Seen");

        drop_scratch(&mut session, &[a, b]).await;
        let _ = session.logout().await;
    }

    #[tokio::test]
    async fn live_a_hostile_mailbox_name_is_rejected_client_side() {
        // No session needed: the rejection happens before any command is sent.
        // Still gated so it only runs in the live lane for parity.
        if std::env::var("THREESTRANDS_IMAP_IT").ok().as_deref() != Some("1") {
            eprintln!("skipping: THREESTRANDS_IMAP_IT != 1");
            return;
        }
        assert!(build_mailbox_arg("Archive\r\nA1 DELETE INBOX").is_err());
    }

    // -------------------------------------------------------------------
    // Phase 4 run 4a: the Sent-copy APPEND primitive, against live Dovecot.
    // These APPEND into a SCRATCH mailbox (never Sent, never INBOX), locate by
    // Message-ID, verify \Seen + byte-identical content via FETCH BODY.PEEK[],
    // a second call -> AlreadyPresent, a near-miss message not matched, and a
    // non-ASCII scratch name. Each cleans up and leaves the seeded fixtures
    // untouched. The orchestrator runs these; this worker cannot run docker.
    // -------------------------------------------------------------------

    use crate::provider::imap::write::{save_sent_copy, SaveSentConfig, SentCopy};

    /// Create an EMPTY scratch mailbox for an APPEND test (unlike
    /// `seed_message`, this does NOT copy a seed in — APPEND provides the
    /// content). Self-heals a leftover from an aborted run first.
    async fn create_empty_scratch(
        session: &mut crate::provider::imap::connection::ConnectedSession,
        mailbox: &str,
    ) {
        let _ = session.select("INBOX").await;
        let _ = session
            .run_command_capture_code(&format!("DELETE {}", build_mailbox_arg(mailbox).unwrap()))
            .await;
        session
            .create_mailbox(mailbox)
            .await
            .expect("create empty scratch mailbox");
    }

    fn live_raw(message_id: &str) -> Vec<u8> {
        // A Bcc header (which the Sent copy KEEPS), a non-ASCII line and a
        // single-dot line, so byte-identity across the APPEND literal is a real
        // test of the dot-stuffing / 8-bit path.
        format!(
            "From: test@threestrands.test\r\nTo: rcpt@threestrands.test\r\nBcc: blind@threestrands.test\r\nMessage-ID: {message_id}\r\nSubject: Grüße\r\n\r\nhello\r\n.\r\nafter the dot\r\n"
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn live_save_sent_copy_appends_locates_and_is_idempotent() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let mailbox = "P4R4-Sent";
        create_empty_scratch(&mut session, mailbox).await;
        let message_id = "<p4r4-live-1@threestrands.test>";
        let raw = live_raw(message_id);
        let config = SaveSentConfig {
            relocate_rechecks: 3,
            recheck_delay: std::time::Duration::from_millis(50),
        };

        // First call: appended and located.
        let first = save_sent_copy(&mut session, mailbox, &raw, message_id, config)
            .await
            .expect("live append + locate");
        let uid = match first {
            SentCopy::Appended { uid, .. } => uid,
            SentCopy::AppendedUnlocated { .. } => {
                // A server with delayed SEARCH visibility: still success. Give
                // it a moment and locate directly for the content assertions.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let located =
                    crate::provider::imap::write::find_by_message_id(&mut session, message_id, 0)
                        .await
                        .expect("locate after settle");
                *located.uids.first().expect("the appended copy is now visible")
            }
            SentCopy::AlreadyPresent { .. } => panic!("scratch mailbox should have been empty"),
        };

        // Verify the stored copy: \Seen set, and byte-identical to raw.
        session.select(mailbox).await.unwrap();
        let flags = session
            .uid_fetch(&uid.to_string(), "FLAGS")
            .await
            .unwrap();
        assert!(
            flags.iter().any(|m| m
                .flags()
                .any(|f| matches!(f, async_imap::types::Flag::Seen))),
            "the Sent copy is \\Seen"
        );
        let fetched = session
            .uid_fetch(&uid.to_string(), "BODY.PEEK[]")
            .await
            .unwrap();
        let stored = fetched
            .iter()
            .find_map(|m| m.body().map(<[u8]>::to_vec))
            .expect("the appended body is fetchable");
        assert_eq!(stored, raw, "the stored Sent copy is byte-identical to raw (Bcc kept)");

        // Second call: AlreadyPresent, still exactly one copy.
        let second = save_sent_copy(&mut session, mailbox, &raw, message_id, config)
            .await
            .expect("idempotent second call");
        match second {
            SentCopy::AlreadyPresent { uids, .. } => assert_eq!(uids.len(), 1),
            other => panic!("expected AlreadyPresent, got {other:?}"),
        }
        session.select(mailbox).await.unwrap();
        assert_eq!(
            session.uid_search("ALL").await.unwrap().len(),
            1,
            "exactly one copy after the second call"
        );

        drop_scratch(&mut session, &[mailbox]).await;
        let _ = session.logout().await;
    }

    #[tokio::test]
    async fn live_a_near_miss_message_id_in_the_same_mailbox_is_not_matched() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        let mailbox = "P4R4-NearMiss";
        create_empty_scratch(&mut session, mailbox).await;
        let config = SaveSentConfig {
            relocate_rechecks: 3,
            recheck_delay: std::time::Duration::from_millis(50),
        };
        // Put a NEAR-MISS message in the mailbox first: <xabc@x> is a superstring
        // of <abc@x>.
        let near = "<xabc@threestrands.test>";
        save_sent_copy(&mut session, mailbox, &live_raw(near), near, config)
            .await
            .expect("append the near-miss");
        // Now save <abc@x>: the SEARCH HEADER substring match may surface the
        // near-miss, but the exact verify must reject it, so this APPENDs
        // rather than reporting AlreadyPresent.
        let exact = "<abc@threestrands.test>";
        let outcome = save_sent_copy(&mut session, mailbox, &live_raw(exact), exact, config)
            .await
            .expect("append the exact id");
        assert!(
            matches!(outcome, SentCopy::Appended { .. } | SentCopy::AppendedUnlocated { .. }),
            "a near-miss must not short-circuit to AlreadyPresent: {outcome:?}"
        );
        session.select(mailbox).await.unwrap();
        assert_eq!(
            session.uid_search("ALL").await.unwrap().len(),
            2,
            "both the near-miss and the exact copy are present (no false dedup)"
        );
        drop_scratch(&mut session, &[mailbox]).await;
        let _ = session.logout().await;
    }

    #[tokio::test]
    async fn live_save_sent_copy_into_a_non_ascii_scratch_mailbox() {
        let Some((mut session, _lease)) = live_session().await else {
            return;
        };
        // A non-ASCII scratch name exercises the modified-UTF-7 EXAMINE/APPEND
        // path end to end against the real server.
        let mailbox = "P4R4-Gesendet-Entw\u{00fc}rfe";
        create_empty_scratch(&mut session, mailbox).await;
        let message_id = "<p4r4-nonascii@threestrands.test>";
        let raw = live_raw(message_id);
        let config = SaveSentConfig {
            relocate_rechecks: 3,
            recheck_delay: std::time::Duration::from_millis(50),
        };
        let outcome = save_sent_copy(&mut session, mailbox, &raw, message_id, config)
            .await
            .expect("append to a non-ASCII scratch mailbox");
        assert!(matches!(
            outcome,
            SentCopy::Appended { .. } | SentCopy::AppendedUnlocated { .. }
        ));
        session.select(mailbox).await.unwrap();
        assert_eq!(session.uid_search("ALL").await.unwrap().len(), 1);
        drop_scratch(&mut session, &[mailbox]).await;
        let _ = session.logout().await;
    }
}

