//! The synchronized entity vocabulary shared by every replicated-sync crate:
//! which application-owned entity types may cross a device boundary, and the
//! payload contract each one must satisfy. Compose drafts are the one kind
//! of authored mail that syncs. Received mail, credentials, attachment
//! bytes and other device-only data have no entity type here, so no sync
//! path can carry them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const MAX_SIGNATURE_HTML_BYTES: usize = 180_000;
pub const MAX_SIGNATURE_TEXT_BYTES: usize = 32_000;

pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
/// Most serialized content one synchronized draft may carry. A larger draft
/// stays on its own device.
pub const MAX_SYNCED_DRAFT_BYTES: usize = 512 * 1024;
/// Most serialized content all synchronized drafts together may carry, so
/// drafts leave room for everything else in one snapshot.
pub const MAX_SYNCED_DRAFTS_TOTAL_BYTES: usize = 8 * 1024 * 1024;

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
    ContactGroup,
    ScheduledSendSummary,
    /// A muted sender. Added in 0.122.0; an earlier build cannot open a
    /// snapshot that holds one.
    SenderRule,
    Signature,
    SignatureDefaults,
    /// A compose draft. Added in 0.147.0; an earlier build cannot open a
    /// snapshot that holds one.
    Draft,
    /// Explicit delivery-pattern evidence, added in 0.173.0. Older builds
    /// cannot open a snapshot containing this entity type.
    SenderConfirmation,
}

/// Most web origins one remembered delivery pattern may contain.
pub const MAX_SENDER_CONFIRMATION_ORIGINS: usize = 500;

/// Most members one synchronized contact group may list.
pub const MAX_CONTACT_GROUP_MEMBERS: usize = 500;
/// Prefix of a contact group's per-member field: `member:<contact id>` is
/// `true` while the contact belongs to the group.
pub const CONTACT_GROUP_MEMBER_PREFIX: &str = "member:";

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
            Self::ContactGroup => "contact_group",
            Self::ScheduledSendSummary => "scheduled_send_summary",
            Self::SenderRule => "sender_rule",
            Self::Signature => "signature",
            Self::SignatureDefaults => "signature_defaults",
            Self::Draft => "draft",
            Self::SenderConfirmation => "sender_confirmation",
        }
    }

    pub fn validate_payload(self, payload: &Value) -> Result<(), String> {
        let object = payload
            .as_object()
            .ok_or_else(|| "A synchronized record must be a JSON object".to_string())?;
        let limit = if self == Self::Draft { MAX_SYNCED_DRAFT_BYTES + 64 * 1024 } else { MAX_PAYLOAD_BYTES };
        if serde_json::to_vec(payload).map_err(display)?.len() > limit {
            return Err("The synchronized record is too large".to_string());
        }
        match self {
            Self::ScheduledSendSummary => {
                if object.len() != 1 { return Err("A scheduled summary contains only a report".into()); }
                let report: ScheduledSendReport = serde_json::from_value(object.get("report").cloned().unwrap_or(Value::Null)).map_err(display)?;
                report.validate()?;
            }
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
            Self::Signature => {
                required_string(object, "id", 128)?;
                if object["id"].as_str().unwrap().len() > 128 { return Err("Signature id is too large".into()); }
                required_string(object, "name", 200)?;
                let (html, text) = signature_content(payload)?;
                if html.trim().is_empty() || html.len() > MAX_SIGNATURE_HTML_BYTES || text.len() > MAX_SIGNATURE_TEXT_BYTES { return Err("Signature content is too large or empty".into()); }
            }
            Self::SignatureDefaults => {
                if object.keys().any(|key| !["id", "account", "address", "newSignatureId", "replySignatureId"].contains(&key.as_str())) { return Err("Unknown signature defaults field".into()); }
                required_string(object, "id", 320)?;
                let id = object["id"].as_str().unwrap();
                if !id.strip_prefix("signature-default:").is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|ch| ch.is_ascii_hexdigit())) { return Err("Invalid signature defaults identity".into()); }
                required_string(object, "account", 320)?;
                let account = object["account"].as_str().unwrap();
                let address = object.get("address").and_then(Value::as_str).ok_or("Signature address must be a string")?;
                if !account.contains('@') || account.chars().any(char::is_control) || address.len() > 320 || (!address.is_empty() && (!address.contains('@') || address.chars().any(char::is_control))) { return Err("Invalid signature address".into()); }
                for field in ["newSignatureId", "replySignatureId"] {
                    // A cleared replica field projects as absence. Both it
                    // and an explicit JSON null mean No Signature.
                    if !matches!(object.get(field), None | Some(Value::Null)) { required_string(object, field, 128)?; }
                }
            }
            Self::Draft => {
                draft_content(payload)?;
            }
            Self::SenderConfirmation => {
                // One atomic field prevents concurrent confirmations from
                // combining authentication evidence with another pattern.
                if object.len() != 1 { return Err("Unknown sender confirmation field".into()); }
                let pattern = object.get("pattern").and_then(Value::as_object).ok_or("Missing sender confirmation")?;
                if pattern.keys().any(|key| !["id", "accountId", "requester", "senderEmail", "replyTo", "linkOrigins", "authenticated", "sourceMessageId", "confirmedAt"].contains(&key.as_str())) {
                    return Err("Unknown sender confirmation detail".into());
                }
                for field in ["id", "accountId", "requester", "senderEmail", "sourceMessageId", "confirmedAt"] {
                    required_string(pattern, field, 2048)?;
                }
                if !matches!(pattern.get("replyTo"), Some(Value::Null) | Some(Value::String(_))) || !pattern.get("authenticated").is_some_and(Value::is_boolean) {
                    return Err("Invalid sender confirmation evidence".into());
                }
                let origins = pattern.get("linkOrigins").and_then(Value::as_array).ok_or("Missing link origins")?;
                if origins.len() > MAX_SENDER_CONFIRMATION_ORIGINS || origins.iter().any(|origin| !origin.is_string()) {
                    return Err("Invalid link origins".into());
                }
            }
            Self::SenderRule => {
                required_string(object, "id", 128)?;
                required_string(object, "accountId", 320)?;
                enum_string(object, "matchKind", &["address", "domain"])?;
                required_string(object, "matchValue", 320)?;
                enum_string(object, "action", &["mute"])?;
                required_string(object, "createdAt", 64)?;
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
                optional_string(object, "nickname", 200)?;
                if object.get("nickname").and_then(Value::as_str)
                    .is_some_and(|name| name.trim().is_empty() || name.chars().any(char::is_control)) {
                    return Err("nickname is invalid".to_string());
                }
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
            Self::ContactGroup => {
                optional_string(object, "id", 128)?;
                required_string(object, "name", 100)?;
                optional_string(object, "createdAt", 64)?;
                let mut members = 0;
                for (field, value) in object.iter().filter(|(field, _)| field.starts_with(CONTACT_GROUP_MEMBER_PREFIX)) {
                    let contact_id = &field[CONTACT_GROUP_MEMBER_PREFIX.len()..];
                    if contact_id.is_empty() || contact_id.len() > 128 {
                        return Err("A contact group member id is invalid".to_string());
                    }
                    match value {
                        Value::Bool(true) => members += 1,
                        Value::Null => {}
                        _ => return Err("A contact group member is invalid".to_string()),
                    }
                }
                if members > MAX_CONTACT_GROUP_MEMBERS {
                    return Err("The contact group has too many members".to_string());
                }
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

/// The portable model uses html/text. Replication writes an atomic content
/// hash plus bounded, hash-addressed chunks, so HTML with an embedded logo
/// fits the envelope's per-value budget without mixing concurrent edits.
pub fn signature_content(payload: &Value) -> Result<(String, String), String> {
    let object = payload.as_object().ok_or("Invalid signature content")?;
    if object.contains_key("html") {
        if object.keys().any(|key| !["id", "name", "html", "text"].contains(&key.as_str())) { return Err("Unknown signature field".into()); }
        return Ok((object["html"].as_str().ok_or("Signature HTML must be a string")?.into(), object.get("text").and_then(Value::as_str).ok_or("Signature text must be a string")?.into()));
    }
    let hash = object.get("contentHash").and_then(Value::as_str).ok_or("Missing signature content hash")?;
    if hash.len() != 64 || !hash.bytes().all(|ch| ch.is_ascii_hexdigit()) { return Err("Invalid signature content hash".into()); }
    let mut html = std::collections::BTreeMap::new();
    let mut text = std::collections::BTreeMap::new();
    for (key, value) in object {
        if ["id", "name", "contentHash"].contains(&key.as_str()) { continue; }
        let mut parts = key.split(':');
        let kind = parts.next().unwrap_or_default();
        let version = parts.next().unwrap_or_default();
        let index = parts.next().unwrap_or_default();
        let number: usize = index.parse().map_err(|_| "Invalid signature chunk index")?;
        if version != hash || index != number.to_string() || parts.next().is_some() { return Err("Invalid signature chunk identity".into()); }
        let value = value.as_str().ok_or("Invalid signature chunk")?;
        match kind { "html" => { html.insert(number, value); }, "text" => { text.insert(number, value); }, _ => return Err("Unknown signature field".into()) }
    }
    let join = |chunks: std::collections::BTreeMap<usize, &str>| -> Result<String, String> {
        if chunks.is_empty() { return Err("Missing signature content chunks".into()); }
        let mut result = String::new();
        for (expected, (index, value)) in chunks.into_iter().enumerate() {
            if expected != index { return Err("Missing signature content chunk".into()); }
            result.push_str(value);
        }
        Ok(result)
    };
    Ok((join(html)?, join(text)?))
}

/// A synchronized draft is `id`, a `contentHash`, and hash-addressed
/// `content:<hash>:<index>` chunks of the draft's JSON. Every field lives in
/// the one content value, so concurrent edits conflict as whole drafts and
/// never mix one device's subject with another's body. Returns the joined
/// content after checking its identity, bounds and hash shape; callers check
/// the hash itself and parse the draft.
pub fn draft_content(payload: &Value) -> Result<String, String> {
    let object = payload.as_object().ok_or("Invalid draft")?;
    required_string(object, "id", 128)?;
    let hash = object.get("contentHash").and_then(Value::as_str).ok_or("Missing draft content hash")?;
    if hash.len() != 64 || !hash.bytes().all(|ch| ch.is_ascii_hexdigit()) { return Err("Invalid draft content hash".into()); }
    let mut chunks = std::collections::BTreeMap::new();
    for (key, value) in object {
        if ["id", "contentHash"].contains(&key.as_str()) { continue; }
        let mut parts = key.split(':');
        let (kind, version, index) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        let number: usize = index.parse().map_err(|_| "Invalid draft chunk index")?;
        if kind != "content" || version != hash || index != number.to_string() || parts.next().is_some() { return Err("Unknown draft field".into()); }
        chunks.insert(number, value.as_str().ok_or("Invalid draft chunk")?);
    }
    if chunks.is_empty() { return Err("Missing draft content".into()); }
    let mut content = String::new();
    for (expected, (index, chunk)) in chunks.into_iter().enumerate() {
        if expected != index { return Err("Missing draft content chunk".into()); }
        content.push_str(chunk);
    }
    if content.len() > MAX_SYNCED_DRAFT_BYTES { return Err("The draft is too large to sync".into()); }
    if !serde_json::from_str::<Value>(&content).map_err(display)?.is_object() { return Err("Invalid draft content".into()); }
    Ok(content)
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
            "contact_group" => Ok(Self::ContactGroup),
            "scheduled_send_summary" => Ok(Self::ScheduledSendSummary),
            "sender_rule" => Ok(Self::SenderRule),
            "sender_confirmation" => Ok(Self::SenderConfirmation),
            "signature" => Ok(Self::Signature),
            "signature_defaults" => Ok(Self::SignatureDefaults),
            "draft" => Ok(Self::Draft),
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
    fn signature_payloads_are_bounded_and_reject_unrelated_data() {
        let valid = json!({"id":"work", "name":"Work", "html":"<p>Regards</p>", "text":"Regards"});
        assert!(EntityType::Signature.validate_payload(&valid).is_ok());
        for length in [MAX_SIGNATURE_HTML_BYTES - 1, MAX_SIGNATURE_HTML_BYTES, MAX_SIGNATURE_HTML_BYTES + 1] {
            let mut value = valid.clone(); value["html"] = json!("x".repeat(length));
            assert_eq!(EntityType::Signature.validate_payload(&value).is_ok(), length <= MAX_SIGNATURE_HTML_BYTES);
        }
        for length in [MAX_SIGNATURE_TEXT_BYTES - 1, MAX_SIGNATURE_TEXT_BYTES, MAX_SIGNATURE_TEXT_BYTES + 1] {
            let mut value = valid.clone(); value["text"] = json!("x".repeat(length));
            assert_eq!(EntityType::Signature.validate_payload(&value).is_ok(), length <= MAX_SIGNATURE_TEXT_BYTES);
        }
        let mut unicode = valid.clone(); unicode["html"] = json!("é".repeat(MAX_SIGNATURE_HTML_BYTES / 2 + 1));
        assert!(EntityType::Signature.validate_payload(&unicode).is_err());
        let mut secret = valid; secret["credentials"] = json!("must stay local");
        assert!(EntityType::Signature.validate_payload(&secret).is_err());
        let defaults = json!({"id":format!("signature-default:{}", "a".repeat(64)), "account":"me@example.com", "address":"alias@example.com", "newSignatureId":"work", "replySignatureId":null});
        assert!(EntityType::SignatureDefaults.validate_payload(&defaults).is_ok());
        let mut cleared = defaults.clone(); cleared.as_object_mut().unwrap().remove("replySignatureId");
        assert!(EntityType::SignatureDefaults.validate_payload(&cleared).is_ok());
        for field in ["id", "account", "address", "newSignatureId"] {
            let mut invalid = defaults.clone(); invalid[field] = json!("bad");
            if field == "newSignatureId" { invalid[field] = json!(42); }
            assert!(EntityType::SignatureDefaults.validate_payload(&invalid).is_err(), "{field}");
        }
    }

    #[test]
    fn draft_payloads_are_hash_addressed_bounded_chunks() {
        let hash = "a".repeat(64);
        let draft = |content: &str| json!({"id": "draft-1", "contentHash": hash, format!("content:{hash}:0"): content});
        assert_eq!(draft_content(&draft("{\"subject\":\"Hi\"}")).unwrap(), "{\"subject\":\"Hi\"}");
        assert!(EntityType::Draft.validate_payload(&draft("{}")).is_ok());
        let mut split = json!({"id": "draft-1", "contentHash": hash});
        split[format!("content:{hash}:0")] = json!("{\"a\":");
        split[format!("content:{hash}:1")] = json!("1}");
        assert_eq!(draft_content(&split).unwrap(), "{\"a\":1}");
        let mut gap = split.clone(); gap.as_object_mut().unwrap().remove(&format!("content:{hash}:0"));
        assert!(draft_content(&gap).is_err());
        let mut other_version = split.clone(); other_version[format!("content:{}:0", "b".repeat(64))] = json!("x");
        assert!(draft_content(&other_version).is_err(), "only the named version's chunks are read");
        let mut extra = draft("{}"); extra["body"] = json!("must live in content");
        assert!(EntityType::Draft.validate_payload(&extra).is_err());
        assert!(EntityType::Draft.validate_payload(&draft("[]")).is_err());
        assert!(EntityType::Draft.validate_payload(&draft("not json")).is_err());
        let mut short = draft("{}"); short["contentHash"] = json!("abc");
        assert!(EntityType::Draft.validate_payload(&short).is_err());
        for length in [MAX_SYNCED_DRAFT_BYTES - 1, MAX_SYNCED_DRAFT_BYTES, MAX_SYNCED_DRAFT_BYTES + 1] {
            let body = "x".repeat(length - "{\"b\":\"\"}".len());
            let content = format!("{{\"b\":\"{body}\"}}");
            assert_eq!(content.len(), length);
            let mut value = json!({"id": "draft-1", "contentHash": hash});
            for (index, chunk) in content.as_bytes().chunks(60_000).enumerate() {
                value[format!("content:{hash}:{index}")] = json!(std::str::from_utf8(chunk).unwrap());
            }
            assert_eq!(EntityType::Draft.validate_payload(&value).is_ok(), length <= MAX_SYNCED_DRAFT_BYTES, "{length}");
        }
    }

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
            | EntityType::Goal
            | EntityType::ContactGroup
            | EntityType::ScheduledSendSummary
            | EntityType::SenderRule
            | EntityType::Signature
            | EntityType::SignatureDefaults
            | EntityType::Draft
            | EntityType::SenderConfirmation => entity_type,
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
            EntityType::ContactGroup,
            EntityType::ScheduledSendSummary,
            EntityType::SenderRule,
            EntityType::Signature,
            EntityType::SignatureDefaults,
            EntityType::Draft,
            EntityType::SenderConfirmation,
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
    fn mail_account_nicknames_are_optional_and_validated() {
        let mut account = json!({"email":"you@example.com","provider":"gmail","color":"#123456"});
        EntityType::MailAccount.validate_payload(&account).unwrap();
        for nickname in [Value::Null, json!("Work"), json!("é".repeat(200))] {
            account["nickname"] = nickname;
            EntityType::MailAccount.validate_payload(&account).unwrap();
        }
        for nickname in [json!("é".repeat(201)), json!("Work\nPersonal"), json!("  "), json!(42)] {
            account["nickname"] = nickname;
            assert!(EntityType::MailAccount.validate_payload(&account).is_err());
        }
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
    fn validates_contact_group_contract_and_bounded_membership() {
        let valid = json!({"id": "group-1", "name": "Board", "createdAt": "2026-10-07T00:00:00Z", "member:contact:a": true, "member:contact:b": null});
        assert!(EntityType::ContactGroup.validate_payload(&valid).is_ok());
        for (field, value) in [
            ("name", json!(" ")),
            ("name", json!("x".repeat(101))),
            ("member:contact:a", json!(false)),
            ("member:contact:a", json!("yes")),
            ("member:", json!(true)),
            ("id", json!(7)),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(EntityType::ContactGroup.validate_payload(&invalid).is_err(), "{field} should be rejected");
        }
        let mut at_limit = json!({"name": "Everyone"});
        for index in 0..MAX_CONTACT_GROUP_MEMBERS {
            at_limit[format!("member:contact:{index}")] = json!(true);
        }
        assert!(EntityType::ContactGroup.validate_payload(&at_limit).is_ok());
        // Removed members (null) don't count toward the limit.
        at_limit["member:contact:removed"] = json!(null);
        assert!(EntityType::ContactGroup.validate_payload(&at_limit).is_ok());
        at_limit["member:contact:one-more"] = json!(true);
        assert!(EntityType::ContactGroup.validate_payload(&at_limit).is_err());
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
    #[test]
    fn sender_confirmation_contract_preserves_atomic_evidence_and_bounds_origins() {
        let valid = serde_json::json!({"pattern": {
            "id":"sender-confirmation:abc", "accountId":"you@example.com", "requester":"ana@partner.example",
            "senderEmail":"requests@service.example", "replyTo":"ana@partner.example", "linkOrigins":[],
            "authenticated":false, "sourceMessageId":"message", "confirmedAt":"2026-10-26T12:00:00Z"
        }});
        for count in [MAX_SENDER_CONFIRMATION_ORIGINS - 1, MAX_SENDER_CONFIRMATION_ORIGINS, MAX_SENDER_CONFIRMATION_ORIGINS + 1] {
            let mut value = valid.clone();
            value["pattern"]["linkOrigins"] = serde_json::json!(vec!["https://service.example"; count]);
            assert_eq!(EntityType::SenderConfirmation.validate_payload(&value).is_ok(), count <= MAX_SENDER_CONFIRMATION_ORIGINS);
        }
        let mut extra = valid.clone(); extra["token"] = serde_json::json!("secret");
        assert!(EntityType::SenderConfirmation.validate_payload(&extra).is_err());
        for field in ["authenticated", "requester", "sourceMessageId", "linkOrigins"] {
            let mut missing = valid.clone(); missing["pattern"].as_object_mut().unwrap().remove(field);
            assert!(EntityType::SenderConfirmation.validate_payload(&missing).is_err(), "{field}");
        }
    }

    #[test]
    fn sender_rule_contract_allows_only_address_or_domain_mutes() {
        let valid = json!({
            "id": "rule-1",
            "accountId": "you@example.com",
            "matchKind": "address",
            "matchValue": "news@launch.example",
            "action": "mute",
            "createdAt": "2026-10-14T00:00:00Z",
        });
        assert!(EntityType::SenderRule.validate_payload(&valid).is_ok());
        for (field, value) in [
            ("matchKind", json!("pattern")),
            ("action", json!("delete")),
            ("matchValue", json!("")),
            ("accountId", json!(null)),
            ("createdAt", json!(7)),
        ] {
            let mut bad = valid.clone();
            bad[field] = value;
            assert!(EntityType::SenderRule.validate_payload(&bad).is_err(), "{field}");
        }
        // A build from before mute rules cannot decode the new type, so it
        // never merges a snapshot partially (see docs/cross-device-sync.md).
        #[derive(Deserialize, Debug)] #[serde(rename_all="snake_case")] #[allow(dead_code)] enum PreviousType {Task,Snippet,SplitInbox,MailAccount,CalendarAccount,CalendarSelection,Preferences,Retention,Contact,Goal,ContactGroup,ScheduledSendSummary}
        assert!(serde_json::from_str::<PreviousType>("\"sender_rule\"").is_err());
        assert!(serde_json::from_str::<PreviousType>("\"scheduled_send_summary\"").is_ok());
    }

    #[test]
    fn scheduled_summary_allowlist_rejects_content_credentials_and_invalid_reports() {
        let report=ScheduledSendReport {operation_id:"00000000-0000-4000-8000-000000000001".into(),owner_installation_id:"00000000-0000-4000-8000-000000000002".into(),owner_sync_device_id:None,owner_name_at_creation:"MacBook".into(),account:"me@example.com".into(),subject:"Hello".into(),scheduled_at:1_800_000_000_000,time_zone:"America/New_York".into(),state:"scheduled".into(),blocked_reason:None,report_revision:1,status_changed_at:1_700_000_000_000};
        let valid=json!({"report":report});assert!(EntityType::ScheduledSendSummary.validate_payload(&valid).is_ok());
        for field in ["body","bodyHtml","to","bcc","raw","attachments","token","error"] {
            let mut bad=valid.clone();bad["report"][field]=json!("private");assert!(EntityType::ScheduledSendSummary.validate_payload(&bad).is_err());
            let mut bad=valid.clone();bad[field]=json!("private");assert!(EntityType::ScheduledSendSummary.validate_payload(&bad).is_err());
        }
        for (field,value) in [("scheduledAt",json!(-1)),("state",json!("resend")),("reportRevision",json!(0)),("timeZone",json!("unknown")),("ownerInstallationId",json!("")),("blockedReason",json!("server error with secret"))] {
            let mut bad=valid.clone();bad["report"][field]=value;assert!(EntityType::ScheduledSendSummary.validate_payload(&bad).is_err(),"{field}");
        }
        // An old vocabulary rejects the new variant; snapshots cannot partially decode.
        #[derive(Deserialize)] #[serde(rename_all="snake_case")] enum PreviousType {Task,Snippet,SplitInbox,MailAccount,CalendarAccount,CalendarSelection,Preferences,Retention,Contact,Goal,ContactGroup}
        assert!(serde_json::from_str::<PreviousType>("\"scheduled_send_summary\"").is_err());
        assert!(serde_json::from_str::<EntityType>("\"task\"").is_ok());
    }

}

/// Read-only metadata. No recipient, MIME, attachment, credential or raw error fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledSendReport {
    pub operation_id: String,
    pub owner_installation_id: String,
    pub owner_sync_device_id: Option<String>,
    pub owner_name_at_creation: String,
    pub account: String,
    pub subject: String,
    pub scheduled_at: i64,
    pub time_zone: String,
    pub state: String,
    pub blocked_reason: Option<String>,
    pub report_revision: i64,
    pub status_changed_at: i64,
}

impl ScheduledSendReport {
    pub fn entity_id(&self) -> String { format!("{}:{}", self.owner_installation_id, self.operation_id) }
    pub fn validate(&self) -> Result<(), String> {
        let uuid = |v: &str| v.len() == 36 && v.bytes().enumerate().all(|(i,b)| if [8,13,18,23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() });
        if !uuid(&self.operation_id) || !uuid(&self.owner_installation_id)
            || self.owner_sync_device_id.as_ref().is_some_and(|v| v.len() != 32 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
            || self.owner_name_at_creation.trim().is_empty() || self.owner_name_at_creation.chars().count() > 60
            || self.account.is_empty() || self.account.len() > 320 || !self.account.contains('@') || self.account.contains(['\r','\n'])
            || self.subject.chars().count() > 998 || self.time_zone.parse::<chrono_tz::Tz>().is_err()
            || !(0..=253402300799999).contains(&self.scheduled_at) || !(0..=253402300799999).contains(&self.status_changed_at)
            || self.report_revision < 1
            || !["scheduled","overdue","undo_pending","ready","sending","sent","failed","uncertain","unverifiable","canceled"].contains(&self.state.as_str())
            || self.blocked_reason.as_ref().is_some_and(|v| !["authorization","owner_unavailable","account_disconnected","provider_unavailable"].contains(&v.as_str())) {
            return Err("Invalid scheduled-send report".into());
        }
        Ok(())
    }
}
