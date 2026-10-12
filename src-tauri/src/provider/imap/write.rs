//! The typed, injection-safe, capability-aware WRITE layer (Phase 3 run 1).
//!
//! `docs/imap-design.md` ("Changes (archive, read, star, labels)" and "User
//! labels") describes every mutation the app will make through IMAP: flag
//! stores (`\Seen`, `\Flagged`, keyword labels), moves between mailboxes
//! (archive / trash / spam / move-to-folder), and label-folder copies and
//! expunges. This module is the single audited place those commands are
//! BUILT, so every later run (flags, moves, labels) calls one safe layer
//! rather than interpolating UIDs, flags and mailbox names into command
//! strings itself.
//!
//! **Nothing in this run changes what the app does.** The provider's
//! `modify_thread`/`modify_messages`/label methods still return
//! `InvalidOperation`, and nothing outside tests calls the code here. The
//! first caller is Phase 3 run 2 (flags/moves/labels), so the unavoidable
//! `dead_code` allow below is scoped to this module and names that run.
//!
//! What this layer guarantees, and the design rule each point enforces:
//!
//! * **Injection safety.** [`build_mailbox_arg`] encodes a mailbox name to
//!   modified UTF-7, quotes it and escapes `"`/`\` exactly as async-imap does
//!   on the read path, and REJECTS (typed error, nothing sent) a name
//!   containing CR, LF or NUL. [`validate_flag`] accepts only the system flags
//!   and atom keywords IMAP allows, rejecting the delimiters an attacker would
//!   use to break out of the flag list. UID sets are built from `u32`s and
//!   range-compressed ([`build_uid_set`]), never from caller strings.
//! * **Capability awareness.** [`WriteCaps`] is derived from `CAPABILITY` and a
//!   writable SELECT's `PERMANENTFLAGS`. It decides whether a flag can be
//!   stored (design: "Only permanent flags"), whether `MOVE`/`UID EXPUNGE` are
//!   available, and which relocation strategy is safe — never planning a plain
//!   `EXPUNGE`, which would delete other clients' `\Deleted` mail (design rule
//!   4, "When unsure, do nothing destructive").
//! * **A UIDVALIDITY guard.** [`select_for_write`] is the ONLY way to obtain a
//!   [`WriteContext`], and every write helper takes that context, so a write
//!   against a mailbox whose UIDVALIDITY changed under us is not expressible —
//!   it is refused as a typed "stale, resync first" error before anything is
//!   sent.
//! * **Verified stores.** After a `UID STORE` the layer re-fetches `FLAGS` and
//!   confirms the change took (the Slice 0 finding: a flag outside
//!   `PERMANENTFLAGS` can be silently ignored). A mismatch is a typed error,
//!   not a false success.
//! * **A crash-honest relocate.** [`relocate_uids`] picks `UID MOVE`, else
//!   `UID COPY` + `\Deleted` + `UID EXPUNGE` (UIDPLUS only), else reports the
//!   operation unsupported. If the copy succeeded but the delete/expunge step
//!   failed it reports exactly that partial state (the message now exists in
//!   two places) rather than claiming success, and never expunges a UID the
//!   caller did not name.

#![allow(dead_code)] // First caller is Phase 3 run 2 (flags / moves / labels).

use async_trait::async_trait;

use crate::provider::ProviderError;

use super::policy;
use super::session::{ImapSession, MailboxStatus};

// ===========================================================================
// Typed write operations and the COPYUID result
// ===========================================================================

/// Whether a `UID STORE` ADDS or REMOVES flags. Both are issued with the
/// `.SILENT` suffix so the server does not stream back an untagged `FETCH` for
/// every message (the design uses `±FLAGS.SILENT` everywhere); the verified
/// store does its own explicit re-fetch instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOp {
    /// `+FLAGS.SILENT`.
    Add,
    /// `-FLAGS.SILENT`.
    Remove,
}

impl StoreOp {
    /// The `UID STORE` data-item name for this operation.
    fn item(self) -> &'static str {
        match self {
            StoreOp::Add => "+FLAGS.SILENT",
            StoreOp::Remove => "-FLAGS.SILENT",
        }
    }
}

/// One UIDPLUS `COPYUID` / `APPENDUID` mapping, parsed from the TYPED
/// `imap-proto` response code rather than from a Debug string. `source` and
/// `dest` are expanded from the server's uid-set members (ranges flattened to
/// individual UIDs) and are positionally paired: `source[i]` moved to
/// `dest[i]` (RFC 4315 guarantees the two lists align).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CopyUid {
    pub uidvalidity: u32,
    pub source: Vec<u32>,
    pub dest: Vec<u32>,
}

impl CopyUid {
    /// The `(source_uid -> dest_uid)` pairs this mapping records.
    pub fn pairs(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.source.iter().copied().zip(self.dest.iter().copied())
    }
}

/// The outcome of a `UID COPY` / `UID MOVE`: the `COPYUID` mapping when the
/// server advertised UIDPLUS and returned one, `None` otherwise (the caller
/// then relies on the next sync to find the moved message by its Message-ID,
/// as the design's non-UIDPLUS fallback specifies).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CopyOutcome {
    pub copyuid: Option<CopyUid>,
}

/// A typed outcome for a write that the server answered `TRYCREATE`: the
/// target mailbox is absent and must be created before the write is retried
/// once. Phase 3 needs this as a DISTINCT outcome — today `error.rs` maps
/// `[TRYCREATE]` onto `TransientTransport`, which the engine retries blindly,
/// so a copy into a not-yet-created label folder would retry forever instead
/// of creating the folder. The write layer surfaces it here so run 2's
/// create-then-retry-once logic can branch on it without matching on message
/// substrings.
#[derive(Debug)]
pub enum WriteError {
    /// The target mailbox does not exist; create it and retry the write ONCE.
    /// Carries the server's human-readable text for logging.
    TryCreate(String),
    /// Any other failure, already classified onto a [`ProviderError`] by the
    /// RFC 5530 map. Carried through unchanged.
    Provider(ProviderError),
}

impl WriteError {
    /// Collapse to a plain [`ProviderError`] for callers that do not branch on
    /// `TryCreate`. A `TryCreate` becomes a permanent `InvalidOperation` with
    /// a clear message — NOT a retryable transport error — so a caller that
    /// forgets to handle it fails loudly instead of looping.
    pub fn into_provider_error(self) -> ProviderError {
        match self {
            WriteError::TryCreate(text) => ProviderError::InvalidOperation(format!(
                "target mailbox does not exist (TRYCREATE); create it and retry once: {text}"
            )),
            WriteError::Provider(error) => error,
        }
    }
}

impl From<ProviderError> for WriteError {
    fn from(error: ProviderError) -> Self {
        WriteError::Provider(error)
    }
}

/// A `Result` whose error distinguishes the `TRYCREATE` outcome from every
/// other failure.
pub type WriteResult<T> = Result<T, WriteError>;

// ===========================================================================
// Session trait additions (the write surface)
// ===========================================================================

/// The write operations layered on top of [`ImapSession`]. Kept as a separate
/// trait so the read-only [`ImapSession`] surface is unchanged and the
/// `imap-next` swap the design reserves stays a single-impl change: a new
/// backend implements both traits, nothing else moves.
///
/// Every method speaks in typed arguments — a UID set as `&[u32]`, a store op
/// as [`StoreOp`], a destination as an already-encoded mailbox arg — so a
/// caller cannot smuggle a raw command fragment through. `uid_store` /
/// `uid_expunge` take the already-built UID-set string from [`build_uid_set`]
/// (one audited builder); `uid_copy` / `uid_move` take it plus an
/// already-encoded destination from [`build_mailbox_arg`].
#[async_trait]
pub trait ImapWriteSession: ImapSession {
    /// `UID STORE <set> ±FLAGS.SILENT (<flags>)`. `uid_set` is a built set
    /// string; `flags` are already-validated wire spellings. Does NOT verify
    /// the change — [`verified_store`] layers that on top.
    async fn uid_store(
        &mut self,
        uid_set: &str,
        op: StoreOp,
        flags: &[String],
    ) -> Result<(), ProviderError>;

    /// `UID COPY <set> <dest>`, returning the typed `COPYUID` when present.
    async fn uid_copy(&mut self, uid_set: &str, dest: &str) -> WriteResult<CopyOutcome>;

    /// `UID MOVE <set> <dest>`, returning the typed `COPYUID` when present.
    /// MOVE sends `COPYUID` in an UNTAGGED `* OK [COPYUID ...]` before the
    /// `* n EXPUNGE` lines, so the parser accepts it from either an untagged
    /// data response or the tagged completion.
    async fn uid_move(&mut self, uid_set: &str, dest: &str) -> WriteResult<CopyOutcome>;

    /// `UID EXPUNGE <set>` (UIDPLUS). Only the named UIDs are expunged, never a
    /// plain `EXPUNGE` over the whole mailbox.
    async fn uid_expunge(&mut self, uid_set: &str) -> Result<(), ProviderError>;

    /// `CREATE <arg>` where `arg` is an ALREADY-ENCODED mailbox argument from
    /// [`build_mailbox_arg`] (modified UTF-7, quoted). A `TRYCREATE`/
    /// `NONEXISTENT` on a missing parent surfaces as [`WriteError::TryCreate`]
    /// so the label-create path can create the container and retry; every
    /// other `NO`/`BAD` is classified through the RFC 5530 map. Distinct from
    /// [`ImapSession::create_mailbox`](super::session::ImapSession::create_mailbox),
    /// which takes a DECODED name and lets async-imap quote it (no modified
    /// UTF-7) — the label paths need the audited encoder for user-chosen,
    /// possibly non-ASCII names.
    async fn create_mailbox_arg(&mut self, arg: &str) -> WriteResult<()>;

    /// `DELETE <arg>` where `arg` is an already-encoded mailbox argument. Maps
    /// `NO`/`BAD` through the RFC 5530 map; deleting an absent mailbox is left
    /// to the caller to tolerate (the label-delete path treats it as
    /// idempotent success).
    async fn delete_mailbox_arg(&mut self, arg: &str) -> Result<(), ProviderError>;

    /// `RENAME <from> <to>` where both are already-encoded mailbox arguments.
    /// Maps `NO`/`BAD` through the RFC 5530 map.
    async fn rename_mailbox_arg(&mut self, from: &str, to: &str) -> Result<(), ProviderError>;

    /// `APPEND <mailbox> (<flags>) {<n>}` + the message literal, through the
    /// async-imap library `append` (which performs the `+`-continuation
    /// handshake and writes the literal correctly — Phase 3 run 1 proved a
    /// hand-rolled literal through [`ImapSession::run_command_capture_code`]
    /// desynchronises the connection, so this MUST go through the library).
    ///
    /// `mailbox` is the DECODED mailbox name (not an encoded arg): the impl
    /// encodes it to modified UTF-7 with [`encode_modified_utf7`] and lets
    /// async-imap quote the ASCII result, so a non-ASCII Sent mailbox round
    /// trips correctly (proven by a test). `flags` are the flags to set on the
    /// appended copy (the Sent copy uses `\Seen`); they are validated through
    /// [`validate_flags`] and emitted as a parenthesised list. No INTERNALDATE
    /// is sent (the server stamps "now", which is correct for a just-sent
    /// message).
    ///
    /// Rejected BEFORE any I/O (nothing reaches the wire): a non-ASCII-safe or
    /// hostile mailbox name ([`build_mailbox_arg`]'s control-char/length
    /// rules), an invalid/empty flag list, content containing a NUL byte (an
    /// IMAP4rev1 literal cannot carry one), empty content, and content over
    /// [`policy::APPEND_MAX_BYTES`].
    ///
    /// Error mapping: a server rejection that arrives BEFORE the `+`
    /// continuation (the shape `NO [OVERQUOTA]` / `NO [TRYCREATE]` / a `BAD`
    /// takes for APPEND) is collapsed by async-imap into an opaque
    /// `Error::Append` that DISCARDS the response code, so it maps to a
    /// PERMANENT rejection (definite "refused, nothing stored") with a message
    /// saying the specific code was not reported — the honest limit of using
    /// the library `append`. `save_sent_copy` sidesteps this for the
    /// missing-mailbox case by EXAMINEing first, so a `[TRYCREATE]` is caught
    /// there as a typed missing-Sent-mailbox error. A dropped connection
    /// (mid-literal, or after the store but before the tagged OK) stays
    /// TRANSIENT with its retry class intact (Phase 3 run 3 lesson: never turn
    /// a transient cause into a permanent `InvalidOperation`) — which is what
    /// makes the lost-reply retry idempotent. A tagged `NO` that DOES carry a
    /// code (some servers) is still lifted to [`WriteError::TryCreate`] for
    /// TRYCREATE/NONEXISTENT.
    async fn append(
        &mut self,
        mailbox: &str,
        flags: &[String],
        content: &[u8],
    ) -> WriteResult<()>;
}

// ===========================================================================
// Safe command construction
// ===========================================================================

/// Build the quoted, modified-UTF-7-encoded mailbox argument for a command,
/// or reject the name. This is the one audited place a mailbox name becomes a
/// command token.
///
/// Rejection (a typed error, NOTHING sent) on CR, LF or NUL, and on a name
/// over [`policy::MAX_MAILBOX_NAME_BYTES`]. Otherwise the name is encoded to
/// modified UTF-7 (RFC 3501 §5.1.3), then quoted with `"` and `\` escaped,
/// matching async-imap's own `quote!` on the read path so a round trip is
/// faithful.
pub fn build_mailbox_arg(name: &str) -> Result<String, ProviderError> {
    reject_control_chars(name, "mailbox name")?;
    policy::check_mailbox_name_len(name.len())?;
    let encoded = encode_modified_utf7(name);
    // async-imap quotes with `\` -> `\\` then `"` -> `\"`. The modified-UTF-7
    // output is ASCII and contains neither, except that a literal `&` in the
    // source becomes `&-`; backslash/quote escaping is still applied for
    // safety and parity with the read path.
    let escaped = encoded.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}

/// Catalog, location and settings mailbox names retain LIST's wire spelling.
/// Quote those names without encoding them a second time. Keeping this
/// representation also preserves existing folder/label IDs across upgrades.
pub(super) fn build_wire_mailbox_arg(name: &str) -> Result<String, ProviderError> {
    reject_control_chars(name, "mailbox name")?;
    policy::check_mailbox_name_len(decode_modified_utf7(name)?.len())?;
    Ok(format!(
        "\"{}\"",
        name.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Decode only for display (IDs and persisted coordinates keep wire names).
/// Malformed modified UTF-7 is rejected rather than identifying another mailbox.
pub(super) fn decode_modified_utf7(input: &str) -> Result<String, ProviderError> {
    use base64::Engine;
    let invalid = || ProviderError::InvalidOperation("invalid modified UTF-7 mailbox name".into());
    let mut remaining = input;
    let mut out = String::new();
    while let Some(start) = remaining.find('&') {
        out.push_str(&remaining[..start]);
        remaining = &remaining[start + 1..];
        let end = remaining.find('-').ok_or_else(invalid)?;
        if end == 0 {
            out.push('&');
        } else {
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(remaining[..end].replace(',', "/"))
                .map_err(|_| invalid())?;
            if bytes.len() % 2 != 0 {
                return Err(invalid());
            }
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            out.push_str(&String::from_utf16(&units).map_err(|_| invalid())?);
        }
        remaining = &remaining[end + 1..];
    }
    out.push_str(remaining);
    Ok(out)
}

/// Reject a string that carries CR, LF or NUL — the bytes a command-injection
/// payload (`a\r\nA1 DELETE INBOX`) needs to start a new command line, and the
/// NUL an attacker uses to truncate a C-side parse. `what` names the field for
/// the error message.
fn reject_control_chars(value: &str, what: &str) -> Result<(), ProviderError> {
    if let Some(bad) = value
        .chars()
        .find(|&c| c == '\r' || c == '\n' || c == '\0')
    {
        return Err(ProviderError::InvalidOperation(format!(
            "{what} contains a forbidden control character ({:#04x}); refusing to build a command",
            bad as u32
        )));
    }
    Ok(())
}

/// Validate one flag against the IMAP flag grammar and return its wire
/// spelling, or reject it.
///
/// Accepts the five system flags (`\Seen \Flagged \Deleted \Answered \Draft`,
/// case-insensitively) and atom-only keywords. An atom (RFC 3501 `atom`)
/// forbids the control and special characters a flag list uses as structure:
/// space, parens, braces, quotes, `%`, `*`, `\`, `]`, CR, LF, NUL and the
/// other listed specials. In particular `*` is rejected AS A FLAG even though
/// `\*` is a valid `PERMANENTFLAGS` capability marker — `\*` means "the server
/// permits new keywords", never "set this flag", so it must never be stored.
/// `\Recent` is rejected: it is server-maintained and cannot be set by a
/// client.
pub fn validate_flag(flag: &str) -> Result<String, ProviderError> {
    let trimmed = flag.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::InvalidOperation(
            "empty flag is not storable".into(),
        ));
    }
    // System flags: fixed set, matched case-insensitively, returned in their
    // canonical spelling. `\Recent` and `\*` are deliberately absent.
    for system in ["\\Seen", "\\Flagged", "\\Deleted", "\\Answered", "\\Draft"] {
        if trimmed.eq_ignore_ascii_case(system) {
            return Ok(system.to_string());
        }
    }
    if trimmed.eq_ignore_ascii_case("\\Recent") {
        return Err(ProviderError::InvalidOperation(
            "\\Recent is server-maintained and cannot be stored by a client".into(),
        ));
    }
    if trimmed == "\\*" || trimmed.contains('*') {
        return Err(ProviderError::InvalidOperation(
            "'*' / '\\*' is the new-keyword capability marker, never a flag to set".into(),
        ));
    }
    // A system-style flag we did not recognise (starts with '\\') is refused
    // rather than passed through as a keyword.
    if trimmed.starts_with('\\') {
        return Err(ProviderError::InvalidOperation(format!(
            "unknown system flag {trimmed:?}"
        )));
    }
    // Otherwise it must be a bare atom keyword. Reject every IMAP atom-special
    // and control character.
    if !is_imap_atom(trimmed) {
        return Err(ProviderError::InvalidOperation(format!(
            "keyword {flag:?} is not a valid IMAP atom (contains a space, control, or special character)"
        )));
    }
    Ok(trimmed.to_string())
}

/// Whether `s` is a non-empty RFC 3501 `atom`: no space, no control char, and
/// none of the atom-specials `( ) { } % * " \ ]`.
fn is_imap_atom(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii()
                && !c.is_control()
                && c != ' '
                && !matches!(c, '(' | ')' | '{' | '}' | '%' | '*' | '"' | '\\' | ']')
        })
}

/// Validate a whole flag list for one command: each flag through
/// [`validate_flag`], and the count against [`policy::MAX_FLAGS_PER_COMMAND`].
/// Returns the validated wire spellings. An empty list is rejected (a STORE
/// with no flags is meaningless).
pub fn validate_flags(flags: &[String]) -> Result<Vec<String>, ProviderError> {
    if flags.is_empty() {
        return Err(ProviderError::InvalidOperation(
            "a flag store needs at least one flag".into(),
        ));
    }
    policy::check_flags_per_command(flags.len())?;
    flags.iter().map(|f| validate_flag(f)).collect()
}

/// Compress a UID list into a minimal IMAP uid-set string (`3`, `3,5`,
/// `3:7`, `3:7,9`), from `u32`s — never interpolated caller text. Input order
/// does not matter and duplicates collapse. Returns `None` for an empty set
/// (there is nothing to act on, so the caller must not send a command).
pub fn build_uid_set(uids: &[u32]) -> Option<String> {
    if uids.is_empty() {
        return None;
    }
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut parts: Vec<String> = Vec::new();
    let mut run_start = sorted[0];
    let mut run_end = sorted[0];
    for &uid in &sorted[1..] {
        if uid == run_end + 1 {
            run_end = uid;
        } else {
            parts.push(format_run(run_start, run_end));
            run_start = uid;
            run_end = uid;
        }
    }
    parts.push(format_run(run_start, run_end));
    Some(parts.join(","))
}

fn format_run(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}:{end}")
    }
}

/// Split a UID list into batches of at most [`policy::MAX_UIDS_PER_COMMAND`]
/// UIDs, each compressed to a uid-set string. Duplicates collapse and order is
/// normalised across the whole input before batching, so the batches partition
/// the distinct UIDs in ascending order. Empty input yields no batches.
pub fn build_uid_set_batches(uids: &[u32]) -> Vec<String> {
    uid_batches(uids)
        .iter()
        .filter_map(|batch| build_uid_set(batch))
        .collect()
}

fn uid_batches(uids: &[u32]) -> Vec<Vec<u32>> {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .chunks(policy::MAX_UIDS_PER_COMMAND)
        .map(<[u32]>::to_vec)
        .collect()
}

/// Encode a string to modified UTF-7 as RFC 3501 §5.1.3 defines it (and as
/// async-imap decodes on the read path): printable ASCII (0x20–0x7e) is kept
/// verbatim except that `&` becomes `&-`; any other run of characters is
/// encoded as `&<modified-base64 of its UTF-16BE bytes>-`. The modified base64
/// alphabet replaces `/` with `,` and uses no padding.
pub(super) fn encode_modified_utf7(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut shifted: Vec<u16> = Vec::new();

    let flush = |shifted: &mut Vec<u16>, out: &mut String| {
        if shifted.is_empty() {
            return;
        }
        let mut bytes: Vec<u8> = Vec::with_capacity(shifted.len() * 2);
        for unit in shifted.iter() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        out.push('&');
        out.push_str(&modified_base64_encode(&bytes));
        out.push('-');
        shifted.clear();
    };

    for ch in input.chars() {
        let code = ch as u32;
        if (0x20..=0x7e).contains(&code) {
            flush(&mut shifted, &mut out);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            // Buffer as UTF-16 code units (surrogate pair for astral chars).
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                shifted.push(*unit);
            }
        }
    }
    flush(&mut shifted, &mut out);
    out
}

/// Modified base64 (RFC 3501): standard base64 alphabet with `+/` replaced by
/// `+,` and NO `=` padding. Encodes raw bytes (here, UTF-16BE code units).
fn modified_base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        }
    }
    out
}

// ===========================================================================
// Capability model
// ===========================================================================
/// The relocation strategy a mailbox pair supports, chosen from capabilities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelocationStrategy {
    /// `UID MOVE` — atomic, server-side (`MOVE` advertised).
    Move,
    /// `UID COPY` + `UID STORE +FLAGS.SILENT (\Deleted)` on the source + `UID
    /// EXPUNGE <set>` (no `MOVE`, but `UIDPLUS` present so a targeted expunge
    /// is safe).
    CopyDeleteExpunge,
    /// Neither `MOVE` nor `UIDPLUS`: a relocation cannot be done safely,
    /// because a plain `EXPUNGE` would also remove other clients' `\Deleted`
    /// mail. Carries a plain-language reason.
    Unsupported(String),
}

/// Whether one capability string names `word`.
///
/// [`ImapSession::capabilities`](super::session::ImapSession::capabilities)
/// renders each capability through `format!("{cap:?}")` uppercased, so a plain
/// extension arrives as `ATOM("MOVE")` and `IMAP4rev1` as `IMAP4REV1`. This
/// matches both that debug form (by extracting the quoted inner atom) and a
/// bare `MOVE` word (what the pure unit tests, and any future bare-word
/// caller, pass), case-insensitively — so `WriteCaps` is correct whichever
/// shape it is handed.
fn capability_matches(cap: &str, word: &str) -> bool {
    if cap.eq_ignore_ascii_case(word) {
        return true;
    }
    if let (Some(open), Some(close)) = (cap.find('"'), cap.rfind('"')) {
        if close > open {
            return cap[open + 1..close].eq_ignore_ascii_case(word);
        }
    }
    false
}

/// What the write layer may do on one account, derived from `CAPABILITY` and a
/// writable SELECT's `PERMANENTFLAGS`. Pure data + pure functions, so the whole
/// matrix is testable without a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteCaps {
    /// `MOVE` (RFC 6851) advertised.
    pub has_move: bool,
    /// `UIDPLUS` (RFC 4315) advertised — targeted `UID EXPUNGE` is available
    /// and `COPYUID`/`APPENDUID` are returned.
    pub has_uidplus: bool,
    /// The mailbox's `PERMANENTFLAGS`, wire spellings, from a WRITABLE SELECT.
    /// EXAMINE-derived flags must never populate this (they describe a
    /// read-only selection, not write capability).
    pub permanent_flags: Vec<String>,
    /// `PERMANENTFLAGS` contained `\*`: the server lets clients create
    /// arbitrary new keywords.
    pub permits_new_keywords: bool,
}

impl WriteCaps {
    /// Derive from the uppercased `CAPABILITY` words and a writable SELECT's
    /// status. `UNSELECT` is read for completeness (the design lists it in the
    /// Bridge profile) but needs no stored flag here; `MOVE` and `UIDPLUS` are
    /// what gate behaviour.
    pub fn derive(capabilities: &[String], writable_select: &MailboxStatus) -> Self {
        let has = |word: &str| {
            capabilities
                .iter()
                .any(|cap| capability_matches(cap, word))
        };
        Self {
            has_move: has("MOVE"),
            has_uidplus: has("UIDPLUS"),
            permanent_flags: writable_select.permanent_flags.clone(),
            permits_new_keywords: writable_select.permanent_keywords,
        }
    }

    /// Whether a flag can be stored on this mailbox.
    ///
    /// * `\Recent` is never storable (server-maintained).
    /// * A SYSTEM flag or an EXISTING keyword is storable only if it is listed
    ///   in `PERMANENTFLAGS` (the design's "only permanent flags" rule: a store
    ///   of a flag the mailbox cannot keep must fail, not appear to work until
    ///   the next reselect).
    /// * An ARBITRARY NEW keyword (one not already in `PERMANENTFLAGS`) is
    ///   storable only when `\*` is present.
    ///
    /// `flag` is matched against `PERMANENTFLAGS` case-insensitively on system
    /// flags and exactly on keywords. The flag must already be a valid wire
    /// spelling (callers pass [`validate_flag`]'s output); an invalid flag is
    /// reported as not storable rather than panicking.
    pub fn can_store_flag(&self, flag: &str) -> bool {
        if flag.eq_ignore_ascii_case("\\Recent") || flag == "\\*" {
            return false;
        }
        let listed = self.permanent_flags.iter().any(|permitted| {
            if flag.starts_with('\\') {
                permitted.eq_ignore_ascii_case(flag)
            } else {
                permitted == flag
            }
        });
        if listed {
            return true;
        }
        // Not listed: only an arbitrary NEW keyword is admissible, and only
        // with `\*`. A system flag not in PERMANENTFLAGS is never storable.
        !flag.starts_with('\\') && self.permits_new_keywords
    }

    /// Whether an atomic server-side `UID MOVE` is available.
    pub fn can_move(&self) -> bool {
        self.has_move
    }

    /// Whether a targeted `UID EXPUNGE` of named UIDs is available (UIDPLUS).
    /// Without it the layer never expunges, because a plain `EXPUNGE` would
    /// remove other clients' `\Deleted` mail.
    pub fn can_expunge_uids(&self) -> bool {
        self.has_uidplus
    }

    /// The relocation strategy for this account: `MOVE` if advertised, else
    /// copy+delete+targeted-expunge if `UIDPLUS`, else unsupported with a
    /// plain-language reason.
    pub fn relocation_strategy(&self) -> RelocationStrategy {
        if self.has_move {
            RelocationStrategy::Move
        } else if self.has_uidplus {
            RelocationStrategy::CopyDeleteExpunge
        } else {
            RelocationStrategy::Unsupported(
                "this server supports neither MOVE nor UIDPLUS, so a message cannot be relocated \
                 safely: a plain EXPUNGE would also delete other clients' messages marked \\Deleted"
                    .into(),
            )
        }
    }
}

// ===========================================================================
// UIDVALIDITY guard + writable-select persistence
// ===========================================================================

/// A guarded writable selection of one mailbox: the only token the write
/// helpers accept. Obtaining one requires a writable SELECT whose UIDVALIDITY
/// matched what the caller expected, so an unguarded or stale write is not
/// expressible.
///
/// Holds the (already-encoded) destination arg too would be wrong — this guards
/// the SOURCE mailbox a write acts in; `relocate_uids` encodes the destination
/// separately. The `caps` captured here come from THIS writable select, so a
/// flag or strategy decision is made against the mailbox actually selected.
#[derive(Clone, Debug)]
pub struct WriteContext {
    /// The mailbox's LIST wire name, as used in persisted coordinates.
    pub mailbox: String,
    /// The UIDVALIDITY confirmed at selection.
    pub uidvalidity: u32,
    /// Capabilities + permanent flags learned from this writable select.
    pub caps: WriteCaps,
}

/// SELECT `mailbox` for WRITE and verify its UIDVALIDITY matches
/// `expected_uidvalidity`, returning a [`WriteContext`] the write helpers
/// require. A mismatch is a typed "stale mailbox" error (classified
/// `TransientTransport` so Phase 3 treats it as transient — a sync refreshes
/// the message's location — but NAMED so it is distinguishable) and NOTHING is
/// written. A read-only selection (the server answered `[READ-ONLY]`, surfaced
/// as a permanent error by [`classify_select_outcome`]) is likewise refused
/// before any write.
///
/// After a successful writable select this also persists the learned
/// `PERMANENTFLAGS` through a column-only update, so a later run can decide
/// `can_store_flag` without another round trip; EXAMINE-derived values never
/// overwrite it (the store's `upsert_mailbox` COALESCEs).
pub async fn select_for_write(
    session: &mut dyn ImapWriteSession,
    store: &super::ImapStateStore,
    mailbox: &str,
    expected_uidvalidity: u32,
) -> Result<WriteContext, ProviderError> {
    // A writable SELECT. The session's `select` maps a `[READ-ONLY]` NO to a
    // ProviderError already; a server that SELECTs successfully but read-only
    // is caught below by the absence of writable permanent flags only if it
    // reports none — most servers answer NO, which select() surfaces. We also
    // guard UIDVALIDITY here.
    let status = session.select(mailbox).await?;
    let server_uidvalidity = status.uid_validity.ok_or_else(|| {
        ProviderError::InvalidOperation(format!(
            "mailbox {mailbox:?} reported no UIDVALIDITY on a writable SELECT"
        ))
    })?;
    if server_uidvalidity != expected_uidvalidity {
        return Err(ProviderError::TransientTransport(format!(
            "stale mailbox {mailbox:?}: UIDVALIDITY changed ({expected_uidvalidity} -> \
             {server_uidvalidity}); resync before writing"
        )));
    }
    let caps = WriteCaps::derive(&session.capabilities().await?, &status);

    // Persist the writable PERMANENTFLAGS so later runs decide without a round
    // trip. A failure here must not sink the write path — the caps we just
    // derived are authoritative for this operation — so a persistence error is
    // folded into the returned context by simply not blocking on it.
    persist_writable_permanent_flags(store, mailbox, &status);

    Ok(WriteContext {
        mailbox: mailbox.to_string(),
        uidvalidity: server_uidvalidity,
        caps,
    })
}

/// Persist a writable SELECT's `PERMANENTFLAGS` into the mailbox catalog with
/// a column-only UPDATE (counters and unknown mailboxes are untouched); a store
/// error is swallowed (logged intent only) so a
/// transient DB hiccup never blocks a write the session already proved it can
/// do. Separated so the guard stays readable.
fn persist_writable_permanent_flags(
    store: &super::ImapStateStore,
    mailbox: &str,
    status: &MailboxStatus,
) {
    let flags_json = match serde_json::to_string(&status.permanent_flags) {
        Ok(json) => json,
        Err(_) => return,
    };
    // Column-only UPDATE: never creates a row and never touches the counters
    // the sync gate trusts (an uncatalogued mailbox is simply not persisted;
    // discovery owns creating catalog rows).
    let _ = store.set_mailbox_write_capabilities(
        mailbox,
        &flags_json,
        status.permanent_keywords,
    );
}

// ===========================================================================
// Verified stores
// ===========================================================================

/// Store flags on a UID set and VERIFY the change took, per the design's "only
/// permanent flags" rule and the Slice 0 finding that a flag outside
/// `PERMANENTFLAGS` can be silently ignored by the server.
///
/// 1. Refuse up front any flag the mailbox cannot store
///    ([`WriteCaps::can_store_flag`]) — a permanent `InvalidOperation` naming
///    the flag, so the mutation queue fails the change rather than retrying.
/// 2. `UID STORE <set> ±FLAGS.SILENT (<flags>)`.
/// 3. `UID FETCH <set> (FLAGS)` and confirm every message now reflects the
///    requested change (added flags present on an Add; removed flags absent on
///    a Remove). A mismatch is a permanent `InvalidOperation` naming the first
///    offending flag — NOT a false success.
///
/// `uids` is the raw UID list; it is compressed and batched here through the
/// one audited builder. `flags` must already be validated wire spellings
/// ([`validate_flags`]).
pub async fn verified_store(
    session: &mut dyn ImapWriteSession,
    ctx: &WriteContext,
    uids: &[u32],
    op: StoreOp,
    flags: &[String],
) -> Result<(), ProviderError> {
    if uids.is_empty() {
        return Ok(()); // Nothing to store; sending an empty-set STORE is an error.
    }
    // Capability gate: refuse any flag this mailbox cannot keep.
    for flag in flags {
        if !ctx.caps.can_store_flag(flag) {
            return Err(ProviderError::InvalidOperation(format!(
                "flag {flag:?} is not in PERMANENTFLAGS for {:?} (and the server does not permit \
                 new keywords), so storing it would be silently ignored",
                ctx.mailbox
            )));
        }
    }

    for batch in build_uid_set_batches(uids) {
        session.uid_store(&batch, op, flags).await?;
        // Verify this batch: re-fetch FLAGS and confirm the change took.
        let fetched = session.uid_fetch(&batch, "FLAGS").await?;
        verify_fetched_flags(&fetched, op, flags, &ctx.mailbox)?;
    }
    Ok(())
}

/// Confirm a batch of `UID FETCH (FLAGS)` responses reflects the requested
/// change: on [`StoreOp::Add`] every requested flag must be present on every
/// message; on [`StoreOp::Remove`] every requested flag must be absent. The
/// first violation is a permanent `InvalidOperation` naming the flag.
fn verify_fetched_flags(
    fetched: &[async_imap::types::Fetch],
    op: StoreOp,
    requested: &[String],
    mailbox: &str,
) -> Result<(), ProviderError> {
    for message in fetched {
        let present: Vec<String> = message
            .flags()
            .map(|flag| super::session::flag_to_wire(&flag))
            .collect();
        for flag in requested {
            let here = present
                .iter()
                .any(|p| flags_equal(p, flag));
            let ok = match op {
                StoreOp::Add => here,
                StoreOp::Remove => !here,
            };
            if !ok {
                let verb = match op {
                    StoreOp::Add => "add",
                    StoreOp::Remove => "remove",
                };
                return Err(ProviderError::InvalidOperation(format!(
                    "the server silently ignored a request to {verb} flag {flag:?} in {mailbox:?} \
                     (re-fetched FLAGS did not reflect the change)"
                )));
            }
        }
    }
    Ok(())
}

/// Compare two flag wire spellings: case-insensitive for system flags (those
/// beginning `\`), exact for keywords.
fn flags_equal(a: &str, b: &str) -> bool {
    if a.starts_with('\\') || b.starts_with('\\') {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

// ===========================================================================
// Relocate helper
// ===========================================================================

/// The result of a [`relocate_uids`] call that at least copied the messages.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RelocateOutcome {
    /// The `COPYUID` source->dest mapping when the server returned one; `None`
    /// otherwise (the caller then relies on the next sync to locate the moved
    /// message by its Message-ID).
    pub copyuid: Option<CopyUid>,
}

/// Move a UID set from the guarded source mailbox to `dest`, choosing the safe
/// strategy from capabilities:
///
/// * `UID MOVE` when `MOVE` is advertised (atomic).
/// * else `UID COPY` + `UID STORE +FLAGS.SILENT (\Deleted)` on the SOURCE +
///   `UID EXPUNGE <set>` (UIDPLUS only).
/// * else [`RelocationStrategy::Unsupported`] → a permanent `InvalidOperation`
///   with the plain-language reason.
///
/// CRASH-HONEST: in the copy+delete+expunge path, if the COPY succeeded but the
/// `\Deleted` store or the `UID EXPUNGE` failed, the error names exactly that
/// partial state — the message now exists in BOTH the source and the
/// destination — so Phase 3's retry can be made idempotent. It never expunges
/// a UID the caller did not name (the expunge targets the SAME set). A
/// `TRYCREATE` on the COPY/MOVE surfaces as [`WriteError::TryCreate`], not a
/// retryable transport error.
///
/// `dest` is the destination's persisted LIST wire name, quoted through
/// [`build_wire_mailbox_arg`] without a second modified UTF-7 encoding.
/// `uids` is compressed/batched through the one audited builder.
pub async fn relocate_uids(
    session: &mut dyn ImapWriteSession,
    ctx: &WriteContext,
    uids: &[u32],
    dest: &str,
) -> WriteResult<RelocateOutcome> {
    if uids.is_empty() {
        return Ok(RelocateOutcome::default());
    }
    let dest_arg = build_wire_mailbox_arg(dest).map_err(WriteError::Provider)?;

    match ctx.caps.relocation_strategy() {
        RelocationStrategy::Move => {
            let mut merged = CopyUid::default();
            let mut saw_copyuid = false;
            for batch in build_uid_set_batches(uids) {
                let outcome = session.uid_move(&batch, &dest_arg).await?;
                if let Some(copyuid) = outcome.copyuid {
                    merge_copyuid(&mut merged, copyuid);
                    saw_copyuid = true;
                }
            }
            Ok(RelocateOutcome {
                copyuid: saw_copyuid.then_some(merged),
            })
        }
        RelocationStrategy::CopyDeleteExpunge => {
            require_targeted_delete(ctx).map_err(WriteError::Provider)?;
            let mut merged = CopyUid::default();
            let mut saw_copyuid = false;
            for batch_uids in uid_batches(uids) {
                let batch = build_uid_set(&batch_uids).expect("non-empty batch");
                // 1. COPY. A TRYCREATE here is the typed outcome (dest absent).
                let outcome = session.uid_copy(&batch, &dest_arg).await?;
                if let Some(copyuid) = outcome.copyuid {
                    merge_copyuid(&mut merged, copyuid);
                    saw_copyuid = true;
                }
                // 2. Mark the SOURCE copies \Deleted. The COPY already
                // succeeded, so a failure now is a PARTIAL state.
                let deleted = vec!["\\Deleted".to_string()];
                if let Err(error) =
                    verified_store(session, ctx, &batch_uids, StoreOp::Add, &deleted).await
                {
                    return Err(WriteError::Provider(partial_relocate_error(
                        &ctx.mailbox,
                        dest,
                        &batch,
                        "COPY succeeded but marking the source copies \\Deleted failed",
                        &error,
                    )));
                }
                // 3. Expunge ONLY the named UIDs (UIDPLUS). Still partial if it
                // fails: the copies exist in both places and the source ones
                // are flagged \Deleted.
                if let Err(error) = verified_expunge(session, &batch).await {
                    return Err(WriteError::Provider(partial_relocate_error(
                        &ctx.mailbox,
                        dest,
                        &batch,
                        "COPY and \\Deleted store succeeded but UID EXPUNGE of the source failed",
                        &error,
                    )));
                }
            }
            Ok(RelocateOutcome {
                copyuid: saw_copyuid.then_some(merged),
            })
        }
        RelocationStrategy::Unsupported(reason) => {
            Err(WriteError::Provider(ProviderError::InvalidOperation(reason)))
        }
    }
}

/// Build the typed partial-relocate error, naming the two mailboxes, the UID
/// set left in both places, and the underlying failure. Phase 3 uses this to
/// make the retry idempotent (the message now has two locations).
fn partial_relocate_error(
    source: &str,
    dest: &str,
    uid_set: &str,
    stage: &str,
    cause: &ProviderError,
) -> ProviderError {
    ProviderError::InvalidOperation(format!(
        "partial relocate from {source:?} to {dest:?} for UIDs {uid_set}: {stage}; the messages \
         now exist in BOTH mailboxes — a retry must be idempotent. Cause: {cause}"
    ))
}

/// Like [`partial_relocate_error`], but keeps the CAUSE's retry class. The
/// recording relocate backs the system-move path, whose idempotent retry only
/// runs if the engine actually retries; wrapping a dropped connection or an
/// `UNAVAILABLE` as a permanent `InvalidOperation` would roll the mutation
/// back and strand the message in both mailboxes. Only a cause that is itself
/// permanent stays permanent.
pub(super) fn classified_partial_relocate_error(
    source: &str,
    dest: &str,
    uid_set: &str,
    stage: &str,
    cause: &ProviderError,
) -> ProviderError {
    let text = format!(
        "partial relocate from {source:?} to {dest:?} for UIDs {uid_set}: {stage}; the messages \
         now exist in BOTH mailboxes — a retry must be idempotent. Cause: {cause}"
    );
    match cause {
        ProviderError::TransientTransport(_) => ProviderError::TransientTransport(text),
        ProviderError::RetryableServer(_) => ProviderError::RetryableServer(text),
        ProviderError::Authentication(_) => ProviderError::Authentication(text),
        ProviderError::ReauthenticationRequired(_) => ProviderError::ReauthenticationRequired(text),
        _ => ProviderError::InvalidOperation(text),
    }
}

// ===========================================================================
// The RECORDING relocate (Phase 3 run 3): a crash-honest move whose partial
// failure hands back the destination it already created, so the caller can
// persist that destination location row BEFORE returning the error. The
// idempotent retry then sees the destination copy already recorded and runs
// only the source-side delete+expunge via [`expunge_source_copies`] — it
// never copies again, so a half-applied move never produces a duplicate.
// ===========================================================================

/// A relocate that failed AFTER at least a successful COPY/MOVE of some
/// messages: the destination now holds those copies even though the whole
/// request did not finish. `copyuid` is whatever the server returned for the
/// copies that DID land (it is always present on the copy+delete+expunge path,
/// which requires UIDPLUS), so the caller can record the destination location
/// rows and make the retry idempotent. `error` is the classified failure to
/// propagate (its retry policy — transient vs permanent — is preserved).
#[derive(Debug)]
pub struct RelocateFailure {
    /// The destination mapping for the copies that landed before the failure,
    /// if the server reported one (UIDPLUS). `None` only when nothing was
    /// copied yet (e.g. a TRYCREATE on the very first COPY) or the server did
    /// not return COPYUID.
    pub copyuid: Option<CopyUid>,
    /// The failure to surface, with its original retry classification.
    pub error: WriteError,
}

/// The result of a [`relocate_uids_recording`] call: either the full outcome,
/// or a [`RelocateFailure`] carrying both the destination mapping created so
/// far and the error to propagate.
pub type RecordingRelocateResult = Result<RelocateOutcome, RelocateFailure>;

/// Move a UID set from the guarded source mailbox to `dest`, exactly like
/// [`relocate_uids`], but surfacing a PARTIAL failure's destination mapping so
/// the caller can persist the destination location rows before returning the
/// error (idempotent retry). The strategy choice is unchanged (`MOVE`, else
/// `UID COPY`+`\Deleted`+`UID EXPUNGE`, else `Unsupported`).
///
/// On the copy+delete+expunge path a failure AFTER the COPY returns
/// `Err(RelocateFailure { copyuid: Some(..), error })`: the copies are in the
/// destination, so the caller records them and the retry skips the COPY (it
/// calls [`expunge_source_copies`] on the still-present source UID). A failure
/// BEFORE any copy lands (a TRYCREATE, an `Unsupported` strategy, a COPY that
/// errored) returns `copyuid: None` and NOTHING was written to the
/// destination.
///
/// Callers that move ONE message at a time (the system-move path) pass a
/// single-UID set so the returned `copyuid` maps unambiguously to that message.
pub async fn relocate_uids_recording(
    session: &mut dyn ImapWriteSession,
    ctx: &WriteContext,
    uids: &[u32],
    dest: &str,
) -> RecordingRelocateResult {
    if uids.is_empty() {
        return Ok(RelocateOutcome::default());
    }
    let dest_arg = match build_wire_mailbox_arg(dest) {
        Ok(arg) => arg,
        Err(error) => {
            return Err(RelocateFailure {
                copyuid: None,
                error: WriteError::Provider(error),
            });
        }
    };

    match ctx.caps.relocation_strategy() {
        RelocationStrategy::Move => {
            // MOVE is atomic server-side: either the message moved or it did
            // not. There is no "copied but not deleted" window, so a failure
            // leaves no destination copy to record.
            let mut merged = CopyUid::default();
            let mut saw_copyuid = false;
            for batch in build_uid_set_batches(uids) {
                match session.uid_move(&batch, &dest_arg).await {
                    Ok(outcome) => {
                        if let Some(copyuid) = outcome.copyuid {
                            merge_copyuid(&mut merged, copyuid);
                            saw_copyuid = true;
                        }
                    }
                    Err(error) => {
                        return Err(RelocateFailure {
                            copyuid: None,
                            error,
                        });
                    }
                }
            }
            Ok(RelocateOutcome {
                copyuid: saw_copyuid.then_some(merged),
            })
        }
        RelocationStrategy::CopyDeleteExpunge => {
            if let Err(error) = require_targeted_delete(ctx) {
                return Err(RelocateFailure {
                    copyuid: None,
                    error: error.into(),
                });
            }
            let mut merged = CopyUid::default();
            let mut saw_copyuid = false;
            for batch_uids in uid_batches(uids) {
                let batch = build_uid_set(&batch_uids).expect("non-empty batch");
                // 1. COPY. A TRYCREATE here (dest absent) means NOTHING landed.
                let outcome = match session.uid_copy(&batch, &dest_arg).await {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return Err(RelocateFailure {
                            copyuid: saw_copyuid.then(|| merged.clone()),
                            error,
                        });
                    }
                };
                if let Some(copyuid) = outcome.copyuid {
                    merge_copyuid(&mut merged, copyuid);
                    saw_copyuid = true;
                }
                // 2. Mark the SOURCE copies \Deleted. The COPY landed, so a
                // failure now is a PARTIAL state the caller must record.
                let deleted = vec!["\\Deleted".to_string()];
                if let Err(error) =
                    verified_store(session, ctx, &batch_uids, StoreOp::Add, &deleted).await
                {
                    return Err(RelocateFailure {
                        copyuid: saw_copyuid.then(|| merged.clone()),
                        error: WriteError::Provider(classified_partial_relocate_error(
                            &ctx.mailbox,
                            dest,
                            &batch,
                            "COPY succeeded but marking the source copies \\Deleted failed",
                            &error,
                        )),
                    });
                }
                // 3. Expunge ONLY the named UIDs. Still partial if it fails.
                if let Err(error) = verified_expunge(session, &batch).await {
                    return Err(RelocateFailure {
                        copyuid: saw_copyuid.then(|| merged.clone()),
                        error: WriteError::Provider(classified_partial_relocate_error(
                            &ctx.mailbox,
                            dest,
                            &batch,
                            "COPY and \\Deleted store succeeded but UID EXPUNGE of the source failed",
                            &error,
                        )),
                    });
                }
            }
            Ok(RelocateOutcome {
                copyuid: saw_copyuid.then_some(merged),
            })
        }
        RelocationStrategy::Unsupported(reason) => Err(RelocateFailure {
            copyuid: None,
            error: WriteError::Provider(ProviderError::InvalidOperation(reason)),
        }),
    }
}

/// Finish a half-applied copy+delete+expunge move whose destination copy is
/// ALREADY present (recorded locally after an earlier partial failure): mark
/// the SOURCE UIDs `\Deleted` and `UID EXPUNGE` them, WITHOUT copying again.
/// This is the idempotent-retry step — the one thing that keeps a retried move
/// from producing a duplicate in the destination.
///
/// Only valid on a UIDPLUS server (targeted `UID EXPUNGE`); the caller reaches
/// it only on the copy+delete+expunge strategy, where UIDPLUS is guaranteed.
/// If the source UID is already gone (the earlier failure was only in
/// recording, not on the wire), the `\Deleted` store + expunge of an absent
/// UID is a harmless no-op the server accepts.
pub async fn expunge_source_copies(
    session: &mut dyn ImapWriteSession,
    ctx: &WriteContext,
    uids: &[u32],
) -> Result<(), ProviderError> {
    require_targeted_delete(ctx)?;
    let deleted = vec!["\\Deleted".to_string()];
    verified_store(session, ctx, uids, StoreOp::Add, &deleted).await?;
    for batch in build_uid_set_batches(uids) {
        verified_expunge(session, &batch).await?;
    }
    Ok(())
}

async fn verified_expunge(
    session: &mut dyn ImapWriteSession,
    batch: &str,
) -> Result<(), ProviderError> {
    session.uid_expunge(batch).await?;
    if !session.uid_fetch(batch, "FLAGS").await?.is_empty() {
        return Err(ProviderError::TransientTransport(
            "UID EXPUNGE left requested copies present; resync before retrying".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_targeted_delete(ctx: &WriteContext) -> Result<(), ProviderError> {
    if !ctx.caps.can_expunge_uids() || !ctx.caps.can_store_flag("\\Deleted") {
        return Err(ProviderError::InvalidOperation(format!(
            "cannot delete copies in {:?}: UIDPLUS and permanent \\Deleted are required",
            ctx.mailbox
        )));
    }
    Ok(())
}

/// Merge a per-batch `COPYUID` into the accumulating one. The uidvalidity is
/// taken from the first batch (all batches of one relocate share the
/// destination's uidvalidity); source/dest pairs are appended.
fn merge_copyuid(into: &mut CopyUid, from: CopyUid) {
    if into.source.is_empty() && into.dest.is_empty() {
        into.uidvalidity = from.uidvalidity;
    }
    into.source.extend(from.source);
    into.dest.extend(from.dest);
}

// ===========================================================================
// Parsing the typed COPYUID response code + SELECT-outcome classification
// ===========================================================================

/// Expand `imap-proto`'s uid-set members (individual UIDs and ranges) into a
/// flat ascending UID list. Shared by the COPYUID parser.
pub(super) fn expand_uid_set_members(
    members: &[async_imap::imap_proto::rfc4315::UidSetMember],
) -> Vec<u32> {
    use async_imap::imap_proto::rfc4315::UidSetMember;
    let mut out = Vec::new();
    for member in members {
        match member {
            UidSetMember::Uid(uid) => out.push(*uid),
            UidSetMember::UidRange(range) => out.extend(range.clone()),
        }
    }
    out
}

/// Build a [`CopyUid`] from a typed `imap-proto` `ResponseCode::CopyUid`. The
/// escape hatch the Slice 0 spike proved: read the TYPED code, never the Debug
/// string. Returns `None` for any other code.
pub(super) fn copyuid_from_code(
    code: &async_imap::imap_proto::ResponseCode<'_>,
) -> Option<CopyUid> {
    use async_imap::imap_proto::ResponseCode;
    match code {
        ResponseCode::CopyUid(uidvalidity, source, dest) => Some(CopyUid {
            uidvalidity: *uidvalidity,
            source: expand_uid_set_members(source),
            dest: expand_uid_set_members(dest),
        }),
        _ => None,
    }
}

/// Classify a SELECT outcome's response code into a write-relevant verdict. A
/// `[READ-ONLY]` after SELECT means the mailbox cannot be written at all
/// (permanent). Exposed for the session impl, which inspects the SELECT
/// completion. (Most servers answer a plain `NO` for a read-only mailbox,
/// which the error map already handles; this covers the `OK [READ-ONLY]`
/// variant some servers send.)
pub(super) fn select_is_read_only(code: &async_imap::imap_proto::ResponseCode<'_>) -> bool {
    matches!(code, async_imap::imap_proto::ResponseCode::ReadOnly)
}

// ===========================================================================
// The Sent-copy APPEND primitive (Phase 4 run 4a)
// ===========================================================================
//
// `docs/imap-design.md` ("Sending", steps 3–4). After SMTP accepts a message,
// IMAP saves the sender's own copy in the Sent mailbox with `APPEND` (flag
// `\Seen`). The one hard requirement is IDEMPOTENCE: calling the primitive
// twice — a deliberate retry, or a retry after a reply was lost — must never
// produce two copies. Run 4b decides WHEN to call this (first-send detection
// against `settings.server_saves_sent`, a durable retry job); run 4a is only
// the correct, well-tested primitive and its session plumbing.
//
// NOTE on Bcc: the raw bytes saved here are the OUTBOX raw, UNMODIFIED — they
// still carry the `Bcc:` header. Only the SMTP WIRE copy had Bcc stripped
// (`send::strip_bcc_header`); a sender's own Sent copy keeps its Bcc line, so
// they can see whom they blind-copied. This primitive deliberately does NOT
// strip anything.

/// Prepare the validated wire inputs for an `APPEND`: the mailbox name and the
/// parenthesised flag list. All rejection happens here, BEFORE any byte reaches
/// the wire.
///
/// `mailbox` is the mailbox's WIRE spelling — exactly what LIST returned and
/// what the catalog, the plan and the settings overrides store (modified
/// UTF-7). It is validated (control characters, length) and returned
/// UNCHANGED for async-imap to quote: encoding it again would turn
/// "Entw&APw-rfe" into "Entw&-APw-rfe" and address a different, nonexistent
/// mailbox, and turn an escaped "&-" into "&--". (Phase 4 run 4a's first
/// version encoded here; the provider passes the plan's wire name, so every
/// non-ASCII Sent mailbox would have failed.)
pub(super) fn prepare_append(
    mailbox: &str,
    flags: &[String],
    content: &[u8],
) -> Result<(String, String), ProviderError> {
    // The audited WIRE-name gate (control chars + length), as every other
    // path that takes a catalog name uses. Discard the quoted form: the
    // library quotes the name itself.
    let _ = build_wire_mailbox_arg(mailbox)?;
    let wire = mailbox.to_string();

    let validated = validate_flags(flags)?;
    let flag_list = format!("({})", validated.join(" "));

    if content.is_empty() {
        return Err(ProviderError::InvalidOperation(
            "refusing to APPEND an empty message".into(),
        ));
    }
    if content.contains(&0u8) {
        return Err(ProviderError::InvalidOperation(
            "message contains a NUL byte, which an IMAP4rev1 literal cannot carry; refusing to \
             APPEND"
                .into(),
        ));
    }
    policy::check_append_bytes(content.len())?;

    Ok((wire, flag_list))
}

/// Map an `async-imap` APPEND error to a [`WriteError`], recovering the typed
/// `TRYCREATE`/`NONEXISTENT` outcome from the lossy `Error::No` text.
///
/// async-imap collapses a tagged `NO [TRYCREATE] ...` into
/// `Error::No("code: Some(TryCreate), info: Some(..)")`; the shared RFC 5530
/// classifier recognises that exact shape as [`ImapFailure::TryCreate`]
/// (and a bare `[NONEXISTENT]`-coded NO likewise), so we lift it to the typed
/// [`WriteError::TryCreate`] here. Everything else goes through the normal map
/// ([`map_imap_error`]), preserving its retry class — a dropped connection
/// stays [`ProviderError::TransientTransport`], `[OVERQUOTA]` stays permanent.
pub(super) fn map_append_error(err: &async_imap::error::Error) -> WriteError {
    use super::error::{classify_async_imap_status_text, ImapFailure};
    use async_imap::error::Error as E;
    if let E::No(text) | E::Bad(text) = err {
        match classify_async_imap_status_text(text) {
            ImapFailure::TryCreate(info) => return WriteError::TryCreate(info),
            // NONEXISTENT arrives WITHOUT a recognised code word (imap-proto
            // does not type it), so it classifies as `Bad`; a Sent mailbox
            // that is simply gone is the same "create it / the caller decides"
            // situation as TRYCREATE for this primitive, so detect it by its
            // (bracketed, server-neutral) code position.
            _ if code_word_is(text, "NONEXISTENT") => {
                return WriteError::TryCreate(text.clone());
            }
            _ => {}
        }
    }
    // async-imap collapses a server rejection that arrives BEFORE the `+`
    // continuation (the correct protocol shape for an APPEND the server refuses
    // outright — `NO [OVERQUOTA]`, `NO [TRYCREATE]`, a `BAD`) into the opaque
    // `Error::Append`, discarding the response code. By the time `save_sent_
    // copy` APPENDs it has already EXAMINEd the mailbox, so a missing-mailbox
    // TRYCREATE cannot reach here through that path; a bare `Error::Append` is
    // therefore a DEFINITE "the server refused and stored nothing" — a
    // PERMANENT rejection, never a transient retry (a mid-literal DROP is an
    // `Io`/`ConnectionLost`, which maps to transient below, so the lost-reply
    // retry still works). We cannot recover the specific code async-imap threw
    // away; the message says so.
    if matches!(err, E::Append) {
        return WriteError::Provider(ProviderError::PermanentClientRejection(
            "the mail server refused the APPEND of the Sent copy (e.g. over quota or a rejected \
             message); it was not saved. The specific reason was not reported by the IMAP library."
                .into(),
        ));
    }
    WriteError::Provider(super::error::map_imap_error(err))
}

/// Whether the async-imap `Error::No`/`Bad` debug text carries the bracketed
/// RFC 5530 code `word` in its leading code position. Server-neutral: it reads
/// only the `info: Some("[WORD] …")` field async-imap formats, never a mailbox
/// name or diagnostic tail, so `Unknown mailbox: NONEXISTENT` is NOT matched.
fn code_word_is(async_imap_no_text: &str, word: &str) -> bool {
    // Shape: `code: None, info: Some("[WORD] human text")` (or `outcome:` for
    // some parsers). Find the quoted information and read its bracketed head.
    let Some(open_quote) = async_imap_no_text.find('"') else {
        return false;
    };
    let tail = &async_imap_no_text[open_quote + 1..];
    tail.trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map(|(code, _)| code.eq_ignore_ascii_case(word))
        .unwrap_or(false)
}

/// A located copy of a message in one mailbox: the mailbox's `UIDVALIDITY` at
/// the time of the search and the UID(s) of every message whose `Message-ID`
/// header EXACTLY matched (there is normally one; more than one means the
/// server already holds duplicates, which the caller treats as "present").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub uidvalidity: u32,
    pub uids: Vec<u32>,
}

/// The outcome of [`save_sent_copy`]. The three states are the whole
/// idempotency contract made explicit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SentCopy {
    /// A copy with this `Message-ID` already existed in the Sent mailbox, so
    /// NOTHING was appended. The idempotent outcome of a retry.
    AlreadyPresent { uidvalidity: u32, uids: Vec<u32> },
    /// We appended the copy and then located it by `Message-ID`.
    Appended { uidvalidity: u32, uid: u32 },
    /// `APPEND` returned OK but the follow-up search did not find the copy
    /// within the bounded re-checks. This is SUCCESS: the message is stored and
    /// the next sync will discover it. The caller must NOT retry the append on
    /// this outcome (that is exactly how a duplicate would be created).
    AppendedUnlocated { uidvalidity: u32 },
}

/// Injectable timing/limits for [`save_sent_copy`], so tests run instantly.
#[derive(Clone, Copy, Debug)]
pub struct SaveSentConfig {
    /// How many times to re-run the locate SEARCH after a successful APPEND
    /// whose first search found nothing, before returning `AppendedUnlocated`.
    pub relocate_rechecks: usize,
    /// Delay between those re-checks. Zero in tests.
    pub recheck_delay: std::time::Duration,
}

impl Default for SaveSentConfig {
    fn default() -> Self {
        Self {
            relocate_rechecks: policy::SENT_COPY_RELOCATE_RECHECKS,
            // A short settle so a server that makes a stored message visible to
            // SEARCH only after a beat is given a chance before we give up and
            // leave discovery to the next sync. Run 4b owns the real per-send
            // serialisation + settle; this is a conservative default.
            recheck_delay: std::time::Duration::from_millis(200),
        }
    }
}

/// Find every copy of `message_id` in the SELECTED/EXAMINED `mailbox` and
/// return their UIDs plus the mailbox UIDVALIDITY, VERIFYING each hit.
///
/// `UID SEARCH HEADER Message-ID "<value>"` is a SUBSTRING match (RFC 3501
/// §6.4.4), so a hit is only a CANDIDATE: each is confirmed by fetching its own
/// `Message-ID` header (`BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)]`, which does not
/// set `\Seen`) and comparing the normalized token EXACTLY. A near-miss
/// (`<abc@x>` vs `<xabc@x>` or `<abc@xy>`) is rejected.
///
/// `message_id` must be our OWN generated id (the raw header value); it is
/// validated to contain no quote, backslash, CR, LF, NUL or whitespace before
/// anything is sent, so it cannot break out of the quoted search string. The
/// number of candidates examined is bounded by
/// [`policy::MAX_MESSAGE_ID_SEARCH_HITS`].
///
/// `uidvalidity` is read from the caller's prior SELECT/EXAMINE (passed in),
/// since SEARCH does not report it.
pub async fn find_by_message_id(
    session: &mut dyn ImapWriteSession,
    message_id: &str,
    uidvalidity: u32,
) -> Result<Located, ProviderError> {
    let normalized_target = validate_search_message_id(message_id)?;

    // SEARCH HEADER with the raw id value, quoted. The value is validated to
    // carry no quote/backslash/CR/LF/NUL/whitespace, so this quoting is safe.
    let query = format!("HEADER Message-ID \"{message_id}\"");
    let mut hits = session.uid_search(&query).await?;
    hits.sort_unstable();
    hits.dedup();
    policy::check_message_id_search_hits(hits.len())?;

    // Verify each candidate by fetching ITS Message-ID header and comparing
    // the normalized token exactly.
    let mut confirmed = Vec::new();
    for uid in hits {
        let Some(set) = build_uid_set(&[uid]) else {
            continue;
        };
        let fetched = session
            .uid_fetch(&set, "BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)]")
            .await?;
        for message in &fetched {
            if let Some(header_bytes) = message.header() {
                if header_message_id_matches(header_bytes, &normalized_target) {
                    confirmed.push(uid);
                    break;
                }
            }
        }
    }
    confirmed.sort_unstable();
    confirmed.dedup();
    Ok(Located {
        uidvalidity,
        uids: confirmed,
    })
}

/// Validate that a Message-ID is safe to embed in a quoted SEARCH string and
/// return its normalized token for exact comparison. Rejects (nothing sent)
/// any quote, backslash, CR, LF, NUL or ASCII whitespace, and an id whose
/// normalized form is empty.
fn validate_search_message_id(message_id: &str) -> Result<String, ProviderError> {
    for ch in message_id.chars() {
        if matches!(ch, '"' | '\\' | '\r' | '\n' | '\0') || (ch.is_whitespace()) {
            return Err(ProviderError::InvalidOperation(format!(
                "Message-ID contains a forbidden character ({:#04x}) for a SEARCH; refusing to \
                 build a command",
                ch as u32
            )));
        }
    }
    let normalized = super::identity::normalize_message_id(message_id);
    if normalized.is_empty() {
        return Err(ProviderError::InvalidOperation(
            "Message-ID normalizes to empty; refusing to search for it".into(),
        ));
    }
    Ok(normalized)
}

/// Whether a fetched `HEADER.FIELDS (MESSAGE-ID)` block carries a `Message-ID`
/// whose normalized token EXACTLY equals `normalized_target`. The block is the
/// header line(s) the server returned; we take the value after the first
/// colon, normalize it with the same [`identity::normalize_message_id`] every
/// other part of the provider compares by, and compare exactly.
fn header_message_id_matches(header_bytes: &[u8], normalized_target: &str) -> bool {
    let text = match std::str::from_utf8(header_bytes) {
        Ok(text) => text,
        // A Message-ID header is ASCII; non-UTF-8 bytes cannot be our own id.
        Err(_) => return false,
    };
    // The block may contain the field name and folded continuation lines. Join
    // continuation lines, then take everything after the first colon.
    let unfolded = text.replace("\r\n", "\n").replace('\n', " ");
    let Some((name, value)) = unfolded.split_once(':') else {
        return false;
    };
    if !name.trim().eq_ignore_ascii_case("message-id") {
        return false;
    }
    super::identity::normalize_message_id(value) == normalized_target
}

/// Save the sender's own copy of a just-sent message into `mailbox` (the Sent
/// mailbox), IDEMPOTENTLY. The core guarantee of Phase 4 run 4.
///
/// Algorithm:
/// 1. EXAMINE the mailbox for its UIDVALIDITY (read-only — we never need a
///    writable select to APPEND, and EXAMINE never sets `\Seen` on the seed
///    messages). A `[TRYCREATE]`/`[NONEXISTENT]`/absent mailbox is a typed
///    permanent error naming the mailbox; this primitive does NOT create it.
/// 2. [`find_by_message_id`]; if any copy exists → `AlreadyPresent` (nothing
///    appended).
/// 3. `APPEND` the raw bytes with flag `\Seen`, no INTERNALDATE.
/// 4. Re-run [`find_by_message_id`] (a short bounded re-check from `config`,
///    injectable) to locate the UID → `Appended`, else `AppendedUnlocated`.
///
/// IDEMPOTENCY CONTRACT (the heart of this run): if step 3 fails with ANY
/// transient/unknown error — INCLUDING the connection dropping AFTER the server
/// stored the message but BEFORE the tagged OK reached us — the caller will
/// retry; the retry's step 2 then finds the copy and returns `AlreadyPresent`
/// with exactly ONE copy on the server. Only a DEFINITE server `NO` (before
/// storing, e.g. `[OVERQUOTA]`) is treated as "not appended". This function
/// NEVER EXPUNGEs or deletes anything.
///
/// RESIDUAL RACE: a server that makes a just-stored message visible to SEARCH
/// only after a delay can, on a fast retry, pass step 2 (not yet visible) and
/// APPEND a second copy. The bounded re-check narrows this; run 4b closes it by
/// SERIALISING per message and adding a settle delay before any retry's step 2.
/// On this primitive alone the window is the re-check budget wide, documented
/// here rather than hidden.
pub async fn save_sent_copy(
    session: &mut dyn ImapWriteSession,
    mailbox: &str,
    raw: &[u8],
    message_id: &str,
    config: SaveSentConfig,
) -> WriteResult<SentCopy> {
    // Validate the APPEND inputs up front (nothing on the wire if they are
    // bad). We need the validated flag list only inside `append`, but failing
    // fast here keeps a bad id/content from even EXAMINEing.
    let seen = vec!["\\Seen".to_string()];
    prepare_append(mailbox, &seen, raw).map_err(WriteError::Provider)?;
    // Validate the id now too, so a hostile id is refused before any command.
    validate_search_message_id(message_id).map_err(WriteError::Provider)?;

    // 1. EXAMINE for UIDVALIDITY. A missing Sent mailbox is a typed permanent
    // error (NoSentMailbox-style); the caller decides whether to create one.
    let uidvalidity = examine_for_append(session, mailbox).await?;

    // 2. Already present? Then nothing to do (idempotent).
    let existing = find_by_message_id(session, message_id, uidvalidity)
        .await
        .map_err(WriteError::Provider)?;
    if !existing.uids.is_empty() {
        return Ok(SentCopy::AlreadyPresent {
            uidvalidity: existing.uidvalidity,
            uids: existing.uids,
        });
    }

    // 3. APPEND with \Seen. A definite NO (OVERQUOTA, …) is "not appended" and
    // propagates; a TRYCREATE is the typed missing-mailbox outcome; a transient
    // error (incl. a drop after the server stored it) propagates so the caller
    // retries and step 2 then finds the copy.
    session.append(mailbox, &seen, raw).await?;

    // 4. Locate the appended copy, with bounded re-checks for a server that
    // makes it visible to SEARCH only after a beat.
    let mut attempts = 0;
    loop {
        let located = find_by_message_id(session, message_id, uidvalidity)
            .await
            .map_err(WriteError::Provider)?;
        if let Some(&uid) = located.uids.first() {
            return Ok(SentCopy::Appended {
                uidvalidity: located.uidvalidity,
                uid,
            });
        }
        if attempts >= config.relocate_rechecks {
            // Stored but not yet locatable: success, the next sync finds it.
            // Never retry the APPEND on this outcome.
            return Ok(SentCopy::AppendedUnlocated { uidvalidity });
        }
        attempts += 1;
        if !config.recheck_delay.is_zero() {
            tokio::time::sleep(config.recheck_delay).await;
        }
    }
}

/// EXAMINE `mailbox` and return its UIDVALIDITY, mapping an absent mailbox to a
/// typed PERMANENT [`WriteError`] naming it (this primitive never creates the
/// Sent mailbox). A `[TRYCREATE]`/`[NONEXISTENT]` NO becomes a
/// permanent `InvalidOperation` (NOT the retryable TryCreate — there is no
/// create-and-retry here, the caller decides out of band). A missing
/// UIDVALIDITY is a permanent error (we cannot anchor a `Located` without it).
async fn examine_for_append(
    session: &mut dyn ImapWriteSession,
    mailbox: &str,
) -> WriteResult<u32> {
    // `mailbox` is the WIRE spelling from the catalog/plan; async-imap's
    // `examine` quotes it, so it goes out verbatim (see `prepare_append`).
    let _ = build_wire_mailbox_arg(mailbox).map_err(WriteError::Provider)?;
    match session.examine(mailbox).await {
        Ok(status) => status.uid_validity.ok_or_else(|| {
            WriteError::Provider(ProviderError::InvalidOperation(format!(
                "Sent mailbox {mailbox:?} reported no UIDVALIDITY on EXAMINE; cannot save a copy"
            )))
        }),
        Err(error) => Err(WriteError::Provider(no_sent_mailbox_error(mailbox, &error))),
    }
}

/// Build the typed "no Sent mailbox" error from an EXAMINE failure. A missing
/// mailbox — `[TRYCREATE]` or `[NONEXISTENT]`, whichever the server sends —
/// becomes a PERMANENT `InvalidOperation` naming the mailbox, so run 4b can
/// present "configure a Sent mailbox" rather than retrying forever (this
/// primitive never creates it). A genuinely transient EXAMINE failure (a
/// dropped connection, `[UNAVAILABLE]`) KEEPS its transient class so the caller
/// retries (Phase 3 run 3 lesson: never turn a transient cause permanent).
fn no_sent_mailbox_error(mailbox: &str, cause: &ProviderError) -> ProviderError {
    // `mailbox` is the wire spelling; show the user the readable name.
    let display = decode_modified_utf7(mailbox).unwrap_or_else(|_| mailbox.to_string());
    let mailbox = display.as_str();
    // A missing mailbox is reported by the RFC 5530 map either as the Slice-3
    // TRYCREATE-transient placeholder (text names TRYCREATE) or as a plain
    // InvalidOperation (NONEXISTENT has no typed code). Both mean "the mailbox
    // is not there" — a permanent configuration issue for this primitive.
    let text = cause.to_string();
    let missing = text.contains("TRYCREATE") || text.to_ascii_uppercase().contains("NONEXISTENT");
    if missing {
        return ProviderError::InvalidOperation(format!(
            "cannot save the Sent copy: the Sent mailbox {mailbox:?} does not exist; configure or \
             create it, then retry ({cause})"
        ));
    }
    match cause {
        // A transient EXAMINE failure KEEPS its class so the caller retries;
        // ProviderError is not Clone, so re-wrap the cause's text in the same
        // variant rather than cloning it.
        ProviderError::TransientTransport(_) => ProviderError::TransientTransport(text),
        ProviderError::RetryableServer(_) => ProviderError::RetryableServer(text),
        ProviderError::Authentication(_) => ProviderError::Authentication(text),
        ProviderError::ReauthenticationRequired(_) => {
            ProviderError::ReauthenticationRequired(text)
        }
        // Any other failure (a plain NO/BAD) is a permanent inability to reach
        // the Sent mailbox.
        _ => ProviderError::InvalidOperation(format!(
            "cannot save the Sent copy: the Sent mailbox {mailbox:?} is not available ({cause}); \
             configure or create it, then retry"
        )),
    }
}

// ===========================================================================
// Phase 4 run 4b: the Sent-copy DECISION (first-send detection)
// ===========================================================================
//
// `docs/imap-design.md` ("Sending" steps 3–4). The engine owns a durable job
// and WHEN to call this; the decision below owns WHAT to do, and is factored
// into a pure async function over a tiny IO trait so the whole state machine is
// table-testable without a server. The provider wires a real session + store
// into the trait; a test wires a scripted fake.
//
// The decision is driven by three inputs the engine carries in a
// `SentCopyRequest`: the user override `settings.server_saves_sent`, whether an
// APPEND of ours was already attempted (`append_attempted`), and the job phase
// (`Pending` first attempt / `Verify` after our APPEND). The EVIDENCE RULE is
// the subtle part: ONLY a copy found BEFORE any APPEND of ours is evidence the
// server saves its own copies. A copy found once `append_attempted` is true may
// be our own (a lost-reply retry), so it is treated as success WITHOUT recording
// "server saves sent".

use crate::provider::{SentCopyPhase, SentCopyStep};

/// The small IO surface the Sent-copy decision needs, so the state machine is
/// pure and table-testable. The provider implements it against a real session
/// + `ImapStateStore`; a test implements it with in-memory fakes.
#[async_trait]
pub(super) trait SentCopyIo {
    /// The user override `imap_account_settings.server_saves_sent` (true =
    /// never APPEND). Read-only, no network.
    fn server_saves_sent_override(&self) -> bool;

    /// The per-device detection result: `Some(true)` server saves its own,
    /// `Some(false)` it does not, `None` not yet decided. No network.
    async fn detection(&mut self) -> Result<Option<bool>, ProviderError>;

    /// Record the per-device detection result. No network.
    async fn record_detection(&mut self, server_saves_sent: bool) -> Result<(), ProviderError>;

    /// Count the copies of our Message-ID currently in the Sent mailbox
    /// (verified exact matches). Used both to find a pre-APPEND server copy and
    /// to count copies in the verify phase. One SEARCH + verify FETCHes.
    async fn count_in_sent(&mut self) -> Result<usize, ProviderError>;

    /// APPEND our copy to the Sent mailbox, idempotently (run 4a's
    /// `save_sent_copy`: it searches before it appends). Returns Ok on a stored
    /// (or already-present) copy; an error carries its retry class.
    async fn append_copy(&mut self) -> Result<(), ProviderError>;
}

/// The pure Sent-copy decision over [`SentCopyIo`]. Returns the engine-facing
/// [`SentCopyStep`] (Done / Recheck) or a classified error. Never deletes,
/// expunges or flags anything. The engine translates the step into job state.
///
/// `verify_delay` is injected (the policy `SENT_COPY_VERIFY_DELAY` in
/// production, zero in tests) so a test needs no clock.
pub(super) async fn decide_sent_copy<Io: SentCopyIo + Send>(
    io: &mut Io,
    request: &crate::provider::SentCopyRequest,
    verify_delay: std::time::Duration,
) -> Result<SentCopyStep, ProviderError> {
    // 1. User override wins with NO network I/O.
    if io.server_saves_sent_override() {
        return Ok(SentCopyStep::Done);
    }

    // 2. A prior per-device decision is authoritative.
    match io.detection().await? {
        Some(true) => return Ok(SentCopyStep::Done), // server saves its own; nothing to do.
        Some(false) => {
            // We save it; run 4a's APPEND is idempotent (searches first).
            io.append_copy().await?;
            return Ok(SentCopyStep::Done);
        }
        None => {} // first send, unconfirmed: fall through to detection.
    }

    // 3. Detection. The phase tells us whether this is the first look or the
    // post-APPEND verify.
    match request.phase {
        SentCopyPhase::Pending => {
            if !request.append_attempted {
                // (a) First look, before any APPEND of ours. A copy present now
                // is the SERVER's own copy -> record "server saves sent".
                if io.count_in_sent().await? >= 1 {
                    io.record_detection(true).await?;
                    return Ok(SentCopyStep::Done);
                }
                // (b) Absent -> APPEND ours, then verify after the delay.
                io.append_copy().await?;
                return Ok(SentCopyStep::Recheck { after: verify_delay });
            }
            // append_attempted is true but we are still in Pending: a retry
            // after our APPEND did not get recorded (lost reply). Evidence rule:
            // a copy present now may be OUR appended copy, so it is NOT evidence
            // the server saves its own. Ensure the copy exists (idempotent
            // APPEND is a no-op if present) and move to verify.
            io.append_copy().await?;
            Ok(SentCopyStep::Recheck { after: verify_delay })
        }
        SentCopyPhase::Verify => {
            // (c) After our APPEND. Count the copies now present.
            let copies = io.count_in_sent().await?;
            if copies >= 2 {
                // The server saved one too; ours is the duplicate. Record
                // "server saves sent" and leave the duplicate ALONE (never
                // delete automatically — the engine/logs note it).
                log::warn!(
                    "imap sent-copy: {} copies of this Message-ID in Sent after our APPEND \
                     (outbox {}); the server saves its own copy, leaving our duplicate in place \
                     and recording 'server saves sent'",
                    copies,
                    request.outbox_id
                );
                io.record_detection(true).await?;
                Ok(SentCopyStep::Done)
            } else if copies == 1 {
                // Exactly our copy; the server does not save its own.
                io.record_detection(false).await?;
                Ok(SentCopyStep::Done)
            } else {
                // Zero copies: our APPEND's copy is not visible yet. Do NOT
                // append again (that is how a duplicate is born). Treat as
                // TRANSIENT so the engine retries the verify later, up to the
                // attempts cap, WITHOUT recording a detection result.
                Err(ProviderError::TransientTransport(
                    "the appended Sent copy is not yet visible to SEARCH; will re-verify".into(),
                ))
            }
        }
    }
}

/// The production [`SentCopyIo`] for one account: a leased session plus the
/// account's `ImapStateStore`, resolving the Sent mailbox from the sync plan
/// exactly as run 4a does. `count_in_sent` EXAMINEs then
/// [`find_by_message_id`]; `append_copy` runs run 4a's idempotent
/// [`save_sent_copy`]. The session is leased once and reused across the calls
/// one decision makes.
pub(super) struct SessionSentCopyIo<'a> {
    pub session: &'a mut dyn ImapWriteSession,
    pub store: &'a super::ImapStateStore,
    pub mailbox: String,
    pub raw: &'a [u8],
    pub message_id: String,
    pub server_saves_sent: bool,
    pub config: SaveSentConfig,
    pub now: fn() -> i64,
}

#[async_trait]
impl SentCopyIo for SessionSentCopyIo<'_> {
    fn server_saves_sent_override(&self) -> bool {
        self.server_saves_sent
    }

    async fn detection(&mut self) -> Result<Option<bool>, ProviderError> {
        self.store
            .sent_copy_detection()
            .map_err(|e| ProviderError::Other(e.to_string()))
    }

    async fn record_detection(&mut self, server_saves_sent: bool) -> Result<(), ProviderError> {
        self.store
            .set_sent_copy_detection(server_saves_sent, (self.now)())
            .map_err(|e| ProviderError::Other(e.to_string()))
    }

    async fn count_in_sent(&mut self) -> Result<usize, ProviderError> {
        // EXAMINE (read-only; never sets \Seen on seeded mail) for UIDVALIDITY,
        // then the verified Message-ID locate.
        let status = self.session.examine(&self.mailbox).await?;
        let uidvalidity = status.uid_validity.ok_or_else(|| {
            ProviderError::InvalidOperation(format!(
                "Sent mailbox {:?} reported no UIDVALIDITY on EXAMINE",
                self.mailbox
            ))
        })?;
        let located = find_by_message_id(self.session, &self.message_id, uidvalidity).await?;
        Ok(located.uids.len())
    }

    async fn append_copy(&mut self) -> Result<(), ProviderError> {
        // Run 4a's idempotent primitive: it searches before appending, so a
        // repeat call never produces a second copy.
        save_sent_copy(
            self.session,
            &self.mailbox,
            self.raw,
            &self.message_id,
            self.config,
        )
        .await
        .map(|_| ())
        .map_err(WriteError::into_provider_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- modified UTF-7 encoding -------------------------------------

    #[test]
    fn ascii_names_encode_verbatim_except_ampersand() {
        assert_eq!(encode_modified_utf7("INBOX"), "INBOX");
        assert_eq!(encode_modified_utf7("Clients/Acme"), "Clients/Acme");
        assert_eq!(encode_modified_utf7("A & B"), "A &- B");
    }

    #[test]
    fn non_ascii_names_encode_to_modified_base64() {
        // RFC 3501's own example: "~peter/mail/台北/日本語".
        assert_eq!(
            encode_modified_utf7("~peter/mail/\u{53f0}\u{5317}/\u{65e5}\u{672c}\u{8a9e}"),
            "~peter/mail/&U,BTFw-/&ZeVnLIqe-"
        );
        // A single BMP char.
        assert_eq!(encode_modified_utf7("\u{00e9}"), "&AOk-"); // é
        // An astral char (emoji) uses a surrogate pair.
        assert_eq!(encode_modified_utf7("\u{1F600}"), "&2D3eAA-");
    }

    #[test]
    fn mailbox_wire_names_round_trip_without_double_encoding() {
        for name in ["R&D", "Café", "日本語", "😀", "a\\b\"c", "&AOk-"] {
            let encoded = encode_modified_utf7(name);
            assert_eq!(decode_modified_utf7(&encoded).unwrap(), name);
            assert_eq!(
                build_wire_mailbox_arg(&encoded).unwrap(),
                build_mailbox_arg(name).unwrap()
            );
        }
        for invalid in ["&", "&AA-", "&2AA-", "&!!!-"] {
            assert!(decode_modified_utf7(invalid).is_err(), "{invalid}");
        }
        for hostile in ["a\r\nA1 DELETE INBOX", "a\0b"] {
            assert!(build_wire_mailbox_arg(hostile).is_err());
        }
    }

    // ---- mailbox arg construction ------------------------------------

    #[test]
    fn mailbox_arg_quotes_and_escapes_like_async_imap() {
        assert_eq!(build_mailbox_arg("INBOX").unwrap(), "\"INBOX\"");
        // A quote and a backslash in the name are escaped inside the quotes.
        assert_eq!(
            build_mailbox_arg("a\"b").unwrap(),
            "\"a\\\"b\""
        );
        assert_eq!(build_mailbox_arg("a\\b").unwrap(), "\"a\\\\b\"");
        // `]` and `)` are ordinary characters in a quoted string — allowed.
        assert_eq!(build_mailbox_arg("a]b)c").unwrap(), "\"a]b)c\"");
    }

    #[test]
    fn hostile_mailbox_names_are_rejected_before_any_command_is_built() {
        for hostile in [
            "a\r\nA1 DELETE INBOX", // CRLF injection
            "a\rb",
            "a\nb",
            "a\0b", // NUL
        ] {
            let err = build_mailbox_arg(hostile).unwrap_err();
            assert!(
                matches!(err, ProviderError::InvalidOperation(_)),
                "{hostile:?} must be refused, got {err:?}"
            );
        }
    }

    #[test]
    fn an_over_length_mailbox_name_is_rejected() {
        let long = "x".repeat(policy::MAX_MAILBOX_NAME_BYTES + 1);
        assert!(matches!(
            build_mailbox_arg(&long).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
        // Exactly at the limit is accepted.
        let exact = "x".repeat(policy::MAX_MAILBOX_NAME_BYTES);
        assert!(build_mailbox_arg(&exact).is_ok());
    }

    #[test]
    fn an_empty_mailbox_name_is_encodable_but_quoted() {
        // An empty name is a degenerate but non-injecting input; it quotes to
        // `""`. (Callers never pass it; the point is it does not panic or
        // inject.)
        assert_eq!(build_mailbox_arg("").unwrap(), "\"\"");
    }

    #[test]
    fn a_non_ascii_mailbox_name_builds_a_utf7_quoted_arg() {
        assert_eq!(
            build_mailbox_arg("Pièces").unwrap(),
            "\"Pi&AOg-ces\""
        );
    }

    // ---- flag validation ---------------------------------------------

    #[test]
    fn system_flags_are_accepted_case_insensitively_and_canonicalised() {
        for (input, canonical) in [
            ("\\seen", "\\Seen"),
            ("\\FLAGGED", "\\Flagged"),
            ("\\Deleted", "\\Deleted"),
            ("\\answered", "\\Answered"),
            ("\\Draft", "\\Draft"),
        ] {
            assert_eq!(validate_flag(input).unwrap(), canonical);
        }
    }

    #[test]
    fn atom_keywords_are_accepted_verbatim() {
        for kw in ["$Forwarded", "Forwarded", "$Junk", "MyLabel", "a1_b-c"] {
            assert_eq!(validate_flag(kw).unwrap(), kw);
        }
    }

    #[test]
    fn hostile_or_structural_flags_are_rejected() {
        for bad in [
            "",
            " ",
            "a b",       // space
            "a(b",       // paren
            "a)b",
            "a{b",
            "a}b",
            "a%b",       // %
            "a\"b",      // quote
            "a\\b",      // backslash (not a known system flag)
            "a]b",       // ]
            "a\r\nb",    // CRLF
            "a\0b",      // NUL
            "\\Recent",  // server-maintained
            "\\Unknown", // unknown system flag
            "*",         // the capability marker, never a flag
            "\\*",
            "a*b",
        ] {
            assert!(
                validate_flag(bad).is_err(),
                "flag {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_non_ascii_or_emoji_keyword_is_rejected_as_a_non_atom() {
        for bad in ["caf\u{00e9}", "\u{1F600}"] {
            assert!(validate_flag(bad).is_err(), "{bad:?} is not an atom");
        }
    }

    #[test]
    fn validate_flags_rejects_empty_and_over_count_lists() {
        assert!(validate_flags(&[]).is_err());
        let too_many: Vec<String> =
            (0..=policy::MAX_FLAGS_PER_COMMAND).map(|i| format!("kw{i}")).collect();
        assert!(validate_flags(&too_many).is_err());
        // Exactly at the limit, all valid atoms: accepted.
        let exact: Vec<String> =
            (0..policy::MAX_FLAGS_PER_COMMAND).map(|i| format!("kw{i}")).collect();
        assert_eq!(validate_flags(&exact).unwrap().len(), policy::MAX_FLAGS_PER_COMMAND);
    }

    // ---- UID set building --------------------------------------------

    #[test]
    fn uid_sets_compress_runs_and_dedup() {
        assert_eq!(build_uid_set(&[]), None);
        assert_eq!(build_uid_set(&[3]).unwrap(), "3");
        assert_eq!(build_uid_set(&[3, 4, 5]).unwrap(), "3:5");
        assert_eq!(build_uid_set(&[5, 3, 4]).unwrap(), "3:5"); // unordered
        assert_eq!(build_uid_set(&[3, 3, 4]).unwrap(), "3:4"); // dup
        assert_eq!(build_uid_set(&[3, 5, 6, 7, 9]).unwrap(), "3,5:7,9");
        assert_eq!(build_uid_set(&[1, 3, 5]).unwrap(), "1,3,5");
    }

    #[test]
    fn uid_set_batches_split_above_the_per_command_limit() {
        let n = policy::MAX_UIDS_PER_COMMAND;
        // Exactly the limit: one batch.
        let exact: Vec<u32> = (1..=n as u32).collect();
        let batches = build_uid_set_batches(&exact);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0], format!("1:{n}"));
        // One over: two batches, the second carrying the single overflow UID.
        let over: Vec<u32> = (1..=(n as u32 + 1)).collect();
        let batches = build_uid_set_batches(&over);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], format!("1:{n}"));
        assert_eq!(batches[1], format!("{}", n + 1));
        // Empty input: no batches.
        assert!(build_uid_set_batches(&[]).is_empty());
    }

    // ---- WriteCaps ----------------------------------------------------

    fn status(flags: &[&str], keywords: bool) -> MailboxStatus {
        MailboxStatus {
            exists: 0,
            uid_next: Some(1),
            uid_validity: Some(1),
            permanent_keywords: keywords,
            permanent_flags: flags.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn write_caps_derive_move_and_uidplus_case_insensitively() {
        let caps = WriteCaps::derive(
            &["move".into(), "UIDPLUS".into(), "UNSELECT".into()],
            &status(&["\\Seen"], false),
        );
        assert!(caps.has_move);
        assert!(caps.has_uidplus);
        let none = WriteCaps::derive(&["IMAP4REV1".into()], &status(&[], false));
        assert!(!none.has_move);
        assert!(!none.has_uidplus);
    }

    #[test]
    fn can_store_flag_follows_permanentflags_and_the_wildcard_rule() {
        // The primary account's PERMANENTFLAGS: no \*, no \Answered.
        let bridge = WriteCaps::derive(
            &["MOVE".into(), "UIDPLUS".into()],
            &status(&["$Forwarded", "Forwarded", "\\Deleted", "\\Flagged", "\\Seen"], false),
        );
        // Listed system flags and keywords: storable.
        assert!(bridge.can_store_flag("\\Seen"));
        assert!(bridge.can_store_flag("\\flagged")); // case-insensitive system
        assert!(bridge.can_store_flag("$Forwarded"));
        // A system flag NOT in PERMANENTFLAGS: not storable (no \*).
        assert!(!bridge.can_store_flag("\\Answered"));
        // An arbitrary NEW keyword: not storable without \*.
        assert!(!bridge.can_store_flag("MyLabel"));
        // \Recent and \* are never storable.
        assert!(!bridge.can_store_flag("\\Recent"));
        assert!(!bridge.can_store_flag("\\*"));

        // A \*-capable server (the Dovecot container) admits new keywords.
        let wildcard = WriteCaps::derive(
            &["MOVE".into(), "UIDPLUS".into()],
            &status(&["\\Seen", "\\Flagged", "\\Deleted", "\\Answered", "\\Draft"], true),
        );
        assert!(wildcard.can_store_flag("MyLabel"));
        assert!(wildcard.can_store_flag("\\Answered"));
        // But \Recent is still never storable, even with \*.
        assert!(!wildcard.can_store_flag("\\Recent"));
    }

    #[test]
    fn relocation_strategy_table() {
        let both = WriteCaps::derive(&["MOVE".into(), "UIDPLUS".into()], &status(&[], false));
        assert_eq!(both.relocation_strategy(), RelocationStrategy::Move);
        assert!(both.can_move());
        assert!(both.can_expunge_uids());

        let uidplus_only = WriteCaps::derive(&["UIDPLUS".into()], &status(&[], false));
        assert_eq!(
            uidplus_only.relocation_strategy(),
            RelocationStrategy::CopyDeleteExpunge
        );
        assert!(!uidplus_only.can_move());
        assert!(uidplus_only.can_expunge_uids());

        let move_only = WriteCaps::derive(&["MOVE".into()], &status(&[], false));
        // MOVE is preferred when present, even without UIDPLUS.
        assert_eq!(move_only.relocation_strategy(), RelocationStrategy::Move);
        assert!(!move_only.can_expunge_uids());

        let neither = WriteCaps::derive(&["IMAP4REV1".into()], &status(&[], false));
        match neither.relocation_strategy() {
            RelocationStrategy::Unsupported(reason) => {
                assert!(reason.contains("EXPUNGE"), "reason names the hazard: {reason}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
        assert!(!neither.can_move());
        assert!(!neither.can_expunge_uids());
    }

    // ---- typed COPYUID parsing ---------------------------------------

    #[test]
    fn copyuid_is_parsed_from_the_typed_code_with_ranges_expanded() {
        use async_imap::imap_proto::rfc4315::UidSetMember;
        use async_imap::imap_proto::ResponseCode;
        let code = ResponseCode::CopyUid(
            42,
            vec![UidSetMember::Uid(1), UidSetMember::UidRange(3..=5)],
            vec![UidSetMember::UidRange(10..=13)],
        );
        let copyuid = copyuid_from_code(&code).unwrap();
        assert_eq!(copyuid.uidvalidity, 42);
        assert_eq!(copyuid.source, vec![1, 3, 4, 5]);
        assert_eq!(copyuid.dest, vec![10, 11, 12, 13]);
        assert_eq!(
            copyuid.pairs().collect::<Vec<_>>(),
            vec![(1, 10), (3, 11), (4, 12), (5, 13)]
        );
        // A non-COPYUID code yields None.
        assert!(copyuid_from_code(&ResponseCode::ReadOnly).is_none());
    }

    #[test]
    fn write_error_trycreate_collapses_to_a_permanent_invalid_operation() {
        let err = WriteError::TryCreate("Mailbox doesn't exist: Labels/New".into())
            .into_provider_error();
        match err {
            ProviderError::InvalidOperation(msg) => {
                assert!(msg.contains("TRYCREATE"), "{msg}");
                assert!(!msg.to_ascii_lowercase().contains("transient"));
            }
            other => panic!("expected InvalidOperation, got {other:?}"),
        }
    }

    // ---- persisted writable PERMANENTFLAGS must not touch sync counters ----

    fn memory_store() -> crate::provider::imap::ImapStateStore {
        let database = std::sync::Arc::new(crate::db::Database::open_memory());
        crate::provider::imap::ImapStateStore::new(database, "me@example.com")
    }

    fn catalogued(uidvalidity: i64, uidnext: i64) -> crate::provider::imap::ImapMailbox {
        crate::provider::imap::ImapMailbox {
            name: "INBOX".into(),
            delimiter: Some("/".into()),
            special_use: None,
            uidvalidity,
            uidnext,
            highestmodseq: Some(77),
            permanent_flags_json: None,
            permanent_keywords: None,
        }
    }

    #[test]
    fn persisting_writable_flags_keeps_every_sync_counter_the_store_holds() {
        let store = memory_store();
        store.upsert_mailbox(&catalogued(10, 500)).unwrap();
        // The writable SELECT reports a DIFFERENT (newer) UIDNEXT than the
        // sync gate has stored; persisting flags must not move the gate.
        let mut selected = status(&["\\Seen", "\\Flagged"], false);
        selected.uid_next = Some(900);
        selected.uid_validity = Some(10);
        persist_writable_permanent_flags(&store, "INBOX", &selected);

        let row = store
            .mailboxes()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "INBOX")
            .unwrap();
        assert_eq!(row.uidnext, 500, "sync's UIDNEXT must be untouched");
        assert_eq!(row.uidvalidity, 10);
        assert_eq!(row.highestmodseq, Some(77));
        assert_eq!(
            row.permanent_flags_json.as_deref(),
            Some(r#"["\\Seen","\\Flagged"]"#)
        );
        assert_eq!(row.permanent_keywords, Some(false));
    }

    #[test]
    fn persisting_writable_flags_never_creates_or_seeds_an_uncatalogued_mailbox() {
        let store = memory_store();
        let mut selected = status(&["\\Seen"], false);
        selected.uid_next = Some(900);
        selected.uid_validity = Some(10);
        persist_writable_permanent_flags(&store, "Brand New", &selected);
        // A row seeded here would carry a UIDNEXT the sync gate would trust,
        // hiding every older message in that mailbox from the first sweep.
        assert!(
            store.mailboxes().unwrap().is_empty(),
            "an uncatalogued mailbox must not be created with sync counters"
        );
    }

    // ---- Phase 4 run 4a: Sent-copy pure helpers -----------------------

    #[test]
    fn prepare_append_keeps_the_wire_mailbox_name_and_builds_the_flag_list() {
        // The mailbox is the catalog's WIRE spelling and must come back
        // unchanged (never encoded a second time); the flag list is
        // parenthesised.
        let (mailbox, flags) =
            prepare_append("Gesendet/Entw&APw-rfe", &["\\Seen".into()], b"hi").unwrap();
        assert_eq!(mailbox, "Gesendet/Entw&APw-rfe");
        assert_eq!(flags, "(\\Seen)");
        let (mailbox, _) = prepare_append("A &- B", &["\\Seen".into()], b"hi").unwrap();
        assert_eq!(mailbox, "A &- B");
        // Malformed modified UTF-7 is refused rather than naming another mailbox.
        assert!(prepare_append("Bad &AOk", &["\\Seen".into()], b"hi").is_err());
        // ASCII name passes through unchanged.
        let (mailbox, _) = prepare_append("Sent", &["\\Seen".into()], b"hi").unwrap();
        assert_eq!(mailbox, "Sent");
    }

    #[test]
    fn prepare_append_rejects_empty_nul_oversize_and_bad_flags_and_names() {
        // Empty content.
        assert!(matches!(
            prepare_append("Sent", &["\\Seen".into()], b"").unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
        // NUL in content.
        assert!(matches!(
            prepare_append("Sent", &["\\Seen".into()], b"a\0b").unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
        // Over-limit content is a PERMANENT rejection.
        let oversize = vec![b'x'; policy::APPEND_MAX_BYTES + 1];
        assert!(matches!(
            prepare_append("Sent", &["\\Seen".into()], &oversize).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
        // Exactly at the limit is accepted (below/exact/above lives in policy).
        let exact = vec![b'x'; policy::APPEND_MAX_BYTES];
        assert!(prepare_append("Sent", &["\\Seen".into()], &exact).is_ok());
        // An empty flag list is refused (a STORE/APPEND needs a flag or the
        // caller means "no flags" — this primitive always sets \Seen).
        assert!(prepare_append("Sent", &[], b"hi").is_err());
        // A hostile mailbox name (CRLF injection) is refused before anything.
        assert!(prepare_append("Sent\r\nA1 DELETE INBOX", &["\\Seen".into()], b"hi").is_err());
    }

    #[test]
    fn validate_search_message_id_rejects_unsafe_values_and_empty_normalization() {
        for bad in ["<a\"b@x>", "<a\\b@x>", "<a b@x>", "<a\rb@x>", "<a\nb@x>", "<a\0b@x>"] {
            assert!(validate_search_message_id(bad).is_err(), "{bad} must be refused");
        }
        // Normalizes to empty (only brackets) -> refused.
        assert!(validate_search_message_id("<>").is_err());
        // A clean id is accepted and returns its normalized token.
        assert_eq!(validate_search_message_id("<ABC@Example.com>").unwrap(), "abc@example.com");
    }

    #[test]
    fn header_message_id_matches_compares_the_normalized_token_exactly() {
        let target = super::super::identity::normalize_message_id("<abc@x>");
        // An exact match (case/bracket/folding insensitive).
        assert!(header_message_id_matches(b"Message-ID: <ABC@x>\r\n\r\n", &target));
        assert!(header_message_id_matches(b"Message-Id: <abc@x>", &target));
        // Near-misses must NOT match (substring hits the SEARCH would surface).
        assert!(!header_message_id_matches(b"Message-ID: <xabc@x>\r\n", &target));
        assert!(!header_message_id_matches(b"Message-ID: <abc@xy>\r\n", &target));
        // A different header, or no colon, never matches.
        assert!(!header_message_id_matches(b"Subject: <abc@x>\r\n", &target));
        assert!(!header_message_id_matches(b"garbage", &target));
    }

    #[test]
    fn code_word_is_reads_only_the_bracketed_leading_code_position() {
        // The async-imap NO text shape for a coded NO.
        assert!(code_word_is(
            r#"code: None, info: Some("[NONEXISTENT] Unknown mailbox: Sent")"#,
            "NONEXISTENT"
        ));
        // A code word appearing only in the DIAGNOSTIC TAIL must NOT match
        // (server-neutral: a mailbox named like a code cannot forge one).
        assert!(!code_word_is(
            r#"code: None, info: Some("Unknown mailbox: NONEXISTENT")"#,
            "NONEXISTENT"
        ));
        // No quoted info -> no match.
        assert!(!code_word_is("code: None, info: None", "NONEXISTENT"));
    }

    #[test]
    fn map_append_error_classifies_the_append_outcomes() {
        use async_imap::error::Error as E;
        // A pre-continuation refusal collapses to Error::Append -> PERMANENT.
        match map_append_error(&E::Append) {
            WriteError::Provider(ProviderError::PermanentClientRejection(_)) => {}
            other => panic!("Error::Append must be a permanent rejection, got {other:?}"),
        }
        // A dropped connection stays TRANSIENT (idempotent-retry depends on it).
        match map_append_error(&E::ConnectionLost) {
            WriteError::Provider(ProviderError::TransientTransport(_)) => {}
            other => panic!("a drop must stay transient, got {other:?}"),
        }
        // A coded TRYCREATE NO (the shape async-imap uses when it DOES surface a
        // code) lifts to the typed TryCreate.
        let trycreate = E::No(r#"code: Some(TryCreate), info: Some("Mailbox doesn't exist: Sent")"#.into());
        assert!(matches!(map_append_error(&trycreate), WriteError::TryCreate(_)));
        // A coded NONEXISTENT NO likewise.
        let nonexistent = E::No(r#"code: None, info: Some("[NONEXISTENT] no such mailbox")"#.into());
        assert!(matches!(map_append_error(&nonexistent), WriteError::TryCreate(_)));
        // An OVERQUOTA NO that DID carry through as text stays permanent.
        let overquota = E::No(r#"code: None, info: Some("[OVERQUOTA] mailbox full")"#.into());
        match map_append_error(&overquota) {
            WriteError::Provider(ProviderError::PermanentClientRejection(_)) => {}
            other => panic!("OVERQUOTA stays permanent, got {other:?}"),
        }
    }

    #[test]
    fn save_sent_config_default_uses_the_policy_recheck_count() {
        let config = SaveSentConfig::default();
        assert_eq!(config.relocate_rechecks, policy::SENT_COPY_RELOCATE_RECHECKS);
        assert!(!config.recheck_delay.is_zero(), "the production default settles between re-checks");
    }

    // ---- Phase 4 run 4b: the Sent-copy DECISION state machine --------------
    //
    // `decide_sent_copy` is tested over an in-memory fake IO that records every
    // call, so every branch of the state machine — including the subtle
    // evidence rule — is asserted without a server. The matching wire-level
    // integration test (`write/wire_tests.rs`) proves the SAME decisions
    // against a real session + scripted server.

    use crate::provider::{SentCopyPhase, SentCopyRequest, SentCopyStep};
    use std::time::Duration;

    /// A scriptable fake [`SentCopyIo`] recording calls and returning canned
    /// values. `count_returns` is a queue popped per `count_in_sent` call.
    struct FakeIo {
        override_saves: bool,
        detection: Option<bool>,
        count_returns: std::cell::RefCell<Vec<usize>>,
        appended: std::cell::Cell<usize>,
        recorded: std::cell::Cell<Option<bool>>,
    }

    impl FakeIo {
        fn new() -> Self {
            Self {
                override_saves: false,
                detection: None,
                count_returns: std::cell::RefCell::new(Vec::new()),
                appended: std::cell::Cell::new(0),
                recorded: std::cell::Cell::new(None),
            }
        }
        fn with_override(mut self) -> Self {
            self.override_saves = true;
            self
        }
        fn with_detection(mut self, value: bool) -> Self {
            self.detection = Some(value);
            self
        }
        fn with_counts(self, counts: &[usize]) -> Self {
            *self.count_returns.borrow_mut() = counts.to_vec();
            self
        }
    }

    #[async_trait]
    impl SentCopyIo for FakeIo {
        fn server_saves_sent_override(&self) -> bool {
            self.override_saves
        }
        async fn detection(&mut self) -> Result<Option<bool>, ProviderError> {
            Ok(self.detection)
        }
        async fn record_detection(&mut self, v: bool) -> Result<(), ProviderError> {
            self.recorded.set(Some(v));
            Ok(())
        }
        async fn count_in_sent(&mut self) -> Result<usize, ProviderError> {
            Ok(self.count_returns.borrow_mut().remove(0))
        }
        async fn append_copy(&mut self) -> Result<(), ProviderError> {
            self.appended.set(self.appended.get() + 1);
            Ok(())
        }
    }

    fn request(phase: SentCopyPhase, append_attempted: bool) -> SentCopyRequest {
        SentCopyRequest {
            raw: b"Message-ID: <m@x>\r\n\r\nbody".to_vec(),
            outbox_id: "o1".into(),
            append_attempted,
            phase,
        }
    }

    async fn decide(io: &mut FakeIo, req: &SentCopyRequest) -> Result<SentCopyStep, ProviderError> {
        decide_sent_copy(io, req, Duration::from_secs(10)).await
    }

    #[tokio::test]
    async fn override_true_is_done_with_no_network_or_detection() {
        let mut io = FakeIo::new().with_override();
        let step = decide(&mut io, &request(SentCopyPhase::Pending, false)).await.unwrap();
        assert_eq!(step, SentCopyStep::Done);
        assert_eq!(io.appended.get(), 0, "override APPENDs nothing");
        assert!(io.recorded.get().is_none(), "override records no detection");
        assert!(io.count_returns.borrow().is_empty(), "override does not count");
    }

    #[tokio::test]
    async fn detection_some_true_is_done_without_appending() {
        let mut io = FakeIo::new().with_detection(true);
        let step = decide(&mut io, &request(SentCopyPhase::Pending, false)).await.unwrap();
        assert_eq!(step, SentCopyStep::Done);
        assert_eq!(io.appended.get(), 0);
    }

    #[tokio::test]
    async fn detection_some_false_appends_our_copy_and_is_done() {
        let mut io = FakeIo::new().with_detection(false);
        let step = decide(&mut io, &request(SentCopyPhase::Pending, false)).await.unwrap();
        assert_eq!(step, SentCopyStep::Done);
        assert_eq!(io.appended.get(), 1, "we save the copy ourselves");
        assert!(io.recorded.get().is_none());
    }

    #[tokio::test]
    async fn first_send_copy_present_before_append_records_server_saves_sent() {
        // None + a copy present BEFORE any APPEND of ours == the server saved
        // its own copy. Record true, append nothing.
        let mut io = FakeIo::new().with_counts(&[1]);
        let step = decide(&mut io, &request(SentCopyPhase::Pending, false)).await.unwrap();
        assert_eq!(step, SentCopyStep::Done);
        assert_eq!(io.appended.get(), 0, "a server-saved copy means we never APPEND");
        assert_eq!(io.recorded.get(), Some(true));
    }

    #[tokio::test]
    async fn first_send_copy_absent_appends_then_asks_to_verify() {
        let mut io = FakeIo::new().with_counts(&[0]);
        let step = decide(&mut io, &request(SentCopyPhase::Pending, false)).await.unwrap();
        assert_eq!(step, SentCopyStep::Recheck { after: Duration::from_secs(10) });
        assert_eq!(io.appended.get(), 1);
        assert!(io.recorded.get().is_none(), "no detection recorded until verify");
    }

    #[tokio::test]
    async fn verify_with_two_copies_records_server_saves_sent_and_leaves_the_duplicate() {
        let mut io = FakeIo::new().with_counts(&[2]);
        let step = decide(&mut io, &request(SentCopyPhase::Verify, true)).await.unwrap();
        assert_eq!(step, SentCopyStep::Done);
        assert_eq!(io.recorded.get(), Some(true), "server saved one too");
        assert_eq!(io.appended.get(), 0, "verify never appends; never deletes the duplicate");
    }

    #[tokio::test]
    async fn verify_with_one_copy_records_server_does_not_save() {
        let mut io = FakeIo::new().with_counts(&[1]);
        let step = decide(&mut io, &request(SentCopyPhase::Verify, true)).await.unwrap();
        assert_eq!(step, SentCopyStep::Done);
        assert_eq!(io.recorded.get(), Some(false));
    }

    #[tokio::test]
    async fn verify_with_zero_copies_is_transient_and_never_appends_again() {
        // Our APPEND's copy is not visible yet. Do NOT append again (that is how
        // a duplicate is born); retry the verify later, record nothing.
        let mut io = FakeIo::new().with_counts(&[0]);
        let err = decide(&mut io, &request(SentCopyPhase::Verify, true)).await.unwrap_err();
        assert!(matches!(err, ProviderError::TransientTransport(_)));
        assert_eq!(io.appended.get(), 0, "a zero-copy verify must NOT re-append");
        assert!(io.recorded.get().is_none(), "no detection recorded on a retry-later verify");
    }

    #[tokio::test]
    async fn lost_reply_retry_with_append_attempted_is_not_server_evidence() {
        // Pending phase but append_attempted=true (a lost reply): a copy present
        // now may be OUR appended copy, so it is NOT evidence the server saves
        // its own. We ensure the copy exists (idempotent) and move to verify —
        // and crucially do NOT record detection true here.
        let mut io = FakeIo::new();
        let step = decide(&mut io, &request(SentCopyPhase::Pending, true)).await.unwrap();
        assert_eq!(step, SentCopyStep::Recheck { after: Duration::from_secs(10) });
        assert!(
            io.recorded.get().is_none(),
            "a copy found after our APPEND is never recorded as server-saves evidence"
        );
        assert_eq!(io.appended.get(), 1, "the idempotent APPEND is a no-op if present");
        assert!(io.count_returns.borrow().is_empty(), "evidence rule: we did not count before deciding");
    }
}

#[cfg(test)]
mod wire_tests;
