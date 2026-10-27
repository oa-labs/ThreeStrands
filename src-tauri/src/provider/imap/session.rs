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
    /// `PERMANENTFLAGS` as the server listed them, each rendered to its
    /// wire spelling (`\Seen`, `$Forwarded`, …). Only a writable SELECT can
    /// supply persisted write capabilities; EXAMINE flags are read-only.
    /// A `\*` wildcard is reflected separately in `permanent_keywords`.
    pub permanent_flags: Vec<String>,
}

impl MailboxStatus {
    fn from_mailbox(mailbox: &Mailbox) -> Self {
        let permanent_keywords = mailbox
            .permanent_flags
            .iter()
            .any(|flag| matches!(flag, async_imap::types::Flag::MayCreate));
        let permanent_flags = mailbox
            .permanent_flags
            .iter()
            .map(flag_to_wire)
            .collect();
        Self {
            exists: mailbox.exists,
            uid_next: mailbox.uid_next,
            uid_validity: mailbox.uid_validity,
            permanent_keywords,
            permanent_flags,
        }
    }
}

/// Render an `async-imap` [`Flag`](async_imap::types::Flag) to its IMAP wire
/// spelling (`\Seen`, `$Forwarded`, …). `async-imap`'s `Flag` has no
/// `Display`, so the system-flag spellings are written out; a `Custom` keyword
/// is used verbatim. `\*` (`MayCreate`) is reflected separately in
/// `permanent_keywords` and is kept here too so the stored list is faithful.
pub(super) fn flag_to_wire(flag: &async_imap::types::Flag<'_>) -> String {
    use async_imap::types::Flag;
    match flag {
        Flag::Seen => "\\Seen".to_string(),
        Flag::Answered => "\\Answered".to_string(),
        Flag::Flagged => "\\Flagged".to_string(),
        Flag::Deleted => "\\Deleted".to_string(),
        Flag::Draft => "\\Draft".to_string(),
        Flag::Recent => "\\Recent".to_string(),
        Flag::MayCreate => "\\*".to_string(),
        Flag::Custom(name) => name.to_string(),
    }
}

/// One mailbox from a `LIST` response: its wire name, hierarchy delimiter,
/// and any RFC 6154 special-use attribute the server returned (`\Sent`,
/// `\Archive`, …). The special-use attribute is kept as a plain string so the
/// mailbox-mapping slice (Slice 3) can act on it without this trait depending
/// on an attribute enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxEntry {
    pub name: String,
    pub delimiter: Option<String>,
    /// `\Noselect` containers are not locations; Slice 3 skips them.
    pub no_select: bool,
    /// The special-use attribute, e.g. `\Sent`, if the server returned one.
    pub special_use: Option<String>,
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

    /// The server's advertised capabilities (post-login `CAPABILITY`),
    /// uppercased. Slice 2's "test and save" reads these to confirm the login
    /// reached a real IMAP server; the sync slices branch on individual ones.
    async fn capabilities(&mut self) -> Result<Vec<String>, ProviderError>;

    /// `LIST "" "*"` — every mailbox with its hierarchy delimiter and the
    /// RFC 6154 special-use attributes the server returns. Slice 2 uses it to
    /// prove the account can enumerate mailboxes; mailbox discovery proper is
    /// Slice 3.
    async fn list_mailboxes(&mut self) -> Result<Vec<MailboxEntry>, ProviderError>;

    /// Run a raw tagged command and return the first response code's debug
    /// string, if any — the escape hatch for `COPYUID`/`APPENDUID` that typed
    /// calls discard (Slice 0 spike finding). Later mutation slices use it.
    /// Only a matching OK completion succeeds; NO/BAD return provider errors.
    #[cfg(test)]
    async fn run_command_capture_code(
        &mut self,
        command: &str,
    ) -> Result<Option<String>, ProviderError>;

    /// CREATE a mailbox. Used by Slice 3's confirmed-mapping commit when the
    /// user chooses to create a missing Archive mailbox, and by the
    /// `TRYCREATE` create-then-retry flow. Succeeds (or maps the server's
    /// rejection to a [`ProviderError`]); creating a mailbox that already
    /// exists is left to the caller to tolerate per the design's idempotence
    /// rule.
    async fn create_mailbox(&mut self, mailbox: &str) -> Result<(), ProviderError>;

    /// LOGOUT and close the session.
    async fn logout(&mut self) -> Result<(), ProviderError>;

    /// Re-borrow this session as the Phase 3 WRITE surface
    /// ([`ImapWriteSession`](super::write::ImapWriteSession)), when it
    /// implements it. The session source hands callers a `Box<dyn
    /// ImapSession>`, which cannot be downcast to the write trait object, so a
    /// writer (run 2's flag mutations) upcasts through this method instead.
    /// The default is `None` for a read-only session (the test stub); the two
    /// real sessions — `AsyncImapSession` and `LeasedSession` — override it to
    /// return `Some(self)`.
    fn as_write_session(&mut self) -> Option<&mut dyn super::write::ImapWriteSession> {
        None
    }
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

    async fn capabilities(&mut self) -> Result<Vec<String>, ProviderError> {
        let caps = self
            .session
            .capabilities()
            .await
            .map_err(|e| map_imap_error(&e))?;
        Ok(caps
            .iter()
            .map(|cap| format!("{cap:?}").to_ascii_uppercase())
            .collect())
    }

    async fn list_mailboxes(&mut self) -> Result<Vec<MailboxEntry>, ProviderError> {
        use async_imap::imap_proto::NameAttribute;
        let stream = self
            .session
            .list(Some(""), Some("*"))
            .await
            .map_err(|e| map_imap_error(&e))?;
        let names: Vec<Result<async_imap::types::Name, _>> = stream.collect().await;
        let mut entries = Vec::with_capacity(names.len());
        for name in names {
            let name = name.map_err(|e| map_imap_error(&e))?;
            let mut no_select = false;
            let mut special_use = None;
            for attribute in name.attributes() {
                match attribute {
                    NameAttribute::NoSelect => no_select = true,
                    // RFC 6154 special-use attributes map to the design's
                    // backslash spelling; captured as-is for Slice 3.
                    NameAttribute::All => special_use = Some("\\All".to_string()),
                    NameAttribute::Archive => special_use = Some("\\Archive".to_string()),
                    NameAttribute::Drafts => special_use = Some("\\Drafts".to_string()),
                    NameAttribute::Flagged => special_use = Some("\\Flagged".to_string()),
                    NameAttribute::Junk => special_use = Some("\\Junk".to_string()),
                    NameAttribute::Sent => special_use = Some("\\Sent".to_string()),
                    NameAttribute::Trash => special_use = Some("\\Trash".to_string()),
                    _ => {}
                }
            }
            entries.push(MailboxEntry {
                name: name.name().to_string(),
                delimiter: name.delimiter().map(str::to_string),
                no_select,
                special_use,
            });
        }
        Ok(entries)
    }

    #[cfg(test)]
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
                } if tag.as_bytes() == id.as_bytes() => {
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
                _ => {}
            }
        }
        Ok(captured)
    }

    async fn create_mailbox(&mut self, mailbox: &str) -> Result<(), ProviderError> {
        self.session
            .create(mailbox)
            .await
            .map_err(|e| map_imap_error(&e))
    }

    async fn logout(&mut self) -> Result<(), ProviderError> {
        self.session.logout().await.map_err(|e| map_imap_error(&e))
    }

    fn as_write_session(&mut self) -> Option<&mut dyn super::write::ImapWriteSession> {
        // `AsyncImapSession<T>` implements `ImapWriteSession` below, so it can
        // be re-borrowed as the write surface.
        Some(self)
    }
}
/// whether it completed OK, and the first typed `CopyUid` seen (from either an
/// untagged `* OK [COPYUID ...]` — MOVE — or the tagged completion — COPY).
struct RawWriteOutcome {
    copyuid: Option<super::write::CopyUid>,
}

impl<T> AsyncImapSession<T>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    /// Run a raw tagged write command, capturing a typed `COPYUID` from either
    /// an UNTAGGED `* OK [COPYUID ...]` data response (the shape `UID MOVE`
    /// sends, before its `* n EXPUNGE` lines) or the TAGGED completion (the
    /// shape `UID COPY` sends). A tagged `NO [TRYCREATE]` becomes
    /// [`write::WriteError::TryCreate`] so the write layer can create the
    /// target and retry; every other `NO`/`BAD` is classified through the RFC
    /// 5530 map. The response codes borrow from the read buffer, so each is
    /// copied into an owned [`write::CopyUid`] before the next `read_response`.
    async fn run_write_command(
        &mut self,
        command: &str,
    ) -> Result<RawWriteOutcome, super::write::WriteError> {
        use async_imap::imap_proto::{Response, ResponseCode, Status};
        let id = self
            .session
            .run_command(command)
            .await
            .map_err(|e| super::write::WriteError::Provider(map_imap_error(&e)))?;
        let mut copyuid: Option<super::write::CopyUid> = None;
        loop {
            let resp = self
                .session
                .read_response()
                .await
                .map_err(|io| {
                    super::write::WriteError::Provider(map_imap_error(
                        &async_imap::error::Error::Io(io),
                    ))
                })?
                .ok_or_else(|| {
                    super::write::WriteError::Provider(map_imap_error(
                        &async_imap::error::Error::ConnectionLost,
                    ))
                })?;
            match resp.parsed() {
                Response::Data {
                    status: Status::Bye,
                    ..
                } => {
                    return Err(super::write::WriteError::Provider(map_imap_error(
                        &async_imap::error::Error::ConnectionLost,
                    )));
                }
                // Untagged data: MOVE's COPYUID arrives here. Keep the first.
                Response::Data { outcome, .. } => {
                    if copyuid.is_none() {
                        if let Some(code) = outcome.code.as_ref() {
                            copyuid = super::write::copyuid_from_code(code);
                        }
                    }
                }
                Response::Done {
                    tag,
                    status,
                    outcome,
                } => {
                    if tag.as_bytes() != id.as_bytes() {
                        continue;
                    }
                    match status {
                        Status::Ok => {
                            if copyuid.is_none() {
                                if let Some(code) = outcome.code.as_ref() {
                                    copyuid = super::write::copyuid_from_code(code);
                                }
                            }
                            return Ok(RawWriteOutcome { copyuid });
                        }
                        Status::No | Status::Bad => {
                            // A TRYCREATE is a distinct, typed outcome.
                            if matches!(outcome.code, Some(ResponseCode::TryCreate)) {
                                return Err(super::write::WriteError::TryCreate(
                                    outcome
                                        .information
                                        .as_deref()
                                        .unwrap_or("TRYCREATE")
                                        .to_string(),
                                ));
                            }
                            return Err(super::write::WriteError::Provider(
                                classify_response_code(
                                    outcome.code.as_ref(),
                                    outcome.information.as_deref().unwrap_or_default(),
                                )
                                .into_provider_error(),
                            ));
                        }
                        _ => {
                            return Err(super::write::WriteError::Provider(
                                ProviderError::InvalidOperation(format!(
                                    "unexpected tagged IMAP status: {status:?}"
                                )),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Run a raw tagged write command that carries no COPYUID (STORE, EXPUNGE),
    /// mapping any `NO`/`BAD` through the RFC 5530 map. A `TRYCREATE` here would
    /// be unexpected (STORE/EXPUNGE do not create mailboxes), so it is left to
    /// the generic classifier.
    async fn run_write_command_plain(&mut self, command: &str) -> Result<(), ProviderError> {
        match self.run_write_command(command).await {
            Ok(_) => Ok(()),
            Err(error) => Err(error.into_provider_error()),
        }
    }
}

#[async_trait]
impl<T> super::write::ImapWriteSession for AsyncImapSession<T>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    async fn uid_store(
        &mut self,
        uid_set: &str,
        op: super::write::StoreOp,
        flags: &[String],
    ) -> Result<(), ProviderError> {
        let item = match op {
            super::write::StoreOp::Add => "+FLAGS.SILENT",
            super::write::StoreOp::Remove => "-FLAGS.SILENT",
        };
        let command = format!("UID STORE {uid_set} {item} ({})", flags.join(" "));
        self.run_write_command_plain(&command).await
    }

    async fn uid_copy(
        &mut self,
        uid_set: &str,
        dest: &str,
    ) -> super::write::WriteResult<super::write::CopyOutcome> {
        let outcome = self
            .run_write_command(&format!("UID COPY {uid_set} {dest}"))
            .await?;
        Ok(super::write::CopyOutcome {
            copyuid: outcome.copyuid,
        })
    }

    async fn uid_move(
        &mut self,
        uid_set: &str,
        dest: &str,
    ) -> super::write::WriteResult<super::write::CopyOutcome> {
        let outcome = self
            .run_write_command(&format!("UID MOVE {uid_set} {dest}"))
            .await?;
        Ok(super::write::CopyOutcome {
            copyuid: outcome.copyuid,
        })
    }

    async fn uid_expunge(&mut self, uid_set: &str) -> Result<(), ProviderError> {
        self.run_write_command_plain(&format!("UID EXPUNGE {uid_set}"))
            .await
    }

    async fn create_mailbox_arg(&mut self, arg: &str) -> super::write::WriteResult<()> {
        // `arg` is already an encoded, quoted mailbox token (build_mailbox_arg),
        // so we build the command directly rather than go through async-imap's
        // `create`, which quotes but does not modified-UTF-7-encode. A missing
        // parent surfaces as the typed TryCreate outcome.
        self.run_write_command(&format!("CREATE {arg}")).await?;
        Ok(())
    }

    async fn delete_mailbox_arg(&mut self, arg: &str) -> Result<(), ProviderError> {
        self.run_write_command_plain(&format!("DELETE {arg}")).await
    }

    async fn append(
        &mut self,
        mailbox: &str,
        flags: &[String],
        content: &[u8],
    ) -> super::write::WriteResult<()> {
        // Validate + encode BEFORE any wire I/O. `prepare_append` returns the
        // modified-UTF-7 mailbox name (async-imap quotes it with `quote!`,
        // which only escapes `"`/`\`; the encoded name is ASCII, so a non-ASCII
        // Sent mailbox round trips as modified UTF-7, not raw UTF-8) and the
        // parenthesised flag list.
        let (encoded_mailbox, flag_list) =
            super::write::prepare_append(mailbox, flags, content)
                .map_err(super::write::WriteError::Provider)?;
        // The library `append` performs the `+`-continuation handshake and
        // writes the literal; a hand-rolled literal desynchronises the stream
        // (Phase 3 run 1). No INTERNALDATE: the server stamps "now".
        self.session
            .append(&encoded_mailbox, Some(&flag_list), None, content)
            .await
            .map_err(|e| super::write::map_append_error(&e))
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
        let mut mb = Mailbox {
            exists: 3,
            uid_next: Some(42),
            uid_validity: Some(95479608),
            permanent_flags: vec![Flag::Seen, Flag::Flagged],
            ..Default::default()
        };
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
}
