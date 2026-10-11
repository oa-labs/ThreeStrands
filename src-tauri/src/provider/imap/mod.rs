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
mod plan;
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

    /// CATALOG-ONLY upsert: insert or update ONLY a mailbox's catalog
    /// attributes — NAME, DELIMITER, SPECIAL_USE — never its counter or
    /// write-capability columns (`uidvalidity`, `uidnext`, `highestmodseq`,
    /// `permanent_flags_json`, `permanent_keywords`).
    ///
    /// This is the round-time catalog refresh (Slice 5b-1 run 3, item 3): a
    /// periodic `LIST` notices a mailbox created after account setup (a new
    /// Sent/Trash/Junk) and records it so the plan can pick it up. A NEW row is
    /// born with ZERO counters, exactly as discovery's first pass leaves a
    /// freshly-listed mailbox (the sync round then fills them from EXAMINE);
    /// an EXISTING row keeps every counter and capability it already learned,
    /// because `upsert_mailbox` would overwrite them and `ImapMailbox` cannot
    /// carry "leave unchanged". `\Noselect` entries must never be passed here
    /// (the caller filters them); this method does not re-check.
    pub fn upsert_mailbox_catalog(
        &self,
        name: &str,
        delimiter: Option<&str>,
        special_use: Option<&str>,
    ) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO imap_mailboxes
                    (account_id, name, delimiter, special_use, uidvalidity, uidnext,
                     highestmodseq, permanent_flags_json, permanent_keywords)
                 VALUES (?1, ?2, ?3, ?4, 0, 0, NULL, NULL, NULL)
                 ON CONFLICT(account_id, name) DO UPDATE SET
                     delimiter = excluded.delimiter,
                     special_use = excluded.special_use",
                rusqlite::params![self.account_id, name, delimiter, special_use],
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

    /// All durable hot-thread markers for baseline recovery, including retired
    /// aliases and threads with no remaining locations. Those ids must reach
    /// the engine to reconcile merges and deletions even after journal pruning.
    pub fn hot_thread_ids(&self) -> DbResult<Vec<String>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT thread_id FROM imap_hot_threads
                 WHERE account_id = ?1 ORDER BY thread_id",
            )?;
            let rows = statement
                .query_map([&self.account_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
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

    /// The threads (and their messages) that share ANY token with the incoming
    /// batch's tokens — the targeted seed set for the new loader. Given the set
    /// of tokens the incoming messages carry, this returns every existing
    /// `(message_id, thread_id, created_generation, token)` row whose token is
    /// in that set OR whose thread is reached through one of those matches,
    /// resolving thread ids through the alias chain. It reads ONLY
    /// `imap_message_tokens` + `imap_threads` + `imap_thread_aliases`; it never
    /// touches `imap_bodies`.
    ///
    /// The join is done in two steps so the alias resolution stays in Rust
    /// (the alias chain is bounded there): first find the DISTINCT message ids
    /// that carry any batch token, then read every token of every thread those
    /// messages belong to, so a seeded thread is seeded with ALL of its tokens
    /// (not only the ones the batch happened to mention). This matters for a
    /// merge: a late linker that references thread A's token must also see
    /// thread B's tokens to merge them, which it does because A and B are
    /// pulled in whole once any of their tokens matches.
    ///
    /// Returns `(thread_id -> (created_generation, [token, …]))`, thread ids
    /// already resolved to their alias survivors and token lists
    /// de-duplicated, ready to hand to [`threading::ThreadState::seed`].
    pub fn seed_state_for_tokens(
        &self,
        batch_tokens: &[String],
    ) -> DbResult<std::collections::BTreeMap<String, (u64, Vec<String>)>> {
        use std::collections::{BTreeMap, BTreeSet};
        if batch_tokens.is_empty() {
            return Ok(BTreeMap::new());
        }
        // De-duplicate the batch tokens for a stable, bounded IN-list.
        let unique: BTreeSet<&str> = batch_tokens.iter().map(String::as_str).collect();

        self.database.with_connection(|connection| {
            // Step 1: messages that carry any batch token, and their threads.
            // Resolve each thread through the alias chain in Rust. CROSS JOIN
            // preserves the selective outer lookup: SQLite may otherwise
            // reorder these joins into an account-wide message or alias scan.
            let mut threads: BTreeSet<String> = BTreeSet::new();
            // SQLite caps bound variables per statement (32,766 in the bundled
            // build, 999 in older ones), and an initial sync threads thousands
            // of messages in one round, so query the token set in chunks.
            let unique_tokens: Vec<&str> = unique.iter().copied().collect();
            for chunk in unique_tokens.chunks(policy::TOKEN_QUERY_CHUNK) {
                let placeholders = std::iter::repeat("?")
                    .take(chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "SELECT DISTINCT t.thread_id
                     FROM imap_message_tokens mt INDEXED BY imap_message_tokens_by_token
                     CROSS JOIN imap_threads t
                       ON t.account_id = mt.account_id AND t.message_id = mt.message_id
                     WHERE mt.account_id = ?1 AND mt.token IN ({placeholders})",
                );
                let mut statement = connection.prepare(&sql)?;
                let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
                params.push(&self.account_id);
                for token in chunk {
                    params.push(token);
                }
                let rows = statement
                    .query_map(params.as_slice(), |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                for thread in rows {
                    threads.insert(self.resolve_thread_alias_conn(connection, &thread)?);
                }
            }
            if threads.is_empty() {
                return Ok(BTreeMap::new());
            }

            // Step 2: visit only the matched thread families. Reverse aliases
            // and message membership both have account-scoped indexes, so a
            // reply does not scan unrelated mail or resolve every stored row.
            // CROSS JOIN keeps the family on the outer side of each lookup;
            // UNION also terminates traversal if persisted aliases are cyclic.
            let mut out: BTreeMap<String, (u64, Vec<String>)> = BTreeMap::new();
            let mut dedup: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

            let mut statement = connection.prepare(
                "WITH RECURSIVE family(thread_id) AS (
                     VALUES (?2)
                     UNION
                     SELECT a.old_id FROM family f
                     CROSS JOIN imap_thread_aliases a ON a.new_id = f.thread_id
                     WHERE a.account_id = ?1
                 )
                 SELECT t.message_id, t.created_generation, mt.token
                 FROM family f
                 CROSS JOIN imap_threads t
                   ON t.account_id = ?1 AND t.thread_id = f.thread_id
                 LEFT JOIN imap_message_tokens mt
                   ON mt.account_id = t.account_id AND mt.message_id = t.message_id",
            )?;
            for thread_id in threads {
                let rows =
                    statement.query_map(rusqlite::params![self.account_id, thread_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)? as u64,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    })?;
                for row in rows {
                    let (message_id, created, token) = row?;
                    let entry = out
                        .entry(thread_id.clone())
                        .or_insert((created, Vec::new()));
                    entry.0 = entry.0.min(created);
                    let bucket = dedup.entry(thread_id.clone()).or_default();
                    // Retain the stable-id anchor even for a legacy member
                    // that has no persisted tokens yet.
                    bucket.insert(message_id);
                    if let Some(token) = token {
                        bucket.insert(token);
                    }
                }
            }
            for (thread_id, tokens) in dedup {
                if let Some(entry) = out.get_mut(&thread_id) {
                    entry.1 = tokens.into_iter().collect();
                }
            }
            Ok(out)
        })
    }

    /// Resolve an alias chain on an already-held connection (so the targeted
    /// seed can resolve many ids without reopening the guard per id). Mirrors
    /// [`resolve_thread_alias`], bounded against a cyclic alias.
    fn resolve_thread_alias_conn(
        &self,
        connection: &rusqlite::Connection,
        thread_id: &str,
    ) -> DbResult<String> {
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
    }

    /// Whether this account's one-time token backfill has completed. A row that
    /// does not exist yet (brand-new account, no sync round) reports `true`:
    /// such an account has no token-less threads, so there is nothing to
    /// backfill. An existing (pre-v59) account's row defaults to `false` until
    /// the backfill finishes.
    pub fn tokens_backfilled(&self) -> DbResult<bool> {
        self.database.with_connection(|connection| {
            let done: Option<i64> = connection
                .query_row(
                    "SELECT tokens_backfilled FROM imap_sync_state WHERE account_id = ?1",
                    [&self.account_id],
                    |row| row.get(0),
                )
                .ok();
            // No row => brand-new account => nothing to backfill => done.
            Ok(done.map(|value| value != 0).unwrap_or(true))
        })
    }

    /// Mark this account's token backfill complete. Creates the sync-state row
    /// if it does not exist yet (so an account whose only state is token rows
    /// still records completion), leaving the generation at its default.
    pub fn mark_tokens_backfilled(&self) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO imap_sync_state (account_id, generation, tokens_backfilled)
                 VALUES (?1, 0, 1)
                 ON CONFLICT(account_id) DO UPDATE SET tokens_backfilled = 1",
                [&self.account_id],
            )?;
            Ok(())
        })
    }

    /// Message ids that have a thread row but NO token row yet — the backfill's
    /// work queue, bounded to `limit` for a crash-resumable, bounded-batch
    /// sweep. Ordered by message id so progress is deterministic across runs.
    pub fn messages_missing_tokens(&self, limit: usize) -> DbResult<Vec<String>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT t.message_id FROM imap_threads t
                 WHERE t.account_id = ?1
                   AND NOT EXISTS (
                     SELECT 1 FROM imap_message_tokens mt
                     WHERE mt.account_id = t.account_id AND mt.message_id = t.message_id)
                 ORDER BY t.message_id
                 LIMIT ?2",
            )?;
            let rows = statement
                .query_map(rusqlite::params![self.account_id, limit as i64], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Write the token rows for one backfilled message, idempotently. Each call
    /// is its own transaction so a crash mid-backfill leaves each completed
    /// message whole and the sweep resumes from `messages_missing_tokens`.
    pub fn write_message_tokens(&self, message_id: &str, tokens: &[String]) -> DbResult<()> {
        self.database.with_transaction(|transaction| {
            for token in tokens {
                transaction.execute(
                    "INSERT OR IGNORE INTO imap_message_tokens
                        (account_id, message_id, token) VALUES (?1, ?2, ?3)",
                    rusqlite::params![self.account_id, message_id, token],
                )?;
            }
            Ok(())
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

    // -----------------------------------------------------------------------
    // Slice 5b-1 multi-mailbox state: per-mailbox cadence counters, hot
    // threads (schema v60).
    // -----------------------------------------------------------------------

    /// The stored `(last_exists, last_uidnext, last_sweep_at)` cadence counters
    /// for one mailbox, or `None` if it was never synced. `backfill_low_uid` is
    /// read separately via [`Self::sent_backfill_low_uid`].
    pub fn mailbox_sync_state(&self, mailbox: &str) -> DbResult<Option<(i64, i64, i64)>> {
        self.database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT last_exists, last_uidnext, last_sweep_at
                     FROM imap_mailbox_sync_state
                     WHERE account_id = ?1 AND mailbox = ?2",
                    rusqlite::params![self.account_id, mailbox],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok())
        })
    }

    /// The reserved `imap_mailbox_sync_state` pseudo-mailbox key that carries
    /// the ACCOUNT-level periodic catalog-refresh clock (item 3). It uses a
    /// control character no real IMAP mailbox name contains, so it never
    /// collides with a mailbox and — because the sync plan is built from
    /// `imap_mailboxes`, not this table — never appears in the plan.
    const CATALOG_SWEEP_KEY: &str = "\u{0}catalog-sweep";

    /// Whether the account is due for a periodic catalog refresh (`LIST` +
    /// catalog-only upsert), given `now` (unix seconds). Never refreshed before
    /// (no sentinel row) is always due — this is the "at baseline" case. The
    /// cadence reuses [`policy::folder_sweep_due`] /
    /// [`policy::FOLDER_SWEEP_INTERVAL_SECS`].
    pub fn catalog_refresh_due(&self, now: i64) -> DbResult<bool> {
        let last = self
            .mailbox_sync_state(Self::CATALOG_SWEEP_KEY)?
            .map(|(_, _, last_sweep_at)| last_sweep_at);
        Ok(match last {
            Some(last_sweep_at) => policy::folder_sweep_due(last_sweep_at, now),
            None => true,
        })
    }

    /// Record that a catalog refresh ran at `now`, so the next one is not due
    /// until the sweep interval elapses. Stored on the reserved sentinel row.
    pub fn record_catalog_refresh(&self, now: i64) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO imap_mailbox_sync_state
                    (account_id, mailbox, last_exists, last_uidnext, last_sweep_at)
                 VALUES (?1, ?2, 0, 0, ?3)
                 ON CONFLICT(account_id, mailbox) DO UPDATE SET
                     last_sweep_at = excluded.last_sweep_at",
                rusqlite::params![self.account_id, Self::CATALOG_SWEEP_KEY, now],
            )?;
            Ok(())
        })
    }

    /// The Sent backfill watermark for one mailbox: the lowest UID acquired so
    /// far WHILE the backfill is incomplete, or `None` once the whole Sent
    /// window is acquired (and `None` too when the mailbox has no sync-state
    /// row yet). It is the pending marker [`policy::sent_backfill_incomplete`]
    /// reads: a non-NULL value means cadence gating must NOT skip the mailbox,
    /// because EXISTS/UIDNEXT being unchanged does not mean there is nothing
    /// left to ACQUIRE. Returns `Ok(Some(None))` to mean "row exists, marker
    /// NULL" vs `Ok(None)` for "no row": the outer Option is row presence, the
    /// inner is the nullable column — flattened by the caller to "is there
    /// pending work", so the two collapse to the same answer (not pending).
    pub fn sent_backfill_low_uid(&self, mailbox: &str) -> DbResult<Option<i64>> {
        self.database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT backfill_low_uid
                     FROM imap_mailbox_sync_state
                     WHERE account_id = ?1 AND mailbox = ?2",
                    rusqlite::params![self.account_id, mailbox],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .ok()
                .flatten())
        })
    }

    /// Whether a thread is currently marked hot (has, or ever had, a location
    /// in INBOX or Sent).
    pub fn is_thread_hot(&self, thread_id: &str) -> DbResult<bool> {
        self.database.with_connection(|connection| {
            self.is_thread_hot_conn(connection, thread_id)
        })
    }

    /// A merge inherits hotness from any member, including a marker left on
    /// a retired id by an earlier version. The reverse traversal is indexed
    /// and UNION makes corrupt cycles finite.
    fn is_thread_hot_conn(
        &self,
        connection: &rusqlite::Connection,
        thread_id: &str,
    ) -> DbResult<bool> {
        let resolved = self.resolve_thread_alias_conn(connection, thread_id)?;
        Ok(connection.query_row(
            "WITH RECURSIVE family(thread_id) AS (
                 VALUES (?2)
                 UNION
                 SELECT a.old_id FROM family f
                 CROSS JOIN imap_thread_aliases a ON a.new_id = f.thread_id
                 WHERE a.account_id = ?1
             )
             SELECT EXISTS (
                 SELECT 1 FROM family f
                 CROSS JOIN imap_hot_threads h
                   ON h.account_id = ?1 AND h.thread_id = f.thread_id
             )",
            rusqlite::params![self.account_id, resolved],
            |row| row.get(0),
        )?)
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
                // Still persist per-mailbox cadence counters so an unchanged
                // folder records that it was examined/swept — this does NOT
                // move the generation (item 4).
                if let Some((mailbox, last_exists, last_uidnext, last_sweep_at)) =
                    &round.mailbox_state
                {
                    transaction.execute(
                        "INSERT INTO imap_mailbox_sync_state
                            (account_id, mailbox, last_exists, last_uidnext, last_sweep_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(account_id, mailbox) DO UPDATE SET
                             last_exists = excluded.last_exists,
                             last_uidnext = excluded.last_uidnext,
                             last_sweep_at = excluded.last_sweep_at",
                        rusqlite::params![
                            self.account_id,
                            mailbox,
                            last_exists,
                            last_uidnext,
                            last_sweep_at
                        ],
                    )?;
                    Self::apply_sent_backfill_low_uid(transaction, &self.account_id, mailbox, round)?;
                }
                return Ok(current as u64);
            }

            let next = current + 1;
            // A brand-new account's row is born `tokens_backfilled = 1`: this
            // run writes tokens at assignment, so a new account never has
            // token-less threads and must not trigger the upgrade backfill. An
            // existing row keeps whatever the migration/backfill left.
            transaction.execute(
                "INSERT INTO imap_sync_state (account_id, generation, tokens_backfilled)
                 VALUES (?1, ?2, 1)
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
            // Persisted threading tokens for each newly-assigned message, in
            // the SAME transaction as the assignment so a failed round writes
            // no tokens. Keyed by message id (not thread id), so a merge that
            // re-points threads below leaves these rows correct.
            for (message_id, tokens) in &round.message_tokens {
                for token in tokens {
                    transaction.execute(
                        "INSERT OR IGNORE INTO imap_message_tokens
                            (account_id, message_id, token) VALUES (?1, ?2, ?3)",
                        rusqlite::params![self.account_id, message_id, token],
                    )?;
                }
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
            // Hot threads: a location in INBOX or Sent appeared this round.
            // Inserted in the SAME transaction as that location (item 5), so a
            // failed round records neither. Idempotent on the PK, and resolve
            // through the alias chain so a survivor of a merge stays hot.
            for thread_id in &round.hot_threads {
                let resolved = self.resolve_thread_alias_conn(transaction, thread_id)?;
                transaction.execute(
                    "INSERT OR IGNORE INTO imap_hot_threads (account_id, thread_id)
                     VALUES (?1, ?2)",
                    rusqlite::params![self.account_id, resolved],
                )?;
            }
            // Resolve hotness AFTER all aliases and newly-hot markers are in
            // place. Both ids of a hot merge must reach the engine, even when
            // the survivor was cold before this transaction. Materialize its
            // inherited marker in the same atomic round as the merge.
            let mut changed: std::collections::BTreeSet<String> =
                round.changed_threads.iter().cloned().collect();
            for (old_id, new_id) in &round.aliases {
                let survivor = self.resolve_thread_alias_conn(transaction, new_id)?;
                if self.is_thread_hot_conn(transaction, &survivor)? {
                    transaction.execute(
                        "INSERT OR IGNORE INTO imap_hot_threads (account_id, thread_id)
                         VALUES (?1, ?2)",
                        rusqlite::params![self.account_id, survivor],
                    )?;
                    changed.insert(old_id.clone());
                    changed.insert(new_id.clone());
                    changed.insert(survivor);
                }
            }
            for thread_id in changed {
                transaction.execute(
                    "INSERT OR IGNORE INTO imap_change_journal
                        (account_id, generation, thread_id) VALUES (?1, ?2, ?3)",
                    rusqlite::params![self.account_id, next, thread_id],
                )?;
            }
            // Per-mailbox cadence counters, in the same atomic round.
            if let Some((mailbox, last_exists, last_uidnext, last_sweep_at)) = &round.mailbox_state {
                transaction.execute(
                    "INSERT INTO imap_mailbox_sync_state
                        (account_id, mailbox, last_exists, last_uidnext, last_sweep_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(account_id, mailbox) DO UPDATE SET
                         last_exists = excluded.last_exists,
                         last_uidnext = excluded.last_uidnext,
                         last_sweep_at = excluded.last_sweep_at",
                    rusqlite::params![
                        self.account_id,
                        mailbox,
                        last_exists,
                        last_uidnext,
                        last_sweep_at
                    ],
                )?;
                Self::apply_sent_backfill_low_uid(transaction, &self.account_id, mailbox, round)?;
            }
            Ok(next as u64)
        })
    }

    /// Apply a round's optional Sent backfill watermark to the mailbox's
    /// `imap_mailbox_sync_state` row (already upserted by the caller). A `None`
    /// request leaves the column untouched; `Some(value)` sets it (NULL marks
    /// the backfill complete, a UID marks it in progress at that low-water
    /// mark). Runs inside the round's transaction, so the watermark commits
    /// atomically with the chunk's locations.
    fn apply_sent_backfill_low_uid(
        connection: &rusqlite::Connection,
        account_id: &str,
        mailbox: &str,
        round: &SyncRoundWrite,
    ) -> rusqlite::Result<()> {
        if let Some(value) = round.sent_backfill_low_uid {
            connection.execute(
                "UPDATE imap_mailbox_sync_state SET backfill_low_uid = ?3
                 WHERE account_id = ?1 AND mailbox = ?2",
                rusqlite::params![account_id, mailbox, value],
            )?;
        }
        Ok(())
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
    /// `(message_id, [token, …])` rows to upsert into `imap_message_tokens`:
    /// the exact token set the threader used for each newly-assigned message
    /// (its stable id, normalized `Message-ID`, and capped ancestry). Written
    /// in the SAME transaction as the assignment, so a failed round leaves no
    /// token rows (the at-least-once contract). A message already carrying
    /// tokens from an earlier round need not reappear here.
    pub message_tokens: Vec<(String, Vec<String>)>,
    /// `(old_id, new_id)` merge aliases to record.
    pub aliases: Vec<(String, String)>,
    /// Thread ids whose content or labels changed this round, appended to the
    /// journal under the new generation.
    pub changed_threads: Vec<String>,
    /// Thread ids that became (or are confirmed) HOT this round — a location in
    /// INBOX or Sent appeared. Inserted into `imap_hot_threads` in the SAME
    /// transaction as the location that makes them hot (Slice 5b-1).
    pub hot_threads: Vec<String>,
    /// Optional per-mailbox cadence state to persist this round:
    /// `(mailbox, last_exists, last_uidnext, last_sweep_at)`. Written even for
    /// an otherwise-empty round (an unchanged folder still records that it was
    /// examined / swept), WITHOUT bumping the generation.
    pub mailbox_state: Option<(String, i64, i64, i64)>,
    /// Optional Sent backfill watermark write, applied to the SAME
    /// `imap_mailbox_sync_state` row as `mailbox_state` and in the SAME
    /// transaction as this round's acquired chunk (so progress is exactly as
    /// crash-safe as the locations it accompanies). The outer `Option` is
    /// "touch the column?"; the inner is the nullable value:
    /// `None` → leave `backfill_low_uid` unchanged;
    /// `Some(Some(uid))` → set it to `uid` (backfill still in progress, this
    /// is the lowest UID acquired so far); `Some(None)` → set it NULL (the
    /// whole Sent window is acquired, backfill complete). Requires
    /// `mailbox_state` to be set for the same mailbox (the watermark lives on
    /// that row); ignored otherwise.
    pub sent_backfill_low_uid: Option<Option<i64>>,
}

impl SyncRoundWrite {
    /// Whether this round changed anything that must bump the generation /
    /// touch the journal. An idle poll (no new mail, no flag change, no
    /// deletion, no threading effect, no newly-hot thread) is empty — the
    /// provider skips the generation/journal work. `mailbox_state` is
    /// DELIBERATELY not counted here: persisting a folder's cadence counters is
    /// not a content change and must not bump the generation (item 4 / idle
    /// rounds), so it is written separately in `commit_sync_round`.
    pub fn is_empty(&self) -> bool {
        self.locations.is_empty()
            && self.deletions.is_empty()
            && self.thread_assignments.is_empty()
            && self.aliases.is_empty()
            && self.changed_threads.is_empty()
            && self.hot_threads.is_empty()
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
        mine.commit_sync_round(&SyncRoundWrite {
            hot_threads: vec!["mine".into()],
            ..Default::default()
        })
        .unwrap();
        theirs
            .commit_sync_round(&SyncRoundWrite {
                hot_threads: vec!["theirs".into()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(mine.hot_thread_ids().unwrap(), ["mine"]);
        assert_eq!(theirs.hot_thread_ids().unwrap(), ["theirs"]);
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

    #[test]
    fn hotness_and_journaling_follow_chained_aliases_atomically() {
        let store = store();
        let initial = store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("hot-message".into(), "hot".into())],
                hot_threads: vec!["hot".into()],
                ..Default::default()
            })
            .unwrap();
        // Fail after alias writes, while materializing inherited hotness.
        store
            .database()
            .with_connection(|c| {
                c.execute_batch(
                    "CREATE TRIGGER reject_hot_survivor BEFORE INSERT ON imap_hot_threads
                WHEN NEW.thread_id = 'survivor'
                BEGIN SELECT RAISE(ABORT, 'injected hotness failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();
        let round = SyncRoundWrite {
            aliases: vec![
                ("hot".into(), "middle".into()),
                ("middle".into(), "survivor".into()),
            ],
            ..Default::default()
        };
        assert!(store.commit_sync_round(&round).is_err());
        assert_eq!(store.generation().unwrap(), initial);
        assert_eq!(
            store.thread_of_message("hot-message").unwrap().as_deref(),
            Some("hot")
        );
        assert_eq!(store.resolve_thread_alias("hot").unwrap(), "hot");
        assert!(!store.is_thread_hot("survivor").unwrap());
        assert!(store.journal_since(initial).unwrap().is_empty());
        store
            .database()
            .with_connection(|c| {
                c.execute_batch("DROP TRIGGER reject_hot_survivor;")?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.commit_sync_round(&round).unwrap(), initial + 1);
        assert_eq!(store.resolve_thread_alias("hot").unwrap(), "survivor");
        assert!(store.is_thread_hot("survivor").unwrap());
        assert_eq!(store.hot_thread_ids().unwrap(), ["hot", "survivor"]);
        assert_eq!(
            store.journal_since(initial).unwrap(),
            ["hot", "middle", "survivor"]
        );
        let canonical_markers: i64 = store
            .database()
            .with_connection(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM imap_hot_threads
                WHERE account_id = ?1 AND thread_id = 'survivor'",
                    [&store.account_id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(canonical_markers, 1);
    }

    #[test]
    fn a_cold_merge_stays_index_only_and_does_not_inherit_another_accounts_hotness() {
        let store = store();
        let other = ImapStateStore::new(store.database().clone(), "other@example.com");
        other
            .commit_sync_round(&SyncRoundWrite {
                hot_threads: vec!["old".into()],
                ..Default::default()
            })
            .unwrap();
        let generation = store
            .commit_sync_round(&SyncRoundWrite {
                aliases: vec![("old".into(), "survivor".into())],
                ..Default::default()
            })
            .unwrap();
        assert!(!store.is_thread_hot("survivor").unwrap());
        assert!(store.journal_since(0).unwrap().is_empty());
        assert_eq!(generation, 1);
    }

    #[test]
    fn legacy_alias_families_keep_their_tokens_creation_order_and_hotness() {
        let store = store();
        // Earlier versions could leave the only hot marker on a retired id.
        // Include members still stored under several raw ids, a tokenless
        // member, and same-named aliases in another account.
        store
            .database()
            .with_connection(|c| {
                c.execute_batch(
                    "INSERT INTO imap_threads VALUES
                ('me@example.com','root-message','root',1),
                ('me@example.com','old-message','old',3),
                ('me@example.com','bare-message','middle',2),
                ('other@example.com','foreign-message','root',0);
                INSERT INTO imap_message_tokens VALUES
                ('me@example.com','root-message','root-token'),
                ('me@example.com','old-message','old-token'),
                ('other@example.com','foreign-message','foreign-token');
                INSERT INTO imap_thread_aliases VALUES
                ('me@example.com','old','middle'),
                ('me@example.com','middle','root'),
                ('other@example.com','root','foreign-root');
                INSERT INTO imap_hot_threads VALUES ('me@example.com','old');",
                )?;
                Ok(())
            })
            .unwrap();
        assert!(store.is_thread_hot("root").unwrap());
        assert!(store.is_thread_hot("middle").unwrap());
        assert!(
            !ImapStateStore::new(store.database().clone(), "other@example.com")
                .is_thread_hot("root")
                .unwrap()
        );
        for token in ["root-token", "old-token"] {
            let seeded = store.seed_state_for_tokens(&[token.into()]).unwrap();
            assert_eq!(seeded.len(), 1);
            assert_eq!(seeded["root"].0, 1);
            assert_eq!(
                seeded["root"].1,
                [
                    "bare-message",
                    "old-message",
                    "old-token",
                    "root-message",
                    "root-token"
                ]
            );
        }
        // Corrupt cycles must still terminate in both reverse traversals.
        store
            .database()
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO imap_thread_aliases VALUES ('me@example.com','root','old')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(store.is_thread_hot("root").unwrap());
        assert_eq!(
            store
                .seed_state_for_tokens(&["root-token".into()])
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn token_seeding_work_does_not_grow_with_unrelated_messages_and_aliases() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let store = store();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("reply-parent".into(), "target".into())],
                message_tokens: vec![("reply-parent".into(), vec!["parent-token".into()])],
                ..Default::default()
            })
            .unwrap();
        let measured_seed = || {
            let operations = Arc::new(AtomicUsize::new(0));
            let counter = operations.clone();
            store
                .database()
                .with_connection(|c| {
                    c.progress_handler(
                        1,
                        Some(move || {
                            counter.fetch_add(1, Ordering::Relaxed);
                            false
                        }),
                    )?;
                    Ok(())
                })
                .unwrap();
            let seeded = store
                .seed_state_for_tokens(&["parent-token".into()])
                .unwrap();
            store
                .database()
                .with_connection(|c| {
                    c.progress_handler(0, None::<fn() -> bool>)?;
                    Ok(())
                })
                .unwrap();
            (seeded, operations.load(Ordering::Relaxed))
        };
        let (expected, small_work) = measured_seed();
        // Populate history directly so the measurement covers just seeding,
        // and not the cost of creating the unrelated fixture.
        store
            .database()
            .with_transaction(|tx| {
                for n in 0..2_000 {
                    let message = format!("unrelated-message-{n}");
                    let thread = format!("unrelated-thread-{n}");
                    tx.execute(
                        "INSERT INTO imap_threads VALUES (?1, ?2, ?3, 1)",
                        rusqlite::params![store.account_id, message, thread],
                    )?;
                    tx.execute(
                        "INSERT INTO imap_message_tokens VALUES (?1, ?2, ?3)",
                        rusqlite::params![
                            store.account_id,
                            message,
                            format!("unrelated-token-{n}")
                        ],
                    )?;
                    tx.execute(
                        "INSERT INTO imap_thread_aliases VALUES (?1, ?2, ?3)",
                        rusqlite::params![store.account_id, format!("unrelated-old-{n}"), thread],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let (actual, large_work) = measured_seed();
        assert_eq!(actual, expected);
        assert!(large_work <= small_work * 2 + 100,
            "seeding one thread must not scan unrelated history: {small_work} -> {large_work} VM operations");
    }

    /// An initial sync threads thousands of messages in ONE round, so the
    /// incoming token set can far exceed SQLite's bound-variable limit. The
    /// seeder must cope (chunked queries), not fail the round.
    #[test]
    fn seeding_copes_with_a_token_set_larger_than_sqlites_variable_limit() {
        let store = store();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:x".into())],
                message_tokens: vec![(
                    "imap:me@example.com:m1".into(),
                    vec!["imap:me@example.com:m1".into(), "tok-0".into()],
                )],
                changed_threads: vec!["imap:t:x".into()],
                ..Default::default()
            })
            .unwrap();
        let tokens: Vec<String> = (0..100_000).map(|n| format!("tok-{n}")).collect();
        let seeded = store.seed_state_for_tokens(&tokens).unwrap();
        assert_eq!(seeded.len(), 1, "the one thread sharing tok-0 is seeded");
    }
}
