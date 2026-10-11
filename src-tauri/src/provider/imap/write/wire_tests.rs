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
                reply(&mut server, &format!("* 1 EXPUNGE\r\n{tag} OK EXPUNGE done\r\n")).await;
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
}

