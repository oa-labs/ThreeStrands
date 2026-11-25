//! After a message is queued: suggest the one task it probably finishes, and
//! link the queued message to a task the user confirms.
//!
//! Nothing here finishes a task by itself. The matcher only proposes; the user's
//! click calls `link_task_to_outbox`, which sets the same `completesTaskId`
//! link **Write Email** sets, so the task finishes through the one path that
//! runs when the outbox row becomes `sent` (`finish_linked_tasks_on`).
use std::collections::BTreeSet;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

use super::daily_plan::plan_day_start;
use super::tasks::{is_active_status, select_sql, task_by_id, task_from_row};
use super::{normalize_sender, serialization_error, Database, DatabaseError, DbResult};
use crate::clock::unix_millis;
use crate::correspondence::{stored_addresses, Draft};
use crate::models::ThreadTask;

/// Shortest name, in characters, that may match a task title: a first name (or
/// a one-word full name) shorter than this is too likely to be a common word or
/// part of another one.
const MIN_NAME_MATCH_CHARS: usize = 3;
/// Length of the local day a due datetime is compared with. A day that changes
/// clocks is an hour off at the far edge only.
const LOCAL_DAY_HOURS: i64 = 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSuggestion {
    pub task_id: String,
    pub title: String,
}

/// Whether the message had already left when the link was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkedSend {
    /// The task was finished with the record of the sent message.
    Sent,
    /// The task finishes when the message leaves.
    Queued,
}

/// Terms by which a task title can name one recipient.
fn name_terms(email: &str, names: &[String]) -> Vec<String> {
    let mut terms = vec![email.to_lowercase()];
    for name in names {
        let name = name.trim().trim_matches('"').trim();
        if name.is_empty() || name.eq_ignore_ascii_case(email) {
            continue;
        }
        if name.chars().count() >= MIN_NAME_MATCH_CHARS {
            terms.push(name.to_lowercase());
        }
        let first = name
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|c: char| !c.is_alphanumeric());
        if first.chars().count() >= MIN_NAME_MATCH_CHARS {
            terms.push(first.to_lowercase());
        }
    }
    terms
}

/// Whether `needle` occurs in `haystack` not touching a letter or digit on
/// either side. Both are lowercase.
fn contains_whole_word(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, found)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + found.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Due on `date` or earlier, by the user's local day.
fn due_by_end_of_day(task: &ThreadTask, date: &str, day_end: DateTime<Utc>) -> bool {
    let Some(value) = task.due_value.as_deref() else {
        return false;
    };
    match task.due_kind.as_str() {
        "date" => NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok_and(|due| {
            NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok_and(|day| due <= day)
        }),
        "datetime" => {
            DateTime::parse_from_rfc3339(value).is_ok_and(|due| due.with_timezone(&Utc) < day_end)
        }
        _ => false,
    }
}

impl Database {
    /// The one open task the queued message `outbox_id` strongly matches, or
    /// `None` for no match or several. See the plan, Phase 3.
    pub fn suggest_task_for_outbox(
        &self,
        outbox_id: &str,
        date: &str,
        day_start: Option<&str>,
    ) -> DbResult<Option<TaskSuggestion>> {
        if !crate::notes::is_daily_date(date) {
            return Err(DatabaseError::invalid("Invalid date"));
        }
        let day_start = plan_day_start(date, day_start)?;
        let day_end = day_start + Duration::hours(LOCAL_DAY_HOURS);
        let payload: Option<String> = self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT payload FROM outbox_messages WHERE id=?1",
                [outbox_id],
                |row| row.get(0),
            )
            .optional()?)
        })?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let draft: Draft = serde_json::from_str(&payload).map_err(serialization_error)?;
        if draft.completes_task_id.is_some() || draft.follow_up_task_id.is_some() {
            return Ok(None);
        }
        let planned: BTreeSet<String> = self
            .daily_plan_from(date, Some(&day_start.to_rfc3339()))?
            .items
            .into_iter()
            .filter_map(|item| item.task_id)
            .collect();
        self.with_connection(|connection| {
            let mine = super::sending_addresses_for_account(connection, &draft.account);
            let own = |email: &str| {
                mine.contains(&normalize_sender(email).0)
                    || normalize_sender(draft.sender_address()).0 == normalize_sender(email).0
            };
            let mut recipients: Vec<(String, Vec<String>)> = Vec::new();
            for (name, address) in [&draft.to, &draft.cc]
                .into_iter()
                .flat_map(|list| stored_addresses(list))
            {
                let email = normalize_sender(&address).0;
                if email.is_empty() || own(&email) {
                    continue;
                }
                let index = recipients
                    .iter()
                    .position(|(known, _)| *known == email)
                    .unwrap_or_else(|| {
                        recipients.push((email.clone(), Vec::new()));
                        recipients.len() - 1
                    });
                if !name.is_empty() {
                    recipients[index].1.push(name);
                }
            }
            for (email, names) in &mut recipients {
                let saved: Option<String> = connection
                    .query_row(
                        "SELECT c.display_name FROM contact_addresses a JOIN contacts c ON c.id=a.contact_id
                         WHERE lower(a.email)=?1",
                        [&*email],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                names.extend(saved);
            }
            let terms: Vec<String> = recipients
                .iter()
                .flat_map(|(email, names)| name_terms(email, names))
                .collect();
            let mut recipient_threads = BTreeSet::new();
            if !recipients.is_empty() {
                let marks = vec!["?"; recipients.len()].join(",");
                let mut statement = connection.prepare(&format!(
                    "SELECT DISTINCT thread_id FROM contact_interactions WHERE email IN ({marks})"
                ))?;
                let rows = statement.query_map(
                    rusqlite::params_from_iter(recipients.iter().map(|(email, _)| email)),
                    |row| row.get::<_, String>(0),
                )?;
                for row in rows {
                    recipient_threads.insert(row?);
                }
            }
            let conversation: Option<String> = match draft.thread_id.as_deref() {
                Some(provider_thread) => connection
                    .query_row(
                        "SELECT id FROM threads WHERE account_id=?1 AND provider_thread_id=?2",
                        params![draft.account, provider_thread],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => None,
            };
            let mut statement = connection.prepare(&format!(
                "{} WHERE status IN ('open', 'in_progress')",
                select_sql()
            ))?;
            let mut found: Vec<ThreadTask> = Vec::new();
            for task in statement.query_map([], task_from_row)? {
                let task = task?;
                let reply = conversation.is_some() && task.thread_id == conversation;
                let relevant = || {
                    planned.contains(&task.id) || due_by_end_of_day(&task, date, day_end)
                };
                let related = || {
                    task.thread_id
                        .as_ref()
                        .is_some_and(|thread| recipient_threads.contains(thread))
                        || {
                            let title = task.title.to_lowercase();
                            terms.iter().any(|term| contains_whole_word(&title, term))
                        }
                };
                if reply || (relevant() && related()) {
                    found.push(task);
                }
            }
            Ok(match found.len() {
                1 => found.pop().map(|task| TaskSuggestion {
                    task_id: task.id,
                    title: task.title,
                }),
                _ => None,
            })
        })
    }

    /// Links the queued (or sent) message `outbox_id` to `task_id`, so the
    /// task finishes with it. One transaction on the one connection: a send
    /// that becomes `sent` in `change_outbox` is wholly before or wholly after
    /// this, so the link is either read by the finish hook or finished here.
    /// Only the stored `payload` changes; the raw message was built at queue
    /// time and is not touched.
    pub fn link_task_to_outbox(
        &self,
        outbox_id: &str,
        task_id: &str,
        at: i64,
    ) -> DbResult<LinkedSend> {
        self.with_transaction(|tx| Self::link_task_to_outbox_in(tx, outbox_id, task_id, at))
    }

    fn link_task_to_outbox_in(
        tx: &Transaction<'_>,
        outbox_id: &str,
        task_id: &str,
        at: i64,
    ) -> DbResult<LinkedSend> {
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT state, payload FROM outbox_messages WHERE id=?1",
                [outbox_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let unsent = || DatabaseError::invalid("That message wasn't sent");
        let Some((state, payload)) = row else {
            return Err(unsent());
        };
        let sent = state == "sent";
        if !sent
            && !matches!(
                state.as_str(),
                "undo_pending" | "ready" | "scheduled" | "sending" | "uncertain"
            )
        {
            return Err(unsent());
        }
        if !task_by_id(tx, task_id)?.is_some_and(|task| is_active_status(&task.status)) {
            return Err(DatabaseError::invalid("That task is no longer open"));
        }
        let mut draft: serde_json::Value =
            serde_json::from_str(&payload).map_err(serialization_error)?;
        let object = draft
            .as_object_mut()
            .ok_or_else(|| DatabaseError::corrupt("Queued message is unreadable"))?;
        if object.get("completesTaskId").is_some_and(|v| !v.is_null())
            || object.get("followUpTaskId").is_some_and(|v| !v.is_null())
        {
            return Err(DatabaseError::invalid(
                "That message is already linked to a task",
            ));
        }
        object.insert("completesTaskId".into(), task_id.into());
        tx.execute(
            "UPDATE outbox_messages SET payload=?2 WHERE id=?1",
            params![
                outbox_id,
                serde_json::to_string(&draft).map_err(serialization_error)?
            ],
        )?;
        if sent {
            Database::finish_linked_tasks_on(tx, outbox_id, at)?;
            Ok(LinkedSend::Sent)
        } else {
            Ok(LinkedSend::Queued)
        }
    }

    /// `link_task_to_outbox` at the current time.
    pub fn link_task_to_outbox_now(&self, outbox_id: &str, task_id: &str) -> DbResult<LinkedSend> {
        self.link_task_to_outbox(outbox_id, task_id, unix_millis())
    }
}

#[cfg(test)]
#[path = "tests/task_suggestions.rs"]
mod tests;
