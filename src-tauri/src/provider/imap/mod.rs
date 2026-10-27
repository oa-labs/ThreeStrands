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
//! * **The live provider** (submodules below): a TLS-secured,
//!   fingerprint-pinned connection layer, mailbox discovery, fetch and the
//!   body cache, the write primitives, SMTP submission, and the
//!   [`ImapProvider`] the sync engine drives. The connection code lifts the
//!   patterns the Slice 0 spike (`examples/imap_spike.rs`) proved — STARTTLS
//!   over a stream we own, the pinning `rustls` `ServerCertVerifier`,
//!   `run_command` for response codes — into the real crate.

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

// Connection layer and certificate pinning, used by account setup.
pub use connection::{CertificateProbe, ConnectionConfig, ImapConnectionManager};
pub use session::ImapSession;
pub use tls::{parse_sha256_fingerprint, Sha256Fingerprint};

// Account-setup surface, consumed by the tauri commands: autodiscovery, the
// non-secret settings, and the test-and-save / cert-probe orchestration.
pub use discovery::{discover, DiscoveryResult};
pub use settings::host_port_key;
pub use settings::{Identity, ImapAccountSettings, LabelStorage, SecurityMode};
pub use setup::{
    connect_with_settings, plain_language, probe_imap_certificate, test_imap, TestReport,
};
pub use smtp::{probe_smtp_certificate, test_smtp};
// Mailbox-discovery surface: the mapping types the frontend renders and the
// discovery/create orchestration.
pub use mailboxes::{create_mailbox_tolerant, discover_and_persist, MailboxMapping, MailboxRole};

pub use fetch::BodyCache;

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
// predicate.
pub use policy::smtp_prepare_retry_due;
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
