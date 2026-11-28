//! SQLite persistence for standing Inbox Assistants, runs, and proposals.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use uuid::Uuid;

use super::{decode_json, Database, DatabaseError, DbResult};
use crate::agents::candidates::CandidateBatch;
use crate::agents::policy::*;
use crate::agents::runner::RunExecutionReport;
use crate::agents::{
    AgentId, AiRunConfig, InboxAgentConfig, InboxAgentProposal, InboxAgentRun, InboxAgentsSnapshot,
    ProposalEvidence, ProposalKind, ProposalPayload, ProposalState, RunState,
    UpdateInboxAgentConfig,
};

#[path = "agents/candidates.rs"]
mod candidates;
use candidates::select_candidates;

const RUNNER_ENABLED_KEY: &str = "inbox_agent_runner_enabled";
const AI_CONFIG_KEY: &str = "inbox_agent_ai_config";

fn encode<T: Serialize>(value: &T) -> DbResult<String> {
    serde_json::to_string(value).map_err(|e| DatabaseError::Serialization(e.to_string()))
}

fn decode<T: DeserializeOwned>(raw: &str) -> DbResult<T> {
    serde_json::from_str(raw).map_err(|e| DatabaseError::corrupt(e.to_string()))
}

fn runner_enabled_conn(conn: &Connection) -> DbResult<bool> {
    Ok(conn
        .query_row(
            "SELECT value != 'false' FROM compose_settings WHERE key = ?1",
            [RUNNER_ENABLED_KEY],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(true))
}

fn saved_ai_config_conn(conn: &Connection) -> DbResult<Option<AiRunConfig>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM compose_settings WHERE key = ?1",
            [AI_CONFIG_KEY],
            |r| r.get(0),
        )
        .optional()?;
    raw.as_deref().map(decode).transpose()
}

fn default_config(agent_id: AgentId) -> InboxAgentConfig {
    let entry = agent_id.catalog();
    InboxAgentConfig {
        agent_id,
        name: entry.name.into(),
        description: entry.description.into(),
        enabled: false,
        age_days: entry.default_age_days,
        max_threads_per_run: entry.default_max_threads,
        include_starred: entry.default_include_starred,
        account_id: None,
        schedule_hours: entry.default_schedule_hours,
        last_run_at: None,
        last_run_status: None,
        last_run_error: None,
        last_run_scanned: 0,
        last_run_proposed: 0,
        updated_at: 0,
    }
}

fn load_configs_conn(conn: &Connection) -> DbResult<Vec<InboxAgentConfig>> {
    let mut map: HashMap<AgentId, InboxAgentConfig> = AgentId::ALL
        .into_iter()
        .map(|id| (id, default_config(id)))
        .collect();
    let mut stmt = conn.prepare(
        "SELECT agent_id, enabled, age_days, max_threads_per_run, include_starred,
                account_id, schedule_hours, last_run_at, last_run_status, last_run_error,
                last_run_scanned, last_run_proposed, updated_at
         FROM inbox_agent_configs",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)? != 0,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)? != 0,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
            row.get::<_, i64>(12)?,
        ))
    })?;
    for row in rows {
        let (
            raw_id,
            enabled,
            age_days,
            max_threads_per_run,
            include_starred,
            account_id,
            schedule_hours,
            last_run_at,
            last_run_status,
            last_run_error,
            last_run_scanned,
            last_run_proposed,
            updated_at,
        ) = row?;
        let Some(agent_id) = AgentId::parse(&raw_id) else {
            continue;
        };
        if let Some(cfg) = map.get_mut(&agent_id) {
            cfg.enabled = enabled;
            cfg.age_days = age_days;
            cfg.max_threads_per_run = max_threads_per_run;
            cfg.include_starred = include_starred;
            cfg.account_id = account_id;
            cfg.schedule_hours = schedule_hours;
            cfg.last_run_at = last_run_at;
            cfg.last_run_status = last_run_status;
            cfg.last_run_error = last_run_error;
            cfg.last_run_scanned = last_run_scanned;
            cfg.last_run_proposed = last_run_proposed;
            cfg.updated_at = updated_at;
        }
    }
    Ok(AgentId::ALL
        .into_iter()
        .map(|id| map.remove(&id).unwrap())
        .collect())
}

fn ensure_config_row(conn: &Connection, agent_id: AgentId, now: i64) -> DbResult<()> {
    let def = default_config(agent_id);
    conn.execute(
        "INSERT OR IGNORE INTO inbox_agent_configs(
            agent_id, enabled, age_days, max_threads_per_run, include_starred,
            account_id, schedule_hours, last_run_scanned, last_run_proposed, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8)",
        params![
            agent_id.as_str(),
            i64::from(def.enabled),
            def.age_days,
            def.max_threads_per_run,
            i64::from(def.include_starred),
            def.account_id,
            def.schedule_hours,
            now
        ],
    )?;
    Ok(())
}

fn parse_run_state(raw: &str) -> RunState {
    match raw {
        "queued" => RunState::Queued,
        "running" => RunState::Running,
        "paused" => RunState::Paused,
        "completed" => RunState::Completed,
        "failed" => RunState::Failed,
        _ => RunState::Cancelled,
    }
}

fn list_active_runs_conn(conn: &Connection) -> DbResult<Vec<InboxAgentRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, account_id, state, reason, scanned_count, proposed_count,
                created_at, started_at, finished_at
         FROM inbox_agent_runs
         WHERE state IN ('queued', 'running')
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, Option<i64>>(9)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            id,
            raw_agent,
            account_id,
            state_raw,
            reason,
            scanned_count,
            proposed_count,
            created_at,
            started_at,
            finished_at,
        ) = row?;
        let Some(agent_id) = AgentId::parse(&raw_agent) else {
            continue;
        };
        out.push(InboxAgentRun {
            id,
            agent_id,
            account_id,
            state: parse_run_state(&state_raw),
            reason,
            scanned_count,
            proposed_count,
            created_at,
            started_at,
            finished_at,
        });
    }
    Ok(out)
}

fn enqueue_run_conn(
    conn: &Connection,
    agent_id: AgentId,
    ai_config: Option<&AiRunConfig>,
    account_id: Option<&str>,
    now: i64,
) -> DbResult<bool> {
    if !runner_enabled_conn(conn)? {
        return Err(DatabaseError::invalid(
            "Inbox assistants are paused. Resume them before running.",
        ));
    }
    let already_active: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM inbox_agent_runs
             WHERE agent_id = ?1 AND state IN ('queued', 'running')
         )",
        [agent_id.as_str()],
        |r| r.get(0),
    )?;
    if already_active {
        return Ok(false);
    }
    let effective_ai = match ai_config {
        Some(cfg) => Some(encode(cfg)?),
        None => saved_ai_config_conn(conn)?
            .map(|cfg| encode(&cfg))
            .transpose()?,
    };
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO inbox_agent_runs(
            id, agent_id, account_id, state, ai_config_json,
            scanned_count, proposed_count, created_at
         ) VALUES (?1, ?2, ?3, 'queued', ?4, 0, 0, ?5)",
        params![id, agent_id.as_str(), account_id, effective_ai, now],
    )?;
    Ok(true)
}

fn list_proposals_conn(
    conn: &Connection,
    account_id: Option<&str>,
    now: i64,
) -> DbResult<Vec<InboxAgentProposal>> {
    // Supersede proposals whose underlying thread moved out of Inbox, received a new
    // message, or is currently snoozed.
    conn.execute(
        "UPDATE inbox_agent_proposals
         SET state = 'superseded', updated_at = ?1
         WHERE state = 'pending'
           AND NOT EXISTS (
               SELECT 1 FROM threads t
               WHERE t.id = inbox_agent_proposals.thread_id
                 AND t.archived = 0
                 AND t.trashed = 0
                 AND t.last_message_at = inbox_agent_proposals.thread_revision
                 AND NOT EXISTS (
                     SELECT 1 FROM conversation_snoozes s
                     WHERE s.id = t.id AND json_extract(s.record_json, '$.state') = 'active'
                 )
           )",
        [now],
    )?;
    // A mute or block rule that now covers the sender settles a Sender Hygiene suggestion.
    conn.execute(
        "UPDATE inbox_agent_proposals
         SET state = 'superseded', updated_at = ?1
         WHERE state = 'pending' AND agent_id = 'sender_hygiene'
           AND EXISTS (
               SELECT 1 FROM sender_rules r
               WHERE lower(r.account_id) = lower(inbox_agent_proposals.account_id)
                 AND (
                     (r.match_kind = 'address'
                      AND r.match_value = json_extract(inbox_agent_proposals.payload_json, '$.senderEmail'))
                     OR (r.match_kind = 'domain'
                      AND r.match_value = substr(
                          json_extract(inbox_agent_proposals.payload_json, '$.senderEmail'),
                          instr(json_extract(inbox_agent_proposals.payload_json, '$.senderEmail'), '@') + 1))
                 )
           )",
        [now],
    )?;

    let mut sql = String::from(
        "SELECT p.id, p.agent_id, p.account_id, p.thread_id, p.thread_revision,
                t.subject, t.participants_json,
                COALESCE(NULLIF(t.last_received_at, ''), t.last_message_at),
                t.snippet, t.unread, t.starred,
                p.kind, p.state, p.headline, p.reason,
                p.payload_json, p.evidence_json, p.created_at, p.updated_at,
                (SELECT m.sender FROM messages m
                 WHERE m.thread_id = t.id
                 ORDER BY m.sent_at DESC LIMIT 1) AS latest_sender
         FROM inbox_agent_proposals p
         JOIN threads t ON t.id = p.thread_id
         WHERE p.state = 'pending'",
    );
    if account_id.is_some() {
        sql.push_str(" AND p.account_id = ?1");
    }
    sql.push_str(" ORDER BY p.created_at DESC, t.last_message_at DESC");

    let mut stmt = conn.prepare(&sql)?;
    let map_row = |row: &rusqlite::Row<'_>| {
        let participants = decode_json(row.get::<_, String>(6)?)?;
        let latest_sender: Option<String> = row.get(19)?;
        let sender = latest_sender
            .filter(|s| !s.trim().is_empty())
            .or_else(|| participants.first().cloned())
            .unwrap_or_default();
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            sender,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, i64>(9)? != 0,
            row.get::<_, i64>(10)? != 0,
            row.get::<_, String>(11)?,
            row.get::<_, String>(12)?,
            row.get::<_, String>(13)?,
            row.get::<_, String>(14)?,
            row.get::<_, String>(15)?,
            row.get::<_, String>(16)?,
            row.get::<_, i64>(17)?,
            row.get::<_, i64>(18)?,
        ))
    };

    let rows = if let Some(account) = account_id {
        stmt.query_map([account], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut proposals = Vec::with_capacity(rows.len());
    for (
        id,
        raw_agent,
        account_id,
        thread_id,
        thread_revision,
        thread_subject,
        thread_sender,
        thread_last_received_at,
        thread_snippet,
        thread_unread,
        thread_starred,
        raw_kind,
        raw_state,
        headline,
        reason,
        payload_json,
        evidence_json,
        created_at,
        updated_at,
    ) in rows
    {
        let Some(agent_id) = AgentId::parse(&raw_agent) else {
            continue;
        };
        let Some(kind) = ProposalKind::parse(&raw_kind) else {
            continue;
        };
        let Some(state) = ProposalState::parse(&raw_state) else {
            continue;
        };
        proposals.push(InboxAgentProposal {
            id,
            agent_id,
            account_id,
            thread_id,
            thread_revision,
            thread_subject,
            thread_sender,
            thread_last_received_at,
            thread_snippet,
            thread_unread,
            thread_starred,
            kind,
            state,
            headline,
            reason,
            payload: decode(&payload_json)?,
            evidence: decode(&evidence_json)?,
            created_at,
            updated_at,
        });
    }
    Ok(proposals)
}

impl Database {
    pub(crate) fn set_inbox_agent_runner_enabled(&self, value: bool, now: i64) -> DbResult<()> {
        self.with_transaction(|conn| {
            conn.execute(
                "INSERT INTO compose_settings(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![RUNNER_ENABLED_KEY, if value { "true" } else { "false" }],
            )?;
            if !value {
                conn.execute(
                    "UPDATE inbox_agent_runs
                     SET state = 'paused', reason = 'disabled', owner = NULL, lease_until = NULL, finished_at = ?1
                     WHERE state IN ('queued', 'running')",
                    [now],
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn save_inbox_agent_ai_config(&self, ai: Option<&AiRunConfig>) -> DbResult<()> {
        self.with_transaction(|conn| {
            if let Some(cfg) = ai {
                conn.execute(
                    "INSERT INTO compose_settings(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![AI_CONFIG_KEY, encode(cfg)?],
                )?;
            } else {
                conn.execute(
                    "DELETE FROM compose_settings WHERE key = ?1",
                    [AI_CONFIG_KEY],
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn get_inbox_agent_config(&self, agent_id: AgentId) -> DbResult<InboxAgentConfig> {
        self.with_connection(|conn| {
            Ok(load_configs_conn(conn)?
                .into_iter()
                .find(|c| c.agent_id == agent_id)
                .unwrap_or_else(|| default_config(agent_id)))
        })
    }

    pub(crate) fn update_inbox_agent_config(
        &self,
        update: &UpdateInboxAgentConfig,
        now: i64,
    ) -> DbResult<InboxAgentConfig> {
        if !(MIN_AGE_DAYS..=MAX_AGE_DAYS).contains(&update.age_days) {
            return Err(DatabaseError::invalid(format!(
                "Age threshold must be between {MIN_AGE_DAYS} and {MAX_AGE_DAYS} days"
            )));
        }
        if !(MIN_THREADS_PER_RUN..=MAX_THREADS_PER_RUN).contains(&update.max_threads_per_run) {
            return Err(DatabaseError::invalid(format!(
                "Max threads per run must be between {MIN_THREADS_PER_RUN} and {MAX_THREADS_PER_RUN}"
            )));
        }
        if !valid_schedule_hours(update.schedule_hours) {
            return Err(DatabaseError::invalid("Unsupported schedule cadence"));
        }
        self.with_transaction(|conn| {
            if let Some(account_id) = update.account_id.as_deref() {
                let exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE email = ?1 AND on_this_device = 1)",
                    [account_id],
                    |r| r.get(0),
                )?;
                if !exists {
                    return Err(DatabaseError::invalid(
                        "Selected account is not available on this device",
                    ));
                }
            }
            ensure_config_row(conn, update.agent_id, now)?;
            conn.execute(
                "UPDATE inbox_agent_configs
                 SET enabled = ?2, age_days = ?3, max_threads_per_run = ?4,
                     include_starred = ?5, account_id = ?6, schedule_hours = ?7, updated_at = ?8
                 WHERE agent_id = ?1",
                params![
                    update.agent_id.as_str(),
                    i64::from(update.enabled),
                    update.age_days,
                    update.max_threads_per_run,
                    i64::from(update.include_starred),
                    update.account_id,
                    update.schedule_hours,
                    now
                ],
            )?;
            Ok(load_configs_conn(conn)?
                .into_iter()
                .find(|c| c.agent_id == update.agent_id)
                .unwrap())
        })
    }

    pub(crate) fn enqueue_inbox_agent_run(
        &self,
        agent_id: AgentId,
        ai_config: Option<&AiRunConfig>,
        account_id: Option<&str>,
        now: i64,
    ) -> DbResult<bool> {
        self.with_transaction(|conn| {
            if let Some(cfg) = ai_config {
                conn.execute(
                    "INSERT INTO compose_settings(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![AI_CONFIG_KEY, encode(cfg)?],
                )?;
            }
            enqueue_run_conn(conn, agent_id, ai_config, account_id, now)
        })
    }

    pub(crate) fn enqueue_enabled_inbox_agent_runs(
        &self,
        ai_config: Option<&AiRunConfig>,
        account_id: Option<&str>,
        now: i64,
    ) -> DbResult<usize> {
        self.with_transaction(|conn| {
            if let Some(cfg) = ai_config {
                conn.execute(
                    "INSERT INTO compose_settings(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![AI_CONFIG_KEY, encode(cfg)?],
                )?;
            }
            let configs = load_configs_conn(conn)?;
            let mut count = 0usize;
            for cfg in configs {
                if cfg.enabled && enqueue_run_conn(conn, cfg.agent_id, ai_config, account_id, now)?
                {
                    count += 1;
                }
            }
            Ok(count)
        })
    }

    pub(crate) fn enqueue_due_scheduled_inbox_agents(&self, now: i64) -> DbResult<()> {
        self.with_transaction(|conn| {
            if !runner_enabled_conn(conn)? {
                return Ok(());
            }
            let configs = load_configs_conn(conn)?;
            for cfg in configs {
                let Some(hours) = cfg.schedule_hours else {
                    continue;
                };
                if !cfg.enabled {
                    continue;
                }
                let due = cfg
                    .last_run_at
                    .is_none_or(|last| last.saturating_add(hours.saturating_mul(3600)) <= now);
                if due {
                    let _ =
                        enqueue_run_conn(conn, cfg.agent_id, None, cfg.account_id.as_deref(), now)?;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn claim_inbox_agent_run(
        &self,
        owner: &str,
        now: i64,
    ) -> DbResult<Option<(InboxAgentRun, Option<AiRunConfig>)>> {
        self.with_transaction(|conn| {
            if !runner_enabled_conn(conn)? {
                return Ok(None);
            }
            let row = conn
                .query_row(
                    "SELECT id, agent_id, account_id, state, reason, ai_config_json,
                            scanned_count, proposed_count, created_at, started_at, finished_at
                     FROM inbox_agent_runs
                     WHERE state = 'queued'
                        OR (state = 'running' AND lease_until IS NOT NULL AND lease_until <= ?1)
                     ORDER BY created_at ASC
                     LIMIT 1",
                    [now],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                            row.get::<_, i64>(8)?,
                            row.get::<_, Option<i64>>(9)?,
                            row.get::<_, Option<i64>>(10)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                id,
                raw_agent,
                account_id,
                reason,
                ai_json,
                scanned_count,
                proposed_count,
                created_at,
                started_at,
                finished_at,
            )) = row
            else {
                return Ok(None);
            };
            let Some(agent_id) = AgentId::parse(&raw_agent) else {
                conn.execute(
                    "UPDATE inbox_agent_runs SET state = 'failed', reason = 'Unknown assistant', finished_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
                return Ok(None);
            };
            let lease_until = now.saturating_add(LEASE_SECONDS);
            let actual_start = started_at.unwrap_or(now);
            conn.execute(
                "UPDATE inbox_agent_runs
                 SET state = 'running', owner = ?2, lease_until = ?3, started_at = ?4
                 WHERE id = ?1",
                params![id, owner, lease_until, actual_start],
            )?;
            let ai_cfg = ai_json.as_deref().map(decode).transpose()?;
            Ok(Some((
                InboxAgentRun {
                    id,
                    agent_id,
                    account_id,
                    state: RunState::Running,
                    reason,
                    scanned_count,
                    proposed_count,
                    created_at,
                    started_at: Some(actual_start),
                    finished_at,
                },
                ai_cfg,
            )))
        })
    }

    pub(crate) fn renew_inbox_agent_run_lease(
        &self,
        run_id: &str,
        owner: &str,
        now: i64,
    ) -> DbResult<bool> {
        self.with_transaction(|conn| {
            if !runner_enabled_conn(conn)? {
                return Ok(false);
            }
            let updated = conn.execute(
                "UPDATE inbox_agent_runs
                 SET lease_until = ?3
                 WHERE id = ?1 AND owner = ?2 AND state = 'running'",
                params![run_id, owner, now.saturating_add(LEASE_SECONDS)],
            )?;
            Ok(updated > 0)
        })
    }

    pub(crate) fn finish_inbox_agent_run(
        &self,
        run_id: &str,
        owner: &str,
        result: Result<RunExecutionReport, String>,
        now: i64,
    ) -> DbResult<()> {
        self.with_transaction(|conn| {
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT agent_id, state FROM inbox_agent_runs WHERE id = ?1 AND owner = ?2",
                    params![run_id, owner],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((raw_agent, state_str)) = row else {
                return Ok(());
            };
            if state_str != "running" {
                return Ok(());
            }
            let Some(agent_id) = AgentId::parse(&raw_agent) else {
                return Ok(());
            };
            ensure_config_row(conn, agent_id, now)?;
            match result {
                Ok(report) => {
                    conn.execute(
                        "UPDATE inbox_agent_runs
                         SET state = 'completed', reason = NULL, scanned_count = ?2, proposed_count = ?3,
                             owner = NULL, lease_until = NULL, finished_at = ?4
                         WHERE id = ?1",
                        params![run_id, report.scanned, report.proposed, now],
                    )?;
                    conn.execute(
                        "UPDATE inbox_agent_configs
                         SET last_run_at = ?2, last_run_status = 'completed', last_run_error = NULL,
                             last_run_scanned = ?3, last_run_proposed = ?4, updated_at = ?2
                         WHERE agent_id = ?1",
                        params![agent_id.as_str(), now, report.scanned, report.proposed],
                    )?;
                }
                Err(error) => {
                    conn.execute(
                        "UPDATE inbox_agent_runs
                         SET state = 'failed', reason = ?2, owner = NULL, lease_until = NULL, finished_at = ?3
                         WHERE id = ?1",
                        params![run_id, error, now],
                    )?;
                    conn.execute(
                        "UPDATE inbox_agent_configs
                         SET last_run_at = ?2, last_run_status = 'failed', last_run_error = ?3, updated_at = ?2
                         WHERE agent_id = ?1",
                        params![agent_id.as_str(), now, error],
                    )?;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn release_inbox_agent_owner(&self, owner: &str, _now: i64) -> DbResult<()> {
        self.with_transaction(|conn| {
            conn.execute(
                "UPDATE inbox_agent_runs
                 SET state = 'queued', owner = NULL, lease_until = NULL
                 WHERE owner = ?1 AND state = 'running'",
                [owner],
            )?;
            Ok(())
        })
    }

    pub(crate) fn with_inbox_agent_candidates(
        &self,
        config: &InboxAgentConfig,
        now: i64,
    ) -> DbResult<CandidateBatch> {
        self.with_connection(|conn| select_candidates(conn, config, now))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn save_inbox_agent_proposal(
        &self,
        agent_id: AgentId,
        account_id: &str,
        thread_id: &str,
        thread_revision: &str,
        kind: ProposalKind,
        headline: &str,
        reason: &str,
        payload: &ProposalPayload,
        evidence: &[ProposalEvidence],
        now: i64,
    ) -> DbResult<bool> {
        self.with_transaction(|conn| {
            let id = Uuid::new_v4().to_string();
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO inbox_agent_proposals(
                    id, agent_id, account_id, thread_id, thread_revision, kind, state,
                    headline, reason, payload_json, evidence_json, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?9, ?10, ?11, ?11)",
                params![
                    id,
                    agent_id.as_str(),
                    account_id,
                    thread_id,
                    thread_revision,
                    kind.as_str(),
                    headline,
                    reason,
                    encode(payload)?,
                    encode(&evidence)?,
                    now
                ],
            )?;
            Ok(inserted > 0)
        })
    }

    pub(crate) fn resolve_inbox_agent_proposal(
        &self,
        id: &str,
        state: ProposalState,
        now: i64,
    ) -> DbResult<()> {
        if !matches!(state, ProposalState::Accepted | ProposalState::Dismissed) {
            return Err(DatabaseError::invalid(
                "Proposal resolution must be accepted or dismissed",
            ));
        }
        self.with_transaction(|conn| {
            let changed = conn.execute(
                "UPDATE inbox_agent_proposals SET state = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, state.as_str(), now],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Assistant proposal"));
            }
            Ok(())
        })
    }

    pub(crate) fn inbox_agents_snapshot(
        &self,
        account_id: Option<&str>,
    ) -> DbResult<InboxAgentsSnapshot> {
        let now = chrono::Utc::now().timestamp();
        self.with_transaction(|conn| {
            Ok(InboxAgentsSnapshot {
                runner_enabled: runner_enabled_conn(conn)?,
                agents: load_configs_conn(conn)?,
                active_runs: list_active_runs_conn(conn)?,
                proposals: list_proposals_conn(conn, account_id, now)?,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MailProviderKind;

    fn seed_thread(
        db: &Database,
        thread_id: &str,
        subject: &str,
        sender: &str,
        sent_at: &str,
        body: &str,
        labels: &str,
        unsubscribe_json: Option<&str>,
    ) {
        let conn = db.connection().unwrap();
        conn.execute(
            "INSERT INTO threads(id, provider_thread_id, subject, snippet, participants_json, last_message_at, labels_json, account_id, last_received_at)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, 'me@example.com', ?5)",
            params![
                thread_id,
                subject,
                body.chars().take(60).collect::<String>(),
                serde_json::to_string(&vec![sender, "me@example.com"]).unwrap(),
                sent_at,
                labels
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(id, thread_id, sender, recipients_json, sent_at, body_html, body_text, unsubscribe_json, unread)
             VALUES (?1, ?2, ?3, '[\"me@example.com\"]', ?4, '', ?5, ?6, 1)",
            params![
                format!("msg-{thread_id}"),
                thread_id,
                sender,
                sent_at,
                body,
                unsubscribe_json
            ],
        )
        .unwrap();
    }

    #[test]
    fn default_configs_and_bounds_validation() {
        let db = Database::open_memory();
        let snapshot = db.inbox_agents_snapshot(None).unwrap();
        assert!(snapshot.runner_enabled);
        assert_eq!(snapshot.agents.len(), 5);
        assert!(snapshot.agents.iter().all(|a| !a.enabled));

        assert!(db
            .update_inbox_agent_config(
                &UpdateInboxAgentConfig {
                    agent_id: AgentId::StaleReads,
                    enabled: true,
                    age_days: 0,
                    max_threads_per_run: 10,
                    include_starred: false,
                    account_id: None,
                    schedule_hours: None,
                },
                100
            )
            .is_err());

        let updated = db
            .update_inbox_agent_config(
                &UpdateInboxAgentConfig {
                    agent_id: AgentId::StaleReads,
                    enabled: true,
                    age_days: 14,
                    max_threads_per_run: 5,
                    include_starred: true,
                    account_id: None,
                    schedule_hours: Some(24),
                },
                100,
            )
            .unwrap();
        assert!(updated.enabled);
        assert_eq!(updated.age_days, 14);
        assert_eq!(updated.schedule_hours, Some(24));
    }

    #[test]
    fn candidates_and_deterministic_completed_task_detection() {
        let db = Database::open_memory();
        db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.connection()
            .unwrap()
            .execute("DELETE FROM threads WHERE id = 'welcome'", [])
            .unwrap();

        // 15-day-old newsletter (stale_reads), 15-day-old thread with completed task, 10-day-old human question
        seed_thread(
            &db,
            "t-news",
            "Weekly Tech Digest",
            "digest@news.example",
            "2026-11-01T10:00:00Z",
            &"Long article content. ".repeat(80),
            "[\"CATEGORY_UPDATES\"]",
            Some("{\"webUrl\":\"https://news.example/unsub\"}"),
        );
        seed_thread(
            &db,
            "t-task",
            "Send tax receipt",
            "acct@partner.example",
            "2026-11-01T10:00:00Z",
            "Please submit your receipt.",
            "[]",
            None,
        );
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO tasks(id, account_id, thread_id, title, kind, due_kind, status, completion_source, created_at, updated_at, completed_at)
                 VALUES ('task-1', 'me@example.com', 't-task', 'Submit tax receipt', 'action', 'none', 'completed', 'user', '2026-11-02T10:00:00Z', '2026-11-03T10:00:00Z', '2026-11-03T10:00:00Z')",
                [],
            )
            .unwrap();
        seed_thread(
            &db,
            "t-human",
            "Coffee next week?",
            "Alex <alex@friend.example>",
            "2026-11-10T10:00:00Z",
            "Are you free Tuesday for coffee?",
            "[\"INBOX\"]",
            None,
        );

        let now = chrono::DateTime::parse_from_rfc3339("2026-11-25T10:00:00Z")
            .unwrap()
            .timestamp();

        let mut stale_cfg = db.get_inbox_agent_config(AgentId::StaleReads).unwrap();
        stale_cfg.account_id = Some("me@example.com".into());
        let stale_batch = db.with_inbox_agent_candidates(&stale_cfg, now).unwrap();
        assert_eq!(stale_batch.ai_candidates.len(), 1);
        assert_eq!(stale_batch.ai_candidates[0].thread_id, "t-news");

        let mut resolved_cfg = db
            .get_inbox_agent_config(AgentId::ResolvedAndExpired)
            .unwrap();
        resolved_cfg.account_id = Some("me@example.com".into());
        let resolved_batch = db.with_inbox_agent_candidates(&resolved_cfg, now).unwrap();
        assert_eq!(resolved_batch.deterministic.len(), 1);
        assert_eq!(resolved_batch.deterministic[0].thread_id, "t-task");
        assert_eq!(
            resolved_batch.deterministic[0].kind,
            ProposalKind::ArchiveAsResolved
        );
        assert_eq!(
            resolved_batch.deterministic[0]
                .payload
                .task_completion_source
                .as_deref(),
            Some("user")
        );

        let mut reply_cfg = db.get_inbox_agent_config(AgentId::AgingReplies).unwrap();
        reply_cfg.account_id = Some("me@example.com".into());
        let reply_batch = db.with_inbox_agent_candidates(&reply_cfg, now).unwrap();
        let reply_ids: Vec<_> = reply_batch
            .ai_candidates
            .iter()
            .map(|c| c.thread_id.as_str())
            .collect();
        assert_eq!(reply_ids, vec!["t-human"]);
    }

    #[test]
    fn completed_task_sweeper_requires_strict_user_completion_and_no_active_tasks() {
        let db = Database::open_memory();
        db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.connection()
            .unwrap()
            .execute("DELETE FROM threads WHERE id = 'welcome'", [])
            .unwrap();

        seed_thread(
            &db,
            "t-reply-closed",
            "Waiting thread",
            "sam@example.com",
            "2026-11-01T10:00:00Z",
            "Will get back to you.",
            "[\"INBOX\"]",
            None,
        );
        seed_thread(
            &db,
            "t-sent-closed",
            "Data Quality Resources",
            "carolyn@example.com",
            "2026-11-01T10:00:00Z",
            "Here are some resource links.",
            "[\"INBOX\"]",
            None,
        );
        seed_thread(
            &db,
            "t-has-open",
            "Dual task thread",
            "pat@example.com",
            "2026-11-01T10:00:00Z",
            "Please do two things.",
            "[\"INBOX\"]",
            None,
        );
        seed_thread(
            &db,
            "t-stale-done",
            "Old completion",
            "kim@example.com",
            "2026-11-10T10:00:00Z",
            "Following up on Nov 10.",
            "[\"INBOX\"]",
            None,
        );

        let conn = db.connection().unwrap();
        // 1) completion_source='reply' must not trigger deterministic archive.
        conn.execute(
            "INSERT INTO tasks(id, account_id, thread_id, title, kind, due_kind, status, completion_source, created_at, updated_at, completed_at)
             VALUES ('task-reply', 'me@example.com', 't-reply-closed', 'Waiting on Sam', 'waiting_for', 'none', 'completed', 'reply', '2026-11-01T09:00:00Z', '2026-11-05T10:00:00Z', '2026-11-05T10:00:00Z')",
            [],
        )
        .unwrap();
        // 2) completed_by_email set (even with completion_source='user') must not trigger deterministic archive.
        conn.execute(
            "INSERT INTO tasks(id, account_id, thread_id, title, kind, due_kind, status, completion_source, completed_by_email, created_at, updated_at, completed_at)
             VALUES ('task-email', 'me@example.com', 't-sent-closed', 'Review Carolyn links', 'follow_up', 'none', 'completed', 'user', '{\"accountId\":\"me@example.com\"}', '2026-11-01T09:00:00Z', '2026-11-05T10:00:00Z', '2026-11-05T10:00:00Z')",
            [],
        )
        .unwrap();
        // 3) User-completed task alongside an active open task must not trigger deterministic archive.
        conn.execute(
            "INSERT INTO tasks(id, account_id, thread_id, title, kind, due_kind, status, completion_source, created_at, updated_at, completed_at)
             VALUES ('task-done-1', 'me@example.com', 't-has-open', 'Part 1', 'action', 'none', 'completed', 'user', '2026-11-01T09:00:00Z', '2026-11-05T10:00:00Z', '2026-11-05T10:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(id, account_id, thread_id, title, kind, due_kind, status, created_at, updated_at)
             VALUES ('task-open-2', 'me@example.com', 't-has-open', 'Part 2', 'action', 'none', 'open', '2026-11-01T09:00:00Z', '2026-11-05T10:00:00Z')",
            [],
        )
        .unwrap();
        // 4) User-completed task whose completed_at <= last_received_at must not trigger deterministic archive.
        conn.execute(
            "INSERT INTO tasks(id, account_id, thread_id, title, kind, due_kind, status, completion_source, created_at, updated_at, completed_at)
             VALUES ('task-old', 'me@example.com', 't-stale-done', 'Older step', 'action', 'none', 'completed', 'user', '2026-11-01T09:00:00Z', '2026-11-12T10:00:00Z', '2026-11-05T10:00:00Z')",
            [],
        )
        .unwrap();
        drop(conn);

        let now = chrono::DateTime::parse_from_rfc3339("2026-11-25T10:00:00Z")
            .unwrap()
            .timestamp();
        let mut resolved_cfg = db
            .get_inbox_agent_config(AgentId::ResolvedAndExpired)
            .unwrap();
        resolved_cfg.account_id = Some("me@example.com".into());
        let resolved_batch = db.with_inbox_agent_candidates(&resolved_cfg, now).unwrap();
        assert!(resolved_batch.deterministic.is_empty());
    }

    #[test]
    fn dismissed_proposals_do_not_reappear_for_same_revision_and_archive_supersedes() {
        let db = Database::open_memory();
        db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
            .unwrap();
        seed_thread(
            &db,
            "t1",
            "Newsletter",
            "news@example.com",
            "2026-11-01T10:00:00Z",
            "Body",
            "[\"CATEGORY_UPDATES\"]",
            Some("{}"),
        );

        assert!(db
            .save_inbox_agent_proposal(
                AgentId::StaleReads,
                "me@example.com",
                "t1",
                "2026-11-01T10:00:00Z",
                ProposalKind::ArchiveWithSummary,
                "Summary",
                "Old newsletter",
                &ProposalPayload::default(),
                &[],
                100
            )
            .unwrap());

        let snap = db.inbox_agents_snapshot(None).unwrap();
        assert_eq!(snap.proposals.len(), 1);
        let id = snap.proposals[0].id.clone();

        db.resolve_inbox_agent_proposal(&id, ProposalState::Dismissed, 110)
            .unwrap();
        assert!(db.inbox_agents_snapshot(None).unwrap().proposals.is_empty());

        // Re-inserting for same revision is ignored.
        assert!(!db
            .save_inbox_agent_proposal(
                AgentId::StaleReads,
                "me@example.com",
                "t1",
                "2026-11-01T10:00:00Z",
                ProposalKind::ArchiveWithSummary,
                "Summary",
                "Old newsletter",
                &ProposalPayload::default(),
                &[],
                120
            )
            .unwrap());

        // A fresh proposal on another thread supersedes automatically once archived.
        seed_thread(
            &db,
            "t2",
            "Newsletter 2",
            "news@example.com",
            "2026-11-01T10:00:00Z",
            "Body 2",
            "[]",
            Some("{}"),
        );
        db.save_inbox_agent_proposal(
            AgentId::StaleReads,
            "me@example.com",
            "t2",
            "2026-11-01T10:00:00Z",
            ProposalKind::ArchiveWithSummary,
            "Summary 2",
            "Old",
            &ProposalPayload::default(),
            &[],
            130,
        )
        .unwrap();
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET archived = 1 WHERE id = 't2'", [])
            .unwrap();
        assert!(db.inbox_agents_snapshot(None).unwrap().proposals.is_empty());
    }

    // ---- Sender Hygiene Advisor ----

    const HYGIENE_NOW: &str = "2026-11-25T10:00:00Z";

    fn hygiene_now() -> i64 {
        chrono::DateTime::parse_from_rfc3339(HYGIENE_NOW)
            .unwrap()
            .timestamp()
    }

    fn hygiene_db() -> Database {
        let db = Database::open_memory();
        db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.connection()
            .unwrap()
            .execute("DELETE FROM threads WHERE id = 'welcome'", [])
            .unwrap();
        db
    }

    /// `count` unread Inbox conversations from `sender`, all older than the default 14 days.
    fn seed_unread_threads(db: &Database, prefix: &str, sender: &str, count: usize, unsub: bool) {
        for index in 0..count {
            let id = format!("{prefix}-{index}");
            seed_thread(
                db,
                &id,
                &format!("{prefix} issue {index}"),
                sender,
                &format!("2026-11-0{}T10:00:00Z", index + 1),
                "Body",
                "[\"INBOX\"]",
                (unsub && index == 0).then_some("{\"webUrl\":\"https://x.example/u\"}"),
            );
            db.connection()
                .unwrap()
                .execute("UPDATE threads SET unread = 1 WHERE id = ?1", [&id])
                .unwrap();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn triage_event(
        db: &Database,
        thread_id: &str,
        email: &str,
        kind: &str,
        action: Option<&str>,
        dwell_ms: Option<i64>,
        scrolled: bool,
        batch: bool,
    ) {
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO triage_events(id, account_id, thread_id, sender_email, sender_domain,
                    event_kind, context, action, opened, dwell_ms, scrolled, batch, created_at)
                 VALUES (?1, 'me@example.com', ?2, ?3, 'x', ?4, 'inbox', ?5, 1, ?6, ?7, ?8, '2026-11-20T10:00:00Z')",
                params![
                    Uuid::new_v4().to_string(),
                    thread_id,
                    email,
                    kind,
                    action,
                    dwell_ms,
                    scrolled,
                    batch
                ],
            )
            .unwrap();
    }

    fn quick_dismissals(db: &Database, thread_id: &str, email: &str, count: usize) {
        for _ in 0..count {
            triage_event(
                db,
                thread_id,
                email,
                "disposition",
                Some("archive"),
                Some(400),
                false,
                false,
            );
        }
    }

    fn hygiene_batch(db: &Database) -> crate::agents::candidates::CandidateBatch {
        let cfg = db.get_inbox_agent_config(AgentId::SenderHygiene).unwrap();
        db.with_inbox_agent_candidates(&cfg, hygiene_now()).unwrap()
    }

    fn hygiene_senders(db: &Database) -> Vec<String> {
        hygiene_batch(db)
            .deterministic
            .into_iter()
            .map(|p| p.payload.sender_email.unwrap())
            .collect()
    }

    #[test]
    fn sender_hygiene_defaults_are_registered() {
        let db = Database::open_memory();
        let cfg = db.get_inbox_agent_config(AgentId::SenderHygiene).unwrap();
        assert_eq!(cfg.name, "Sender Hygiene Advisor");
        assert_eq!(
            (cfg.age_days, cfg.max_threads_per_run, cfg.include_starred),
            (14, 10, false)
        );
        assert!(!cfg.enabled);
    }

    #[test]
    fn sender_hygiene_qualifies_on_quick_dismissals_with_concrete_numbers() {
        let db = hygiene_db();
        seed_thread(
            &db,
            "d1",
            "Acme Deals",
            "Acme Deals <deals@acme.example>",
            "2026-11-24T10:00:00Z",
            "Sale",
            "[\"INBOX\"]",
            Some("{\"webUrl\":\"https://acme.example/u\"}"),
        );
        quick_dismissals(&db, "d1", "deals@acme.example", 6);
        triage_event(
            &db,
            "d1",
            "deals@acme.example",
            "disposition",
            Some("archive"),
            Some(5000),
            false,
            false,
        );

        let batch = hygiene_batch(&db);
        assert!(batch.ai_candidates.is_empty());
        assert_eq!(batch.deterministic.len(), 1);
        let proposal = &batch.deterministic[0];
        assert_eq!(proposal.kind, ProposalKind::SenderHygiene);
        assert_eq!(proposal.thread_id, "d1");
        assert_eq!(proposal.thread_revision, "2026-11-24T10:00:00Z");
        assert_eq!(proposal.headline, "You usually skip Acme Deals");
        assert_eq!(proposal.reason, "Dismissed 6 of 7 emails within a second.");
        let payload = &proposal.payload;
        assert_eq!(payload.sender_email.as_deref(), Some("deals@acme.example"));
        assert_eq!(payload.quick_dismiss_count, Some(6));
        assert_eq!(payload.dismiss_count, Some(7));
        assert_eq!(payload.recommendation.as_deref(), Some("unsubscribe"));
        assert_eq!(payload.message_id.as_deref(), Some("msg-d1"));
        assert_eq!(
            payload.inbox_thread_ids.as_deref(),
            Some(&["d1".to_string()][..])
        );
        assert_eq!(proposal.evidence.len(), 1);
        assert_eq!(proposal.evidence[0].kind, "thread");
    }

    #[test]
    fn sender_hygiene_qualifies_on_aging_unread_and_recommends_mute_without_unsubscribe() {
        let db = hygiene_db();
        seed_unread_threads(&db, "a", "Pat <pat@club.example>", 4, false);
        seed_unread_threads(&db, "b", "news@list.example", 3, true);
        // Two aging unread conversations is below the threshold.
        seed_unread_threads(&db, "c", "few@list.example", 2, false);

        let batch = hygiene_batch(&db);
        let by_sender: HashMap<_, _> = batch
            .deterministic
            .iter()
            .map(|p| (p.payload.sender_email.clone().unwrap(), p))
            .collect();
        assert_eq!(by_sender.len(), 2);
        let pat = by_sender["pat@club.example"];
        assert_eq!(pat.payload.recommendation.as_deref(), Some("mute"));
        assert_eq!(pat.payload.message_id, None);
        assert_eq!(pat.payload.aging_unread_count, Some(4));
        assert_eq!(pat.reason, "4 unread emails older than 14 days.");
        assert_eq!(pat.headline, "You usually skip Pat");
        assert_eq!(pat.payload.inbox_thread_ids.as_ref().unwrap().len(), 4);
        assert_eq!(pat.evidence.len(), 3);
        // Anchor is the sender's newest Inbox conversation.
        assert_eq!(pat.thread_id, "a-3");
        assert_eq!(
            by_sender["news@list.example"]
                .payload
                .recommendation
                .as_deref(),
            Some("unsubscribe")
        );
    }

    #[test]
    fn sender_hygiene_ignores_batch_scrolled_slow_and_read_mail() {
        let db = hygiene_db();
        seed_thread(
            &db,
            "x1",
            "X",
            "x@x.example",
            "2026-11-24T10:00:00Z",
            "B",
            "[\"INBOX\"]",
            None,
        );
        for _ in 0..3 {
            triage_event(
                &db,
                "x1",
                "x@x.example",
                "disposition",
                Some("archive"),
                Some(300),
                false,
                true,
            );
            triage_event(
                &db,
                "x1",
                "x@x.example",
                "disposition",
                Some("archive"),
                Some(300),
                true,
                false,
            );
            triage_event(
                &db,
                "x1",
                "x@x.example",
                "disposition",
                Some("archive"),
                Some(1500),
                false,
                false,
            );
        }
        // Aging but already read: not counted.
        seed_unread_threads(&db, "r", "read@x.example", 3, false);
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET unread = 0", [])
            .unwrap();
        // Quick rate below 60%: 3 quick of 6.
        seed_thread(
            &db,
            "y1",
            "Y",
            "y@x.example",
            "2026-11-24T10:00:00Z",
            "B",
            "[\"INBOX\"]",
            None,
        );
        quick_dismissals(&db, "y1", "y@x.example", 3);
        for _ in 0..3 {
            triage_event(
                &db,
                "y1",
                "y@x.example",
                "disposition",
                Some("trash"),
                Some(9000),
                false,
                false,
            );
        }
        assert!(hygiene_senders(&db).is_empty());
    }

    #[test]
    fn sender_hygiene_applies_exclusions() {
        let db = hygiene_db();
        for (prefix, sender) in [
            ("own", "me@example.com"),
            ("resp", "resp@x.example"),
            ("rest", "rest@x.example"),
            ("wrote", "wrote@x.example"),
            ("mute", "mute@x.example"),
            ("dom", "anyone@muted.example"),
            ("blk", "blk@x.example"),
            ("ok", "ok@x.example"),
        ] {
            seed_unread_threads(&db, prefix, sender, 3, false);
        }
        triage_event(
            &db,
            "resp-0",
            "resp@x.example",
            "response",
            None,
            None,
            false,
            false,
        );
        triage_event(
            &db,
            "rest-0",
            "rest@x.example",
            "restore",
            Some("archive"),
            None,
            false,
            false,
        );
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO contact_interactions(message_id, thread_id, account_id, email, direction, sent_at)
                 VALUES ('msg-wrote-0', 'wrote-0', 'me@example.com', 'wrote@x.example', 'sent', '2026-10-01T10:00:00Z')",
                [],
            )
            .unwrap();
        for (kind, value, action) in [
            ("address", "mute@x.example", "mute"),
            ("domain", "muted.example", "mute"),
            ("address", "blk@x.example", "block"),
        ] {
            db.connection()
                .unwrap()
                .execute(
                    "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at)
                     VALUES (?1, 'me@example.com', ?2, ?3, ?4, '2026-10-01T00:00:00Z')",
                    params![Uuid::new_v4().to_string(), kind, value, action],
                )
                .unwrap();
        }
        assert_eq!(hygiene_senders(&db), vec!["ok@x.example".to_string()]);
    }

    #[test]
    fn sender_hygiene_skips_recently_dismissed_and_already_pending_senders() {
        let db = hygiene_db();
        seed_unread_threads(&db, "ok", "ok@x.example", 3, false);
        let run = |now: i64| {
            let cfg = db.get_inbox_agent_config(AgentId::SenderHygiene).unwrap();
            let batch = db.with_inbox_agent_candidates(&cfg, now).unwrap();
            for p in &batch.deterministic {
                db.save_inbox_agent_proposal(
                    AgentId::SenderHygiene,
                    &p.account_id,
                    &p.thread_id,
                    &p.thread_revision,
                    p.kind,
                    &p.headline,
                    &p.reason,
                    &p.payload,
                    &p.evidence,
                    now,
                )
                .unwrap();
            }
            batch.deterministic.len()
        };
        let now = hygiene_now();
        assert_eq!(run(now), 1);
        // Pending already: a second run proposes nothing new.
        assert_eq!(run(now), 0);
        let id = db.inbox_agents_snapshot(None).unwrap().proposals[0]
            .id
            .clone();
        db.resolve_inbox_agent_proposal(&id, ProposalState::Dismissed, now - 10 * 86_400)
            .unwrap();
        assert_eq!(run(now), 0, "dismissed 10 days ago");
        db.resolve_inbox_agent_proposal(&id, ProposalState::Dismissed, now - 40 * 86_400)
            .unwrap();
        assert_eq!(run(now + 100), 0, "same anchor revision stays dismissed");
        // New mail changes the anchor revision; after the cooldown the sender can return.
        seed_thread(
            &db,
            "more-0",
            "newer",
            "ok@x.example",
            "2026-11-10T10:00:00Z",
            "Body",
            "[\"INBOX\"]",
            None,
        );
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET unread = 1 WHERE id = 'more-0'", [])
            .unwrap();
        assert_eq!(run(now), 1);
    }

    #[test]
    fn sender_hygiene_respects_account_scope_and_max_per_run() {
        let db = hygiene_db();
        db.adopt_mail_account("other@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.connection()
            .unwrap()
            .execute(
                "DELETE FROM threads WHERE id <> ''  AND account_id = 'other@example.com'",
                [],
            )
            .unwrap();
        seed_unread_threads(&db, "p", "p@x.example", 3, false);
        seed_unread_threads(&db, "q", "q@x.example", 3, false);
        seed_unread_threads(&db, "o", "o@x.example", 3, false);
        db.connection()
            .unwrap()
            .execute(
                "UPDATE threads SET account_id = 'other@example.com' WHERE id LIKE 'o-%'",
                [],
            )
            .unwrap();
        quick_dismissals(&db, "q-0", "q@x.example", 2);

        let mut cfg = db.get_inbox_agent_config(AgentId::SenderHygiene).unwrap();
        cfg.account_id = Some("me@example.com".into());
        let batch = db.with_inbox_agent_candidates(&cfg, hygiene_now()).unwrap();
        let senders: Vec<_> = batch
            .deterministic
            .iter()
            .map(|p| p.payload.sender_email.clone().unwrap())
            .collect();
        assert_eq!(senders.len(), 2);
        assert!(!senders.contains(&"o@x.example".to_string()));

        cfg.account_id = None;
        assert_eq!(
            db.with_inbox_agent_candidates(&cfg, hygiene_now())
                .unwrap()
                .deterministic
                .len(),
            3
        );

        // More quick dismissals rank first when the cap bites.
        cfg.max_threads_per_run = 1;
        triage_event(
            &db,
            "q-0",
            "q@x.example",
            "disposition",
            Some("trash"),
            Some(100),
            false,
            false,
        );
        let capped = db.with_inbox_agent_candidates(&cfg, hygiene_now()).unwrap();
        assert_eq!(capped.deterministic.len(), 1);
        assert_eq!(
            capped.deterministic[0].payload.sender_email.as_deref(),
            Some("q@x.example")
        );
    }

    #[test]
    fn a_new_mute_rule_supersedes_a_pending_sender_hygiene_proposal() {
        let db = hygiene_db();
        seed_unread_threads(&db, "ok", "ok@x.example", 3, false);
        seed_unread_threads(&db, "ot", "other@kept.example", 3, false);
        let now = hygiene_now();
        let cfg = db.get_inbox_agent_config(AgentId::SenderHygiene).unwrap();
        for p in db
            .with_inbox_agent_candidates(&cfg, now)
            .unwrap()
            .deterministic
        {
            db.save_inbox_agent_proposal(
                AgentId::SenderHygiene,
                &p.account_id,
                &p.thread_id,
                &p.thread_revision,
                p.kind,
                &p.headline,
                &p.reason,
                &p.payload,
                &p.evidence,
                now,
            )
            .unwrap();
        }
        assert_eq!(db.inbox_agents_snapshot(None).unwrap().proposals.len(), 2);
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at)
                 VALUES ('r1', 'me@example.com', 'domain', 'x.example', 'mute', '2026-11-25T00:00:00Z')",
                [],
            )
            .unwrap();
        let remaining = db.inbox_agents_snapshot(None).unwrap().proposals;
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].payload.sender_email.as_deref(),
            Some("other@kept.example")
        );
    }

    const DIGEST_NOW: &str = "2026-11-25T10:00:00Z";

    fn digest_now() -> i64 {
        chrono::DateTime::parse_from_rfc3339(DIGEST_NOW)
            .unwrap()
            .timestamp()
    }

    /// An unread bulk read (labelled Updates) from a newsletter sender.
    fn seed_digest_thread(db: &Database, id: &str, sent_at: &str) {
        seed_thread(
            db,
            id,
            &format!("Issue {id}"),
            "News <news@letters.example>",
            sent_at,
            "A long weekly roundup of things worth knowing.",
            "[\"INBOX\",\"CATEGORY_UPDATES\"]",
            None,
        );
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET unread = 1 WHERE id = ?1", [id])
            .unwrap();
    }

    fn digest_ids(db: &Database) -> Vec<String> {
        let cfg = db
            .get_inbox_agent_config(AgentId::StaleReadsDigest)
            .unwrap();
        db.with_inbox_agent_candidates(&cfg, digest_now())
            .unwrap()
            .deterministic
            .into_iter()
            .map(|p| p.thread_id)
            .collect()
    }

    #[test]
    fn stale_reads_digest_defaults_are_registered_with_a_weekly_schedule() {
        let db = Database::open_memory();
        let cfg = db
            .get_inbox_agent_config(AgentId::StaleReadsDigest)
            .unwrap();
        assert_eq!(cfg.name, "Stale Reads Digest");
        assert_eq!(
            (cfg.age_days, cfg.max_threads_per_run, cfg.include_starred),
            (7, 30, false)
        );
        assert_eq!(cfg.schedule_hours, Some(168));
        assert!(!cfg.enabled);
        // Other assistants keep their unscheduled defaults.
        let stale = db.get_inbox_agent_config(AgentId::StaleReads).unwrap();
        assert_eq!(stale.schedule_hours, None);
    }

    #[test]
    fn stale_reads_digest_selects_only_unread_bulk_reads_older_than_the_age_setting() {
        let db = hygiene_db();
        // Qualifies: unread, 10 days old, labelled Updates.
        seed_digest_thread(&db, "ok-label", "2026-11-15T10:00:00Z");
        // Qualifies through an unsubscribe option instead of a label.
        seed_thread(
            &db,
            "ok-unsub",
            "Weekly",
            "News <news@letters.example>",
            "2026-11-14T10:00:00Z",
            "Body text",
            "[\"INBOX\"]",
            Some("{\"webUrl\":\"https://x.example/u\"}"),
        );
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET unread = 1 WHERE id = 'ok-unsub'", [])
            .unwrap();
        // Too recent (3 days).
        seed_digest_thread(&db, "recent", "2026-11-22T10:00:00Z");
        // Read already.
        seed_digest_thread(&db, "read", "2026-11-10T10:00:00Z");
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET unread = 0 WHERE id = 'read'", [])
            .unwrap();
        // A person writing to the user: not a bulk read.
        seed_thread(
            &db,
            "human",
            "Coffee?",
            "Alex <alex@friend.example>",
            "2026-11-10T10:00:00Z",
            "Free Tuesday?",
            "[\"INBOX\"]",
            None,
        );
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET unread = 1 WHERE id = 'human'", [])
            .unwrap();
        // Bulk, but the user wrote in the conversation.
        seed_digest_thread(&db, "replied", "2026-11-09T10:00:00Z");
        db.connection()
            .unwrap()
            .execute(
                "INSERT INTO messages(id, thread_id, sender, recipients_json, sent_at, body_html, body_text, unread)
                 VALUES ('mine', 'replied', 'me@example.com', '[]', '2026-11-09T11:00:00Z', '', 'thanks', 0)",
                [],
            )
            .unwrap();
        // Archived.
        seed_digest_thread(&db, "archived", "2026-11-08T10:00:00Z");
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET archived = 1 WHERE id = 'archived'", [])
            .unwrap();

        // Oldest first.
        assert_eq!(digest_ids(&db), vec!["ok-unsub", "ok-label"]);

        let cfg = db
            .get_inbox_agent_config(AgentId::StaleReadsDigest)
            .unwrap();
        let batch = db.with_inbox_agent_candidates(&cfg, digest_now()).unwrap();
        assert_eq!(batch.ai_candidates.len(), 2);
        let item = batch
            .deterministic
            .iter()
            .find(|p| p.thread_id == "ok-label")
            .unwrap();
        assert_eq!(item.kind, ProposalKind::DigestItem);
        assert_eq!(item.headline, "Issue ok-label");
        assert_eq!(item.reason, "Unread for 10 days");
        assert_eq!(
            item.payload.summary.as_deref(),
            Some("A long weekly roundup of things worth knowing.")
        );
        assert_eq!(item.payload.suggest_save_to_note, Some(false));
        assert_eq!(item.payload.note_title.as_deref(), Some("Issue ok-label"));
        assert_eq!(item.evidence.len(), 1);
    }

    #[test]
    fn stale_reads_digest_trims_the_gist_to_the_limit() {
        let db = hygiene_db();
        seed_digest_thread(&db, "long", "2026-11-10T10:00:00Z");
        db.connection()
            .unwrap()
            .execute(
                "UPDATE threads SET snippet = ?1 WHERE id = 'long'",
                [format!(
                    "{}   {}",
                    "w".repeat(DIGEST_GIST_MAX_CHARS),
                    "tail"
                )],
            )
            .unwrap();
        let cfg = db
            .get_inbox_agent_config(AgentId::StaleReadsDigest)
            .unwrap();
        let batch = db.with_inbox_agent_candidates(&cfg, digest_now()).unwrap();
        let gist = batch.deterministic[0].payload.summary.clone().unwrap();
        assert_eq!(gist.chars().count(), DIGEST_GIST_MAX_CHARS);
        assert!(gist.ends_with('\u{2026}'));
    }

    #[test]
    fn stale_reads_digest_respects_starred_account_scope_and_the_cap() {
        let db = hygiene_db();
        seed_digest_thread(&db, "d1", "2026-11-01T10:00:00Z");
        seed_digest_thread(&db, "d2", "2026-11-02T10:00:00Z");
        seed_digest_thread(&db, "d3", "2026-11-03T10:00:00Z");
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET starred = 1 WHERE id = 'd1'", [])
            .unwrap();
        assert_eq!(digest_ids(&db), vec!["d2", "d3"]);

        let mut cfg = db
            .get_inbox_agent_config(AgentId::StaleReadsDigest)
            .unwrap();
        cfg.include_starred = true;
        cfg.max_threads_per_run = 2;
        let capped = db.with_inbox_agent_candidates(&cfg, digest_now()).unwrap();
        // The cap keeps the oldest threads.
        let ids: Vec<_> = capped
            .deterministic
            .iter()
            .map(|p| p.thread_id.as_str())
            .collect();
        assert_eq!(ids, vec!["d1", "d2"]);
        assert_eq!(capped.ai_candidates.len(), 2);

        cfg.account_id = Some("someone-else@example.com".into());
        assert!(db
            .with_inbox_agent_candidates(&cfg, digest_now())
            .unwrap()
            .deterministic
            .is_empty());
    }

    #[test]
    fn stale_reads_digest_and_stale_reads_never_propose_the_same_thread() {
        let db = hygiene_db();
        seed_digest_thread(&db, "shared", "2026-11-10T10:00:00Z");
        seed_digest_thread(&db, "other", "2026-11-11T10:00:00Z");
        let stale_ids = |db: &Database| {
            let cfg = db.get_inbox_agent_config(AgentId::StaleReads).unwrap();
            db.with_inbox_agent_candidates(&cfg, digest_now())
                .unwrap()
                .ai_candidates
                .into_iter()
                .map(|c| c.thread_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(digest_ids(&db).len(), 2);
        assert_eq!(stale_ids(&db).len(), 2);

        // A pending digest item hides the thread from Stale Reads Summarizer.
        assert!(db
            .save_inbox_agent_proposal(
                AgentId::StaleReadsDigest,
                "me@example.com",
                "shared",
                "2026-11-10T10:00:00Z",
                ProposalKind::DigestItem,
                "H",
                "R",
                &ProposalPayload::default(),
                &[],
                1,
            )
            .unwrap());
        assert_eq!(stale_ids(&db), vec!["other"]);
        // ... and is not re-proposed by the digest itself.
        assert_eq!(digest_ids(&db), vec!["other"]);

        // A pending stale-reads proposal hides the thread from the digest.
        assert!(db
            .save_inbox_agent_proposal(
                AgentId::StaleReads,
                "me@example.com",
                "other",
                "2026-11-11T10:00:00Z",
                ProposalKind::ArchiveWithSummary,
                "H",
                "R",
                &ProposalPayload::default(),
                &[],
                1,
            )
            .unwrap());
        assert!(digest_ids(&db).is_empty());
    }

    #[test]
    fn a_dismissed_digest_item_is_not_proposed_again_for_the_same_revision() {
        let db = hygiene_db();
        seed_digest_thread(&db, "once", "2026-11-10T10:00:00Z");
        db.save_inbox_agent_proposal(
            AgentId::StaleReadsDigest,
            "me@example.com",
            "once",
            "2026-11-10T10:00:00Z",
            ProposalKind::DigestItem,
            "H",
            "R",
            &ProposalPayload::default(),
            &[],
            1,
        )
        .unwrap();
        let id = db.inbox_agents_snapshot(None).unwrap().proposals[0]
            .id
            .clone();
        db.resolve_inbox_agent_proposal(&id, ProposalState::Dismissed, 2)
            .unwrap();
        assert!(digest_ids(&db).is_empty());
    }

    #[test]
    fn digest_items_supersede_when_the_thread_is_archived() {
        let db = hygiene_db();
        seed_digest_thread(&db, "gone", "2026-11-10T10:00:00Z");
        db.save_inbox_agent_proposal(
            AgentId::StaleReadsDigest,
            "me@example.com",
            "gone",
            "2026-11-10T10:00:00Z",
            ProposalKind::DigestItem,
            "H",
            "R",
            &ProposalPayload::default(),
            &[],
            1,
        )
        .unwrap();
        assert_eq!(db.inbox_agents_snapshot(None).unwrap().proposals.len(), 1);
        db.connection()
            .unwrap()
            .execute("UPDATE threads SET archived = 1 WHERE id = 'gone'", [])
            .unwrap();
        assert!(db.inbox_agents_snapshot(None).unwrap().proposals.is_empty());
    }
}
