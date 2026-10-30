//! Device-local, bounded AI validation diagnostics. Capture consent is memory-only.
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

const MAX_ENTRIES: usize = 100;
const RETENTION_DAYS: i64 = 7;
const CAPTURE_MINUTES: i64 = 15;
const MAX_FIELD_CHARS: usize = 160;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    at: DateTime<Utc>,
    request_id: String,
    feature: String,
    provider: String,
    model: String,
    reason: String,
    details: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    entries: Vec<Entry>,
    capture_until: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct Store {
    path: Option<PathBuf>,
    entries: Vec<Entry>,
    capture_until: Option<DateTime<Utc>>,
}

impl Store {
    fn prune(&mut self, now: DateTime<Utc>) {
        self.entries
            .retain(|entry| entry.at >= now - Duration::days(RETENTION_DAYS));
        if self.entries.len() > MAX_ENTRIES {
            self.entries.drain(..self.entries.len() - MAX_ENTRIES);
        }
        if self.capture_until.is_some_and(|until| until <= now) {
            self.capture_until = None;
        }
    }

    fn persist(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let data = serde_json::to_vec(&self.entries).map_err(|e| e.to_string())?;
        // Use a private temporary file and replace, so interruption cannot leave
        // a partial document and scheduling values are owner-readable only.
        let temporary = path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        options
            .open(&temporary)
            .and_then(|mut f| f.write_all(&data))
            .map_err(|e| e.to_string())?;
        std::fs::rename(temporary, path).map_err(|e| e.to_string())
    }

    fn snapshot(&mut self, now: DateTime<Utc>) -> Snapshot {
        self.prune(now);
        Snapshot {
            entries: self.entries.iter().rev().cloned().collect(),
            capture_until: self.capture_until,
        }
    }
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

tokio::task_local! {
    static REQUEST: Entry;
}

pub(crate) fn initialize(path: PathBuf) {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    // Only read the bounded file we write; never load an arbitrarily large file.
    store.entries = std::fs::metadata(&path)
        .ok()
        .filter(|m| m.len() <= 1_000_000)
        .and_then(|_| std::fs::read(&path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    store.path = Some(path);
    store.capture_until = None;
    store.prune(Utc::now());
    if let Err(error) = store.persist() {
        log::warn!(target: "ai_diagnostics", "could not persist diagnostics: {error}");
    }
}

pub(crate) fn snapshot() -> Snapshot {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let previous_count = store.entries.len();
    let result = store.snapshot(Utc::now());
    if previous_count != store.entries.len() {
        if let Err(error) = store.persist() {
            log::warn!(target: "ai_diagnostics", "could not persist diagnostics: {error}");
        }
    }
    result
}

pub(crate) fn set_capture(enabled: bool) -> Snapshot {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.capture_until = enabled.then(|| Utc::now() + Duration::minutes(CAPTURE_MINUTES));
    store.snapshot(Utc::now())
}

pub(crate) fn clear() -> Result<Snapshot, String> {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.entries.clear();
    store.persist()?;
    Ok(store.snapshot(Utc::now()))
}

pub(crate) async fn with_request<T>(
    feature: &str,
    thread_id: &str,
    provider: super::AiProvider,
    model: &str,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let context = Entry {
        at: Utc::now(),
        request_id: uuid::Uuid::new_v4().to_string(),
        feature: feature.into(),
        provider: provider.id().into(),
        model: model.chars().take(160).collect(),
        reason: String::new(),
        details: None,
    };
    log::info!(target: "ai_diagnostics", "request {} starting: {} thread={} provider={} model={}", context.request_id, feature, thread_id, context.provider, context.model);
    REQUEST
        .scope(context, async {
            let result = future.await;
            if result.is_err() {
                rejected(
                    "AI request failed; see application log for the underlying error",
                    None,
                );
            }
            result
        })
        .await
}

// These are the only provider values detailed capture retains: never titles,
// participants, notes, evidence, endpoints, keys, or full provider responses.
fn scheduling_details(item: &Value) -> Value {
    let mut fields = serde_json::Map::new();
    for key in [
        "normalizedStart",
        "normalizedEnd",
        "eventDate",
        "searchRangeStart",
        "searchRangeEnd",
        "durationMinutes",
        "timeZone",
        "dueKind",
        "dueValue",
        "repeatIntervalDays",
    ] {
        if let Some(value) = item.get(key) {
            if value.is_null() || value.is_number() || value.is_boolean() {
                fields.insert(key.into(), value.clone());
            } else if let Some(text) = value.as_str() {
                fields.insert(
                    key.into(),
                    Value::String(text.chars().take(MAX_FIELD_CHARS).collect()),
                );
            }
        }
    }
    Value::Object(fields)
}

pub(super) fn rejected(reason: &str, item: Option<&Value>) {
    let Ok(mut entry) = REQUEST.try_with(Clone::clone) else {
        return;
    };
    let now = Utc::now();
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.prune(now);
    entry.at = now;
    entry.reason = reason.into();
    if store.capture_until.is_some() {
        entry.details = item.map(scheduling_details);
    }
    log::warn!(target: "ai_diagnostics", "request {}: {}", entry.request_id, reason);
    store.entries.push(entry);
    store.prune(now);
    if let Err(error) = store.persist() {
        log::warn!(target: "ai_diagnostics", "could not persist diagnostics: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_only_keeps_bounded_scheduling_fields() {
        let details = scheduling_details(&serde_json::json!({
            "title":"Private subject", "evidence":{"excerpt":"Private mail"},
            "participants":["private@example.com"], "normalizedStart":"bad timestamp",
            "dueValue":"x".repeat(MAX_FIELD_CHARS + 1), "timeZone":{"secret":"no"}, "durationMinutes":30
        }));
        assert_eq!(details["normalizedStart"], "bad timestamp");
        assert_eq!(details["durationMinutes"], 30);
        assert_eq!(details["dueValue"].as_str().unwrap().len(), MAX_FIELD_CHARS);
        assert!(details.get("title").is_none());
        assert!(details.get("evidence").is_none());
        assert!(details.get("participants").is_none());
        assert!(details.get("timeZone").is_none());
    }
    #[test]
    fn retention_count_and_capture_expiry_are_bounded() {
        let now = Utc::now();
        let entry = Entry {
            at: now,
            request_id: "id".into(),
            feature: "brief".into(),
            provider: "test".into(),
            model: "test".into(),
            reason: "reason".into(),
            details: None,
        };
        let mut store = Store {
            entries: vec![entry.clone(); MAX_ENTRIES + 1],
            capture_until: Some(now),
            ..Store::default()
        };
        store.prune(now);
        assert_eq!(store.entries.len(), MAX_ENTRIES);
        assert!(store.capture_until.is_none());
        store.entries[0].at = now - Duration::days(RETENTION_DAYS) - Duration::seconds(1);
        store.entries[1].at = now - Duration::days(RETENTION_DAYS);
        assert_eq!(store.snapshot(now).entries.len(), MAX_ENTRIES - 1);
    }
    #[tokio::test]
    async fn validation_records_reasons_and_isolates_requests_with_capture_opt_in() {
        use super::super::proposals::validate_proposal_items;
        let directory =
            std::env::temp_dir().join(format!("ai-diagnostics-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("diagnostics.json");
        initialize(path.clone());
        let task = serde_json::json!({
            "type":"task", "kind":"action", "title":"Private title", "notes":"Private notes",
            "dueKind":"date", "dueValue":"not a date", "confidence":0.8,
            "evidence":{"sourceMessageId":"private-id", "excerpt":"Private evidence"}
        });
        with_request(
            "suggestions",
            "thread-one",
            super::super::AiProvider::OpenAi,
            "model-one",
            async {
                let analysis = validate_proposal_items(vec![task.clone()], &[])?;
                assert_eq!(analysis.hidden_count, 1);
                Ok(())
            },
        )
        .await
        .unwrap();
        let ordinary = snapshot();
        assert_eq!(ordinary.entries.len(), 1);
        assert!(ordinary.entries[0].details.is_none());
        assert_eq!(
            ordinary.entries[0].reason,
            "The AI provider returned an invalid task due value"
        );
        let first_id = ordinary.entries[0].request_id.clone();
        set_capture(true);
        let meeting = serde_json::json!({
            "type":"meeting", "intent":"schedule", "title":"Private title", "participants":[],
            "rawTimeLanguage":"Private time language", "normalizedStart":"bad timestamp", "confidence":0.8,
            "evidence":{"sourceMessageId":"private-id", "excerpt":"Private evidence"}
        });
        let (a, b) = tokio::join!(
            with_request(
                "brief",
                "thread-two",
                super::super::AiProvider::OpenAi,
                "model-two",
                async {
                    tokio::task::yield_now().await;
                    validate_proposal_items(vec![meeting], &[])?;
                    Ok(())
                }
            ),
            with_request(
                "chat",
                "thread-three",
                super::super::AiProvider::OpenAi,
                "model-three",
                async {
                    validate_proposal_items(
                        vec![task, serde_json::json!({"private":"secret"})],
                        &[],
                    )?;
                    tokio::task::yield_now().await;
                    Err::<(), _>("Private provider error with a credential".to_string())
                }
            ),
        );
        a.unwrap();
        assert!(b.is_err());
        let captured = snapshot();
        assert_eq!(captured.entries.len(), 5);
        assert!(captured
            .entries
            .iter()
            .filter(|e| e.feature == "brief")
            .all(|e| e.model == "model-two" && e.request_id != first_id));
        let chats: Vec<_> = captured
            .entries
            .iter()
            .filter(|e| e.feature == "chat")
            .collect();
        assert_eq!(chats.len(), 3);
        assert!(chats
            .iter()
            .all(|e| e.request_id == chats[0].request_id && e.model == "model-three"));
        assert!(chats.iter().any(|e| e.reason.contains("invalid schema")));
        assert!(chats.iter().any(|e| e
            .details
            .as_ref()
            .is_some_and(|d| d["dueValue"] == "not a date")));
        let serialized = std::fs::read_to_string(&path).unwrap();
        for private in [
            "Private",
            "private-id",
            "secret",
            "credential",
            "thread-one",
        ] {
            assert!(!serialized.contains(private), "leaked {private}");
        }
        // Restart retains diagnostics but never restores capture consent.
        initialize(path.clone());
        assert!(snapshot().capture_until.is_none());
        assert_eq!(snapshot().entries.len(), 5);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        clear().unwrap();
        let saved: Vec<Entry> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.is_empty());
        *STORE.lock().unwrap() = Store::default();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
