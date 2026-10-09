//! Local triage observation persistence.

use super::DatabaseError;
use super::{normalize_sender, Database, DbResult};
use crate::models::{TriageAction, TriageContext, TriageEvent, TriageEventKind, TriageSenderStats};
use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

impl Database {
    /// Resolves sender identity from cached mail rather than trusting input.
    pub fn record_triage_event(&self, event: &TriageEvent) -> DbResult<()> {
        match (&event.kind, &event.action) {
            (
                TriageEventKind::Open | TriageEventKind::Close | TriageEventKind::Response,
                Some(_),
            ) => return Err(DatabaseError::invalid("Open and close triage events cannot have an action")),
            (TriageEventKind::Disposition | TriageEventKind::Restore, None) => {
                return Err(DatabaseError::invalid("Disposition and restore triage events require an action"))
            }
            _ => {}
        }
        self.with_transaction(|transaction| {
            let Some((account_id, sender_email, sender_domain)) =
                sender_identity_for_thread(transaction, &event.thread_id)?
            else {
                // Nothing was written; committing the read-only transaction
                // is equivalent to the rollback-on-drop this replaced.
                return Ok(());
            };
            let dwell_ms = event.dwell_ms.map(|value| value.clamp(0, 86_400_000));
            transaction.execute(
                "INSERT INTO triage_events(
                    id, account_id, thread_id, sender_email, sender_domain,
                    event_kind, context, action, opened, dwell_ms, scrolled,
                    batch, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    Uuid::new_v4().to_string(),
                    account_id,
                    event.thread_id,
                    sender_email,
                    sender_domain,
                    triage_event_kind_name(&event.kind),
                    triage_context_name(&event.context),
                    event.action.as_ref().map(triage_action_name),
                    event.opened,
                    dwell_ms,
                    event.scrolled,
                    event.batch,
                    Utc::now().to_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }

    /// Returns the current top sender candidates for one account. This is a
    /// derived view over raw events: the limit is intentionally bounded for a
    /// future UI, while the underlying observations remain available locally.
    pub fn list_triage_sender_stats(
        &self,
        account_id: &str,
        limit: usize,
    ) -> DbResult<Vec<TriageSenderStats>> {
        self.with_connection(|connection| {
            let limit = limit.clamp(1, 100) as i64;
            let mut statement = connection.prepare(
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
            )?;
            let rows = statement.query_map(params![account_id, limit], |row| {
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
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

fn sender_identity_for_thread(
    transaction: &Transaction<'_>,
    thread_id: &str,
) -> DbResult<Option<(String, String, String)>> {
    let account_id: Option<String> = transaction
        .query_row(
            "SELECT account_id FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(account_id) = account_id else {
        return Ok(None);
    };

    let mut statement = transaction.prepare(
        "SELECT sender FROM messages
             WHERE thread_id = ?1
             ORDER BY sent_at DESC, id DESC",
    )?;
    let rows = statement.query_map([thread_id], |row| row.get::<_, String>(0))?;
    let account_email = normalize_sender(&account_id).0;
    let mut fallback = None;
    for row in rows {
        let sender = row?;
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

#[cfg(test)]
#[path = "tests/triage.rs"]
mod tests;
