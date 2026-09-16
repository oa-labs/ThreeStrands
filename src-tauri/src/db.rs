use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::mime::{GmailMessage, NormalizedMessage, UnsubscribeMetadata};
use crate::models::{
    Account, ContactSuggestion, MailboxUnreadCounts, Message, SearchThreadsRequest, SplitInbox,
    SyncStatus, Thread, ThreadDetail, ThreadMutation, ThreadPage, TriageAction, TriageContext,
    TriageEvent, TriageEventKind, TriageSenderStats, UnsubscribeMethod, UnsubscribeTarget,
};
use crate::transfer::{TransferAccount, TransferSplitInbox};

mod accounts;
mod contacts;
mod split_inboxes;
mod threads;
mod triage;

/// Assigned to newly connected accounts in rotation, so each has a distinct
/// color for switcher/thread-row indicators without asking the user to pick
/// one up front.
const ACCOUNT_COLORS: [&str; 8] = [
    "#4285F4", "#34A853", "#EA4335", "#FBBC05", "#9C27B0", "#00ACC1", "#FF7043", "#5C6BC0",
];

#[derive(Debug, Clone)]
pub struct PendingMutation {
    pub id: String,
    pub provider_thread_id: String,
    pub mutation: ThreadMutation,
}

/// `threads.id`, derived from the pair that's actually unique: Gmail thread
/// IDs are unique only within one account, not across two different
/// accounts, so the bare provider ID can't be used as the local primary key
/// once more than one account is connected.
fn local_thread_id(account_id: &str, provider_thread_id: &str) -> String {
    format!("{account_id}:{provider_thread_id}")
}

/// Best-effort: narrow the database file to owner-only access. Not fatal if
/// it fails (e.g. an unsupported filesystem) since the containing directory
/// is already locked down by the caller.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) {}

pub struct Database(Mutex<Connection>);

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut connection = Connection::open(path).map_err(display_error)?;
        restrict_to_owner(path);
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .map_err(display_error)?;
        connection
            .execute(
                "UPDATE mutations SET state = 'pending', last_error = 'Interrupted before acknowledgement'
                 WHERE state = 'running'",
                [],
            )
            .map_err(display_error)?;
        crate::schema::migrate(&mut connection)?;
        ensure_query_indexes(&connection).map_err(display_error)?;
        seed_if_empty(&connection).map_err(display_error)?;
        Ok(Self(Mutex::new(connection)))
    }

    #[cfg(test)]
    pub(crate) fn open_memory() -> Self {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        crate::schema::migrate(&mut connection).unwrap();
        ensure_query_indexes(&connection).unwrap();
        seed_if_empty(&connection).unwrap();
        Self(Mutex::new(connection))
    }

    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.0
            .lock()
            .map_err(|_| "Local database lock was poisoned".to_string())
    }

    /// The Inbox is unarchived/untrashed threads *minus* anything claimed by
    /// a split inbox rule — a split inbox is meant to pull its matches out
    /// of the Inbox, not just mirror them into a second view. Split inbox
    /// rule sets are realistically small, so filtering the base set in
    /// memory (rather than the SQL-level pagination `list_threads_page_where`
    /// uses) is simpler than pushing the rule matching into SQL.
    pub fn list_threads_page(
        &self,
        account_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ThreadPage, String> {
        let rules = self.list_split_inboxes()?;
        if rules.is_empty() {
            return self.list_threads_page_where(account_id, "archived = 0 AND trashed = 0", offset, limit);
        }
        let matched: Vec<Thread> = self
            .list_threads(account_id)?
            .into_iter()
            .filter(|thread| !rules.iter().any(|rule| split_inbox_matches(rule, thread)))
            .collect();
        let page_limit = limit.min(200);
        let has_more = matched.len() > offset.saturating_add(page_limit);
        let threads = matched.into_iter().skip(offset).take(page_limit).collect();
        Ok(ThreadPage { threads, has_more })
    }

    pub fn list_all_mail_page(
        &self,
        account_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ThreadPage, String> {
        self.list_threads_page_where(account_id, "trashed = 0", offset, limit)
    }

    pub fn list_trash_page(
        &self,
        account_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ThreadPage, String> {
        self.list_threads_page_where(account_id, "trashed = 1", offset, limit)
    }

    fn list_threads_where(
        &self,
        account_id: Option<&str>,
        filter: &str,
    ) -> Result<Vec<Thread>, String> {
        let connection = self.connection()?;
        let sql = format!(
            "SELECT id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed, account_id,
                    summary, summary_generated_at, has_attachments, last_received_at
             FROM threads
             WHERE {filter} {}
             ORDER BY last_received_at DESC",
            if account_id.is_some() {
                "AND account_id = ?1"
            } else {
                ""
            }
        );
        let mut statement = connection.prepare(&sql).map_err(display_error)?;
        let rows = match account_id {
            Some(id) => statement
                .query_map([id], thread_from_row)
                .map_err(display_error)?,
            None => statement
                .query_map([], thread_from_row)
                .map_err(display_error)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    fn list_threads_page_where(
        &self,
        account_id: Option<&str>,
        filter: &str,
        offset: usize,
        limit: usize,
    ) -> Result<ThreadPage, String> {
        let connection = self.connection()?;
        let page_limit = limit.min(200);
        let fetch_limit = page_limit.saturating_add(1) as i64;
        let sql = format!(
            "SELECT id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed, account_id,
                    summary, summary_generated_at, has_attachments, last_received_at
             FROM threads
             WHERE {filter} {}
             ORDER BY last_received_at DESC
             LIMIT ?{} OFFSET ?{}",
            if account_id.is_some() {
                "AND account_id = ?1"
            } else {
                ""
            },
            if account_id.is_some() { 2 } else { 1 },
            if account_id.is_some() { 3 } else { 2 },
        );
        let mut statement = connection.prepare(&sql).map_err(display_error)?;
        let rows = match account_id {
            Some(id) => statement
                .query_map(params![id, fetch_limit, offset as i64], thread_from_row)
                .map_err(display_error)?,
            None => statement
                .query_map(params![fetch_limit, offset as i64], thread_from_row)
                .map_err(display_error)?,
        };
        let mut threads = rows.collect::<Result<Vec<_>, _>>().map_err(display_error)?;
        let has_more = threads.len() > page_limit;
        threads.truncate(page_limit);
        Ok(ThreadPage { threads, has_more })
    }

    pub fn get_thread(&self, id: &str) -> Result<ThreadDetail, String> {
        let connection = self.connection()?;
        let thread = connection
            .query_row(
                "SELECT id, provider_thread_id, subject, snippet, participants_json,
                        last_message_at, unread, starred, archived, labels_json, trashed, account_id,
                        summary, summary_generated_at, has_attachments, last_received_at
                 FROM threads WHERE id = ?1",
                [id],
                thread_from_row,
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Thread not found".to_string())?;

        let mut statement = connection
            .prepare(
                "SELECT id, thread_id, sender, recipients_json, sent_at, body_html, body_text,
                        body_html_z, body_text_z, unsubscribe_json, unread, attachments_json
                 FROM messages WHERE thread_id = ?1 ORDER BY sent_at",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([id], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    sender: row.get(2)?,
                    recipients: decode_json(row.get::<_, String>(3)?)?,
                    sent_at: row.get(4)?,
                    body_html: resolve_body(7, row.get(5)?, row.get(7)?)?,
                    body_text: resolve_body(8, row.get(6)?, row.get(8)?)?,
                    unsubscribe: row
                        .get::<_, Option<String>>(9)?
                        .and_then(|value| serde_json::from_str::<UnsubscribeMetadata>(&value).ok())
                        .map(|value| value.info()),
                    unread: row.get::<_, i64>(10)? != 0,
                    attachments: serde_json::from_str(&row.get::<_, String>(11)?).map_err(
                        |error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                11,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        },
                    )?,
                })
            })
            .map_err(display_error)?;
        let messages = rows.collect::<Result<Vec<_>, _>>().map_err(display_error)?;
        Ok(ThreadDetail { thread, messages })
    }

    pub fn get_thread_for_message(&self, message_id: &str) -> Result<ThreadDetail, String> {
        let thread_id = self
            .connection()?
            .query_row(
                "SELECT thread_id FROM messages WHERE id = ?1",
                [message_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Reply source message not found".to_string())?;
        self.get_thread(&thread_id)
    }

    pub fn attachment_message(&self, message_id: &str) -> Result<(String, GmailMessage), String> {
        self.connection()?
            .query_row(
                "SELECT t.account_id, mm.payload
                 FROM messages m
                 JOIN threads t ON t.id = m.thread_id
                 JOIN message_metadata mm ON mm.id = m.id
                 WHERE m.id = ?1",
                [message_id],
                |row| {
                    let account_id: String = row.get(0)?;
                    let payload: String = row.get(1)?;
                    let message = serde_json::from_str(&payload).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            payload.len(),
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok((account_id, message))
                },
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Attachment source not found".to_string())
    }

    /// Returns the current top sender candidates for one account. This is a
    /// derived view over raw events: the limit is intentionally bounded for a
    /// future UI, while the underlying observations remain available locally.
    pub fn list_triage_sender_stats(
        &self,
        account_id: &str,
        limit: usize,
    ) -> Result<Vec<TriageSenderStats>, String> {
        let connection = self.connection()?;
        let limit = limit.clamp(1, 100) as i64;
        let mut statement = connection
            .prepare(
                "WITH stats AS (
                    SELECT
                        account_id,
                        sender_email,
                        sender_domain,
                        SUM(CASE WHEN event_kind = 'open' AND context = 'inbox'
                                 THEN 1 ELSE 0 END) AS exposure_count,
                        SUM(CASE WHEN event_kind = 'close' AND context = 'inbox'
                                      AND (scrolled = 1 OR COALESCE(dwell_ms, 0) > 1000)
                                 THEN 1 ELSE 0 END) AS engaged_view_count,
                        SUM(CASE WHEN event_kind = 'disposition' AND context = 'inbox'
                                 THEN 1 ELSE 0 END) AS disposition_count,
                        SUM(CASE WHEN event_kind = 'disposition' AND context = 'inbox'
                                      AND action = 'archive'
                                 THEN 1 ELSE 0 END) AS archive_count,
                        SUM(CASE WHEN event_kind = 'disposition' AND context = 'inbox'
                                      AND action = 'trash'
                                 THEN 1 ELSE 0 END) AS trash_count,
                        SUM(CASE WHEN event_kind = 'disposition' AND context = 'inbox'
                                      AND action IN ('archive', 'trash')
                                      AND opened = 1 AND batch = 0 AND scrolled = 0
                                      AND dwell_ms BETWEEN 0 AND 1000
                                 THEN 1 ELSE 0 END) AS quick_disposition_count,
                        SUM(CASE WHEN event_kind = 'disposition' AND context = 'inbox'
                                      AND batch = 1
                                 THEN 1 ELSE 0 END) AS batch_disposition_count,
                        SUM(CASE WHEN event_kind = 'restore' AND context = 'inbox'
                                 THEN 1 ELSE 0 END) AS restore_count,
                        SUM(CASE WHEN event_kind = 'response' AND context = 'inbox'
                                 THEN 1 ELSE 0 END) AS response_count,
                        MAX(created_at) AS last_seen_at
                    FROM triage_events
                    WHERE account_id = ?1
                    GROUP BY account_id, sender_email, sender_domain
                )
                SELECT account_id, sender_email, sender_domain,
                       exposure_count, engaged_view_count, disposition_count,
                       archive_count, trash_count, quick_disposition_count,
                       batch_disposition_count, restore_count, response_count, last_seen_at
                FROM stats
                WHERE exposure_count > 0 OR disposition_count > 0
                ORDER BY quick_disposition_count DESC,
                         CASE WHEN disposition_count > 0
                              THEN CAST(quick_disposition_count AS REAL) / disposition_count
                              ELSE 0 END DESC,
                         disposition_count DESC,
                         last_seen_at DESC
                LIMIT ?2",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map(params![account_id, limit], |row| {
                let exposure_count: i64 = row.get(3)?;
                let disposition_count: i64 = row.get(5)?;
                let quick_disposition_count: i64 = row.get(8)?;
                Ok(TriageSenderStats {
                    account_id: row.get(0)?,
                    sender_email: row.get(1)?,
                    sender_domain: row.get(2)?,
                    exposure_count,
                    engaged_view_count: row.get(4)?,
                    disposition_count,
                    archive_count: row.get(6)?,
                    trash_count: row.get(7)?,
                    quick_disposition_count,
                    batch_disposition_count: row.get(9)?,
                    restore_count: row.get(10)?,
                    response_count: row.get(11)?,
                    quick_disposition_rate: if disposition_count > 0 {
                        quick_disposition_count as f64 / disposition_count as f64
                    } else {
                        0.0
                    },
                    last_seen_at: row.get(12)?,
                })
            })
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    /// Ranks past correspondents for compose autocomplete. Deliberately
    /// doesn't import Google's address book: every suggestion is mined from
    /// this account's own cached `messages` (who it sent to, who it heard
    /// from), so results are inherently people the user has actually
    /// corresponded with, plus anything explicitly pinned. A sender is
    /// excluded from the "heard from" side when its message carries
    /// unsubscribe metadata (List-Unsubscribe/one-click) — that marks
    /// bulk/automated mail, not a real correspondent — unless the account
    /// also sent that address mail directly or pinned it. Bounded to the
    /// most recent messages so a large mailbox can't make every keystroke
    /// re-parse years of history.
    pub fn list_contact_suggestions(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ContactSuggestion>, String> {
        let limit = limit.clamp(1, 50);
        let connection = self.connection()?;
        let account_email = account_id.to_ascii_lowercase();

        struct Agg {
            display_name: Option<String>,
            sent_count: i64,
            received_count: i64,
            last_interacted_at: String,
            pinned: bool,
        }
        let mut by_email: HashMap<String, Agg> = HashMap::new();

        let mut statement = connection
            .prepare(
                "SELECT m.sender, m.recipients_json, m.sent_at, m.unsubscribe_json
                 FROM messages m JOIN threads t ON t.id = m.thread_id
                 WHERE t.account_id = ?1
                 ORDER BY m.sent_at DESC
                 LIMIT 20000",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map(params![account_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(display_error)?;

        for row in rows {
            let (sender, recipients_json, sent_at, unsubscribe_json) =
                row.map_err(display_error)?;
            // Best-effort: a message with an address this parser rejects
            // just contributes nothing to suggestions rather than failing
            // the whole ranking.
            let Some((sender_name, sender_email)) = crate::correspondence::addresses(&sender)
                .ok()
                .and_then(|parsed| parsed.into_iter().next())
            else {
                continue;
            };
            let sender_email = sender_email.to_ascii_lowercase();
            if sender_email == account_email {
                let Ok(recipients) = serde_json::from_str::<Vec<String>>(&recipients_json) else {
                    continue;
                };
                for raw in recipients {
                    let Some((name, email)) = crate::correspondence::addresses(&raw)
                        .ok()
                        .and_then(|parsed| parsed.into_iter().next())
                    else {
                        continue;
                    };
                    let email = email.to_ascii_lowercase();
                    if email.is_empty() || email == account_email {
                        continue;
                    }
                    let entry = by_email.entry(email).or_insert_with(|| Agg {
                        display_name: None,
                        sent_count: 0,
                        received_count: 0,
                        last_interacted_at: sent_at.clone(),
                        pinned: false,
                    });
                    entry.sent_count += 1;
                    if entry.display_name.is_none() && !name.is_empty() {
                        entry.display_name = Some(name);
                    }
                    if sent_at > entry.last_interacted_at {
                        entry.last_interacted_at = sent_at.clone();
                    }
                }
            } else if !sender_email.is_empty() && unsubscribe_json.is_none() {
                // A List-Unsubscribe/one-click header marks bulk/automated
                // mail (newsletters, notifications) — never a real
                // correspondent, so it must not seed or bump a suggestion.
                // Doesn't affect an entry already earned by being sent to,
                // or a manually pinned contact.
                let entry = by_email.entry(sender_email).or_insert_with(|| Agg {
                    display_name: None,
                    sent_count: 0,
                    received_count: 0,
                    last_interacted_at: sent_at.clone(),
                    pinned: false,
                });
                entry.received_count += 1;
                if entry.display_name.is_none() && !sender_name.is_empty() {
                    entry.display_name = Some(sender_name);
                }
                if sent_at > entry.last_interacted_at {
                    entry.last_interacted_at = sent_at.clone();
                }
            }
        }

        let mut pinned_statement = connection
            .prepare(
                "SELECT email, display_name, pinned_at FROM pinned_contacts WHERE account_id = ?1",
            )
            .map_err(display_error)?;
        let pinned_rows = pinned_statement
            .query_map(params![account_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(display_error)?;
        for row in pinned_rows {
            let (email, display_name, pinned_at) = row.map_err(display_error)?;
            let entry = by_email.entry(email).or_insert_with(|| Agg {
                display_name: display_name.clone(),
                sent_count: 0,
                received_count: 0,
                last_interacted_at: pinned_at,
                pinned: false,
            });
            entry.pinned = true;
            if entry.display_name.is_none() {
                entry.display_name = display_name;
            }
        }

        let needle = query.trim().to_ascii_lowercase();
        let mut suggestions: Vec<ContactSuggestion> = by_email
            .into_iter()
            .filter(|(email, agg)| {
                let domain_matches = email
                    .split_once('@')
                    .is_some_and(|(_, domain)| domain.contains(&needle));
                needle.is_empty()
                    || email.starts_with(&needle)
                    || domain_matches
                    || agg
                        .display_name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains(&needle))
            })
            .map(|(email, agg)| ContactSuggestion {
                email,
                display_name: agg.display_name,
                sent_count: agg.sent_count,
                received_count: agg.received_count,
                last_interacted_at: agg.last_interacted_at,
                pinned: agg.pinned,
            })
            .collect();
        suggestions.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.sent_count.cmp(&a.sent_count))
                .then(b.received_count.cmp(&a.received_count))
                .then(b.last_interacted_at.cmp(&a.last_interacted_at))
        });
        suggestions.truncate(limit);
        Ok(suggestions)
    }

    pub fn set_thread_summary(
        &self,
        thread_id: &str,
        summary: &str,
        generated_at: &str,
    ) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE threads SET summary = ?1, summary_generated_at = ?2 WHERE id = ?3",
                params![summary, generated_at, thread_id],
            )
            .map_err(display_error)?;
        Ok(())
    }

    /// Resolves the unsubscribe URL from locally cached message metadata and
    /// records the attempt before any external side effect occurs. The
    /// webview supplies only the stable message ID, never an arbitrary URL.
    pub fn begin_unsubscribe(&self, message_id: &str) -> Result<UnsubscribeTarget, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let (thread_id, metadata_json): (String, Option<String>) = transaction
            .query_row(
                "SELECT thread_id, unsubscribe_json FROM messages WHERE id = ?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Message not found".to_string())?;
        let metadata = metadata_json
            .ok_or_else(|| "This message has no unsubscribe option".to_string())
            .and_then(|value| {
                serde_json::from_str::<UnsubscribeMetadata>(&value).map_err(display_error)
            })?;
        let (method, url) = if let Some(url) = metadata.one_click_url {
            (UnsubscribeMethod::OneClick, url)
        } else if let Some(url) = metadata.mailto_url {
            (UnsubscribeMethod::Mailto, url)
        } else if let Some(url) = metadata.web_url {
            (UnsubscribeMethod::Web, url)
        } else {
            return Err("This message has no usable unsubscribe option".to_string());
        };
        let request_id = Uuid::new_v4().to_string();
        transaction
            .execute(
                "INSERT INTO unsubscribe_requests(
                    id, message_id, thread_id, method, state, created_at
                 ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params![
                    request_id,
                    message_id,
                    thread_id,
                    unsubscribe_method_name(&method),
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)?;
        Ok(UnsubscribeTarget {
            request_id,
            method,
            url,
        })
    }

    pub fn finish_unsubscribe(
        &self,
        request_id: &str,
        state: &str,
        http_status: Option<u16>,
        error: Option<&str>,
    ) -> Result<(), String> {
        if !matches!(state, "succeeded" | "opened" | "failed") {
            return Err("Invalid unsubscribe request state".to_string());
        }
        let changed = self
            .connection()?
            .execute(
                "UPDATE unsubscribe_requests
                 SET state = ?1, http_status = ?2, completed_at = ?3, last_error = ?4
                 WHERE id = ?5 AND state = 'pending'",
                params![
                    state,
                    http_status,
                    Utc::now().to_rfc3339(),
                    error,
                    request_id
                ],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Unsubscribe request was not pending".to_string());
        }
        Ok(())
    }

    pub fn message_ids_for_thread(&self, thread_id: &str) -> Result<Vec<String>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id FROM messages WHERE thread_id = ?1 ORDER BY sent_at")
            .map_err(display_error)?;
        let ids = statement
            .query_map([thread_id], |row| row.get(0))
            .map_err(display_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display_error)?;
        Ok(ids)
    }

    /// `account_id` merges every account when `None` — the unified inbox —
    /// or scopes to just that account when set, same as [`Self::list_threads`].
    pub fn search_threads(
        &self,
        request: &SearchThreadsRequest,
        account_id: Option<&str>,
    ) -> Result<Vec<Thread>, String> {
        if request.query.trim().is_empty() {
            return self.list_threads(account_id);
        }
        let connection = self.connection()?;
        let limit = request.limit.unwrap_or(50).min(200) as i64;
        let offset = request.offset.unwrap_or(0) as i64;
        let query = fts_query(&request.query);
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // Trashed threads are hidden alongside archived ones by default; the
        // same "include archived" search toggle reveals both, since neither
        // belongs in the everyday inbox view.
        let archived_filter = if request.include_archived.unwrap_or(false) {
            ""
        } else {
            "AND t.archived = 0 AND t.trashed = 0"
        };
        let account_filter = if account_id.is_some() {
            "AND t.account_id = ?4"
        } else {
            ""
        };
        // -1 asks FTS5 to excerpt whichever column has the most matches, so a
        // hit on the body or a recipient still produces a relevant snippet.
        // The match itself is wrapped in \u{1}/\u{2} rather than HTML markup
        // so the frontend can highlight it without ever parsing untrusted HTML.
        let sql = format!(
            "SELECT t.id, t.provider_thread_id, t.subject, t.snippet,
                    t.participants_json, t.last_message_at, t.unread, t.starred,
                    t.archived, t.labels_json, t.trashed, t.account_id,
                    t.summary, t.summary_generated_at, t.has_attachments, t.last_received_at,
                    snippet(thread_search, -1, '\u{1}', '\u{2}', '…', 12) AS match_snippet
             FROM thread_search s
             JOIN threads t ON t.id = s.thread_id
             WHERE thread_search MATCH ?1 {archived_filter} {account_filter}
             ORDER BY rank, t.last_received_at DESC
             LIMIT ?2 OFFSET ?3"
        );
        let mut statement = connection.prepare(&sql).map_err(display_error)?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(Thread {
                id: row.get(0)?,
                provider_thread_id: row.get(1)?,
                subject: row.get(2)?,
                snippet: row.get(3)?,
                participants: decode_json(row.get::<_, String>(4)?)?,
                last_message_at: row.get(5)?,
                unread: row.get(6)?,
                starred: row.get(7)?,
                archived: row.get(8)?,
                labels: decode_json(row.get::<_, String>(9)?)?,
                trashed: row.get(10)?,
                account_id: row.get(11)?,
                summary: row.get(12)?,
                summary_generated_at: row.get(13)?,
                has_attachments: row.get(14)?,
                last_received_at: row.get(15)?,
                match_snippet: row.get(16)?,
            })
        };
        let rows = match account_id {
            Some(id) => statement
                .query_map(params![query, limit, offset, id], map_row)
                .map_err(display_error)?,
            None => statement
                .query_map(params![query, limit, offset], map_row)
                .map_err(display_error)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    fn apply_mutation(
        transaction: &Transaction<'_>,
        mutation: &ThreadMutation,
    ) -> Result<(), String> {
        let (kind, value) = match mutation {
            ThreadMutation::Archive { value, .. } => ("archive", *value),
            ThreadMutation::Trash { value, .. } => ("trash", *value),
            ThreadMutation::Spam { value, .. } => ("spam", *value),
            ThreadMutation::Read { value, .. } => ("read", *value),
            ThreadMutation::Star { value, .. } => ("star", *value),
            ThreadMutation::Label { value, .. } => ("label", *value),
        };
        let changed = match mutation {
            ThreadMutation::Spam { thread_id, value } => {
                let labels: String = transaction
                    .query_row(
                        "SELECT labels_json FROM threads WHERE id = ?1",
                        [thread_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(display_error)?
                    .ok_or_else(|| "Thread not found".to_string())?;
                let mut labels: Vec<String> =
                    serde_json::from_str(&labels).map_err(display_error)?;
                labels.retain(|item| item != "SPAM" && item != "INBOX");
                labels.push(if *value { "SPAM" } else { "INBOX" }.to_string());
                labels.sort();
                labels.dedup();
                transaction
                    .execute(
                        "UPDATE threads SET labels_json = ?1, archived = ?2 WHERE id = ?3",
                        params![
                            serde_json::to_string(&labels).map_err(display_error)?,
                            value,
                            thread_id,
                        ],
                    )
                    .map_err(display_error)?
            }
            ThreadMutation::Label {
                thread_id,
                label_id,
                value,
            } => {
                let labels: String = transaction
                    .query_row(
                        "SELECT labels_json FROM threads WHERE id = ?1",
                        [thread_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(display_error)?
                    .ok_or_else(|| "Thread not found".to_string())?;
                let mut labels: Vec<String> =
                    serde_json::from_str(&labels).map_err(display_error)?;
                labels.retain(|item| item != label_id);
                if *value {
                    labels.push(label_id.clone());
                    labels.sort();
                    labels.dedup();
                }
                let unread = labels.iter().any(|label| label == "UNREAD");
                let starred = labels.iter().any(|label| label == "STARRED");
                let archived = !labels.iter().any(|label| label == "INBOX");
                transaction
                    .execute(
                        "UPDATE threads
                         SET labels_json = ?1, unread = ?2, starred = ?3, archived = ?4
                         WHERE id = ?5",
                        params![
                            serde_json::to_string(&labels).map_err(display_error)?,
                            unread,
                            starred,
                            archived,
                            thread_id
                        ],
                    )
                    .map_err(display_error)?
            }
            ThreadMutation::Read { thread_id, value } => {
                let changed = transaction
                    .execute(
                        "UPDATE threads SET unread = ?1 WHERE id = ?2",
                        params![!value, thread_id],
                    )
                    .map_err(display_error)?;
                // Mirrors Gmail: marking read clears every message in the
                // thread, but marking unread only brings back the most
                // recent message as unread, not the whole history.
                transaction
                    .execute(
                        "UPDATE messages SET unread = 0 WHERE thread_id = ?1",
                        [thread_id],
                    )
                    .map_err(display_error)?;
                if !value {
                    transaction
                        .execute(
                            "UPDATE messages SET unread = 1
                             WHERE thread_id = ?1 AND sent_at = (
                                 SELECT MAX(sent_at) FROM messages WHERE thread_id = ?1
                             )",
                            [thread_id],
                        )
                        .map_err(display_error)?;
                }
                changed
            }
            _ => {
                let column = match mutation {
                    ThreadMutation::Archive { .. } => "archived",
                    ThreadMutation::Trash { .. } => "trashed",
                    ThreadMutation::Spam { .. } => unreachable!(),
                    ThreadMutation::Read { .. } => unreachable!(),
                    ThreadMutation::Star { .. } => "starred",
                    ThreadMutation::Label { .. } => unreachable!(),
                };
                let stored_value = match mutation {
                    ThreadMutation::Read { .. } => unreachable!(),
                    ThreadMutation::Spam { .. } => unreachable!(),
                    _ => value,
                };
                let sql = format!("UPDATE threads SET {column} = ?1 WHERE id = ?2");
                transaction
                    .execute(&sql, params![stored_value, mutation.thread_id()])
                    .map_err(display_error)?
            }
        };
        if changed == 0 {
            return Err("Thread not found".to_string());
        }
        let account_id: String = transaction
            .query_row(
                "SELECT account_id FROM threads WHERE id = ?1",
                [mutation.thread_id()],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        let payload = serde_json::to_string(mutation).map_err(display_error)?;
        let duplicate: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM mutations
                    WHERE thread_id = ?1 AND kind = ?2 AND payload_json = ?3
                      AND state IN ('pending', 'running')
                 )",
                params![mutation.thread_id(), kind, payload],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        if !duplicate {
            transaction
                .execute(
                    "INSERT INTO mutations(
                    id, account_id, thread_id, kind, payload_json, state, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                    params![
                        Uuid::new_v4().to_string(),
                        account_id,
                        mutation.thread_id(),
                        kind,
                        payload,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(display_error)?;
        }
        Ok(())
    }

    pub fn mutate_thread(&self, mutation: &ThreadMutation) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        Self::apply_mutation(&transaction, mutation)?;
        transaction.commit().map_err(display_error)
    }

    pub fn mutate_threads(&self, mutations: &[ThreadMutation]) -> Result<(), String> {
        if mutations.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        for mutation in mutations {
            Self::apply_mutation(&transaction, mutation)?;
        }
        transaction.commit().map_err(display_error)
    }

    pub fn sync_status(&self, account_id: &str) -> Result<SyncStatus, String> {
        let connection = self.connection()?;
        let (cursor, last_successful_sync, mut error): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT cursor, last_successful_sync, last_error
                 FROM sync_state WHERE account_id = ?1",
                [account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(display_error)?;
        if error.is_none() {
            error = connection
                .query_row(
                    "SELECT last_error FROM mutations
                     WHERE state = 'failed' AND account_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(display_error)?
                .flatten();
        }
        let pending_mutations = connection
            .query_row(
                "SELECT count(*) FROM mutations WHERE state IN ('pending', 'running') AND account_id = ?1",
                [account_id],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        Ok(SyncStatus {
            state: if error.is_some() { "error" } else { "idle" },
            last_successful_sync,
            cursor,
            pending_mutations,
            error,
        })
    }

    pub fn cursor(&self, account_id: &str) -> Result<Option<String>, String> {
        self.connection()?
            .query_row(
                "SELECT cursor FROM sync_state WHERE account_id = ?1",
                [account_id],
                |row| row.get(0),
            )
            .map_err(display_error)
    }

    pub fn finish_sync(&self, account_id: &str, cursor: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute(
                "UPDATE sync_state SET cursor = ?1, last_successful_sync = ?2, last_error = NULL
                 WHERE account_id = ?3",
                params![cursor, now, account_id],
            )
            .map_err(display_error)?;
        transaction
            .execute(
                "UPDATE accounts SET last_synced_at = ?1 WHERE email = ?2",
                params![now, account_id],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn fail_sync(&self, account_id: &str, error: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_state SET last_error = ?1 WHERE account_id = ?2",
                params![error, account_id],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    /// Wipes only `account_id`'s cached threads before a full resync, never
    /// another connected account's mail.
    pub fn begin_full_sync(&self, account_id: &str) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM thread_search WHERE thread_id IN
                    (SELECT id FROM threads WHERE account_id = ?1)",
                [account_id],
            )
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM threads WHERE account_id = ?1", [account_id])
            .map_err(display_error)?;
        // An interrupted full import must restart in full. Keeping the old
        // cursor here would make the next startup perform an incremental sync
        // against an intentionally emptied cache.
        transaction
            .execute(
                "UPDATE sync_state SET cursor = NULL WHERE account_id = ?1",
                [account_id],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn delete_gmail_thread(
        &self,
        account_id: &str,
        provider_thread_id: &str,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let thread_id: Option<String> = transaction
            .query_row(
                "SELECT id FROM threads WHERE account_id = ?1 AND provider_thread_id = ?2",
                params![account_id, provider_thread_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(display_error)?;
        if let Some(thread_id) = thread_id {
            transaction
                .execute(
                    "DELETE FROM thread_search WHERE thread_id = ?1",
                    [&thread_id],
                )
                .map_err(display_error)?;
            transaction
                .execute("DELETE FROM threads WHERE id = ?1", [&thread_id])
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)
    }

    /// Deletes threads (and their messages, via `ON DELETE CASCADE`) whose
    /// newest message is older than the configured retention window.
    /// Starred and trashed threads are always kept regardless of age. A
    /// no-op when retention is unset (unlimited). Returns the number of
    /// threads removed.
    pub fn prune_expired_threads(&self) -> Result<usize, String> {
        let Some(days) = self.retention_days()? else {
            return Ok(0);
        };
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM thread_search WHERE thread_id IN
                    (SELECT id FROM threads WHERE last_message_at < ?1 AND starred = 0 AND trashed = 0)",
                [&cutoff],
            )
            .map_err(display_error)?;
        let removed = transaction
            .execute(
                "DELETE FROM threads WHERE last_message_at < ?1 AND starred = 0 AND trashed = 0",
                [&cutoff],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)?;
        Ok(removed)
    }

    /// Returns freed pages to the OS. Cheap as long as `auto_vacuum` is
    /// already `INCREMENTAL` (see `vacuum_to_incremental`); otherwise a
    /// harmless no-op.
    pub fn reclaim_space(&self) -> Result<(), String> {
        self.connection()?
            .execute_batch("PRAGMA incremental_vacuum;")
            .map_err(display_error)
    }

    /// Whether the database still needs the one-time conversion to
    /// incremental auto-vacuum mode.
    pub fn needs_vacuum_upgrade(&self) -> Result<bool, String> {
        let mode: i64 = self
            .connection()?
            .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
            .map_err(display_error)?;
        Ok(mode != 2)
    }

    /// One-time conversion to incremental auto-vacuum. Rewrites the entire
    /// file (like `VACUUM`), so it can be slow on a large existing database
    /// — call this off the async runtime's blocking pool, not inline at
    /// startup. Must not run inside a transaction.
    pub fn vacuum_to_incremental(&self) -> Result<(), String> {
        self.connection()?
            .execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")
            .map_err(display_error)
    }

    /// Compresses a bounded batch of message bodies still on the legacy
    /// plaintext columns (written before the body-compression migration)
    /// into `body_html_z`/`body_text_z`, clearing the plaintext columns as
    /// it goes so the freed space is reclaimable. Returns the number of rows
    /// converted; call repeatedly (e.g. from a background loop) until it
    /// returns 0.
    pub fn compress_next_body_batch(&self, batch_size: usize) -> Result<usize, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let rows: Vec<(String, String, String)> = {
            let mut statement = transaction
                .prepare(
                    "SELECT id, body_html, body_text FROM messages
                     WHERE body_html_z IS NULL LIMIT ?1",
                )
                .map_err(display_error)?;
            let collected = statement
                .query_map(params![batch_size as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(display_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(display_error)?;
            collected
        };
        let count = rows.len();
        for (id, body_html, body_text) in rows {
            transaction
                .execute(
                    "UPDATE messages SET body_html = '', body_text = '',
                        body_html_z = ?1, body_text_z = ?2
                     WHERE id = ?3",
                    params![compress_body(&body_html), compress_body(&body_text), id],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)?;
        Ok(count)
    }

    fn apply_gmail_thread(
        transaction: &Transaction<'_>,
        account_id: &str,
        messages: &[NormalizedMessage],
    ) -> Result<(), String> {
        let Some(latest) = messages.iter().max_by(|a, b| a.date.cmp(&b.date)) else {
            return Ok(());
        };
        let provider_thread_id = &latest.thread_id;
        let thread_id = local_thread_id(account_id, provider_thread_id);
        let mut participants: Vec<String> = messages
            .iter()
            .map(|message| message.from.clone())
            .collect();
        participants.sort();
        participants.dedup();
        // Indexed separately from `participants`: recipients should be
        // searchable even though they aren't shown in the thread list's
        // "From" line.
        let mut search_participants: Vec<String> = messages
            .iter()
            .flat_map(|message| {
                std::iter::once(message.from.clone()).chain(message.to.iter().cloned())
            })
            .collect();
        search_participants.sort();
        search_participants.dedup();
        let mut labels: Vec<String> = messages
            .iter()
            .flat_map(|message| message.labels.iter().cloned())
            .collect();
        labels.sort();
        labels.dedup();
        let unread = labels.iter().any(|label| label == "UNREAD");
        let starred = labels.iter().any(|label| label == "STARRED");
        let archived = !labels.iter().any(|label| label == "INBOX");
        let trashed = labels.iter().any(|label| label == "TRASH");
        let has_attachments = messages.iter().any(|message| {
            message
                .attachments
                .iter()
                .any(|attachment| !attachment.inline)
        });
        // Only messages someone else sent should bump a thread's place in the
        // inbox order; otherwise replying to a thread buried in the list
        // would jump it straight to the top like a freshly received message.
        let last_received_at = messages
            .iter()
            .filter(|message| !message.labels.iter().any(|label| label == "SENT"))
            .max_by(|a, b| a.date.cmp(&b.date))
            .map(|message| message.date.clone())
            .unwrap_or_else(|| latest.date.clone());
        transaction
            .execute(
                "INSERT INTO threads(
                    id, account_id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed, has_attachments,
                    last_received_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(id) DO UPDATE SET
                    subject=excluded.subject, snippet=excluded.snippet,
                    participants_json=excluded.participants_json,
                    last_message_at=excluded.last_message_at, unread=excluded.unread,
                    starred=excluded.starred, archived=excluded.archived,
                    labels_json=excluded.labels_json, trashed=excluded.trashed,
                    has_attachments=excluded.has_attachments,
                    last_received_at=excluded.last_received_at",
                params![
                    thread_id,
                    account_id,
                    provider_thread_id,
                    latest.subject,
                    latest.snippet,
                    serde_json::to_string(&participants).map_err(display_error)?,
                    latest.date,
                    unread,
                    starred,
                    archived,
                    serde_json::to_string(&labels).map_err(display_error)?,
                    trashed,
                    has_attachments,
                    last_received_at,
                ],
            )
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM messages WHERE thread_id = ?1", [&thread_id])
            .map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM thread_search WHERE thread_id = ?1",
                [&thread_id],
            )
            .map_err(display_error)?;
        let mut body = String::new();
        for message in messages {
            transaction.execute("INSERT INTO message_metadata(id, payload) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![message.id, message.metadata_json]).map_err(display_error)?;
            body.push_str(&message.body_text);
            body.push(' ');
            let message_unread = message.labels.iter().any(|label| label == "UNREAD");
            transaction
                .execute(
                    "INSERT INTO messages(
                        id, thread_id, sender, recipients_json, sent_at, body_html, body_text,
                        body_html_z, body_text_z, unsubscribe_json, unread, attachments_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        message.id,
                        thread_id,
                        message.from,
                        serde_json::to_string(&message.to).map_err(display_error)?,
                        message.date,
                        "",
                        "",
                        compress_body(&message.body_html),
                        compress_body(&message.body_text),
                        message
                            .unsubscribe
                            .as_ref()
                            .map(|value| serde_json::to_string(value).map_err(display_error))
                            .transpose()?,
                        message_unread,
                        serde_json::to_string(&message.attachments).map_err(display_error)?,
                    ],
                )
                .map_err(display_error)?;
        }
        transaction
            .execute(
                "INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    thread_id,
                    latest.subject,
                    latest.snippet,
                    search_participants.join(" "),
                    body
                ],
            )
            .map_err(display_error)?;
        Ok(())
    }

    pub fn upsert_gmail_thread(
        &self,
        account_id: &str,
        messages: &[NormalizedMessage],
    ) -> Result<(), String> {
        self.upsert_gmail_threads(account_id, &[messages.to_vec()])
    }

    pub fn upsert_gmail_threads(
        &self,
        account_id: &str,
        message_groups: &[Vec<NormalizedMessage>],
    ) -> Result<(), String> {
        if message_groups.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        for messages in message_groups {
            Self::apply_gmail_thread(&transaction, account_id, messages)?;
        }
        transaction.commit().map_err(display_error)
    }

    /// Claims only `account_id`'s pending mutations, so one account's poller
    /// never picks up and tries to deliver another account's mutation
    /// through the wrong Gmail session.
    pub fn claim_mutations(
        &self,
        account_id: &str,
        limit: usize,
    ) -> Result<Vec<PendingMutation>, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let result = {
            let mut statement = transaction
                .prepare(
                    "SELECT m.id, t.provider_thread_id, m.payload_json
                     FROM mutations m JOIN threads t ON t.id = m.thread_id
                     WHERE m.state = 'pending' AND m.account_id = ?1
                     ORDER BY m.created_at LIMIT ?2",
                )
                .map_err(display_error)?;
            let rows = statement
                .query_map(params![account_id, limit as i64], |row| {
                    let payload: String = row.get(2)?;
                    let mutation = serde_json::from_str(&payload).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            payload.len(),
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(PendingMutation {
                        id: row.get(0)?,
                        provider_thread_id: row.get(1)?,
                        mutation,
                    })
                })
                .map_err(display_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(display_error)?
        };
        for mutation in &result {
            transaction
                .execute(
                    "UPDATE mutations SET state = 'running', attempts = attempts + 1
                     WHERE id = ?1 AND state = 'pending'",
                    [&mutation.id],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)?;
        Ok(result)
    }

    pub fn complete_mutation(&self, id: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE mutations SET state = 'done', last_error = NULL WHERE id = ?1",
                [id],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    pub fn reject_mutation(&self, id: &str, error: &str, retry: bool) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE mutations SET state = ?1, last_error = ?2 WHERE id = ?3",
                params![if retry { "pending" } else { "failed" }, error, id],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    /// Applies the native portion of a settings transfer atomically. Existing
    /// destination accounts keep their connection status because their
    /// keychain credentials are deliberately not part of the transfer. An
    /// account seen only in the imported file is created as `needs_reauth`.
    pub(crate) fn import_transfer_data(
        &self,
        accounts: &[TransferAccount],
        split_inboxes: &[TransferSplitInbox],
        retention_days: Option<i64>,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute(
                "UPDATE accounts SET sort_order = sort_order + ?1",
                [accounts.len() as i64],
            )
            .map_err(display_error)?;
        let connected_at = Utc::now().to_rfc3339();
        for account in accounts {
            transaction
                .execute(
                    "INSERT INTO accounts(
                         email, display_name, color, status, sort_order, connected_at, last_synced_at
                     ) VALUES (?1, ?2, ?3, 'needs_reauth', ?4, ?5, NULL)
                     ON CONFLICT(email) DO UPDATE SET
                         display_name = excluded.display_name,
                         color = excluded.color,
                         sort_order = excluded.sort_order",
                    params![
                        account.email,
                        account.display_name,
                        account.color,
                        account.sort_order,
                        connected_at,
                    ],
                )
                .map_err(display_error)?;
        }

        transaction
            .execute("DELETE FROM split_inboxes", [])
            .map_err(display_error)?;
        for split in split_inboxes {
            transaction
                .execute(
                    "INSERT INTO split_inboxes(
                         id, name, match_kind, match_value, sort_order, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        split.id,
                        split.name,
                        split.match_kind,
                        split.match_value,
                        split.sort_order,
                        split.created_at,
                    ],
                )
                .map_err(display_error)?;
        }

        match retention_days {
            Some(days) => transaction
                .execute(
                    "INSERT INTO compose_settings(key, value) VALUES ('retention_days', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    [days.to_string()],
                )
                .map_err(display_error)?,
            None => transaction
                .execute(
                    "DELETE FROM compose_settings WHERE key = 'retention_days'",
                    [],
                )
                .map_err(display_error)?,
        };
        transaction.commit().map_err(display_error)
    }

    pub fn create_split_inbox(
        &self,
        name: &str,
        match_kind: &str,
        match_value: &str,
    ) -> Result<SplitInbox, String> {
        let name = name.trim();
        let match_value = match_value.trim();
        if name.is_empty() {
            return Err("Split inbox name cannot be empty".to_string());
        }
        if match_value.is_empty() {
            return Err("Split inbox match value cannot be empty".to_string());
        }
        if !matches!(match_kind, "domain" | "label" | "pattern") {
            return Err("Unknown split inbox match kind".to_string());
        }
        // Domains and patterns are matched case-insensitively against
        // lowercased addresses (see `split_inbox_matches`), so normalize
        // once here rather than on every match. Label ids are case-sensitive
        // Gmail identifiers and must be stored as-is.
        let match_value = if match_kind == "label" {
            match_value.to_string()
        } else {
            match_value.to_ascii_lowercase()
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let sort_order: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM split_inboxes",
                [],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();
        transaction
            .execute(
                "INSERT INTO split_inboxes(id, name, match_kind, match_value, sort_order, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, name, match_kind, match_value, sort_order, created_at],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)?;
        Ok(SplitInbox {
            id,
            name: name.to_string(),
            match_kind: match_kind.to_string(),
            match_value,
            sort_order,
            created_at,
        })
    }

    pub fn update_split_inbox(&self, id: &str, name: &str) -> Result<SplitInbox, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Split inbox name cannot be empty".to_string());
        }
        let changed = self
            .connection()?
            .execute(
                "UPDATE split_inboxes SET name = ?1 WHERE id = ?2",
                params![name, id],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Split inbox not found".to_string());
        }
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT id, name, match_kind, match_value, sort_order, created_at
                 FROM split_inboxes WHERE id = ?1",
                [id],
                split_inbox_from_row,
            )
            .map_err(display_error)
    }

    pub fn delete_split_inbox(&self, id: &str) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM split_inboxes WHERE id = ?1", [id])
            .map_err(display_error)?;
        Ok(())
    }

    pub fn reorder_split_inboxes(&self, ordered_ids: &[String]) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        for (index, id) in ordered_ids.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE split_inboxes SET sort_order = ?1 WHERE id = ?2",
                    params![index as i64, id],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)
    }

    /// Filters the same unarchived/untrashed base set `list_threads` uses
    /// down to one split inbox's rule, then paginates in memory. That base
    /// set is realistically bounded (an actively-triaged inbox), so
    /// re-filtering it on every page load is simpler than building true
    /// SQL-level cursor pagination over a JSON column with no useful index.
    pub fn list_split_inbox_page(
        &self,
        split_inbox_id: &str,
        account_id: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ThreadPage, String> {
        let rule = self
            .connection()
            .and_then(|connection| {
                connection
                    .query_row(
                        "SELECT id, name, match_kind, match_value, sort_order, created_at
                         FROM split_inboxes WHERE id = ?1",
                        [split_inbox_id],
                        split_inbox_from_row,
                    )
                    .optional()
                    .map_err(display_error)
            })?
            .ok_or_else(|| "Split inbox not found".to_string())?;
        let matched: Vec<Thread> = self
            .list_threads(account_id)?
            .into_iter()
            .filter(|thread| split_inbox_matches(&rule, thread))
            .collect();
        let page_limit = limit.min(200);
        let has_more = matched.len() > offset.saturating_add(page_limit);
        let threads = matched.into_iter().skip(offset).take(page_limit).collect();
        Ok(ThreadPage { threads, has_more })
    }

    /// Unread totals for the Inbox and each split inbox tab, scoped to one
    /// account (or merged across all when `account_id` is `None`). A thread
    /// counts toward the Inbox only when no split inbox rule claims it,
    /// mirroring the exclusion `list_threads_page` applies.
    pub fn mailbox_unread_counts(
        &self,
        account_id: Option<&str>,
    ) -> Result<MailboxUnreadCounts, String> {
        let rules = self.list_split_inboxes()?;
        let threads = self.list_threads(account_id)?;
        let mut splits: HashMap<String, i64> = HashMap::new();
        let mut inbox = 0i64;
        for thread in &threads {
            if !thread.unread {
                continue;
            }
            let mut matched_any = false;
            for rule in &rules {
                if split_inbox_matches(rule, thread) {
                    *splits.entry(rule.id.clone()).or_insert(0) += 1;
                    matched_any = true;
                }
            }
            if !matched_any {
                inbox += 1;
            }
        }
        Ok(MailboxUnreadCounts { inbox, splits })
    }
}

fn ensure_query_indexes(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS threads_account_mailbox_order
         ON threads(account_id, trashed, archived, last_received_at DESC);
         CREATE INDEX IF NOT EXISTS threads_mailbox_order
         ON threads(trashed, archived, last_received_at DESC);
         CREATE INDEX IF NOT EXISTS mutations_account_pending
         ON mutations(account_id, state, created_at);",
    )
}

fn sender_identity_for_thread(
    transaction: &Transaction<'_>,
    thread_id: &str,
) -> Result<Option<(String, String, String)>, String> {
    let account_id: Option<String> = transaction
        .query_row(
            "SELECT account_id FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(display_error)?;
    let Some(account_id) = account_id else {
        return Ok(None);
    };

    let mut statement = transaction
        .prepare(
            "SELECT sender FROM messages
             WHERE thread_id = ?1
             ORDER BY sent_at DESC, id DESC",
        )
        .map_err(display_error)?;
    let rows = statement
        .query_map([thread_id], |row| row.get::<_, String>(0))
        .map_err(display_error)?;
    let account_email = normalize_sender(&account_id).0;
    let mut fallback = None;
    for row in rows {
        let sender = row.map_err(display_error)?;
        let (email, domain) = normalize_sender(&sender);
        if email.is_empty() {
            continue;
        }
        if fallback.is_none() {
            fallback = Some((email.clone(), domain.clone()));
        }
        if account_email.is_empty() || email != account_email {
            return Ok(Some((account_id, email, domain)));
        }
    }
    if account_email.is_empty() {
        Ok(fallback.map(|(email, domain)| (account_id, email, domain)))
    } else {
        // A sent-only thread has no sender preference to learn from.
        Ok(None)
    }
}

fn normalize_sender(value: &str) -> (String, String) {
    let trimmed = value.trim();
    let candidate = match (trimmed.rfind('<'), trimmed.rfind('>')) {
        (Some(open), Some(close)) if close > open => &trimmed[open + 1..close],
        _ => trimmed,
    };
    let email = candidate
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
        .to_ascii_lowercase();
    let domain = email
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_string())
        .unwrap_or_default();
    (email, domain)
}

fn triage_event_kind_name(kind: &TriageEventKind) -> &'static str {
    match kind {
        TriageEventKind::Open => "open",
        TriageEventKind::Close => "close",
        TriageEventKind::Disposition => "disposition",
        TriageEventKind::Restore => "restore",
        TriageEventKind::Response => "response",
    }
}

fn triage_context_name(context: &TriageContext) -> &'static str {
    match context {
        TriageContext::Inbox => "inbox",
        TriageContext::Other => "other",
    }
}

fn triage_action_name(action: &TriageAction) -> &'static str {
    match action {
        TriageAction::Archive => "archive",
        TriageAction::Trash => "trash",
    }
}

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        email: row.get(0)?,
        display_name: row.get(1)?,
        color: row.get(2)?,
        status: row.get(3)?,
        sort_order: row.get(4)?,
        connected_at: row.get(5)?,
        last_synced_at: row.get(6)?,
    })
}

fn split_inbox_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SplitInbox> {
    Ok(SplitInbox {
        id: row.get(0)?,
        name: row.get(1)?,
        match_kind: row.get(2)?,
        match_value: row.get(3)?,
        sort_order: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// `match_value` is normalized to lowercase at creation time for `domain`
/// and `pattern` rules (see `Database::create_split_inbox`), so only the
/// participant side needs lowercasing here.
fn split_inbox_matches(rule: &SplitInbox, thread: &Thread) -> bool {
    match rule.match_kind.as_str() {
        "domain" => thread
            .participants
            .iter()
            .any(|participant| normalize_sender(participant).1 == rule.match_value),
        "label" => thread.labels.iter().any(|label| label == &rule.match_value),
        "pattern" => thread
            .participants
            .iter()
            .any(|participant| normalize_sender(participant).0.contains(&rule.match_value)),
        _ => false,
    }
}

fn unsubscribe_method_name(method: &UnsubscribeMethod) -> &'static str {
    match method {
        UnsubscribeMethod::OneClick => "oneClick",
        UnsubscribeMethod::Mailto => "mailto",
        UnsubscribeMethod::Web => "web",
    }
}

fn thread_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: row.get(0)?,
        provider_thread_id: row.get(1)?,
        subject: row.get(2)?,
        snippet: row.get(3)?,
        participants: decode_json(row.get::<_, String>(4)?)?,
        last_message_at: row.get(5)?,
        unread: row.get(6)?,
        starred: row.get(7)?,
        archived: row.get(8)?,
        labels: decode_json(row.get::<_, String>(9)?)?,
        trashed: row.get(10)?,
        account_id: row.get(11)?,
        summary: row.get(12)?,
        summary_generated_at: row.get(13)?,
        has_attachments: row.get(14)?,
        last_received_at: row.get(15)?,
        match_snippet: None,
    })
}

fn decode_json(value: String) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

/// zstd-compresses message body text for the `body_html_z`/`body_text_z`
/// columns. Encoding an in-memory byte slice cannot meaningfully fail.
fn compress_body(text: &str) -> Vec<u8> {
    zstd::stream::encode_all(text.as_bytes(), 3)
        .expect("zstd encoding of an in-memory byte slice cannot fail")
}

/// Prefers the compressed column when present (every row written after the
/// body-compression migration); falls back to the legacy plaintext column
/// for rows synced before it.
fn resolve_body(
    column_index: usize,
    legacy: String,
    compressed: Option<Vec<u8>>,
) -> rusqlite::Result<String> {
    match compressed {
        Some(bytes) => zstd::stream::decode_all(bytes.as_slice())
            .ok()
            .and_then(|buf| String::from_utf8(buf).ok())
            .ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    column_index,
                    rusqlite::types::Type::Blob,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "failed to decompress message body",
                    )),
                )
            }),
        None => Ok(legacy),
    }
}

/// Builds an FTS5 MATCH expression, ANDing together every unquoted word and
/// every "quoted phrase" as a prefix match. `input.split('"')` alternates
/// unquoted segments (even indices) with quoted ones (odd indices); an
/// unterminated trailing quote is simply treated as still-quoted.
fn fts_query(input: &str) -> String {
    let mut terms: Vec<String> = Vec::new();
    for (index, segment) in input.split('"').enumerate() {
        if index % 2 == 0 {
            for word in segment.split_whitespace() {
                terms.push(format!("\"{}\"*", word));
            }
        } else {
            let words: Vec<&str> = segment.split_whitespace().collect();
            if words.is_empty() {
                continue;
            }
            terms.push(format!("\"{}\"*", words.join(" ")));
        }
    }
    terms.join(" AND ")
}

fn seed_if_empty(connection: &Connection) -> rusqlite::Result<()> {
    let count: i64 = connection.query_row("SELECT count(*) FROM threads", [], |row| row.get(0))?;
    if count != 0 {
        return Ok(());
    }
    let transaction = connection.unchecked_transaction()?;
    insert_demo(
        &transaction,
        "welcome",
        "Welcome to Dispatch",
        "A keyboard-first inbox that keeps your mail on this device.",
        "Dispatch",
        "2026-03-05T16:30:00Z",
        true,
        false,
        "<p>Welcome to <strong>Dispatch</strong>.</p><p>Use <kbd>j</kbd> and <kbd>k</kbd> to move, <kbd>e</kbd> to archive, <kbd>s</kbd> to star, and <kbd>⌘K</kbd> to open the command palette.</p>",
    )?;
    transaction.commit()
}

#[allow(clippy::too_many_arguments)]
fn insert_demo(
    transaction: &Transaction<'_>,
    id: &str,
    subject: &str,
    snippet: &str,
    participant: &str,
    sent_at: &str,
    unread: bool,
    starred: bool,
    body: &str,
) -> rusqlite::Result<()> {
    let participants = serde_json::to_string(&[participant]).expect("static data serializes");
    let labels = serde_json::to_string(&["INBOX"]).expect("static data serializes");
    transaction.execute(
        "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, 0, 'default', NULL, NULL, 0, ?6)",
        params![
            id,
            format!("demo-{id}"),
            subject,
            snippet,
            participants,
            sent_at,
            unread,
            starred,
            labels
        ],
    )?;
    transaction.execute(
        "INSERT INTO messages(
            id, thread_id, sender, recipients_json, sent_at, body_html, body_text, unread
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            format!("{id}-message"),
            id,
            format!("{participant} <hello@dispatch.local>"),
            "[\"You <you@example.com>\"]",
            sent_at,
            body,
            snippet,
            unread,
        ],
    )?;
    transaction.execute(
        "INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, subject, snippet, participant, snippet],
    )?;
    Ok(())
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{TransferAccount, TransferSplitInbox};

    fn database() -> Database {
        let database = Database::open_memory();
        let connection = database.connection().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        insert_demo(
            &transaction,
            "roadmap",
            "Phase 1: read and triage",
            "The first vertical slice includes local search and optimistic actions.",
            "Product Team",
            "2026-03-05T14:15:00Z",
            false,
            true,
            "<p>The first read-and-triage vertical slice is now running from local SQLite.</p>",
        )
        .unwrap();
        transaction.commit().unwrap();
        drop(connection);
        database
    }

    #[test]
    fn fresh_database_does_not_seed_internal_roadmap_message() {
        let database = Database::open_memory();
        let threads = database.list_threads(None).unwrap();

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].subject, "Welcome to Dispatch");
        assert!(threads
            .iter()
            .all(|thread| thread.subject != "Phase 1: read and triage"));
    }

    #[test]
    fn import_transfer_preserves_local_credentials_and_marks_new_accounts_for_connection() {
        let database = database();
        database.adopt_account("connected@example.com").unwrap();
        database
            .create_split_inbox("Old rule", "domain", "old.example")
            .unwrap();

        database
            .import_transfer_data(
                &[
                    TransferAccount {
                        email: "new@example.com".to_string(),
                        display_name: Some("New account".to_string()),
                        color: "#123456".to_string(),
                        sort_order: 0,
                    },
                    TransferAccount {
                        email: "connected@example.com".to_string(),
                        display_name: Some("Connected account".to_string()),
                        color: "#654321".to_string(),
                        sort_order: 1,
                    },
                ],
                &[TransferSplitInbox {
                    id: "imported-split".to_string(),
                    name: "Imported rule".to_string(),
                    match_kind: "pattern".to_string(),
                    match_value: "newsletter".to_string(),
                    sort_order: 0,
                    created_at: "2026-03-06T00:00:00Z".to_string(),
                }],
                Some(90),
            )
            .unwrap();

        assert_eq!(
            database
                .get_account("connected@example.com")
                .unwrap()
                .unwrap()
                .status,
            "connected"
        );
        assert_eq!(
            database
                .get_account("new@example.com")
                .unwrap()
                .unwrap()
                .status,
            "needs_reauth"
        );
        let splits = database.list_split_inboxes().unwrap();
        assert_eq!(splits.len(), 1);
        assert_eq!(splits[0].name, "Imported rule");
        assert_eq!(database.retention_days().unwrap(), Some(90));
    }

    #[test]
    fn searches_local_fts_index() {
        let result = database()
            .search_threads(
                &SearchThreadsRequest {
                    query: "keyboard".into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(result[0].id, "welcome");
        assert!(result[0].match_snippet.is_some());
    }

    #[test]
    fn triage_events_attribute_senders_and_rank_quick_dismissals() {
        let database = database();
        let mut quick_message = message(
            "quick-message",
            "quick-thread",
            "2026-01-01T00:00:00Z",
            "quick body",
        );
        quick_message.from = "Noise <Newsletter@Example.com>".into();
        database
            .upsert_gmail_thread("work@example.com", &[quick_message])
            .unwrap();

        let mut engaged_message = message(
            "engaged-message",
            "engaged-thread",
            "2026-01-02T00:00:00Z",
            "engaged body",
        );
        engaged_message.from = "A Person <person@example.com>".into();
        database
            .upsert_gmail_thread("work@example.com", &[engaged_message])
            .unwrap();

        for _ in 0..2 {
            database
                .record_triage_event(&TriageEvent {
                    thread_id: "work@example.com:quick-thread".into(),
                    kind: TriageEventKind::Open,
                    context: TriageContext::Inbox,
                    action: None,
                    opened: false,
                    dwell_ms: None,
                    scrolled: false,
                    batch: false,
                })
                .unwrap();
            database
                .record_triage_event(&TriageEvent {
                    thread_id: "work@example.com:quick-thread".into(),
                    kind: TriageEventKind::Disposition,
                    context: TriageContext::Inbox,
                    action: Some(TriageAction::Archive),
                    opened: true,
                    dwell_ms: Some(400),
                    scrolled: false,
                    batch: false,
                })
                .unwrap();
        }
        database
            .record_triage_event(&TriageEvent {
                thread_id: "work@example.com:quick-thread".into(),
                kind: TriageEventKind::Response,
                context: TriageContext::Inbox,
                action: None,
                opened: true,
                dwell_ms: None,
                scrolled: false,
                batch: false,
            })
            .unwrap();
        database
            .record_triage_event(&TriageEvent {
                thread_id: "work@example.com:engaged-thread".into(),
                kind: TriageEventKind::Open,
                context: TriageContext::Inbox,
                action: None,
                opened: false,
                dwell_ms: None,
                scrolled: false,
                batch: false,
            })
            .unwrap();
        database
            .record_triage_event(&TriageEvent {
                thread_id: "work@example.com:engaged-thread".into(),
                kind: TriageEventKind::Close,
                context: TriageContext::Inbox,
                action: None,
                opened: false,
                dwell_ms: Some(2400),
                scrolled: true,
                batch: false,
            })
            .unwrap();

        let stats = database
            .list_triage_sender_stats("work@example.com", 100)
            .unwrap();
        assert_eq!(stats[0].sender_email, "newsletter@example.com");
        assert_eq!(stats[0].sender_domain, "example.com");
        assert_eq!(stats[0].exposure_count, 2);
        assert_eq!(stats[0].archive_count, 2);
        assert_eq!(stats[0].quick_disposition_count, 2);
        assert_eq!(stats[0].quick_disposition_rate, 1.0);
        assert_eq!(stats[0].response_count, 1);
        assert_eq!(stats[1].sender_email, "person@example.com");
        assert_eq!(stats[1].engaged_view_count, 1);
        assert_eq!(stats[1].quick_disposition_count, 0);

        // Context is retained in the raw log but does not contaminate the
        // inbox-derived candidate stats.
        database
            .record_triage_event(&TriageEvent {
                thread_id: "work@example.com:quick-thread".into(),
                kind: TriageEventKind::Disposition,
                context: TriageContext::Other,
                action: Some(TriageAction::Trash),
                opened: true,
                dwell_ms: Some(100),
                scrolled: false,
                batch: false,
            })
            .unwrap();
        let stats = database
            .list_triage_sender_stats("work@example.com", 100)
            .unwrap();
        assert_eq!(stats[0].trash_count, 0);
    }

    #[test]
    fn contact_suggestions_rank_sent_recipients_above_mere_senders_and_filter_by_prefix() {
        let database = database();
        let mut sent = message(
            "sent-message",
            "sent-thread",
            "2026-01-01T00:00:00Z",
            "body",
        );
        sent.from = "you@example.com".into();
        sent.to = vec!["Jane Doe <jane@example.com>".into()];
        database
            .upsert_gmail_thread("you@example.com", &[sent])
            .unwrap();

        let mut received = message(
            "received-message",
            "received-thread",
            "2026-01-02T00:00:00Z",
            "body",
        );
        received.from = "Newsletter <newsletter@example.com>".into();
        received.to = vec!["you@example.com".into()];
        database
            .upsert_gmail_thread("you@example.com", &[received])
            .unwrap();

        let suggestions = database
            .list_contact_suggestions("you@example.com", "", 10)
            .unwrap();
        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].email, "jane@example.com");
        assert_eq!(suggestions[0].display_name.as_deref(), Some("Jane Doe"));
        assert_eq!(suggestions[0].sent_count, 1);
        assert_eq!(suggestions[1].email, "newsletter@example.com");
        assert_eq!(suggestions[1].received_count, 1);

        let filtered = database
            .list_contact_suggestions("you@example.com", "jan", 10)
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].email, "jane@example.com");

        let filtered_out = database
            .list_contact_suggestions("you@example.com", "zzz", 10)
            .unwrap();
        assert!(filtered_out.is_empty());
    }

    #[test]
    fn contact_suggestions_match_email_prefixes_display_names_and_domains() {
        let database = database();
        let mut sent = message(
            "sent-to-kristen",
            "sent-to-kristen-thread",
            "2026-01-01T00:00:00Z",
            "body",
        );
        sent.from = "you@example.com".into();
        sent.to = vec!["Kristen Hammett <khammett@carsonwealth.com>".into()];
        database
            .upsert_gmail_thread("you@example.com", &[sent])
            .unwrap();

        for query in ["kham", "Kristen", "hammett", "CARS", "wealth"] {
            let matches = database
                .list_contact_suggestions("you@example.com", query, 10)
                .unwrap();
            assert_eq!(matches.len(), 1, "query {query:?} should match");
            assert_eq!(matches[0].email, "khammett@carsonwealth.com");
        }
    }

    #[test]
    fn pinned_contacts_outrank_history_and_survive_without_any_messages() {
        let database = database();
        let mut sent = message(
            "sent-message",
            "sent-thread",
            "2026-01-01T00:00:00Z",
            "body",
        );
        sent.from = "you@example.com".into();
        sent.to = vec!["Frequent <frequent@example.com>".into()];
        database
            .upsert_gmail_thread("you@example.com", &[sent])
            .unwrap();

        database
            .pin_contact(
                "you@example.com",
                "Pinned@Example.com",
                Some("Pinned Person"),
            )
            .unwrap();

        let suggestions = database
            .list_contact_suggestions("you@example.com", "", 10)
            .unwrap();
        assert_eq!(suggestions[0].email, "pinned@example.com");
        assert!(suggestions[0].pinned);
        assert_eq!(suggestions[0].sent_count, 0);
        assert_eq!(suggestions[1].email, "frequent@example.com");
        assert!(!suggestions[1].pinned);

        database
            .unpin_contact("you@example.com", "pinned@example.com")
            .unwrap();
        let after_unpin = database
            .list_contact_suggestions("you@example.com", "", 10)
            .unwrap();
        assert_eq!(after_unpin.len(), 1);
        assert_eq!(after_unpin[0].email, "frequent@example.com");
    }

    #[test]
    fn contact_suggestions_exclude_automated_senders_unless_sent_to_or_pinned() {
        let database = database();
        let mut newsletter = message(
            "newsletter-message",
            "newsletter-thread",
            "2026-01-01T00:00:00Z",
            "body",
        );
        newsletter.from = "Newsletter <newsletter@example.com>".into();
        newsletter.to = vec!["you@example.com".into()];
        newsletter.unsubscribe = Some(UnsubscribeMetadata {
            one_click_url: Some("https://example.com/unsubscribe".into()),
            mailto_url: None,
            web_url: None,
            list_id: None,
        });
        database
            .upsert_gmail_thread("you@example.com", &[newsletter])
            .unwrap();

        assert!(database
            .list_contact_suggestions("you@example.com", "", 10)
            .unwrap()
            .is_empty());

        // Mailing that same address directly still earns it a suggestion —
        // the exclusion only blocks the "heard from" side.
        let mut sent = message(
            "sent-message",
            "sent-thread",
            "2026-01-02T00:00:00Z",
            "body",
        );
        sent.from = "you@example.com".into();
        sent.to = vec!["newsletter@example.com".into()];
        database
            .upsert_gmail_thread("you@example.com", &[sent])
            .unwrap();

        let suggestions = database
            .list_contact_suggestions("you@example.com", "", 10)
            .unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].email, "newsletter@example.com");
        assert_eq!(suggestions[0].sent_count, 1);
        assert_eq!(suggestions[0].received_count, 0);
    }

    #[test]
    fn phrase_search_requires_contiguous_words() {
        let database = database();
        let matches = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "\"keeps your mail\"".into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(matches[0].id, "welcome");

        let no_matches = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "\"mail your keeps\"".into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert!(no_matches.is_empty());
    }

    #[test]
    fn search_excludes_archived_unless_requested() {
        let database = database();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();

        let hidden = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "keyboard".into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert!(hidden.is_empty());

        let shown = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "keyboard".into(),
                    limit: None,
                    offset: None,
                    include_archived: Some(true),
                },
                None,
            )
            .unwrap();
        assert_eq!(shown[0].id, "welcome");
    }

    #[test]
    fn search_supports_offset_pagination() {
        let database = database();
        for (id, date) in [
            ("alpha", "2026-01-01T00:00:00Z"),
            ("beta", "2026-01-02T00:00:00Z"),
        ] {
            database
                .upsert_gmail_thread(
                    "default",
                    &[NormalizedMessage {
                        id: format!("{id}-message"),
                        thread_id: id.into(),
                        subject: "Pagination test".into(),
                        from: "sender@example.com".into(),
                        to: vec!["recipient@example.com".into()],
                        date: date.into(),
                        body_html: String::new(),
                        body_text: "unique-pagination-term".into(),
                        snippet: "unique-pagination-term".into(),
                        labels: vec!["INBOX".into()],
                        metadata_json: "{}".into(),
                        unsubscribe: None,
                        attachments: vec![],
                    }],
                )
                .unwrap();
        }

        let first_page = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "unique-pagination-term".into(),
                    limit: Some(1),
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        let second_page = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "unique-pagination-term".into(),
                    limit: Some(1),
                    offset: Some(1),
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(second_page.len(), 1);
        assert_ne!(first_page[0].id, second_page[0].id);
    }

    #[test]
    fn mailbox_pages_report_remaining_rows() {
        let database = database();
        let first = database.list_threads_page(None, 0, 1).unwrap();
        let second = database.list_threads_page(None, 1, 1).unwrap();
        assert_eq!(first.threads.len(), 1);
        assert!(first.has_more);
        assert_eq!(second.threads.len(), 1);
        assert!(!second.has_more);
    }

    #[test]
    fn unread_counts_include_only_unread_inbox_threads_and_group_by_account() {
        let database = database();
        let connection = database.connection().unwrap();
        connection
            .execute(
                "UPDATE threads SET account_id = 'work@example.com' WHERE id = 'roadmap'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE threads SET unread = 1 WHERE id IN ('welcome', 'roadmap')",
                [],
            )
            .unwrap();
        drop(connection);

        let counts = database.list_unread_counts().unwrap();
        assert_eq!(counts.get("default"), Some(&1));
        assert_eq!(counts.get("work@example.com"), Some(&1));

        let connection = database.connection().unwrap();
        connection
            .execute("UPDATE threads SET archived = 1 WHERE id = 'roadmap'", [])
            .unwrap();
        drop(connection);
        let counts = database.list_unread_counts().unwrap();
        assert_eq!(counts.get("default"), Some(&1));
        assert!(!counts.contains_key("work@example.com"));
    }

    #[test]
    fn batch_mutations_commit_together() {
        let database = database();
        database
            .mutate_threads(&[
                ThreadMutation::Star {
                    thread_id: "welcome".into(),
                    value: true,
                },
                ThreadMutation::Star {
                    thread_id: "roadmap".into(),
                    value: false,
                },
            ])
            .unwrap();
        let threads = database.list_threads(None).unwrap();
        assert!(
            threads
                .iter()
                .find(|thread| thread.id == "welcome")
                .unwrap()
                .starred
        );
        assert!(
            !threads
                .iter()
                .find(|thread| thread.id == "roadmap")
                .unwrap()
                .starred
        );
    }

    #[test]
    fn deleting_a_thread_also_removes_its_search_index_row() {
        let database = database();
        database
            .delete_gmail_thread("default", "demo-welcome")
            .unwrap();
        let remaining: i64 = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM thread_search WHERE thread_id = 'welcome'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn trashing_a_thread_hides_it_from_the_inbox_and_search() {
        let database = database();
        database
            .mutate_thread(&ThreadMutation::Trash {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        assert!(!database
            .list_threads(None)
            .unwrap()
            .iter()
            .any(|thread| thread.id == "welcome"));

        let hidden = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "keyboard".into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert!(hidden.is_empty());

        let shown = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "keyboard".into(),
                    limit: None,
                    offset: None,
                    include_archived: Some(true),
                },
                None,
            )
            .unwrap();
        assert!(shown[0].trashed);
    }

    #[test]
    fn mutation_is_optimistic_and_durable() {
        let database = database();
        let mutation = ThreadMutation::Archive {
            thread_id: "welcome".into(),
            value: true,
        };
        database.mutate_thread(&mutation).unwrap();
        database.mutate_thread(&mutation).unwrap();
        assert!(!database
            .list_threads(None)
            .unwrap()
            .iter()
            .any(|thread| thread.id == "welcome"));
        assert_eq!(
            database.sync_status("default").unwrap().pending_mutations,
            1
        );
    }

    #[test]
    fn interrupted_delivery_is_recovered_on_open() {
        let path = std::env::temp_dir().join(format!("dispatch-{}.sqlite", Uuid::new_v4()));
        {
            let database = Database::open(&path).unwrap();
            database
                .mutate_thread(&ThreadMutation::Star {
                    thread_id: "welcome".into(),
                    value: true,
                })
                .unwrap();
            database
                .connection()
                .unwrap()
                .execute("UPDATE mutations SET state = 'running'", [])
                .unwrap();
        }
        let reopened = Database::open(&path).unwrap();
        assert_eq!(reopened.claim_mutations("default", 10).unwrap().len(), 1);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    }

    #[test]
    fn interrupted_full_sync_cannot_reuse_the_previous_cursor() {
        let database = database();
        database.finish_sync("default", "old-cursor").unwrap();
        database.begin_full_sync("default").unwrap();
        assert_eq!(database.cursor("default").unwrap(), None);
    }

    #[test]
    fn adopting_an_account_creates_it_and_rewrites_legacy_default_state() {
        let database = database();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();

        let account = database.adopt_account("you@gmail.com").unwrap();
        assert_eq!(account.email, "you@gmail.com");
        assert_eq!(account.status, "connected");
        assert_eq!(account.sort_order, 0);

        let connection = database.connection().unwrap();
        let sync_account_id: String = connection
            .query_row(
                "SELECT account_id FROM sync_state WHERE account_id = 'you@gmail.com'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sync_account_id, "you@gmail.com");
        let mutation_account_id: String = connection
            .query_row("SELECT account_id FROM mutations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mutation_account_id, "you@gmail.com");
    }

    #[test]
    fn adopting_the_same_account_twice_does_not_duplicate_it() {
        let database = database();
        database.adopt_account("you@gmail.com").unwrap();
        database.adopt_account("you@gmail.com").unwrap();
        assert_eq!(database.list_accounts().unwrap().len(), 1);
    }

    #[test]
    fn accounts_get_increasing_sort_order_and_rotating_colors() {
        let database = database();
        let first = database.adopt_account("first@gmail.com").unwrap();
        let second = database.adopt_account("second@gmail.com").unwrap();
        assert_eq!(first.sort_order, 0);
        assert_eq!(second.sort_order, 1);
        assert_ne!(first.color, second.color);
    }

    #[test]
    fn removing_an_account_deletes_its_row() {
        let database = database();
        database.adopt_account("you@gmail.com").unwrap();
        database.remove_account("you@gmail.com").unwrap();
        assert!(database.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn set_account_color_updates_an_existing_account_and_rejects_an_unknown_one() {
        let database = database();
        database.adopt_account("you@gmail.com").unwrap();
        database
            .set_account_color("you@gmail.com", "#123456")
            .unwrap();
        assert_eq!(
            database
                .get_account("you@gmail.com")
                .unwrap()
                .unwrap()
                .color,
            "#123456"
        );
        assert!(database
            .set_account_color("missing@gmail.com", "#123456")
            .is_err());
    }

    #[test]
    fn sender_display_name_can_be_saved_cleared_and_cannot_inject_headers() {
        let database = database();
        database.adopt_account("you@gmail.com").unwrap();
        database
            .set_account_display_name("you@gmail.com", Some("  Joel Reed  "))
            .unwrap();
        assert_eq!(
            database
                .get_account("you@gmail.com")
                .unwrap()
                .unwrap()
                .display_name
                .as_deref(),
            Some("Joel Reed")
        );

        assert!(database
            .set_account_display_name("you@gmail.com", Some("Joel\r\nBcc: attacker@example.com"))
            .is_err());
        database
            .set_account_display_name("you@gmail.com", Some("  "))
            .unwrap();
        assert_eq!(
            database
                .get_account("you@gmail.com")
                .unwrap()
                .unwrap()
                .display_name,
            None
        );
        assert!(database
            .set_account_display_name("missing@gmail.com", Some("Nobody"))
            .is_err());
    }

    #[test]
    fn reorder_accounts_updates_sort_order_by_position() {
        let database = database();
        database.adopt_account("first@gmail.com").unwrap();
        database.adopt_account("second@gmail.com").unwrap();
        database
            .reorder_accounts(&["second@gmail.com".into(), "first@gmail.com".into()])
            .unwrap();
        let accounts = database.list_accounts().unwrap();
        assert_eq!(accounts[0].email, "second@gmail.com");
        assert_eq!(accounts[1].email, "first@gmail.com");
    }

    fn test_thread(participants: &[&str], labels: &[&str]) -> Thread {
        Thread {
            id: "t1".into(),
            provider_thread_id: "p1".into(),
            subject: "Hi".into(),
            snippet: "".into(),
            participants: participants.iter().map(|value| value.to_string()).collect(),
            last_message_at: "".into(),
            last_received_at: "".into(),
            unread: false,
            starred: false,
            archived: false,
            trashed: false,
            labels: labels.iter().map(|value| value.to_string()).collect(),
            account_id: "default".into(),
            match_snippet: None,
            summary: None,
            summary_generated_at: None,
            has_attachments: false,
        }
    }

    fn test_rule(match_kind: &str, match_value: &str) -> SplitInbox {
        SplitInbox {
            id: "s1".into(),
            name: "Test".into(),
            match_kind: match_kind.into(),
            match_value: match_value.into(),
            sort_order: 0,
            created_at: "".into(),
        }
    }

    #[test]
    fn split_inbox_matches_covers_domain_label_and_pattern_rules() {
        let thread = test_thread(&["Jane Doe <jane@Acme.com>"], &["IMPORTANT"]);

        assert!(split_inbox_matches(&test_rule("domain", "acme.com"), &thread));
        assert!(!split_inbox_matches(&test_rule("domain", "other.com"), &thread));

        assert!(split_inbox_matches(&test_rule("label", "IMPORTANT"), &thread));
        assert!(!split_inbox_matches(&test_rule("label", "STARRED"), &thread));

        assert!(split_inbox_matches(&test_rule("pattern", "jane@"), &thread));
        assert!(!split_inbox_matches(&test_rule("pattern", "john@"), &thread));
    }

    #[test]
    fn create_update_delete_and_reorder_split_inboxes() {
        let database = database();
        let acme = database
            .create_split_inbox("Acme", "domain", "Acme.com")
            .unwrap();
        assert_eq!(acme.match_value, "acme.com", "domain values are lowercased");
        let widgets = database
            .create_split_inbox("Widgets Co", "pattern", "widgets")
            .unwrap();

        let listed = database.list_split_inboxes().unwrap();
        assert_eq!(listed.iter().map(|s| &s.name).collect::<Vec<_>>(), vec!["Acme", "Widgets Co"]);

        let renamed = database.update_split_inbox(&acme.id, "Acme Corp").unwrap();
        assert_eq!(renamed.name, "Acme Corp");
        assert_eq!(renamed.match_value, "acme.com", "rename leaves the rule untouched");

        database
            .reorder_split_inboxes(&[widgets.id.clone(), acme.id.clone()])
            .unwrap();
        let reordered = database.list_split_inboxes().unwrap();
        assert_eq!(reordered[0].id, widgets.id);
        assert_eq!(reordered[1].id, acme.id);

        database.delete_split_inbox(&widgets.id).unwrap();
        assert_eq!(database.list_split_inboxes().unwrap().len(), 1);

        assert!(database.create_split_inbox("", "domain", "acme.com").is_err());
        assert!(database.create_split_inbox("Acme", "domain", "").is_err());
        assert!(database.create_split_inbox("Acme", "bogus", "acme.com").is_err());
    }

    #[test]
    fn list_split_inbox_page_filters_the_inbox_and_paginates() {
        let database = database();
        let split_inbox = database
            .create_split_inbox("Inbox label", "label", "INBOX")
            .unwrap();

        let first_page = database
            .list_split_inbox_page(&split_inbox.id, None, 0, 1)
            .unwrap();
        assert_eq!(first_page.threads.len(), 1);
        assert!(first_page.has_more);

        let second_page = database
            .list_split_inbox_page(&split_inbox.id, None, 1, 1)
            .unwrap();
        assert_eq!(second_page.threads.len(), 1);
        assert!(!second_page.has_more);
        assert_ne!(first_page.threads[0].id, second_page.threads[0].id);

        assert!(database
            .list_split_inbox_page("missing", None, 0, 10)
            .is_err());
    }

    #[test]
    fn list_threads_page_excludes_threads_claimed_by_a_split_inbox() {
        let database = database();
        let before = database.list_threads_page(None, 0, 10).unwrap();
        assert_eq!(before.threads.len(), 2, "welcome and roadmap are both seeded, unclaimed by any split");

        // "roadmap"'s only participant is "Product Team" (see `insert_demo`),
        // which the `pattern` rule matches on the sender's normalized address.
        database.create_split_inbox("Product", "pattern", "product").unwrap();

        let after = database.list_threads_page(None, 0, 10).unwrap();
        assert_eq!(after.threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["welcome"]);
    }

    #[test]
    fn mailbox_unread_counts_buckets_unread_threads_by_split_and_excludes_them_from_inbox() {
        let database = database();
        let mut product_message = message(
            "product-message",
            "product-thread",
            "2026-01-02T00:00:00Z",
            "body",
        );
        product_message.from = "Team <team@product.example>".into();
        product_message.labels = vec!["INBOX".into(), "UNREAD".into()];
        database
            .upsert_gmail_thread("work@example.com", &[product_message])
            .unwrap();

        // "welcome" (seeded, unread) and the new product thread both count
        // toward the Inbox until a split inbox claims the latter.
        let before = database.mailbox_unread_counts(None).unwrap();
        assert_eq!(before.inbox, 2);
        assert!(before.splits.is_empty());

        let split = database
            .create_split_inbox("Product", "domain", "product.example")
            .unwrap();
        let after = database.mailbox_unread_counts(None).unwrap();
        assert_eq!(after.inbox, 1, "the product thread moved out of the Inbox bucket");
        assert_eq!(after.splits.get(&split.id), Some(&1));
    }

    #[test]
    fn thread_summary_round_trips_through_get_thread() {
        let database = database();
        let before = database.get_thread("welcome").unwrap().thread;
        assert_eq!(before.summary, None);
        assert_eq!(before.summary_generated_at, None);

        database
            .set_thread_summary(
                "welcome",
                "- Point one\n- Point two",
                "2026-03-05T16:30:00Z",
            )
            .unwrap();

        let after = database.get_thread("welcome").unwrap().thread;
        assert_eq!(after.summary.as_deref(), Some("- Point one\n- Point two"));
        assert_eq!(
            after.summary_generated_at.as_deref(),
            Some("2026-03-05T16:30:00Z")
        );
    }

    fn message(id: &str, thread_id: &str, date: &str, body: &str) -> NormalizedMessage {
        NormalizedMessage {
            id: id.into(),
            thread_id: thread_id.into(),
            subject: "Subject".into(),
            from: "sender@example.com".into(),
            to: vec!["recipient@example.com".into()],
            date: date.into(),
            body_html: String::new(),
            body_text: body.into(),
            snippet: body.into(),
            labels: vec!["INBOX".into()],
            metadata_json: "{}".into(),
            unsubscribe: None,
            attachments: vec![],
        }
    }

    #[test]
    fn attachment_metadata_sets_thread_flag_and_round_trips_on_message() {
        let database = database();
        let mut normalized = message(
            "attachment-message",
            "attachment-thread",
            "2026-01-01T00:00:00Z",
            "body",
        );
        normalized
            .attachments
            .push(crate::models::MessageAttachment {
                id: "gmail-attachment-id".into(),
                filename: "invoice.pdf".into(),
                mime_type: "application/pdf".into(),
                size: 42,
                content_id: None,
                inline: false,
            });
        database
            .upsert_gmail_thread("work@example.com", &[normalized])
            .unwrap();

        let detail = database
            .get_thread("work@example.com:attachment-thread")
            .unwrap();
        assert!(detail.thread.has_attachments);
        assert_eq!(detail.messages[0].attachments[0].filename, "invoice.pdf");
    }

    #[test]
    fn unsubscribe_metadata_is_exposed_and_attempts_are_recorded() {
        let database = database();
        let mut normalized = message(
            "newsletter-message",
            "newsletter",
            "2026-01-01T00:00:00Z",
            "body",
        );
        normalized.unsubscribe = Some(UnsubscribeMetadata {
            one_click_url: Some("https://lists.example/one-click".into()),
            mailto_url: Some("mailto:list@example.com?subject=unsubscribe".into()),
            web_url: Some("https://lists.example/preferences".into()),
            list_id: Some("news.example".into()),
        });
        database
            .upsert_gmail_thread("work@example.com", &[normalized])
            .unwrap();

        let detail = database.get_thread("work@example.com:newsletter").unwrap();
        let info = detail.messages[0].unsubscribe.as_ref().unwrap();
        assert_eq!(info.methods.len(), 3);
        assert_eq!(info.list_id.as_deref(), Some("news.example"));

        let target = database.begin_unsubscribe("newsletter-message").unwrap();
        assert!(matches!(target.method, UnsubscribeMethod::OneClick));
        assert_eq!(target.url, "https://lists.example/one-click");
        let pending: String = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT state FROM unsubscribe_requests WHERE id = ?1",
                [&target.request_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, "pending");

        database
            .finish_unsubscribe(&target.request_id, "succeeded", Some(204), None)
            .unwrap();
        let completed: (String, Option<i64>) = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT state, http_status FROM unsubscribe_requests WHERE id = ?1",
                [&target.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(completed, ("succeeded".into(), Some(204)));
    }

    #[test]
    fn two_accounts_with_the_same_provider_thread_id_stay_fully_separate() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message(
                    "work-msg",
                    "shared-id",
                    "2026-01-01T00:00:00Z",
                    "work body",
                )],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "personal@example.com",
                &[message(
                    "personal-msg",
                    "shared-id",
                    "2026-01-01T00:00:00Z",
                    "personal body",
                )],
            )
            .unwrap();

        let threads = database.list_threads(None).unwrap();
        let work = threads
            .iter()
            .find(|t| t.id == "work@example.com:shared-id")
            .unwrap();
        let personal = threads
            .iter()
            .find(|t| t.id == "personal@example.com:shared-id")
            .unwrap();
        assert_eq!(
            database.get_thread(&work.id).unwrap().messages[0].id,
            "work-msg"
        );
        assert_eq!(
            database.get_thread(&personal.id).unwrap().messages[0].id,
            "personal-msg"
        );

        database
            .delete_gmail_thread("work@example.com", "shared-id")
            .unwrap();
        let remaining = database.list_threads(None).unwrap();
        assert!(!remaining.iter().any(|t| t.id == work.id));
        assert!(remaining.iter().any(|t| t.id == personal.id));
    }

    #[test]
    fn full_sync_wipes_only_the_given_accounts_threads() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "personal@example.com",
                &[message("m2", "t2", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database.begin_full_sync("work@example.com").unwrap();
        let threads = database.list_threads(None).unwrap();
        assert!(!threads.iter().any(|t| t.id == "work@example.com:t1"));
        assert!(threads.iter().any(|t| t.id == "personal@example.com:t2"));
    }

    #[test]
    fn prune_expired_threads_is_a_noop_when_retention_is_unset() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        let removed = database.prune_expired_threads().unwrap();
        assert_eq!(removed, 0);
        assert!(database
            .list_threads(None)
            .unwrap()
            .iter()
            .any(|t| t.id == "work@example.com:t1"));
    }

    #[test]
    fn prune_expired_threads_removes_only_old_unstarred_threads() {
        let database = database();
        database.begin_full_sync("default").unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m2", "t2", &Utc::now().to_rfc3339(), "body")],
            )
            .unwrap();
        database.set_retention_days(Some(30)).unwrap();
        let removed = database.prune_expired_threads().unwrap();
        assert_eq!(removed, 1);
        let threads = database.list_threads(None).unwrap();
        assert!(!threads.iter().any(|t| t.id == "work@example.com:t1"));
        assert!(threads.iter().any(|t| t.id == "work@example.com:t2"));
    }

    #[test]
    fn prune_expired_threads_keeps_starred_threads_regardless_of_age() {
        let database = database();
        database.begin_full_sync("default").unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "work@example.com:t1".into(),
                value: true,
            })
            .unwrap();
        database.set_retention_days(Some(30)).unwrap();
        let removed = database.prune_expired_threads().unwrap();
        assert_eq!(removed, 0);
        assert!(database
            .list_threads(None)
            .unwrap()
            .iter()
            .any(|t| t.id == "work@example.com:t1"));
    }

    #[test]
    fn pruning_a_thread_also_removes_its_search_index_row() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database.set_retention_days(Some(30)).unwrap();
        database.prune_expired_threads().unwrap();
        let remaining: i64 = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM thread_search WHERE thread_id = 'work@example.com:t1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn message_bodies_round_trip_through_compression() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message(
                    "m1",
                    "t1",
                    "2026-01-01T00:00:00Z",
                    "Hello, this is the plaintext body ✓",
                )],
            )
            .unwrap();
        let detail = database.get_thread("work@example.com:t1").unwrap();
        assert_eq!(
            detail.messages[0].body_text,
            "Hello, this is the plaintext body ✓"
        );
        // Stored compressed, not as plaintext, in the legacy columns.
        let (body_html, body_text): (String, String) = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT body_html, body_text FROM messages WHERE id = 'm1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(body_html, "");
        assert_eq!(body_text, "");
    }

    #[test]
    fn legacy_uncompressed_bodies_still_read_back_correctly() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "placeholder")],
            )
            .unwrap();
        // Simulate a row written before the compression migration: only the
        // legacy plaintext columns are populated.
        database
            .connection()
            .unwrap()
            .execute(
                "UPDATE messages SET body_html = 'Legacy <b>html</b>', body_text = 'Legacy text',
                    body_html_z = NULL, body_text_z = NULL
                 WHERE id = 'm1'",
                [],
            )
            .unwrap();
        let detail = database.get_thread("work@example.com:t1").unwrap();
        assert_eq!(detail.messages[0].body_html, "Legacy <b>html</b>");
        assert_eq!(detail.messages[0].body_text, "Legacy text");
    }

    #[test]
    fn compress_next_body_batch_converts_legacy_rows_and_drains_to_zero() {
        let database = database();
        database.begin_full_sync("default").unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "placeholder")],
            )
            .unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "UPDATE messages SET body_html = 'Legacy <b>html</b>', body_text = 'Legacy text',
                    body_html_z = NULL, body_text_z = NULL
                 WHERE id = 'm1'",
                [],
            )
            .unwrap();
        let converted = database.compress_next_body_batch(500).unwrap();
        assert_eq!(converted, 1);
        assert_eq!(database.compress_next_body_batch(500).unwrap(), 0);
        let detail = database.get_thread("work@example.com:t1").unwrap();
        assert_eq!(detail.messages[0].body_html, "Legacy <b>html</b>");
        assert_eq!(detail.messages[0].body_text, "Legacy text");
    }

    #[test]
    fn claim_mutations_only_claims_the_given_accounts_mutations() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "personal@example.com",
                &[message("m2", "t2", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "work@example.com:t1".into(),
                value: true,
            })
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "personal@example.com:t2".into(),
                value: true,
            })
            .unwrap();

        let claimed = database.claim_mutations("work@example.com", 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].provider_thread_id, "t1");
    }

    #[test]
    fn sync_state_is_isolated_per_account() {
        let database = database();
        database.adopt_account("work@example.com").unwrap();
        database.adopt_account("personal@example.com").unwrap();
        database
            .finish_sync("work@example.com", "work-cursor")
            .unwrap();
        assert_eq!(
            database.cursor("work@example.com").unwrap().as_deref(),
            Some("work-cursor")
        );
        assert_eq!(database.cursor("personal@example.com").unwrap(), None);
    }

    #[test]
    fn finish_sync_stamps_the_account_last_synced_at() {
        let database = database();
        database.adopt_account("work@example.com").unwrap();
        database.adopt_account("personal@example.com").unwrap();
        database
            .finish_sync("work@example.com", "work-cursor")
            .unwrap();
        let accounts = database.list_accounts().unwrap();
        let work = accounts
            .iter()
            .find(|account| account.email == "work@example.com")
            .unwrap();
        assert!(work.last_synced_at.is_some());
        let personal = accounts
            .iter()
            .find(|account| account.email == "personal@example.com")
            .unwrap();
        assert_eq!(personal.last_synced_at, None);
    }

    #[test]
    fn list_threads_merges_by_default_and_filters_when_scoped() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "personal@example.com",
                &[message("m2", "t2", "2026-01-02T00:00:00Z", "body")],
            )
            .unwrap();

        let merged = database.list_threads(None).unwrap();
        assert!(merged.iter().any(|t| t.id == "work@example.com:t1"));
        assert!(merged.iter().any(|t| t.id == "personal@example.com:t2"));

        let scoped = database.list_threads(Some("work@example.com")).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "work@example.com:t1");
        assert_eq!(scoped[0].account_id, "work@example.com");
    }

    #[test]
    fn list_all_mail_excludes_trash_but_keeps_archived() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "inbox", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m2", "archived", "2026-01-02T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m3", "trashed", "2026-01-03T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "work@example.com:archived".into(),
                value: true,
            })
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Trash {
                thread_id: "work@example.com:trashed".into(),
                value: true,
            })
            .unwrap();

        let all_mail = database.list_all_mail(None).unwrap();
        let ids: Vec<_> = all_mail.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"work@example.com:inbox"));
        assert!(ids.contains(&"work@example.com:archived"));
        assert!(!ids.contains(&"work@example.com:trashed"));
    }

    #[test]
    fn list_trash_only_returns_trashed_threads() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "inbox", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m2", "trashed", "2026-01-02T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Trash {
                thread_id: "work@example.com:trashed".into(),
                value: true,
            })
            .unwrap();

        let trash = database.list_trash(None).unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, "work@example.com:trashed");

        let scoped = database.list_trash(Some("personal@example.com")).unwrap();
        assert!(scoped.is_empty());
    }
}
