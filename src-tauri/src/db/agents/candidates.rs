//! Stage 1 deterministic local SQLite candidate selection for Inbox Assistants.

use std::collections::{BTreeSet, HashMap, HashSet};

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
    if config.agent_id == AgentId::SenderHygiene {
        return select_sender_hygiene(conn, config, now_secs);
    }
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
            AgentId::SenderHygiene => continue,
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

/// One of a sender's conversations still in the Inbox.
struct HygieneThread {
    id: String,
    subject: String,
    snippet: String,
    last_message_at: String,
    last_received_at: String,
    unread: bool,
    /// Newest message in the conversation from this sender that offers unsubscribing.
    unsubscribe_message_id: Option<String>,
}

/// Everything known about one (account, sender) pair.
#[derive(Default)]
struct HygieneSender {
    name: String,
    threads: Vec<HygieneThread>,
}

#[derive(Default)]
struct TriageCounts {
    quick: i64,
    dispositions: i64,
    restores: i64,
    responses: i64,
}

fn display_name_of(sender: &str, email: &str) -> String {
    let name = sender
        .split('<')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim();
    if name.is_empty() || name.contains('@') {
        email.to_string()
    } else {
        name.to_string()
    }
}

fn plural(count: i64, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Senders the user keeps ignoring: unread mail left to age in the Inbox, or
/// mail dismissed within a second. Local and zero-token, so every proposal is
/// deterministic and no AI provider is involved.
fn select_sender_hygiene(
    conn: &Connection,
    config: &InboxAgentConfig,
    now_secs: i64,
) -> DbResult<CandidateBatch> {
    let now_dt = DateTime::<Utc>::from_timestamp(now_secs, 0).unwrap_or_else(Utc::now);
    let age_cutoff = (now_dt - Duration::days(config.age_days.max(1))).to_rfc3339();
    let lookback = (now_dt - Duration::days(HYGIENE_LOOKBACK_DAYS)).to_rfc3339();
    let dismissed_since = now_secs - HYGIENE_DISMISSED_COOLDOWN_DAYS * 86_400;
    let max_out = config
        .max_threads_per_run
        .clamp(MIN_THREADS_PER_RUN, MAX_THREADS_PER_RUN) as usize;
    let mut batch = CandidateBatch {
        deterministic: Vec::new(),
        ai_candidates: Vec::new(),
    };

    // The account's Inbox conversations, by thread.
    let mut sql = String::from(
        "SELECT t.id, t.account_id, t.subject, t.snippet, t.last_message_at,
                COALESCE(NULLIF(t.last_received_at, ''), t.last_message_at), t.unread
         FROM threads t
         WHERE t.archived = 0 AND t.trashed = 0
           AND NOT EXISTS (
               SELECT 1 FROM conversation_snoozes s
               WHERE s.id = t.id AND json_extract(s.record_json, '$.state') = 'active'
           )",
    );
    if !config.include_starred {
        sql.push_str(" AND t.starred = 0");
    }
    if config.account_id.is_some() {
        sql.push_str(" AND t.account_id = ?1");
    }
    let mut stmt = conn.prepare(&sql)?;
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            HygieneThread {
                id: row.get(0)?,
                subject: row.get(2)?,
                snippet: row.get(3)?,
                last_message_at: row.get(4)?,
                last_received_at: row.get(5)?,
                unread: row.get::<_, i64>(6)? != 0,
                unsubscribe_message_id: None,
            },
        ))
    };
    let thread_rows = if let Some(account_id) = config.account_id.as_deref() {
        stmt.query_map([account_id], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    if thread_rows.is_empty() {
        return Ok(batch);
    }

    // Messages of those conversations, newest first, to attribute each one to
    // its sender the way triage does: the newest message not sent by the user.
    let mut messages_by_thread: HashMap<String, Vec<(String, String, String, bool)>> =
        HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.thread_id, m.id, m.sender, m.sent_at, m.unsubscribe_json IS NOT NULL
             FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE t.archived = 0 AND t.trashed = 0
             ORDER BY m.sent_at DESC, m.id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? != 0,
            ))
        })?;
        for row in rows {
            let (thread_id, id, sender, sent_at, unsub) = row?;
            messages_by_thread
                .entry(thread_id)
                .or_default()
                .push((id, sender, sent_at, unsub));
        }
    }

    let mut own_cache: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut senders: HashMap<(String, String), HygieneSender> = HashMap::new();
    for (thread_id, account_id, thread) in thread_rows {
        let Some(messages) = messages_by_thread.get(&thread_id) else {
            continue;
        };
        let own = own_cache
            .entry(account_id.clone())
            .or_insert_with(|| sending_addresses_for_account(conn, &account_id));
        let attributed = messages
            .iter()
            .map(|(_, sender, _, _)| (sender, normalize_sender(sender).0))
            .find(|(_, email)| !email.is_empty() && (own.is_empty() || !own.contains(email)));
        // A sent-only conversation has no sender to judge.
        let Some((raw_sender, email)) = attributed else {
            continue;
        };
        let entry = senders.entry((account_id, email.clone())).or_default();
        if entry.name.is_empty() {
            entry.name = display_name_of(raw_sender, &email);
        }
        // Messages are newest first, so the first match is the newest.
        let mut thread = thread;
        thread.unsubscribe_message_id = messages
            .iter()
            .find(|(_, sender, _, unsub)| *unsub && normalize_sender(sender).0 == email)
            .map(|(id, _, _, _)| id.clone());
        entry.threads.push(thread);
    }

    // Triage telemetry in the lookback window. Quick dispositions mirror
    // `list_triage_sender_stats` and `isQuickTriageDisposition` in triage.ts.
    let mut triage: HashMap<(String, String), TriageCounts> = HashMap::new();
    {
        let mut sql = String::from(
            "SELECT account_id, sender_email,
                    SUM(CASE WHEN event_kind = 'disposition' AND context = 'inbox'
                                  AND action IN ('archive', 'trash')
                                  AND opened = 1 AND batch = 0 AND scrolled = 0
                                  AND dwell_ms BETWEEN 0 AND 1000
                             THEN 1 ELSE 0 END),
                    SUM(CASE WHEN event_kind = 'disposition' AND context = 'inbox'
                             THEN 1 ELSE 0 END),
                    SUM(CASE WHEN event_kind = 'restore' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN event_kind = 'response' THEN 1 ELSE 0 END)
             FROM triage_events
             WHERE created_at >= ?1",
        );
        if config.account_id.is_some() {
            sql.push_str(" AND account_id = ?2");
        }
        sql.push_str(" GROUP BY account_id, sender_email");
        let mut stmt = conn.prepare(&sql)?;
        let map = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                TriageCounts {
                    quick: row.get(2)?,
                    dispositions: row.get(3)?,
                    restores: row.get(4)?,
                    responses: row.get(5)?,
                },
            ))
        };
        let rows = if let Some(account_id) = config.account_id.as_deref() {
            stmt.query_map(params![lookback, account_id], map)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![lookback], map)?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (account, email, counts) in rows {
            triage.insert((account, email), counts);
        }
    }

    // Mute and block rules, and suggestions already pending or recently dismissed.
    let mut rules: HashMap<String, (HashSet<String>, HashSet<String>)> = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT lower(account_id), match_kind, match_value FROM sender_rules")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (account, kind, value) = row?;
            let entry = rules.entry(account).or_default();
            if kind == "domain" {
                entry.1.insert(value);
            } else {
                entry.0.insert(value);
            }
        }
    }
    let mut already: HashSet<(String, String)> = HashSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT account_id, json_extract(payload_json, '$.senderEmail')
             FROM inbox_agent_proposals
             WHERE agent_id = ?1
               AND json_extract(payload_json, '$.senderEmail') IS NOT NULL
               AND (state = 'pending' OR (state = 'dismissed' AND updated_at >= ?2))",
        )?;
        let rows = stmt.query_map(
            params![AgentId::SenderHygiene.as_str(), dismissed_since],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        for row in rows {
            already.insert(row?);
        }
    }

    struct Qualified {
        account_id: String,
        email: String,
        sender: HygieneSender,
        aging: i64,
        quick: i64,
        dispositions: i64,
    }
    let mut qualified: Vec<Qualified> = Vec::new();
    for ((account_id, email), sender) in senders {
        let counts = triage.get(&(account_id.clone(), email.clone()));
        let (quick, dispositions) = counts.map_or((0, 0), |c| (c.quick, c.dispositions));
        if counts.is_some_and(|c| c.responses > 0 || c.restores > 0) {
            continue;
        }
        let aging = sender
            .threads
            .iter()
            .filter(|t| t.unread && t.last_received_at <= age_cutoff)
            .count() as i64;
        let by_dismissal = quick >= HYGIENE_MIN_QUICK_DISMISSALS
            && dispositions > 0
            && quick as f64 / dispositions as f64 >= HYGIENE_MIN_QUICK_RATE;
        if !by_dismissal && aging < HYGIENE_MIN_AGING_UNREAD {
            continue;
        }
        let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
        if let Some((addresses, domains)) = rules.get(&account_id.to_lowercase()) {
            if addresses.contains(&email) || domains.contains(domain) {
                continue;
            }
        }
        if already.contains(&(account_id.clone(), email.clone())) {
            continue;
        }
        if sending_addresses_for_account(conn, &account_id).contains(&email) {
            continue;
        }
        let written_to: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM contact_interactions
                 WHERE direction = 'sent' AND email = ?1 AND account_id = lower(trim(?2)))",
            params![email, account_id],
            |row| row.get(0),
        )?;
        if written_to {
            continue;
        }
        qualified.push(Qualified {
            account_id,
            email,
            sender,
            aging,
            quick,
            dispositions,
        });
    }
    qualified.sort_by(|a, b| {
        b.quick
            .cmp(&a.quick)
            .then(b.aging.cmp(&a.aging))
            .then_with(|| a.account_id.cmp(&b.account_id))
            .then_with(|| a.email.cmp(&b.email))
    });

    for mut item in qualified {
        if batch.deterministic.len() >= max_out {
            break;
        }
        item.sender.threads.sort_by(|a, b| {
            b.last_message_at
                .cmp(&a.last_message_at)
                .then(b.id.cmp(&a.id))
        });
        // Anchor on the newest conversation, preferring the newest that offers
        // unsubscribing so the proposal's message opens with its conversation.
        let anchor_index = item
            .sender
            .threads
            .iter()
            .position(|t| t.unsubscribe_message_id.is_some())
            .unwrap_or(0);
        let anchor = &item.sender.threads[anchor_index];
        // An anchor revision already proposed or dismissed would be ignored on
        // save; skip it so it does not use up a slot in this run.
        let seen: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM inbox_agent_proposals
                 WHERE agent_id = ?1 AND thread_id = ?2 AND thread_revision = ?3
                   AND state IN ('pending', 'dismissed'))",
            params![
                AgentId::SenderHygiene.as_str(),
                anchor.id,
                anchor.last_message_at
            ],
            |row| row.get(0),
        )?;
        if seen {
            continue;
        }
        let mut reasons = Vec::new();
        if item.quick > 0 && item.dispositions > 0 {
            reasons.push(format!(
                "Dismissed {} of {} within a second",
                item.quick,
                plural(item.dispositions, "email")
            ));
        }
        if item.aging > 0 {
            reasons.push(format!(
                "{} older than {}",
                plural(item.aging, "unread email"),
                plural(config.age_days.max(1), "day")
            ));
        }
        let reason = format!("{}.", reasons.join("; "));
        let message_id = anchor.unsubscribe_message_id.clone();
        let recommendation = if message_id.is_some() {
            "unsubscribe"
        } else {
            "mute"
        };
        batch.deterministic.push(DeterministicProposal {
            account_id: item.account_id.clone(),
            thread_id: anchor.id.clone(),
            thread_revision: anchor.last_message_at.clone(),
            kind: ProposalKind::SenderHygiene,
            headline: format!("You usually skip {}", item.sender.name),
            reason,
            payload: ProposalPayload {
                sender_email: Some(item.email.clone()),
                sender_name: Some(item.sender.name.clone()),
                aging_unread_count: Some(item.aging),
                quick_dismiss_count: Some(item.quick),
                dismiss_count: Some(item.dispositions),
                inbox_thread_ids: Some(
                    item.sender
                        .threads
                        .iter()
                        .take(HYGIENE_MAX_INBOX_THREADS)
                        .map(|t| t.id.clone())
                        .collect(),
                ),
                recommendation: Some(recommendation.into()),
                message_id,
                ..Default::default()
            },
            evidence: item
                .sender
                .threads
                .iter()
                .take(HYGIENE_EVIDENCE_THREADS)
                .map(|t| ProposalEvidence {
                    kind: "thread".into(),
                    id: t.id.clone(),
                    label: t.subject.clone(),
                    excerpt: Some(truncate_chars(&t.snippet, MAX_EXCERPT_CHARS)),
                })
                .collect(),
        });
    }
    Ok(batch)
}
