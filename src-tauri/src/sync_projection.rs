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

use keyring::Entry;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use threestrands_sync_protocol::EntityType;
use crate::models::ContactRecord;

use crate::{
    db::{serialization_error, Database, DatabaseError, DbResult},
    enrollment::EnrollmentStatus,
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
    ) -> DbResult<()> {
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
    pub fn record_local_entity_deletion(&self, entity_type: EntityType, entity_id: &str) -> DbResult<()> {
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
                EntityType::ScheduledSendSummary => self.delete_scheduled_summary(entity_id)?,
                EntityType::Task => self.delete_synced_task(entity_id)?,
                EntityType::Signature => self.delete_signature(entity_id)?,
                EntityType::SignatureDefaults => self.delete_signature_defaults(entity_id)?,
                // Drafts reach other devices through the account's server
                // drafts now. Records from earlier versions are ignored, so
                // a device that hasn't updated can't remove a draft here.
                EntityType::Draft => {}
                EntityType::Snippet => self.delete_synced_snippet(entity_id)?,
                EntityType::SplitInbox => self.delete_synced_split(entity_id)?,
                EntityType::SenderConfirmation => self.forget_sender_delivery(entity_id)?,
                EntityType::SenderRule => self.delete_synced_sender_rule(entity_id)?,
                EntityType::Contact => self.delete_contact_profile(entity_id)?,
                // Unlinks local tasks and goals the same way the deleting device did.
                EntityType::Goal => {
                    self.delete_goal(entity_id)?;
                }
                EntityType::ContactGroup => self.delete_contact_group(entity_id)?,
                EntityType::MailAccount => self.remove_synced_mail_account(entity_id, |key| {
                    clear_provider_credential("app.threestrands.mail", key)
                })?,
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
                EntityType::ScheduledSendSummary => {
                    entity_type.validate_payload(payload).map_err(DatabaseError::invalid)?;
                    let report: threestrands_sync_protocol::ScheduledSendReport = serde_json::from_value(payload["report"].clone()).map_err(serialization_error)?;
                    if report.entity_id()!=entity_id { return Err(DatabaseError::invalid("Scheduled summary identity mismatch")); }
                    self.upsert_scheduled_summary(entity_id, &payload["report"].to_string())?;
                }
                EntityType::Task => {
                    self.upsert_synced_task(serde_json::from_value(payload.clone()).map_err(serialization_error)?)?
                }
                EntityType::Signature => {
                    let item = crate::signatures::signature_from_sync(payload).map_err(DatabaseError::invalid)?;
                    if item.id != entity_id { return Err(DatabaseError::invalid("Signature identity mismatch")); }
                    self.store_signature(&item)?;
                }
                EntityType::SignatureDefaults => {
                    let item: crate::signatures::SignatureDefaults = serde_json::from_value(payload.clone()).map_err(serialization_error)?;
                    if item.id != entity_id { return Err(DatabaseError::invalid("Signature defaults identity mismatch")); }
                    self.store_signature_defaults(&item)?;
                }
                EntityType::Draft => {}
                EntityType::Snippet => {
                    self.upsert_synced_snippet(serde_json::from_value(payload.clone()).map_err(serialization_error)?)?
                }
                EntityType::SplitInbox => {
                    self.upsert_synced_split(serde_json::from_value(payload.clone()).map_err(serialization_error)?)?
                }
                EntityType::SenderConfirmation => {
                    entity_type.validate_payload(payload).map_err(DatabaseError::invalid)?;
                    let record: crate::db::sender_feedback::SenderConfirmation = serde_json::from_value(payload["pattern"].clone()).map_err(serialization_error)?;
                    if record.id != entity_id { return Err(DatabaseError::invalid("Sender confirmation identity mismatch")); }
                    self.upsert_synced_sender_confirmation(&record)?;
                }
                EntityType::SenderRule => {
                    entity_type.validate_payload(payload).map_err(DatabaseError::invalid)?;
                    let rule: crate::models::SenderRule = serde_json::from_value(payload.clone()).map_err(serialization_error)?;
                    if rule.id != entity_id { return Err(DatabaseError::invalid("Mute rule identity mismatch")); }
                    self.upsert_synced_sender_rule(&rule)?
                }
                EntityType::Contact => self.upsert_synced_contact(payload)?,
                EntityType::Goal => {
                    self.upsert_synced_goal(&serde_json::from_value(payload.clone()).map_err(serialization_error)?)?
                }
                EntityType::ContactGroup => self.upsert_synced_contact_group(entity_id, payload)?,
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
                    self.store_synced_preferences(&payload.to_string())?
                }
                EntityType::Retention => self.set_retention_days(payload.get("days").and_then(Value::as_i64))?,
            }
        }
        Ok(())
    }

    pub(crate) fn upsert_synced_contact(&self, value: &Value) -> DbResult<()> {
        let item: ContactRecord = serde_json::from_value(value.clone()).map_err(serialization_error)?;
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

    fn upsert_synced_calendar_selection(&self, value: &Value) -> DbResult<()> {
        let email = value.get("accountId").and_then(Value::as_str).ok_or_else(|| DatabaseError::corrupt("Invalid calendar selection"))?;
        let ids = value
            .get("calendarIds")
            .and_then(Value::as_array)
            .ok_or_else(|| DatabaseError::corrupt("Invalid calendar selection"))?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if self.list_calendar_accounts()?.iter().any(|a| a.email == email) {
            self.set_calendar_selection(email, &ids)?;
        }
        Ok(())
    }

    /// Applies a synchronized mail-account deletion. The id is lowercased,
    /// while the local row and its credential keep the address's typed case
    /// (see `catalogued_account_email`), so both spellings are cleared.
    /// Credentials go first: a failure leaves the row for the retry to find.
    fn remove_synced_mail_account(
        &self,
        entity_id: &str,
        clear_credential: impl Fn(&str) -> DbResult<()>,
    ) -> DbResult<()> {
        let stored = self.catalogued_account_email(entity_id)?;
        clear_credential(entity_id)?;
        if let Some(email) = stored {
            if email != entity_id {
                clear_credential(&email)?;
            }
            self.remove_account(&email)?;
        }
        Ok(())
    }
}

fn clear_provider_credential(service: &str, key: &str) -> DbResult<()> {
    let keychain = |error: keyring::Error| DatabaseError::Keychain(error.to_string());
    match Entry::new(service, key).map_err(keychain)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(keychain(error)),
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
    fn a_synced_calendar_account_takes_the_order_another_device_chose() {
        let db = Database::open_memory();
        db.adopt_calendar_account("home@example.com").unwrap();
        db.adopt_calendar_account("work@example.com").unwrap();
        let project = |email: &str, payload: Value| {
            db.materialize_entity(EntityType::CalendarAccount, email, Some(&payload), false).unwrap();
        };
        let emails = || db.list_calendar_accounts().unwrap().into_iter().map(|account| account.email).collect::<Vec<_>>();

        project("work@example.com", json!({"email": "work@example.com", "sortOrder": 0}));
        project("home@example.com", json!({"email": "home@example.com", "sortOrder": 1}));
        assert_eq!(emails(), ["work@example.com", "home@example.com"]);

        // An older device sends no order: a new account goes last and an
        // existing one stays where it is.
        project("side@example.com", json!({"email": "side@example.com"}));
        project("work@example.com", json!({"email": "work@example.com"}));
        assert_eq!(emails(), ["work@example.com", "home@example.com", "side@example.com"]);
    }

    #[test]
    fn a_synced_mail_account_deletion_removes_an_address_typed_with_capitals() {
        let db = Database::open_memory();
        db.adopt_account("Joel@Proton.me").unwrap();
        db.create_split_inbox("Mine", "domain", "acme.com", "Joel@Proton.me").unwrap();
        db.adopt_account("other@proton.me").unwrap();
        let cleared = std::cell::RefCell::new(Vec::new());

        // The synced id is the lowercased address.
        db.remove_synced_mail_account("joel@proton.me", |key| {
            cleared.borrow_mut().push(key.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(cleared.into_inner(), ["joel@proton.me", "Joel@Proton.me"]);
        let left = db.list_accounts().unwrap().into_iter().map(|account| account.email).collect::<Vec<_>>();
        assert_eq!(left, ["other@proton.me"]);
        assert!(db.list_split_inboxes().unwrap().is_empty());
    }

    #[test]
    fn a_synced_mail_account_deletion_clears_the_credential_once_for_a_lowercase_address() {
        let db = Database::open_memory();
        db.adopt_account("you@example.com").unwrap();
        let cleared = std::cell::RefCell::new(Vec::new());
        db.remove_synced_mail_account("you@example.com", |key| {
            cleared.borrow_mut().push(key.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(cleared.into_inner(), ["you@example.com"]);
        assert!(db.get_account("you@example.com").unwrap().is_none());

        // An account this device never had still clears its credential.
        let cleared = std::cell::RefCell::new(Vec::new());
        db.remove_synced_mail_account("gone@example.com", |key| {
            cleared.borrow_mut().push(key.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(cleared.into_inner(), ["gone@example.com"]);
    }

    #[test]
    fn a_failed_credential_clear_keeps_the_account_for_the_retry() {
        let db = Database::open_memory();
        db.adopt_account("Joel@Proton.me").unwrap();
        let result = db.remove_synced_mail_account("joel@proton.me", |key| {
            if key == "Joel@Proton.me" { Err(DatabaseError::Keychain("locked".into())) } else { Ok(()) }
        });
        assert!(result.is_err());
        assert!(db.get_account("Joel@Proton.me").unwrap().is_some());
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
    fn signature_templates_and_alias_defaults_materialize_update_and_delete_independently() {
        use crate::signatures::tests::{template, defaults};
        let db = Database::open_memory();
        let signature = serde_json::to_value(template("work")).unwrap();
        let defaults = defaults("me@example.com", "alias@example.com", Some("work"), None);
        db.materialize_entity(EntityType::SignatureDefaults, &defaults.id, Some(&serde_json::to_value(&defaults).unwrap()), false).unwrap();
        db.materialize_entity(EntityType::Signature, "work", Some(&signature), false).unwrap();
        assert_eq!(db.default_draft_signature("me@example.com", "alias@example.com", "new").unwrap().unwrap().text, "work");
        let mut edited = signature.clone(); edited["html"] = json!("<b>Updated</b>"); edited["text"] = json!("Updated");
        db.materialize_entity(EntityType::Signature, "work", Some(&edited), false).unwrap();
        assert_eq!(db.signature_library().unwrap().signatures[0].text, "Updated");
        db.materialize_entity(EntityType::Signature, "work", None, true).unwrap();
        assert!(db.default_draft_signature("me@example.com", "alias@example.com", "new").unwrap().is_none());
        assert_eq!(db.signature_library().unwrap().defaults.len(), 1);
        db.materialize_entity(EntityType::SignatureDefaults, &defaults.id, None, true).unwrap();
        assert!(db.signature_library().unwrap().defaults.is_empty());
        assert!(db.materialize_entity(EntityType::Signature, "wrong-id", Some(&signature), false).is_err());
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
            Ok::<_, String>(())
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
    fn a_muted_sender_syncs_to_peers_and_an_unmute_removes_it_there() {
        let db = Database::open_memory();
        db.adopt_account("you@example.com").unwrap();
        let rule = db.mute_sender("you@example.com", "domain", "launch.example", &[]).unwrap().rule;
        record_full(&db, EntityType::SenderRule, &rule.id, serde_json::to_value(&rule).unwrap());

        let peer = Database::open_memory();
        sync_into(&peer, &db);
        assert_eq!(peer.list_sender_rules().unwrap(), vec![rule.clone()]);

        db.delete_sender_rule(&rule.id).unwrap();
        db.record_replicated_deletion(EntityType::SenderRule, &rule.id).unwrap();
        sync_into(&peer, &db);
        assert!(peer.list_sender_rules().unwrap().is_empty());
    }

    #[test]
    fn a_synced_mute_rule_must_match_its_entity_and_contract() {
        let db = Database::open_memory();
        let payload = json!({
            "id": "rule-1", "accountId": "you@example.com", "matchKind": "address",
            "matchValue": "news@launch.example", "action": "mute", "createdAt": "2026-10-14T00:00:00Z",
        });
        assert!(db.materialize_entity(EntityType::SenderRule, "other-id", Some(&payload), false).is_err());
        let mut pattern = payload.clone();
        pattern["matchKind"] = json!("pattern");
        assert!(db.materialize_entity(EntityType::SenderRule, "rule-1", Some(&pattern), false).is_err());
        assert!(db.list_sender_rules().unwrap().is_empty());
        db.materialize_entity(EntityType::SenderRule, "rule-1", Some(&payload), false).unwrap();
        assert_eq!(db.list_sender_rules().unwrap()[0].match_value, "news@launch.example");
        db.materialize_entity(EntityType::SenderRule, "rule-1", None, true).unwrap();
        assert!(db.list_sender_rules().unwrap().is_empty());
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
