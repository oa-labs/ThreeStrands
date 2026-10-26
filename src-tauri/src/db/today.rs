//! Unread mail for the Today view, with the signals its grouping reads, and
//! the promises in the user's sent mail still waiting on them.

use super::messages::resolve_body;
use super::threads::{thread_from_row, THREAD_COLUMNS};
use super::{normalize_sender, sending_addresses_for_account, Database, DbResult};
use crate::mime::readable_body_text;
use crate::models::{MessageAttachment, OpenPromise, TodayMail, TodayMailPage};
use crate::quoted_history::ThreadHistory;
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeSet, HashMap, HashSet};
use threestrands_mail_text::promise_text::promise_sentence;

/// Most unread conversations the Today view lists; the rest are only counted.
pub(crate) const MAX_TODAY_MAIL: usize = 200;

/// How far back Today looks for promises in the user's sent mail.
pub(crate) const PROMISE_LOOKBACK_DAYS: i64 = 14;

/// Most open promises Today lists, newest first.
pub(crate) const MAX_OPEN_PROMISES: usize = 20;

const UNREAD_FILTER: &str = "archived = 0 AND trashed = 0 AND unread = 1";

impl Database {
    /// Every unread conversation not archived or trashed, newest arrival
    /// first, split inbox matches included. `account_id` scopes to one
    /// account, or merges every account when absent.
    pub fn list_today_mail(&self, account_id: Option<&str>) -> DbResult<TodayMailPage> {
        self.with_connection(|connection| {
            let scope = if account_id.is_some() { "AND account_id = ?1" } else { "" };
            let total: i64 = match account_id {
                Some(id) => connection.query_row(&format!("SELECT COUNT(*) FROM threads WHERE {UNREAD_FILTER} {scope}"), [id], |row| row.get(0))?,
                None => connection.query_row(&format!("SELECT COUNT(*) FROM threads WHERE {UNREAD_FILTER}"), [], |row| row.get(0))?,
            };
            let sql = format!(
                "SELECT {THREAD_COLUMNS} FROM threads AS t WHERE {UNREAD_FILTER} {scope}
                 ORDER BY last_received_at DESC LIMIT ?{}",
                if account_id.is_some() { 2 } else { 1 },
            );
            let mut statement = connection.prepare(&sql)?;
            let threads = match account_id {
                Some(id) => statement.query_map(params![id, MAX_TODAY_MAIL as i64], thread_from_row)?.collect::<Result<Vec<_>, _>>()?,
                None => statement.query_map([MAX_TODAY_MAIL as i64], thread_from_row)?.collect::<Result<Vec<_>, _>>()?,
            };

            let mut newest_first = connection.prepare(
                "SELECT sender, recipients_json, unsubscribe_json IS NOT NULL, attachments_json FROM messages WHERE thread_id = ?1 ORDER BY sent_at DESC",
            )?;
            let mut wrote_to = connection.prepare(
                "SELECT EXISTS(SELECT 1 FROM contact_interactions WHERE direction = 'sent' AND email = ?1)",
            )?;
            // Each account's addresses are read once, not once per conversation.
            let mut identity_sets: HashMap<String, BTreeSet<String>> = HashMap::new();
            let mut listed = Vec::with_capacity(threads.len());
            for thread in threads {
                let mine = identity_sets
                    .entry(thread.account_id.clone())
                    .or_insert_with(|| sending_addresses_for_account(connection, &thread.account_id));
                let mut inbound: Option<(String, String, bool, String)> = None;
                let mut rows = newest_first.query([&thread.id])?;
                while let Some(row) = rows.next()? {
                    let sender: String = row.get(0)?;
                    if !mine.contains(&normalize_sender(&sender).0) {
                        inbound = Some((sender, row.get(1)?, row.get(2)?, row.get(3)?));
                        break;
                    }
                }
                drop(rows);
                let replied_here = thread.labels.iter().any(|label| label == "SENT");
                let item = match inbound {
                    Some((sender, recipients_json, list_unsubscribe, attachments_json)) => {
                        let recipients: Vec<String> = serde_json::from_str(&recipients_json).unwrap_or_default();
                        let attachments: Vec<MessageAttachment> = serde_json::from_str(&attachments_json).unwrap_or_default();
                        let has_invitation = attachments.iter().any(is_calendar_attachment);
                        let addressed_to_me = recipients.iter().any(|recipient| mine.contains(&normalize_sender(recipient).0));
                        let corresponded = replied_here
                            || wrote_to.query_row([normalize_sender(&sender).0], |row| row.get::<_, bool>(0))?;
                        TodayMail { thread, sender: Some(sender), list_unsubscribe, addressed_to_me, to_count: recipients.len(), has_invitation, corresponded }
                    }
                    None => TodayMail { thread, sender: None, list_unsubscribe: false, addressed_to_me: false, to_count: 0, has_invitation: false, corresponded: replied_here },
                };
                listed.push(item);
            }
            Ok(TodayMailPage { threads: listed, total })
        })
    }
}

impl Database {
    /// Promises the user made in mail they sent since `since` (RFC 3339) that
    /// still look open, newest first. Only each conversation's newest sent
    /// message is read, and only its own text, never the quoted history under
    /// it. A promise counts as kept, and is left out, once the user has sent
    /// anything later to someone on its To line, in any conversation. Trashed
    /// conversations and ones with an open task are left out too: the task
    /// already tracks it. `account_id` scopes to one account.
    pub fn list_open_promises(&self, account_id: Option<&str>, since: &str) -> DbResult<Vec<OpenPromise>> {
        self.with_connection(|connection| {
            let mut sent = connection.prepare(
                "SELECT m.id, m.thread_id, m.sender, m.recipients_json, m.sent_at, t.account_id
                 FROM messages AS m JOIN threads AS t ON t.id = m.thread_id
                 WHERE t.trashed = 0 AND m.sent_at >= ?1 AND (?2 IS NULL OR t.account_id = ?2)
                   AND NOT EXISTS (SELECT 1 FROM tasks WHERE tasks.thread_id = t.id AND tasks.status IN ('open', 'in_progress'))
                 ORDER BY m.sent_at DESC",
            )?;
            let mut body = connection.prepare("SELECT body_text, body_text_z FROM messages WHERE id = ?1")?;
            let mut sent_later = connection.prepare(
                "SELECT EXISTS(SELECT 1 FROM contact_interactions WHERE direction = 'sent' AND email = ?1 AND sent_at > ?2)",
            )?;
            let mut thread = connection.prepare(&format!("SELECT {THREAD_COLUMNS} FROM threads AS t WHERE t.id = ?1"))?;

            let mut identity_sets: HashMap<String, BTreeSet<String>> = HashMap::new();
            let mut seen_threads: HashSet<String> = HashSet::new();
            let mut promises = Vec::new();
            let mut rows = sent.query(params![since, account_id])?;
            while let Some(row) = rows.next()? {
                if promises.len() >= MAX_OPEN_PROMISES {
                    break;
                }
                let (message_id, thread_id, sender, recipients_json, sent_at, owner): (String, String, String, String, String, String) =
                    (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?);
                let mine = identity_sets
                    .entry(owner.clone())
                    .or_insert_with(|| sending_addresses_for_account(connection, &owner));
                // Rows run newest first, so the first one the user sent is the
                // conversation's newest sent message; older ones are not read.
                if !mine.contains(&normalize_sender(&sender).0) || !seen_threads.insert(thread_id.clone()) {
                    continue;
                }
                let recipients: Vec<String> = serde_json::from_str::<Vec<String>>(&recipients_json)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|recipient| {
                        let address = normalize_sender(recipient).0;
                        !address.is_empty() && !mine.contains(&address)
                    })
                    .collect();
                if recipients.is_empty() {
                    continue;
                }
                let mut kept = false;
                for recipient in &recipients {
                    if sent_later.query_row(params![normalize_sender(recipient).0, sent_at], |row| row.get::<_, bool>(0))? {
                        kept = true;
                        break;
                    }
                }
                if kept {
                    continue;
                }
                let text = body
                    .query_row([&message_id], |row| resolve_body(1, row.get(0)?, row.get(1)?))
                    .optional()?
                    .map(readable_body_text)
                    .unwrap_or_default();
                let own_text = &text[..ThreadHistory::default().preview_text_len(&text)];
                let Some(excerpt) = promise_sentence(own_text) else { continue };
                let Some(thread) = thread.query_row([&thread_id], thread_from_row).optional()? else { continue };
                promises.push(OpenPromise { thread, message_id, sent_at, recipients, excerpt });
            }
            Ok(promises)
        })
    }
}

/// The same test as Files and the reader's invitation card: a text/calendar part or an .ics file.
fn is_calendar_attachment(attachment: &MessageAttachment) -> bool {
    attachment.mime_type.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/calendar")
        || attachment.filename.to_ascii_lowercase().ends_with(".ics")
}

#[cfg(test)]
#[path = "tests/today.rs"]
mod tests;
