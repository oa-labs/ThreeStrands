//! Stage 1 deterministic local SQLite candidate selection for Inbox Assistants.

use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};

use crate::agents::candidates::{
    CandidateBatch, CandidateMessage, CandidateThread, DeterministicProposal, RelatedEventContext,
    RelatedThreadContext,
};
use crate::agents::policy::*;
use crate::agents::{AgentId, InboxAgentConfig, ProposalEvidence, ProposalKind, ProposalPayload};
use crate::db::messages::resolve_body;
use crate::db::{decode_json, normalize_sender, sending_addresses_for_account, DbResult};
use crate::mime::readable_body_text;

const EXPIRY_KEYWORDS: &[&str] = &[
    "webinar",
    "expires",
    "expiring",
    "expiration",
    "deadline",
    "offer ends",
    "last chance",
    "ends today",
    "ends tonight",
    "rsvp",
    "register by",
    "valid until",
    "valid through",
    "verification code",
    "one-time password",
    "otp",
    "coupon",
];

const AUTOMATED_SENDER_PREFIXES: &[&str] = &[
    "noreply@",
    "no-reply@",
    "donotreply@",
    "do-not-reply@",
    "mailer-daemon@",
    "notifications@",
    "notification@",
    "bounce@",
    "bounces@",
    "automated@",
];

const BULK_LABELS: &[&str] = &[
    "CATEGORY_PROMOTIONS",
    "CATEGORY_UPDATES",
    "CATEGORY_SOCIAL",
    "CATEGORY_FORUMS",
    "SPAM",
];

fn parse_rfc3339_age_days(raw: &str, now_secs: i64) -> i64 {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| ((now_secs - dt.timestamp()).max(0)) / 86_400)
        .unwrap_or(0)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn is_automated_sender(email: &str) -> bool {
    let lower = email.to_ascii_lowercase();
    AUTOMATED_SENDER_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn has_bulk_label(labels: &[String]) -> bool {
    labels
        .iter()
        .any(|label| BULK_LABELS.iter().any(|bulk| label == bulk))
}

fn contains_expiry_clue(subject: &str, messages: &[CandidateMessage]) -> bool {
    let mut combined = subject.to_ascii_lowercase();
    for msg in messages.iter().rev().take(2) {
        combined.push(' ');
        combined.push_str(
            &msg.body_text
                .chars()
                .take(1_500)
                .collect::<String>()
                .to_ascii_lowercase(),
        );
    }
    EXPIRY_KEYWORDS.iter().any(|kw| combined.contains(kw))
}

struct ThreadRow {
    id: String,
    provider_thread_id: String,
    account_id: String,
    subject: String,
    participants: Vec<String>,
    last_message_at: String,
    last_received_at: String,
    labels: Vec<String>,
}

fn load_aging_inbox_threads(
    conn: &Connection,
    agent_id: AgentId,
    config: &InboxAgentConfig,
    cutoff_iso: &str,
    limit: usize,
) -> DbResult<Vec<ThreadRow>> {
    let mut sql = String::from(
        "SELECT t.id, t.provider_thread_id, t.account_id, t.subject,
                t.participants_json, t.last_message_at,
                COALESCE(NULLIF(t.last_received_at, ''), t.last_message_at) AS received_at,
                t.labels_json
         FROM threads t
         WHERE t.archived = 0
           AND t.trashed = 0
           AND NOT EXISTS (
               SELECT 1 FROM conversation_snoozes s
               WHERE s.id = t.id AND json_extract(s.record_json, '$.state') = 'active'
           )
           AND NOT EXISTS (
               SELECT 1 FROM inbox_agent_proposals p
               WHERE p.agent_id = ?1
                 AND p.thread_id = t.id
                 AND p.thread_revision = t.last_message_at
                 AND p.state IN ('pending', 'dismissed')
           )
           AND COALESCE(NULLIF(t.last_received_at, ''), t.last_message_at) <= ?2",
    );
    if !config.include_starred {
        sql.push_str(" AND t.starred = 0");
    }
    if config.account_id.is_some() {
        sql.push_str(" AND t.account_id = ?3");
    }
    sql.push_str(" ORDER BY received_at DESC LIMIT ");
    sql.push_str(&limit.to_string());

    let mut stmt = conn.prepare(&sql)?;
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok(ThreadRow {
            id: row.get(0)?,
            provider_thread_id: row.get(1)?,
            account_id: row.get(2)?,
            subject: row.get(3)?,
            participants: decode_json(row.get::<_, String>(4)?)?,
            last_message_at: row.get(5)?,
            last_received_at: row.get(6)?,
            labels: decode_json(row.get::<_, String>(7)?)?,
        })
    };

    let rows = if let Some(account_id) = config.account_id.as_deref() {
        stmt.query_map(params![agent_id.as_str(), cutoff_iso, account_id], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![agent_id.as_str(), cutoff_iso], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows)
}

fn load_thread_messages(
    conn: &Connection,
    thread_id: &str,
    own_addresses: &BTreeSet<String>,
) -> DbResult<Vec<CandidateMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, sender, recipients_json, sent_at, body_text, body_text_z, unsubscribe_json
         FROM messages
         WHERE thread_id = ?1
         ORDER BY sent_at ASC",
    )?;
    let rows = stmt
        .query_map([thread_id], |row| {
            let sender: String = row.get(1)?;
            let body = readable_body_text(resolve_body(5, row.get(4)?, row.get(5)?)?);
            let unsub: Option<String> = row.get(6)?;
            Ok(CandidateMessage {
                id: row.get(0)?,
                is_own: own_addresses.contains(&normalize_sender(&sender).0),
                sender,
                recipients: decode_json(row.get::<_, String>(2)?)?,
                sent_at: row.get(3)?,
                body_text: body,
                has_unsubscribe: unsub.is_some(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() <= MAX_CANDIDATE_MESSAGES {
        Ok(rows
            .into_iter()
            .map(|mut msg| {
                msg.body_text = truncate_chars(&msg.body_text, MAX_CANDIDATE_BODY_CHARS);
                msg
            })
            .collect())
    } else {
        let start = rows.len() - MAX_CANDIDATE_MESSAGES;
        Ok(rows
            .into_iter()
            .skip(start)
            .map(|mut msg| {
                msg.body_text = truncate_chars(&msg.body_text, MAX_CANDIDATE_BODY_CHARS);
                msg
            })
            .collect())
    }
}

fn completed_task_for_thread(
    conn: &Connection,
    thread_id: &str,
    last_received_at: &str,
) -> DbResult<Option<(String, String, String)>> {
    let has_active_task: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM tasks
             WHERE thread_id = ?1
               AND status IN ('open', 'in_progress')
         )",
        params![thread_id],
        |row| row.get(0),
    )?;
    if has_active_task {
        return Ok(None);
    }

    let row = conn.query_row(
        "SELECT id, title, COALESCE(completed_at, updated_at) AS done_at
         FROM tasks
         WHERE thread_id = ?1
           AND status = 'completed'
           AND completion_source = 'user'
           AND completed_by_email IS NULL
         ORDER BY done_at DESC
         LIMIT 1",
        params![thread_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    );
    let found = match row {
        Ok(found) => found,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let completed_after = match (
        DateTime::parse_from_rfc3339(&found.2),
        DateTime::parse_from_rfc3339(last_received_at),
    ) {
        (Ok(done_dt), Ok(received_dt)) => done_dt > received_dt,
        _ => found.2.as_str() > last_received_at,
    };
    if completed_after {
        Ok(Some(found))
    } else {
        Ok(None)
    }
}

fn find_related_threads(
    conn: &Connection,
    thread: &ThreadRow,
    own_addresses: &BTreeSet<String>,
) -> DbResult<Vec<RelatedThreadContext>> {
    let external_emails: HashSet<String> = thread
        .participants
        .iter()
        .map(|p| normalize_sender(p).0)
        .filter(|e| !e.is_empty() && !own_addresses.contains(e) && !is_automated_sender(e))
        .collect();
    if external_emails.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT id, subject, last_message_at, snippet, participants_json, labels_json
         FROM threads
         WHERE account_id = ?1
           AND id != ?2
           AND trashed = 0
           AND last_message_at > ?3
         ORDER BY last_message_at DESC
         LIMIT 25",
    )?;
    let mut matches = Vec::new();
    let rows = stmt.query_map(
        params![thread.account_id, thread.id, thread.last_received_at],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                decode_json(row.get::<_, String>(4)?)?,
                decode_json(row.get::<_, String>(5)?)?,
            ))
        },
    )?;
    for row in rows {
        let (id, subject, last_message_at, snippet, participants, labels) = row?;
        let shares_external = participants
            .iter()
            .map(|p| normalize_sender(p).0)
            .any(|email| external_emails.contains(&email));
        if !shares_external {
            continue;
        }
        let user_replied = labels.iter().any(|l| l == "SENT");
        if !user_replied {
            continue;
        }
        matches.push(RelatedThreadContext {
            thread_id: id,
            subject,
            last_message_at,
            snippet: truncate_chars(&snippet, 220),
            user_replied,
        });
        if matches.len() >= MAX_RELATED_THREADS {
            break;
        }
    }
    Ok(matches)
}

fn find_related_events(
    conn: &Connection,
    thread: &ThreadRow,
    own_addresses: &BTreeSet<String>,
    now_iso: &str,
) -> DbResult<Vec<RelatedEventContext>> {
    let external_emails: HashSet<String> = thread
        .participants
        .iter()
        .map(|p| normalize_sender(p).0)
        .filter(|e| !e.is_empty() && !own_addresses.contains(e) && !is_automated_sender(e))
        .collect();
    if external_emails.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT id, start, end, body
         FROM note_event_cache
         WHERE end < ?1 AND start > ?2
         ORDER BY end DESC
         LIMIT 40",
    )?;
    let mut matches = Vec::new();
    let rows = stmt.query_map(params![now_iso, thread.last_received_at], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (event_id, start, end, body) = row?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        let attendee_hit = value["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|att| att["email"].as_str())
            .any(|email| external_emails.contains(&email.to_ascii_lowercase()));
        if !attendee_hit {
            continue;
        }
        let title = value["summary"]
            .as_str()
            .or_else(|| value["title"].as_str())
            .unwrap_or("Calendar event")
            .to_string();
        matches.push(RelatedEventContext {
            event_id,
            title,
            start,
            end,
        });
        if matches.len() >= MAX_RELATED_EVENTS {
            break;
        }
    }
    Ok(matches)
}

fn thread_has_draft(
    conn: &Connection,
    thread_id: &str,
    provider_thread_id: &str,
) -> DbResult<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM drafts
             WHERE json_valid(payload)
               AND json_extract(payload, '$.threadId') IN (?1, ?2)
         )",
        params![thread_id, provider_thread_id],
        |row| row.get(0),
    )?;
    Ok(exists)
}

pub(super) fn select_candidates(
    conn: &Connection,
    config: &InboxAgentConfig,
    now_secs: i64,
) -> DbResult<CandidateBatch> {
    let now_dt = DateTime::<Utc>::from_timestamp(now_secs, 0).unwrap_or_else(Utc::now);
    let now_iso = now_dt.to_rfc3339();
    let cutoff_iso = (now_dt - Duration::days(config.age_days.max(1))).to_rfc3339();
    let max_out = config
        .max_threads_per_run
        .clamp(MIN_THREADS_PER_RUN, MAX_THREADS_PER_RUN) as usize;
    let pool = load_aging_inbox_threads(conn, config.agent_id, config, &cutoff_iso, max_out * 4)?;

    let mut own_cache: std::collections::HashMap<String, BTreeSet<String>> =
        std::collections::HashMap::new();
    let mut batch = CandidateBatch {
        deterministic: Vec::new(),
        ai_candidates: Vec::new(),
    };

    for thread in pool {
        if batch.deterministic.len() + batch.ai_candidates.len() >= max_out {
            break;
        }
        let own = own_cache
            .entry(thread.account_id.clone())
            .or_insert_with(|| sending_addresses_for_account(conn, &thread.account_id))
            .clone();
        let messages = load_thread_messages(conn, &thread.id, &own)?;
        if messages.is_empty() {
            continue;
        }
        let age_days = parse_rfc3339_age_days(&thread.last_received_at, now_secs);
        let has_unsubscribe = messages.iter().any(|m| m.has_unsubscribe);

        match config.agent_id {
            AgentId::StaleReads => {
                let any_own = messages.iter().any(|m| m.is_own);
                if any_own {
                    continue;
                }
                let total_chars: usize = messages.iter().map(|m| m.body_text.chars().count()).sum();
                let is_bulk_read = has_unsubscribe
                    || thread
                        .labels
                        .iter()
                        .any(|l| l == "CATEGORY_UPDATES" || l == "CATEGORY_PROMOTIONS");
                if !is_bulk_read && total_chars < STALE_READ_MIN_CHARS {
                    continue;
                }
                batch.ai_candidates.push(CandidateThread {
                    thread_id: thread.id,
                    account_id: thread.account_id,
                    subject: thread.subject,
                    last_message_at: thread.last_message_at,
                    age_days,
                    has_unsubscribe,
                    messages,
                    related_threads: Vec::new(),
                    related_events: Vec::new(),
                });
            }
            AgentId::ResolvedAndExpired => {
                if let Some((task_id, task_title, completed_at)) =
                    completed_task_for_thread(conn, &thread.id, &thread.last_received_at)?
                {
                    batch.deterministic.push(DeterministicProposal {
                        account_id: thread.account_id,
                        thread_id: thread.id,
                        thread_revision: thread.last_message_at,
                        kind: ProposalKind::ArchiveAsResolved,
                        headline: format!("Completed task \"{task_title}\""),
                        reason: format!(
                            "You marked the linked task \"{task_title}\" done after this email arrived."
                        ),
                        payload: ProposalPayload {
                            resolved_by_kind: Some("task".into()),
                            resolved_by_id: Some(task_id.clone()),
                            task_completion_source: Some("user".into()),
                            ..Default::default()
                        },
                        evidence: vec![ProposalEvidence {
                            kind: "task".into(),
                            id: task_id,
                            label: task_title,
                            excerpt: Some(format!("Marked done {completed_at}")),
                        }],
                    });
                    continue;
                }
                let related_threads = find_related_threads(conn, &thread, &own)?;
                let related_events = find_related_events(conn, &thread, &own, &now_iso)?;
                let has_expiry_clues = contains_expiry_clue(&thread.subject, &messages);
                if related_threads.is_empty() && related_events.is_empty() && !has_expiry_clues {
                    continue;
                }
                batch.ai_candidates.push(CandidateThread {
                    thread_id: thread.id,
                    account_id: thread.account_id,
                    subject: thread.subject,
                    last_message_at: thread.last_message_at,
                    age_days,
                    has_unsubscribe,
                    messages,
                    related_threads,
                    related_events,
                });
            }
            AgentId::AgingReplies => {
                let latest = messages.last().unwrap();
                if latest.is_own || has_unsubscribe || has_bulk_label(&thread.labels) {
                    continue;
                }
                let latest_email = normalize_sender(&latest.sender).0;
                if is_automated_sender(&latest_email) {
                    continue;
                }
                if thread_has_draft(conn, &thread.id, &thread.provider_thread_id)? {
                    continue;
                }
                if completed_task_for_thread(conn, &thread.id, &thread.last_received_at)?.is_some()
                {
                    continue;
                }
                batch.ai_candidates.push(CandidateThread {
                    thread_id: thread.id,
                    account_id: thread.account_id,
                    subject: thread.subject,
                    last_message_at: thread.last_message_at,
                    age_days,
                    has_unsubscribe,
                    messages,
                    related_threads: Vec::new(),
                    related_events: Vec::new(),
                });
            }
        }
    }

    Ok(batch)
}
