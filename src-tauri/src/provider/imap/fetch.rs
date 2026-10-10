//! Typed fetch helpers over the [`ImapSession`] trait, plus the on-disk body
//! cache the read path serves from.
//!
//! `docs/imap-design.md` ("What gets synced" / "Bodies"): a message body never
//! changes in IMAP, so `BODY.PEEK[]` is fetched once per stable message id and
//! cached; a flag change never re-downloads it, and a copy of the same message
//! in a second mailbox is not fetched again. Three rules shape this module:
//!
//! 1. **`BODY.PEEK` only.** Every fetch here uses `BODY.PEEK[...]`, never
//!    `BODY[...]`, so reading or syncing a message never sets `\Seen` on the
//!    server (guiding rule 3). The command strings live in one place
//!    ([`IDENTITY_ITEMS`], [`BODY_ITEMS`]) and a test asserts neither contains
//!    a non-peek `BODY[`.
//! 2. **Identity first, body once.** The cheap identity pass fetches
//!    `UID FLAGS RFC822.SIZE` plus the four header fields; the stable id is
//!    resolved from those (and sticky reuse); only then, and only if the body
//!    is not already cached and its advertised size fits, is a bounded
//!    `BODY.PEEK[]` fetched. One extra byte detects unadvertised oversize.
//! 3. **Keep the `imap-next` swap cheap.** These helpers go THROUGH the
//!    existing `ImapSession` trait rather than widening it. async-imap's
//!    `Fetch` does not surface the RFC 8474 `EMAILID`, so on the live path the
//!    id takes the mandatory hash route; the best-effort OBJECTID route is
//!    exercised by [`parse_email_id`] against a synthetic response (see its
//!    doc and the report).
//!
//! ### EMAILID (RFC 8474), investigated
//!
//! `imap-proto` 0.17 parses `EMAILID`/`THREADID` into
//! `AttributeValue::EmailId` / `ThreadId`, but async-imap 0.12's `Fetch` type
//! exposes no accessor for them and keeps its parsed attributes private, so a
//! live `uid_fetch` cannot read an EMAILID back without either forking
//! async-imap or re-parsing the raw wire bytes it does not hand out. Per the
//! brief this slice ships the HASH path on the live route (mandatory, always
//! correct) and keeps the OBJECTID path as a pure parser unit-tested with a
//! synthetic response, ready for the day the trait is backed by `imap-next`
//! (which exposes attributes) — the swap stays local to the session impl.

use async_imap::imap_proto::{AttributeValue, Response};

use super::identity::{derive_message_id, IdentityInputs};
use super::policy;
use super::session::{flag_to_wire, ImapSession};
use super::{ImapLocation, ImapStateStore};
use crate::db::DbResult;
use crate::provider::ProviderError;

/// The identity/flags pass: cheap, no body. `BODY.PEEK[HEADER.FIELDS (...)]`
/// returns only those four headers and never sets `\Seen`.
pub const IDENTITY_ITEMS: &str =
    "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID DATE FROM SUBJECT)])";

/// A bounded full-body pass, with one sentinel byte beyond the cache ceiling
/// to detect oversize instead of silently truncating. PEEK never sets `\Seen`.
pub static BODY_ITEMS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("(UID BODY.PEEK[]<0.{}>)", policy::MAX_RAW_FETCH_BYTES));

/// One message's identity/flags row from the cheap pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityRow {
    pub uid: u32,
    pub flags: Vec<String>,
    pub inputs: IdentityInputs,
}

/// Parse the four identity headers out of a `BODY.PEEK[HEADER.FIELDS (...)]`
/// block into [`IdentityInputs`] (minus any EMAILID, which the caller supplies
/// separately). Unfolds continuation lines; never panics on hostile bytes.
pub fn parse_identity_headers(header_block: &[u8], rfc822_size: u32) -> IdentityInputs {
    let text = String::from_utf8_lossy(header_block);
    let mut inputs = IdentityInputs {
        rfc822_size,
        ..Default::default()
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    for (name, value) in headers {
        match name.as_str() {
            "message-id" => inputs.message_id = Some(value),
            "date" => inputs.date = Some(value),
            "from" => inputs.from = Some(value),
            "subject" => inputs.subject = Some(value),
            _ => {}
        }
    }
    inputs
}

/// Best-effort RFC 8474 `EMAILID` extraction from a raw FETCH response line.
///
/// Pure and panic-free: parses the bytes with `imap-proto` and returns the
/// `EMAILID` object id when present. This is the OBJECTID path; it is unit-
/// tested with a synthetic server response because async-imap's typed `Fetch`
/// cannot surface the attribute on the live route (see the module doc).
pub fn parse_email_id(fetch_line: &[u8]) -> Option<String> {
    match Response::parse(fetch_line) {
        Ok((_rest, Response::Fetch(_seq, attrs))) => {
            attrs.into_iter().find_map(|attr| match attr {
                AttributeValue::EmailId(id) => Some(id.into_owned()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// Fetch just the flags for a UID set with [`FLAGS_ITEMS`] — no body, no
/// headers, no size. Returns `(uid, wire flags)` per responded UID. The
/// per-poll flag sweep uses this so it never re-fetches headers for a whole
/// window of messages.
pub async fn fetch_flags_only(
    session: &mut dyn ImapSession,
    uid_set: &str,
) -> Result<Vec<(u32, Vec<String>)>, ProviderError> {
    let fetches = session.uid_fetch(uid_set, FLAGS_ITEMS).await?;
    let mut rows = Vec::with_capacity(fetches.len());
    for fetch in &fetches {
        let Some(uid) = fetch.uid else {
            continue;
        };
        let flags = fetch.flags().map(|flag| flag_to_wire(&flag)).collect();
        rows.push((uid, flags));
    }
    Ok(rows)
}

/// Collect identity rows for a UID set via the cheap identity pass. EMAILID is
/// not read here (async-imap's `Fetch` cannot surface it); the id resolves via
/// the mandatory hash route from the four headers plus RFC822.SIZE.
pub async fn fetch_identity(
    session: &mut dyn ImapSession,
    uid_set: &str,
) -> Result<Vec<IdentityRow>, ProviderError> {
    let fetches = session.uid_fetch(uid_set, IDENTITY_ITEMS).await?;
    let mut rows = Vec::with_capacity(fetches.len());
    for fetch in &fetches {
        let Some(uid) = fetch.uid else {
            // A FETCH without a UID is not addressable; skip rather than guess.
            continue;
        };
        let flags = fetch.flags().map(|flag| flag_to_wire(&flag)).collect();
        let header_block = fetch.header().unwrap_or_default();
        let inputs = parse_identity_headers(header_block, fetch.size.unwrap_or(0));
        rows.push(IdentityRow { uid, flags, inputs });
    }
    Ok(rows)
}

/// The single operation [`ensure_body`] needs from the wire: fetch one UID's
/// whole body with `BODY.PEEK[]`.
///
/// This is a NARROW seam over [`ImapSession`], not a widening of it: the real
/// implementation ([`SessionBodyFetcher`]) wraps an `ImapSession` and calls its
/// `uid_fetch`. async-imap's `Fetch` cannot be constructed outside its crate,
/// so this trait is also what lets `ensure_body` be unit-tested with a counting
/// fake that returns bytes directly — proving "fetched once" without a server.
#[async_trait::async_trait]
pub trait BodyFetcher: Send {
    /// Return the raw RFC 5322 bytes for `uid`, or `None` if the server gave
    /// no body. Must use a bounded `BODY.PEEK[]` request so it never sets
    /// `\Seen` or requests an unbounded literal when RFC822.SIZE is missing.
    async fn fetch_raw_body(&mut self, uid: u32) -> Result<Option<Vec<u8>>, ProviderError>;
}

/// The production [`BodyFetcher`]: `BODY.PEEK[]` over a real [`ImapSession`].
pub struct SessionBodyFetcher<'a> {
    pub session: &'a mut dyn ImapSession,
}

#[async_trait::async_trait]
impl BodyFetcher for SessionBodyFetcher<'_> {
    async fn fetch_raw_body(&mut self, uid: u32) -> Result<Option<Vec<u8>>, ProviderError> {
        let fetches = self
            .session
            .uid_fetch(&uid.to_string(), &BODY_ITEMS)
            .await?;
        for fetch in &fetches {
            if fetch.uid == Some(uid) {
                if let Some(body) = fetch.body() {
                    policy::check_raw_message_bytes(body.len())?;
                    return Ok(Some(body.to_vec()));
                }
            }
        }
        // Some servers omit UID in the single-UID response. Accept that
        // fallback, but never attribute a different UID's body to this one.
        if let Some(body) = fetches
            .iter()
            .filter(|f| f.uid.is_none())
            .find_map(|f| f.body())
        {
            policy::check_raw_message_bytes(body.len())?;
            return Ok(Some(body.to_vec()));
        }
        Ok(None)
    }
}

/// One cached body row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedBody {
    pub raw: Vec<u8>,
    pub size: i64,
    pub fetched_at: i64,
}

/// Account-scoped accessor for the `imap_bodies` cache table (migration 57).
///
/// Keyed by the STABLE message id, so a `UIDVALIDITY` reset — which drops a
/// mailbox's `imap_locations` rows via
/// [`ImapStateStore::drop_mailbox_locations`] — never drops a body. Like every
/// other table behind [`ImapStateStore`], it is scoped to one account and is
/// provider-internal: it is a rebuildable cache and is deliberately NOT part of
/// the settings-transfer export (`transfer.rs` is untouched).
#[derive(Clone)]
pub struct BodyCache {
    store: ImapStateStore,
}

impl BodyCache {
    pub fn new(store: ImapStateStore) -> Self {
        Self { store }
    }

    fn account(&self) -> &str {
        self.store.account_id()
    }

    /// Store (or replace) a message body, keyed by its stable id. `fetched_at`
    /// is a caller-supplied unix timestamp so tests stay deterministic.
    pub fn put(&self, message_id: &str, raw: &[u8], fetched_at: i64) -> DbResult<()> {
        let account = self.account().to_string();
        let size = raw.len() as i64;
        self.store.database().with_connection(|connection| {
            connection.execute(
                "INSERT INTO imap_bodies (account_id, message_id, raw, size, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(account_id, message_id) DO UPDATE SET
                     raw = excluded.raw,
                     size = excluded.size,
                     fetched_at = excluded.fetched_at",
                rusqlite::params![account, message_id, raw, size, fetched_at],
            )?;
            Ok(())
        })
    }

    /// Read a cached body back, or `None` when it is not cached.
    pub fn get(&self, message_id: &str) -> DbResult<Option<CachedBody>> {
        let account = self.account().to_string();
        self.store.database().with_connection(|connection| {
            let row = connection
                .query_row(
                    "SELECT raw, size, fetched_at FROM imap_bodies
                     WHERE account_id = ?1 AND message_id = ?2",
                    rusqlite::params![account, message_id],
                    |row| {
                        Ok(CachedBody {
                            raw: row.get(0)?,
                            size: row.get(1)?,
                            fetched_at: row.get(2)?,
                        })
                    },
                )
                .ok();
            Ok(row)
        })
    }

    /// Whether a body is already cached, without reading its bytes.
    pub fn contains(&self, message_id: &str) -> DbResult<bool> {
        let account = self.account().to_string();
        self.store.database().with_connection(|connection| {
            let exists = connection.query_row(
                "SELECT 1 FROM imap_bodies WHERE account_id = ?1 AND message_id = ?2",
                rusqlite::params![account, message_id],
                |_| Ok(()),
            );
            Ok(matches!(exists, Ok(())))
        })
    }

    /// Total cached body bytes for this account (size accounting).
    pub fn total_size(&self) -> DbResult<i64> {
        let account = self.account().to_string();
        self.store.database().with_connection(|connection| {
            let total = connection.query_row(
                "SELECT COALESCE(SUM(size), 0) FROM imap_bodies WHERE account_id = ?1",
                rusqlite::params![account],
                |row| row.get(0),
            )?;
            Ok(total)
        })
    }
}

/// Resolve a message's stable id, record its location, and cache its body
/// exactly once.
///
/// The sequence (`docs/imap-design.md` "Bodies"):
/// 1. **Sticky id.** If `(mailbox, uidvalidity, uid)` is already in
///    `imap_locations`, reuse its recorded `message_id`. A server that later
///    starts advertising OBJECTID must not re-key a message we already know.
/// 2. Otherwise derive the id from the identity inputs (hash route live).
/// 3. Upsert the `imap_locations` row (with flags).
/// 4. If the body is not cached, `BODY.PEEK[]` once and store it.
///
/// A second call for the same message, OR a second copy of it in another
/// mailbox, resolves to the same id and performs NO second body fetch.
/// `fetched_at` is caller-supplied for deterministic tests.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_body(
    body_fetcher: &mut dyn BodyFetcher,
    store: &ImapStateStore,
    cache: &BodyCache,
    mailbox: &str,
    uidvalidity: i64,
    row: &IdentityRow,
    fetched_at: i64,
) -> Result<String, ProviderError> {
    let uid = row.uid as i64;

    // 1. Sticky id: an existing location for this exact coordinate wins.
    let existing = store
        .location_message_id(mailbox, uidvalidity, uid)
        .map_err(db_to_provider)?;
    let message_id = match existing {
        Some(id) => id,
        None => derive_message_id(store.account_id(), &row.inputs),
    };

    // 3. Record where this UID lives (and its flags), id resolved above.
    store
        .upsert_location(&ImapLocation {
            mailbox: mailbox.to_string(),
            uidvalidity,
            uid,
            message_id: message_id.clone(),
            flags_json: serde_json::to_string(&row.flags).unwrap_or_else(|_| "[]".to_string()),
            modseq: None,
        })
        .map_err(db_to_provider)?;

    // 4. Body once: a cached id (reached from any mailbox) is never refetched.
    if !cache.contains(&message_id).map_err(db_to_provider)? {
        // The cheap pass already tells us when the body cannot fit. Do not
        // issue any body request for it; the bounded request also protects
        // against an absent or understated advertised size.
        policy::check_raw_message_bytes(row.inputs.rfc822_size as usize)?;
        if let Some(raw) = body_fetcher.fetch_raw_body(row.uid).await? {
            policy::check_raw_message_bytes(raw.len())?;
            cache
                .put(&message_id, &raw, fetched_at)
                .map_err(db_to_provider)?;
        }
    }

    Ok(message_id)
}

fn db_to_provider(error: crate::db::DatabaseError) -> ProviderError {
    ProviderError::Other(error.to_string())
}

/// A FLAGS-only fetch item: no body, no header fields, no RFC822.SIZE. Used by
/// the per-poll flag sweep, which only needs each UID's current flags and must
/// not re-fetch headers for up to a full window of messages every poll
/// ([`IDENTITY_ITEMS`] stays the new-mail identity pass). `UID` is implicit in
/// a `UID FETCH` response but asked for so the response always carries it.
pub const FLAGS_ITEMS: &str = "(UID FLAGS)";

/// The outcome of resolving a message's id and caching its body WITHOUT
/// writing its location row — the location is deferred to the atomic round
/// commit so a mid-round failure writes nothing durable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBody {
    /// The stable message id (sticky if already recorded, else derived).
    pub message_id: String,
    /// True when the body was skipped because it is oversize/understated: the
    /// message is still recorded (location + thread) but has no cached body,
    /// and the reason is logged. False when the body is cached (or was already).
    pub body_skipped: bool,
}

/// Like [`ensure_body`], but it does NOT write the `imap_locations` row — it
/// resolves the sticky-or-derived id and caches the body, and leaves the
/// location write to the caller's atomic round commit. It also NEVER returns a
/// policy rejection for an oversize message: such a message is recorded with
/// `body_skipped = true` and a logged reason so one bad message cannot wedge a
/// round (SLICE5A_FIXES item 2). Only a transport/auth error from the body
/// fetch propagates.
///
/// `ensure_body`'s Slice-4 contract is untouched; this is the restructured
/// variant the live sync routine uses.
pub async fn resolve_and_cache_body(
    body_fetcher: &mut dyn BodyFetcher,
    store: &ImapStateStore,
    cache: &BodyCache,
    mailbox: &str,
    uidvalidity: i64,
    row: &IdentityRow,
    fetched_at: i64,
) -> Result<ResolvedBody, ProviderError> {
    let uid = row.uid as i64;

    // Sticky id: an existing location for this exact coordinate wins.
    let existing = store
        .location_message_id(mailbox, uidvalidity, uid)
        .map_err(db_to_provider)?;
    let message_id = match existing {
        Some(id) => id,
        None => derive_message_id(store.account_id(), &row.inputs),
    };

    // Body once: a cached id (reached from any mailbox) is never refetched.
    if cache.contains(&message_id).map_err(db_to_provider)? {
        return Ok(ResolvedBody {
            message_id,
            body_skipped: false,
        });
    }

    // Advertised oversize: skip the body, record the message anyway.
    if policy::check_raw_message_bytes(row.inputs.rfc822_size as usize).is_err() {
        log::warn!(
            "imap sync: skipping body for {message_id} (advertised {} bytes over cache limit)",
            row.inputs.rfc822_size
        );
        return Ok(ResolvedBody {
            message_id,
            body_skipped: true,
        });
    }

    match body_fetcher.fetch_raw_body(row.uid).await {
        Ok(Some(raw)) => {
            // A body larger than the limit despite an honest advertised size
            // (understated/absent): skip it, do not fail the round.
            if policy::check_raw_message_bytes(raw.len()).is_err() {
                log::warn!(
                    "imap sync: skipping body for {message_id} ({} bytes over cache limit)",
                    raw.len()
                );
                return Ok(ResolvedBody {
                    message_id,
                    body_skipped: true,
                });
            }
            cache
                .put(&message_id, &raw, fetched_at)
                .map_err(db_to_provider)?;
            Ok(ResolvedBody {
                message_id,
                body_skipped: false,
            })
        }
        Ok(None) => {
            // The server listed this UID in UID SEARCH ALL but returned no
            // body. That is anomalous — most often a connection dropped
            // mid-fetch (async-imap ends the fetch stream on EOF without an
            // error). Treat it as TRANSIENT so the round rolls back and
            // retries, rather than silently recording a permanently body-less
            // message. A genuine oversize was already handled above, before
            // the fetch, so it never reaches here.
            Err(ProviderError::TransientTransport(format!(
                "no body returned for {message_id}; treating as a dropped fetch"
            )))
        }
        // Only a transport/auth failure aborts the round; a policy rejection
        // was already handled above, so anything here is genuine transport.
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use async_trait::async_trait;
    use std::sync::Arc;

    #[test]
    fn the_fetch_item_lists_are_peek_only() {
        for items in [IDENTITY_ITEMS, BODY_ITEMS.as_str()] {
            assert!(items.contains("BODY.PEEK["), "{items} must use BODY.PEEK");
            // No non-peek BODY[ fetch anywhere — that would set \Seen.
            assert!(
                !items.replace("BODY.PEEK[", "").contains("BODY["),
                "{items} must never contain a non-peek BODY["
            );
        }
    }

    #[test]
    fn the_flags_only_item_fetches_no_body_at_all() {
        // SLICE5A_FIXES item 3: the per-poll flag sweep must not re-fetch
        // bodies or headers — FLAGS_ITEMS carries no BODY whatsoever.
        assert_eq!(FLAGS_ITEMS, "(UID FLAGS)");
        assert!(!FLAGS_ITEMS.contains("BODY"), "the flag sweep item has no BODY");
        assert!(!FLAGS_ITEMS.contains("RFC822"), "and no RFC822.SIZE / headers");
    }

    #[test]
    fn identity_headers_parse_and_unfold() {
        let block = b"Message-ID: <abc@x>\r\nSubject: Hello\r\n world\r\nFrom: a@b.com\r\nDate: Mon, 06 Oct 2025 09:00:00 +0000\r\n\r\n";
        let inputs = parse_identity_headers(block, 4242);
        assert_eq!(inputs.message_id.as_deref(), Some("<abc@x>"));
        assert_eq!(inputs.subject.as_deref(), Some("Hello world"));
        assert_eq!(inputs.from.as_deref(), Some("a@b.com"));
        assert_eq!(inputs.rfc822_size, 4242);
    }

    #[test]
    fn email_id_parses_off_a_synthetic_objectid_response() {
        // The OBJECTID path, proven with a synthetic FETCH response because the
        // live async-imap Fetch cannot surface EMAILID.
        let line = b"* 1 FETCH (UID 123 EMAILID (M6d952b5c6f82bfd8) RFC822.SIZE 42)\r\n";
        assert_eq!(parse_email_id(line).as_deref(), Some("M6d952b5c6f82bfd8"));
        // A response with no EMAILID yields None, not a panic.
        let none = b"* 1 FETCH (UID 123 RFC822.SIZE 42)\r\n";
        assert_eq!(parse_email_id(none), None);
        // Garbage never panics.
        assert_eq!(parse_email_id(b"not a fetch line"), None);
    }

    fn store() -> ImapStateStore {
        ImapStateStore::new(Arc::new(Database::open_memory()), "me@example.com")
    }

    // A fake body fetcher that counts calls and serves one fixed body, so
    // "body fetched exactly once" is directly observable. It implements the
    // narrow BodyFetcher seam rather than ImapSession, because async-imap's
    // Fetch cannot be constructed outside its crate.
    struct CountingBodyFetcher {
        body: Vec<u8>,
        fetches: usize,
    }

    #[async_trait]
    impl BodyFetcher for CountingBodyFetcher {
        async fn fetch_raw_body(&mut self, _uid: u32) -> Result<Option<Vec<u8>>, ProviderError> {
            self.fetches += 1;
            Ok(Some(self.body.clone()))
        }
    }

    fn row(uid: u32, message_id: &str) -> IdentityRow {
        IdentityRow {
            uid,
            flags: vec!["\\Seen".into()],
            inputs: IdentityInputs {
                message_id: Some(message_id.into()),
                ..Default::default()
            },
        }
    }

    async fn scripted_session(
        exchanges: Vec<(String, String)>,
    ) -> (
        super::super::session::AsyncImapSession<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (client, server) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
            let exchanges = std::iter::once((
                "LOGIN \"user\" \"password\"".to_owned(),
                "{tag} OK logged in\r\n".to_owned(),
            ))
            .chain(exchanges);
            for (expected, response) in exchanges {
                let mut command = String::new();
                server.read_line(&mut command).await.unwrap();
                let (tag, command) = command.trim_end().split_once(' ').unwrap();
                assert_eq!(command, expected);
                server
                    .get_mut()
                    .write_all(response.replace("{tag}", tag).as_bytes())
                    .await
                    .unwrap();
            }
        });
        let session = async_imap::Client::new(client)
            .login("user", "password")
            .await
            .unwrap();
        (super::super::session::AsyncImapSession::new(session), task)
    }

    #[tokio::test]
    async fn wire_fetch_uses_bounded_peek_and_persists_wire_flags() {
        let headers = "Message-ID: <wire@example.com>\r\nSubject: Wire\r\n\r\n";
        let body = "Subject: Wire\r\n\r\nbody";
        let (mut session, server) = scripted_session(vec![
            (format!("UID FETCH 7 {IDENTITY_ITEMS}"), format!(
                "* 1 FETCH (UID 7 FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft \\Recent Project $Forwarded) RFC822.SIZE {} BODY[HEADER.FIELDS (MESSAGE-ID DATE FROM SUBJECT)] {{{}}}\r\n{headers})\r\n{{tag}} OK done\r\n", body.len(), headers.len()
            )),
            (format!("UID FETCH 7 {}", BODY_ITEMS.as_str()), format!(
                "* 1 FETCH (UID 7 BODY[]<0> {{{}}}\r\n{body})\r\n{{tag}} OK done\r\n", body.len()
            )),
        ]).await;
        let rows = fetch_identity(&mut session, "7").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].inputs.message_id.as_deref(),
            Some("<wire@example.com>")
        );
        assert_eq!(
            rows[0].flags,
            [
                "\\Seen",
                "\\Answered",
                "\\Flagged",
                "\\Deleted",
                "\\Draft",
                "\\Recent",
                "Project",
                "$Forwarded"
            ]
        );
        assert_eq!(
            BODY_ITEMS.as_str(),
            format!("(UID BODY.PEEK[]<0.{}>)", policy::MAX_RAW_MESSAGE_BYTES + 1)
        );
        let store = store();
        let cache = BodyCache::new(store.clone());
        let mut fetcher = SessionBodyFetcher {
            session: &mut session,
        };
        let id = ensure_body(&mut fetcher, &store, &cache, "INBOX", 1, &rows[0], 1)
            .await
            .unwrap();
        assert_eq!(cache.get(&id).unwrap().unwrap().raw, body.as_bytes());
        let locations = store.locations_for_message(&id).unwrap();
        let stored_flags: Vec<String> = serde_json::from_str(&locations[0].flags_json).unwrap();
        assert_eq!(stored_flags, rows[0].flags);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn advertised_size_is_checked_before_any_body_request() {
        for size in [
            policy::MAX_RAW_MESSAGE_BYTES - 1,
            policy::MAX_RAW_MESSAGE_BYTES,
            policy::MAX_RAW_MESSAGE_BYTES + 1,
        ] {
            let store = store();
            let cache = BodyCache::new(store.clone());
            let mut fetcher = CountingBodyFetcher {
                body: b"Subject: Small\r\n\r\nbody".to_vec(),
                fetches: 0,
            };
            let mut row = row(7, "<size@example.com>");
            row.inputs.rfc822_size = size as u32;
            let id = derive_message_id(store.account_id(), &row.inputs);
            let result = ensure_body(&mut fetcher, &store, &cache, "INBOX", 1, &row, 1).await;
            if size > policy::MAX_RAW_MESSAGE_BYTES {
                assert!(matches!(
                    result,
                    Err(ProviderError::PermanentClientRejection(_))
                ));
                assert_eq!(fetcher.fetches, 0);
                assert!(!cache.contains(&id).unwrap());
            } else {
                assert!(result.is_ok());
                assert_eq!(fetcher.fetches, 1);
                assert!(cache.contains(&id).unwrap());
            }
        }
    }

    #[tokio::test]
    async fn wire_body_size_boundary_accepts_complete_messages_and_rejects_the_sentinel() {
        for (size, uid_attribute) in [
            (policy::MAX_RAW_MESSAGE_BYTES - 1, "UID 7 "),
            (policy::MAX_RAW_MESSAGE_BYTES, "UID 7 "),
            (policy::MAX_RAW_FETCH_BYTES, "UID 7 "),
            (policy::MAX_RAW_FETCH_BYTES, ""),
        ] {
            let body = "a".repeat(size);
            let response = format!(
                "* 1 FETCH ({uid_attribute}BODY[]<0> {{{size}}}\r\n{body})\r\n{{tag}} OK done\r\n"
            );
            drop(body);
            let (mut session, server) = scripted_session(vec![(
                format!("UID FETCH 7 {}", BODY_ITEMS.as_str()),
                response,
            )])
            .await;
            let mut fetcher = SessionBodyFetcher {
                session: &mut session,
            };
            let result = fetcher.fetch_raw_body(7).await;
            if size > policy::MAX_RAW_MESSAGE_BYTES {
                assert!(matches!(
                    result,
                    Err(ProviderError::PermanentClientRejection(_))
                ));
            } else {
                assert_eq!(result.unwrap().unwrap().len(), size);
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn understated_or_missing_size_cannot_cache_an_oversize_body() {
        let store = store();
        let cache = BodyCache::new(store.clone());
        let mut fetcher = CountingBodyFetcher {
            body: vec![b'a'; policy::MAX_RAW_FETCH_BYTES],
            fetches: 0,
        };
        for advertised in [0, 10] {
            let mut row = row(7, "<size@example.com>");
            row.inputs.rfc822_size = advertised;
            let result = ensure_body(&mut fetcher, &store, &cache, "INBOX", 1, &row, 1).await;
            assert!(matches!(
                result,
                Err(ProviderError::PermanentClientRejection(_))
            ));
            assert_eq!(cache.total_size().unwrap(), 0);
        }
        assert_eq!(fetcher.fetches, 2);
    }

    #[tokio::test]
    async fn resolve_and_cache_body_skips_an_oversize_message_without_erroring() {
        // SLICE5A_FIXES item 2 core: resolve_and_cache_body never returns a
        // policy rejection — an oversize message is reported body_skipped so a
        // single bad message cannot abort/wedge a round. It also writes NO
        // location (deferred to the atomic round commit).
        let store = store();
        let cache = BodyCache::new(store.clone());

        // Advertised oversize: no body fetched, skipped, no location written.
        let mut fetcher = CountingBodyFetcher {
            body: b"small".to_vec(),
            fetches: 0,
        };
        let mut big = row(7, "<huge@x>");
        big.inputs.rfc822_size = (policy::MAX_RAW_MESSAGE_BYTES + 1) as u32;
        let resolved = resolve_and_cache_body(&mut fetcher, &store, &cache, "INBOX", 1, &big, 1)
            .await
            .unwrap();
        assert!(resolved.body_skipped, "advertised-oversize is skipped");
        assert_eq!(fetcher.fetches, 0, "no body request for an advertised-oversize message");
        assert!(!cache.contains(&resolved.message_id).unwrap(), "body not cached");
        assert!(
            store
                .location_message_id("INBOX", 1, 7)
                .unwrap()
                .is_none(),
            "no location written — that is deferred to the atomic round commit"
        );

        // Understated size: the body IS fetched, found oversize, and skipped —
        // still no error, still no cached body.
        let mut understated = CountingBodyFetcher {
            body: vec![b'a'; policy::MAX_RAW_FETCH_BYTES],
            fetches: 0,
        };
        let mut sneaky = row(8, "<sneaky@x>");
        sneaky.inputs.rfc822_size = 10; // lies
        let resolved = resolve_and_cache_body(
            &mut understated,
            &store,
            &cache,
            "INBOX",
            1,
            &sneaky,
            1,
        )
        .await
        .unwrap();
        assert!(resolved.body_skipped, "understated oversize is skipped");
        assert_eq!(understated.fetches, 1, "the body was fetched then rejected");
        assert!(!cache.contains(&resolved.message_id).unwrap());
    }

    #[tokio::test]
    async fn an_empty_body_for_a_known_uid_is_treated_as_a_dropped_fetch() {
        // async-imap ends a fetch stream on a graceful mid-round connection
        // close WITHOUT an error, so an empty body for a UID the server listed
        // must be treated as TRANSIENT (the round rolls back and retries),
        // never a silent permanent body-skip. A genuine oversize is handled
        // BEFORE the fetch, so it never reaches this arm.
        struct EmptyFetcher;
        #[async_trait]
        impl BodyFetcher for EmptyFetcher {
            async fn fetch_raw_body(&mut self, _uid: u32) -> Result<Option<Vec<u8>>, ProviderError> {
                Ok(None)
            }
        }
        let store = store();
        let cache = BodyCache::new(store.clone());
        let mut fetcher = EmptyFetcher;
        let row = row(5, "<x@x>");
        let result =
            resolve_and_cache_body(&mut fetcher, &store, &cache, "INBOX", 1, &row, 1).await;
        assert!(
            matches!(result, Err(ProviderError::TransientTransport(_))),
            "an empty body for a known UID is a dropped fetch, not a skip: {result:?}"
        );
    }

    #[tokio::test]
    async fn resolve_and_cache_body_propagates_a_transport_error() {
        // A transport/auth failure from the body fetch DOES abort (so the
        // round rolls back): only policy rejections are swallowed as skips.
        struct FailingFetcher;
        #[async_trait]
        impl BodyFetcher for FailingFetcher {
            async fn fetch_raw_body(&mut self, _uid: u32) -> Result<Option<Vec<u8>>, ProviderError> {
                Err(ProviderError::TransientTransport("dropped".into()))
            }
        }
        let store = store();
        let cache = BodyCache::new(store.clone());
        let mut fetcher = FailingFetcher;
        let row = row(9, "<x@x>");
        let result =
            resolve_and_cache_body(&mut fetcher, &store, &cache, "INBOX", 1, &row, 1).await;
        assert!(matches!(result, Err(ProviderError::TransientTransport(_))));
    }

    #[tokio::test]
    async fn one_message_in_two_mailboxes_is_one_id_two_locations_one_body() {
        // The harness-seeded case, at the unit level with the counting fake:
        // the same Message-ID in INBOX and Sent collapses to one id with two
        // location rows and exactly one body fetch.
        let store = store();
        let cache = BodyCache::new(store.clone());
        let mut fetcher = CountingBodyFetcher {
            body: b"Subject: Bcc to self\r\nMessage-ID: <shared@x>\r\n\r\nbody".to_vec(),
            fetches: 0,
        };
        let shared = row(10, "<shared@x>");
        let mut in_sent = shared.clone();
        in_sent.uid = 20; // a different UID in the Sent mailbox

        let id_inbox = ensure_body(&mut fetcher, &store, &cache, "INBOX", 1, &shared, 1000)
            .await
            .unwrap();
        let id_sent = ensure_body(&mut fetcher, &store, &cache, "Sent", 1, &in_sent, 1000)
            .await
            .unwrap();

        assert_eq!(id_inbox, id_sent, "one message id for both copies");
        assert_eq!(fetcher.fetches, 1, "body fetched exactly once");
        let locations = store.locations_for_message(&id_inbox).unwrap();
        let mailboxes: Vec<_> = locations.iter().map(|l| l.mailbox.as_str()).collect();
        assert_eq!(mailboxes, vec!["INBOX", "Sent"]);
        assert!(cache.contains(&id_inbox).unwrap());
        assert_eq!(cache.total_size().unwrap(), fetcher.body.len() as i64);
    }

    #[tokio::test]
    async fn a_second_call_for_the_same_message_does_not_refetch_the_body() {
        let store = store();
        let cache = BodyCache::new(store.clone());
        let mut fetcher = CountingBodyFetcher {
            body: b"Subject: x\r\nMessage-ID: <a@x>\r\n\r\nbody".to_vec(),
            fetches: 0,
        };
        let r = row(5, "<a@x>");
        ensure_body(&mut fetcher, &store, &cache, "INBOX", 1, &r, 1)
            .await
            .unwrap();
        ensure_body(&mut fetcher, &store, &cache, "INBOX", 1, &r, 1)
            .await
            .unwrap();
        assert_eq!(fetcher.fetches, 1);
    }

    #[tokio::test]
    async fn a_sticky_id_is_reused_even_if_the_inputs_would_now_derive_differently() {
        // Pre-seed a location with a known id, then call ensure_body with
        // inputs that WOULD derive a different id (as if the server began
        // advertising OBJECTID). The recorded id must win.
        let store = store();
        let cache = BodyCache::new(store.clone());
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(),
                uidvalidity: 1,
                uid: 7,
                message_id: "imap:me@example.com:STICKY".into(),
                flags_json: "[]".into(),
                modseq: None,
            })
            .unwrap();
        let mut fetcher = CountingBodyFetcher {
            body: b"Subject: x\r\n\r\nbody".to_vec(),
            fetches: 0,
        };
        let mut r = row(7, "<would-derive-differently@x>");
        r.inputs.email_id = Some("NEWOBJECTID".into());
        let id = ensure_body(&mut fetcher, &store, &cache, "INBOX", 1, &r, 1)
            .await
            .unwrap();
        assert_eq!(
            id, "imap:me@example.com:STICKY",
            "the recorded id is reused"
        );
    }

    #[test]
    fn a_uidvalidity_reset_drops_locations_but_keeps_the_cached_body() {
        // Bodies are keyed by stable id, so drop_mailbox_locations must not
        // disturb the cache.
        let store = store();
        let cache = BodyCache::new(store.clone());
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(),
                uidvalidity: 1,
                uid: 1,
                message_id: "imap:me@example.com:keep".into(),
                flags_json: "[]".into(),
                modseq: None,
            })
            .unwrap();
        cache
            .put("imap:me@example.com:keep", b"raw bytes", 42)
            .unwrap();

        store.drop_mailbox_locations("INBOX").unwrap();

        assert!(
            store
                .locations_for_message("imap:me@example.com:keep")
                .unwrap()
                .is_empty(),
            "the location was dropped on the UIDVALIDITY reset"
        );
        let body = cache.get("imap:me@example.com:keep").unwrap().unwrap();
        assert_eq!(body.raw, b"raw bytes");
        assert_eq!(body.size, 9);
        assert_eq!(body.fetched_at, 42);
    }

    #[test]
    fn the_cache_is_account_scoped() {
        let database = Arc::new(Database::open_memory());
        let mine = BodyCache::new(ImapStateStore::new(database.clone(), "me@example.com"));
        let theirs = BodyCache::new(ImapStateStore::new(database, "other@example.com"));
        theirs
            .put("imap:other@example.com:x", b"secret", 1)
            .unwrap();
        assert!(mine.get("imap:other@example.com:x").unwrap().is_none());
        assert_eq!(mine.total_size().unwrap(), 0);
        assert_eq!(theirs.total_size().unwrap(), 6);
    }

    // Optional live coverage against the Dovecot test container. GATED on
    // `DOVECOT_TEST_FP` so the default `cargo test` never needs docker, exactly
    // like the Slice 1/3 live tests:
    //   scripts/dovecot-test-server.sh up
    //   DOVECOT_TEST_FP="$(scripts/dovecot-test-server.sh fingerprint)" \
    //     cargo test --lib provider::imap::fetch::tests::live_ -- --nocapture
    //   scripts/dovecot-test-server.sh down
    //
    // Proves end to end: (1) BODY.PEEK[] fetches a body and `\Seen` is still
    // absent afterwards (guiding rule 3); (2) the harness-seeded message whose
    // Message-ID is in BOTH INBOX and Sent collapses to ONE id with TWO
    // locations and ONE cached body.
    mod live {
        use super::*;
        use crate::provider::imap::connection::{self, ConnectionConfig, TlsMode};
        use crate::provider::imap::session::ImapSession;
        use crate::provider::imap::tls::parse_sha256_fingerprint;

        const HOST: &str = "127.0.0.1";
        const PORT: u16 = 11143;
        const USER: &str = "test@threestrands.test";
        const PASSWORD: &str = "testpassword";

        async fn connect() -> Option<connection::ConnectedSession> {
            let fp_str = std::env::var("DOVECOT_TEST_FP").ok()?;
            let fp = parse_sha256_fingerprint(&fp_str)
                .expect("DOVECOT_TEST_FP must be a 64-hex-digit SHA-256 fingerprint");
            let config = ConnectionConfig {
                host: HOST.to_string(),
                port: PORT,
                tls_mode: TlsMode::StartTls,
                pinned_fingerprint: Some(fp),
            };
            Some(
                connection::connect(&config, USER, PASSWORD)
                    .await
                    .expect("live IMAP login against the Dovecot harness should succeed"),
            )
        }

        /// Flags use their IMAP wire spelling, including the system prefix.
        fn mentions_seen(flags: &[String]) -> bool {
            flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"))
        }

        #[tokio::test]
        async fn live_body_peek_never_sets_seen() {
            let Some(mut session) = connect().await else {
                eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
                return;
            };
            // EXAMINE keeps the mailbox read-only; SELECT would be fine too,
            // the point is the FETCH, not the SELECT.
            session.select("INBOX").await.expect("SELECT INBOX");
            let uids = session.uid_search("ALL").await.expect("UID SEARCH ALL");
            assert!(!uids.is_empty(), "the harness seeds INBOX messages");
            let uid = uids[0];

            // Flags before the body fetch — Seen must be absent on fresh mail.
            let before = fetch_identity(&mut session, &uid.to_string())
                .await
                .expect("identity pass");
            assert!(
                before.iter().all(|r| !mentions_seen(&r.flags)),
                "seeded mail must start unseen: {before:?}"
            );

            // A full BODY.PEEK[] fetch.
            let mut fetcher = SessionBodyFetcher {
                session: &mut session,
            };
            let body = fetcher.fetch_raw_body(uid).await.expect("BODY.PEEK[]");
            assert!(body.is_some(), "the server returns a body");

            // Flags AFTER the peek — still unseen. A plain BODY[] would have
            // set \Seen here; BODY.PEEK[] must not.
            let after = fetch_identity(&mut session, &uid.to_string())
                .await
                .expect("identity pass after peek");
            assert!(
                after.iter().all(|r| !mentions_seen(&r.flags)),
                "BODY.PEEK[] must not set \\Seen: {after:?}"
            );
            let _ = session.logout().await;
        }

        #[tokio::test]
        async fn live_two_seeded_copies_collapse_to_one_id_and_one_body() {
            let Some(mut session) = connect().await else {
                eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
                return;
            };
            let store = ImapStateStore::new(Arc::new(Database::open_memory()), USER);
            let cache = BodyCache::new(store.clone());

            // Walk INBOX then Sent, running the full ensure_body path for every
            // message. The seeded shared Message-ID lives in both.
            let mut ids = std::collections::BTreeSet::new();
            for mailbox in ["INBOX", "Sent"] {
                let status = session.select(mailbox).await.expect("SELECT");
                let uidvalidity = status.uid_validity.unwrap_or(0) as i64;
                let uids = session.uid_search("ALL").await.expect("UID SEARCH ALL");
                let set = uids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let rows = fetch_identity(&mut session, &set)
                    .await
                    .expect("identity pass");
                for row in rows {
                    // SessionBodyFetcher borrows the session mutably only for
                    // the body fetch, released before the next loop turn.
                    let mut fetcher = SessionBodyFetcher {
                        session: &mut session,
                    };
                    let id = ensure_body(
                        &mut fetcher,
                        &store,
                        &cache,
                        mailbox,
                        uidvalidity,
                        &row,
                        1_700_000_000,
                    )
                    .await
                    .expect("ensure_body");
                    ids.insert(id);
                }
            }
            let _ = session.logout().await;

            // The shared-Message-ID copy resolves to one id with two locations.
            let shared = ids
                .iter()
                .find(|id| store.locations_for_message(id).unwrap().len() == 2)
                .expect("the seeded shared Message-ID must have exactly two locations");
            let locations = store.locations_for_message(shared).unwrap();
            let mailboxes: std::collections::BTreeSet<_> =
                locations.iter().map(|l| l.mailbox.as_str()).collect();
            assert!(
                mailboxes.contains("INBOX") && mailboxes.contains("Sent"),
                "the two copies are in INBOX and Sent: {mailboxes:?}"
            );
            // One id, one cached body — the second copy did not refetch.
            assert!(
                cache.contains(shared).unwrap(),
                "the shared message's body is cached once"
            );
        }
    }
}
