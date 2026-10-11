//! Local scheduling authority is never reconstructed from replicated summaries.
use crate::{
    correspondence::{build_mime, OutboxItem},
    db::{Database, DatabaseError, DbResult},
};
use chrono::{NaiveDateTime, Offset, TimeZone};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
use threestrands_sync_protocol::EntityType;
pub use threestrands_sync_protocol::ScheduledSendReport;

pub const DISPATCH_GRACE_MS: i64 = 60_000;
pub type SendingIdentity = dyn Fn(bool) -> Result<String, String> + Send + Sync;

/// `create` is false whenever a persisted job needs its original identity.
pub fn keychain_identity(create: bool) -> Result<String, String> {
    let entry = keyring::Entry::new("app.threestrands.sending-device", "installation")
        .map_err(|_| "Unable to access the sending-device keychain entry")?;
    match entry.get_password() {
        Ok(value) if uuid::Uuid::parse_str(&value).is_ok() => Ok(value),
        Ok(_) => Err("The sending-device identity is damaged; scheduled mail is paused".into()),
        Err(keyring::Error::NoEntry) if create => {
            let id = uuid::Uuid::new_v4().to_string();
            entry
                .set_password(&id)
                .map_err(|_| "Unable to save the sending-device identity")?;
            Ok(id)
        }
        Err(_) => {
            Err("The sending-device identity is unavailable; scheduled mail is paused".into())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub report: ScheduledSendReport,
    pub visibility_group_id: Option<String>,
    #[serde(default)]
    pub can_manage: bool,
    #[serde(default)]
    pub visibility: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub local_time: String,
    pub time_zone: String,
    pub offset_seconds: Option<i32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeChoice {
    pub scheduled_at: i64,
    pub offset_seconds: i32,
}

pub fn choices(local_time: &str, time_zone: &str) -> Result<Vec<TimeChoice>, String> {
    let tz: chrono_tz::Tz = time_zone
        .parse()
        .map_err(|_| "Choose a valid IANA timezone")?;
    let local = NaiveDateTime::parse_from_str(local_time, "%Y-%m-%dT%H:%M")
        .map_err(|_| "Choose a valid date and time")?;
    let values = match tz.from_local_datetime(&local) {
        chrono::LocalResult::None => {
            return Err(
                "That time does not exist because the clocks change. Choose another time.".into(),
            )
        }
        chrono::LocalResult::Single(t) => vec![t],
        chrono::LocalResult::Ambiguous(a, b) => vec![a, b],
    };
    Ok(values
        .into_iter()
        .map(|t| TimeChoice {
            scheduled_at: t.timestamp_millis(),
            offset_seconds: t.offset().fix().local_minus_utc(),
        })
        .collect())
}

pub fn resolve(selection: &Selection, at: i64) -> Result<i64, String> {
    let options = choices(&selection.local_time, &selection.time_zone)?;
    let choice = if options.len() == 1 && selection.offset_seconds.is_none() {
        options.first()
    } else {
        options
            .iter()
            .find(|v| Some(v.offset_seconds) == selection.offset_seconds)
    }
    .ok_or("This time occurs twice. Choose its UTC offset.")?;
    if choice.scheduled_at <= at {
        return Err("Choose a time in the future".into());
    }
    Ok(choice.scheduled_at)
}

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
pub(crate) fn record(tx: &Transaction, id: &str, owner: &str, at: i64) -> DbResult<()> {
    let row: Option<(String, String)> = tx.query_row("SELECT schedule_json,state FROM outbox_messages WHERE id=?1 AND schedule_json IS NOT NULL", [id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((json, state)) = row else {
        return Ok(());
    };
    let mut schedule: Schedule =
        serde_json::from_str(&json).map_err(crate::db::serialization_error)?;
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
            let draft: crate::correspondence::Draft =
                serde_json::from_str(&payload).map_err(crate::db::serialization_error)?;
            if let Some(task_id) = draft.follow_up_task_id {
                match Database::record_follow_up_on(tx, &task_id) {
                    Ok(task) => {
                        if group(tx)?.is_some() {
                            crate::sync_state::record_replicated_write_in_transaction(
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
                                    .map_err(crate::db::serialization_error)?,
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
    let report = serde_json::to_value(&schedule.report).map_err(crate::db::serialization_error)?;
    let updated = serde_json::to_string(&schedule).map_err(crate::db::serialization_error)?;
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
            crate::sync_state::record_replicated_write_in_transaction(
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
    pub fn schedule_draft(
        &self,
        id: &str,
        revision: i64,
        selection: &Selection,
        owner: &str,
        root: &Path,
        at: i64,
    ) -> Result<OutboxItem, String> {
        if let Some(item) = self
            .outbox()?
            .into_iter()
            .find(|o| o.draft.id == id && o.draft.revision == revision)
        {
            return Ok(item);
        }
        let scheduled_at = resolve(selection, at)?;
        let draft = self.draft(id)?;
        if draft.revision != revision {
            return Err("Draft is still saving".into());
        }
        let sender_name = self
            .get_account(&draft.account)?
            .and_then(|a| a.display_name);
        let operation_id = uuid::Uuid::new_v4().to_string();
        let raw = build_mime(&draft, sender_name.as_deref(), &operation_id, root)?;
        let mut c = self.connection()?;
        let tx = c.transaction().map_err(|e| e.to_string())?;
        let visibility_group_id = group(&tx).map_err(|e| e.to_string())?;
        let owner_sync_device_id: Option<String> = tx
            .query_row(
                "SELECT device_id FROM sync_devices WHERE is_self=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let label: Option<String> = tx
            .query_row(
                "SELECT label FROM sync_device_labels WHERE device_id=?1",
                [&owner_sync_device_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let report = ScheduledSendReport {
            operation_id: operation_id.clone(),
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
            time_zone: selection.time_zone.clone(),
            state: "scheduled".into(),
            blocked_reason: None,
            report_revision: 1,
            status_changed_at: at,
        };
        report.validate()?;
        let schedule = Schedule {
            report,
            visibility_group_id,
            can_manage: true,
            visibility: "local".into(),
        };
        if tx
            .execute(
                "DELETE FROM drafts WHERE id=?1 AND revision=?2",
                params![id, revision],
            )
            .map_err(|e| e.to_string())?
            != 1
        {
            return Err("Draft changed while preparing send".into());
        }
        tx.execute("INSERT INTO outbox_messages(id,draft_id,revision,account,state,deadline,payload,raw,schedule_json) VALUES(?1,?2,?3,?4,'scheduled',?5,?6,?7,?8)", params![operation_id,id,revision,draft.account,scheduled_at,serde_json::to_string(&draft).map_err(|e|e.to_string())?,raw,serde_json::to_string(&schedule).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        record(&tx, &operation_id, owner, at).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        drop(c);
        self.outbox()?
            .into_iter()
            .find(|o| o.id == operation_id)
            .ok_or("Scheduled message missing".into())
    }

    pub fn manage_schedule(
        &self,
        id: &str,
        revision: i64,
        selection: Option<&Selection>,
        send_now: bool,
        owner: &str,
        at: i64,
    ) -> Result<(), String> {
        let next = selection.map(|s| resolve(s, at)).transpose()?;
        self.with_transaction(|tx| {
            let (json, state): (String, String) = tx.query_row(
                "SELECT schedule_json,state FROM outbox_messages WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut schedule: Schedule =
                serde_json::from_str(&json).map_err(crate::db::serialization_error)?;
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
                    serde_json::to_string(&schedule).map_err(crate::db::serialization_error)?
                ],
            )?;
            record(tx, id, owner, at)
        })
        .map_err(|e| e.to_string())
    }

    pub fn overdue_schedules(&self, at: i64, resumed: bool) -> Result<(), String> {
        // Recovery changes only local rows; only the owner may publish them.
        self.overdue_schedules_owned(at, resumed, "")
    }

    pub fn overdue_schedules_owned(
        &self,
        at: i64,
        resumed: bool,
        owner: &str,
    ) -> Result<(), String> {
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
                record(tx, &id, owner, at)?;
            }
            Ok(())
        })
        .map_err(|e| e.to_string())
    }

    pub fn block_schedule(
        &self,
        id: &str,
        owner: &str,
        reason: Option<&str>,
        at: i64,
    ) -> Result<(), String> {
        self.with_transaction(|tx| {
            let json: Option<String> = tx.query_row(
                "SELECT schedule_json FROM outbox_messages WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            let Some(json) = json else { return Ok(()) };
            let mut s: Schedule =
                serde_json::from_str(&json).map_err(crate::db::serialization_error)?;
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
                    serde_json::to_string(&s).map_err(crate::db::serialization_error)?
                ],
            )?;
            record(tx, id, owner, at)
        })
        .map_err(|e| e.to_string())
    }

    pub fn schedule_visibility(&self, s: &Schedule) -> Result<String, String> {
        if s.visibility_group_id.is_none() {
            return Ok("local".into());
        }
        self.with_transaction(|tx| {
            if s.visibility_group_id!=group(tx)? {return Ok("paused".into())}
            let dirty:bool=tx.query_row("SELECT COALESCE((SELECT dirty FROM sync_local_state WHERE id=1),1)",[],|r|r.get(0))?;
            let published:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_head_publications WHERE json_extract(head_content,'$.stateSequence')=(SELECT state_sequence FROM sync_local_state WHERE id=1))",[],|r|r.get(0))?;
            Ok(if !dirty && published {"published"} else {"pending"}.into())
        }).map_err(|e|e.to_string())
    }

    pub fn refresh_schedule_reports(&self, owner: &str, at: i64) -> Result<(), String> {
        self.with_transaction(|tx| {
            let ids = tx
                .prepare("SELECT id FROM outbox_messages WHERE schedule_json IS NOT NULL")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for id in ids {
                record(tx, &id, owner, at)?;
            }
            Ok(())
        })
        .map_err(|e| e.to_string())
    }

    pub fn change_outbox(
        &self,
        sql: &str,
        values: impl rusqlite::Params,
        id: &str,
        owner: &str,
        at: i64,
    ) -> Result<usize, String> {
        self.with_transaction(|tx| {
            let count = tx.execute(sql, values)?;
            if count > 0 {
                record(tx, id, owner, at)?;
            }
            Ok(count)
        })
        .map_err(|e| e.to_string())
    }

    pub fn scheduled_summaries(&self) -> Result<Vec<serde_json::Value>, String> {
        let c = self.connection()?;
        let mut q = c
            .prepare("SELECT report FROM scheduled_send_summaries ORDER BY rowid DESC")
            .map_err(|e| e.to_string())?;
        let values = q
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        values
            .into_iter()
            .map(|v| {
                let report: ScheduledSendReport =
                    serde_json::from_str(&v).map_err(|e| e.to_string())?;
                let label: Option<String> = c
                    .query_row(
                        "SELECT label FROM sync_device_labels WHERE device_id=?1",
                        [&report.owner_sync_device_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                let contact: Option<i64> = c
                    .query_row(
                        "SELECT last_head_seen_at_ms FROM sync_remote_states WHERE device_id=?1",
                        [&report.owner_sync_device_id],
                        |r| r.get::<_, Option<i64>>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                    .flatten();
                let mut value = serde_json::to_value(report).map_err(|e| e.to_string())?;
                value["ownerName"] = serde_json::json!(label);
                value["lastContactAt"] = serde_json::json!(contact);
                Ok(value)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWNER: &str = "00000000-0000-4000-8000-000000000001";
    fn db() -> Database {
        let db = Database::open_memory();
        db.adopt_account("me@example.com").unwrap();
        db.set_compose_identity("me@example.com").unwrap();
        db
    }
    fn queue(db: &Database) -> OutboxItem {
        let mut draft = db.create_draft("new", None, "me@example.com").unwrap();
        draft.to = "to@example.com".into();
        draft.bcc = "hidden@example.com".into();
        draft.subject = "Summary only".into();
        draft.body = "secret body".into();
        let d = db.save_draft(draft).unwrap();
        db.schedule_draft(
            &d.id,
            d.revision,
            &Selection {
                local_time: "2030-01-01T09:00".into(),
                time_zone: "UTC".into(),
                offset_seconds: None,
            },
            OWNER,
            Path::new("/unused"),
            1_800_000_000_000,
        )
        .unwrap()
    }
    fn group_setup(db: &Database, key: u8) {
        db.set_beta_features_enabled(true).unwrap();
        db.with_transaction(|tx| {
            crate::replicated_sync::ensure_space_and_device(tx)?;
            tx.execute(
                "UPDATE sync_spaces SET recovery_public_key=?1,active_epoch=1",
                [vec![key; 32]],
            )?;
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn timezone_gap_overlap_offsets_invalid_dates_and_future_validation() {
        assert!(choices("2026-03-08T02:30", "America/New_York").is_err());
        let overlap = choices("2026-11-01T01:30", "America/New_York").unwrap();
        assert_eq!(overlap.len(), 2);
        assert_eq!(overlap[1].scheduled_at - overlap[0].scheduled_at, 3_600_000);
        let mut s = Selection {
            local_time: "2026-11-01T01:30".into(),
            time_zone: "America/New_York".into(),
            offset_seconds: None,
        };
        assert!(resolve(&s, 0).is_err());
        for choice in overlap {
            s.offset_seconds = Some(choice.offset_seconds);
            assert_eq!(resolve(&s, 0).unwrap(), choice.scheduled_at);
            assert!(resolve(&s, choice.scheduled_at).is_err());
        }
        assert!(choices("2026-02-30T12:00", "UTC").is_err());
        assert!(choices("2026-11-01T12:00", "invalid").is_err());
    }
    #[test]
    fn snapshots_replicate_only_metadata_and_never_create_sendable_rows() {
        let owner = db();
        group_setup(&owner, 7);
        let item = queue(&owner);
        let identity = owner
            .with_transaction(crate::replicated_sync::ensure_space_and_device)
            .unwrap();
        let (snapshot, _) = owner
            .take_local_snapshot(
                threestrands_sync_envelope::DeviceId::from_bytes(identity),
                1,
                item.deadline - 1,
            )
            .unwrap();
        let bytes = serde_json::to_string(&snapshot).unwrap();
        for secret in [
            "secret body",
            "hidden@example.com",
            "to@example.com",
            "raw",
            "attachments",
        ] {
            assert!(!bytes.contains(secret), "leaked {secret}");
        }
        let peer = db();
        let state = crate::sync_state::snapshot_to_state(&snapshot).unwrap();
        peer.merge_replica_state(&state).unwrap();
        let report = &item.schedule.as_ref().unwrap().report;
        peer.materialize_entity(
            EntityType::ScheduledSendSummary,
            &report.entity_id(),
            Some(&serde_json::json!({"report":report})),
            false,
        )
        .unwrap();
        assert!(peer.outbox().unwrap().is_empty());
        assert!(peer.drafts().unwrap().is_empty());
        assert_eq!(peer.scheduled_summaries().unwrap().len(), 1);
        assert!(peer
            .materialize_entity(
                EntityType::ScheduledSendSummary,
                "different-id",
                Some(&serde_json::json!({"report":report})),
                false
            )
            .is_err());
        owner
            .manage_schedule(
                &item.id,
                1,
                Some(&Selection {
                    local_time: "2030-01-02T09:00".into(),
                    time_zone: "UTC".into(),
                    offset_seconds: None,
                }),
                false,
                OWNER,
                item.deadline - 1,
            )
            .unwrap();
        let (newer, _) = owner
            .take_local_snapshot(
                threestrands_sync_envelope::DeviceId::from_bytes(identity),
                2,
                item.deadline - 1,
            )
            .unwrap();
        peer.merge_replica_state(&crate::sync_state::snapshot_to_state(&newer).unwrap())
            .unwrap();
        peer.merge_replica_state(&state).unwrap();
        let value = peer
            .resolve_field_winner(
                EntityType::ScheduledSendSummary,
                &report.entity_id(),
                "report",
            )
            .unwrap()
            .unwrap();
        assert!(value["reportRevision"].as_i64().unwrap() > 1);
        assert!(peer.outbox().unwrap().is_empty());
    }
    #[test]
    fn disabled_sync_backlog_and_group_changes_do_not_republish_old_jobs() {
        let owner = db();
        group_setup(&owner, 7);
        let item = queue(&owner);
        let entity = item.schedule.as_ref().unwrap().report.entity_id();
        owner.set_beta_features_enabled(false).unwrap();
        owner
            .overdue_schedules_owned(item.deadline + 60_001, false, OWNER)
            .unwrap();
        assert_eq!(
            owner
                .resolve_field_winner(EntityType::ScheduledSendSummary, &entity, "report")
                .unwrap()
                .unwrap()["state"],
            "scheduled"
        );
        owner.set_beta_features_enabled(true).unwrap();
        owner
            .refresh_schedule_reports(OWNER, item.deadline + 60_002)
            .unwrap();
        assert_eq!(
            owner
                .resolve_field_winner(EntityType::ScheduledSendSummary, &entity, "report")
                .unwrap()
                .unwrap()["state"],
            "overdue"
        );
        owner.leave_sync_space().unwrap();
        group_setup(&owner, 8);
        owner
            .refresh_schedule_reports(OWNER, item.deadline + 60_003)
            .unwrap();
        assert!(owner
            .resolve_field_winner(EntityType::ScheduledSendSummary, &entity, "report")
            .unwrap()
            .is_none());
        assert_eq!(
            owner.outbox().unwrap()[0]
                .schedule
                .as_ref()
                .unwrap()
                .report
                .owner_installation_id,
            OWNER
        );
    }
    #[test]
    fn restart_preserves_future_time_and_never_releases_elapsed_schedules() {
        let db = db();
        let item = queue(&db);
        crate::schema::migrate(&mut db.connection().unwrap()).unwrap();
        assert_eq!(db.outbox().unwrap()[0].deadline, item.deadline);
        db.overdue_schedules(item.deadline + 1, true).unwrap();
        assert_eq!(db.outbox().unwrap()[0].state, "overdue");
        assert!(!db.pending_undo().unwrap());
    }
}
