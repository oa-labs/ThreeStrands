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
        schedule_hours: None,
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
        assert_eq!(snapshot.agents.len(), 3);
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
                "INSERT INTO tasks(id, account_id, thread_id, title, kind, due_kind, status, created_at, updated_at)
                 VALUES ('task-1', 'me@example.com', 't-task', 'Submit tax receipt', 'action', 'none', 'completed', '2026-11-02T10:00:00Z', '2026-11-03T10:00:00Z')",
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
}
