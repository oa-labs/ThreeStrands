use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const API_VERSION: &str = "v1";
pub const MAX_OPERATIONS_PER_REQUEST: usize = 500;
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Task,
    Snippet,
    SplitInbox,
    MailAccount,
    CalendarAccount,
    CalendarSelection,
    Preferences,
    Retention,
}

impl EntityType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Snippet => "snippet",
            Self::SplitInbox => "split_inbox",
            Self::MailAccount => "mail_account",
            Self::CalendarAccount => "calendar_account",
            Self::CalendarSelection => "calendar_selection",
            Self::Preferences => "preferences",
            Self::Retention => "retention",
        }
    }

    pub fn validate_payload(self, payload: &Value) -> Result<(), String> {
        let object = payload
            .as_object()
            .ok_or_else(|| "A synchronized record must be a JSON object".to_string())?;
        if serde_json::to_vec(payload).map_err(display)?.len() > MAX_PAYLOAD_BYTES {
            return Err("The synchronized record is too large".to_string());
        }
        match self {
            Self::Task => {
                required_string(object, "title", 240)?;
                enum_string(object, "kind", &["action", "follow_up", "waiting_for"])?;
                enum_string(object, "dueKind", &["none", "date", "datetime"])?;
                enum_string(object, "status", &["open", "completed", "cancelled"])?;
                optional_string(object, "notes", 8_000)?;
                optional_string(object, "evidenceText", 4_000)?;
            }
            Self::Snippet => {
                required_string(object, "name", 200)?;
                required_string(object, "body", 32_000)?;
            }
            Self::SplitInbox => {
                required_string(object, "name", 200)?;
                enum_string(object, "matchKind", &["domain", "label", "pattern"])?;
                required_string(object, "matchValue", 2_048)?;
                required_string(object, "accountId", 320)?;
            }
            Self::MailAccount => {
                required_string(object, "email", 320)?;
                required_string(object, "provider", 40)?;
                required_string(object, "color", 20)?;
            }
            Self::CalendarAccount => required_string(object, "email", 320)?,
            Self::CalendarSelection => {
                required_string(object, "accountId", 320)?;
                let values = object
                    .get("calendarIds")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "calendarIds must be an array".to_string())?;
                if values.len() > 500
                    || values.iter().any(|value| {
                        value.as_str().is_none_or(|value| value.len() > 1_000)
                    })
                {
                    return Err("calendarIds is invalid".to_string());
                }
            }
            Self::Preferences => validate_preferences(object)?,
            Self::Retention => {
                if !matches!(object.get("days"), Some(Value::Null) | Some(Value::Number(_))) {
                    return Err("Retention days must be a number or null".to_string());
                }
            }
        }
        Ok(())
    }
}

impl std::str::FromStr for EntityType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "task" => Ok(Self::Task),
            "snippet" => Ok(Self::Snippet),
            "split_inbox" => Ok(Self::SplitInbox),
            "mail_account" => Ok(Self::MailAccount),
            "calendar_account" => Ok(Self::CalendarAccount),
            "calendar_selection" => Ok(Self::CalendarSelection),
            "preferences" => Ok(Self::Preferences),
            "retention" => Ok(Self::Retention),
            _ => Err("Unknown synchronized entity type".to_string()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncOperation {
    pub operation_id: String,
    pub device_id: String,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub base_version: i64,
    pub changed_fields: BTreeSet<String>,
    pub patch: Option<Value>,
    #[serde(default)]
    pub deleted: bool,
    pub local_sequence: i64,
}

impl SyncOperation {
    pub fn validate(&self) -> Result<(), String> {
        bounded_id("operation id", &self.operation_id)?;
        bounded_id("device id", &self.device_id)?;
        bounded_id("entity id", &self.entity_id)?;
        if self.base_version < 0 || self.local_sequence < 0 {
            return Err("Sync versions cannot be negative".to_string());
        }
        if self.changed_fields.is_empty() || self.changed_fields.len() > 100 {
            return Err("A sync operation must name its changed fields".to_string());
        }
        if self.deleted {
            if self.patch.is_some() || !self.changed_fields.contains("*") {
                return Err("A deletion must use the wildcard field and no patch".to_string());
            }
        } else {
            let patch = self
                .patch
                .as_ref()
                .ok_or_else(|| "A sync update must include a patch".to_string())?;
            let object = patch
                .as_object()
                .ok_or_else(|| "A sync patch must be an object".to_string())?;
            if object.keys().any(|key| !self.changed_fields.contains(key)) {
                return Err("The patch contains an undeclared changed field".to_string());
            }
            if self.base_version == 0 {
                self.entity_type.validate_payload(patch)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRequest {
    pub cursor: i64,
    #[serde(default)]
    pub operations: Vec<SyncOperation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecord {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub version: i64,
    pub payload: Option<Value>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub id: String,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub current_version: i64,
    pub overlapping_fields: BTreeSet<String>,
    pub cloud_payload: Option<Value>,
    pub cloud_deleted: bool,
    pub device_patch: Option<Value>,
    pub device_deleted: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationAck {
    pub operation_id: String,
    pub status: OperationStatus,
    pub version: Option<i64>,
    pub conflict_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Applied,
    Conflict,
    Duplicate,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResponse {
    pub acknowledgements: Vec<OperationAck>,
    pub changes: Vec<SyncRecord>,
    pub conflicts: Vec<SyncConflict>,
    pub next_cursor: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveConflictRequest {
    pub current_version: i64,
    pub resolved_payload: Option<Value>,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entitlement {
    pub feature: String,
    pub source: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub entitlements: Vec<Entitlement>,
}

pub fn merge_patch(target: &mut Value, patch: &Value) -> Result<(), String> {
    let target = target
        .as_object_mut()
        .ok_or_else(|| "Stored synchronized record is invalid".to_string())?;
    let patch = patch
        .as_object()
        .ok_or_else(|| "A sync patch must be an object".to_string())?;
    for (key, value) in patch {
        target.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn validate_preferences(object: &Map<String, Value>) -> Result<(), String> {
    if let Some(theme) = object.get("theme") {
        if !matches!(theme.as_str(), Some("light" | "dark" | "system")) {
            return Err("The synchronized theme is invalid".to_string());
        }
    }
    if let Some(scale) = object.get("fontScale").and_then(Value::as_i64) {
        if !(80..=140).contains(&scale) {
            return Err("The synchronized font scale is invalid".to_string());
        }
    }
    optional_string(object, "fontFamily", 200)?;
    optional_string(object, "aiModel", 2_048)?;
    optional_string(object, "aiEndpoint", 2_048)?;
    Ok(())
}

fn bounded_id(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 200 || value.chars().any(char::is_control) {
        return Err(format!("The {label} is invalid"));
    }
    Ok(())
}

fn required_string(object: &Map<String, Value>, key: &str, max: usize) -> Result<(), String> {
    let value = object.get(key).and_then(Value::as_str).unwrap_or_default();
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(format!("{key} is invalid"));
    }
    Ok(())
}

fn optional_string(object: &Map<String, Value>, key: &str, max: usize) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        if !value.is_null()
            && value
                .as_str()
                .is_none_or(|value| value.chars().count() > max)
        {
            return Err(format!("{key} is invalid"));
        }
    }
    Ok(())
}

fn enum_string(object: &Map<String, Value>, key: &str, values: &[&str]) -> Result<(), String> {
    let value = object.get(key).and_then(Value::as_str).unwrap_or_default();
    if !values.contains(&value) {
        return Err(format!("{key} is invalid"));
    }
    Ok(())
}

fn display(value: impl std::fmt::Display) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_task_contract_and_rejects_unknown_status() {
        let valid = json!({
            "title": "Follow up",
            "kind": "follow_up",
            "dueKind": "none",
            "status": "open"
        });
        assert!(EntityType::Task.validate_payload(&valid).is_ok());
        let invalid = json!({ "title": "x", "kind": "action", "dueKind": "none", "status": "lost" });
        assert!(EntityType::Task.validate_payload(&invalid).is_err());
    }

    #[test]
    fn merges_only_top_level_declared_fields() {
        let mut target = json!({"title":"old", "notes":"keep"});
        merge_patch(&mut target, &json!({"title":"new"})).unwrap();
        assert_eq!(target, json!({"title":"new", "notes":"keep"}));
    }
}
