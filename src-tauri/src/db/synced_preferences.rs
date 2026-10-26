//! The portable preferences replicated across devices, as last materialized
//! here, plus this device's shared name entries inside them.

use std::collections::BTreeSet;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use threestrands_sync_protocol::EntityType;

use super::{serialization_error, Database, DbResult};
use crate::error_text::display;

/// Replaces the stored portable preferences with `value`.
fn write_synced_preferences(connection: &Connection, value: &str) -> DbResult<()> {
    connection.execute(
        "INSERT INTO synced_preferences(key,value,updated_at) VALUES('portable',?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
        params![value, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

impl Database {
    /// Stores portable preferences another device wrote.
    pub(crate) fn store_synced_preferences(&self, value: &str) -> DbResult<()> {
        self.with_connection(|connection| write_synced_preferences(connection, value))
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
                .map_err(serialization_error)?
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
            crate::db::sync_state::record_replicated_write_in_transaction(
                tx, EntityType::Preferences, "portable", &fields, &preferences,
            )?;
            // Retain native-owned device names and fields from newer clients
            // that this frontend does not include in its portable allowlist.
            let mut updated = current.as_object().cloned().unwrap_or_default();
            updated.extend(incoming.clone());
            write_synced_preferences(tx, &Value::Object(updated).to_string())?;
            Ok(true)
        })
        .map_err(String::from)
    }
}
