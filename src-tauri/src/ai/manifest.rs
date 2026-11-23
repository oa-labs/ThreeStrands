//! One request-context contract for every provider feature. Text stays transient;
//! only source references and fingerprints may enter the request journal.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const CONTEXT_VERSION: u32 = 1;
pub const MAX_MANIFEST_SOURCES: usize = 16384;
pub const MAX_CONTEXT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Summary,
    Brief,
    Suggestions,
    Chat,
    Reply,
    DraftReview,
    Contact,
    FindTimes,
    Connection,
}
impl Feature {
    pub fn id(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Brief => "brief",
            Self::Suggestions => "suggestions",
            Self::Chat => "chat",
            Self::Reply => "reply",
            Self::DraftReview => "draft_review",
            Self::Contact => "contact",
            Self::FindTimes => "find_times",
            Self::Connection => "connection",
        }
    }
    /// Bump when instructions/interpretation change; hashes also detect drift.
    pub fn prompt_version(self) -> u32 {
        match self {
            Self::Summary => 2,
            Self::Brief => 3,
            Self::Suggestions => 1,
            Self::Chat => 2,
            Self::Reply => 2,
            Self::DraftReview => 2,
            Self::Contact => 2,
            Self::FindTimes => 1,
            Self::Connection => 1,
        }
    }
    pub fn schema_version(self) -> u32 {
        match self {
            Self::Summary | Self::Reply | Self::Connection => 0,
            Self::Brief | Self::Chat | Self::DraftReview | Self::Contact => 2,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceScope {
    pub account_ids: Vec<String>,
    pub record_id: Option<String>,
    pub revision: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceReference {
    /// JSON pointer into the exact outbound payload, never a model-generated ID.
    pub path: String,
    pub source_id: Option<String>,
    pub revision: String,
    pub characters: usize,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextManifest {
    pub version: u32,
    pub payload_revision: String,
    pub scope: SourceScope,
    pub sources: Vec<SourceReference>,
}
/// The preview, evidence checks, and provider all read this exact payload.
/// It has no Serialize implementation: persisting it would persist mail text.
pub struct PreparedContext {
    pub payload: String,
    pub manifest: ContextManifest,
}
pub fn fingerprint(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
impl PreparedContext {
    pub fn new(payload: &str, scope: SourceScope) -> Result<Self, String> {
        if payload.len() > MAX_CONTEXT_BYTES {
            return Err("This AI context is too large to send".into());
        }
        let mut sources = Vec::new();
        fn visit(
            value: &Value,
            path: &str,
            inherited: Option<&str>,
            truncated: bool,
            sources: &mut Vec<SourceReference>,
        ) -> Result<(), String> {
            if path.len() > 1024 {
                return Err("An AI source path is too long".into());
            }
            match value {
                Value::Object(object) => {
                    let id = ["sourceMessageId", "fromMessageId", "threadId", "id"]
                        .iter()
                        .find_map(|key| object.get(*key).and_then(Value::as_str))
                        .or(inherited);
                    if id.is_some_and(|id| id.len() > 512) {
                        return Err("An AI source reference is too long".into());
                    }
                    let truncated = object
                        .get("truncated")
                        .and_then(Value::as_bool)
                        .unwrap_or(truncated);
                    for (key, value) in object {
                        let key = key.replace('~', "~0").replace('/', "~1");
                        visit(value, &format!("{path}/{key}"), id, truncated, sources)?;
                    }
                }
                Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        visit(
                            value,
                            &format!("{path}/{index}"),
                            inherited,
                            truncated,
                            sources,
                        )?;
                    }
                }
                Value::String(text) => {
                    if sources.len() == MAX_MANIFEST_SOURCES {
                        return Err("This AI context has too many sources".into());
                    }
                    sources.push(SourceReference {
                        path: path.into(),
                        source_id: inherited.map(str::to_string),
                        revision: fingerprint(text.as_bytes()),
                        characters: text.chars().count(),
                        truncated,
                    });
                }
                _ => {}
            }
            Ok(())
        }
        // Legacy plain-text summaries remain a single explicit thread excerpt;
        // JSON-based features expose individual message/attachment/profile fields.
        let value = serde_json::from_str(payload).unwrap_or_else(|_| Value::String(payload.into()));
        visit(&value, "", None, false, &mut sources)?;
        Ok(Self {
            payload: payload.into(),
            manifest: ContextManifest {
                version: CONTEXT_VERSION,
                payload_revision: fingerprint(payload.as_bytes()),
                scope,
                sources,
            },
        })
    }
    /// Resolve only against material actually included, including truncated text.
    pub fn exact_evidence(&self, source_id: &str, excerpt: &str) -> bool {
        if excerpt.trim().is_empty() {
            return false;
        }
        let Ok(value) = serde_json::from_str::<Value>(&self.payload) else {
            return false;
        };
        self.manifest.sources.iter().any(|source| {
            (source.path.ends_with("/bodyText")
                || source.path.ends_with("/body_text")
                || source.path.ends_with("/text"))
                && source.source_id.as_deref() == Some(source_id)
                && value
                    .pointer(&source.path)
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains(excerpt))
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn previews_and_evidence_use_the_exact_selected_payload_and_journal_has_no_text() {
        let payload = r#"{"messages":[{"sourceMessageId":"m1","bodyText":"Selected excerpt","truncated":true}],"writing_profile":{"aboutYou":"Private profile"}}"#;
        let prepared = PreparedContext::new(payload, SourceScope::default()).unwrap();
        assert_eq!(prepared.payload, payload);
        assert!(prepared.exact_evidence("m1", "Selected excerpt"));
        assert!(!prepared.exact_evidence("m1", "Omitted tail"));
        assert!(!prepared.exact_evidence("m1", "m1"));
        assert!(!prepared.exact_evidence("other", "Selected excerpt"));
        let serialized = serde_json::to_string(&prepared.manifest).unwrap();
        assert!(!serialized.contains("Selected excerpt"));
        assert!(!serialized.contains("Private profile"));
        assert!(prepared
            .manifest
            .sources
            .iter()
            .any(|source| source.truncated));
        let changed = PreparedContext::new(
            &payload.replace("Selected excerpt", "Changed excerpt"),
            SourceScope::default(),
        )
        .unwrap();
        assert_ne!(
            prepared.manifest.payload_revision,
            changed.manifest.payload_revision
        );
    }
    #[test]
    fn context_limits_reject_instead_of_silently_omitting_sources() {
        for size in [
            MAX_CONTEXT_BYTES - 1,
            MAX_CONTEXT_BYTES,
            MAX_CONTEXT_BYTES + 1,
        ] {
            assert_eq!(
                PreparedContext::new(&"a".repeat(size), SourceScope::default()).is_ok(),
                size <= MAX_CONTEXT_BYTES
            );
        }
        for count in [
            MAX_MANIFEST_SOURCES - 1,
            MAX_MANIFEST_SOURCES,
            MAX_MANIFEST_SOURCES + 1,
        ] {
            let payload = serde_json::to_string(&vec!["x"; count]).unwrap();
            assert_eq!(
                PreparedContext::new(&payload, SourceScope::default()).is_ok(),
                count <= MAX_MANIFEST_SOURCES
            );
        }
    }
}
