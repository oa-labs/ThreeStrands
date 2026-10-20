//! Compose drafts in replicated sync. A draft syncs as one hash-addressed
//! content value (see `threestrands_sync_protocol::draft_content`), so two
//! devices editing one draft at once conflict as whole drafts and the newest
//! edit wins. Attachment bytes never sync: another device sees each
//! attachment's metadata and must attach the file again, or download a
//! forwarded one, before sending.
use std::collections::BTreeSet;

use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use threestrands_sync_protocol::{
    draft_content, EntityType, MAX_SYNCED_DRAFTS_TOTAL_BYTES, MAX_SYNCED_DRAFT_BYTES,
};
use uuid::Uuid;

use crate::{
    correspondence::Draft,
    db::{serialization_error, Database, DatabaseError, DbResult},
    signatures::{sha256_hex, value_sized_chunks},
};

/// Draft fields that describe this device's copy and never leave it.
const DEVICE_FIELDS: [&str; 3] = ["revision", "tooLargeToSync", "serverDraft"];
const CONTENT_PREFIX: &str = "content:";

fn draft_sync_content(draft: &Draft) -> Result<String, String> {
    let mut value = serde_json::to_value(draft).map_err(|e| e.to_string())?;
    let object = value.as_object_mut().ok_or("Invalid draft")?;
    for field in DEVICE_FIELDS {
        object.remove(field);
    }
    serde_json::to_string(&value).map_err(|e| e.to_string())
}

/// The replica record for `draft`, or `None` when the draft is too large to
/// sync on its own.
pub(crate) fn draft_sync_payload(draft: &Draft) -> Result<Option<Value>, String> {
    let content = draft_sync_content(draft)?;
    if content.len() > MAX_SYNCED_DRAFT_BYTES {
        return Ok(None);
    }
    let hash = sha256_hex(content.as_bytes());
    let mut value = json!({ "id": draft.id, "contentHash": hash });
    for (index, chunk) in value_sized_chunks(&content).into_iter().enumerate() {
        value[format!("{CONTENT_PREFIX}{hash}:{index}")] = json!(chunk);
    }
    EntityType::Draft.validate_payload(&value)?;
    Ok(Some(value))
}

/// Keeps only the chunks of the version `contentHash` names. A conflict's
/// losing chunks stay in the replica until this device next edits the draft.
pub(crate) fn selected_draft_payload(value: &Value) -> Result<Value, String> {
    let hash = value
        .get("contentHash")
        .and_then(Value::as_str)
        .ok_or("Missing draft content hash")?;
    let prefix = format!("{CONTENT_PREFIX}{hash}:");
    let mut selected = value.as_object().ok_or("Invalid draft record")?.clone();
    selected.retain(|key, _| !key.starts_with(CONTENT_PREFIX) || key.starts_with(&prefix));
    Ok(Value::Object(selected))
}

/// Reads a synced draft. The revision is this device's to assign.
pub(crate) fn draft_from_sync(value: &Value) -> Result<Draft, String> {
    let content = draft_content(value)?;
    if value["contentHash"].as_str() != Some(sha256_hex(content.as_bytes()).as_str()) {
        return Err("Draft content checksum mismatch".into());
    }
    let mut object: serde_json::Map<String, Value> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;
    if DEVICE_FIELDS.iter().any(|field| object.contains_key(*field)) {
        return Err("A synced draft carries device-only fields".into());
    }
    object.insert("revision".into(), json!(0));
    let draft: Draft = serde_json::from_value(Value::Object(object)).map_err(|e| e.to_string())?;
    if value["id"].as_str() != Some(draft.id.as_str()) {
        return Err("Draft identity mismatch".into());
    }
    // Attachment ids name files under the attachment root.
    if draft.attachments.iter().any(|attachment| Uuid::parse_str(&attachment.id).is_err()) {
        return Err("A synced draft has an invalid attachment".into());
    }
    Ok(draft)
}

fn draft_recorded(tx: &Transaction, id: &str) -> DbResult<bool> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM sync_values WHERE entity_type=?1 AND entity_id=?2 LIMIT 1",
            params![EntityType::Draft.as_str(), id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Records `draft` in the replica within the caller's transaction. Returns
/// `false` when the draft is too large to sync, alone or alongside the other
/// synced drafts. Its copy on other devices is then removed, so none of them
/// can send a stale version.
pub(crate) fn record_draft_write(tx: &Transaction, draft: &Draft) -> DbResult<bool> {
    let payload = draft_sync_payload(draft).map_err(DatabaseError::invalid)?;
    let others: i64 = tx.query_row(
        "SELECT COALESCE(SUM(LENGTH(value)),0) FROM sync_values WHERE entity_type=?1 AND entity_id<>?2 AND field LIKE 'content:%'",
        params![EntityType::Draft.as_str(), draft.id],
        |row| row.get(0),
    )?;
    let size = |payload: &Value| -> usize {
        payload
            .as_object()
            .map(|object| object.iter().filter(|(key, _)| key.starts_with(CONTENT_PREFIX)).map(|(_, value)| value.to_string().len()).sum())
            .unwrap_or(0)
    };
    let Some(payload) = payload.filter(|payload| others as usize + size(payload) <= MAX_SYNCED_DRAFTS_TOTAL_BYTES) else {
        if draft_recorded(tx, &draft.id)? {
            crate::sync_state::record_replicated_deletion_in_transaction(tx, EntityType::Draft, &draft.id)?;
        }
        return Ok(false);
    };
    let fields: BTreeSet<String> = payload
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    crate::sync_state::record_replicated_write_in_transaction(tx, EntityType::Draft, &draft.id, &fields, &payload)?;
    // The replica context keeps the replaced chunks' dots, so an older
    // snapshot cannot bring them back.
    tx.execute(
        "DELETE FROM sync_values WHERE entity_type=?1 AND entity_id=?2 AND field LIKE 'content:%' AND field NOT LIKE ?3",
        params![EntityType::Draft.as_str(), draft.id, format!("{CONTENT_PREFIX}{}:%", payload["contentHash"].as_str().unwrap_or_default())],
    )?;
    Ok(true)
}

/// Records a draft's removal (sent, scheduled or discarded) within the
/// caller's transaction.
pub(crate) fn record_draft_deletion(tx: &Transaction, id: &str) -> DbResult<()> {
    if draft_recorded(tx, id)? {
        crate::sync_state::record_replicated_deletion_in_transaction(tx, EntityType::Draft, id)?;
    }
    Ok(())
}

impl Database {
    /// Whether a draft write on this thread is recorded for other devices.
    pub(crate) fn draft_sync_enabled(&self) -> DbResult<bool> {
        Ok(self.replicated_sync_active()? && !self.is_projecting_remote_operation())
    }

    /// Applies a draft another device wrote. The local revision moves only
    /// when the content changed, so an open composer notices a real edit and
    /// nothing else. An attachment is ready only when this device already has
    /// its file.
    pub(crate) fn materialize_synced_draft(&self, entity_id: &str, payload: &Value) -> DbResult<()> {
        let mut incoming = draft_from_sync(payload).map_err(DatabaseError::invalid)?;
        if incoming.id != entity_id {
            return Err(DatabaseError::invalid("Draft identity mismatch"));
        }
        self.with_transaction(|tx| {
            let local: Option<String> = tx
                .query_row("SELECT payload FROM drafts WHERE id=?1", [entity_id], |row| row.get(0))
                .optional()?;
            let local: Option<Draft> = local
                .map(|payload| serde_json::from_str(&payload))
                .transpose()
                .map_err(serialization_error)?;
            for attachment in &mut incoming.attachments {
                attachment.ready = local
                    .as_ref()
                    .and_then(|local| local.attachments.iter().find(|candidate| candidate.id == attachment.id))
                    .is_some_and(|candidate| candidate.ready);
            }
            match local {
                Some(local) => {
                    incoming.revision = local.revision;
                    let same = serde_json::to_value(&local).map_err(serialization_error)?
                        == serde_json::to_value(&incoming).map_err(serialization_error)?;
                    if !same {
                        incoming.revision += 1;
                        tx.execute(
                            "UPDATE drafts SET revision=?1,payload=?2 WHERE id=?3",
                            params![incoming.revision, serde_json::to_string(&incoming).map_err(serialization_error)?, entity_id],
                        )?;
                    }
                }
                None => {
                    tx.execute(
                        "INSERT INTO drafts VALUES (?1,?2,?3)",
                        params![entity_id, incoming.revision, serde_json::to_string(&incoming).map_err(serialization_error)?],
                    )?;
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correspondence::Attachment;

    fn database() -> Database {
        let db = Database::open_memory();
        db.set_compose_identity("you@example.com").unwrap();
        db.adopt_account("you@example.com").unwrap();
        db.set_beta_features_enabled(true).unwrap();
        db
    }
    fn saved(db: &Database, body: &str) -> Draft {
        let mut d = db.create_draft("new", None, "you@example.com", &[]).unwrap();
        d.to = "Jane <jane@example.com>".into();
        d.subject = "Hello".into();
        d.body = body.into();
        db.save_draft(d).unwrap()
    }
    fn recorded_fields(db: &Database, id: &str) -> Vec<String> {
        let connection = db.connection().unwrap();
        let mut query = connection
            .prepare("SELECT field FROM sync_values WHERE entity_type='draft' AND entity_id=?1 ORDER BY field")
            .unwrap();
        query.query_map([id], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
    }
    fn attachment(ready: bool) -> Attachment {
        Attachment {
            id: Uuid::new_v4().to_string(),
            name: "plan.pdf".into(),
            size: 3,
            mime: "application/pdf".into(),
            ready,
            message_id: None,
            provider_id: None,
            inline: false,
            content_id: None,
        }
    }

    #[test]
    fn a_draft_round_trips_through_bounded_chunks_without_device_fields() {
        let db = database();
        let draft = saved(&db, &"é\"\n".repeat(30_000));
        let payload = draft_sync_payload(&draft).unwrap().unwrap();
        let chunks = payload.as_object().unwrap().keys().filter(|key| key.starts_with(CONTENT_PREFIX)).count();
        assert!(chunks > 1, "a body over one value's budget is split");
        for value in payload.as_object().unwrap().values() {
            assert!(value.to_string().len() <= threestrands_sync_envelope::limits::MAX_VALUE_BYTES);
        }
        let content = draft_content(&payload).unwrap();
        assert!(!content.contains("\"revision\"") && !content.contains("tooLargeToSync"));
        let back = draft_from_sync(&payload).unwrap();
        assert_eq!((back.revision, back.body.as_str(), back.subject.as_str()), (0, draft.body.as_str(), "Hello"));

        let hash = payload["contentHash"].as_str().unwrap();
        let other = draft_sync_payload(&Draft { subject: "Hullo".into(), ..draft.clone() }).unwrap().unwrap();
        let other_hash = other["contentHash"].as_str().unwrap();
        let mut tampered = json!({ "id": draft.id, "contentHash": hash });
        for (key, value) in other.as_object().unwrap().iter().filter(|(key, _)| key.starts_with(CONTENT_PREFIX)) {
            tampered[key.replace(other_hash, hash)] = value.clone();
        }
        assert!(draft_content(&tampered).is_ok());
        assert!(draft_from_sync(&tampered).is_err(), "content must match its hash");
        let mut unsafe_attachment = draft.clone();
        unsafe_attachment.attachments.push(Attachment { id: "../../secrets".into(), ..attachment(true) });
        assert!(draft_from_sync(&draft_sync_payload(&unsafe_attachment).unwrap().unwrap()).is_err());
    }

    #[test]
    fn saves_record_the_draft_only_while_sync_is_on() {
        let off = Database::open_memory();
        off.set_compose_identity("you@example.com").unwrap();
        off.adopt_account("you@example.com").unwrap();
        let draft = saved(&off, "Local only");
        assert!(recorded_fields(&off, &draft.id).is_empty());
        assert!(!draft.too_large_to_sync, "the flag is about sync, which is off");

        let db = database();
        let draft = saved(&db, "First");
        let first = recorded_fields(&db, &draft.id);
        assert!(first.contains(&"contentHash".to_string()) && first.iter().any(|field| field.starts_with(CONTENT_PREFIX)));
        let mut edited = draft.clone();
        edited.body = "Second".into();
        let edited = db.save_draft(edited).unwrap();
        let second = recorded_fields(&db, &draft.id);
        assert_eq!(second.iter().filter(|field| field.starts_with(CONTENT_PREFIX)).count(), 1, "replaced chunks are removed");
        assert_ne!(first, second);
        let stored: String = db.connection().unwrap().query_row("SELECT payload FROM drafts WHERE id=?1", [&edited.id], |row| row.get(0)).unwrap();
        assert!(!stored.contains("tooLargeToSync"));
        db.discard_draft(&edited.id).unwrap();
        assert!(recorded_fields(&db, &edited.id).is_empty());
    }

    #[test]
    fn a_draft_over_the_size_limit_stays_on_this_device_and_leaves_other_devices() {
        let db = database();
        let draft = saved(&db, "Small");
        assert!(!draft.too_large_to_sync);
        let overhead = draft_sync_content(&Draft { body: String::new(), ..draft.clone() }).unwrap().len();
        for (length, fits) in [(MAX_SYNCED_DRAFT_BYTES - overhead - 1, true), (MAX_SYNCED_DRAFT_BYTES - overhead, true), (MAX_SYNCED_DRAFT_BYTES - overhead + 1, false)] {
            let mut next = db.draft(&draft.id).unwrap();
            next.body = "x".repeat(length);
            let next = db.save_draft(next).unwrap();
            assert_eq!(next.too_large_to_sync, !fits, "{length}");
            assert_eq!(recorded_fields(&db, &draft.id).is_empty(), !fits, "{length}");
        }
        let mut small = db.draft(&draft.id).unwrap();
        small.body = "Small again".into();
        assert!(!db.save_draft(small).unwrap().too_large_to_sync);
        assert!(!recorded_fields(&db, &draft.id).is_empty());
    }

    #[test]
    fn synced_drafts_share_one_total_budget() {
        let db = database();
        let body = "x".repeat(MAX_SYNCED_DRAFT_BYTES - 4_096);
        let count = MAX_SYNCED_DRAFTS_TOTAL_BYTES / MAX_SYNCED_DRAFT_BYTES;
        let drafts: Vec<Draft> = (0..=count).map(|_| saved(&db, &body)).collect();
        assert!(drafts[..count].iter().all(|draft| !draft.too_large_to_sync));
        assert!(drafts[count].too_large_to_sync, "the draft past the budget stays local");
        let total: i64 = db.connection().unwrap().query_row("SELECT SUM(LENGTH(value)) FROM sync_values WHERE entity_type='draft' AND field LIKE 'content:%'", [], |row| row.get(0)).unwrap();
        assert!(total as usize <= MAX_SYNCED_DRAFTS_TOTAL_BYTES);
        db.discard_draft(&drafts[0].id).unwrap();
        db.reconcile_replicated_sync_backlog().unwrap();
        assert!(!recorded_fields(&db, &drafts[count].id).is_empty());
    }

    #[test]
    fn applying_a_remote_draft_keeps_local_files_and_moves_the_revision_only_on_change() {
        let db = database();
        let mut draft = saved(&db, "Hello");
        let (here, elsewhere) = (attachment(true), attachment(true));
        draft.attachments = vec![here.clone()];
        db.connection().unwrap().execute("UPDATE drafts SET payload=?1 WHERE id=?2", params![serde_json::to_string(&draft).unwrap(), draft.id]).unwrap();

        let mut remote = draft.clone();
        remote.attachments = vec![here.clone(), elsewhere.clone()];
        remote.body = "Edited elsewhere".into();
        db.materialize_synced_draft(&draft.id, &draft_sync_payload(&remote).unwrap().unwrap()).unwrap();
        let applied = db.draft(&draft.id).unwrap();
        assert_eq!(applied.body, "Edited elsewhere");
        assert_eq!(applied.revision, draft.revision + 1);
        let ready: Vec<bool> = applied.attachments.iter().map(|attachment| attachment.ready).collect();
        assert_eq!(ready, [true, false], "a file this device lacks must be attached again");

        let again = draft_sync_payload(&Draft { revision: 99, ..applied.clone() }).unwrap().unwrap();
        db.materialize_synced_draft(&draft.id, &again).unwrap();
        assert_eq!(db.draft(&draft.id).unwrap().revision, applied.revision, "unchanged content leaves an open composer alone");

        let fresh = Draft { id: Uuid::new_v4().to_string(), ..remote.clone() };
        db.materialize_synced_draft(&fresh.id, &draft_sync_payload(&fresh).unwrap().unwrap()).unwrap();
        assert!(db.draft(&fresh.id).unwrap().attachments.iter().all(|attachment| !attachment.ready));
        assert!(db.materialize_synced_draft("another-id", &draft_sync_payload(&fresh).unwrap().unwrap()).is_err());
    }

    #[test]
    fn only_the_winning_version_s_chunks_materialize() {
        let hash = "a".repeat(64);
        let losing = "b".repeat(64);
        let value = json!({"id": "d", "contentHash": hash, format!("content:{hash}:0"): "{}", format!("content:{losing}:0"): "{}"});
        let selected = selected_draft_payload(&value).unwrap();
        assert!(selected.get(format!("content:{losing}:0")).is_none());
        assert!(selected.get(format!("content:{hash}:0")).is_some());
        assert!(EntityType::Draft.validate_payload(&value).is_err());
        assert!(EntityType::Draft.validate_payload(&selected).is_ok());
    }
}
