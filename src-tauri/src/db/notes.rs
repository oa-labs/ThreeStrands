//! Note persistence and the indexes built from notes.
//!
//! A note is one replicated entity (`EntityType::Note`). A local change
//! writes the `notes` row and records the changed fields in the replica in
//! the same transaction. While sync is off, the changed field names wait in
//! `replica_dirty_fields` and are recorded once it is on. The link index,
//! search index, and visit times belong to this device and are rebuilt from
//! the notes; none of them leaves it.

use std::collections::{BTreeSet, HashSet};

use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use threestrands_sync_protocol::{EntityType, MAX_NOTE_TITLE_CHARS, MAX_SYNCED_NOTES_TOTAL_BYTES};

use super::search::fts_query;
use super::sync_state::{
    record_replicated_deletion_in_transaction, record_replicated_write_in_transaction,
};
use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::models::{
    Note, NoteBacklink, NoteLinks, NoteRelink, NoteSave, NoteSearchHit, NoteSearchScope,
    NoteSummary, NoteTitle, NotesStorage,
};
use crate::notes::{
    clean_note_title, daily_note_id, extract_links, is_daily_date, plain_text, rewrite_links,
    title_key,
};

const NOTE_COLUMNS: &str =
    "id, title, body, daily_date, created_at, updated_at, trashed, trashed_at, authored_title";
/// Most characters of a note's body shown under its title in the list.
const PREVIEW_CHARS: usize = 140;
/// Most rows one list or search returns.
const MAX_LIST_ROWS: usize = 500;

const TOO_LONG: &str = "This note is too long to sync. Shorten it to save your changes.";
const CONFLICTED: &str =
    "This note was also changed on another device. Choose which version to keep before editing it.";
const OUT_OF_SPACE: &str =
    "Notes have used all of their sync space. Empty Trash or shorten other notes to save this one.";

fn note_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    let title: String = row.get(1)?;
    let authored: String = row.get(8)?;
    Ok(Note {
        id: row.get(0)?,
        authored_title: (authored != title).then_some(authored),
        title,
        body: row.get(2)?,
        daily_date: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        trashed: row.get(6)?,
        trashed_at: row.get(7)?,
    })
}

pub(crate) fn get(tx: &Transaction<'_>, id: &str) -> DbResult<Option<Note>> {
    Ok(tx
        .query_row(
            &format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id=?1"),
            [id],
            note_from_row,
        )
        .optional()?)
}

/// The synchronized fields of a note. A daily note's date stands in for its
/// creation time, so two devices that start the same day's note agree on
/// every field but the body.
fn sync_payload(note: &Note) -> Value {
    let mut payload = json!({
        "id": note.id,
        "title": note.title,
        "body": note.body,
        "trashed": note.trashed,
    });
    match &note.daily_date {
        Some(date) => payload["dailyDate"] = json!(date),
        None => payload["createdAt"] = json!(note.created_at),
    }
    payload
}

/// The replica keeps the authored title, even when this device displays a
/// disambiguated title. Body edits and Trash changes must not publish that
/// derived name as an unintended title edit.
fn write_payload(tx: &Transaction<'_>, note: &Note, changed: &BTreeSet<String>) -> DbResult<Value> {
    let mut payload = sync_payload(note);
    if !changed.contains("title") {
        let authored: Option<String> = tx
            .query_row(
                "SELECT authored_title FROM notes WHERE id=?1",
                [&note.id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(title) = authored {
            payload["title"] = json!(title);
        }
    }
    Ok(payload)
}

/// A deterministic projection of authored titles: the lowest note id keeps
/// each title; the others get numbered names. Reserve all authored names
/// before generating suffixes, so an actual "Plan 2" is never displaced.
/// This changes neither the replica nor bodies: title-only links cannot
/// identify which independently-created note their author meant.
pub(crate) fn reconcile_titles(tx: &Transaction<'_>) -> DbResult<()> {
    let notes = {
        let mut query =
            tx.prepare("SELECT id, title, authored_title FROM notes WHERE trashed=0 ORDER BY id")?;
        let rows = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let mut reserved: HashSet<String> =
        notes.iter().map(|(_, _, title)| title_key(title)).collect();
    let mut claimed = HashSet::new();
    for (id, displayed, authored) in notes {
        let mut title = authored.clone();
        if !claimed.insert(title_key(&authored)) {
            let mut number = 2;
            loop {
                let suffix = format!(" {number}");
                let base: String = authored
                    .chars()
                    .take(MAX_NOTE_TITLE_CHARS - suffix.len())
                    .collect();
                title = format!("{}{suffix}", base.trim_end());
                if reserved.insert(title_key(&title)) {
                    break;
                }
                number += 1;
            }
        }
        if displayed != title {
            let mut note = get(tx, &id)?.ok_or(DatabaseError::NotFound("Note"))?;
            note.title = title;
            note.updated_at = Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE notes SET title=?2, title_key=?3, updated_at=?4 WHERE id=?1",
                params![note.id, note.title, title_key(&note.title), note.updated_at],
            )?;
            index(tx, &note)?;
        }
    }
    Ok(())
}

fn payload_bytes(payload: &Value) -> DbResult<i64> {
    Ok(serde_json::to_vec(payload)
        .map_err(serialization_error)?
        .len() as i64)
}

/// Whether local changes are recorded for other devices now.
pub(super) fn sync_recording(tx: &Transaction<'_>) -> DbResult<bool> {
    if crate::replicated_sync::enabled() {
        return Ok(true);
    }
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_spaces WHERE enabled=1)",
        [],
        |row| row.get(0),
    )?)
}

fn dirty_fields(tx: &Transaction<'_>, id: &str) -> DbResult<BTreeSet<String>> {
    let json: Option<String> = tx
        .query_row(
            "SELECT replica_dirty_fields FROM notes WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    json.map(|json| serde_json::from_str(&json).map_err(serialization_error))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Refuses a local change that the snapshot couldn't carry: a field over its
/// limit, or a note that would take notes past their share of the
/// snapshot. A change that doesn't grow the note is always allowed, so a
/// user over the budget (from notes another device added) can still trim.
fn check_sync_limits(tx: &Transaction<'_>, note: &Note, payload: &Value) -> DbResult<i64> {
    EntityType::Note
        .validate_payload(payload)
        .map_err(|error| match error.as_str() {
            "The note is too long to sync" => DatabaseError::invalid(TOO_LONG),
            _ => DatabaseError::invalid(error),
        })?;
    let bytes = payload_bytes(payload)?;
    let (others, current): (i64, Option<i64>) = tx.query_row(
        "SELECT COALESCE(SUM(CASE WHEN id<>?1 THEN sync_bytes END), 0),
                MAX(CASE WHEN id=?1 THEN sync_bytes END)
         FROM notes",
        [&note.id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let grows = current.is_none_or(|current| bytes > current);
    if grows && others + bytes > MAX_SYNCED_NOTES_TOTAL_BYTES as i64 {
        return Err(DatabaseError::invalid(OUT_OF_SPACE));
    }
    Ok(bytes)
}

/// Writes the note row and rebuilds its link and search entries. With
/// `changed`, this is a local change: those fields are recorded in the
/// replica now if sync is on, or remembered until it is.
fn store(
    tx: &Transaction<'_>,
    note: &Note,
    sync_bytes: i64,
    changed: Option<&BTreeSet<String>>,
) -> DbResult<()> {
    let pending = match changed {
        Some(changed) => {
            let mut fields = dirty_fields(tx, &note.id)?;
            fields.extend(changed.iter().cloned());
            fields
        }
        None => dirty_fields(tx, &note.id)?,
    };
    let recording = changed.is_some() && sync_recording(tx)?;
    let stored_pending = if recording || pending.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&pending).map_err(serialization_error)?)
    };
    let authored_payload = match changed {
        Some(fields) => write_payload(tx, note, fields)?,
        None => sync_payload(note),
    };
    let previous: Option<(String, String, bool, Option<String>)> = tx
        .query_row(
            "SELECT title, authored_title, trashed, daily_date FROM notes WHERE id=?1",
            [&note.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let titles_changed = previous
        .as_ref()
        .is_none_or(|(_, authored, trashed, daily)| {
            authored_payload["title"].as_str() != Some(authored.as_str())
                || *trashed != note.trashed
                || daily != &note.daily_date
        });
    let mut projected = note.clone();
    if !titles_changed {
        // A remote body edit carries the authored title. Keep its existing
        // display name without reassigning every other note's name.
        projected.title = previous.as_ref().unwrap().0.clone();
    }
    let note = &projected;
    tx.execute(
        "INSERT INTO notes(id, title, title_key, body, daily_date, created_at, updated_at, trashed, trashed_at, sync_bytes, replica_dirty_fields, authored_title)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, title_key=excluded.title_key,
            body=excluded.body, daily_date=excluded.daily_date, created_at=excluded.created_at,
            updated_at=excluded.updated_at, trashed=excluded.trashed, trashed_at=excluded.trashed_at,
            sync_bytes=excluded.sync_bytes, replica_dirty_fields=excluded.replica_dirty_fields,
            authored_title=excluded.authored_title",
        params![
            note.id,
            note.title,
            title_key(&note.title),
            note.body,
            note.daily_date,
            note.created_at,
            note.updated_at,
            note.trashed,
            note.trashed_at,
            sync_bytes,
            stored_pending,
            authored_payload["title"].as_str().unwrap_or_default(),
        ],
    )?;
    index(tx, note)?;
    if recording {
        record_replicated_write_in_transaction(
            tx,
            EntityType::Note,
            &note.id,
            &pending,
            &authored_payload,
        )?;
    }
    if titles_changed {
        reconcile_titles(tx)?;
    }
    Ok(())
}

fn index(tx: &Transaction<'_>, note: &Note) -> DbResult<()> {
    super::note_entities::index(tx, note)?;
    index_links(tx, note)?;
    tx.execute("DELETE FROM note_search WHERE note_id=?1", [&note.id])?;
    tx.execute(
        "INSERT INTO note_search(note_id, title, body) VALUES (?1,?2,?3)",
        params![note.id, note.title, note.body],
    )?;
    Ok(())
}

/// Rebuilds a note's `[[link]]` rows from its text.
pub(crate) fn index_links(tx: &Transaction<'_>, note: &Note) -> DbResult<()> {
    tx.execute("DELETE FROM note_links WHERE note_id=?1", [&note.id])?;
    for (position, link) in extract_links(&note.body).iter().enumerate() {
        tx.execute(
            "INSERT INTO note_links(note_id, position, target_key, target_title, context) VALUES (?1,?2,?3,?4,?5)",
            params![note.id, position as i64, link.key, link.title, link.context],
        )?;
    }
    Ok(())
}

fn remove(tx: &Transaction<'_>, id: &str) -> DbResult<()> {
    for statement in [
        "DELETE FROM notes WHERE id=?1",
        "DELETE FROM note_links WHERE note_id=?1",
        "DELETE FROM note_search WHERE note_id=?1",
        "DELETE FROM note_visits WHERE note_id=?1",
        "DELETE FROM note_tasks WHERE note_id=?1",
        "DELETE FROM note_entity_links WHERE note_id=?1",
        "DELETE FROM note_index_warnings WHERE note_id=?1",
    ] {
        tx.execute(statement, [id])?;
    }
    Ok(())
}

/// Refuses a title another note in use already has, since a `[[link]]`
/// names one note by its title.
pub(super) fn check_title_free(tx: &Transaction<'_>, id: &str, title: &str) -> DbResult<()> {
    let taken: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM notes WHERE title_key=?1 AND id<>?2 AND trashed=0)",
        params![title_key(title), id],
        |row| row.get(0),
    )?;
    if taken {
        return Err(DatabaseError::invalid(format!(
            "A note named \u{201c}{title}\u{201d} already exists"
        )));
    }
    Ok(())
}

/// Whether the replica holds different values for one of a note's fields,
/// written on different devices before either saw the other's. A local
/// write replaces every value of a field, so editing it now would discard
/// the other device's version without the user choosing.
fn field_in_conflict(tx: &Transaction<'_>, id: &str, field: &str) -> DbResult<bool> {
    let values: i64 = tx.query_row(
        "SELECT COUNT(DISTINCT value) FROM sync_values WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
        params![EntityType::Note.as_str(), id, field],
        |row| row.get(0),
    )?;
    Ok(values > 1)
}

/// Applies a local edit of title and body, writing only when one changed.
/// A field in conflict can't be edited until the user picks a version.
pub(super) fn edit(
    tx: &Transaction<'_>,
    mut note: Note,
    title: String,
    body: &str,
) -> DbResult<Note> {
    if note.trashed {
        return Err(DatabaseError::invalid("Restore this note to edit it"));
    }
    let mut changed = BTreeSet::new();
    if note.title != title {
        check_title_free(tx, &note.id, &title)?;
        note.title = title;
        changed.insert("title".to_string());
    }
    if note.body != body {
        note.body =
            crate::note_entities::normalize(&note.id, body).map_err(DatabaseError::invalid)?;
        changed.insert("body".to_string());
    }
    if changed.is_empty() {
        return Ok(note);
    }
    for field in &changed {
        if field_in_conflict(tx, &note.id, field)? {
            return Err(DatabaseError::invalid(CONFLICTED));
        }
    }
    note.updated_at = Utc::now().to_rfc3339();
    let bytes = check_sync_limits(tx, &note, &write_payload(tx, &note, &changed)?)?;
    store(tx, &note, bytes, Some(&changed))?;
    get(tx, &note.id)?.ok_or(DatabaseError::NotFound("Note"))
}

pub(super) fn create(tx: &Transaction<'_>, mut note: Note) -> DbResult<Note> {
    note.body =
        crate::note_entities::normalize(&note.id, &note.body).map_err(DatabaseError::invalid)?;
    let payload = sync_payload(&note);
    let bytes = check_sync_limits(tx, &note, &payload)?;
    let fields = payload
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    store(tx, &note, bytes, Some(&fields))?;
    get(tx, &note.id)?.ok_or(DatabaseError::NotFound("Note"))
}

fn set_trashed(tx: &Transaction<'_>, id: &str, trashed: bool) -> DbResult<Note> {
    let mut note = get(tx, id)?.ok_or(DatabaseError::NotFound("Note"))?;
    if note.trashed == trashed {
        return Ok(note);
    }
    if !trashed && note.daily_date.is_none() {
        // Another note may have taken the title while this one was in Trash.
        check_title_free(tx, id, &note.title).map_err(|_| {
            DatabaseError::invalid(format!(
                "A note named \u{201c}{}\u{201d} already exists. Rename that note, then restore this one.",
                note.title
            ))
        })?;
    }
    note.trashed = trashed;
    note.trashed_at = trashed.then(|| Utc::now().to_rfc3339());
    let changed = BTreeSet::from(["trashed".to_string()]);
    let bytes = payload_bytes(&write_payload(tx, &note, &changed)?)?;
    store(tx, &note, bytes, Some(&changed))?;
    get(tx, &note.id)?.ok_or(DatabaseError::NotFound("Note"))
}

fn update_in(
    tx: &Transaction<'_>,
    id: &str,
    title: &str,
    body: &str,
) -> DbResult<(Note, Option<NoteRelink>)> {
    let note = get(tx, id)?.ok_or(DatabaseError::NotFound("Note"))?;
    let old_key = title_key(&note.title);
    let title = if note.daily_date.is_some() {
        note.title.clone()
    } else {
        clean_note_title(title).map_err(DatabaseError::invalid)?
    };
    let saved = edit(tx, note, title, body)?;
    // Links match titles by key, so a change of case or spacing leaves
    // them working and rewrites nothing.
    let relink = if title_key(&saved.title) == old_key {
        None
    } else {
        Some(relink(tx, &saved.id, &old_key, &saved.title)?)
    };
    Ok((saved, relink))
}

/// Points links to a renamed note at its new title, as ordinary edits of
/// the notes outside Trash that hold them. A note whose body is in
/// conflict, or that the change would push past a sync limit, keeps the
/// old link and is named in the result.
fn relink(tx: &Transaction<'_>, id: &str, old_key: &str, new_title: &str) -> DbResult<NoteRelink> {
    let ids: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT DISTINCT n.id FROM note_links l JOIN notes n ON n.id=l.note_id
             WHERE l.target_key=?1 AND n.id<>?2 AND n.trashed=0
             ORDER BY n.id",
        )?;
        let rows = statement.query_map(params![old_key, id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut result = NoteRelink {
        updated: 0,
        kept_old_link: Vec::new(),
    };
    for linking_id in ids {
        let Some(mut note) = get(tx, &linking_id)? else {
            continue;
        };
        let Some(body) = rewrite_links(&note.body, old_key, new_title) else {
            continue;
        };
        if field_in_conflict(tx, &note.id, "body")? {
            result.kept_old_link.push(note.title);
            continue;
        }
        note.body = body;
        note.updated_at = Utc::now().to_rfc3339();
        let changed = BTreeSet::from(["body".to_string()]);
        let bytes = match check_sync_limits(tx, &note, &write_payload(tx, &note, &changed)?) {
            Ok(bytes) => bytes,
            Err(DatabaseError::Validation(_)) => {
                result.kept_old_link.push(note.title);
                continue;
            }
            Err(error) => return Err(error),
        };
        store(
            tx,
            &note,
            bytes,
            Some(&BTreeSet::from(["body".to_string()])),
        )?;
        result.updated += 1;
    }
    Ok(result)
}

pub(super) fn save_daily_in(
    tx: &Transaction<'_>,
    date: &str,
    body: &str,
) -> DbResult<Option<Note>> {
    let id = daily_note_id(date);
    match get(tx, &id)? {
        Some(note) => {
            let title = note.title.clone();
            edit(tx, note, title, body).map(Some)
        }
        None if body.is_empty() => Ok(None),
        None => {
            let now = Utc::now().to_rfc3339();
            create(
                tx,
                Note {
                    id,
                    title: date.to_string(),
                    body: body.to_string(),
                    daily_date: Some(date.to_string()),
                    created_at: now.clone(),
                    updated_at: now,
                    trashed: false,
                    trashed_at: None,
                    authored_title: None,
                },
            )
            .map(Some)
        }
    }
}

/// Whether an edit that began at `expected_updated_at` can still be saved
/// over `current`. Trashing (here or on another device) changes no text, so
/// it leaves `updated_at` alone; a note in Trash is never current, so the
/// editor offers its choices instead of a save that would be refused.
pub(super) fn is_current(current: Option<&Note>, expected_updated_at: Option<&str>) -> bool {
    current.map(|note| note.updated_at.as_str()) == expected_updated_at
        && !current.is_some_and(|note| note.trashed)
}

fn note_title_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteTitle> {
    Ok(NoteTitle {
        id: row.get(0)?,
        title: row.get(1)?,
        daily_date: row.get(2)?,
    })
}

fn preview(body: &str) -> String {
    let text = plain_text(body);
    if text.chars().count() <= PREVIEW_CHARS {
        return text;
    }
    let mut preview: String = text.chars().take(PREVIEW_CHARS - 1).collect();
    preview.push('…');
    preview
}

fn summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteSummary> {
    Ok(NoteSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        daily_date: row.get(2)?,
        preview: preview(&row.get::<_, String>(3)?),
        updated_at: row.get(4)?,
        visited_at: row.get(5)?,
        trashed_at: row.get(6)?,
    })
}

fn note_field<'a>(payload: &'a Value, field: &str) -> Option<&'a str> {
    payload.get(field).and_then(Value::as_str)
}

impl Database {
    pub fn get_note(&self, id: &str) -> DbResult<Option<Note>> {
        self.with_transaction(|tx| get(tx, id))
    }

    /// The daily note for `date`, if one was written.
    pub fn get_daily_note(&self, date: &str) -> DbResult<Option<Note>> {
        if !is_daily_date(date) {
            return Err(DatabaseError::invalid("That isn't a valid date"));
        }
        self.get_note(&daily_note_id(date))
    }

    pub fn create_note(&self, title: &str, body: &str) -> DbResult<Note> {
        let title = clean_note_title(title).map_err(DatabaseError::invalid)?;
        let now = Utc::now().to_rfc3339();
        let note = Note {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            body: body.to_string(),
            daily_date: None,
            created_at: now.clone(),
            updated_at: now,
            trashed: false,
            trashed_at: None,
            authored_title: None,
        };
        self.with_transaction(|tx| {
            check_title_free(tx, &note.id, &note.title)?;
            create(tx, note)
        })
    }

    /// Saves a regular note's title and body, if the note is still the
    /// version the edit started from (`expected_updated_at`) and isn't in
    /// Trash. Otherwise
    /// nothing is written, so an edit made elsewhere in the meantime isn't
    /// silently replaced, and the current note comes back for the user to
    /// choose. Saving what is already stored writes nothing, so opening and
    /// closing a note never syncs a change.
    pub fn save_note(
        &self,
        id: &str,
        title: &str,
        body: &str,
        expected_updated_at: &str,
    ) -> DbResult<NoteSave> {
        self.with_transaction(|tx| {
            let current = get(tx, id)?;
            if !is_current(current.as_ref(), Some(expected_updated_at)) {
                return Ok(NoteSave {
                    saved: false,
                    note: current,
                    relink: None,
                });
            }
            let (note, relink) = update_in(tx, id, title, body)?;
            Ok(NoteSave {
                saved: true,
                note: Some(note),
                relink,
            })
        })
    }

    /// Saves the body of the daily note for `date`, if the day is still in
    /// the state the edit started from: the note's `updatedAt`, or `None`
    /// when the day had no note, and the note isn't in Trash. Saving an empty body for a day without a
    /// note writes nothing, so looking at a day never creates its note.
    pub fn save_daily_note_edit(
        &self,
        date: &str,
        body: &str,
        expected_updated_at: Option<&str>,
    ) -> DbResult<NoteSave> {
        if !is_daily_date(date) {
            return Err(DatabaseError::invalid("That isn't a valid date"));
        }
        self.with_transaction(|tx| {
            let current = get(tx, &daily_note_id(date))?;
            if !is_current(current.as_ref(), expected_updated_at) {
                return Ok(NoteSave {
                    saved: false,
                    note: current,
                    relink: None,
                });
            }
            Ok(NoteSave {
                saved: true,
                note: save_daily_in(tx, date, body)?,
                relink: None,
            })
        })
    }

    #[cfg(test)]
    pub fn update_note(&self, id: &str, title: &str, body: &str) -> DbResult<Note> {
        self.with_transaction(|tx| update_in(tx, id, title, body).map(|(note, _)| note))
    }

    #[cfg(test)]
    pub fn save_daily_note(&self, date: &str, body: &str) -> DbResult<Option<Note>> {
        if !is_daily_date(date) {
            return Err(DatabaseError::invalid("That isn't a valid date"));
        }
        self.with_transaction(|tx| save_daily_in(tx, date, body))
    }

    /// Moves a note to Trash. Other devices keep any edit made at the same
    /// time, because trashing changes one field rather than deleting.
    pub fn trash_note(&self, id: &str) -> DbResult<Note> {
        self.with_transaction(|tx| set_trashed(tx, id, true))
    }

    pub fn restore_note(&self, id: &str) -> DbResult<Note> {
        self.with_transaction(|tx| set_trashed(tx, id, false))
    }

    /// Deletes a note from Trash on every device, with its attachments.
    pub fn delete_note(&self, id: &str) -> DbResult<()> {
        self.with_transaction(|tx| {
            let note = get(tx, id)?.ok_or(DatabaseError::NotFound("Note"))?;
            if !note.trashed {
                return Err(DatabaseError::invalid(
                    "Move this note to Trash before deleting it",
                ));
            }
            remove(tx, id)?;
            reconcile_titles(tx)?;
            super::note_attachments::delete_note_attachments(tx, id)?;
            // Once the replica holds the note, its deletion is recorded even
            // while sync is off, so the note doesn't return when sync resumes.
            let in_replica: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type=?1 AND entity_id=?2)",
                params![EntityType::Note.as_str(), id],
                |row| row.get(0),
            )?;
            if in_replica {
                record_replicated_deletion_in_transaction(tx, EntityType::Note, id)?;
            }
            Ok(())
        })
    }

    /// Every note's id, and the title keys of notes outside Trash, which an
    /// import must not reuse.
    pub fn note_identities(
        &self,
    ) -> DbResult<(
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    )> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id, title_key, trashed FROM notes")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })?;
            let (mut ids, mut titles) = (
                std::collections::HashSet::new(),
                std::collections::HashSet::new(),
            );
            for row in rows {
                let (id, key, trashed) = row?;
                ids.insert(id);
                if !trashed {
                    titles.insert(key);
                }
            }
            Ok((ids, titles))
        })
    }

    /// Adds an imported note, with its dates, and its files (each `(id,
    /// name, declared type, bytes)`), in one transaction. Returns false,
    /// writing nothing, when a note with its id is already here. A title in
    /// use, a note too long to sync, or notes out of sync space is refused
    /// with its reason, as a new note would be.
    pub fn import_note(
        &self,
        note: Note,
        files: &[(String, String, String, Vec<u8>)],
    ) -> DbResult<bool> {
        let note = match &note.daily_date {
            Some(date) => {
                if !is_daily_date(date) || note.id != daily_note_id(date) || note.title != *date {
                    return Err(DatabaseError::invalid("Invalid daily note"));
                }
                note
            }
            None => Note {
                title: clean_note_title(&note.title).map_err(DatabaseError::invalid)?,
                ..note
            },
        };
        self.with_transaction(|tx| {
            if get(tx, &note.id)?.is_some() {
                return Ok(false);
            }
            if note.daily_date.is_none() {
                check_title_free(tx, &note.id, &note.title)?;
            }
            let note_id = note.id.clone();
            create(tx, note)?;
            for (id, name, declared_type, bytes) in files {
                super::note_attachments::insert_attachment(
                    tx,
                    id,
                    &note_id,
                    name,
                    declared_type,
                    bytes,
                )?;
            }
            Ok(true)
        })
    }

    /// Notes not in Trash, most recently opened or changed on this device
    /// first.
    pub fn list_recent_notes(&self, limit: usize) -> DbResult<Vec<NoteSummary>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT n.id, n.title, n.daily_date, substr(n.body, 1, 1000), n.updated_at, v.visited_at, n.trashed_at
                 FROM notes n LEFT JOIN note_visits v ON v.note_id=n.id
                 WHERE n.trashed=0
                 ORDER BY max(COALESCE(v.visited_at, ''), n.updated_at) DESC, n.id
                 LIMIT ?1",
            )?;
            let rows = statement.query_map([limit.min(MAX_LIST_ROWS) as i64], summary_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Notes in Trash, most recently trashed first.
    pub fn list_trashed_notes(&self, limit: usize) -> DbResult<Vec<NoteSummary>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT n.id, n.title, n.daily_date, substr(n.body, 1, 1000), n.updated_at, v.visited_at, n.trashed_at
                 FROM notes n LEFT JOIN note_visits v ON v.note_id=n.id
                 WHERE n.trashed=1
                 ORDER BY n.trashed_at DESC, n.id
                 LIMIT ?1",
            )?;
            let rows = statement.query_map([limit.min(MAX_LIST_ROWS) as i64], summary_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Days from `from` through `to` (`YYYY-MM-DD`, inclusive) whose daily
    /// note has text and is outside Trash, oldest first. For the Notes
    /// calendar's marks, so a day emptied again shows no mark.
    pub fn list_daily_note_dates(&self, from: &str, to: &str) -> DbResult<Vec<String>> {
        if !is_daily_date(from) || !is_daily_date(to) {
            return Err(DatabaseError::invalid("That isn't a valid date"));
        }
        self.with_connection(|connection| {
            // A daily note's id is its date after a fixed prefix, so the
            // primary key orders days by date.
            let mut statement = connection.prepare(
                "SELECT daily_date FROM notes
                 WHERE id BETWEEN ?1 AND ?2 AND daily_date IS NOT NULL AND trashed=0
                   AND trim(body, ' ' || char(9, 10, 13)) <> ''
                 ORDER BY id
                 LIMIT ?3",
            )?;
            let rows = statement.query_map(
                params![daily_note_id(from), daily_note_id(to), MAX_LIST_ROWS as i64],
                |row| row.get::<_, String>(0),
            )?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Remembers that the user opened a note on this device.
    pub fn record_note_visit(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO note_visits(note_id, visited_at)
                 SELECT id, ?2 FROM notes WHERE id=?1
                 ON CONFLICT(note_id) DO UPDATE SET visited_at=excluded.visited_at",
                params![id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// Notes not in Trash whose title or body matches every word of `query`.
    /// Regular notes outside Trash whose titles contain `query`, ignoring
    /// case and spacing: titles that start with it first, then the most
    /// recently opened or changed. An empty query lists recent notes. For
    /// the `[[` link picker.
    pub fn suggest_note_titles(&self, query: &str, limit: usize) -> DbResult<Vec<NoteTitle>> {
        let key = title_key(query);
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT n.id, n.title, n.daily_date
                 FROM notes n LEFT JOIN note_visits v ON v.note_id=n.id
                 WHERE n.trashed=0 AND n.daily_date IS NULL AND instr(n.title_key, ?1) > 0
                 ORDER BY instr(n.title_key, ?1) <> 1, max(COALESCE(v.visited_at, ''), n.updated_at) DESC, n.id
                 LIMIT ?2",
            )?;
            let rows = statement.query_map(params![key, limit.min(MAX_LIST_ROWS) as i64], note_title_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// The note outside Trash a `[[link]]` to `title` opens, matched by key.
    pub fn find_note_by_title(&self, title: &str) -> DbResult<Option<NoteTitle>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT id, title, daily_date FROM notes WHERE title_key=?1 AND trashed=0 ORDER BY id LIMIT 1",
                    [title_key(title)],
                    note_title_from_row,
                )
                .optional()?)
        })
    }

    pub fn search_notes(&self, query: &str, limit: usize) -> DbResult<Vec<NoteSearchHit>> {
        self.search_notes_in_scope(query, limit, NoteSearchScope::Active)
    }

    pub fn search_notes_in_scope(
        &self,
        query: &str,
        limit: usize,
        scope: NoteSearchScope,
    ) -> DbResult<Vec<NoteSearchHit>> {
        let query = fts_query(query);
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT n.id, n.title, n.daily_date,
                        snippet(note_search, -1, '\u{1}', '\u{2}', '…', 12), n.trashed
                 FROM note_search s JOIN notes n ON n.id=s.note_id
                 WHERE note_search MATCH ?1 AND (?3 IS NULL OR n.trashed=?3)
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let rows = statement.query_map(
                params![
                    query,
                    limit.min(MAX_LIST_ROWS) as i64,
                    match scope {
                        NoteSearchScope::Active => Some(false),
                        NoteSearchScope::Trash => Some(true),
                        NoteSearchScope::All => None,
                    }
                ],
                |row| {
                    Ok(NoteSearchHit {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        daily_date: row.get(2)?,
                        match_snippet: row.get(3)?,
                        trashed: row.get(4)?,
                    })
                },
            )?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// The notes that link to `id`, and the titles `id` links to that no
    /// note outside Trash has.
    pub fn note_links(&self, id: &str) -> DbResult<NoteLinks> {
        self.with_transaction(|tx| {
            let note = get(tx, id)?.ok_or(DatabaseError::NotFound("Note"))?;
            let backlinks = {
                let mut statement = tx.prepare(
                    "SELECT n.id, n.title, n.daily_date, l.context
                     FROM note_links l JOIN notes n ON n.id=l.note_id
                     WHERE l.target_key=?1 AND n.id<>?2 AND n.trashed=0
                     ORDER BY n.updated_at DESC, n.id, l.position
                     LIMIT ?3",
                )?;
                let rows = statement.query_map(
                    params![title_key(&note.title), id, MAX_LIST_ROWS as i64],
                    |row| {
                        Ok(NoteBacklink {
                            reference: None,
                            note_id: row.get(0)?,
                            title: row.get(1)?,
                            daily_date: row.get(2)?,
                            context: row.get(3)?,
                        })
                    },
                )?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            let unlinked = {
                let mut statement = tx.prepare(
                    "SELECT target_title FROM note_links l
                     WHERE l.note_id=?1 AND l.target_key<>?2
                       AND NOT EXISTS(SELECT 1 FROM notes t WHERE t.title_key=l.target_key AND t.trashed=0)
                     GROUP BY l.target_key
                     ORDER BY min(l.position)",
                )?;
                let rows = statement
                    .query_map(params![id, title_key(&note.title)], |row| row.get(0))?;
                rows.collect::<Result<Vec<String>, _>>()?
            };
            let links = {
                let mut statement = tx.prepare(
                    "SELECT target_title FROM note_links WHERE note_id=?1
                     GROUP BY target_key ORDER BY min(position)",
                )?;
                let rows = statement.query_map([id], |row| row.get(0))?;
                rows.collect::<Result<Vec<String>, _>>()?
            };
            Ok(NoteLinks {
                backlinks,
                unlinked,
                links,
            })
        })
    }

    /// How much of their snapshot budget notes use, Trash included.
    /// Every note outside Trash, daily notes by date then the rest by
    /// title, for Export to Markdown Folder.
    pub fn notes_for_export(&self) -> DbResult<Vec<Note>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {NOTE_COLUMNS} FROM notes WHERE trashed=0
                 ORDER BY daily_date IS NULL, daily_date, title_key, id"
            ))?;
            let rows = statement.query_map([], note_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn notes_storage(&self) -> DbResult<NotesStorage> {
        self.with_connection(|connection| {
            let used: i64 = connection.query_row(
                "SELECT COALESCE(SUM(sync_bytes), 0) FROM notes",
                [],
                |row| row.get(0),
            )?;
            let (attachment_count, attachment_bytes): (i64, i64) = connection.query_row(
                "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM note_attachments",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok(NotesStorage {
                used_bytes: used as u64,
                limit_bytes: MAX_SYNCED_NOTES_TOTAL_BYTES as u64,
                attachment_count: attachment_count as u64,
                attachment_bytes: attachment_bytes as u64,
            })
        })
    }
}

impl Database {
    /// Stores a note another device wrote. A note with local changes that
    /// haven't been recorded yet keeps them. Sync records those changes
    /// before it pulls (see `ReplicatedSync::sync_once`), so this only
    /// guards against a merge that reaches the note first.
    pub(crate) fn upsert_synced_note(&self, id: &str, payload: &Value) -> DbResult<()> {
        if note_field(payload, "id") != Some(id) {
            return Err(DatabaseError::invalid("Note identity mismatch"));
        }
        let title = note_field(payload, "title").unwrap_or_default();
        let body = note_field(payload, "body").unwrap_or_default();
        let daily_date = note_field(payload, "dailyDate").map(str::to_string);
        if daily_date
            .as_deref()
            .is_some_and(|date| !is_daily_date(date))
        {
            return Err(DatabaseError::invalid("Invalid daily note date"));
        }
        let trashed = payload
            .get("trashed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.with_transaction(|tx| {
            if !dirty_fields(tx, id)?.is_empty() {
                return Ok(());
            }
            let now = Utc::now().to_rfc3339();
            let existing = get(tx, id)?;
            let mut authored_existing = existing.clone();
            if let Some(note) = &mut authored_existing {
                note.authored_title = None;
                note.title = tx.query_row(
                    "SELECT authored_title FROM notes WHERE id=?1",
                    [id],
                    |row| row.get(0),
                )?;
            }
            let created_at = note_field(payload, "createdAt")
                .map(str::to_string)
                .or_else(|| existing.as_ref().map(|note| note.created_at.clone()))
                .unwrap_or_else(|| now.clone());
            let note = Note {
                id: id.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                authored_title: None,
                updated_at: match &authored_existing {
                    Some(old) if old.title == title && old.body == body => old.updated_at.clone(),
                    _ => now.clone(),
                },
                trashed_at: match &existing {
                    _ if !trashed => None,
                    Some(old) if old.trashed => old.trashed_at.clone(),
                    _ => Some(now),
                },
                daily_date,
                created_at,
                trashed,
            };
            if authored_existing.as_ref() == Some(&note) {
                return Ok(());
            }
            let bytes = payload_bytes(&sync_payload(&note))?;
            store(tx, &note, bytes, None)
        })
    }

    /// Removes a note another device deleted.
    pub(crate) fn delete_synced_note(&self, id: &str) -> DbResult<()> {
        self.with_transaction(|tx| {
            remove(tx, id)?;
            reconcile_titles(tx)
        })
    }

    /// Records the changes made to notes while sync was off.
    pub(crate) fn publish_dirty_notes(&self) -> DbResult<()> {
        self.with_transaction(|tx| {
            if !sync_recording(tx)? {
                return Ok(());
            }
            let ids = {
                let mut statement =
                    tx.prepare("SELECT id FROM notes WHERE replica_dirty_fields IS NOT NULL")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            for id in ids {
                let Some(note) = get(tx, &id)? else { continue };
                let fields = dirty_fields(tx, &id)?;
                tx.execute(
                    "UPDATE notes SET replica_dirty_fields=NULL WHERE id=?1",
                    [&id],
                )?;
                record_replicated_write_in_transaction(
                    tx,
                    EntityType::Note,
                    &id,
                    &fields,
                    &write_payload(tx, &note, &BTreeSet::new())?,
                )?;
            }
            Ok(())
        })
    }

    /// Every note's synchronized fields, for the backlog sweep that records
    /// any note the replica has never seen.
    pub(crate) fn note_sync_payloads(&self) -> DbResult<Vec<(String, Value)>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare(&format!("SELECT {NOTE_COLUMNS} FROM notes ORDER BY id"))?;
            let rows = statement.query_map([], |row| {
                let mut note = note_from_row(row)?;
                note.title = row.get(8)?;
                note.authored_title = None;
                Ok(note)
            })?;
            let notes = rows.collect::<Result<Vec<_>, _>>()?;
            Ok(notes
                .into_iter()
                .map(|note| (note.id.clone(), sync_payload(&note)))
                .collect())
        })
    }
}

#[cfg(test)]
#[path = "tests/notes.rs"]
mod tests;
