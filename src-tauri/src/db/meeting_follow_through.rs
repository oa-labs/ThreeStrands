//! Saving a meeting's reviewed follow-through into a note, and undoing it.
//! One guarded transaction checks the note's version, Trash and conflict
//! state, that the note is about this occurrence, the linked tasks, the
//! Markdown, and the note's size and sync budgets; then it appends the
//! section and writes a receipt. All accepted items save together or none
//! do, and no other text in the note changes.
use super::{receipts, Database, DatabaseError, DbResult};
use crate::{
    meeting_follow_through::{self as format, FollowThroughItem},
    models::Note,
    note_entities::{self, NoteReference},
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

const RECEIPT_KIND: &str = "meetingFollowThrough";

/// The note the follow-through goes into.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FollowThroughDestination {
    pub note_id: String,
}

/// What the user accepted in the review.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptedFollowThrough {
    /// The day of the review, written on the section's line.
    pub date: String,
    /// The meeting's name, used where the section has to link the meeting.
    pub meeting: String,
    #[serde(default)]
    pub summary: Option<String>,
    pub items: Vec<FollowThroughItem>,
    #[serde(default)]
    pub recording_id: Option<String>,
    #[serde(default)]
    pub attach_transcript: bool,
}

/// A saved follow-through, as first committed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowThroughSave {
    pub operation_id: String,
    /// The note as saved.
    pub note: Note,
    /// The inserted text, and its UTF-8 byte range in `note.body`.
    pub section: String,
    pub start: usize,
    pub end: usize,
    /// New round tasks, by their Tasks ids (`note:<note>:<marker>`).
    pub task_ids: Vec<String>,
    /// New checklist items, by the same kind of id.
    pub checklist_ids: Vec<String>,
    #[serde(default)]
    pub attachment_id: Option<String>,
    #[serde(default)]
    pub recording_id: Option<String>,
    /// True when this came from the receipt of an earlier attempt and
    /// nothing was written now.
    #[serde(default)]
    pub replayed: bool,
}

/// The note after its follow-through was taken out.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowThroughUndo {
    pub note: Note,
}

fn conflicted(tx: &Transaction<'_>, id: &str) -> DbResult<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=?1 GROUP BY field HAVING count(DISTINCT value)>1)",
        [id],
        |r| r.get(0),
    )?)
}

/// A linked task with the title it has now, or an error when it is gone.
fn resolve_links(
    tx: &Transaction<'_>,
    items: &[FollowThroughItem],
) -> DbResult<Vec<FollowThroughItem>> {
    items
        .iter()
        .map(|item| match item {
            FollowThroughItem::Link { task, .. } => {
                if task.kind != "task" || !task.validate() {
                    return Err(DatabaseError::invalid(
                        "Only a task can be linked as an action.",
                    ));
                }
                let title: Option<String> = match &task.note {
                    Some(note) => tx
                        .query_row(
                            "SELECT t.title FROM note_tasks t JOIN notes n ON n.id=t.note_id
                             WHERE t.note_id=?1 AND t.marker_id=?2 AND n.trashed=0",
                            params![note, task.id],
                            |r| r.get(0),
                        )
                        .optional()?,
                    None => tx
                        .query_row("SELECT title FROM tasks WHERE id=?1", [&task.id], |r| {
                            r.get(0)
                        })
                        .optional()?,
                };
                let title = title.ok_or_else(|| {
                    DatabaseError::invalid(
                        "A linked task no longer exists. Remove it and try again.",
                    )
                })?;
                Ok(FollowThroughItem::Link {
                    task: task.clone(),
                    title,
                })
            }
            other => Ok(other.clone()),
        })
        .collect()
}

fn attach_transcript_in_tx(
    tx: &Transaction<'_>,
    note_id: &str,
    date: &str,
    meeting_title: &str,
    recording_id: &str,
) -> DbResult<(String, String, String)> {
    use crate::db::recordings::{
        format_transcript_markdown, transcript_attachment_filename, TranscriptRow,
    };
    use crate::recording::capture::Track;

    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM meeting_recordings WHERE id=?1)",
        [recording_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(DatabaseError::NotFound("Recording"));
    }
    let speaker_labels: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM meeting_recording_tracks WHERE recording_id=?1 AND track='system' AND first_frame_offset_ms IS NOT NULL)",
        [recording_id],
        |r| r.get(0),
    )?;
    let rows: Vec<TranscriptRow> = {
        let mut statement = tx.prepare(
            "SELECT track,start_ms,end_ms,text,rms,dropped_as_echo
             FROM meeting_recording_segments WHERE recording_id=?1 ORDER BY seq",
        )?;
        let mapped = statement.query_map([recording_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in mapped {
            let (track, start_ms, end_ms, text, rms, dropped_as_echo) = row?;
            out.push(TranscriptRow {
                track: Track::parse(&track)
                    .ok_or_else(|| DatabaseError::corrupt(format!("Unknown track {track}")))?,
                start_ms,
                end_ms,
                text,
                rms: rms.map(|v| v as f32),
                dropped_as_echo,
            });
        }
        out
    };
    let filename = transcript_attachment_filename(meeting_title, date);
    let markdown = format_transcript_markdown(meeting_title, date, speaker_labels, &rows);
    let att_id = crate::replicated_sync::encode_id(&crate::replicated_sync::random_id());
    super::note_attachments::insert_attachment(
        tx,
        &att_id,
        note_id,
        &filename,
        "text/markdown",
        markdown.as_bytes(),
    )?;
    let href = format!("{}{att_id}", super::note_attachments::ATTACHMENT_SCHEME);
    Ok((att_id, filename, href))
}

fn mark_recording_reviewed(
    tx: &Transaction<'_>,
    recording_id: Option<&str>,
    operation_id: &str,
) -> DbResult<()> {
    if let Some(recording_id) = recording_id {
        let changed = tx.execute(
            "UPDATE meeting_recordings SET reviewed_operation_id=?2 WHERE id=?1",
            params![recording_id, operation_id],
        )?;
        if changed == 0 {
            return Err(DatabaseError::NotFound("Recording"));
        }
    }
    Ok(())
}

fn collect_new_items(
    saved: &Note,
    start: usize,
    end: usize,
) -> DbResult<(Vec<String>, Vec<String>)> {
    let parsed = note_entities::parse(&saved.id, &saved.body).map_err(DatabaseError::invalid)?;
    let (mut task_ids, mut checklist_ids) = (Vec::new(), Vec::new());
    for item in parsed
        .items
        .iter()
        .filter(|item| start <= item.start && item.start < end)
    {
        let id = format!("note:{}:{}", saved.id, item.id);
        if item.task {
            task_ids.push(id);
        } else {
            checklist_ids.push(id);
        }
    }
    Ok((task_ids, checklist_ids))
}

impl Database {
    /// Appends the accepted decisions and actions to `destination` as one
    /// follow-through section for `reference`, if the note is still at
    /// `expected_revision`. Retrying with the same `operation_id` returns
    /// the first result without writing again.
    pub fn save_meeting_follow_through(
        &self,
        reference: &NoteReference,
        destination: &FollowThroughDestination,
        expected_revision: &str,
        operation_id: &str,
        accepted: &AcceptedFollowThrough,
    ) -> DbResult<FollowThroughSave> {
        if reference.kind != "event" || !reference.validate() {
            return Err(DatabaseError::invalid("Invalid event reference"));
        }
        receipts::checked_operation_id(operation_id)?;
        let scope = reference.target().map_err(DatabaseError::invalid)?;
        self.with_transaction(|tx| {
            if let Some(receipt) = receipts::matching(tx, operation_id, RECEIPT_KIND, &scope)? {
                let mut saved: FollowThroughSave =
                    serde_json::from_str(&receipt.result).map_err(super::serialization_error)?;
                saved.replayed = true;
                return Ok(saved);
            }
            let note = super::notes::get(tx, &destination.note_id)?
                .ok_or(DatabaseError::NotFound("Note"))?;
            if note.trashed {
                return Err(DatabaseError::invalid(
                    "That note is in Trash. Restore it or choose another note.",
                ));
            }
            if note.updated_at != expected_revision {
                return Err(DatabaseError::invalid(
                    "That note changed since you started this review. Check the destination and save again.",
                ));
            }
            if conflicted(tx, &note.id)? {
                return Err(DatabaseError::invalid(
                    "Resolve that note's conflict before saving follow-through to it.",
                ));
            }
            if !super::meetings::occurrence_note_ids(tx, reference)?.contains(&note.id) {
                return Err(DatabaseError::invalid(
                    "That note isn't about this meeting. Choose one of its notes, or add a note first.",
                ));
            }
            let items = resolve_links(tx, &accepted.items)?;
            let attached = if accepted.attach_transcript {
                let rec_id = accepted.recording_id.as_deref().ok_or_else(|| {
                    DatabaseError::invalid("Choose a recording to attach its transcript.")
                })?;
                Some(attach_transcript_in_tx(
                    tx,
                    &note.id,
                    &accepted.date,
                    &accepted.meeting,
                    rec_id,
                )?)
            } else {
                None
            };
            let transcript_arg = attached
                .as_ref()
                .map(|(_, filename, href)| (filename.as_str(), href.as_str()));
            // Pin existing tasks' ids first, so only the new ones change below.
            let previous =
                note_entities::normalize(&note.id, &note.body).map_err(DatabaseError::invalid)?;
            let inserted = format::insert_with_extras(
                &note.id,
                &previous,
                reference,
                &accepted.meeting,
                &accepted.date,
                accepted.summary.as_deref(),
                &items,
                transcript_arg,
            )
            .map_err(DatabaseError::invalid)?;
            let next = note_entities::normalize(&note.id, &inserted.body)
                .map_err(DatabaseError::invalid)?;
            let start = inserted.start;
            // Normalizing adds task anchors only inside the new text.
            let end = next.len() - (previous.len() - start);
            if end < start
                || next.get(..start) != previous.get(..start)
                || next.get(end..) != previous.get(start..)
            {
                return Err(DatabaseError::invalid(
                    "This note's layout changed. Open it before saving follow-through.",
                ));
            }
            let title = note.title.clone();
            let saved = super::notes::edit(tx, note, title, &next)?;
            if saved.body != next {
                return Err(DatabaseError::invalid(
                    "This note's layout changed. Open it before saving follow-through.",
                ));
            }
            mark_recording_reviewed(tx, accepted.recording_id.as_deref(), operation_id)?;
            let (task_ids, checklist_ids) = collect_new_items(&saved, start, end)?;
            let result = FollowThroughSave {
                operation_id: operation_id.to_string(),
                section: saved.body[start..end].to_string(),
                note: saved,
                start,
                end,
                task_ids,
                checklist_ids,
                attachment_id: attached.map(|(id, _, _)| id),
                recording_id: accepted.recording_id.clone(),
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

    /// Saves an ad-hoc recording's follow-through section under `## Meetings` in `date`'s daily note.
    pub fn save_adhoc_follow_through(
        &self,
        date: &str,
        time_label: &str,
        expected_revision: Option<&str>,
        operation_id: &str,
        accepted: &AcceptedFollowThrough,
    ) -> DbResult<FollowThroughSave> {
        if !crate::notes::is_daily_date(date) {
            return Err(DatabaseError::invalid("That isn't a valid date"));
        }
        receipts::checked_operation_id(operation_id)?;
        let target_id = crate::notes::daily_note_id(date);
        let scope = format!("adhoc:{date}");
        self.with_transaction(|tx| {
            if let Some(receipt) = receipts::matching(tx, operation_id, RECEIPT_KIND, &scope)? {
                let mut saved: FollowThroughSave =
                    serde_json::from_str(&receipt.result).map_err(super::serialization_error)?;
                saved.replayed = true;
                return Ok(saved);
            }
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
                    "That note changed since you started this review. Check the destination and save again.",
                ));
            }
            if conflicted(tx, &target_id)? {
                return Err(DatabaseError::invalid(
                    "Resolve that note's conflict before saving follow-through to it.",
                ));
            }
            let items = resolve_links(tx, &accepted.items)?;
            let attached = if accepted.attach_transcript {
                let rec_id = accepted.recording_id.as_deref().ok_or_else(|| {
                    DatabaseError::invalid("Choose a recording to attach its transcript.")
                })?;
                Some(attach_transcript_in_tx(
                    tx,
                    &target_id,
                    &accepted.date,
                    &accepted.meeting,
                    rec_id,
                )?)
            } else {
                None
            };
            let transcript_arg = attached
                .as_ref()
                .map(|(_, filename, href)| (filename.as_str(), href.as_str()));
            let raw_body = existing.as_ref().map(|n| n.body.as_str()).unwrap_or("");
            let previous =
                note_entities::normalize(&target_id, raw_body).map_err(DatabaseError::invalid)?;
            let inserted = format::insert_adhoc(
                &target_id,
                &previous,
                time_label,
                &accepted.meeting,
                &accepted.date,
                accepted.summary.as_deref(),
                &items,
                transcript_arg,
            )
            .map_err(DatabaseError::invalid)?;
            let next = note_entities::normalize(&target_id, &inserted.body)
                .map_err(DatabaseError::invalid)?;
            let start = inserted.start;
            let end = next.len() - (previous.len() - start);
            if end < start
                || next.get(..start) != previous.get(..start)
                || next.get(end..) != previous.get(start..)
            {
                return Err(DatabaseError::invalid(
                    "This note's layout changed. Open it before saving follow-through.",
                ));
            }
            let saved = super::notes::save_daily_in(tx, date, &next)?
                .ok_or(DatabaseError::NotFound("Note"))?;
            if saved.body != next {
                return Err(DatabaseError::invalid(
                    "This note's layout changed. Open it before saving follow-through.",
                ));
            }
            mark_recording_reviewed(tx, accepted.recording_id.as_deref(), operation_id)?;
            let (task_ids, checklist_ids) = collect_new_items(&saved, start, end)?;
            let result = FollowThroughSave {
                operation_id: operation_id.to_string(),
                section: saved.body[start..end].to_string(),
                note: saved,
                start,
                end,
                task_ids,
                checklist_ids,
                attachment_id: attached.map(|(id, _, _)| id),
                recording_id: accepted.recording_id.clone(),
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

    /// Takes out exactly the section `operation_id` saved. Refuses once the
    /// note changed after that save, including a task in the section being
    /// completed, so it never removes newer writing or a changed action.
    pub fn undo_meeting_follow_through(&self, operation_id: &str) -> DbResult<FollowThroughUndo> {
        receipts::checked_operation_id(operation_id)?;
        self.with_transaction(|tx| {
            let receipt = receipts::find(tx, operation_id)?
                .filter(|receipt| receipt.kind == RECEIPT_KIND)
                .ok_or_else(|| {
                    DatabaseError::invalid("This follow-through can no longer be undone here.")
                })?;
            let saved: FollowThroughSave =
                serde_json::from_str(&receipt.result).map_err(super::serialization_error)?;
            let note = super::notes::get(tx, &saved.note.id)?
                .ok_or(DatabaseError::NotFound("Note"))?;
            if receipt.undone_at.is_some() {
                return Ok(FollowThroughUndo { note });
            }
            const CHANGED: &str = "That note changed after the follow-through was saved. Open it to change the follow-through.";
            if note.trashed
                || note.updated_at != saved.note.updated_at
                || note.body.get(saved.start..saved.end) != Some(saved.section.as_str())
            {
                return Err(DatabaseError::invalid(CHANGED));
            }
            if conflicted(tx, &note.id)? {
                return Err(DatabaseError::invalid(CHANGED));
            }
            let mut body = note.body.clone();
            body.replace_range(saved.start..saved.end, "");
            let title = note.title.clone();
            let note = super::notes::edit(tx, note, title, &body)?;
            if let Some(attachment_id) = saved.attachment_id.as_ref() {
                super::note_attachments::delete_attachments(
                    tx,
                    &note.id,
                    std::slice::from_ref(attachment_id),
                )?;
            }
            if let Some(recording_id) = saved.recording_id.as_deref() {
                tx.execute(
                    "UPDATE meeting_recordings SET reviewed_operation_id=NULL WHERE id=?1 AND reviewed_operation_id=?2",
                    params![recording_id, operation_id],
                )?;
            }
            receipts::mark_undone(tx, operation_id)?;
            Ok(FollowThroughUndo { note })
        })
    }
}

#[cfg(test)]
#[path = "tests/meeting_follow_through.rs"]
mod tests;
