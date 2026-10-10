//! The internal `ImapSession` trait and its `async-imap` implementation.
//!
//! `docs/imap-design.md` ("Recommended stack") puts an `async-imap` client
//! "behind an internal `ImapSession` trait". The trait is the seam that keeps
//! the `imap-next` fallback local: every later slice calls IMAP through this
//! trait, so swapping the backing library (should `async-imap` ever stop
//! fitting) is a change to one impl, not to the read/mutation slices.
//!
//! The surface is deliberately minimal — only the operations the later read
//! slices actually need: SELECT/EXAMINE a mailbox, UID FETCH, UID SEARCH, a
//! raw `run_command` (to recover `COPYUID`/`APPENDUID` codes `async-imap`
//! discards, as the Slice 0 spike proved), NOOP, and LOGOUT. IDLE is a Slice 5
//! concern (incremental sync); the trait reserves it with an
//! `InvalidOperation` stub and a `// Slice 5` note rather than widening the
//! surface before there is a caller.
//!
//! Nothing here fetches, syncs, or discovers mailboxes — those are later
//! slices. This is the authenticated-session abstraction they build on.

use async_imap::types::{Fetch, Mailbox};
use async_imap::Session;
use async_trait::async_trait;
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::provider::ProviderError;

use super::error::{classify_response_code, map_imap_error};

/// A mailbox's post-SELECT status, the subset of `async-imap`'s `Mailbox` the
/// later slices read: the UID counters a resync compares and whether arbitrary
/// keyword labels are storable (`PERMANENTFLAGS` carried `\*`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxStatus {
    pub exists: u32,
    pub uid_next: Option<u32>,
    pub uid_validity: Option<u32>,
    /// `PERMANENTFLAGS` contained `\*` — the server lets clients create
    /// arbitrary keywords (so this account can store keyword labels).
    pub permanent_keywords: bool,
}

impl MailboxStatus {
    fn from_mailbox(mailbox: &Mailbox) -> Self {
        let permanent_keywords = mailbox
            .permanent_flags
            .iter()
            .any(|flag| matches!(flag, async_imap::types::Flag::MayCreate));
        Self {
            exists: mailbox.exists,
            uid_next: mailbox.uid_next,
            uid_validity: mailbox.uid_validity,
            permanent_keywords,
        }
    }
}

/// An authenticated IMAP session.
///
/// Implemented for `async-imap` by [`AsyncImapSession`]; the trait keeps that
/// choice swappable (the design's `imap-next` fallback). Methods use the shared
/// IMAP error classification so callers only ever see [`ProviderError`].
#[async_trait]
pub trait ImapSession: Send {
    /// SELECT a mailbox for read/write and return its status.
    async fn select(&mut self, mailbox: &str) -> Result<MailboxStatus, ProviderError>;

    /// EXAMINE a mailbox read-only and return its status.
    async fn examine(&mut self, mailbox: &str) -> Result<MailboxStatus, ProviderError>;

    /// UID SEARCH in the selected mailbox; returns matching UIDs (unordered).
    async fn uid_search(&mut self, query: &str) -> Result<Vec<u32>, ProviderError>;

    /// UID FETCH `items` for `uid_set` in the selected mailbox; returns the
    /// raw `Fetch` responses collected. (The read slices turn these into
    /// `RawMessage`s; Slice 1 only proves the round-trip.)
    async fn uid_fetch(&mut self, uid_set: &str, items: &str) -> Result<Vec<Fetch>, ProviderError>;

    /// Run a raw tagged command and return the first response code's debug
    /// string, if any — the escape hatch for `COPYUID`/`APPENDUID` that typed
    /// calls discard (Slice 0 spike finding). Later mutation slices use it.
    /// Only a matching OK completion succeeds; NO/BAD return provider errors.
    async fn run_command_capture_code(
        &mut self,
        command: &str,
    ) -> Result<Option<String>, ProviderError>;

    /// NOOP — a liveness probe / unsolicited-response pump.
    async fn noop(&mut self) -> Result<(), ProviderError>;

    /// Enter IDLE on the selected mailbox. Slice 5 (incremental sync) owns the
    /// 25-minute re-issue loop; the trait reserves the method so the pool and
    /// the sync engine agree on the surface, but Slice 1 has no IDLE caller.
    async fn idle_reserved(&mut self) -> Result<(), ProviderError> {
        // Slice 5: IDLE + 25-min re-issue. No caller in Slice 1.
        Err(ProviderError::InvalidOperation(
            "IDLE is not wired until Slice 5 (incremental sync)".into(),
        ))
    }

    /// LOGOUT and close the session.
    async fn logout(&mut self) -> Result<(), ProviderError>;
}

/// The `async-imap`-backed [`ImapSession`]. Wraps an authenticated
/// `Session<T>` over any TLS stream the connection layer upgraded.
pub struct AsyncImapSession<T>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    session: Session<T>,
}

impl<T> AsyncImapSession<T>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    /// Takes ownership of an already-authenticated `async-imap` session.
    pub fn new(session: Session<T>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl<T> ImapSession for AsyncImapSession<T>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    async fn select(&mut self, mailbox: &str) -> Result<MailboxStatus, ProviderError> {
        let mb = self
            .session
            .select(mailbox)
            .await
            .map_err(|e| map_imap_error(&e))?;
        Ok(MailboxStatus::from_mailbox(&mb))
    }

    async fn examine(&mut self, mailbox: &str) -> Result<MailboxStatus, ProviderError> {
        let mb = self
            .session
            .examine(mailbox)
            .await
            .map_err(|e| map_imap_error(&e))?;
        Ok(MailboxStatus::from_mailbox(&mb))
    }

    async fn uid_search(&mut self, query: &str) -> Result<Vec<u32>, ProviderError> {
        let uids = self
            .session
            .uid_search(query)
            .await
            .map_err(|e| map_imap_error(&e))?;
        Ok(uids.into_iter().collect())
    }

    async fn uid_fetch(&mut self, uid_set: &str, items: &str) -> Result<Vec<Fetch>, ProviderError> {
        let stream = self
            .session
            .uid_fetch(uid_set, items)
            .await
            .map_err(|e| map_imap_error(&e))?;
        let fetches: Vec<Result<Fetch, _>> = stream.collect().await;
        fetches
            .into_iter()
            .map(|f| f.map_err(|e| map_imap_error(&e)))
            .collect()
    }

    async fn run_command_capture_code(
        &mut self,
        command: &str,
    ) -> Result<Option<String>, ProviderError> {
        use async_imap::imap_proto::{Response, Status};
        let id = self
            .session
            .run_command(command)
            .await
            .map_err(|e| map_imap_error(&e))?;
        let mut captured: Option<String> = None;
        loop {
            let resp = self
                .session
                .read_response()
                .await
                .map_err(|io| map_imap_error(&async_imap::error::Error::Io(io)))?
                .ok_or_else(|| map_imap_error(&async_imap::error::Error::ConnectionLost))?;
            match resp.parsed() {
                Response::Data {
                    status: Status::Bye,
                    ..
                } => {
                    return Err(map_imap_error(&async_imap::error::Error::ConnectionLost));
                }
                Response::Data { outcome, .. } => {
                    if let Some(code) = outcome.code.as_ref() {
                        captured = Some(format!("{code:?}"));
                    }
                }
                Response::Done {
                    tag,
                    status,
                    outcome,
                } => {
                    if tag.as_bytes() == id.as_bytes() {
                        match status {
                            Status::Ok => {}
                            Status::No | Status::Bad => {
                                return Err(classify_response_code(
                                    outcome.code.as_ref(),
                                    outcome.information.as_deref().unwrap_or_default(),
                                )
                                .into_provider_error());
                            }
                            _ => {
                                return Err(ProviderError::InvalidOperation(format!(
                                    "unexpected tagged IMAP status: {status:?}"
                                )));
                            }
                        }
                        if let Some(code) = outcome.code.as_ref() {
                            captured = Some(format!("{code:?}"));
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
        Ok(captured)
    }

    async fn noop(&mut self) -> Result<(), ProviderError> {
        self.session.noop().await.map_err(|e| map_imap_error(&e))
    }

    async fn logout(&mut self) -> Result<(), ProviderError> {
        self.session.logout().await.map_err(|e| map_imap_error(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
    use tokio::task::JoinHandle;

    // Exercise async-imap's actual wire parser and error formatting without
    // a network server. Each response uses the request's real tag.
    fn scripted_client(
        responses: Vec<&'static str>,
    ) -> (async_imap::Client<DuplexStream>, JoinHandle<()>) {
        let (client, server) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            server
                .get_mut()
                .write_all(b"* OK test server ready\r\n")
                .await
                .unwrap();
            for response in responses {
                let mut command = String::new();
                server.read_line(&mut command).await.unwrap();
                let tag = command.split_whitespace().next().expect("a tagged command");
                let response = format!("{}\r\n", response.replace("{tag}", tag));
                server
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .unwrap();
            }
        });
        (async_imap::Client::new(client), task)
    }

    async fn scripted_session(
        response: &'static str,
    ) -> (AsyncImapSession<DuplexStream>, JoinHandle<()>) {
        let (client, task) = scripted_client(vec!["{tag} OK logged in", response]);
        let session = client.login("user", "password").await.unwrap();
        (AsyncImapSession::new(session), task)
    }

    fn rejected_commands() -> Vec<(&'static str, ProviderError)> {
        vec![
            (
                "{tag} NO [AUTHENTICATIONFAILED] Invalid credentials",
                ProviderError::ReauthenticationRequired(String::new()),
            ),
            (
                "{tag} NO [UNAVAILABLE] Backend AUTHENTICATIONFAILED",
                ProviderError::TransientTransport(String::new()),
            ),
            (
                "{tag} NO [INUSE] Mailbox busy",
                ProviderError::TransientTransport(String::new()),
            ),
            (
                "{tag} NO [OVERQUOTA] Storage exhausted",
                ProviderError::PermanentClientRejection(String::new()),
            ),
            (
                "{tag} NO [TRYCREATE] Missing mailbox",
                ProviderError::TransientTransport(String::new()),
            ),
            (
                "{tag} NO Permission denied",
                ProviderError::InvalidOperation(String::new()),
            ),
            (
                "{tag} BAD Command syntax error",
                ProviderError::InvalidOperation(String::new()),
            ),
        ]
    }

    fn assert_error_category(actual: &ProviderError, expected: &ProviderError, response: &str) {
        assert_eq!(
            std::mem::discriminant(actual),
            std::mem::discriminant(expected),
            "wrong error category for {response}: {actual:?}"
        );
    }

    #[tokio::test]
    async fn raw_commands_map_no_and_bad_completions_to_provider_errors() {
        for (response, expected) in rejected_commands() {
            let (mut session, server) = scripted_session(response).await;
            let err = session
                .run_command_capture_code("UID COPY 1 Archive")
                .await
                .unwrap_err();
            assert_error_category(&err, &expected, response);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn raw_commands_preserve_successful_uid_response_codes() {
        for (response, expected) in [
            (
                "{tag} OK [COPYUID 7 1 9] copied",
                Some("CopyUid(7, [Uid(1)], [Uid(9)])"),
            ),
            (
                "{tag} OK [APPENDUID 7 9] appended",
                Some("AppendUid(7, [Uid(9)])"),
            ),
            ("{tag} OK complete", None),
            ("other BAD unrelated command\r\n{tag} OK complete", None),
        ] {
            let (mut session, server) = scripted_session(response).await;
            let code = session
                .run_command_capture_code("UID COPY 1 Archive")
                .await
                .unwrap();
            assert_eq!(code.as_deref(), expected, "{response}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn raw_commands_fail_when_the_server_sends_bye_or_drops_the_connection() {
        for response in [
            "* BYE [UNAVAILABLE] Shutting down",
            "* OK No command completion",
        ] {
            let (mut session, server) = scripted_session(response).await;
            let err = session
                .run_command_capture_code("UID COPY 1 Archive")
                .await
                .unwrap_err();
            assert!(
                matches!(err, ProviderError::TransientTransport(_)),
                "{err:?}"
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn typed_commands_map_actual_async_imap_error_formats() {
        for (response, expected) in rejected_commands() {
            // LOGOUT uses `code: ..., info: ...`; SELECT uses `outcome: ...`.
            let (mut session, server) = scripted_session(response).await;
            let err = session.logout().await.unwrap_err();
            assert_error_category(&err, &expected, response);
            server.await.unwrap();

            let (mut session, server) = scripted_session(response).await;
            let err = session.select("INBOX").await.unwrap_err();
            assert_error_category(&err, &expected, response);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn login_maps_actual_async_imap_error_formats() {
        for (response, expected) in rejected_commands() {
            let (client, server) = scripted_client(vec![response]);
            let (err, _) = client.login("user", "password").await.unwrap_err();
            assert_error_category(&map_imap_error(&err), &expected, response);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn mailbox_names_and_diagnostics_do_not_become_response_codes() {
        for response in [
            "{tag} NO [NONEXISTENT] Unknown mailbox: INUSE",
            "{tag} NO [NONEXISTENT] Unknown mailbox: AUTHENTICATIONFAILED",
            "{tag} NO Unknown mailbox: OVERQUOTA",
            "{tag} NO Mailbox doesn't exist: [TRYCREATE]",
            "{tag} NO [UNAVAILABLE-LATER] Unrecognized code",
            "{tag} NO [ALERT] [AUTHENTICATIONFAILED] diagnostic only",
        ] {
            let (mut session, server) = scripted_session(response).await;
            let err = session
                .run_command_capture_code("UID COPY 1 INUSE")
                .await
                .unwrap_err();
            assert!(
                matches!(err, ProviderError::InvalidOperation(_)),
                "{response}: {err:?}"
            );
            server.await.unwrap();

            let (mut session, server) = scripted_session(response).await;
            let err = session.logout().await.unwrap_err();
            assert!(
                matches!(err, ProviderError::InvalidOperation(_)),
                "{response}: {err:?}"
            );
            server.await.unwrap();

            let (mut session, server) = scripted_session(response).await;
            let err = session.select("INUSE").await.unwrap_err();
            assert!(
                matches!(err, ProviderError::InvalidOperation(_)),
                "{response}: {err:?}"
            );
            server.await.unwrap();
        }
    }

    // `MailboxStatus` reads `\*` out of PERMANENTFLAGS as the keyword-storable
    // signal, independent of any live server.
    #[test]
    fn mailbox_status_reads_permanent_keywords_from_the_wildcard_flag() {
        use async_imap::types::Flag;
        let mut mb = Mailbox::default();
        mb.exists = 3;
        mb.uid_next = Some(42);
        mb.uid_validity = Some(95479608);
        mb.permanent_flags = vec![Flag::Seen, Flag::Flagged];
        let without = MailboxStatus::from_mailbox(&mb);
        assert_eq!(without.exists, 3);
        assert_eq!(without.uid_next, Some(42));
        assert_eq!(without.uid_validity, Some(95479608));
        assert!(!without.permanent_keywords);

        mb.permanent_flags.push(Flag::MayCreate);
        let with = MailboxStatus::from_mailbox(&mb);
        assert!(with.permanent_keywords);
    }

    // The reserved IDLE method refuses until Slice 5 wires it, rather than
    // silently doing nothing.
    #[tokio::test]
    async fn idle_is_reserved_until_slice_5() {
        struct Stub;
        #[async_trait]
        impl ImapSession for Stub {
            async fn select(&mut self, _: &str) -> Result<MailboxStatus, ProviderError> {
                unreachable!()
            }
            async fn examine(&mut self, _: &str) -> Result<MailboxStatus, ProviderError> {
                unreachable!()
            }
            async fn uid_search(&mut self, _: &str) -> Result<Vec<u32>, ProviderError> {
                unreachable!()
            }
            async fn uid_fetch(&mut self, _: &str, _: &str) -> Result<Vec<Fetch>, ProviderError> {
                unreachable!()
            }
            async fn run_command_capture_code(
                &mut self,
                _: &str,
            ) -> Result<Option<String>, ProviderError> {
                unreachable!()
            }
            async fn noop(&mut self) -> Result<(), ProviderError> {
                unreachable!()
            }
            async fn logout(&mut self) -> Result<(), ProviderError> {
                unreachable!()
            }
        }
        let mut stub = Stub;
        let err = stub.idle_reserved().await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)));
    }
}
