//! The IMAP provider: connection layer and persistent sync state.
//!
//! This module roots everything IMAP. Two concerns live under it:
//!
//! * **Persistent sync state** (this file). `docs/imap-design.md`
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
mod policy;
mod provider;
mod rfc822;
mod session;
mod settings;
mod setup;
mod threading;
mod tls;

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
pub(crate) use settings::{read_settings_row, write_settings_row};
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
    plain_language, probe_imap_certificate, probe_smtp_certificate, test_imap, test_smtp, TestReport,
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
pub use setup::{default_imap_port, default_smtp_port};
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
pub use identity::{derive_message_id, IdentityInputs};
#[allow(unused_imports)]
pub use rfc822::{attachment_bytes_from_raw, to_raw_message};
#[allow(unused_imports)]
pub use rfc822::{threading_headers_from_raw, ThreadingHeaders};

// Slice 5a live-sync surface: the provider the engine drives, plus the pure
// logic modules it is built from.
pub use provider::{label_model_for, ImapProvider, ImapProviderConfig};

use std::sync::Arc;

use crate::db::{Database, DbResult};

/// One IMAP mailbox's catalog row: its decoded name, hierarchy delimiter,
/// special-use attribute, and the UID counters a resync compares against.
///
/// Mirrors the `imap_mailboxes` columns in `docs/imap-design.md`. The
/// permanent-flags list is stored as its JSON text exactly as the server
/// listed it, and `permanent_keywords` records whether `PERMANENTFLAGS`
/// contained `\*` (so the account can store arbitrary keyword labels).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImapMailbox {
    pub name: String,
    pub delimiter: Option<String>,
    pub special_use: Option<String>,
    pub uidvalidity: i64,
    pub uidnext: i64,
    pub highestmodseq: Option<i64>,
    /// Unknown until a writable SELECT reports PERMANENTFLAGS.
    pub permanent_flags_json: Option<String>,
    pub permanent_keywords: Option<bool>,
}

/// Where one message copy lives: a `(mailbox, uidvalidity, uid)` coordinate
/// mapped to the stable cross-account message id, with that copy's flags and
/// optional CONDSTORE modseq. Mirrors the `imap_locations` columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImapLocation {
    pub mailbox: String,
    pub uidvalidity: i64,
    pub uid: i64,
    pub message_id: String,
    pub flags_json: String,
    pub modseq: Option<i64>,
}

/// A narrow, account-scoped handle onto the IMAP provider's persistent state.
///
/// Constructed when the IMAP provider is built (phase 2) and handed only the
/// account it serves, so one account's provider can never read another's UID
/// map. Every method is a thin wrapper over the shared [`Database`]
/// connection; the two tables are owned here rather than in `db/` because they
/// are provider-internal and have no reader outside the IMAP provider.
#[derive(Clone)]
pub struct ImapStateStore {
    database: Arc<Database>,
    account_id: String,
}

impl ImapStateStore {
    /// Binds a store to one account's rows in the shared database.
    pub fn new(database: Arc<Database>, account_id: impl Into<String>) -> Self {
        Self {
            database,
            account_id: account_id.into(),
        }
    }

    /// The account this store is scoped to.
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// The shared database handle, for the sibling body cache
    /// ([`crate::provider::imap::BodyCache`]) which owns its own table but
    /// scopes every statement to this store's account.
    pub(super) fn database(&self) -> &Arc<Database> {
        &self.database
    }

    /// The stable message id already recorded for one `(mailbox, uidvalidity,
    /// uid)` coordinate, if any. This is the sticky-id lookup `ensure_body`
    /// does before deriving: a message we already know keeps its id even if the
    /// server later begins advertising OBJECTID.
    pub fn location_message_id(
        &self,
        mailbox: &str,
        uidvalidity: i64,
        uid: i64,
    ) -> DbResult<Option<String>> {
        self.database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT message_id FROM imap_locations
                     WHERE account_id = ?1 AND mailbox = ?2 AND uidvalidity = ?3 AND uid = ?4",
                    rusqlite::params![self.account_id, mailbox, uidvalidity, uid],
                    |row| row.get(0),
                )
                .ok())
        })
    }

    /// Inserts or updates one mailbox's catalog row. Unknown write capabilities
    /// from read-only discovery preserve values learned by a writable SELECT.
    pub fn upsert_mailbox(&self, mailbox: &ImapMailbox) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO imap_mailboxes
                    (account_id, name, delimiter, special_use, uidvalidity, uidnext,
                     highestmodseq, permanent_flags_json, permanent_keywords)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(account_id, name) DO UPDATE SET
                     delimiter = excluded.delimiter,
                     special_use = excluded.special_use,
                     uidvalidity = excluded.uidvalidity,
                     uidnext = excluded.uidnext,
                     highestmodseq = excluded.highestmodseq,
                     permanent_flags_json = COALESCE(excluded.permanent_flags_json, imap_mailboxes.permanent_flags_json),
                     permanent_keywords = COALESCE(excluded.permanent_keywords, imap_mailboxes.permanent_keywords)",
                rusqlite::params![
                    self.account_id,
                    mailbox.name,
                    mailbox.delimiter,
                    mailbox.special_use,
                    mailbox.uidvalidity,
                    mailbox.uidnext,
                    mailbox.highestmodseq,
                    mailbox.permanent_flags_json,
                    mailbox.permanent_keywords.map(i64::from),
                ],
            )?;
            Ok(())
        })
    }

    /// This account's mailboxes, ordered by name for a stable listing.
    pub fn mailboxes(&self) -> DbResult<Vec<ImapMailbox>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT name, delimiter, special_use, uidvalidity, uidnext,
                        highestmodseq, permanent_flags_json, permanent_keywords
                 FROM imap_mailboxes WHERE account_id = ?1 ORDER BY name",
            )?;
            let rows = statement
                .query_map([&self.account_id], |row| {
                    Ok(ImapMailbox {
                        name: row.get(0)?,
                        delimiter: row.get(1)?,
                        special_use: row.get(2)?,
                        uidvalidity: row.get(3)?,
                        uidnext: row.get(4)?,
                        highestmodseq: row.get(5)?,
                        permanent_flags_json: row.get(6)?,
                        permanent_keywords: row.get::<_, Option<i64>>(7)?.map(|value| value != 0),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Inserts or replaces one UID's location row.
    pub fn upsert_location(&self, location: &ImapLocation) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO imap_locations
                    (account_id, mailbox, uidvalidity, uid, message_id, flags_json, modseq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(account_id, mailbox, uidvalidity, uid) DO UPDATE SET
                     message_id = excluded.message_id,
                     flags_json = excluded.flags_json,
                     modseq = excluded.modseq",
                rusqlite::params![
                    self.account_id,
                    location.mailbox,
                    location.uidvalidity,
                    location.uid,
                    location.message_id,
                    location.flags_json,
                    location.modseq,
                ],
            )?;
            Ok(())
        })
    }

    /// Every place a given stable message id is currently stored, so a
    /// message with copies in several mailboxes resolves to all its UIDs.
    pub fn locations_for_message(&self, message_id: &str) -> DbResult<Vec<ImapLocation>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT mailbox, uidvalidity, uid, message_id, flags_json, modseq
                 FROM imap_locations
                 WHERE account_id = ?1 AND message_id = ?2
                 ORDER BY mailbox, uid",
            )?;
            let rows = statement
                .query_map(rusqlite::params![self.account_id, message_id], |row| {
                    Ok(ImapLocation {
                        mailbox: row.get(0)?,
                        uidvalidity: row.get(1)?,
                        uid: row.get(2)?,
                        message_id: row.get(3)?,
                        flags_json: row.get(4)?,
                        modseq: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Drops every location in one mailbox: used on a `UIDVALIDITY` reset,
    /// which invalidates that mailbox's UIDs without touching cached bodies
    /// (keyed by the stable message id) or any other mailbox's rows.
    pub fn drop_mailbox_locations(&self, mailbox: &str) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "DELETE FROM imap_locations WHERE account_id = ?1 AND mailbox = ?2",
                rusqlite::params![self.account_id, mailbox],
            )?;
            Ok(())
        })
    }

    /// Every location row in one mailbox, ordered by UID. The sync round reads
    /// this to build its local view for the delta.
    pub fn locations_in_mailbox(&self, mailbox: &str) -> DbResult<Vec<ImapLocation>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT mailbox, uidvalidity, uid, message_id, flags_json, modseq
                 FROM imap_locations
                 WHERE account_id = ?1 AND mailbox = ?2
                 ORDER BY uid",
            )?;
            let rows = statement
                .query_map(rusqlite::params![self.account_id, mailbox], |row| {
                    Ok(ImapLocation {
                        mailbox: row.get(0)?,
                        uidvalidity: row.get(1)?,
                        uid: row.get(2)?,
                        message_id: row.get(3)?,
                        flags_json: row.get(4)?,
                        modseq: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Delete one UID's location row (expunged by another client). Bodies and
    /// thread rows are untouched; a thread whose last location is gone resolves
    /// to no messages and `fetch_thread` reports it absent.
    pub fn delete_location(&self, mailbox: &str, uidvalidity: i64, uid: i64) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "DELETE FROM imap_locations
                 WHERE account_id = ?1 AND mailbox = ?2 AND uidvalidity = ?3 AND uid = ?4",
                rusqlite::params![self.account_id, mailbox, uidvalidity, uid],
            )?;
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // Slice 5a live-sync state: generation, change journal, thread grouping,
    // and merge aliases (schema v58). These back the at-least-once sync cursor
    // and local threading; see `docs/imap-design.md` ("Sync" / "Threading").
    // -----------------------------------------------------------------------

    /// The account's current sync generation (0 before any sync round).
    pub fn generation(&self) -> DbResult<u64> {
        self.database.with_connection(|connection| {
            let generation: i64 = connection
                .query_row(
                    "SELECT generation FROM imap_sync_state WHERE account_id = ?1",
                    [&self.account_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(generation as u64)
        })
    }

    /// Which local thread a message belongs to, if it has been threaded.
    pub fn thread_of_message(&self, message_id: &str) -> DbResult<Option<String>> {
        self.database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT thread_id FROM imap_threads
                     WHERE account_id = ?1 AND message_id = ?2",
                    rusqlite::params![self.account_id, message_id],
                    |row| row.get(0),
                )
                .ok())
        })
    }

    /// The stable message ids currently grouped under one thread id.
    pub fn messages_in_thread(&self, thread_id: &str) -> DbResult<Vec<String>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT message_id FROM imap_threads
                 WHERE account_id = ?1 AND thread_id = ?2 ORDER BY message_id",
            )?;
            let rows = statement
                .query_map(rusqlite::params![self.account_id, thread_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Resolve an id through the alias chain to the surviving thread id. A
    /// thread never merged resolves to itself. Bounded against a cyclic or
    /// self-referential alias so a corrupt row cannot loop forever.
    pub fn resolve_thread_alias(&self, thread_id: &str) -> DbResult<String> {
        self.database.with_connection(|connection| {
            let mut current = thread_id.to_string();
            for _ in 0..64 {
                let next: Option<String> = connection
                    .query_row(
                        "SELECT new_id FROM imap_thread_aliases
                         WHERE account_id = ?1 AND old_id = ?2",
                        rusqlite::params![self.account_id, current],
                        |row| row.get(0),
                    )
                    .ok();
                match next {
                    Some(new_id) if new_id != current => current = new_id,
                    _ => break,
                }
            }
            Ok(current)
        })
    }

    /// Every thread id recorded in the change journal with a generation
    /// strictly greater than `since`, de-duplicated. This is the poll scan:
    /// `poll(cursor=g)` returns these.
    pub fn journal_since(&self, since: u64) -> DbResult<Vec<String>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT DISTINCT thread_id FROM imap_change_journal
                 WHERE account_id = ?1 AND generation > ?2 ORDER BY thread_id",
            )?;
            let rows = statement
                .query_map(rusqlite::params![self.account_id, since as i64], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Every `(message_id, thread_id, created_generation)` row for this
    /// account, ordered by message id. The sync round reads this to rebuild
    /// the threader's prior state — including each thread's PERSISTED creation
    /// generation, so a merge picks the genuinely older thread rather than a
    /// hash ordering.
    pub fn all_message_threads(&self) -> DbResult<Vec<(String, String, u64)>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT message_id, thread_id, created_generation FROM imap_threads
                 WHERE account_id = ?1 ORDER BY message_id",
            )?;
            let rows = statement
                .query_map([&self.account_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? as u64,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Thread ids currently located in a given mailbox (via the location ->
    /// thread join). Used by `list_inbox`. Returned sorted and de-duplicated.
    pub fn thread_ids_in_mailbox(&self, mailbox: &str) -> DbResult<Vec<String>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT DISTINCT t.thread_id
                 FROM imap_locations l JOIN imap_threads t
                   ON t.account_id = l.account_id AND t.message_id = l.message_id
                 WHERE l.account_id = ?1 AND l.mailbox = ?2
                 ORDER BY t.thread_id",
            )?;
            let rows = statement
                .query_map(rusqlite::params![self.account_id, mailbox], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Apply one sync round's writes in a SINGLE transaction. This is the
    /// at-least-once anchor: locations, flag updates, deletions, thread
    /// assignments, aliases, the journal append AND the generation bump all
    /// commit together, so a round that fails before this call leaves durable
    /// state exactly as the previous round left it (body-cache puts aside,
    /// which are rebuildable and deduplicated by stable id).
    ///
    /// Each newly-assigned thread records a `created_generation` so thread age
    /// is a persisted fact, not a hash ordering: a thread's creation
    /// generation is the minimum across its messages, and an existing thread
    /// keeps its earliest. Returns the new generation.
    ///
    /// An empty round (nothing changed) is a no-op that returns the CURRENT
    /// generation without bumping it or writing the journal — see
    /// [`SyncRoundWrite::is_empty`].
    pub fn commit_sync_round(&self, round: &SyncRoundWrite) -> DbResult<u64> {
        self.database.with_transaction(|transaction| {
            let current: i64 = transaction
                .query_row(
                    "SELECT generation FROM imap_sync_state WHERE account_id = ?1",
                    [&self.account_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            // Idle round: do not bump the generation or touch the journal.
            if round.is_empty() {
                return Ok(current as u64);
            }

            let next = current + 1;
            transaction.execute(
                "INSERT INTO imap_sync_state (account_id, generation) VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET generation = excluded.generation",
                rusqlite::params![self.account_id, next],
            )?;

            // Location writes (new mail + flag updates) and deletions, so the
            // next round's local view already reflects this round even if a
            // LATER round fails: everything a round decides is applied together.
            for location in &round.locations {
                transaction.execute(
                    "INSERT INTO imap_locations
                        (account_id, mailbox, uidvalidity, uid, message_id, flags_json, modseq)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(account_id, mailbox, uidvalidity, uid) DO UPDATE SET
                         message_id = excluded.message_id,
                         flags_json = excluded.flags_json,
                         modseq = excluded.modseq",
                    rusqlite::params![
                        self.account_id,
                        location.mailbox,
                        location.uidvalidity,
                        location.uid,
                        location.message_id,
                        location.flags_json,
                        location.modseq,
                    ],
                )?;
            }
            for (mailbox, uidvalidity, uid) in &round.deletions {
                transaction.execute(
                    "DELETE FROM imap_locations
                     WHERE account_id = ?1 AND mailbox = ?2 AND uidvalidity = ?3 AND uid = ?4",
                    rusqlite::params![self.account_id, mailbox, uidvalidity, uid],
                )?;
            }

            for (message_id, thread_id) in &round.thread_assignments {
                // A thread's creation generation is the earliest generation any
                // of its messages was assigned: an existing thread keeps its
                // recorded minimum; a brand-new thread is created at `next`.
                let existing: Option<i64> = transaction
                    .query_row(
                        "SELECT MIN(created_generation) FROM imap_threads
                         WHERE account_id = ?1 AND thread_id = ?2",
                        rusqlite::params![self.account_id, thread_id],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten();
                let created = existing.unwrap_or(next);
                transaction.execute(
                    "INSERT INTO imap_threads (account_id, message_id, thread_id, created_generation)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(account_id, message_id) DO UPDATE SET
                         thread_id = excluded.thread_id",
                    rusqlite::params![self.account_id, message_id, thread_id, created],
                )?;
            }
            for (old_id, new_id) in &round.aliases {
                // Move pre-existing members as well as this round's assignments.
                // Alias order follows merge order, so chained merges move again.
                transaction.execute(
                    "UPDATE imap_threads SET thread_id = ?3
                     WHERE account_id = ?1 AND thread_id = ?2",
                    rusqlite::params![self.account_id, old_id, new_id],
                )?;
                transaction.execute(
                    "INSERT INTO imap_thread_aliases (account_id, old_id, new_id)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(account_id, old_id) DO UPDATE SET new_id = excluded.new_id",
                    rusqlite::params![self.account_id, old_id, new_id],
                )?;
            }
            for thread_id in &round.changed_threads {
                transaction.execute(
                    "INSERT OR IGNORE INTO imap_change_journal
                        (account_id, generation, thread_id) VALUES (?1, ?2, ?3)",
                    rusqlite::params![self.account_id, next, thread_id],
                )?;
            }
            Ok(next as u64)
        })
    }

    pub fn thread_aliases(&self) -> DbResult<Vec<(String, String)>> {
        let old_ids = self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT old_id FROM imap_thread_aliases WHERE account_id = ?1 ORDER BY old_id",
            )?;
            let rows = statement.query_map([&self.account_id], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })?;
        old_ids
            .into_iter()
            .map(|old| {
                let canonical = self.resolve_thread_alias(&old)?;
                Ok((old, canonical))
            })
            .collect()
    }

    /// A thread's persisted creation generation (the minimum across its
    /// messages), used to decide which of two threads is older on a merge. A
    /// thread with no rows yet returns `None`.
    pub fn thread_created_generation(&self, thread_id: &str) -> DbResult<Option<u64>> {
        self.database.with_connection(|connection| {
            let created: Option<i64> = connection
                .query_row(
                    "SELECT MIN(created_generation) FROM imap_threads
                     WHERE account_id = ?1 AND thread_id = ?2",
                    rusqlite::params![self.account_id, thread_id],
                    |row| row.get(0),
                )
                .ok()
                .flatten();
            Ok(created.map(|value| value as u64))
        })
    }

    /// Prune journal rows older than the retention bound relative to the
    /// current generation, so the journal does not grow without limit. A
    /// cursor pointing at a pruned generation is answered with
    /// `InvalidCursor` by [`policy::journal_cursor_is_answerable`].
    pub fn prune_journal(&self) -> DbResult<()> {
        self.database.with_connection(|connection| {
            let current: i64 = connection
                .query_row(
                    "SELECT generation FROM imap_sync_state WHERE account_id = ?1",
                    [&self.account_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let oldest_kept =
                current.saturating_sub(policy::JOURNAL_RETENTION_GENERATIONS as i64);
            connection.execute(
                "DELETE FROM imap_change_journal
                 WHERE account_id = ?1 AND generation < ?2",
                rusqlite::params![self.account_id, oldest_kept],
            )?;
            Ok(())
        })
    }
}

/// The writes one sync round applies atomically through
/// [`ImapStateStore::commit_sync_round`].
///
/// Every mutation a round makes to durable sync state is carried here and
/// applied in a SINGLE transaction, so a round that fails part-way (a dropped
/// connection on a later batch) leaves NOTHING written: the next round sees the
/// same prior state and redoes the work. Body-cache puts are the one exception
/// — they happen eagerly during fetch because the cache is rebuildable and is
/// keyed by the stable id, so a re-fetched body is deduplicated, never
/// double-counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncRoundWrite {
    /// Location rows to insert or update (new mail and flag changes alike).
    pub locations: Vec<ImapLocation>,
    /// `(mailbox, uidvalidity, uid)` location rows to delete (expunged).
    pub deletions: Vec<(String, i64, i64)>,
    /// `(message_id, thread_id)` rows to upsert into `imap_threads`.
    pub thread_assignments: Vec<(String, String)>,
    /// `(old_id, new_id)` merge aliases to record.
    pub aliases: Vec<(String, String)>,
    /// Thread ids whose content or labels changed this round, appended to the
    /// journal under the new generation.
    pub changed_threads: Vec<String>,
}

impl SyncRoundWrite {
    /// Whether this round changed anything at all. An idle poll (no new mail,
    /// no flag change, no deletion, no threading effect) produces an empty
    /// round, and the provider skips the commit entirely so the generation and
    /// journal do not move.
    pub fn is_empty(&self) -> bool {
        self.locations.is_empty()
            && self.deletions.is_empty()
            && self.thread_assignments.is_empty()
            && self.aliases.is_empty()
            && self.changed_threads.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    // An in-memory database so the store's SQL actually runs (no sandbox FS).
    fn store() -> ImapStateStore {
        let database = Arc::new(Database::open_memory());
        ImapStateStore::new(database, "me@example.com")
    }

    #[test]
    fn a_mailbox_upserts_and_reads_back_with_its_counters() {
        let store = store();
        let inbox = ImapMailbox {
            name: "INBOX".into(),
            delimiter: Some("/".into()),
            special_use: None,
            uidvalidity: 95479608,
            uidnext: 979,
            highestmodseq: None,
            permanent_flags_json: Some(r#"["\\Seen","\\Flagged"]"#.into()),
            permanent_keywords: Some(false),
        };
        store.upsert_mailbox(&inbox).unwrap();
        // A second upsert updates in place rather than duplicating.
        let moved = ImapMailbox {
            uidnext: 1001,
            ..inbox.clone()
        };
        store.upsert_mailbox(&moved).unwrap();
        let mailboxes = store.mailboxes().unwrap();
        assert_eq!(mailboxes, vec![moved]);
    }

    #[test]
    fn one_message_in_two_mailboxes_resolves_to_both_locations() {
        let store = store();
        for mailbox in ["INBOX", "Sent"] {
            store
                .upsert_location(&ImapLocation {
                    mailbox: mailbox.into(),
                    uidvalidity: 1,
                    uid: 42,
                    message_id: "imap:me@example.com:abc".into(),
                    flags_json: r#"["\\Seen"]"#.into(),
                    modseq: None,
                })
                .unwrap();
        }
        let locations = store.locations_for_message("imap:me@example.com:abc").unwrap();
        let mailboxes: Vec<_> = locations.iter().map(|location| location.mailbox.as_str()).collect();
        assert_eq!(mailboxes, vec!["INBOX", "Sent"]);
    }

    #[test]
    fn dropping_a_mailboxs_locations_leaves_other_mailboxes_alone() {
        let store = store();
        for (mailbox, uid) in [("INBOX", 1), ("Archive", 2)] {
            store
                .upsert_location(&ImapLocation {
                    mailbox: mailbox.into(),
                    uidvalidity: 1,
                    uid,
                    message_id: format!("imap:me@example.com:{mailbox}"),
                    flags_json: "[]".into(),
                    modseq: None,
                })
                .unwrap();
        }
        store.drop_mailbox_locations("INBOX").unwrap();
        assert!(store
            .locations_for_message("imap:me@example.com:INBOX")
            .unwrap()
            .is_empty());
        assert_eq!(
            store.locations_for_message("imap:me@example.com:Archive").unwrap().len(),
            1
        );
    }

    #[test]
    fn a_store_only_sees_its_own_accounts_rows() {
        let database = Arc::new(Database::open_memory());
        let mine = ImapStateStore::new(database.clone(), "me@example.com");
        let theirs = ImapStateStore::new(database, "other@example.com");
        theirs
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(),
                uidvalidity: 1,
                uid: 1,
                message_id: "imap:other@example.com:x".into(),
                flags_json: "[]".into(),
                modseq: None,
            })
            .unwrap();
        // The other account's row is invisible here, and my own catalog is empty.
        assert!(mine
            .locations_for_message("imap:other@example.com:x")
            .unwrap()
            .is_empty());
        assert!(mine.mailboxes().unwrap().is_empty());
    }

    #[test]
    fn an_empty_sync_round_is_a_no_op_and_does_not_bump_the_generation() {
        // SLICE5A_FIXES item 5 at the store level: committing an empty round
        // leaves the generation and journal untouched.
        let store = store();
        assert_eq!(store.generation().unwrap(), 0);
        let unchanged = store.commit_sync_round(&SyncRoundWrite::default()).unwrap();
        assert_eq!(unchanged, 0, "empty round returns the current generation");
        assert_eq!(store.generation().unwrap(), 0, "no bump");
        assert!(store.journal_since(0).unwrap().is_empty(), "no journal write");
    }

    #[test]
    fn a_sync_round_applies_locations_deletions_and_journal_atomically() {
        // SLICE5A_FIXES item 1 at the store level: a round's location writes,
        // deletions, thread rows and journal all land together and bump the
        // generation exactly once.
        let store = store();
        // Pre-existing location to be deleted by the round.
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(),
                uidvalidity: 1,
                uid: 1,
                message_id: "imap:me@example.com:old".into(),
                flags_json: "[]".into(),
                modseq: None,
            })
            .unwrap();
        let round = SyncRoundWrite {
            locations: vec![ImapLocation {
                mailbox: "INBOX".into(),
                uidvalidity: 1,
                uid: 2,
                message_id: "imap:me@example.com:new".into(),
                flags_json: r#"["\\Seen"]"#.into(),
                modseq: None,
            }],
            deletions: vec![("INBOX".into(), 1, 1)],
            thread_assignments: vec![(
                "imap:me@example.com:new".into(),
                "imap:t:new".into(),
            )],
            changed_threads: vec!["imap:t:new".into()],
            ..Default::default()
        };
        let generation = store.commit_sync_round(&round).unwrap();
        assert_eq!(generation, 1);
        // The new location exists, the old one is gone.
        assert_eq!(
            store.locations_for_message("imap:me@example.com:new").unwrap().len(),
            1
        );
        assert!(store
            .locations_for_message("imap:me@example.com:old")
            .unwrap()
            .is_empty());
        // The thread was journaled under the new generation.
        assert_eq!(store.journal_since(0).unwrap(), vec!["imap:t:new".to_string()]);
    }

    #[test]
    fn a_thread_keeps_its_earliest_creation_generation() {
        // SLICE5A_FIXES item 4 at the store level: a thread's creation
        // generation is the minimum across its messages and never increases
        // when a later message joins.
        let store = store();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:x".into())],
                changed_threads: vec!["imap:t:x".into()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.thread_created_generation("imap:t:x").unwrap(), Some(1));
        // A later round adds a second message to the same thread.
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:m2".into(), "imap:t:x".into())],
                changed_threads: vec!["imap:t:x".into()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            store.thread_created_generation("imap:t:x").unwrap(),
            Some(1),
            "the thread keeps its earliest creation generation"
        );
    }
}
