//! Account-scoped identity reservations. These may commit before acquisition:
//! like the body cache, they are reusable after an interrupted round, but never
//! advance the sync cursor, create a location, or imply a delivered message.

use std::collections::BTreeSet;

use rusqlite::{params, OptionalExtension};

use super::ImapStateStore;
use crate::db::{DatabaseError, DbResult};
use crate::provider::imap::{
    identity::{external_message_id, header_identity, IdentityInputs},
    policy,
};

const HEADER_NAMESPACE: &str = "header-hash";

impl ImapStateStore {
    /// Resolve and reserve identity in one transaction for every ingest tier.
    /// Header evidence is many-to-many and is never used to bind a newly seen
    /// authoritative server ID to an existing message. Exact locations can teach
    /// new namespaces; server mappings then survive moves and UIDVALIDITY resets.
    pub(crate) fn resolve_message_identity(
        &self,
        mailbox: &str,
        uidvalidity: i64,
        uid: i64,
        inputs: &IdentityInputs,
    ) -> DbResult<String> {
        let server_ids = inputs.server_identities();
        for (_, token) in &server_ids {
            if token.len() > policy::MAX_EXTERNAL_ID_BYTES
                || token
                    .chars()
                    .any(|ch| ch.is_control() || ch.is_whitespace())
            {
                return Err(DatabaseError::invalid(
                    "Invalid IMAP external message identity",
                ));
            }
        }
        let header_id = header_identity(&self.account_id, inputs);
        // A conflict is a committed result so contradictory evidence survives a
        // restart. Database/SQL failures still roll the entire transaction back.
        let resolved = self.database.with_transaction(|tx| {
            let sticky: Option<String> = tx.query_row(
                "SELECT message_id FROM imap_locations
                 WHERE account_id=?1 AND mailbox=?2 AND uidvalidity=?3 AND uid=?4",
                params![self.account_id, mailbox, uidvalidity, uid], |row| row.get(0),
            ).optional()?;
            if sticky.is_none() && server_ids.is_empty() {
                let ambiguous: bool = tx.query_row("SELECT
                    (SELECT COUNT(DISTINCT message_id) FROM imap_message_identities
                        WHERE account_id=?1 AND namespace=?2 AND external_id=?3) > 1
                    OR EXISTS(SELECT 1 FROM imap_message_identities
                        WHERE account_id=?1 AND namespace=?2 AND external_id=?3 AND message_id<>?3)
                    OR EXISTS(SELECT 1 FROM imap_message_identities
                        WHERE account_id=?1 AND message_id=?3 AND namespace<>?2)",
                    params![self.account_id, HEADER_NAMESPACE, header_id], |row| row.get(0))?;
                if ambiguous {
                    return Ok(Err("IMAP message identity is ambiguous without a server ID; message acquisition was stopped".to_string()));
                }
            }
            let mut candidates = BTreeSet::<String>::new();
            for (namespace, token) in &server_ids {
                let mut query = tx.prepare("SELECT message_id FROM imap_message_identities
                    WHERE account_id=?1 AND namespace=?2 AND external_id=?3")?;
                for value in query.query_map(params![self.account_id, namespace, token], |row| row.get(0))? {
                    candidates.insert(value?);
                }
            }
            if let Some(id) = &sticky {
                candidates.insert(id.clone());
            }
            if candidates.len() > 1 {
                // Preserve all conflicting candidates for each observed strong key.
                // No caller receives a guessed ID, and no existing row is replaced.
                for (namespace, token) in &server_ids {
                    for candidate in &candidates {
                        tx.execute("INSERT OR IGNORE INTO imap_message_identities VALUES (?1,?2,?3,?4)",
                            params![self.account_id, namespace, token, candidate])?;
                    }
                }
                return Ok(Err("Conflicting IMAP message identities; message acquisition was stopped".to_string()));
            }
            let id = match candidates.into_iter().next() {
                Some(id) => id,
                None if server_ids.is_empty() => header_id.clone(),
                // New server-identified messages use deterministic IDs on every
                // fresh device, independent of which duplicate header arrives
                // first. Existing exact locations still keep their recorded ID.
                None => external_message_id(&self.account_id, server_ids[0].0, &server_ids[0].1),
            };
            let mut conflict = false;
            for (namespace, token) in &server_ids {
                let different: bool = tx.query_row("SELECT EXISTS(
                    SELECT 1 FROM imap_message_identities
                    WHERE account_id=?1 AND message_id=?2 AND namespace=?3 AND external_id<>?4)",
                    params![self.account_id, id, namespace, token], |row| row.get(0))?;
                conflict |= different;
                tx.execute("INSERT OR IGNORE INTO imap_message_identities VALUES (?1,?2,?3,?4)",
                    params![self.account_id, namespace, token, id])?;
            }
            tx.execute("INSERT OR IGNORE INTO imap_message_identities VALUES (?1,?2,?3,?4)",
                params![self.account_id, HEADER_NAMESPACE, header_id, id])?;
            if conflict {
                Ok(Err("Conflicting IMAP server identities for a known message; message acquisition was stopped".to_string()))
            } else {
                Ok(Ok(id))
            }
        })?;
        resolved.map_err(DatabaseError::invalid)
    }
}

#[cfg(test)]
mod tests;
