//! The boundary between application tables and the replicated-sync
//! operation graph, in both directions:
//!
//! - Local mutations call [`Database::record_local_entity_write`] /
//!   [`Database::record_local_entity_deletion`] after their app-table write.
//!   Both are no-ops unless replicated sync is active on this device, so a
//!   local-only installation records nothing that could leave it.
//! - Pulled remote state reaches application tables only through
//!   [`Database::materialize_entity`].

use std::collections::BTreeSet;

use chrono::Utc;
use keyring::Entry;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use threestrands_sync_protocol::EntityType;
use crate::models::ContactRecord;

use crate::{
    db::{Database, DbResult},
    enrollment::EnrollmentStatus,
    error_text::display,
    models::{Account, Snippet, SplitInbox, ThreadTask},
};

impl Database {
    /// Whether local changes on this device currently reach other devices:
    /// replicated sync is active *and* this device holds the space's active
    /// epoch. Account removal uses this to decide whether a local disconnect
    /// must keep the synchronized account catalog entry, and whether "remove
    /// everywhere" can be honored at all.
    pub fn cross_device_sync_enrolled(&self) -> Result<bool, String> {
        Ok(self.replicated_sync_active()?
            && matches!(self.enrollment_status()?, EnrollmentStatus::Enrolled { .. }))
    }

    /// Records a local creation or update of a synchronized entity. `fields`
    /// names the fields this write changed; `None` means every top-level
    /// field of `payload` (a full write).
    pub fn record_local_entity_write(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        payload: Value,
        fields: Option<BTreeSet<String>>,
    ) -> Result<(), String> {
        if !self.replicated_sync_active()? {
            return Ok(());
        }
        let fields = fields.unwrap_or_else(|| {
            payload
                .as_object()
                .map(|value| value.keys().cloned().collect())
                .unwrap_or_default()
        });
        self.record_replicated_write(entity_type, entity_id, &fields, &payload)
    }

    /// Records a local deletion of a synchronized entity.
    pub fn record_local_entity_deletion(&self, entity_type: EntityType, entity_id: &str) -> Result<(), String> {
        if !self.replicated_sync_active()? {
            return Ok(());
        }
        self.record_replicated_deletion(entity_type, entity_id)
    }

    /// The materialization boundary from the operation graph to application
    /// tables: a one-way function from a resolved, complete entity value to
    /// a table write. The only place that knows how to turn a
    /// `(entity_type, entity_id, payload, deleted)` triple into local state.
    pub(crate) fn materialize_entity(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        payload: Option<&Value>,
        deleted: bool,
    ) -> DbResult<()> {
        if deleted {
            match entity_type {
                EntityType::Task => self.delete_synced_row("DELETE FROM tasks WHERE id=?1", entity_id)?,
                EntityType::Snippet => self.delete_synced_row("DELETE FROM snippets WHERE id=?1", entity_id)?,
                EntityType::SplitInbox => self.delete_synced_row("DELETE FROM split_inboxes WHERE id=?1", entity_id)?,
                EntityType::Contact => self.delete_contact_profile(entity_id)?,
                // Unlinks local tasks and goals the same way the deleting device did.
                EntityType::Goal => {
                    self.delete_goal(entity_id)?;
                }
                EntityType::MailAccount => {
                    clear_provider_credential("app.threestrands.mail", entity_id)?;
                    if self.get_account(entity_id)?.is_some() {
                        self.remove_account(entity_id)?;
                    }
                }
                EntityType::CalendarAccount => {
                    clear_provider_credential("app.threestrands.calendar", entity_id)?;
                    if let Err(error) = self.remove_calendar_account(entity_id) {
                        log::warn!(target: "replicated_sync", "removing a synced calendar account failed: {error}");
                    }
                }
                _ => {}
            }
        } else if let Some(payload) = payload {
            match entity_type {
                EntityType::Task => {
                    self.upsert_synced_task(serde_json::from_value(payload.clone()).map_err(display)?)?
                }
                EntityType::Snippet => {
                    self.upsert_synced_snippet(serde_json::from_value(payload.clone()).map_err(display)?)?
                }
                EntityType::SplitInbox => {
                    self.upsert_synced_split(serde_json::from_value(payload.clone()).map_err(display)?)?
                }
                EntityType::Contact => self.upsert_synced_contact(payload)?,
                EntityType::Goal => {
                    self.upsert_synced_goal(&serde_json::from_value(payload.clone()).map_err(display)?)?
                }
                EntityType::MailAccount => self.upsert_synced_account(payload)?,
                EntityType::CalendarAccount => self.upsert_synced_calendar(payload)?,
                EntityType::CalendarSelection => self.upsert_synced_calendar_selection(payload)?,
                EntityType::Preferences => {
                    if let Some(fields) = payload.as_object() {
                        for (field, value) in fields.iter().filter(|(field, _)| field.starts_with("deviceName:")) {
                            let device_id_hex = &field["deviceName:".len()..];
                            self.materialize_device_name(device_id_hex, value.as_str())?;
                        }
                    }
                    self.with_connection(|connection| {
                        connection.execute(
                            "INSERT INTO synced_preferences(key,value,updated_at) VALUES('portable',?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                            params![payload.to_string(), Utc::now().to_rfc3339()],
                        )?;
                        Ok(())
                    })?
                }
                EntityType::Retention => self.set_retention_days(payload.get("days").and_then(Value::as_i64))?,
            }
        }
        Ok(())
    }

    /// Runs one single-id `DELETE` for a synced deletion.
    fn delete_synced_row(&self, sql: &str, entity_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(sql, [entity_id])?;
            Ok(())
        })
    }

    fn upsert_synced_task(&self, task: ThreadTask) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO tasks(id,account_id,thread_id,source_message_id,subject_snapshot,title,notes,kind,due_kind,due_value,time_zone,repeat_interval_days,status,completion_source,evidence_text,wait_after,created_at,updated_at,completed_at,goal_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20) ON CONFLICT(id) DO UPDATE SET goal_id=excluded.goal_id, account_id=excluded.account_id,thread_id=excluded.thread_id,source_message_id=excluded.source_message_id,subject_snapshot=excluded.subject_snapshot,title=excluded.title,notes=excluded.notes,kind=excluded.kind,due_kind=excluded.due_kind,due_value=excluded.due_value,time_zone=excluded.time_zone,repeat_interval_days=excluded.repeat_interval_days,status=excluded.status,completion_source=excluded.completion_source,evidence_text=excluded.evidence_text,wait_after=excluded.wait_after,updated_at=excluded.updated_at,completed_at=excluded.completed_at",
                params![
                    task.id,
                    task.account_id,
                    task.thread_id,
                    task.source_message_id,
                    task.subject_snapshot,
                    task.title,
                    task.notes,
                    task.kind,
                    task.due_kind,
                    task.due_value,
                    task.time_zone,
                    task.repeat_interval_days,
                    task.status,
                    task.completion_source,
                    task.evidence_text,
                    task.wait_after,
                    task.created_at,
                    task.updated_at,
                    task.completed_at,
                    task.goal_id
                ],
            )?;
            Ok(())
        })
    }

    pub(crate) fn upsert_synced_contact(&self, value: &Value) -> DbResult<()> {
        let item: ContactRecord = serde_json::from_value(value.clone()).map_err(display)?;
        // A device on a build before birthdays and keep-in-touch sends
        // records without those keys. Treat a missing key as "unknown to the
        // sender" and keep the local value rather than clearing it.
        let local = if value.get("birthday").is_none() || value.get("keepInTouch").is_none() {
            self.get_contact_profile(&item.id)?
        } else {
            None
        };
        let birthday = match (&local, value.get("birthday")) {
            (Some(local), None) => local.birthday.clone(),
            _ => item.birthday,
        };
        let keep_in_touch = match (&local, value.get("keepInTouch")) {
            (Some(local), None) => local.keep_in_touch.clone(),
            _ => item.keep_in_touch,
        };
        self.save_contact_profile(&crate::models::SaveContactRequest {
            id:Some(item.id),display_name:item.display_name,role:item.role,company:item.company,
            location:item.location,bio:item.bio,notes:item.notes,links:item.links,
            photo_data:item.photo_data,favorite:item.favorite,addresses:item.addresses,
            birthday,keep_in_touch:Some(keep_in_touch),
        }).map(|_|())
    }

    fn upsert_synced_snippet(&self, item: Snippet) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO snippets(id,name,body,created_at,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET name=excluded.name,body=excluded.body,updated_at=excluded.updated_at",
                params![item.id, item.name, item.body, item.created_at, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn upsert_synced_split(&self, item: SplitInbox) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO split_inboxes(id,name,match_kind,match_value,sort_order,created_at,account_id,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET name=excluded.name,match_kind=excluded.match_kind,match_value=excluded.match_value,sort_order=excluded.sort_order,account_id=excluded.account_id,updated_at=excluded.updated_at",
                params![
                    item.id,
                    item.name,
                    item.match_kind,
                    item.match_value,
                    item.sort_order,
                    item.created_at,
                    item.account_id,
                    Utc::now().to_rfc3339()
                ],
            )?;
            Ok(())
        })
    }

    fn upsert_synced_account(&self, value: &Value) -> DbResult<()> {
        let item: Account = serde_json::from_value(json!({
            "email": value["email"],
            "displayName": value.get("displayName").cloned().unwrap_or(Value::Null),
            "color": value["color"],
            "status": "needs_reauth",
            "provider": value["provider"],
            "sortOrder": value["sortOrder"],
            "connectedAt": Utc::now().to_rfc3339(),
            "lastSyncedAt": null,
        }))
        .map_err(display)?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO accounts(email,display_name,color,status,provider,sort_order,connected_at) VALUES(?1,?2,?3,'needs_reauth',?4,?5,?6) ON CONFLICT(email) DO UPDATE SET display_name=excluded.display_name,color=excluded.color,provider=excluded.provider,sort_order=excluded.sort_order",
                params![item.email, item.display_name, item.color, item.provider, item.sort_order, item.connected_at],
            )?;
            Ok(())
        })
    }

    fn upsert_synced_calendar(&self, value: &Value) -> DbResult<()> {
        let email = value.get("email").and_then(Value::as_str).ok_or("Invalid calendar account")?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO calendar_accounts(email,connected_at,status) VALUES(?1,?2,'needs_reauth') ON CONFLICT(email) DO NOTHING",
                params![email, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn upsert_synced_calendar_selection(&self, value: &Value) -> DbResult<()> {
        let email = value.get("accountId").and_then(Value::as_str).ok_or("Invalid calendar selection")?;
        let ids = value
            .get("calendarIds")
            .and_then(Value::as_array)
            .ok_or("Invalid calendar selection")?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if self.list_calendar_accounts()?.iter().any(|a| a.email == email) {
            self.set_calendar_selection(email, &ids)?;
        }
        Ok(())
    }

    /// The most recently materialized portable preferences, if any device
    /// has synchronized them yet.
    pub fn synced_preferences(&self) -> Result<Option<Value>, String> {
        let value = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT value FROM synced_preferences WHERE key='portable'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })?;
        let Some(value) = value else { return Ok(None) };
        let mut preferences: Value = serde_json::from_str(&value).map_err(display)?;
        if let Some(object) = preferences.as_object_mut() {
            object.retain(|key, _| !key.starts_with("deviceName:"));
            if object.is_empty() {
                return Ok(None);
            }
        }
        Ok(Some(preferences))
    }

    /// Saves a local portable preference edit. The replica and the snapshot
    /// read by sync-status refreshes commit together, so a completed save
    /// cannot be undone by reading the preceding materialized preferences.
    /// Returns whether a replica write was needed.
    pub fn update_synced_preferences(&self, preferences: Value) -> Result<bool, String> {
        let incoming = preferences
            .as_object()
            .ok_or_else(|| "Synced preferences must be an object".to_string())?;
        if !self.replicated_sync_active()? {
            return Ok(false);
        }
        self.with_transaction(|tx| {
            let stored: Option<String> = tx.query_row(
                "SELECT value FROM synced_preferences WHERE key='portable'",
                [],
                |row| row.get(0),
            ).optional()?;
            let current: Value = stored
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(display)?
                .unwrap_or_else(|| json!({}));
            let has_synced_record: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='preferences' AND entity_id='portable')",
                [],
                |row| row.get(0),
            )?;
            let fields = incoming.iter()
                .filter_map(|(key, value)| {
                    (!has_synced_record || current.get(key) != Some(value)).then_some(key.clone())
                })
                .collect::<BTreeSet<_>>();
            if fields.is_empty() {
                return Ok(false);
            }
            // This is an explicit local edit, even if another thread is
            // projecting remote entities. It must not be silently suppressed.
            crate::sync_state::record_replicated_write_in_transaction(
                tx, EntityType::Preferences, "portable", &fields, &preferences,
            )?;
            // Retain native-owned device names and fields from newer clients
            // that this frontend does not include in its portable allowlist.
            let mut updated = current.as_object().cloned().unwrap_or_default();
            updated.extend(incoming.clone());
            tx.execute(
                "INSERT INTO synced_preferences(key,value,updated_at) VALUES('portable',?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                params![Value::Object(updated).to_string(), Utc::now().to_rfc3339()],
            )?;
            Ok(true)
        })
        .map_err(String::from)
    }
}

fn clear_provider_credential(service: &str, key: &str) -> Result<(), String> {
    match Entry::new(service, key).map_err(display)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(display(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation_count(db: &Database, entity_id: &str) -> i64 {
        db.connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_values WHERE entity_id=?1",
                [entity_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn inactive_replicated_sync_records_no_local_writes() {
        let db = Database::open_memory();
        db.record_local_entity_write(EntityType::Snippet, "one", json!({"name": "n", "body": "b"}), None)
            .unwrap();
        db.record_local_entity_deletion(EntityType::Snippet, "one").unwrap();
        assert_eq!(operation_count(&db, "one"), 0);
    }

    #[test]
    fn active_replicated_sync_records_creations_updates_and_deletions() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();

        db.record_local_entity_write(EntityType::Snippet, "one", json!({"name": "n", "body": "b"}), None)
            .unwrap();
        // The existence marker plus one value per field.
        assert_eq!(operation_count(&db, "one"), 3);

        // An update to an already-recorded entity replaces only its changed
        // fields' values, independent of any account sign-in.
        db.record_local_entity_write(
            EntityType::Snippet,
            "one",
            json!({"name": "renamed", "body": "b"}),
            Some(BTreeSet::from(["name".to_string()])),
        )
        .unwrap();
        assert_eq!(operation_count(&db, "one"), 3);
        let counters: Vec<(String, i64)> = {
            let connection = db.connection().unwrap();
            let mut statement = connection.prepare("SELECT field, counter FROM sync_values WHERE entity_id='one' ORDER BY field").unwrap();
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
            rows
        };
        assert_eq!(counters, vec![("_entity".to_string(), 1), ("body".to_string(), 1), ("name".to_string(), 2)]);

        // A deletion removes every value.
        db.record_local_entity_deletion(EntityType::Snippet, "one").unwrap();
        assert_eq!(operation_count(&db, "one"), 0);
    }

    #[test]
    fn turning_replicated_sync_off_preserves_local_workflow_data() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        let snippet = db.create_snippet("Saved", "Still here").unwrap();
        db.record_local_entity_write(EntityType::Snippet, &snippet.id, serde_json::to_value(&snippet).unwrap(), None)
            .unwrap();
        db.set_beta_features_enabled(false).unwrap();
        assert_eq!(db.list_snippets().unwrap().len(), 1);
    }

    #[test]
    fn cross_device_sync_requires_an_active_beta_and_completed_enrollment() {
        let db = Database::open_memory();
        assert!(!db.cross_device_sync_enrolled().unwrap());
        db.set_beta_features_enabled(true).unwrap();
        // Active but never enrolled: nothing can reach another device yet.
        assert!(!db.cross_device_sync_enrolled().unwrap());
    }

    #[test]
    fn local_font_edits_are_visible_to_the_next_synced_preference_read() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        let original = json!({"fontFamily": "system", "fontScale": 100});
        db.update_synced_preferences(original.clone()).unwrap();
        // The stored snapshot from the preceding sync cycle.
        db.materialize_entity(EntityType::Preferences, "portable", Some(&original), false).unwrap();

        for family in ["Georgia", "Menlo", "system"] {
            let edited = json!({"fontFamily": family, "fontScale": 100});
            db.update_synced_preferences(edited.clone()).unwrap();
            assert_eq!(db.synced_preferences().unwrap(), Some(edited.clone()));
            assert_eq!(db.resolve_field_winner(EntityType::Preferences, "portable", "fontFamily").unwrap(), Some(json!(family)));
            // A status refresh republishes the snapshot it just read. It must
            // not create another write or restore the preceding font.
            let before = db.load_replica_state().unwrap();
            db.update_synced_preferences(db.synced_preferences().unwrap().unwrap()).unwrap();
            assert_eq!(db.load_replica_state().unwrap(), before);
            assert_eq!(db.synced_preferences().unwrap(), Some(edited));
        }
    }

    #[test]
    fn initial_local_preferences_are_readable_without_a_peer_sync_cycle() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        let preferences = json!({"fontFamily": "Georgia", "fontScale": 120});
        db.update_synced_preferences(preferences.clone()).unwrap();
        assert_eq!(db.synced_preferences().unwrap(), Some(preferences));
    }

    #[test]
    fn local_preferences_preserve_native_and_unknown_fields_and_write_only_changes() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        let original = json!({
            "fontFamily": "system", "fontScale": 100,
            "deviceName:123": "Laptop", "futurePreference": true,
        });
        db.update_synced_preferences(original).unwrap();
        let before = db.load_replica_state().unwrap();
        let preferences = json!({"fontFamily": "Georgia", "fontScale": 100});
        assert!(db.update_synced_preferences(preferences.clone()).unwrap());
        assert_eq!(db.synced_preferences().unwrap(), Some(json!({
            "fontFamily": "Georgia", "fontScale": 100, "futurePreference": true,
        })));
        let after = db.load_replica_state().unwrap();
        for (key, values) in before.fields() {
            if key.field != "fontFamily" {
                assert_eq!(after.values(key), values, "unchanged field {}", key.field);
            }
        }
        assert!(!db.update_synced_preferences(preferences).unwrap());
        assert_eq!(db.load_replica_state().unwrap(), after);
        let stored: String = db.connection().unwrap().query_row(
            "SELECT value FROM synced_preferences WHERE key='portable'", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap()["deviceName:123"], "Laptop");
    }

    #[test]
    fn inactive_sync_does_not_record_or_materialize_local_preferences() {
        let db = Database::open_memory();
        assert!(!db.update_synced_preferences(json!({"fontFamily": "Georgia"})).unwrap());
        assert_eq!(db.synced_preferences().unwrap(), None);
        assert_eq!(operation_count(&db, "portable"), 0);
    }

    #[test]
    fn explicit_local_preference_edits_are_not_suppressed_by_remote_projection() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        db.with_remote_projection(|| {
            assert!(db.update_synced_preferences(json!({"fontFamily": "Georgia"}))?);
            Ok(())
        }).unwrap();
        assert_eq!(db.synced_preferences().unwrap(), Some(json!({"fontFamily": "Georgia"})));
        assert_eq!(db.resolve_field_winner(EntityType::Preferences, "portable", "fontFamily").unwrap(), Some(json!("Georgia")));
    }

    #[test]
    fn a_failed_preference_snapshot_save_rolls_back_the_replica_write() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        db.update_synced_preferences(json!({"fontFamily": "system"})).unwrap();
        let before = db.load_replica_state().unwrap();
        db.connection().unwrap().execute_batch(
            "CREATE TRIGGER reject_preference_snapshot BEFORE UPDATE ON synced_preferences
             BEGIN SELECT RAISE(ABORT, 'snapshot write failed'); END;",
        ).unwrap();

        assert!(db.update_synced_preferences(json!({"fontFamily": "Georgia"})).is_err());
        assert_eq!(db.synced_preferences().unwrap(), Some(json!({"fontFamily": "system"})));
        assert_eq!(db.load_replica_state().unwrap(), before);
    }

    #[test]
    fn a_peers_older_snapshot_does_not_restore_the_preceding_font() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        db.update_synced_preferences(json!({"fontFamily": "system"})).unwrap();
        let peer = Database::open_memory();
        let touched = peer.merge_replica_state(&db.load_replica_state().unwrap()).unwrap();
        peer.materialize_touched_entities(&touched).unwrap();
        let older = peer.load_replica_state().unwrap();

        db.update_synced_preferences(json!({"fontFamily": "Georgia"})).unwrap();
        let touched = db.merge_replica_state(&older).unwrap();
        db.materialize_touched_entities(&touched).unwrap();
        assert_eq!(db.synced_preferences().unwrap(), Some(json!({"fontFamily": "Georgia"})));
        let touched = peer.merge_replica_state(&db.load_replica_state().unwrap()).unwrap();
        peer.materialize_touched_entities(&touched).unwrap();
        assert_eq!(peer.synced_preferences().unwrap(), Some(json!({"fontFamily": "Georgia"})));
    }

    fn record_full(db: &Database, entity_type: EntityType, id: &str, payload: Value) {
        let fields = payload.as_object().unwrap().keys().cloned().collect();
        db.record_replicated_write(entity_type, id, &fields, &payload).unwrap();
    }

    fn sync_into(peer: &Database, from: &Database) {
        let touched = peer.merge_replica_state(&from.load_replica_state().unwrap()).unwrap();
        peer.materialize_touched_entities(&touched).unwrap();
    }

    #[test]
    fn goals_and_task_goal_links_sync_and_a_goal_deletion_unlinks_its_tasks_on_peers() {
        let db = Database::open_memory();
        let goal = db.create_goal(&crate::models::CreateGoalRequest {
            account_id: "you@example.com".into(), title: "Ship IMAP".into(), notes: Some("Phase 2".into()),
            horizon: "quarter".into(), period: "2026-Q4".into(), parent_goal_id: None,
        }).unwrap();
        record_full(&db, EntityType::Goal, &goal.id, serde_json::to_value(&goal).unwrap());
        let task = db.create_task(&crate::models::CreateTaskRequest {
            account_id: "you@example.com".into(), thread_id: None, source_message_id: None, subject_snapshot: None,
            title: "Write the design".into(), notes: None, kind: "action".into(), due_kind: "none".into(),
            due_value: None, time_zone: None, repeat_interval_days: None, evidence_text: None, goal_id: Some(goal.id.clone()),
        }).unwrap();
        let task = db.set_task_status(&task.id, "in_progress", "user").unwrap();
        record_full(&db, EntityType::Task, &task.id, serde_json::to_value(&task).unwrap());
        // A task written by a version from before goals carries no goalId at all.
        let mut legacy = serde_json::to_value(&task).unwrap();
        legacy.as_object_mut().unwrap().remove("goalId");
        legacy["id"] = json!("legacy-task");
        record_full(&db, EntityType::Task, "legacy-task", legacy);

        let peer = Database::open_memory();
        sync_into(&peer, &db);
        assert_eq!(peer.list_goals(Some("you@example.com")).unwrap(), vec![goal.clone()]);
        let peer_tasks = peer.list_tasks(None, None).unwrap();
        let synced = peer_tasks.iter().find(|candidate| candidate.id == task.id).unwrap();
        assert_eq!(synced.goal_id.as_deref(), Some(goal.id.as_str()));
        assert_eq!(synced.status, "in_progress");
        assert_eq!(peer_tasks.iter().find(|candidate| candidate.id == "legacy-task").unwrap().goal_id, None);

        let deletion = db.delete_goal(&goal.id).unwrap();
        db.record_replicated_deletion(EntityType::Goal, &goal.id).unwrap();
        for unlinked in &deletion.tasks {
            let payload = serde_json::to_value(unlinked).unwrap();
            db.record_replicated_write(EntityType::Task, &unlinked.id, &["goalId".to_string(), "updatedAt".to_string()].into(), &payload).unwrap();
        }
        sync_into(&peer, &db);
        assert!(peer.list_goals(None).unwrap().is_empty());
        assert_eq!(peer.list_tasks(None, None).unwrap().iter().find(|candidate| candidate.id == task.id).unwrap().goal_id, None);
        assert!(peer.list_frontier_conflicts().unwrap().is_empty());
    }

    #[test]
    fn synced_preferences_round_trip_through_materialization() {
        let db = Database::open_memory();
        assert_eq!(db.synced_preferences().unwrap(), None);
        db.materialize_entity(EntityType::Preferences, "portable", Some(&json!({"theme": "dark"})), false)
            .unwrap();
        assert_eq!(db.synced_preferences().unwrap(), Some(json!({"theme": "dark"})));
    }
}
