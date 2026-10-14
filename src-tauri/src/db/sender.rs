//! A sender's history in one account, and a message's trust signals, for the
//! context panel's Sender and Trust sections.
//!
//! The contact index leaves automated mail out, so a newsletter or
//! notification sender has no contact history. These reads go to the stored
//! messages directly instead.

use super::messages::resolve_body;
use super::{normalize_sender, Database, DbResult};
use crate::mime::RawMessage;
use crate::models::{SenderHistory, SenderThread};
use crate::sender_trust::{message_trust, MessageTrust};
use rusqlite::{params, OptionalExtension};

/// Most conversations a sender history lists.
pub(crate) const MAX_SENDER_THREADS: usize = 50;
/// Arrival times returned for estimating a cadence, newest first.
const MAX_SENDER_ARRIVALS: usize = 24;

impl Database {
    /// Every stored message from `email` in `account_id`: how many arrived and
    /// how many are read, when the first came, the user's latest message to
    /// the address, the conversations still in the Inbox, and the newest
    /// conversations outside Trash.
    pub fn sender_history(&self, account_id: &str, email: &str, limit: usize) -> DbResult<SenderHistory> {
        let email = email.trim().to_ascii_lowercase();
        let empty = SenderHistory {
            received_count: 0,
            read_count: 0,
            first_at: None,
            last_sent_at: None,
            recent_received_at: Vec::new(),
            inbox_thread_ids: Vec::new(),
            threads: Vec::new(),
        };
        if !email.contains('@') {
            return Ok(empty);
        }
        let limit = limit.clamp(1, MAX_SENDER_THREADS);
        self.with_connection(|connection| {
            // `instr` narrows the scan; the exact address is compared below,
            // since `sender` holds the raw header ("Name <address>").
            let mut statement = connection.prepare(
                "SELECT m.thread_id, m.sender, m.sent_at, m.unread, t.subject, t.snippet, t.archived, t.trashed
                 FROM messages m JOIN threads t ON t.id = m.thread_id
                 WHERE t.account_id = ?1 AND instr(lower(m.sender), ?2) > 0
                 ORDER BY m.sent_at DESC, m.id",
            )?;
            let rows = statement.query_map(params![account_id, email], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, bool>(7)?,
                ))
            })?;
            let mut history = empty;
            for row in rows {
                let (thread_id, sender, sent_at, unread, subject, snippet, archived, trashed) = row?;
                if normalize_sender(&sender).0 != email {
                    continue;
                }
                history.received_count += 1;
                if !unread {
                    history.read_count += 1;
                }
                // Rows arrive newest first, so the last one seen is the first.
                history.first_at = Some(sent_at.clone());
                if history.recent_received_at.len() < MAX_SENDER_ARRIVALS {
                    history.recent_received_at.push(sent_at.clone());
                }
                if !archived && !trashed && !history.inbox_thread_ids.contains(&thread_id) {
                    history.inbox_thread_ids.push(thread_id.clone());
                }
                if !trashed && history.threads.len() < limit && !history.threads.iter().any(|item| item.thread_id == thread_id) {
                    history.threads.push(SenderThread { thread_id, subject, snippet, sent_at });
                }
            }
            history.last_sent_at = connection.query_row(
                "SELECT MAX(sent_at) FROM contact_interactions
                 WHERE direction = 'sent' AND email = ?1 AND account_id = lower(trim(?2))",
                params![email, account_id],
                |row| row.get(0),
            )?;
            Ok(history)
        })
    }

    /// Trust signals read from the message's stored headers, or None when
    /// its provider payload is not cached (or cannot be read).
    pub fn message_trust(&self, message_id: &str) -> DbResult<Option<MessageTrust>> {
        let stored = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT payload, payload_z FROM message_metadata WHERE id = ?1",
                    [message_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
                )
                .optional()?)
        })?;
        // Decoded after the connection is released; a payload that no longer
        // parses leaves the section out rather than failing the panel.
        Ok(stored
            .and_then(|(legacy, compressed)| resolve_body(1, legacy, compressed).ok())
            .and_then(|payload| serde_json::from_str::<RawMessage>(&payload).ok())
            .filter(|message| !message.payload.headers.is_empty())
            .map(|message| message_trust(&message)))
    }
}

#[cfg(test)]
#[path = "tests/sender.rs"]
mod tests;
