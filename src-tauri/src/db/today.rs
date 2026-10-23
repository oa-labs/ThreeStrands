//! Unread mail for the Today view, with the signals its grouping reads.

use super::threads::{thread_from_row, THREAD_COLUMNS};
use super::{normalize_sender, sending_addresses_for_account, Database, DbResult};
use crate::models::{TodayMail, TodayMailPage};
use rusqlite::params;
use std::collections::{BTreeSet, HashMap};

/// Most unread conversations the Today view lists; the rest are only counted.
pub(crate) const MAX_TODAY_MAIL: usize = 200;

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
                "SELECT sender, recipients_json, unsubscribe_json IS NOT NULL FROM messages WHERE thread_id = ?1 ORDER BY sent_at DESC",
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
                let mut inbound: Option<(String, String, bool)> = None;
                let mut rows = newest_first.query([&thread.id])?;
                while let Some(row) = rows.next()? {
                    let sender: String = row.get(0)?;
                    if !mine.contains(&normalize_sender(&sender).0) {
                        inbound = Some((sender, row.get(1)?, row.get(2)?));
                        break;
                    }
                }
                drop(rows);
                let replied_here = thread.labels.iter().any(|label| label == "SENT");
                let item = match inbound {
                    Some((sender, recipients_json, list_unsubscribe)) => {
                        let recipients: Vec<String> = serde_json::from_str(&recipients_json).unwrap_or_default();
                        let addressed_to_me = recipients.iter().any(|recipient| mine.contains(&normalize_sender(recipient).0));
                        let corresponded = replied_here
                            || wrote_to.query_row([normalize_sender(&sender).0], |row| row.get::<_, bool>(0))?;
                        TodayMail { thread, sender: Some(sender), list_unsubscribe, addressed_to_me, corresponded }
                    }
                    None => TodayMail { thread, sender: None, list_unsubscribe: false, addressed_to_me: false, corresponded: replied_here },
                };
                listed.push(item);
            }
            Ok(TodayMailPage { threads: listed, total })
        })
    }
}

#[cfg(test)]
#[path = "tests/today.rs"]
mod tests;
