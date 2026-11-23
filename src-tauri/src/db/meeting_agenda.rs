//! Guarded insertion of accepted meeting agenda items into a note, with
//! operation receipts and Undo mirroring `db/meeting_follow_through.rs`.
use super::{receipts, Database, DatabaseError, DbResult};
use crate::{
    meeting_excerpt::{self, MeetingBlockKind},
    meeting_policy as policy,
    models::Note,
    note_entities::{self, NoteReference},
    note_meetings::{self, MeetingEntry},
};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};

const RECEIPT_KIND: &str = "meetingAgenda";

fn conflicted(tx: &Transaction<'_>, id: &str) -> DbResult<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=?1 GROUP BY field HAVING count(DISTINCT value)>1)",
        [id],
        |r| r.get(0),
    )?)
}

/// Result of inserting accepted agenda bullets into a note.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingAgendaSave {
    pub operation_id: String,
    pub note: Note,
    pub inserted: String,
    pub start: usize,
    pub end: usize,
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingAgendaUndo {
    pub note: Note,
}

fn clean_agenda_items(items: &[String]) -> DbResult<Vec<String>> {
    if items.is_empty() {
        return Err(DatabaseError::invalid(
            "Select at least one agenda item to add to the note.",
        ));
    }
    if items.len() > policy::MAX_AGENDA_ITEMS {
        return Err(DatabaseError::invalid(format!(
            "At most {} agenda items can be added at once.",
            policy::MAX_AGENDA_ITEMS
        )));
    }
    let mut cleaned = Vec::with_capacity(items.len());
    for raw in items {
        let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            continue;
        }
        if collapsed.chars().count() > policy::MAX_AGENDA_ITEM_CHARS {
            return Err(DatabaseError::invalid(format!(
                "Each agenda item must be at most {} characters.",
                policy::MAX_AGENDA_ITEM_CHARS
            )));
        }
        cleaned.push(note_meetings::escape_inline(&collapsed));
    }
    if cleaned.is_empty() {
        return Err(DatabaseError::invalid(
            "Select at least one non-empty agenda item to add to the note.",
        ));
    }
    Ok(cleaned)
}

fn insert_under_occurrence(
    body: &str,
    occurrence: &NoteReference,
    items: &[String],
) -> DbResult<Option<(String, String, usize, usize)>> {
    let blocks = meeting_excerpt::occurrence_blocks("agenda", body, occurrence)
        .map_err(DatabaseError::invalid)?;
    let Some(block) = blocks
        .into_iter()
        .find(|b| b.kind == MeetingBlockKind::Excerpt)
    else {
        return Ok(None);
    };
    let line = &body[block.start..block.end];
    let first_line = line.lines().next().unwrap_or("");
    let indent_prefix: String = first_line
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    let child_indent = format!("{indent_prefix}  ");
    let mut inserted = String::new();
    if !body[..block.end].ends_with('\n') {
        inserted.push('\n');
    }
    for item in items {
        inserted.push_str(&child_indent);
        inserted.push_str("- ");
        inserted.push_str(item);
        inserted.push('\n');
    }
    let start = block.end;
    let end = start + inserted.len();
    let mut next = String::with_capacity(body.len() + inserted.len());
    next.push_str(&body[..start]);
    next.push_str(&inserted);
    next.push_str(&body[start..]);
    Ok(Some((next, inserted, start, end)))
}

fn append_agenda_section(body: &str, items: &[String]) -> (String, String, usize, usize) {
    let mut inserted = String::new();
    if !body.is_empty() {
        if !body.ends_with('\n') {
            inserted.push('\n');
        }
        if !body.ends_with("\n\n") {
            inserted.push('\n');
        }
    }
    inserted.push_str("## Agenda\n\n");
    for item in items {
        inserted.push_str("- ");
        inserted.push_str(item);
        inserted.push('\n');
    }
    let start = body.len();
    let end = start + inserted.len();
    let mut next = String::with_capacity(body.len() + inserted.len());
    next.push_str(body);
    next.push_str(&inserted);
    (next, inserted, start, end)
}

impl Database {
    /// Appends accepted agenda items under the meeting's line in `note_id`
    /// (or adds the meeting line to `date`'s daily note first when no note exists).
    /// Idempotent per `operation_id` and guarded by `expected_revision`.
    pub fn save_meeting_agenda(
        &self,
        reference: &NoteReference,
        note_id: Option<&str>,
        date: &str,
        expected_revision: Option<&str>,
        operation_id: &str,
        entry: &MeetingEntry,
        items: &[String],
    ) -> DbResult<MeetingAgendaSave> {
        if reference.kind != "event" || !reference.validate() {
            return Err(DatabaseError::invalid("Invalid event reference"));
        }
        receipts::checked_operation_id(operation_id)?;
        let cleaned = clean_agenda_items(items)?;
        let scope = reference.target().map_err(DatabaseError::invalid)?;
        self.with_transaction(|tx| {
            if let Some(record) = receipts::matching(tx, operation_id, RECEIPT_KIND, &scope)? {
                let mut replayed: MeetingAgendaSave =
                    serde_json::from_str(&record.result).map_err(super::serialization_error)?;
                replayed.replayed = true;
                return Ok(replayed);
            }

            let target_id = note_id
                .map(str::to_string)
                .unwrap_or_else(|| crate::notes::daily_note_id(date));
            let existing = super::notes::get(tx, &target_id)?;
            if let Some(note) = &existing {
                if note.trashed {
                    return Err(DatabaseError::invalid(
                        "That note is in Trash. Restore it or choose another note.",
                    ));
                }
            }
            if existing.as_ref().map(|n| n.updated_at.as_str()) != expected_revision {
                return Err(DatabaseError::invalid(
                    "That note changed since you started preparing. Refresh and try again.",
                ));
            }
            if conflicted(tx, &target_id)? {
                return Err(DatabaseError::invalid(
                    "Resolve that note's conflict before adding an agenda to it.",
                ));
            }
            let current_body = existing.as_ref().map(|n| n.body.as_str()).unwrap_or("");
            let is_daily = existing
                .as_ref()
                .map(|n| n.daily_date.is_some())
                .unwrap_or_else(|| target_id == crate::notes::daily_note_id(date));

            let (next_body, inserted, start, end) = if let Some(found) =
                insert_under_occurrence(current_body, reference, &cleaned)?
            {
                found
            } else if is_daily {
                let line = note_meetings::meeting_line(entry, None)
                    .map_err(DatabaseError::invalid)?;
                let with_line = note_meetings::insert_meeting_line(current_body, &line).map_err(
                    |_| {
                        DatabaseError::invalid(
                            "That day's note has an unclosed code block where the meeting would go.",
                        )
                    },
                )?;
                let (final_body, _, _, _) =
                    insert_under_occurrence(&with_line, reference, &cleaned)?.ok_or_else(|| {
                        DatabaseError::invalid("Could not locate the added meeting line")
                    })?;
                // Track exact diff against `current_body` for clean Undo.
                let prefix_len = current_body
                    .bytes()
                    .zip(final_body.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                let suffix_len = current_body[prefix_len..]
                    .bytes()
                    .rev()
                    .zip(final_body[prefix_len..].bytes().rev())
                    .take_while(|(a, b)| a == b)
                    .count();
                let start = prefix_len;
                let end = final_body.len() - suffix_len;
                let inserted = final_body[start..end].to_string();
                (final_body, inserted, start, end)
            } else {
                append_agenda_section(current_body, &cleaned)
            };

            let saved = if is_daily {
                let day = existing
                    .as_ref()
                    .and_then(|n| n.daily_date.as_deref())
                    .unwrap_or(date);
                super::notes::save_daily_in(tx, day, &next_body)?
                    .ok_or(DatabaseError::NotFound("Note"))?
            } else {
                let note = existing.ok_or(DatabaseError::NotFound("Note"))?;
                let title = note.title.clone();
                super::notes::edit(tx, note, title, &next_body)?
            };

            let result = MeetingAgendaSave {
                operation_id: operation_id.to_string(),
                note: saved,
                inserted,
                start,
                end,
                replayed: false,
            };
            receipts::record(
                tx,
                operation_id,
                RECEIPT_KIND,
                &scope,
                Some(&result.note.id),
                &serde_json::to_string(&result).map_err(super::serialization_error)?,
            )?;
            Ok(result)
        })
    }

    /// Removes the exact range inserted by `save_meeting_agenda`, only while
    /// the note is still at the revision that save produced.
    pub fn undo_meeting_agenda(&self, operation_id: &str) -> DbResult<MeetingAgendaUndo> {
        receipts::checked_operation_id(operation_id)?;
        self.with_transaction(|tx| {
            let record = receipts::find(tx, operation_id)?
                .filter(|r| r.kind == RECEIPT_KIND)
                .ok_or_else(|| {
                    DatabaseError::invalid("That agenda addition can no longer be undone.")
                })?;
            let saved: MeetingAgendaSave =
                serde_json::from_str(&record.result).map_err(super::serialization_error)?;
            let note = super::notes::get(tx, &saved.note.id)?
                .ok_or_else(|| DatabaseError::invalid("That note no longer exists."))?;
            if record.undone_at.is_some() {
                return Ok(MeetingAgendaUndo { note });
            }
            if note.trashed || note.updated_at != saved.note.updated_at || conflicted(tx, &note.id)?
            {
                return Err(DatabaseError::invalid(
                    "That note changed after the agenda was added, so it can't be undone automatically.",
                ));
            }
            if saved.end > note.body.len()
                || note.body.get(saved.start..saved.end) != Some(saved.inserted.as_str())
            {
                return Err(DatabaseError::invalid(
                    "That note changed after the agenda was added, so it can't be undone automatically.",
                ));
            }
            let restored_body = format!("{}{}", &note.body[..saved.start], &note.body[saved.end..]);
            let restored = if let Some(day) = note.daily_date.as_deref() {
                super::notes::save_daily_in(tx, day, &restored_body)?.unwrap_or_else(|| Note {
                    body: restored_body.clone(),
                    ..note.clone()
                })
            } else {
                let title = note.title.clone();
                super::notes::edit(tx, note, title, &restored_body)?
            };
            receipts::mark_undone(tx, operation_id)?;
            let _ = note_entities::parse(&restored.id, &restored.body);
            Ok(MeetingAgendaUndo { note: restored })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_meetings::MeetingEntry;

    fn occurrence(id: &str) -> NoteReference {
        NoteReference {
            version: 1,
            kind: "event".into(),
            id: id.into(),
            account: Some("me@example.com".into()),
            calendar: Some("work".into()),
            note: None,
            contact: None,
            series: None,
        }
    }

    fn entry(id: &str) -> MeetingEntry {
        MeetingEntry {
            event: occurrence(id),
            time: "2:00pm".into(),
            attendees: vec![],
            name: "Marketing Power Hour".into(),
            link_note: false,
        }
    }

    #[test]
    fn saves_agenda_under_meeting_line_replays_idempotently_and_undoes_cleanly() {
        let db = Database::open_memory();
        let op = "22222222-2222-4222-8222-222222222222";
        let saved = db
            .save_meeting_agenda(
                &occurrence("evt-1"),
                None,
                "2026-11-23",
                None,
                op,
                &entry("evt-1"),
                &[
                    "Approve Q4 creative".into(),
                    "Confirm conversion metrics".into(),
                ],
            )
            .unwrap();
        assert!(!saved.replayed);
        assert!(saved.note.body.contains("Marketing Power Hour"));
        assert!(saved.note.body.contains("  - Approve Q4 creative\n"));
        assert!(saved.note.body.contains("  - Confirm conversion metrics\n"));

        // Replaying with the same operation id returns the first result without writing again.
        let replay = db
            .save_meeting_agenda(
                &occurrence("evt-1"),
                None,
                "2026-11-23",
                None,
                op,
                &entry("evt-1"),
                &["Approve Q4 creative".into()],
            )
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.note.body, saved.note.body);

        // Undo removes the inserted range cleanly.
        let undone = db.undo_meeting_agenda(op).unwrap();
        assert_eq!(undone.note.body, "");
    }
}
