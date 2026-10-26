//! The IMAP provider: connection layer and persistent sync state.
//!
//! This module roots everything IMAP. Two concerns live under it:
//!
//! * **Persistent sync state** (`crate::db::imap`, re-exported here).
//!   `docs/imap-design.md`
//!   ("Data model") gives the IMAP provider a narrow handle onto two tables
//!   that the opaque [`SyncCursor`](super::SyncCursor) deliberately does not
//!   carry: the per-account mailbox catalog (`imap_mailboxes`, each mailbox's
//!   UID counters) and the UID-to-message-id location map (`imap_locations`).
//!   The cursor "stays small, holding only a sync generation number";
//!   everything heavier lives in [`ImapStateStore`]. No Gmail code path reads
//!   or writes these tables, so Gmail's own sync state is untouched. The
//!   tables themselves are created by schema migration v54; see
//!   `crate::schema`.
//!
//! * **The connection layer** (Phase 2 Slice 1, submodules below). The first
//!   shipped IMAP wire code: a TLS-secured, fingerprint-pinned session behind
//!   the internal [`ImapSession`] trait, the connection-cap policy, and the
//!   RFC 5530 -> [`ProviderError`](super::ProviderError) mapping. It is
//!   infrastructure the later read slices build on. It implements NO sync,
//!   fetch, mailbox discovery, account setup, or SMTP yet, and it is NOT wired
//!   into live provider dispatch: no account can construct an IMAP provider
//!   until Slice 2's account setup lands. Gmail is unchanged.
//!
//! The connection code lifts the patterns the Slice 0 spike
//! (`examples/imap_spike.rs`) proved — STARTTLS over a stream we own, the
//! pinning `rustls` `ServerCertVerifier`, `run_command` for response codes —
//! into the real crate; the spike stays as a throwaway example.

// The IMAP provider that drives this seam lands across phase 2; until each
// piece has its live caller (Slice 2 account setup constructs sessions; the
// read slices call fetch/search) the connection types and the state store
// have no non-test caller. The allow is scoped to this module tree and
// removed with those first callers, matching how slice 1 scoped the unused
// `ProviderCapabilities` fields.
#![allow(dead_code)]

mod connection;
mod delta;
mod discovery;
mod error;
mod fetch;
mod identity;
mod labels;
mod mailboxes;
pub(crate) mod plan;
pub(crate) mod policy;
mod provider;
mod rfc822;
mod search;
mod send;
mod sending_identity;
mod session;
mod settings;
mod setup;
mod smtp;
mod threading;
mod tls;
mod write;

// These are the connection layer's public surface for the later read slices
// (Slice 2 account setup constructs a manager + sessions; the read slices call
// the session methods and the error mapper). Until those callers land the
// re-exports are unused, so the allow is scoped here and removed with the
// first consumer — matching how the module scopes `dead_code`.
#[allow(unused_imports)]
pub use connection::{ConnectionConfig, ImapConnectionManager, TlsMode};
#[allow(unused_imports)]
pub use error::map_imap_error;
#[allow(unused_imports)]
pub use session::{ImapSession, MailboxStatus};
// Phase 3 run 1 write surface: the typed, injection-safe, capability-aware
// write primitives. Their first caller is Phase 3 run 2 (flags/moves/labels),
// so the re-exports are forward-looking exactly like the connection-layer ones
// above; the allow is removed with that first consumer.
#[allow(unused_imports)]
pub use write::{
    build_mailbox_arg, build_uid_set, build_uid_set_batches, find_by_message_id, relocate_uids,
    save_sent_copy, select_for_write, validate_flag, validate_flags, verified_store, CopyOutcome,
    CopyUid, ImapWriteSession, Located, RelocateOutcome, RelocationStrategy, SaveSentConfig,
    SentCopy, StoreOp, WriteCaps, WriteContext, WriteError, WriteResult,
};
#[allow(unused_imports)]
pub use tls::{parse_sha256_fingerprint, PinnedCertVerifier, Sha256Fingerprint};

// Slice 2 account-setup surface, consumed by the tauri commands in
// `crate::lib`: autodiscovery, the non-secret settings store, and the
// test-and-save / cert-probe orchestration.
pub use discovery::{discover, DiscoveryResult};
// `ImapSettingsStore` and `FingerprintDecision` are the live-provider seam the
// read/mutation slices construct; they have no caller in Slice 2 yet, so the
// allow is scoped here and removed with the first consumer — the same way the
// connection-layer re-exports above scope it.
pub use settings::{Identity, ImapAccountSettings, LabelStorage, SecurityMode};
#[allow(unused_imports)]
pub use settings::{FingerprintDecision, ImapSettingsStore};
pub use settings::host_port_key;
pub use connection::CertificateProbe;
// Slice 3 mailbox-discovery surface, consumed by the tauri commands: the pure
// mapping types the frontend renders and the discovery/create orchestration.
// The nested proposal/mailbox/role/source types reach the frontend through
// `MailboxMapping`'s fields rather than by name in Rust, so the allow is
// scoped here exactly as the connection-layer re-exports above scope it.
#[allow(unused_imports)]
pub use mailboxes::{
    create_mailbox_tolerant, discover_and_persist, DiscoveredMailbox, MailboxMapping, MailboxRole,
    MappingSource, RoleProposal,
};
pub use setup::{
    plain_language, probe_imap_certificate, test_imap, TestReport,
};
// The SMTP dry-run surface moved to `smtp` in Phase 4 run 1; re-export it from
// the same module path so the tauri setup commands that call
// `imap::test_smtp` / `imap::probe_smtp_certificate` are unchanged.
#[allow(unused_imports)]
pub use smtp::{probe_smtp_certificate, test_smtp};
// Phase 4 run 1 submission client. No caller until a later run wires it into
// MailSend/the UI, so the re-exports are forward-looking exactly like the
// connection-layer ones above; the allow is removed with that first consumer.
#[allow(unused_imports)]
pub use smtp::{
    frame_message, validate_envelope_address, Phase, SmtpCredentials, SmtpEndpoint, SmtpFailure,
    SmtpOutcome, SmtpReply, SmtpSession, SmtpTimeouts, SmtpUnknown,
};
pub use setup::connect_with_settings;
// Named only as `connect_with_settings`'s return type, so the re-export itself
// reads as unused; scoped the same way as the other forward-looking exports.
#[allow(unused_imports)]
pub use connection::ConnectedSession;
// Port defaults and the certificate-info type are part of the setup surface
// the frontend form reaches through commands added as the UI fills in; no
// caller yet, so the allow is scoped and removed with it.
#[allow(unused_imports)]
pub use setup::default_imap_port;
#[allow(unused_imports)]
pub use smtp::default_smtp_port;
#[allow(unused_imports)]
pub use tls::CertificateInfo;

// Slice 4 message-identity + body-cache surface. The read slices (Slice 5)
// construct these; until then they have no non-test caller, so the re-exports
// are forward-looking exactly like the connection-layer ones above.
#[allow(unused_imports)]
pub use fetch::{
    ensure_body, fetch_identity, parse_email_id, parse_identity_headers, BodyCache, BodyFetcher,
    CachedBody, IdentityRow, SessionBodyFetcher, BODY_ITEMS, IDENTITY_ITEMS,
};
#[allow(unused_imports)]
pub use fetch::{fetch_flags_only, resolve_and_cache_body, ResolvedBody, FLAGS_ITEMS};
#[allow(unused_imports)]
pub use fetch::{fetch_identity_index, parse_threading_headers, INDEX_IDENTITY_ITEMS};
#[allow(unused_imports)]
pub use identity::{derive_message_id, IdentityInputs};
#[allow(unused_imports)]
pub use rfc822::{attachment_bytes_from_raw, to_raw_message};
#[allow(unused_imports)]
pub use rfc822::{threading_headers_from_raw, ThreadingHeaders};

// Slice 5a live-sync surface: the provider the engine drives, plus the pure
// logic modules it is built from.
pub use provider::{label_model_for, ImapProvider, ImapProviderConfig};

// Phase 4 run 2 sending-identity surface: the one source of truth for an
// account's sending addresses and the two pure decisions built on it. The
// MailSend seam (`identities`, `message_id_domain`) and the correspondence
// reply/compose path consume these.
pub use sending_identity::{
    account_identities, choose_reply_identity, identity_normalized,
    message_id_domain_for_address, normalize_identity_address, ReplyHeaders, SendingIdentity,
    validate_identity_edit,
};
// Phase 4 run 3 SMTP prepare-retry gate: the engine's outbox tick reads the
// predicate; the interval constant is read by the engine's gate tests. The
// constant has no non-test reader in the lib build, so its re-export carries
// the same scoped allow the other forward-looking re-exports use.
pub use policy::smtp_prepare_retry_due;
#[allow(unused_imports)]
pub use policy::SMTP_PREPARE_RETRY_INTERVAL_MS;
// Phase 4 run 4b: the engine's outbox tick seeds and drives the durable
// Sent-copy job from these. SENT_COPY_SETTLE sets the job's first `next_at`,
// `sent_copy_backoff` schedules a transient retry (never earlier than the
// settle window), and the caps bound the runner.
pub use policy::{
    sent_copy_backoff, SENT_COPY_MAX_ATTEMPTS, SENT_COPY_MAX_JOBS_PER_TICK, SENT_COPY_SETTLE,
    SENT_COPY_VERIFY_DELAY,
};

pub use crate::db::imap::{ImapLocation, ImapMailbox, ImapStateStore, SyncRoundWrite};

#[cfg(test)]
mod tests {
    use crate::provider::SyncCursor;

    // A generation cursor round-trips as a plain decimal string and is
    // distinguishable from a Gmail historyId-style cursor.
    #[test]
    fn sync_generation_round_trips_through_the_opaque_cursor() {
        let cursor = SyncCursor::from_generation(7);
        assert_eq!(cursor.as_str(), "7");
        assert_eq!(cursor.generation(), Some(7));
        // A multi-token Gmail cursor (historyId + page token) is not a
        // generation.
        assert_eq!(SyncCursor::new("123456 pageTok").generation(), None);
    }
}
