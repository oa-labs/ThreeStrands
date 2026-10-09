//! The IMAP provider's own persistent sync state.
//!
//! `docs/imap-design.md` ("Data model") gives the IMAP provider a narrow
//! handle onto two tables that the opaque [`SyncCursor`](super::SyncCursor)
//! deliberately does not carry: the per-account mailbox catalog
//! (`imap_mailboxes`, each mailbox's UID counters) and the UID-to-message-id
//! location map (`imap_locations`). The cursor "stays small, holding only a
//! sync generation number"; everything heavier lives here.
//!
//! This is the seam only. No IMAP network protocol lives here, and no Gmail
//! code path reads or writes these tables, so Gmail's own sync state is
//! untouched. The IMAP provider (phase 2) is constructed with an
//! [`ImapStateStore`] onto the shared database and does its UID bookkeeping
//! through it.
//!
//! The tables themselves are created by schema migration v54; see
//! `crate::schema`.

// The IMAP provider that drives this seam lands in phase 2; until then the
// store type and its rows have no non-test caller. The allow is scoped to
// this module and removed with that first caller, matching how slice 1
// scoped the unused `ProviderCapabilities` fields.
#![allow(dead_code)]

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
    pub permanent_flags_json: String,
    pub permanent_keywords: bool,
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

    /// Inserts or replaces one mailbox's catalog row.
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
                     permanent_flags_json = excluded.permanent_flags_json,
                     permanent_keywords = excluded.permanent_keywords",
                rusqlite::params![
                    self.account_id,
                    mailbox.name,
                    mailbox.delimiter,
                    mailbox.special_use,
                    mailbox.uidvalidity,
                    mailbox.uidnext,
                    mailbox.highestmodseq,
                    mailbox.permanent_flags_json,
                    mailbox.permanent_keywords as i64,
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
                        permanent_keywords: row.get::<_, i64>(7)? != 0,
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
            permanent_flags_json: r#"["\\Seen","\\Flagged"]"#.into(),
            permanent_keywords: false,
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
}
