//! Scheduled-send rows in the outbox and their replicated summary reports.
//! Local scheduling authority is never reconstructed from replicated summaries.

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::correspondence::Draft;
use crate::scheduled_send::{resolve, Schedule, ScheduledSendReport, Selection, DISPATCH_GRACE_MS};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::BTreeSet;
use threestrands_sync_protocol::EntityType;

fn group(tx: &Transaction) -> DbResult<Option<String>> {
    let value: Option<Vec<u8>> = tx
        .query_row(
            "SELECT recovery_public_key FROM sync_spaces WHERE id=?1 AND (enabled=1 OR ?2=1)",
            params![
                crate::replicated_sync::SPACE_ID,
                crate::replicated_sync::enabled()
            ],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(value.map(|v| v.iter().map(|b| format!("{b:02x}")).collect()))
}

/// Called in the same transaction as a local state change. Reports survive a
/// disabled connector; backlog reconciliation later replays them to the graph.
pub(crate) fn record_schedule_report(tx: &Transaction, id: &str, owner: &str, at: i64) -> DbResult<()> {
    let row: Option<(String, String)> = tx.query_row("SELECT schedule_json,state FROM outbox_messages WHERE id=?1 AND schedule_json IS NOT NULL", [id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((json, state)) = row else {
        return Ok(());
    };
    let mut schedule: Schedule =
        serde_json::from_str(&json).map_err(serialization_error)?;
    if schedule.report.owner_installation_id != owner {
        return Ok(());
    }
    if schedule.report.state != state {
        if state == "sent" {
            let payload: String = tx.query_row(
                "SELECT payload FROM outbox_messages WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            let draft: Draft =
                serde_json::from_str(&payload).map_err(serialization_error)?;
            if let Some(task_id) = draft.follow_up_task_id {
                match Database::record_follow_up_on(tx, &task_id) {
                    Ok(task) => {
                        if group(tx)?.is_some() {
                            crate::db::sync_state::record_replicated_write_in_transaction(
                                tx,
                                EntityType::Task,
                                &task.id,
                                &BTreeSet::from([
                                    "dueValue".into(),
                                    "waitAfter".into(),
                                    "completionSource".into(),
                                    "completedAt".into(),
                                    "updatedAt".into(),
                                ]),
                                &serde_json::to_value(&task)
                                    .map_err(serialization_error)?,
                            )?;
                        }
                    }
                    Err(DatabaseError::NotFound(_)) | Err(DatabaseError::Validation(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        schedule.report.state = state;
        schedule.report.report_revision += 1;
        schedule.report.status_changed_at = at;
    }
    schedule.report.validate().map_err(DatabaseError::invalid)?;
    let report = serde_json::to_value(&schedule.report).map_err(serialization_error)?;
    let updated = serde_json::to_string(&schedule).map_err(serialization_error)?;
    if updated != json {
        tx.execute(
            "UPDATE outbox_messages SET schedule_json=?2 WHERE id=?1",
            params![id, updated],
        )?;
    }
    if schedule.visibility_group_id.is_some() && schedule.visibility_group_id == group(tx)? {
        let entity_id = schedule.report.entity_id();
        let current: Option<String> = tx.query_row("SELECT value FROM sync_values WHERE entity_type='scheduled_send_summary' AND entity_id=?1 AND field='report' ORDER BY lamport DESC LIMIT 1", [&entity_id], |r| r.get(0)).optional()?.flatten();
        if current.as_deref() != Some(report.to_string().as_str()) {
            crate::db::sync_state::record_replicated_write_in_transaction(
                tx,
                EntityType::ScheduledSendSummary,
                &entity_id,
                &BTreeSet::from(["report".into()]),
                &serde_json::json!({"report":report}),
            )?;
        }
    }
    Ok(())
}

impl Database {
    /// Moves a draft at `revision` into the outbox as a scheduled message
    /// carrying its prepared MIME, and records its first report.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_scheduled_draft(
        &self,
        draft: &Draft,
        revision: i64,
        operation_id: &str,
        raw: &[u8],
        owner: &str,
        scheduled_at: i64,
        time_zone: &str,
        at: i64,
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let visibility_group_id = group(tx)?;
            let owner_sync_device_id: Option<String> = tx
                .query_row(
                    "SELECT device_id FROM sync_devices WHERE is_self=1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let label: Option<String> = tx
                .query_row(
                    "SELECT label FROM sync_device_labels WHERE device_id=?1",
                    [&owner_sync_device_id],
                    |r| r.get(0),
                )
                .optional()?;
            let report = ScheduledSendReport {
                operation_id: operation_id.into(),
                owner_installation_id: owner.into(),
                owner_sync_device_id,
                owner_name_at_creation: label.unwrap_or_else(|| {
                    gethostname::gethostname()
                        .to_string_lossy()
                        .chars()
                        .take(60)
                        .collect()
                }),
                account: draft.account.clone(),
                subject: draft.subject.chars().take(998).collect(),
                scheduled_at,
                time_zone: time_zone.into(),
                state: "scheduled".into(),
                blocked_reason: None,
                report_revision: 1,
                status_changed_at: at,
            };
            report.validate().map_err(DatabaseError::invalid)?;
            let schedule = Schedule {
                report,
                visibility_group_id,
                can_manage: true,
                visibility: "local".into(),
            };
            if tx.execute(
                "DELETE FROM drafts WHERE id=?1 AND revision=?2",
                params![draft.id, revision],
            )? != 1
            {
                return Err(DatabaseError::invalid("Draft changed while preparing send"));
            }
            Database::draft_left_drafts(tx, &draft.id, at)?;
            tx.execute("INSERT INTO outbox_messages(id,draft_id,revision,account,state,deadline,payload,raw,schedule_json) VALUES(?1,?2,?3,?4,'scheduled',?5,?6,?7,?8)", params![operation_id,draft.id,revision,draft.account,scheduled_at,serde_json::to_string(draft).map_err(serialization_error)?,raw,serde_json::to_string(&schedule).map_err(serialization_error)?])?;
            record_schedule_report(tx, operation_id, owner, at)
        })
    }

    pub fn manage_schedule(
        &self,
        id: &str,
        revision: i64,
        selection: Option<&Selection>,
        send_now: bool,
        owner: &str,
        at: i64,
    ) -> DbResult<()> {
        let next = selection
            .map(|s| resolve(s, at))
            .transpose()
            .map_err(DatabaseError::invalid)?;
        self.with_transaction(|tx| {
            let (json, state): (String, String) = tx.query_row(
                "SELECT schedule_json,state FROM outbox_messages WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut schedule: Schedule =
                serde_json::from_str(&json).map_err(serialization_error)?;
            if schedule.report.owner_installation_id != owner
                || schedule.report.report_revision != revision
                || !["scheduled", "overdue"].contains(&state.as_str())
                || (send_now && state != "overdue")
            {
                return Err(DatabaseError::invalid(
                    "Schedule changed or belongs to another computer",
                ));
            }
            if let Some(next) = next {
                schedule.report.scheduled_at = next;
                schedule.report.time_zone = selection.unwrap().time_zone.clone();
            }
            schedule.report.report_revision += 1;
            schedule.report.status_changed_at = at;
            schedule.report.blocked_reason = None;
            tx.execute(
                "UPDATE outbox_messages SET state=?2,deadline=?3,schedule_json=?4 WHERE id=?1",
                params![
                    id,
                    if send_now {
                        "undo_pending"
                    } else {
                        "scheduled"
                    },
                    if send_now { at + 10_000 } else { next.unwrap() },
                    serde_json::to_string(&schedule).map_err(serialization_error)?
                ],
            )?;
            record_schedule_report(tx, id, owner, at)
        })
    }

    #[cfg(test)]
    fn overdue_schedules(&self, at: i64, resumed: bool) -> DbResult<()> {
        // Recovery changes only local rows; only the owner may publish them.
        self.overdue_schedules_owned(at, resumed, "")
    }

    pub fn overdue_schedules_owned(
        &self,
        at: i64,
        resumed: bool,
        owner: &str,
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let threshold = if resumed {
                at
            } else {
                at.saturating_sub(DISPATCH_GRACE_MS)
            };
            let ids = tx
                .prepare("SELECT id FROM outbox_messages WHERE state='scheduled' AND deadline<?1")?
                .query_map([threshold], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for id in ids {
                tx.execute(
                    "UPDATE outbox_messages SET state='overdue' WHERE id=?1",
                    [&id],
                )?;
                record_schedule_report(tx, &id, owner, at)?;
            }
            Ok(())
        })
    }

    pub fn block_schedule(
        &self,
        id: &str,
        owner: &str,
        reason: Option<&str>,
        at: i64,
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let json: Option<String> = tx.query_row(
                "SELECT schedule_json FROM outbox_messages WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            let Some(json) = json else { return Ok(()) };
            let mut s: Schedule =
                serde_json::from_str(&json).map_err(serialization_error)?;
            if s.report.owner_installation_id != owner
                || s.report.blocked_reason.as_deref() == reason
            {
                return Ok(());
            }
            s.report.blocked_reason = reason.map(str::to_string);
            s.report.report_revision += 1;
            s.report.status_changed_at = at;
            tx.execute(
                "UPDATE outbox_messages SET schedule_json=?2 WHERE id=?1",
                params![
                    id,
                    serde_json::to_string(&s).map_err(serialization_error)?
                ],
            )?;
            record_schedule_report(tx, id, owner, at)
        })
    }

    pub fn schedule_visibility(&self, s: &Schedule) -> DbResult<String> {
        if s.visibility_group_id.is_none() {
            return Ok("local".into());
        }
        self.with_transaction(|tx| {
            if s.visibility_group_id!=group(tx)? {return Ok("paused".into())}
            let dirty:bool=tx.query_row("SELECT COALESCE((SELECT dirty FROM sync_local_state WHERE id=1),1)",[],|r|r.get(0))?;
            let published:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_head_publications WHERE json_extract(head_content,'$.stateSequence')=(SELECT state_sequence FROM sync_local_state WHERE id=1))",[],|r|r.get(0))?;
            Ok(if !dirty && published {"published"} else {"pending"}.into())
        })
    }

    pub fn refresh_schedule_reports(&self, owner: &str, at: i64) -> DbResult<()> {
        self.with_transaction(|tx| {
            let ids = tx
                .prepare("SELECT id FROM outbox_messages WHERE schedule_json IS NOT NULL")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for id in ids {
                record_schedule_report(tx, &id, owner, at)?;
            }
            Ok(())
        })
    }

    pub(super) fn change_outbox(
        &self,
        sql: &str,
        values: impl rusqlite::Params,
        id: &str,
        owner: &str,
        at: i64,
    ) -> DbResult<usize> {
        self.with_transaction(|tx| {
            let count = tx.execute(sql, values)?;
            if count > 0 {
                record_schedule_report(tx, id, owner, at)?;
            }
            Ok(count)
        })
    }

    pub fn scheduled_summaries(&self) -> DbResult<Vec<serde_json::Value>> {
        self.with_connection(|c| {
            let mut q =
                c.prepare("SELECT report FROM scheduled_send_summaries ORDER BY rowid DESC")?;
            let values = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            values
                .into_iter()
                .map(|v| {
                    let report: ScheduledSendReport =
                        serde_json::from_str(&v).map_err(serialization_error)?;
                    let label: Option<String> = c
                        .query_row(
                            "SELECT label FROM sync_device_labels WHERE device_id=?1",
                            [&report.owner_sync_device_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let contact: Option<i64> = c
                        .query_row(
                            "SELECT last_head_seen_at_ms FROM sync_remote_states WHERE device_id=?1",
                            [&report.owner_sync_device_id],
                            |r| r.get::<_, Option<i64>>(0),
                        )
                        .optional()?
                        .flatten();
                    let mut value = serde_json::to_value(report).map_err(serialization_error)?;
                    value["ownerName"] = serde_json::json!(label);
                    value["lastContactAt"] = serde_json::json!(contact);
                    Ok(value)
                })
                .collect()
        })
    }
}

impl Database {
    /// Stores another device's scheduled-send summary report.
    pub(crate) fn upsert_scheduled_summary(&self, id: &str, report_json: &str) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute("INSERT INTO scheduled_send_summaries(id,report) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET report=excluded.report",params![id,report_json])?;
            Ok(())
        })
    }

    /// Removes a scheduled-send summary another device deleted.
    pub(crate) fn delete_scheduled_summary(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM scheduled_send_summaries WHERE id=?1", [id])?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "tests/scheduled_send.rs"]
mod tests;
