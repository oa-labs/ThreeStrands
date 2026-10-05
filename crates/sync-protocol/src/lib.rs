//! The synchronized entity vocabulary shared by every replicated-sync crate:
//! which application-owned entity types may cross a device boundary, and the
//! payload contract each one must satisfy. Mail bodies, credentials, and
//! other device-only data have no entity type here, so no sync path can
//! carry them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
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
    Contact,
    // New variants go last: the derived order is the snapshots' canonical field order.
    Goal,
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
            Self::Contact => "contact",
            Self::Goal => "goal",
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
                enum_string(object, "status", &["open", "in_progress", "completed", "cancelled"])?;
                optional_string(object, "notes", 8_000)?;
                optional_string(object, "evidenceText", 4_000)?;
                optional_string(object, "goalId", 128)?;
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
            Self::Goal => {
                required_string(object, "title", 240)?;
                required_string(object, "accountId", 320)?;
                enum_string(object, "status", &["active", "achieved", "dropped"])?;
                let horizon = object.get("horizon").and_then(Value::as_str).unwrap_or_default();
                let period = object.get("period").and_then(Value::as_str).unwrap_or_default();
                if !goal_period_matches(horizon, period) {
                    return Err("The goal horizon or period is invalid".to_string());
                }
                optional_string(object, "notes", 8_000)?;
                optional_string(object, "parentGoalId", 128)?;
            }
            Self::Contact => {
                required_string(object, "id", 128)?;
                optional_string(object, "displayName", 200)?;
                optional_string(object, "role", 200)?;
                optional_string(object, "company", 200)?;
                optional_string(object, "location", 200)?;
                optional_string(object, "bio", 4_000)?;
                optional_string(object, "notes", 8_000)?;
                optional_string(object, "photoData", 90_000)?;
                let addresses = object.get("addresses").and_then(Value::as_array).ok_or_else(|| "addresses must be an array".to_string())?;
                if addresses.is_empty() || addresses.len() > 100 || addresses.iter().any(|value| value.as_str().is_none_or(|email| email.len() > 320 || !email.contains('@'))) {
                    return Err("addresses are invalid".to_string());
                }
                let links = object.get("links").and_then(Value::as_array).ok_or_else(|| "links must be an array".to_string())?;
                if links.len() > 20 || links.iter().any(|value| value.as_str().is_none_or(|link| link.len() > 2_048 || !link.starts_with("https://"))) {
                    return Err("links are invalid".to_string());
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
            "contact" => Ok(Self::Contact),
            "goal" => Ok(Self::Goal),
            _ => Err("Unknown synchronized entity type".to_string()),
        }
    }
}

/// Whether `period` names one period of `horizon`: `2026` for a year,
/// `2026-H2` for a half, `2026-Q4` for a quarter.
pub fn goal_period_matches(horizon: &str, period: &str) -> bool {
    let bytes = period.as_bytes();
    let year = bytes.len() >= 4 && bytes[..4].iter().all(u8::is_ascii_digit);
    match horizon {
        "year" => year && bytes.len() == 4,
        "half" => year && bytes.len() == 7 && &bytes[4..6] == b"-H" && matches!(bytes[6], b'1' | b'2'),
        "quarter" => year && bytes.len() == 7 && &bytes[4..6] == b"-Q" && matches!(bytes[6], b'1'..=b'4'),
        _ => false,
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
    fn every_entity_type_parses_back_from_its_stored_name() {
        // Adding a variant fails to compile here until it is listed below.
        let listed = |entity_type: EntityType| match entity_type {
            EntityType::Task
            | EntityType::Snippet
            | EntityType::SplitInbox
            | EntityType::MailAccount
            | EntityType::CalendarAccount
            | EntityType::CalendarSelection
            | EntityType::Preferences
            | EntityType::Retention
            | EntityType::Contact
            | EntityType::Goal => entity_type,
        };
        for entity_type in [
            EntityType::Task,
            EntityType::Snippet,
            EntityType::SplitInbox,
            EntityType::MailAccount,
            EntityType::CalendarAccount,
            EntityType::CalendarSelection,
            EntityType::Preferences,
            EntityType::Retention,
            EntityType::Contact,
            EntityType::Goal,
        ] {
            let entity_type = listed(entity_type);
            assert_eq!(entity_type.as_str().parse::<EntityType>(), Ok(entity_type));
        }
        assert!("unknown".parse::<EntityType>().is_err());
    }

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
    fn accepts_in_progress_tasks_and_an_optional_goal_link() {
        let in_progress = json!({ "title": "Draft", "kind": "action", "dueKind": "none", "status": "in_progress" });
        assert!(EntityType::Task.validate_payload(&in_progress).is_ok());
        // Payloads from versions before goals have no goalId at all.
        let linked = json!({ "title": "Draft", "kind": "action", "dueKind": "none", "status": "open", "goalId": "goal-1" });
        assert!(EntityType::Task.validate_payload(&linked).is_ok());
        let unlinked = json!({ "title": "Draft", "kind": "action", "dueKind": "none", "status": "open", "goalId": null });
        assert!(EntityType::Task.validate_payload(&unlinked).is_ok());
        let oversized = json!({ "title": "Draft", "kind": "action", "dueKind": "none", "status": "open", "goalId": "g".repeat(129) });
        assert!(EntityType::Task.validate_payload(&oversized).is_err());
    }

    #[test]
    fn validates_goal_contract() {
        let valid = json!({
            "id": "goal-1", "accountId": "you@example.com", "title": "Ship the IMAP provider",
            "horizon": "quarter", "period": "2026-Q4", "status": "active", "notes": null, "parentGoalId": null
        });
        assert!(EntityType::Goal.validate_payload(&valid).is_ok());
        for (field, value) in [
            ("title", json!(" ")),
            ("accountId", json!(null)),
            ("status", json!("done")),
            ("horizon", json!("month")),
            ("period", json!("2026-H2")),
            ("notes", json!("x".repeat(8_001))),
            ("parentGoalId", json!(7)),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(EntityType::Goal.validate_payload(&invalid).is_err(), "{field} should be rejected");
        }
    }

    #[test]
    fn goal_periods_match_their_horizon() {
        assert!(goal_period_matches("year", "2026"));
        assert!(goal_period_matches("half", "2026-H1"));
        assert!(goal_period_matches("half", "2026-H2"));
        assert!(goal_period_matches("quarter", "2026-Q1"));
        assert!(goal_period_matches("quarter", "2026-Q4"));
        for (horizon, period) in [
            ("year", "26"), ("year", "2026-Q1"), ("half", "2026-H0"), ("half", "2026-H3"),
            ("quarter", "2026-Q0"), ("quarter", "2026-Q5"), ("quarter", "2026-q4"), ("quarter", "20x6-Q4"), ("week", "2026"),
        ] {
            assert!(!goal_period_matches(horizon, period), "{horizon} {period}");
        }
    }

    #[test]
    fn validates_contact_sync_payload_and_bounded_photo() {
        let valid=json!({"id":"contact:jane@example.com","displayName":"Jane","role":null,"company":null,"location":null,"bio":null,"notes":null,"links":[],"photoData":null,"favorite":false,"addresses":["jane@example.com"],"sentCount":0,"receivedCount":0,"lastInteractedAt":null});
        assert!(EntityType::Contact.validate_payload(&valid).is_ok());
        let mut invalid=valid.clone();
        invalid["addresses"]=json!([]);
        assert!(EntityType::Contact.validate_payload(&invalid).is_err());
        invalid=valid;
        invalid["photoData"]=json!("x".repeat(90_001));
        assert!(EntityType::Contact.validate_payload(&invalid).is_err());
    }
}
