//! Files attached to notes.
//!
//! A note refers to an attachment by `attachment:<id>` in its Markdown. The
//! attachment's description (name, type, size, hash) is one replicated
//! entity (`EntityType::NoteAttachment`); its bytes stay in
//! `note_attachment_data` and travel to other devices as sealed objects of
//! their own, so a file never rides in, or grows, a snapshot. An
//! attachment is recorded in the replica only once its bytes are sealed
//! (see `replicated_sync::seal_note_attachments`), so another device never
//! learns of a file it has no way to fetch.
//!
//! An attachment lives as long as its note: deleting a note from Trash
//! deletes its attachments on every device. Removing an image from the
//! text keeps the file, so undo and older versions still show it.

use std::collections::BTreeSet;

use chrono::{Duration, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use threestrands_sync_protocol::{
    EntityType, MAX_NOTE_ATTACHMENT_BYTES, MAX_NOTE_ATTACHMENT_NAME_CHARS,
    MAX_SYNCED_NOTE_ATTACHMENTS,
};

use super::notes::sync_recording;
use super::sync_state::{
    record_replicated_deletion_in_transaction, record_replicated_write_in_transaction,
};
use super::{Database, DatabaseError, DbResult};
use crate::image_format;
use crate::models::NoteAttachment;
use crate::replicated_sync::{encode_id, hex_encode, random_id};

/// How a Markdown destination names an attachment: `attachment:<id>`.
pub(crate) const ATTACHMENT_SCHEME: &str = "attachment:";

const COLUMNS: &str = "a.id, a.note_id, a.name, a.mime_type, a.size, \
     EXISTS(SELECT 1 FROM note_attachment_data d WHERE d.id=a.id)";

const TOO_LARGE: &str = "This file is larger than 20 MB, the most a note can hold.";
const TOO_MANY: &str = "Notes already hold 2,000 files, the most they can sync. Delete notes with files you no longer need from Trash to add more.";

/// The longest a download of another device's file waits before it is
/// tried again.
const MAX_FETCH_BACKOFF_MINUTES: i64 = 6 * 60;

fn attachment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteAttachment> {
    let mime_type: String = row.get(3)?;
    Ok(NoteAttachment {
        id: row.get(0)?,
        note_id: row.get(1)?,
        name: row.get(2)?,
        image: image_format::is_supported_raster_mime(&mime_type),
        mime_type,
        size: row.get::<_, i64>(4)? as u64,
        available: row.get(5)?,
    })
}

/// Whether `id` has the shape of an attachment id: 32 lowercase hex digits.
pub(crate) fn is_attachment_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The attachment id a Markdown destination names, if it names one.
pub(crate) fn attachment_id_of(destination: &str) -> Option<&str> {
    destination
        .strip_prefix(ATTACHMENT_SCHEME)
        .filter(|id| is_attachment_id(id))
}

/// A file name safe to store and show: the last path segment, with control
/// characters removed, and no longer than the synchronized limit.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NOTE_ATTACHMENT_NAME_CHARS)
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "Attachment".to_string()
    } else {
        cleaned
    }
}

/// The type to store for a file. An image is identified by its bytes, never
/// by the name or type it arrived with, so only real raster images are
/// shown in a note. Anything else keeps a plausible declared type, or
/// `application/octet-stream`.
fn stored_mime_type(declared: &str, bytes: &[u8]) -> String {
    if let Some(sniffed) = image_format::sniff_raster_mime(bytes) {
        return sniffed.to_string();
    }
    let essence = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&byte))
    };
    match essence.split_once('/') {
        Some((kind, sub))
            if essence.len() <= 127
                && token(kind)
                && token(sub)
                && !image_format::is_supported_raster_mime(&essence) =>
        {
            essence
        }
        _ => "application/octet-stream".to_string(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

/// The synchronized fields of a sealed attachment.
fn sync_payload(tx: &Transaction<'_>, id: &str) -> DbResult<Option<Value>> {
    Ok(tx
        .query_row(
            "SELECT note_id, name, mime_type, size, sha256, created_at, sealed_by, index_cid
             FROM note_attachments WHERE id=?1 AND index_cid IS NOT NULL",
            [id],
            |row| {
                Ok(json!({
                    "id": id,
                    "noteId": row.get::<_, String>(0)?,
                    "name": row.get::<_, String>(1)?,
                    "mimeType": row.get::<_, String>(2)?,
                    "size": row.get::<_, i64>(3)?,
                    "sha256": row.get::<_, String>(4)?,
                    "createdAt": row.get::<_, String>(5)?,
                    "sealedBy": row.get::<_, String>(6)?,
                    "index": row.get::<_, String>(7)?,
                }))
            },
        )
        .optional()?)
}

fn in_replica(tx: &Transaction<'_>, id: &str) -> DbResult<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type=?1 AND entity_id=?2)",
        params![EntityType::NoteAttachment.as_str(), id],
        |row| row.get(0),
    )?)
}

/// Schedules the objects this device sealed for `id` to be deleted from
/// each connector that has them, once a snapshot without the attachment is
/// published there, and drops any delivery still waiting.
fn retire_sealed_objects(tx: &Transaction<'_>, id: &str) -> DbResult<()> {
    let sequence: i64 = tx
        .query_row(
            "SELECT state_sequence FROM sync_local_state WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let cids: Vec<String> = {
        let mut statement =
            tx.prepare("SELECT cid FROM note_attachment_objects WHERE attachment_id=?1")?;
        let rows = statement.query_map([id], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for cid in cids {
        tx.execute(
            "INSERT OR IGNORE INTO sync_retired_objects(cid, transport_instance_id, state_sequence)
             SELECT cid, transport_instance_id, ?2 FROM sync_deliveries WHERE cid=?1 AND state='delivered'",
            params![cid, sequence],
        )?;
        tx.execute("DELETE FROM sync_deliveries WHERE cid=?1", [&cid])?;
        tx.execute("DELETE FROM sync_objects WHERE cid=?1", [&cid])?;
    }
    tx.execute(
        "DELETE FROM note_attachment_objects WHERE attachment_id=?1",
        [id],
    )?;
    Ok(())
}

fn remove(tx: &Transaction<'_>, id: &str) -> DbResult<()> {
    retire_sealed_objects(tx, id)?;
    tx.execute("DELETE FROM note_attachment_data WHERE id=?1", [id])?;
    tx.execute("DELETE FROM note_attachments WHERE id=?1", [id])?;
    Ok(())
}

/// Deletes every attachment of `note_id`, on every device once the replica
/// holds it. Part of deleting the note, in its transaction.
pub(super) fn delete_note_attachments(tx: &Transaction<'_>, note_id: &str) -> DbResult<()> {
    let ids: Vec<String> = {
        let mut statement = tx.prepare("SELECT id FROM note_attachments WHERE note_id=?1")?;
        let rows = statement.query_map([note_id], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in ids {
        let recorded = in_replica(tx, &id)?;
        remove(tx, &id)?;
        if recorded {
            record_replicated_deletion_in_transaction(tx, EntityType::NoteAttachment, &id)?;
        }
    }
    Ok(())
}

/// Stores a file as attachment `id` of `note_id`, within the caller's
/// transaction, after the size and count checks every new file gets.
pub(super) fn insert_attachment(
    tx: &Transaction<'_>,
    id: &str,
    note_id: &str,
    name: &str,
    declared_type: &str,
    bytes: &[u8],
) -> DbResult<()> {
    if bytes.len() as u64 > MAX_NOTE_ATTACHMENT_BYTES {
        return Err(DatabaseError::invalid(TOO_LARGE));
    }
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM note_attachments", [], |row| {
        row.get(0)
    })?;
    if count as usize >= MAX_SYNCED_NOTE_ATTACHMENTS {
        return Err(DatabaseError::invalid(TOO_MANY));
    }
    tx.execute(
        "INSERT INTO note_attachments(id, note_id, name, mime_type, size, sha256, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            id,
            note_id,
            clean_name(name),
            stored_mime_type(declared_type, bytes),
            bytes.len() as i64,
            sha256_hex(bytes),
            Utc::now().to_rfc3339()
        ],
    )?;
    tx.execute(
        "INSERT INTO note_attachment_data(id, bytes) VALUES (?1, ?2)",
        params![id, bytes],
    )?;
    Ok(())
}

/// A sealed attachment's objects, ready to store: each one's CID, kind
/// (`attachment_chunk` or `attachment_index`), and bytes.
pub(crate) struct SealedAttachment {
    pub(crate) sealed_by: String,
    pub(crate) index_cid: String,
    pub(crate) objects: Vec<(String, &'static str, Vec<u8>)>,
}

/// Another device's file this device should download now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentFetch {
    pub(crate) id: String,
    pub(crate) sealed_by: String,
    pub(crate) index_cid: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

impl Database {
    /// Stores a file the user attached to a note and returns it. The note
    /// is named by id; a daily note doesn't need to be saved yet. The file is
    /// sealed for other devices on the next sync.
    pub fn add_note_attachment(
        &self,
        note_id: &str,
        name: &str,
        declared_type: &str,
        bytes: &[u8],
    ) -> DbResult<NoteAttachment> {
        if bytes.len() as u64 > MAX_NOTE_ATTACHMENT_BYTES {
            return Err(DatabaseError::invalid(TOO_LARGE));
        }
        let daily = note_id
            .strip_prefix(threestrands_sync_protocol::DAILY_NOTE_ID_PREFIX)
            .is_some_and(crate::notes::is_daily_date);
        let id = encode_id(&random_id());
        self.with_transaction(|tx| {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM notes WHERE id=?1)",
                [note_id],
                |row| row.get(0),
            )?;
            if !exists && !daily {
                return Err(DatabaseError::NotFound("Note"));
            }
            insert_attachment(tx, &id, note_id, name, declared_type, bytes)
        })?;
        self.note_attachment(&id)?
            .ok_or(DatabaseError::NotFound("Attachment"))
    }

    pub fn note_attachment(&self, id: &str) -> DbResult<Option<NoteAttachment>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    &format!("SELECT {COLUMNS} FROM note_attachments a WHERE a.id=?1"),
                    [id],
                    attachment_from_row,
                )
                .optional()?)
        })
    }

    /// An attachment and its bytes, when this device has them.
    pub fn note_attachment_bytes(&self, id: &str) -> DbResult<Option<(NoteAttachment, Vec<u8>)>> {
        let Some(attachment) = self.note_attachment(id)? else {
            return Ok(None);
        };
        let bytes: Option<Vec<u8>> = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT bytes FROM note_attachment_data WHERE id=?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        Ok(bytes.map(|bytes| (attachment, bytes)))
    }

    /// The attachments of notes outside Trash, for export.
    pub fn note_attachments_for_export(&self) -> DbResult<Vec<NoteAttachment>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {COLUMNS} FROM note_attachments a
                 JOIN notes n ON n.id = a.note_id
                 WHERE n.trashed=0 ORDER BY a.note_id, a.created_at, a.id"
            ))?;
            let rows = statement.query_map([], attachment_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

// ---- Sync ----

impl Database {
    /// Attachments whose bytes this device holds but hasn't sealed for the
    /// current group yet. Empty while sync isn't recording.
    pub(crate) fn note_attachments_to_seal(&self) -> DbResult<Vec<String>> {
        self.with_transaction(|tx| {
            if !sync_recording(tx)? {
                return Ok(Vec::new());
            }
            let mut statement = tx.prepare(
                "SELECT a.id FROM note_attachments a
                 WHERE a.index_cid IS NULL
                   AND EXISTS(SELECT 1 FROM note_attachment_data d WHERE d.id=a.id)
                 ORDER BY a.created_at, a.id",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Stores the sealed objects of attachment `id`, schedules their delivery
    /// to every connector, and records the attachment in the replica, in
    /// one transaction. A no-op when the attachment was deleted meanwhile.
    pub(crate) fn store_sealed_note_attachment(
        &self,
        id: &str,
        sealed: &SealedAttachment,
        transports: &[threestrands_sync_transport::TransportInstanceId],
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let unsealed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM note_attachments WHERE id=?1 AND index_cid IS NULL)",
                [id],
                |row| row.get(0),
            )?;
            if !unsealed {
                return Ok(());
            }
            for (index, (cid, kind, bytes)) in sealed.objects.iter().enumerate() {
                tx.execute(
                    "INSERT OR IGNORE INTO sync_objects(cid, object_kind, state_sequence, chunk_index, chunk_count, bytes)
                     VALUES (?1, ?2, NULL, ?3, ?4, ?5)",
                    params![cid, kind, index as i64, sealed.objects.len() as i64, bytes],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO note_attachment_objects(attachment_id, cid) VALUES (?1, ?2)",
                    params![id, cid],
                )?;
                for transport in transports {
                    tx.execute(
                        "INSERT OR IGNORE INTO sync_deliveries(cid, transport_instance_id, state, attempts)
                         VALUES (?1, ?2, 'pending', 0)",
                        params![cid, transport.0],
                    )?;
                }
            }
            tx.execute(
                "UPDATE note_attachments SET sealed_by=?2, index_cid=?3 WHERE id=?1",
                params![id, sealed.sealed_by, sealed.index_cid],
            )?;
            let payload = sync_payload(tx, id)?.ok_or(DatabaseError::NotFound("Attachment"))?;
            let fields: BTreeSet<String> = payload
                .as_object()
                .map(|object| object.keys().cloned().collect())
                .unwrap_or_default();
            record_replicated_write_in_transaction(
                tx,
                EntityType::NoteAttachment,
                id,
                &fields,
                &payload,
            )
        })
    }

    /// Sealed attachments' synchronized fields, for the backlog sweep.
    pub(crate) fn note_attachment_sync_payloads(&self) -> DbResult<Vec<(String, Value)>> {
        self.with_transaction(|tx| {
            let ids: Vec<String> = {
                let mut statement = tx.prepare(
                    "SELECT id FROM note_attachments WHERE index_cid IS NOT NULL ORDER BY id",
                )?;
                let rows = statement.query_map([], |row| row.get(0))?;
                rows.collect::<Result<_, _>>()?
            };
            let mut payloads = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(payload) = sync_payload(tx, &id)? {
                    payloads.push((id, payload));
                }
            }
            Ok(payloads)
        })
    }

    /// Another device's files this device hasn't downloaded and may try now,
    /// oldest first.
    pub(crate) fn note_attachments_to_fetch(&self) -> DbResult<Vec<AttachmentFetch>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT a.id, a.sealed_by, a.index_cid, a.size, a.sha256 FROM note_attachments a
                 WHERE a.index_cid IS NOT NULL AND a.sealed_by IS NOT NULL
                   AND NOT EXISTS(SELECT 1 FROM note_attachment_data d WHERE d.id=a.id)
                   AND (a.fetch_after IS NULL OR a.fetch_after <= ?1)
                 ORDER BY a.created_at, a.id",
            )?;
            let rows = statement.query_map([Utc::now().to_rfc3339()], |row| {
                Ok(AttachmentFetch {
                    id: row.get(0)?,
                    sealed_by: row.get(1)?,
                    index_cid: row.get(2)?,
                    size: row.get::<_, i64>(3)? as u64,
                    sha256: row.get(4)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Stores a downloaded file, once it is exactly the file its record
    /// describes. A record that changed meanwhile (deleted, or sealed again
    /// with other objects) keeps waiting for its own download.
    pub(crate) fn store_fetched_note_attachment(
        &self,
        fetch: &AttachmentFetch,
        bytes: &[u8],
    ) -> DbResult<()> {
        if bytes.len() as u64 != fetch.size || sha256_hex(bytes) != fetch.sha256 {
            return Err(DatabaseError::invalid(
                "The downloaded file doesn't match its description",
            ));
        }
        self.with_transaction(|tx| {
            let current: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM note_attachments WHERE id=?1 AND index_cid=?2 AND sha256=?3)",
                params![fetch.id, fetch.index_cid, fetch.sha256],
                |row| row.get(0),
            )?;
            if current {
                tx.execute(
                    "INSERT OR REPLACE INTO note_attachment_data(id, bytes) VALUES (?1, ?2)",
                    params![fetch.id, bytes],
                )?;
                tx.execute(
                    "UPDATE note_attachments SET fetch_attempts=0, fetch_after=NULL, fetch_error=NULL WHERE id=?1",
                    [&fetch.id],
                )?;
            }
            Ok(())
        })
    }

    /// Records a failed download and when to try again: a minute after the
    /// first failure, doubling up to six hours.
    pub(crate) fn record_note_attachment_fetch_failure(
        &self,
        id: &str,
        error: &str,
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            let attempts: i64 = connection
                .query_row(
                    "SELECT fetch_attempts FROM note_attachments WHERE id=?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            let minutes = 1i64
                .checked_shl(attempts.clamp(0, 16) as u32)
                .unwrap_or(i64::MAX)
                .min(MAX_FETCH_BACKOFF_MINUTES);
            connection.execute(
                "UPDATE note_attachments SET fetch_attempts=fetch_attempts+1, fetch_after=?2, fetch_error=?3 WHERE id=?1",
                params![id, (Utc::now() + Duration::minutes(minutes)).to_rfc3339(), error],
            )?;
            Ok(())
        })
    }

    /// Stores an attachment record another device wrote. The bytes follow
    /// separately (see `note_attachments_to_fetch`). When the record now
    /// names other sealed objects than this device's own, this device's
    /// copies are retired, and a download already held is kept only if
    /// the file is the same.
    pub(crate) fn upsert_synced_note_attachment(&self, id: &str, payload: &Value) -> DbResult<()> {
        EntityType::NoteAttachment
            .validate_payload(payload)
            .map_err(DatabaseError::invalid)?;
        let text = |key: &str| payload[key].as_str().unwrap_or_default().to_string();
        if text("id") != id {
            return Err(DatabaseError::invalid("Attachment identity mismatch"));
        }
        let (note_id, name, mime_type, sha256, sealed_by, index_cid) = (
            text("noteId"),
            clean_name(&text("name")),
            text("mimeType"),
            text("sha256"),
            text("sealedBy"),
            text("index"),
        );
        let size = payload["size"].as_u64().unwrap_or_default() as i64;
        let created_at = payload["createdAt"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        self.with_transaction(|tx| {
            let existing: Option<(Option<String>, String)> = tx
                .query_row(
                    "SELECT index_cid, sha256 FROM note_attachments WHERE id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((old_index, old_sha256)) = &existing {
                if old_index.as_deref() != Some(index_cid.as_str()) {
                    retire_sealed_objects(tx, id)?;
                }
                if *old_sha256 != sha256 {
                    tx.execute("DELETE FROM note_attachment_data WHERE id=?1", [id])?;
                }
            }
            tx.execute(
                "INSERT INTO note_attachments(id, note_id, name, mime_type, size, sha256, created_at, sealed_by, index_cid)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(id) DO UPDATE SET note_id=excluded.note_id, name=excluded.name,
                    mime_type=excluded.mime_type, size=excluded.size, sha256=excluded.sha256,
                    created_at=excluded.created_at, sealed_by=excluded.sealed_by, index_cid=excluded.index_cid,
                    fetch_attempts=CASE WHEN note_attachments.index_cid IS excluded.index_cid THEN fetch_attempts ELSE 0 END,
                    fetch_after=CASE WHEN note_attachments.index_cid IS excluded.index_cid THEN fetch_after END",
                params![id, note_id, name, mime_type, size, sha256, created_at, sealed_by, index_cid],
            )?;
            Ok(())
        })
    }

    /// Removes an attachment another device deleted, and the objects this
    /// device sealed for it.
    pub(crate) fn delete_synced_note_attachment(&self, id: &str) -> DbResult<()> {
        self.with_transaction(|tx| remove(tx, id))
    }
}

#[cfg(test)]
#[path = "tests/note_attachments.rs"]
mod tests;
