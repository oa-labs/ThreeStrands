//! A sender's history in one account, and a message's trust signals, for the
//! context panel's Sender and Trust sections.
//!
//! The contact index leaves automated mail out, so a newsletter or
//! notification sender has no contact history. These reads go to the stored
//! messages directly instead.

use super::messages::resolve_body;
use super::{normalize_sender, Database, DbResult};
use crate::mime::RawMessage;
use crate::models::{SenderFamiliarity, SenderHistory, SenderThread};
use crate::sender_trust::{message_trust, MessageTrust};
use rusqlite::{params, OptionalExtension};

/// Most conversations a sender history lists.
pub(crate) const MAX_SENDER_THREADS: usize = 50;
/// Arrival times returned for estimating a cadence, newest first.
const MAX_SENDER_ARRIVALS: usize = 24;
/// Other addresses a familiarity read lists under the sender's display name.
const MAX_NAME_MATCHES: usize = 3;
/// Domains the user corresponds with that a familiarity read returns.
pub(crate) const MAX_KNOWN_DOMAINS: usize = 5000;

/// A display name folded for comparison: lowercased, quotes dropped, and
/// whitespace collapsed. Names of one word, or shorter than five characters,
/// fold to None: "Jane" or "Admin" is shared by too many people to suggest
/// impersonation.
fn comparable_name(name: &str) -> Option<String> {
    let folded = name.replace(['"', '\''], " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    (folded.split(' ').count() >= 2 && folded.chars().count() >= 5).then_some(folded)
}

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

    /// How well the user knows `email`, the sender of `message_id`: what they
    /// sent it, whether its mail arrived before this message, its saved
    /// contact, other known addresses that go by `name`, and the domains the
    /// user corresponds with. Every account counts, since a person writes to
    /// one inbox from the same address they write to another.
    pub fn sender_familiarity(&self, message_id: &str, email: &str, name: &str) -> DbResult<SenderFamiliarity> {
        let email = email.trim().to_ascii_lowercase();
        let name = comparable_name(name);
        let delivery_patterns = self.sender_delivery_patterns(message_id)?;
        self.with_connection(|connection| {
            let mut familiarity = SenderFamiliarity { delivery_patterns, ..SenderFamiliarity::default() };
            if email.contains('@') {
                (familiarity.sent_count, familiarity.first_sent_at) = connection.query_row(
                    "SELECT COUNT(DISTINCT message_id), MIN(sent_at) FROM contact_interactions WHERE direction = 'sent' AND email = ?1",
                    [&email],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                familiarity.contact_name = connection.query_row(
                    "SELECT c.display_name FROM contact_addresses a JOIN contacts c ON c.id = a.contact_id WHERE a.email = ?1",
                    [&email],
                    |row| row.get::<_, Option<String>>(0),
                ).optional()?.flatten();
                // Any stored message counts, automated mail included, so the
                // raw From header is compared exactly after `instr` narrows it.
                let sent_at: Option<String> = connection.query_row("SELECT sent_at FROM messages WHERE id = ?1", [message_id], |row| row.get(0)).optional()?;
                if let Some(sent_at) = sent_at {
                    let mut statement = connection.prepare(
                        "SELECT sender FROM messages WHERE id <> ?1 AND sent_at < ?2 AND instr(lower(sender), ?3) > 0",
                    )?;
                    let mut rows = statement.query(params![message_id, sent_at, email])?;
                    while let Some(row) = rows.next()? {
                        if normalize_sender(&row.get::<_, String>(0)?).0 == email {
                            familiarity.received_before = true;
                            break;
                        }
                    }
                }
            }
            if let Some(name) = name {
                let mut statement = connection.prepare(
                    "SELECT email FROM (
                         SELECT a.email AS email FROM contact_addresses a JOIN contacts c ON c.id = a.contact_id WHERE lower(trim(c.display_name)) = ?1
                         UNION
                         SELECT email FROM contact_interactions WHERE direction = 'sent' AND lower(trim(display_name)) = ?1
                     ) WHERE email <> ?2 ORDER BY email LIMIT ?3",
                )?;
                familiarity.name_matches = statement
                    .query_map(params![name, email, MAX_NAME_MATCHES as i64], |row| row.get(0))?
                    .collect::<Result<_, _>>()?;
            }
            let mut statement = connection.prepare(
                "SELECT DISTINCT lower(substr(email, instr(email, '@') + 1)) AS domain FROM (
                     SELECT email FROM contact_interactions WHERE direction = 'sent'
                     UNION SELECT email FROM contact_addresses
                 ) WHERE instr(email, '@') > 0 ORDER BY domain LIMIT ?1",
            )?;
            familiarity.known_domains = statement
                .query_map([MAX_KNOWN_DOMAINS as i64], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            Ok(familiarity)
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
