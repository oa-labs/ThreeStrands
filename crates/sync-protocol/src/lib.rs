//! The synchronized entity vocabulary shared by every replicated-sync crate:
//! which application-owned entity types may cross a device boundary, and the
//! payload contract each one must satisfy. Mail bodies, credentials, and
//! other device-only data have no entity type here, so no sync path can
//! carry them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    for (key, value) in object
        .iter()
        .filter(|(key, _)| key.starts_with("deviceName:"))
    {
        let device_id = &key["deviceName:".len()..];
        if device_id.len() != 32 || !device_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("A synchronized device name has an invalid device id".to_string());
        }
        if !matches!(value, Value::Null)
            && value
                .as_str()
                .is_none_or(|name| name.trim().is_empty() || name.chars().count() > 60)
        {
            return Err("A synchronized device name is invalid".to_string());
        }
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
}
